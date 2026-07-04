//! `FileCheckpointStore` — milestone 1b-A1 step 3.
//!
//! Directory layout (per CLAUDE.md plan):
//!   `{root}/checkpoints/{execution_id}/{checkpoint_id}.json`
//!
//! Each checkpoint is one JSON file. We deliberately scan the directory on
//! `latest()` / `list()` instead of maintaining an in-memory index — the OS
//! page cache makes this fast for the scales we care about, and we avoid the
//! cache-coherence bugs that an index would introduce across processes.
//!
//! Corrupt files (unparseable JSON) are quarantined to
//!   `{execution_id}/.corrupt/{checkpoint_id}.json`
//! so a single bad write doesn't poison `latest()` for the whole execution.

use std::fs;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use chrono::Utc;
use tracing::warn;

use super::store::{CheckpointStore, StoreError};
use super::types::{Checkpoint, CheckpointMeta};

/// Filesystem-backed [`CheckpointStore`].
///
/// `root` is the parent directory; checkpoints live under
/// `{root}/checkpoints/`. Gateway points this at
/// `{home}/workspace/workflow/checkpoints/` so the layout becomes
/// `{home}/workspace/workflow/checkpoints/{exec}/{cp}.json`.
pub struct FileCheckpointStore {
    root: PathBuf,
}

impl FileCheckpointStore {
    /// Create the store, ensuring the `checkpoints/` subdirectory exists.
    pub fn new<P: AsRef<Path>>(root: P) -> Result<Self, StoreError> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(root.join("checkpoints"))?;
        Ok(Self { root })
    }

    fn checkpoints_dir(&self) -> PathBuf {
        self.root.join("checkpoints")
    }

    fn exec_dir(&self, execution_id: &str) -> PathBuf {
        self.checkpoints_dir().join(sanitize_path_component(execution_id))
    }

    fn checkpoint_path(&self, execution_id: &str, checkpoint_id: &str) -> PathBuf {
        self.exec_dir(execution_id)
            .join(format!("{}.json", sanitize_path_component(checkpoint_id)))
    }

    fn corrupt_dir(&self, execution_id: &str) -> PathBuf {
        self.exec_dir(execution_id).join(".corrupt")
    }

    /// Atomically write `bytes` to `final_path` via tmp-file + rename.
    /// `tmp_suffix` disambiguates concurrent writes for different checkpoints.
    fn atomic_write(final_path: &Path, bytes: &[u8], tmp_suffix: &str) -> Result<(), StoreError> {
        if let Some(parent) = final_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp_path = final_path.with_extension(format!("{}.tmp", tmp_suffix));
        fs::write(&tmp_path, bytes)?;
        fs::rename(&tmp_path, final_path)?;
        Ok(())
    }

    /// Move an unparseable file to `{exec}/.corrupt/` so it stops breaking
    /// directory scans. Returns the moved path on success.
    fn quarantine_corrupt(
        &self,
        execution_id: &str,
        cp_path: &Path,
    ) -> Option<PathBuf> {
        let corrupt_dir = self.corrupt_dir(execution_id);
        let file_name = cp_path.file_name()?.to_string_lossy().to_string();
        let dest = corrupt_dir.join(file_name);
        match fs::create_dir_all(&corrupt_dir).and_then(|_| fs::rename(cp_path, &dest)) {
            Ok(_) => Some(dest),
            Err(e) => {
                warn!(
                    target: "nemesis_workflow::checkpoint::file_store",
                    execution_id = execution_id,
                    error = %e,
                    "failed to quarantine corrupt checkpoint; removing in place"
                );
                let _ = fs::remove_file(cp_path);
                None
            }
        }
    }
}

