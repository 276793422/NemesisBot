//! Persistent workspace state manager.
//!
//! Mirrors Go `module/state/state.go` — provides atomic save/load of workspace
//! state (last channel, last chat ID, timestamp).

use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
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
    #[serde(default = "Utc::now")]
    pub timestamp: DateTime<Utc>,
}

impl Default for WorkspaceState {
    fn default() -> Self {
        Self {
            last_channel: String::new(),
            last_chat_id: String::new(),
            timestamp: Utc::now(),
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
}

impl WorkspaceStateManager {
    /// Create a new state manager for the given workspace directory.
    ///
    /// Creates `<workspace>/state/` if it doesn't exist. If the new state file
    /// doesn't exist, attempts to migrate from the old location
    /// (`<workspace>/state.json`).
    pub fn new(workspace: impl Into<PathBuf>) -> Arc<Self> {
        let workspace = workspace.into();
        let state_dir = workspace.join("state");
        let state_file = state_dir.join("state.json");
        let old_state_file = workspace.join("state.json");

        // Create state directory if it doesn't exist
        let _ = fs::create_dir_all(&state_dir);

        let state = WorkspaceState::default();

        let mgr = Arc::new(Self {
            workspace,
            state: Arc::new(RwLock::new(state)),
            state_file,
        });

        // Try to load from new location first
        if !mgr.state_file.exists() {
            // New file doesn't exist, try migrating from old location
            if let Ok(data) = fs::read_to_string(&old_state_file) {
                if let Ok(loaded) = serde_json::from_str::<WorkspaceState>(&data) {
                    *mgr.state.write() = loaded;
                    // Migrate to new location
                    let _ = mgr.save_atomic();
                    tracing::info!(
                        "state: migrated state from {:?} to {:?}",
                        old_state_file,
                        mgr.state_file
                    );
                }
            }
        } else {
            // Load from new location
            let _ = mgr.load();
        }

        mgr
    }

    /// Atomically update the last channel and save.
    pub fn set_last_channel(&self, channel: &str) -> Result<(), String> {
        {
            let mut state = self.state.write();
            state.last_channel = channel.to_string();
            state.timestamp = Utc::now();
        }
        self.save_atomic()
    }

    /// Atomically update the last chat ID and save.
    pub fn set_last_chat_id(&self, chat_id: &str) -> Result<(), String> {
        {
            let mut state = self.state.write();
            state.last_chat_id = chat_id.to_string();
            state.timestamp = Utc::now();
        }
        self.save_atomic()
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
    pub fn get_timestamp(&self) -> DateTime<Utc> {
        self.state.read().timestamp
    }

    /// Get a snapshot of the entire workspace state.
    pub fn snapshot(&self) -> WorkspaceState {
        self.state.read().clone()
    }

    /// Write state to a temp file, then atomically rename to the target.
    fn save_atomic(&self) -> Result<(), String> {
        let state = self.state.read();
        let data =
            serde_json::to_string_pretty(&*state).map_err(|e| format!("marshal state: {}", e))?;

        let temp_file = self.state_file.with_extension("json.tmp");

        // Write to temp file
        let mut f = fs::File::create(&temp_file)
            .map_err(|e| format!("create temp file {:?}: {}", temp_file, e))?;
        f.write_all(data.as_bytes())
            .map_err(|e| format!("write temp file: {}", e))?;
        f.sync_all()
            .map_err(|e| format!("sync temp file: {}", e))?;
        drop(f);

        // Atomic rename from temp to target
        if let Err(e) = fs::rename(&temp_file, &self.state_file) {
            // Cleanup temp file if rename fails
            let _ = fs::remove_file(&temp_file);
            return Err(format!("rename temp file: {}", e));
        }

        Ok(())
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

/// Internal channels that should not be exposed or recorded as last active.
const INTERNAL_CHANNELS: &[&str] = &["cli", "system", "subagent"];

/// Check if a channel name is an internal channel.
/// Mirrors Go `constants.IsInternalChannel`.
pub fn is_internal_channel(channel: &str) -> bool {
    INTERNAL_CHANNELS.contains(&channel)
}

#[cfg(test)]
mod tests;
