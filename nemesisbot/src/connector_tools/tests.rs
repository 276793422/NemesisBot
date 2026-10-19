//! connector_tools 单测（独立测试文件——内联测试禁令）。
//!
//! 覆盖：op_slug_for 投影矩阵 / connectors_config 缺文件回落默认 /
//! connector_find 列举与损坏入列 / operation_override 投影与 fail-closed /
//! execute action 臂（paper_order happy+缺价 / 预算 fail-closed / disabled /
//! positions / cancel 诚实报错）/ 未知名无回执。

use super::*;
use nemesis_connectors::manifest::{OpClass, RiskLevel};
use nemesis_connectors::receipt::list_receipts;

fn ctx() -> RequestContext {
    RequestContext::new("web", "chat-1", "user-1", "session-1")
}

fn write_connector(ws: &Path, id: &str, toml_text: &str) {
    let dir = ws.join("connectors").join(id);
    std::fs::create_dir_all(&dir).expect("create connector dir");
    std::fs::write(dir.join("connector.toml"), toml_text).expect("write manifest");
}

const MIXED_MANIFEST: &str = r#"
[connector]
id = "paper-broker"
name = "纸面券商"
version = "0.1.0"
kind = "rest"
description = "零账号纸面下单 demo"

[connector.http]
base = "https://quotes.example.com"

[[op]]
id = "quote"
intent = ["查询行情"]
risk = "low"
class = "network"
http = { method = "GET", path = "/quote?symbol={symbol}" }

[[op]]
id = "place_order"
intent = ["买入", "卖出"]
risk = "critical"
class = "write"
confirm = "required"
budget = "finance"
receipt = "order_id"
action = "builtin.paper_order"
args_schema = { type = "object", required = ["side", "symbol", "qty", "price"] }

[[op]]
id = "positions"
intent = ["查持仓"]
risk = "low"
class = "read"
action = "builtin.paper_positions"

[[op]]
id = "cancel_order"
intent = ["撤单"]
risk = "medium"
class = "write"
action = "builtin.paper_cancel"
"#;

/// 显式限额 config（paper_order happy path 用）。
fn write_finance_config(path: &Path) {
    std::fs::write(
        path,
        r#"{
  "connectors": {
    "enabled": true,
    "max_ops_per_hour": 10,
    "finance": { "max_order_qty": 100.0, "max_orders_per_day": 5 }
  }
}"#,
    )
    .expect("write config");
}

fn invoke_tool(ws: &Path, config: &Path) -> ConnectorInvokeTool {
    ConnectorInvokeTool::new(
        ws.to_path_buf(),
        config.to_path_buf(),
        #[cfg(feature = "security")]
        None,
    )
}

#[tokio::test]
async fn test_op_slug_for_danger_escalation_matrix() {
    use RiskLevel::{Critical, High, Low, Medium};
    // 静态表正常位
    assert_eq!(op_slug_for(Low, OpClass::Read), "file_read");
    assert_eq!(op_slug_for(Low, OpClass::Network), "network_request");
    assert_eq!(op_slug_for(Medium, OpClass::Network), "network_request");
    assert_eq!(op_slug_for(High, OpClass::Write), "file_write");
    assert_eq!(op_slug_for(Critical, OpClass::Exec), "process_exec");
    // 危险度取高者：high/critical 的 read 也投影到写/exec 档
    assert_eq!(op_slug_for(High, OpClass::Read), "file_write");
    assert_eq!(op_slug_for(Critical, OpClass::Read), "process_exec");
    assert_eq!(op_slug_for(Critical, OpClass::Write), "process_exec");
    assert_eq!(op_slug_for(Medium, OpClass::Read), "registry_read");
}

#[test]
fn test_connectors_config_missing_file_falls_back_to_default() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = connectors_config(&tmp.path().join("nope.json"));
    assert!(cfg.enabled); // 工具面默认开
    assert_eq!(cfg.max_ops_per_hour, 0); // 限频关
    assert!(cfg.finance.is_none()); // 资金 fail-closed（全 0）
}

/// ConnectorFindTool 随 security 闸（唯一消费者 = agent_factory 注册块）——
/// 测试同闸；无 security 构建该类型整体缺席。
#[cfg(feature = "security")]
#[tokio::test]
async fn test_find_lists_connectors_with_ops_and_errors() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(ws, "paper-broker", MIXED_MANIFEST);
    // 损坏目录（缺 manifest）
    std::fs::create_dir_all(ws.join("connectors").join("broken")).expect("broken dir");

    let tool = ConnectorFindTool::new(ws.to_path_buf());
    let out = tool.execute("{}", &ctx()).await.expect("find ok");
    assert!(out.contains("\"paper-broker\""), "lists connector: {out}");
    assert!(out.contains("place_order"), "lists ops: {out}");
    assert!(
        out.contains("\"confirm_required\": true"),
        "op metadata: {out}"
    );
    assert!(out.contains("broken"), "损坏目录诚实入列: {out}");

    // 词元过滤
    let filtered = tool
        .execute(r#"{"query": "行情"}"#, &ctx())
        .await
        .expect("filtered find ok");
    assert!(filtered.contains("quote"), "intent 命中: {filtered}");
}

#[tokio::test]
async fn test_operation_override_projects_and_fail_closes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(ws, "paper-broker", MIXED_MANIFEST);
    let tool = invoke_tool(ws, &ws.join("config.json"));

    // confirm_required op → Some + force_approval + receipt_ctx
    let o = tool
        .operation_override(r#"{"connector":"paper-broker","op":"place_order","args":{"qty":10}}"#)
        .expect("override projected");
    assert_eq!(o.op_type, "process_exec"); // (critical, write)：rank = max(2,3) = 3 → exec 档
    assert!(o.force_approval.is_some(), "confirm=required 必强审批");
    let ctx_json = o.receipt_ctx.expect("receipt ctx present");
    assert!(ctx_json.contains("\"connector\":\"paper-broker\""));
    assert!(ctx_json.contains("\"risk\":\"critical\""));

    // 未知 connector / op → None（静态 fail-closed 兜底）
    assert!(
        tool.operation_override(r#"{"connector":"ghost","op":"x"}"#)
            .is_none()
    );
    assert!(
        tool.operation_override(r#"{"connector":"paper-broker","op":"ghost"}"#)
            .is_none(),
        "未知 op 必须 None"
    );
}

#[tokio::test]
async fn test_paper_order_happy_path_writes_ledger_and_receipt() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(ws, "paper-broker", MIXED_MANIFEST);
    let config = ws.join("config.json");
    write_finance_config(&config);
    let tool = invoke_tool(ws, &config);

    let out = tool
        .execute(
            r#"{"connector":"paper-broker","op":"place_order","args":{"side":"buy","symbol":"AAPL","qty":10,"price":123.45}}"#,
            &ctx(),
        )
        .await
        .expect("order ok");
    assert!(out.contains("\"executed\": true"), "result: {out}");
    assert!(out.contains("NB-ORD-1"), "order id: {out}");

    // 账本落盘（内容断言，非存在性）
    let ledger = nemesis_connectors::ledger::load_ledger(ws).expect("ledger");
    assert_eq!(ledger.orders.len(), 1);
    assert_eq!(ledger.cash, 1_000_000.0 - 1234.5);

    // 回执 executed + 主键
    let receipts = list_receipts(ws, 50, None).expect("receipts");
    assert_eq!(receipts.len(), 1, "exactly one receipt");
    let r = &receipts[0];
    assert_eq!(r.outcome, "executed");
    assert_eq!(r.receipt_key.as_deref(), Some("order_id"));
    assert_eq!(r.receipt_value.as_deref(), Some("NB-ORD-1"));
    assert_eq!(r.connector, "paper-broker");
    assert_eq!(r.risk, "critical");
    assert_eq!(r.args_digest.len(), 16);
    assert_eq!(r.budget_consumed.as_ref().map(|b| b.ops_last_hour), Some(1));
}

#[tokio::test]
async fn test_paper_order_without_price_rejected_by_schema_gate() {
    // R4：place_order args_schema required 含 price → 缺价在出网前的
    // schema 闸被拒，不触达 builtin（quote 引导自查保留给直连路径），
    // 无回执（动作从未发生）、账本零变动。
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(ws, "paper-broker", MIXED_MANIFEST);
    let config = ws.join("config.json");
    write_finance_config(&config);
    let tool = invoke_tool(ws, &config);

    let err = tool
        .execute(
            r#"{"connector":"paper-broker","op":"place_order","args":{"side":"buy","symbol":"AAPL","qty":10}}"#,
            &ctx(),
        )
        .await
        .expect_err("missing price must error");
    assert!(err.contains("⛔ INVALID ARGS"), "msg: {err}");
    assert!(err.contains("price"), "{err}");
    // schema 拒绝无回执（动作从未发生）
    let receipts = list_receipts(ws, 50, None).expect("receipts");
    assert!(receipts.is_empty(), "schema 拒绝无回执");
    // 账本零变动
    let ledger = nemesis_connectors::ledger::load_ledger(ws).expect("ledger");
    assert!(ledger.orders.is_empty());
}

#[tokio::test]
async fn test_finance_budget_fail_closed_without_config() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(ws, "paper-broker", MIXED_MANIFEST);
    let tool = invoke_tool(ws, &ws.join("config.json")); // 无 config = finance 全 0

    let err = tool
        .execute(
            r#"{"connector":"paper-broker","op":"place_order","args":{"side":"buy","symbol":"AAPL","qty":1,"price":1.0}}"#,
            &ctx(),
        )
        .await
        .expect_err("fail-closed must reject");
    assert!(err.contains("⛔ RATE LIMIT"), "prefix: {err}");
    assert!(err.contains("fail-closed"), "denial message: {err}");

    let receipts = list_receipts(ws, 50, None).expect("receipts");
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].outcome, "budget_blocked");
    // 账本零变动
    let ledger = nemesis_connectors::ledger::load_ledger(ws).expect("ledger");
    assert!(ledger.orders.is_empty());
}

#[tokio::test]
async fn test_disabled_connector_writes_disabled_receipt() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    let disabled = MIXED_MANIFEST.replace(
        "version = \"0.1.0\"",
        "version = \"0.1.0\"\nenabled = false",
    );
    write_connector(ws, "paper-broker", &disabled);
    let tool = invoke_tool(ws, &ws.join("config.json"));

    let err = tool
        .execute(r#"{"connector":"paper-broker","op":"positions"}"#, &ctx())
        .await
        .expect_err("disabled must error");
    assert!(err.contains("已停用"), "msg: {err}");
    let receipts = list_receipts(ws, 50, None).expect("receipts");
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].outcome, "disabled");
}

#[tokio::test]
async fn test_unknown_connector_and_op_are_honest_errors_without_receipt() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(ws, "paper-broker", MIXED_MANIFEST);
    let tool = invoke_tool(ws, &ws.join("config.json"));

    for args in [
        r#"{"connector":"ghost","op":"x"}"#,
        r#"{"connector":"paper-broker","op":"ghost"}"#,
        r#"{"connector":"paper-broker"}"#,
    ] {
        let err = tool.execute(args, &ctx()).await.expect_err("must error");
        assert!(err.contains("Tool error"), "honest error: {err}");
    }
    // 无动作可记 = 无回执
    let receipts = list_receipts(ws, 50, None).expect("receipts");
    assert!(receipts.is_empty(), "no receipts for unknown targets");
}

#[tokio::test]
async fn test_paper_positions_and_cancel_honest_boundary() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(ws, "paper-broker", MIXED_MANIFEST);
    let config = ws.join("config.json");
    write_finance_config(&config);
    let tool = invoke_tool(ws, &config);

    // positions（空账本）
    let out = tool
        .execute(r#"{"connector":"paper-broker","op":"positions"}"#, &ctx())
        .await
        .expect("positions ok");
    assert!(out.contains("1000000"), "default cash: {out}");

    // buy 一单
    tool.execute(
        r#"{"connector":"paper-broker","op":"place_order","args":{"side":"buy","symbol":"AAPL","qty":10,"price":100.0}}"#,
        &ctx(),
    )
    .await
    .expect("buy ok");

    // positions 有持仓
    let out = tool
        .execute(r#"{"connector":"paper-broker","op":"positions"}"#, &ctx())
        .await
        .expect("positions ok 2");
    assert!(out.contains("AAPL"), "position listed: {out}");

    // cancel = 诚实报错（v1 即时成交无挂单）
    let err = tool
        .execute(
            r#"{"connector":"paper-broker","op":"cancel_order","args":{"order_id":"NB-ORD-1"}}"#,
            &ctx(),
        )
        .await
        .expect_err("cancel must honestly error");
    assert!(
        err.contains("已即时成交") || err.contains("无挂单"),
        "msg: {err}"
    );
}

#[tokio::test]
async fn test_unknown_action_binding_is_honest_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    let manifest = MIXED_MANIFEST.replace(
        "action = \"builtin.paper_cancel\"",
        "action = \"builtin.nope\"",
    );
    write_connector(ws, "paper-broker", &manifest);
    let tool = invoke_tool(ws, &ws.join("config.json"));
    let err = tool
        .execute(
            r#"{"connector":"paper-broker","op":"cancel_order","args":{}}"#,
            &ctx(),
        )
        .await
        .expect_err("unknown action must error");
    assert!(err.contains("未知内置动作"), "msg: {err}");
}

#[tokio::test]
async fn test_builtin_noop_is_invocable_minimal_action() {
    // 前端「新建连接器」最小模板的运行期支撑：builtin.noop 恒成功、零副作用，
    // 走完整治理面（预算/凭条照记）——粘模板即得可真实 invoke 的连接器。
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(
        ws,
        "noop-demo",
        r#"
[connector]
id = "noop-demo"
name = "Noop Demo"
version = "0.1.0"
kind = "local"

[[op]]
id = "hello"
intent = ["示例操作"]
risk = "low"
class = "read"
action = "builtin.noop"
"#,
    );
    let tool = invoke_tool(ws, &ws.join("config.json"));
    let out = tool
        .execute(
            r#"{"connector":"noop-demo","op":"hello","args":{}}"#,
            &ctx(),
        )
        .await
        .expect("noop must succeed");
    assert!(out.contains("noop"), "summary 应点名 noop，实际: {out}");
    assert!(
        out.contains("noop-demo") && out.contains("`hello`"),
        "summary 应带 connector 与 op 定位，实际: {out}"
    );
    // 凭条照记（治理面走过场）。
    let receipts = list_receipts(ws, 10, None).expect("receipts readable");
    assert!(
        receipts
            .iter()
            .any(|r| r.connector == "noop-demo" && r.op == "hello"),
        "noop 调用必须留凭条，实际: {receipts:?}"
    );
}

#[test]
fn test_receipt_card_contains_receipt_id_and_quota() {
    use nemesis_connectors::receipt::{args_digest, new_receipt_id};
    let now = chrono::Utc::now();
    let r = Receipt {
        receipt_id: new_receipt_id(now.timestamp_millis()),
        ts: now.to_rfc3339(),
        connector: "paper-broker".into(),
        op: "place_order".into(),
        risk: "critical".into(),
        class: "write".into(),
        outcome: "executed".into(),
        args_digest: args_digest(&serde_json::json!({"qty": 1})),
        summary: "买入 1 股".into(),
        receipt_key: Some("order_id".into()),
        receipt_value: Some("NB-ORD-1".into()),
        budget_consumed: None,
        approval_ref: None,
        session_key: Some("session-1".into()),
        undoable: false,
    };
    let card = receipt_card(&r, 3, 10, Some((1, 5)));
    assert!(card.contains(&r.receipt_id));
    assert!(card.contains("paper-broker/place_order"));
    assert!(card.contains("critical·write"));
    assert!(card.contains("3/10"));
    assert!(card.contains("1/5"));
    assert!(card.contains("NB-ORD-1"));
}

/// PLAN §九 验收 6：越界 op 用例证明拦截成立——http 绑定指向回环地址，
/// 真 SecurityPlugin（SSRF 闸默认开、block_localhost）必须先于发送拦截；
/// 且拦截即有账（error 回执落盘，summary 指认根因）。
#[cfg(feature = "security")]
#[tokio::test]
async fn test_out_of_bounds_http_op_blocked_by_ssrf_guard() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(
        ws,
        "oob-conn",
        r#"
[connector]
id = "oob-conn"
name = "越界探针"
version = "0.1.0"
kind = "rest"

[connector.http]
base = "http://127.0.0.1:9"

[[op]]
id = "pwn"
intent = ["越界探测"]
risk = "medium"
class = "network"
http = { method = "GET", path = "/pwn" }
"#,
    );
    let tool = ConnectorInvokeTool::new(
        ws.to_path_buf(),
        ws.join("config.json"),
        #[cfg(feature = "security")]
        Some(Arc::new(nemesis_security::pipeline::SecurityPlugin::new(
            nemesis_security::pipeline::SecurityPluginConfig::default(),
        ))),
    );
    let err = tool
        .execute(r#"{"connector":"oob-conn","op":"pwn","args":{}}"#, &ctx())
        .await
        .expect_err("回环 op 必须被 SSRF 闸拦截");
    assert!(err.contains("SSRF"), "错误应指认 SSRF 闸: {err}");

    // 拦截即有账：error 回执落盘且 summary 指认根因（存在性是账）。
    let receipts = list_receipts(ws, 10, Some("oob-conn")).expect("list");
    assert_eq!(receipts.len(), 1, "拦截也要落回执");
    assert_eq!(receipts[0].outcome, "error");
    assert!(
        receipts[0].summary.contains("SSRF"),
        "summary 应指认 SSRF: {}",
        receipts[0].summary
    );
}

