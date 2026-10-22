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
//! →lockfile→落位全走 nemesis-plugins-wasm::install 单一真相源，本
//! 模块零旁路）；config.get 按 manifest x-secret 脱敏（值不回传 Dashboard）。
//!
//! 加载纪律（2026-10-22）：安装 ≠ 加载——`list` 的 `enabled` =
//! `config.plugin.wasm.json` 开关（加载意愿，重启保持），`loaded` =
//! 注册表在场（运行态）。开关走 [`PluginInstaller::set_enabled`]：
//! enable = 装配注册（新装/升级开关关着时注册表缺席），disable = 真摘除。

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

/// 2026-10-22 样板工程批 4：循环策略面同步事件（install/enable/disable/
/// uninstall 时把策略槽接管变化同步进当前 AgentLoop；启动装载走
/// agent_factory 的 `register_policy_bridges`，两路同语义——hook 侧按
/// 注册表现状整体对账三槽，事件只当触发器 + 日志上下文）。
#[derive(Debug, Clone)]
pub struct PolicySyncEvent {
    /// true = 接管装桥，false = 摘桥回内置哨兵。
    pub add: bool,
    pub slug: String,
    /// 事件涉及策略点（kebab 导出名：compact / judge-final /
    /// validate-args；摘桥方在注册表摘除**前**抓取）。
    pub points: Vec<String>,
}

type PolicySyncHook = Arc<dyn Fn(PolicySyncEvent) + Send + Sync>;
static POLICY_SYNC_HOOK: std::sync::OnceLock<PolicySyncHook> = std::sync::OnceLock::new();

/// gateway init_agent 注入策略面同步回调（幂等失败 = 重复注入，首个胜出）。
pub fn set_policy_sync_hook(hook: PolicySyncHook) {
    let _ = POLICY_SYNC_HOOK.set(hook);
}

