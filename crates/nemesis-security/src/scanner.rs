//! Virus Scanner Interface
//!
//! Defines:
//! - `VirusScanner` trait and `ScanResult`
//! - `ScanChain` -- multi-engine scanner chain with extension-based filtering
//! - `ExtensionRules` -- file extension allow/deny lists
//! - `StubScanner` -- no-op placeholder
//! - Engine registry (`create_engine`, `available_engines`)
//! - `ScanChainResult` -- aggregated scan result from multiple engines
//! - `DatabaseStatus`, `EngineInfo` -- engine metadata types

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, warn};

// ---------------------------------------------------------------------------
// Status constants
// ---------------------------------------------------------------------------

/// Install status constants.
pub const INSTALL_STATUS_PENDING: &str = "pending";
pub const INSTALL_STATUS_INSTALLED: &str = "installed";
pub const INSTALL_STATUS_FAILED: &str = "failed";

/// Database status constants.
pub const DB_STATUS_MISSING: &str = "missing";
pub const DB_STATUS_READY: &str = "ready";
pub const DB_STATUS_STALE: &str = "stale";

// ---------------------------------------------------------------------------
// Engine state types (mirrored from nemesis-config for self-containment)
// ---------------------------------------------------------------------------

/// Engine state tracking for scanner engines.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EngineState {
    /// Install status: "pending", "installed", or "failed".
    #[serde(default)]
    pub install_status: String,
    /// Last install error message.
    #[serde(default)]
    pub install_error: String,
    /// Timestamp of last install attempt.
    #[serde(default)]
    pub last_install_attempt: String,
    /// Database status: "missing", "ready", or "stale".
    #[serde(default)]
    pub db_status: String,
    /// Timestamp of last database update.
    #[serde(default)]
    pub last_db_update: String,
}

/// ClamAV-specific engine configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClamAVEngineConfig {
    /// Download URL for ClamAV distribution.
    #[serde(default)]
    pub url: String,
    /// Path to ClamAV installation directory.
    #[serde(default)]
    pub clamav_path: String,
    /// TCP address for the ClamAV daemon.
    #[serde(default)]
    pub address: String,
    /// Whether to scan files on write.
    #[serde(default)]
    pub scan_on_write: bool,
    /// Whether to scan files on download.
    #[serde(default)]
    pub scan_on_download: bool,
    /// Whether to scan files on execution.
    #[serde(default)]
    pub scan_on_exec: bool,
    /// File extensions to scan (whitelist).
    #[serde(default)]
    pub scan_extensions: Vec<String>,
    /// File extensions to skip (blacklist).
    #[serde(default)]
    pub skip_extensions: Vec<String>,
    /// Maximum file size to scan in bytes.
    #[serde(default)]
    pub max_file_size: i64,
    /// Database update interval.
    #[serde(default)]
    pub update_interval: String,
    /// Data directory for ClamAV databases.
    #[serde(default)]
    pub data_dir: String,
    /// Engine state tracking.
    #[serde(default)]
    pub state: EngineState,
}
// ---------------------------------------------------------------------------

/// Result of a single scan operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanResult {
    /// File path (if applicable).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub path: String,
    /// `true` if malware was detected.
    pub infected: bool,
    /// Name of the detected virus/threat.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub virus: String,
    /// Raw output from the scanner engine.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub raw: String,
    /// Name of the engine that produced this result.
    pub engine: String,
    /// Duration of the scan (human-readable).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub duration: String,
}

impl ScanResult {
    /// Convenience: create a clean result.
    pub fn clean() -> Self {
        Self {
            path: String::new(),
            infected: false,
            virus: String::new(),
            raw: String::new(),
            engine: String::new(),
            duration: String::new(),
        }
    }

    /// Create a clean result attributed to a specific engine.
    pub fn clean_from(engine: &str) -> Self {
        Self {
            path: String::new(),
            infected: false,
            virus: String::new(),
            raw: String::new(),
            engine: engine.to_string(),
            duration: String::new(),
        }
    }

    /// Create a clean result with a specific path.
    pub fn clean_with_path(engine: &str, path: &str) -> Self {
        Self {
            path: path.to_string(),
            infected: false,
            virus: String::new(),
            raw: String::new(),
            engine: engine.to_string(),
            duration: String::new(),
        }
    }

    /// Create a result with a detected threat.
    pub fn with_threats(engine: &str, virus: &str, path: &str) -> Self {
        Self {
            path: path.to_string(),
            infected: true,
            virus: virus.to_string(),
            raw: String::new(),
            engine: engine.to_string(),
            duration: String::new(),
        }
    }

    /// Returns true when the scanned subject is malware-free.
    pub fn is_clean(&self) -> bool {
        !self.infected
    }

    /// Merge another result into this one. If either has threats, the result is not clean.
    pub fn merge(&mut self, other: &ScanResult) {
        if other.infected {
            self.infected = true;
            if self.virus.is_empty() {
                self.virus = other.virus.clone();
            }
            if self.engine.is_empty() {
                self.engine = other.engine.clone();
            }
        }
        if self.path.is_empty() && !other.path.is_empty() {
            self.path = other.path.clone();
        }
    }
}

// ---------------------------------------------------------------------------
// ScanChainResult
// ---------------------------------------------------------------------------

/// Aggregated result from multiple engines in the scan chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanChainResult {
    /// `true` when no threats were detected.
    pub clean: bool,
    /// `true` when a threat was detected and the operation should be blocked.
    pub blocked: bool,
    /// Engine that detected the threat.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub engine: String,
    /// Name of the detected virus.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub virus: String,
    /// File path scanned.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub path: String,
    /// Individual results from each engine.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub results: Vec<ScanResult>,
}

impl ScanChainResult {
    /// Create a clean chain result.
    pub fn clean() -> Self {
        Self {
            clean: true,
            blocked: false,
            engine: String::new(),
            virus: String::new(),
            path: String::new(),
            results: Vec::new(),
        }
    }

    /// Create a blocked chain result.
    pub fn blocked(engine: &str, virus: &str, path: &str, results: Vec<ScanResult>) -> Self {
        Self {
            clean: false,
            blocked: true,
            engine: engine.to_string(),
            virus: virus.to_string(),
            path: path.to_string(),
            results,
        }
    }
}