// ---- 行动代理 P0（PLAN §6.2 凭条 approval_ref 契约）----
// ctx.approval_ref（dispatch 经 execute_tracked 拿到的交互审批引用）必须原样
// 进凭条；ctx 无引用（直通/规则表/免打扰）= None——凭条不伪造背书。

#[tokio::test]
async fn test_receipt_carries_approval_ref_from_ctx() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(ws, "paper-broker", MIXED_MANIFEST);
    let config = ws.join("config.json");
    write_finance_config(&config);
    let tool = invoke_tool(ws, &config);

    let mut c = ctx();
    c.approval_ref = Some("appr-card-42".to_string());
    let out = tool
        .execute(
            r#"{"connector":"paper-broker","op":"place_order","args":{"side":"buy","symbol":"AAPL","qty":1,"price":100.0}}"#,
            &c,
        )
        .await
        .expect("order ok");
    assert!(out.contains("\"executed\": true"), "result: {out}");

    let receipts = list_receipts(ws, 50, None).expect("receipts");
    assert_eq!(receipts.len(), 1);
    assert_eq!(
        receipts[0].approval_ref.as_deref(),
        Some("appr-card-42"),
        "receipt must carry the interactive approval reference verbatim"
    );
}

#[tokio::test]
async fn test_receipt_without_ctx_approval_ref_stays_none() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(ws, "paper-broker", MIXED_MANIFEST);
    let config = ws.join("config.json");
    write_finance_config(&config);
    let tool = invoke_tool(ws, &config);

    // ctx.approval_ref 缺省 None（直通路径）——凭条不得伪造审批引用。
    // quote 走真实 HTTP（测试环境必然失败）——顺带覆盖 error 臂：错误凭条同样无引用。
    let out = tool
        .execute(
            r#"{"connector":"paper-broker","op":"quote","args":{"symbol":"AAPL"}}"#,
            &ctx(),
        )
        .await;

    let receipts = list_receipts(ws, 50, None).expect("receipts");
    assert_eq!(receipts.len(), 1, "quote produced one receipt, out={out:?}");
    assert_eq!(receipts[0].outcome, "error", "no network in tests: {out:?}");
    assert!(
        receipts[0].approval_ref.is_none(),
        "no-approval path must not fabricate approval_ref"
    );
}

// ---- 收官批 A 段：静态头 / 凭据 prefix / PATCH ----

#[test]
fn test_merge_request_headers_credential_wins_and_folds_case() {
    let mut static_headers = std::collections::BTreeMap::new();
    static_headers.insert("User-Agent".to_string(), "agent/1.0".to_string());
    static_headers.insert("Accept".to_string(), "application/json".to_string());
    let merged = merge_request_headers(
        &static_headers,
        vec![("user-agent".to_string(), "token abc".to_string())],
    );
    assert_eq!(
        merged,
        vec![
            ("Accept".to_string(), "application/json".to_string()),
            ("user-agent".to_string(), "token abc".to_string()),
        ],
        "凭据同名胜出（大小写折叠比较）+ 输出按小写头名排序（确定性可测）"
    );
}

/// 收官批 A 段 wiremock e2e：静态头发送 + PATCH 方法 + vault 凭据 prefix
/// 端到端（`Authorization: token <PAT>` 认证头形态），同时钉死同名冲突
/// 裁决——静态头 `Authorization` 被 vault 凭据覆盖。
#[cfg(feature = "security")]
#[test]
fn test_http_static_headers_patch_and_vault_prefix_end_to_end() {
    // env 口令是进程单例：与 vault_runtime / commands::vault 的 env 触及测试群
    // 经 crate::GLOBAL_STATE_LOCK 互斥（本地 cargo test 并行防互踩；nextest
    // 每测试独立进程，锁无竞争直通）。std 锁 guard 不得活过 await（clippy
    // await_holding_lock，CI -D warnings 红）——锁在同步外壳持有，async 体经
    // block_on 执行（commands/vault run_arms 同款形态）。
    let _env_guard = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(test_http_static_headers_patch_and_vault_prefix_end_to_end_inner());
}

#[cfg(feature = "security")]
async fn test_http_static_headers_patch_and_vault_prefix_end_to_end_inner() {
    use nemesis_security::vault::VaultStore;

    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("PATCH"))
        .and(wiremock::matchers::path("/repos/octocat/demo/issues/1"))
        .and(wiremock::matchers::header(
            "authorization",
            "token gitea-pat-1234",
        ))
        .and(wiremock::matchers::header(
            "user-agent",
            "nemesisbot-connector/0.1",
        ))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"state": "closed"})),
        )
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(
        ws,
        "gitea",
        &r#"
[connector]
id = "gitea"
name = "Gitea"
version = "0.1.0"
kind = "rest"

[connector.http]
base = "{BASE}"
[connector.http.headers]
User-Agent = "nemesisbot-connector/0.1"
Authorization = "static-should-lose"

[connector.credentials]
pat = { required = true, into = "header:Authorization", prefix = "token " }

[[op]]
id = "issue_close"
risk = "high"
class = "write"
confirm = "required"
http = { method = "PATCH", path = "/repos/{owner}/{repo}/issues/{index}" }
"#
        .replace("{BASE}", &server.uri()),
    );

    // 读侧 invoke 凭据注入走 vault_runtime::open_unlocked（argon2 vault 需 env
    // 解锁）；CI/ET 为 nextest 每测试独立进程天然隔离，本地 cargo test 并行用后即摘。
    unsafe { std::env::set_var("NEMESISBOT_VAULT_PASSPHRASE", "test-passphrase") };
    // 临时 vault 写入 connector/gitea/pat（显式 Argon2id 跨平台一致：
    // default_mode 在非 Windows 需口令而 None 直接报错，Linux ET 假红根因）
    let vault_path = nemesis_path::resolve_vault_path_in_workspace(ws);
    let mut store = VaultStore::create(
        &vault_path,
        nemesis_security::vault::VaultMode::Argon2id,
        Some("test-passphrase"),
    )
    .expect("create vault");
    store
        .set(
            "connector/gitea/pat",
            "gitea-pat-1234",
            "connector",
            "测试 PAT",
        )
        .expect("set pat");
    store.save().expect("save vault");

    let tool = invoke_tool(ws, &ws.join("config.json"));
    let out = tool
        .execute(
            r#"{"connector":"gitea","op":"issue_close","args":{"owner":"octocat","repo":"demo","index":1,"state":"closed"}}"#,
            &ctx(),
        )
        .await
        .expect("close ok");
    assert!(out.contains("closed"), "result: {out}");

    // 头匹配器即断言：静态头在发 + 同名冲突 vault 凭据胜 + prefix 拼接。
    let receipts = list_receipts(ws, 50, None).expect("receipts");
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].outcome, "executed");
    unsafe { std::env::remove_var("NEMESISBOT_VAULT_PASSPHRASE") };
}

/// required 凭据缺失 = 诚实报错（不动手），不带 vault 文件的全新环境直触发。
#[cfg(feature = "security")]
#[tokio::test]
async fn test_required_credential_missing_is_honest_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(
        ws,
        "gitea",
        r#"
[connector]
id = "gitea"
name = "Gitea"
version = "0.1.0"
kind = "rest"

[connector.http]
base = "http://127.0.0.1:9"

[connector.credentials]
pat = { required = true, into = "header:Authorization" }

[[op]]
id = "issues_list"
risk = "low"
class = "network"
http = { method = "GET", path = "/repos/{owner}/{repo}/issues" }
"#,
    );
    let tool = invoke_tool(ws, &ws.join("config.json"));
    let err = tool
        .execute(
            r#"{"connector":"gitea","op":"issues_list","args":{"owner":"o","repo":"r"}}"#,
            &ctx(),
        )
        .await
        .expect_err("required credential missing must error");
    assert!(err.contains("凭据槽位 `pat` 缺失"), "msg: {err}");
    // 未动手 = 无网络错误凭条（只有缺失事实）；缺失不落 executed。
    let receipts = list_receipts(ws, 50, None).expect("receipts");
    assert!(
        receipts.iter().all(|r| r.outcome != "executed"),
        "missing credential must not execute: {receipts:?}"
    );
}

// ---- 收官批 A2：Gitea 模板 connector（mock 快速层）----

/// workspace 模板恒合法 + op 面与验收表逐一对齐 + 越界面（危险 op 不声明）。
/// 模板腐化（改坏 manifest / 悄悄加 op / 减验收 op）在这里红。
#[test]
fn test_workspace_template_gitea_manifest_is_valid_and_scoped() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/workspace/connectors/gitea/connector.toml"
    );
    let text = std::fs::read_to_string(path).expect("gitea 模板存在");
    let c = nemesis_connectors::manifest::parse_connector_manifest(&text)
        .expect("模板 manifest 必须恒合法");
    assert_eq!(c.id, "gitea");
    let ids: Vec<&str> = c.ops.iter().map(|o| o.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "issues_list",
            "issue_get",
            "issue_comment",
            "issue_close",
            "pr_list",
            "pr_get",
        ]
    );
    // issue_close = high + 强授权卡（验收表行）
    let close = c.ops.iter().find(|o| o.id == "issue_close").unwrap();
    assert!(close.confirm_required);
    assert_eq!(close.risk, RiskLevel::High);
    assert_eq!(close.class, OpClass::Write);
    // 凭据形态：Authorization 头 + token 前缀（Gitea PAT 认证头惯用形态）
    let pat = c.credentials.get("pat").expect("pat slot");
    assert!(pat.required);
    assert_eq!(pat.prefix.as_deref(), Some("token "));
    assert!(
        matches!(pat.into, nemesis_connectors::manifest::CredentialInto::Header(ref h) if h == "Authorization")
    );
}

// ---- 收官批 C2：web-ui 模板 connector（浏览器兜底，playwright-mcp）----

/// 示例模板（examples/connectors/，A3 起不随 onboard 提取）恒合法 + **默认停用**
/// （只声明不默认开）+ op 面对齐 C 段验收表
/// （读 low/read 直通、写 high/write 强授权卡）+ 全 tool 绑定 playwright-mcp +
/// 危险 op 不声明（evaluate/run_code_unsafe/file_upload 不进清单）。
#[test]
fn test_workspace_template_webui_manifest_is_valid_and_disabled_by_default() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../examples/connectors/web-ui/connector.toml"
    );
    let text = std::fs::read_to_string(path).expect("web-ui 模板存在");
    let c = nemesis_connectors::manifest::parse_connector_manifest(&text)
        .expect("模板 manifest 必须恒合法");
    assert_eq!(c.id, "web-ui");
    // 只声明不默认开：浏览器兜底是用户显式开启的能力。
    assert!(!c.enabled, "web-ui 模板必须默认停用（enabled = false）");
    let ids: Vec<&str> = c.ops.iter().map(|o| o.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "browser_navigate",
            "browser_snapshot",
            "browser_click",
            "browser_type",
        ]
    );
    // 读面：low/read、无强授权卡。
    for id in ["browser_navigate", "browser_snapshot"] {
        let op = c.ops.iter().find(|o| o.id == id).unwrap();
        assert_eq!(op.risk, RiskLevel::Low, "{id}");
        assert_eq!(op.class, OpClass::Read, "{id}");
        assert!(!op.confirm_required, "{id} 不应要强授权卡");
    }
    // 写面：high/write + 强授权卡。
    for id in ["browser_click", "browser_type"] {
        let op = c.ops.iter().find(|o| o.id == id).unwrap();
        assert_eq!(op.risk, RiskLevel::High, "{id}");
        assert_eq!(op.class, OpClass::Write, "{id}");
        assert!(op.confirm_required, "{id} 必须强授权卡");
    }
    // 全部 tool 绑定到 playwright-mcp server（前缀对齐 adapter.rs 的注册
    // 形态 mcp_{server}_{tool}——sanitize 后的 name 单下划线）；无 receipt
    // 主键（MCP adapter 扁平化文本，凭条走首行 summary——C4 实施期裁定）。
    for op in &c.ops {
        match &op.binding {
            nemesis_connectors::manifest::OpBinding::Tool { tool } => {
                assert!(
                    tool.starts_with("mcp_playwright_"),
                    "{op_id} 绑定 {tool} 应指向 playwright-mcp",
                    op_id = op.id
                );
            }
            other => panic!("{} 应为 tool 绑定，实际 {other:?}", op.id),
        }
        assert!(op.receipt_key.is_none(), "{} 不声明 receipt", op.id);
    }
    // 越界面：危险工具不声明（不存在即安全）。
    let bound: Vec<&str> = c
        .ops
        .iter()
        .filter_map(|o| match &o.binding {
            nemesis_connectors::manifest::OpBinding::Tool { tool } => Some(tool.as_str()),
            _ => None,
        })
        .collect();
    for dangerous in [
        "browser_evaluate",
        "browser_run_code_unsafe",
        "browser_file_upload",
    ] {
        assert!(
            !bound.iter().any(|t| t.contains(dangerous)),
            "模板不得声明危险工具 {dangerous}"
        );
    }
}

/// 6 op 对 wiremock（Gitea 同形态 API）全跑通：认证头注入 / GET query /
/// POST+PATCH body / 回执主键（issue_comment→id、issue_close→number）。
#[cfg(feature = "security")]
#[test]
fn test_gitea_template_ops_against_mock_gitea() {
    // env 口令是进程单例：与 vault_runtime / commands::vault 的 env 触及测试群
    // 经 crate::GLOBAL_STATE_LOCK 互斥（本地 cargo test 并行防互踩；nextest
    // 每测试独立进程，锁无竞争直通）。std 锁 guard 不得活过 await（clippy
    // await_holding_lock，CI -D warnings 红）——锁在同步外壳持有，async 体经
    // block_on 执行（commands/vault run_arms 同款形态）。
    let _env_guard = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(test_gitea_template_ops_against_mock_gitea_inner());
}

#[cfg(feature = "security")]
async fn test_gitea_template_ops_against_mock_gitea_inner() {
    use nemesis_security::vault::VaultStore;

    let server = wiremock::MockServer::start().await;
    let auth = || wiremock::matchers::header("authorization", "token mock-pat");
    for (method_m, path_m, body) in [
        (
            wiremock::matchers::method("GET"),
            wiremock::matchers::path("/api/v1/repos/o/r/issues"),
            serde_json::json!([{"number": 1, "title": "t"}]),
        ),
        (
            wiremock::matchers::method("GET"),
            wiremock::matchers::path("/api/v1/repos/o/r/issues/1"),
            serde_json::json!({"number": 1, "state": "open"}),
        ),
        (
            wiremock::matchers::method("POST"),
            wiremock::matchers::path("/api/v1/repos/o/r/issues/1/comments"),
            serde_json::json!({"id": 77, "body": "验收评论"}),
        ),
        (
            wiremock::matchers::method("PATCH"),
            wiremock::matchers::path("/api/v1/repos/o/r/issues/1"),
            serde_json::json!({"number": 1, "state": "closed"}),
        ),
        (
            wiremock::matchers::method("GET"),
            wiremock::matchers::path("/api/v1/repos/o/r/pulls"),
            serde_json::json!([]),
        ),
        (
            wiremock::matchers::method("GET"),
            wiremock::matchers::path("/api/v1/repos/o/r/pulls/2"),
            serde_json::json!({"number": 2, "state": "open"}),
        ),
    ] {
        wiremock::Mock::given(method_m)
            .and(path_m)
            .and(auth())
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    let template = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/workspace/connectors/gitea/connector.toml"
    ))
    .expect("template");
    write_connector(
        ws,
        "gitea",
        &template.replace(
            "http://127.0.0.1:3000/api/v1",
            &format!("{}/api/v1", server.uri()),
        ),
    );

    // 读侧 invoke 凭据注入走 vault_runtime::open_unlocked（argon2 vault 需 env
    // 解锁）；CI/ET 为 nextest 每测试独立进程天然隔离，本地 cargo test 并行用后即摘。
    unsafe { std::env::set_var("NEMESISBOT_VAULT_PASSPHRASE", "test-passphrase") };
    let vault_path = nemesis_path::resolve_vault_path_in_workspace(ws);
    let mut store = VaultStore::create(
        &vault_path,
        nemesis_security::vault::VaultMode::Argon2id,
        Some("test-passphrase"),
    )
    .expect("vault");
    store
        .set("connector/gitea/pat", "mock-pat", "connector", "mock")
        .expect("set");
    store.save().expect("save");

    let config = ws.join("config.json");
    write_finance_config(&config);
    let tool = invoke_tool(ws, &config);

    let cases: Vec<(&str, &str, bool)> = vec![
        (
            r#"{"connector":"gitea","op":"issues_list","args":{"owner":"o","repo":"r","state":"all"}}"#,
            "\"number\":1",
            true,
        ),
        (
            r#"{"connector":"gitea","op":"issue_get","args":{"owner":"o","repo":"r","index":1}}"#,
            "\"state\":\"open\"",
            true,
        ),
        (
            r#"{"connector":"gitea","op":"issue_comment","args":{"owner":"o","repo":"r","index":1,"body":"验收评论"}}"#,
            "\"id\":77",
            true,
        ),
        (
            r#"{"connector":"gitea","op":"issue_close","args":{"owner":"o","repo":"r","index":1,"state":"closed"}}"#,
            "\"state\":\"closed\"",
            true,
        ),
        (
            r#"{"connector":"gitea","op":"pr_list","args":{"owner":"o","repo":"r"}}"#,
            "[]",
            true,
        ),
        (
            r#"{"connector":"gitea","op":"pr_get","args":{"owner":"o","repo":"r","index":2}}"#,
            "\"number\":2",
            true,
        ),
        // 越界：未声明 op = 诚实报错（不存在即安全）
        (
            r#"{"connector":"gitea","op":"repo_delete","args":{"owner":"o","repo":"r"}}"#,
            "",
            false,
        ),
    ];
    for (args, expect, ok) in cases {
        let result = tool.execute(args, &ctx()).await;
        if ok {
            let out = result.unwrap_or_else(|e| panic!("{args}: {e}"));
            assert!(out.contains(expect), "{args}: want {expect} in {out}");
        } else {
            let err = result.expect_err("undeclared op must error");
            assert!(!err.contains("executed"), "{args}: {err}");
        }
    }

    // 回执主键：issue_comment→id=77、issue_close→number=1
    let receipts = list_receipts(ws, 50, Some("gitea")).expect("receipts");
    let comment = receipts.iter().find(|r| r.op == "issue_comment").unwrap();
    assert_eq!(comment.outcome, "executed");
    assert_eq!(comment.receipt_key.as_deref(), Some("id"));
    assert_eq!(comment.receipt_value.as_deref(), Some("77"));
    let close = receipts.iter().find(|r| r.op == "issue_close").unwrap();
    assert_eq!(close.outcome, "executed");
    assert_eq!(close.receipt_key.as_deref(), Some("number"));
    assert_eq!(close.receipt_value.as_deref(), Some("1"));
    unsafe { std::env::remove_var("NEMESISBOT_VAULT_PASSPHRASE") };
}