/// 触发策略面同步（hook 未注入 = 无 loop 可同步（CLI/测试形态），静默略过）。
pub fn notify_policy_sync(ev: PolicySyncEvent) {
    if let Some(hook) = POLICY_SYNC_HOOK.get() {
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

/// 已装载行投影（tool/policy 面快照齐全）。`enabled` 取开关记录——注册表
/// 在场只说明**已加载**，两字段语义分离（2026-10-22：安装 ≠ 加载）。
fn row_of(
    reg: &nemesis_plugins_wasm::registry::RegisteredPlugin,
    enabled: bool,
) -> serde_json::Value {
    let m = &reg.manifest;
    serde_json::json!({
        "slug": m.slug,
        "name": m.name,
        "version": m.version,
        "kind": m.kind.as_str(),
        "trust": reg.trust.as_str(),
        "enabled": enabled,
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
        "policy": reg.policy_meta.as_ref().map(|pm| serde_json::json!({
            "compact": pm.compact,
            "final_judge": pm.final_judge,
            "validation": pm.validation,
        })),
        "data_dir": reg.data_dir.display().to_string(),
    })
}

/// 未装载行投影（新装缺省 / 升级开关关 / lockfile 有记录载荷缺席）。
fn pending_row(
    slug: &str,
    name: &str,
    version: &str,
    trust: &str,
    enabled: bool,
    extra: serde_json::Value,
) -> serde_json::Value {
    let mut row = serde_json::json!({
        "slug": slug,
        "name": name,
        "version": version,
        "kind": serde_json::Value::Null,
        "trust": trust,
        "enabled": enabled,
        "loaded": false,
    });
    let obj = row.as_object_mut().expect("row literal is object");
    if let Some(map) = extra.as_object() {
        for (k, v) in map {
            obj.insert(k.clone(), v.clone());
        }
    }
    row
}

/// 开关记录全量（`config.plugin.wasm.json`；读路径顺带触发旧实例文件惰性迁移）。
fn switch_map(
    mgr: &nemesis_plugins_wasm::registry::PluginManager,
) -> std::collections::BTreeMap<String, nemesis_config::plugin_wasm::WasmSwitch> {
    nemesis_config::plugin_wasm::load_switches(&mgr.workspace_root().join("config"))
}

/// `wasm.list`：三源合流——①盘上载荷发现（扫 `plugins_wasm/` 含
/// `plugin.toml` 的子目录，含手工拷入）②lockfile 记录（载荷被手工删掉的
/// 可见性兜底）③注册表（loaded 与 tool/policy 面快照）。`enabled` 恒取
/// 开关记录，`trust` 无 lockfile 记录 = `unknown`（手工拷入，enable 时
/// 补跑装配漏斗）。
pub fn cmd_list() -> Result<serde_json::Value, String> {
    list_impl(require_installer()?.as_ref())
}

/// list 本体（纯函数形态——测试直注 installer，绕全局槽 OnceLock 首胜
/// 语义：并行用例共享槽 + 首 test 的 TempDir 落盘即删 = 后到者读到空目录）。
fn list_impl(installer: &PluginInstaller) -> Result<serde_json::Value, String> {
    let mgr = &installer.manager;
    let switches = switch_map(mgr);
    let enabled_of = |slug: &str| switches.get(slug).is_some_and(|sw| sw.enabled);
    let lockfile = read_lockfile(mgr);

    let mut rows: Vec<serde_json::Value> = Vec::new();
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    // ① 发现面（字典序确定性——read_dir 顺序是平台实现细节）。
    let mut discovered: Vec<std::path::PathBuf> = match std::fs::read_dir(mgr.plugins_dir()) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.join("plugin.toml").is_file())
            .collect(),
        Err(_) => Vec::new(),
    };
    discovered.sort();
    for pdir in discovered {
        let slug = pdir
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        seen.insert(slug.clone());
        let reg = mgr.get(&slug);
        let trust = lockfile
            .as_ref()
            .and_then(|lf| lf.plugins.get(&slug))
            .map(|e| e.trust.clone())
            .unwrap_or_else(|| "unknown".to_string());
        let mut row = match nemesis_plugins_wasm::manifest::PluginManifest::load_from_dir(&pdir) {
            Ok((m, _)) => serde_json::json!({
                "slug": slug,
                "name": m.name,
                "version": m.version,
                "kind": m.kind.as_str(),
                "trust": trust,
                "enabled": enabled_of(&slug),
                "loaded": reg.is_some(),
                "wasm_sha256": m.wasm_sha256,
                "min_tier": m.min_tier,
                "takes_over": m.takeover,
                "egress": m.permissions.egress,
                "x_secret": m.permissions.x_secret,
                "data_dir": mgr.plugin_data_dir(&slug).display().to_string(),
            }),
            Err(e) => pending_row(
                &slug,
                &slug,
                "",
                &trust,
                enabled_of(&slug),
                serde_json::json!({ "error": format!("manifest 解析失败: {e}") }),
            ),
        };
        if let Some(reg) = reg.as_ref() {
            let obj = row.as_object_mut().expect("row literal is object");
            if let Some(t) = reg.tool_meta.as_ref() {
                obj.insert(
                    "tool".into(),
                    serde_json::json!({
                        "name": t.name,
                        "base": t.base,
                        "title": t.title,
                        "description": t.description,
                        "operation_type": t.operation_type,
                        "min_tier": t.min_tier,
                    }),
                );
            }
            if let Some(pm) = reg.policy_meta.as_ref() {
                obj.insert(
                    "policy".into(),
                    serde_json::json!({
                        "compact": pm.compact,
                        "final_judge": pm.final_judge,
                        "validation": pm.validation,
                    }),
                );
            }
        }
        rows.push(row);
    }

    // ② lockfile 记录但载荷缺席（盘上被手工删了；enabled 位照读——
    // 用户开关还开着时 enable 路径的诚实报错靠这行可见）。
    if let Some(lf) = lockfile.as_ref() {
        for (slug, e) in &lf.plugins {
            if !seen.contains(slug) {
                rows.push(pending_row(
                    slug,
                    slug,
                    &e.version,
                    &e.trust,
                    enabled_of(slug),
                    serde_json::json!({
                        "wasm_sha256": e.wasm_sha256,
                        "signed_by": e.signed_by,
                        "error": "载荷目录缺失（lockfile 记录仍在；重装或卸载）",
                    }),
                ));
            }
        }
    }
    Ok(serde_json::json!({ "plugins": rows }))
}

