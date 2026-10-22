//! QUIC 中继服务器（feature=quic-relay）——自建 iroh 中继回落（内网穿透）。
//!
//! 装配形态对齐官方 iroh-relay `--dev` 模式：HTTP relay transport 明文
//! （`http_port`，客户端零证书负担——relay_url 用 `http://` 即接入）+
//! QAD（QUIC Address Discovery，`quic_port`，TLS 1.3；iroh 1.0.2 的 QUIC
//! 臂**只有地址发现**，relay 流量本身恒走 HTTP/WS transport，无 QUIC relay
//! transport——`quic_port` 属 v2 预留面）。TLS 三档：
//! - [`RelayTls::SelfSigned`]（默认）：rcgen 自签，PEM 落
//!   `<workspace>/cluster/relay/`，存在即复用（幂等）；
//! - [`RelayTls::Manual`]：用户自备 PEM（cert_path + key_path，仅 PKCS#8 私钥）；
//! - [`RelayTls::None`]：QAD 关，纯 HTTP relay transport（受限兜底档）。
//!
//! 访问控制：token 非空 = SharedToken（`Authorization: Bearer` / `?token=`，
//! 官方 binary 同款形态）；空 = `AllowAll`（解析层默认）——但三个启动路径
//! （`quic relay` CLI / `gateway --quic-relay` / 随网关 in-process）都在
//! spawn 前对空 token fail-closed 拒绝，无鉴权中继不允许裸奔。
//!
//! rendezvous 成员注册表（2026-10-22）：token 非空且 `registry_port>0` 时随
//! 中继并行拉起（[`crate::roaming::rendezvous_server`]，默认 3341）——设备
//! 接入即报到、周期拉名册，零手工配置互见；拉起失败 = WARN + 中继照常。
//!
//! Spike 结论（2026-10-21，iroh 1.0.2 源码核对）：
//! - 客户端信任面存在——`Endpoint::builder().ca_tls_config(CaTlsConfig::
//!   custom_roots([cert]))` 可把自签证书 pin 为唯一信任根；
//!   `insecure_skip_verify` 有 `cfg(test/test-utils)` 门控，生产不可用（好事）。
//! - v1 客户端走 HTTP transport（零证书负担）；QUIC transport 的证书分发
//!   （peer 怎么拿到 relay_cert.pem）留 v2 管理面。

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use iroh_relay::server::{
    Access, AccessControl, AllowAll, ClientRequest, DynAccessControl, QuicConfig,
    RelayConfig as ServerRelayConfig, Server, ServerConfig,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

use crate::roaming::config::{RelaySettings, RelayTls};

/// 中继证书目录（`<workspace>/cluster/relay/`）。
pub fn relay_dir(workspace: &Path) -> PathBuf {
    workspace.join("cluster").join("relay")
}

/// 自签证书 PEM 路径（`relay_cert.pem`）。
pub fn relay_cert_path(workspace: &Path) -> PathBuf {
    relay_dir(workspace).join("relay_cert.pem")
}

/// 自签私钥 PEM 路径（`relay_key.pem`；Unix 0600）。
pub fn relay_key_path(workspace: &Path) -> PathBuf {
    relay_dir(workspace).join("relay_key.pem")
}

/// 对外交付的 relay URL（客户端填进自己 `roaming.relay_url` 的形态）。
///
/// `external_host` 空 = 自动探测本机主 IP（`Cluster::start` 同款枚举；
/// 全回环环境兜底 127.0.0.1——仅 loopback 测试可达，HTTP transport 形态）。
pub fn advertised_relay_url(settings: &RelaySettings) -> String {
    let host = if settings.external_host.trim().is_empty() {
        let ip = crate::network::get_local_ip();
        if ip.is_loopback() || ip.is_unspecified() {
            Ipv4Addr::LOCALHOST.to_string()
        } else {
            ip.to_string()
        }
    } else {
        settings.external_host.trim().to_string()
    };
    format!("http://{host}:{}", settings.http_port)
}

/// 运行中的自建中继句柄（shutdown 幂等——`server` 槽取空即无操作）。
pub struct RelayServerHandle {
    server: Option<Server>,
    /// HTTP relay transport 实际监听地址（端口 0 = OS 分配时尤其有用）。
    pub http_addr: Option<SocketAddr>,
    /// QUIC relay transport 实际监听地址（tls=none 时为 None）。
    pub quic_addr: Option<SocketAddr>,
    /// rendezvous 成员注册表（token 非空且 registry_port>0 时随中继拉起；
    /// 拉起失败 = None + WARN——名册是附加设施，不拖垮传输面）。
    pub registry: Option<crate::roaming::rendezvous_server::RegistryServerHandle>,
}

impl RelayServerHandle {
    /// 是否仍在运行（shutdown 后恒 false）。
    pub fn running(&self) -> bool {
        self.server.is_some()
    }

    /// 优雅停机（幂等；停机错误 best-effort 记日志不炸调用方）。
    /// 顺序：先注册表后中继（名册不再收新报到，传输面最后关）。
    pub async fn shutdown(&mut self) {
        if let Some(mut registry) = self.registry.take() {
            registry.shutdown().await;
        }
        if let Some(server) = self.server.take()
            && let Err(e) = server.shutdown().await
        {
            tracing::warn!(error = %e, "iroh 中继服务器 shutdown 失败（best-effort 忽略）");
        }
    }
}

/// 启动自建中继（按 settings 装配 TLS 三档 + 访问控制）。
///
/// 绑定 `0.0.0.0:<port>`；端口 0 = OS 分配（测试用）。启动失败 loud 报错
/// （端口占用 / 证书非法 / QUIC spawn 失败等——调用方决定告警形态）。
pub async fn spawn_relay_server(
    settings: &RelaySettings,
    workspace: &Path,
) -> Result<RelayServerHandle, String> {
    let quic_config = match settings.tls {
        RelayTls::None => None,
        RelayTls::SelfSigned => {
            let (cert_pem, key_pem) = ensure_self_signed_cert(workspace, &settings.external_host)?;
            Some(quic_tls_arm(&cert_pem, &key_pem, settings.quic_port)?)
        }
        RelayTls::Manual => {
            if settings.cert_path.trim().is_empty() || settings.key_path.trim().is_empty() {
                return Err("tls=manual 需要 relay.cert_path + relay.key_path".to_string());
            }
            let cert_pem = std::fs::read_to_string(settings.cert_path.trim())
                .map_err(|e| format!("读取证书 {} 失败: {e}", settings.cert_path.trim()))?;
            let key_pem = std::fs::read_to_string(settings.key_path.trim())
                .map_err(|e| format!("读取私钥 {} 失败: {e}", settings.key_path.trim()))?;
            Some(quic_tls_arm(&cert_pem, &key_pem, settings.quic_port)?)
        }
    };

    // HTTP relay transport 明文常开（客户端 relay_url `http://host:http_port`
    // 即接入，零证书负担）；TLS 只喂 QUIC 臂——官方 --dev 模式同形。
    let mut relay_config =
        ServerRelayConfig::new(SocketAddr::from(([0, 0, 0, 0], settings.http_port)));
    relay_config.access = access_control(&settings.token);

    let mut server_config = ServerConfig::default();
    server_config.relay = Some(relay_config);
    server_config.quic = quic_config;

    let server = Server::spawn(server_config)
        .await
        .map_err(|e| format!("iroh 中继服务器启动失败: {e}"))?;

    // rendezvous 成员注册表（自动发现面）：token 非空（有门禁）且
    // registry_port>0（未显式关）才拉起；失败 = warn + 中继照常（名册是
    // 附加设施，不拖垮传输面）。
    let registry = if settings.registry_port > 0 && !settings.token.trim().is_empty() {
        match crate::roaming::rendezvous_server::spawn_registry_server(
            settings.registry_port,
            settings.token.trim(),
        )
        .await
        {
            Ok(handle) => Some(handle),
            Err(e) => {
                tracing::warn!(error = %e, "[Rendezvous] 成员注册表拉起失败（中继照常运行，自动发现不可用）");
                None
            }
        }
    } else {
        None
    };

    Ok(RelayServerHandle {
        http_addr: server.http_addr(),
        quic_addr: server.quic_addr(),
        server: Some(server),
        registry,
    })
}

/// QUIC 臂 TLS 装配（证书 PEM → rustls ServerConfig → QuicConfig）。
fn quic_tls_arm(cert_pem: &str, key_pem: &str, quic_port: u16) -> Result<QuicConfig, String> {
    let mut config = QuicConfig::new(SocketAddr::from(([0, 0, 0, 0], quic_port)));
    config.server_config = Some(build_rustls_server_config(
        parse_certs_pem(cert_pem)?,
        parse_key_pem(key_pem)?,
    )?);
    Ok(config)
}

/// 访问控制：token 非空 = SharedToken；空 = AllowAll。
fn access_control(token: &str) -> Arc<dyn DynAccessControl> {
    if token.trim().is_empty() {
        Arc::new(AllowAll)
    } else {
        Arc::new(SharedTokenAccess(token.trim().to_string()))
    }
}

/// SharedToken 访问控制（官方 binary 同款形态）：`Authorization: Bearer`
/// 头或 `?token=` 查询参数携带；不匹配一票拒绝。
#[derive(Debug)]
struct SharedTokenAccess(String);

impl AccessControl for SharedTokenAccess {
    async fn on_connect(&self, request: &ClientRequest) -> Access {
        match request.auth_token() {
            Some(token) if token == self.0 => Access::Allow,
            _ => Access::Deny { reason: None },
        }
    }
}

/// 生成或复用自签证书（幂等：已有非空 PEM 即原样复用——二次 spawn 同证书）。
///
/// CN=nb-relay；SAN = localhost（DNS）+ 127.0.0.1（IP）+ external_host
/// （设了才加，IP 形态按 IP SAN、域名按 DNS SAN）。有效期 rcgen 默认
/// （1975-4096，自签本地设施不设过期管理负担）。
fn ensure_self_signed_cert(
    workspace: &Path,
    external_host: &str,
) -> Result<(String, String), String> {
    let cert_path = relay_cert_path(workspace);
    let key_path = relay_key_path(workspace);
    if let (Ok(cert), Ok(key)) = (
        std::fs::read_to_string(&cert_path),
        std::fs::read_to_string(&key_path),
    ) && !cert.trim().is_empty()
        && !key.trim().is_empty()
    {
        return Ok((cert, key));
    }

    let host = external_host.trim();
    let mut sans: Vec<rcgen::SanType> = vec![
        rcgen::SanType::DnsName(
            "localhost"
                .try_into()
                .map_err(|_| "localhost 不是合法 Ia5String".to_string())?,
        ),
        rcgen::SanType::IpAddress(IpAddr::V4(Ipv4Addr::LOCALHOST)),
    ];
    if !host.is_empty() {
        if let Ok(ip) = host.parse::<IpAddr>() {
            sans.push(rcgen::SanType::IpAddress(ip));
        } else {
            sans.push(rcgen::SanType::DnsName(
                host.try_into()
                    .map_err(|_| format!("external_host {host:?} 不是合法 DNS 名"))?,
            ));
        }
    }

    let mut params = rcgen::CertificateParams::default();
    params.subject_alt_names = sans;
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "nb-relay");

    let key = rcgen::KeyPair::generate().map_err(|e| format!("生成中继密钥失败: {e}"))?;
    let cert = params
        .self_signed(&key)
        .map_err(|e| format!("生成自签证书失败: {e}"))?;
    let cert_pem = cert.pem();
    let key_pem = key.serialize_pem();

    let dir = relay_dir(workspace);
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建 {} 失败: {e}", dir.display()))?;
    std::fs::write(&cert_path, &cert_pem)
        .map_err(|e| format!("写 {} 失败: {e}", cert_path.display()))?;
    std::fs::write(&key_path, &key_pem)
        .map_err(|e| format!("写 {} 失败: {e}", key_path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("收紧 {} 权限失败: {e}", key_path.display()))?;
    }
    tracing::info!(
        cert = %cert_path.display(),
        key = %key_path.display(),
        "[Cluster] 自建中继自签证书已生成（复用即免重生）"
    );
    Ok((cert_pem, key_pem))
}

