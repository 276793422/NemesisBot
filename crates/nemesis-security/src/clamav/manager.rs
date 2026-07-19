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
            tracing::info!("[Scanner] ClamAV integration is disabled");
            return Ok(());
        }

        // Step 1: Auto-detect ClamAV path if not set
        let mut clamav_path = self.config.clamav_path.clone();
        if clamav_path.is_empty() {
            clamav_path = config::detect_clamav_path()
                .ok_or_else(|| "ClamAV installation not found; set clamav_path in config or install ClamAV".to_string())?;
            tracing::info!(path = %clamav_path, "[Scanner] Auto-detected ClamAV");
        }

        // Resolve to absolute path
        let clamav_path = std::path::Path::new(&clamav_path)
            .canonicalize()
            .unwrap_or_else(|_| std::path::PathBuf::from(&clamav_path))
            .to_string_lossy()
            .to_string();

        // Step 2: Setup data directories. Default to the ClamAV install dir:
        // its `database/` subdir is where the virus DB already lives. Do NOT
        // fall back to the system temp dir — that directory is empty, so clamd
        // would load zero signatures and silently report every file as clean.
        let data_dir = if self.config.data_dir.is_empty() {
            std::path::PathBuf::from(&clamav_path)
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
            tracing::info!("[Scanner] Downloading virus database before starting clamd");
            match updater.update(tokio_util::sync::CancellationToken::new(), None).await {
                Ok(()) => {
                    tracing::info!("[Scanner] Virus database downloaded successfully");
                }
                Err(e) => {
                    tracing::warn!(error = %e, "[Scanner] Initial database download failed");
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
            "[Scanner] ClamAV manager started"
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
        tracing::info!("[Scanner] ClamAV manager stopped");
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
mod tests;