// ---------------------------------------------------------------------------
// EngineInfo
// ---------------------------------------------------------------------------

/// Metadata about a scanner engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineInfo {
    /// Engine name.
    pub name: String,
    /// Engine version (if available).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub version: String,
    /// Engine address (e.g. TCP address for ClamAV).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub address: String,
    /// Whether the engine is ready.
    pub ready: bool,
    /// Start time of the engine.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub start_time: String,
}

// ---------------------------------------------------------------------------
// DatabaseStatus
// ---------------------------------------------------------------------------

/// Status of a scanner's virus definitions database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseStatus {
    /// Whether a database is available.
    pub available: bool,
    /// Database version string.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub version: String,
    /// Last update timestamp.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_update: String,
    /// Path to the database file.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub path: String,
    /// Size of the database in bytes.
    #[serde(default)]
    pub size_bytes: i64,
}

impl Default for DatabaseStatus {
    fn default() -> Self {
        Self {
            available: false,
            version: String::new(),
            last_update: String::new(),
            path: String::new(),
            size_bytes: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// VirusScanner trait
// ---------------------------------------------------------------------------

/// Trait that virus-scanner backends must implement.
#[async_trait]
pub trait VirusScanner: Send + Sync {
    /// Human-readable engine name.
    fn name(&self) -> &str;

    /// Get engine metadata.
    async fn get_info(&self) -> EngineInfo;

    /// Start the engine.
    async fn start(&self) -> Result<(), String>;

    /// Stop the engine.
    async fn stop(&self) -> Result<(), String>;

    /// Whether the engine is ready to scan.
    async fn is_ready(&self) -> bool;

    /// Scan a single file.
    async fn scan_file(&self, path: &Path) -> ScanResult;

    /// Scan raw bytes in memory.
    async fn scan_content(&self, content: &[u8]) -> ScanResult;

    /// Scan all files in a directory.
    async fn scan_directory(&self, dir: &Path) -> Vec<ScanResult>;

    /// Get database status.
    async fn get_database_status(&self) -> DatabaseStatus;

    /// Update virus database.
    async fn update_database(&self) -> Result<(), String>;

    /// Get engine-specific statistics.
    fn get_stats(&self) -> HashMap<String, serde_json::Value>;
}

// ---------------------------------------------------------------------------
// InstallableEngine trait
// ---------------------------------------------------------------------------

/// Extension of `VirusScanner` for engines that can be downloaded, installed,
/// and have their installation and database state detected.
///
/// Equivalent to Go's `InstallableEngine` interface.
#[async_trait]
pub trait InstallableEngine: VirusScanner {
    /// Returns the executable names to look for on the current OS.
    ///
    /// For example, ClamAV returns `["clamd.exe"]` on Windows and `["clamd"]` on Linux.
    fn target_executables(&self) -> Vec<String>;

    /// Recursively search `dir` for target executables, returning the directory
    /// containing the first match.
    fn detect_install_path(&self, dir: &Path) -> Result<String, String>;

    /// Returns the primary database file name (e.g. `"main.cvd"` for ClamAV).
    fn database_file_name(&self) -> String;

    /// Returns the current engine state (install status, db status, etc.).
    fn get_engine_state(&self) -> EngineState;

    /// Download the engine distribution to the given directory.
    async fn download(&self, dir: &str) -> Result<(), String>;

    /// Validate that the directory contains a valid installation.
    fn validate(&self, dir: &str) -> Result<(), String>;

    /// Set up the engine from raw JSON config.
    fn setup(&self, config: &serde_json::Value) -> Result<(), String>;
}

// ---------------------------------------------------------------------------
// ClamAVEngine (full InstallableEngine implementation)
// ---------------------------------------------------------------------------

/// ClamAV engine implementing both `VirusScanner` and `InstallableEngine`.
///
/// Equivalent to Go's `ClamAVEngine`.
pub struct ClamAVEngine {
    config: parking_lot::RwLock<ClamAVEngineConfig>,
    scanner: parking_lot::RwLock<Option<Arc<crate::clamav::scanner::Scanner>>>,
    started: AtomicBool,
}

impl ClamAVEngine {
    /// Create a new ClamAV engine from the given configuration.
    pub fn new(config: ClamAVEngineConfig) -> Self {
        Self {
            config: parking_lot::RwLock::new(config),
            scanner: parking_lot::RwLock::new(None),
            started: AtomicBool::new(false),
        }
    }

    /// Get the current ClamAV installation path.
    pub fn get_clamav_path(&self) -> String {
        self.config.read().clamav_path.clone()
    }

    /// Set the data directory path used by the engine.
    pub fn set_data_dir(&self, dir: &str) {
        self.config.write().data_dir = dir.to_string();
    }

    /// Get extension rules from the engine config.
    pub fn get_extension_rules(&self) -> ExtensionRules {
        let cfg = self.config.read();
        ExtensionRules::new(
            cfg.scan_extensions.clone(),
            cfg.skip_extensions.clone(),
        )
    }
}

#[async_trait]
impl VirusScanner for ClamAVEngine {
    fn name(&self) -> &str {
        "clamav"
    }

    async fn get_info(&self) -> EngineInfo {
        let address = self.config.read().address.clone();
        drop(self.config.read());
        let ready = self.is_ready().await;
        EngineInfo {
            name: "clamav".to_string(),
            version: String::new(),
            address,
            ready,
            start_time: String::new(),
        }
    }

    async fn start(&self) -> Result<(), String> {
        if self.started.load(Ordering::SeqCst) {
            return Ok(());
        }
        let address = {
            let cfg = self.config.read();
            if cfg.address.is_empty() {
                "127.0.0.1:3310".to_string()
            } else {
                cfg.address.clone()
            }
        };

        let scanner_config = crate::clamav::scanner::ScannerConfig {
            enabled: true,
            address: address.clone(),
            ..Default::default()
        };

        let scanner = crate::clamav::scanner::Scanner::new(scanner_config);
        // Verify connectivity
        scanner
            .ping()
            .await
            .map_err(|e| format!("ClamAV ping failed: {}", e))?;

        *self.scanner.write() = Some(Arc::new(scanner));
        self.started.store(true, Ordering::SeqCst);
        tracing::info!("[Scanner] ClamAV engine started at {}", address);
        Ok(())
    }

    async fn stop(&self) -> Result<(), String> {
        if !self.started.swap(false, Ordering::SeqCst) {
            return Ok(());
        }
        *self.scanner.write() = None;
        tracing::info!("[Scanner] ClamAV engine stopped");
        Ok(())
    }

    async fn is_ready(&self) -> bool {
        if !self.started.load(Ordering::SeqCst) {
            return false;
        }
        let scanner = self.scanner.read().clone();
        match scanner {
            Some(s) => s.ping().await.is_ok(),
            None => false,
        }
    }

    async fn scan_file(&self, path: &Path) -> ScanResult {
        let start = std::time::Instant::now();
        let scanner_opt = self.scanner.read().clone();
        match scanner_opt {
            None => ScanResult {
                path: path.to_string_lossy().to_string(),
                infected: false,
                virus: String::new(),
                raw: "engine not ready".to_string(),
                engine: "clamav".to_string(),
                duration: format!("{:?}", start.elapsed()),
            },
            Some(s) => match s.scan_file(path).await {
                Ok(result) => ScanResult {
                    path: result.path,
                    infected: result.infected,
                    virus: result.virus,
                    raw: result.raw,
                    engine: "clamav".to_string(),
                    duration: format!("{:?}", start.elapsed()),
                },
                Err(e) => ScanResult {
                    path: path.to_string_lossy().to_string(),
                    infected: false,
                    virus: String::new(),
                    raw: format!("scan error: {}", e),
                    engine: "clamav".to_string(),
                    duration: format!("{:?}", start.elapsed()),
                },
            },
        }
    }

    async fn scan_content(&self, content: &[u8]) -> ScanResult {
        let start = std::time::Instant::now();
        let scanner_opt = self.scanner.read().clone();
        match scanner_opt {
            None => ScanResult {
                path: String::new(),
                infected: false,
                virus: String::new(),
                raw: "engine not ready".to_string(),
                engine: "clamav".to_string(),
                duration: format!("{:?}", start.elapsed()),
            },
            Some(s) => match s.scan_content(content).await {
                Ok(result) => ScanResult {
                    path: result.path,
                    infected: result.infected,
                    virus: result.virus,
                    raw: result.raw,
                    engine: "clamav".to_string(),
                    duration: format!("{:?}", start.elapsed()),
                },
                Err(e) => ScanResult {
                    path: String::new(),
                    infected: false,
                    virus: String::new(),
                    raw: format!("scan error: {}", e),
                    engine: "clamav".to_string(),
                    duration: format!("{:?}", start.elapsed()),
                },
            },
        }
    }

    async fn scan_directory(&self, dir: &Path) -> Vec<ScanResult> {
        let mut results = Vec::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    results.push(self.scan_file(&path).await);
                }
            }
        }
        results
    }

