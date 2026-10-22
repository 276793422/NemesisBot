//! QUIC 中继 WSAPI（2026-10-23 与集群解耦）——`quic.relay.*` + `quic.registry.members`。
//!
//! **零 Cluster 类型依赖**（解耦的编译级验收点）：本模块不 import
//! `nemesis_cluster::cluster::Cluster`、不读 `AppState.cluster`——集群未编译/
//! 未启用时 QUIC 中继管理面照常工作。config 真相源 = `config.quic.json`
//! （`relay` 服务端段 + `client` 客户端段，见
//! `nemesis_cluster::roaming::quic_config`）；运行态 = `AppState.quic_relay`
//! 槽（`QuicRelayManager`，gateway 未装配时诚实降级）。
//!
//! feature：无模块级闸（命令面恒注册，L1 清单稳定，voice-capture/roaming
//! 先例）；`quic-relay` 关 = status 诚实降级 `available:false`，写面命令
//! 运行期报「feature 未编译」。`client.roaming_enabled` 观测键读集群
//! config（纯 config 函数，非 Cluster 类型——客户端是否消费 relay_url
//! 由集群 roaming 开关决定，展示给用户判断连通形态）。

use crate::handlers::require_workspace;
use crate::ws_router::{ModuleHandler, RequestContext};
#[cfg(feature = "quic-relay")]
use nemesis_cluster::roaming::config::{RelayTls, resolved_roaming};
#[cfg(feature = "quic-relay")]
use nemesis_cluster::roaming::quic_config::resolved_quic;
#[cfg(feature = "quic-relay")]
use nemesis_cluster::roaming::relay_server;
#[cfg(feature = "quic-relay")]
use std::path::{Path, PathBuf};

pub struct QuicHandler {
    _priv: (),
}

impl Default for QuicHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl QuicHandler {
    pub fn new() -> Self {
        Self { _priv: () }
    }
}

#[async_trait::async_trait]
impl ModuleHandler for QuicHandler {
    fn module_name(&self) -> &str {
        "quic"
    }

    fn commands(&self) -> &'static [&'static str] {
        &[
            // QUIC 自建中继（feature=quic-relay；关 = status 诚实降级
            // available:false，config 保存/开关/名册运行期报未编译——命令面
            // 恒注册，L1 清单稳定，roaming.status 同款先例）
            "relay.status",
            "relay.config",
            // 实时开关（免重启网关）：config 闸落盘 + 进程内拉起/停掉一体。
            "relay.enable",
            "relay.disable",
            // 服务端全量名册（中继机视角；客户端集群视图经 features 严格
            // 过滤另行形成，归 cluster.roaming.status 的 rendezvous 段）
            "registry.members",
        ]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        match cmd {
            "relay.status" => self.relay_status(ctx),
            "relay.config" => {
                let data = data.as_ref().ok_or("missing data")?;
                self.relay_config_save(ctx, data)
            }
            "relay.enable" => self.relay_toggle(ctx, true).await,
            "relay.disable" => self.relay_toggle(ctx, false).await,
            "registry.members" => self.registry_members(ctx),
            _ => Err(format!("unknown command: quic.{}", cmd)),
        }
    }
}

impl QuicHandler {
    /// quic.relay.status：config 闸 + 运行态 + 证书状态 + 客户端面。
    /// feature 关 = Ok(available:false)——前端据此渲染降级面板。
    #[cfg(feature = "quic-relay")]
    fn relay_status(&self, ctx: &RequestContext) -> Result<Option<serde_json::Value>, String> {
        let workspace = require_workspace(ctx)?.to_string();
        let (active, http_addr, quic_addr) = match ctx.state.quic_relay.as_ref() {
            Some(mgr) => mgr.status(),
            None => (false, None, None),
        };
        relay_status_payload(&workspace, active, http_addr, quic_addr)
    }

    #[cfg(not(feature = "quic-relay"))]
    fn relay_status(&self, ctx: &RequestContext) -> Result<Option<serde_json::Value>, String> {
        let _ = require_workspace(ctx)?;
        Ok(Some(serde_json::json!({
            "available": false,
            "enabled": false,
            "active": false,
            "requires_restart": false,
            "reason": "quic-relay feature 未编译（QUIC 中继层不可用）",
        })))
    }

    /// quic.relay.config：保存服务端 relay 段（合并写，兄弟键保留；
    /// 空串 = 清除该键回落默认）。参数面（端口/地址/TLS/令牌）——中继在
    /// 启动时读 config，改动后重新开关中继（或重启网关）生效；开关面走
    /// relay.enable / relay.disable（实时生效，免重启）。
    fn relay_config_save(
        &self,
        ctx: &RequestContext,
        data: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        let workspace = require_workspace(ctx)?.to_string();
        #[cfg(feature = "quic-relay")]
        return relay_config_save_impl(&workspace, data);
        #[cfg(not(feature = "quic-relay"))]
        {
            let _ = (workspace, data);
            Err("quic-relay feature 未编译（QUIC 中继层不可用）".to_string())
        }
    }

