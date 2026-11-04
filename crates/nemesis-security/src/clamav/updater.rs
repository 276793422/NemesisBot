//! ClamAV virus database updater.

use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
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
    ///
    /// `cancel_token` allows the caller to abort the update by killing the
    /// freshclam child process. `on_progress` is called with `(0, 0)` at the
    /// start and `(100, 100)` on completion.
    pub async fn update(
        &self,
        cancel_token: tokio_util::sync::CancellationToken,
        on_progress: Option<Arc<dyn Fn(u64, u64) + Send + Sync>>,
    ) -> Result<(), String> {
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
        // kill_on_drop：外层超时（auto-update 300s / CLI install 120s） drop
        // update() future 时，Child 句柄随之 drop——无此标志 freshclam 成孤儿
        // 继续后台跑（占 DB 目录，下个 tick 撞 freshclam 内部锁）。对齐
        // daemon.rs clamd 同款 belt-and-suspenders；正常路径 child.wait()
        // 显式收尸，不受影响。
        cmd.kill_on_drop(true);

        if !self.config.config_file.is_empty() {
            cmd.arg("--config-file").arg(&self.config.config_file);
        }
        if !self.config.database_dir.is_empty() {
            cmd.arg("--datadir").arg(&self.config.database_dir);
        }

        if let Some(ref cb) = on_progress {
            cb(0, 0);
        }

        // Discard freshclam's inherited stdio. Its normal log already goes to
        // `UpdateLogFile` from freshclam.conf; this prevents the output from
        // leaking into the NemesisBot log.
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::null());
        // freshclam 是 console 程序；gateway 托盘/无控制台运行（release
        // windows 子系统，2026-09-21）时压掉弹窗（stdio 已 null，不受影响）。
        #[cfg(target_os = "windows")]
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("freshclam failed: {}", e))?;

        let status = tokio::select! {
            _ = cancel_token.cancelled() => {
                let _ = child.kill().await;
                return Err("database update cancelled".to_string());
            }
            result = child.wait() => {
                result.map_err(|e| format!("freshclam wait failed: {}", e))?
            }
        };

        if !status.success() {
            return Err("freshclam exited with non-zero status".to_string());
        }

        *self.last_update.lock().unwrap() = Some(SystemTime::now());

        if let Some(ref cb) = on_progress {
            cb(100, 100);
        }

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
                    // BUG-20261104-376：freshclam 日常只滚动 daily.cvd/cld
                    // （main.cvd 数月一更，bytecode.cvd 偶更）——只看
                    // main.cvd 的 mtime，DB 明明新鲜也恒判 stale，每次
                    // Manager.start 都白跑一遍 freshclam（联网延迟 + 离线
                    // 启动噪音），24h 新鲜度闸形同虚设。取全家族文件
                    // （main/daily/bytecode × cvd/cld）的最新 mtime 做新鲜
                    // 度 = 最后一次 freshclam 实际落盘时间；全缺 = stale。
                    let dir = Path::new(&self.config.database_dir);
                    let mut newest: Option<SystemTime> = None;
                    for name in [
                        "main.cvd",
                        "main.cld",
                        "daily.cvd",
                        "daily.cld",
                        "bytecode.cvd",
                        "bytecode.cld",
                    ] {
                        if let Ok(meta) = std::fs::metadata(dir.join(name))
                            && let Ok(m) = meta.modified()
                            && newest.is_none_or(|n| m > n)
                        {
                            newest = Some(m);
                        }
                    }
                    if let Some(n) = newest
                        && n.elapsed().unwrap_or(Duration::MAX) <= max_age
                    {
                        return false;
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
                self.update(tokio_util::sync::CancellationToken::new(), None),
            )
            .await
            {
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