    async fn get_database_status(&self) -> DatabaseStatus {
        DatabaseStatus::default()
    }

    async fn update_database(&self) -> Result<(), String> {
        if !self.is_ready().await {
            return Err("clamav engine not ready".to_string());
        }
        Ok(())
    }

    fn get_stats(&self) -> HashMap<String, serde_json::Value> {
        let mut stats = HashMap::new();
        stats.insert("started".to_string(), serde_json::json!(self.started.load(Ordering::SeqCst)));
        stats
    }
}

#[async_trait]
impl InstallableEngine for ClamAVEngine {
    fn target_executables(&self) -> Vec<String> {
        if cfg!(windows) {
            vec!["clamd.exe".to_string()]
        } else {
            vec!["clamd".to_string()]
        }
    }

    fn detect_install_path(&self, dir: &Path) -> Result<String, String> {
        let targets = self.target_executables();
        let mut found_path = None;

        if let Ok(entries) = walkdir(dir) {
            for path in entries {
                if let Some(name) = path.file_name() {
                    if targets.iter().any(|t| name == t.as_str()) {
                        found_path = Some(path.parent().unwrap_or(Path::new(".")).to_string_lossy().to_string());
                        break;
                    }
                }
            }
        }

        match found_path {
            Some(p) => Ok(p),
            None => Err(format!(
                "target executable not found in {} (looked for: {:?})",
                dir.display(),
                targets
            )),
        }
    }

    fn database_file_name(&self) -> String {
        "main.cvd".to_string()
    }

    fn get_engine_state(&self) -> EngineState {
        self.config.read().state.clone()
    }

    async fn download(&self, dir: &str) -> Result<(), String> {
        let url = {
            let cfg = self.config.read();
            if cfg.url.is_empty() {
                return Err("no download URL configured for clamav".to_string());
            }
            cfg.url.clone()
        };

        // Create target directory
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("failed to create directory: {}", e))?;

        // Download the archive with streaming (matching Go's progressive download)
        let response = reqwest::get(&url)
            .await
            .map_err(|e| format!("download failed: {}", e))?;

        if !response.status().is_success() {
            return Err(format!("download failed with status: {}", response.status()));
        }

        let total_size = response.content_length();
        let dir_path = Path::new(dir);

        // Create temp file in target directory
        let tmp_path = dir_path.join("clamav-download.zip");
        let mut tmp_file = tokio::fs::File::create(&tmp_path)
            .await
            .map_err(|e| format!("failed to create temp file: {}", e))?;

