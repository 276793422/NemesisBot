//! ClamAV manager - top-level orchestrator.
//!
//! Manages the daemon lifecycle, scanner, updater, and provides the scan hook.
//! Mirrors the Go `clamav.Manager` from `module/security/scanner/clamav/manager.go`.

use super::config::{self, DaemonConfig};
use super::daemon::Daemon;
use super::hook::ScanHook;
use super::scanner::{Scanner, ScannerConfig};
use super::updater::{Updater, UpdaterConfig};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Manager configuration.
#[derive(Debug, Clone)]
pub struct ManagerConfig {
    pub enabled: bool,
    pub clamav_path: String,
    pub data_dir: String,
    pub address: String,
    pub scanner: Option<ScannerConfig>,
    pub update_interval: String,
}

/// Top-level ClamAV integration manager.
///
/// Manages the daemon lifecycle, scanner, updater, and provides the scan hook.
/// The initialization sequence mirrors Go's `Manager.Start()`:
/// 1. Auto-detect ClamAV path if not set
/// 2. Setup data directories (database, config, temp)
/// 3. Generate clamd.conf and freshclam.conf
/// 4. Download virus database if stale
/// 5. Start daemon and wait for readiness
/// 6. Create scanner and hook
/// 7. Start auto-update goroutine if configured
pub struct Manager {
    config: ManagerConfig,
    daemon: Option<Arc<Daemon>>,
    scanner: Option<Arc<Scanner>>,
    updater: Option<Arc<Updater>>,
    hook: Option<Arc<ScanHook>>,
    started: AtomicBool,
}

impl Manager {
    pub fn new(config: ManagerConfig) -> Self {
        Self {
            config,
            daemon: None,
            scanner: None,
            updater: None,
            hook: None,
            started: AtomicBool::new(false),
        }
    }

    /// Start all ClamAV components.
    ///
    /// This performs the full initialization sequence:
    /// 1. Auto-detect ClamAV path if not configured
    /// 2. Create data directories (database, config, temp)
    /// 3. Generate clamd.conf and freshclam.conf configuration files
    /// 4. Download virus database if stale (via freshclam)
    /// 5. Start clamd daemon and wait for readiness
    /// 6. Create scanner connected to the daemon
    /// 7. Create scan hook for security pipeline integration
    /// 8. Start periodic auto-update if configured
    pub async fn start(&mut self) -> Result<(), String> {
        if self.started.load(Ordering::SeqCst) {
            return Err("ClamAV manager already started".to_string());
        }

        if !self.config.enabled {
            tracing::info!("ClamAV integration is disabled");
            return Ok(());
        }

        // Step 1: Auto-detect ClamAV path if not set
        let mut clamav_path = self.config.clamav_path.clone();
        if clamav_path.is_empty() {
            clamav_path = config::detect_clamav_path()
                .ok_or_else(|| "ClamAV installation not found; set clamav_path in config or install ClamAV".to_string())?;
            tracing::info!(path = %clamav_path, "Auto-detected ClamAV");
        }

        // Resolve to absolute path
        let clamav_path = std::path::Path::new(&clamav_path)
            .canonicalize()
            .unwrap_or_else(|_| std::path::PathBuf::from(&clamav_path))
            .to_string_lossy()
            .to_string();

        // Step 2: Setup data directories
        let data_dir = if self.config.data_dir.is_empty() {
            std::env::temp_dir().join("nemesisbot-clamav")
        } else {
            std::path::PathBuf::from(&self.config.data_dir)
        };
        let data_dir = data_dir
            .canonicalize()
            .unwrap_or(data_dir)
            .to_string_lossy()
            .to_string();

        let db_dir = Path::new(&data_dir).join("database");
        let config_dir = Path::new(&data_dir).join("config");
        let temp_dir = Path::new(&data_dir).join("temp");

        for dir in [&db_dir, &config_dir, &temp_dir] {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("failed to create directory {}: {}", dir.display(), e))?;
        }

        // Step 3: Generate configuration files
        let address = if self.config.address.is_empty() {
            "127.0.0.1:3310".to_string()
        } else {
            self.config.address.clone()
        };

        let clamd_conf = config_dir.join("clamd.conf");
        let freshclam_conf = config_dir.join("freshclam.conf");

        let daemon_cfg = DaemonConfig {
            clamav_path: clamav_path.clone(),
            config_file: clamd_conf.to_string_lossy().to_string(),
            database_dir: db_dir.to_string_lossy().to_string(),
            listen_addr: address.clone(),
            temp_dir: temp_dir.to_string_lossy().to_string(),
            ..Default::default()
        };

        config::generate_clamd_config(&daemon_cfg)?;
        config::generate_freshclam_config(
            &db_dir.to_string_lossy(),
            &freshclam_conf.to_string_lossy(),
        )?;

