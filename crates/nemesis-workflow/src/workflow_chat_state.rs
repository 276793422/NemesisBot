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
//!
//! ### Race fix: `early_completions`
//!
//! The send handler calls `store_guard` *after* `start_async` returns. On a
//! multi-threaded tokio runtime the spawned workflow task can run on another
//! worker and reach a terminal state before the send handler reaches
//! `store_guard`. The observer then visits `take_guard` first, finds nothing,
//! and leaves — at which point a later `store_guard` would park the guard
//! forever (no future terminal event will fire), permanently locking the
//! per-workflow mutex.
//!
//! `early_completions` records execution_ids that the observer already saw.
//! `store_guard` checks it atomically and drops the guard on hit instead of
//! inserting.

use dashmap::DashMap;
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

/// Per-workflow chat serialization state.
///
/// See module docs for the design rationale.
pub struct WorkflowChatState {
    /// One mutex per workflow_name. Lazily allocated on first acquire.
    mutexes: DashMap<String, Arc<AsyncMutex<()>>>,
    /// All mutable per-execution state guarded by a single std mutex so
    /// `store_guard` and `take_guard` can coordinate atomically (see the
    /// `early_completions` race fix in the module docs). Uses parking_lot —
    /// no poisoning if a panic happens while the guard is held.
    inner: Mutex<Inner>,
}

struct Inner {
    /// In-flight execution_id → guard. The send handler inserts here after
    /// `start_async` returns; the reply observer removes the entry (dropping
    /// the guard, which releases the per-workflow mutex) when the workflow
    /// reaches a terminal state.
    pending_guards: HashMap<String, OwnedMutexGuard<()>>,
    /// Execution_ids whose terminal event fired before `store_guard` was
    /// called. `store_guard` consumes the entry and drops the guard on hit
    /// so the per-workflow mutex doesn't get parked forever.
    early_completions: HashSet<String>,
}

impl Default for WorkflowChatState {
    fn default() -> Self {
        Self {
            mutexes: DashMap::new(),
            inner: Mutex::new(Inner {
                pending_guards: HashMap::new(),
                early_completions: HashSet::new(),
            }),
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
                .or_insert_with(|| Arc::new(AsyncMutex::new(())));
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
    ///
    /// **Race fix**: if the observer already visited `take_guard` for this
    /// execution_id (because the workflow terminated between `start_async`
    /// returning and this call), the guard is dropped here instead of being
    /// parked in `pending_guards` forever.
    pub fn store_guard(&self, execution_id: &str, guard: OwnedMutexGuard<()>) {
        let mut inner = self.inner.lock();
        if inner.early_completions.remove(execution_id) {
            // Observer already came & went; releasing the guard here is the
            // only way to unlock the per-workflow mutex.
            drop(guard);
            return;
        }
        inner
            .pending_guards
            .insert(execution_id.to_string(), guard);
    }

    /// Remove and return the guard for a finished execution.
    ///
    /// The caller drops the returned guard to release the per-workflow mutex.
    /// Returns `None` if no entry exists — either the execution wasn't
    /// started via workflow_chat (regular manual runs, event triggers, etc.),
    /// or `store_guard` hasn't been called yet (early completion race — see
    /// module docs). In the latter case the execution_id is recorded so the
    /// eventual `store_guard` can release the guard immediately.
    pub fn take_guard(&self, execution_id: &str) -> Option<OwnedMutexGuard<()>> {
        let mut inner = self.inner.lock();
        if let Some(g) = inner.pending_guards.remove(execution_id) {
            return Some(g);
        }
        // Record so store_guard can detect the early-completion race.
        inner.early_completions.insert(execution_id.to_string());
        None
    }

    /// Number of currently in-flight workflow_chat executions. For tests and
    /// diagnostics.
    #[cfg(test)]
    pub fn pending_count(&self) -> usize {
        self.inner.lock().pending_guards.len()
    }

    /// Number of execution_ids recorded as early-completed but not yet
    /// reconciled by `store_guard`. For tests and diagnostics.
    #[cfg(test)]
    pub fn early_completion_count(&self) -> usize {
        self.inner.lock().early_completions.len()
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

    /// Regression: if the observer's `take_guard` fires before `store_guard`
    /// (workflow completed on another worker between `start_async` returning
    /// and `store_guard`), `store_guard` must drop the guard immediately
    /// instead of parking it in `pending_guards` forever (which would lock
    /// the per-workflow mutex permanently).
    #[tokio::test]
    async fn store_guard_after_observer_took_releases_immediately() {
        let state = WorkflowChatState::new();
        let g = state.acquire("wf-a").await;

        // Observer visits first — nothing to take, but the execution_id is
        // now recorded as an early completion.
        assert!(state.take_guard("exec-1").is_none());
        assert_eq!(state.early_completion_count(), 1);

        // Send handler belatedly calls store_guard. The guard must NOT sit in
        // pending_guards; it must be consumed by the early-completion entry.
        state.store_guard("exec-1", g);
        assert_eq!(state.pending_count(), 0);
        assert_eq!(state.early_completion_count(), 0);

        // And the per-workflow mutex must be releasable: a fresh acquire for
        // the same workflow should succeed without blocking.
        let reacquire = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            state.acquire("wf-a"),
        )
        .await;
        assert!(
            reacquire.is_ok(),
            "per-workflow mutex should be free after race-safe store_guard"
        );
    }

    /// Regression: the normal order (store_guard before take_guard) still
    /// works and the early_completions set stays empty.
    #[tokio::test]
    async fn normal_order_does_not_pollute_early_completions() {
        let state = WorkflowChatState::new();
        let g = state.acquire("wf-a").await;
        state.store_guard("exec-1", g);
        assert_eq!(state.early_completion_count(), 0);
        let taken = state.take_guard("exec-1");
        assert!(taken.is_some());
        assert_eq!(state.early_completion_count(), 0);
        assert_eq!(state.pending_count(), 0);
    }
}