        // Stream download with progress logging every 2 seconds
        let mut stream = response.bytes_stream();
        use futures_util::StreamExt;
        let mut written: u64 = 0;
        let mut last_log = std::time::Instant::now();
        use tokio::io::AsyncWriteExt;

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| format!("download read failed: {}", e))?;
            tmp_file.write_all(&chunk)
                .await
                .map_err(|e| {
                    let _ = std::fs::remove_file(&tmp_path);
                    format!("download write failed: {}", e)
                })?;
            written += chunk.len() as u64;

            // Log progress every 2 seconds
            if last_log.elapsed() >= std::time::Duration::from_secs(2) {
                match total_size {
                    Some(total) => {
                        let pct = written as f64 / total as f64 * 100.0;
                        debug!(
                            "Downloading ClamAV: {:.1}% ({}/{} bytes)",
                            pct,
                            format_bytes(written),
                            format_bytes(total)
                        );
                    }
                    None => {
                        debug!("Downloading ClamAV: {} bytes", format_bytes(written));
                    }
                }
                last_log = std::time::Instant::now();
            }
        }

        tmp_file.flush().await.map_err(|e| format!("flush failed: {}", e))?;
        drop(tmp_file);

        if let Some(total) = total_size {
            debug!("ClamAV download complete: {}", format_bytes(total));
        } else {
            debug!("ClamAV download complete: {} bytes", written);
        }

        // Extract zip
        extract_zip_archive(&tmp_path, dir_path)?;

        // Clean up temp file
        let _ = std::fs::remove_file(&tmp_path);

        // Auto-detect install path
        let install_path = self.detect_install_path(dir_path)?;
        self.config.write().clamav_path = install_path.clone();

        debug!("ClamAV downloaded and detected: url={}, dir={}, install_path={}", url, dir, install_path);
        Ok(())
    }

    fn validate(&self, dir: &str) -> Result<(), String> {
        let exe_name = if cfg!(windows) { "clamd.exe" } else { "clamd" };
        let exe_path = Path::new(dir).join(exe_name);
        if !exe_path.exists() {
            return Err(format!("clamd executable not found at {}", exe_path.display()));
        }
        Ok(())
    }

    fn setup(&self, config: &serde_json::Value) -> Result<(), String> {
        if config.is_null() {
            return Ok(());
        }
        let updated: ClamAVEngineConfig =
            serde_json::from_value(config.clone()).map_err(|e| format!("invalid config: {}", e))?;
        *self.config.write() = updated;
        Ok(())
    }
}

/// Format bytes into a human-readable string (e.g., "42.5 MB").
///
/// Matches Go's `formatBytes()`.
fn format_bytes(b: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    if b < MB {
        format!("{} KB", b / KB)
    } else {
        format!("{:.1} MB", b as f64 / MB as f64)
    }
}

/// Walk a directory tree collecting all file paths.
fn walkdir(dir: &Path) -> Result<Vec<std::path::PathBuf>, String> {
    let mut paths = Vec::new();
    walkdir_recursive(dir, &mut paths)?;
    Ok(paths)
}

fn walkdir_recursive(dir: &Path, paths: &mut Vec<std::path::PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("read_dir failed: {}", e))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("dir entry failed: {}", e))?;
        let path = entry.path();
        if path.is_dir() {
            walkdir_recursive(&path, paths)?;
        } else {
            paths.push(path);
        }
    }
    Ok(())
}

/// Extract a zip archive to the given directory.
fn extract_zip_archive(zip_path: &Path, dest_dir: &Path) -> Result<(), String> {
    let file = std::fs::File::open(zip_path)
        .map_err(|e| format!("failed to open zip file: {}", e))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| format!("failed to read zip archive: {}", e))?;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)
            .map_err(|e| format!("failed to read zip entry {}: {}", i, e))?;

        let outpath = match entry.enclosed_name() {
            Some(path) => dest_dir.join(path),
            None => continue,
        };

        if entry.is_dir() {
            std::fs::create_dir_all(&outpath)
                .map_err(|e| format!("failed to create dir {}: {}", outpath.display(), e))?;
        } else {
            if let Some(parent) = outpath.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("failed to create parent dir: {}", e))?;
            }
            let mut outfile = std::fs::File::create(&outpath)
                .map_err(|e| format!("failed to create file {}: {}", outpath.display(), e))?;
            std::io::copy(&mut entry, &mut outfile)
                .map_err(|e| format!("failed to extract file {}: {}", outpath.display(), e))?;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// StubScanner
// ---------------------------------------------------------------------------

/// Stub scanner that always reports clean.
///
/// Useful as a no-op placeholder in environments where no real scanner is
/// installed, and as a baseline for testing.
pub struct StubScanner;

#[async_trait]
impl VirusScanner for StubScanner {
    fn name(&self) -> &str {
        "stub"
    }

    async fn get_info(&self) -> EngineInfo {
        EngineInfo {
            name: "stub".to_string(),
            version: String::new(),
            address: String::new(),
            ready: true,
            start_time: String::new(),
        }
    }

    async fn start(&self) -> Result<(), String> {
        Ok(())
    }

    async fn stop(&self) -> Result<(), String> {
        Ok(())
    }

    async fn scan_file(&self, path: &Path) -> ScanResult {
        ScanResult::clean_with_path("stub", &path.to_string_lossy())
    }

    async fn scan_content(&self, _content: &[u8]) -> ScanResult {
        ScanResult::clean_from("stub")
    }

    async fn scan_directory(&self, _dir: &Path) -> Vec<ScanResult> {
        Vec::new()
    }

    async fn is_ready(&self) -> bool {
        true
    }

    async fn get_database_status(&self) -> DatabaseStatus {
        DatabaseStatus::default()
    }

    async fn update_database(&self) -> Result<(), String> {
        Ok(())
    }

    fn get_stats(&self) -> HashMap<String, serde_json::Value> {
        let mut stats = HashMap::new();
        stats.insert("ready".to_string(), serde_json::Value::Bool(true));
        stats
    }
}

// ---------------------------------------------------------------------------
// ScanEngine enum
// ---------------------------------------------------------------------------

/// Selectable scan engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanEngine {
    /// No-op stub (always clean).
    Stub,
    /// ClamAV virus scanner (connects to clamav daemon via TCP).
    ClamAV,
}

impl Default for ScanEngine {
    fn default() -> Self {
        Self::Stub
    }
}

impl ScanEngine {
    /// Build the scanner corresponding to this engine variant.
    pub fn build(&self) -> Box<dyn VirusScanner> {
        match self {
            ScanEngine::Stub => Box::new(StubScanner),
            ScanEngine::ClamAV => {
                let config = crate::clamav::scanner::ScannerConfig::default();
                Box::new(ClamavScannerWrapper::new(config))
            }
        }
    }

