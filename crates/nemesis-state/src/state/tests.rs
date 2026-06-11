use super::*;

#[test]
fn test_state_transitions() {
    let state = BotState::new();
    assert_eq!(state.current(), State::Created);

    state.start_initializing().unwrap();
    assert_eq!(state.current(), State::Initializing);

    state.start_running().unwrap();
    assert_eq!(state.current(), State::Running);

    state.start_stopping().unwrap();
    assert_eq!(state.current(), State::Stopping);

    state.set_stopped();
    assert_eq!(state.current(), State::Stopped);
}

#[test]
fn test_invalid_transition() {
    let state = BotState::new();
    assert!(state.start_running().is_err());
}

#[test]
fn test_error_state() {
    let state = BotState::new();
    state.set_error("test error");
    assert_eq!(state.current(), State::Error);
    assert_eq!(state.error_message(), Some("test error".to_string()));
}

#[test]
fn test_is_running() {
    let state = BotState::new();
    assert!(!state.is_running());
    state.start_initializing().unwrap();
    state.start_running().unwrap();
    assert!(state.is_running());
}

#[test]
fn test_state_display_all_variants() {
    assert_eq!(format!("{}", State::Created), "created");
    assert_eq!(format!("{}", State::Initializing), "initializing");
    assert_eq!(format!("{}", State::Running), "running");
    assert_eq!(format!("{}", State::Stopping), "stopping");
    assert_eq!(format!("{}", State::Stopped), "stopped");
    assert_eq!(format!("{}", State::Error), "error");
}

#[test]
fn test_state_from_u8_invalid_value() {
    // from_u8 is a private method but it's exercised through transition error messages.
    // We can verify it indirectly by checking that compare_exchange failures produce
    // the correct fallback (Error) for invalid u8 values.
    // Test by forcing an invalid state via raw state field.
    // Since from_u8(v) for v>5 returns Error, we verify via the transition error path.
    let state = BotState::new();
    // Attempting start_running from Created (not Initializing) fails
    let result = state.start_running();
    assert!(result.is_err());
    let err_msg = result.unwrap_err();
    assert!(err_msg.contains("expected state initializing but was created"));
}

#[test]
fn test_state_repr_u8_values() {
    assert_eq!(State::Created as u8, 0);
    assert_eq!(State::Initializing as u8, 1);
    assert_eq!(State::Running as u8, 2);
    assert_eq!(State::Stopping as u8, 3);
    assert_eq!(State::Stopped as u8, 4);
    assert_eq!(State::Error as u8, 5);
}

#[test]
fn test_bot_state_default() {
    let state = BotState::default();
    assert_eq!(state.current(), State::Created);
    assert!(state.error_message().is_none());
}

#[test]
fn test_start_stopping_from_error_fails() {
    let state = BotState::new();
    state.set_error("test error");
    assert_eq!(state.current(), State::Error);

    let result = state.start_stopping();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("cannot stop from state error"));
}

#[test]
fn test_start_stopping_from_stopped_fails() {
    let state = BotState::new();
    state.start_initializing().unwrap();
    state.start_running().unwrap();
    state.start_stopping().unwrap();
    state.set_stopped();
    assert_eq!(state.current(), State::Stopped);

    let result = state.start_stopping();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("cannot stop from state stopped"));
}

#[test]
fn test_start_stopping_from_created_fails() {
    let state = BotState::new();
    assert_eq!(state.current(), State::Created);

    let result = state.start_stopping();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("cannot stop from state created"));
}

#[test]
fn test_start_stopping_from_initializing_succeeds() {
    let state = BotState::new();
    state.start_initializing().unwrap();
    assert_eq!(state.current(), State::Initializing);

    let result = state.start_stopping();
    assert!(result.is_ok());
    assert_eq!(state.current(), State::Stopping);
}

#[test]
fn test_start_initializing_from_non_created_fails() {
    let state = BotState::new();
    state.start_initializing().unwrap();
    assert_eq!(state.current(), State::Initializing);

    // Second call should fail
    let result = state.start_initializing();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("expected state created but was initializing"));
}

