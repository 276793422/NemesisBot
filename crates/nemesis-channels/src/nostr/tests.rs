//! nostr 通道测试（P27）：NIP-04 加解密 / NIP-01 事件与帧形态 / 通道行为 /
//! mock relay 契约（进程内 tokio-tungstenite WS server，与生产同一条 0.26 栈）。
//!
//! 密钥纪律：测试用私钥只有两类——**硬编码 TEST-ONLY 常量**（`11`×32 /
//! `22`×32，可读性一眼可辨非真实密钥）与**程序随机生成值**；绝不出现任何
//! 可用的真实密钥。真实 relay 端到端挂账（无凭证/无外网保证）。

use super::*;
use futures::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::Message;

/// TEST-ONLY 私钥 A（`11`×32，合法 secp256k1 标量，非真实密钥）。
const TEST_SK_A: &str = "1111111111111111111111111111111111111111111111111111111111111111";
/// TEST-ONLY 私钥 B（`22`×32，合法 secp256k1 标量，非真实密钥）。
const TEST_SK_B: &str = "2222222222222222222222222222222222222222222222222222222222222222";

fn test_bus() -> broadcast::Sender<InboundMessage> {
    let (tx, _) = broadcast::channel(256);
    tx
}

/// 生成随机 32 字节 → 64-hex 私钥（测试专用，程序随机值）。
fn random_secret_hex() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn valid_config(private_key: &str) -> NostrConfig {
    NostrConfig {
        relays: vec!["ws://127.0.0.1:1".to_string()],
        private_key: private_key.to_string(),
        allow_from: Vec::new(),
        reconnect_secs: 1,
    }
}

// ---------------------------------------------------------------------------
// 密钥
// ---------------------------------------------------------------------------

#[test]
fn test_keys_derive_x_only_public_key() {
    let keys = NostrKeys::from_hex_secret(TEST_SK_A).unwrap();
    let pk = keys.x_only_public_key();
    assert_eq!(pk.len(), 64, "x-only 公钥须为 64-hex");
    assert!(hex::decode(pk).is_ok());
    // 同一私钥推导必须确定。
    assert_eq!(
        NostrKeys::from_hex_secret(TEST_SK_A)
            .unwrap()
            .x_only_public_key(),
        pk
    );
    // 不同私钥推导不同公钥。
    assert_ne!(
        NostrKeys::from_hex_secret(TEST_SK_B)
            .unwrap()
            .x_only_public_key(),
        pk
    );
}

#[test]
fn test_unresolved_reference_prefix_loud_reject() {
    // config 只存引用——残留引用前缀必须响亮拒绝，绝不静默当值用。
    for raw in ["vault:nostr-key", "env:NOSTR_PRIVATE_KEY", "yaml:nostr"] {
        let err = NostrKeys::from_hex_secret(raw).unwrap_err().to_string();
        assert!(err.contains("引用"), "前缀 {raw} 应报「未解析引用」: {err}");
    }
}

#[test]
fn test_invalid_hex_rejected() {
    assert!(NostrKeys::from_hex_secret("zzzz").is_err());
    // 长度错误（31 字节）。
    assert!(NostrKeys::from_hex_secret(&"11".repeat(31)).is_err());
    // 空值在 channel new() 层先拦，这里拦 32 字节外的合法 hex。
    assert!(NostrKeys::from_hex_secret("00").is_err());
}

// ---------------------------------------------------------------------------
// NIP-04：加密 / 解密
// ---------------------------------------------------------------------------

#[test]
fn test_nip04_roundtrip_both_directions() {
    // 随机密钥对双向往返：A→B 与 B→A。
    let a = NostrKeys::from_hex_secret(&random_secret_hex()).unwrap();
    let b = NostrKeys::from_hex_secret(&random_secret_hex()).unwrap();

    let plaintext = "你好，nostr！NIP-04 roundtrip 🤖";
    let content = encrypt_dm(&a, b.x_only_public_key(), plaintext).unwrap();
    let decrypted = decrypt_dm(&b, a.x_only_public_key(), &content).unwrap();
    assert_eq!(decrypted, plaintext);

    let content2 = encrypt_dm(&b, a.x_only_public_key(), plaintext).unwrap();
    let decrypted2 = decrypt_dm(&a, b.x_only_public_key(), &content2).unwrap();
    assert_eq!(decrypted2, plaintext);
}

