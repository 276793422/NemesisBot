//! 桥客户端状态槽测试（批次三）。
//!
//! CLIENT_STATUS / RECONNECT_KICK 是进程级全局——并行测试会互相踩。
//! 全部用例经 [`GLOBAL_TEST_LOCK`] 串行化；每个用例先把槽打到已知
//! 初态，断言后清理（防泄漏进同进程其他 relay 测试）。

use super::client_status::*;

/// 串行化闸（槽是全局的，测试必须互斥）。tokio Mutex：async 用例
/// `.lock().await` 跨 await 持锁（clippy await_holding_lock 只针对 std
/// guard），同步用例走 `blocking_lock()`。
static GLOBAL_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 测试结束清槽（Drop 保 panic 路径也清理）。
struct SlotGuard;
impl Drop for SlotGuard {
    fn drop(&mut self) {
        if let Ok(mut slot) = CLIENT_STATUS.lock() {
            *slot = None;
        }
    }
}

fn sample_status() -> BridgeClientStatus {
    BridgeClientStatus {
        enabled: true,
        state: BridgeClientState::Connecting,
        relay_url: "ws://127.0.0.1:60600".to_string(),
        node_id: "bridge-testnode".to_string(),
        last_error: None,
        updated_at: 1700000000,
    }
}

#[test]
fn test_report_and_read_roundtrip() {
    let _lock = GLOBAL_TEST_LOCK.blocking_lock();
    let _guard = SlotGuard;

    assert!(client_status().is_none(), "初态必须为空（未运行客户端）");
    report_client_status(sample_status());
    let snap = client_status().expect("上报后必须可读");
    assert!(snap.enabled);
    assert_eq!(snap.state, BridgeClientState::Connecting);
    assert_eq!(snap.relay_url, "ws://127.0.0.1:60600");
    assert_eq!(snap.node_id, "bridge-testnode");
    assert!(snap.last_error.is_none());
}

#[test]
fn test_report_state_updates_in_place() {
    let _lock = GLOBAL_TEST_LOCK.blocking_lock();
    let _guard = SlotGuard;

    report_client_status(sample_status());
    // 状态迁移：connected → disconnected（带错误）→ 展示字段保留。
    report_client_state(BridgeClientState::Disconnected, Some("timeout".into()));
    let snap = client_status().unwrap();
    assert_eq!(snap.state, BridgeClientState::Disconnected);
    assert_eq!(snap.last_error.as_deref(), Some("timeout"));
    assert_eq!(snap.relay_url, "ws://127.0.0.1:60600", "展示字段不能丢");
    assert_eq!(snap.node_id, "bridge-testnode");
    assert!(snap.updated_at >= 1700000000, "updated_at 刷新为当前时刻");
}

#[test]
fn test_report_state_ignored_when_slot_empty() {
    let _lock = GLOBAL_TEST_LOCK.blocking_lock();
    let _guard = SlotGuard;

    // 槽为空时增量上报必须静默忽略（无底可改，且不能 panic）。
    report_client_state(BridgeClientState::Connected, None);
    assert!(client_status().is_none());
}

#[test]
fn test_state_as_str() {
    assert_eq!(BridgeClientState::Connecting.as_str(), "connecting");
    assert_eq!(BridgeClientState::Connected.as_str(), "connected");
    assert_eq!(BridgeClientState::Rejected.as_str(), "rejected");
    assert_eq!(BridgeClientState::Disconnected.as_str(), "disconnected");
}

#[tokio::test]
async fn test_kick_wakes_notified_waiter() {
    let _lock = GLOBAL_TEST_LOCK.lock().await;
    let _guard = SlotGuard;

    // kick 语义：先订阅后 kick，notified() 立即完成（不等待）。
    let notify = reconnect_notify();
    let waiter = tokio::spawn(async move { notify.notified().await });
    // 让 waiter 先跑到 notified() 注册点（yield 几轮足够）。
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;
    kick_reconnect();
    tokio::time::timeout(std::time::Duration::from_secs(2), waiter)
        .await
        .expect("kick 后 waiter 必须被唤醒")
        .unwrap();
}