    /// Build a ClamAV scanner with a specific address.
    pub fn build_with_address(&self, address: &str) -> Box<dyn VirusScanner> {
        match self {
            ScanEngine::Stub => Box::new(StubScanner),
            ScanEngine::ClamAV => {
                let mut config = crate::clamav::scanner::ScannerConfig::default();
                config.address = address.to_string();
                Box::new(ClamavScannerWrapper::new(config))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ClamAV VirusScanner wrapper
// ---------------------------------------------------------------------------

/// Wraps the ClamAV `Scanner` to implement the `VirusScanner` trait.
///
/// On `start()`, this creates a `clamav::Manager` which handles the full
/// daemon lifecycle (config generation, DB download, daemon start, auto-update).
struct ClamavScannerWrapper {
    scanner: crate::clamav::scanner::Scanner,
    config_address: String,
    /// Manager config fields extracted from the engine config JSON.
    manager_config: ClamavManagerFields,
    /// Manager instance, created on start().
    manager: tokio::sync::Mutex<Option<crate::clamav::manager::Manager>>,
    started: AtomicBool,
}

/// Fields extracted from the engine config JSON needed to create a Manager.
struct ClamavManagerFields {
    clamav_path: String,
    data_dir: String,
    update_interval: String,
}

impl ClamavScannerWrapper {
    fn new(config: crate::clamav::scanner::ScannerConfig) -> Self {
        let addr = config.address.clone();
        Self {
            scanner: crate::clamav::scanner::Scanner::new(config),
            config_address: addr,
            manager_config: ClamavManagerFields {
                clamav_path: String::new(),
                data_dir: String::new(),
                update_interval: String::new(),
            },
            manager: tokio::sync::Mutex::new(None),
            started: AtomicBool::new(false),
        }
    }

    /// Set the ClamAV installation path (used by Manager to find clamd.exe).
    fn set_clamav_path(&mut self, path: String) {
        self.manager_config.clamav_path = path;
    }

    /// Set the data directory for ClamAV databases.
    fn set_data_dir(&mut self, dir: String) {
        self.manager_config.data_dir = dir;
    }

    /// Set the update interval string (e.g., "24h").
    fn set_update_interval(&mut self, interval: String) {
        self.manager_config.update_interval = interval;
    }
}

#[async_trait]
impl VirusScanner for ClamavScannerWrapper {
    fn name(&self) -> &str {
        "clamav"
    }

    async fn get_info(&self) -> EngineInfo {
        let ready = self.scanner.ping().await.is_ok();
        EngineInfo {
            name: "clamav".to_string(),
            version: String::new(),
            address: self.config_address.clone(),
            ready,
            start_time: String::new(),
        }
    }

    async fn start(&self) -> Result<(), String> {
        if self.started.load(Ordering::SeqCst) {
            return Ok(());
        }

        // Try to use Manager to start the daemon (full lifecycle management).
        // This handles: config generation, DB download, daemon start, auto-update.
        if !self.manager_config.clamav_path.is_empty() {
            let mgr_config = crate::clamav::manager::ManagerConfig {
                enabled: true,
                clamav_path: self.manager_config.clamav_path.clone(),
                data_dir: self.manager_config.data_dir.clone(),
                address: self.config_address.clone(),
                scanner: None,
                update_interval: self.manager_config.update_interval.clone(),
            };

            let mut mgr = crate::clamav::manager::Manager::new(mgr_config);
            match mgr.start().await {
                Ok(()) => {
                    tracing::info!("[Scanner] ClamAV daemon started via Manager");
                    *self.manager.lock().await = Some(mgr);
                    self.started.store(true, Ordering::SeqCst);
                    return Ok(());
                }
                Err(e) => {
                    tracing::warn!("[Scanner] Manager start failed ({}), falling back to ping-only mode", e);
                    // Fall through to ping-only mode
                }
            }
        }

        // Fallback: just verify connectivity to an already-running daemon
        self.scanner.ping().await.map_err(|e| format!("ClamAV ping failed: {}", e))?;
        self.started.store(true, Ordering::SeqCst);
        Ok(())
    }

    async fn stop(&self) -> Result<(), String> {
        if !self.started.swap(false, Ordering::SeqCst) {
            return Ok(());
        }
        let mut guard = self.manager.lock().await;
        if let Some(mgr) = guard.take() {
            mgr.stop().await?;
            tracing::info!("[Scanner] ClamAV daemon stopped via Manager");
        }
        Ok(())
    }

    async fn is_ready(&self) -> bool {
        if !self.started.load(Ordering::SeqCst) {
            return false;
        }
        // If manager is present, check its running state
        let guard = self.manager.lock().await;
        if let Some(ref mgr) = *guard {
            if !mgr.is_running() {
                return false;
            }
        }
        drop(guard);
        self.scanner.ping().await.is_ok()
    }

    async fn scan_file(&self, path: &Path) -> ScanResult {
        let start = std::time::Instant::now();
        match self.scanner.scan_file(path).await {
            Ok(result) => ScanResult {
                path: result.path,
                infected: result.infected,
                virus: result.virus,
                raw: result.raw,
                engine: "clamav".to_string(),
                duration: format!("{:?}", start.elapsed()),
            },
            Err(e) => ScanResult {
                path: path.to_string_lossy().to_string(),
                infected: false,
                virus: String::new(),
                raw: format!("scan error: {}", e),
                engine: "clamav".to_string(),
                duration: format!("{:?}", start.elapsed()),
            },
        }
    }

    async fn scan_content(&self, content: &[u8]) -> ScanResult {
        let start = std::time::Instant::now();
        match self.scanner.scan_content(content).await {
            Ok(result) => ScanResult {
                path: result.path,
                infected: result.infected,
                virus: result.virus,
                raw: result.raw,
                engine: "clamav".to_string(),
                duration: format!("{:?}", start.elapsed()),
            },
            Err(e) => ScanResult {
                path: String::new(),
                infected: false,
                virus: String::new(),
                raw: format!("scan error: {}", e),
                engine: "clamav".to_string(),
                duration: format!("{:?}", start.elapsed()),
            },
        }
    }

    async fn scan_directory(&self, dir: &Path) -> Vec<ScanResult> {
        let mut results = Vec::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    results.push(self.scan_file(&path).await);
                }
            }
        }
        results
    }

    async fn get_database_status(&self) -> DatabaseStatus {
        DatabaseStatus::default()
    }

    async fn update_database(&self) -> Result<(), String> {
        // Database update is handled by the ClamAV daemon externally
        Ok(())
    }

    fn get_stats(&self) -> HashMap<String, serde_json::Value> {
        // Use tokio runtime to get stats synchronously
        HashMap::new()
    }
}

// ---------------------------------------------------------------------------
// Engine registry
// ---------------------------------------------------------------------------

/// Instantiate a `VirusScanner` by engine name.
///
/// Currently only "clamav" and "stub" are recognized.
pub fn create_engine(name: &str, config: &serde_json::Value) -> Result<Box<dyn VirusScanner>, String> {
    match name {
        "clamav" => {
            let mut scanner_config = crate::clamav::scanner::ScannerConfig::default();
            // Apply config overrides if provided
            if let Some(addr) = config.get("address").and_then(|v| v.as_str()) {
                scanner_config.address = addr.to_string();
            }
            if let Some(enabled) = config.get("enabled").and_then(|v| v.as_bool()) {
                scanner_config.enabled = enabled;
            }
            if let Some(timeout) = config.get("timeout_secs").and_then(|v| v.as_u64()) {
                scanner_config.timeout = std::time::Duration::from_secs(timeout);
            }

            let mut wrapper = ClamavScannerWrapper::new(scanner_config);

            // Extract Manager-related config fields for daemon lifecycle management
            if let Some(path) = config.get("clamav_path").and_then(|v| v.as_str()) {
                wrapper.set_clamav_path(path.to_string());
            }
            if let Some(dir) = config.get("data_dir").and_then(|v| v.as_str()) {
                wrapper.set_data_dir(dir.to_string());
            }
            if let Some(interval) = config.get("update_interval").and_then(|v| v.as_str()) {
                wrapper.set_update_interval(interval.to_string());
            }

            Ok(Box::new(wrapper))
        }
        "stub" => Ok(Box::new(StubScanner)),
        _ => Err(format!("unknown scanner engine: {}", name)),
    }
}

/// List all built-in engine names.
pub fn available_engines() -> Vec<&'static str> {
    vec!["clamav", "stub"]
}