#[test]
fn test_nip04_shared_secret_symmetry() {
    // ECDH 对称性：shared(A_priv, B_pub) == shared(B_priv, A_pub)——
    // 这是 A 加密 B 能解的根本前提。
    let a = NostrKeys::from_hex_secret(&random_secret_hex()).unwrap();
    let b = NostrKeys::from_hex_secret(&random_secret_hex()).unwrap();
    let sa = shared_secret(&a, b.x_only_public_key()).unwrap();
    let sb = shared_secret(&b, a.x_only_public_key()).unwrap();
    assert_eq!(sa, sb);
}

#[test]
fn test_nip04_content_format_and_deterministic_iv() {
    let a = NostrKeys::from_hex_secret(TEST_SK_A).unwrap();
    let b = NostrKeys::from_hex_secret(TEST_SK_B).unwrap();

    let iv = [0x42u8; 16];
    let content = encrypt_dm_with_iv(&a, b.x_only_public_key(), "determinism", &iv).unwrap();

    // 格式：`base64(ct)?iv=base64(iv)`。
    let (ct_b64, iv_b64) = content.split_once("?iv=").expect("缺少 ?iv= 分隔符");
    let iv_decoded = base64::engine::general_purpose::STANDARD
        .decode(iv_b64)
        .unwrap();
    assert_eq!(iv_decoded.len(), 16);
    assert_eq!(iv_decoded, iv);
    assert!(
        base64::engine::general_purpose::STANDARD
            .decode(ct_b64)
            .is_ok()
    );

    // 同 IV 同输入 → 同密文（确定性）。
    let again = encrypt_dm_with_iv(&a, b.x_only_public_key(), "determinism", &iv).unwrap();
    assert_eq!(content, again);

    // 不同 IV → 不同密文。
    let mut other_iv = [0x42u8; 16];
    other_iv[0] = 0x43;
    let different =
        encrypt_dm_with_iv(&a, b.x_only_public_key(), "determinism", &other_iv).unwrap();
    assert_ne!(content, different);

    // 随机 IV 版本可解回。
    let randomized = encrypt_dm(&a, b.x_only_public_key(), "random iv").unwrap();
    assert_eq!(
        decrypt_dm(&b, a.x_only_public_key(), &randomized).unwrap(),
        "random iv"
    );
}

#[test]
fn test_nip04_decrypt_garbage_rejected() {
    let a = NostrKeys::from_hex_secret(TEST_SK_A).unwrap();
    let b = NostrKeys::from_hex_secret(TEST_SK_B).unwrap();

    // 缺 ?iv= 分隔。
    assert!(decrypt_dm(&b, a.x_only_public_key(), "notnip04").is_err());
    // IV 长度错。
    let bad_iv = format!(
        "AAAA?iv={}",
        base64::engine::general_purpose::STANDARD.encode([0u8; 8])
    );
    assert!(decrypt_dm(&b, a.x_only_public_key(), &bad_iv).is_err());
    // 非法 base64。
    assert!(decrypt_dm(&b, a.x_only_public_key(), "%%%?iv=%%%").is_err());
}

// ---------------------------------------------------------------------------
// NIP-01：事件 id / 签署 / 校验
// ---------------------------------------------------------------------------

#[test]
fn test_event_id_canonical_serialization() {
    // 钉死 canonical 序列化形态：紧凑 JSON、无空格、[0,pubkey,created_at,kind,tags,content]。
    let pubkey = "aa".repeat(32);
    let tags = vec![vec!["p".to_string(), "bb".repeat(32)]];
    let id = event_id(&pubkey, 1700000000, 4, &tags, "hi");

    let expected = format!(
        "[0,\"{pubkey}\",1700000000,4,[[\"p\",\"{}\"]],\"hi\"]",
        "bb".repeat(32)
    );
    let expected_id = {
        use sha2::Digest;
        hex::encode(sha2::Sha256::digest(expected.as_bytes()))
    };
    assert_eq!(id, expected_id);
}

