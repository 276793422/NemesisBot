//! ClamAV daemon lifecycle management.

use super::client::Client;
use super::config::DaemonConfig;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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
        // Kill clamd when the Child handle drops — otherwise the daemon orphans
        // on gateway exit, holds port 3310 (~1GB RAM), and the next gateway
        // start hits a port conflict → ping-only fallback. Belt-and-suspenders
        // alongside the explicit stop_scanner() in the gateway shutdown path.
        cmd.kill_on_drop(true);

        let child = cmd
            .spawn()
            .map_err(|e| format!("failed to start clamd: {}", e))?;

        *self.process.lock().await = Some(child);
        self.running.store(true, Ordering::SeqCst);

        // Wait for readiness with timeout
        let timeout = Duration::from_secs(self.config.startup_timeout_secs);
        let start = std::time::Instant::now();
        while start.elapsed() < timeout {
            if self.client.ping().await.is_ok() {
                tracing::info!("[Scanner] ClamAV daemon started and ready");
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

        tracing::info!("[Scanner] ClamAV daemon stopped");
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
mod tests;
