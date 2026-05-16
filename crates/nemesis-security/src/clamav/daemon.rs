//! ClamAV daemon lifecycle management.

use super::client::Client;
use super::config::DaemonConfig;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

/// ClamAV daemon manager.
pub struct Daemon {
    config: DaemonConfig,
    client: Client,
    running: Arc<AtomicBool>,
    process: Arc<Mutex<Option<Child>>>,
}

impl Daemon {
    pub fn new(config: DaemonConfig) -> Self {
        let listen_addr = if config.listen_addr.is_empty() {
            "127.0.0.1:3310".to_string()
        } else {
            config.listen_addr.clone()
        };
        let client = Client::new(&listen_addr);
        Self {
            config,
            client,
            running: Arc::new(AtomicBool::new(false)),
            process: Arc::new(Mutex::new(None)),
        }
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Start the clamd daemon and wait for readiness.
    pub async fn start(&self) -> Result<(), String> {
        if self.running.load(Ordering::SeqCst) {
            return Err("clamd daemon is already running".to_string());
        }

        let clamd_exe = super::find_executable(&self.config.clamav_path, "clamd");
        if !std::path::Path::new(&clamd_exe).exists() {
            return Err(format!("clamd executable not found at {}", clamd_exe));
        }

        if self.config.config_file.is_empty() {
            return Err("clamd config file path is required".to_string());
        }

        let mut cmd = Command::new(&clamd_exe);
        cmd.args(["--config-file", &self.config.config_file, "-F"]);
        cmd.current_dir(&self.config.clamav_path);

        let child = cmd.spawn().map_err(|e| format!("failed to start clamd: {}", e))?;

        *self.process.lock().await = Some(child);
        self.running.store(true, Ordering::SeqCst);

        // Wait for readiness with timeout
        let timeout = Duration::from_secs(self.config.startup_timeout_secs);
        let start = std::time::Instant::now();
        while start.elapsed() < timeout {
            if self.client.ping().await.is_ok() {
                tracing::info!("ClamAV daemon started and ready");
                return Ok(());
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }

        self.stop().await.ok();
        Err(format!("clamd failed to become ready within {:?}", timeout))
    }

    /// Stop the clamd daemon.
    pub async fn stop(&self) -> Result<(), String> {
        if !self.running.load(Ordering::SeqCst) {
            return Ok(());
        }

        let mut proc = self.process.lock().await;
        if let Some(ref mut child) = *proc {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        *proc = None;
        self.running.store(false, Ordering::SeqCst);

        tracing::info!("ClamAV daemon stopped");
        Ok(())
    }

    /// Check if daemon is running.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Check if daemon is responsive.
    pub async fn is_ready(&self) -> bool {
        if !self.is_running() {
            return false;
        }
        self.client.ping().await.is_ok()
    }

    /// Block until the daemon is ready or the context is cancelled.
    ///
    /// Mirrors Go's `Daemon.WaitForReady`. Polls the daemon with 500ms
    /// intervals until `ping()` succeeds or the deadline elapses.
    pub async fn wait_for_ready(&self, deadline: Duration) -> Result<(), String> {
        let start = std::time::Instant::now();
        loop {
            if self.client.ping().await.is_ok() {
                return Ok(());
            }
            if start.elapsed() >= deadline {
                return Err("wait_for_ready timed out".to_string());
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> DaemonConfig {
        DaemonConfig {
            clamav_path: "/usr/bin".to_string(),
            config_file: "/tmp/clamd.conf".to_string(),
            database_dir: "/tmp/db".to_string(),
            listen_addr: "127.0.0.1:3310".to_string(),
            temp_dir: "/tmp".to_string(),
            startup_timeout_secs: 120,
        }
    }

    #[test]
    fn test_daemon_config_defaults() {
        let cfg = DaemonConfig::default();
        assert!(cfg.clamav_path.is_empty());
        assert!(cfg.config_file.is_empty());
        assert!(cfg.database_dir.is_empty());
        assert_eq!(cfg.listen_addr, "127.0.0.1:3310");
        assert!(cfg.temp_dir.is_empty());
        assert_eq!(cfg.startup_timeout_secs, 120);
    }

    #[test]
    fn test_daemon_new() {
        let daemon = Daemon::new(test_config());
        assert!(!daemon.is_running());
    }

    #[test]
    fn test_daemon_new_empty_listen_addr() {
        let mut cfg = test_config();
        cfg.listen_addr = String::new();
        let daemon = Daemon::new(cfg);
        assert!(!daemon.is_running());
        // Verify the client was configured with default address
        assert_eq!(daemon.client().address(), "127.0.0.1:3310");
    }

    #[test]
    fn test_daemon_is_running_initially_false() {
        let daemon = Daemon::new(test_config());
        assert!(!daemon.is_running());
    }

    #[test]
    fn test_daemon_is_ready_not_running() {
        let daemon = Daemon::new(test_config());
        let rt = tokio::runtime::Runtime::new().unwrap();
        assert!(!rt.block_on(async { daemon.is_ready().await }));
    }

    #[test]
    fn test_daemon_client() {
        let daemon = Daemon::new(test_config());
        assert_eq!(daemon.client().address(), "127.0.0.1:3310");
    }

    #[tokio::test]
    async fn test_daemon_stop_when_not_running() {
        let daemon = Daemon::new(test_config());
        let result = daemon.stop().await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_find_executable() {
        let exe = super::super::find_executable("/usr/bin", "clamd");
        if cfg!(target_os = "windows") {
            assert!(exe.ends_with("clamd.exe"));
        } else {
            assert!(exe.ends_with("clamd"));
        }
    }

    #[tokio::test]
    async fn test_daemon_start_already_running() {
        let daemon = Daemon::new(test_config());
        daemon.running.store(true, Ordering::SeqCst);
        let result = daemon.start().await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("already running"));
    }

    #[tokio::test]
    async fn test_daemon_start_exe_not_found() {
        let daemon = Daemon::new(DaemonConfig {
            clamav_path: "/nonexistent/path".to_string(),
            config_file: "/tmp/clamd.conf".to_string(),
            ..Default::default()
        });
        let result = daemon.start().await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[tokio::test]
    async fn test_daemon_start_empty_config_file() {
        let daemon = Daemon::new(DaemonConfig {
            clamav_path: "/usr/bin".to_string(),
            config_file: String::new(),
            ..Default::default()
        });
        // If clamd exists at /usr/bin, this will fail due to empty config file
        // If clamd doesn't exist, it will fail due to missing exe
        let result = daemon.start().await;
        // Either way it should fail
        assert!(result.is_err());
    }

    #[test]
    fn test_daemon_config_debug() {
        let cfg = test_config();
        let debug = format!("{:?}", cfg);
        assert!(debug.contains("/usr/bin"));
        assert!(debug.contains("3310"));
    }

    #[test]
    fn test_daemon_is_ready_when_running_but_no_daemon() {
        let daemon = Daemon::new(test_config());
        daemon.running.store(true, Ordering::SeqCst);
        // Running is true but no actual daemon, so ping should fail
        let rt = tokio::runtime::Runtime::new().unwrap();
        assert!(!rt.block_on(async { daemon.is_ready().await }));
        daemon.running.store(false, Ordering::SeqCst);
    }

    // ============================================================
    // Additional coverage tests
    // ============================================================

    #[test]
    fn test_daemon_config_default_values() {
        let cfg = DaemonConfig::default();
        assert_eq!(cfg.startup_timeout_secs, 120);
        assert_eq!(cfg.listen_addr, "127.0.0.1:3310");
    }

    #[test]
    fn test_daemon_new_with_custom_address() {
        let mut cfg = test_config();
        cfg.listen_addr = "192.168.1.1:9999".to_string();
        let daemon = Daemon::new(cfg);
        assert_eq!(daemon.client().address(), "192.168.1.1:9999");
        assert!(!daemon.is_running());
    }

    #[test]
    fn test_daemon_is_running_flag_toggle() {
        let daemon = Daemon::new(test_config());
        assert!(!daemon.is_running());
        daemon.running.store(true, Ordering::SeqCst);
        assert!(daemon.is_running());
        daemon.running.store(false, Ordering::SeqCst);
        assert!(!daemon.is_running());
    }

    #[tokio::test]
    async fn test_daemon_stop_idempotent() {
        let daemon = Daemon::new(test_config());
        // Stop when not running should succeed
        assert!(daemon.stop().await.is_ok());
        // Stop again should still succeed
        assert!(daemon.stop().await.is_ok());
    }

    #[test]
    fn test_daemon_client_default_address_on_empty() {
        let mut cfg = test_config();
        cfg.listen_addr = String::new();
        let daemon = Daemon::new(cfg);
        assert_eq!(daemon.client().address(), "127.0.0.1:3310");
    }

    #[tokio::test]
    async fn test_daemon_start_already_running_different_state() {
        let daemon = Daemon::new(test_config());
        // Set running to true, then try to start
        daemon.running.store(true, Ordering::SeqCst);
        let result = daemon.start().await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("already running"));
        // Reset state
        daemon.running.store(false, Ordering::SeqCst);
    }

    #[tokio::test]
    async fn test_daemon_start_empty_clamav_path() {
        let daemon = Daemon::new(DaemonConfig {
            clamav_path: String::new(),
            config_file: "/tmp/clamd.conf".to_string(),
            ..Default::default()
        });
        let result = daemon.start().await;
        assert!(result.is_err());
        // Should fail because clamd not found
    }

    #[test]
    fn test_daemon_config_clone() {
        let cfg = test_config();
        let cloned = cfg.clone();
        assert_eq!(cfg.clamav_path, cloned.clamav_path);
        assert_eq!(cfg.config_file, cloned.config_file);
        assert_eq!(cfg.listen_addr, cloned.listen_addr);
        assert_eq!(cfg.startup_timeout_secs, cloned.startup_timeout_secs);
    }

    #[tokio::test]
    async fn test_daemon_is_ready_returns_false_when_not_running() {
        let daemon = Daemon::new(test_config());
        assert!(!daemon.is_running());
        // is_ready checks is_running first, so should return false
        assert!(!daemon.is_ready().await);
    }

    #[tokio::test]
    async fn test_daemon_process_initially_none() {
        let daemon = Daemon::new(test_config());
        // The internal process should be None
        let proc = daemon.process.lock().await;
        assert!(proc.is_none());
    }
}