// ---- 收官批 A2：Gitea 真实链路 e2e（scripts/test-gitea-e2e.sh 拉起真实实例后跑）----

/// 递归收集目录下全部文件（泄露扫描用；只遍历不消费顺序——平台序无影响）。
#[cfg(feature = "security")]
fn walk_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk_files(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// gitea op 调用便捷形态（owner/repo 固定，extra 为剩余 args JSON 片段）。
#[cfg(feature = "security")]
async fn gitea_invoke(
    tool: &ConnectorInvokeTool,
    owner: &str,
    repo: &str,
    op: &str,
    extra: &str,
) -> Result<String, String> {
    let args = format!(
        r#"{{"connector":"gitea","op":"{op}","args":{{"owner":"{owner}","repo":"{repo}"{extra}}}}}"#
    );
    tool.execute(&args, &ctx()).await
}

/// 真实 Gitea 实例上的全链路验收（验收表 §3.2 行 4-9）：模板 manifest →
/// vault PAT 注入 → HTTP 真发 → 真实副作用（评论计数 +1 / state=closed）→
/// 未声明 op 越界诚实拒 → 凭据轮换立即生效（坏 token 401 / 好 token 恢复）
/// → 全 workspace 字节级泄露扫描零命中（凭据铁律：vault 密文、回执 digest）。
///
/// env `GITEA_E2E_URL`/`GITEA_E2E_TOKEN`/`GITEA_E2E_REPO` 缺席 = 诚实 SKIP
/// （真实链路层不可用不假红；`scripts/test-gitea-e2e.sh` 可拉起实例）。
/// security=None：SSRF 闸默认拦回环而真实 Gitea 就在回环——闸行为已由
/// 独立单测覆盖（test_connector_outbound_ssrf_blocked）。
#[cfg(feature = "security")]
#[test]
fn gitea_real_chain_e2e() {
    // env 口令是进程单例：与 vault_runtime / commands::vault 的 env 触及测试群
    // 经 crate::GLOBAL_STATE_LOCK 互斥（本地 cargo test 并行防互踩；nextest
    // 每测试独立进程，锁无竞争直通）。std 锁 guard 不得活过 await（clippy
    // await_holding_lock，CI -D warnings 红）——锁在同步外壳持有，async 体经
    // block_on 执行（commands/vault run_arms 同款形态）。
    let _env_guard = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(gitea_real_chain_e2e_inner());
}

#[cfg(feature = "security")]
async fn gitea_real_chain_e2e_inner() {
    use nemesis_security::vault::VaultStore;

    let Some(url) = std::env::var("GITEA_E2E_URL").ok() else {
        println!(
            "SKIP: GITEA_E2E_URL 未设置（真实链路层缺席；scripts/test-gitea-e2e.sh 可拉起实例）"
        );
        return;
    };
    let token = std::env::var("GITEA_E2E_TOKEN").expect("GITEA_E2E_TOKEN");
    let repo_full = std::env::var("GITEA_E2E_REPO").expect("GITEA_E2E_REPO");
    let (owner, repo) = repo_full.split_once('/').expect("REPO 形如 owner/name");

    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    let template = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/workspace/connectors/gitea/connector.toml"
    ))
    .expect("gitea 模板存在");
    write_connector(
        ws,
        "gitea",
        &template.replace("http://127.0.0.1:3000/api/v1", &format!("{url}/api/v1")),
    );

    // 读侧 invoke 凭据注入走 vault_runtime::open_unlocked（argon2 vault 需 env
    // 解锁）；CI/ET 为 nextest 每测试独立进程天然隔离，本地 cargo test 并行用后即摘。
    unsafe { std::env::set_var("NEMESISBOT_VAULT_PASSPHRASE", "test-passphrase") };
    let vault_path = nemesis_path::resolve_vault_path_in_workspace(ws);
    let mut store = VaultStore::create(
        &vault_path,
        nemesis_security::vault::VaultMode::Argon2id,
        Some("test-passphrase"),
    )
    .expect("create vault");
    store
        .set("connector/gitea/pat", &token, "connector", "e2e PAT")
        .expect("set pat");
    store.save().expect("save vault");

    // 限额放宽：本测试单进程连打 ~7 个成功 op，单测默认 10 太紧
    let config = ws.join("config.json");
    std::fs::write(
        &config,
        r#"{"connectors":{"enabled":true,"max_ops_per_hour":100}}"#,
    )
    .expect("write config");
    let tool = invoke_tool(ws, &config);

    // 1) issues_list 挑一个 open issue（脚本每轮新建目标 issue，保证 ≥1）
    let out = gitea_invoke(
        &tool,
        owner,
        repo,
        "issues_list",
        r#","state":"open","limit":1"#,
    )
    .await
    .expect("issues_list ok");
    let list: serde_json::Value = serde_json::from_str(&out)
        .unwrap_or_else(|e| panic!("issues_list 输出非 JSON: {e}\n{out}"));
    let issue_no = list[0]["number"]
        .as_i64()
        .unwrap_or_else(|| panic!("无 open issue 可用（GITEA_E2E_KEEP=1 可保留现场排查）: {out}"));

    // 2) issue_get 评论计数基线（Gitea issue JSON `comments` = 评论数）
    let out = gitea_invoke(
        &tool,
        owner,
        repo,
        "issue_get",
        &format!(r#","index":{issue_no}"#),
    )
    .await
    .expect("issue_get ok");
    let issue: serde_json::Value = serde_json::from_str(&out).expect("issue_get JSON");
    let base_comments = issue["comments"].as_i64().expect("comments 字段");

    // 3) issue_comment → executed 回执 + receipt 主键 id（真实副作用前半）
    let out = gitea_invoke(
        &tool,
        owner,
        repo,
        "issue_comment",
        &format!(r#","index":{issue_no},"body":"收官批 A2 真实链路验收评论""#),
    )
    .await
    .expect("issue_comment ok");
    assert!(out.contains("\"id\""), "评论响应带 id: {out}");

    // 4) issue_get → 计数 +1
    let out = gitea_invoke(
        &tool,
        owner,
        repo,
        "issue_get",
        &format!(r#","index":{issue_no}"#),
    )
    .await
    .expect("issue_get ok");
    let issue: serde_json::Value = serde_json::from_str(&out).expect("issue_get JSON");
    assert_eq!(
        issue["comments"].as_i64(),
        Some(base_comments + 1),
        "评论必须真实落库（平台侧副作用）"
    );

    // 5) issue_close（confirm=required；直调绕审批卡与 mock 同款）→ state=closed
    let out = gitea_invoke(
        &tool,
        owner,
        repo,
        "issue_close",
        &format!(r#","index":{issue_no},"state":"closed""#),
    )
    .await
    .expect("issue_close ok");
    assert!(out.contains("closed"), "close 响应: {out}");

    // 6) issue_get → 平台侧 state 真为 closed
    let out = gitea_invoke(
        &tool,
        owner,
        repo,
        "issue_get",
        &format!(r#","index":{issue_no}"#),
    )
    .await
    .expect("issue_get ok");
    let issue: serde_json::Value = serde_json::from_str(&out).expect("issue_get JSON");
    assert_eq!(issue["state"].as_str(), Some("closed"), "真实关闭未生效");

    // 回执面：comment/close 均 executed 且主键形态正确
    let receipts = list_receipts(ws, 50, Some("gitea")).expect("receipts");
    let rc = receipts
        .iter()
        .find(|r| r.op == "issue_comment" && r.outcome == "executed")
        .expect("comment executed 回执");
    assert_eq!(rc.receipt_key.as_deref(), Some("id"));
    assert!(
        rc.receipt_value
            .as_deref()
            .is_some_and(|v| v.parse::<i64>().is_ok()),
        "receipt id 形态: {rc:?}"
    );
    let rclose = receipts
        .iter()
        .find(|r| r.op == "issue_close" && r.outcome == "executed")
        .expect("close executed 回执");
    assert_eq!(rclose.receipt_key.as_deref(), Some("number"));
    assert_eq!(
        rclose.receipt_value.as_deref(),
        Some(issue_no.to_string().as_str())
    );

    // 7) 越界：repo 删除等未声明 op = 诚实报错（不存在即安全）
    let err = gitea_invoke(&tool, owner, repo, "repo_delete", "")
        .await
        .expect_err("未声明 op 必须拒绝");
    assert!(err.contains("无 op `repo_delete`"), "越界报错: {err}");

    // 8) 凭据轮换立即生效：坏 token → 401 诚实失败；set 回好 token → 恢复
    //    （resolve_alias_at 每次现开 vault，无缓存——rotation 不重启即生效）
    let mut v = VaultStore::open(&vault_path).expect("reopen vault");
    // argon2 文件 open 即锁定态，set/save 需先 unlock（DPAPI 平台已解锁，幂等成功）
    v.unlock("test-passphrase").expect("unlock vault");
    v.set(
        "connector/gitea/pat",
        "deadbeef-invalid-token",
        "connector",
        "坏",
    )
    .expect("set bad");
    v.save().expect("save bad");
    let err = gitea_invoke(&tool, owner, repo, "pr_list", "")
        .await
        .expect_err("坏 token 必须失败");
    assert!(err.contains("HTTP 401"), "401 诚实透传: {err}");

    v.set("connector/gitea/pat", &token, "connector", "恢复")
        .expect("set good");
    v.save().expect("save good");
    let out = gitea_invoke(&tool, owner, repo, "pr_list", "")
        .await
        .expect("恢复后成功");
    assert!(
        out.contains('[') || out.contains('{'),
        "轮换恢复后响应形态: {out}"
    );

    // 9) 泄露扫描：整个临时 workspace 逐文件字节级找 token——零命中
    //    （vault.enc 密文不落明文；回执只存 args_digest；manifest/config 无凭据）
    let mut files = Vec::new();
    walk_files(ws, &mut files);
    assert!(files.len() > 3, "workspace 应有产物: {files:?}");
    let needle = token.as_bytes();
    for f in &files {
        let Ok(bytes) = std::fs::read(f) else {
            continue;
        };
        assert!(
            !bytes.windows(needle.len()).any(|w| w == needle),
            "token 字节泄露于 {}（凭据铁律破线）",
            f.display()
        );
    }
    unsafe { std::env::remove_var("NEMESISBOT_VAULT_PASSPHRASE") };
}

// ===========================================================================
// 收官批 C 段：tool 绑定回执语义（C4）+ leniency（绑定工具缺席 = 诚实报错）
// ===========================================================================

/// C4 分类矩阵：⛔ 家族 = 安全层已裁决（透传不双写）；错误形态文本
/// （Error:/Tool error:/Exit code:/Plan mode:）= dispatch 未代写凭条 →
/// error 回执；其余 = 成功。F2 超时特判（2026-10-16 二轮复查）：错误形态
/// 且含 "timed out" 且非 `Exit code:` 前缀 = 结果未知（UnknownOutcome，
/// 保留账目不回吐）——`Exit code:` 是进程跑完的确定性失败，即便输出里
/// 恰好含 "timed out" 字样（测试二进制超时输出混入）也保持 Failed。
#[test]
fn test_classify_tool_result_receipt_semantics() {
    use ToolBindingClass::{DispatchRejected, Done, Failed, UnknownOutcome};
    // ⛔ 家族（dispatch 各安全分支的真实前缀形态）
    assert_eq!(
        classify_tool_result("⛔ SECURITY BLOCKED [layer:abac|policy:deny] no"),
        DispatchRejected
    );
    assert_eq!(
        classify_tool_result("⛔ ESTOP: 已急停 — 工具调用已被拒绝"),
        DispatchRejected
    );
    assert_eq!(
        classify_tool_result("⛔ HOOK BLOCKED [layer:hook|policy:tool_hook] x"),
        DispatchRejected
    );
    // 错误形态：dispatch 早退族
    assert_eq!(
        classify_tool_result("Error: Unknown tool 'mcp_ghost__click'"),
        Failed
    );
    assert_eq!(
        classify_tool_result("Error: Tool 'exec' is hidden by configuration"),
        Failed
    );
    // 错误形态：绑定工具执行失败
    assert_eq!(classify_tool_result("Tool error: args JSON 非法"), Failed);
    assert_eq!(classify_tool_result("Exit code: 1"), Failed);
    // 错误形态：Plan 闸（此前落 Done 产生假 executed 凭条，已根修）
    assert_eq!(
        classify_tool_result("Plan mode: file modifications are denied."),
        Failed
    );
    // F2 超时特判：真实超时文本形态（executor.rs/shell.rs）→ UnknownOutcome
    assert_eq!(
        classify_tool_result("Tool error: tool exec timed out after 300s"),
        UnknownOutcome
    );
    assert_eq!(
        classify_tool_result("Tool error: command timed out after 60s"),
        UnknownOutcome
    );
    // M2 扩面回归锁：exec 工具超时收尸 = `Ok("Command timed out …")`
    // （loop_tools.rs 单一真相源，**无错误前缀**）——此前落 Done = 假
    // executed 凭条，script 超时诚实臂的根修。
    assert_eq!(
        classify_tool_result(
            "Command timed out after 5 seconds. Partial output:\n(no output before timeout)"
        ),
        UnknownOutcome
    );
    // F2 误判防线：`Exit code:` 前缀 = 进程跑完退出非零（确定性失败），
    // 输出含 "timed out" 字样也不误判为 unknown
    assert_eq!(
        classify_tool_result(
            "Exit code: 1\ntest result: FAILED. 1 passed; 1 failed\n---- name stdout ----\nbenchmark timed out"
        ),
        Failed
    );
    // 成功（含多行首行为空形态）
    assert_eq!(classify_tool_result("page title: Example"), Done);
    assert_eq!(classify_tool_result("{\"status\":\"ok\"}"), Done);
}

/// F4（2026-10-16 二轮复查）：unknown 是最需要推送的结局（结果未知 →
/// 需人核账），必须进推送白名单；读类非资金仍静默只入账本。
#[test]
fn test_should_push_covers_unknown_outcome() {
    // 写/exec/网络类：四种执行面结局全推送（含 unknown）
    for outcome in ["executed", "error", "budget_blocked", "unknown"] {
        assert!(
            ConnectorInvokeTool::should_push(OpClass::Write, false, outcome),
            "write 类 outcome={outcome} 必须推送"
        );
    }
    // 读类：executed 非资金静默；资金类照常推送；unknown 资金类必推
    assert!(!ConnectorInvokeTool::should_push(
        OpClass::Read,
        false,
        "executed"
    ));
    assert!(ConnectorInvokeTool::should_push(
        OpClass::Read,
        true,
        "executed"
    ));
    assert!(
        ConnectorInvokeTool::should_push(OpClass::Read, true, "unknown"),
        "读+资金类 unknown 必须推送（最需人核账的结局）"
    );
}

/// tool 绑定 e2e（真 AgentLoop + 注册桩工具）：成功路径走完整 dispatch
/// 瀑布，executed 回执携带 manifest 声明 receipt_key 抠出的主键（C4：
/// 镜像 run_http）。桩工具模拟 MCP adapter 工具的注册/执行形态——瀑布
/// 对注册表工具一律同路，MCP 侧真实性由 nemesis-mcp 自己的测试承载。
#[tokio::test]
async fn test_tool_binding_success_walks_waterfall_and_carries_receipt_key() {
    use nemesis_agent::r#loop::{LlmMessage, LlmProvider, LlmResponse};
    use nemesis_agent::types::{AgentConfig, ChatOptions, ToolDefinition};

    struct NullProvider;
    #[async_trait::async_trait]
    impl LlmProvider for NullProvider {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<LlmMessage>,
            _options: Option<ChatOptions>,
            _tools: Vec<ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            unreachable!("tool binding 不触发 LLM")
        }
    }

    /// 桩 MCP 工具：返回 JSON（playwright-mcp 返回形态），带 title 主键。
    struct StubPageTool;
    #[async_trait::async_trait]
    impl Tool for StubPageTool {
        async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
            Ok(r#"{"status":"ok","title":"Example Domain"}"#.to_string())
        }
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(
        ws,
        "ui-bot",
        r#"
[connector]
id = "ui-bot"
name = "UI 自动化"
version = "0.1.0"
kind = "local"
description = "tool 绑定 C4 验证"

[[op]]
id = "read_title"
intent = ["读页面标题"]
risk = "low"
class = "read"
receipt = "status"
tool = "mcp_stub__read_page"
"#,
    );
    let tool = invoke_tool(ws, &ws.join("config.json"));
    let mut agent_loop = nemesis_agent::r#loop::AgentLoop::new(
        Box::new(NullProvider),
        AgentConfig {
            model: "test-model".to_string(),
            max_turns: 1,
            ..Default::default()
        },
    );
    agent_loop.register_tool("mcp_stub__read_page".to_string(), Box::new(StubPageTool));
    let loop_arc = Arc::new(agent_loop);
    tool.set_loop(Arc::downgrade(&loop_arc));

    let out = tool
        .execute(r#"{"connector":"ui-bot","op":"read_title"}"#, &ctx())
        .await
        .expect("tool binding ok");
    assert!(out.contains("Example Domain"), "result passthrough: {out}");

    let receipts = list_receipts(ws, 50, None).expect("receipts");
    assert_eq!(receipts.len(), 1);
    let r = &receipts[0];
    assert_eq!(r.outcome, "executed");
    assert_eq!(r.receipt_key.as_deref(), Some("status"));
    assert_eq!(r.receipt_value.as_deref(), Some("ok"));
}

/// leniency e2e（验收 5 第三臂）：绑定工具在注册表缺席时——manifest 合法
///（parse 不查注册表），调用 = dispatch 诚实报错 `Error: Unknown tool`，
/// 且工具侧落 error 回执（C4：错误也是账；此前该族透传无账）。
#[tokio::test]
async fn test_tool_binding_unknown_tool_is_honest_error_with_receipt() {
    use nemesis_agent::r#loop::{LlmMessage, LlmProvider, LlmResponse};
    use nemesis_agent::types::{AgentConfig, ChatOptions, ToolDefinition};

    struct NullProvider;
    #[async_trait::async_trait]
    impl LlmProvider for NullProvider {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<LlmMessage>,
            _options: Option<ChatOptions>,
            _tools: Vec<ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            unreachable!("tool binding 不触发 LLM")
        }
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(
        ws,
        "ui-bot",
        r#"
[connector]
id = "ui-bot"
name = "UI 自动化"
version = "0.1.0"
kind = "local"
description = "tool 绑定 leniency 验证"

[[op]]
id = "click"
intent = ["点击页面元素"]
risk = "high"
class = "write"
tool = "mcp_absent__browser_click"
"#,
    );
    let tool = invoke_tool(ws, &ws.join("config.json"));
    let loop_arc = Arc::new(nemesis_agent::r#loop::AgentLoop::new(
        Box::new(NullProvider),
        AgentConfig {
            model: "test-model".to_string(),
            max_turns: 1,
            ..Default::default()
        },
    ));
    tool.set_loop(Arc::downgrade(&loop_arc));

    // manifest 层 leniency：注册表缺席不拒 parse（回归锁钉死现行为）
    let (c, _trust) = find_connector(ws, "ui-bot").expect("manifest parses despite absent tool");
    assert!(matches!(
        c.ops[0].binding,
        nemesis_connectors::manifest::OpBinding::Tool { .. }
    ));

    let err = tool
        .execute(
            r##"{"connector":"ui-bot","op":"click","args":{"element":"#btn"}}"##,
            &ctx(),
        )
        .await
        .expect_err("absent bound tool must error honestly");
    assert!(err.contains("Unknown tool"), "honest dispatch error: {err}");

    let receipts = list_receipts(ws, 50, None).expect("receipts");
    assert_eq!(receipts.len(), 1, "error 也是账");
    let r = &receipts[0];
    assert_eq!(r.outcome, "error");
    assert!(r.summary.contains("Unknown tool"), "summary: {}", r.summary);
}

// ===========================================================================
// 收官批 E1：远程执行面（target_node → 集群 RPC → 镜像凭条）
// ===========================================================================

/// mock 通道捕获的调用（peer, action, payload）。
type CapturedCalls = Arc<std::sync::Mutex<Vec<(String, String, serde_json::Value)>>>;

/// 构造 mock remote_call_fn（签名与 ClusterWiring.cluster_rpc_call_fn 同形），
/// 每次调用记入 `calls` 并按 `respond` 同步应答。
fn mock_remote_fn(
    calls: CapturedCalls,
    respond: impl Fn(&str, &str, &serde_json::Value) -> Result<serde_json::Value, String>
    + Send
    + Sync
    + 'static,
) -> RemoteCallFn {
    Arc::new(
        move |peer: &str, action: &str, payload: serde_json::Value| {
            calls.lock().expect("calls lock").push((
                peer.to_string(),
                action.to_string(),
                payload.clone(),
            ));
            let resp = respond(peer, action, &payload);
            Box::pin(async move { resp })
        },
    )
}

/// 对端 worker 响应里的 executed 凭条（镜像测试的源形态）。
fn worker_receipt_json(outcome: &str, order_id: &str) -> serde_json::Value {
    serde_json::json!({
        "receipt_id": "wr-1770000000-abcd",
        "ts": "2026-10-15T00:00:00+00:00",
        "connector": "paper-broker",
        "op": "place_order",
        "risk": "critical",
        "class": "write",
        "outcome": outcome,
        "args_digest": "worker-side-digest",
        "summary": "买入 1 股 AAPL",
        "receipt_key": "order_id",
        "receipt_value": order_id,
        "undoable": false,
    })
}

#[tokio::test]
async fn test_remote_invoke_executed_mirrors_receipt_and_records_budget() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(ws, "paper-broker", MIXED_MANIFEST);
    let config = ws.join("config.json");
    write_finance_config(&config);
    let tool = invoke_tool(ws, &config);

    let calls: CapturedCalls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = calls.clone();
    tool.set_remote_call_fn(mock_remote_fn(calls, |_peer, _action, _payload| {
        Ok(serde_json::json!({
            "receipt": worker_receipt_json("executed", "NB-ORD-9"),
            "result_text": "{\"executed\": true, \"order_id\": \"NB-ORD-9\"}",
        }))
    }));

    let mut c = ctx();
    c.approval_ref = Some("appr-master-7".to_string());
    let (result, returned) = tool
        .execute_with_receipt(
            r#"{"connector":"paper-broker","op":"place_order","target_node":"node-b","args":{"side":"buy","symbol":"AAPL","qty":1,"price":100.0}}"#,
            &c,
        )
        .await;
    let out = result.expect("remote executed → Ok");
    assert!(out.contains("NB-ORD-9"), "worker text passthrough: {out}");
    assert!(out.contains("node-b"), "note names the target: {out}");

    // 载荷断言：action / peer / 无凭据字段（铁律）。
    let calls = captured.lock().expect("calls lock");
    assert_eq!(calls.len(), 1, "exactly one RPC dispatch");
    let (peer, action, payload) = &calls[0];
    assert_eq!(peer, "node-b");
    assert_eq!(action, "connector_invoke");
    let keys: Vec<&str> = payload
        .as_object()
        .expect("payload object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        vec!["args", "connector", "op", "session_key"],
        "payload 只含四键（凭据永不上 RPC 线）: {payload}"
    );
    assert_eq!(payload["connector"], "paper-broker");
    assert_eq!(payload["op"], "place_order");
    assert_eq!(payload["session_key"], "session-1");
    assert_eq!(payload["args"]["symbol"], "AAPL");
    drop(calls);

    // 镜像凭条：新 receipt_id（非对端的）、outcome/主键镜像、undoable=false。
    let r = returned.expect("mirror receipt returned");
    assert_ne!(r.receipt_id, "wr-1770000000-abcd", "master 自铸新凭条 id");
    assert_eq!(r.outcome, "executed");
    assert_eq!(r.receipt_key.as_deref(), Some("order_id"));
    assert_eq!(r.receipt_value.as_deref(), Some("NB-ORD-9"));
    assert!(!r.undoable, "远程凭条 undoable=false（跨节点撤销 v1 挂账）");
    assert!(
        r.summary.starts_with("[远程@node-b · wr-1770000000-abcd]"),
        "summary 前缀指认远程与对端凭条: {}",
        r.summary
    );
    assert_eq!(r.approval_ref.as_deref(), Some("appr-master-7"));
    assert_eq!(r.session_key.as_deref(), Some("session-1"));
    assert!(
        r.budget_consumed.as_ref().map(|b| b.ops_last_hour) == Some(1),
        "executed 才记 master 额度: {:?}",
        r.budget_consumed
    );

    // 落盘真相：账本里在册且额度已记。
    let receipts = list_receipts(ws, 50, None).expect("receipts");
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].outcome, "executed");
    assert_eq!(
        tool.budget
            .rate_used("paper-broker", chrono::Utc::now().timestamp_millis()),
        1
    );
}

