//! A1 P2 验收：iroh 跨网 P2P 叠加层 RPC 双栈往返（loopback 直连形态）。
//!
//! 验证语义：`iroh:<hex>@<ip:port>` 形态地址经 RpcClient 前缀分派走 iroh
//! QUIC bi-stream，桥到 **真实 RpcServer** 的 handler 表——与 TCP 路径同一
//! 帧格式、同一 AEAD token 语义、同一 `handle_request` 分发。服务器侧不
//! start() TCP（只取 handler 表 + token 快照做 iroh 入站桥），客户端不配
//! TCP。Minimal 预设（bind_direct）零公网依赖。
#![cfg(feature = "cluster-roaming")]

use std::sync::Arc;
use std::time::Duration;

use nemesis_cluster::roaming::bridge::parse_roaming_address;
use nemesis_cluster::roaming::endpoint::{
    RoamingEndpoint, RoamingIdentity, RoamingProtocolHandler,
};
use nemesis_cluster::rpc::client::{LocalNetworkInterface, PeerResolver, RpcClient};
use nemesis_cluster::rpc::server::{RpcServer, RpcServerConfig};
use nemesis_cluster::rpc_types::{ActionType, KnownAction, RPCRequest};

/// 返回 `iroh:<hex>@<ip:port>`（对端 node_id hex + 直连 socket）。
fn roaming_addr_for(endpoint: &RoamingEndpoint) -> String {
    // bound_sockets 可能含 0.0.0.0/[::] 通配形态（connect 到通配地址平台
    // 行为不定）——取 IPv4 端口落 127.0.0.1（P1 ping_loopback_direct 同款）。
    let port = endpoint
        .direct_addrs()
        .into_iter()
        .find(|a| a.is_ipv4())
        .expect("bound socket")
        .port();
    format!("iroh:{}@127.0.0.1:{}", endpoint.node_id_hex(), port)
}

/// 静态解析器：peer 恒解析到给定 iroh 地址（在线）。
struct RoamingResolver {
    addr: String,
}

impl PeerResolver for RoamingResolver {
    fn get_peer_info(&self, _peer_id: &str) -> Option<(Vec<String>, u16, bool)> {
        Some((vec![self.addr.clone()], 0, true))
    }
    fn get_local_interfaces(&self) -> Vec<LocalNetworkInterface> {
        vec![]
    }
    fn get_node_id(&self) -> String {
        "roaming-test-client".into()
    }
}

/// 服务器侧装配：RpcServer（带 token + 自定义 handler）+ iroh 入站桥。
/// 返回 (端点, 完整 roaming 地址)。TCP 不启动——iroh 桥只需要 handler 表。
/// `trust` = Some(信任簿) 时启用 allowlist 裁决（P3 accept 侧闸）。
async fn make_roaming_server(
    token: &str,
    trust: Option<nemesis_cluster::roaming::trust::TrustBook>,
) -> (Arc<RoamingEndpoint>, String) {
    let identity = RoamingIdentity::generate();
    let endpoint = RoamingEndpoint::bind_direct(&identity, true)
        .await
        .expect("server iroh bind");

    let server = RpcServer::new(RpcServerConfig {
        bind_address: "127.0.0.1:0".into(),
        ..Default::default()
    });
    server.set_auth_token(token);
    server.register_handler(
        "roaming_probe",
        Box::new(|p| {
            Ok(serde_json::json!({
                "echo": p,
                "via": "iroh-bridge",
            }))
        }),
    );

    let handler = server.roaming_conn_handler(trust);
    endpoint
        .serve(RoamingProtocolHandler::bridge(handler))
        .await
        .expect("server serve bridge");

    let addr = roaming_addr_for(&endpoint);
    (Arc::new(endpoint), addr)
}

/// 客户端侧装配：iroh 端点（accept=false）+ RpcClient（resolver 指向
/// roaming 地址 + 同款 token + 端点注入）。
async fn make_roaming_client(token: &str, addr: String) -> RpcClient {
    let identity = RoamingIdentity::generate();
    let endpoint = RoamingEndpoint::bind_direct(&identity, false)
        .await
        .expect("client iroh bind");

    let client = RpcClient::with_resolver(Arc::new(RoamingResolver { addr }));
    client.set_auth_token(token.to_string());
    client.set_roaming_endpoint(Arc::new(endpoint));
    client
}

