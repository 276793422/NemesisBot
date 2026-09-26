//! nostr 通道（P27，Wave 3 能力扩展）：NIP-04 加密 DM over NIP-01 relay WebSocket。
//!
//! # 协议面（手写实现，不引 nostr-sdk）
//!
//! - **NIP-01**：relay WebSocket 消息三类——入站订阅 `["REQ", sub_id, filter]`、
//!   事件下发 `["EVENT", sub_id, event]`、发布 `["EVENT", event]`；事件 id =
//!   sha256(`[0,pubkey,created_at,kind,tags,content]` 紧凑 JSON)；签名 = BIP-340
//!   Schnorr（对 32 字节 id 直接签，非预哈希）。
//! - **NIP-04**（加密 DM，kind 4）：共享密钥 = ECDH(本机私钥, 对端 x-only 公钥)
//!   的 x 坐标（32 字节，无二次哈希——x-only 公钥按 BIP-340 约定取偶数 y）；
//!   AES-256-CBC + PKCS#7，随机 16 字节 IV；密文形态
//!   `base64(ciphertext) + "?iv=" + base64(iv)`；事件 p-tag 指向收件人。
//!
//! # 依赖评估结论（nostr-sdk vs 手写，2026-09-26）
//!
//! `nostr-sdk 0.45.4` 闭包核心新增：`nostr`（→ C 绑定的 secp256k1 0.30 +
//! bitcoin_hashes + bech32）、`async-wsocket`（→ **tokio-tungstenite 0.28**，
//! 与工作区钉死的 0.26 双版本并存）、**rand 0.10**（工作区 0.8 双版本）、
//! default features 拉进 **ring** TLS 栈，外加 negentropy / nostr-gossip / lru /
//! async-utility / universal-time 等与本场景无关的子系统。为一个 kind-4 DM
//! 引入这些过重。手写净新增仅 3 个 optional crate：`k256 0.13`（纯 Rust
//! secp256k1，与树内 p256 0.13 同族共享 elliptic-curve/signature/sha2）、
//! `aes 0.8`（已被 aes-gcm 拉进工作区树）、`cbc 0.1`（唯一全新编译单元，
//! cipher 0.4 家族与 aes 0.8 配套）。
//! 协议面小且稳定（NIP-04 是已冻结的兼容规范），选**手写**。
//!
//! # 密钥管理（config 只存引用，不存明文）
//!
//! `NostrConfig::private_key` 约定收**装配点已解析的** 64-hex 私钥：config.json
//! 里该字段只写 `vault:` / `env:` / `yaml:` 引用（与 line.channel_access_token、
//! web.auth_token 同一机制），gateway 装配点用
//! `nemesisbot::common::resolve_secret_or_empty` 解析后传入。本模块对带未解析
//! 引用前缀的值**响亮拒绝**（绝不把引用字符串当密钥用）。nsec/npub bech32
//! 解码 v1 不支持（挂账）。
//!
//! # 诚实边界
//!
//! - 真实 relay 端到端不做（无凭证/无外网保证），契约测试用进程内 mock relay
//!   （tokio-tungstenite server，与生产同一条 tungstenite 0.26 栈）。
//! - NIP-04 交叉实现测试向量挂账（无外网可信源）；正确性由往返 + 对称性 +
//!   确定性 IV 向量 + 格式断言钉死。
//! - NIP-44（NIP-04 的后继）v1 不做，NIP-04 上游明确保留兼容。

#![allow(dead_code)] // channel API 完整面（加密/事件构造函数供装配点与测试复用），部分当前未调用

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, info, warn};

use aes::Aes256;
use base64::Engine as _;
use cbc::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use k256::elliptic_curve::sec1::ToEncodedPoint;
// DecompactPoint：x-only 公钥重构（BIP-340 约定；y 奇偶不影响 ECDH 共享 x 坐标，
// 因为 ±P 的标量乘结果互为取负，x 坐标相同）。
use k256::elliptic_curve::point::DecompactPoint;
use k256::schnorr::{Signature, SigningKey, VerifyingKey};
use k256::{AffinePoint, FieldBytes, SecretKey};
use sha2::Digest;

