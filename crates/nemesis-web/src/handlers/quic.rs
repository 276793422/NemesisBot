//! QUIC 中继 WSAPI（2026-10-23 与集群解耦；QUIC 插件化 M1 起**代理化**）
//! —— `quic.relay.*` + `quic.registry.members`。
//!
//! **零 Cluster 运行面类型依赖 + 零 iroh 依赖**：config 真相源 =
//! `config.quic.json`（`relay` 服务端段 + `client` 客户端段，见
//! `nemesis_cluster::roaming::quic_config`，纯 serde 解析函数）；运行态 =
//! `AppState.quic_relay` 槽（plugin_roaming C-ABI 句柄，fn 指针 Copy）——
//! 启停/成员查询经 `spawn_blocking` 调插件（拍板 10），ok:false 展开
//! 与 nemesisbot loader `call_roaming` 同源形态。插件未装载 =
//! 诚实降级（status 带插件缺席注记、启停/名册报 not loaded）。
//!
//! feature：无模块级闸（M4 起 cluster-roaming feature 已删，本模块恒编译
//! ——命令面恒注册，L1 清单稳定）。`client.roaming_enabled` 观测键读集群
//! config（纯 config 函数——客户端是否消费 relay_url 由集群 roaming 开关
//! 决定，展示给用户判断连通形态）。

use crate::handlers::require_workspace;
use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_cluster::roaming::config::{RelayTls, resolved_roaming};
use nemesis_cluster::roaming::quic_config::resolved_quic;
use nemesis_cluster::roaming::relay_info;
use nemesis_plugin::roaming::RoamingPluginHandle;
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
            // QUIC 自建中继（运行面在 plugin_roaming 插件；插件缺席 = status
            // 诚实降级 available:false，config 保存/开关/名册报 not loaded——
            // 命令面恒注册，L1 清单稳定，roaming.status 同款先例）
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
            "relay.status" => self.relay_status(ctx).await,
            "relay.config" => {
                let data = data.as_ref().ok_or("missing data")?;
                self.relay_config_save(ctx, data)
            }
            "relay.enable" => self.relay_toggle(ctx, true).await,
            "relay.disable" => self.relay_toggle(ctx, false).await,
            "registry.members" => self.registry_members(ctx).await,
            _ => Err(format!("unknown command: quic.{}", cmd)),
        }
    }
}

impl QuicHandler {
    /// quic.relay.status：config 闸 + 插件运行态 + 证书状态 + 客户端面。
    /// 插件未装载 = Ok(available:false)——前端据此渲染降级面板。
    async fn relay_status(
        &self,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let workspace = require_workspace(ctx)?.to_string();
        let (plugin_loaded, active, http_addr, quic_addr) =
            plugin_runtime_state(ctx.state.quic_relay).await;
        let mut p = relay_status_payload(&workspace, active, http_addr, quic_addr)?;
        if let Some(obj) = p.as_mut().and_then(|v| v.as_object_mut()) {
            obj.insert(
                "plugin_loaded".to_string(),
                serde_json::json!(plugin_loaded),
            );
        }
        Ok(p)
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
        relay_config_save_impl(&workspace, data)
    }

    /// quic.relay.enable / relay.disable：**实时开关**（免重启网关）。config 闸
    /// 落盘（relay_config_save_impl 同源合并语义）+ 插件内拉起/停掉中继
    /// （roaming_start 幂等 / roaming_stop 与停机链同源单一实现；拍板 10
    /// spawn_blocking）。返回与 relay.status 同款载荷——动态启停后
    /// enabled/active 恒同步，requires_restart 自然熄灭。enable 失败
    /// （token 空 fail-closed）Err 上抛且 config 闸保持开——修复后重开开关
    /// 即重试，与「配置说了算」语义一致。插件未装载 = 只落 config 闸
    /// （requires_restart 诚实呈现，不假成功）。
    async fn relay_toggle(
        &self,
        ctx: &RequestContext,
        enable: bool,
    ) -> Result<Option<serde_json::Value>, String> {
        let workspace = require_workspace(ctx)?.to_string();
        relay_toggle_impl(ctx.state.quic_relay, &workspace, enable).await
    }

    /// quic.registry.members：服务端全量名册（中继机视角，含 TTL 内全部
    /// 报到成员）。中继未运行 / 注册表未拉起 = 空表（不是错误——状态诚实）；
    /// 插件未装载 = available:false + reason（空表会伪装「在跑没人」）。
    async fn registry_members(
        &self,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let _ = require_workspace(ctx)?;
        let Some(handle) = ctx.state.quic_relay else {
            return Ok(Some(serde_json::json!({
                "available": false,
                "count": 0,
                "members": [],
                "reason": "roaming 插件未装载（plugins/plugin_roaming 缺席或装载失败）",
            })));
        };
        let v = plugin_call(handle, "roaming_status", RoamingPluginHandle::status).await?;
        let members = v
            .get("registry_members")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([]));
        let count = members.as_array().map(|a| a.len()).unwrap_or(0);
        Ok(Some(serde_json::json!({
            "available": true,
            "count": count,
            "members": members,
        })))
    }
}

// ---------------------------------------------------------------------------
// 插件调用辅助（拍板 10：同步 FFI 内 block_on 插件 runtime——宿主 spawn_blocking；
// ok:false 展开与 nemesisbot roaming_plugin_loader::call_roaming 同源形态）
// ---------------------------------------------------------------------------