fn probe_request(id: &str) -> RPCRequest {
    RPCRequest {
        id: id.into(),
        action: ActionType::Known(KnownAction::Ping),
        payload: serde_json::json!({"probe": "roaming"}),
        source: "roaming-test-client".into(),
        target: Some("roaming-test-server".into()),
    }
}

fn custom_request(id: &str) -> RPCRequest {
    RPCRequest {
        id: id.into(),
        action: ActionType::Custom("roaming_probe".into()),
        payload: serde_json::json!({"probe": "roaming"}),
        source: "roaming-test-client".into(),
        target: Some("roaming-test-server".into()),
    }
}

/// 主验收：iroh 往返——自定义 handler + 内置 ping 都经 QUIC bi-stream 桥
/// 到真实 RpcServer handler 表。
#[tokio::test]
async fn iroh_rpc_roundtrip_custom_and_builtin() {
    let (server_ep, addr) = make_roaming_server("shared-token", None).await;
    let client = make_roaming_client("shared-token", addr).await;

    // 自定义 handler（payload 原样回显 + via 标记）。
    let resp = client
        .call_with_timeout(
            "peer-b",
            custom_request("rt-custom"),
            Duration::from_secs(15),
        )
        .await
        .expect("custom handler roundtrip");
    assert_eq!(resp.id, "rt-custom");
    let result = resp.result.expect("custom handler result");
    assert_eq!(result["via"], "iroh-bridge");
    assert_eq!(result["echo"]["probe"], "roaming");

    // 内置 ping（默认 handler 表成员）。
    let resp = client
        .call_with_timeout("peer-b", probe_request("rt-ping"), Duration::from_secs(15))
        .await
        .expect("builtin ping roundtrip");
    assert_eq!(resp.id, "rt-ping");
    assert_eq!(resp.result.expect("ping result")["status"], "pong");

    server_ep.shutdown().await;
}

/// token 失配 = 显式失败（GCM tag 校验），不是垃圾数据当成功。
#[tokio::test]
async fn iroh_rpc_wrong_token_fails_loud() {
    let (server_ep, addr) = make_roaming_server("server-token", None).await;
    let client = make_roaming_client("other-token", addr).await;

    let err = client
        .call_with_timeout("peer-b", probe_request("rt-bad"), Duration::from_secs(15))
        .await
        .expect_err("token mismatch must fail");
    // 服务端解密失败 → 连接层错误（TcpConn 帧读失败/空流），绝不返回
    // 带结果的成功响应。
    let msg = match &err {
        nemesis_cluster::rpc::client::RpcClientError::Connection(m) => m.clone(),
        nemesis_cluster::rpc::client::RpcClientError::RemoteError(m) => m.clone(),
        other => format!("{other:?}"),
    };
    assert!(
        !msg.is_empty(),
        "failure must carry a message (decrypt/connection)"
    );

    server_ep.shutdown().await;
}

/// 地址语法：`iroh:<hex>` 与 `iroh:<hex>@<socket>` 两形态 + 非法输入 loud。
#[test]
fn roaming_address_grammar() {
    let hex = "aa".repeat(32);
    let hex_upper = "AA".repeat(32);

    // 纯 node_id 形态（发现层供址）。
    let a = parse_roaming_address(&format!("iroh:{hex}")).expect("plain form");
    assert!(a.ip_addrs().next().is_none(), "no pinned addr expected");

    // 直连钉址形态。
    let b =
        parse_roaming_address(&format!("iroh:{hex_upper}@127.0.0.1:12345")).expect("pinned form");
    let sa = b.ip_addrs().next().expect("pinned socket");
    assert_eq!(sa.to_string(), "127.0.0.1:12345");
    assert_eq!(
        nemesis_cluster::roaming::endpoint::node_id_to_hex(&b.id),
        hex,
        "hex case-insensitive"
    );

    // 非法输入 loud。
    assert!(parse_roaming_address("tcp:1.2.3.4:5").is_err());
    assert!(parse_roaming_address("iroh:zz@1.2.3.4:5").is_err());
    assert!(parse_roaming_address("iroh:abcd").is_err());
    assert!(parse_roaming_address(&format!("iroh:{hex}@")).is_err());
    assert!(parse_roaming_address(&format!("iroh:{hex}@not-a-socket")).is_err());
}