use nemesis_types::channel::{InboundMessage, OutboundMessage};
use nemesis_types::error::{NemesisError, Result};

use crate::base::{BaseChannel, Channel};

/// NIP-01 DM 事件 kind。
pub const NOSTR_KIND_DM: u32 = 4;
/// 订阅 sub_id（mock relay 契约测试锚定此值）。
pub const NOSTR_SUB_ID: &str = "nemesisbot";

type Aes256CbcEnc = cbc::Encryptor<Aes256>;
type Aes256CbcDec = cbc::Decryptor<Aes256>;

/// 未解析引用前缀集合——config 只存引用，装配点必须解析后再传入。
const SECRET_REFERENCE_PREFIXES: [&str; 3] = ["vault:", "env:", "yaml:"];

// ---------------------------------------------------------------------------
// 密钥
// ---------------------------------------------------------------------------

/// nostr 密钥对（BIP-340 x-only 形态公钥 + secp256k1 私钥）。
///
/// 手工 Debug：绝不把私钥材料带进日志/调试输出。
#[derive(Clone)]
pub struct NostrKeys {
    /// ECDH 用 secp256k1 私钥。
    secret: SecretKey,
    /// 事件签名用 BIP-340 Schnorr 私钥（同一 32 字节标量的另一封装）。
    signing: SigningKey,
    /// 本机 x-only 公钥（64-hex 小写）。
    x_only_pub: String,
}

impl std::fmt::Debug for NostrKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NostrKeys")
            .field("x_only_pub", &self.x_only_pub)
            .field("secret", &"<redacted>")
            .field("signing", &"<redacted>")
            .finish()
    }
}

impl NostrKeys {
    /// 从 64-hex 私钥构造。拒绝 `vault:`/`env:`/`yaml:` 引用前缀——
    /// config 只存引用，引用解析属装配点职责（见模块头"密钥管理"）。
    pub fn from_hex_secret(hex_sk: &str) -> Result<Self> {
        let raw = hex_sk.trim();
        for prefix in SECRET_REFERENCE_PREFIXES {
            if raw.starts_with(prefix) {
                return Err(NemesisError::Channel(format!(
                    "nostr private_key 仍是未解析的引用（{prefix}…）——config 只存引用，\
                     装配点须先用 resolve_secret_or_empty 解析成明文再传入"
                )));
            }
        }

        let bytes = hex::decode(raw)
            .map_err(|e| NemesisError::Channel(format!("nostr private_key 非法 hex: {e}")))?;
        if bytes.len() != 32 {
            return Err(NemesisError::Channel(format!(
                "nostr private_key 须为 32 字节（64 hex），收到 {} 字节",
                bytes.len()
            )));
        }

        let secret = SecretKey::from_slice(&bytes)
            .map_err(|e| NemesisError::Channel(format!("nostr private_key 非法标量: {e}")))?;
        let signing = SigningKey::from_bytes(&bytes)
            .map_err(|e| NemesisError::Channel(format!("nostr signing key 构造失败: {e}")))?;

        // x-only 公钥 = 压缩 SEC1 点去掉 0x02/0x03 前缀的 32 字节 x 坐标。
        let encoded = secret.public_key().as_affine().to_encoded_point(true);
        let x_only_pub = hex::encode(&encoded.as_bytes()[1..33]);

        Ok(Self {
            secret,
            signing,
            x_only_pub,
        })
    }

    /// 本机 x-only 公钥（64-hex 小写）。
    pub fn x_only_public_key(&self) -> &str {
        &self.x_only_pub
    }
}

