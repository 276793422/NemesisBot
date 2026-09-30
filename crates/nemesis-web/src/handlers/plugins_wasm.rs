//! WASM 插件管理面（`plugins.wasm.*` WSAPI，W6）。
//!
//! 宿主运行时**整槽注入**：gateway init_agent 构建的 `PluginInstaller`
//! （内含 PluginManager + 验签器 + 扫描链 + 审批面）经 [`set_installer`]
//! 静态槽注入（AppState 字面量散布百处测试不可爆破——COMMANDS_REGISTRY /
//! metrics_plugin_slot 同款静态槽先例）。槽空 = 子系统未装配（未启用 /
//! headless / 测试态），命令统一诚实报错。
//!
//! 审批：WSAPI install 走注入 installer 自带的审批面（gateway 侧
//! LateInstallApprover 已被 run_runtime bind 到 WebApprovalManager →
//! Dashboard 审批卡阻塞等答，超时=拒绝；测试态 AutoApprove 直通）。
//!
//! 命令（module=`plugins`，cmd 带 `wasm.` 前缀，由 plugins.rs 代理）：
//! `wasm.list` / `wasm.install` / `wasm.enable` / `wasm.disable` /
//! `wasm.uninstall` / `wasm.config.get` / `wasm.config.set` / `wasm.logs` /
//! `wasm.devkit_download`（plugins.rs 自处理分支，不依赖本模块宿主槽）。
//!
//! 安全面：install 是九步装配漏斗唯一运行期入口（验签→扫描→审批→编译
//! →落位→lockfile 全走 nemesis-plugins-wasm::install 单一真相源，本模块
//! 零旁路）；config.get 按 manifest x-secret 脱敏（值不回传 Dashboard）。

use std::path::Path;
use std::sync::Arc;

use nemesis_plugins_wasm::install::PluginInstaller;

static INSTALLER: std::sync::OnceLock<Arc<PluginInstaller>> = std::sync::OnceLock::new();

/// gateway init_agent 注入装配产物（幂等失败 = 重复注入，首个胜出——
/// gateway 生命周期内只建一次 installer，语义即单发）。
pub fn set_installer(installer: Arc<PluginInstaller>) {
    let _ = INSTALLER.set(installer);
}

/// W7：agent 工具面同步事件（热装/卸/启停时桥接回调把插件工具同步进
/// AgentLoop 注册表；gateway init_agent 注入闭包，桥构造留在 gateway 侧——
/// `PluginToolBridge` 是 bin crate 类型，web 只带必要信息）。
#[derive(Debug, Clone)]
pub struct ToolSyncEvent {
    /// true = 注册（install/enable），false = 注销（uninstall/disable）。
    pub add: bool,
    pub slug: String,
    /// 工具全名 `plugin.<slug>.<base>`（注销方在注册表摘除**前**抓取）。
    pub tool_name: String,
    /// guest `operation_type` 声明（注册侧 declare_tool_operation 映射用）。
    pub operation_type: String,
}

type ToolSyncHook = Arc<dyn Fn(ToolSyncEvent) + Send + Sync>;
static TOOL_SYNC_HOOK: std::sync::OnceLock<ToolSyncHook> = std::sync::OnceLock::new();

/// gateway init_agent 注入工具面同步回调（幂等失败 = 重复注入，首个胜出）。
pub fn set_tool_sync_hook(hook: ToolSyncHook) {
    let _ = TOOL_SYNC_HOOK.set(hook);
}

/// 触发工具面同步（hook 未注入 = 无 loop 可同步（CLI/测试形态），静默略过）。
pub fn notify_tool_sync(ev: ToolSyncEvent) {
    if let Some(hook) = TOOL_SYNC_HOOK.get() {
        hook(ev);
    }
}

fn installer() -> Option<Arc<PluginInstaller>> {
    INSTALLER.get().cloned()
}

fn require_installer() -> Result<Arc<PluginInstaller>, String> {
    installer().ok_or_else(|| {
        "WASM 插件子系统未装配（plugins.wasm.enabled=false 或非 gateway 运行态）".to_string()
    })
}

// ---------------------------------------------------------------------------
// 命令实现（ctx 无关——测试直接调内部 fn，不经 RequestContext）
// ---------------------------------------------------------------------------

/// 行投影（已装载）。
fn row_of(reg: &nemesis_plugins_wasm::registry::RegisteredPlugin) -> serde_json::Value {
    let m = &reg.manifest;
    serde_json::json!({
        "slug": m.slug,
        "name": m.name,
        "version": m.version,
        "kind": m.kind.as_str(),
        "trust": reg.trust.as_str(),
        "enabled": reg.enabled.load(std::sync::atomic::Ordering::SeqCst),
        "loaded": true,
        "wasm_sha256": m.wasm_sha256,
        "min_tier": m.min_tier,
        "egress": m.permissions.egress,
        "x_secret": m.permissions.x_secret,
        "tool": reg.tool_meta.as_ref().map(|t| serde_json::json!({
            "name": t.name,
            "base": t.base,
            "title": t.title,
            "description": t.description,
            "operation_type": t.operation_type,
            "min_tier": t.min_tier,
        })),
        "data_dir": reg.data_dir.display().to_string(),
    })
}