        // Step 4: Download virus database if stale
        let update_interval = parse_duration_string(&self.config.update_interval);
        let updater = Updater::new(UpdaterConfig {
            clamav_path: clamav_path.clone(),
            database_dir: db_dir.to_string_lossy().to_string(),
            config_file: freshclam_conf.to_string_lossy().to_string(),
            update_interval,
            mirror_urls: Vec::new(),
        });

        if updater.is_database_stale(Duration::from_secs(24 * 3600)) {
            tracing::info!("Downloading virus database before starting clamd");
            match updater.update().await {
                Ok(()) => {
                    tracing::info!("Virus database downloaded successfully");
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Initial database download failed");
                }
            }
        }
        self.updater = Some(Arc::new(updater));

        // Step 5: Start daemon
        let daemon = Arc::new(Daemon::new(daemon_cfg));
        daemon.start().await?;
        self.daemon = Some(daemon.clone());

        // Step 6: Create scanner
        let mut scanner_cfg = self.config.scanner.clone().unwrap_or_default();
        scanner_cfg.address = address;
        let scanner = Arc::new(Scanner::new(scanner_cfg));
        self.scanner = Some(scanner.clone());

        // Step 7: Create scan hook
        self.hook = Some(Arc::new(ScanHook::new(scanner)));

        // Step 8: Start auto-update if configured
        if update_interval > Duration::ZERO {
            let updater = self.updater.clone();
            let handle = tokio::spawn(async move {
                if let Some(ref updater) = updater {
                    updater.start_auto_update().await;
                }
            });
            let _ = handle;
        }

        self.started.store(true, Ordering::SeqCst);
        tracing::info!(
            path = %clamav_path,
            address = %self.config.address,
            data_dir = %data_dir,
            "ClamAV manager started"
        );

        Ok(())
    }

    /// Stop all ClamAV components.
    pub async fn stop(&self) -> Result<(), String> {
        if !self.started.load(Ordering::SeqCst) {
            return Ok(());
        }

        // Stop updater
        if let Some(ref updater) = self.updater {
            updater.stop();
        }

        // Stop daemon
        if let Some(ref daemon) = self.daemon {
            daemon.stop().await?;
        }

        self.started.store(false, Ordering::SeqCst);
        tracing::info!("ClamAV manager stopped");
        Ok(())
    }

    /// Check if the manager is running.
    pub fn is_running(&self) -> bool {
        self.started.load(Ordering::SeqCst)
    }

    /// Get the scan hook for security pipeline integration.
    pub fn hook(&self) -> Option<&Arc<ScanHook>> {
        self.hook.as_ref()
    }

    /// Get the virus scanner.
    pub fn scanner(&self) -> Option<&Arc<Scanner>> {
        self.scanner.as_ref()
    }

    /// Get scanning statistics.
    pub async fn get_stats(&self) -> serde_json::Value {
        let mut stats = serde_json::json!({
            "enabled": self.config.enabled,
            "started": self.started.load(Ordering::SeqCst),
        });

        if let Some(ref scanner) = self.scanner {
            let scan_stats = scanner.get_stats().await;
            stats.as_object_mut().unwrap().insert(
                "scanner".to_string(),
                serde_json::json!({
                    "total_scans": scan_stats.total_scans,
                    "clean_scans": scan_stats.clean_scans,
                    "infected_scans": scan_stats.infected_scans,
                    "errors": scan_stats.errors,
                    "total_bytes": scan_stats.total_bytes,
                }),
            );
        }

        if let Some(ref updater) = self.updater {
            if let Some(last_update) = updater.last_update() {
                if let Ok(elapsed) = last_update.elapsed() {
                    stats.as_object_mut().unwrap().insert(
                        "last_update_secs_ago".to_string(),
                        serde_json::Value::Number(elapsed.as_secs().into()),
                    );
                }
            }
        }

        stats
    }
}

