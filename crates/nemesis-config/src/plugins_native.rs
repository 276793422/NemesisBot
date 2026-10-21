//! 原生插件库开关（本地 C-ABI 插件 DLL 的启用/停用持久化）。
//!
//! 四个原生插件（plugin_ui / plugin_onnx / eval_monitor_dll / plugin_wasm）
//! 共用一份开关文件 `<workspace>/config/plugins_native.json`（用户要求的
//! 「独立配置文件作为长期记忆」）。schema：
//!
//! ```json
//! { "plugins": { "plugin_ui": { "enabled": false } } }
//! ```
//!
//! **缺省语义 = enabled**（文件/键缺席按启用处理——向后兼容既有安装，
//! 首次使用无需迁移）。装载点消费 [`load_enabled`]；Dashboard 开关切
//! 走 [`set_enabled`]（读-改-写，保留其他插件条目）。装载闸与签名策略
//! 在 nemesisbot 侧（`native_plugins` 模块）——本 crate 只管文件真相。

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
}

impl NativePluginId {
    /// 全量目录（页面清单顺序 = 此处顺序）。
    pub const ALL: &'static [NativePluginId] = &[
        NativePluginId::PluginOnnx,
        NativePluginId::PluginUi,
        NativePluginId::EvalMonitor,
        NativePluginId::PluginWasm,
    ];

    /// 库探测名（find_plugin_library base_name）。
    pub fn id(self) -> &'static str {
        match self {
            NativePluginId::PluginUi => "plugin_ui",
            NativePluginId::PluginOnnx => "plugin_onnx",
            NativePluginId::EvalMonitor => "eval_monitor_dll",
            NativePluginId::PluginWasm => "plugin_wasm",
        }
    }

    /// WSAPI/CLI 字符串解析（未知 id = None）。
    pub fn from_id(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|p| p.id() == s)
    }

    /// 开关是否即时生效（false = 重启后生效）。目前只有 eval 监控是
    /// 事件驱动装载（每次 eval 会话开始查闸）——其余三个都在 gateway
    /// 装配期一次性消费。
    pub fn immediate_effect(self) -> bool {
        matches!(self, NativePluginId::EvalMonitor)
    }
}

/// 开关文件路径（`<config_dir>/plugins_native.json`）。
pub fn plugins_native_config_path(config_dir: &Path) -> std::path::PathBuf {
    config_dir.join("plugins_native.json")
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
/// 放行并让日志可见性留给调用方）。
pub fn load_enabled(config_dir: &Path, id: NativePluginId) -> bool {
    let path = plugins_native_config_path(config_dir);
    let Ok(content) = std::fs::read_to_string(&path) else {
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
/// 罩 config 家族写串行锁（与 config 写臂族互斥，同 approval_rules 惯例）。
pub fn set_enabled(config_dir: &Path, id: NativePluginId, enabled: bool) -> Result<(), String> {
    let _serial = crate::config_write_lock();
    let path = plugins_native_config_path(config_dir);
    let mut file = std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str::<SwitchFile>(&c).ok())
        .unwrap_or_default();
    file.plugins
        .insert(id.id().to_string(), SwitchEntry { enabled });
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建配置目录失败：{e}"))?;
    }
    let json = serde_json::to_string_pretty(&file).map_err(|e| format!("序列化失败：{e}"))?;
    std::fs::write(&path, json + "\n").map_err(|e| format!("写入失败：{e}"))
}
