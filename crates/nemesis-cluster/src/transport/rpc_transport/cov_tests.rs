// transport/rpc_transport.rs 覆盖率补充测试（非匹配响应跳过臂：先收到
// id 不符的杂音响应，再收到真响应 → 98 的 if 闭合臂被执行）。

use super::*;
use crate::rpc_types::ActionType;
use crate::transport::conn::{TcpConn, TcpConnConfig, WireMessage};
use crate::transport::pool::AsyncPoolConfig;
use std::time::Duration;

/// 杂音先行：服务端先回一条 id 不匹配的响应（客户端跳过），再回真响应
/// （客户端匹配返回）。覆盖匹配 if 的假臂闭合区（98）。
#[tokio::test]
async fn call_skips_non_matching_response_then_matches() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let server_addr = addr.clone();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut conn = TcpConn::new(
            stream,
            TcpConnConfig {
                address: server_addr.clone(),
                ..Default::default()
            },
        );
        conn.start().await.unwrap();

        let msg = conn.receive().await.unwrap();
        // 杂音：id 不同 → 客户端必须跳过而不是误配。
        let mut decoy = WireMessage::new_response(&msg, serde_json::json!({"decoy": true}));
        decoy.id = "decoy-id".into();
        conn.send(&decoy).await.unwrap();
        // 真响应：id 一致 → 匹配返回。
        let resp = WireMessage::new_response(&msg, serde_json::json!({"status": "ok"}));
        conn.send(&resp).await.unwrap();
        tokio::time::sleep(Duration::from_millis(80)).await;
    });

    let pool = Pool::new(AsyncPoolConfig {
        dial_timeout: Duration::from_secs(5),
        ..Default::default()
    });
    let transport = RpcTransport::with_pool(pool);

    let request = RPCRequest {
        id: "cov-req-skip".into(),
        action: ActionType::Known(crate::rpc_types::KnownAction::Ping),
        payload: serde_json::json!({}),
        source: "cov-client".into(),
        target: Some("cov-server".into()),
    };

    let response = transport.call("cov-server", &addr, request).await.unwrap();
    assert_eq!(
        response.result.as_ref().unwrap()["status"],
        "ok",
        "必须拿到真响应而不是杂音"
    );

    server.await.unwrap();
}