/// 解析 64-hex x-only 公钥为 32 字节。
fn parse_x_only_pubkey(hex_pk: &str) -> Result<[u8; 32]> {
    let bytes = hex::decode(hex_pk.trim())
        .map_err(|e| NemesisError::Channel(format!("nostr pubkey 非法 hex: {e}")))?;
    if bytes.len() != 32 {
        return Err(NemesisError::Channel(format!(
            "nostr pubkey 须为 32 字节 x-only（64 hex），收到 {} 字节",
            bytes.len()
        )));
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

// ---------------------------------------------------------------------------
// NIP-04：ECDH + AES-256-CBC
// ---------------------------------------------------------------------------

/// 计算 NIP-04 共享密钥：ECDH(本机私钥, 对端 x-only 公钥) 的 x 坐标。
///
/// x-only 公钥按 BIP-340 约定补偶数 y（与 nostr 官方实现一致）。
fn shared_secret(keys: &NostrKeys, peer_x_only: &str) -> Result<[u8; 32]> {
    let peer_x = parse_x_only_pubkey(peer_x_only)?;
    // x-only 公钥重构仿射点（DecompactPoint；y 奇偶不影响共享 x 坐标，见 import 注）。
    let point = AffinePoint::decompact(FieldBytes::from_slice(&peer_x))
        .into_option()
        .ok_or_else(|| NemesisError::Channel("nostr pubkey 不在曲线上".to_string()))?;

    let shared =
        k256::elliptic_curve::ecdh::diffie_hellman(&keys.secret.to_nonzero_scalar(), &point);
    let mut out = [0u8; 32];
    out.copy_from_slice(&shared.raw_secret_bytes());
    Ok(out)
}

/// NIP-04 加密（随机 IV）：返回 `base64(ct)?iv=base64(iv)` 密文串。
pub fn encrypt_dm(keys: &NostrKeys, recipient_x_only: &str, plaintext: &str) -> Result<String> {
    let mut iv = [0u8; 16];
    use rand::RngCore;
    rand::thread_rng().fill_bytes(&mut iv);
    encrypt_dm_with_iv(keys, recipient_x_only, plaintext, &iv)
}

/// NIP-04 加密（指定 IV，确定性——测试向量与单测锚定用）。
pub fn encrypt_dm_with_iv(
    keys: &NostrKeys,
    recipient_x_only: &str,
    plaintext: &str,
    iv: &[u8; 16],
) -> Result<String> {
    let key = shared_secret(keys, recipient_x_only)?;
    let ciphertext = Aes256CbcEnc::new((&key).into(), iv.into())
        .encrypt_padded_vec_mut::<cbc::cipher::block_padding::Pkcs7>(plaintext.as_bytes());
    Ok(format!(
        "{}?iv={}",
        base64::engine::general_purpose::STANDARD.encode(&ciphertext),
        base64::engine::general_purpose::STANDARD.encode(iv)
    ))
}

/// NIP-04 解密：输入 `base64(ct)?iv=base64(iv)` 密文串。
pub fn decrypt_dm(keys: &NostrKeys, sender_x_only: &str, content: &str) -> Result<String> {
    let (ct_b64, iv_b64) = content.split_once("?iv=").ok_or_else(|| {
        NemesisError::Channel("nostr DM content 缺少 ?iv= 分隔符（非 NIP-04 形态）".to_string())
    })?;
    let ciphertext = base64::engine::general_purpose::STANDARD
        .decode(ct_b64.trim())
        .map_err(|e| NemesisError::Channel(format!("nostr DM 密文 base64 非法: {e}")))?;
    let iv_raw = base64::engine::general_purpose::STANDARD
        .decode(iv_b64.trim())
        .map_err(|e| NemesisError::Channel(format!("nostr DM IV base64 非法: {e}")))?;
    if iv_raw.len() != 16 {
        return Err(NemesisError::Channel(format!(
            "nostr DM IV 须为 16 字节，收到 {}",
            iv_raw.len()
        )));
    }
    let mut iv = [0u8; 16];
    iv.copy_from_slice(&iv_raw);

    let key = shared_secret(keys, sender_x_only)?;
    let plaintext = Aes256CbcDec::new((&key).into(), (&iv).into())
        .decrypt_padded_vec_mut::<cbc::cipher::block_padding::Pkcs7>(&ciphertext)
        .map_err(|e| NemesisError::Channel(format!("nostr DM 解密失败: {e}")))?;
    String::from_utf8(plaintext)
        .map_err(|e| NemesisError::Channel(format!("nostr DM 明文非 UTF-8: {e}")))
}

// ---------------------------------------------------------------------------
// NIP-01：事件构造 / 校验 / relay 帧
// ---------------------------------------------------------------------------

/// nostr 事件（NIP-01 字段序即线上 JSON 序）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NostrEvent {
    pub id: String,
    pub pubkey: String,
    pub created_at: i64,
    pub kind: u32,
    pub tags: Vec<Vec<String>>,
    pub content: String,
    pub sig: String,
}

/// 计算 NIP-01 事件 id：sha256(`[0,pubkey,created_at,kind,tags,content]` 紧凑 JSON)。
pub fn event_id(
    pubkey: &str,
    created_at: i64,
    kind: u32,
    tags: &[Vec<String>],
    content: &str,
) -> String {
    let serialization = serde_json::to_string(&serde_json::json!([
        0, pubkey, created_at, kind, tags, content
    ]))
    .expect("NIP-01 事件序列化不可失败");
    let digest = sha2::Sha256::digest(serialization.as_bytes());
    hex::encode(digest)
}

/// 构造并签署 NIP-04 DM 事件（随机 IV）。
pub fn build_dm_event(
    keys: &NostrKeys,
    recipient_x_only: &str,
    plaintext: &str,
    created_at: i64,
) -> Result<NostrEvent> {
    let mut iv = [0u8; 16];
    use rand::RngCore;
    rand::thread_rng().fill_bytes(&mut iv);
    build_dm_event_with_iv(keys, recipient_x_only, plaintext, created_at, &iv)
}

/// 构造并签署 NIP-04 DM 事件（指定 IV，确定性——测试用）。
pub fn build_dm_event_with_iv(
    keys: &NostrKeys,
    recipient_x_only: &str,
    plaintext: &str,
    created_at: i64,
    iv: &[u8; 16],
) -> Result<NostrEvent> {
    parse_x_only_pubkey(recipient_x_only)?;
    let content = encrypt_dm_with_iv(keys, recipient_x_only, plaintext, iv)?;
    let pubkey = keys.x_only_pub.clone();
    let tags = vec![vec!["p".to_string(), recipient_x_only.to_string()]];
    let id = event_id(&pubkey, created_at, NOSTR_KIND_DM, &tags, &content);

    // BIP-340 Schnorr：对 32 字节事件 id 直接签（非预哈希）。
    // 必须用 sign_raw——Signer trait 会先 SHA-256 一次（对 NIP-01 是二次哈希，
    // 会让全生态 relay 拒签）；sign_raw 的 message 直接进 challenge。
    let mut id_bytes = [0u8; 32];
    id_bytes.copy_from_slice(&hex::decode(&id).expect("自产 id 必为合法 hex"));
    let mut aux = [0u8; 32];
    {
        use rand::RngCore;
        rand::thread_rng().fill_bytes(&mut aux);
    }
    let sig: Signature = keys
        .signing
        .sign_raw(&id_bytes, &aux)
        .map_err(|e| NemesisError::Channel(format!("nostr 事件签名失败: {e}")))?;

    Ok(NostrEvent {
        id,
        pubkey,
        created_at,
        kind: NOSTR_KIND_DM,
        tags,
        content,
        sig: hex::encode(sig.to_bytes()),
    })
}

/// 完整校验事件：id 重算一致 + BIP-340 Schnorr 签名有效。
///
/// 安全通道的诚实丢弃策略：校验失败静默丢弃（debug 日志），不投递进总线。
pub fn verify_event(event: &NostrEvent) -> bool {
    let expect_id = event_id(
        &event.pubkey,
        event.created_at,
        event.kind,
        &event.tags,
        &event.content,
    );
    if expect_id != event.id {
        return false;
    }
    let Ok(id_bytes) = hex::decode(&event.id) else {
        return false;
    };
    let Ok(id_arr) = <[u8; 32]>::try_from(id_bytes.as_slice()) else {
        return false;
    };
    let Ok(sig_bytes) = hex::decode(&event.sig) else {
        return false;
    };
    let Ok(sig) = Signature::try_from(sig_bytes.as_slice()) else {
        return false;
    };
    let Ok(peer_x) = parse_x_only_pubkey(&event.pubkey) else {
        return false;
    };
    let Ok(vk) = VerifyingKey::from_bytes(&peer_x) else {
        return false;
    };
    // verify_raw：raw BIP-340 校验（Verifier trait 会二次哈希，与 NIP-01 不符）。
    vk.verify_raw(&id_arr, &sig).is_ok()
}

/// 构造入站订阅帧：`["REQ", "nemesisbot", {"kinds":[4],"#p":[本机公钥],"since":ts}]`。
///
/// `#p` 过滤 p-tag 指向本机的 kind-4 加密 DM（NIP-01 tag 过滤语义）。
pub fn build_req_frame(our_pubkey: &str, since_unix: i64) -> String {
    serde_json::to_string(&serde_json::json!([
        "REQ",
        NOSTR_SUB_ID,
        {
            "kinds": [NOSTR_KIND_DM],
            "#p": [our_pubkey],
            "since": since_unix,
        }
    ]))
    .expect("REQ 帧序列化不可失败")
}

/// 构造发布帧：`["EVENT", event]`。
pub fn build_event_frame(event: &NostrEvent) -> String {
    serde_json::to_string(&serde_json::json!(["EVENT", event])).expect("EVENT 帧序列化不可失败")
}

/// relay 下行消息归类（v1 只消费 EVENT，其余日志级处理）。
enum RelayFrame {
    /// `["EVENT", sub_id, event]` 下发事件。
    Event(Box<NostrEvent>),
    /// 其余帧（EOSE/NOTICE/OK/CLOSE）——不需处理但需区分于解析失败。
    Ignored,
}

/// 解析 relay 下行 JSON 帧。
fn parse_relay_frame(text: &str) -> Option<RelayFrame> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let arr = value.as_array()?;
    match arr.first()?.as_str()? {
        "EVENT" => {
            // 兼容两种形态：["EVENT", sub_id, event]（订阅下发）与
            // ["EVENT", event]（部分 relay 对发布的回显）。
            let event_value = arr.get(2).or_else(|| arr.get(1))?;
            let event: NostrEvent = serde_json::from_value(event_value.clone()).ok()?;
            Some(RelayFrame::Event(Box::new(event)))
        }
        _ => Some(RelayFrame::Ignored),
    }
}

