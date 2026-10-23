//! connector 数据面 WSAPI 测试（行动代理 P0，2026-10-14）。
//!
//! 覆盖三条只读命令的投影面：`connectors.list`（合法 + 损坏诚实入列）、
//! `connectors.receipts.list`（append → 回读 / limit / connector 过滤）、
//! `paperbroker.ledger`（缺文件默认账本 + 落单后投影）。数据面直读
//! nemesis-connectors 存储——存储层单测归 nemesis-connectors，这里钉
//! handler 的字段投影与错误分支。

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::{AuthMethod, SessionManager};
use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_connectors::receipt::Receipt;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

// ---------------------------------------------------------------------------
// 夹具
// ---------------------------------------------------------------------------

/// 带临时 workspace 的 RequestContext（TempDir 由调用方持有保活）。
fn make_ctx(dir: &Path) -> RequestContext {
    let ws = dir.to_string_lossy().to_string();
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: Some(ws.clone()),
        home: Some(ws.clone()),
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new("test-model".to_string())),
        model_base: Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: Arc::new(AtomicBool::new(false)),
        event_hub: Arc::new(EventHub::new()),
        running: Arc::new(AtomicBool::new(true)),
        session_manager: Arc::new(SessionManager::with_default_timeout()),
        inbound_tx: None,
        streaming_provider: None,
        ws_router: None,
        agent_service: None,
        data_store: None,
        memory_manager: None,
        forge: None,
        agent_loop: Arc::new(parking_lot::RwLock::new(None)),
        cluster: None,
        quic_relay: None,
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: None,
        #[cfg(feature = "workflow")]
        chat_secret_store: std::sync::Arc::new(
            nemesis_workflow::chat_secrets::ChatSecretStore::in_memory(),
        ),
        #[cfg(not(feature = "workflow"))]
        chat_secret_store: std::sync::Arc::new(()),
        #[cfg(feature = "workflow")]
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        #[cfg(not(feature = "workflow"))]
        webhook_rate_limiter: std::sync::Arc::new(()),
        internal_cmd_tx: None,
        estop: None,
        signature_verify: None,
        skills_install_gate: None,
        cron: None,
        board: None,
    });
    RequestContext {
        session_id: "test-session".to_string(),
        chat_id: "test-chat".to_string(),
        workspace: Some(ws),
        home: Some(dir.to_string_lossy().to_string()),
        state,
        auth_method: AuthMethod::default(),
    }
}

/// workspace: None 的 ctx（workspace_of 错误分支）。
fn make_ctx_no_workspace() -> RequestContext {
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: None,
        home: None,
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new("test-model".to_string())),
        model_base: Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: Arc::new(AtomicBool::new(false)),
        event_hub: Arc::new(EventHub::new()),
        running: Arc::new(AtomicBool::new(true)),
        session_manager: Arc::new(SessionManager::with_default_timeout()),
        inbound_tx: None,
        streaming_provider: None,
        ws_router: None,
        agent_service: None,
        data_store: None,
        memory_manager: None,
        forge: None,
        agent_loop: Arc::new(parking_lot::RwLock::new(None)),
        cluster: None,
        quic_relay: None,
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: None,
        #[cfg(feature = "workflow")]
        chat_secret_store: std::sync::Arc::new(
            nemesis_workflow::chat_secrets::ChatSecretStore::in_memory(),
        ),
        #[cfg(not(feature = "workflow"))]
        chat_secret_store: std::sync::Arc::new(()),
        #[cfg(feature = "workflow")]
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        #[cfg(not(feature = "workflow"))]
        webhook_rate_limiter: std::sync::Arc::new(()),
        internal_cmd_tx: None,
        estop: None,
        signature_verify: None,
        skills_install_gate: None,
        cron: None,
        board: None,
    });
    RequestContext {
        session_id: "test-session".to_string(),
        chat_id: "test-chat".to_string(),
        workspace: None,
        home: None,
        state,
        auth_method: AuthMethod::default(),
    }
}

/// 写一个最小合法 connector（http 绑定 op）。
fn write_http_connector(ws: &Path, dir_name: &str, id: &str) {
    let dir = ws
        .join(nemesis_connectors::registry::CONNECTORS_DIR)
        .join(dir_name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(nemesis_connectors::registry::MANIFEST_FILE),
        format!(
            r#"[connector]
id = "{id}"
name = "演示行情"
version = "0.1.0"
kind = "rest"
description = "demo"

[connector.http]
base = "https://example.com"

[[op]]
id = "quote"
intent = ["查询行情"]
risk = "low"
class = "network"

[op.http]
method = "GET"
path = "/quote/{{symbol}}"

[[op]]
id = "buy"
intent = ["买入"]
risk = "high"
class = "network"
confirm = "required"
budget = "finance"

[op.http]
method = "POST"
path = "/order"
"#
        ),
    )
    .unwrap();
}

fn mk_receipt(connector: &str, op: &str, millis: i64, outcome: &str) -> Receipt {
    Receipt {
        receipt_id: nemesis_connectors::receipt::new_receipt_id(millis),
        ts: chrono::DateTime::from_timestamp_millis(millis)
            .unwrap()
            .to_rfc3339(),
        connector: connector.to_string(),
        op: op.to_string(),
        risk: "low".to_string(),
        class: "network".to_string(),
        outcome: outcome.to_string(),
        args_digest: "0123456789abcdef0123456789abcdef".to_string(),
        summary: "demo summary".to_string(),
        receipt_key: None,
        receipt_value: None,
        budget_consumed: None,
        approval_ref: None,
        session_key: None,
        undoable: false,
    }
}

// ---------------------------------------------------------------------------
// connectors.list
// ---------------------------------------------------------------------------

/// 空注册表（缺 connectors 目录）= count 0 + enabled 缺省 true（无
/// config.json 时与供给闸同源语义）。
#[test]
fn list_empty_registry_defaults_enabled() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(dir.path());
    let out = cmd_list(&ctx).unwrap().unwrap();
    assert_eq!(out["count"], 0);
    assert_eq!(out["enabled"], true);
    assert_eq!(out["connectors"].as_array().unwrap().len(), 0);
}