// ---------------------------------------------------------------------------
// Extension rules
// ---------------------------------------------------------------------------

/// File extension rules for scan filtering.
///
/// Uses a scan-extensions (whitelist) and skip-extensions (blacklist) model:
/// - If `scan_extensions` is non-empty, only scan files with those extensions.
/// - Otherwise, skip files whose extension is in `skip_extensions`.
/// - If both are empty, scan everything.
pub struct ExtensionRules {
    /// Whitelist: only scan files with these extensions.
    pub scan_extensions: Vec<String>,
    /// Blacklist: skip files with these extensions.
    pub skip_extensions: Vec<String>,
}

impl Default for ExtensionRules {
    fn default() -> Self {
        Self {
            scan_extensions: Vec::new(),
            skip_extensions: Vec::new(),
        }
    }
}

impl ExtensionRules {
    /// Create a new set of extension rules.
    pub fn new(scan_extensions: Vec<String>, skip_extensions: Vec<String>) -> Self {
        Self {
            scan_extensions,
            skip_extensions,
        }
    }

    /// Determine if a file should be scanned based on its extension.
    pub fn should_scan_file(&self, path: &Path) -> bool {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        // Whitelist mode: only scan listed extensions.
        if !self.scan_extensions.is_empty() {
            return self.scan_extensions.iter().any(|e| e.to_lowercase() == ext);
        }

        // Blacklist mode: skip listed extensions.
        if !self.skip_extensions.is_empty() {
            if self.skip_extensions.iter().any(|e| e.to_lowercase() == ext) {
                return false;
            }
        }

        // Default: scan everything.
        true
    }
}

// ---------------------------------------------------------------------------
// ScanChainConfig
// ---------------------------------------------------------------------------

/// Configuration for the scanner chain.
#[derive(Debug, Clone)]
pub struct ScanChainConfig {
    /// Whether the scanner chain is enabled.
    pub enabled: bool,
    /// Maximum file size to scan (bytes). 0 = no limit.
    pub max_file_size: u64,
}

impl Default for ScanChainConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_file_size: 50 * 1024 * 1024, // 50 MB
        }
    }
}

// ---------------------------------------------------------------------------
// ScanChain
// ---------------------------------------------------------------------------

/// Multi-engine scanner chain.
///
/// Scans files through all registered engines. Scanning short-circuits on the
/// first engine that detects a threat.
pub struct ScanChain {
    engines: Vec<Box<dyn VirusScanner>>,
    configs: HashMap<String, serde_json::Value>,
    #[allow(dead_code)]
    config: ScanChainConfig,
    rules: ExtensionRules,
    enabled: std::sync::atomic::AtomicBool,
}

impl ScanChain {
    /// Create a new empty scan chain.
    pub fn new(config: ScanChainConfig) -> Self {
        Self {
            engines: Vec::new(),
            configs: HashMap::new(),
            config,
            rules: ExtensionRules::default(),
            enabled: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Create a scan chain with default config.
    pub fn with_defaults() -> Self {
        Self::new(ScanChainConfig::default())
    }

    /// Add a scanner engine to the chain.
    pub fn add_engine(&mut self, engine: Box<dyn VirusScanner>) {
        debug!("Adding scanner engine: {}", engine.name());
        self.engines.push(engine);
    }

    /// Enable or disable the scan chain.
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, std::sync::atomic::Ordering::Relaxed);
    }

    /// Check if the chain is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Get the number of registered engines.
    pub fn engine_count(&self) -> usize {
        self.engines.len()
    }

    /// Remove all registered engines from the chain.
    pub fn clear_engines(&mut self) {
        self.engines.clear();
        self.configs.clear();
    }

    /// Get the list of engines in execution order.
    pub fn engines(&self) -> Vec<&dyn VirusScanner> {
        self.engines.iter().map(|e| e.as_ref()).collect()
    }

    /// Get the raw JSON config for a given engine name.
    pub fn raw_config(&self, name: &str) -> Option<&serde_json::Value> {
        self.configs.get(name)
    }

