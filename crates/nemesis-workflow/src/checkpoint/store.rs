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
    NotFound {
        execution_id: String,
        checkpoint_id: String,
    },

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
    async fn load(&self, execution_id: &str, checkpoint_id: &str)
    -> Result<Checkpoint, StoreError>;

    /// Return the most recently saved checkpoint for the given execution.
    /// `Ok(None)` if the execution has no checkpoints.
    async fn latest(&self, execution_id: &str) -> Result<Option<Checkpoint>, StoreError>;

    /// List checkpoints for an execution, oldest first. Returns only metadata.
    async fn list(&self, execution_id: &str) -> Result<Vec<CheckpointMeta>, StoreError>;

    /// Delete a single checkpoint.
    async fn delete(&self, execution_id: &str, checkpoint_id: &str) -> Result<(), StoreError>;

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

    async fn latest(&self, execution_id: &str) -> Result<Option<Checkpoint>, StoreError> {
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

    async fn delete(&self, execution_id: &str, checkpoint_id: &str) -> Result<(), StoreError> {
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
mod tests;