/// 读 lockfile（损坏/缺席 = None——列表降级为发现面，不炸）。
fn read_lockfile(
    mgr: &nemesis_plugins_wasm::registry::PluginManager,
) -> Option<nemesis_plugins_wasm::install::PluginLockfile> {
    let s = std::fs::read_to_string(mgr.plugins_dir().join("lockfile.json")).ok()?;
    serde_json::from_str(&s).ok()
}

/// `wasm.install`（运行期装配唯一入口 = install 漏斗；含升级重跑）。
/// 返回行带 `loaded` 位：新装 = false（默认不加载，用户开开关才装配）；
/// 升级且开关原本开着 = true（热升级）。
pub async fn cmd_install(data: Option<serde_json::Value>) -> Result<serde_json::Value, String> {
    let installer = require_installer()?;
    let data = data.ok_or("missing data")?;
    let source_dir = data
        .get("source_dir")
        .and_then(|v| v.as_str())
        .ok_or("source_dir (string) is required")?;
    validate_source_dir(source_dir)?;
    let allow_unsigned = data
        .get("allow_unsigned")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let outcome = installer
        .install(Path::new(source_dir), allow_unsigned)
        .await
        .map_err(|e| e.to_string())?;
    // 面同步事件只在「本次已装载」（升级且开关原开）时发：新装/开关关的
    // 升级注册表缺席，loop 面无此插件可清（此前 disable/卸载路径已摘净）。
    // 工具事件恒发（方向由 install_sync_event 决策——tool→observer 化升级
    // 走前缀摘除，2026-09-30 复查 #4 纪律）；策略事件恒发（hook 收到即
    // 全槽对账，换形旧接管不留幽灵）。
    if let Some(reg) = outcome.registered.as_ref() {
        notify_tool_sync(install_sync_event(
            reg.manifest.kind.as_str(),
            &outcome.slug,
            reg.tool_meta
                .as_ref()
                .map(|m| (m.name.as_str(), m.operation_type.as_str())),
            true,
        ));
        notify_policy_sync(policy_sync_event(
            reg.manifest.kind.as_str(),
            &outcome.slug,
            reg.policy_meta.as_ref(),
            true,
        ));
    }
    let row = match outcome.registered.as_ref() {
        Some(reg) => row_of(reg, true),
        None => pending_row(
            &outcome.slug,
            &outcome.slug,
            &outcome.version,
            outcome.trust.as_str(),
            false,
            serde_json::json!({ "note": "已安装，默认不加载——在插件页打开加载开关" }),
        ),
    };
    Ok(serde_json::json!({
        "installed": true,
        "loaded": outcome.registered.is_some(),
        "plugin": row,
    }))
}

/// source_dir 入参校验（WSAPI 层；CLI 保持宽容——2026-09-30 复查 #10）。
/// 拒绝：空串 / 相对路径（install 漏斗 canonicalize 会把它钉到 gateway
/// 进程 cwd，与操作员直觉不符）/ UNC 前缀（`\\`、`//` 开头——网络盘/
/// 设备路径面，扫描与落位语义未定义）。绝对路径判定交给
/// [`std::path::Path::is_absolute`]（Windows 盘符形态覆盖）。
fn validate_source_dir(raw: &str) -> Result<(), String> {
    if raw.trim().is_empty() {
        return Err("source_dir 不能为空".to_string());
    }
    if !Path::new(raw).is_absolute() {
        return Err(format!("source_dir 必须为绝对路径: {raw}"));
    }
    if raw.starts_with("\\\\") || raw.starts_with("//") {
        return Err(format!("source_dir 不接受 UNC/网络路径: {raw}"));
    }
    Ok(())
}