    /// quic.relay.enable / relay.disable：**实时开关**（免重启网关）。config 闸
    /// 落盘（relay_config_save_impl 同源合并语义）+ 进程内拉起/停掉中继
    /// （QuicRelayManager::setup 幂等 / stop 与停机 teardown 同源单一实现）。
    /// 返回与 relay.status 同款载荷——动态启停后 enabled/active 恒
    /// 同步，requires_restart 自然熄灭。enable 失败（token 空 fail-closed）Err
    /// 上抛且 config 闸保持开——修复后重开开关即重试，与「配置说了算」语义一致。
    async fn relay_toggle(
        &self,
        ctx: &RequestContext,
        enable: bool,
    ) -> Result<Option<serde_json::Value>, String> {
        let workspace = require_workspace(ctx)?.to_string();
        #[cfg(feature = "quic-relay")]
        return relay_toggle_impl(ctx.state.quic_relay.as_deref(), &workspace, enable).await;
        #[cfg(not(feature = "quic-relay"))]
        {
            let _ = (workspace, enable);
            Err("quic-relay feature 未编译（QUIC 中继层不可用）".to_string())
        }
    }

    /// quic.registry.members：服务端全量名册（中继机视角，含 TTL 内全部
    /// 报到成员）。中继未运行 / 注册表未拉起 = 空表（不是错误——状态诚实）。
    #[cfg(feature = "quic-relay")]
    fn registry_members(&self, ctx: &RequestContext) -> Result<Option<serde_json::Value>, String> {
        let _ = require_workspace(ctx)?;
        let members = match ctx.state.quic_relay.as_ref() {
            Some(mgr) => mgr.registry_members(),
            None => Vec::new(),
        };
        Ok(Some(serde_json::json!({
            "available": true,
            "count": members.len(),
            "members": members,
        })))
    }

    #[cfg(not(feature = "quic-relay"))]
    fn registry_members(&self, ctx: &RequestContext) -> Result<Option<serde_json::Value>, String> {
        let _ = require_workspace(ctx)?;
        Err("quic-relay feature 未编译（QUIC 中继层不可用）".to_string())
    }
}

// ---------------------------------------------------------------------------
// 自由函数（feature=quic-relay 穿透）
// ---------------------------------------------------------------------------

/// QUIC 中继配置路径（config.quic.json；2026-10-23 与集群解耦后的真相源）。
#[cfg(feature = "quic-relay")]
pub(crate) fn quic_config_path(workspace: &str) -> PathBuf {
    nemesis_path::resolve_quic_config_path_in_workspace(Path::new(workspace))
}

/// relay 段统一投影（status/config 保存回显共用）。**token 永不回传**——
/// 只有 token_set bool（凭据面纪律，与漫游信任簿同源）。证书面按 TLS 档：
/// self-signed = workspace 路径 + generated/pending；manual = 用户路径回显
/// + ready/missing；none = unused。
#[cfg(feature = "quic-relay")]
fn relay_settings_payload(
    relay: &nemesis_cluster::roaming::config::RelaySettings,
    workspace: &str,
) -> serde_json::Value {
    let tls_str = match relay.tls {
        RelayTls::SelfSigned => "self-signed",
        RelayTls::Manual => "manual",
        RelayTls::None => "none",
    };
    let (cert_path, cert_state, key_path, key_state) = match relay.tls {
        RelayTls::Manual => {
            let cert = Path::new(&relay.cert_path);
            let key = Path::new(&relay.key_path);
            (
                relay.cert_path.clone(),
                if !relay.cert_path.is_empty() && cert.exists() {
                    "ready"
                } else {
                    "missing"
                },
                relay.key_path.clone(),
                if !relay.key_path.is_empty() && key.exists() {
                    "ready"
                } else {
                    "missing"
                },
            )
        }
        RelayTls::None => (String::new(), "unused", String::new(), "unused"),
        RelayTls::SelfSigned => {
            let cert = relay_server::relay_cert_path(Path::new(workspace));
            let key = relay_server::relay_key_path(Path::new(workspace));
            (
                cert.display().to_string(),
                if cert.exists() {
                    "generated"
                } else {
                    "pending"
                },
                key.display().to_string(),
                if key.exists() { "generated" } else { "pending" },
            )
        }
    };
    serde_json::json!({
        "enabled": relay.enabled,
        "http_port": relay.http_port,
        "quic_port": relay.quic_port,
        "external_host": relay.external_host,
        "tls": tls_str,
        "token_set": !relay.token.is_empty(),
        "cert_path": cert_path,
        "cert_state": cert_state,
        "key_path": key_path,
        "key_state": key_state,
        "advertised_url": relay_server::advertised_relay_url(relay),
    })
}