    /// Get extension rules.
    pub fn extension_rules(&self) -> &ExtensionRules {
        &self.rules
    }

    /// Start all engines in the chain.
    pub async fn start(&self) {
        for engine in &self.engines {
            if let Err(e) = engine.start().await {
                warn!("Failed to start engine {}: {}", engine.name(), e);
            }
        }
    }

    /// Stop all engines in the chain.
    pub async fn stop(&self) {
        for engine in &self.engines {
            if let Err(e) = engine.stop().await {
                warn!("Failed to stop engine {}: {}", engine.name(), e);
            }
        }
    }

    /// Get statistics for each engine.
    pub fn get_stats(&self) -> HashMap<String, HashMap<String, serde_json::Value>> {
        let mut stats = HashMap::new();
        for engine in &self.engines {
            stats.insert(engine.name().to_string(), engine.get_stats());
        }
        stats
    }

    /// Scan a single file through all engines. Short-circuits on first detection.
    pub async fn scan_file(&self, path: &Path) -> ScanChainResult {
        if self.engines.is_empty() {
            return ScanChainResult::clean();
        }

        if !self.rules.should_scan_file(path) {
            return ScanChainResult::clean();
        }

        let mut results = Vec::new();
        for engine in &self.engines {
            if !engine.is_ready().await {
                warn!("Engine not ready, skipping: {}", engine.name());
                continue;
            }

            let result = engine.scan_file(path).await;
            results.push(result.clone());
            if result.infected {
                return ScanChainResult::blocked(
                    engine.name(),
                    &result.virus,
                    &path.to_string_lossy(),
                    results,
                );
            }
        }

        ScanChainResult {
            clean: true,
            blocked: false,
            engine: String::new(),
            virus: String::new(),
            path: path.to_string_lossy().to_string(),
            results,
        }
    }

    /// Scan raw content through all engines.
    pub async fn scan_content(&self, content: &[u8]) -> ScanChainResult {
        if self.engines.is_empty() {
            return ScanChainResult::clean();
        }

        let mut results = Vec::new();
        for engine in &self.engines {
            if !engine.is_ready().await {
                continue;
            }

            let result = engine.scan_content(content).await;
            results.push(result.clone());
            if result.infected {
                return ScanChainResult::blocked(
                    engine.name(),
                    &result.virus,
                    "",
                    results,
                );
            }
        }

        ScanChainResult {
            clean: true,
            blocked: false,
            engine: String::new(),
            virus: String::new(),
            path: String::new(),
            results,
        }
    }

    /// Scan a directory through all engines.
    pub async fn scan_directory(&self, dir: &Path) -> ScanChainResult {
        if self.engines.is_empty() {
            return ScanChainResult::clean();
        }

        let mut all_results = Vec::new();
        for engine in &self.engines {
            if !engine.is_ready().await {
                continue;
            }

            let results = engine.scan_directory(dir).await;
            for r in results {
                all_results.push(r.clone());
                if r.infected {
                    return ScanChainResult::blocked(
                        engine.name(),
                        &r.virus,
                        &r.path,
                        all_results,
                    );
                }
            }
        }

        ScanChainResult {
            clean: true,
            blocked: false,
            engine: String::new(),
            virus: String::new(),
            path: dir.to_string_lossy().to_string(),
            results: all_results,
        }
    }

