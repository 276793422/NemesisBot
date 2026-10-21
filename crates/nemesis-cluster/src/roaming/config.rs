//! roaming 段配置解析（config.cluster.json 顶层 `roaming` 对象）。
//!
//! 单一真相源：[`resolved_roaming`] 纯函数——缺文件 / 坏 JSON / 缺段 /
//! 字段类型不符一律逐字段落默认值（照 `Cluster::roaming_enabled_in_config`
//! 的严格语义惯例：`enabled` 必须是严格 JSON bool，缺省恒 false）。
//! 旧配置（2026-10-18 形态，只有 `enabled` 键）兼容：缺新键 = 默认值，
//! `enabled` 语义字节不变。
//!
//! 键语义（完整表见 docs/PLAN/2026-10-21_quic-relay-server.md §3.2）：
//! - `roaming.enabled`：叠加层总开关；
//! - `roaming.relay_url`：**客户端**自建中继地址；空 = n0 公共中继（现行为）；
//! - `roaming.relay_token`：**客户端**中继访问令牌（服务端 SharedToken 配对；
//!   仅 relay_url 非空时有意义——n0 公共中继不吃本 token）；
//! - `roaming.relay.*`：**服务端**自建中继设置（enabled/端口/tls 档/token）。

use std::path::Path;

/// HTTP relay transport 默认端口（非特权——VPS/NAS Linux 无需 root）。
pub const DEFAULT_RELAY_HTTP_PORT: u16 = 3340;
/// QUIC relay transport 默认端口（iroh-relay 上游默认）。
pub const DEFAULT_RELAY_QUIC_PORT: u16 = 7842;

/// relay transport TLS 档（`roaming.relay.tls`）。
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

/// 自建中继服务端设置（`roaming.relay` 段）。
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
            cert_path: String::new(),
            key_path: String::new(),
        }
    }
}

/// roaming 叠加层配置（config.cluster.json 顶层 `roaming` 段解析产物）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RoamingOverlayConfig {
    /// 叠加层总开关（严格 JSON bool；与 `roaming_enabled_in_config` 同源）。
    pub enabled: bool,
    /// 客户端自建中继地址（如 `http://vps.example.com:3340`）；空 = n0 公共中继。
    pub relay_url: String,
    /// 客户端中继访问令牌（与自建中继服务端 `relay.token` 配对；relay_url
    /// 空（n0 公共中继）时无意义）。
    pub relay_token: String,
    /// 服务端自建中继设置。
    pub relay: RelaySettings,
}

/// 解析 config.cluster.json 顶层 `roaming` 段（纯函数，零副作用）。
///
/// 任何解析失败（缺文件 / 坏 JSON / 缺段 / 字段类型不符）逐字段落默认。
pub fn resolved_roaming(config_path: &Path) -> RoamingOverlayConfig {
    let raw = std::fs::read_to_string(config_path).ok();
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
    out.relay_url = str_field(roaming.get("relay_url"));
    out.relay_token = str_field(roaming.get("relay_token"));

    let Some(relay) = roaming.get("relay") else {
        return out;
    };
    out.relay.enabled = strict_bool(relay.get("enabled"));
    out.relay.http_port = port_field(relay.get("http_port"), DEFAULT_RELAY_HTTP_PORT);
    out.relay.quic_port = port_field(relay.get("quic_port"), DEFAULT_RELAY_QUIC_PORT);
    out.relay.external_host = str_field(relay.get("external_host"));
    out.relay.tls = relay
        .get("tls")
        .and_then(|v| v.as_str())
        .map(RelayTls::from_config_str)
        .unwrap_or_default();
    out.relay.token = str_field(relay.get("token"));
    out.relay.cert_path = str_field(relay.get("cert_path"));
    out.relay.key_path = str_field(relay.get("key_path"));
    out
}

/// 严格 JSON bool（非 bool 类型 → false，照 `roaming_enabled_in_config` 语义）。
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

#[cfg(test)]
mod tests;
