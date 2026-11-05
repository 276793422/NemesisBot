//! Persistent workspace state manager.
//!
//! Mirrors Go `module/state/state.go` — provides atomic save/load of workspace
//! state (last channel, last chat ID, timestamp).

use chrono::{DateTime, Local};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

/// Persistent workspace state.
/// Stored as JSON in `<workspace>/state/state.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceState {
    /// Last channel used for communication (e.g. "web:user123").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_channel: String,
    /// Last chat ID used for communication.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_chat_id: String,
    /// Timestamp of the last state update.
    #[serde(default = "Local::now")]
    pub timestamp: DateTime<Local>,
}

impl Default for WorkspaceState {
    fn default() -> Self {
        Self {
            last_channel: String::new(),
            last_chat_id: String::new(),
            timestamp: Local::now(),
        }
    }
}

/// Manager for persistent workspace state with atomic saves.
///
/// Uses a temp-file + rename pattern to ensure the state file is never corrupted
/// even if the process crashes during a write.
pub struct WorkspaceStateManager {
    #[allow(dead_code)]
    workspace: PathBuf,
    state: Arc<RwLock<WorkspaceState>>,
    state_file: PathBuf,
    /// 测试钩子（仅 test 构建）：快照→落盘之间注入**一次性**停顿（首个
    /// 保存消费后归零），确定性复现并发保存交错（release 构建不存在）。
    #[cfg(test)]
    save_delay_ms: std::sync::atomic::AtomicU64,
}

impl WorkspaceStateManager {
    /// Create a new state manager for the given workspace directory.
    ///
    /// Creates `<workspace>/state/` if it doesn't exist. If the new state file
    /// doesn't exist, attempts to migrate from the old location
    /// (`<workspace>/state.json`).
    pub fn new(workspace: impl Into<PathBuf>) -> Arc<Self> {
        let workspace = workspace.into();
        // 委托 nemesis-path 唯一拼接点。
        let state_dir = nemesis_path::resolve_state_dir_in_workspace(&workspace);
        let state_file = state_dir.join("state.json");
        let old_state_file = workspace.join("state.json");

        // Create state directory if it doesn't exist
        let _ = fs::create_dir_all(&state_dir);

        let state = WorkspaceState::default();

        let mgr = Arc::new(Self {
            workspace,
            state: Arc::new(RwLock::new(state)),
            state_file,
            #[cfg(test)]
            save_delay_ms: std::sync::atomic::AtomicU64::new(0),
        });

        // Try to load from new location first
        if !mgr.state_file.exists() {
            // New file doesn't exist, try migrating from old location
            if let Ok(data) = fs::read_to_string(&old_state_file)
                && let Ok(loaded) = serde_json::from_str::<WorkspaceState>(&data)
            {
                *mgr.state.write() = loaded;
                // Migrate to new location
                let _ = mgr.save_atomic();
                tracing::info!(
                    "state: migrated state from {:?} to {:?}",
                    old_state_file,
                    mgr.state_file
                );
            }
        } else {
            // Load from new location
            let _ = mgr.load();
        }

        mgr
    }

    /// Atomically update the last channel and save.
    pub fn set_last_channel(&self, channel: &str) -> Result<(), String> {
        // 变异+快照+落盘全程同一把写锁——旧形态变异锁
        // 释放后 save_atomic 重新取读锁快照，「快照→rename」对无闸：两写
        // 并发（主 loop max_concurrent_turns 生产默认 8，并发 turn 各自
        // record_last_channel）时旧快照的 rename 可晚于新快照落盘
        // （last-rename-wins = 磁盘丢更新，重启后 last_channel 回退）。
        // 同族（session 侧已修 save_gate，本位点横向扫漏）。
        // 状态文件极小，写锁持穿 fsync 的争用代价可忽略。
        let mut state = self.state.write();
        state.last_channel = channel.to_string();
        state.timestamp = Local::now();
        self.persist(&state)
    }

    /// Atomically update the last chat ID and save.
    pub fn set_last_chat_id(&self, chat_id: &str) -> Result<(), String> {
        // 同 set_last_channel——变异+快照+落盘同一把写锁。
        let mut state = self.state.write();
        state.last_chat_id = chat_id.to_string();
        state.timestamp = Local::now();
        self.persist(&state)
    }

    /// Get the last channel from the state.
    pub fn get_last_channel(&self) -> String {
        self.state.read().last_channel.clone()
    }

    /// Get the last chat ID from the state.
    pub fn get_last_chat_id(&self) -> String {
        self.state.read().last_chat_id.clone()
    }

    /// Get the timestamp of the last state update.
    pub fn get_timestamp(&self) -> DateTime<Local> {
        self.state.read().timestamp
    }

    /// Get a snapshot of the entire workspace state.
    pub fn snapshot(&self) -> WorkspaceState {
        self.state.read().clone()
    }

    /// 序列化给定状态快照并原子落盘。调用方必须**已持 state 写锁**且覆盖
    /// 「变异→本函数落盘」整段——锁跨度分离即复现 竞态
    /// （旧快照 rename 晚落覆盖新值）。测试钩子停顿也在锁内（并发方在写锁
    /// 上排队，无法插入交错窗口）。
    fn persist(&self, state: &WorkspaceState) -> Result<(), String> {
        let data =
            serde_json::to_string_pretty(state).map_err(|e| format!("marshal state: {}", e))?;
        // 测试钩子：确定性制造「快照后、落盘前」的并发窗口（仅 test 构建；
        // 一次性——首个保存消费后归零）。 红证/回归锁用。
        #[cfg(test)]
        {
            let d = self
                .save_delay_ms
                .swap(0, std::sync::atomic::Ordering::SeqCst);
            if d > 0 {
                std::thread::sleep(std::time::Duration::from_millis(d));
            }
        }
        nemesis_utils::file::write_file_atomic(
            &self.state_file.to_string_lossy(),
            data.as_bytes(),
            0o644,
        )
        .map_err(|e| format!("atomic write state file: {e}"))
    }

    /// 迁移路径专用（new() 旧位置收编）：写锁内快照+落盘。
    fn save_atomic(&self) -> Result<(), String> {
        let state = self.state.write();
        self.persist(&state)
    }

    /// 装备一次性「快照后、落盘前」停顿（测试钩子，仅 test 构建）。
    #[cfg(test)]
    fn test_arm_save_delay_ms(&self, ms: u64) {
        self.save_delay_ms
            .store(ms, std::sync::atomic::Ordering::SeqCst);
    }

    /// Load state from disk.
    fn load(&self) -> Result<(), String> {
        let data = fs::read_to_string(&self.state_file).map_err(|e| {
            if self.state_file.exists() {
                format!("read state file: {}", e)
            } else {
                // File doesn't exist yet, that's OK
                String::new()
            }
        })?;

        // Empty error means file simply doesn't exist
        if data.is_empty() {
            return Ok(());
        }

        let loaded: WorkspaceState =
            serde_json::from_str(&data).map_err(|e| format!("unmarshal state: {}", e))?;
        *self.state.write() = loaded;
        Ok(())
    }
}

/// Check if a channel name is an internal channel.
/// Mirrors Go `constants.IsInternalChannel`.
/// 委托仓内统一定义 `nemesis_types::is_internal_channel`
/// （收编，本文件原为手抄常量副本）。
pub fn is_internal_channel(channel: &str) -> bool {
    nemesis_types::is_internal_channel(channel)
}

#[cfg(test)]
mod tests;
