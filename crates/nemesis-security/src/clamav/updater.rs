//! ClamAV virus database updater.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};
use tokio::process::Command;

/// Updater configuration.
#[derive(Debug, Clone)]
pub struct UpdaterConfig {
    pub clamav_path: String,
    pub database_dir: String,
    pub config_file: String,
    pub update_interval: Duration,
    pub mirror_urls: Vec<String>,
}

/// Virus database updater.
pub struct Updater {
    config: UpdaterConfig,
    last_update: std::sync::Mutex<Option<SystemTime>>,
    running: AtomicBool,
}

impl Updater {
    pub fn new(config: UpdaterConfig) -> Self {
        Self {
            config,
            last_update: std::sync::Mutex::new(None),
            running: AtomicBool::new(false),
        }
    }

    /// Run a virus database update.
    pub async fn update(&self) -> Result<(), String> {
        let freshclam_exe = super::find_executable(&self.config.clamav_path, "freshclam");
        if !Path::new(&freshclam_exe).exists() {
            return Err(format!("freshclam not found at {}", freshclam_exe));
        }

        if !self.config.database_dir.is_empty() {
            tokio::fs::create_dir_all(&self.config.database_dir)
                .await
                .map_err(|e| format!("create db dir: {}", e))?;
        }

        let mut cmd = Command::new(&freshclam_exe);
        cmd.current_dir(&self.config.clamav_path);

        if !self.config.config_file.is_empty() {
            cmd.arg("--config-file").arg(&self.config.config_file);
        }
        if !self.config.database_dir.is_empty() {
            cmd.arg("--datadir").arg(&self.config.database_dir);
        }

        let output = cmd.output().await.map_err(|e| format!("freshclam failed: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("freshclam failed: {}", stderr));
        }

        *self.last_update.lock().unwrap() = Some(SystemTime::now());
        tracing::info!("[Scanner] Virus database updated");
        Ok(())
    }

    /// Check if the database is older than the given duration.
    pub fn is_database_stale(&self, max_age: Duration) -> bool {
        let last = self.last_update.lock().unwrap();
        match *last {
            Some(t) => t.elapsed().unwrap_or(Duration::MAX) > max_age,
            None => {
                // Check file modification times
                if !self.config.database_dir.is_empty() {
                    let main_cvd = Path::new(&self.config.database_dir).join("main.cvd");
                    if let Ok(meta) = std::fs::metadata(&main_cvd) {
                        if let Ok(modified) = meta.modified() {
                            if modified.elapsed().unwrap_or(Duration::MAX) <= max_age {
                                return false;
                            }
                        }
                    }
                }
                true
            }
        }
    }

    /// Get the last update time.
    pub fn last_update(&self) -> Option<SystemTime> {
        *self.last_update.lock().unwrap()
    }

    /// Start periodic database updates.
    ///
    /// Mirrors Go's `Updater.StartAutoUpdate`. Runs an update loop on a
    /// ticker using the configured `update_interval`. The loop stops when
    /// `stop()` is called.
    pub async fn start_auto_update(&self) {
        if self.config.update_interval.is_zero() {
            return;
        }

        self.running.store(true, Ordering::SeqCst);

        tracing::info!(
            interval_secs = self.config.update_interval.as_secs(),
            "[Scanner] Auto-update started"
        );

        while self.running.load(Ordering::SeqCst) {
            tokio::time::sleep(self.config.update_interval).await;

            if !self.running.load(Ordering::SeqCst) {
                break;
            }

            // Perform update with a 5-minute timeout
            match tokio::time::timeout(
                Duration::from_secs(300),
                self.update(),
            ).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::error!(error = %e, "[Scanner] Auto-update failed");
                }
                Err(_) => {
                    tracing::error!("[Scanner] Auto-update timed out");
                }
            }
        }

        tracing::info!("[Scanner] Auto-update stopped");
    }

    /// Stop the updater.
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

}

#[cfg(test)]
mod tests;