// ---------------------------------------------------------------------------
// 通道配置
// ---------------------------------------------------------------------------

/// nostr 通道配置。
#[derive(Debug, Clone)]
pub struct NostrConfig {
    /// relay WebSocket 地址列表（`wss://` / `ws://`），至少一条。
    pub relays: Vec<String>,
    /// 本机私钥（64-hex）。config.json 只存 `vault:`/`env:`/`yaml:` 引用，
    /// 装配点解析后传明文（见模块头"密钥管理"）。
    pub private_key: String,
    /// 发件人 x-only 公钥白名单（64-hex）。空 = 放行所有（与其他通道
    /// `allow_from` 语义一致；加密 DM 场景建议显式配置）。
    pub allow_from: Vec<String>,
    /// 断线重连间隔秒数（默认 5，0 视作缺省）。
    pub reconnect_secs: u64,
}

/// nostr 通道：relay WebSocket 订阅 + NIP-04 加密 DM。
pub struct NostrChannel {
    base: BaseChannel,
    config: NostrConfig,
    keys: NostrKeys,
    running: Arc<parking_lot::RwLock<bool>>,
    /// 已处理事件 id 去重（relay 重连重放防御）。
    seen_events: Arc<parking_lot::RwLock<HashMap<String, bool>>>,
    /// 各 relay 连接的出站帧通道（按连接 id 存取，断线摘除）。
    sinks: Arc<parking_lot::Mutex<HashMap<u64, mpsc::Sender<String>>>>,
    bus_sender: broadcast::Sender<InboundMessage>,
}

