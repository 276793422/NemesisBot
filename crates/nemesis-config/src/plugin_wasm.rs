//! WASM 插件加载开关 + 实例配置（`<config_dir>/config.plugin.wasm.json`）。
//!
//! schema：
//!
//! ```json
//! { "panel_llm": { "enabled": false },
//!   "plugins": { "<slug>": { "enabled": false, "config": {} } } }
//! ```
//!
//! `panel_llm.enabled`（2026-11-01 决策 C）：面板帧 llm-complete 总闸，
//! **缺省 false**——插件 manifest 还须 `x-llm = true` 声明（双闸，任一缺 =
//! 帧内恒 unavailable）。每调用现读无缓存（[`runtime_enabled`] 同款热语义）：
//! 开关翻转即时生效，无需重启。选本文件而非 config.json 做真相源：本文件
//! 已有读改写函数 + config 写锁 + fail-closed 纪律，且不受 typed save_config
//! 影响（config.json 顶层未知段会被 typed 保存静默丢弃）。
//!
//! **缺省语义 = enabled=false**（文件/键缺席按未启用处理——与 native 插件
//! 的缺省启用相反：wasm 组件无论 sha256/签名/信任态如何一律默认不加载，
//! 用户在 Dashboard 打开加载开关才装配；开关位即长期记忆，重启后保持）。
//!
//! 旧 per-slug 实例文件（`config/plugins/<slug>.json`，enabled 缺省 true）
//! 退役：读路径首次命中且旧文件在场 → 并入本文件（**enabled 强制 false**
//! ——新纪律全量接管；config entries 保留）→ 删旧文件，见
//! [`migrate_legacy_instances`]。
//!
//! 损坏纪律 = **fail-closed**（与 native 插件的 fail-open 相反）：开关文件
//! 损坏时全部视为未启用 + WARN——wasm 的纪律是「没明确开就不加载」，损坏
//! 不应导致未审阅的组件被装配。
//!
//! 注意与 native 插件开关（`plugins_native` 模块，`config.plugin.native.json`）
//! 互不兼容，勿混用。

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// 单个 wasm 插件的开关与实例配置。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct WasmSwitch {
    /// 加载开关（缺省 false = 不加载）。
    #[serde(default)]
    pub enabled: bool,
    /// 普通配置键值（仅 manifest config-schema 声明键；`x-` 前缀键拒绝设置）。
    #[serde(default)]
    pub config: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, Default)]
struct SwitchFile {
    /// 面板 LLM 总闸（缺省缺席 = false；`skip_serializing_if` 保老文件
    /// 字节稳定——开关从未打开过就不落键）。
    #[serde(default, skip_serializing_if = "panel_llm_off")]
    panel_llm: PanelLlmSwitch,
    #[serde(default)]
    plugins: BTreeMap<String, WasmSwitch>,
}

/// 面板帧 llm-complete 总闸（manifest `x-llm` 声明之外的授权面）。
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, Default)]
struct PanelLlmSwitch {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    enabled: bool,
}

/// `skip_serializing_if` 谓词：panel_llm 整段关（= 缺省）时不序列化。
fn panel_llm_off(s: &PanelLlmSwitch) -> bool {
    !s.enabled
}

/// 面板帧 llm-complete 总闸现读（每调用读盘，无缓存——开关翻转即时生效；
/// 文件缺席/损坏 = false，fail-closed 与整个文件同一纪律）。
#[must_use]
pub fn panel_llm_enabled(config_dir: &Path) -> bool {
    read_switch_file(config_dir).panel_llm.enabled
}

/// 写面板 LLM 总闸（读-改-写；罩 config 家族写串行锁）。
pub fn set_panel_llm(config_dir: &Path, enabled: bool) -> Result<(), String> {
    let _serial = crate::config_write_lock();
    let mut file = read_switch_file(config_dir);
    file.panel_llm.enabled = enabled;
    write_switch_file(config_dir, &file)
}

/// 旧 per-slug 实例文件形态（`config/plugins/<slug>.json`，已退役）。
#[derive(Debug, Default, serde::Deserialize)]
struct LegacyInstanceFile {
    #[serde(default)]
    entries: BTreeMap<String, String>,
}

/// 开关文件路径（`<config_dir>/config.plugin.wasm.json`）。
pub fn plugin_wasm_config_path(config_dir: &Path) -> std::path::PathBuf {
    config_dir.join("config.plugin.wasm.json")
}

