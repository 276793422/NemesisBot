use serde::{Deserialize, Serialize};

/// Commands delivered to the gateway process via `POST /api/internal`
/// (fire-and-forget mpsc; see [`crate::api_handlers::handle_api_internal`]
/// and the gateway's internal-command listener in
/// `nemesisbot/src/commands/gateway.rs`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InternalCommand {
    OpenDashboard,
    /// (BUG #31, quality-hardening goal 冲刺 S11e) Ask the gateway for the
    /// same graceful-stop path as Ctrl+C (ServiceManager broadcast →
    /// Step 23 `wait_for_shutdown` releases → Step 24 teardown). This is the
    /// backend of the CLI `nemesisbot shutdown` Method-3 HTTP fallback.
    Shutdown,
}
