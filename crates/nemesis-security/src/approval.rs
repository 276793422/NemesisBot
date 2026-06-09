//! Security Approval Workflow
//!
//! Manages approval requests for operations that require human authorisation.
//! Supports both in-memory tracking and multi-process approval via child process spawning.
//!
//! Additional methods matching Go's `ApprovalManager` interface:
//! - `ApprovalConfig` - configuration struct with all Go fields
//! - `set_config()` / `get_config()` on `MultiProcessApprovalManager`

use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;
use tokio::time::timeout;

// ---------------------------------------------------------------------------
// ApprovalConfig
// ---------------------------------------------------------------------------

/// Configuration for the approval manager.
///
/// Equivalent to Go's `ApprovalConfig`.
#[derive(Debug, Clone)]
pub struct ApprovalConfig {
    /// Whether approval is enabled.
    pub enabled: bool,
    /// Timeout for approval dialogs.
    pub timeout: Duration,
    /// Minimum risk level that triggers approval ("LOW", "MEDIUM", "HIGH", "CRITICAL").
    pub min_risk_level: String,
    /// Dialog width in pixels.
    pub dialog_width: u32,
    /// Dialog height in pixels.
    pub dialog_height: u32,
    /// Whether to play a sound when requesting approval.
    pub enable_sound: bool,
    /// Whether to show animation in the approval dialog.
    pub enable_animation: bool,
}

impl Default for ApprovalConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            timeout: Duration::from_secs(30),
            min_risk_level: "MEDIUM".to_string(),
            dialog_width: 550,
            dialog_height: 480,
            enable_sound: true,
            enable_animation: true,
        }
    }
}

/// Status of an approval request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalStatus {
    /// Waiting for a decision.
    Pending,
    /// Approved by an authorised party.
    Approved,
    /// Explicitly denied.
    Denied,
    /// Timed out before a decision was made.
    Expired,
}

impl std::fmt::Display for ApprovalStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Approved => write!(f, "approved"),
            Self::Denied => write!(f, "denied"),
            Self::Expired => write!(f, "expired"),
        }
    }
}

/// A single approval request (basic in-memory variant).
#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    /// Unique identifier.
    pub id: String,
    /// Description of the operation to be approved.
    pub operation: String,
    /// Identity of the requester.
    pub requester: String,
    /// RFC 3339 timestamp of when the request was created.
    pub timestamp: String,
    /// Current status.
    pub status: ApprovalStatus,
    /// Optional reason for denial (set when status becomes Denied).
    pub deny_reason: Option<String>,
}

// ---------------------------------------------------------------------------
// Multi-process approval types (matching Go's approval package)
// ---------------------------------------------------------------------------

/// Detailed approval request for multi-process mode (matches Go's ApprovalRequest).
#[derive(Debug, Clone)]
pub struct MultiApprovalRequest {
    /// Unique request ID.
    pub request_id: String,
    /// Operation type string (e.g. "file_write", "process_exec").
    pub operation: String,
    /// Target of the operation (file path, command, URL, etc.).
    pub target: String,
    /// Risk level string ("LOW", "MEDIUM", "HIGH", "CRITICAL").
    pub risk_level: String,
    /// Reason why approval is needed.
    pub reason: String,
    /// Additional context key-value pairs.
    pub context: HashMap<String, String>,
    /// Timeout in seconds for the approval dialog.
    pub timeout_seconds: u64,
    /// Unix timestamp of when the request was created.
    pub timestamp: i64,
}

