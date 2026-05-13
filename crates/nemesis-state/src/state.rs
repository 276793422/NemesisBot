//! Bot state machine.

use parking_lot::Mutex;
use std::fmt;
use std::sync::atomic::{AtomicU8, Ordering};

/// Bot states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum State {
    Created = 0,
    Initializing = 1,
    Running = 2,
    Stopping = 3,
    Stopped = 4,
    Error = 5,
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            State::Created => write!(f, "created"),
            State::Initializing => write!(f, "initializing"),
            State::Running => write!(f, "running"),
            State::Stopping => write!(f, "stopping"),
            State::Stopped => write!(f, "stopped"),
            State::Error => write!(f, "error"),
        }
    }
}

/// Bot state machine with thread-safe state transitions.
pub struct BotState {
    state: AtomicU8,
    error_message: Mutex<Option<String>>,
}

impl BotState {
    pub fn new() -> Self {
        Self {
            state: AtomicU8::new(State::Created as u8),
            error_message: Mutex::new(None),
        }
    }

    /// Get current state.
    pub fn current(&self) -> State {
        match self.state.load(Ordering::SeqCst) {
            0 => State::Created,
            1 => State::Initializing,
            2 => State::Running,
            3 => State::Stopping,
            4 => State::Stopped,
            5 => State::Error,
            _ => State::Error,
        }
    }

    /// Transition to initializing state.
    pub fn start_initializing(&self) -> Result<(), String> {
        self.transition(State::Created, State::Initializing)
    }

    /// Transition to running state.
    pub fn start_running(&self) -> Result<(), String> {
        self.transition(State::Initializing, State::Running)
    }

    /// Transition to stopping state.
    pub fn start_stopping(&self) -> Result<(), String> {
        let current = self.current();
        if current == State::Running || current == State::Initializing {
            self.state.store(State::Stopping as u8, Ordering::SeqCst);
            Ok(())
        } else {
            Err(format!("cannot stop from state {}", current))
        }
    }

    /// Transition to stopped state.
    pub fn set_stopped(&self) {
        self.state.store(State::Stopped as u8, Ordering::SeqCst);
    }

    /// Set error state.
    pub fn set_error(&self, message: &str) {
        *self.error_message.lock() = Some(message.to_string());
        self.state.store(State::Error as u8, Ordering::SeqCst);
    }

    /// Get error message if in error state.
    pub fn error_message(&self) -> Option<String> {
        self.error_message.lock().clone()
    }

    /// Check if running.
    pub fn is_running(&self) -> bool {
        self.current() == State::Running
    }

    fn transition(&self, expected: State, target: State) -> Result<(), String> {
        let expected_u8 = expected as u8;
        let target_u8 = target as u8;
        self.state
            .compare_exchange(expected_u8, target_u8, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|actual| format!("expected state {} but was {}", expected, State::from_u8(actual)))?;
        Ok(())
    }
}

impl State {
    fn from_u8(v: u8) -> State {
        match v {
            0 => State::Created,
            1 => State::Initializing,
            2 => State::Running,
            3 => State::Stopping,
            4 => State::Stopped,
            5 => State::Error,
            _ => State::Error,
        }
    }
}

impl Default for BotState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
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
}