/// WASM 插件子系统**运行期**总闸（2026-11-01 BUG 修复：宿主壳/总闸开关
/// 此前只在启动装配期一次性消费——运行中翻关后 WSAPI 面板/guest 命令/
/// flow 引擎/agent 工具照常生效，刷新 UI 面板仍在）。
///
/// 热语义：任一真相源关 = 服务面拒绝，开关改回**即时恢复**（每调用现读，
/// 无缓存）。真相源两件：
/// ① `<home>/config.json` 的 `plugins.wasm.enabled`（缺省 true）；home 与
///    workspace 同级是全局约定（`workspace_root.parent()` = home）。
/// ② `<config_dir>/config.plugin.native.json` 的 `plugin_wasm.enabled`
///    （宿主壳闸，缺省 true）。壳的**装载/卸载**仍是启动期一次性
///    （`NativePluginId::immediate_effect` 语义不变）——本闸只裁已装
///    子系统对外服务面。
pub fn runtime_enabled(workspace_root: &Path) -> bool {
    let config_json = workspace_root
        .parent()
        .map(|home| home.join("config.json"))
        .unwrap_or_else(|| workspace_root.join("config.json"));
    // BUG-20261103-334：走 read_config_string 剥 BOM（记事本保存的
    // config.json）——旧形态裸 read_to_string 时 BOM → serde_json parse
    // 失败 → 本源按缺省 true 跳过，用户 `plugins.wasm.enabled=false` 被静默
    // 忽略。与 197/246 行同源纪律。
    if let Ok(raw) = nemesis_utils::fsutil::read_config_string(&config_json)
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw)
    {
        let on = v
            .pointer("/plugins/wasm/enabled")
            .and_then(|x| x.as_bool())
            .unwrap_or(true);
        if !on {
            return false;
        }
    }
    crate::plugins_native::load_enabled(
        &workspace_root.join("config"),
        crate::plugins_native::NativePluginId::PluginWasm,
    )
}

/// 旧实例文件目录（`<config_dir>/plugins/`，仅迁移用）。
const LEGACY_INSTANCE_DIR: &str = "plugins";

/// 全量读开关（列出/启动装载用）。文件缺席 = 空；损坏 = 空（fail-closed）
/// + WARN。迁移旧实例文件后把合并结果写回。
///
/// BUG-20261103-333：迁移写（并入主文件的 RMW + 删旧文件）全程罩
/// config_write_lock——旧形态读路径迁移写零锁，与 set_enabled/set_config/
/// remove（全程罩锁）并发互相以旧底覆盖；开关写丢 = 用户切换静默回退，
/// 迁移写丢更重（旧实例文件已删、数据不可恢复）。本函数非纯读（可能迁移
/// 落盘），不入「读路径不入锁」豁免。
pub fn load_switches(config_dir: &Path) -> BTreeMap<String, WasmSwitch> {
    let _serial = crate::config_write_lock();
    let mut file = read_switch_file(config_dir);
    if migrate_legacy_instances(config_dir, &mut file) {
        // 读路径的迁移写失败不阻塞读（WARN 在写函数内部；下次读再试）。
        let _ = write_switch_file(config_dir, &file);
    }
    file.plugins
}

/// 读单个 slug 的开关（文件无该 slug 且旧实例文件在场 → 先迁移再读）。
/// 锁纪律同 load_switches（BUG-20261103-333：迁移写全程罩锁）。
pub fn switch(config_dir: &Path, slug: &str) -> WasmSwitch {
    let _serial = crate::config_write_lock();
    let mut file = read_switch_file(config_dir);
    if !file.plugins.contains_key(slug) && migrate_legacy_instances(config_dir, &mut file) {
        let _ = write_switch_file(config_dir, &file);
    }
    file.plugins.get(slug).cloned().unwrap_or_default()
}

/// 写加载开关（读-改-写；罩 config 家族写串行锁，同 native 模块惯例）。
pub fn set_enabled(config_dir: &Path, slug: &str, enabled: bool) -> Result<(), String> {
    let _serial = crate::config_write_lock();
    let mut file = read_switch_file(config_dir);
    let entry = file.plugins.entry(slug.to_string()).or_default();
    entry.enabled = enabled;
    write_switch_file(config_dir, &file)
}

