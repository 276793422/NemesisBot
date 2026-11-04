// ---------------------------------------------------------------------------
// 2026-11-05 机械拆分：本文件内容由 lib.rs 原样搬入（纯代码搬家，行为零变更）。
// ---------------------------------------------------------------------------

use super::*;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LoggingConfig {
    pub llm: Option<LlmLogConfig>,
    pub general: Option<GeneralLogConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
