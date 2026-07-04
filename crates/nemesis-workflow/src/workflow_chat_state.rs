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
}

#[cfg(test)]
mod tests;