    /// Check whether a tool invocation should be blocked.
    ///
    /// Returns `(allowed, virus_error)`. If `allowed` is false, `virus_error`
    /// describes the detected threat.
    pub async fn scan_tool_invocation(
        &self,
        tool_name: &str,
        args: &serde_json::Value,
    ) -> (bool, Option<String>) {
        if !self.is_enabled() || self.engines.is_empty() {
            return (true, None);
        }

        // Extract file path from args.
        let file_path = args
            .get("path")
            .or_else(|| args.get("save_path"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Check extension rules.
        if !file_path.is_empty() && !self.rules.should_scan_file(Path::new(file_path)) {
            return (true, None);
        }

        match tool_name {
            "write_file" | "edit_file" | "append_file" => {
                if let Some(content) = args.get("content").and_then(|v| v.as_str()) {
                    if !content.is_empty() {
                        let result = self.scan_content(content.as_bytes()).await;
                        if result.blocked {
                            return (
                                false,
                                Some(format!(
                                    "virus detected by {}: {} (virus: {})",
                                    result.engine, file_path, result.virus
                                )),
                            );
                        }
                    }
                }
            }
            "download" => {
                if !file_path.is_empty() {
                    let result = self.scan_file(Path::new(file_path)).await;
                    if result.blocked {
                        return (
                            false,
                            Some(format!(
                                "virus detected by {}: {} (virus: {})",
                                result.engine, file_path, result.virus
                            )),
                        );
                    }
                }
            }
            "exec" | "execute_command" | "shell" | "exec_async" => {
                if !file_path.is_empty() {
                    let result = self.scan_file(Path::new(file_path)).await;
                    if result.blocked {
                        return (
                            false,
                            Some(format!(
                                "virus detected by {}: {} (virus: {})",
                                result.engine, file_path, result.virus
                            )),
                        );
                    }
                }
            }
            "web_fetch" => {
                // Scan saved file if present
                if !file_path.is_empty() {
                    let result = self.scan_file(Path::new(file_path)).await;
                    if result.blocked {
                        return (
                            false,
                            Some(format!(
                                "virus detected by {}: {} (virus: {})",
                                result.engine, file_path, result.virus
                            )),
                        );
                    }
                }
                // Scan inline content fields
                for key in &["content", "data", "body", "html"] {
                    if let Some(content) = args.get(*key).and_then(|v| v.as_str()) {
                        if !content.is_empty() {
                            let result = self.scan_content(content.as_bytes()).await;
                            if result.blocked {
                                return (
                                    false,
                                    Some(format!(
                                        "virus detected by {}: content from web_fetch (virus: {})",
                                        result.engine, result.virus
                                    )),
                                );
                            }
                        }
                    }
                }
            }
            "screen_capture" | "install_skill" => {
                if !file_path.is_empty() {
                    let result = self.scan_file(Path::new(file_path)).await;
                    if result.blocked {
                        return (
                            false,
                            Some(format!(
                                "virus detected by {}: {} (virus: {})",
                                result.engine, file_path, result.virus
                            )),
                        );
                    }
                }
            }
            "cron" => {
                // cron can schedule shell commands — scan referenced paths
                if let Some(cmd) = args.get("command").and_then(|v| v.as_str()) {
                    if !cmd.is_empty() {
                        let result = self.scan_content(cmd.as_bytes()).await;
                        if result.blocked {
                            return (
                                false,
                                Some(format!(
                                    "virus detected by {}: cron command (virus: {})",
                                    result.engine, result.virus
                                )),
                            );
                        }
                    }
                }
            }
            _ => {}
        }

        (true, None)
    }

    /// Extract file paths from tool arguments based on tool name.
    pub fn extract_paths_from_args(&self, tool_name: &str, args: &serde_json::Value) -> Vec<String> {
        let mut paths = Vec::new();

        match tool_name {
            "file_write" | "file_edit" | "file_append" | "write_file" | "edit_file" | "append_file" => {
                if let Some(path) = args.get("path").and_then(|v| v.as_str()) {
                    paths.push(path.to_string());
                }
                if let Some(path) = args.get("file_path").and_then(|v| v.as_str()) {
                    paths.push(path.to_string());
                }
            }
            "download" | "network_download" => {
                if let Some(path) = args.get("save_path").and_then(|v| v.as_str()) {
                    paths.push(path.to_string());
                }
                if let Some(path) = args.get("path").and_then(|v| v.as_str()) {
                    paths.push(path.to_string());
                }
            }
            "exec" | "shell" | "exec_async" | "process_exec" | "execute_command" => {
                if let Some(cmd) = args.get("command").and_then(|v| v.as_str()) {
                    for part in cmd.split_whitespace() {
                        if part.contains('/') || part.contains('\\') || part.contains('.') {
                            paths.push(part.to_string());
                        }
                    }
                }
            }
            "screen_capture" => {
                if let Some(path) = args.get("save_path").and_then(|v| v.as_str()) {
                    paths.push(path.to_string());
                }
                if let Some(path) = args.get("path").and_then(|v| v.as_str()) {
                    paths.push(path.to_string());
                }
            }
            "install_skill" => {
                if let Some(path) = args.get("path").and_then(|v| v.as_str()) {
                    paths.push(path.to_string());
                }
            }
            "cron" => {
                // Extract paths from scheduled commands
                if let Some(cmd) = args.get("command").and_then(|v| v.as_str()) {
                    for part in cmd.split_whitespace() {
                        if part.contains('/') || part.contains('\\') || part.contains('.') {
                            paths.push(part.to_string());
                        }
                    }
                }
            }
            _ => {}
        }

        paths
    }

    /// Load engines from a scanner configuration.
    ///
    /// Takes a list of engine configs and creates/registers engines
    /// that are in "installed" state.
    pub fn load_from_configs(&mut self, configs: &[ScannerEngineConfig]) {
        for cfg in configs {
            if cfg.install_status == "installed" {
                debug!("Loading scanner engine: {} ({})", cfg.name, cfg.engine_type);
                self.add_engine(Box::new(StubScanner));
            } else {
                debug!("Skipping non-installed engine: {} ({})", cfg.name, cfg.install_status);
            }
        }
    }

    /// Build the scan chain from a `ScannerFullConfig`.
    ///
    /// Only engines listed in `enabled` (in order) are instantiated.
    /// Engines not in "installed" state are silently skipped.
    pub fn load_from_full_config(&mut self, full_config: &ScannerFullConfig) {
        // Store raw configs.
        self.configs = full_config.engines.clone();

        // Instantiate only enabled engines.
        for name in &full_config.enabled {
            let raw_cfg = match full_config.engines.get(name) {
                Some(c) => c,
                None => {
                    warn!("Engine listed in enabled but has no config: {}", name);
                    continue;
                }
            };

            let engine = match create_engine(name, raw_cfg) {
                Ok(e) => e,
                Err(e) => {
                    warn!("Failed to create engine {}: {}", name, e);
                    continue;
                }
            };

            // Skip engines not installed.
            if let Some(state) = raw_cfg.get("state").and_then(|s| s.get("install_status")) {
                if let Some(status) = state.as_str() {
                    if status != INSTALL_STATUS_INSTALLED {
                        debug!("Skipping engine {} (status: {})", name, status);
                        continue;
                    }
                }
            }

            self.engines.push(engine);
        }
    }

    /// Get extension rules from the first engine config that has ClamAV-style
    /// extension settings. Returns empty rules if none found.
    pub fn get_extension_rules(&self) -> ExtensionRules {
        for raw in self.configs.values() {
            let scan_ext: Vec<String> = raw
                .get("scan_extensions")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            let skip_ext: Vec<String> = raw
                .get("skip_extensions")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            if !scan_ext.is_empty() || !skip_ext.is_empty() {
                return ExtensionRules::new(scan_ext, skip_ext);
            }
        }
        ExtensionRules::default()
    }
}

impl Default for ScanChain {
    fn default() -> Self {
        Self::with_defaults()
    }
}

// ---------------------------------------------------------------------------
// Configuration types
// ---------------------------------------------------------------------------

/// Configuration for a single scanner engine.
#[derive(Debug, Clone)]
pub struct ScannerEngineConfig {
    /// Engine name.
    pub name: String,
    /// Engine type (e.g., "clamav", "stub").
    pub engine_type: String,
    /// Installation status: "pending", "installed", "failed".
    pub install_status: String,
}

/// Full scanner configuration with enabled engines list and per-engine configs.
#[derive(Debug, Clone, Default)]
pub struct ScannerFullConfig {
    /// Ordered list of engine names to enable.
    pub enabled: Vec<String>,
    /// Per-engine raw JSON configs, keyed by engine name.
    pub engines: HashMap<String, serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Shared scan chain
// ---------------------------------------------------------------------------

/// Thread-safe wrapper for ScanChain (shared across pipeline layers).
pub type SharedScanChain = Arc<RwLock<ScanChain>>;

/// Create a new shared scan chain with default configuration.
pub fn shared_scan_chain() -> SharedScanChain {
    Arc::new(RwLock::new(ScanChain::with_defaults()))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