impl NostrChannel {
    /// 创建 nostr 通道。
    pub fn new(config: NostrConfig, bus_sender: broadcast::Sender<InboundMessage>) -> Result<Self> {
        let relays: Vec<String> = config
            .relays
            .iter()
            .map(|r| r.trim().trim_end_matches('/').to_string())
            .filter(|r| !r.is_empty())
            .collect();
        if relays.is_empty() {
            return Err(NemesisError::Channel(
                "nostr 通道需要至少一条 relay 地址（relays 非空）".to_string(),
            ));
        }
        for relay in &relays {
            if !relay.starts_with("ws://") && !relay.starts_with("wss://") {
                return Err(NemesisError::Channel(format!(
                    "nostr relay 地址须以 ws:// 或 wss:// 开头: {relay}"
                )));
            }
        }
        if config.private_key.is_empty() {
            return Err(NemesisError::Channel(
                "nostr 通道需要 private_key（64-hex 或装配点解析后的引用值）".to_string(),
            ));
        }
        let keys = NostrKeys::from_hex_secret(&config.private_key)?;
        let reconnect_secs = if config.reconnect_secs == 0 {
            5
        } else {
            config.reconnect_secs
        };

        Ok(Self {
            base: BaseChannel::with_allow_list("nostr", config.allow_from.clone()),
            config: NostrConfig {
                relays,
                private_key: config.private_key,
                allow_from: config.allow_from,
                reconnect_secs,
            },
            keys,
            running: Arc::new(parking_lot::RwLock::new(false)),
            seen_events: Arc::new(parking_lot::RwLock::new(HashMap::new())),
            sinks: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            bus_sender,
        })
    }