#[test]
fn test_build_dm_event_fields_and_signature() {
    let a = NostrKeys::from_hex_secret(TEST_SK_A).unwrap();
    let b_pub = NostrKeys::from_hex_secret(TEST_SK_B)
        .unwrap()
        .x_only_public_key()
        .to_string();

    let event =
        build_dm_event_with_iv(&a, &b_pub, "signed hello", 1700000001, &[0x07u8; 16]).unwrap();

    assert_eq!(event.kind, NOSTR_KIND_DM);
    assert_eq!(event.pubkey, a.x_only_public_key());
    assert_eq!(event.created_at, 1700000001);
    assert_eq!(event.tags, vec![vec!["p".to_string(), b_pub.clone()]]);
    assert!(event.content.contains("?iv="));
    assert_eq!(event.sig.len(), 128, "BIP-340 签名 = 64 字节 = 128 hex");

    // id 自洽 + 签名可验。
    assert_eq!(
        event.id,
        event_id(
            &event.pubkey,
            event.created_at,
            event.kind,
            &event.tags,
            &event.content
        )
    );
    assert!(verify_event(&event), "自产事件签名必须通过校验");
}

#[test]
fn test_verify_event_detects_tamper() {
    let a = NostrKeys::from_hex_secret(TEST_SK_A).unwrap();
    let b_pub = NostrKeys::from_hex_secret(TEST_SK_B)
        .unwrap()
        .x_only_public_key()
        .to_string();
    let mut event =
        build_dm_event_with_iv(&a, &b_pub, "original", 1700000002, &[0x09u8; 16]).unwrap();
    assert!(verify_event(&event));

    // 篡改 content → id 不再匹配。
    event.content = format!("{}x", event.content);
    assert!(!verify_event(&event), "篡改 content 必须校验失败");
    event.content = event.content[..event.content.len() - 1].to_string();

    // 篡改签名（id 不变）→ Schnorr 校验失败。
    let mut sig_bytes = hex::decode(&event.sig).unwrap();
    sig_bytes[0] ^= 0xFF;
    event.sig = hex::encode(&sig_bytes);
    assert!(!verify_event(&event), "篡改签名必须校验失败");

    // 冒名事件（B 的公钥 + A 的签名）→ 校验失败。
    let mut forged =
        build_dm_event_with_iv(&a, &b_pub, "forged", 1700000003, &[0x0Au8; 16]).unwrap();
    forged.pubkey = b_pub.clone();
    assert!(!verify_event(&forged), "冒名公钥必须校验失败");
}

// ---------------------------------------------------------------------------
// 跨实现一致性（nostr-tools 参考实现，node → Rust 方向）
// ---------------------------------------------------------------------------

/// 跨实现测试钥（`01..20` 递增序列，TEST-ONLY 一次性值，非真实密钥）。
const CROSS_SK: &str = "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20";
/// 同钥推导的 x-only 公钥（nostr-tools 侧一致）。
const CROSS_PUB: &str = "84bf7562262bbd6940085748f3be6afa52ae317155181ece31b66351ccffa4b0";
const CROSS_PLAINTEXT: &str = "hello cross-implementation 一致性验证";
/// nostr-tools `nip04.encrypt` 输出（随机 IV，NIP-04 载荷形态）。
const CROSS_NODE_ENCRYPTED: &str =
    "vcCMcNOnGuu0jzJj3seoguOUQleRj14I5DaaqDrNgCxEBuvE6pjaG5RjkpxRAY79?iv=IxU0koRarKFfy5RsM/nwKw==";
/// nostr-tools `finalizeEvent` 签署的 kind-4 加密 DM 事件。
const CROSS_NODE_EVENT_JSON: &str = r#"{"kind":4,"created_at":1735689600,"tags":[["p","84bf7562262bbd6940085748f3be6afa52ae317155181ece31b66351ccffa4b0"]],"content":"vcCMcNOnGuu0jzJj3seoguOUQleRj14I5DaaqDrNgCxEBuvE6pjaG5RjkpxRAY79?iv=IxU0koRarKFfy5RsM/nwKw==","pubkey":"84bf7562262bbd6940085748f3be6afa52ae317155181ece31b66351ccffa4b0","id":"1d66d1650979958a1cf7d9310604fa3a2afe36cf7925af87baa1d745b025702d","sig":"290d3f14436939ea5e4f57d0ac945937e7bf05a3060a1fb5365e63907045456bca5fd8e4c4240d4071b39b2cdabc0e5fc91d8909011a6e8fc39d0088689c15d3"}"#;