/// P3 验收（信任簿）：allowlist 启用后，未列入 trusted 的对端连接在
/// spawn 前同步拒绝（连接关闭 = 客户端显式失败），不进 handler 分发。
#[tokio::test]
async fn iroh_rpc_trust_book_rejects_unlisted() {
    use nemesis_cluster::roaming::trust::{TrustBook, TrustBookData};

    // 启用信任簿但名单为空（trusted 空 + revoked 空 = 全拒）。
    let book = TrustBook::from_data(TrustBookData::default());
    let (server_ep, addr) = make_roaming_server("shared-token", Some(book)).await;
    let client = make_roaming_client("shared-token", addr).await;

    let err = client
        .call_with_timeout("peer-b", probe_request("rt-trust"), Duration::from_secs(15))
        .await
        .expect_err("unlisted peer must be rejected by trust book");
    let msg = match &err {
        nemesis_cluster::rpc::client::RpcClientError::Connection(m) => m.clone(),
        nemesis_cluster::rpc::client::RpcClientError::RemoteError(m) => m.clone(),
        other => format!("{other:?}"),
    };
    assert!(!msg.is_empty(), "rejection must surface as a failure");

    server_ep.shutdown().await;
}

/// P3 验收（吊销后拒绝）：trusted 放行正常往返；同一客户端身份进
/// revoked（一票否决优先于 trusted）后，同款调用立即失败。
#[tokio::test]
async fn iroh_rpc_trust_book_trusted_then_revoked() {
    use nemesis_cluster::roaming::trust::{TrustBook, TrustBookData};

    let identity = RoamingIdentity::generate();
    let client_hex = identity.node_id_hex();

    // 第一阶段：trusted 名单成员 → 正常往返。
    let book = TrustBook::from_data(TrustBookData {
        trusted: vec![client_hex.clone()],
        revoked: vec![],
    });
    let (server_ep, addr) = make_roaming_server("shared-token", Some(book)).await;

    // 客户端用与服务端信任簿一致的固定身份（trusted 名单成员）。
    let endpoint = RoamingEndpoint::bind_direct(&identity, false)
        .await
        .expect("client iroh bind");
    let client = RpcClient::with_resolver(Arc::new(RoamingResolver { addr: addr.clone() }));
    client.set_auth_token("shared-token".into());
    client.set_roaming_endpoint(Arc::new(endpoint));

    let resp = client
        .call_with_timeout("peer-b", probe_request("rt-ok"), Duration::from_secs(15))
        .await
        .expect("trusted peer roundtrip");
    assert_eq!(resp.result.expect("ping result")["status"], "pong");

    server_ep.shutdown().await;

    // 第二阶段：同一身份进 revoked（revoked 优先于 trusted 的一票否决
    // 语义由 trust.rs 单测钉死；这里验端到端：吊销后的连接在 accept 侧
    // 被 spawn 前拒绝）。
    let book2 = TrustBook::from_data(TrustBookData {
        trusted: vec![client_hex.clone()],
        revoked: vec![client_hex],
    });
    let (server_ep2, addr2) = make_roaming_server("shared-token", Some(book2)).await;

    let endpoint2 = RoamingEndpoint::bind_direct(&identity, false)
        .await
        .expect("client iroh bind #2");
    let client2 = RpcClient::with_resolver(Arc::new(RoamingResolver { addr: addr2 }));
    client2.set_auth_token("shared-token".into());
    client2.set_roaming_endpoint(Arc::new(endpoint2));

    let err = client2
        .call_with_timeout(
            "peer-b",
            probe_request("rt-revoked"),
            Duration::from_secs(15),
        )
        .await
        .expect_err("revoked peer must be rejected");
    let msg = match &err {
        nemesis_cluster::rpc::client::RpcClientError::Connection(m) => m.clone(),
        nemesis_cluster::rpc::client::RpcClientError::RemoteError(m) => m.clone(),
        other => format!("{other:?}"),
    };
    assert!(!msg.is_empty(), "revocation rejection must carry a message");

    server_ep2.shutdown().await;
}
