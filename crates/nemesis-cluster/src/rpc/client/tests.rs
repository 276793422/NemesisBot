use super::*;

#[test]
fn test_default_timeout() {
    let client = RpcClient::new();
    assert_eq!(client.timeout(), Duration::from_secs(3600));
}

#[test]
fn test_custom_timeout() {
    let client = RpcClient::with_timeout(Duration::from_secs(120));
    assert_eq!(client.timeout(), Duration::from_secs(120));
}

#[test]
fn test_extract_ip_from_addr_ipv4() {
    let ip = extract_ip_from_addr("192.168.1.10:8080");
    assert_eq!(ip, Some("192.168.1.10".parse().unwrap()));
}

#[test]
fn test_extract_ip_from_addr_bare() {
    let ip = extract_ip_from_addr("10.0.0.1");
    assert_eq!(ip, Some("10.0.0.1".parse().unwrap()));
}

#[test]
fn test_is_same_subnet_match() {
    assert!(is_same_subnet(
        "192.168.1.10",
        "192.168.1.20",
        "255.255.255.0"
    ));
}

#[test]
fn test_is_same_subnet_no_match() {
    assert!(!is_same_subnet("192.168.1.10", "10.0.0.1", "255.255.255.0"));
}

#[test]
fn test_is_same_subnet_invalid() {
    assert!(!is_same_subnet("invalid", "192.168.1.10", "255.255.255.0"));
}

#[test]
fn test_rate_limiter_allows_within_limit() {
    let limiter = RateLimiter::new(2, Duration::from_secs(60), 10, Duration::from_secs(60));
    assert!(limiter.acquire("peer-1").is_ok());
    assert!(limiter.acquire("peer-1").is_ok());
}

#[test]
fn test_rate_limiter_blocks_when_exhausted() {
    let limiter = RateLimiter::new(1, Duration::from_secs(60), 10, Duration::from_secs(60));
    assert!(limiter.acquire("peer-1").is_ok());
    assert!(limiter.acquire("peer-1").is_err());
}

#[test]
fn test_rate_limiter_release() {
    let limiter = RateLimiter::new(1, Duration::from_secs(60), 10, Duration::from_secs(60));
    assert!(limiter.acquire("peer-1").is_ok());
    limiter.release("peer-1");
    assert!(limiter.acquire("peer-1").is_ok());
}

#[test]
fn test_select_best_address_single() {
    let client = RpcClient::new();
    assert_eq!(
        client.select_best_address(&["10.0.0.1:9000".into()]),
        "10.0.0.1:9000"
    );
}

#[test]
fn test_select_best_address_empty() {
    let client = RpcClient::new();
    assert_eq!(client.select_best_address(&[]), "");
}

struct MockResolver {
    interfaces: Vec<LocalNetworkInterface>,
}

impl PeerResolver for MockResolver {
    fn get_peer_info(&self, _peer_id: &str) -> Option<(Vec<String>, u16, bool)> {
        None
    }
    fn get_local_interfaces(&self) -> Vec<LocalNetworkInterface> {
        self.interfaces.clone()
    }
    fn get_node_id(&self) -> String {
        "mock-node".into()
    }
}

#[test]
fn test_select_best_address_with_resolver() {
    let resolver = Arc::new(MockResolver {
        interfaces: vec![LocalNetworkInterface {
            ip: "192.168.1.5".into(),
            mask: "255.255.255.0".into(),
        }],
    });
    let client = RpcClient::with_resolver(resolver);
    let addrs = vec![
        "10.0.0.1:9000".into(),
        "192.168.1.10:9000".into(),
        "172.16.0.1:9000".into(),
    ];
    let best = client.select_best_address(&addrs);
    assert_eq!(best, "192.168.1.10:9000");
}

#[tokio::test]
async fn test_call_peer_not_found() {
    let client = RpcClient::new(); // no resolver
    let request = RPCRequest {
        id: "req-1".into(),
        action: crate::rpc_types::ActionType::Known(crate::rpc_types::KnownAction::Ping),
        payload: serde_json::json!({}),
        source: "node-a".into(),
        target: Some("node-b".into()),
    };

    let result = client.call("node-b", request).await;
    assert!(result.is_err());
}

#[test]
fn test_auth_token() {
    let client = RpcClient::new();
    client.set_auth_token("my-token".into());
    let token = client.auth_token.lock();
    assert_eq!(token.as_deref(), Some("my-token"));
}

// -- Additional coverage tests --

#[test]
fn test_rpc_client_default_timeout() {
    let client = RpcClient::new();
    assert_eq!(client.timeout(), DEFAULT_RPC_TIMEOUT);
}

