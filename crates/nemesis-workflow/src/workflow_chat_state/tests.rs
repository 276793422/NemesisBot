use super::*;

impl WorkflowChatState {
    /// Number of currently in-flight workflow_chat executions. For tests and
    /// diagnostics.
    pub fn pending_count(&self) -> usize {
        self.inner.lock().pending_guards.len()
    }

    /// Number of execution_ids recorded as early-completed but not yet
    /// reconciled by `store_guard`. For tests and diagnostics.
    pub fn early_completion_count(&self) -> usize {
        self.inner.lock().early_completions.len()
    }
}

#[tokio::test]
async fn acquire_serializes_same_workflow() {
    let state = WorkflowChatState::new();
    let g1 = state.acquire("wf-a").await;
    // Second acquire for same workflow should block — verify by checking
    // that a try_acquire on a fresh task fails while g1 is held.
    let state2 = state;
    let try_guard =
        tokio::time::timeout(std::time::Duration::from_millis(50), state2.acquire("wf-a")).await;
    assert!(try_guard.is_err(), "second acquire should still be blocked");
    drop(g1);
    // Now it should succeed quickly.
    let try_guard2 =
        tokio::time::timeout(std::time::Duration::from_secs(1), state2.acquire("wf-a")).await;
    assert!(
        try_guard2.is_ok(),
        "second acquire should succeed after drop"
    );
}

#[tokio::test]
async fn different_workflows_run_in_parallel() {
    let state = WorkflowChatState::new();
    let g1 = state.acquire("wf-a").await;
    let g2 =
        tokio::time::timeout(std::time::Duration::from_millis(50), state.acquire("wf-b")).await;
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
    let reacquire =
        tokio::time::timeout(std::time::Duration::from_millis(200), state.acquire("wf-a")).await;
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
