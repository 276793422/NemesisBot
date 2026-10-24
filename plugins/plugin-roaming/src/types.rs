//! 插件内数据类型副本（搬自宿主 nemesis-cluster::roaming::config / rendezvous）。
//!
//! 纪律：宿主与插件经 JSON 解耦，但**字段集合与解析规则必须同源**——本文件
//! 是宿主 `config.rs`（RelaySettings/RelayTls/parse_relay_settings）与
//! `rendezvous.rs`（RendezvousMember/常量）的镜像副本。宿主侧改动 = 同步本
//! 文件（漂移由 e2e wire 形态测试兜底）。
//!
//! 宿主 init 传入的 config JSON 形态：`{"relay": {...RelaySettings serde
//! 规范化形态...}}`（宿主负责从 config.quic.json 解析出 relay 段再序列化——
//! 解析单一真相源在宿主，插件只认规范化形态）。

use serde::{Deserialize, Serialize};

/// HTTP relay transport 默认端口（非特权——VPS/NAS Linux 无需 root）。
pub const DEFAULT_RELAY_HTTP_PORT: u16 = 3340;
/// QUIC relay transport 默认端口（iroh-relay 上游默认）。
pub const DEFAULT_RELAY_QUIC_PORT: u16 = 7842;
/// rendezvous 成员注册表默认端口（= http_port + 1 惯例）。
pub const DEFAULT_REGISTRY_PORT: u16 = 3341;

/// 经名册入册的成员携带的哨兵 tag（与宿主 rendezvous.rs 同值；M2 名册
/// 循环接线后消费）。
#[allow(dead_code)]
pub const VIA_RELAY_TAG: &str = "via-relay";

/// 成员 TTL：超过此时长未报到即从名册剔除。
pub const MEMBER_TTL_SECS: u64 = 120;

/// 名册容量闸（超出拒绝注册——中继是公网面，无界存储不设防）。
pub const MAX_MEMBERS: usize = 256;

/// relay transport TLS 档（镜像宿主 config.rs `RelayTls`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RelayTls {
    /// 首启自签证书，落 `<workspace>/cluster/relay/`，存在即复用（默认档）。
    #[default]
    SelfSigned,
    /// 用户自备证书（`cert_path` + `key_path` pem）。
    Manual,
    /// 无 TLS：纯 HTTP relay transport（QUIC relay transport 关——受限兜底档）。
    None,
}

impl RelayTls {
    /// 配置串 → 档位（大小写不敏感；未知值落默认 SelfSigned——宿主解析层
    /// 同款宽容语义）。
    fn from_config_str(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "manual" => Self::Manual,
            "none" => Self::None,
            _ => Self::SelfSigned,
        }
    }
}

/// 自建中继服务端设置（镜像宿主 config.rs `RelaySettings`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelaySettings {
    /// 随 gateway 进程内启动自建中继（宿主侧闸；插件 start 不看此键——
    /// 独立命令形态的显式意志由宿主裁决后调 start）。
    pub enabled: bool,
    /// HTTP relay transport 监听端口（默认 3340）。
    pub http_port: u16,
    /// QUIC relay transport 监听端口（默认 7842）。
    pub quic_port: u16,
    /// 对端可达主机名/IP（生成对外 relay URL；空 = 自动探测本机主 IP）。
    pub external_host: String,
    /// TLS 档（默认 SelfSigned）。
    pub tls: RelayTls,
    /// 访问令牌（非空 = SharedToken 访问控制；空 = AllowAll——但 start
    /// 对空 token fail-closed 拒绝，与宿主三路收口纪律同源）。
    pub token: String,
    /// rendezvous 成员注册表监听端口（默认 3341；0=关——纯传输形态）。
    pub registry_port: u16,
    /// Manual 档证书路径（用户自备 pem）。
    pub cert_path: String,
    /// Manual 档私钥路径。
    pub key_path: String,
}

impl Default for RelaySettings {
    /// 与宿主 config.rs 手工 Default 同值（3340/7842/3341 + SelfSigned）。
    fn default() -> Self {
        Self {
            enabled: false,
            http_port: DEFAULT_RELAY_HTTP_PORT,
            quic_port: DEFAULT_RELAY_QUIC_PORT,
            registry_port: DEFAULT_REGISTRY_PORT,
            external_host: String::new(),
            tls: RelayTls::SelfSigned,
            token: String::new(),
            cert_path: String::new(),
            key_path: String::new(),
        }
    }
}