#[tokio::test]
async fn test_remote_invoke_without_cluster_channel_is_honest_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(ws, "paper-broker", MIXED_MANIFEST);
    let tool = invoke_tool(ws, &ws.join("config.json"));

    // 未注入 remote_call_fn（集群未启用）= 请求不出线、未尝试执行 = 无回执。
    let err = tool
        .execute(
            r#"{"connector":"paper-broker","op":"positions","target_node":"node-b"}"#,
            &ctx(),
        )
        .await
        .expect_err("no cluster channel must error honestly");
    assert!(err.contains("未启用集群"), "msg: {err}");
    let receipts = list_receipts(ws, 50, None).expect("receipts");
    assert!(receipts.is_empty(), "未尝试执行 = 无回执: {receipts:?}");
}

#[tokio::test]
async fn test_remote_invoke_rpc_failure_writes_unknown_receipt() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(ws, "paper-broker", MIXED_MANIFEST);
    let config = ws.join("config.json");
    write_finance_config(&config);
    let tool = invoke_tool(ws, &config);
    tool.set_remote_call_fn(mock_remote_fn(
        Arc::new(std::sync::Mutex::new(Vec::new())),
        |_peer, _action, _payload| Err("connection refused".to_string()),
    ));

    let err = tool
        .execute(
            r#"{"connector":"paper-broker","op":"positions","target_node":"node-b"}"#,
            &ctx(),
        )
        .await
        .expect_err("RPC failure must error");
    assert!(err.contains("远程执行失败"), "msg: {err}");
    assert!(err.contains("connection refused"), "root cause: {err}");

    let receipts = list_receipts(ws, 50, None).expect("receipts");
    assert_eq!(receipts.len(), 1);
    // RPC 失败 = 结果未知（请求可能已送达并在 B 端执行），凭条词表与本地
    // 超时臂同源落 "unknown" 而非 "error"（undo 拒撤措辞依赖此区分）。
    assert_eq!(receipts[0].outcome, "unknown");
    assert!(
        receipts[0]
            .summary
            .contains("[远程@node-b] RPC 失败（结果未知）: connection refused"),
        "summary: {}",
        receipts[0].summary
    );
}

#[tokio::test]
async fn test_remote_invoke_budget_blocked_never_leaves_master() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(ws, "paper-broker", MIXED_MANIFEST);
    // 无 finance config = 资金 fail-closed（qty 全 0 拒）。
    let tool = invoke_tool(ws, &ws.join("config.json"));

    let calls: CapturedCalls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = calls.clone();
    tool.set_remote_call_fn(mock_remote_fn(calls, |_peer, _action, _payload| {
        panic!("预算被拒后请求不得出线")
    }));

    let err = tool
        .execute(
            r#"{"connector":"paper-broker","op":"place_order","target_node":"node-b","args":{"side":"buy","symbol":"AAPL","qty":1,"price":1.0}}"#,
            &ctx(),
        )
        .await
        .expect_err("fail-closed must reject before dispatch");
    assert!(err.contains("⛔ RATE LIMIT"), "msg: {err}");
    assert!(
        captured.lock().expect("calls lock").is_empty(),
        "被拒请求不得出线"
    );
    let receipts = list_receipts(ws, 50, None).expect("receipts");
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].outcome, "budget_blocked");
}

#[tokio::test]
async fn test_remote_invoke_denied_approval_is_transparent_ok_without_budget() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(ws, "paper-broker", MIXED_MANIFEST);
    let config = ws.join("config.json");
    write_finance_config(&config);
    let tool = invoke_tool(ws, &config);

    tool.set_remote_call_fn(mock_remote_fn(
        Arc::new(std::sync::Mutex::new(Vec::new())),
        |_peer, _action, _payload| {
            // 对端审批被拒（无人值守 300s 超时同形态）：⛔ 文本 + denied 回执。
            Ok(serde_json::json!({
                "receipt": worker_receipt_json("denied_approval", ""),
                "result_text": "⛔ SECURITY BLOCKED [layer:abac|policy:deny] 用户拒绝了该操作",
            }))
        },
    ));

    let out = tool
        .execute(
            r#"{"connector":"paper-broker","op":"place_order","target_node":"node-b","args":{"side":"buy","symbol":"AAPL","qty":1,"price":1.0}}"#,
            &ctx(),
        )
        .await
        .expect("denied_approval 与本地 DispatchRejected 同为 Ok 透传");
    assert!(out.contains("⛔ SECURITY BLOCKED"), "原文透传: {out}");
    assert!(!out.contains("Tool error"), "不得二次包装: {out}");

    // 镜像凭条 denied_approval + master 额度不记（deny 不占）。
    let receipts = list_receipts(ws, 50, None).expect("receipts");
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].outcome, "denied_approval");
    assert!(receipts[0].budget_consumed.is_none());
    assert_eq!(
        tool.budget
            .rate_used("paper-broker", chrono::Utc::now().timestamp_millis()),
        0
    );
}

#[tokio::test]
async fn test_remote_invoke_worker_error_text_prefix_stripped() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(ws, "paper-broker", MIXED_MANIFEST);
    let config = ws.join("config.json");
    write_finance_config(&config);
    let tool = invoke_tool(ws, &config);

    tool.set_remote_call_fn(mock_remote_fn(
        Arc::new(std::sync::Mutex::new(Vec::new())),
        |_peer, _action, _payload| {
            // 对端 error：dispatch 包装形态 "Tool error: ..."（worker handler
            // 恒 Ok 回传，result_text 带 dispatch 前缀）。
            Ok(serde_json::json!({
                "receipt": worker_receipt_json("error", ""),
                "result_text": "Tool error: worker blew up",
            }))
        },
    ));

    let err = tool
        .execute(
            r#"{"connector":"paper-broker","op":"positions","target_node":"node-b"}"#,
            &ctx(),
        )
        .await
        .expect_err("worker error → master Err");
    // 前缀还原：master 再回 Err 时 dispatch 会重新包一层 "Tool error: "——
    // 这里剥掉对端已带的前缀，最终文本不出现双前缀。
    assert!(err.starts_with("worker blew up"), "prefix stripped: {err}");
    assert!(
        !err.contains("Tool error: Tool error:"),
        "no doubling: {err}"
    );

    let receipts = list_receipts(ws, 50, None).expect("receipts");
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].outcome, "error");
    assert!(receipts[0].budget_consumed.is_none(), "error 不记额度");
}

/// 回执 envelope 缺 outcome = 结果未知（R3 决策表第四臂，2026-10-16 回检
/// 补口）：请求可能已送达并在 B 端执行——不得按 "error" 兜底走 release
/// 回吐预算（R3 语义旁路）。落 "unknown" 镜像凭条 + 预算占用保留。
#[tokio::test]
async fn test_remote_invoke_missing_outcome_retains_budget() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(ws, "paper-broker", MIXED_MANIFEST);
    let config = ws.join("config.json");
    write_finance_config(&config);
    let tool = invoke_tool(ws, &config);

    tool.set_remote_call_fn(mock_remote_fn(
        Arc::new(std::sync::Mutex::new(Vec::new())),
        |_peer, _action, _payload| {
            // envelope 不完整：worker 侧漏写 outcome 字段（同形态：旧节点 /
            // 手工构造的回执）。master 无法得知请求是否已被执行。
            let mut r = worker_receipt_json("executed", "ord-unknown-1");
            r.as_object_mut()
                .expect("receipt is object")
                .remove("outcome");
            Ok(serde_json::json!({
                "receipt": r,
                "result_text": "买入 1 股 AAPL",
            }))
        },
    ));

    // unknown 落决策表 `_` 臂 = 诚实失败回模型（不带 executed/denied_approval 语义）。
    let err = tool
        .execute(
            r#"{"connector":"paper-broker","op":"place_order","target_node":"node-b","args":{"side":"buy","symbol":"AAPL","qty":1,"price":1.0}}"#,
            &ctx(),
        )
        .await
        .expect_err("missing outcome must error honestly");
    assert!(err.contains("买入 1 股 AAPL"), "worker summary: {err}");

    let receipts = list_receipts(ws, 50, None).expect("receipts");
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].outcome, "unknown");
    assert!(receipts[0].budget_consumed.is_none());

    // 预算占用保留：rate_used 记 1（release 会归 0）、资金类占用不回吐。
    assert_eq!(
        tool.budget
            .rate_used("paper-broker", chrono::Utc::now().timestamp_millis()),
        1,
        "unknown 不得回吐（release = R3 语义旁路）"
    );
    let (orders, qty) = tool
        .budget
        .finance_today(chrono::Utc::now().timestamp_millis());
    assert_eq!(orders, 1, "资金类笔数保留");
    assert!((qty - 1.0).abs() < 1e-9, "资金类数量保留，实际 {qty}");
}