/// 「自洽 ≠ 互操作」的互操作半边：参考实现（nostr-tools 2.25.2）加密的
/// 载荷本方必须能解，参考实现签署的事件本方必须验得过。
#[test]
fn test_nip04_cross_impl_nostr_tools_node_to_rust() {
    // 密钥推导与参考实现一致。
    let keys = NostrKeys::from_hex_secret(CROSS_SK).unwrap();
    assert_eq!(keys.x_only_public_key(), CROSS_PUB, "ECDH 公钥推导须与 nostr-tools 一致");

    // 参考实现加密的载荷 → 本方解密出同一明文。
    let decrypted = decrypt_dm(&keys, CROSS_PUB, CROSS_NODE_ENCRYPTED).unwrap();
    assert_eq!(decrypted, CROSS_PLAINTEXT);

    // 参考实现签署的事件 → id 重算一致 + Schnorr 验签通过。
    let event: NostrEvent = serde_json::from_str(CROSS_NODE_EVENT_JSON).unwrap();
    assert_eq!(
        event.id,
        event_id(&event.pubkey, event.created_at, event.kind, &event.tags, &event.content),
        "canonical 序列化须与 nostr-tools 的 id 计算一致"
    );
    assert!(verify_event(&event), "nostr-tools 签署的事件必须通过本方 Schnorr 校验");
}

/// bech32 形态（nip19 `nsec1…`/`npub1…`，nostr-tools 对同一测试钥的真实
/// 编码）响亮拒绝——本通道密钥契约是 64-hex 裸值，绝不静默接受 bech32。
#[test]
fn test_bech32_secret_form_loudly_rejected() {
    let nsec = "nsec1qypqxpq9qcrsszg2pvxq6rs0zqg3yyc5z5tpwxqergd3c8g7rusqpqcc2y";
    let npub = "npub1sjlh2c3x9w7kjsqg2ay080n2lff2uvt325vpan33ke34rn8l5jcqawh57m";
    assert!(
        NostrKeys::from_hex_secret(nsec).is_err(),
        "nsec1 bech32 私钥形态必须拒绝"
    );
    assert!(
        NostrKeys::from_hex_secret(npub).is_err(),
        "npub1 bech32 形态当私钥必须拒绝"
    );
}

// ---------------------------------------------------------------------------
// relay 帧形态
// ---------------------------------------------------------------------------

#[test]
fn test_build_req_frame_shape() {
    let our_pub = NostrKeys::from_hex_secret(TEST_SK_A)
        .unwrap()
        .x_only_public_key()
        .to_string();
    let frame = build_req_frame(&our_pub, 1700000000);

    let parsed: serde_json::Value = serde_json::from_str(&frame).unwrap();
    let arr = parsed.as_array().unwrap();
    assert_eq!(arr[0], "REQ");
    assert_eq!(arr[1], NOSTR_SUB_ID);
    let filter = &arr[2];
    assert_eq!(
        filter["kinds"],
        serde_json::json!([4]),
        "只订阅 kind-4 加密 DM"
    );
    assert_eq!(
        filter["#p"],
        serde_json::json!([our_pub]),
        "#p 过滤 p-tag 指向本机"
    );
    assert_eq!(filter["since"], 1700000000);
}

#[test]
fn test_build_event_frame_shape() {
    let a = NostrKeys::from_hex_secret(TEST_SK_A).unwrap();
    let b_pub = NostrKeys::from_hex_secret(TEST_SK_B)
        .unwrap()
        .x_only_public_key()
        .to_string();
    let event = build_dm_event_with_iv(&a, &b_pub, "frame", 1700000004, &[0x0Bu8; 16]).unwrap();

    let frame = build_event_frame(&event);
    let parsed: serde_json::Value = serde_json::from_str(&frame).unwrap();
    let arr = parsed.as_array().unwrap();
    assert_eq!(arr[0], "EVENT");
    assert_eq!(arr[1]["id"], event.id);
    assert_eq!(arr[1]["kind"], 4);
    assert_eq!(arr[1]["content"], event.content);
}