/// 从 init 传入 config JSON 的 `relay` 段解析 [`RelaySettings`]
/// （镜像宿主 `parse_relay_settings`——逐字段宽容降级，规则同源）。
pub fn parse_relay_settings(relay: &serde_json::Value) -> RelaySettings {
    RelaySettings {
        enabled: strict_bool(relay.get("enabled")),
        http_port: port_field(relay.get("http_port"), DEFAULT_RELAY_HTTP_PORT),
        quic_port: port_field(relay.get("quic_port"), DEFAULT_RELAY_QUIC_PORT),
        registry_port: port_field(relay.get("registry_port"), DEFAULT_REGISTRY_PORT),
        external_host: str_field(relay.get("external_host")),
        tls: relay
            .get("tls")
            .and_then(|v| v.as_str())
            .map(RelayTls::from_config_str)
            .unwrap_or_default(),
        token: str_field(relay.get("token")),
        cert_path: str_field(relay.get("cert_path")),
        key_path: str_field(relay.get("key_path")),
    }
}

/// 严格 JSON bool（非 bool 类型 → false）。
fn strict_bool(v: Option<&serde_json::Value>) -> bool {
    v.and_then(|v| v.as_bool()).unwrap_or(false)
}

/// 字符串字段（非字符串类型 → 空；首尾空白剥离）。
fn str_field(v: Option<&serde_json::Value>) -> String {
    v.and_then(|v| v.as_str())
        .map(str::trim)
        .unwrap_or("")
        .to_string()
}

/// 端口字段（非 u64 / 超 u16 值域 → 默认端口）。
fn port_field(v: Option<&serde_json::Value>, default: u16) -> u16 {
    v.and_then(|v| v.as_u64())
        .and_then(|p| u16::try_from(p).ok())
        .unwrap_or(default)
}

/// 名册成员（镜像宿主 rendezvous.rs `RendezvousMember`——wire 同构，serde
/// default 兼容缺字段）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RendezvousMember {
    /// 集群节点 id（对端 peers.toml 段键语义）。
    pub node_id: String,
    /// 节点显示名。
    #[serde(default)]
    pub name: String,
    /// 角色（worker/coordinator/...）。
    #[serde(default)]
    pub role: String,
    /// 分类（development/...）。
    #[serde(default)]
    pub category: String,
    /// 节点类型（Cluster::node_type，默认 "agent"）。
    #[serde(default)]
    pub node_type: String,
    /// 集群 RPC 端口（入册闸要求 >0）。
    #[serde(default)]
    pub rpc_port: u16,
    /// 标签（派发匹配用）。
    #[serde(default)]
    pub tags: Vec<String>,
    /// 专业职能（slug 形态 `family[:spec]`）。
    #[serde(default)]
    pub professions: Vec<String>,
    /// 能力档位（mini/normal/big）。
    #[serde(default)]
    pub tier: Option<String>,
    /// iroh 漫游身份（64 位小写 hex；注册必填且校验）。
    pub iroh_id: String,
    /// 构建号（git 短 hash）。
    #[serde(default)]
    pub build: String,
    /// 该成员的对外 relay_url（对端拼 `iroh:<hex>@<relay_url>` 用）。
    #[serde(default)]
    pub relay_url: String,
    /// 功能宣告（严格不兼容语义；服务端登记容忍缺字段）。
    #[serde(default)]
    pub features: Vec<String>,
    /// 服务器侧最近报到时刻（epoch ms）。
    #[serde(default)]
    pub last_seen_ms: u64,
}

/// iroh 身份校验（镜像宿主 `endpoint::node_id_from_hex` 全语义：hex 解码 →
/// 32 字节 → ed25519 公钥点校验——非法曲线点身份对谁都没用，与宿主同强）。
/// 错误文案与宿主同源（外层 sanitize 统一包 `iroh_id 非法: ` 前缀）。
pub fn valid_iroh_id(s: &str) -> Result<(), String> {
    let bytes = hex::decode(s.trim())
        .map_err(|e| format!("node_id hex 非法（应为 64 位十六进制）: {e}"))?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|v: Vec<u8>| format!("node_id 长度 {} 字节 ≠ 32", v.len()))?;
    ed25519_dalek::VerifyingKey::from_bytes(&arr)
        .map(|_| ())
        .map_err(|e| format!("node_id 非法: {e}"))
}

/// 成员拨号地址（`iroh:<hex>@<relay>`；成员 relay_url 空回落 fallback——
/// 宿主 rendezvous.rs 同名函数镜像，M3 客户端面收编后为插件内唯一形态）。
/// M2 阶段仅 tests 消费（cdylib 不导出纯 Rust fn）。
#[allow(dead_code)]
pub fn member_dial_address(m: &RendezvousMember, fallback_relay_url: &str) -> String {
    let relay = if m.relay_url.is_empty() {
        fallback_relay_url
    } else {
        &m.relay_url
    };
    format!("iroh:{}@{}", m.iroh_id, relay)
}
