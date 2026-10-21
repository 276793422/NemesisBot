//! `deliveries.*` WSAPI 用例（P4-c）：未装配诚实报错 / 各命令臂经 bridge /
//! list 视图不带 patch 体 / L1 `commands()` 清单。进程级 bridge 槽是共享
//! 态——全部用例持 [`DELIVERIES_BRIDGE_TEST_LOCK`] 串行（同 AGENTS_BRIDGE
//! 家族纪律）。
// 持锁跨 await 是刻意的测试串行化（槽位置换必须覆盖整个 handler await
// 窗口，否则并行用例互相置换 bridge 假绿）；本进程内无其他同锁取用者，
// 无死锁面。agents_ws_tests 同款豁免。
#![allow(clippy::await_holding_lock)]

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::RequestContext;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

/// 最小 RequestContext（同 agents_ws_tests make_ctx 形态；deliveries
/// handler 不消费 ctx 状态，全字段 None/空即可）。TempDir 当场 drop——
/// 只用字符串。
fn make_ctx() -> RequestContext {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: Some(ws.clone()),
        home: Some(ws.clone()),
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new("m".to_string())),
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
        session_id: "s".to_string(),
        chat_id: "c".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

/// 假 bridge（录制调用 + 固定应答）——handler 只消费 trait 面，不依赖
/// nemesisbot（依赖方向不许反向）。
struct FakeBridge {
    calls: Mutex<Vec<String>>,
    reject_reason: Mutex<Option<String>>,
}

impl FakeBridge {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            reject_reason: Mutex::new(None),
        })
    }
    fn note(&self, c: &str) {
        self.calls.lock().unwrap().push(c.to_string());
    }
}

fn fake_info(key: &str, with_patch: bool) -> DeliveryInfo {
    DeliveryInfo {
        id: format!("d-{key}"),
        run_id: Some("t-42".to_string()),
        kind: "branch".to_string(),
        idempotency_key: key.to_string(),
        status: "delivered".to_string(),
        created_at: "2026-10-17T00:00:00+08:00".to_string(),
        delivered_at: Some("2026-10-17T00:00:00+08:00".to_string()),
        confirmed_at: None,
        rejected_at: None,
        reject_reason: None,
        notify_state: "pending".to_string(),
        notify_attempts: 0,
        notify_last_error: None,
        branch: Some("run-t-42".to_string()),
        node: Some("node-a".to_string()),
        files_changed: Some(2),
        insertions: Some(10),
        deletions: Some(1),
        patch_truncated: Some(false),
        patch_path: Some("logs/deliveries/t-42.patch".to_string()),
        patch_present: with_patch,
        commit: Some("abc123".to_string()),
        head_tree: Some("tree-42".to_string()),
        base_tree: Some("tree-41".to_string()),
    }
}

impl DeliveriesBridge for FakeBridge {
    fn list(&self, limit: usize) -> Vec<DeliveryInfo> {
        self.note("list");
        assert!(
            (1..=500).contains(&limit),
            "handler 必须钳制 limit: {limit}"
        );
        vec![fake_info("k1", false), fake_info("k2", true)]
    }
    fn by_run(&self, run_id: &str) -> Vec<DeliveryInfo> {
        self.note("by_run");
        assert_eq!(run_id, "t-42");
        vec![fake_info("k1", false)]
    }
    fn get(&self, idempotency_key: &str) -> Result<DeliveryInfo, String> {
        self.note("get");
        if idempotency_key == "k1" {
            Ok(fake_info(idempotency_key, false))
        } else {
            Err(format!("交付不存在: {idempotency_key}"))
        }
    }
    fn confirm(&self, idempotency_key: &str) -> Result<bool, String> {
        self.note("confirm");
        if idempotency_key == "k1" {
            Ok(true)
        } else {
            Ok(false) // 已 confirmed（幂等）
        }
    }
    fn reject(&self, idempotency_key: &str, reason: &str) -> Result<bool, String> {
        self.note("reject");
        *self.reject_reason.lock().unwrap() = Some(reason.to_string());
        Ok(idempotency_key == "k1")
    }
    fn patch(&self, idempotency_key: &str) -> Result<String, String> {
        self.note("patch");
        if idempotency_key == "k2" {
            Ok("diff --git a/f b/f\n+line\n".to_string())
        } else {
            Err("交付 payload 无内联 patch 文本。完整 patch 在 worker 侧留底".to_string())
        }
    }
}

async fn call(
    cmd: &str,
    data: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    let ctx = make_ctx();
    let handler = DeliveriesHandler;
    handler.handle_cmd(cmd, data, &ctx).await
}

#[tokio::test]
async fn uninstalled_bridge_is_honest_error() {
    let _g = DELIVERIES_BRIDGE_TEST_LOCK.lock();
    set_deliveries_bridge_for_test(None);
    let err = call("list", None).await.expect_err("未装配必须诚实报错");
    assert!(err.contains("未装配"), "错误文案应指明未装配: {err}");
}

