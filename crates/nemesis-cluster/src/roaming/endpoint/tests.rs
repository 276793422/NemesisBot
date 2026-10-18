//! roaming 模块测试：身份密钥 roundtrip / 落盘持久化 / 本机双 endpoint
//! 直连互 ping（真通路，P1 验收核心）/ TrustBook 裁决。
//!
//! 通路测试用 `bind_direct`（Minimal 预设）+ 显式 IP EndpointAddr——零
//! 外网/零 mDNS/零 relay 依赖，CI 可复现。

use std::time::Duration;

use iroh::EndpointAddr;

use super::{RoamingEndpoint, RoamingIdentity, RoamingProtocolHandler, node_id_from_hex};
use crate::roaming::trust::{TrustBook, TrustBookData};

// ---------------------------------------------------------------------------
// 身份密钥
// ---------------------------------------------------------------------------

#[test]
fn identity_hex_roundtrip() {
    let id = RoamingIdentity::generate();
    let hex = id.node_id_hex();
    assert_eq!(hex.len(), 64, "node_id hex 应为 64 字符");
    let parsed = node_id_from_hex(&hex).expect("hex 应可解析");
    assert_eq!(parsed.as_bytes(), id.node_id().as_bytes());
    // 大小写不敏感。
    let upper = node_id_from_hex(&hex.to_uppercase()).expect("大写 hex 应可解析");
    assert_eq!(upper.as_bytes(), id.node_id().as_bytes());
}

#[test]
fn node_id_from_hex_rejects_garbage() {
    assert!(node_id_from_hex("zzzz").is_err(), "非 hex 字符应拒绝");
    assert!(node_id_from_hex("abcd").is_err(), "长度不足应拒绝");
    assert!(node_id_from_hex(&"ab".repeat(33)).is_err(), "33 字节应拒绝");
}

#[test]
fn from_seed_reproduces_identity() {
    let id = RoamingIdentity::generate();
    let seed = id.seed();
    let restored = RoamingIdentity::from_seed(&seed);
    assert_eq!(restored.node_id().as_bytes(), id.node_id().as_bytes());
}

#[test]
fn load_or_create_persists_and_reloads() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    let (id1, created1) = RoamingIdentity::load_or_create(ws).expect("首次应成功");
    assert!(created1, "首次应报告新生成");
    let (id2, created2) = RoamingIdentity::load_or_create(ws).expect("二次应成功");
    assert!(!created2, "二次应报告已存在");
    assert_eq!(id1.node_id().as_bytes(), id2.node_id().as_bytes());
}

// ---------------------------------------------------------------------------
// P1 通路：本机双 endpoint 直连互 ping（真 QUIC 帧级往返）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ping_loopback_direct() {
    let server_id = RoamingIdentity::generate();
    let client_id = RoamingIdentity::generate();

    let server = RoamingEndpoint::bind_direct(&server_id, true)
        .await
        .expect("server bind");
    server
        .serve(RoamingProtocolHandler::echo())
        .await
        .expect("server serve");

    let client = RoamingEndpoint::bind_direct(&client_id, false)
        .await
        .expect("client bind");

    // 显式 loopback 地址（对端 IPv4 绑定的端口 + 127.0.0.1）——零发现依赖；
    // bound_sockets 可能含 0.0.0.0/[::] 通配形态，connect 到通配地址平台行为
    // 不定，测试取 IPv4 端口落到 loopback。
    let server_port = server
        .direct_addrs()
        .iter()
        .find(|a| a.is_ipv4())
        .map(|a| a.port())
        .expect("server 应有 IPv4 绑定");
    let addr = EndpointAddr::new(server_id.node_id())
        .with_ip_addr(std::net::SocketAddr::from(([127, 0, 0, 1], server_port)));

    let rtt = tokio::time::timeout(
        Duration::from_secs(15),
        client.ping_addr(addr, Duration::from_secs(10)),
    )
    .await
    .expect("ping 整体超时")
    .expect("ping 应成功");
    assert!(rtt < 10_000, "loopback RTT 应远小于 10s（实际 {rtt}ms）");

    server.shutdown().await;
    client.shutdown().await;
}