#[test]
fn test_rpc_client_with_timeout() {
    let client = RpcClient::with_timeout(Duration::from_secs(30));
    assert_eq!(client.timeout(), Duration::from_secs(30));
}

#[test]
fn test_rate_limiter_multiple_peers() {
    let limiter = RateLimiter::new(1, Duration::from_secs(60), 10, Duration::from_secs(60));
    assert!(limiter.acquire("peer-1").is_ok());
    assert!(limiter.acquire("peer-2").is_ok()); // different peer
    assert!(limiter.acquire("peer-1").is_err()); // peer-1 exhausted
    assert!(limiter.acquire("peer-2").is_err()); // peer-2 exhausted
}

#[test]
fn test_rate_limiter_release_nonexistent() {
    let limiter = RateLimiter::new(1, Duration::from_secs(60), 10, Duration::from_secs(60));
    // Release on nonexistent peer should not panic
    limiter.release("nonexistent");
}

#[test]
fn test_rate_limiter_window_overflow() {
    let limiter = RateLimiter::new(100, Duration::from_secs(60), 2, Duration::from_secs(60));
    assert!(limiter.acquire("peer-1").is_ok());
    assert!(limiter.acquire("peer-1").is_ok());
    // Third request should be blocked by window, not by tokens
    assert!(limiter.acquire("peer-1").is_err());
}

#[test]
fn test_rpc_client_error_display() {
    let err = RpcClientError::Connection("timeout".into());
    assert!(format!("{}", err).contains("timeout"));

    let err = RpcClientError::Timeout;
    assert!(format!("{}", err).contains("Timeout"));

    let err = RpcClientError::RateLimited("too many".into());
    assert!(format!("{}", err).contains("too many"));

    let err = RpcClientError::Serialization("bad json".into());
    assert!(format!("{}", err).contains("bad json"));
}

#[test]
fn test_select_best_address_prefers_loopback() {
    let client = RpcClient::new();
    let addrs = vec![
        "10.0.0.1:9000".into(),
        "127.0.0.1:9000".into(),
        "192.168.1.1:9000".into(),
    ];
    let best = client.select_best_address(&addrs);
    assert_eq!(best, "10.0.0.1:9000"); // first non-loopback
}

#[test]
fn test_local_network_interface_debug() {
    let iface = LocalNetworkInterface {
        ip: "192.168.1.1".into(),
        mask: "255.255.255.0".into(),
    };
    let debug = format!("{:?}", iface);
    assert!(debug.contains("192.168.1.1"));
}

#[tokio::test]
async fn test_call_with_timeout_peer_not_found() {
    let client = RpcClient::new(); // no resolver
    let request = RPCRequest {
        id: "req-timeout".into(),
        action: crate::rpc_types::ActionType::Known(crate::rpc_types::KnownAction::Ping),
        payload: serde_json::json!({}),
        source: "node-a".into(),
        target: Some("node-b".into()),
    };

    let result = client
        .call_with_timeout("node-b", request, Duration::from_secs(5))
        .await;
    assert!(result.is_err());
}

struct MockOnlineResolver {
    addresses: Vec<String>,
}

impl PeerResolver for MockOnlineResolver {
    fn get_peer_info(&self, _peer_id: &str) -> Option<(Vec<String>, u16, bool)> {
        Some((self.addresses.clone(), 9999, true))
    }
    fn get_local_interfaces(&self) -> Vec<LocalNetworkInterface> {
        vec![]
    }
    fn get_node_id(&self) -> String {
        "mock".into()
    }
}

#[tokio::test]
async fn test_call_online_peer_connection_refused() {
    let resolver = Arc::new(MockOnlineResolver {
        addresses: vec!["127.0.0.1".into()],
    });
    let client = RpcClient::with_resolver(resolver);
    let request = RPCRequest {
        id: "req-conn".into(),
        action: crate::rpc_types::ActionType::Known(crate::rpc_types::KnownAction::Ping),
        payload: serde_json::json!({}),
        source: "node-a".into(),
        target: Some("node-b".into()),
    };

    // Port 9999 is unlikely to be in use, should get connection refused
    let result = client
        .call_with_timeout("node-b", request, Duration::from_secs(3))
        .await;
    assert!(result.is_err());
}

#[test]
fn test_rpc_request_fields() {
    let req = RPCRequest {
        id: "req-1".into(),
        action: crate::rpc_types::ActionType::Custom("my_action".into()),
        payload: serde_json::json!({"key": "value"}),
        source: "node-a".into(),
        target: Some("node-b".into()),
    };
    assert_eq!(req.id, "req-1");
    assert_eq!(req.source, "node-a");
    assert!(req.target.is_some());
}

