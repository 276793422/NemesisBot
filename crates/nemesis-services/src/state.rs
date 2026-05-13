//! BotState - State machine for BotService lifecycle.
//!
//! Mirrors the Go `BotState` type with JSON serialization, Display, and
//! state-transition predicates (CanStart, CanStop, IsRunning).

use serde::{Deserialize, Serialize};
use std::fmt;

/// Represents the current state of the Bot service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BotState {
    /// Bot has not been started yet.
    #[serde(rename = "not_started")]
    NotStarted,
    /// Bot is currently starting up.
    #[serde(rename = "starting")]
    Starting,
    /// Bot is running normally.
    #[serde(rename = "running")]
    Running,
    /// Bot is in an error state.
    #[serde(rename = "error")]
    Error,
}

impl BotState {
    /// Returns true if the bot is in a running state.
    pub fn is_running(self) -> bool {
        matches!(self, BotState::Running)
    }

    /// Returns true if the bot can be started from this state.
    pub fn can_start(self) -> bool {
        matches!(self, BotState::NotStarted | BotState::Error)
    }

    /// Returns true if the bot can be stopped from this state.
    pub fn can_stop(self) -> bool {
        matches!(self, BotState::Running | BotState::Starting)
    }

    /// Parse a BotState from its string representation.
    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "not_started" => BotState::NotStarted,
            "starting" => BotState::Starting,
            "running" => BotState::Running,
            "error" => BotState::Error,
            _ => BotState::NotStarted,
        }
    }
}

impl fmt::Display for BotState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BotState::NotStarted => write!(f, "not_started"),
            BotState::Starting => write!(f, "starting"),
            BotState::Running => write!(f, "running"),
            BotState::Error => write!(f, "error"),
        }
    }
}

impl Default for BotState {
    fn default() -> Self {
        BotState::NotStarted
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_display() {
        assert_eq!(BotState::NotStarted.to_string(), "not_started");
        assert_eq!(BotState::Starting.to_string(), "starting");
        assert_eq!(BotState::Running.to_string(), "running");
        assert_eq!(BotState::Error.to_string(), "error");
    }

    #[test]
    fn test_serde_roundtrip() {
        let states = vec![
            BotState::NotStarted,
            BotState::Starting,
            BotState::Running,
            BotState::Error,
        ];
        for state in states {
            let json = serde_json::to_string(&state).unwrap();
            let back: BotState = serde_json::from_str(&json).unwrap();
            assert_eq!(state, back);
        }
    }

    #[test]
    fn test_serde_values() {
        assert_eq!(
            serde_json::to_string(&BotState::NotStarted).unwrap(),
            "\"not_started\""
        );
        assert_eq!(
            serde_json::to_string(&BotState::Running).unwrap(),
            "\"running\""
        );
    }

    #[test]
    fn test_is_running() {
        assert!(!BotState::NotStarted.is_running());
        assert!(!BotState::Starting.is_running());
        assert!(BotState::Running.is_running());
        assert!(!BotState::Error.is_running());
    }

    #[test]
    fn test_can_start() {
        assert!(BotState::NotStarted.can_start());
        assert!(!BotState::Starting.can_start());
        assert!(!BotState::Running.can_start());
        assert!(BotState::Error.can_start());
    }

    #[test]
    fn test_can_stop() {
        assert!(!BotState::NotStarted.can_stop());
        assert!(BotState::Starting.can_stop());
        assert!(BotState::Running.can_stop());
        assert!(!BotState::Error.can_stop());
    }

    #[test]
    fn test_from_str_lossy() {
        assert_eq!(BotState::from_str_lossy("not_started"), BotState::NotStarted);
        assert_eq!(BotState::from_str_lossy("starting"), BotState::Starting);
        assert_eq!(BotState::from_str_lossy("running"), BotState::Running);
        assert_eq!(BotState::from_str_lossy("error"), BotState::Error);
        assert_eq!(BotState::from_str_lossy("unknown"), BotState::NotStarted);
    }

    #[test]
    fn test_default() {
        assert_eq!(BotState::default(), BotState::NotStarted);
    }

    #[test]
    fn test_state_transitions() {
        // NotStarted can start
        let state = BotState::NotStarted;
        assert!(state.can_start());
        state_can_transition_to(state, BotState::Starting);

        // Starting can transition to running or error
        let state = BotState::Starting;
        assert!(state.can_stop());
        state_can_transition_to(state, BotState::Running);
        state_can_transition_to(state, BotState::Error);

        // Running can stop
        let state = BotState::Running;
        assert!(state.can_stop());
        assert!(state.is_running());
        state_can_transition_to(state, BotState::Error);

        // Error can start again
        let state = BotState::Error;
        assert!(state.can_start());
        assert!(!state.is_running());
        state_can_transition_to(state, BotState::Starting);
    }

    fn state_can_transition_to(_from: BotState, _to: BotState) {
        // This is a documentation test showing valid transitions
    }

    #[test]
    fn test_all_states_serilize_to_snake_case() {
        assert_eq!(serde_json::to_string(&BotState::NotStarted).unwrap(), "\"not_started\"");
        assert_eq!(serde_json::to_string(&BotState::Starting).unwrap(), "\"starting\"");
        assert_eq!(serde_json::to_string(&BotState::Running).unwrap(), "\"running\"");
        assert_eq!(serde_json::to_string(&BotState::Error).unwrap(), "\"error\"");
    }

    #[test]
    fn test_from_str_lossy_all_valid() {
        assert_eq!(BotState::from_str_lossy("not_started"), BotState::NotStarted);
        assert_eq!(BotState::from_str_lossy("starting"), BotState::Starting);
        assert_eq!(BotState::from_str_lossy("running"), BotState::Running);
        assert_eq!(BotState::from_str_lossy("error"), BotState::Error);
    }

    #[test]
    fn test_from_str_lossy_empty_string() {
        assert_eq!(BotState::from_str_lossy(""), BotState::NotStarted);
    }

    #[test]
    fn test_display_matches_from_str_lossy() {
        for state in [BotState::NotStarted, BotState::Starting, BotState::Running, BotState::Error] {
            let s = state.to_string();
            assert_eq!(BotState::from_str_lossy(&s), state);
        }
    }

    #[test]
    fn test_bot_state_copy() {
        let state = BotState::Running;
        let copied = state;
        assert_eq!(state, copied);
    }

    #[test]
    fn test_bot_state_debug() {
        let debug = format!("{:?}", BotState::Running);
        assert_eq!(debug, "Running");
    }

    #[test]
    fn test_bot_state_hash() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(BotState::Running);
        set.insert(BotState::Running);
        set.insert(BotState::Error);
        assert_eq!(set.len(), 2);
    }
}
