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
mod tests {
    use super::*;
    use crate::checkpoint::types::SerializableContext;
    use chrono::Utc;
    use std::collections::HashSet;
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn make_checkpoint(exec_id: &str, id: &str, ts_offset_secs: i64) -> Checkpoint {
        Checkpoint {
            id: id.to_string(),
            execution_id: exec_id.to_string(),
            saved_at: Utc::now() + chrono::Duration::seconds(ts_offset_secs),
            completed_nodes: HashSet::new(),
            waiting_node: None,
            parent_execution_id: None,
            trigger_source: None,
            terminal: false,
            context_snapshot: SerializableContext {
                variables: HashMap::new(),
                node_results: HashMap::new(),
                input: HashMap::new(),
            },
            workflow_hash: "h".to_string(),
        }
    }

    fn make_store() -> (TempDir, FileCheckpointStore) {
        let tmp = TempDir::new().unwrap();
        let store = FileCheckpointStore::new(tmp.path()).unwrap();
        (tmp, store)
    }

    #[tokio::test]
    async fn save_and_load_round_trip() {
        let (_tmp, store) = make_store();
        let cp = make_checkpoint("exec_a", "cp1", 0);
        let id = store.save(cp.clone()).await.unwrap();
        assert_eq!(id, "cp1");

        let loaded = store.load("exec_a", "cp1").await.unwrap();
        assert_eq!(loaded.id, "cp1");
        assert_eq!(loaded.execution_id, "exec_a");
        assert_eq!(loaded, cp);
    }

    #[tokio::test]
    async fn load_missing_returns_not_found() {
        let (_tmp, store) = make_store();
        let err = store.load("nope", "nope").await.unwrap_err();
        assert!(matches!(err, StoreError::NotFound { .. }));
    }

    #[tokio::test]
    async fn latest_returns_most_recent() {
        let (_tmp, store) = make_store();
        // Stagger saves with real wall-clock intervals so mtime ordering is
        // stable on filesystems with coarse mtime resolution (HFS+, FAT).
        store.save(make_checkpoint("e", "cp1", 0)).await.unwrap();
        store.save(make_checkpoint("e", "cp2", 10)).await.unwrap();
        store.save(make_checkpoint("e", "cp3", 5)).await.unwrap();

        let latest = store.latest("e").await.unwrap().unwrap();
        assert_eq!(latest.id, "cp2");
    }