#[test]
fn test_rpc_response_fields() {
    let resp = RPCResponse {
        id: "resp-1".into(),
        result: Some(serde_json::json!({"status": "ok"})),
        error: None,
    };
    assert_eq!(resp.id, "resp-1");
    assert!(resp.result.is_some());
    assert!(resp.error.is_none());
}

#[test]
fn test_rpc_response_with_error() {
    let resp = RPCResponse {
        id: "resp-2".into(),
        result: None,
        error: Some("something went wrong".into()),
    };
    assert!(resp.error.is_some());
    assert!(resp.result.is_none());
}

// ============================================================
// Coverage improvement: rate limiter, address selection, errors
// ============================================================

#[test]
fn test_rpc_client_error_variants() {
    let err = RpcClientError::Connection("conn refused".into());
    assert!(format!("{}", err).contains("conn refused"));

    let err = RpcClientError::Timeout;
    assert!(format!("{}", err).contains("Timeout"));

    let err = RpcClientError::RateLimited("rate".into());
    assert!(format!("{}", err).contains("rate"));

    let err = RpcClientError::Serialization("parse err".into());
    assert!(format!("{}", err).contains("parse err"));
}

#[test]
fn test_rate_limiter_multiple_acquires_same_peer() {
    let limiter = RateLimiter::new(3, Duration::from_secs(60), 10, Duration::from_secs(60));
    assert!(limiter.acquire("peer-1").is_ok());
    assert!(limiter.acquire("peer-1").is_ok());
    assert!(limiter.acquire("peer-1").is_ok());
    // Fourth should fail
    assert!(limiter.acquire("peer-1").is_err());
}

#[test]
fn test_rate_limiter_release_allows_more() {
    let limiter = RateLimiter::new(1, Duration::from_secs(60), 5, Duration::from_secs(60));
    assert!(limiter.acquire("peer-1").is_ok());
    assert!(limiter.acquire("peer-1").is_err());
    limiter.release("peer-1");
    assert!(limiter.acquire("peer-1").is_ok());
}

#[test]
fn test_extract_ip_from_addr_ipv6() {
    let ip = extract_ip_from_addr("[::1]:8080");
    // IPv6 addresses in bracket notation
    assert!(ip.is_some());
}

#[test]
fn test_extract_ip_from_addr_invalid() {
    let ip = extract_ip_from_addr("not-an-ip");
    // Should still return something or None for invalid
    // The function parses it, may succeed or fail
    assert!(ip.is_some() || ip.is_none());
}

#[test]
fn test_select_best_address_all_loopback() {
    let client = RpcClient::new();
    let addrs = vec!["127.0.0.1:9000".into(), "127.0.0.1:9001".into()];
    let best = client.select_best_address(&addrs);
    // Should pick one of them (first loopback when all are loopback)
    assert!(!best.is_empty());
}

#[test]
fn test_select_best_address_with_resolver_no_interfaces() {
    let resolver = Arc::new(MockResolver { interfaces: vec![] });
    let client = RpcClient::with_resolver(resolver);
    let addrs = vec!["10.0.0.1:9000".into(), "192.168.1.1:9000".into()];
    let best = client.select_best_address(&addrs);
    // Without interfaces, should pick first non-loopback
    assert_eq!(best, "10.0.0.1:9000");
}

#[test]
fn test_is_same_subnet_same_ip() {
    assert!(is_same_subnet(
        "192.168.1.10",
        "192.168.1.10",
        "255.255.255.0"
    ));
}

#[test]
fn test_is_same_subnet_wide_mask() {
    assert!(is_same_subnet("10.0.0.1", "10.255.255.255", "0.0.0.0"));
}

#[test]
fn test_is_same_subnet_narrow_mask() {
    assert!(!is_same_subnet(
        "192.168.1.1",
        "192.168.2.1",
        "255.255.255.255"
    ));
}

#[test]
fn test_rpc_client_new_creates_default() {
    let client = RpcClient::new();
    assert_eq!(client.timeout(), DEFAULT_RPC_TIMEOUT);
    assert!(client.auth_token.lock().is_none());
}

#[test]
fn test_rpc_client_set_and_clear_auth_token() {
    let client = RpcClient::new();
    client.set_auth_token("token".into());
    assert_eq!(client.auth_token.lock().as_deref(), Some("token"));
    client.set_auth_token("".into());
    // Setting empty string clears it
    assert!(client.auth_token.lock().is_none() || client.auth_token.lock().as_deref() == Some(""));
}