/// `wasm.list`：注册表快照 + lockfile 交叉（盘上有但未装载的行
/// `loaded=false`——启动装载失败/刚被 CLI 装入未重启的可见性兜底）。
pub fn cmd_list() -> Result<serde_json::Value, String> {
    let installer = require_installer()?;
    let mgr = &installer.manager;
    let mut rows: Vec<serde_json::Value> = mgr.list().iter().map(|r| row_of(r)).collect();
    let loaded: std::collections::BTreeSet<String> =
        mgr.list().iter().map(|r| r.manifest.slug.clone()).collect();
    let lock_path = mgr.plugins_dir().join("lockfile.json");
    if let Ok(s) = std::fs::read_to_string(&lock_path)
        && let Ok(lf) = serde_json::from_str::<nemesis_plugins_wasm::install::PluginLockfile>(&s)
    {
        for (slug, e) in &lf.plugins {
            if !loaded.contains(slug) {
                rows.push(serde_json::json!({
                    "slug": slug,
                    "version": e.version,
                    "kind": "unknown",
                    "trust": e.trust,
                    "enabled": false,
                    "loaded": false,
                    "wasm_sha256": e.wasm_sha256,
                    "signed_by": e.signed_by,
                }));
            }
        }
    }
    Ok(serde_json::json!({ "plugins": rows }))
}

/// `wasm.install`（运行期装配唯一入口 = install 漏斗；含升级重跑）。
pub async fn cmd_install(data: Option<serde_json::Value>) -> Result<serde_json::Value, String> {
    let installer = require_installer()?;
    let data = data.ok_or("missing data")?;
    let source_dir = data
        .get("source_dir")
        .and_then(|v| v.as_str())
        .ok_or("source_dir (string) is required")?;
    let allow_unsigned = data
        .get("allow_unsigned")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let reg = installer
        .install(Path::new(source_dir), allow_unsigned)
        .await
        .map_err(|e| e.to_string())?;
    // W7：工具型插件热注册进 agent 工具面（观察者无工具面，不触发）。
    // enabled=false（升级了一个已禁用的插件）不发 add——hook 的 add=false
    // 路径按 `plugin.<slug>.` 前缀整面摘除，顺带清掉 loop 里可能残留的旧
    // 版工具（CLI disable 只改盘不改 loop 的场景，2026-09-29 交付审查 1.2）。
    if *reg.manifest.kind == nemesis_plugins_wasm::manifest::PluginKind::Tool
        && let Some(meta) = reg.tool_meta.as_ref()
    {
        let enabled = reg.enabled.load(std::sync::atomic::Ordering::SeqCst);
        notify_tool_sync(ToolSyncEvent {
            add: enabled,
            slug: reg.manifest.slug.clone(),
            tool_name: meta.name.clone(),
            operation_type: meta.operation_type.clone(),
        });
    }
    Ok(serde_json::json!({
        "installed": true,
        "plugin": row_of(reg.as_ref()),
    }))
}