/// 合法 + 损坏条目同场：合法投影全字段（ops 含 http binding 大写 method），
/// 损坏条目诚实入列带 error（enabled=false）。
#[test]
fn list_reports_valid_and_corrupt_connectors() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    write_http_connector(ws, "a-demo", "demo-quotes");
    let broken = ws
        .join(nemesis_connectors::registry::CONNECTORS_DIR)
        .join("b-broken");
    std::fs::create_dir_all(&broken).unwrap();
    std::fs::write(
        broken.join(nemesis_connectors::registry::MANIFEST_FILE),
        "not toml [",
    )
    .unwrap();

    let ctx = make_ctx(ws);
    let out = cmd_list(&ctx).unwrap().unwrap();
    let rows = out["connectors"].as_array().unwrap();
    assert_eq!(out["count"], 2, "合法+损坏都入列: {out}");

    // 目录名升序确定性：a-demo 在前。
    let valid = &rows[0];
    assert_eq!(valid["id"], "demo-quotes");
    assert_eq!(valid["kind"], "rest");
    assert_eq!(valid["enabled"], true);
    // M1：未签名 connector → unsigned 徽标（信任库缺席 = 全 unsigned）。
    assert_eq!(valid["trust"], "unsigned");
    assert!(valid["error"].is_null());
    let ops = valid["ops"].as_array().unwrap();
    assert_eq!(ops.len(), 2);
    assert_eq!(ops[0]["id"], "quote");
    assert_eq!(ops[0]["binding"]["type"], "http");
    assert_eq!(ops[0]["binding"]["method"], "GET");
    assert_eq!(ops[1]["id"], "buy");
    assert_eq!(ops[1]["risk"], "high");
    assert_eq!(ops[1]["confirm_required"], true);
    assert_eq!(ops[1]["finance_budget"], true);

    let corrupt = &rows[1];
    assert_eq!(corrupt["id"], "b-broken", "损坏条目以目录名代 id");
    assert_eq!(corrupt["enabled"], false);
    // M1：损坏条目 trust = null（无有效清单，谈不上「未签名」的判断）。
    assert!(corrupt["trust"].is_null(), "corrupt trust: {corrupt}");
    let err = corrupt["error"].as_str().unwrap();
    assert!(!err.is_empty(), "损坏原因必须可见: {corrupt}");
}

/// M1 信任徽标三态（2026-10-19）：verified（信任库内密钥签名）/ unsigned
/// （无 sig 文件）/ 损坏（null）同场投影——前端徽标色的数据面契约。
#[test]
fn list_trust_badges_three_states() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    // verified：写清单 → 信任库收录密钥 → 签名。
    let vdir = ws
        .join(nemesis_connectors::registry::CONNECTORS_DIR)
        .join("a-signed");
    write_http_connector(ws, "a-signed", "signed-quotes");
    let pair = nemesis_connectors::signing::generate_key_pair().expect("keygen");
    let store = nemesis_connectors::signing::workspace_trust_store(ws);
    store.add_key(
        &pair.public_key,
        "test-signer",
        nemesis_connectors::signing::TrustLevel::Verified,
    );
    nemesis_connectors::signing::sign_connector_dir(&vdir, &pair.private_key).expect("sign");

    // unsigned：不签名的普通清单。
    write_http_connector(ws, "b-plain", "plain-quotes");

    // corrupt：坏 TOML。
    let broken = ws
        .join(nemesis_connectors::registry::CONNECTORS_DIR)
        .join("c-broken");
    std::fs::create_dir_all(&broken).unwrap();
    std::fs::write(
        broken.join(nemesis_connectors::registry::MANIFEST_FILE),
        "not toml [",
    )
    .unwrap();

    let ctx = make_ctx(ws);
    let out = cmd_list(&ctx).unwrap().unwrap();
    let rows = out["connectors"].as_array().unwrap();
    assert_eq!(rows.len(), 3, "三态同场: {out}");
    assert_eq!(rows[0]["id"], "signed-quotes");
    assert_eq!(rows[0]["trust"], "verified", "信任库内密钥签名 = verified");
    assert_eq!(rows[1]["id"], "plain-quotes");
    assert_eq!(rows[1]["trust"], "unsigned");
    assert_eq!(rows[2]["id"], "c-broken");
    assert!(rows[2]["trust"].is_null());
}

/// workspace 未配置 → workspace_of 诚实 Err。
#[test]
fn list_without_workspace_errors() {
    let ctx = make_ctx_no_workspace();
    let err = cmd_list(&ctx).unwrap_err();
    assert!(err.contains("workspace"), "{err}");
}

// ---------------------------------------------------------------------------
// connectors.receipts.list
// ---------------------------------------------------------------------------

/// append 三条 → 回读新→旧全量；limit 截断取最新的 N 条。
#[test]
fn receipts_list_roundtrip_and_limit() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let ctx = make_ctx(ws);

    // 空流水：count 0 不报错。
    let out = cmd_receipts_list(&ctx, None).unwrap().unwrap();
    assert_eq!(out["count"], 0);

    for (i, (conn, millis)) in [("demo", 1_000i64), ("other", 2_000), ("demo", 3_000)]
        .iter()
        .enumerate()
    {
        let mut r = mk_receipt(conn, "quote", *millis, "executed");
        r.receipt_id = format!("RCPT-{millis}-{}", i + 1);
        nemesis_connectors::receipt::append_receipt(ws, &r).unwrap();
    }

    let out = cmd_receipts_list(&ctx, None).unwrap().unwrap();
    assert_eq!(out["count"], 3);
    let rows = out["receipts"].as_array().unwrap();
    // 新→旧。
    assert_eq!(rows[0]["receipt_id"], "RCPT-3000-3");
    assert_eq!(rows[2]["receipt_id"], "RCPT-1000-1");
    assert_eq!(rows[0]["connector"], "demo");
    assert_eq!(rows[0]["outcome"], "executed");

    // limit=2 → 只留最新两条。
    let out = cmd_receipts_list(&ctx, Some(&serde_json::json!({ "limit": 2 })))
        .unwrap()
        .unwrap();
    assert_eq!(out["count"], 2);
    assert_eq!(out["limit"], 2);
    assert_eq!(out["receipts"][0]["receipt_id"], "RCPT-3000-3");
    assert_eq!(out["receipts"][1]["receipt_id"], "RCPT-2000-2");
}