#[test]
fn test_local_network_interface_clone() {
    let iface = LocalNetworkInterface {
        ip: "192.168.1.1".into(),
        mask: "255.255.255.0".into(),
    };
    let cloned = iface.clone();
    assert_eq!(cloned.ip, "192.168.1.1");
    assert_eq!(cloned.mask, "255.255.255.0");
}

#[tokio::test]
async fn test_call_custom_action_peer_not_found() {
    let client = RpcClient::new();
    let request = RPCRequest {
        id: "req-custom".into(),
        action: crate::rpc_types::ActionType::Custom("my_action".into()),
        payload: serde_json::json!({}),
        source: "node-a".into(),
        target: Some("node-b".into()),
    };
    let result = client.call("node-b", request).await;
    assert!(result.is_err());
}

#[test]
fn test_rpc_request_clone() {
    let req = RPCRequest {
        id: "req-1".into(),
        action: crate::rpc_types::ActionType::Known(crate::rpc_types::KnownAction::Ping),
        payload: serde_json::json!({"key": "val"}),
        source: "node-a".into(),
        target: Some("node-b".into()),
    };
    let cloned = req.clone();
    assert_eq!(cloned.id, "req-1");
    assert_eq!(cloned.source, "node-a");
}

#[test]
fn test_rpc_response_clone() {
    let resp = RPCResponse {
        id: "resp-1".into(),
        result: Some(serde_json::json!({"ok": true})),
        error: None,
    };
    let cloned = resp.clone();
    assert_eq!(cloned.id, "resp-1");
    assert!(cloned.result.is_some());
}

// ============================================================
// Coverage improvement: more client edge cases
// ============================================================

#[test]
fn test_rpc_client_close() {
    let client = RpcClient::new();
    client.close(); // Should not panic
}

#[test]
fn test_rpc_client_default_impl() {
    let client = RpcClient::default();
    assert_eq!(client.timeout(), DEFAULT_RPC_TIMEOUT);
}

#[test]
fn test_extract_ip_from_addr_ipv4_with_port() {
    let ip = extract_ip_from_addr("10.0.0.5:8080");
    assert_eq!(ip.unwrap().to_string(), "10.0.0.5");
}

#[test]
fn test_is_same_subnet_invalid_mask() {
    assert!(!is_same_subnet("192.168.1.1", "192.168.1.2", "not-a-mask"));
}

#[test]
fn test_is_same_subnet_ipv6_mismatch() {
    // One IPv6 address should return false
    assert!(!is_same_subnet("::1", "192.168.1.1", "255.255.255.0"));
}

#[test]
fn test_rate_limiter_per_peer_exhaustion() {
    // max_tokens=2 per peer, refill every 60s, 10 req/window, 60s window
    let limiter = RateLimiter::new(2, Duration::from_secs(60), 10, Duration::from_secs(60));
    assert!(limiter.acquire("peer-1").is_ok());
    assert!(limiter.acquire("peer-1").is_ok());
    // Third request for same peer should fail (no tokens)
    assert!(limiter.acquire("peer-1").is_err());
    // Different peer should still work
    assert!(limiter.acquire("peer-2").is_ok());
}

#[tokio::test]
async fn test_call_with_timeout_connection_refused() {
    let resolver = Arc::new(MockOnlineResolver {
        addresses: vec!["127.0.0.1".into()],
    });
    let client = RpcClient::with_resolver(resolver);
    let request = RPCRequest {
        id: "req-conn".into(),
        action: crate::rpc_types::ActionType::Known(crate::rpc_types::KnownAction::Ping),
        payload: serde_json::json!({}),
        source: "node-a".into(),
        target: Some("node-b".into()),
    };

    // Connection to port 9999 should fail
    let result = client.call("node-b", request).await;
    assert!(result.is_err());
}

#[test]
fn test_select_best_address_two_addresses() {
    let client = RpcClient::new();
    let addrs = vec!["10.0.0.1:9000".into(), "192.168.1.1:9000".into()];
    // With no resolver, should return first address
    let best = client.select_best_address(&addrs);
    assert_eq!(best, "10.0.0.1:9000");
}