/// 组装 rustls ServerConfig（ring provider 显式注入——不装进程级默认
/// provider，避免与 reqwest 等其他 rustls 消费方的全局状态互踩）。
fn build_rustls_server_config(
    certs: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
) -> Result<rustls::ServerConfig, String> {
    rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("TLS 协议版本装配失败: {e}"))?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("TLS 证书装配失败: {e}"))
}

/// 提取全部 PEM 段（指定 label）并 base64 解码为 DER 列表。
fn pem_to_ders(pem: &str, label: &str) -> Result<Vec<Vec<u8>>, String> {
    let begin = format!("-----BEGIN {label}-----");
    let end = format!("-----END {label}-----");
    let mut out = Vec::new();
    let mut rest = pem;
    while let Some(start) = rest.find(&begin) {
        let after = &rest[start + begin.len()..];
        let Some(end_pos) = after.find(&end) else {
            return Err(format!("PEM 段 {label} 缺 END 标记"));
        };
        let body: String = after[..end_pos]
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        let der =
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, body.as_bytes())
                .map_err(|e| format!("PEM base64 解码失败（{label}）: {e}"))?;
        out.push(der);
        rest = &after[end_pos + end.len()..];
    }
    if out.is_empty() {
        return Err(format!("PEM 未找到 {label} 段"));
    }
    Ok(out)
}

/// 解析证书 PEM（支持链形态多段）。
fn parse_certs_pem(pem: &str) -> Result<Vec<CertificateDer<'static>>, String> {
    pem_to_ders(pem, "CERTIFICATE").map(|ders| ders.into_iter().map(CertificateDer::from).collect())
}

/// 解析私钥 PEM（仅 PKCS#8：`-----BEGIN PRIVATE KEY-----`；自签路径
/// rcgen 产物即此形态；其他格式诚实报错附转换命令）。
fn parse_key_pem(pem: &str) -> Result<PrivateKeyDer<'static>, String> {
    let der = pem_to_ders(pem, "PRIVATE KEY")?
        .pop()
        .ok_or_else(|| "PEM 未找到 PRIVATE KEY 段".to_string())?;
    Ok(PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(der)))
}

#[cfg(test)]
mod tests;