    /// 本机 x-only 公钥（入站订阅 filter 与白名单比较均用此形态）。
    pub fn public_key(&self) -> &str {
        self.keys.x_only_public_key()
    }

    /// 把出站帧广播到所有在连 relay。
    fn publish_frame(&self, frame: &str) -> Result<usize> {
        let sinks = self.sinks.lock();
        if sinks.is_empty() {
            return Err(NemesisError::Channel(
                "nostr 通道无在连 relay，无法发布事件".to_string(),
            ));
        }
        let mut delivered = 0;
        for sink in sinks.values() {
            // 满队列/已断连的单个 relay 不阻塞整体发布（fire-and-forget 语义）。
            if sink.try_send(frame.to_string()).is_ok() {
                delivered += 1;
            }
        }
        if delivered == 0 {
            return Err(NemesisError::Channel(
                "nostr 出站帧未能送达任何 relay（全部队列满或断连）".to_string(),
            ));
        }
        Ok(delivered)
    }

    /// 单条 relay 连接任务：连接 → REQ 订阅 → 双向泵 → 断线退避重连。
    async fn relay_task(
        relay_url: String,
        req_frame: String,
        running: Arc<parking_lot::RwLock<bool>>,
        sinks: Arc<parking_lot::Mutex<HashMap<u64, mpsc::Sender<String>>>>,
        next_conn_id: Arc<AtomicU64>,
        reconnect_secs: u64,
        event_tx: mpsc::Sender<NostrEvent>,
    ) {
        let mut backoff = std::time::Duration::from_secs(1);
        let max_backoff = std::time::Duration::from_secs(60);

        loop {
            if !*running.read() {
                break;
            }

            let connect = tokio_tungstenite::connect_async(&relay_url).await;
            let ws = match connect {
                Ok((ws, _)) => ws,
                Err(e) => {
                    warn!(relay = %relay_url, error = %e, "[NostrChannel] relay 连接失败");
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(max_backoff);
                    continue;
                }
            };
            backoff = std::time::Duration::from_secs(1);
            info!(relay = %relay_url, "[NostrChannel] relay 已连接");

            let (mut sink, mut stream) = ws.split();
            let (out_tx, mut out_rx) = mpsc::channel::<String>(64);
            let conn_id = next_conn_id.fetch_add(1, Ordering::Relaxed);
            sinks.lock().insert(conn_id, out_tx.clone());

            // 发送 REQ 订阅。
            if let Err(e) = sink
                .send(tokio_tungstenite::tungstenite::Message::Text(
                    req_frame.clone().into(),
                ))
                .await
            {
                warn!(relay = %relay_url, error = %e, "[NostrChannel] REQ 订阅发送失败");
                sinks.lock().remove(&conn_id);
                continue;
            }

            // 双向泵：出站队列 → WS sink；WS stream → 事件解析。
            use futures::{SinkExt, StreamExt};
            loop {
                tokio::select! {
                    out = out_rx.recv() => {
                        match out {
                            Some(frame) => {
                                if sink
                                    .send(tokio_tungstenite::tungstenite::Message::Text(
                                        frame.into(),
                                    ))
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                            }
                            None => break,
                        }
                    }
                    msg = stream.next() => {
                        match msg {
                            Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                                match parse_relay_frame(&text) {
                                    Some(RelayFrame::Event(event)) => {
                                        if event_tx.send(*event).await.is_err() {
                                            break;
                                        }
                                    }
                                    Some(RelayFrame::Ignored) => {}
                                    None => {
                                        debug!(relay = %relay_url, "[NostrChannel] 无法解析的 relay 帧")
                                    }
                                }
                            }
                            Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_))) => break,
                            Some(Ok(_)) => {}
                            Some(Err(e)) => {
                                debug!(relay = %relay_url, error = %e, "[NostrChannel] relay 读错误");
                                break;
                            }
                            None => break,
                        }
                    }
                    _ = async {
                        // running 翻转为 false 时主动收线。
                        while *running.read() {
                            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                        }
                    } => break,
                }
            }

            sinks.lock().remove(&conn_id);
            if !*running.read() {
                break;
            }
            warn!(relay = %relay_url, "[NostrChannel] relay 断开，{}s 后重连", reconnect_secs);
            tokio::time::sleep(std::time::Duration::from_secs(reconnect_secs)).await;
        }
        debug!(relay = %relay_url, "[NostrChannel] relay 任务退出");
    }
}

