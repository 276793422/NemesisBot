//! `CheckpointStore` trait + `InMemoryCheckpointStore` — milestone 1b-A1 step 2.
//!
//! All access is keyed by `execution_id` first, then `checkpoint_id`. This
//! matches Spike 3 decision 4: there is intentionally no global list-all
//! method, because cross-execution enumeration risks O(N) scans that would
//! let one slow execution stall the gateway's restart-recovery path.

use std::sync::Arc;

use async_trait::async_trait;
use dashmap::DashMap;
use parking_lot::Mutex;

use super::types::{Checkpoint, CheckpointMeta};

/// Errors returned by [`CheckpointStore`] implementations.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("checkpoint not found: execution={execution_id} id={checkpoint_id}")]
    NotFound { execution_id: String, checkpoint_id: String },

    #[error("corrupt checkpoint file: {0}")]
    Corrupt(String),
}

/// Persistent (or in-memory) checkpoint store.
///
/// Implementations must be safe to call from multiple tokio tasks. The trait
/// intentionally exposes only execution-keyed queries — cross-execution
/// enumeration (e.g. for gateway restart-recovery) uses
/// [`list_executions`](Self::list_executions), which returns just the IDs.
#[async_trait]
pub trait CheckpointStore: Send + Sync {
    /// Persist a checkpoint. Returns the checkpoint ID.
    async fn save(&self, checkpoint: Checkpoint) -> Result<String, StoreError>;

    /// Load a specific checkpoint by ID.
    async fn load(
        &self,
        execution_id: &str,
        checkpoint_id: &str,
    ) -> Result<Checkpoint, StoreError>;

    /// Return the most recently saved checkpoint for the given execution.
    /// `Ok(None)` if the execution has no checkpoints.
    async fn latest(&self, execution_id: &str)
        -> Result<Option<Checkpoint>, StoreError>;

    /// List checkpoints for an execution, oldest first. Returns only metadata.
    async fn list(&self, execution_id: &str) -> Result<Vec<CheckpointMeta>, StoreError>;

    /// Delete a single checkpoint.
    async fn delete(
        &self,
        execution_id: &str,
        checkpoint_id: &str,
    ) -> Result<(), StoreError>;

    /// List all execution IDs that have at least one checkpoint.
    /// Used by gateway restart-recovery to find in-flight executions.
    async fn list_executions(&self) -> Result<Vec<String>, StoreError>;
}

/// In-memory `CheckpointStore` for tests and ephemeral runs.
///
/// Layout: `DashMap<execution_id, Arc<Mutex<Vec<Checkpoint>>>>`. The inner
/// `Arc<Mutex<Vec<_>>>` keeps each execution's checkpoint list sorted by
/// `saved_at` while still allowing concurrent inserts across executions.
pub struct InMemoryCheckpointStore {
    data: DashMap<String, Arc<Mutex<Vec<Checkpoint>>>>,
}

impl InMemoryCheckpointStore {
    pub fn new() -> Self {
        Self {
            data: DashMap::new(),
        }
    }
}

impl Default for InMemoryCheckpointStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CheckpointStore for InMemoryCheckpointStore {
    async fn save(&self, checkpoint: Checkpoint) -> Result<String, StoreError> {
        let id = checkpoint.id.clone();
        let entry = self
            .data
            .entry(checkpoint.execution_id.clone())
            .or_insert_with(|| Arc::new(Mutex::new(Vec::new())));
        let mut guard = entry.lock();
        // Insert keeping the vector sorted by saved_at so latest() / list()
        // are O(1) / O(n) reads with no re-sort.
        let pos = guard
            .binary_search_by(|c| c.saved_at.cmp(&checkpoint.saved_at))
            .unwrap_or_else(|p| p);
        guard.insert(pos, checkpoint);
        Ok(id)
    }

    async fn load(
        &self,
        execution_id: &str,
        checkpoint_id: &str,
    ) -> Result<Checkpoint, StoreError> {
        let entry = match self.data.get(execution_id) {
            Some(e) => e,
            None => {
                return Err(StoreError::NotFound {
                    execution_id: execution_id.to_string(),
                    checkpoint_id: checkpoint_id.to_string(),
                });
            }
        };
        let guard = entry.lock();
        guard
            .iter()
            .find(|c| c.id == checkpoint_id)
            .cloned()
            .ok_or_else(|| StoreError::NotFound {
                execution_id: execution_id.to_string(),
                checkpoint_id: checkpoint_id.to_string(),
            })
    }

    async fn latest(&self, execution_id: &str)
        -> Result<Option<Checkpoint>, StoreError> {
        let entry = match self.data.get(execution_id) {
            Some(e) => e,
            None => return Ok(None),
        };
        let guard = entry.lock();
        Ok(guard.last().cloned())
    }

    async fn list(&self, execution_id: &str) -> Result<Vec<CheckpointMeta>, StoreError> {
        let entry = match self.data.get(execution_id) {
            Some(e) => e,
            None => return Ok(Vec::new()),
        };
        let guard = entry.lock();
        Ok(guard.iter().map(CheckpointMeta::from).collect())
    }

