//! BotState - State machine for BotService lifecycle.
//!
//! Mirrors the Go `BotState` type with JSON serialization, Display, and
//! state-transition predicates (CanStart, CanStop, IsRunning).

use serde::{Deserialize, Serialize};
use std::fmt;

/// Represents the current state of the Bot service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum BotState {
    /// Bot has not been started yet.
    #[serde(rename = "not_started")]
    #[default]
    NotStarted,
    /// Bot is currently starting up.
    #[serde(rename = "starting")]
    Starting,
    /// Bot is running normally.
    #[serde(rename = "running")]
    Running,
    /// Bot is in the middle of stopping (BUG-20261123-008 stop() 占坑态：
    /// can_stop 检查与占坑在同一 write 锁区间内原子完成，并发第二个 stop
    /// 在此状态上诚实拒绝——此前 check-then-act 分离，Ctrl+C 臂与
    /// /api/internal 双停机源赛跑可双双通过 can_stop、双调 stop_all）。
    #[serde(rename = "stopping")]
    Stopping,
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
            "stopping" => BotState::Stopping,
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
            BotState::Stopping => write!(f, "stopping"),
            BotState::Error => write!(f, "error"),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