/// connector 过滤只回匹配条目；cap 500 生效（超大 limit 收敛）。
#[test]
fn receipts_list_filters_by_connector_and_caps_limit() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let ctx = make_ctx(ws);
    for (conn, millis) in [("demo", 1_000i64), ("other", 2_000)] {
        nemesis_connectors::receipt::append_receipt(
            ws,
            &mk_receipt(conn, "quote", millis, "executed"),
        )
        .unwrap();
    }

    let out = cmd_receipts_list(&ctx, Some(&serde_json::json!({ "connector": "other" })))
        .unwrap()
        .unwrap();
    assert_eq!(out["count"], 1);
    assert_eq!(out["connector"], "other");
    assert_eq!(out["receipts"][0]["connector"], "other");

    // limit 上限 500（999 请求 → 500）。
    let out = cmd_receipts_list(&ctx, Some(&serde_json::json!({ "limit": 999 })))
        .unwrap()
        .unwrap();
    assert_eq!(out["limit"], 500);
}

// ---------------------------------------------------------------------------
// paperbroker.ledger
// ---------------------------------------------------------------------------

/// 缺账本文件 = 全新模拟账本（现金 1M 零持仓零订单，零账号 demo 常态）。
#[test]
fn ledger_missing_file_returns_default() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(dir.path());
    let out = cmd_ledger(&ctx).unwrap().unwrap();
    assert_eq!(out["cash"], 1_000_000.0);
    assert_eq!(out["positions"].as_array().unwrap().len(), 0);
    assert_eq!(out["orders"].as_array().unwrap().len(), 0);
    assert_eq!(out["order_count"], 0);
    assert!(
        out["ledger_path"]
            .as_str()
            .unwrap()
            .ends_with("paper_broker.json"),
        "ledger_path: {out}"
    );
}

/// 账本落单后投影：cash/positions/orders 全量透出。
#[test]
fn ledger_projects_saved_state() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let ctx = make_ctx(ws);

    let mut l = nemesis_connectors::ledger::load_ledger(ws).unwrap();
    let ts = "2026-10-14T08:00:00Z";
    l.buy("600519", 100.0, 1500.0, ts).unwrap();
    nemesis_connectors::ledger::save_ledger(ws, &l).unwrap();

    let out = cmd_ledger(&ctx).unwrap().unwrap();
    assert!(
        (out["cash"].as_f64().unwrap() - 850_000.0).abs() < 1e-6,
        "现金扣减: {out}"
    );
    let positions = out["positions"].as_array().unwrap();
    assert_eq!(positions.len(), 1);
    assert_eq!(positions[0]["symbol"], "600519");
    assert_eq!(positions[0]["qty"], 100.0);
    assert_eq!(positions[0]["avg_cost"], 1500.0);
    let orders = out["orders"].as_array().unwrap();
    assert_eq!(orders.len(), 1);
    assert_eq!(orders[0]["side"], "buy");
    assert_eq!(orders[0]["status"], "filled");
    assert_eq!(out["order_count"], 1);
}

// ---------------------------------------------------------------------------
// ModuleHandler 契约
// ---------------------------------------------------------------------------

/// commands() 清单 + dispatch 路由 + 未知命令诚实报错。
#[tokio::test]
async fn handler_contract_and_unknown_command() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(dir.path());

    let h = ConnectorsHandler;
    assert_eq!(h.module_name(), "connectors");
    assert_eq!(
        h.commands(),
        &[
            "list",
            "receipts.list",
            "undo",
            "create",
            "set_enabled",
            "remove",
            "set_master",
            "tool_policy"
        ]
    );
    let out = h.handle_cmd("list", None, &ctx).await.unwrap().unwrap();
    assert!(out["connectors"].is_array());
    let err = h.handle_cmd("nope", None, &ctx).await.unwrap_err();
    assert!(err.contains("unknown command: connectors.nope"), "{err}");

    // undo 路由可达：缺 receipt_id 走到参数校验（而非 unknown command）。
    let err = h.handle_cmd("undo", None, &ctx).await.unwrap_err();
    assert!(err.contains("receipt_id"), "{err}");

    let p = PaperBrokerHandler;
    assert_eq!(p.module_name(), "paperbroker");
    assert_eq!(p.commands(), &["ledger"]);
    let out = p.handle_cmd("ledger", None, &ctx).await.unwrap().unwrap();
    assert_eq!(out["cash"], 1_000_000.0);
    let err = p.handle_cmd("nope", None, &ctx).await.unwrap_err();
    assert!(err.contains("unknown command: paperbroker.nope"), "{err}");
}

// ---------------------------------------------------------------------------
// connectors.undo（E3 页内撤销）
// ---------------------------------------------------------------------------

/// 带撤销声明矩阵的 connector：read（quote）/ write+undo（place_order ←
/// cancel_order）/ write 无 undo（post_note）/ network 无 undo（ping_net）。
fn write_undo_connector(ws: &Path) {
    let dir = ws
        .join(nemesis_connectors::registry::CONNECTORS_DIR)
        .join("u-broker");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(nemesis_connectors::registry::MANIFEST_FILE),
        r#"[connector]
id = "u-broker"
name = "撤销测试"
version = "0.1.0"
kind = "rest"
description = "E3 undo 矩阵"

[[op]]
id = "quote"
intent = ["查询"]
risk = "low"
class = "read"
action = "builtin.demo_read"

[[op]]
id = "place_order"
intent = ["下单"]
risk = "critical"
class = "write"
confirm = "required"
receipt = "order_id"
undo = { op = "cancel_order", arg = "order_id" }
action = "builtin.demo_order"

[[op]]
id = "cancel_order"
intent = ["撤单"]
risk = "medium"
class = "write"
action = "builtin.demo_cancel"

[[op]]
id = "post_note"
intent = ["发帖"]
risk = "high"
class = "write"
action = "builtin.demo_post"

[[op]]
id = "ping_net"
intent = ["探活"]
risk = "low"
class = "network"
action = "builtin.demo_net"
"#,
    )
    .unwrap();
}

