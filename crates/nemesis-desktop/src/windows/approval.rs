//! Approval window - Security approval popup.
//!
//! Manages approval window data and submission logic for security
//! action approvals.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::websocket::client::WebSocketClient;
use super::window_base::{WindowBase, WindowData};

/// Data for an approval window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalWindowData {
    pub request_id: String,
    pub operation: String,
    #[serde(default)]
    pub operation_name: String,
    pub target: String,
    pub risk_level: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub timeout_seconds: i32,
    #[serde(default)]
    pub context: HashMap<String, String>,
    #[serde(default)]
    pub timestamp: i64,
}

impl WindowData for ApprovalWindowData {
    fn validate(&self) -> Result<(), String> {
        if self.request_id.is_empty() {
            return Err("request_id is required".to_string());
        }
        if self.operation.is_empty() {
            return Err("operation is required".to_string());
        }
        Ok(())
    }

    fn get_type(&self) -> String {
        "approval".to_string()
    }
}

impl ApprovalWindowData {
    /// Get the configured timeout as u64 seconds.
    ///
    /// Returns the timeout_seconds value cast to u64. If timeout_seconds
    /// is 0 or negative, returns a default of 60 seconds.
    pub fn get_timeout(&self) -> u64 {
        if self.timeout_seconds > 0 {
            self.timeout_seconds as u64
        } else {
            60
        }
    }
}

/// An approval window instance.
pub struct ApprovalWindow {
    base: WindowBase,
    data: Arc<ApprovalWindowData>,
}

impl ApprovalWindow {
    /// Create a new approval window.
    pub fn new(
        window_id: String,
        data: ApprovalWindowData,
        ws_client: Option<Arc<WebSocketClient>>,
    ) -> Self {
        let data_arc = Arc::new(data);
        Self {
            base: WindowBase::new(
                window_id,
                "approval".to_string(),
                data_arc.clone(),
                ws_client,
            ),
            data: data_arc,
        }
    }

    /// Startup the approval window.
    pub fn startup(&self) -> Result<(), String> {
        self.base.startup()
    }

    /// Shutdown the approval window.
    pub fn shutdown(&self) {
        self.base.shutdown();
    }

    /// Get the window ID.
    pub fn get_id(&self) -> &str {
        self.base.get_id()
    }

    /// Submit an approval decision.
    pub fn submit_approval(&self, approved: bool, reason: &str) -> Result<(), String> {
        let result = serde_json::json!({
            "approved": approved,
            "reason": reason,
            "request_id": self.data.request_id,
            "timestamp": chrono::Utc::now().timestamp(),
        });

        self.base.send_result(result)
    }

    /// Get the approval data.
    pub fn get_data(&self) -> &ApprovalWindowData {
        &self.data
    }

    /// Get the request ID.
    pub fn get_request_id(&self) -> &str {
        &self.data.request_id
    }

    /// Get the operation.
    pub fn get_operation(&self) -> &str {
        &self.data.operation
    }

    /// Get the risk level.
    pub fn get_risk_level(&self) -> &str {
        &self.data.risk_level
    }

    /// Get the target.
    pub fn get_target(&self) -> &str {
        &self.data.target
    }

    /// Get the reason.
    pub fn get_reason(&self) -> &str {
        &self.data.reason
    }

    /// Get the timeout.
    pub fn get_timeout(&self) -> i32 {
        self.data.timeout_seconds
    }

    /// Get the context.
    pub fn get_context(&self) -> &HashMap<String, String> {
        &self.data.context
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