#[test]
fn test_parse_relay_frame_variants() {
    // 订阅下发形态 ["EVENT", sub_id, event]。
    let a = NostrKeys::from_hex_secret(TEST_SK_A).unwrap();
    let b_pub = NostrKeys::from_hex_secret(TEST_SK_B)
        .unwrap()
        .x_only_public_key()
        .to_string();
    let event = build_dm_event_with_iv(&a, &b_pub, "push", 1700000005, &[0x0Cu8; 16]).unwrap();
    let with_sub =
        serde_json::to_string(&serde_json::json!(["EVENT", NOSTR_SUB_ID, event])).unwrap();
    assert!(matches!(
        parse_relay_frame(&with_sub),
        Some(RelayFrame::Event(_))
    ));

    // 发布回显形态 ["EVENT", event]。
    let echo = serde_json::to_string(&serde_json::json!(["EVENT", event])).unwrap();
    assert!(matches!(
        parse_relay_frame(&echo),
        Some(RelayFrame::Event(_))
    ));

    // EOSE / NOTICE → 忽略。
    assert!(matches!(
        parse_relay_frame(r#"["EOSE","nemesisbot"]"#),
        Some(RelayFrame::Ignored)
    ));
    assert!(matches!(
        parse_relay_frame(r#"["NOTICE","hi"]"#),
        Some(RelayFrame::Ignored)
    ));
    // 非法 JSON → None。
    assert!(parse_relay_frame("not json").is_none());
    // 空数组 → None。
    assert!(parse_relay_frame("[]").is_none());
}

// ---------------------------------------------------------------------------
// 通道构造 / 生命周期
// ---------------------------------------------------------------------------

#[test]
fn test_channel_new_validation() {
    // 空 relays。
    let mut cfg = valid_config(TEST_SK_A);
    cfg.relays = Vec::new();
    assert!(NostrChannel::new(cfg, test_bus()).is_err());

    // 非 ws/wss scheme。
    let mut cfg = valid_config(TEST_SK_A);
    cfg.relays = vec!["https://relay.example".to_string()];
    assert!(NostrChannel::new(cfg, test_bus()).is_err());

    // 空 private_key。
    let cfg = valid_config("");
    assert!(NostrChannel::new(cfg, test_bus()).is_err());

    // 未解析引用前缀。
    let cfg = valid_config("env:NOSTR_PRIVATE_KEY");
    let err = match NostrChannel::new(cfg, test_bus()) {
        Err(e) => e.to_string(),
        Ok(_) => panic!("引用前缀应被响亮拒绝"),
    };
    assert!(err.contains("引用"), "引用前缀应被响亮拒绝: {err}");

    // 合法构造 + reconnect_secs 缺省归一。
    let mut cfg = valid_config(TEST_SK_A);
    cfg.reconnect_secs = 0;
    let ch = NostrChannel::new(cfg, test_bus()).unwrap();
    assert_eq!(ch.name(), "nostr");
    assert_eq!(ch.config.reconnect_secs, 5);
    assert_eq!(ch.public_key(), ch.keys.x_only_public_key());
}

#[tokio::test]
async fn test_channel_lifecycle_start_stop() {
    let ch = NostrChannel::new(valid_config(TEST_SK_A), test_bus()).unwrap();
    assert!(!ch.is_running());

    // relay 地址指向未监听端口：连接失败走退避重连，不 crash。
    ch.start().await.unwrap();
    assert!(ch.is_running());

    ch.stop().await.unwrap();
    assert!(!ch.is_running());
    ch.stop().await.unwrap(); // 双重 stop 幂等
}

// ---------------------------------------------------------------------------
// 事件处理器：白名单 / 去重 / 解密
// ---------------------------------------------------------------------------

#[test]
fn test_processor_allowlist_and_dedup() {
    let bot = NostrKeys::from_hex_secret(&random_secret_hex()).unwrap();
    let peer = NostrKeys::from_hex_secret(&random_secret_hex()).unwrap();
    let stranger = NostrKeys::from_hex_secret(&random_secret_hex()).unwrap();

    let (bus, mut rx) = broadcast::channel(16);
    // 直接构造处理器（bot 的 keys + peer 白名单）。
    let processor = NostrEventProcessor {
        keys: bot.clone(),
        x_only_pub: bot.x_only_public_key().to_string(),
        base: BaseChannel::with_allow_list("nostr", vec![peer.x_only_public_key().to_string()]),
        seen_events: Arc::new(parking_lot::RwLock::new(HashMap::new())),
        bus_sender: bus,
    };

    // 陌生发件人 → 丢弃。
    let from_stranger =
        build_dm_event(&stranger, bot.x_only_public_key(), "stranger", 1700000010).unwrap();
    processor.process_relay_event(&from_stranger);
    assert!(rx.try_recv().is_err(), "白名单外发件人不得进总线");

    // 白名单内发件人 → 解密进总线。
    let from_peer =
        build_dm_event(&peer, bot.x_only_public_key(), "hello peer", 1700000011).unwrap();
    processor.process_relay_event(&from_peer);
    let inbound = rx.try_recv().expect("白名单内发件人应进总线");
    assert_eq!(inbound.channel, "nostr");
    assert_eq!(inbound.sender_id, peer.x_only_public_key());
    assert_eq!(inbound.chat_id, peer.x_only_public_key());
    assert_eq!(inbound.content, "hello peer");
    assert_eq!(inbound.session_key, peer.x_only_public_key());

    // 同一事件重放 → 去重丢弃。
    processor.process_relay_event(&from_peer);
    assert!(rx.try_recv().is_err(), "重复事件不得重复投递");

    // 非 kind-4 → 忽略。
    let mut other_kind =
        build_dm_event(&peer, bot.x_only_public_key(), "meta", 1700000012).unwrap();
    other_kind.kind = 1;
    // kind 改了 id/sig 不再匹配 → 校验失败丢弃（行为一致：不进总线）。
    processor.process_relay_event(&other_kind);
    assert!(rx.try_recv().is_err());
}

#[test]
fn test_processor_rejects_unsigned_event() {
    let bot = NostrKeys::from_hex_secret(&random_secret_hex()).unwrap();
    let peer = NostrKeys::from_hex_secret(&random_secret_hex()).unwrap();

    let (bus, mut rx) = broadcast::channel(16);
    let processor = NostrEventProcessor {
        keys: bot.clone(),
        x_only_pub: bot.x_only_public_key().to_string(),
        base: BaseChannel::with_allow_list("nostr", Vec::new()),
        seen_events: Arc::new(parking_lot::RwLock::new(HashMap::new())),
        bus_sender: bus,
    };

    // 签名无效的加密事件 → 诚实丢弃（不进总线）。
    let mut event = build_dm_event(&peer, bot.x_only_public_key(), "unsigned", 1700000020).unwrap();
    event.sig = "00".repeat(64);
    processor.process_relay_event(&event);
    assert!(rx.try_recv().is_err(), "签名校验失败的事件不得进总线");
}

// ---------------------------------------------------------------------------
// mock relay 契约测试（订阅 filter 形态 / EVENT 发布格式 / 加解密端到端）
// ---------------------------------------------------------------------------

/// 进程内 mock relay：接受一条连接，读 REQ（回传给测试）→ 推送注入事件 →
/// 泵出后续下行帧（通道的 EVENT 发布）给测试断言。
async fn spawn_mock_relay(
    push_event: String,
) -> (
    String,
    oneshot::Receiver<String>,
    tokio::sync::mpsc::Receiver<String>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (req_tx, req_rx) = oneshot::channel::<String>();
    let (out_tx, out_rx) = tokio::sync::mpsc::channel::<String>(32);

    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("mock relay accept");
        let mut ws = tokio_tungstenite::accept_async(stream)
            .await
            .expect("mock relay ws 握手");

        // 第一帧必须是 REQ 订阅。
        let first = ws.next().await.expect("mock relay 读 REQ").unwrap();
        let req_text = match first {
            Message::Text(t) => t.to_string(),
            other => panic!("mock relay 首帧非 Text: {other:?}"),
        };
        let _ = req_tx.send(req_text);

        // 推送注入事件。
        ws.send(Message::Text(push_event.into())).await.unwrap();

        // 泵出通道后续下行（EVENT 发布等）。
        while let Some(Ok(msg)) = ws.next().await {
            if let Message::Text(t) = msg {
                if out_tx.send(t.to_string()).await.is_err() {
                    break;
                }
            }
        }
    });

    (format!("ws://{addr}"), req_rx, out_rx)
}

