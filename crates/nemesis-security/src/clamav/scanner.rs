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
            tracing::warn!(path = %file_path.display(), virus = %result.virus, "Virus detected");
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
                    "Virus detected in directory scan"
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
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_config() -> ScannerConfig {
        ScannerConfig {
            enabled: true,
            address: "127.0.0.1:3310".to_string(),
            scan_on_write: true,
            scan_on_download: true,
            scan_on_exec: true,
            max_file_size: 50 * 1024 * 1024,
            timeout: Duration::from_secs(60),
        }
    }

    fn disabled_config() -> ScannerConfig {
        ScannerConfig {
            enabled: false,
            ..test_config()
        }
    }

    #[test]
    fn test_should_scan_write_file() {
        let scanner = Scanner::new(test_config());
        assert!(scanner.should_scan("write_file"));
    }

    #[test]
    fn test_should_scan_edit_file() {
        let scanner = Scanner::new(test_config());
        assert!(scanner.should_scan("edit_file"));
    }

    #[test]
    fn test_should_scan_append_file() {
        let scanner = Scanner::new(test_config());
        assert!(scanner.should_scan("append_file"));
    }

    #[test]
    fn test_should_scan_download() {
        let scanner = Scanner::new(test_config());
        assert!(scanner.should_scan("download"));
    }

    #[test]
    fn test_should_scan_exec() {
        let scanner = Scanner::new(test_config());
        assert!(scanner.should_scan("exec"));
    }

    #[test]
    fn test_should_scan_execute_command() {
        let scanner = Scanner::new(test_config());
        assert!(scanner.should_scan("execute_command"));
    }

    #[test]
    fn test_should_scan_unknown() {
        let scanner = Scanner::new(test_config());
        assert!(!scanner.should_scan("unknown"));
        assert!(!scanner.should_scan("read_file"));
        assert!(!scanner.should_scan("list_dir"));
    }

    #[test]
    fn test_should_scan_disabled() {
        let scanner = Scanner::new(disabled_config());
        assert!(!scanner.should_scan("write_file"));
        assert!(!scanner.should_scan("download"));
        assert!(!scanner.should_scan("exec"));
    }

    #[test]
    fn test_should_scan_file_safe_extensions() {
        let scanner = Scanner::new(test_config());
        // Safe extensions should NOT be scanned
        assert!(!scanner.should_scan_file(&PathBuf::from("test.txt")));
        assert!(!scanner.should_scan_file(&PathBuf::from("readme.md")));
        assert!(!scanner.should_scan_file(&PathBuf::from("data.json")));
        assert!(!scanner.should_scan_file(&PathBuf::from("config.yaml")));
        assert!(!scanner.should_scan_file(&PathBuf::from("config.yml")));
        assert!(!scanner.should_scan_file(&PathBuf::from("data.xml")));
        assert!(!scanner.should_scan_file(&PathBuf::from("data.csv")));
        assert!(!scanner.should_scan_file(&PathBuf::from("app.log")));
        assert!(!scanner.should_scan_file(&PathBuf::from("app.ini")));
        assert!(!scanner.should_scan_file(&PathBuf::from("app.toml")));
        assert!(!scanner.should_scan_file(&PathBuf::from("page.html")));
        assert!(!scanner.should_scan_file(&PathBuf::from("style.css")));
        assert!(!scanner.should_scan_file(&PathBuf::from("app.js")));
        assert!(!scanner.should_scan_file(&PathBuf::from("app.ts")));
    }

    #[test]
    fn test_should_scan_file_executable_extensions() {
        let scanner = Scanner::new(test_config());
        // Executable extensions should always be scanned
        assert!(scanner.should_scan_file(&PathBuf::from("program.exe")));
        assert!(scanner.should_scan_file(&PathBuf::from("library.dll")));
        assert!(scanner.should_scan_file(&PathBuf::from("script.bat")));
        assert!(scanner.should_scan_file(&PathBuf::from("script.cmd")));
        assert!(scanner.should_scan_file(&PathBuf::from("script.ps1")));
        assert!(scanner.should_scan_file(&PathBuf::from("script.sh")));
        assert!(scanner.should_scan_file(&PathBuf::from("lib.so")));
        assert!(scanner.should_scan_file(&PathBuf::from("lib.dylib")));
        assert!(scanner.should_scan_file(&PathBuf::from("setup.msi")));
        assert!(scanner.should_scan_file(&PathBuf::from("script.vbs")));
        assert!(scanner.should_scan_file(&PathBuf::from("program.com")));
        assert!(scanner.should_scan_file(&PathBuf::from("screen.scr")));
        assert!(scanner.should_scan_file(&PathBuf::from("app.jar")));
        assert!(scanner.should_scan_file(&PathBuf::from("script.py")));
    }

    #[test]
    fn test_should_scan_file_unknown_extension() {
        let scanner = Scanner::new(test_config());
        // Unknown extensions should be scanned (conservative)
        assert!(scanner.should_scan_file(&PathBuf::from("data.xyz")));
        assert!(scanner.should_scan_file(&PathBuf::from("archive.zip")));
        assert!(scanner.should_scan_file(&PathBuf::from("file")));
    }

    #[test]
    fn test_should_scan_file_disabled() {
        let scanner = Scanner::new(disabled_config());
        // When disabled, nothing should be scanned
        assert!(!scanner.should_scan_file(&PathBuf::from("program.exe")));
        assert!(!scanner.should_scan_file(&PathBuf::from("data.xyz")));
    }

    #[test]
    fn test_default_scanner_config_values() {
        let cfg = default_scanner_config();
        assert!(cfg.enabled);
        assert_eq!(cfg.address, "127.0.0.1:3310");
        assert!(cfg.scan_on_write);
        assert!(cfg.scan_on_download);
        assert!(cfg.scan_on_exec);
        assert_eq!(cfg.max_file_size, 50 * 1024 * 1024);
        assert_eq!(cfg.timeout, Duration::from_secs(60));
    }

    #[tokio::test]
    async fn test_get_stats_initial() {
        let scanner = Scanner::new(test_config());
        let stats = scanner.get_stats().await;
        assert_eq!(stats.total_scans, 0);
        assert_eq!(stats.clean_scans, 0);
        assert_eq!(stats.infected_scans, 0);
        assert_eq!(stats.errors, 0);
        assert_eq!(stats.total_bytes, 0);
    }

    #[tokio::test]
    async fn test_scan_file_disabled() {
        let scanner = Scanner::new(disabled_config());
        let result = scanner.scan_file(Path::new("/tmp/test.txt")).await.unwrap();
        assert!(!result.infected);
        assert_eq!(result.raw, "scanning disabled");
    }

    #[tokio::test]
    async fn test_scan_content_disabled() {
        let scanner = Scanner::new(disabled_config());
        let result = scanner.scan_content(b"hello world").await.unwrap();
        assert!(!result.infected);
        assert_eq!(result.raw, "scanning disabled");
    }

    #[tokio::test]
    async fn test_scan_directory_disabled() {
        let scanner = Scanner::new(disabled_config());
        let results = scanner.scan_directory(Path::new("/tmp")).await.unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_scan_stats_default() {
        let stats = ScanStats::default();
        assert_eq!(stats.total_scans, 0);
        assert_eq!(stats.clean_scans, 0);
        assert_eq!(stats.infected_scans, 0);
        assert_eq!(stats.errors, 0);
        assert_eq!(stats.total_bytes, 0);
    }

    #[test]
    fn test_scanner_config_debug() {
        let config = test_config();
        let debug = format!("{:?}", config);
        assert!(debug.contains("enabled"));
        assert!(debug.contains("3310"));
    }

    #[test]
    fn test_scanner_config_clone() {
        let config = test_config();
        let cloned = config.clone();
        assert_eq!(cloned.address, config.address);
        assert_eq!(cloned.max_file_size, config.max_file_size);
    }

    #[tokio::test]
    async fn test_scanner_new_with_client() {
        let client = Client::new("127.0.0.1:3310");
        let scanner = Scanner::new_with_client(client, test_config());
        assert!(scanner.ping().await.is_err()); // no daemon running
    }

    #[tokio::test]
    async fn test_scan_file_too_large() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("large.bin");
        std::fs::write(&file_path, vec![0u8; 1024]).unwrap();
        let config = ScannerConfig {
            enabled: true,
            max_file_size: 100, // Very small limit
            ..test_config()
        };
        let scanner = Scanner::new(config);
        let result = scanner.scan_file(&file_path).await.unwrap();
        assert!(!result.infected);
        assert!(result.raw.contains("too large"));
    }

    #[tokio::test]
    async fn test_scan_content_too_large() {
        let config = ScannerConfig {
            enabled: true,
            max_file_size: 10, // Very small limit
            ..test_config()
        };
        let scanner = Scanner::new(config);
        let data = vec![0u8; 100];
        let result = scanner.scan_content(&data).await.unwrap();
        assert!(!result.infected);
        assert!(result.raw.contains("too large"));
    }

    #[test]
    fn test_should_scan_file_no_extension() {
        let scanner = Scanner::new(test_config());
        // File without extension should be scanned (unknown extension)
        assert!(scanner.should_scan_file(&PathBuf::from("Makefile")));
        assert!(scanner.should_scan_file(&PathBuf::from("README")));
    }

    #[test]
    fn test_should_scan_file_pif_extension() {
        let scanner = Scanner::new(test_config());
        assert!(scanner.should_scan_file(&PathBuf::from("program.pif")));
    }

    #[tokio::test]
    async fn test_record_scan_stats() {
        let scanner = Scanner::new(test_config());
        // Manually record scans
        scanner.record_scan(100, false, false).await;
        scanner.record_scan(200, true, false).await;
        scanner.record_scan(50, false, true).await;

        let stats = scanner.get_stats().await;
        assert_eq!(stats.total_scans, 3);
        assert_eq!(stats.clean_scans, 1);
        assert_eq!(stats.infected_scans, 1);
        assert_eq!(stats.errors, 1);
        assert_eq!(stats.total_bytes, 350);
    }

    #[test]
    fn test_scan_stats_debug() {
        let stats = ScanStats {
            total_scans: 10,
            clean_scans: 8,
            infected_scans: 1,
            errors: 1,
            total_bytes: 4096,
        };
        let debug = format!("{:?}", stats);
        assert!(debug.contains("10"));
        assert!(debug.contains("4096"));
    }

    #[test]
    fn test_default_scanner_config_function() {
        let cfg = default_scanner_config();
        assert!(cfg.enabled);
        assert_eq!(cfg.address, "127.0.0.1:3310");
    }

    #[tokio::test]
    async fn test_scan_file_nonexistent_when_enabled() {
        let scanner = Scanner::new(test_config());
        // scan_file calls client.scan_file which connects to clamd
        // When clamd is not running, this should return an error
        let result = scanner.scan_file(Path::new("/tmp/nonexistent_file_for_test.txt")).await;
        // Will fail because no clamd daemon
        assert!(result.is_err());
    }

    #[test]
    fn test_should_scan_file_scr_extension() {
        let scanner = Scanner::new(test_config());
        assert!(scanner.should_scan_file(&PathBuf::from("screensaver.scr")));
    }

    #[test]
    fn test_should_scan_file_com_extension() {
        let scanner = Scanner::new(test_config());
        assert!(scanner.should_scan_file(&PathBuf::from("program.com")));
    }
}