/// 对端 dispatch 层拒绝（estop 冻结族，T-CONN-1 ② 单测形态，2026-10-16
/// ET 根修）：worker 从未执行、凭条从未落账（receipt=null 诚实不造假），
/// result_text 带 ⛔ 前缀 = 确定性「未执行」——镜像 outcome=error（决策
/// 表 release 回吐预算），summary 带 `· ?]` 署名特征 + 拒因入账。与上一
/// 测试构成对照：缺 outcome 且无 ⛔ 标记 = unknown 保留，⛔ 标记 = error
/// 回吐。
#[tokio::test]
async fn test_remote_invoke_estop_refusal_no_receipt_maps_to_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(ws, "paper-broker", MIXED_MANIFEST);
    let config = ws.join("config.json");
    write_finance_config(&config);
    let tool = invoke_tool(ws, &config);

    tool.set_remote_call_fn(mock_remote_fn(
        Arc::new(std::sync::Mutex::new(Vec::new())),
        |_peer, _action, _payload| {
            // 急停期对端真实回包形态（gateway runtime.rs worker handler）：
            // 工具在 dispatch 层被拒（⛔ 前缀），凭条面无账（receipt=null）。
            Ok(serde_json::json!({
                "receipt": serde_json::Value::Null,
                "result_text": "⛔ ESTOP: 已急停，工具调用已拒绝",
            }))
        },
    ));

    let err = tool
        .execute(
            r#"{"connector":"paper-broker","op":"positions","target_node":"node-b"}"#,
            &ctx(),
        )
        .await
        .expect_err("对端明确拒绝 = 诚实失败回模型");
    assert!(err.contains("ESTOP"), "refusal text passthrough: {err}");

    let receipts = list_receipts(ws, 50, None).expect("receipts");
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].outcome, "error");
    assert!(
        receipts[0].summary.contains("· ?]"),
        "summary 无凭条署名特征: {}",
        receipts[0].summary
    );
    assert!(
        receipts[0].summary.contains("ESTOP"),
        "拒因入账（审计自解释）: {}",
        receipts[0].summary
    );
    assert!(receipts[0].budget_consumed.is_none(), "error 不记额度");

    // 预算回吐：error = 对端确认未执行（R3），rate_used 归 0。
    assert_eq!(
        tool.budget
            .rate_used("paper-broker", chrono::Utc::now().timestamp_millis()),
        0,
        "⛔ 拒绝 = 确定未执行，必须回吐（保留 = unknown 语义串臂）"
    );
}

/// F1（2026-10-16 二轮复查）回归：连接建立失败（connection refused）=
/// 请求**未发出** = 确定性未生效 → `is_connect()` 臂保持 Err → 决策表
/// Err 臂 release 回吐额度（rate_used 归 0）+ error 凭条。与上一测试
/// 构成对照：超时/传输中断 = unknown 保留占用，连接拒绝 = error 回吐。
/// 端口取法：bind(0) 拿端口后立即 drop listener → 内核释放端口 →
/// connect 立即被拒（非超时路径）。
#[tokio::test]
async fn test_connect_refused_releases_budget_and_writes_error_receipt() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let port = listener.local_addr().expect("local addr").port();
    drop(listener);

    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(
        ws,
        "dead-endpoint",
        &r#"
[connector]
id = "dead-endpoint"
name = "死端点"
version = "0.1.0"
kind = "rest"

[connector.http]
base = "http://127.0.0.1:{PORT}"

[[op]]
id = "push_thing"
risk = "high"
class = "write"
http = { method = "POST", path = "/things" }
"#
        .replace("{PORT}", &port.to_string()),
    );
    let config = ws.join("config.json");
    let tool = invoke_tool(ws, &config);

    let err = tool
        .execute(
            r#"{"connector":"dead-endpoint","op":"push_thing","args":{}}"#,
            &ctx(),
        )
        .await
        .expect_err("connection refused must error honestly");
    assert!(
        err.contains("HTTP 请求失败") || err.contains("connect"),
        "诚实错误文本: {err}"
    );

    // error 凭条 + 额度回吐（rate_used 归 0——误落 unknown 臂会是 1）。
    let receipts = list_receipts(ws, 50, None).expect("receipts");
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].outcome, "error");
    assert_eq!(
        tool.budget
            .rate_used("dead-endpoint", chrono::Utc::now().timestamp_millis()),
        0,
        "连接未建立 = 确定性未生效，release 必须回吐（对照 unknown 臂的保留）"
    );
}

// ===========================================================================
// R4：执行前 args 校验（出网前拒绝 + 零额度 + 零 HTTP + 资金形态强制）
// ===========================================================================

/// enum/required 越界在出网前拒绝：wiremock 零请求、零凭据依赖（闸在
/// 凭据解析之前）、无回执（动作从未发生——与 budget_blocked 的「发生过
/// 被拦」回执语义相区分）。
#[tokio::test]
async fn test_invalid_args_rejected_before_outbound_wiremock_zero_requests() {
    let server = wiremock::MockServer::start().await;
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    let template = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/workspace/connectors/gitea/connector.toml"
    ))
    .expect("template");
    write_connector(
        ws,
        "gitea",
        &template.replace(
            "http://127.0.0.1:3000/api/v1",
            &format!("{}/api/v1", server.uri()),
        ),
    );
    // 无 vault：闸在凭据解析之前，拒绝不依赖凭据在场。
    let tool = invoke_tool(ws, &ws.join("config.json"));

    // enum 越界（state 只许 "closed"）
    let err = tool
        .execute(
            r#"{"connector":"gitea","op":"issue_close","args":{"owner":"o","repo":"r","index":1,"state":"open"}}"#,
            &ctx(),
        )
        .await
        .expect_err("enum 越界必须拒");
    assert!(err.contains("⛔ INVALID ARGS"), "prefix: {err}");
    assert!(err.contains("state"), "指认字段: {err}");
    assert!(err.contains("允许范围"), "指认原因: {err}");
    assert!(err.contains("未执行，未消耗额度"), "诚实注记: {err}");

    // required 缺失
    let err = tool
        .execute(
            r#"{"connector":"gitea","op":"issue_close","args":{"owner":"o","repo":"r","state":"closed"}}"#,
            &ctx(),
        )
        .await
        .expect_err("required 缺失必须拒");
    assert!(err.contains("index"), "指认缺失字段: {err}");

    // 零 HTTP + 零回执：动作从未发生
    let received = server.received_requests().await;
    assert!(
        received.as_ref().is_none_or(|v| v.is_empty()),
        "非法参数不得出网"
    );
    assert!(
        list_receipts(ws, 50, Some("gitea"))
            .expect("receipts")
            .is_empty(),
        "未发生的动作无回执"
    );
}

/// 资金参数形态强制（R4）：finance op 的 qty 必须在场且为有限正数——
/// 缺失/null 由 required 闸拒，字符串/零/负数由形态闸拒；全部发生在
/// 预算 reserve 之前 = 零额度消耗 + 零资金动作 + 零回执。
#[tokio::test]
async fn test_finance_qty_forms_rejected_before_budget() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(ws, "paper-broker", MIXED_MANIFEST);
    let config = ws.join("config.json");
    write_finance_config(&config);
    let tool = invoke_tool(ws, &config);

    // (args JSON 片段, 期望错误里指认的内容)
    let cases: Vec<(&str, &str)> = vec![
        (r#"{"side":"buy","symbol":"AAPL","price":1.0}"#, "qty"), // 缺失 → required
        (
            r#"{"side":"buy","symbol":"AAPL","qty":null,"price":1.0}"#,
            "qty",
        ), // null 视为缺席
        (
            r#"{"side":"buy","symbol":"AAPL","qty":"10","price":1.0}"#,
            "number",
        ), // 字符串 → 形态闸
        (
            r#"{"side":"buy","symbol":"AAPL","qty":0,"price":1.0}"#,
            "正数",
        ),
        (
            r#"{"side":"buy","symbol":"AAPL","qty":-5,"price":1.0}"#,
            "正数",
        ),
    ];
    for (args, want) in cases {
        let call = format!(r#"{{"connector":"paper-broker","op":"place_order","args":{args}}}"#);
        let err = tool
            .execute(&call, &ctx())
            .await
            .expect_err("qty 非法形态必须拒");
        assert!(err.contains("⛔ INVALID ARGS"), "{args}: {err}");
        assert!(err.contains(want), "{args}: want `{want}` in {err}");
    }

    // 零额度 + 零资金动作 + 零回执
    let ledger = nemesis_connectors::ledger::load_ledger(ws).expect("ledger");
    assert!(ledger.orders.is_empty(), "无外部资金动作");
    assert!(list_receipts(ws, 50, None).expect("receipts").is_empty());
    assert_eq!(
        tool.budget
            .rate_used("paper-broker", chrono::Utc::now().timestamp_millis()),
        0,
        "拒绝发生在 reserve 之前，零额度消耗"
    );
}

/// R4 单点在远程分支之前：非法参数的远程调用不出线（mock RPC 零捕获）、
/// 零额度、零回执——与本地路径同覆盖。
#[tokio::test]
async fn test_remote_invoke_invalid_args_never_leaves_master() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(ws, "paper-broker", MIXED_MANIFEST);
    let config = ws.join("config.json");
    write_finance_config(&config);
    let tool = invoke_tool(ws, &config);

    let calls: CapturedCalls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = calls.clone();
    tool.set_remote_call_fn(mock_remote_fn(calls, |_peer, _action, _payload| {
        panic!("非法参数不得出线")
    }));

    // qty 字符串形态（required 在场、schema 无 properties → 类型闸不设限，
    // 资金形态闸兜底）
    let err = tool
        .execute(
            r#"{"connector":"paper-broker","op":"place_order","target_node":"node-b","args":{"side":"buy","symbol":"AAPL","qty":"10","price":1.0}}"#,
            &ctx(),
        )
        .await
        .expect_err("must reject before remote dispatch");
    assert!(err.contains("⛔ INVALID ARGS"), "msg: {err}");
    assert!(
        captured.lock().expect("calls lock").is_empty(),
        "非法参数不得出线"
    );
    assert!(list_receipts(ws, 50, None).expect("receipts").is_empty());
    assert_eq!(
        tool.budget
            .rate_used("paper-broker", chrono::Utc::now().timestamp_millis()),
        0
    );
}

// ---- E4（收官批 2026-10-15）monitor 语义变化检测 ----
// fetch 之后的全链（快照/比对/轮转/摘要分流）收在 monitor_finalize 纯函数，
// 单测不经 loop 直接驱动；LLM 真调归验收（PLAN §十 10：小模型通道三态）。

const MONITOR_MANIFEST: &str = r#"
[connector]
id = "monitor"
name = "网页监控"
version = "0.2.0"
kind = "local"
description = "test monitor"

[[op]]
id = "check"
intent = ["检查更新"]
risk = "low"
class = "network"
action = "builtin.monitor_check"
args_schema = { type = "object", required = ["url", "name"] }
"#;

fn snap_path(ws: &Path, name: &str) -> PathBuf {
    ws.join("data")
        .join("monitor_snapshots")
        .join(format!("{}.txt", monitor_slug(name)))
}

#[test]
fn test_monitor_finalize_semantic_off_full_chain() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();

    // 首跑：建基线，无比对结论；快照落盘。
    let d =
        monitor_finalize(ws, "首页", "https://a.example.com/x", false, "内容A").expect("baseline");
    assert!(matches!(d, MonitorDecision::Baseline { .. }));
    assert_eq!(
        std::fs::read_to_string(snap_path(ws, "首页")).expect("snap"),
        "内容A"
    );

    // 全等：无变化（零 token）；快照保持。
    let d =
        monitor_finalize(ws, "首页", "https://a.example.com/x", false, "内容A").expect("unchanged");
    assert!(matches!(d, MonitorDecision::Unchanged { .. }));

    // 变化 + semantic=false：机械比对（P0 旧行为），快照轮转到新内容。
    let d =
        monitor_finalize(ws, "首页", "https://a.example.com/x", false, "内容B").expect("changed");
    match d {
        MonitorDecision::ChangedMechanical { summary } => {
            assert!(summary.contains("semantic=false"), "{summary}");
        }
        other => panic!("expect ChangedMechanical, got {other:?}"),
    }
    assert_eq!(
        std::fs::read_to_string(snap_path(ws, "首页")).expect("snap"),
        "内容B"
    );

    // 再变回去：轮转可反复（每次都以最新内容为新基线）。
    let d = monitor_finalize(ws, "首页", "https://a.example.com/x", false, "内容A")
        .expect("changed again");
    assert!(matches!(d, MonitorDecision::ChangedMechanical { .. }));
    assert_eq!(
        std::fs::read_to_string(snap_path(ws, "首页")).expect("snap"),
        "内容A"
    );
}

#[test]
fn test_monitor_finalize_semantic_on_builds_prompt_and_rotates_first() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();

    let d = monitor_finalize(ws, "home", "https://a.example.com/y", true, "old body")
        .expect("baseline");
    assert!(matches!(d, MonitorDecision::Baseline { .. }));

    // semantic=true：变化 → ChangedNeedsSummary（prompt 带名称/URL/新旧两节）；
    // 快照在摘要**之前**已轮转（摘要失败也不丢新基线——行为契约）。
    let d =
        monitor_finalize(ws, "home", "https://a.example.com/y", true, "new body").expect("changed");
    match d {
        MonitorDecision::ChangedNeedsSummary { prompt } => {
            assert!(prompt.contains("home"), "{prompt}");
            assert!(prompt.contains("https://a.example.com/y"));
            assert!(prompt.contains("=== 旧内容 ==="));
            assert!(prompt.contains("old body"));
            assert!(prompt.contains("=== 新内容 ==="));
            assert!(prompt.contains("new body"));
        }
        other => panic!("expect ChangedNeedsSummary, got {other:?}"),
    }
    assert_eq!(
        std::fs::read_to_string(snap_path(ws, "home")).expect("snap"),
        "new body"
    );
}

#[test]
fn test_monitor_compare_clipped_heuristic_boundary() {
    // 诚实边界钉死：差异只落在截断点之后 = 视为无变化（比对在截断形态上）。
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    let base = "x".repeat(MONITOR_SNAPSHOT_MAX_CHARS);

    let d =
        monitor_finalize(ws, "k", "https://a", false, &format!("{base}TAIL")).expect("baseline");
    assert!(matches!(d, MonitorDecision::Baseline { .. }));

    // 截断点之后的差异 → Unchanged（启发式边界的实现形态）。
    let d = monitor_finalize(ws, "k", "https://a", false, &base).expect("clipped equal");
    assert!(matches!(d, MonitorDecision::Unchanged { .. }));

    // 截断点之内的差异 → changed 照常成立。
    let d = monitor_finalize(
        ws,
        "k",
        "https://a",
        false,
        &format!("{}y", "x".repeat(MONITOR_SNAPSHOT_MAX_CHARS - 1)),
    )
    .expect("within-cap diff");
    assert!(matches!(d, MonitorDecision::ChangedMechanical { .. }));
}

#[test]
fn test_monitor_slug_no_traversal_and_no_ascii_collision() {
    // 路径穿越载荷：sanitize 白名单摊平，文件不逃出快照目录。
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    monitor_finalize(ws, "../../evil", "https://a", false, "x").expect("baseline");
    let dir = ws.join("data").join("monitor_snapshots");
    let entries: Vec<_> = std::fs::read_dir(&dir)
        .expect("read_dir")
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(entries.len(), 1, "穿越名不得产生越界文件");
    assert!(!ws.join("evil.txt").exists());

    // 非 ASCII 碰撞分离：「首页」「仓库」摊平同名 "__" → 摘要 8 位分离。
    assert_ne!(monitor_slug("首页"), monitor_slug("仓库"));
    // ASCII 直通名保持干净形态。
    assert_eq!(monitor_slug("homepage-1"), "homepage-1");
}

#[tokio::test]
async fn test_monitor_check_execute_arg_gates_and_no_loop_honest_error() {
    // 经 execute 全链：缺 url / 缺 name 在 R4 schema 闸（出网前）被拒——
    // 不触达 builtin、不消耗额度、不落回执（动作从未发生）；带全参但无
    // loop（停机窗口）→ fetch 诚实失败，错误也是账。
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(ws, "monitor", MONITOR_MANIFEST);
    let config = ws.join("config.json");
    let tool = invoke_tool(ws, &config);

    let err = tool
        .execute(r#"{"connector":"monitor","op":"check","args":{}}"#, &ctx())
        .await
        .expect_err("缺 url 必须报错");
    assert!(err.contains("⛔ INVALID ARGS"), "{err}");
    assert!(err.contains("url"), "{err}");

    let err = tool
        .execute(
            r#"{"connector":"monitor","op":"check","args":{"url":"https://a.example.com"}}"#,
            &ctx(),
        )
        .await
        .expect_err("缺 name 必须报错");
    assert!(err.contains("⛔ INVALID ARGS"), "{err}");
    assert!(err.contains("name"), "{err}");

    let err = tool
        .execute(
            r#"{"connector":"monitor","op":"check","args":{"url":"https://a.example.com","name":"首页"}}"#,
            &ctx(),
        )
        .await
        .expect_err("无 loop（停机窗口）必须诚实失败");
    assert!(err.contains("agent loop unavailable"), "{err}");

    // schema 拒绝无回执（未发生）；loop 失败落 error 回执（错误也是账）。
    let receipts = list_receipts(ws, 10, Some("monitor")).expect("receipts");
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].outcome, "error");
}

