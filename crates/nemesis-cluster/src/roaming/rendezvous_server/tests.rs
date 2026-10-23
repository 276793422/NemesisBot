//! rendezvous 服务端单测：载荷校验/钳制、token 门禁、TTL 剔除、容量闸 +
//! axum 薄壳 HTTP e2e（register/members/401/400 往返）+ 合并 e2e（中继 +
//! 注册表同进程拉起，双客户端经名册互见后 iroh ping 通——自动发现全链）。
//!
//! 全部端口 0（OS 分配）或临时探测空闲端口，并行安全。

use super::{RegisterError, RendezvousRegistry, sanitize_member, spawn_registry_server};
use crate::roaming::bridge::parse_roaming_address;
use crate::roaming::config::{RelaySettings, RelayTls};
use crate::roaming::endpoint::{RoamingEndpoint, RoamingIdentity, RoamingProtocolHandler};
use crate::roaming::relay_server::spawn_relay_server;
use crate::roaming::rendezvous::{RendezvousMember, member_dial_address, sync_once};

const HEX_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HEX_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn member(node_id: &str, iroh_id: &str) -> RendezvousMember {
    RendezvousMember {
        node_id: node_id.to_string(),
        name: format!("名册-{node_id}"),
        role: "worker".to_string(),
        category: "development".to_string(),
        node_type: "agent".to_string(),
        rpc_port: 21949,
        tags: vec!["dev".to_string()],
        professions: vec!["dev:cpp".to_string()],
        tier: Some("normal".to_string()),
        iroh_id: iroh_id.to_string(),
        build: "aeb1221c".to_string(),
        relay_url: "http://vps.example.com:3340".to_string(),
        features: vec!["cluster".to_string()],
        last_seen_ms: 0,
    }
}

// ---------------------------------------------------------------------------
// sanitize_member：校验 + 钳制
// ---------------------------------------------------------------------------

#[test]
fn sanitize_accepts_valid_and_fills_nothing_unexpected() {
    let m = sanitize_member(member("node-b", HEX_B)).expect("合法载荷应通过");
    assert_eq!(m.node_id, "node-b");
    assert_eq!(m.iroh_id, HEX_B, "已是小写原样");
}

#[test]
fn sanitize_normalizes_iroh_hex_to_lowercase() {
    let upper = HEX_B.to_ascii_uppercase();
    let m = sanitize_member(member("n", &upper)).expect("大写 hex 应通过");
    assert_eq!(m.iroh_id, HEX_B, "iroh_id 归一小写");
}

#[test]
fn sanitize_rejects_empty_node_id_and_bad_hex() {
    assert_eq!(
        sanitize_member(member("", HEX_B)),
        Err(RegisterError::Invalid("node_id 为空".to_string())),
        "空 node_id loud 拒"
    );
    let not_hex = sanitize_member(member("n", "zzzz")); // 非 hex 字符
    assert!(
        matches!(&not_hex, Err(RegisterError::Invalid(msg)) if msg.contains("iroh_id")),
        "{not_hex:?}"
    );
    let short = sanitize_member(member("n", "aabb"));
    assert!(
        matches!(short, Err(RegisterError::Invalid(msg)) if msg.contains("iroh_id")),
        "长度不足 32 字节必须拒"
    );
}

#[test]
fn sanitize_rejects_non_http_relay_url_but_allows_empty() {
    let mut m = member("n", HEX_A);
    m.relay_url = "ftp://example.com".to_string();
    assert!(
        matches!(sanitize_member(m), Err(RegisterError::Invalid(msg)) if msg.contains("relay_url"))
    );
    let mut m = member("n", HEX_A);
    m.relay_url = String::new();
    assert!(
        sanitize_member(m).is_ok(),
        "空 relay_url 合法（拉取方以自己 relay_url 兜底）"
    );
}

#[test]
fn sanitize_clamps_oversized_fields() {
    let mut m = member("n", HEX_A);
    m.node_id = "x".repeat(100);
    assert!(
        matches!(sanitize_member(m), Err(RegisterError::Invalid(msg)) if msg.contains("node_id"))
    );

    let mut m = member("n", HEX_A);
    m.name = "名".repeat(100); // 100 个 CJK 字符
    m.tags = (0..30).map(|i| format!("tag{i}")).collect();
    m.professions = vec!["dev".to_string(); 40];
    let out = sanitize_member(m).expect("超长可钳制字段应通过（截断不拒）");
    assert_eq!(out.name.chars().count(), 64, "name 按 64 字符截断");
    assert_eq!(out.tags.len(), 16, "tags 项数截到 16");
    assert_eq!(out.professions.len(), 16, "professions 项数截到 16");
}

// ---------------------------------------------------------------------------
// RendezvousRegistry：token 门禁 / upsert / TTL / 容量
// ---------------------------------------------------------------------------