fn undo_receipt(connector: &str, op: &str, outcome: &str) -> Receipt {
    let mut r = mk_receipt(connector, op, 5_000, outcome);
    r.receipt_id = format!("RCPT-UNDO-{}", op);
    r
}

/// 缺 receipt_id / 凭条不存在 / 非 executed 三道前置闸。
#[tokio::test]
async fn undo_front_gates() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    write_undo_connector(ws);
    let ctx = make_ctx(ws);

    let err = cmd_undo(&ctx, None).await.unwrap_err();
    assert!(err.contains("receipt_id"), "{err}");

    let err = cmd_undo(&ctx, Some(&json!({ "receipt_id": "RCPT-NOPE" })))
        .await
        .unwrap_err();
    assert!(err.contains("不存在"), "{err}");

    let r = undo_receipt("u-broker", "place_order", "denied");
    receipt::append_receipt(ws, &r).unwrap();
    let err = cmd_undo(&ctx, Some(&json!({ "receipt_id": r.receipt_id })))
        .await
        .unwrap_err();
    assert!(err.contains("未真实生效"), "{err}");
}

/// 撤销防重闸（边界补强 2026-10-20）：已发起过撤销的凭条拒绝再撤——
/// 反向 op 重复执行会叠加外部副作用（双退款形态）。
#[tokio::test]
async fn undo_rejects_already_undone() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    write_undo_connector(ws);
    let ctx = make_ctx(ws);

    let mut r = undo_receipt("u-broker", "place_order", "executed");
    r.receipt_value = Some("NB-ORD-77".to_string());
    r.session_key = Some("web:u-undo".to_string());
    receipt::append_receipt(ws, &r).unwrap();

    // 未标记：走到后续闸（harness 无 loop → resolve 诚实拒，不是防重文案）。
    let err = cmd_undo(&ctx, Some(&json!({ "receipt_id": r.receipt_id })))
        .await
        .unwrap_err();
    assert!(err.contains("agent loop not running"), "{err}");

    // 落标记后再撤：防重闸拒绝（优先于注册表/loop 闸）。
    nemesis_connectors::state::mark_undone(ws, &r.receipt_id, None);
    let err = cmd_undo(&ctx, Some(&json!({ "receipt_id": r.receipt_id })))
        .await
        .unwrap_err();
    assert!(err.contains("已发起过撤销"), "{err}");
    assert!(err.contains("叠加副作用"), "{err}");

    // read 类不经过防重（无副作用，诚实 noop 照常）。
    let qr = undo_receipt("u-broker", "quote", "executed");
    receipt::append_receipt(ws, &qr).unwrap();
    nemesis_connectors::state::mark_undone(ws, &qr.receipt_id, None);
    let out = cmd_undo(&ctx, Some(&json!({ "receipt_id": qr.receipt_id })))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["via"], "none", "read 类照常诚实 noop");
}

/// connector 从注册表消失 / op 从清单移除：按当下裁决诚实拒。
#[tokio::test]
async fn undo_rejects_stale_registry() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let ctx = make_ctx(ws);

    // connector 不在注册表（未写清单）。
    let r = undo_receipt("ghost", "place_order", "executed");
    receipt::append_receipt(ws, &r).unwrap();
    let err = cmd_undo(&ctx, Some(&json!({ "receipt_id": r.receipt_id })))
        .await
        .unwrap_err();
    assert!(err.contains("不在注册表"), "{err}");

    // op 已从清单移除（凭条 op = removed_op，清单里没有）。
    write_undo_connector(ws);
    let r = undo_receipt("u-broker", "removed_op", "executed");
    receipt::append_receipt(ws, &r).unwrap();
    let err = cmd_undo(&ctx, Some(&json!({ "receipt_id": r.receipt_id })))
        .await
        .unwrap_err();
    assert!(err.contains("清单移除"), "{err}");
}

/// read 类凭条：诚实说明无需撤销（不算错）。
#[tokio::test]
async fn undo_read_class_is_honest_noop() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    write_undo_connector(ws);
    let ctx = make_ctx(ws);

    let r = undo_receipt("u-broker", "quote", "executed");
    receipt::append_receipt(ws, &r).unwrap();
    let out = cmd_undo(&ctx, Some(&json!({ "receipt_id": r.receipt_id })))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["undone"], false);
    assert_eq!(out["via"], "none");
    assert_eq!(out["capability"], "none");
    assert!(out["reason"].as_str().unwrap().contains("read"), "{out}");
}

/// 域 C 两分支：receipt_value 缺失拒；有值但测试 harness 无 loop = 诚实拒
/// （resolve_session_loop 落主槽 None）——撤销必须经完整 dispatch 瀑布，
/// 拿不到执行体就不撤。
#[tokio::test]
async fn undo_domain_c_receipt_value_and_loop_gates() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    write_undo_connector(ws);
    let ctx = make_ctx(ws);

    // receipt_value 空 → 无法回填撤销参数。
    let r = undo_receipt("u-broker", "place_order", "executed");
    receipt::append_receipt(ws, &r).unwrap();
    let err = cmd_undo(&ctx, Some(&json!({ "receipt_id": r.receipt_id })))
        .await
        .unwrap_err();
    assert!(err.contains("主键值"), "{err}");

    // receipt_value 有值 → resolve_session_loop 诚实失败（harness 无 loop）。
    let mut r = undo_receipt("u-broker", "place_order", "executed");
    r.receipt_value = Some("NB-ORD-42".to_string());
    r.session_key = Some("web:test-chat".to_string());
    receipt::append_receipt(ws, &r).unwrap();
    let err = cmd_undo(&ctx, Some(&json!({ "receipt_id": r.receipt_id })))
        .await
        .unwrap_err();
    assert!(err.contains("agent loop not running"), "{err}");
}