#[tokio::test]
async fn test_kick_permit_survives_until_subscribed() {
    let _lock = GLOBAL_TEST_LOCK.lock().await;
    let _guard = SlotGuard;

    // 先 kick（无订阅者）→ 后订阅：permit 存活，notified() 立即完成。
    // 这正是「客户端未运行时点重连」的语义：permit 留给下次 spawn。
    kick_reconnect();
    let notify = reconnect_notify();
    tokio::select! {
        _ = notify.notified() => {} // 期望：立即走这条
        _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
            panic!("kick 的 permit 必须留到下一次 notified() 立即消费");
        }
    }
}

// ---------------------------------------------------------------------------
// 批次三通道页端点（handler 直调——路由挂载形态见 server.rs，直调足以
// 覆盖 JSON 语义；与 /api/relay/status 的 handler 测试同思路）
// ---------------------------------------------------------------------------

use axum::body::Body;
use axum::http::Request;

async fn overview_json(
    relay: Option<std::sync::Arc<crate::relay::RelayServer>>,
) -> serde_json::Value {
    let resp = crate::relay::handle_relay_api_overview(relay).await;
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn test_overview_relay_none_client_null() {
    let _lock = GLOBAL_TEST_LOCK.lock().await;
    let _guard = SlotGuard;

    // relay 未配置 + 槽空：server/client 双 null（前端显示「未配置/未启用」）。
    let json = overview_json(None).await;
    assert!(json["server"].is_null());
    assert!(json["client"].is_null());
}

#[tokio::test]
async fn test_overview_merges_server_and_client() {
    let _lock = GLOBAL_TEST_LOCK.lock().await;
    let _guard = SlotGuard;

    report_client_status(sample_status());
    let relay = std::sync::Arc::new(crate::relay::RelayServer::new("tok".to_string(), true));
    let json = overview_json(Some(relay.clone())).await;
    // 服务端态（与状态页 /api/relay/status 同源：is_enabled/is_full_mode/devices）
    assert_eq!(json["server"]["enabled"], relay.is_enabled());
    assert_eq!(json["server"]["full_mode"], true);
    assert!(json["server"]["devices"].is_array());
    // 客户端态
    assert_eq!(json["client"]["state"], "connecting");
    assert_eq!(json["client"]["node_id"], "bridge-testnode");
    assert_eq!(json["client"]["relay_url"], "ws://127.0.0.1:60600");
}

#[tokio::test]
async fn test_enabled_runtime_toggle_only() {
    let relay = std::sync::Arc::new(crate::relay::RelayServer::new("tok".to_string(), true));
    let req = Request::builder()
        .method("POST")
        .uri("/api/relay/enabled")
        .body(Body::from(r#"{"on": false}"#))
        .unwrap();
    let resp = crate::relay::handle_relay_api_enabled(Some(relay.clone()), req).await;
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    // 运行时开关真被翻下去了
    assert!(!relay.is_enabled());
    let req = Request::builder()
        .method("POST")
        .uri("/api/relay/enabled")
        .body(Body::from(r#"{"on": true}"#))
        .unwrap();
    let resp = crate::relay::handle_relay_api_enabled(Some(relay.clone()), req).await;
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    assert!(relay.is_enabled());
}

#[tokio::test]
async fn test_enabled_relay_none_is_bad_request() {
    let req = Request::builder()
        .method("POST")
        .uri("/api/relay/enabled")
        .body(Body::from(r#"{"on": false}"#))
        .unwrap();
    let resp = crate::relay::handle_relay_api_enabled(None, req).await;
    assert_eq!(resp.status(), axum::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_enabled_missing_body_is_bad_request() {
    let relay = std::sync::Arc::new(crate::relay::RelayServer::new("tok".to_string(), true));
    let req = Request::builder()
        .method("POST")
        .uri("/api/relay/enabled")
        .body(Body::from("not json"))
        .unwrap();
    let resp = crate::relay::handle_relay_api_enabled(Some(relay), req).await;
    assert_eq!(resp.status(), axum::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_client_reconnect_returns_ok() {
    // handler 本身只是 kick 转发（kick/permit 语义已有专测）——断言 200 即可。
    let resp = crate::relay::handle_relay_api_client_reconnect().await;
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
}
