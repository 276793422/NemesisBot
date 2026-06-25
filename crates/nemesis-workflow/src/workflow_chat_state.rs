//! Per-workflow chat serialization state.
//!
//! When multiple workflow_chat messages arrive for the same workflow in quick
//! succession, each spawns a fresh execution that writes to the same
//! `wf_chat:<workflow_name>` session key. Without serialization those writes
//! race on the SessionManager's per-key vector and can interleave/corrupt
//! history.
//!
//! [`WorkflowChatState`] solves this by holding:
//! 1. A per-workflow `tokio::sync::Mutex` (`mutexes`) — acquired at send time
//!    so concurrent sends to the same workflow queue up.
//! 2. A pending-execution map (`pending_guards`) keyed by execution_id that
//!    owns the guard until the workflow completes — released by the reply
//!    observer when it sees the matching Completed/Failed/Cancelled event.
//!
//! Lives inside [`crate::engine::WorkflowEngine`] so the engine, the WebSocket
//! handler, and the reply observer all share one instance without plumbing a
//! second Arc through AppState (which would require touching ~40 test
//! constructors).

use dashmap::DashMap;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, OwnedMutexGuard};

/// Per-workflow chat serialization state.
///
/// See module docs for the design rationale.
pub struct WorkflowChatState {
    /// One mutex per workflow_name. Lazily allocated on first acquire.
    mutexes: DashMap<String, Arc<Mutex<()>>>,
    /// In-flight execution_id → guard. The send handler inserts here after
    /// `start_async` returns; the reply observer removes the entry (dropping
    /// the guard, which releases the per-workflow mutex) when the workflow
    /// reaches a terminal state.
    pending_guards: std::sync::Mutex<HashMap<String, OwnedMutexGuard<()>>>,
}

impl Default for WorkflowChatState {
    fn default() -> Self {
        Self {
            mutexes: DashMap::new(),
            pending_guards: std::sync::Mutex::new(HashMap::new()),
        }
    }
}

impl WorkflowChatState {
    /// Create an empty state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Acquire the per-workflow mutex, returning an owned guard that can be
    /// held across the `.await` on `start_async` and stored in
    /// [`pending_guards`](Self::pending_guards) until the workflow completes.
    ///
    /// The returned future yields when no other send handler holds the lock
    /// for this workflow.
    pub async fn acquire(&self, workflow_name: &str) -> OwnedMutexGuard<()> {
        let mutex = {
            let entry = self
                .mutexes
                .entry(workflow_name.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(())));
            entry.clone()
        };
        mutex.lock_owned().await
    }

    /// Record an in-flight execution so the reply observer can release the
    /// per-workflow lock when the workflow terminates.
    ///
    /// Called after `engine.start_async(...)` returns the execution_id.
    /// Overwrites any existing entry for the same execution_id (shouldn't
    /// happen in practice — execution_ids are UUIDs).
    pub fn store_guard(&self, execution_id: &str, guard: OwnedMutexGuard<()>) {
        let mut map = self.pending_guards.lock().unwrap();
        map.insert(execution_id.to_string(), guard);
    }

    /// Remove and return the guard for a finished execution.
    ///
    /// The caller drops the returned guard to release the per-workflow mutex.
    /// Returns `None` if no entry exists — happens when the execution wasn't
    /// started via workflow_chat (regular manual runs, event triggers, etc.),
    /// so the observer can use this to no-op on irrelevant executions.
    pub fn take_guard(&self, execution_id: &str) -> Option<OwnedMutexGuard<()>> {
        let mut map = self.pending_guards.lock().unwrap();
        map.remove(execution_id)
    }

    /// Number of currently in-flight workflow_chat executions. For tests and
    /// diagnostics.
    #[cfg(test)]
    pub fn pending_count(&self) -> usize {
        self.pending_guards.lock().unwrap().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn acquire_serializes_same_workflow() {
        let state = WorkflowChatState::new();
        let g1 = state.acquire("wf-a").await;
        // Second acquire for same workflow should block — verify by checking
        // that a try_acquire on a fresh task fails while g1 is held.
        let state2 = state;
        let try_guard = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            state2.acquire("wf-a"),
        )
        .await;
        assert!(try_guard.is_err(), "second acquire should still be blocked");
        drop(g1);
        // Now it should succeed quickly.
        let try_guard2 = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            state2.acquire("wf-a"),
        )
        .await;
        assert!(try_guard2.is_ok(), "second acquire should succeed after drop");
    }

    #[tokio::test]
    async fn different_workflows_run_in_parallel() {
        let state = WorkflowChatState::new();
        let g1 = state.acquire("wf-a").await;
        let g2 = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            state.acquire("wf-b"),
        )
        .await;
        assert!(g2.is_ok(), "different workflow should not block");
        drop(g1);
    }

    #[tokio::test]
    async fn store_and_take_guard_round_trip() {
        let state = WorkflowChatState::new();
        let g = state.acquire("wf-a").await;
        state.store_guard("exec-1", g);
        assert_eq!(state.pending_count(), 1);
        let taken = state.take_guard("exec-1");
        assert!(taken.is_some());
        assert_eq!(state.pending_count(), 0);
        // Second take returns None.
        assert!(state.take_guard("exec-1").is_none());
    }

    #[test]
    fn take_guard_for_unknown_execution_returns_none() {
        let state = WorkflowChatState::new();
        assert!(state.take_guard("never-seen").is_none());
    }
}