#[async_trait]
impl Channel for NostrChannel {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn is_running(&self) -> bool {
        self.base.is_running()
    }

    async fn start(&self) -> Result<()> {
        info!(
            "[NostrChannel] 启动 nostr 通道（本机公钥 {}）",
            self.keys.x_only_pub
        );
        *self.running.write() = true;
        self.base.set_enabled(true);
        self.base.set_running(true);

        let running = self.running.clone();
        let sinks = self.sinks.clone();
        let next_conn_id = Arc::new(AtomicU64::new(1));
        let reconnect_secs = self.config.reconnect_secs;
        let req_frame = build_req_frame(&self.keys.x_only_pub, chrono::Utc::now().timestamp());

        // relay 任务解析出的事件经队列交给处理者（与 WS 泵解耦）。
        let (event_tx, mut event_rx) = mpsc::channel::<NostrEvent>(256);
        let processor = self.clone_shared();
        tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                processor.process_relay_event(&event);
            }
        });

        for relay in self.config.relays.clone() {
            let running = running.clone();
            let sinks = sinks.clone();
            let next_conn_id = next_conn_id.clone();
            let req = req_frame.clone();
            let event_tx = event_tx.clone();
            tokio::spawn(Self::relay_task(
                relay,
                req,
                running,
                sinks,
                next_conn_id,
                reconnect_secs,
                event_tx,
            ));
        }
        drop(event_tx); // 全部 relay 任务结束后处理循环随之退出

        info!(
            "[NostrChannel] 通道已启动（{} 条 relay）",
            self.config.relays.len()
        );
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("[NostrChannel] 停止 nostr 通道");
        *self.running.write() = false;
        self.base.set_enabled(false);
        self.base.set_running(false);
        // 清空出站 sinks（relay 任务退出时会自行摘除，这里兜底防悬挂引用）。
        self.sinks.lock().clear();
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        if !*self.running.read() {
            return Err(NemesisError::Channel("nostr 通道未运行".to_string()));
        }
        if msg.chat_id.trim().is_empty() {
            return Err(NemesisError::Channel(
                "nostr 出站需要 chat_id = 收件人 x-only 公钥（64-hex）".to_string(),
            ));
        }

        // NIP-04 加密 → NIP-01 签署 → EVENT 发布到全部在连 relay。
        let event = build_dm_event(
            &self.keys,
            msg.chat_id.trim(),
            &msg.content,
            chrono::Utc::now().timestamp(),
        )?;
        let frame = build_event_frame(&event);
        self.publish_frame(&frame)?;
        self.base.record_sent();
        debug!(recipient = %msg.chat_id, event_id = %event.id, "[NostrChannel] DM 已发布");
        Ok(())
    }
}

