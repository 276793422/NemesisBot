//! relay_server 单测：spawn→addr 探测→shutdown 全链（端口 0 = OS 分配）+
//! 自签证书幂等（二次 spawn 复用同证书字节）+ PEM 解析辅助 + URL 组装。
//!
//! 全部绑定 0.0.0.0:0（OS 分配端口），无固定端口占用，并行安全。

use super::{
    advertised_relay_url, ensure_self_signed_cert, parse_certs_pem, parse_key_pem, relay_cert_path,
    relay_key_path, spawn_relay_server,
};
use crate::roaming::bridge::parse_roaming_address;
use crate::roaming::config::{RelaySettings, RelayTls};
use crate::roaming::endpoint::{RoamingEndpoint, RoamingIdentity, RoamingProtocolHandler};

/// 基础 settings（http/quic 端口 0 = OS 分配）。
fn base_settings() -> RelaySettings {
    RelaySettings {
        enabled: true,
        http_port: 0,
        quic_port: 0,
        ..RelaySettings::default()
    }
}

// ---------------------------------------------------------------------------
// spawn → addr 探测 → shutdown（tls=none：纯 HTTP transport）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn spawn_none_tls_http_only_probe_and_shutdown() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let settings = RelaySettings {
        tls: RelayTls::None,
        ..base_settings()
    };
    let mut handle = spawn_relay_server(&settings, tmp.path())
        .await
        .expect("spawn should succeed");
    assert!(handle.running());
    let http = handle.http_addr.expect("http addr must be bound");
    assert_ne!(http.port(), 0, "port 0 请求必须拿到 OS 分配的真实端口");
    assert!(handle.quic_addr.is_none(), "tls=none 不开 QUIC transport");

    handle.shutdown().await;
    assert!(!handle.running());
    // 幂等：二次 shutdown 无操作不炸。
    handle.shutdown().await;
}

// ---------------------------------------------------------------------------
// spawn（tls=self-signed：HTTP + QUIC 双臂 + 证书落盘 + 幂等）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn spawn_self_signed_dual_arm_and_cert_files() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let settings = base_settings(); // 默认 tls=self-signed
    let mut handle = spawn_relay_server(&settings, tmp.path())
        .await
        .expect("spawn should succeed");
    assert!(handle.http_addr.expect("http").port() != 0);
    assert!(handle.quic_addr.is_some(), "self-signed 档 QUIC 臂必须监听");
    assert!(handle.quic_addr.expect("quic").port() != 0);

    let cert = std::fs::read_to_string(relay_cert_path(tmp.path())).expect("cert file");
    let key = std::fs::read_to_string(relay_key_path(tmp.path())).expect("key file");
    assert!(cert.contains("BEGIN CERTIFICATE"));
    assert!(key.contains("BEGIN PRIVATE KEY"), "rcgen 产物 = PKCS#8");

    handle.shutdown().await;
}

#[tokio::test]
async fn self_signed_cert_is_idempotent_across_spawns() {
    let tmp = tempfile::tempdir().expect("tempdir");

    let mut first = spawn_relay_server(&base_settings(), tmp.path())
        .await
        .expect("first spawn");
    let cert1 = std::fs::read_to_string(relay_cert_path(tmp.path())).expect("cert");
    let key1 = std::fs::read_to_string(relay_key_path(tmp.path())).expect("key");
    first.shutdown().await;

    let mut second = spawn_relay_server(&base_settings(), tmp.path())
        .await
        .expect("second spawn");
    let cert2 = std::fs::read_to_string(relay_cert_path(tmp.path())).expect("cert");
    let key2 = std::fs::read_to_string(relay_key_path(tmp.path())).expect("key");
    second.shutdown().await;

    assert_eq!(cert1, cert2, "二次 spawn 必须复用同证书（幂等，不重生）");
    assert_eq!(key1, key2);
}

// ---------------------------------------------------------------------------
// ensure_self_signed_cert：SAN 形态 + 直接调用幂等
// ---------------------------------------------------------------------------

#[test]
fn self_signed_san_ip_and_dns_forms() {
    let tmp = tempfile::tempdir().expect("tempdir");

    // external_host = IP 形态 → IP SAN。
    let (cert, _key) =
        ensure_self_signed_cert(tmp.path(), "192.168.1.10").expect("generate (ip host)");
    let ders = parse_certs_pem(&cert).expect("cert pem parses");
    assert_eq!(ders.len(), 1);

    // external_host = 域名形态 → DNS SAN。
    let (cert_dns, _) =
        ensure_self_signed_cert(tmp.path(), "vps.example.com").expect("generate (dns host)");
    assert!(cert_dns.contains("BEGIN CERTIFICATE"));

    // external_host 空 → 仅 localhost + 127.0.0.1 基础 SAN。
    let (cert_empty, _) = ensure_self_signed_cert(tmp.path(), "").expect("generate (no host)");
    assert!(cert_empty.contains("BEGIN CERTIFICATE"));
}