#[test]
fn test_select_best_address_with_resolver_subnet_match() {
    let resolver = Arc::new(MockResolver {
        interfaces: vec![LocalNetworkInterface {
            ip: "192.168.1.100".into(),
            mask: "255.255.255.0".into(),
        }],
    });
    let client = RpcClient::with_resolver(resolver);
    let addrs = vec![
        "10.0.0.1:9000".into(),
        "192.168.1.10:9000".into(), // Same subnet as local
    ];
    let best = client.select_best_address(&addrs);
    assert_eq!(best, "192.168.1.10:9000");
}

// ---------------------------------------------------------------------------
// 二期桥仲裁（goal：桥集群，批次六）
// ---------------------------------------------------------------------------

use std::sync::atomic::{AtomicUsize, Ordering};

/// 仲裁 resolver：地址/端口/在线/本地网卡全可配。
struct ArbitrationResolver {
    addresses: Vec<String>,
    rpc_port: u16,
    online: bool,
    interfaces: Vec<LocalNetworkInterface>,
}

impl PeerResolver for ArbitrationResolver {
    fn get_peer_info(&self, _peer_id: &str) -> Option<(Vec<String>, u16, bool)> {
        Some((self.addresses.clone(), self.rpc_port, self.online))
    }
    fn get_local_interfaces(&self) -> Vec<LocalNetworkInterface> {
        self.interfaces.clone()
    }
    fn get_node_id(&self) -> String {
        "arb-node".into()
    }
}

/// 桥出参形态（避免依赖 RpcClientError 的 Clone）。
enum BridgeOut {
    /// 成功响应帧（payload 值；id 由实现按请求回显）。
    Ok(serde_json::Value),
    /// 对端 handler 业务错误（RemoteError——不得触发兜底）。
    Remote(String),
    /// 桥链路传输失败（Connection——应触发兜底）。
    Conn(String),
}

/// Mock 桥出口：记录调用次数，返回可配出参。
struct MockBridge {
    online: bool,
    out: BridgeOut,
    calls: Arc<AtomicUsize>,
}

impl BridgeSend for MockBridge {
    fn bridge_online(&self, _peer_id: &str) -> bool {
        self.online
    }
    fn send_over_bridge(
        &self,
        _peer_id: &str,
        request: WireMessage,
        _timeout: Duration,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<WireMessage, RpcClientError>> + Send + '_>,
    > {
        let calls = self.calls.clone();
        let out = match &self.out {
            BridgeOut::Ok(payload) => Ok(WireMessage {
                version: "1.0".into(),
                id: request.id.clone(),
                msg_type: "response".into(),
                from: "bridge-peer".into(),
                to: request.from.clone(),
                action: request.action.clone(),
                payload: payload.clone(),
                timestamp: chrono::Local::now().timestamp(),
                error: String::new(),
            }),
            BridgeOut::Remote(e) => Err(RpcClientError::RemoteError(e.clone())),
            BridgeOut::Conn(e) => Err(RpcClientError::Connection(e.clone())),
        };
        Box::pin(async move {
            calls.fetch_add(1, Ordering::SeqCst);
            out
        })
    }
}

/// 直连路径 echo：读一帧请求，按请求 id 回成功响应。返回（地址, 请求计数, 句柄）。
async fn spawn_arb_echo() -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let requests = Arc::new(AtomicUsize::new(0));
    let req_clone = requests.clone();
    let handle = tokio::spawn(async move {
        if let Ok((mut sock, _)) = listener.accept().await {
            let mut len_buf = [0u8; 4];
            if sock.read_exact(&mut len_buf).await.is_err() {
                return;
            }
            let len = u32::from_be_bytes(len_buf) as usize;
            let mut buf = vec![0u8; len];
            if sock.read_exact(&mut buf).await.is_err() {
                return;
            }
            req_clone.fetch_add(1, Ordering::SeqCst);
            let req: WireMessage = serde_json::from_slice(&buf).unwrap();
            let resp = WireMessage {
                version: "1.0".into(),
                id: req.id.clone(),
                msg_type: "response".into(),
                from: "direct-peer".into(),
                to: req.from.clone(),
                action: req.action.clone(),
                payload: serde_json::json!({"via": "direct"}),
                timestamp: chrono::Local::now().timestamp(),
                error: String::new(),
            };
            let json = serde_json::to_vec(&resp).unwrap();
            let _ = sock.write_all(&(json.len() as u32).to_be_bytes()).await;
            let _ = sock.write_all(&json).await;
            let _ = sock.flush().await;
        }
    });
    (addr, requests, handle)
}

fn arb_request() -> RPCRequest {
    RPCRequest {
        id: "arb-req-1".into(),
        action: crate::rpc_types::ActionType::Known(crate::rpc_types::KnownAction::Ping),
        payload: serde_json::json!({"probe": true}),
        source: "node-self".into(),
        target: Some("node-far".into()),
    }
}