#[tokio::test]
async fn test_monitor_check_unknown_action_message_lists_monitor_check() {
    // 未知动作的诚实报错带最新支持清单（含 builtin.monitor_check）。
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(
        ws,
        "bad-act",
        r#"
[connector]
id = "bad-act"
name = "坏动作"
version = "0.1.0"
kind = "local"

[[op]]
id = "go"
intent = ["测试"]
risk = "low"
class = "read"
action = "builtin.no_such_action"
"#,
    );
    let config = ws.join("config.json");
    let tool = invoke_tool(ws, &config);
    let err = tool
        .execute(r#"{"connector":"bad-act","op":"go","args":{}}"#, &ctx())
        .await
        .expect_err("未知动作必须报错");
    assert!(err.contains("builtin.monitor_check"), "{err}");
}

// ---------------------------------------------------------------------------
// R6：GET 剩余 args 合并发送（extra_query 此前收集后从未消费）
// ---------------------------------------------------------------------------

/// merge 纯函数五形态：普通参数在前保序、凭据同名时凭据胜（普通 args 不
/// 得覆盖凭据）、非同名凭据对追加在尾、无凭据 = 原样透传、凭据同名多对
/// 全部胜出。
#[test]
fn test_merge_query_pairs_credential_wins_and_preserves_order() {
    // 普通参数在前 + 凭据同名胜出 + 非同名凭据追加。
    let merged = merge_query_pairs(
        vec![
            ("state".to_string(), "closed".to_string()),
            ("api_key".to_string(), "from-args".to_string()),
            ("limit".to_string(), "5".to_string()),
        ],
        &[("api_key".to_string(), "vault-secret".to_string())],
    );
    assert_eq!(
        merged,
        vec![
            ("state".to_string(), "closed".to_string()),
            ("limit".to_string(), "5".to_string()),
            ("api_key".to_string(), "vault-secret".to_string()),
        ],
        "普通参数保序在前，同名 api_key 被剔除，凭据对胜出"
    );

    // 无凭据 = 原样透传（含重复键形态）。
    let merged = merge_query_pairs(
        vec![
            ("tag".to_string(), "a".to_string()),
            ("tag".to_string(), "b".to_string()),
        ],
        &[],
    );
    assert_eq!(merged.len(), 2, "无凭据不去重");

    // 凭据多对全胜（多个同名槽位极端形态）。
    let merged = merge_query_pairs(
        vec![("k".to_string(), "x".to_string())],
        &[
            ("k".to_string(), "v1".to_string()),
            ("k".to_string(), "v2".to_string()),
        ],
    );
    assert_eq!(
        merged,
        vec![
            ("k".to_string(), "v1".to_string()),
            ("k".to_string(), "v2".to_string()),
        ],
    );
}

/// wiremock e2e：剩余 args 进请求 query——中文 / 空格 / `&` 值（decode 后
/// 完整到达 = percent-encoding 正确）+ 既有查询串 URL 用 `&` 追加（src 与
/// tag 同时到达）。无凭据依赖，plain tokio::test。
#[tokio::test]
async fn test_http_get_extra_query_merged_and_encoded() {
    let server = wiremock::MockServer::start().await;
    // 匹配器宽松（method+path），严格断言放在 received URL 上——避免
    // query_param 匹配器对 form 编码（空格→+）的解码歧义，URL 原始字节
    // 才是唯一真相。
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/search"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({"hit": true})),
        )
        .mount(&server)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/feed"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({"items": [1]})),
        )
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(
        ws,
        "search-api",
        &r#"
[connector]
id = "search-api"
name = "Search"
version = "0.1.0"
kind = "rest"

[connector.http]
base = "{BASE}"

[[op]]
id = "search"
intent = ["搜索"]
risk = "low"
class = "read"
http = { method = "GET", path = "/search" }

[[op]]
id = "feed"
intent = ["订阅"]
risk = "low"
class = "read"
http = { method = "GET", path = "/feed?src=manifest" }
"#
        .replace("{BASE}", &server.uri()),
    );
    let config = ws.join("config.json");
    let tool = invoke_tool(ws, &config);

    let out = tool
        .execute(
            r#"{"connector":"search-api","op":"search","args":{"q":"中文 空格 & 符号","limit":5}}"#,
            &ctx(),
        )
        .await
        .expect("search ok");
    assert!(out.contains("hit"), "result: {out}");

    let out = tool
        .execute(
            r#"{"connector":"search-api","op":"feed","args":{"tag":"rust"}}"#,
            &ctx(),
        )
        .await
        .expect("feed ok");
    assert!(out.contains("items"), "result: {out}");

    // mock 宽松收下，严格断言在收到 URL 上：percent-encoding 正确（值内
    // `&` 编码不拆对、中文 UTF-8 percent 形态）、既有查询串用 `&` 追加。
    let received = server.received_requests().await.expect("received requests");
    assert_eq!(received.len(), 2, "两笔请求都到齐");
    let search_url = received[0].url.as_str();
    assert!(search_url.contains("limit=5"), "limit 在 URI: {search_url}");
    // 中文「中」= E4 B8 AD；空格 = +（form 编码）；值内 `&` = %26。
    assert!(
        search_url.contains("q=%E4%B8%AD%E6%96%87+%E7%A9%BA%E6%A0%BC+%26+%E7%AC%A6%E5%8F%B7"),
        "q 值三形态编码完整: {search_url}"
    );
    assert!(
        !search_url.contains("%2B"),
        "空格按 form 编码为 + 非 %20: {search_url}"
    );
    let feed_url = received[1].url.as_str();
    assert!(
        feed_url.contains("src=manifest") && feed_url.contains("tag=rust"),
        "既有查询串保留 + 追加参数: {feed_url}"
    );
    assert!(feed_url.contains('&'), "追加用 & 连接: {feed_url}");
}

/// R6 同名冲突端到端：凭据 query（vault 注入 `into = "query:api_key"`）
/// 与 args 同名 `api_key` 同场——wiremock 收到的必须只有 vault 值（普通
/// args 不得覆盖/伴随凭据出现）。GLOBAL_STATE_LOCK 形态同既有 vault e2e。
#[cfg(feature = "security")]
#[test]
fn test_http_query_credential_wins_over_same_name_arg() {
    let _env_guard = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(test_http_query_credential_wins_over_same_name_arg_inner());
}

#[cfg(feature = "security")]
async fn test_http_query_credential_wins_over_same_name_arg_inner() {
    use nemesis_security::vault::VaultStore;

    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/data"))
        .and(wiremock::matchers::query_param(
            "api_key",
            "vault-secret-999",
        ))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})),
        )
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(
        ws,
        "kv-api",
        &r#"
[connector]
id = "kv-api"
name = "KV"
version = "0.1.0"
kind = "rest"

[connector.http]
base = "{BASE}"

[connector.credentials]
key = { required = true, into = "query:api_key" }

[[op]]
id = "fetch"
intent = ["拉数据"]
risk = "low"
class = "read"
http = { method = "GET", path = "/data" }
"#
        .replace("{BASE}", &server.uri()),
    );

    unsafe { std::env::set_var("NEMESISBOT_VAULT_PASSPHRASE", "test-passphrase") };
    let vault_path = nemesis_path::resolve_vault_path_in_workspace(ws);
    let mut store = VaultStore::create(
        &vault_path,
        nemesis_security::vault::VaultMode::Argon2id,
        Some("test-passphrase"),
    )
    .expect("create vault");
    store
        .set(
            "connector/kv-api/key",
            "vault-secret-999",
            "connector",
            "测试 key",
        )
        .expect("set key");
    store.save().expect("save vault");

    let config = ws.join("config.json");
    let tool = invoke_tool(ws, &config);
    // args 带同名 api_key=from-args：merge 后凭据胜——唯一 api_key 对必须是
    // vault 值。
    let out = tool
        .execute(
            r#"{"connector":"kv-api","op":"fetch","args":{"api_key":"from-args","page":2}}"#,
            &ctx(),
        )
        .await
        .expect("fetch ok");
    // mock 的 query_param 匹配器即断言：只有 vault 值的 api_key 才命中 200。
    assert!(out.contains("ok"), "result: {out}");

    let received = server.received_requests().await.expect("received requests");
    assert_eq!(received.len(), 1);
    let url = received[0].url.as_str();
    assert!(
        url.contains("api_key=vault-secret-999"),
        "凭据在 URI: {url}"
    );
    assert!(
        !url.contains("from-args"),
        "普通 args 同名不得出现（凭据胜）: {url}"
    );
    assert!(url.contains("page=2"), "非同名普通参数照常发送: {url}");
    unsafe { std::env::remove_var("NEMESISBOT_VAULT_PASSPHRASE") };
}

// ---------------------------------------------------------------------------
// R7：工具桥上下文透传（ctx.clone 只改审计标签 + depth 透传）
// ---------------------------------------------------------------------------

/// R7 e2e：tool 绑定回灌的 ctx 必须是原 ctx 的 clone（只改审计标签
/// channel="connector-binding"）——correlation_id / tool_path_base /
/// async_callback 原样保留（此前 RequestContext::new 全置 None：档案管线
/// 任务经 connector 绑定调 write_file，相对 path 静默落工作副本之外）；
/// approval_ref 由桥内 dispatch 既有安全语义接管（tool_dispatch.rs 刻意
/// 覆盖——凭条只信本次调用真实发生的审批，透传 = 伪造背书）；原 ctx 不被
/// 修改；`set_invocation_depth` 注入的父深度透传给桥内 dispatch（经
/// connector 绑定的 spawn 嵌套限制不重置）；tool_path_base 的相对 path
/// 重写与直接执行同形（同一工作副本）。
#[tokio::test]
async fn test_tool_binding_preserves_context_and_propagates_depth() {
    use nemesis_agent::r#loop::{LlmMessage, LlmProvider, LlmResponse};
    use nemesis_agent::types::{AgentConfig, ChatOptions, ToolDefinition};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct NullProvider;
    #[async_trait::async_trait]
    impl LlmProvider for NullProvider {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<LlmMessage>,
            _options: Option<ChatOptions>,
            _tools: Vec<ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            unreachable!("tool binding 不触发 LLM")
        }
    }

    /// 桩 write_file（dispatch 的 PATH_ARG_TOOLS 名单认名不认实现——
    /// 重写照跑）：记录收到的 ctx / args / 注入深度。
    struct SpyWriteFile {
        seen_ctx: Arc<std::sync::Mutex<Option<RequestContext>>>,
        seen_args: Arc<std::sync::Mutex<String>>,
        seen_depth: Arc<AtomicUsize>,
    }
    #[async_trait::async_trait]
    impl Tool for SpyWriteFile {
        fn set_invocation_depth(&self, depth: usize) {
            self.seen_depth.store(depth, Ordering::SeqCst);
        }
        async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String> {
            *self.seen_ctx.lock().unwrap() = Some(context.clone());
            *self.seen_args.lock().unwrap() = args.to_string();
            Ok("done".to_string())
        }
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    let workcopy = ws.join("workcopy");
    std::fs::create_dir_all(&workcopy).expect("workcopy");
    write_connector(
        ws,
        "archiver",
        r#"
[connector]
id = "archiver"
name = "Archiver"
version = "0.1.0"
kind = "local"

[[op]]
id = "snapshot_file"
intent = ["写快照"]
risk = "low"
class = "write"
tool = "write_file"
"#,
    );

    let seen_ctx = Arc::new(std::sync::Mutex::new(None));
    let seen_args = Arc::new(std::sync::Mutex::new(String::new()));
    let seen_depth = Arc::new(AtomicUsize::new(usize::MAX));
    let mut agent_loop = nemesis_agent::r#loop::AgentLoop::new(
        Box::new(NullProvider),
        AgentConfig {
            model: "test-model".to_string(),
            max_turns: 1,
            ..Default::default()
        },
    );
    agent_loop.register_tool(
        "write_file".to_string(),
        Box::new(SpyWriteFile {
            seen_ctx: seen_ctx.clone(),
            seen_args: seen_args.clone(),
            seen_depth: seen_depth.clone(),
        }),
    );
    let loop_arc = Arc::new(agent_loop);

    let tool = invoke_tool(ws, &ws.join("config.json"));
    tool.set_loop(Arc::downgrade(&loop_arc));

    // 原 ctx：带全量四字段（档案管线任务形态）。
    let mut original = RequestContext::new("web", "chat-1", "user-1", "session-1");
    original.correlation_id = Some("cid-R7".to_string());
    original.tool_path_base = Some(workcopy.clone());
    original.approval_ref = Some("appr-R7".to_string());

    // depth 注入（模拟 dispatch set_invocation_depth 形态，spawn_detached_tests 同先例）。
    tool.set_invocation_depth(2);

    let out = tool
        .execute(
            r#"{"connector":"archiver","op":"snapshot_file","args":{"path":"out.txt","content":"x"}}"#,
            &original,
        )
        .await
        .expect("tool binding ok");
    assert!(out.contains("done"), "result: {out}");

    // 桥内 spy 收到的 ctx：审计标签换位 + 四字段全保留。
    let ctx_seen = seen_ctx.lock().unwrap().clone().expect("spy saw ctx");
    assert_eq!(ctx_seen.channel, "connector-binding", "审计标签换位");
    assert_eq!(ctx_seen.chat_id, "chat-1");
    assert_eq!(ctx_seen.session_key, "session-1");
    assert_eq!(
        ctx_seen.correlation_id.as_deref(),
        Some("cid-R7"),
        "correlation_id 跨桥保留"
    );
    assert_eq!(
        ctx_seen.tool_path_base.as_deref(),
        Some(workcopy.as_path()),
        "tool_path_base 跨桥保留"
    );
    // approval_ref 不透传是 dispatch 既有安全语义（tool_dispatch.rs L781
    // 刻意覆盖：凭条只信「本次调用真实发生的交互审批」，透传上游引用 =
    // 伪造背书）——桥内调用未触发审批 → spy 收到 None。
    assert!(
        ctx_seen.approval_ref.is_none(),
        "approval_ref 由桥内 dispatch 语义接管（防伪造背书），不透传上游引用"
    );

    // 相对 path 被重写进工作副本（与直接执行同形）。
    let args_seen = seen_args.lock().unwrap().clone();
    let args_v: serde_json::Value = serde_json::from_str(&args_seen).expect("spy args json");
    let rewritten = args_v["path"].as_str().expect("path str");
    assert!(
        std::path::Path::new(rewritten).is_absolute()
            && rewritten.contains("workcopy")
            && rewritten.contains("out.txt"),
        "相对 path 重写进工作副本: {rewritten}"
    );

    // depth 透传：桥内 dispatch 收到 depth=2。
    assert_eq!(seen_depth.load(Ordering::SeqCst), 2, "父深度透传桥内");

    // 原 ctx 不被修改。
    assert_eq!(original.channel, "web", "原 ctx channel 不变");
    assert_eq!(original.correlation_id.as_deref(), Some("cid-R7"));
    assert_eq!(original.tool_path_base.as_deref(), Some(workcopy.as_path()));
    assert_eq!(original.approval_ref.as_deref(), Some("appr-R7"));

    // 对照臂：直接执行（不经 connector 桥）同 ctx 同 args——重写落点一致。
    *seen_ctx.lock().unwrap() = None;
    *seen_args.lock().unwrap() = String::new();
    let direct = loop_arc
        .handle_tool_call_at_depth(
            &nemesis_agent::ToolCallInfo {
                id: "direct-1".to_string(),
                name: "write_file".to_string(),
                arguments: r#"{"path":"out.txt","content":"x"}"#.to_string(),
            },
            &original,
            0,
        )
        .await;
    assert!(direct.contains("done"), "direct result: {direct}");
    let direct_args = seen_args.lock().unwrap().clone();
    let direct_v: serde_json::Value = serde_json::from_str(&direct_args).expect("direct args json");
    assert_eq!(
        direct_v["path"].as_str(),
        args_v["path"].as_str(),
        "直接执行与经桥执行落同一工作副本"
    );
}

// ---------------------------------------------------------------------------
// M1 信任徽标消费面（2026-10-19）：require_signed 闸 / 审批卡警示 / find 徽标
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_require_signed_gate_blocks_unsigned_with_trust_blocked_receipt() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(ws, "paper-broker", MIXED_MANIFEST);
    let config = ws.join("config.json");
    std::fs::write(
        &config,
        r#"{
  "connectors": {
    "enabled": true,
    "max_ops_per_hour": 10,
    "finance": { "max_order_qty": 100.0, "max_orders_per_day": 5 },
    "require_signed": true
  }
}"#,
    )
    .expect("write config");
    let tool = invoke_tool(ws, &config);

    // unsigned + require_signed=true → 出网前拒（本地/远程同闸单点）
    let err = tool
        .execute(
            r#"{"connector":"paper-broker","op":"place_order","args":{"side":"buy","symbol":"AAPL","qty":1,"price":1.0}}"#,
            &ctx(),
        )
        .await
        .expect_err("unsigned must be rejected");
    assert!(err.contains("⛔ TRUST"), "prefix: {err}");
    assert!(
        err.contains("require_signed"),
        "denial names the gate: {err}"
    );

    // trust_blocked 诚实凭条（拒绝有账，额度未消耗——budget_consumed 不在场）
    let receipts = list_receipts(ws, 50, None).expect("receipts");
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].outcome, "trust_blocked");
    assert!(receipts[0].budget_consumed.is_none(), "额度未消耗");
    // 账本零变动
    let ledger = nemesis_connectors::ledger::load_ledger(ws).expect("ledger");
    assert!(ledger.orders.is_empty());

    // 对照臂：同一 unsigned connector，缺省 config（require_signed 缺省
    // false）→ 不拦截，action 正常执行。
    let config2 = ws.join("config2.json");
    write_finance_config(&config2);
    let tool2 = invoke_tool(ws, &config2);
    let out = tool2
        .execute(
            r#"{"connector":"paper-broker","op":"place_order","args":{"side":"buy","symbol":"AAPL","qty":1,"price":1.0}}"#,
            &ctx(),
        )
        .await
        .expect("default config must not gate");
    assert!(out.contains("NB-ORD-1"), "order ok: {out}");
}