/// Reduce an arbitrary ID to a path-safe single component (no slashes, no
/// `..`, no `:`). Anything outside `[A-Za-z0-9_.-]` becomes `_`.
fn sanitize_path_component(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[async_trait]
impl CheckpointStore for FileCheckpointStore {
    async fn save(&self, checkpoint: Checkpoint) -> Result<String, StoreError> {
        let id = checkpoint.id.clone();
        let path = self.checkpoint_path(&checkpoint.execution_id, &id);
        // Tmp suffix uses the checkpoint id (already sanitized) so concurrent
        // saves of different checkpoints don't collide on the same tmp file.
        let bytes = serde_json::to_vec_pretty(&checkpoint)?;
        Self::atomic_write(&path, &bytes, &sanitize_path_component(&id))?;
        Ok(id)
    }

    async fn load(
        &self,
        execution_id: &str,
        checkpoint_id: &str,
    ) -> Result<Checkpoint, StoreError> {
        let path = self.checkpoint_path(execution_id, checkpoint_id);
        let bytes = match fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(StoreError::NotFound {
                    execution_id: execution_id.to_string(),
                    checkpoint_id: checkpoint_id.to_string(),
                });
            }
            Err(e) => return Err(StoreError::Io(e)),
        };
        match serde_json::from_slice::<Checkpoint>(&bytes) {
            Ok(cp) => Ok(cp),
            Err(e) => {
                warn!(
                    target: "nemesis_workflow::checkpoint::file_store",
                    execution_id = execution_id,
                    checkpoint_id = checkpoint_id,
                    error = %e,
                    "corrupt checkpoint file; quarantining"
                );
                self.quarantine_corrupt(execution_id, &path);
                Err(StoreError::Corrupt(format!(
                    "checkpoint {checkpoint_id} for execution {execution_id}: {e}"
                )))
            }
        }
    }

    async fn latest(&self, execution_id: &str) -> Result<Option<Checkpoint>, StoreError> {
        let dir = self.exec_dir(execution_id);
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(StoreError::Io(e)),
        };

        let mut latest: Option<(Checkpoint, chrono::DateTime<Utc>)> = None;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            // Skip the .corrupt subdir and anything without .json extension.
            if path.is_dir() {
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }

            let bytes = match fs::read(&path) {
                Ok(b) => b,
                Err(e) => {
                    warn!(
                        target: "nemesis_workflow::checkpoint::file_store",
                        path = %path.display(),
                        error = %e,
                        "failed to read checkpoint file during latest() scan; skipping"
                    );
                    continue;
                }
            };
            let cp: Checkpoint = match serde_json::from_slice(&bytes) {
                Ok(c) => c,
                Err(e) => {
                    warn!(
                        target: "nemesis_workflow::checkpoint::file_store",
                        path = %path.display(),
                        error = %e,
                        "corrupt checkpoint file during latest() scan; quarantining"
                    );
                    self.quarantine_corrupt(execution_id, &path);
                    continue;
                }
            };
            match &latest {
                None => latest = Some((cp.clone(), cp.saved_at)),
                Some((_, ts)) => {
                    if cp.saved_at > *ts {
                        latest = Some((cp.clone(), cp.saved_at));
                    }
                }
            }
        }
        Ok(latest.map(|(cp, _)| cp))
    }

    async fn list(&self, execution_id: &str) -> Result<Vec<CheckpointMeta>, StoreError> {
        let dir = self.exec_dir(execution_id);
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(StoreError::Io(e)),
        };

        let mut metas: Vec<CheckpointMeta> = Vec::new();
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }

            let bytes = match fs::read(&path) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let cp: Checkpoint = match serde_json::from_slice(&bytes) {
                Ok(c) => c,
                Err(_) => {
                    self.quarantine_corrupt(execution_id, &path);
                    continue;
                }
            };
            metas.push(CheckpointMeta::from(&cp));
        }
        // Oldest first (matches InMemoryCheckpointStore semantics).
        metas.sort_by_key(|m| m.saved_at);
        Ok(metas)
    }

    async fn delete(
        &self,
        execution_id: &str,
        checkpoint_id: &str,
    ) -> Result<(), StoreError> {
        let path = self.checkpoint_path(execution_id, checkpoint_id);
        match fs::remove_file(&path) {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(StoreError::Io(e)),
        }
    }

    async fn list_executions(&self) -> Result<Vec<String>, StoreError> {
        let root = self.checkpoints_dir();
        let entries = match fs::read_dir(&root) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(StoreError::Io(e)),
        };

        let mut ids: Vec<String> = Vec::new();
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            // Skip hidden dirs like .corrupt (which live *under* execution dirs
            // anyway, but defend against future layout changes).
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            ids.push(name);
        }
        ids.sort();
        Ok(ids)
    }
}

#[cfg(test)]
mod tests;