#[test]
fn self_signed_reuses_existing_files_even_if_host_changed() {
    // 幂等语义 = 文件存在即复用（host 变化不触发重生——重生会导致已分发
    // 证书失效；确需换证书由用户删文件显式触发）。
    let tmp = tempfile::tempdir().expect("tempdir");
    let (cert1, key1) = ensure_self_signed_cert(tmp.path(), "10.0.0.1").expect("first");
    let (cert2, key2) = ensure_self_signed_cert(tmp.path(), "172.16.0.1").expect("second");
    assert_eq!(cert1, cert2);
    assert_eq!(key1, key2);
}

// ---------------------------------------------------------------------------
// PEM 解析辅助
// ---------------------------------------------------------------------------

#[test]
fn parse_key_pem_rejects_certificate_labeled_input() {
    let cert_pem = "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n";
    let err = parse_key_pem(cert_pem).expect_err("证书段喂私钥解析器必须报错");
    assert!(err.contains("PRIVATE KEY"), "错误要指明缺什么: {err}");
}

#[test]
fn parse_pem_missing_end_marker_is_error() {
    let broken = "-----BEGIN CERTIFICATE-----\nAAAA\n"; // 无 END
    let err = parse_certs_pem(broken).expect_err("缺 END 必须报错");
    assert!(err.contains("END"), "{err}");
}

#[test]
fn parse_pem_chain_multiple_certs() {
    let chain = "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n\
                 -----BEGIN CERTIFICATE-----\nBBBB\n-----END CERTIFICATE-----\n";
    let ders = parse_certs_pem(chain).expect("chain parses");
    assert_eq!(ders.len(), 2);
    // "AAAA"（6bit 全零）→ 3 个 0x00；"BBBB"（6bit 全 000001）→ 0x04,0x10,0x41。
    assert_eq!(ders[0].as_ref(), &[0x00u8, 0x00, 0x00][..]);
    assert_eq!(ders[1].as_ref(), &[0x04u8, 0x10, 0x41][..]);
}

// ---------------------------------------------------------------------------
// 自建中继端到端：双客户端 bind_custom_relay 经本 relay ping/echo 往返
// ---------------------------------------------------------------------------

/// P2 通路验证：spawn 中继（tls=none）→ A/B 两端 `bind_custom_relay` 接入 →
/// A 以 `iroh:<hex>@http://127.0.0.1:<port>` 地址形态（bridge 解析 →
/// `with_relay_url`）dial B → ping/echo 往返成立。整链零 n0 依赖（Minimal
/// 基座 + 显式中继地址，纯 loopback hermetic）。
#[tokio::test]
async fn loopback_client_pings_through_self_relay() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let settings = RelaySettings {
        tls: RelayTls::None,
        ..base_settings()
    };
    let mut relay = spawn_relay_server(&settings, tmp.path())
        .await
        .expect("relay spawn");
    let http_port = relay.http_addr.expect("http addr").port();
    let relay_url = format!("http://127.0.0.1:{http_port}");

    let id_b = RoamingIdentity::generate();
    let node_b_hex = id_b.node_id_hex();
    let ep_b = RoamingEndpoint::bind_custom_relay(&id_b, true, &relay_url, None)
        .await
        .expect("B bind");
    ep_b.serve(RoamingProtocolHandler::echo())
        .await
        .expect("B serve");

    let id_a = RoamingIdentity::generate();
    let ep_a = RoamingEndpoint::bind_custom_relay(&id_a, false, &relay_url, None)
        .await
        .expect("A bind");

    // 生产同款地址形态：bridge 解析 `iroh:<hex>@http://...` → with_relay_url。
    let addr = parse_roaming_address(&format!("iroh:{node_b_hex}@{relay_url}"))
        .expect("roaming addr parse");

    let rtt = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        ep_a.ping_addr(addr, std::time::Duration::from_secs(15)),
    )
    .await
    .expect("ping 整体 deadline")
    .expect("ping through self relay 应成立");
    assert!(rtt <= 15_000);

    ep_a.shutdown().await;
    ep_b.shutdown().await;
    relay.shutdown().await;
}

/// 寻址边界钉死：自建中继形态（Minimal 基座，零发现层）下**裸** `iroh:<hex>`
/// 拨号必须失败——iroh 1.0 的 Minimal 预设只设 crypto provider（无
/// dns/pkarr address lookup，也无 relay 型发现），裸 node_id 无解析路径。
/// 这是 announce 侧必须携带 `iroh:<hex>@<relay_url>` 完整形态的根因
/// （cluster 自报 addresses 收割该形态；对端入池 → bridge with_relay_url）。
#[tokio::test]
async fn bare_node_id_dial_fails_on_self_relay_minimal_base() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let settings = RelaySettings {
        tls: RelayTls::None,
        ..base_settings()
    };
    let mut relay = spawn_relay_server(&settings, tmp.path())
        .await
        .expect("relay spawn");
    let relay_url = format!("http://127.0.0.1:{}", relay.http_addr.expect("http").port());

    let id_b = RoamingIdentity::generate();
    let ep_b = RoamingEndpoint::bind_custom_relay(&id_b, true, &relay_url, None)
        .await
        .expect("B bind");
    ep_b.serve(RoamingProtocolHandler::echo())
        .await
        .expect("B serve");

    let id_a = RoamingIdentity::generate();
    let ep_a = RoamingEndpoint::bind_custom_relay(&id_a, false, &relay_url, None)
        .await
        .expect("A bind");
    let addr =
        parse_roaming_address(&format!("iroh:{}", id_b.node_id_hex())).expect("裸形态解析本身合法");

    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        ep_a.ping_addr(addr, std::time::Duration::from_secs(10)),
    )
    .await
    .expect("ping 整体 deadline");
    assert!(
        outcome.is_err(),
        "Minimal 基座上裸 node_id 拨号必须失败（实际: {outcome:?}）——若此断言开始失败，说明 iroh 引入了 relay 型发现，announce 侧可回归裸形态"
    );

    ep_a.shutdown().await;
    ep_b.shutdown().await;
    relay.shutdown().await;
}

