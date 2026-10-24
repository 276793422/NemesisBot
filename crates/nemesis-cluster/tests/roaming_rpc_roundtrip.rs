//! QUIC 插件化 M3 验收：iroh RPC 环回集成测试（宿主全链 + 插件模拟器）。
//!
//! 真实 iroh QUIC 传输本体已在 plugin_roaming 插件内（插件侧 32/32 测试
//! 覆盖拨号/accept/trust 闸）；本文件验**宿主侧全链**：
//! `roaming_connect_and_send`（[`RoamingDialer`] 出口注入）→ loopback 交接 →
//! [`RpcServer::spawn_roaming_acceptor`] 消费循环 → [`TcpConn`] 帧管线 →
//! handler 表——与 TCP 路径同一帧格式、同一 AEAD token 语义、同一
//! `handle_request` 分发。
//!
//! 插件用进程内模拟器替身：一次性 loopback TCP 监听作「传输层」，每条
//! 入站连接开 L2 loopback 监听 + `dispatch_incoming` 通知 + 双向 splice——
//! 与真插件 accept 后的 loopback 交接协议逐字同构（dial 侧 [`LocalDialer`]
//! 同理，真插件走桥 FFI 时替换为 BridgeDialer）。服务器 TCP 不 start()——
//! roaming 臂只需要 handler 表。
//!
//! 旧版三测（地址语法 / 信任簿拒绝 / 吊销）随端点迁入插件：解析归插件
//! endpoint 测试、裁决归插件 accept 闸测试，宿主侧不再覆盖。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use nemesis_cluster::roaming::plugin_bridge::{self, RoamingDialer};
use nemesis_cluster::rpc::client::{
    LocalNetworkInterface, PeerResolver, RpcClient, RpcClientError,
};
use nemesis_cluster::rpc::server::{RpcServer, RpcServerConfig};
use nemesis_cluster::rpc_types::{ActionType, KnownAction, RPCRequest};
use serde_json::json;

// ---------------------------------------------------------------------------
// 测试替身：插件模拟器（服务器侧）+ LocalDialer（客户端侧）
// ---------------------------------------------------------------------------

/// 服务器侧插件模拟器 accept 循环：真插件 accept iroh 连接后的动作序列是
/// 「开一次性 loopback 监听 → notify_incoming → 把 bi-stream 与 loopback
/// 连接双向 splice」——此处以 TCP 传输替身逐字复刻该协议。
async fn run_plugin_simulator(listener: tokio::net::TcpListener, remote_hex: String) {
    let mut conn_seq: u64 = 0;
    loop {
        let Ok((transport_conn, _peer)) = listener.accept().await else {
            break; // 监听关闭 = 模拟器退出
        };
        conn_seq += 1;
        let conn_id = conn_seq;
        let remote_hex = remote_hex.clone();
        tokio::spawn(async move {
            // 一次性 loopback L2 监听（插件 accept 后开的交接端口）。
            let Ok(l2) = tokio::net::TcpListener::bind("127.0.0.1:0").await else {
                return;
            };
            let l2_port = match l2.local_addr() {
                Ok(a) => a.port(),
                Err(_) => return,
            };
            // 通知宿主受理臂（serve_roaming_loopback 将 connect 该端口）。
            plugin_bridge::dispatch_incoming(conn_id, l2_port, &remote_hex);
            let Ok((mut l2_conn, _)) = l2.accept().await else {
                return;
            };
            // bi-stream ↔ loopback 双向 splice（帧字节原样搬运）。
            let mut transport_conn = transport_conn;
            let _ = tokio::io::copy_bidirectional(&mut transport_conn, &mut l2_conn).await;
        });
    }
}

/// 客户端拨号替身：真插件 dial 的动作序列是「连对端 → 开一次性 loopback
/// 监听 → splice → 返回 (端口, conn_id)」。解析 `iroh:<hex>@<socket>` 的
/// `@socket` 作传输替身地址。
struct LocalDialer;

impl RoamingDialer for LocalDialer {
    fn dial(&self, addr: &str) -> Pin<Box<dyn Future<Output = Result<(u16, u64), String>> + Send>> {
        let addr = addr.to_string();
        Box::pin(async move {
            let socket = addr
                .split_once('@')
                .map(|(_, s)| s)
                .ok_or_else(|| format!("LocalDialer 需要 @socket 钉址形态: {addr}"))?;
            let transport = tokio::time::timeout(
                Duration::from_secs(5),
                tokio::net::TcpStream::connect(socket),
            )
            .await
            .map_err(|_| format!("dial timeout to {socket}"))?
            .map_err(|e| format!("connect {socket}: {e}"))?;
            let l3 = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .map_err(|e| format!("bind L3: {e}"))?;
            let l3_port = l3.local_addr().map_err(|e| format!("L3 addr: {e}"))?.port();
            static SEQ: AtomicU64 = AtomicU64::new(1);
            let conn_id = SEQ.fetch_add(1, Ordering::Relaxed);
            tokio::spawn(async move {
                if let Ok((mut l3_conn, _)) = l3.accept().await {
                    let mut transport = transport;
                    let _ = tokio::io::copy_bidirectional(&mut transport, &mut l3_conn).await;
                }
            });
            Ok((l3_port, conn_id))
        })
    }