/// 域 B：write 无 undo 声明 → 会话级撤销。session_key 双源（入参 >
/// 凭条）都空 = 拒；有 session_key 但 harness 无 loop = 诚实拒。
#[tokio::test]
async fn undo_domain_b_session_key_and_loop_gates() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    write_undo_connector(ws);
    let ctx = make_ctx(ws);

    // 双源皆空 → 拒。
    let r = undo_receipt("u-broker", "post_note", "executed");
    receipt::append_receipt(ws, &r).unwrap();
    let err = cmd_undo(&ctx, Some(&json!({ "receipt_id": r.receipt_id })))
        .await
        .unwrap_err();
    assert!(err.contains("session_key"), "{err}");

    // 入参 session_key 有值 → resolve 诚实失败（harness 无 loop）。
    let err = cmd_undo(
        &ctx,
        Some(&json!({ "receipt_id": r.receipt_id, "session_key": "web:u1" })),
    )
    .await
    .unwrap_err();
    assert!(err.contains("agent loop not running"), "{err}");

    // 凭条 session_key 兜底同样被读取（同款诚实拒）。
    let mut r2 = undo_receipt("u-broker", "post_note", "executed");
    r2.receipt_id = "RCPT-UNDO-post_note2".to_string();
    r2.session_key = Some("web:u2".to_string());
    receipt::append_receipt(ws, &r2).unwrap();
    let err = cmd_undo(&ctx, Some(&json!({ "receipt_id": r2.receipt_id })))
        .await
        .unwrap_err();
    assert!(err.contains("agent loop not running"), "{err}");
}

/// network/exec 类无 undo 声明：能力建模 = none（R5）——结构化响应诚实
/// 说明无通用撤销语义（前端按 capability 隐藏撤销按钮/点击明示）。
#[tokio::test]
async fn undo_network_class_without_undo_reports_no_capability() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    write_undo_connector(ws);
    let ctx = make_ctx(ws);

    let r = undo_receipt("u-broker", "ping_net", "executed");
    receipt::append_receipt(ws, &r).unwrap();
    let out = cmd_undo(&ctx, Some(&json!({ "receipt_id": r.receipt_id })))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["undone"], false);
    assert_eq!(out["capability"], "none");
    assert!(
        out["reason"].as_str().unwrap().contains("无通用撤销语义"),
        "{out}"
    );
}

// ---------------------------------------------------------------------------
// undo_ledger_verdict（R5 结构化判定纯函数，三臂）
// ---------------------------------------------------------------------------

/// 三臂：新凭条 executed → Ok(新凭条 id)；新凭条非 executed → Err（含
/// outcome）；无新凭条 → Err。外加两条钉死不误判：派发前在册（before
/// 快照内）的旧 executed 凭条不算撤销成功；connector/op 不匹配的新凭条
/// 不算（比如并发其他 op 也落了账）。
#[test]
fn undo_ledger_verdict_three_arms() {
    let mut before = std::collections::HashSet::new();
    let old = undo_receipt("u-broker", "cancel_order", "executed");
    before.insert(old.receipt_id.clone());

    // 臂 1：新增 executed 凭条 → Ok(id)。
    let mut ok_new = undo_receipt("u-broker", "cancel_order", "executed");
    ok_new.receipt_id = "RCPT-AFTER-OK".to_string();
    let after = vec![old.clone(), ok_new.clone()];
    let v = undo_ledger_verdict(&before, &after, "u-broker", "cancel_order");
    assert_eq!(
        v.unwrap().as_str(),
        "RCPT-AFTER-OK",
        "新 executed 凭条 = 撤销成功"
    );

    // 臂 2：新增凭条但 outcome 非 executed（如 budget_blocked）→ Err 含 outcome。
    let mut blocked = undo_receipt("u-broker", "cancel_order", "budget_blocked");
    blocked.receipt_id = "RCPT-AFTER-BLOCKED".to_string();
    let after = vec![old.clone(), blocked];
    let err = undo_ledger_verdict(&before, &after, "u-broker", "cancel_order").unwrap_err();
    assert!(err.contains("budget_blocked"), "{err}");
    assert!(err.contains("未真实生效"), "{err}");

    // F3（2026-10-16 二轮复查）：unknown 措辞与前缀闸同口径——「结果未知，
    // 可能未被撤销」，不出现「未真实生效」的假确定性。
    let mut unknown = undo_receipt("u-broker", "cancel_order", "unknown");
    unknown.receipt_id = "RCPT-AFTER-UNKNOWN".to_string();
    let after = vec![old.clone(), unknown];
    let err = undo_ledger_verdict(&before, &after, "u-broker", "cancel_order").unwrap_err();
    assert!(err.contains("结果未知"), "{err}");
    assert!(!err.contains("未真实生效"), "unknown 不得断言未生效: {err}");

    // 臂 3：无新凭条（被拦在凭条前/执行体不可达）→ Err。
    let err = undo_ledger_verdict(
        &before,
        std::slice::from_ref(&old),
        "u-broker",
        "cancel_order",
    )
    .unwrap_err();
    assert!(err.contains("未落新凭条"), "{err}");

    // before 内旧 executed 凭条不误判为撤销成功。
    let v = undo_ledger_verdict(
        &before,
        std::slice::from_ref(&old),
        "u-broker",
        "cancel_order",
    );
    assert!(v.is_err(), "旧凭条不算新账");

    // connector/op 不匹配的新凭条不算（并发他 op 落账不背锅）。
    let mut other_op = undo_receipt("u-broker", "quote", "executed");
    other_op.receipt_id = "RCPT-OTHER-OP".to_string();
    let mut other_conn = undo_receipt("other-conn", "cancel_order", "executed");
    other_conn.receipt_id = "RCPT-OTHER-CONN".to_string();
    let err = undo_ledger_verdict(
        &before,
        &[old, other_op, other_conn],
        "u-broker",
        "cancel_order",
    )
    .unwrap_err();
    assert!(err.contains("未落新凭条"), "{err}");
}

// ---------------------------------------------------------------------------
// M4b（2026-10-19）tool 绑定在位探测（mcp_ready）
// ---------------------------------------------------------------------------

/// tool 绑定 connector 模板（window-mcp 形态；desktop-ui 模板同构）。
fn write_tool_connector(ws: &Path, dir: &str, id: &str) {
    let dir = ws
        .join(nemesis_connectors::registry::CONNECTORS_DIR)
        .join(dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(nemesis_connectors::registry::MANIFEST_FILE),
        format!(
            r#"[connector]
id = "{id}"
name = "Desktop UI"
version = "0.1.0"
kind = "local"
description = "桌面只读面"

[[op]]
id = "list_windows"
intent = ["枚举窗口"]
risk = "low"
class = "read"
tool = "mcp_window_enumerate_windows"
"#
        ),
    )
    .unwrap();
}