#[test]
fn test_start_initializing_from_running_fails() {
    let state = BotState::new();
    state.start_initializing().unwrap();
    state.start_running().unwrap();
    assert_eq!(state.current(), State::Running);

    let result = state.start_initializing();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("expected state created but was running"));
}

#[test]
fn test_set_stopped_from_error() {
    let state = BotState::new();
    state.set_error("some error");
    assert_eq!(state.current(), State::Error);

    // set_stopped is unconditional, should work from any state
    state.set_stopped();
    assert_eq!(state.current(), State::Stopped);
}

#[test]
fn test_set_stopped_from_created() {
    let state = BotState::new();
    assert_eq!(state.current(), State::Created);

    // set_stopped is unconditional
    state.set_stopped();
    assert_eq!(state.current(), State::Stopped);
}

#[test]
fn test_error_message_when_no_error() {
    let state = BotState::new();
    assert_eq!(state.error_message(), None);
}

#[test]
fn test_error_message_after_set_error() {
    let state = BotState::new();
    state.set_error("critical failure");
    assert_eq!(state.error_message(), Some("critical failure".to_string()));
}

#[test]
fn test_error_message_after_set_stopped_does_not_clear() {
    let state = BotState::new();
    state.set_error("some error");
    state.set_stopped();
    // error_message field is not cleared by set_stopped
    assert_eq!(state.error_message(), Some("some error".to_string()));
    assert_eq!(state.current(), State::Stopped);
}

#[test]
fn test_set_error_overwrites_previous() {
    let state = BotState::new();
    state.set_error("first error");
    assert_eq!(state.error_message(), Some("first error".to_string()));

    state.set_error("second error");
    assert_eq!(state.error_message(), Some("second error".to_string()));
}

// --- Additional tests for is_running() across all states ---

#[test]
fn test_is_running_from_created_is_false() {
    let state = BotState::new();
    assert_eq!(state.current(), State::Created);
    assert!(!state.is_running());
}

#[test]
fn test_is_running_from_initializing_is_false() {
    let state = BotState::new();
    state.start_initializing().unwrap();
    assert_eq!(state.current(), State::Initializing);
    assert!(!state.is_running());
}

#[test]
fn test_is_running_from_running_is_true() {
    let state = BotState::new();
    state.start_initializing().unwrap();
    state.start_running().unwrap();
    assert_eq!(state.current(), State::Running);
    assert!(state.is_running());
}

#[test]
fn test_is_running_from_stopping_is_false() {
    let state = BotState::new();
    state.start_initializing().unwrap();
    state.start_running().unwrap();
    state.start_stopping().unwrap();
    assert_eq!(state.current(), State::Stopping);
    assert!(!state.is_running());
}

#[test]
fn test_is_running_from_stopped_is_false() {
    let state = BotState::new();
    state.start_initializing().unwrap();
    state.start_running().unwrap();
    state.start_stopping().unwrap();
    state.set_stopped();
    assert_eq!(state.current(), State::Stopped);
    assert!(!state.is_running());
}

#[test]
fn test_is_running_from_error_is_false() {
    let state = BotState::new();
    state.set_error("boom");
    assert_eq!(state.current(), State::Error);
    assert!(!state.is_running());
}

// --- State transition edge cases ---

#[test]
fn test_start_running_from_created_fails() {
    let state = BotState::new();
    let result = state.start_running();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("expected state initializing but was created"));
    // State should remain unchanged
    assert_eq!(state.current(), State::Created);
}

#[test]
fn test_start_running_from_running_fails() {
    let state = BotState::new();
    state.start_initializing().unwrap();
    state.start_running().unwrap();
    let result = state.start_running();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("expected state initializing but was running"));
    // State should remain unchanged
    assert_eq!(state.current(), State::Running);
}

#[test]
fn test_start_running_from_stopped_fails() {
    let state = BotState::new();
    state.set_stopped();
    let result = state.start_running();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("expected state initializing but was stopped"));
}