#[tokio::test]
async fn test_mock_relay_end_to_end() {
    let flow = async {
        // 随机测试密钥对（bot = 本机通道，peer = 远端用户）。
        let bot = NostrKeys::from_hex_secret(&random_secret_hex()).unwrap();
        let peer = NostrKeys::from_hex_secret(&random_secret_hex()).unwrap();

        // peer → bot 的加密 DM 事件（模拟 relay 上已存在的消息）。
        let inbound_event = build_dm_event(
            &peer,
            bot.x_only_public_key(),
            "hello via relay",
            chrono::Utc::now().timestamp(),
        )
        .unwrap();
        let push_frame =
            serde_json::to_string(&serde_json::json!(["EVENT", NOSTR_SUB_ID, inbound_event]))
                .unwrap();

        let (relay_url, req_rx, mut out_rx) = spawn_mock_relay(push_frame).await;

        // 启动通道（白名单只收 peer；私钥 = bot 的随机测试密钥）。
        let bot_sk_hex = {
            let bytes = bot.secret.to_bytes();
            hex::encode(bytes.as_slice())
        };
        let cfg = NostrConfig {
            relays: vec![relay_url],
            private_key: bot_sk_hex,
            allow_from: vec![peer.x_only_public_key().to_string()],
            reconnect_secs: 1,
        };
        let (bus, mut bus_rx) = broadcast::channel(16);
        let ch = NostrChannel::new(cfg, bus).unwrap();
        ch.start().await.unwrap();

        // ① 契约：通道首帧 = REQ，filter 形态正确。
        let req_text = req_rx.await.expect("通道应发送 REQ 订阅");
        let req: serde_json::Value = serde_json::from_str(&req_text).unwrap();
        let req_arr = req.as_array().unwrap();
        assert_eq!(req_arr[0], "REQ");
        assert_eq!(req_arr[1], NOSTR_SUB_ID);
        assert_eq!(req_arr[2]["kinds"], serde_json::json!([4]));
        assert_eq!(
            req_arr[2]["#p"],
            serde_json::json!([bot.x_only_public_key()]),
            "订阅 filter 必须指向本机公钥"
        );

        // ② 契约：relay 下发的加密 DM → 解密进总线。
        let inbound = bus_rx.recv().await.expect("解密后的 DM 应进总线");
        assert_eq!(inbound.channel, "nostr");
        assert_eq!(inbound.sender_id, peer.x_only_public_key());
        assert_eq!(inbound.content, "hello via relay");

        // ③ 契约：出站 → NIP-04 加密 → EVENT 发布帧格式正确、可解密验签。
        let outbound = OutboundMessage {
            channel: "nostr".to_string(),
            chat_id: peer.x_only_public_key().to_string(),
            content: "reply via relay".to_string(),
            message_type: String::new(),
            meta: Default::default(),
        };
        ch.send(outbound).await.expect("出站发布应成功");

        let published = tokio::time::timeout(std::time::Duration::from_secs(5), out_rx.recv())
            .await
            .expect("等待出站 EVENT 超时")
            .expect("mock relay 应收到出站帧");
        let frame: serde_json::Value = serde_json::from_str(&published).unwrap();
        assert_eq!(frame[0], "EVENT", "出站帧必须是 [\"EVENT\", event] 形态");
        let published_event: NostrEvent = serde_json::from_value(frame[1].clone()).unwrap();
        assert_eq!(published_event.kind, NOSTR_KIND_DM);
        assert_eq!(published_event.pubkey, bot.x_only_public_key());
        assert!(
            verify_event(&published_event),
            "发布的 EVENT 签名必须通过完整校验"
        );
        let decrypted =
            decrypt_dm(&peer, &published_event.pubkey, &published_event.content).unwrap();
        assert_eq!(decrypted, "reply via relay");

        ch.stop().await.unwrap();
    };

    tokio::time::timeout(std::time::Duration::from_secs(20), flow)
        .await
        .expect("mock relay 端到端整体超时（20s）——检查连接/订阅/泵时序");
}

