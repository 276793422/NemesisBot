//! rendezvous 服务端单测（移植自宿主 rendezvous_server/tests.rs，QUIC 插件化
//! M1）：载荷校验/钳制、token 门禁、TTL 剔除、容量闸 + axum 薄壳 HTTP e2e
//! （register/members/401/400 往返）+ 合并 e2e（中继 + 注册表同进程拉起，
//! 双客户端经名册互见后 iroh ping 通——自动发现全链）。
//!
//! 类型分工：单元测走插件内 [`crate::types::RendezvousMember`]；HTTP e2e 的
//! 客户端循环走**插件** [`crate::registry_client::sync_once`]（M2 拍板 7：
//! sync_once 搬插件，宿主副本已删——插件客户端打插件服务端）；wire 同构
//! 证明保留在 raw HTTP 负例（`serde_json::json!` 手工同字段集——字段对齐
//! 纪律见宿主 rendezvous.rs `RendezvousMember`，M3 删 dev-dep 后注释级
//! 双侧同步）。容量闸身份用插件 [`Identity`] 真实 keypair（校验是 ed25519
//! 公钥点级——任意 hex 会被合法拒，与宿主同强）。
//!
//! 全部端口 0（OS 分配）或临时探测空闲端口，并行安全。

use super::{RegisterError, RendezvousRegistry, sanitize_member, spawn_registry_server};
use crate::endpoint::{EndpointConfig, parse_roaming_address, spawn_endpoint};
use crate::identity::Identity;
use crate::registry_client::sync_once;
use crate::types::{MAX_MEMBERS, RendezvousMember as PluginMember, member_dial_address};

const HEX_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HEX_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