#[tokio::test]
async fn test_require_signed_gate_covers_remote_branch_too() {
    // 远程分支（target_node 在场）同样被闸——单点在远程分派之前。
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(ws, "paper-broker", MIXED_MANIFEST);
    let config = ws.join("config.json");
    std::fs::write(
        &config,
        r#"{ "connectors": { "enabled": true, "require_signed": true } }"#,
    )
    .expect("write config");
    let tool = invoke_tool(ws, &config);

    let err = tool
        .execute(
            r#"{"connector":"paper-broker","op":"quote","args":{"symbol":"AAPL"},"target_node":"Node-B"}"#,
            &ctx(),
        )
        .await
        .expect_err("remote branch must be gated too");
    assert!(err.contains("⛔ TRUST"), "msg: {err}");
    let receipts = list_receipts(ws, 50, None).expect("receipts");
    assert_eq!(receipts[0].outcome, "trust_blocked");
}

#[tokio::test]
async fn test_operation_override_trust_warning_unsigned_high_risk() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(ws, "paper-broker", MIXED_MANIFEST);
    let tool = invoke_tool(ws, &ws.join("config.json"));

    // unsigned + critical(confirm=required)：confirm 理由 + 警示行同卡
    let o = tool
        .operation_override(r#"{"connector":"paper-broker","op":"place_order","args":{}}"#)
        .expect("override projected");
    let f = o.force_approval.expect("confirm 必强审批");
    assert!(f.contains("confirm=required"), "confirm 理由在前: {f}");
    assert!(f.contains("未经签名验证"), "M1 警示并入: {f}");

    // unsigned + low/read：零警示零强审批（未签名读操作不值得弹卡）
    let o_low = tool
        .operation_override(r#"{"connector":"paper-broker","op":"quote","args":{}}"#)
        .expect("override projected");
    assert!(o_low.force_approval.is_none(), "low+unsigned 不弹卡");

    // 对照臂：签名 + 信任库收录 → verified，警示消失。
    let dir = ws.join("connectors").join("paper-broker");
    let pair = nemesis_connectors::signing::generate_key_pair().expect("keygen");
    let store = nemesis_connectors::signing::workspace_trust_store(ws);
    store.add_key(
        &pair.public_key,
        "test-signer",
        nemesis_connectors::signing::TrustLevel::Verified,
    );
    nemesis_connectors::signing::sign_connector_dir(&dir, &pair.private_key).expect("sign");
    let o_signed = tool
        .operation_override(r#"{"connector":"paper-broker","op":"place_order","args":{}}"#)
        .expect("override projected");
    let f_signed = o_signed.force_approval.expect("confirm 仍在");
    assert!(
        !f_signed.contains("未经签名验证"),
        "verified 后警示必须消失: {f_signed}"
    );
}

#[cfg(feature = "security")]
#[tokio::test]
async fn test_find_emits_trust_field() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(ws, "paper-broker", MIXED_MANIFEST);
    let tool = ConnectorFindTool::new(ws.to_path_buf());
    let out = tool.execute("{}", &ctx()).await.expect("find ok");
    assert!(
        out.contains("\"trust\": \"unsigned\""),
        "find 结果必须携带信任徽标: {out}"
    );
}

// ---- M2（2026-10-19）script 绑定 e2e（真 AgentLoop + 真 ExecTool）----

/// script 连接器清单（平台感知：慢命令臂 Windows=ping / Unix=sleep）。
/// `clean` op 的命令故意撞命令守卫黑名单（rm_rf）——deny 臂用。
fn script_manifest() -> String {
    let slow = if cfg!(windows) {
        "ping -n 3 127.0.0.1"
    } else {
        "sleep 2"
    };
    format!(
        r#"
[connector]
id = "script-demo"
name = "脚本连接器"
version = "0.1.0"
kind = "local"
description = "script 绑定 e2e"

[[op]]
id = "greet"
intent = ["打招呼"]
risk = "low"
class = "exec"
script = {{ command = "echo hello-{{name}}", capture = 8 }}
args_schema = {{ type = "object", properties = {{ name = {{ type = "string" }} }} }}

[[op]]
id = "slow"
intent = ["慢命令"]
risk = "low"
class = "exec"
script = {{ command = "{slow}", timeout_secs = 1 }}

[[op]]
id = "clean"
intent = ["清理文件"]
risk = "high"
class = "exec"
script = {{ command = "rm -rf marker.txt" }}
"#
    )
}

/// 裸 AgentLoop + 真注册 `exec` 工具：script 绑定经 `run_tool_binding` 走
/// 完整 dispatch 瀑布到 ExecTool——与生产 tool 绑定同路（不是直连执行器）。
fn loop_with_exec(ws: &Path) -> Arc<nemesis_agent::r#loop::AgentLoop> {
    use nemesis_agent::r#loop::{LlmMessage, LlmProvider, LlmResponse};
    use nemesis_agent::types::{AgentConfig, ChatOptions, ToolDefinition};

    struct NullProvider;
    #[async_trait::async_trait]
    impl LlmProvider for NullProvider {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<LlmMessage>,
            _options: Option<ChatOptions>,
            _tools: Vec<ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            unreachable!("script 绑定不触发 LLM")
        }
    }

    let mut agent_loop = nemesis_agent::r#loop::AgentLoop::new(
        Box::new(NullProvider),
        AgentConfig {
            model: "test-model".to_string(),
            max_turns: 1,
            ..Default::default()
        },
    );
    let ws_str = ws.to_string_lossy().to_string();
    agent_loop.register_tool(
        "exec".to_string(),
        Box::new(nemesis_agent::loop_tools::ExecTool::new(&ws_str, false)),
    );
    Arc::new(agent_loop)
}

/// 臂①：正常执行 + 占位符单遍展开 + capture 尾部截断进凭条 summary。
#[tokio::test]
async fn test_script_binding_executes_with_placeholder_expansion_and_capture() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(ws, "script-demo", &script_manifest());
    let tool = invoke_tool(ws, &ws.join("config.json"));
    let loop_arc = loop_with_exec(ws);
    tool.set_loop(Arc::downgrade(&loop_arc));

    let out = tool
        .execute(
            r#"{"connector":"script-demo","op":"greet","args":{"name":"e2e"}}"#,
            &ctx(),
        )
        .await
        .expect("script ok");
    assert!(out.contains("hello-e2e"), "占位符展开: {out}");

    let receipts = list_receipts(ws, 50, None).expect("receipts");
    assert_eq!(receipts.len(), 1);
    let r = &receipts[0];
    assert_eq!(r.outcome, "executed");
    // capture=8：exec 原样回传 stdout（含行尾 \r\n），summary = UTF-8 尾部
    // 8 字节（ASCII 硬截断）。
    let full = "hello-e2e\r\n";
    assert_eq!(r.summary, &full[full.len() - 8..], "capture 尾部截断进凭条");
    assert!(r.budget_consumed.is_some(), "executed 记账随行");
}

/// 臂②：超时诚实——ExecTool 超时收尸文本（无错误前缀）必须落 unknown
/// 凭条（结果未知，可能已执行了一部分），绝不落 executed；模型侧收到
/// 诚实 Err。回归锁：classify_tool_result 扩面前该文本落 Done = 假账。
#[tokio::test]
async fn test_script_binding_timeout_is_unknown_outcome_not_executed() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(ws, "script-demo", &script_manifest());
    let tool = invoke_tool(ws, &ws.join("config.json"));
    let loop_arc = loop_with_exec(ws);
    tool.set_loop(Arc::downgrade(&loop_arc));

    let err = tool
        .execute(r#"{"connector":"script-demo","op":"slow"}"#, &ctx())
        .await
        .expect_err("超时必须诚实报错");
    assert!(err.contains("timed out"), "超时原文透传: {err}");

    let receipts = list_receipts(ws, 50, None).expect("receipts");
    assert_eq!(receipts.len(), 1);
    assert_eq!(
        receipts[0].outcome, "unknown",
        "超时=结果未知（unknown），绝不是 executed"
    );
}

/// 臂③（cfg security）：命令守卫拦截路径——script 展开后的命令经 dispatch
/// 瀑布过安全 8 层（Layer 2 command guard），撞 rm_rf 黑名单 = ⛔ 拒绝，
/// 零副作用（目标文件仍在）+ 零 executed 凭条。script 绑定不是绕过命令
/// 守卫的后门的反向证明。
#[cfg(feature = "security")]
#[tokio::test]
async fn test_script_binding_command_guard_blocks_with_zero_side_effect() {
    use nemesis_agent::r#loop::{LlmMessage, LlmProvider, LlmResponse};
    use nemesis_agent::types::{AgentConfig, ChatOptions, ToolDefinition};
    use nemesis_security::pipeline::{SecurityPlugin, SecurityPluginConfig};

    struct NullProvider;
    #[async_trait::async_trait]
    impl LlmProvider for NullProvider {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<LlmMessage>,
            _options: Option<ChatOptions>,
            _tools: Vec<ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            unreachable!("script 绑定不触发 LLM")
        }
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(ws, "script-demo", &script_manifest());
    let marker = ws.join("marker.txt");
    std::fs::write(&marker, "keep").expect("marker");

    let tool = invoke_tool(ws, &ws.join("config.json"));
    let mut agent_loop = nemesis_agent::r#loop::AgentLoop::new(
        Box::new(NullProvider),
        AgentConfig {
            model: "test-model".to_string(),
            max_turns: 1,
            ..Default::default()
        },
    );
    let ws_str = ws.to_string_lossy().to_string();
    agent_loop.register_tool(
        "exec".to_string(),
        Box::new(nemesis_agent::loop_tools::ExecTool::new(&ws_str, false)),
    );
    // 真 SecurityPlugin（默认配置 = command guard enabled）：dispatch 瀑布
    // Layer 2 对 ProcessExec 做黑名单扫描。
    agent_loop.set_security_plugin(Arc::new(SecurityPlugin::new(
        SecurityPluginConfig::default(),
    )));
    let loop_arc = Arc::new(agent_loop);
    tool.set_loop(Arc::downgrade(&loop_arc));

    let out = tool
        .execute(r#"{"connector":"script-demo","op":"clean"}"#, &ctx())
        .await
        .expect("⛔ 拒绝以 Ok 透传（DispatchRejected 家族）");
    assert!(out.contains("⛔"), "命令守卫拒绝原文透传: {out}");

    // 零副作用：rm -rf 从未执行（文件仍在）。
    assert_eq!(
        std::fs::read_to_string(&marker).expect("marker 未被删"),
        "keep",
        "命令守卫拦截 = 命令从未运行"
    );
    // 零 executed 凭条（拒了就是拒了，不造假账）。
    let receipts = list_receipts(ws, 50, None).expect("receipts");
    assert!(
        receipts.iter().all(|r| r.outcome != "executed"),
        "拦截不得落 executed: {:?}",
        receipts
            .iter()
            .map(|r| r.outcome.as_str())
            .collect::<Vec<_>>()
    );
}

/// 臂④：config `connectors.script.default_timeout_secs = 0` = 全局禁用
/// script op（绑定分派层 Err——错误也是账：error 凭条 + 额度回吐）。
#[tokio::test]
async fn test_script_binding_config_zero_disables_script_ops() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(ws, "script-demo", &script_manifest());
    let config = ws.join("config.json");
    std::fs::write(
        &config,
        r#"{"connectors":{"enabled":true,"script":{"default_timeout_secs":0}}}"#,
    )
    .expect("config");
    let tool = invoke_tool(ws, &config);
    let loop_arc = loop_with_exec(ws);
    tool.set_loop(Arc::downgrade(&loop_arc));

    let err = tool
        .execute(
            r#"{"connector":"script-demo","op":"greet","args":{"name":"x"}}"#,
            &ctx(),
        )
        .await
        .expect_err("0 = 全拒");
    assert!(err.contains("script 执行被配置禁用"), "{err}");
    // 配置闸在绑定分派层拒（dispatch Err 臂）——错误也是账：error 凭条 +
    // 额度回吐，绝不落 executed。
    let receipts = list_receipts(ws, 50, None).expect("receipts");
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].outcome, "error");
    assert!(receipts[0].summary.contains("script 执行被配置禁用"));
    assert!(receipts[0].budget_consumed.is_none(), "拒绝 = 额度回吐");
}

/// 臂⑤：缺占位符实参（R4）——审批/预算之前响亮拒绝，零凭条零额度。
#[tokio::test]
async fn test_script_binding_missing_placeholder_arg_rejected_before_budget() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(ws, "script-demo", &script_manifest());
    let tool = invoke_tool(ws, &ws.join("config.json"));
    let loop_arc = loop_with_exec(ws);
    tool.set_loop(Arc::downgrade(&loop_arc));

    let err = tool
        .execute(
            r#"{"connector":"script-demo","op":"greet","args":{}}"#,
            &ctx(),
        )
        .await
        .expect_err("缺实参必须拒");
    assert!(
        err.contains("⛔ INVALID ARGS") && err.contains("占位符 `name` 缺实参"),
        "{err}"
    );
    assert!(
        list_receipts(ws, 50, None).expect("receipts").is_empty(),
        "预算/审批前拒绝 = 零凭条"
    );
}

/// 纯函数单测：expand_script_command 单遍扫描——值内 `{xxx}` 字面量不得
/// 二次展开（注入预防）；非白名单占位符保留字面；非 string 值 JSON 化；
/// 多字节字符跨扫描不劈。
#[test]
fn test_expand_script_command_single_pass_semantics() {
    let ph = |names: &[&str]| names.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    // 值里带 {name} 字面量：单遍扫描不回头，绝不二次展开
    let out = expand_script_command(
        "echo {name}",
        &ph(&["name"]),
        &serde_json::json!({"name": "{name}"}),
    )
    .expect("ok");
    assert_eq!(out, "echo {name}", "值内占位符字面量不二次展开");
    // 非白名单 {xxx} 保留字面
    let out = expand_script_command(
        "echo {unk} {name}",
        &ph(&["name"]),
        &serde_json::json!({"name": "v"}),
    )
    .expect("ok");
    assert_eq!(out, "echo {unk} v");
    // 缺实参 = Err
    assert!(expand_script_command("echo {name}", &ph(&["name"]), &serde_json::json!({})).is_err());
    // 非 string 值 JSON 序列化形态
    let out =
        expand_script_command("n={n}", &ph(&["n"]), &serde_json::json!({"n": 42})).expect("ok");
    assert_eq!(out, "n=42");
    // 多字节字符逐字符推进不劈 UTF-8
    let out = expand_script_command(
        "echo 你好{name}",
        &ph(&["name"]),
        &serde_json::json!({"name": "界"}),
    )
    .expect("ok");
    assert_eq!(out, "echo 你好界");
}

/// utf8_tail 字符边界对齐：切点向后找边界，宁短不劈半个字符。
#[test]
fn test_utf8_tail_char_boundary() {
    assert_eq!(utf8_tail("hello", 10), "hello");
    assert_eq!(utf8_tail("hello", 5), "hello");
    assert_eq!(utf8_tail("hello", 3), "llo");
    // "ab你"（5 字节）：tail 3 → 边界起点 2 = "你"；tail 4 → "b你"
    assert_eq!(utf8_tail("ab你", 3), "你");
    assert_eq!(utf8_tail("ab你", 4), "b你");
    // tail < 单个多字节字符 = 空串（宁短不劈）
    assert_eq!(utf8_tail("ab你", 2), "");
    assert_eq!(utf8_tail("ab你", 1), "");
}

// ---- M4b（2026-10-19）：desktop-ui 模板 connector（桌面只读面，window-mcp）----

/// 示例模板（examples/connectors/，A3 起不随 onboard 提取）恒合法 + **默认停用** +
/// 只读三 op 全 low/read 无强授权卡 +
/// 全 tool 绑定 window-mcp + 交互写入/落盘截图不声明（web-ui 同款防线）。
#[test]
fn test_workspace_template_desktopui_manifest_is_valid_and_disabled_by_default() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../examples/connectors/desktop-ui/connector.toml"
    );
    let text = std::fs::read_to_string(path).expect("desktop-ui 模板存在");
    let c = nemesis_connectors::manifest::parse_connector_manifest(&text)
        .expect("模板 manifest 必须恒合法");
    assert_eq!(c.id, "desktop-ui");
    // 只声明不默认开：桌面只读面是用户显式开启的能力。
    assert!(!c.enabled, "desktop-ui 模板必须默认停用（enabled = false）");
    let ids: Vec<&str> = c.ops.iter().map(|o| o.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["list_windows", "find_window", "window_text"],
        "只读三 op：枚举 / 查找 / 文本提取"
    );
    // 读面：全部 low/read 直通、无强授权卡（模板定位就是只读面）。
    for op in &c.ops {
        assert_eq!(op.risk, RiskLevel::Low, "{}", op.id);
        assert_eq!(op.class, OpClass::Read, "{}", op.id);
        assert!(!op.confirm_required, "{} 不应要强授权卡", op.id);
    }
    // 全部 tool 绑定到 window-mcp server（前缀对齐 adapter 注册形态
    // mcp_{server}_{tool}）；无 receipt 主键（MCP 扁平化文本，凭条走首行
    // summary——web-ui 同款裁定）。
    for op in &c.ops {
        match &op.binding {
            nemesis_connectors::manifest::OpBinding::Tool { tool } => {
                assert!(
                    tool.starts_with("mcp_window_"),
                    "{op_id} 绑定 {tool} 应指向 window-mcp",
                    op_id = op.id
                );
            }
            other => panic!("{} 应为 tool 绑定，实际 {other:?}", op.id),
        }
        assert!(op.receipt_key.is_none(), "{} 不声明 receipt", op.id);
    }
    // 越界面：交互写入与落盘截图不声明（不存在即安全）。
    let bound: Vec<&str> = c
        .ops
        .iter()
        .filter_map(|o| match &o.binding {
            nemesis_connectors::manifest::OpBinding::Tool { tool } => Some(tool.as_str()),
            _ => None,
        })
        .collect();
    for dangerous in [
        "click_window",
        "click_at",
        "type_text",
        "send_key_to_window",
        "capture_screenshot_to_file",
    ] {
        assert!(
            !bound.iter().any(|t| t.contains(dangerous)),
            "模板不得声明危险工具 {dangerous}"
        );
    }
}