fn row_of<'a>(out: &'a serde_json::Value, id: &str) -> &'a serde_json::Value {
    out["connectors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == id)
        .unwrap_or_else(|| panic!("connector {id} 未入列: {out}"))
}

/// 在位探测三态：window server 已注册 → true；config.mcp.json 缺席 →
/// false（诚实「MCP 未注册」，不假装可用）；http 绑定 → null（不适用）。
/// server 名 sanitize 对齐 adapter 挂载面（`my.window` → `mcp_my_window_`）。
#[test]
fn list_tool_bindings_probe_mcp_registration() {
    // ① 已注册 → mcp_ready = true
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    write_tool_connector(ws, "d-ui", "desktop-ui");
    write_http_connector(ws, "a-rest", "demo-quotes");
    let cfg_dir = ws.join("config");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(
        cfg_dir.join("config.mcp.json"),
        r#"{"enabled": true, "servers": [{"name": "window", "transport_type": "stdio", "command": "window-mcp.exe"}]}"#,
    )
    .unwrap();
    let ctx = make_ctx(ws);
    let out = cmd_list(&ctx).unwrap().unwrap();
    assert_eq!(
        row_of(&out, "desktop-ui")["mcp_ready"],
        true,
        "window server 已注册 → 就绪: {out}"
    );
    assert!(
        row_of(&out, "demo-quotes")["mcp_ready"].is_null(),
        "http 绑定探测不适用 → null: {out}"
    );

    // ② 无 config.mcp.json → false
    let dir2 = tempfile::tempdir().unwrap();
    write_tool_connector(dir2.path(), "d-ui", "desktop-ui");
    let ctx2 = make_ctx(dir2.path());
    let out2 = cmd_list(&ctx2).unwrap().unwrap();
    assert_eq!(
        row_of(&out2, "desktop-ui")["mcp_ready"],
        false,
        "MCP server 缺席 → 诚实未就绪: {out2}"
    );

    // ③ sanitize 对齐：server 名带非 [a-z0-9_] 字符 → 前缀归一同 adapter。
    let dir3 = tempfile::tempdir().unwrap();
    let ws3 = dir3.path();
    write_tool_connector(ws3, "d-ui", "desktop-ui");
    std::fs::create_dir_all(ws3.join("config")).unwrap();
    std::fs::write(
        ws3.join("config").join("config.mcp.json"),
        r#"{"enabled": true, "servers": [{"name": "my.window", "transport_type": "stdio", "command": "window-mcp.exe"}]}"#,
    )
    .unwrap();
    // 绑定名按 sanitize 后前缀（my.window → my_window）才命中。
    let dir_t = ws3
        .join(nemesis_connectors::registry::CONNECTORS_DIR)
        .join("d-ui");
    std::fs::write(
        dir_t.join(nemesis_connectors::registry::MANIFEST_FILE),
        r#"[connector]
id = "desktop-ui"
name = "Desktop UI"
version = "0.1.0"
kind = "local"
description = "桌面只读面"

[[op]]
id = "list_windows"
intent = ["枚举窗口"]
risk = "low"
class = "read"
tool = "mcp_my_window_enumerate_windows"
"#,
    )
    .unwrap();
    let ctx3 = make_ctx(ws3);
    let out3 = cmd_list(&ctx3).unwrap().unwrap();
    assert_eq!(
        row_of(&out3, "desktop-ui")["mcp_ready"],
        true,
        "sanitize 后前缀命中（my.window → mcp_my_window_）: {out3}"
    );
}

// ---------------------------------------------------------------------------
// 管理面（2026-10-19）：create / set_enabled / remove
// ---------------------------------------------------------------------------

const HANDLER_TOML: &str = r#"[connector]
id = "mng-c"
name = "管理面测试"
version = "0.1.0"
kind = "local"

[[op]]
id = "noop"
risk = "low"
class = "read"
action = "builtin.noop"
"#;

#[test]
fn management_commands_registered_in_surface() {
    let cmds = ConnectorsHandler.commands();
    assert_eq!(
        cmds.len(),
        8,
        "list/receipts.list/undo + create/set_enabled/remove + set_master/tool_policy"
    );
    assert!(cmds.contains(&"create"));
    assert!(cmds.contains(&"set_enabled"));
    assert!(cmds.contains(&"remove"));
    assert!(cmds.contains(&"set_master"));
    assert!(cmds.contains(&"tool_policy"));
}