#[test]
fn token_gate_bearer_and_query_param() {
    let reg = RendezvousRegistry::new(" s3cret ");
    assert!(reg.token_ok(Some("s3cret"), None), "Bearer 头配对");
    assert!(reg.token_ok(None, Some("s3cret")), "?token= 查询参数配对");
    assert!(!reg.token_ok(Some("wrong"), None));
    assert!(!reg.token_ok(None, None), "缺凭据拒绝");
    assert!(!reg.token_ok(None, Some("")));

    let open = RendezvousRegistry::new("");
    assert!(
        open.token_ok(None, None),
        "空 token = 放行（仅测试 loopback；生产 fail-closed 不走到）"
    );
}

#[test]
fn register_upserts_by_iroh_id_and_touches_last_seen() {
    let reg = RendezvousRegistry::new("t");
    let first = reg.register(member("node-b", HEX_B)).expect("first");
    assert_eq!(
        first.last_seen_ms, 0,
        "返回值是客户端视角载荷（服务端 last_seen 在存储层）"
    );
    assert_eq!(reg.live_count(), 1);

    // 同 iroh_id 换 node_id = 更新而非新增（iroh_id 是存储键）。
    let mut renamed = member("node-b-renamed", HEX_B);
    renamed.tags = vec!["updated".to_string()];
    reg.register(renamed).expect("upsert");
    assert_eq!(reg.live_count(), 1, "同 iroh_id upsert 不增条目");
    let members = reg.live_members();
    assert_eq!(members[0].node_id, "node-b-renamed");
    assert!(members[0].last_seen_ms > 0, "存储层 last_seen 已刷新");
}

#[test]
fn ttl_sweep_removes_stale_members() {
    let reg = RendezvousRegistry::new("t");
    reg.register(member("fresh", HEX_A)).expect("fresh");
    // 直接操纵存储层注入一个过期时刻（TTL 120s 不可真等）——用父模块私有
    // 函数 sweep_expired 验证纯逻辑。
    {
        let mut members = std::collections::HashMap::from([(
            HEX_B.to_string(),
            (
                member("stale", HEX_B),
                std::time::Instant::now() - std::time::Duration::from_secs(121),
            ),
        )]);
        super::sweep_expired(&mut members);
        assert!(members.is_empty(), "过期成员必须被剔除");
    }
    assert_eq!(reg.live_count(), 1, "TTL 内成员不受影响");
}

#[test]
fn capacity_gate_full_after_max_members() {
    let reg = RendezvousRegistry::new("t");
    // 容量闸：填满 MAX_MEMBERS 后新身份拒绝，回头客（已存在键）不受限。
    // 身份用真实 iroh keypair（node_id_from_hex 做 25519 公钥校验——任意
    // hex 会被合法拒）。
    let mut first_hex = String::new();
    for i in 0..crate::roaming::rendezvous::MAX_MEMBERS {
        let hex = crate::roaming::endpoint::RoamingIdentity::generate().node_id_hex();
        if i == 0 {
            first_hex = hex.clone();
        }
        reg.register(member(&format!("n{i}"), &hex))
            .expect("容量内应成功");
    }
    let new_guy = sanitize_member(member("overflow", HEX_B)).expect("载荷本身合法");
    assert_eq!(
        reg.register(new_guy),
        Err(RegisterError::Full),
        "第 257 个新身份必须拒"
    );
    // 回头客：已存在身份再报到不受容量闸拒绝。
    let again = reg
        .register(member("n0", &first_hex))
        .expect("回头客永远进得来");
    assert_eq!(again.node_id, "n0");
}

// ---------------------------------------------------------------------------
// serde wire 形态（缺省字段兼容）
// ---------------------------------------------------------------------------

#[test]
fn member_deserializes_with_missing_optional_fields() {
    let m: RendezvousMember =
        serde_json::from_str(&format!(r#"{{"node_id":"n","iroh_id":"{HEX_A}"}}"#))
            .expect("最小载荷");
    assert_eq!(m.name, "");
    assert!(m.tags.is_empty());
    assert_eq!(m.tier, None);
    assert_eq!(m.last_seen_ms, 0);
}

// ---------------------------------------------------------------------------
// HTTP e2e：spawn → sync_once 双客户端互见 → 401/400 负例
// ---------------------------------------------------------------------------

#[tokio::test]
async fn registry_http_roundtrip_two_clients_mutual_visibility() {
    let mut handle = spawn_registry_server(0, "tok")
        .await
        .expect("spawn（端口 0 = OS 分配）");
    assert_ne!(handle.addr.port(), 0);
    let base = format!("http://127.0.0.1:{}", handle.addr.port());
    let client = reqwest::Client::new();

    let a = member("node-a", HEX_A);
    let seen_by_a = sync_once(&client, &base, "tok", &a)
        .await
        .expect("A register+拉取");
    assert_eq!(seen_by_a.len(), 1, "A 首轮只见自己");
    assert_eq!(seen_by_a[0].iroh_id, HEX_A);

    let b = member("node-b", HEX_B);
    let seen_by_b = sync_once(&client, &base, "tok", &b)
        .await
        .expect("B register+拉取");
    assert_eq!(seen_by_b.len(), 2, "B 起来后名册两员");
    assert_eq!(handle.live_count(), 2);

    // A 再拉：互见成立；排序按 node_id（node-a 在前）。
    let again = sync_once(&client, &base, "tok", &a).await.expect("A 二轮");
    assert_eq!(again.len(), 2);
    assert_eq!(again[0].node_id, "node-a");
    assert_eq!(again[1].node_id, "node-b");

    // 负例：错 token / 无 token = 401；坏 iroh hex = 400。
    let resp = client
        .get(format!("{base}/v1/roaming/members"))
        .bearer_auth("wrong")
        .send()
        .await
        .expect("http");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "错 token 401"
    );
    let resp = client
        .get(format!("{base}/v1/roaming/members"))
        .send()
        .await
        .expect("http");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "无凭据 401"
    );
    let resp = client
        .post(format!("{base}/v1/roaming/register"))
        .bearer_auth("tok")
        .json(&member("bad", "nothex"))
        .send()
        .await
        .expect("http");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::BAD_REQUEST,
        "坏 iroh hex 400"
    );

    handle.shutdown().await;
    handle.shutdown().await; // 幂等
}

