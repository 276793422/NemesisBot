//! Headless window - Windowless mode for testing.
//!
//! Runs without a UI, auto-approving after a delay. Used for testing
//! the communication flow without requiring a real window.

use std::sync::Arc;
use std::time::Duration;

use tracing::info;

use crate::websocket::client::WebSocketClient;
use super::approval::ApprovalWindowData;

/// Run a headless window that auto-approves after a delay.
///
/// This is used for testing the parent-child communication flow
/// without requiring a real window.
pub async fn run_headless_window(
    window_id: &str,
    data: &ApprovalWindowData,
    ws_client: Option<Arc<WebSocketClient>>,
) -> Result<(), String> {
    info!("[HeadlessWindow] {}: Starting", window_id);

    // Wait 1 second before auto-approving
    tokio::time::sleep(Duration::from_secs(1)).await;

    let result = serde_json::json!({
        "approved": true,
        "reason": "auto-approve (test mode)",
        "request_id": data.request_id,
        "timestamp": chrono::Utc::now().timestamp(),
    });

    info!("[HeadlessWindow] {}: Sending auto-approve result", window_id);

    // Send result via WebSocket
    if let Some(ref client) = ws_client {
        client.notify("approval.submit", result)?;
    }

    // Keep alive for a bit to ensure result is sent
    tokio::time::sleep(Duration::from_secs(2)).await;

    info!("[HeadlessWindow] {}: Completed", window_id);
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
