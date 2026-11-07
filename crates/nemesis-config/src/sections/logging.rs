// 机械拆分：本文件内容由 lib.rs 原样搬入（纯代码搬家，行为零变更）。

use super::*;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LoggingConfig {
    pub llm: Option<LlmLogConfig>,
    pub general: Option<GeneralLogConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmLogConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub log_dir: String,
    #[serde(default = "default_detail_level")]
    pub detail_level: String,
    #[serde(default)]
    pub save_raw: bool,
}

// 手写 Default（不 derive）：`detail_level` 的 serde 缺省是 "full"，
// derive 给空串（同 FsWatcherConfig/AgentsConfig 的坑——当前两 struct 均
// Option 包裹、反序列化路径暂不可达 derive，防御性收敛防未来加
// `#[serde(default)]` 时复发，BUG-20261107-014 同批）。
impl Default for LlmLogConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            log_dir: String::new(),
            detail_level: default_detail_level(),
            save_raw: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralLogConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub enable_console: bool,
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default)]
    pub file: String,
}

// 手写 Default（不 derive）：`enabled`/`enable_console`/`level` 的 serde
// 缺省是 true/true/"INFO"，derive 给 false/false/""（同上，防御性收敛）。
impl Default for GeneralLogConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            enable_console: default_true(),
            level: default_log_level(),
            file: String::new(),
        }
    }
}
