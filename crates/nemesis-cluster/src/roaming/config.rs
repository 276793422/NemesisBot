//! roaming 段配置解析（config.cluster.json 顶层 `roaming` 对象）。
//!
//! 单一真相源：[`resolved_roaming`] 纯函数——缺文件 / 坏 JSON / 缺段 /
//! 字段类型不符一律落默认值（照 `Cluster::roaming_enabled_in_config`
//! 的严格语义惯例：`enabled` 必须是严格 JSON bool，缺省恒 false）。
//!
//! 2026-10-23 与 QUIC 解耦：段内**只剩 `enabled` 一个键**（叠加层总开关）。
//! 客户端三键（relay_url/relay_token/rendezvous_url）与服务端 `relay` 段
//! 全部迁 `config.quic.json`（[`super::quic_config`]）——QUIC 的配置不丢在
//! 集群里，集群只是 QUIC 的一个消费方。旧键残留在 cluster.json 中被本解析
//! 忽略（首读迁移只拷贝不删除，无害遗留）。

use std::path::Path;

/// HTTP relay transport 默认端口（非特权——VPS/NAS Linux 无需 root）。
pub const DEFAULT_RELAY_HTTP_PORT: u16 = 3340;
/// QUIC relay transport 默认端口（iroh-relay 上游默认）。
pub const DEFAULT_RELAY_QUIC_PORT: u16 = 7842;
/// rendezvous 成员注册表默认端口（= http_port + 1 惯例；iroh relay 的 HTTP
/// server 固定路由挂不了自定义路由，注册表独立监听）。
pub const DEFAULT_REGISTRY_PORT: u16 = 3341;

/// relay transport TLS 档（config.quic.json `relay.tls`）。
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
    /// 配置串 → 档位（大小写不敏感；未知值落默认 SelfSigned——解析层
    /// 惯例是逐字段宽容降级，解析产物经状态面可见，不做静默猜测）。
    fn from_config_str(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "manual" => Self::Manual,
            "none" => Self::None,
            _ => Self::SelfSigned,
        }
    }
}

/// 自建中继服务端设置（config.quic.json `relay` 段）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelaySettings {
    /// 随 gateway 进程内启动自建中继（独立命令 `quic relay` 不受此闸约束）。
    pub enabled: bool,
    /// HTTP relay transport 监听端口（默认 3340）。
    pub http_port: u16,
    /// QUIC relay transport 监听端口（默认 7842）。
    pub quic_port: u16,
    /// 对端可达主机名/IP（生成对外 relay URL；空 = 自动探测本机主 IP）。
    pub external_host: String,
    /// TLS 档（默认 SelfSigned）。
    pub tls: RelayTls,
    /// 访问令牌（非空 = SharedToken 访问控制；空 = AllowAll）。
    pub token: String,
    /// rendezvous 成员注册表监听端口（默认 3341；0=关——纯传输形态）。
    /// 注册表随中继进程拉起（token 非空才生效——无门禁的名册不设防）。
    pub registry_port: u16,
    /// Manual 档证书路径（用户自备 pem）。
    pub cert_path: String,
    /// Manual 档私钥路径。
    pub key_path: String,
}

impl Default for RelaySettings {
    fn default() -> Self {
        Self {
            enabled: false,
            http_port: DEFAULT_RELAY_HTTP_PORT,
            quic_port: DEFAULT_RELAY_QUIC_PORT,
            external_host: String::new(),
            tls: RelayTls::SelfSigned,
            token: String::new(),
            registry_port: DEFAULT_REGISTRY_PORT,
            cert_path: String::new(),
            key_path: String::new(),
        }
    }
}

/// roaming 叠加层开关（config.cluster.json 顶层 `roaming` 段解析产物）。
///
/// 2026-10-23 起只剩 enabled——QUIC 客户端/服务端配置全部迁
/// `config.quic.json`（[`super::quic_config`]）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RoamingOverlayConfig {
    /// 叠加层总开关（严格 JSON bool；与 `roaming_enabled_in_config` 同源）。
    pub enabled: bool,
}

/// 解析 config.cluster.json 顶层 `roaming` 段（纯函数，零副作用）。
///
/// 任何解析失败（缺文件 / 坏 JSON / 缺段 / 字段类型不符）落默认。
pub fn resolved_roaming(config_path: &Path) -> RoamingOverlayConfig {
    let raw = nemesis_utils::fsutil::read_config_string(config_path).ok();
    let cfg = raw
        .as_deref()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());
    parse_roaming_value(cfg.as_ref())
}

/// 从（可选的）配置 JSON 树解析 roaming 段——测试友好拆分。
pub(crate) fn parse_roaming_value(cfg: Option<&serde_json::Value>) -> RoamingOverlayConfig {
    let mut out = RoamingOverlayConfig::default();
    let Some(roaming) = cfg.and_then(|v| v.get("roaming")) else {
        return out;
    };
    out.enabled = strict_bool(roaming.get("enabled"));
    out
}

/// 从 relay 段 JSON 解析 [`RelaySettings`]——config.rs 与 quic_config.rs 共用
///（旧 `roaming.relay` 与新 `config.quic.json` 的 `relay` 段字段一一对应，
/// 解析规则同源单一真相源）。
pub(crate) fn parse_relay_settings(relay: &serde_json::Value) -> RelaySettings {
    let mut out = RelaySettings::default();
    out.enabled = strict_bool(relay.get("enabled"));
    out.http_port = port_field(relay.get("http_port"), DEFAULT_RELAY_HTTP_PORT);
    out.quic_port = port_field(relay.get("quic_port"), DEFAULT_RELAY_QUIC_PORT);
    out.registry_port = port_field(relay.get("registry_port"), DEFAULT_REGISTRY_PORT);
    out.external_host = str_field(relay.get("external_host"));
    out.tls = relay
        .get("tls")
        .and_then(|v| v.as_str())
        .map(RelayTls::from_config_str)
        .unwrap_or_default();
    out.token = str_field(relay.get("token"));
    out.cert_path = str_field(relay.get("cert_path"));
    out.key_path = str_field(relay.get("key_path"));
    out
}

/// 严格 JSON bool（非 bool 类型 → false，照 `roaming_enabled_in_config` 语义）。
fn strict_bool(v: Option<&serde_json::Value>) -> bool {
    v.and_then(|v| v.as_bool()).unwrap_or(false)
}

/// 字符串字段（非字符串类型 → 空；首尾空白剥离）。
pub(crate) fn str_field(v: Option<&serde_json::Value>) -> String {
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

#[cfg(test)]
mod tests;
