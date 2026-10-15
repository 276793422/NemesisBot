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
async fn test_paper_order_without_price_is_honest_error_with_receipt() {
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
    assert!(err.contains("price"), "guides to quote op: {err}");
    // error 回执在册（动作发生过）
    let receipts = list_receipts(ws, 50, None).expect("receipts");
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].outcome, "error");
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