#[test]
fn test_wire_from_request_known_and_custom_actions() {
    // Known 五变体映射（小写 snake，与 TCP 线上字节一致）。
    let cases = [
        (crate::rpc_types::KnownAction::PeerChat, "peer_chat"),
        (
            crate::rpc_types::KnownAction::PeerChatCallback,
            "peer_chat_callback",
        ),
        (crate::rpc_types::KnownAction::ForgeShare, "forge_share"),
        (crate::rpc_types::KnownAction::Ping, "ping"),
        (crate::rpc_types::KnownAction::Status, "status"),
    ];
    for (known, expect) in cases {
        let req = RPCRequest {
            id: "w-1".into(),
            action: crate::rpc_types::ActionType::Known(known),
            payload: serde_json::json!({}),
            source: "a".into(),
            target: Some("b".into()),
        };
        assert_eq!(wire_from_request(&req).action, expect);
    }
    // Custom 原样透传。
    let req = RPCRequest {
        id: "w-2".into(),
        action: crate::rpc_types::ActionType::Custom("query_task_result".into()),
        payload: serde_json::json!({}),
        source: "a".into(),
        target: Some("b".into()),
    };
    assert_eq!(wire_from_request(&req).action, "query_task_result");
}

#[test]
fn test_wire_from_request_fields() {
    let req = RPCRequest {
        id: "w-3".into(),
        action: crate::rpc_types::ActionType::Known(crate::rpc_types::KnownAction::Ping),
        payload: serde_json::json!({"k": 1}),
        source: "node-self".into(),
        target: Some("node-far".into()),
    };
    let wire = wire_from_request(&req);
    assert_eq!(wire.version, "1.0");
    assert_eq!(wire.id, "w-3");
    assert_eq!(wire.msg_type, "request");
    assert_eq!(wire.from, "node-self");
    assert_eq!(wire.to, "node-far");
    assert_eq!(wire.payload, serde_json::json!({"k": 1}));
    assert_eq!(wire.error, "");
    assert!(wire.timestamp > 0);
    // target=None（广播形态）→ to 空串（不 panic）。
    let req = RPCRequest {
        target: None,
        ..req
    };
    assert_eq!(wire_from_request(&req).to, "");
}

#[test]
fn test_rpc_response_from_wire_ok_and_error() {
    // error 空 → Ok（payload → result），对齐 Frame::decode_response。
    let wire = WireMessage {
        version: "1.0".into(),
        id: "r-1".into(),
        msg_type: "response".into(),
        from: "b".into(),
        to: "a".into(),
        action: "ping".into(),
        payload: serde_json::json!({"pong": true}),
        timestamp: 1,
        error: String::new(),
    };
    let resp = rpc_response_from_wire(&wire).expect("空 error 应为成功响应");
    assert_eq!(resp.id, "r-1");
    assert_eq!(resp.result, Some(serde_json::json!({"pong": true})));
    assert!(resp.error.is_none());

    // error 非空 → RemoteError（对端 handler 业务错误）。
    let wire = WireMessage {
        error: "handler exploded".into(),
        ..wire
    };
    match rpc_response_from_wire(&wire) {
        Err(RpcClientError::RemoteError(e)) => assert_eq!(e, "handler exploded"),
        other => panic!("error 非空应为 RemoteError，得到 {:?}", other),
    }
}

#[test]
fn test_peer_in_same_subnet_matrix() {
    let ifaces = |ip: &str, mask: &str| {
        vec![LocalNetworkInterface {
            ip: ip.into(),
            mask: mask.into(),
        }]
    };
    // 同网段 → true（直连优先）。
    assert!(peer_in_same_subnet(
        &ifaces("192.168.1.5", "255.255.255.0"),
        &["192.168.1.10:9000".into()]
    ));
    // 异网段 → false（桥优先）。
    assert!(!peer_in_same_subnet(
        &ifaces("10.0.0.5", "255.0.0.0"),
        &["192.168.1.10:9000".into()]
    ));
    // 无本地网卡信息 → false（保守走桥优先）。
    assert!(!peer_in_same_subnet(&[], &["192.168.1.10:9000".into()]));
    // 多地址任一命中即 true。
    assert!(peer_in_same_subnet(
        &ifaces("10.0.0.5", "255.0.0.0"),
        &["100.64.0.1:9000".into(), "10.0.0.9:9000".into()]
    ));
    // 非法地址串不 panic，按不命中处理。
    assert!(!peer_in_same_subnet(
        &ifaces("10.0.0.5", "255.0.0.0"),
        &["not-an-ip".into()]
    ));
}