// ---------------------------------------------------------------------------
// 合并 e2e：中继 + 注册表同进程拉起，A/B 经名册互见后 iroh ping 通
// ---------------------------------------------------------------------------

/// 临时探测一个空闲 TCP 端口（bind→读 port→drop；测试场景 TOCTOU 可接受）。
async fn probe_free_port() -> u16 {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("probe bind");
    l.local_addr().expect("probe addr").port()
}

#[tokio::test]
async fn relay_with_registry_auto_discovery_end_to_end() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let registry_port = probe_free_port().await;
    let settings = RelaySettings {
        tls: RelayTls::None,
        token: "tok".to_string(),
        registry_port,
        ..RelaySettings::default()
    };
    let mut relay = spawn_relay_server(&settings, tmp.path())
        .await
        .expect("relay spawn");
    let http_port = relay.http_addr.expect("http addr").port();
    let relay_url = format!("http://127.0.0.1:{http_port}");
    let registry = relay
        .registry
        .as_ref()
        .expect("token 非空 + registry_port>0 = 注册表必须随中继拉起");
    let registry_url = format!("http://127.0.0.1:{}", registry.addr.port());

    // A/B 各自以真实 iroh 身份向名册报到（生产客户端循环同款调用）。
    let id_b = RoamingIdentity::generate();
    let id_a = RoamingIdentity::generate();
    let client = reqwest::Client::new();

    let self_b = RendezvousMember {
        node_id: "node-b".to_string(),
        iroh_id: id_b.node_id_hex(),
        relay_url: relay_url.clone(),
        ..member("node-b", &id_b.node_id_hex())
    };
    // `member()` 已带 node_id；上面 struct update 语义 = 以 member(...) 为底、
    // 显式覆盖三键。B 先报到，A 同步时即见 B。
    sync_once(&client, &registry_url, "tok", &self_b)
        .await
        .expect("B register");

    let self_a = RendezvousMember {
        node_id: "node-a".to_string(),
        iroh_id: id_a.node_id_hex(),
        relay_url: relay_url.clone(),
        ..member("node-a", &id_a.node_id_hex())
    };
    let members = sync_once(&client, &registry_url, "tok", &self_a)
        .await
        .expect("A register+拉取");
    assert_eq!(members.len(), 2, "名册两员（含自己）");

    // A 侧合并循环同款：过滤自己 → 组拨号地址 → 解析 → ping 通。
    let peer = members
        .iter()
        .find(|m| m.iroh_id != id_a.node_id_hex())
        .expect("B 必须在名册");
    let dial = member_dial_address(peer, &relay_url);
    let addr = parse_roaming_address(&dial).expect("iroh:<hex>@<relay> 形态解析");

    let ep_b = RoamingEndpoint::bind_custom_relay(&id_b, true, &relay_url, Some("tok"))
        .await
        .expect("B bind");
    ep_b.serve(RoamingProtocolHandler::echo())
        .await
        .expect("B serve");
    let ep_a = RoamingEndpoint::bind_custom_relay(&id_a, false, &relay_url, Some("tok"))
        .await
        .expect("A bind");

    let rtt = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        ep_a.ping_addr(addr, std::time::Duration::from_secs(15)),
    )
    .await
    .expect("ping 整体 deadline")
    .expect("经名册发现的地址必须 ping 通（自动发现全链成立）");
    assert!(rtt <= 15_000);

    ep_a.shutdown().await;
    ep_b.shutdown().await;
    relay.shutdown().await; // 注册表 + 中继对称停（先注册表后 relay）
    assert!(relay.registry.is_none(), "停机后注册表槽清空");
}