fn require_slug(data: Option<&serde_json::Value>) -> Result<String, String> {
    data.and_then(|d| d.get("slug"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "slug (string) is required".to_string())
}

/// `wasm.enable` / `wasm.disable`（内存 + 实例配置落盘，热生效）。
pub fn cmd_set_enabled(slug: &str, on: bool) -> Result<serde_json::Value, String> {
    let installer = require_installer()?;
    installer
        .manager
        .set_enabled_plugin(slug, on)
        .map_err(|e| e.to_string())?;
    // W7：禁用 = 工具面注销（LLM 不再可见），启用 = 重注册（仍在册时）。
    sync_tool_face(&installer.manager, slug, on);
    Ok(serde_json::json!({ "slug": slug, "enabled": on }))
}

/// `wasm.uninstall`（注销 + 删载荷 + lockfile 摘除；数据目录保留）。
pub async fn cmd_uninstall(slug: &str) -> Result<serde_json::Value, String> {
    let installer = require_installer()?;
    // 注销前抓工具面信息（注册表摘除后拿不到 tool_name/op）。
    let sync = tool_event_of(&installer.manager, slug, false);
    let existed = nemesis_plugins_wasm::install::uninstall(installer.manager.as_ref(), slug)
        .await
        .map_err(|e| e.to_string())?;
    if existed && let Some(ev) = sync {
        notify_tool_sync(ev);
    }
    Ok(serde_json::json!({ "slug": slug, "existed": existed }))
}

/// 从注册表投影工具面同步事件（`kind=tool` 且已装载才产生；`add=false` 时
/// 用于注销——须在注册表摘除前调用）。
fn tool_event_of(
    mgr: &nemesis_plugins_wasm::registry::PluginManager,
    slug: &str,
    add: bool,
) -> Option<ToolSyncEvent> {
    let reg = mgr.get(slug)?;
    if *reg.manifest.kind != nemesis_plugins_wasm::manifest::PluginKind::Tool {
        return None;
    }
    let meta = reg.tool_meta.as_ref()?;
    Some(ToolSyncEvent {
        add,
        slug: slug.to_string(),
        tool_name: meta.name.clone(),
        operation_type: meta.operation_type.clone(),
    })
}

/// 按当前注册表状态同步工具面（enable/disable 共用；插件未装载 = 无事件）。
fn sync_tool_face(mgr: &nemesis_plugins_wasm::registry::PluginManager, slug: &str, on: bool) {
    if let Some(ev) = tool_event_of(mgr, slug, on) {
        notify_tool_sync(ev);
    }
}

/// `wasm.config.get`：实例配置 + 声明面（x-secret 值脱敏不回传）。
pub fn cmd_config_get(slug: &str) -> Result<serde_json::Value, String> {
    let installer = require_installer()?;
    let mgr = &installer.manager;
    let reg = mgr
        .get(slug)
        .ok_or_else(|| format!("plugin not registered: {slug}"))?;
    let cfg = mgr.get_config(slug);
    let secret_keys = &reg.manifest.permissions.x_secret;
    let entries: serde_json::Map<String, serde_json::Value> = cfg
        .entries
        .iter()
        .map(|(k, v)| {
            let masked = secret_keys.iter().any(|s| s == k);
            (
                k.clone(),
                serde_json::Value::String(if masked { "******".into() } else { v.clone() }),
            )
        })
        .collect();
    Ok(serde_json::json!({
        "slug": slug,
        "enabled": cfg.enabled,
        "entries": entries,
        "schema_keys": reg.manifest.config_schema_keys(),
        "secret_keys": secret_keys,
    }))
}

/// `wasm.config.set`：写键（remove=true 走清空；x-secret 拒收——vault 通道）。
pub fn cmd_config_set(data: Option<&serde_json::Value>) -> Result<serde_json::Value, String> {
    let installer = require_installer()?;
    let data = data.ok_or("missing data")?;
    let slug = require_slug(Some(data))?;
    let key = data
        .get("key")
        .and_then(|v| v.as_str())
        .ok_or("key (string) is required")?;
    if data
        .get("remove")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        installer
            .manager
            .unset_config_key(&slug, key)
            .map_err(|e| e.to_string())?;
        return Ok(serde_json::json!({ "slug": slug, "key": key, "removed": true }));
    }
    let value = data
        .get("value")
        .and_then(|v| v.as_str())
        .ok_or("value (string) is required")?;
    installer
        .manager
        .set_config_key(&slug, key, value)
        .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "slug": slug, "key": key, "set": true }))
}

/// `wasm.logs`：宿主日志环形缓冲快照（有界 1024 条 + 丢弃计数）。
pub fn cmd_logs(slug: &str) -> Result<serde_json::Value, String> {
    let installer = require_installer()?;
    let reg = installer
        .manager
        .get(slug)
        .ok_or_else(|| format!("plugin not registered: {slug}"))?;
    let (lines, dropped) = reg.logs.snapshot();
    let rows: Vec<serde_json::Value> = lines
        .iter()
        .map(|l| {
            serde_json::json!({
                "ts_ms": l.ts_ms,
                "level": l.level,
                "message": l.message,
            })
        })
        .collect();
    Ok(serde_json::json!({ "slug": slug, "lines": rows, "dropped": dropped }))
}

/// PluginsHandler `wasm.*` 代理入口（cmd 已剥 `wasm.` 前缀）。
pub async fn handle(
    cmd: &str,
    data: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    match cmd {
        "list" => cmd_list().map(Some),
        "install" => cmd_install(data).await.map(Some),
        "enable" => {
            let slug = require_slug(data.as_ref())?;
            cmd_set_enabled(&slug, true).map(Some)
        }
        "disable" => {
            let slug = require_slug(data.as_ref())?;
            cmd_set_enabled(&slug, false).map(Some)
        }
        "uninstall" => {
            let slug = require_slug(data.as_ref())?;
            cmd_uninstall(&slug).await.map(Some)
        }
        "config.get" => {
            let slug = require_slug(data.as_ref())?;
            cmd_config_get(&slug).map(Some)
        }
        "config.set" => cmd_config_set(data.as_ref()).map(Some),
        "logs" => {
            let slug = require_slug(data.as_ref())?;
            cmd_logs(&slug).map(Some)
        }
        _ => Err(format!("unknown command: plugins.wasm.{cmd}")),
    }
}

#[cfg(test)]
mod tests;