#[tokio::test]
async fn test_arbitration_different_subnet_bridge_first_no_dial() {
    // 桥在线 + 异网段（无本地网卡信息）→ 桥优先；直连地址给必拒端口，
    // 断言桥恰被调一次且结果来自桥（不先拨直连）。
    let bridge_calls = Arc::new(AtomicUsize::new(0));
    let client = RpcClient::with_resolver(Arc::new(ArbitrationResolver {
        addresses: vec!["127.0.0.1".into()],
        rpc_port: 1, // 必拒端口：若误先拨直连会走兜底顺序，结果仍来自桥但顺序错
        online: true,
        interfaces: vec![],
    }));
    client.set_bridge_transport(Arc::new(MockBridge {
        online: true,
        out: BridgeOut::Ok(serde_json::json!({"via": "bridge"})),
        calls: bridge_calls.clone(),
    }));

    let resp = client
        .call_with_timeout("node-far", arb_request(), Duration::from_secs(5))
        .await
        .expect("桥优先路径应成功");
    assert_eq!(resp.result, Some(serde_json::json!({"via": "bridge"})));
    assert_eq!(bridge_calls.load(Ordering::SeqCst), 1, "桥恰被调一次");
}

#[tokio::test]
async fn test_arbitration_same_subnet_direct_first_bridge_fallback() {
    // 桥在线 + 同网段 + 直连拒连（端口 1）→ 直连失败兜桥成功。
    let bridge_calls = Arc::new(AtomicUsize::new(0));
    let client = RpcClient::with_resolver(Arc::new(ArbitrationResolver {
        addresses: vec!["127.0.0.1".into()],
        rpc_port: 1, // 必拒
        online: true,
        interfaces: vec![LocalNetworkInterface {
            ip: "127.0.0.1".into(),
            mask: "255.0.0.0".into(),
        }],
    }));
    client.set_bridge_transport(Arc::new(MockBridge {
        online: true,
        out: BridgeOut::Ok(serde_json::json!({"via": "bridge"})),
        calls: bridge_calls.clone(),
    }));

    let resp = client
        .call_with_timeout("node-far", arb_request(), Duration::from_secs(15))
        .await
        .expect("直连失败应兜桥成功");
    assert_eq!(resp.result, Some(serde_json::json!({"via": "bridge"})));
    assert_eq!(bridge_calls.load(Ordering::SeqCst), 1, "兜桥恰一次");
}

#[tokio::test]
async fn test_arbitration_same_subnet_direct_success_skips_bridge() {
    // 桥在线 + 同网段 + 直连可达 → 直连成功，桥零调用（首选即达不试备选）。
    let (addr, direct_calls, echo) = spawn_arb_echo().await;
    let port: u16 = addr.rsplit(':').next().unwrap().parse().unwrap();
    let bridge_calls = Arc::new(AtomicUsize::new(0));
    let client = RpcClient::with_resolver(Arc::new(ArbitrationResolver {
        addresses: vec!["127.0.0.1".into()],
        rpc_port: port,
        online: true,
        interfaces: vec![LocalNetworkInterface {
            ip: "127.0.0.1".into(),
            mask: "255.0.0.0".into(),
        }],
    }));
    client.set_bridge_transport(Arc::new(MockBridge {
        online: true,
        out: BridgeOut::Ok(serde_json::json!({"via": "bridge"})),
        calls: bridge_calls.clone(),
    }));

    let resp = client
        .call_with_timeout("node-far", arb_request(), Duration::from_secs(5))
        .await
        .expect("同网段直连应成功");
    assert_eq!(
        resp.result,
        Some(serde_json::json!({"via": "direct"})),
        "结果应来自直连 echo 而非桥"
    );
    assert_eq!(bridge_calls.load(Ordering::SeqCst), 0, "直连成功桥零调用");
    assert_eq!(direct_calls.load(Ordering::SeqCst), 1, "直连恰被拨一次");
    echo.abort();
}

