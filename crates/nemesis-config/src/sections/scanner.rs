// ---------------------------------------------------------------------------
// 2026-11-05 机械拆分：本文件内容由 lib.rs 原样搬入（纯代码搬家，行为零变更）。
// ---------------------------------------------------------------------------

use super::*;

/// Scanner configuration loaded from config.scanner.json.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ScannerFullConfig {
    #[serde(default)]
    pub enabled: Vec<String>,
    #[serde(default)]
    pub engines: std::collections::HashMap<String, serde_json::Value>,
}

/// Engine state tracking for scanner engines.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EngineState {
    #[serde(default)]
    pub install_status: String,
    #[serde(default)]
    pub install_error: String,
    #[serde(default)]
    pub last_install_attempt: String,
    #[serde(default)]
    pub db_status: String,
    #[serde(default)]
    pub last_db_update: String,
}

/// ClamAV-specific scanner configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClamAVEngineConfig {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub clamav_path: String,
    #[serde(default)]
    pub address: String,
    #[serde(default)]
    pub scan_on_write: bool,
    #[serde(default)]
    pub scan_on_download: bool,
    #[serde(default)]
    pub scan_on_exec: bool,
    #[serde(default)]
    pub scan_extensions: Vec<String>,
    #[serde(default)]
    pub skip_extensions: Vec<String>,
    #[serde(default)]
    pub max_file_size: i64,
    #[serde(default)]
    pub update_interval: String,
    #[serde(default)]
    pub data_dir: String,
    #[serde(default)]
    pub state: EngineState,
}

// ============================================================================
// Embedded defaults management
// ============================================================================
