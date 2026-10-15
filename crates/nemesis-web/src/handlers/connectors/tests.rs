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
    let err = corrupt["error"].as_str().unwrap();
    assert!(!err.is_empty(), "损坏原因必须可见: {corrupt}");
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
    assert_eq!(h.commands(), &["list", "receipts.list"]);
    let out = h.handle_cmd("list", None, &ctx).await.unwrap().unwrap();
    assert!(out["connectors"].is_array());
    let err = h.handle_cmd("nope", None, &ctx).await.unwrap_err();
    assert!(err.contains("unknown command: connectors.nope"), "{err}");

    let p = PaperBrokerHandler;
    assert_eq!(p.module_name(), "paperbroker");
    assert_eq!(p.commands(), &["ledger"]);
    let out = p.handle_cmd("ledger", None, &ctx).await.unwrap().unwrap();
    assert_eq!(out["cash"], 1_000_000.0);
    let err = p.handle_cmd("nope", None, &ctx).await.unwrap_err();
    assert!(err.contains("unknown command: paperbroker.nope"), "{err}");
}