    async fn delete(
        &self,
        execution_id: &str,
        checkpoint_id: &str,
    ) -> Result<(), StoreError> {
        // Drop the entry guard before remove to avoid dead-lock.
        let existed = {
            let entry = match self.data.get(execution_id) {
                Some(e) => e,
                None => return Ok(()),
            };
            let mut guard = entry.lock();
            let len_before = guard.len();
            guard.retain(|c| c.id != checkpoint_id);
            len_before != guard.len()
        };
        let _ = existed;
        Ok(())
    }

    async fn list_executions(&self) -> Result<Vec<String>, StoreError> {
        let mut ids: Vec<String> = self.data.iter().map(|e| e.key().clone()).collect();
        ids.sort();
        Ok(ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::collections::{HashMap, HashSet};

    fn make_checkpoint(exec_id: &str, id: &str, ts_offset_secs: i64) -> Checkpoint {
        Checkpoint {
            id: id.to_string(),
            execution_id: exec_id.to_string(),
            saved_at: Utc::now() + chrono::Duration::seconds(ts_offset_secs),
            completed_nodes: HashSet::new(),
            waiting_node: None,
            parent_execution_id: None,
            context_snapshot: super::super::types::SerializableContext {
                variables: HashMap::new(),
                node_results: HashMap::new(),
                input: HashMap::new(),
            },
            workflow_hash: "h".to_string(),
        }
    }

    #[tokio::test]
    async fn test_save_and_load() {
        let store = InMemoryCheckpointStore::new();
        let cp = make_checkpoint("exec_a", "cp1", 0);
        let id = store.save(cp.clone()).await.unwrap();
        assert_eq!(id, "cp1");

        let loaded = store.load("exec_a", "cp1").await.unwrap();
        assert_eq!(loaded.id, "cp1");
        assert_eq!(loaded.execution_id, "exec_a");
    }

    #[tokio::test]
    async fn test_load_missing_returns_not_found() {
        let store = InMemoryCheckpointStore::new();
        let err = store.load("nope", "nope").await.unwrap_err();
        assert!(matches!(err, StoreError::NotFound { .. }));
    }

    #[tokio::test]
    async fn test_latest_returns_most_recent() {
        let store = InMemoryCheckpointStore::new();
        let cp1 = make_checkpoint("e", "cp1", 0);
        let cp2 = make_checkpoint("e", "cp2", 10);
        let cp3 = make_checkpoint("e", "cp3", 5);
        // Insert out of order — store must sort by saved_at.
        store.save(cp1).await.unwrap();
        store.save(cp2).await.unwrap();
        store.save(cp3).await.unwrap();

        let latest = store.latest("e").await.unwrap().unwrap();
        assert_eq!(latest.id, "cp2");
    }

    #[tokio::test]
    async fn test_latest_missing_execution_returns_none() {
        let store = InMemoryCheckpointStore::new();
        assert!(store.latest("none").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_list_returns_oldest_first() {
        let store = InMemoryCheckpointStore::new();
        store.save(make_checkpoint("e", "cp2", 10)).await.unwrap();
        store.save(make_checkpoint("e", "cp1", 0)).await.unwrap();
        store.save(make_checkpoint("e", "cp3", 20)).await.unwrap();

        let list = store.list("e").await.unwrap();
        let ids: Vec<_> = list.into_iter().map(|m| m.id).collect();
        assert_eq!(ids, vec!["cp1", "cp2", "cp3"]);
    }

    #[tokio::test]
    async fn test_list_executions_dedup() {
        let store = InMemoryCheckpointStore::new();
        store.save(make_checkpoint("e1", "cp1", 0)).await.unwrap();
        store.save(make_checkpoint("e1", "cp2", 1)).await.unwrap();
        store.save(make_checkpoint("e2", "cp3", 2)).await.unwrap();

        let mut execs = store.list_executions().await.unwrap();
        execs.sort();
        assert_eq!(execs, vec!["e1".to_string(), "e2".to_string()]);
    }

    #[tokio::test]
    async fn test_isolation_between_executions() {
        let store = InMemoryCheckpointStore::new();
        store.save(make_checkpoint("a", "cp_a", 0)).await.unwrap();
        store.save(make_checkpoint("b", "cp_b", 0)).await.unwrap();

        // Cross-execution query must not find other execution's checkpoints.
        let err = store.load("a", "cp_b").await.unwrap_err();
        assert!(matches!(err, StoreError::NotFound { .. }));
        assert!(store.latest("a").await.unwrap().unwrap().id == "cp_a");
        assert!(store.latest("b").await.unwrap().unwrap().id == "cp_b");
    }

    #[tokio::test]
    async fn test_delete_removes_checkpoint() {
        let store = InMemoryCheckpointStore::new();
        store.save(make_checkpoint("e", "cp1", 0)).await.unwrap();
        store.delete("e", "cp1").await.unwrap();

        assert!(store.list("e").await.unwrap().is_empty());
        assert!(store.latest("e").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_delete_missing_is_ok() {
        let store = InMemoryCheckpointStore::new();
        // Deleting something that was never saved is a no-op.
        store.delete("none", "none").await.unwrap();
    }
}
