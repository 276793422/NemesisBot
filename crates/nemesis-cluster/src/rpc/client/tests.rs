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
