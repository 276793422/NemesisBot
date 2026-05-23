//! ClamAV scanner - high-level virus scanning operations.

use super::client::{Client, ClamavScanResult};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

const SAFE_EXTENSIONS: &[&str] = &["txt", "md", "json", "yaml", "yml", "xml", "csv", "log", "ini", "toml", "html", "css", "js", "ts"];
const EXEC_EXTENSIONS: &[&str] = &["exe", "dll", "bat", "cmd", "ps1", "sh", "so", "dylib", "msi", "vbs", "com", "scr", "pif", "jar", "py"];

/// Scanner configuration.
#[derive(Debug, Clone)]
pub struct ScannerConfig {
    pub enabled: bool,
    pub address: String,
    pub scan_on_write: bool,
    pub scan_on_download: bool,
    pub scan_on_exec: bool,
    pub max_file_size: u64,
    pub timeout: Duration,
}

impl Default for ScannerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            address: "127.0.0.1:3310".to_string(),
            scan_on_write: true,
            scan_on_download: true,
            scan_on_exec: true,
            max_file_size: 50 * 1024 * 1024,
            timeout: Duration::from_secs(60),
        }
    }
}

/// Returns the default scanner configuration.
///
/// Mirrors Go's `DefaultScannerConfig()`.
pub fn default_scanner_config() -> ScannerConfig {
    ScannerConfig::default()
}

/// Scan statistics.
#[derive(Debug, Clone, Default)]
pub struct ScanStats {
    pub total_scans: u64,
    pub clean_scans: u64,
    pub infected_scans: u64,
    pub errors: u64,
    pub total_bytes: u64,
}

/// High-level virus scanner.
pub struct Scanner {
    client: Client,
    config: ScannerConfig,
    stats: Arc<Mutex<ScanStats>>,
}

impl Scanner {
    pub fn new(config: ScannerConfig) -> Self {
        let client = Client::with_timeout(&config.address, config.timeout);
        Self {
            client,
            config,
            stats: Arc::new(Mutex::new(ScanStats::default())),
        }
    }

    /// Create a scanner with an existing client (dependency injection).
    ///
    /// Mirrors Go's `NewScannerWithClient`. Useful for testing with mock
    /// clients or custom client configurations.
    pub fn new_with_client(client: Client, config: ScannerConfig) -> Self {
        Self {
            client,
            config,
            stats: Arc::new(Mutex::new(ScanStats::default())),
        }
    }

    /// Scan a file by path.
    pub async fn scan_file(&self, file_path: &Path) -> Result<ClamavScanResult, String> {
        if !self.config.enabled {
            return Ok(ClamavScanResult {
                path: file_path.to_string_lossy().to_string(),
                infected: false,
                virus: String::new(),
                raw: "scanning disabled".to_string(),
            });
        }

        if self.config.max_file_size > 0 {
            if let Ok(meta) = tokio::fs::metadata(file_path).await {
                if meta.len() > self.config.max_file_size {
                    return Ok(ClamavScanResult {
                        path: file_path.to_string_lossy().to_string(),
                        infected: false,
                        virus: String::new(),
                        raw: format!("file too large ({} bytes)", meta.len()),
                    });
                }
            }
        }

        let result = self.client.scan_file(file_path).await?;
        self.record_scan(0, result.infected, false).await;

        if result.infected {
            tracing::warn!(path = %file_path.display(), virus = %result.virus, "[Scanner] Virus detected");
        }

        Ok(result)
    }

    /// Scan content bytes.
    pub async fn scan_content(&self, data: &[u8]) -> Result<ClamavScanResult, String> {
        if !self.config.enabled {
            return Ok(ClamavScanResult {
                path: String::new(),
                infected: false,
                virus: String::new(),
                raw: "scanning disabled".to_string(),
            });
        }

        if self.config.max_file_size > 0 && data.len() as u64 > self.config.max_file_size {
            return Ok(ClamavScanResult {
                path: String::new(),
                infected: false,
                virus: String::new(),
                raw: format!("content too large ({} bytes)", data.len()),
            });
        }

        let result = self.client.scan_stream(data).await?;
        self.record_scan(data.len() as u64, result.infected, false).await;
        Ok(result)
    }

    /// Check if a file operation should trigger a scan.
    pub fn should_scan(&self, operation: &str) -> bool {
        if !self.config.enabled {
            return false;
        }
        match operation {
            "write_file" | "edit_file" | "append_file" => self.config.scan_on_write,
            "download" => self.config.scan_on_download,
            "exec" | "execute_command" => self.config.scan_on_exec,
            _ => false,
        }
    }

    /// Check if a specific file should be scanned based on its extension.
    ///
    /// Mirrors Go's `Scanner.ShouldScanFile`.
    pub fn should_scan_file(&self, file_path: &Path) -> bool {
        if !self.config.enabled {
            return false;
        }

        let ext = file_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        // Skip known safe file types
        if SAFE_EXTENSIONS.contains(&ext.as_str()) {
            return false;
        }

        // Always scan executable types
        if EXEC_EXTENSIONS.contains(&ext.as_str()) {
            return true;
        }

        // Scan unknown extensions (conservative approach)
        true
    }

    /// Scan all files in a directory.
    ///
    /// Mirrors Go's `Scanner.ScanDirectory`.
    pub async fn scan_directory(&self, dir_path: &Path) -> Result<Vec<ClamavScanResult>, String> {
        if !self.config.enabled {
            return Ok(Vec::new());
        }

        let results = self.client.cont_scan(dir_path).await?;

        for r in &results {
            self.record_scan(0, r.infected, false).await;
            if r.infected {
                tracing::warn!(
                    path = %r.path,
                    virus = %r.virus,
                    dir = %dir_path.display(),
                    "[Scanner] Virus detected in directory scan"
                );
            }
        }

        Ok(results)
    }

    /// Get scan statistics.
    pub async fn get_stats(&self) -> ScanStats {
        self.stats.lock().await.clone()
    }

    /// Ping the scanner backend.
    pub async fn ping(&self) -> Result<(), String> {
        self.client.ping().await
    }

    async fn record_scan(&self, bytes: u64, infected: bool, is_error: bool) {
        let mut stats = self.stats.lock().await;
        stats.total_scans += 1;
        stats.total_bytes += bytes;
        if is_error {
            stats.errors += 1;
        } else if infected {
            stats.infected_scans += 1;
        } else {
            stats.clean_scans += 1;
        }
    }
}

#[cfg(test)]
mod tests;