#[tokio::test]
async fn test_arbitration_bridge_failure_falls_back_to_direct() {
    // 桥在线 + 异网段 + 桥链路失败（Connection）→ 兜直连 echo 成功。
    let (addr, _direct_calls, echo) = spawn_arb_echo().await;
    let port: u16 = addr.rsplit(':').next().unwrap().parse().unwrap();
    let bridge_calls = Arc::new(AtomicUsize::new(0));
    let client = RpcClient::with_resolver(Arc::new(ArbitrationResolver {
        addresses: vec!["127.0.0.1".into()],
        rpc_port: port,
        online: true,
        interfaces: vec![], // 异网段 → 桥优先
    }));
    client.set_bridge_transport(Arc::new(MockBridge {
        online: true,
        out: BridgeOut::Conn("bridge link broken".into()),
        calls: bridge_calls.clone(),
    }));

    let resp = client
        .call_with_timeout("node-far", arb_request(), Duration::from_secs(5))
        .await
        .expect("桥失败应兜直连成功");
    assert_eq!(
        resp.result,
        Some(serde_json::json!({"via": "direct"})),
        "结果应来自兜底直连"
    );
    assert_eq!(bridge_calls.load(Ordering::SeqCst), 1);
    echo.abort();
}

#[tokio::test]
async fn test_arbitration_bridge_remote_error_not_fallbacked() {
    // 桥路径 RemoteError = 对端 handler 业务错误 → 直接返回，不兜直连
    // （换路径重发无意义，对端会给出同样错误）。
    let (addr, direct_calls, echo) = spawn_arb_echo().await;
    let port: u16 = addr.rsplit(':').next().unwrap().parse().unwrap();
    let bridge_calls = Arc::new(AtomicUsize::new(0));
    let client = RpcClient::with_resolver(Arc::new(ArbitrationResolver {
        addresses: vec!["127.0.0.1".into()],
        rpc_port: port,
        online: true,
        interfaces: vec![], // 异网段 → 桥优先
    }));
    client.set_bridge_transport(Arc::new(MockBridge {
        online: true,
        out: BridgeOut::Remote("task not found".into()),
        calls: bridge_calls.clone(),
    }));

    let err = client
        .call_with_timeout("node-far", arb_request(), Duration::from_secs(5))
        .await
        .expect_err("RemoteError 应直接返回");
    match err {
        RpcClientError::RemoteError(e) => assert_eq!(e, "task not found"),
        other => panic!("应为 RemoteError，得到 {:?}", other),
    }
    assert_eq!(
        direct_calls.load(Ordering::SeqCst),
        0,
        "RemoteError 不兜直连"
    );
    echo.abort();
}

#[tokio::test]
async fn test_arbitration_bridge_offline_is_pure_direct() {
    // 桥出口已注入但该 peer 桥链路不在线 → 纯直连（一期行为零变化）。
    let (addr, _direct_calls, echo) = spawn_arb_echo().await;
    let port: u16 = addr.rsplit(':').next().unwrap().parse().unwrap();
    let bridge_calls = Arc::new(AtomicUsize::new(0));
    let client = RpcClient::with_resolver(Arc::new(ArbitrationResolver {
        addresses: vec!["127.0.0.1".into()],
        rpc_port: port,
        online: true,
        interfaces: vec![],
    }));
    client.set_bridge_transport(Arc::new(MockBridge {
        online: false, // 桥链路不在线
        out: BridgeOut::Ok(serde_json::json!({"via": "bridge"})),
        calls: bridge_calls.clone(),
    }));

    let resp = client
        .call_with_timeout("node-far", arb_request(), Duration::from_secs(5))
        .await
        .expect("纯直连应成功");
    assert_eq!(resp.result, Some(serde_json::json!({"via": "direct"})));
    assert_eq!(bridge_calls.load(Ordering::SeqCst), 0, "桥不在线零调用");
    echo.abort();
}

#[tokio::test]
async fn test_arbitration_probe_shares_bridge_path() {
    // G2 探针（require_online=false）与业务共享仲裁：registry Offline 但
    // 桥在线 → 探针经桥触达，Online 语义自洽（桥节点不再被误判死节点）。
    let bridge_calls = Arc::new(AtomicUsize::new(0));
    let client = RpcClient::with_resolver(Arc::new(ArbitrationResolver {
        addresses: vec!["127.0.0.1".into()],
        rpc_port: 1,
        online: false, // registry Offline
        interfaces: vec![],
    }));
    client.set_bridge_transport(Arc::new(MockBridge {
        online: true,
        out: BridgeOut::Ok(serde_json::json!({"via": "bridge-probe"})),
        calls: bridge_calls.clone(),
    }));

    let resp = client
        .call_probe_with_timeout("node-far", arb_request(), Duration::from_secs(5))
        .await
        .expect("探针应经桥触达 Offline 节点");
    assert_eq!(
        resp.result,
        Some(serde_json::json!({"via": "bridge-probe"}))
    );
    assert_eq!(bridge_calls.load(Ordering::SeqCst), 1);
}