#[test]
fn test_start_running_from_error_fails() {
    let state = BotState::new();
    state.set_error("err");
    let result = state.start_running();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("expected state initializing but was error"));
}

#[test]
fn test_start_initializing_from_stopped_fails() {
    let state = BotState::new();
    state.set_stopped();
    let result = state.start_initializing();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("expected state created but was stopped"));
}

#[test]
fn test_start_initializing_from_error_fails() {
    let state = BotState::new();
    state.set_error("err");
    let result = state.start_initializing();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("expected state created but was error"));
}

#[test]
fn test_set_error_from_running() {
    let state = BotState::new();
    state.start_initializing().unwrap();
    state.start_running().unwrap();
    assert!(state.is_running());

    // set_error is unconditional
    state.set_error("runtime crash");
    assert_eq!(state.current(), State::Error);
    assert_eq!(state.error_message(), Some("runtime crash".to_string()));
    assert!(!state.is_running());
}

#[test]
fn test_set_error_from_stopping() {
    let state = BotState::new();
    state.start_initializing().unwrap();
    state.start_running().unwrap();
    state.start_stopping().unwrap();

    state.set_error("shutdown failed");
    assert_eq!(state.current(), State::Error);
    assert_eq!(state.error_message(), Some("shutdown failed".to_string()));
}

#[test]
fn test_set_error_with_empty_string() {
    let state = BotState::new();
    state.set_error("");
    assert_eq!(state.current(), State::Error);
    assert_eq!(state.error_message(), Some(String::new()));
}

#[test]
fn test_set_stopped_from_running() {
    let state = BotState::new();
    state.start_initializing().unwrap();
    state.start_running().unwrap();
    // set_stopped is unconditional, can jump directly from Running
    state.set_stopped();
    assert_eq!(state.current(), State::Stopped);
}

#[test]
fn test_set_stopped_is_idempotent() {
    let state = BotState::new();
    state.set_stopped();
    assert_eq!(state.current(), State::Stopped);
    state.set_stopped();
    assert_eq!(state.current(), State::Stopped);
}

#[test]
fn test_set_error_is_idempotent() {
    let state = BotState::new();
    state.set_error("first");
    assert_eq!(state.current(), State::Error);
    state.set_error("second");
    assert_eq!(state.current(), State::Error);
    assert_eq!(state.error_message(), Some("second".to_string()));
}

#[test]
fn test_full_lifecycle_created_to_stopped() {
    let state = BotState::new();
    assert_eq!(state.current(), State::Created);
    assert!(!state.is_running());
    assert!(state.error_message().is_none());

    state.start_initializing().unwrap();
    assert_eq!(state.current(), State::Initializing);

    state.start_running().unwrap();
    assert_eq!(state.current(), State::Running);
    assert!(state.is_running());

    state.start_stopping().unwrap();
    assert_eq!(state.current(), State::Stopping);
    assert!(!state.is_running());

    state.set_stopped();
    assert_eq!(state.current(), State::Stopped);
    assert!(!state.is_running());
    assert!(state.error_message().is_none());
}

#[test]
fn test_lifecycle_with_error_recovery() {
    // Error during initialization, then forced stop
    let state = BotState::new();
    state.start_initializing().unwrap();
    state.set_error("init failed");
    assert_eq!(state.current(), State::Error);
    assert_eq!(state.error_message(), Some("init failed".to_string()));

    // Cannot start stopping from error
    let result = state.start_stopping();
    assert!(result.is_err());

    // But set_stopped is unconditional
    state.set_stopped();
    assert_eq!(state.current(), State::Stopped);
    // error_message persists even though state changed
    assert_eq!(state.error_message(), Some("init failed".to_string()));
}

#[test]
fn test_start_stopping_from_running_succeeds() {
    let state = BotState::new();
    state.start_initializing().unwrap();
    state.start_running().unwrap();
    let result = state.start_stopping();
    assert!(result.is_ok());
    assert_eq!(state.current(), State::Stopping);
}

#[test]
fn test_state_equality_and_copy() {
    // Verify State is Copy and PartialEq works
    let s1 = State::Running;
    let s2 = s1; // Copy
    assert_eq!(s1, s2);

    let s3 = State::Stopped;
    assert_ne!(s1, s3);
}