impl MultiApprovalRequest {
    /// Validate the request fields.
    ///
    /// Mirrors Go's `ApprovalRequest.Validate`. Checks all required fields
    /// are present and that risk_level is a valid value.
    pub fn validate(&self) -> Result<(), String> {
        if self.request_id.is_empty() {
            return Err("request_id is required".to_string());
        }
        if self.operation.is_empty() {
            return Err("operation is required".to_string());
        }
        if self.target.is_empty() {
            return Err("target is required".to_string());
        }
        if self.risk_level.is_empty() {
            return Err("risk_level is required".to_string());
        }
        let valid_levels = ["LOW", "MEDIUM", "HIGH", "CRITICAL"];
        if !valid_levels.contains(&self.risk_level.as_str()) {
            return Err(format!("invalid risk_level: {}", self.risk_level));
        }
        if self.timeout_seconds == 0 {
            return Err("timeout_seconds must be positive".to_string());
        }
        Ok(())
    }
}

/// Response from a multi-process approval dialog (matches Go's ApprovalResponse).
#[derive(Debug, Clone)]
pub struct MultiApprovalResponse {
    /// The request ID this response corresponds to.
    pub request_id: String,
    /// Whether the operation was approved.
    pub approved: bool,
    /// Whether the request timed out waiting for user response.
    pub timed_out: bool,
    /// How long the approval process took in seconds.
    pub duration_seconds: f64,
    /// Unix timestamp of when the response was received.
    pub response_time: i64,
}

/// Trait for child process factories that can spawn approval windows.
///
/// In a desktop environment, this would spawn a subprocess that displays a
/// GUI approval dialog. Implementations are provided by the desktop layer.
pub trait ChildProcessFactory: Send + Sync {
    /// Spawn a child process of the given window type with the provided data.
    ///
    /// Returns:
    /// - `child_id`: The ID of the spawned child process.
    /// - A receiver channel that will yield the result when the child completes.
    fn spawn_child(
        &self,
        window_type: &str,
        data: HashMap<String, serde_json::Value>,
    ) -> Result<(String, oneshot::Receiver<serde_json::Value>), String>;
}

// ---------------------------------------------------------------------------
// Safe operations auto-approval
// ---------------------------------------------------------------------------

/// Determine whether an operation is safe enough to auto-approve without UI.
///
/// Safe operations (LOW risk reads, listings, etc.) can proceed without
/// human approval when no child process factory is available.
pub fn is_safe_operation(operation: &str, risk_level: &str) -> bool {
    let safe_ops = [
        "file_read",
        "dir_list",
        "network_request",
        "hardware_i2c",
        "registry_read",
    ];
    safe_ops.contains(&operation) && risk_level == "LOW"
}

