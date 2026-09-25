// rpc/client.rs 覆盖率补充测试（is_peer_online 三态 / 限流窗口溢出拒绝 /
// 零超时映射 Timeout + 失败日志臂 / 同网段直连失败兜桥臂）。
//
// 豁免：无（本文件各臂均可确定性驱动）。

use super::*;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex as StdMutex;

/// 记录型 resolver：peer 表可配，本地网卡固定回环段（驱动同网段判定）。
struct CovResolver {
    peers: StdMutex<HashMap<String, (Vec<String>, u16, bool)>>,
    interfaces: Vec<LocalNetworkInterface>,
}

impl CovResolver {
    fn with_peer(peer_id: &str, addresses: Vec<String>, rpc_port: u16, online: bool) -> Self {
        let mut peers = HashMap::new();
        peers.insert(peer_id.to_string(), (addresses, rpc_port, online));
        Self {
            peers: StdMutex::new(peers),
            interfaces: vec![LocalNetworkInterface {
                ip: "127.0.0.1".into(),
                mask: "255.0.0.0".into(),
            }],
        }
    }
}

impl PeerResolver for CovResolver {
    fn get_peer_info(&self, peer_id: &str) -> Option<(Vec<String>, u16, bool)> {
        self.peers.lock().unwrap().get(peer_id).cloned()
    }
    fn get_local_interfaces(&self) -> Vec<LocalNetworkInterface> {
        self.interfaces.clone()
    }
    fn get_node_id(&self) -> String {
        "cov-node".into()
    }
}

/// 桥出口：恒在线，回显响应（result 原样带回）。
struct EchoBridge;

impl BridgeSend for EchoBridge {
    fn bridge_online(&self, _peer_id: &str) -> bool {
        true
    }
    fn send_over_bridge(
        &self,
        _peer_id: &str,
        request: WireMessage,
        _timeout: Duration,
    ) -> Pin<Box<dyn Future<Output = Result<WireMessage, RpcClientError>> + Send + '_>> {
        Box::pin(async move {
            let payload = request.payload.clone();
            Ok(WireMessage::new_response(&request, payload))
        })
    }
}

fn ping_request() -> RPCRequest {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    RPCRequest {
        id: format!("cov-req-{n}"),
        action: crate::rpc_types::ActionType::Known(crate::rpc_types::KnownAction::Ping),
        payload: serde_json::json!({"via": "cov"}),
        source: "cov-a".into(),
        target: Some("cov-b".into()),
    }
}

/// is_peer_online：无 resolver → None；已知节点回其在线态；未知节点 → None。
#[test]
fn is_peer_online_readonly_variants() {
    let bare = RpcClient::new();
    assert_eq!(bare.is_peer_online("any"), None, "未装配 resolver → None");

    let client = RpcClient::with_resolver(Arc::new(CovResolver::with_peer(
        "p1",
        vec!["127.0.0.1:1".into()],
        1,
        true,
    )));
    assert_eq!(client.is_peer_online("p1"), Some(true));
    assert_eq!(client.is_peer_online("ghost"), None, "未知节点 → None");
}

/// 限流：窗口内请求数达 max_requests_per_window → 溢出拒绝臂（125/134）。
#[test]
fn rate_limiter_window_overflow_rejects() {
    let rl = RateLimiter::new(10, Duration::from_secs(60), 1, Duration::from_secs(60));
    assert!(rl.acquire("p").is_ok(), "首请求放行");
    let err = rl.acquire("p").unwrap_err();
    assert!(
        matches!(err, RpcClientError::RateLimited(_)),
        "窗口溢出必须拒绝：{err}"
    );
}

/// 零超时：外层 timeout 立即到期 → 超时 error 日志臂 + Timeout 映射（592），
/// 随后走失败 warn 臂（644）。
#[tokio::test]
async fn call_zero_timeout_maps_to_timeout_and_failed_log() {
    let client = RpcClient::with_resolver(Arc::new(CovResolver::with_peer(
        "p-dead",
        vec!["127.0.0.1:1".into()],
        1,
        true,
    )));
    let err = client
        .call_with_timeout("p-dead", ping_request(), Duration::ZERO)
        .await
        .unwrap_err();
    assert!(matches!(err, RpcClientError::Timeout), "{err}");
}

/// 同网段直连失败兜桥（548/549）：回环地址判同网段 → 直连优先，死端口
/// 连接失败（非 RemoteError）→ warn 兜桥 → 桥回显成功。
#[tokio::test]
async fn bridge_same_subnet_direct_fail_falls_back_to_bridge() {
    let client = RpcClient::with_resolver(Arc::new(CovResolver::with_peer(
        "p-bridge",
        vec!["127.0.0.1:1".into()],
        1,
        true,
    )));
    client.set_bridge_transport(Arc::new(EchoBridge));

    let resp = client
        .call_with_timeout("p-bridge", ping_request(), Duration::from_secs(10))
        .await
        .expect("直连失败必须兜桥成功");
    assert_eq!(resp.result.as_ref().unwrap()["via"], "cov");
}

/// 异网段桥优先（对照臂）：接口表不含对端网段 → 桥先行，直接成功。
#[tokio::test]
async fn bridge_cross_subnet_bridge_first_succeeds() {
    let mut resolver = CovResolver::with_peer("p-cross", vec!["10.9.9.9:1".into()], 1, true);
    resolver.interfaces = vec![LocalNetworkInterface {
        ip: "127.0.0.1".into(),
        mask: "255.0.0.0".into(),
    }];
    let client = RpcClient::with_resolver(Arc::new(resolver));
    client.set_bridge_transport(Arc::new(EchoBridge));

    let resp = client
        .call_with_timeout("p-cross", ping_request(), Duration::from_secs(10))
        .await
        .expect("桥优先路径直接成功");
    assert_eq!(resp.result.as_ref().unwrap()["via"], "cov");
}
