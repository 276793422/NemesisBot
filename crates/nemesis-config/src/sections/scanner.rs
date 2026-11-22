// 机械拆分：本文件内容由 lib.rs 原样搬入（纯代码搬家，行为零变更）。

use super::*;

/// Scanner configuration loaded from config.scanner.json.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ScannerFullConfig {
    #[serde(default)]
    pub enabled: Vec<String>,
    #[serde(default)]
    pub engines: std::collections::BTreeMap<String, serde_json::Value>,
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
///
/// BUG-20261118-089：缺省收敛——serde 字段缺省与 [`Default`] 必须答出与出厂
/// 模板（config.scanner.default.json）及 CLI（nemesisbot/commands/scanner.rs
/// 同名结构的 Default impl）一致的健值集。此前 derive 零值 Default 使
/// Dashboard `scanner.add`（handlers/scanner.rs cmd_add 用 `::default()`
/// 构造新引擎条目）物化落盘 `max_file_size=0`（运行时尺寸闸关闭——
/// scanner_config_from_raw 真读该键）、`skip_extensions=[]`（无跳过表全量
/// 扫描）、`update_interval=""`（空串命中 parse_duration_string 空臂 →
/// ZERO → 病毒库自动更新循环不启动）；CLI 同操作写的是健值集——同操作
/// 双入口产出不同运行时行为。`engine.update_config` 对存储缺失键的 serde
/// 缺省物化同根因（parse 后整结构回写）。双路径对齐纪律
/// （BUG-20261114-034 先例）：字段缺省 fn 与 Default impl 必须同答。
/// 注意 scan_on_write/scan_on_exec 键本身当前运行时不消费（声名未读取
/// 家族），但落盘值应与模板语义一致，防未来接线时双面分裂。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClamAVEngineConfig {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub clamav_path: String,
    #[serde(default = "default_clamav_address")]
    pub address: String,
    #[serde(default = "default_clamav_scan_on_write")]
    pub scan_on_write: bool,
    /// 模板与 CLI 缺省 false（与 scan_on_write/scan_on_exec 的 true 不同）。
    #[serde(default)]
    pub scan_on_download: bool,
    #[serde(default = "default_clamav_scan_on_exec")]
    pub scan_on_exec: bool,
    /// File extensions to scan (whitelist).
    #[serde(default)]
    pub scan_extensions: Vec<String>,
    /// File extensions to skip (blacklist).
    #[serde(default = "default_clamav_skip_extensions")]
    pub skip_extensions: Vec<String>,
    /// Maximum file size to scan in bytes.
    #[serde(default = "default_clamav_max_file_size")]
    pub max_file_size: i64,
    /// Database update interval.
    #[serde(default = "default_clamav_update_interval")]
    pub update_interval: String,
    /// Data directory for ClamAV databases.
    #[serde(default)]
    pub data_dir: String,
    /// Engine state tracking.
    #[serde(default)]
    pub state: EngineState,
}

impl Default for ClamAVEngineConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            clamav_path: String::new(),
            address: default_clamav_address(),
            scan_on_write: default_clamav_scan_on_write(),
            scan_on_download: false,
            scan_on_exec: default_clamav_scan_on_exec(),
            scan_extensions: Vec::new(),
            skip_extensions: default_clamav_skip_extensions(),
            max_file_size: default_clamav_max_file_size(),
            update_interval: default_clamav_update_interval(),
            data_dir: String::new(),
            state: EngineState::default(),
        }
    }
}

fn default_clamav_address() -> String {
    "127.0.0.1:3310".to_string()
}

fn default_clamav_scan_on_write() -> bool {
    true
}

fn default_clamav_scan_on_exec() -> bool {
    true
}

fn default_clamav_skip_extensions() -> Vec<String> {
    [
        ".txt", ".md", ".json", ".yaml", ".yml", ".toml", ".log", ".css", ".html",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

fn default_clamav_max_file_size() -> i64 {
    52_428_800
}

fn default_clamav_update_interval() -> String {
    "24h".to_string()
}

// ============================================================================
// Embedded defaults management
// ============================================================================