/// 单次插件调用：spawn_blocking + JSON 解析 + ok:false 展开。
async fn plugin_call(
    handle: RoamingPluginHandle,
    what: &'static str,
    f: impl FnOnce(RoamingPluginHandle) -> Result<String, String> + Send + 'static,
) -> Result<serde_json::Value, String> {
    let raw = tokio::task::spawn_blocking(move || f(handle))
        .await
        .map_err(|e| format!("{what}: spawn_blocking join 失败: {e}"))??;
    let v: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("{what}: 返回非 JSON: {e}"))?;
    if v.get("ok").and_then(|b| b.as_bool()) == Some(false) {
        let code = v
            .get("err_code")
            .and_then(|c| c.as_str())
            .unwrap_or("unknown");
        let msg = v.get("err_msg").and_then(|m| m.as_str()).unwrap_or("");
        return Err(format!("{what}: [{code}] {msg}"));
    }
    Ok(v)
}

/// 运行态四元组（plugin_loaded / running / http_addr / quic_addr）。
/// 插件缺席或 status 失败 = 未运行诚实呈现（地址 null）。
async fn plugin_runtime_state(
    handle: Option<RoamingPluginHandle>,
) -> (bool, bool, serde_json::Value, serde_json::Value) {
    let Some(h) = handle else {
        return (
            false,
            false,
            serde_json::Value::Null,
            serde_json::Value::Null,
        );
    };
    match plugin_call(h, "roaming_status", RoamingPluginHandle::status).await {
        Ok(v) => (
            true,
            v.get("running").and_then(|b| b.as_bool()).unwrap_or(false),
            v.get("http_addr")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
            v.get("quic_addr")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        ),
        Err(_) => (
            true,
            false,
            serde_json::Value::Null,
            serde_json::Value::Null,
        ),
    }
}

// ---------------------------------------------------------------------------
// 自由函数
// ---------------------------------------------------------------------------

/// QUIC 中继配置路径（config.quic.json；2026-10-23 与集群解耦后的真相源）。
pub(crate) fn quic_config_path(workspace: &str) -> PathBuf {
    nemesis_path::resolve_quic_config_path_in_workspace(Path::new(workspace))
}

/// relay 段统一投影（status/config 保存回显共用）。**token 永不回传**——
/// 只有 token_set bool（凭据面纪律，与漫游信任簿同源）。证书面按 TLS 档：
/// self-signed = workspace 路径 + generated/pending；manual = 用户路径回显
/// + ready/missing；none = unused。
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
            // 插件化 M1 后路径真相源 = relay_info（与插件副本同源对账）。
            let cert = relay_info::relay_cert_path(Path::new(workspace));
            let key = relay_info::relay_key_path(Path::new(workspace));
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
        "advertised_url": relay_info::advertised_relay_url(relay),
    })
}

/// quic.relay.status 载荷。enabled=config 闸（resolved_quic 单一真相
/// 源，config.quic.json）；active=运行态（插件内中继在跑）；requires_restart
/// = 两者不一致。client 面带客户端三键（roaming_enabled 来自 cluster.json
/// roaming 段 / relay_url/relay_token_set 来自 quic.json client 段——令牌只
/// 给 bool）。地址为插件回执的字符串/null 原样透传。
fn relay_status_payload(
    workspace: &str,
    active: bool,
    http_addr: serde_json::Value,
    quic_addr: serde_json::Value,
) -> Result<Option<serde_json::Value>, String> {
    let resolved = resolved_quic(&quic_config_path(workspace));
    let roaming_enabled = resolved_roaming(&cluster_config_path(workspace)).enabled;
    Ok(Some(serde_json::json!({
        "available": true,
        "enabled": resolved.relay.enabled,
        "active": active,
        "requires_restart": resolved.relay.enabled != active,
        "http_addr": http_addr,
        "quic_addr": quic_addr,
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

/// relay_toggle 的实现核心（ctx 解耦——tests 以 handle=None 直接验证
/// config+载荷契约；真中继拉起/停掉的 roundtrip 归插件侧 e2e +
/// gateway 装载冒烟，[`crate::handlers::quic::tests`] 只钉 None 臂）。
///
/// ① config 闸先动（单一真相源，config.quic.json relay.enabled）：enable
/// 中途失败（token 空 fail-closed）闸保持开——修复后重开开关即重试。
/// ② 插件内拉起/停掉（幂等；句柄未装载时只落 config 闸——运行态诚实
/// 呈现 requires_restart）。③ 最新状态回前端。
async fn relay_toggle_impl(
    handle: Option<RoamingPluginHandle>,
    workspace: &str,
    enable: bool,
) -> Result<Option<serde_json::Value>, String> {
    relay_config_save_impl(workspace, &serde_json::json!({ "enabled": enable }))?;
    if let Some(h) = handle {
        if enable {
            plugin_call(h, "roaming_start", RoamingPluginHandle::start).await?;
        } else {
            plugin_call(h, "roaming_stop", RoamingPluginHandle::stop).await?;
        }
    }
    let (_, active, http_addr, quic_addr) = plugin_runtime_state(handle).await;
    relay_status_payload(workspace, active, http_addr, quic_addr)
}

#[cfg(test)]
mod tests;
