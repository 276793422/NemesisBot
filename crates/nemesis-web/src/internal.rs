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
    /// L9 selfupdate：CLI `nemesisbot selfupdate apply` 经 /api/internal 触发
    /// gateway 侧九步 apply（estop/验签/v4 签名闸全在 gateway 侧同源执行；
    /// apply 完成后 gateway 自行走优雅停机——新进程已在等本进程退场）。
    SelfUpdateApply {
        allow_downgrade: bool,
    },
}