/// quic.relay.status 载荷。enabled=config 闸（resolved_quic 单一真相
/// 源，config.quic.json）；active=运行态（gateway 进程内中继在跑）；
/// requires_restart = 两者不一致。client 面带客户端三键（roaming_enabled
/// 来自 cluster.json roaming 段 / relay_url/relay_token_set 来自 quic.json
/// client 段——令牌只给 bool）。
#[cfg(feature = "quic-relay")]
fn relay_status_payload(
    workspace: &str,
    active: bool,
    http_addr: Option<std::net::SocketAddr>,
    quic_addr: Option<std::net::SocketAddr>,
) -> Result<Option<serde_json::Value>, String> {
    let resolved = resolved_quic(&quic_config_path(workspace));
    let roaming_enabled = resolved_roaming(&cluster_config_path(workspace)).enabled;
    Ok(Some(serde_json::json!({
        "available": true,
        "enabled": resolved.relay.enabled,
        "active": active,
        "requires_restart": resolved.relay.enabled != active,
        "http_addr": http_addr.map(|a| a.to_string()),
        "quic_addr": quic_addr.map(|a| a.to_string()),
        "relay": relay_settings_payload(&resolved.relay, workspace),
        "client": {
            "roaming_enabled": roaming_enabled,
            "relay_url": resolved.client.relay_url,
            "relay_token_set": !resolved.client.relay_token.is_empty(),
        },
    })))
}

/// 集群 config 路径（roaming_enabled 观测键数据源；纯路径换算——
/// 路径唯一真相源 = nemesis-path）。
#[cfg(feature = "quic-relay")]
fn cluster_config_path(workspace: &str) -> PathBuf {
    nemesis_path::resolve_cluster_config_path_in_workspace(Path::new(workspace))
}