/// 写实例配置（整体替换该 slug 的 config 表；开关位不动）。
pub fn set_config(
    config_dir: &Path,
    slug: &str,
    config: BTreeMap<String, serde_json::Value>,
) -> Result<(), String> {
    let _serial = crate::config_write_lock();
    let mut file = read_switch_file(config_dir);
    let entry = file.plugins.entry(slug.to_string()).or_default();
    entry.config = config;
    write_switch_file(config_dir, &file)
}

/// 删除 slug 条目（uninstall 收尾；slug 不在场也视为成功——幂等）。
pub fn remove(config_dir: &Path, slug: &str) -> Result<(), String> {
    let _serial = crate::config_write_lock();
    let mut file = read_switch_file(config_dir);
    if file.plugins.remove(slug).is_none() {
        return Ok(());
    }
    write_switch_file(config_dir, &file)
}

/// 读开关文件；缺席/损坏 = 空 + WARN（fail-closed）。
fn read_switch_file(config_dir: &Path) -> SwitchFile {
    let path = plugin_wasm_config_path(config_dir);
    let Ok(content) = nemesis_utils::fsutil::read_config_string(&path) else {
        return SwitchFile::default();
    };
    match serde_json::from_str::<SwitchFile>(&content) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "[WasmPlugin] 开关文件解析失败，全部按未启用处理（fail-closed）"
            );
            SwitchFile::default()
        }
    }
}

/// 原子写（BUG-20261025-45 起走 `nemesis_utils::write_file_atomic`：唯一
/// 临时名 + `sync_all` 先于 rename + 失败清理——旧手搓 tmp 名只到毫秒精度，
/// 读路径迁移写不罩锁时同毫秒并发可撞名，且无 fsync 崩溃可撕裂目标文件）。
fn write_switch_file(config_dir: &Path, file: &SwitchFile) -> Result<(), String> {
    let path = plugin_wasm_config_path(config_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建配置目录失败：{e}"))?;
    }
    let json = serde_json::to_string_pretty(file).map_err(|e| format!("序列化失败：{e}"))?;
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), (json + "\n").as_bytes(), 0o600)
}

/// 旧实例文件并入（一次性惰性迁移）：扫 `<config_dir>/plugins/*.json`，
/// 主文件无该 slug 的 → 并入（**enabled 强制 false**，entries 转 config
/// 字符串值保留）+ 删旧文件；主文件已有的 slug（用户已用新纪律表态）→
/// 旧文件直接作废弃副本删除。返回是否有任何变更。
fn migrate_legacy_instances(config_dir: &Path, file: &mut SwitchFile) -> bool {
    let legacy_dir = config_dir.join(LEGACY_INSTANCE_DIR);
    let Ok(entries) = std::fs::read_dir(&legacy_dir) else {
        return false;
    };
    // read_dir 顺序是平台实现细节（NTFS 字典序 vs ext4 哈希序）——显式排序。
    let mut names: BTreeSet<String> = BTreeSet::new();
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.ends_with(".json") {
            names.insert(name);
        }
    }
    let mut changed = false;
    for name in names {
        let legacy_path = legacy_dir.join(&name);
        let slug = name.trim_end_matches(".json").to_string();
        let Ok(content) = nemesis_utils::fsutil::read_config_string(&legacy_path) else {
            continue;
        };
        let legacy = serde_json::from_str::<LegacyInstanceFile>(&content).unwrap_or_default();
        if !file.plugins.contains_key(&slug) {
            let config: BTreeMap<String, serde_json::Value> = legacy
                .entries
                .into_iter()
                .map(|(k, v)| (k, serde_json::Value::String(v)))
                .collect();
            file.plugins.insert(
                slug.clone(),
                WasmSwitch {
                    enabled: false,
                    config,
                },
            );
        }
        match std::fs::remove_file(&legacy_path) {
            Ok(()) => tracing::info!(
                slug = %slug,
                from = %legacy_path.display(),
                "[WasmPlugin] 旧实例文件已并入 config.plugin.wasm.json（enabled 强制 false）"
            ),
            Err(e) => tracing::warn!(
                slug = %slug,
                path = %legacy_path.display(),
                error = %e,
                "[WasmPlugin] 旧实例文件删除失败（合并已生效，残留副本下次重试）"
            ),
        }
        changed = true;
    }
    if changed {
        // 目录空了顺手清掉（失败不致命——残留空目录下次迁移再试）。
        let _ = std::fs::remove_dir(&legacy_dir);
    }
    changed
}