    /// 半开兜底：真实现打桥 FFI 关整条 QUIC 连接；替身无资源可收。
    fn close(&self, _conn_id: u64) {}
}

// ---------------------------------------------------------------------------
// 装配助手（旧形态保留：静态解析器 + 请求构造）
// ---------------------------------------------------------------------------

/// 静态解析器：peer 恒解析到给定 roaming 地址（在线）。
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

/// 服务器侧装配：RpcServer（token + 自定义 handler）+ 漫游入站受理臂 +
/// 插件模拟器。TCP 不 start()——roaming 臂只需要 handler 表。
/// `dispatch_incoming` 的 sink 是进程级单例，两测必须串行（`#[serial]`）。
async fn make_roaming_server(token: &str) -> (Arc<RpcServer>, String) {
    let transport = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind 传输替身");
    let transport_port = transport.local_addr().expect("传输替身地址").port();

    let server = Arc::new(RpcServer::new(RpcServerConfig {
        bind_address: "127.0.0.1:0".into(),
        ..Default::default()
    }));
    server.set_auth_token(token);
    server.register_handler(
        "roaming_probe",
        Box::new(|p| {
            Ok(json!({
                "echo": p,
                "via": "roaming-plugin",
            }))
        }),
    );
    server.spawn_roaming_acceptor().expect("装配漫游入站受理臂");

    let remote_hex = "aa".repeat(32);
    tokio::spawn(run_plugin_simulator(transport, remote_hex.clone()));

    (
        server,
        format!("iroh:{remote_hex}@127.0.0.1:{transport_port}"),
    )
}

/// 客户端侧装配：RpcClient（resolver 指向 roaming 地址 + 同款 token +
/// LocalDialer 注入）。
fn make_roaming_client(token: &str, addr: String) -> RpcClient {
    let client = RpcClient::with_resolver(Arc::new(RoamingResolver { addr }));
    client.set_auth_token(token.to_string());
    client.set_roaming_dialer(Arc::new(LocalDialer));
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

// ---------------------------------------------------------------------------
// 验收测试（sink 单例 → 串行）
// ---------------------------------------------------------------------------

/// 主验收：roaming 往返——自定义 handler + 内置 ping 都经模拟器 splice 桥
/// 到真实 RpcServer handler 表（宿主全链：RoamingDialer 出口 → loopback
/// 交接 → 受理臂 → TcpConn 帧管线）。
#[tokio::test]
#[serial_test::serial]
async fn iroh_rpc_roundtrip_custom_and_builtin() {
    let (_server, addr) = make_roaming_server("shared-token").await;
    let client = make_roaming_client("shared-token", addr);

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
    assert_eq!(result["via"], "roaming-plugin");
    assert_eq!(result["echo"]["probe"], "roaming");

    // 内置 ping（默认 handler 表成员）。
    let resp = client
        .call_with_timeout("peer-b", probe_request("rt-ping"), Duration::from_secs(15))
        .await
        .expect("builtin ping roundtrip");
    assert_eq!(resp.id, "rt-ping");
    assert_eq!(resp.result.expect("ping result")["status"], "pong");
}

/// token 失配 = 显式失败（GCM tag 校验），不是垃圾数据当成功。
#[tokio::test]
#[serial_test::serial]
async fn iroh_rpc_wrong_token_fails_loud() {
    let (_server, addr) = make_roaming_server("server-token").await;
    let client = make_roaming_client("other-token", addr);

    let err = client
        .call_with_timeout("peer-b", probe_request("rt-bad"), Duration::from_secs(15))
        .await
        .expect_err("token mismatch must fail");
    // 服务端解密失败 → 连接层错误（TcpConn 帧读失败/空流），绝不返回
    // 带结果的成功响应。
    let msg = match &err {
        RpcClientError::Connection(m) => m.clone(),
        RpcClientError::RemoteError(m) => m.clone(),
        other => format!("{other:?}"),
    };
    assert!(
        !msg.is_empty(),
        "failure must carry a message (decrypt/connection)"
    );
}