/// 安装完成后的工具面同步事件决策（纯函数，独立可测；2026-09-30 复查 #4）。
/// `tool + 有 meta + 启用` → add=true 注册；其余一律 add=false——observer
/// 化升级 / 缺 meta / 禁用升级都走前缀摘除路径，把 loop 里残留的旧版工具
/// 清掉（hook 的 remove 按 `plugin.<slug>.` 前缀整面摘除，tool_name 不用）。
fn install_sync_event(
    kind: &str,
    slug: &str,
    tool: Option<(&str, &str)>,
    enabled: bool,
) -> ToolSyncEvent {
    match (kind, tool, enabled) {
        ("tool", Some((name, op)), true) => ToolSyncEvent {
            add: true,
            slug: slug.to_string(),
            tool_name: name.to_string(),
            operation_type: op.to_string(),
        },
        _ => ToolSyncEvent {
            add: false,
            slug: slug.to_string(),
            tool_name: String::new(),
            operation_type: String::new(),
        },
    }
}

/// 安装完成后的策略面同步事件决策（纯函数，与 [`install_sync_event`] 同构）。
/// `policy + 有 meta + 启用` → add=true 装桥；其余一律 add=false 摘桥
/// （升级换形 / 禁用升级都回内置哨兵，旧接管不留幽灵）。
fn policy_sync_event(
    kind: &str,
    slug: &str,
    policy: Option<&nemesis_plugins_wasm::registry::PolicyMetaSnapshot>,
    enabled: bool,
) -> PolicySyncEvent {
    match (kind, policy, enabled) {
        ("policy", Some(pm), true) => PolicySyncEvent {
            add: true,
            slug: slug.to_string(),
            points: policy_points_of(pm),
        },
        _ => PolicySyncEvent {
            add: false,
            slug: slug.to_string(),
            points: policy.map(policy_points_of).unwrap_or_default(),
        },
    }
}

/// `PolicyMetaSnapshot` → kebab 导出名列表（事件投影用）。
fn policy_points_of(pm: &nemesis_plugins_wasm::registry::PolicyMetaSnapshot) -> Vec<String> {
    pm.points()
        .iter()
        .map(|p| p.export_name().to_string())
        .collect()
}