#[tokio::test]
async fn list_returns_deliveries_count_and_payload_summary() {
    let _g = DELIVERIES_BRIDGE_TEST_LOCK.lock();
    set_deliveries_bridge_for_test(Some(FakeBridge::new()));
    let out = call("list", None).await.unwrap().unwrap();
    assert_eq!(out["count"], 2);
    let rows = out["deliveries"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    // payload 展开字段在顶层可见。
    assert_eq!(rows[0]["branch"], "run-t-42");
    assert_eq!(rows[0]["files_changed"], 2);
    assert_eq!(rows[0]["insertions"], 10);
    assert_eq!(rows[0]["deletions"], 1);
    assert_eq!(rows[0]["patch_present"], false);
    // limit 显式传参照常透传（clamp 边界在 handler）。
    let out = call("list", Some(serde_json::json!({"limit": 5})))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["count"], 2);
    set_deliveries_bridge_for_test(None);
}

#[tokio::test]
async fn by_run_returns_run_scoped_deliveries() {
    let _g = DELIVERIES_BRIDGE_TEST_LOCK.lock();
    set_deliveries_bridge_for_test(Some(FakeBridge::new()));
    let out = call("by_run", Some(serde_json::json!({"run_id": "t-42"})))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["run_id"], "t-42");
    assert_eq!(out["count"], 1);
    assert_eq!(out["deliveries"][0]["idempotency_key"], "k1");
    // 缺 run_id → 参数层报错。
    let err = call("by_run", Some(serde_json::json!({})))
        .await
        .expect_err("缺参必须报错");
    assert!(err.contains("run_id"), "缺参文案应点名字段: {err}");
    set_deliveries_bridge_for_test(None);
}

#[tokio::test]
async fn get_missing_key_propagates_bridge_error() {
    let _g = DELIVERIES_BRIDGE_TEST_LOCK.lock();
    set_deliveries_bridge_for_test(Some(FakeBridge::new()));
    let err = call("get", Some(serde_json::json!({"idempotency_key": "nope"})))
        .await
        .expect_err("不存在必须报错");
    assert!(err.contains("不存在"), "got: {err}");
    let out = call("get", Some(serde_json::json!({"idempotency_key": "k1"})))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["delivery"]["status"], "delivered");
    assert_eq!(out["delivery"]["patch_present"], false);
    set_deliveries_bridge_for_test(None);
}

#[tokio::test]
async fn confirm_reports_first_vs_idempotent() {
    let _g = DELIVERIES_BRIDGE_TEST_LOCK.lock();
    set_deliveries_bridge_for_test(Some(FakeBridge::new()));
    let out = call(
        "confirm",
        Some(serde_json::json!({"idempotency_key": "k1"})),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["confirmed"], true);
    assert_eq!(out["status"], "confirmed");
    // 幂等臂：已确认 = false + note 说明无变化。
    let out = call(
        "confirm",
        Some(serde_json::json!({"idempotency_key": "k2"})),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["confirmed"], false);
    let note = out["note"].as_str().unwrap();
    assert!(note.contains("幂等"), "note 应说明幂等: {note}");
    set_deliveries_bridge_for_test(None);
}

#[tokio::test]
async fn reject_carries_reason_and_defaults_when_blank() {
    let _g = DELIVERIES_BRIDGE_TEST_LOCK.lock();
    let bridge = FakeBridge::new();
    set_deliveries_bridge_for_test(Some(bridge.clone()));
    // 显式原因透传。
    let out = call(
        "reject",
        Some(serde_json::json!({"idempotency_key": "k1", "reason": "锚点双检 FAIL"})),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["rejected"], true);
    assert_eq!(out["reason"], "锚点双检 FAIL");
    // 空白原因 → 兜底占位（留痕不允许空原因）。
    let out = call(
        "reject",
        Some(serde_json::json!({"idempotency_key": "k1", "reason": "  "})),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["reason"], "(未填写原因)");
    // reason 缺省同兜底。
    let out = call("reject", Some(serde_json::json!({"idempotency_key": "k1"})))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["reason"], "(未填写原因)");
    assert_eq!(
        *bridge.reject_reason.lock().unwrap(),
        Some("(未填写原因)".to_string())
    );
    set_deliveries_bridge_for_test(None);
}

#[tokio::test]
async fn patch_returns_text_or_honest_hint() {
    let _g = DELIVERIES_BRIDGE_TEST_LOCK.lock();
    set_deliveries_bridge_for_test(Some(FakeBridge::new()));
    let out = call("patch", Some(serde_json::json!({"idempotency_key": "k2"})))
        .await
        .unwrap()
        .unwrap();
    assert!(
        out["patch"].as_str().unwrap().starts_with("diff --git"),
        "patch 文本应原样返回"
    );
    let err = call("patch", Some(serde_json::json!({"idempotency_key": "k1"})))
        .await
        .expect_err("无内联 patch 必须报错");
    assert!(err.contains("留底"), "错误应带指引: {err}");
    set_deliveries_bridge_for_test(None);
}

#[tokio::test]
async fn unknown_cmd_is_rejected() {
    let _g = DELIVERIES_BRIDGE_TEST_LOCK.lock();
    set_deliveries_bridge_for_test(Some(FakeBridge::new()));
    let err = call("nope", None).await.expect_err("未知命令必须报错");
    assert!(err.contains("unknown deliveries cmd"), "got: {err}");
    set_deliveries_bridge_for_test(None);
}

#[test]
fn l1_commands_registry_lists_all_deliveries_commands() {
    let handler = DeliveriesHandler;
    assert_eq!(handler.module_name(), "deliveries");
    let cmds = handler.commands();
    assert_eq!(
        cmds,
        &["list", "by_run", "get", "confirm", "reject", "patch"]
    );
}