/// 事件处理器：把 `process_relay_event` 的逻辑从 Channel 对象中拆出，
/// 让处理任务与 relay 连接任务解耦（不持有 &self 生命周期）。
/// 校验 → 白名单 → 去重 → 解密 → 进总线；任何一步失败都诚实丢弃。
struct NostrEventProcessor {
    keys: NostrKeys,
    x_only_pub: String,
    base: BaseChannel,
    seen_events: Arc<parking_lot::RwLock<HashMap<String, bool>>>,
    bus_sender: broadcast::Sender<InboundMessage>,
}

impl NostrEventProcessor {
    fn process_relay_event(&self, event: &NostrEvent) {
        if event.kind != NOSTR_KIND_DM {
            return;
        }

        // 签名/id 完整性校验——加密 DM 通道不处理未签名事件。
        if !verify_event(event) {
            debug!(event_id = %event.id, "[NostrChannel] 事件签名/id 校验失败，丢弃");
            return;
        }

        // 自环（自己发出的事件经 relay 重放回来）跳过。
        if event.pubkey == self.x_only_pub {
            return;
        }

        // p-tag 必须指向本机（双保险：relay filter 已过滤，客户端不信任 relay）。
        let p_tag_ok = event.tags.iter().any(|tag| {
            tag.first().map(String::as_str) == Some("p")
                && tag.get(1).map(String::as_str) == Some(self.x_only_pub.as_str())
        });
        if !p_tag_ok {
            debug!(event_id = %event.id, "[NostrChannel] 事件 p-tag 不指向本机，丢弃");
            return;
        }

        // 发件人白名单（BaseChannel 语义：空 = 放行所有）。
        if !self.base.is_allowed(&event.pubkey) {
            debug!(sender = %event.pubkey, "[NostrChannel] 发件人不在白名单，丢弃");
            return;
        }

        // 去重（relay 重连会重放窗口内事件）。
        {
            let mut seen = self.seen_events.write();
            if seen.contains_key(&event.id) {
                return;
            }
            seen.insert(event.id.clone(), true);
            if seen.len() > 10000 {
                let keys: Vec<String> = seen.keys().take(5000).cloned().collect();
                for key in keys {
                    seen.remove(&key);
                }
            }
        }

        let content = match decrypt_dm(&self.keys, &event.pubkey, &event.content) {
            Ok(text) => text,
            Err(e) => {
                debug!(event_id = %event.id, error = %e, "[NostrChannel] DM 解密失败，丢弃");
                return;
            }
        };
        if content.is_empty() {
            return;
        }

        // 回复寻址：chat_id = 发件人公钥（出站时按它加密回去）。
        let chat_id = event.pubkey.clone();
        info!(
            sender = %event.pubkey,
            "[NostrChannel] 收到加密 DM（{} 字节）",
            content.len()
        );
        let inbound = InboundMessage {
            channel: "nostr".to_string(),
            sender_id: event.pubkey.clone(),
            chat_id: chat_id.clone(),
            content,
            media: Vec::new(),
            session_key: chat_id,
            correlation_id: String::new(),
            metadata: HashMap::new(),
            voice_playback: None,
        };
        self.base.record_received();
        let _ = self.bus_sender.send(inbound);
    }
}

/// 内部共享件克隆（事件处理者需要跨任务持有处理状态）。
impl NostrChannel {
    fn clone_shared(&self) -> NostrEventProcessor {
        NostrEventProcessor {
            keys: self.keys.clone(),
            x_only_pub: self.keys.x_only_pub.clone(),
            base: BaseChannel::with_allow_list("nostr", self.config.allow_from.clone()),
            seen_events: self.seen_events.clone(),
            bus_sender: self.bus_sender.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