fn require_slug(data: Option<&serde_json::Value>) -> Result<String, String> {
    data.and_then(|d| d.get("slug"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "slug (string) is required".to_string())
}

/// `wasm.enable` / `wasm.disable`（开关 = 加载闸，热生效；记录落
/// `config.plugin.wasm.json`，重启保持）。enable = 装配注册（lockfile 有
/// 记录走原路，手工拷入补跑装配漏斗含审批卡）；disable = 真摘除。
pub async fn cmd_set_enabled(slug: &str, on: bool) -> Result<serde_json::Value, String> {
    let installer = require_installer()?;
    let loaded = if on {
        let loaded = installer
            .set_enabled(slug, true)
            .await
            .map_err(|e| e.to_string())?;
        if loaded.is_some() {
            // 装配成功：注册表现在在场 → 面投影 add（工具 hook 先按 slug
            // 前缀整面摘旧再挂新；策略 hook 收到即全槽对账）。
            sync_tool_face(&installer.manager, slug, true);
            sync_policy_face(&installer.manager, slug, true);
        }
        // loaded=None = 已在册幂等（面已在，无事件）。
        loaded
    } else {
        // 摘除前先抓面事件（注册表摘除后拿不到元数据）；hook 侧语义 =
        // 工具前缀整面摘 + 策略全槽回内置。
        let tool_ev = tool_event_of(&installer.manager, slug, false);
        let policy_ev = policy_event_of(&installer.manager, slug, false);
        installer
            .set_enabled(slug, false)
            .await
            .map_err(|e| e.to_string())?;
        if let Some(ev) = tool_ev {
            notify_tool_sync(ev);
        }
        if let Some(ev) = policy_ev {
            notify_policy_sync(ev);
        }
        None
    };
    Ok(serde_json::json!({
        "slug": slug,
        "enabled": on,
        "loaded": loaded.is_some(),
    }))
}

/// `wasm.uninstall`（注销 + 删载荷 + lockfile 摘除；数据目录保留）。
pub async fn cmd_uninstall(slug: &str) -> Result<serde_json::Value, String> {
    let installer = require_installer()?;
    // 注销前抓工具面信息（注册表摘除后拿不到 tool_name/op）。
    let sync = tool_event_of(&installer.manager, slug, false);
    // 同理先抓策略面事件（注册表摘除后拿不到接管点清单）。
    let policy_ev = policy_event_of(&installer.manager, slug, false);
    let existed = nemesis_plugins_wasm::install::uninstall(installer.manager.as_ref(), slug)
        .await
        .map_err(|e| e.to_string())?;
    if existed {
        if let Some(ev) = sync {
            notify_tool_sync(ev);
        }
        if let Some(ev) = policy_ev {
            notify_policy_sync(ev);
        }
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

/// 从注册表投影策略面同步事件（`kind=policy` 且已装载才产生；`add=false`
/// 用于禁用/注销——须在注册表摘除或禁用落盘前调用）。
fn policy_event_of(
    mgr: &nemesis_plugins_wasm::registry::PluginManager,
    slug: &str,
    add: bool,
) -> Option<PolicySyncEvent> {
    let reg = mgr.get(slug)?;
    if *reg.manifest.kind != nemesis_plugins_wasm::manifest::PluginKind::Policy {
        return None;
    }
    let pm = reg.policy_meta.as_ref()?;
    Some(PolicySyncEvent {
        add,
        slug: slug.to_string(),
        points: policy_points_of(pm),
    })
}

/// 按当前注册表状态同步策略面（enable/disable 共用；非 policy 型 = 无事件）。
fn sync_policy_face(mgr: &nemesis_plugins_wasm::registry::PluginManager, slug: &str, on: bool) {
    if let Some(ev) = policy_event_of(mgr, slug, on) {
        notify_policy_sync(ev);
    }
}

/// `wasm.config.get`：实例配置 + 声明面（x-secret 值脱敏不回传）。
/// 未加载也可读——schema/x-secret 取盘上 manifest，配置取开关记录
/// （配置与装载解耦：开关开着时装载生效）。
pub fn cmd_config_get(slug: &str) -> Result<serde_json::Value, String> {
    let installer = require_installer()?;
    let mgr = &installer.manager;
    let manifest = mgr
        .disk_manifest(slug)
        .map_err(|e| format!("plugin not available: {slug}: {e}"))?;
    let sw = mgr.get_config(slug);
    let secret_keys = &manifest.permissions.x_secret;
    let entries: serde_json::Map<String, serde_json::Value> = sw
        .config
        .iter()
        .map(|(k, v)| {
            let masked = secret_keys.iter().any(|s| s == k);
            let shown = match v {
                serde_json::Value::String(s) => serde_json::Value::String(s.clone()),
                other => other.clone(),
            };
            (
                k.clone(),
                if masked {
                    serde_json::Value::String("******".into())
                } else {
                    shown
                },
            )
        })
        .collect();
    Ok(serde_json::json!({
        "slug": slug,
        "enabled": sw.enabled,
        "loaded": mgr.get(slug).is_some(),
        "entries": entries,
        "schema_keys": manifest.config_schema_keys(),
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
            cmd_set_enabled(&slug, true).await.map(Some)
        }
        "disable" => {
            let slug = require_slug(data.as_ref())?;
            cmd_set_enabled(&slug, false).await.map(Some)
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