/// quic.relay.config 保存实现：校验 → RMW 合并 **config.quic.json** 的
/// `relay` 段（服务端面）+ `client` 段两键（relay_url/relay_token；quic.json
/// 其他键全保留）。**空串 = 删除该键 = 恢复解析默认**（token 空回解析默认
/// 空——启动面对空 token fail-closed 拒绝，中继不起；relay_url 空 = 客户端
/// 直连不走向继）；端口强校验 1-65535（0 会让对外
/// URL 变 host:0 废话）；tls 强校验三值——解析层对未知 tls 宽容降级
/// SelfSigned，写面不许（写面是当次意志，静默纠偏会把 manual 写成
/// self-signed）。
#[cfg(feature = "quic-relay")]
fn relay_config_save_impl(
    workspace: &str,
    data: &serde_json::Value,
) -> Result<Option<serde_json::Value>, String> {
    let obj = data.as_object().ok_or("data must be an object")?;
    let mut patch = serde_json::Map::new();
    if let Some(v) = obj.get("enabled") {
        let b = v.as_bool().ok_or("enabled must be a bool")?;
        patch.insert("enabled".to_string(), serde_json::json!(b));
    }
    for key in ["http_port", "quic_port"] {
        if let Some(v) = obj.get(key) {
            let n = v
                .as_u64()
                .ok_or_else(|| format!("{key} must be a port number"))?;
            let p = u16::try_from(n).map_err(|_| format!("{key} out of range (1-65535)"))?;
            if p == 0 {
                return Err(format!(
                    "{key} 必须在 1-65535（0 会让对外 relay URL 无意义）"
                ));
            }
            patch.insert(key.to_string(), serde_json::json!(p));
        }
    }
    if let Some(v) = obj.get("external_host") {
        let s = v.as_str().ok_or("external_host must be a string")?;
        let s = s.trim();
        // 空串 = 清除（回落自动探测）；Null 同效（下同）。
        patch.insert(
            "external_host".to_string(),
            if s.is_empty() {
                serde_json::Value::Null
            } else {
                serde_json::json!(s)
            },
        );
    }
    if let Some(v) = obj.get("tls") {
        let s = v.as_str().ok_or("tls must be a string")?;
        let s = s.trim().to_ascii_lowercase();
        match s.as_str() {
            "self-signed" | "manual" | "none" => {
                patch.insert("tls".to_string(), serde_json::json!(s));
            }
            other => {
                return Err(format!(
                    "tls 档非法: {other}（合法: self-signed | manual | none）"
                ));
            }
        }
    }
    if let Some(v) = obj.get("token") {
        let s = v.as_str().ok_or("token must be a string")?;
        let s = s.trim();
        // 空串 = 清除令牌（解析默认空；启动面 fail-closed 拒绝无鉴权中继）。
        patch.insert(
            "token".to_string(),
            if s.is_empty() {
                serde_json::Value::Null
            } else {
                serde_json::json!(s)
            },
        );
    }
    for key in ["cert_path", "key_path"] {
        if let Some(v) = obj.get(key) {
            let s = v
                .as_str()
                .ok_or_else(|| format!("{key} must be a string"))?;
            let s = s.trim();
            patch.insert(
                key.to_string(),
                if s.is_empty() {
                    serde_json::Value::Null
                } else {
                    serde_json::json!(s)
                },
            );
        }
    }
    // 客户端面两键（写 quic.json client 段，非 relay 子段）：relay_url 空 =
    // 客户端不走向继（直连/打洞）；relay_token 空 = 清除。
    let mut client_patch = serde_json::Map::new();
    for key in ["relay_url", "relay_token"] {
        if let Some(v) = obj.get(key) {
            let s = v
                .as_str()
                .ok_or_else(|| format!("{key} must be a string"))?;
            let s = s.trim();
            client_patch.insert(
                key.to_string(),
                if s.is_empty() {
                    serde_json::Value::Null
                } else {
                    serde_json::json!(s)
                },
            );
        }
    }
    if patch.is_empty() && client_patch.is_empty() {
        return Err(
            "no recognized relay fields（可写键: enabled/http_port/quic_port/external_host/tls/token/cert_path/key_path/relay_url/relay_token）"
                .to_string(),
        );
    }

    let _cfg_serial = nemesis_config::config_write_lock();
    let path = quic_config_path(workspace);
    let mut cfg: serde_json::Value = if path.exists() {
        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("failed to read quic config: {}", e))?;
        serde_json::from_str(&content).unwrap_or(serde_json::json!({}))
    } else {
        serde_json::json!({})
    };
    let root = cfg
        .as_object_mut()
        .ok_or("quic config root must be an object")?;
    let relay = root
        .entry("relay".to_string())
        .or_insert_with(|| serde_json::json!({}));
    let relay = relay
        .as_object_mut()
        .ok_or("relay section must be an object")?;
    for (k, v) in patch {
        if v.is_null() {
            relay.remove(&k);
        } else {
            relay.insert(k, v);
        }
    }
    let client = root
        .entry("client".to_string())
        .or_insert_with(|| serde_json::json!({}));
    let client = client
        .as_object_mut()
        .ok_or("client section must be an object")?;
    for (k, v) in client_patch {
        if v.is_null() {
            client.remove(&k);
        } else {
            client.insert(k, v);
        }
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let json =
        serde_json::to_string_pretty(&cfg).map_err(|e| format!("failed to serialize: {}", e))?;
    // 统一原子写入（config.quic.json 含 token，REL-002 同款）。
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), json.as_bytes(), 0o600)
        .map_err(|e| format!("failed to write quic config: {}", e))?;
    let resolved = resolved_quic(&path);
    Ok(Some(serde_json::json!({
        "saved": true,
        "requires_restart": true,
        "relay": relay_settings_payload(&resolved.relay, workspace),
        "client": {
            "relay_url": resolved.client.relay_url,
            "relay_token_set": !resolved.client.relay_token.is_empty(),
        },
    })))
}

/// relay_toggle 的实现核心（ctx 解耦——tests 以 manager=None 直接验证
/// config+载荷契约；真中继拉起/停掉的 roundtrip 归 cluster 侧 cov 测试，
/// [`crate::handlers::quic::tests`] 只钉 None 臂）。
///
/// ① config 闸先动（单一真相源，config.quic.json relay.enabled）：enable
/// 中途失败（token 空 fail-closed）闸保持开——修复后重开开关即重试。
/// ② 进程内拉起/停掉（幂等；manager 未装配时只落 config 闸——运行态诚实
/// 呈现 requires_restart）。③ 最新状态回前端。
#[cfg(feature = "quic-relay")]
async fn relay_toggle_impl(
    manager: Option<&nemesis_cluster::roaming::relay_manager::QuicRelayManager>,
    workspace: &str,
    enable: bool,
) -> Result<Option<serde_json::Value>, String> {
    relay_config_save_impl(workspace, &serde_json::json!({ "enabled": enable }))?;
    if let Some(mgr) = manager {
        if enable {
            mgr.setup().await?;
        } else {
            mgr.stop();
        }
    }
    let (active, http_addr, quic_addr) = match manager {
        Some(mgr) => mgr.status(),
        None => (false, None, None),
    };
    relay_status_payload(workspace, active, http_addr, quic_addr)
}

#[cfg(test)]
mod tests;
