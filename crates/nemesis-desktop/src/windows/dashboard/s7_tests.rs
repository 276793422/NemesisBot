//! S7 冲刺覆盖测试：dashboard.rs 中 ws_client 存在时 startup() 注册的
//! 4 个处理器（3 个通知 + 1 个 system.ping 请求）。
//!
//! 既有测试全部用 `None` 作为 ws_client，所以这些注册分支从未执行。
//! 用一个未连接的 WebSocketClient（处理器注册只落在 dispatcher 上，
//! 不需要真实连接）覆盖注册路径，并通过 dispatcher 直接派发验证行为。

use std::sync::Arc;

use super::*;
use crate::websocket::client::{WebSocketClient, WebSocketKey};
use crate::websocket::protocol::Message;

fn make_client() -> Arc<WebSocketClient> {
    Arc::new(WebSocketClient::new(&WebSocketKey {
        key: "s7-key".to_string(),
        port: 49000,
        path: "/ws".to_string(),
    }))
}

fn make_dashboard_data() -> DashboardWindowData {
    DashboardWindowData {
        token: "s7-token".to_string(),
        web_port: 8080,
        web_host: "127.0.0.1".to_string(),
    }
}

#[test]
fn s7_startup_with_ws_client_registers_dashboard_handlers() {
    let client = make_client();
    let window = DashboardWindow::new(
        "s7-window".to_string(),
        make_dashboard_data(),
        Some(client.clone()),
    );
    assert!(window.startup().is_ok());

    let dispatcher = client.dispatcher();

    // 3 个通知处理器：派发为通知（无 id + method），返回 Ok(None)。
    for method in ["window.bring_to_front", "window.minimize", "state.service_status"] {
        let note = Message::new_notification(method, serde_json::Value::Null);
        let result = dispatcher.dispatch(&note);
        assert!(
            result.is_ok(),
            "notification dispatch failed for {}: {:?}",
            method,
            result.err()
        );
    }

    // system.ping 请求处理器：必须返回 status=ok 且回显请求 id。
    let req = Message::new_request("system.ping", serde_json::Value::Null);
    let resp = dispatcher
        .dispatch(&req)
        .expect("ping dispatch failed")
        .expect("ping handler must return a response");
    assert_eq!(resp.id.as_deref(), Some(req.id.as_deref().unwrap()));
    assert_eq!(resp.result.unwrap()["status"], "ok");

    window.shutdown();
}