// ---------------------------------------------------------------------------
// B2（2026-10-19 独占接管）：镜像 op 基线投影 / 执行 / 例外 / find 枚举
// ---------------------------------------------------------------------------

/// 独占接管 connector 模板（接管 `stub` server；显式 op 一个，其余走镜像）。
const EXCLUSIVE_MANIFEST: &str = r#"
[connector]
id = "excl"
name = "独占壳"
version = "0.1.0"
kind = "local"
exclusive_mcp = "stub"
description = "B2 镜像验证"

[[op]]
id = "noop"
intent = ["空转"]
risk = "low"
class = "read"
action = "builtin.noop"
"#;

/// 桩 MCP 工具（同 tool 绑定 e2e 形态：JSON 返回带主键）。
struct StubPageTool;
#[async_trait::async_trait]
impl Tool for StubPageTool {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok(r#"{"status":"ok","title":"Example Domain"}"#.to_string())
    }
}

/// 真 AgentLoop + 注册桩工具 + 直灌 MCP 工具快照（register_tool 不刷快照
/// ——快照是 MCP 注册/reload 时整表重建的，测试按生产数据形态直灌）。
fn loop_with_snapshot(
    names: &[(&str, serde_json::Value)],
) -> Arc<nemesis_agent::r#loop::AgentLoop> {
    let entries: Vec<(&str, serde_json::Value, Option<bool>, Option<bool>)> = names
        .iter()
        .map(|(n, p)| (*n, p.clone(), None, None))
        .collect();
    loop_with_snapshot_hints(entries.as_slice())
}

/// B3：带 annotations hints 的变体（description 也随参——A4 检索词元测
/// 需要英文词根描述）。
fn loop_with_snapshot_hints(
    entries: &[(&str, serde_json::Value, Option<bool>, Option<bool>)],
) -> Arc<nemesis_agent::r#loop::AgentLoop> {
    loop_with_snapshot_full(entries, &[])
}

/// 全参形态：hints + 每工具独立 description。
fn loop_with_snapshot_full(
    entries: &[(&str, serde_json::Value, Option<bool>, Option<bool>)],
    descriptions: &[&str],
) -> Arc<nemesis_agent::r#loop::AgentLoop> {
    use nemesis_agent::r#loop::{LlmMessage, LlmProvider, LlmResponse};
    use nemesis_agent::types::{AgentConfig, ChatOptions, ToolDefinition};

    struct NullProvider;
    #[async_trait::async_trait]
    impl LlmProvider for NullProvider {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<LlmMessage>,
            _options: Option<ChatOptions>,
            _tools: Vec<ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            unreachable!("镜像验证不触发 LLM")
        }
    }

    let mut agent_loop = nemesis_agent::r#loop::AgentLoop::new(
        Box::new(NullProvider),
        AgentConfig {
            model: "test-model".to_string(),
            max_turns: 1,
            ..Default::default()
        },
    );
    agent_loop.register_tool("mcp_stub__read_page".to_string(), Box::new(StubPageTool));
    {
        let snapshot = agent_loop.mcp_tool_snapshot();
        let mut snap = snapshot.write();
        for (i, (name, params, ro, destr)) in entries.iter().enumerate() {
            snap.push(nemesis_agent::r#loop::McpToolSnapshotEntry {
                name: name.to_string(),
                description: descriptions
                    .get(i)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("{name} 描述")),
                parameters: params.clone(),
                read_only_hint: *ro,
                destructive_hint: *destr,
            });
        }
    }
    Arc::new(agent_loop)
}

/// operation_override 镜像分支：基线投影 registry_read 档、无强制审批、
/// receipt_ctx 记 medium/read。快照缺席的名字不镜像（None → 静态
/// fail-closed 兜底）。
#[cfg(feature = "security")]
#[tokio::test]
async fn test_operation_override_mirror_baseline_projection() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(ws, "excl", EXCLUSIVE_MANIFEST);
    let tool = invoke_tool(ws, &ws.join("config.json"));
    let loop_arc = loop_with_snapshot(&[
        (
            "mcp_stub__read_page",
            serde_json::json!({"type":"object","required":["url"],"properties":{"url":{"type":"string"}}}),
        ),
        ("mcp_stub__click", serde_json::json!({"type":"object"})),
    ]);
    tool.set_loop(Arc::downgrade(&loop_arc));

    // 镜像 id（原生工具名）→ 基线投影。
    let o = tool
        .operation_override(r#"{"connector":"excl","op":"mcp_stub__read_page"}"#)
        .expect("mirror resolves");
    assert_eq!(
        o.op_type, "registry_read",
        "medium+read → op_slug_for 投影 registry_read 档（管线按 slug 反查 = MEDIUM，不比裸奔松）"
    );
    assert!(
        o.force_approval.is_none(),
        "基线无 confirm 卡: {:?}",
        o.force_approval
    );
    let ctx: serde_json::Value =
        serde_json::from_str(o.receipt_ctx.as_deref().expect("receipt_ctx")).expect("json");
    assert_eq!(ctx["risk"], "medium");
    assert_eq!(ctx["class"], "read");
    assert_eq!(ctx["op"], "mcp_stub__read_page");

    // 不在镜像面的名字（快照缺席）→ None → 静态 fail-closed 兜底。
    assert!(
        tool.operation_override(r#"{"connector":"excl","op":"mcp_stub__absent"}"#)
            .is_none(),
        "快照缺席的名字不镜像（None = 静态路径 fail-closed）"
    );
}

/// B3 四档基线：hints 驱动镜像 op 档位（readOnly 自述=low/read 免卡，
/// destructive 自述=high/write 强制确认卡；未声明维持 medium/read）。
/// 管线 slug 投影同源：high+write → file_write（plan 模式写闸免费同享）。
#[cfg(feature = "security")]
#[tokio::test]
async fn test_operation_override_mirror_hint_tiers() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(ws, "excl", EXCLUSIVE_MANIFEST);
    let tool = invoke_tool(ws, &ws.join("config.json"));
    let loop_arc = loop_with_snapshot_hints(&[
        (
            "mcp_stub__read_page",
            serde_json::json!({"type":"object"}),
            Some(true),
            None,
        ),
        (
            "mcp_stub__click",
            serde_json::json!({"type":"object"}),
            None,
            Some(true),
        ),
        (
            "mcp_stub__plain",
            serde_json::json!({"type":"object"}),
            None,
            None,
        ),
    ]);
    tool.set_loop(Arc::downgrade(&loop_arc));

    // readOnly 自述 → low/read：slug file_read（LOW 档），无确认卡。
    let o = tool
        .operation_override(r#"{"connector":"excl","op":"mcp_stub__read_page"}"#)
        .expect("ro mirror resolves");
    assert_eq!(o.op_type, "file_read", "low+read → LOW 档 slug");
    assert!(o.force_approval.is_none());
    let ctx: serde_json::Value = serde_json::from_str(o.receipt_ctx.as_deref().unwrap()).unwrap();
    assert_eq!(ctx["risk"], "low");
    assert_eq!(ctx["class"], "read");

    // destructive 自述 → high/write：slug file_write（HIGH 档）+ 强制确认卡。
    let o = tool
        .operation_override(r#"{"connector":"excl","op":"mcp_stub__click"}"#)
        .expect("destructive mirror resolves");
    assert_eq!(o.op_type, "file_write", "high+write → HIGH 档 slug");
    assert!(o.force_approval.is_some(), "自述破坏性 = 强制确认卡");
    let ctx: serde_json::Value = serde_json::from_str(o.receipt_ctx.as_deref().unwrap()).unwrap();
    assert_eq!(ctx["risk"], "high");
    assert_eq!(ctx["class"], "write");

    // 未声明 → 维持 medium/read 保守基线（v1 行为不变）。
    let o = tool
        .operation_override(r#"{"connector":"excl","op":"mcp_stub__plain"}"#)
        .expect("plain mirror resolves");
    assert_eq!(o.op_type, "registry_read");
    assert!(o.force_approval.is_none());
}

/// A4：镜像描述词元可命中——英文描述词根检索（原生工具 description 是
/// 独占面最富的检索素材；id 词元命中路径既有测试已覆盖）。
#[cfg(feature = "security")]
#[tokio::test]
async fn test_find_matches_mirror_description_tokens() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(ws, "excl", EXCLUSIVE_MANIFEST);
    let find = ConnectorFindTool::new(ws.to_path_buf());
    let loop_arc = loop_with_snapshot_full(
        &[(
            "mcp_stub__read_page",
            serde_json::json!({"type":"object"}),
            Some(true),
            None,
        )],
        &["Navigate to a URL and capture the page snapshot"],
    );
    find.set_loop(Arc::downgrade(&loop_arc));

    // token "snapshot" 不在 id（mcp_stub__read_page 无该词根）——命中的只能是
    // 英文描述词根，验证描述维度的检索。
    let out = find
        .execute(r#"{"query":"snapshot"}"#, &ctx())
        .await
        .expect("find ok");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let hits = v["connectors"].as_array().unwrap();
    let excl = hits.iter().find(|h| h["id"] == "excl").expect("excl hit");
    let matched = excl["matched_ops"].as_array().unwrap();
    assert!(
        matched.iter().any(|m| m == "mcp_stub__read_page"),
        "描述词根命中镜像: {excl}"
    );
}

/// 镜像 e2e：execute 走完整预检/预算/回执/dispatch 瀑布，原生工具真实执行
/// （绑定回灌通道放行独占闸），executed 回执带基线档位记账。
#[cfg(feature = "security")]
#[tokio::test]
async fn test_execute_mirror_op_runs_native_tool_with_receipt() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(ws, "excl", EXCLUSIVE_MANIFEST);
    let tool = invoke_tool(ws, &ws.join("config.json"));
    let loop_arc = loop_with_snapshot(&[(
        "mcp_stub__read_page",
        serde_json::json!({"type":"object","required":["url"],"properties":{"url":{"type":"string"}}}),
    )]);
    tool.set_loop(Arc::downgrade(&loop_arc));

    let out = tool
        .execute(
            r#"{"connector":"excl","op":"mcp_stub__read_page","args":{"url":"https://example.com"}}"#,
            &ctx(),
        )
        .await
        .expect("mirror execute ok");
    assert!(out.contains("Example Domain"), "原生工具结果透传: {out}");

    let receipts = list_receipts(ws, 50, None).expect("receipts");
    assert_eq!(receipts.len(), 1, "镜像基线 receipt 恒写");
    let r = &receipts[0];
    assert_eq!(r.outcome, "executed");
    assert_eq!(r.connector, "excl");
    assert_eq!(r.op, "mcp_stub__read_page");
    assert_eq!(r.risk, "medium", "基线档位记账");
    assert_eq!(r.class, "read");
}

/// args_schema 预检（R4）对镜像 op 同样生效：缺必填参数 = 出网前诚实拒。
#[cfg(feature = "security")]
#[tokio::test]
async fn test_execute_mirror_validates_args_against_snapshot_schema() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(ws, "excl", EXCLUSIVE_MANIFEST);
    let tool = invoke_tool(ws, &ws.join("config.json"));
    let loop_arc = loop_with_snapshot(&[(
        "mcp_stub__read_page",
        serde_json::json!({"type":"object","required":["url"],"properties":{"url":{"type":"string"}}}),
    )]);
    tool.set_loop(Arc::downgrade(&loop_arc));

    let (result, _receipt) = tool
        .execute_with_receipt(
            r#"{"connector":"excl","op":"mcp_stub__read_page","args":{}}"#,
            &ctx(),
        )
        .await;
    let err = result.expect_err("缺必填 url 必须拒");
    assert!(err.contains("url"), "拒绝原因点名缺失参数: {err}");
}

/// 封禁/豁免/快照缺席三态：镜像都不提供 → 诚实报错 + 独占接管 hint
/// （指路 connector_find / Dashboard 例外）。非独占 connector 无 hint。
#[cfg(feature = "security")]
#[tokio::test]
async fn test_mirror_blocked_exempt_absent_are_honest_errors_with_hint() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(ws, "excl", EXCLUSIVE_MANIFEST);
    let tool = invoke_tool(ws, &ws.join("config.json"));
    let loop_arc = loop_with_snapshot(&[
        (
            "mcp_stub__read_page",
            serde_json::json!({"type":"object","required":["url"],"properties":{"url":{"type":"string"}}}),
        ),
        ("mcp_stub__click", serde_json::json!({"type":"object"})),
    ]);
    tool.set_loop(Arc::downgrade(&loop_arc));

    // 封禁：镜像不提供（原生照隐——能力对 agent 消失）。
    nemesis_connectors::state::set_tool_policy(
        ws,
        "excl",
        "mcp_stub__click",
        nemesis_connectors::state::ToolPolicy::Blocked,
    )
    .expect("blocked");
    let err = tool
        .execute(r#"{"connector":"excl","op":"mcp_stub__click"}"#, &ctx())
        .await
        .expect_err("封禁 = 诚实拒");
    assert!(err.contains("独占接管"), "hint 指路治理面: {err}");

    // 豁免：回原生直调，不经镜像（原生侧独占闸放行）。
    nemesis_connectors::state::set_tool_policy(
        ws,
        "excl",
        "mcp_stub__click",
        nemesis_connectors::state::ToolPolicy::Exempt,
    )
    .expect("exempt");
    let err = tool
        .execute(r#"{"connector":"excl","op":"mcp_stub__click"}"#, &ctx())
        .await
        .expect_err("豁免 = 镜像不提供（走原生直调）");
    assert!(err.contains("独占接管"), "{err}");

    // 快照缺席的名字：同错误路径 + hint（可用清单只列显式 op，镜像面
    // 指路 connector_find——清单可能很长，不往错误里塞）。
    let err = tool
        .execute(r#"{"connector":"excl","op":"mcp_stub__nope"}"#, &ctx())
        .await
        .expect_err("未知名字诚实拒");
    assert!(err.contains("独占接管"), "{err}");
    assert!(
        err.contains("connector_find 可列"),
        "hint 指路镜像枚举通道: {err}"
    );
    assert!(err.contains("可用: noop"), "显式 op 清单在场: {err}");

    // 非独占 connector：同款无 op 错误但无独占 hint。
    write_connector(ws, "paper-broker", MIXED_MANIFEST);
    let err = tool
        .execute(r#"{"connector":"paper-broker","op":"nope"}"#, &ctx())
        .await
        .expect_err("未知 op 诚实拒");
    assert!(
        !err.contains("独占接管"),
        "非独占 connector 不带接管 hint: {err}"
    );
}

/// connector_find：镜像 op 独立数组枚举（binding mirror:tool:* + 快照
/// schema 随行）；token 命中镜像工具名 = 该 connector 入列；无 loop /
/// 无独占声明 = mirrors 空。
#[cfg(feature = "security")]
#[tokio::test]
async fn test_find_lists_mirror_ops_for_exclusive_connector() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write_connector(ws, "excl", EXCLUSIVE_MANIFEST);
    write_connector(ws, "paper-broker", MIXED_MANIFEST);

    let find = ConnectorFindTool::new(ws.to_path_buf());
    let loop_arc = loop_with_snapshot(&[(
        "mcp_stub__read_page",
        serde_json::json!({"type":"object","required":["url"],"properties":{"url":{"type":"string"}}}),
    )]);
    find.set_loop(Arc::downgrade(&loop_arc));

    // 全列表：excl 带 mirrors 数组；paper-broker mirrors 空。
    let out = find
        .execute(r#"{"query":""}"#, &ctx())
        .await
        .expect("find ok");
    let v: serde_json::Value = serde_json::from_str(&out).expect("json");
    let rows = v["connectors"].as_array().unwrap();
    let excl = rows.iter().find(|r| r["id"] == "excl").expect("excl 入列");
    let mirrors = excl["mirrors"].as_array().unwrap();
    assert_eq!(mirrors.len(), 1, "{excl}");
    assert_eq!(mirrors[0]["id"], "mcp_stub__read_page");
    assert_eq!(mirrors[0]["binding"], "mirror:tool:mcp_stub__read_page");
    assert_eq!(mirrors[0]["risk"], "medium");
    assert_eq!(mirrors[0]["confirm_required"], false);
    assert_eq!(
        mirrors[0]["args_schema"]["required"],
        serde_json::json!(["url"]),
        "快照 schema 随行"
    );
    let plain = rows
        .iter()
        .find(|r| r["id"] == "paper-broker")
        .expect("paper-broker 入列");
    assert_eq!(
        plain["mirrors"].as_array().unwrap().len(),
        0,
        "非独占 connector 无镜像面"
    );

    // token 检索：按原生根名词命中镜像（read_page 既是镜像 id 也是桩工具名）。
    let out = find
        .execute(r#"{"query":"read_page"}"#, &ctx())
        .await
        .expect("find ok");
    assert!(out.contains("\"excl\""), "镜像 token 命中 connector: {out}");
    assert!(
        out.contains("mcp_stub__read_page"),
        "matched_ops 含镜像 id: {out}"
    );

    // 无 loop 的 find（独立装配形态）：诚实不假列。
    let bare = ConnectorFindTool::new(ws.to_path_buf());
    let out = bare
        .execute(r#"{"query":""}"#, &ctx())
        .await
        .expect("find ok");
    let v: serde_json::Value = serde_json::from_str(&out).expect("json");
    let excl = v["connectors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == "excl")
        .expect("excl 入列");
    assert_eq!(
        excl["mirrors"].as_array().unwrap().len(),
        0,
        "无 loop = 不假列调不到的镜像"
    );
}
