//! 原生插件库开关（本地 C-ABI 插件 DLL 的启用/停用持久化）。
//!
//! 六个原生插件（plugin_ui / plugin_onnx / eval_monitor_dll / plugin_wasm /
//! plugin_vm / plugin_roaming）共用一份开关文件
//! `<workspace>/config/config.plugin.native.json`（独立
//! 配置文件作为长期记忆；2026-10-22 前身文件名 `plugins_native.json`，
//! 首次读取时惰性迁移改名，见 [`migrate_legacy_file`]）。schema：
//!
//! ```json
//! { "plugins": { "plugin_ui": { "enabled": false } } }
//! ```
//!
//! **缺省语义 = enabled**（文件/键缺席按启用处理——向后兼容既有安装，
//! 首次使用无需迁移）。装载点消费 [`load_enabled`]；Dashboard 开关切
//! 走 [`set_enabled`]（读-改-写，保留其他插件条目）。装载闸与签名策略
//! 在 nemesisbot 侧（`native_plugins` 模块）——本 crate 只管文件真相。
//!
//! 注意与 wasm 插件开关（`plugin_wasm` 模块，`config.plugin.wasm.json`）
//! 的纪律差异：native 有签名验签体系、缺省启用；wasm 组件一律默认
//! 不加载（用户显式开开关才装配）。两份文件互不兼容，勿混用。

use std::collections::BTreeMap;
use std::path::Path;

/// 原生插件目录身份（与 `nemesis_utils::find_plugin_library` 的 base_name
/// 同源——id 即库探测名，Windows 自带连字符变体回退）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativePluginId {
    /// WebView UI / 系统托盘（desktop 子系统）。
    PluginUi,
    /// ONNX 嵌入推理（强化记忆）。
    PluginOnnx,
    /// eval 沙盒监控 DLL。
    EvalMonitor,
    /// WASM 插件宿主壳。
    PluginWasm,
    /// VM 显示通道（sandbox VM 画面中继到 Dashboard；C-ABI 解冻首例）。
    PluginVm,
    /// QUIC 中继/名册传输面（iroh 组网；C-ABI 解冻第二例，新式交互插件）。
    PluginRoaming,
}

impl NativePluginId {
    /// 全量目录（页面清单顺序 = 此处顺序）。
    pub const ALL: &'static [NativePluginId] = &[
        NativePluginId::PluginOnnx,
        NativePluginId::PluginUi,
        NativePluginId::EvalMonitor,
        NativePluginId::PluginWasm,
        NativePluginId::PluginVm,
        NativePluginId::PluginRoaming,
    ];

    /// 库探测名（find_plugin_library base_name）。
    pub fn id(self) -> &'static str {
        match self {
            NativePluginId::PluginUi => "plugin_ui",
            NativePluginId::PluginOnnx => "plugin_onnx",
            NativePluginId::EvalMonitor => "eval_monitor_dll",
            NativePluginId::PluginWasm => "plugin_wasm",
            NativePluginId::PluginVm => "plugin_vm",
            NativePluginId::PluginRoaming => "plugin_roaming",
        }
    }

    /// WSAPI/CLI 字符串解析（未知 id = None）。
    pub fn from_id(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|p| p.id() == s)
    }

    /// 开关是否即时生效（false = 重启后生效）。目前只有 eval 监控是
    /// 事件驱动装载（每次 eval 会话开始查闸）——其余五个都在 gateway
    /// 装配期一次性消费。
    pub fn immediate_effect(self) -> bool {
        matches!(self, NativePluginId::EvalMonitor)
    }
}

/// 开关文件路径（`<config_dir>/config.plugin.native.json`）。
pub fn plugins_native_config_path(config_dir: &Path) -> std::path::PathBuf {
    config_dir.join("config.plugin.native.json")
}

/// 旧版开关文件路径（2026-10-22 前的 `plugins_native.json`，仅迁移用）。
const LEGACY_FILE_NAME: &str = "plugins_native.json";

/// 惰性迁移：新版文件缺席且旧版在场 → 旧内容原样落到新版文件名，删除旧
/// 文件。失败不阻塞读路径（WARN + 留守旧位，下次再试）。
///
/// 写侧（`set_enabled`）自带罩锁；本函数在锁内调用，读侧（`load_enabled`）
/// 单读不改盘，重复调用靠「新版在场即短路」幂等。
fn migrate_legacy_file(path: &Path) {
    let Some(parent) = path.parent() else { return };
    let legacy = parent.join(LEGACY_FILE_NAME);
    if !legacy.exists() {
        return; // 旧文件不在场 = 无可迁移
    }
    if path.exists() {
        // 新版已在场（并发或先前迁移过写了一半）——旧文件作废弃副本清理。
        let _ = std::fs::remove_file(&legacy);
        return;
    }
    // 同目录 rename = 原子迁移（BUG-20261025-45：旧「读+写+删」中段崩溃
    // 新旧两份全丢；rename 失败源文件原位留守，下次读重试）。
    match std::fs::rename(&legacy, path) {
        Ok(()) => tracing::info!(
            from = %legacy.display(),
            to = %path.display(),
            "[NativePlugin] 开关文件已迁移为规范命名"
        ),
        Err(e) => tracing::warn!(
            from = %legacy.display(),
            to = %path.display(),
            error = %e,
            "[NativePlugin] 开关文件迁移未完成（留守旧位，下次重试）"
        ),
    }
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct SwitchFile {
    #[serde(default)]
    plugins: BTreeMap<String, SwitchEntry>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SwitchEntry {
    #[serde(default = "default_true")]
    enabled: bool,
}

fn default_true() -> bool {
    true
}

/// 读开关。文件/键缺席 = 启用（向后兼容缺省）；非法 JSON = 启用 + 保守
/// 放行（开关文件损坏不应肢解产品功能——损坏本身由写入方负责，读侧诚实
/// 放行并让日志可见性留给调用方）。首次调用完成旧文件名惰性迁移。
pub fn load_enabled(config_dir: &Path, id: NativePluginId) -> bool {
    let path = plugins_native_config_path(config_dir);
    migrate_legacy_file(&path);
    let Ok(content) = nemesis_utils::fsutil::read_config_string(&path) else {
        return true;
    };
    let Ok(file) = serde_json::from_str::<SwitchFile>(&content) else {
        tracing::warn!(
            path = %path.display(),
            "[NativePlugin] 开关文件解析失败，按全启用处理"
        );
        return true;
    };
    file.plugins.get(id.id()).map(|e| e.enabled).unwrap_or(true)
}

/// 写开关（读-改-写整文件；文件不存在 = 只写目标键。保留其他插件条目）。
/// 原子落盘（`write_file_atomic`，BUG-20261025-45：直写撕裂会让下次 RMW
/// 以空底重建、静默抹掉全部兄弟插件条目）；罩 config 家族写串行锁
/// （与 config 写臂族互斥，同 approval_rules 惯例）。
pub fn set_enabled(config_dir: &Path, id: NativePluginId, enabled: bool) -> Result<(), String> {
    let _serial = crate::config_write_lock();
    let path = plugins_native_config_path(config_dir);
    migrate_legacy_file(&path);
    let mut file = nemesis_utils::fsutil::read_config_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str::<SwitchFile>(&c).ok())
        .unwrap_or_default();
    file.plugins
        .insert(id.id().to_string(), SwitchEntry { enabled });
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建配置目录失败：{e}"))?;
    }
    let json = serde_json::to_string_pretty(&file).map_err(|e| format!("序列化失败：{e}"))?;
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), (json + "\n").as_bytes(), 0o600)
}