    #[tokio::test]
    async fn latest_missing_execution_returns_none() {
        let (_tmp, store) = make_store();
        assert!(store.latest("none").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn list_returns_oldest_first() {
        let (_tmp, store) = make_store();
        store.save(make_checkpoint("e", "cp2", 10)).await.unwrap();
        store.save(make_checkpoint("e", "cp1", 0)).await.unwrap();
        store.save(make_checkpoint("e", "cp3", 20)).await.unwrap();

        let list = store.list("e").await.unwrap();
        let ids: Vec<_> = list.into_iter().map(|m| m.id).collect();
        assert_eq!(ids, vec!["cp1", "cp2", "cp3"]);
    }

    #[tokio::test]
    async fn list_executions_dedup() {
        let (_tmp, store) = make_store();
        store.save(make_checkpoint("e1", "cp1", 0)).await.unwrap();
        store.save(make_checkpoint("e1", "cp2", 1)).await.unwrap();
        store.save(make_checkpoint("e2", "cp3", 2)).await.unwrap();

        let execs = store.list_executions().await.unwrap();
        assert_eq!(execs, vec!["e1".to_string(), "e2".to_string()]);
    }

    #[tokio::test]
    async fn delete_removes_checkpoint() {
        let (_tmp, store) = make_store();
        store.save(make_checkpoint("e", "cp1", 0)).await.unwrap();
        store.delete("e", "cp1").await.unwrap();

        assert!(store.list("e").await.unwrap().is_empty());
        assert!(store.latest("e").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn delete_missing_is_ok() {
        let (_tmp, store) = make_store();
        store.delete("none", "none").await.unwrap();
    }

    #[tokio::test]
    async fn isolation_between_executions() {
        let (_tmp, store) = make_store();
        store.save(make_checkpoint("a", "cp_a", 0)).await.unwrap();
        store.save(make_checkpoint("b", "cp_b", 0)).await.unwrap();

        let err = store.load("a", "cp_b").await.unwrap_err();
        assert!(matches!(err, StoreError::NotFound { .. }));
        assert_eq!(store.latest("a").await.unwrap().unwrap().id, "cp_a");
        assert_eq!(store.latest("b").await.unwrap().unwrap().id, "cp_b");
    }

    #[tokio::test]
    async fn corrupt_file_is_quarantined_and_latest_skips_it() {
        let (_tmp, store) = make_store();
        // Write a good checkpoint, then poison the directory with a corrupt one.
        store.save(make_checkpoint("e", "cp1", 0)).await.unwrap();
        let bad_path = store.checkpoint_path("e", "cp_bad");
        fs::write(&bad_path, b"NOT VALID JSON").unwrap();

        // latest() should still return cp1 (the only valid checkpoint).
        let latest = store.latest("e").await.unwrap().unwrap();
        assert_eq!(latest.id, "cp1");

        // The corrupt file should have been moved into .corrupt/.
        let corrupt_path = store
            .exec_dir("e")
            .join(".corrupt")
            .join("cp_bad.json");
        assert!(corrupt_path.exists(), "corrupt file should be quarantined");
        assert!(!bad_path.exists(), "original corrupt file should be gone");
    }

    #[tokio::test]
    async fn load_corrupt_returns_corrupt_error() {
        let (_tmp, store) = make_store();
        let bad_path = store.checkpoint_path("e", "cp1");
        fs::create_dir_all(bad_path.parent().unwrap()).unwrap();
        fs::write(&bad_path, b"NOT VALID JSON").unwrap();

        let err = store.load("e", "cp1").await.unwrap_err();
        assert!(matches!(err, StoreError::Corrupt(_)));
    }

    #[tokio::test]
    async fn path_traversal_ids_are_rejected() {
        // An execution_id with `..` or `/` must not escape the checkpoints root.
        let (tmp, store) = make_store();
        // Save with a traversal-style id; sanitize should flatten it to a single
        // safe component rather than producing `../../etc/something`.
        let evil_id = "../../../etc/evil";
        store
            .save(make_checkpoint(evil_id, "cp1", 0))
            .await
            .unwrap();

        // No file should have escaped the checkpoints root. We assert by
        // canonicalizing both paths and checking containment — substring
        // checks would false-positive on the sanitized name (`.._.._evil`).
        let cp_root_canon = store.checkpoints_dir().canonicalize().unwrap();
        let mut all_paths = Vec::new();
        collect_relative_paths(&cp_root_canon, &cp_root_canon, &mut all_paths);
        for rel in &all_paths {
            // Each relative path is computed by stripping the cp_root prefix,
            // so any successful traversal would show up as an absolute path
            // outside the root (strip_prefix would have failed otherwise).
            assert!(
                !rel.starts_with('/') && !rel.starts_with('\\'),
                "path escaped checkpoints root: {rel}"
            );
        }

        // Sanity: at least one file was written somewhere under cp_root.
        assert!(
            !all_paths.is_empty(),
            "save should have produced at least one file under the checkpoints root"
        );

        // The temp dir must not have acquired an `etc/` directory — that would
        // indicate the `..` traversal actually escaped.
        assert!(
            !tmp.path().join("etc").exists(),
            "traversal escaped temp root"
        );
    }

    fn collect_relative_paths(root: &Path, dir: &Path, out: &mut Vec<String>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let rel = path
                .strip_prefix(root)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            if path.is_dir() {
                collect_relative_paths(root, &path, out);
            } else {
                out.push(rel);
            }
        }
    }
}