/// Parse a duration string (e.g., "24h", "1h30m"). Returns 0 if empty or invalid.
fn parse_duration_string(s: &str) -> Duration {
    if s.is_empty() {
        return Duration::ZERO;
    }
    // Simple parsing for common formats: "24h", "30m", "1h30m"
    let mut total_secs: u64 = 0;
    let mut current_num: u64 = 0;
    for ch in s.chars() {
        if ch.is_ascii_digit() {
            current_num = current_num * 10 + (ch as u64 - '0' as u64);
        } else {
            match ch {
                'h' => total_secs += current_num * 3600,
                'm' => total_secs += current_num * 60,
                's' => total_secs += current_num,
                'd' => total_secs += current_num * 86400,
                _ => return Duration::ZERO,
            }
            current_num = 0;
        }
    }
    Duration::from_secs(total_secs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_parse_duration_24h() {
        assert_eq!(parse_duration_string("24h"), Duration::from_secs(24 * 3600));
    }

    #[test]
    fn test_parse_duration_1h30m() {
        assert_eq!(parse_duration_string("1h30m"), Duration::from_secs(90 * 60));
    }

    #[test]
    fn test_parse_duration_30m() {
        assert_eq!(parse_duration_string("30m"), Duration::from_secs(30 * 60));
    }

    #[test]
    fn test_parse_duration_1d() {
        assert_eq!(parse_duration_string("1d"), Duration::from_secs(86400));
    }

    #[test]
    fn test_parse_duration_seconds() {
        assert_eq!(parse_duration_string("45s"), Duration::from_secs(45));
    }

    #[test]
    fn test_parse_duration_composite() {
        assert_eq!(parse_duration_string("1d2h30m15s"), Duration::from_secs(86400 + 7200 + 1800 + 15));
    }

    #[test]
    fn test_parse_duration_empty() {
        assert_eq!(parse_duration_string(""), Duration::ZERO);
    }

    #[test]
    fn test_parse_duration_invalid() {
        assert_eq!(parse_duration_string("abc"), Duration::ZERO);
    }

    #[test]
    fn test_parse_duration_invalid_mixed() {
        assert_eq!(parse_duration_string("1x"), Duration::ZERO);
    }

    #[test]
    fn test_manager_new() {
        let config = ManagerConfig {
            enabled: false,
            clamav_path: String::new(),
            data_dir: String::new(),
            address: String::new(),
            scanner: None,
            update_interval: String::new(),
        };
        let manager = Manager::new(config);
        assert!(!manager.is_running());
        assert!(manager.hook().is_none());
        assert!(manager.scanner().is_none());
    }

    #[tokio::test]
    async fn test_manager_get_stats_not_started() {
        let config = ManagerConfig {
            enabled: false,
            clamav_path: String::new(),
            data_dir: String::new(),
            address: String::new(),
            scanner: None,
            update_interval: String::new(),
        };
        let manager = Manager::new(config);
        let stats = manager.get_stats().await;
        assert_eq!(stats["enabled"], false);
        assert_eq!(stats["started"], false);
        assert!(stats.get("scanner").is_none());
    }

    #[tokio::test]
    async fn test_manager_stop_when_not_started() {
        let config = ManagerConfig {
            enabled: false,
            clamav_path: String::new(),
            data_dir: String::new(),
            address: String::new(),
            scanner: None,
            update_interval: String::new(),
        };
        let manager = Manager::new(config);
        // Should succeed without error even when not started
        let result = manager.stop().await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_manager_config_debug() {
        let config = ManagerConfig {
            enabled: true,
            clamav_path: "/usr/bin".to_string(),
            data_dir: "/tmp/clamav".to_string(),
            address: "127.0.0.1:3310".to_string(),
            scanner: None,
            update_interval: "24h".to_string(),
        };
        let debug = format!("{:?}", config);
        assert!(debug.contains("enabled"));
        assert!(debug.contains("/usr/bin"));
    }

    #[tokio::test]
    async fn test_manager_start_disabled() {
        let mut manager = Manager::new(ManagerConfig {
            enabled: false,
            clamav_path: String::new(),
            data_dir: String::new(),
            address: String::new(),
            scanner: None,
            update_interval: String::new(),
        });
        let result = manager.start().await;
        assert!(result.is_ok());
        assert!(!manager.is_running());
    }

    #[tokio::test]
    async fn test_manager_start_already_started() {
        let mut manager = Manager::new(ManagerConfig {
            enabled: false,
            clamav_path: String::new(),
            data_dir: String::new(),
            address: String::new(),
            scanner: None,
            update_interval: String::new(),
        });
        manager.started.store(true, Ordering::SeqCst);
        let result = manager.start().await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("already started"));
    }

    #[tokio::test]
    async fn test_manager_start_missing_clamav() {
        let mut manager = Manager::new(ManagerConfig {
            enabled: true,
            clamav_path: "/nonexistent/path".to_string(),
            data_dir: String::new(),
            address: String::new(),
            scanner: None,
            update_interval: String::new(),
        });
        // This will fail because the path doesn't exist
        let result = manager.start().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_manager_get_stats_with_updater() {
        let mut manager = Manager::new(ManagerConfig {
            enabled: false,
            clamav_path: String::new(),
            data_dir: String::new(),
            address: String::new(),
            scanner: None,
            update_interval: String::new(),
        });
        // Manually inject an updater with a recent last_update
        let updater = Arc::new(Updater::new(UpdaterConfig {
            clamav_path: String::new(),
            database_dir: String::new(),
            config_file: String::new(),
            update_interval: Duration::from_secs(3600),
            mirror_urls: Vec::new(),
        }));
        manager.updater = Some(updater);
        let stats = manager.get_stats().await;
        assert_eq!(stats["enabled"], false);
        // last_update_secs_ago should not be present since last_update is None
    }

    #[tokio::test]
    async fn test_manager_hook_and_scanner_none_before_start() {
        let manager = Manager::new(ManagerConfig {
            enabled: false,
            clamav_path: String::new(),
            data_dir: String::new(),
            address: String::new(),
            scanner: None,
            update_interval: String::new(),
        });
        assert!(manager.hook().is_none());
        assert!(manager.scanner().is_none());
    }
}