/// 插件类型成员构造（单元测：sanitize/register/registry 行为）。
fn pmember(node_id: &str, iroh_id: &str) -> PluginMember {
    PluginMember {
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
    let m = sanitize_member(pmember("node-b", HEX_B)).expect("合法载荷应通过");
    assert_eq!(m.node_id, "node-b");
    assert_eq!(m.iroh_id, HEX_B, "已是小写原样");
}

#[test]
fn sanitize_normalizes_iroh_hex_to_lowercase() {
    let upper = HEX_B.to_ascii_uppercase();
    let m = sanitize_member(pmember("n", &upper)).expect("大写 hex 应通过");
    assert_eq!(m.iroh_id, HEX_B, "iroh_id 归一小写");
}

#[test]
fn sanitize_rejects_empty_node_id_and_bad_hex() {
    assert_eq!(
        sanitize_member(pmember("", HEX_B)),
        Err(RegisterError::Invalid("node_id 为空".to_string())),
        "空 node_id loud 拒"
    );
    let not_hex = sanitize_member(pmember("n", "zzzz")); // 非 hex 字符
    assert!(
        matches!(&not_hex, Err(RegisterError::Invalid(msg)) if msg.contains("iroh_id")),
        "{not_hex:?}"
    );
    let short = sanitize_member(pmember("n", "aabb"));
    assert!(
        matches!(short, Err(RegisterError::Invalid(msg)) if msg.contains("iroh_id")),
        "长度不足 32 字节必须拒"
    );
}

#[test]
fn valid_iroh_id_agrees_with_official_curve_check() {
    // 同强证明（不猜具体哪些编码非法）：真实身份双方都接受；对首字节扫描
    // 256 个变体，逐个断言插件与 iroh 官方曲线校验判定一致，且必存在双方
    // 同拒的变体（约一半 y 值不在曲线上，全过概率 2^-256）。这钉住「不止
    // 形态闸、与 iroh 官方 PublicKey::from_bytes 同强同判」的升级语义。
    let real = Identity::generate().node_id_hex();
    assert!(
        crate::types::valid_iroh_id(&real).is_ok(),
        "真实身份必须通过"
    );
    assert!(
        iroh::PublicKey::from_bytes(
            &hex::decode(&real)
                .expect("hex decode")
                .try_into()
                .expect("32B")
        )
        .is_ok(),
        "官方曲线校验同身份也必须接受（sanity）"
    );

    let raw = hex::decode(&real).expect("hex decode");
    let mut found_reject = None;
    for v in 0u8..=255u8 {
        let mut corrupt = raw.clone();
        corrupt[0] = v;
        let s = hex::encode(&corrupt);
        let arr: [u8; 32] = corrupt.clone().try_into().expect("32B");
        let official_ok = iroh::PublicKey::from_bytes(&arr).is_ok();
        let plugin_ok = crate::types::valid_iroh_id(&s).is_ok();
        assert_eq!(
            official_ok, plugin_ok,
            "byte0={v} 插件与官方判定必须一致（official_ok={official_ok}）"
        );
        if !official_ok && found_reject.is_none() {
            found_reject = Some(v);
        }
    }
    assert!(
        found_reject.is_some(),
        "256 个变体必含非法曲线点（每个 ~50% 概率非法）"
    );
}

#[test]
fn sanitize_rejects_non_http_relay_url_but_allows_empty() {
    let mut m = pmember("n", HEX_A);
    m.relay_url = "ftp://example.com".to_string();
    assert!(
        matches!(sanitize_member(m), Err(RegisterError::Invalid(msg)) if msg.contains("relay_url"))
    );
    let mut m = pmember("n", HEX_A);
    m.relay_url = String::new();
    assert!(
        sanitize_member(m).is_ok(),
        "空 relay_url 合法（拉取方以自己 relay_url 兜底）"
    );
}

#[test]
fn sanitize_clamps_oversized_fields() {
    let mut m = pmember("n", HEX_A);
    m.node_id = "x".repeat(100);
    assert!(
        matches!(sanitize_member(m), Err(RegisterError::Invalid(msg)) if msg.contains("node_id"))
    );

    let mut m = pmember("n", HEX_A);
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
    let first = reg.register(pmember("node-b", HEX_B)).expect("first");
    assert_eq!(
        first.last_seen_ms, 0,
        "返回值是客户端视角载荷（服务端 last_seen 在存储层）"
    );
    assert_eq!(reg.live_count(), 1);

    // 同 iroh_id 换 node_id = 更新而非新增（iroh_id 是存储键）。
    let mut renamed = pmember("node-b-renamed", HEX_B);
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
    reg.register(pmember("fresh", HEX_A)).expect("fresh");
    // 直接操纵存储层注入一个过期时刻（TTL 120s 不可真等）——用父模块私有
    // 函数 sweep_expired 验证纯逻辑。
    {
        let mut members = std::collections::HashMap::from([(
            HEX_B.to_string(),
            (
                pmember("stale", HEX_B),
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
    // 身份用真实 iroh keypair（valid_iroh_id 做 ed25519 公钥点校验——任意
    // hex 会被合法拒，与官方同强）。
    let mut first_hex = String::new();
    for i in 0..MAX_MEMBERS {
        let hex = Identity::generate().node_id_hex();
        if i == 0 {
            first_hex = hex.clone();
        }
        reg.register(pmember(&format!("n{i}"), &hex))
            .expect("容量内应成功");
    }
    let new_guy = sanitize_member(pmember("overflow", HEX_B)).expect("载荷本身合法");
    assert_eq!(
        reg.register(new_guy),
        Err(RegisterError::Full),
        "第 257 个新身份必须拒"
    );
    // 回头客：已存在身份再报到不受容量闸拒绝。
    let again = reg
        .register(pmember("n0", &first_hex))
        .expect("回头客永远进得来");
    assert_eq!(again.node_id, "n0");
}

// ---------------------------------------------------------------------------
// serde wire 形态（缺省字段兼容）
// ---------------------------------------------------------------------------

#[test]
fn member_deserializes_with_missing_optional_fields() {
    let m: PluginMember =
        serde_json::from_str(&format!(r#"{{"node_id":"n","iroh_id":"{HEX_A}"}}"#))
            .expect("最小载荷");
    assert_eq!(m.name, "");
    assert!(m.tags.is_empty());
    assert_eq!(m.tier, None);
    assert_eq!(m.last_seen_ms, 0);
}

// ---------------------------------------------------------------------------
// HTTP e2e：spawn → sync_once 双客户端互见 → 401/400 负例
//（宿主类型结构体直接打插件服务端——wire 同构证明）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn registry_http_roundtrip_two_clients_mutual_visibility() {
    let mut handle = spawn_registry_server(0, "tok")
        .await
        .expect("spawn（端口 0 = OS 分配）");
    assert_ne!(handle.addr.port(), 0);
    let base = format!("http://127.0.0.1:{}", handle.addr.port());
    let client = reqwest::Client::new();

    let a = pmember("node-a", HEX_A);
    let seen_by_a = sync_once(&client, &base, "tok", &a)
        .await
        .expect("A register+拉取");
    assert_eq!(seen_by_a.len(), 1, "A 首轮只见自己");
    assert_eq!(seen_by_a[0].iroh_id, HEX_A);

    let b = pmember("node-b", HEX_B);
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
    // wire 同构证明：serde_json::json! 手工构造与宿主 RendezvousMember 同
    // 字段集（字段对齐纪律见宿主 rendezvous.rs——M3 删 dev-dep 后注释级
    // 双侧同步），直接打插件服务端。
    let resp = client
        .post(format!("{base}/v1/roaming/register"))
        .bearer_auth("tok")
        .json(&serde_json::json!({
            "node_id": "bad",
            "name": "名册-bad",
            "role": "worker",
            "category": "development",
            "node_type": "agent",
            "rpc_port": 21949,
            "tags": ["dev"],
            "professions": ["dev:cpp"],
            "tier": "normal",
            "iroh_id": "nothex",
            "build": "aeb1221c",
            "relay_url": "http://vps.example.com:3340",
            "features": ["cluster"],
            "last_seen_ms": 0,
        }))
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
    let ws_a = tempfile::tempdir().expect("ws A");
    let ws_b = tempfile::tempdir().expect("ws B");
    let registry_port = probe_free_port().await;
    let settings = crate::types::RelaySettings {
        tls: crate::types::RelayTls::None,
        token: "tok".to_string(),
        registry_port,
        ..crate::types::RelaySettings::default()
    };
    let mut relay = crate::relay_server::spawn_relay_server(&settings, tmp.path())
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
    let id_b = Identity::generate();
    let id_a = Identity::generate();
    let client = reqwest::Client::new();

    let self_b = PluginMember {
        node_id: "node-b".to_string(),
        iroh_id: id_b.node_id_hex(),
        relay_url: relay_url.clone(),
        ..pmember("node-b", &id_b.node_id_hex())
    };
    // `pmember()` 已带 node_id；上面 struct update 语义 = 以 pmember(...) 为
    // 底、显式覆盖三键。B 先报到，A 同步时即见 B。
    sync_once(&client, &registry_url, "tok", &self_b)
        .await
        .expect("B register");

    let self_a = PluginMember {
        node_id: "node-a".to_string(),
        iroh_id: id_a.node_id_hex(),
        relay_url: relay_url.clone(),
        ..pmember("node-a", &id_a.node_id_hex())
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

    // 双端拉起插件内端点（spawn_endpoint 内置 BridgeAcceptor serve；B 端
    // trust 簿放行 A——PING 探针同样过闸）。
    let mk_cfg = |token: &str| EndpointConfig {
        relay_url: relay_url.clone(),
        relay_token: token.to_string(),
        ..EndpointConfig::default()
    };
    let ep_b = spawn_endpoint(&mk_cfg("tok"), &id_b.seed(), ws_b.path())
        .await
        .expect("B bind");
    crate::endpoint::save_trust_book(
        ws_b.path(),
        &crate::endpoint::TrustBookData {
            trusted: vec![id_a.node_id_hex()],
            revoked: vec![],
        },
    )
    .expect("B trust 簿写入");
    let ep_a = spawn_endpoint(&mk_cfg("tok"), &id_a.seed(), ws_a.path())
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