#[tokio::test]
async fn test_send_requires_running_and_recipient() {
    let ch = NostrChannel::new(valid_config(TEST_SK_A), test_bus()).unwrap();

    // 未运行 → 拒绝。
    let msg = OutboundMessage {
        channel: "nostr".to_string(),
        chat_id: "aa".repeat(32),
        content: "x".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    assert!(
        ch.send(msg.clone()).await.is_err(),
        "未运行的通道出站应报错"
    );

    // 运行但无在连 relay → 拒绝（端口 1 不可达）。
    ch.start().await.unwrap();
    // 给 relay 任务一点时间确认连不上、sinks 为空。
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let err = ch.send(msg).await.unwrap_err().to_string();
    assert!(
        err.contains("relay"),
        "无在连 relay 应报 relay 相关错误: {err}"
    );
    ch.stop().await.unwrap();

    // 空收件人（在连 relay 也缺收件人字段）→ 拒绝。
    let ch2 = NostrChannel::new(valid_config(TEST_SK_A), test_bus()).unwrap();
    let no_rcpt = OutboundMessage {
        channel: "nostr".to_string(),
        chat_id: String::new(),
        content: "x".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    let _ = ch2.start().await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(ch2.send(no_rcpt).await.is_err(), "空 chat_id 出站应报错");
    ch2.stop().await.unwrap();
}
