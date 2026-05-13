//! Bot state machine and persistent workspace state.

pub mod state;
pub mod workspace_state;

pub use state::BotState;
pub use workspace_state::{WorkspaceState, WorkspaceStateManager, is_internal_channel};