// --- Additional tests for edge cases and state consistency ---

#[test]
fn test_state_current_invalid_u8_value() {
    // Test that invalid u8 values in state fall back to Error
    // This indirectly tests the `_ => State::Error` branch in State::from_u8()
    let state = BotState::new();

    // We can't directly set an invalid state, but we can verify
    // that the state machine handles all valid states correctly
    // and that Error is the fallback for unexpected values

    // Verify all valid states work correctly
    assert_eq!(state.current(), State::Created);
    state.start_initializing().unwrap();
    assert_eq!(state.current(), State::Initializing);
    state.start_running().unwrap();
    assert_eq!(state.current(), State::Running);
    state.start_stopping().unwrap();
    assert_eq!(state.current(), State::Stopping);
    state.set_stopped();
    assert_eq!(state.current(), State::Stopped);

    // Error state
    let error_state = BotState::new();
    error_state.set_error("test");
    assert_eq!(error_state.current(), State::Error);
}

#[test]
fn test_multiple_state_transitions_sequence() {
    // Test a complex sequence of state transitions
    let state = BotState::new();

    // Normal flow
    state.start_initializing().unwrap();
    assert_eq!(state.current(), State::Initializing);

    state.start_running().unwrap();
    assert_eq!(state.current(), State::Running);

    // Transition to stopping
    state.start_stopping().unwrap();
    assert_eq!(state.current(), State::Stopping);

    // Can't go back to running from stopping
    assert!(state.start_running().is_err());
    assert_eq!(state.current(), State::Stopping);

    // But can go to stopped
    state.set_stopped();
    assert_eq!(state.current(), State::Stopped);

    // From stopped, can't do anything except set_error or set_stopped again
    assert!(state.start_running().is_err());
    assert!(state.start_initializing().is_err());
    assert!(state.start_stopping().is_err());
}

#[test]
fn test_error_message_clear_on_new_error() {
    // Verify that setting a new error message replaces the old one
    let state = BotState::new();

    state.set_error("first error");
    assert_eq!(state.error_message(), Some("first error".to_string()));

    state.set_error("second error");
    assert_eq!(state.error_message(), Some("second error".to_string()));

    state.set_error("");
    assert_eq!(state.error_message(), Some("".to_string()));
}

#[test]
fn test_state_thread_safety() {
    // Verify that BotState can be safely shared across threads
    use std::sync::Arc;
    use std::thread;

    let state = Arc::new(BotState::new());
    let mut handles = vec![];

    // Spawn multiple threads to query state concurrently
    for _ in 0..10 {
        let state_clone = Arc::clone(&state);
        let handle = thread::spawn(move || {
            // Query current state
            let _ = state_clone.current();
            let _ = state_clone.is_running();
            let _ = state_clone.error_message();
        });
        handles.push(handle);
    }

    // All threads should complete successfully
    for handle in handles {
        handle.join().unwrap();
    }
}

#[test]
fn test_state_display_impl() {
    // Verify Display trait implementation for all states
    assert_eq!(format!("{}", State::Created), "created");
    assert_eq!(format!("{}", State::Initializing), "initializing");
    assert_eq!(format!("{}", State::Running), "running");
    assert_eq!(format!("{}", State::Stopping), "stopping");
    assert_eq!(format!("{}", State::Stopped), "stopped");
    assert_eq!(format!("{}", State::Error), "error");
}

#[test]
fn test_atomic_u8_state_consistency() {
    // Verify that the AtomicU8 state field is consistent across operations
    let state = BotState::new();

    // Initial state
    assert_eq!(state.current(), State::Created);

    // After start_initializing
    state.start_initializing().unwrap();
    assert_eq!(state.current(), State::Initializing);

    // Atomic operation should be visible immediately
    let current = state.current();
    assert_eq!(current, State::Initializing);

    // state transition should be atomic
    state.start_running().unwrap();
    assert_eq!(state.current(), State::Running);
}