/// Get a human-readable display name for an operation.
pub fn operation_display_name(operation: &str) -> String {
    match operation {
        "file_read" => "File Read".to_string(),
        "file_write" => "File Write".to_string(),
        "file_delete" => "File Delete".to_string(),
        "dir_read" | "dir_list" => "Directory Listing".to_string(),
        "dir_create" => "Create Directory".to_string(),
        "dir_delete" => "Delete Directory".to_string(),
        "process_exec" => "Execute Command".to_string(),
        "process_spawn" => "Spawn Process".to_string(),
        "process_kill" => "Kill Process".to_string(),
        "network_request" => "Network Request".to_string(),
        "network_download" => "Download File".to_string(),
        "network_upload" => "Upload File".to_string(),
        "hardware_i2c" => "I2C Access".to_string(),
        "hardware_spi" => "SPI Access".to_string(),
        "hardware_gpio" => "GPIO Access".to_string(),
        "registry_read" => "Registry Read".to_string(),
        "registry_write" => "Registry Write".to_string(),
        "registry_delete" => "Registry Delete".to_string(),
        "system_shutdown" => "System Shutdown".to_string(),
        "system_reboot" => "System Reboot".to_string(),
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------
// ApprovalManager (basic in-memory)
// ---------------------------------------------------------------------------

/// Manager that holds pending approval requests in memory and provides
/// operations for the full approve / deny / expire lifecycle.
pub struct ApprovalManager {
    /// Seconds after which a pending request is considered expired.
    timeout_secs: u64,
    requests: RwLock<HashMap<String, ApprovalRequest>>,
}

impl ApprovalManager {
    /// Create a new manager with the given expiry timeout (in seconds).
    pub fn new(timeout_secs: u64) -> Self {
        Self {
            timeout_secs,
            requests: RwLock::new(HashMap::new()),
        }
    }

    /// Create a manager with a default 5-minute timeout.
    pub fn with_default_timeout() -> Self {
        Self::new(300)
    }

    /// Submit a new approval request.
    ///
    /// Returns the generated request ID.
    pub fn request_approval(&self, operation: &str, requester: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let req = ApprovalRequest {
            id: id.clone(),
            operation: operation.to_string(),
            requester: requester.to_string(),
            timestamp: chrono::Local::now().to_rfc3339(),
            status: ApprovalStatus::Pending,
            deny_reason: None,
        };
        self.requests.write().insert(id.clone(), req);
        id
    }

    /// Approve a pending request.
    ///
    /// Returns `Ok(())` if the request existed and was pending.
    /// Returns `Err` with a description if not found or not pending.
    pub fn approve(&self, request_id: &str) -> Result<(), String> {
        let mut map = self.requests.write();
        let req = map
            .get_mut(request_id)
            .ok_or_else(|| format!("request not found: {}", request_id))?;
        if req.status != ApprovalStatus::Pending {
            return Err(format!(
                "request is not pending (current: {})",
                req.status
            ));
        }
        req.status = ApprovalStatus::Approved;
        Ok(())
    }

    /// Deny a pending request.
    pub fn deny(&self, request_id: &str, reason: &str) -> Result<(), String> {
        let mut map = self.requests.write();
        let req = map
            .get_mut(request_id)
            .ok_or_else(|| format!("request not found: {}", request_id))?;
        if req.status != ApprovalStatus::Pending {
            return Err(format!(
                "request is not pending (current: {})",
                req.status
            ));
        }
        req.status = ApprovalStatus::Denied;
        req.deny_reason = Some(reason.to_string());
        Ok(())
    }

    /// Remove all expired pending requests.
    ///
    /// A request is expired when the elapsed time since its `timestamp` exceeds
    /// `timeout_secs`.
    ///
    /// Returns the number of requests that were expired.
    pub fn cleanup_expired(&self) -> usize {
        let now = chrono::Local::now();
        let timeout_dur = chrono::Duration::seconds(self.timeout_secs as i64);

        let mut map = self.requests.write();
        let expired_ids: Vec<String> = map
            .iter()
            .filter(|(_, req)| {
                if req.status != ApprovalStatus::Pending {
                    return false;
                }
                let Ok(ts) = chrono::DateTime::parse_from_rfc3339(&req.timestamp) else {
                    return false;
                };
                now.signed_duration_since(ts.with_timezone(&chrono::Local)) > timeout_dur
            })
            .map(|(id, _)| id.clone())
            .collect();

        let count = expired_ids.len();
        for id in &expired_ids {
            if let Some(req) = map.get_mut(id) {
                req.status = ApprovalStatus::Expired;
            }
        }
        count
    }

    /// List all currently pending requests.
    pub fn list_pending(&self) -> Vec<ApprovalRequest> {
        self.requests
            .read()
            .values()
            .filter(|r| r.status == ApprovalStatus::Pending)
            .cloned()
            .collect()
    }

    /// Get a specific request by ID.
    pub fn get(&self, request_id: &str) -> Option<ApprovalRequest> {
        self.requests.read().get(request_id).cloned()
    }

    /// Total number of requests (any status) currently tracked.
    pub fn total_count(&self) -> usize {
        self.requests.read().len()
    }
}

// ---------------------------------------------------------------------------
// MultiProcessApprovalManager
// ---------------------------------------------------------------------------

/// Multi-process approval manager that uses a child process factory for UI dialogs.
///
/// When a child process factory is available, it spawns an approval window subprocess.
/// When no factory is available, it falls back to auto-approving safe operations
/// or rejecting dangerous ones.
pub struct MultiProcessApprovalManager {
    /// Seconds after which a pending request is considered expired.
    timeout_secs: u64,
    /// Optional child process factory for spawning approval dialogs.
    child_factory: Arc<Mutex<Option<Arc<dyn ChildProcessFactory>>>>,
    /// Whether the manager is currently running.
    running: AtomicBool,
    /// Configuration for approval behavior.
    config: RwLock<ApprovalConfig>,
}

use std::sync::atomic::{AtomicBool, Ordering};

impl MultiProcessApprovalManager {
    /// Create a new multi-process approval manager.
    pub fn new(timeout_secs: u64) -> Self {
        Self {
            timeout_secs,
            child_factory: Arc::new(Mutex::new(None)),
            running: AtomicBool::new(false),
            config: RwLock::new(ApprovalConfig::default()),
        }
    }

    /// Create with default 30-second timeout.
    pub fn with_default_timeout() -> Self {
        Self::new(30)
    }

    /// Set the child process factory for spawning approval windows.
    pub fn set_child_factory(&self, factory: Arc<dyn ChildProcessFactory>) {
        *self.child_factory.lock() = Some(factory);
        tracing::info!("[approval] ChildProcessFactory registered");
    }

    /// Dynamically update the approval configuration.
    ///
    /// Equivalent to Go's `ApprovalManager.SetConfig()`.
    pub fn set_config(&self, config: ApprovalConfig) {
        tracing::info!(
            enabled = config.enabled,
            timeout_secs = config.timeout.as_secs(),
            min_risk_level = %config.min_risk_level,
            dialog_width = config.dialog_width,
            dialog_height = config.dialog_height,
            "[approval] Configuration updated"
        );
        *self.config.write() = config;
    }

    /// Get the current approval configuration.
    ///
    /// Equivalent to Go's `ApprovalManager.GetConfig()`.
    pub fn get_config(&self) -> ApprovalConfig {
        self.config.read().clone()
    }

    /// Start the approval manager.
    pub fn start(&self) -> Result<(), String> {
        self.running.store(true, Ordering::SeqCst);
        tracing::info!("[approval] Multi-process approval manager started");
        Ok(())
    }

    /// Stop the approval manager.
    pub fn stop(&self) -> Result<(), String> {
        self.running.store(false, Ordering::SeqCst);
        tracing::info!("[approval] Multi-process approval manager stopped");
        Ok(())
    }

    /// Check if the manager is running.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Request approval for an operation using the multi-process flow.
    ///
    /// If a child process factory is available, spawns an approval dialog subprocess
    /// and waits for the user's response. Falls back to auto-approve/reject logic
    /// when no factory is set.
    ///
    /// This is an async method that mirrors the Go `RequestApproval` implementation.
    pub async fn request_approval(
        &self,
        req: &MultiApprovalRequest,
    ) -> Result<MultiApprovalResponse, String> {
        if !self.is_running() {
            return Err("approval manager is not running".to_string());
        }

        tracing::info!(
            "[approval] Requesting approval (multi-process): request_id={}, operation={}, target={}, risk_level={}",
            req.request_id, req.operation, req.target, req.risk_level
        );

        let start = Instant::now();

        // Check if a child process factory is available
        let factory = self.child_factory.lock().clone();

        match factory {
            None => {
                // No child process factory - fallback behavior
                tracing::info!(
                    "[approval] No ChildProcessFactory set, using default behavior"
                );

                if is_safe_operation(&req.operation, &req.risk_level) {
                    tracing::info!(
                        "[approval] Auto-approving safe operation: {}",
                        req.operation
                    );
                    return Ok(MultiApprovalResponse {
                        request_id: req.request_id.clone(),
                        approved: true,
                        timed_out: false,
                        duration_seconds: start.elapsed().as_secs_f64(),
                        response_time: chrono::Local::now().timestamp(),
                    });
                }

                tracing::info!(
                    "[approval] No ChildProcessFactory available, rejecting dangerous operation"
                );
                return Ok(MultiApprovalResponse {
                    request_id: req.request_id.clone(),
                    approved: false,
                    timed_out: false,
                    duration_seconds: start.elapsed().as_secs_f64(),
                    response_time: chrono::Local::now().timestamp(),
                });
            }
            Some(factory) => {
                // Prepare window data for the child process
                let mut window_data = HashMap::new();
                window_data.insert(
                    "request_id".to_string(),
                    serde_json::Value::String(req.request_id.clone()),
                );
                window_data.insert(
                    "operation".to_string(),
                    serde_json::Value::String(req.operation.clone()),
                );
                window_data.insert(
                    "operation_name".to_string(),
                    serde_json::Value::String(operation_display_name(&req.operation)),
                );
                window_data.insert(
                    "target".to_string(),
                    serde_json::Value::String(req.target.clone()),
                );
                window_data.insert(
                    "risk_level".to_string(),
                    serde_json::Value::String(req.risk_level.clone()),
                );
                window_data.insert(
                    "reason".to_string(),
                    serde_json::Value::String(req.reason.clone()),
                );
                window_data.insert(
                    "timeout_seconds".to_string(),
                    serde_json::Value::Number(req.timeout_seconds.into()),
                );
                window_data.insert(
                    "timestamp".to_string(),
                    serde_json::Value::Number(chrono::Local::now().timestamp().into()),
                );

                // Spawn child process
                tracing::info!("[approval] Creating child process for approval window");

                let (child_id, result_rx) = match factory.spawn_child("approval", window_data) {
                    Ok(result) => result,
                    Err(e) => {
                        // If popup is not supported, reject the request
                        if e.contains("popup not supported") {
                            tracing::info!(
                                "[approval] Popup not supported, rejecting request: {}",
                                req.request_id
                            );
                            return Ok(MultiApprovalResponse {
                                request_id: req.request_id.clone(),
                                approved: false,
                                timed_out: false,
                                duration_seconds: start.elapsed().as_secs_f64(),
                                response_time: chrono::Local::now().timestamp(),
                            });
                        }
                        return Err(format!("failed to create approval window: {}", e));
                    }
                };

                tracing::info!(
                    "[approval] Child process created: child_id={}",
                    child_id
                );

                // Wait for result with timeout
                let timeout_duration = if req.timeout_seconds > 0 {
                    Duration::from_secs(req.timeout_seconds)
                } else {
                    Duration::from_secs(self.timeout_secs)
                };

                match timeout(timeout_duration, result_rx).await {
                    Ok(Ok(result)) => {
                        // Parse the result from the child process
                        let approved = result
                            .get("approved")
                            .or_else(|| result.get("data").and_then(|d| d.get("approved")))
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);

                        let resp = MultiApprovalResponse {
                            request_id: req.request_id.clone(),
                            approved,
                            timed_out: false,
                            duration_seconds: start.elapsed().as_secs_f64(),
                            response_time: chrono::Local::now().timestamp(),
                        };

                        tracing::info!(
                            "[approval] Approval received: request_id={}, approved={}, duration={:.3}",
                            resp.request_id, resp.approved, resp.duration_seconds
                        );

                        Ok(resp)
                    }
                    Ok(Err(_)) => {
                        // Channel closed without result
                        Err("child process returned empty result".to_string())
                    }
                    Err(_) => {
                        // Timed out
                        tracing::info!(
                            "[approval] Approval timed out: request_id={}, duration={:.3}",
                            req.request_id,
                            start.elapsed().as_secs_f64()
                        );

                        Ok(MultiApprovalResponse {
                            request_id: req.request_id.clone(),
                            approved: false,
                            timed_out: true,
                            duration_seconds: start.elapsed().as_secs_f64(),
                            response_time: chrono::Local::now().timestamp(),
                        })
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