/// token 闸负例：服务端带 token，客户端不带（或不配对）→ ping 必须失败
/// （SharedToken 拒绝在连接建立前，dial 显式报错而非黑洞超时）。
#[tokio::test]
async fn relay_token_mismatch_rejects_client() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let settings = RelaySettings {
        tls: RelayTls::None,
        token: "s3cret".to_string(),
        ..base_settings()
    };
    let mut relay = spawn_relay_server(&settings, tmp.path())
        .await
        .expect("relay spawn");
    let relay_url = format!("http://127.0.0.1:{}", relay.http_addr.expect("http").port());

    let id_b = RoamingIdentity::generate();
    let node_b_hex = id_b.node_id_hex();
    // B（accept 端）同样必须携带配对 token——SharedToken 闸对中继连接本身
    // 生效（不带 token 的 B 根本进不了 relay，C 的 dial 会无的放矢）。
    let ep_b = RoamingEndpoint::bind_custom_relay(&id_b, true, &relay_url, Some("s3cret"))
        .await
        .expect("B bind");
    ep_b.serve(RoamingProtocolHandler::echo())
        .await
        .expect("B serve");

    // 无 token 客户端（负例臂）：中继接入被 SharedToken 拒绝 → dial 必败。
    let id_a = RoamingIdentity::generate();
    let ep_a = RoamingEndpoint::bind_custom_relay(&id_a, false, &relay_url, None)
        .await
        .expect("A bind");
    let addr = parse_roaming_address(&format!("iroh:{node_b_hex}@{relay_url}"))
        .expect("roaming addr parse");

    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        ep_a.ping_addr(addr, std::time::Duration::from_secs(10)),
    )
    .await
    .expect("ping 整体 deadline");
    assert!(
        outcome.is_err(),
        "无 token 客户端必须被拒（实际: {outcome:?}）"
    );

    // 配对 token 客户端（对照）：同 relay 必须通。
    let id_c = RoamingIdentity::generate();
    let ep_c = RoamingEndpoint::bind_custom_relay(&id_c, false, &relay_url, Some("s3cret"))
        .await
        .expect("C bind");
    let addr_c = parse_roaming_address(&format!("iroh:{node_b_hex}@{relay_url}"))
        .expect("roaming addr parse");
    let rtt = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        ep_c.ping_addr(addr_c, std::time::Duration::from_secs(10)),
    )
    .await
    .expect("ping 整体 deadline")
    .expect("配对 token 客户端应通");
    assert!(rtt <= 10_000);

    ep_a.shutdown().await;
    ep_c.shutdown().await;
    ep_b.shutdown().await;
    relay.shutdown().await;
}

// ---------------------------------------------------------------------------
// advertised_relay_url
// ---------------------------------------------------------------------------

#[test]
fn advertised_url_uses_external_host_when_set() {
    let settings = RelaySettings {
        http_port: 3340,
        external_host: "vps.example.com".to_string(),
        ..RelaySettings::default()
    };
    assert_eq!(
        advertised_relay_url(&settings),
        "http://vps.example.com:3340"
    );
}

#[test]
fn advertised_url_trims_whitespace_host() {
    let settings = RelaySettings {
        http_port: 7842,
        external_host: " 10.1.2.3 ".to_string(),
        ..RelaySettings::default()
    };
    assert_eq!(advertised_relay_url(&settings), "http://10.1.2.3:7842");
}

#[test]
fn advertised_url_empty_host_falls_back_shape() {
    // 空主机 = 探测本机 IP（环境相关）——只断言形态：http://<host>:<port>。
    let settings = RelaySettings {
        http_port: 3340,
        ..RelaySettings::default()
    };
    let url = advertised_relay_url(&settings);
    assert!(url.starts_with("http://"), "{url}");
    assert!(url.ends_with(":3340"), "{url}");
    let host = url
        .strip_prefix("http://")
        .and_then(|u| u.strip_suffix(":3340"))
        .expect("shape");
    assert!(
        !host.is_empty() && !host.contains(':'),
        "host 段非空且无端口残留: {url}"
    );
}