#[tokio::test]
async fn ping_wrong_target_times_out() {
    // 无人 serve 的空端点：ping 应在超时内诚实失败，而非假成功。
    let ghost_id = RoamingIdentity::generate();
    let client_id = RoamingIdentity::generate();
    let client = RoamingEndpoint::bind_direct(&client_id, false)
        .await
        .expect("client bind");
    let ghost_port = client
        .direct_addrs()
        .iter()
        .find(|a| a.is_ipv4())
        .map(|a| a.port())
        .expect("client 应有 IPv4 绑定");
    // 同机地址 + 未知 node_id：QUIC 握手会拒绝（身份不符）→ 显式错误。
    let addr = EndpointAddr::new(ghost_id.node_id())
        .with_ip_addr(std::net::SocketAddr::from(([127, 0, 0, 1], ghost_port)));
    let result = tokio::time::timeout(
        Duration::from_secs(20),
        client.ping_addr(addr, Duration::from_secs(5)),
    )
    .await
    .expect("ping 整体超时（测试级）");
    assert!(result.is_err(), "对不存在节点 ping 应失败");
    client.shutdown().await;
}

// ---------------------------------------------------------------------------
// TrustBook
// ---------------------------------------------------------------------------

#[test]
fn trust_book_decision_priority() {
    let book = TrustBook::from_data(TrustBookData {
        trusted: vec![
            format!("iroh:{}", "aa".repeat(32)),
            "BB".repeat(32).to_lowercase(),
        ],
        revoked: vec!["cc".repeat(32)],
    });
    // trusted 放行（带 iroh: 前缀 + 大小写归一）。
    assert!(book.is_trusted(&format!("iroh:{}", "AA".repeat(32))));
    assert!(book.is_trusted(&"bb".repeat(32)));
    // revoked 一票否决（即使再出现在 trusted 也拒——此处单测 revoked 形态）。
    assert!(!book.is_trusted(&"cc".repeat(32)));
    // 默认拒（allowlist 语义）。
    assert!(!book.is_trusted(&"dd".repeat(32)));
}

#[test]
fn trust_book_revoke_removes_from_trusted() {
    let mut book = TrustBook::from_data(TrustBookData {
        trusted: vec!["ee".repeat(32)],
        revoked: vec![],
    });
    assert!(book.is_trusted(&"ee".repeat(32)));
    book.revoke(&format!("iroh:{}", "ee".repeat(32)));
    assert!(!book.is_trusted(&"ee".repeat(32)), "吊销应立即生效");
    // revoked 留痕。
    assert_eq!(book.data().revoked.len(), 1);
    assert!(book.data().trusted.is_empty());
    // unrevoke 后仍不自动恢复信任（重新信任是显式动作）。
    book.unrevoke(&"ee".repeat(32));
    assert!(!book.is_trusted(&"ee".repeat(32)));
}

#[test]
fn trust_book_save_load_roundtrip() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    let mut book = TrustBook::from_data(TrustBookData::default());
    book.trust(&"aa".repeat(32));
    book.revoke(&"bb".repeat(32));
    book.save(ws).expect("save");

    // 文件存在 → load 返回 Some。
    let reloaded = TrustBook::load(ws).expect("load").expect("应已启用");
    assert!(reloaded.is_trusted(&"aa".repeat(32)));
    assert!(!reloaded.is_trusted(&"bb".repeat(32)));
}

#[test]
fn trust_book_absent_means_not_enabled() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let loaded = TrustBook::load(tmp.path()).expect("load 不应报错");
    assert!(loaded.is_none(), "文件不存在 = 未启用（None）");
}
