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
mod tests;