#[tokio::test]
async fn management_roundtrip_create_toggle_remove() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = make_ctx(tmp.path());
    let h = ConnectorsHandler;

    // create → list 可见
    let r = h
        .handle_cmd(
            "create",
            Some(json!({ "dir_name": "mng-c", "toml": HANDLER_TOML })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["ok"], json!(true));
    assert_eq!(r["id"], json!("mng-c"));
    let list = h.handle_cmd("list", None, &ctx).await.unwrap().unwrap();
    assert_eq!(list["count"], json!(1));
    assert_eq!(list["connectors"][0]["id"], json!("mng-c"));

    // set_enabled false → list 反映停用
    let r = h
        .handle_cmd(
            "set_enabled",
            Some(json!({ "dir": "mng-c", "enabled": false })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["enabled"], json!(false));
    let list = h.handle_cmd("list", None, &ctx).await.unwrap().unwrap();
    assert_eq!(list["connectors"][0]["enabled"], json!(false));

    // remove → list 清空；目录消失
    let r = h
        .handle_cmd("remove", Some(json!({ "dir": "mng-c" })), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["receipts_retained"], json!(true));
    let list = h.handle_cmd("list", None, &ctx).await.unwrap().unwrap();
    assert_eq!(list["count"], json!(0));
}

#[tokio::test]
async fn management_param_and_registry_errors_are_honest() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = make_ctx(tmp.path());
    let h = ConnectorsHandler;

    // 缺参数
    assert!(h.handle_cmd("create", None, &ctx).await.is_err());
    assert!(
        h.handle_cmd("create", Some(json!({ "dir_name": "x" })), &ctx)
            .await
            .is_err()
    );
    assert!(
        h.handle_cmd("set_enabled", Some(json!({ "dir": "x" })), &ctx)
            .await
            .is_err(),
        "缺 enabled"
    );
    assert!(
        h.handle_cmd(
            "set_enabled",
            Some(json!({ "dir": "x", "enabled": "yes" })),
            &ctx
        )
        .await
        .is_err(),
        "enabled 必须是 bool"
    );
    assert!(h.handle_cmd("remove", None, &ctx).await.is_err());

    // 清单非法：错误原文透传（前端展示）
    let err = h
        .handle_cmd(
            "create",
            Some(json!({ "dir_name": "bad", "toml": "not toml [" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("TOML 解析失败"), "{err}");

    // 不在册目录启停/删除
    let err = h
        .handle_cmd(
            "set_enabled",
            Some(json!({ "dir": "ghost", "enabled": true })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("不在注册表"), "{err}");
    let err = h
        .handle_cmd("remove", Some(json!({ "dir": "ghost" })), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("不在注册表"), "{err}");
}

// ---------------------------------------------------------------------------
// B4 总开关（2026-10-19）：connectors.set_master
// ---------------------------------------------------------------------------

/// 关→config.json 真落键→list 反映；再开走「已有段更新」分支。缺省开
/// （无 config.json 时与供给闸同源语义）。
#[tokio::test]
async fn set_master_roundtrip_config_and_list() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = make_ctx(tmp.path());
    let h = ConnectorsHandler;

    // 缺省开。
    let list = h.handle_cmd("list", None, &ctx).await.unwrap().unwrap();
    assert_eq!(list["enabled"], json!(true));

    // 关：响应 + config.json + list 三处一致。
    let r = h
        .handle_cmd("set_master", Some(json!({ "enabled": false })), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r, json!({ "ok": true, "enabled": false }));
    let cfg: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(tmp.path().join("config.json")).unwrap())
            .unwrap();
    assert_eq!(cfg["connectors"]["enabled"], json!(false), "真落盘: {cfg}");
    let list = h.handle_cmd("list", None, &ctx).await.unwrap().unwrap();
    assert_eq!(list["enabled"], json!(false));

    // 再开：已有 connectors 段走更新分支。
    let r = h
        .handle_cmd("set_master", Some(json!({ "enabled": true })), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["enabled"], json!(true));
    let list = h.handle_cmd("list", None, &ctx).await.unwrap().unwrap();
    assert_eq!(list["enabled"], json!(true));
}

/// 参数面：缺 data / 缺 enabled / enabled 非 bool 都诚实拒（不静默）。
#[tokio::test]
async fn set_master_param_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = make_ctx(tmp.path());
    let h = ConnectorsHandler;

    assert!(h.handle_cmd("set_master", None, &ctx).await.is_err());
    assert!(
        h.handle_cmd("set_master", Some(json!({})), &ctx)
            .await
            .is_err(),
        "缺 enabled"
    );
    assert!(
        h.handle_cmd("set_master", Some(json!({ "enabled": "yes" })), &ctx)
            .await
            .is_err(),
        "enabled 必须是 bool"
    );
    // 拒绝路径不落盘。
    assert!(!tmp.path().join("config.json").exists());
}

// ---------------------------------------------------------------------------
// B2 per-tool 例外（2026-10-19）：connectors.tool_policy + mirrors 投影
// ---------------------------------------------------------------------------

/// 独占接管 connector 模板（接管 `stub` server）。
fn write_excl_connector(ws: &Path, dir: &str, id: &str) {
    let dir = ws
        .join(nemesis_connectors::registry::CONNECTORS_DIR)
        .join(dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(nemesis_connectors::registry::MANIFEST_FILE),
        format!(
            r#"[connector]
id = "{id}"
name = "独占壳"
version = "0.1.0"
kind = "local"
exclusive_mcp = "stub"

[[op]]
id = "noop"
risk = "low"
class = "read"
action = "builtin.noop"
"#
        ),
    )
    .unwrap();
}

/// blocked → list.mirrors 行投影 policy；exempt 覆盖；default 清记录。
/// harness 无 loop（快照空）= server 离线形态：行来自 sidecar 记录，
/// args_schema null 诚实展示。
#[tokio::test]
async fn tool_policy_writes_sidecar_and_list_projects_mirrors() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    write_excl_connector(ws, "x-ui", "excl-takeover");
    let ctx = make_ctx(ws);
    let h = ConnectorsHandler;

    // blocked 写入。
    let r = h
        .handle_cmd(
            "tool_policy",
            Some(json!({ "connector": "excl-takeover", "tool": "mcp_stub__click", "policy": "blocked" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["ok"], json!(true));
    assert_eq!(r["policy"], json!("blocked"));

    // list 投影：exclusive_mcp + mirrors 行（离线例外行，schema null）。
    let list = h.handle_cmd("list", None, &ctx).await.unwrap().unwrap();
    let row = row_of(&list, "excl-takeover");
    assert_eq!(row["exclusive_mcp"], json!(["stub"]));
    let mirrors = row["mirrors"].as_array().unwrap();
    assert_eq!(mirrors.len(), 1, "{row}");
    assert_eq!(mirrors[0]["id"], json!("mcp_stub__click"));
    assert_eq!(mirrors[0]["policy"], json!("blocked"));
    assert!(
        mirrors[0]["args_schema"].is_null(),
        "server 离线（无快照）= schema 缺席诚实展示"
    );

    // exempt 覆盖 blocked（豁免 = 更强的显式意志）。
    h.handle_cmd(
        "tool_policy",
        Some(
            json!({ "connector": "excl-takeover", "tool": "mcp_stub__click", "policy": "exempt" }),
        ),
        &ctx,
    )
    .await
    .unwrap()
    .unwrap();
    let list = h.handle_cmd("list", None, &ctx).await.unwrap().unwrap();
    let mirrors = row_of(&list, "excl-takeover")["mirrors"]
        .as_array()
        .unwrap();
    assert_eq!(mirrors[0]["policy"], json!("exempt"));

    // default → 记录清除 → sidecar 无行（快照也空）= mirrors 空表。
    h.handle_cmd(
        "tool_policy",
        Some(
            json!({ "connector": "excl-takeover", "tool": "mcp_stub__click", "policy": "default" }),
        ),
        &ctx,
    )
    .await
    .unwrap()
    .unwrap();
    let list = h.handle_cmd("list", None, &ctx).await.unwrap().unwrap();
    assert_eq!(
        row_of(&list, "excl-takeover")["mirrors"]
            .as_array()
            .unwrap()
            .len(),
        0,
        "全 default = 无例外行"
    );

    // 非独占 connector：mirrors 恒空。
    write_http_connector(ws, "a-rest", "demo-quotes");
    let list = h.handle_cmd("list", None, &ctx).await.unwrap().unwrap();
    assert_eq!(
        row_of(&list, "demo-quotes")["mirrors"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
}

/// 参数面：缺参数 / 未知 policy / 非法 connector id 诚实拒。
#[tokio::test]
async fn tool_policy_param_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = make_ctx(tmp.path());
    let h = ConnectorsHandler;

    assert!(h.handle_cmd("tool_policy", None, &ctx).await.is_err());
    assert!(
        h.handle_cmd(
            "tool_policy",
            Some(json!({ "connector": "c", "tool": "t" })),
            &ctx
        )
        .await
        .is_err(),
        "缺 policy"
    );
    let err = h
        .handle_cmd(
            "tool_policy",
            Some(json!({ "connector": "c", "tool": "t", "policy": "always" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("未知 policy"), "{err}");
    // 在册校验先行（边界补强 2026-10-20）：`../escape` 与任意不存在的
    // id 一样不在注册表——成员资格是最早、最安全的拒绝面（不拼路径）；
    // 字符集闸仍在下一层 state_path 兜底（其它调用方仍受它保护）。
    let err = h
        .handle_cmd(
            "tool_policy",
            Some(json!({ "connector": "../escape", "tool": "t", "policy": "blocked" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("不在注册表"), "{err}");
    let err = h
        .handle_cmd(
            "tool_policy",
            Some(json!({ "connector": "no-such-connector", "tool": "mcp_x__y", "policy": "blocked" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("不在注册表"), "{err}");
    // 幽灵 sidecar 不得落盘（空工作区无 connector_state 条目）。
    let state_dir = tmp.path().join("config").join("connector_state");
    assert!(
        !state_dir.exists() || std::fs::read_dir(&state_dir).unwrap().next().is_none(),
        "幽灵 sidecar 不得落盘"
    );
}
/// exclusive_tool_rows 直测：live 快照行（schema 随行 + 按当下 sidecar 裁
/// policy）∪ 离线例外行（sidecar 记录但不在 live 表）；前缀外不连坐；
/// 字典序。
#[test]
fn exclusive_tool_rows_projects_live_tools_and_offline_exceptions() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    // 目录名 = id（find_connector 按目录名定位）。
    write_excl_connector(ws, "excl-takeover", "excl-takeover");
    let c = nemesis_connectors::registry::find_connector(ws, "excl-takeover")
        .unwrap()
        .0;

    let live = vec![
        McpToolRow {
            name: "mcp_stub__read_page".to_string(),
            params: json!({ "type": "object", "required": ["url"] }),
            read_only_hint: Some(true),
            destructive_hint: None,
        },
        McpToolRow {
            name: "mcp_stub__click".to_string(),
            params: json!({ "type": "object" }),
            read_only_hint: None,
            destructive_hint: Some(true),
        },
        McpToolRow {
            name: "mcp_other__list".to_string(),
            params: json!({ "type": "object" }),
            read_only_hint: None,
            destructive_hint: None,
        },
    ];

    // 无例外：两行 default（前缀外 other 不连坐），schema 随行。
    let rows = exclusive_tool_rows(ws, &c, &live, None);
    let ids: Vec<&str> = rows.iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert_eq!(
        ids,
        vec!["mcp_stub__click", "mcp_stub__read_page"],
        "字典序"
    );
    assert_eq!(rows[0]["policy"], json!("default"));
    assert_eq!(
        rows[1]["args_schema"],
        json!({ "type": "object", "required": ["url"] })
    );
    // G1 标记（边界补强 2026-10-20）：无 loop 形态 = 配置隐藏标记恒 false
    // （行照列、不带假信号）；带 loop 时由 tool_matches_hidden_config 现裁。
    assert_eq!(rows[0]["hidden_by_config"], json!(false));

    // B3 四档基线随行：readOnly 自述=low/read 免卡；destructive 自述=
    // high/write 强制确认卡。
    assert_eq!(rows[0]["risk"], json!("high"));
    assert_eq!(rows[0]["class"], json!("write"));
    assert_eq!(rows[0]["confirm_required"], json!(true));
    assert_eq!(rows[1]["risk"], json!("low"));
    assert_eq!(rows[1]["class"], json!("read"));
    assert_eq!(rows[1]["confirm_required"], json!(false));

    // exempt read_page（live 行翻 exempt）+ blocked ghost（离线例外行）。
    nemesis_connectors::state::set_tool_policy(
        ws,
        "excl-takeover",
        "mcp_stub__read_page",
        nemesis_connectors::state::ToolPolicy::Exempt,
    )
    .unwrap();
    nemesis_connectors::state::set_tool_policy(
        ws,
        "excl-takeover",
        "mcp_stub__ghost",
        nemesis_connectors::state::ToolPolicy::Blocked,
    )
    .unwrap();
    let rows = exclusive_tool_rows(ws, &c, &live, None);
    let ids: Vec<&str> = rows.iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert_eq!(
        ids,
        vec!["mcp_stub__click", "mcp_stub__ghost", "mcp_stub__read_page"],
        "live 行 ∪ 离线例外行，字典序"
    );
    let ghost = rows
        .iter()
        .find(|r| r["id"] == json!("mcp_stub__ghost"))
        .unwrap();
    assert_eq!(ghost["policy"], json!("blocked"));
    assert!(ghost["args_schema"].is_null(), "离线行 schema 缺席");
    let exempt_row = rows
        .iter()
        .find(|r| r["id"] == json!("mcp_stub__read_page"))
        .unwrap();
    assert_eq!(exempt_row["policy"], json!("exempt"));
}
