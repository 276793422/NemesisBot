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
            timestamp: chrono::Utc::now().to_rfc3339(),
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
        let now = chrono::Utc::now();
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
                now.signed_duration_since(ts.with_timezone(&chrono::Utc)) > timeout_dur
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
                        response_time: chrono::Utc::now().timestamp(),
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
                    response_time: chrono::Utc::now().timestamp(),
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
                    serde_json::Value::Number(chrono::Utc::now().timestamp().into()),
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
                                response_time: chrono::Utc::now().timestamp(),
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
                            response_time: chrono::Utc::now().timestamp(),
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
                            response_time: chrono::Utc::now().timestamp(),
                        })
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_and_approve() {
        let mgr = ApprovalManager::with_default_timeout();
        let id = mgr.request_approval("file_write /etc/hosts", "agent-1");
        let req = mgr.get(&id).unwrap();
        assert_eq!(req.status, ApprovalStatus::Pending);
        assert_eq!(req.requester, "agent-1");

        mgr.approve(&id).unwrap();
        let req = mgr.get(&id).unwrap();
        assert_eq!(req.status, ApprovalStatus::Approved);
    }

    #[test]
    fn test_request_and_deny() {
        let mgr = ApprovalManager::with_default_timeout();
        let id = mgr.request_approval("process_exec rm -rf /", "agent-2");

        mgr.deny(&id, "dangerous operation").unwrap();
        let req = mgr.get(&id).unwrap();
        assert_eq!(req.status, ApprovalStatus::Denied);
        assert_eq!(req.deny_reason.as_deref(), Some("dangerous operation"));
    }

    #[test]
    fn test_approve_nonexistent_fails() {
        let mgr = ApprovalManager::with_default_timeout();
        assert!(mgr.approve("does-not-exist").is_err());
    }

    #[test]
    fn test_double_approve_fails() {
        let mgr = ApprovalManager::with_default_timeout();
        let id = mgr.request_approval("file_read /tmp/test", "agent-3");
        mgr.approve(&id).unwrap();
        // Second approval should fail because status is no longer Pending.
        assert!(mgr.approve(&id).is_err());
    }

    #[test]
    fn test_cleanup_expired() {
        // Use a 0-second timeout so everything is immediately expired.
        let mgr = ApprovalManager::new(0);

        let id1 = mgr.request_approval("op-a", "agent");
        let id2 = mgr.request_approval("op-b", "agent");

        // Give a tiny moment so the timestamps are captured.
        std::thread::sleep(std::time::Duration::from_millis(10));

        let expired = mgr.cleanup_expired();
        assert_eq!(expired, 2);

        assert_eq!(mgr.get(&id1).unwrap().status, ApprovalStatus::Expired);
        assert_eq!(mgr.get(&id2).unwrap().status, ApprovalStatus::Expired);

        // Pending list should be empty after cleanup.
        assert!(mgr.list_pending().is_empty());
    }

    // ---- Multi-process tests ----

    #[test]
    fn test_multi_process_start_stop() {
        let mgr = MultiProcessApprovalManager::new(30);
        assert!(!mgr.is_running());
        mgr.start().unwrap();
        assert!(mgr.is_running());
        mgr.stop().unwrap();
        assert!(!mgr.is_running());
    }

    #[tokio::test]
    async fn test_multi_process_not_running_error() {
        let mgr = MultiProcessApprovalManager::new(30);
        let req = MultiApprovalRequest {
            request_id: "test-1".to_string(),
            operation: "file_write".to_string(),
            target: "/tmp/test".to_string(),
            risk_level: "HIGH".to_string(),
            reason: "test".to_string(),
            context: HashMap::new(),
            timeout_seconds: 5,
            timestamp: chrono::Utc::now().timestamp(),
        };
        let result = mgr.request_approval(&req).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not running"));
    }

    #[tokio::test]
    async fn test_multi_process_auto_approve_safe() {
        let mgr = MultiProcessApprovalManager::new(30);
        mgr.start().unwrap();

        let req = MultiApprovalRequest {
            request_id: "test-safe".to_string(),
            operation: "file_read".to_string(),
            target: "/tmp/test".to_string(),
            risk_level: "LOW".to_string(),
            reason: "test".to_string(),
            context: HashMap::new(),
            timeout_seconds: 5,
            timestamp: chrono::Utc::now().timestamp(),
        };
        let resp = mgr.request_approval(&req).await.unwrap();
        assert!(resp.approved);
        assert!(!resp.timed_out);
    }

    #[tokio::test]
    async fn test_multi_process_reject_dangerous_no_factory() {
        let mgr = MultiProcessApprovalManager::new(30);
        mgr.start().unwrap();

        let req = MultiApprovalRequest {
            request_id: "test-danger".to_string(),
            operation: "process_exec".to_string(),
            target: "rm -rf /".to_string(),
            risk_level: "CRITICAL".to_string(),
            reason: "dangerous".to_string(),
            context: HashMap::new(),
            timeout_seconds: 5,
            timestamp: chrono::Utc::now().timestamp(),
        };
        let resp = mgr.request_approval(&req).await.unwrap();
        assert!(!resp.approved);
        assert!(!resp.timed_out);
    }

    #[test]
    fn test_is_safe_operation() {
        assert!(is_safe_operation("file_read", "LOW"));
        assert!(is_safe_operation("dir_list", "LOW"));
        assert!(is_safe_operation("network_request", "LOW"));
        assert!(!is_safe_operation("file_read", "MEDIUM"));
        assert!(!is_safe_operation("process_exec", "LOW"));
        assert!(!is_safe_operation("file_write", "LOW"));
    }

    #[test]
    fn test_operation_display_name() {
        assert_eq!(operation_display_name("file_read"), "File Read");
        assert_eq!(operation_display_name("process_exec"), "Execute Command");
        assert_eq!(operation_display_name("network_request"), "Network Request");
        assert_eq!(operation_display_name("unknown_op"), "unknown_op");
    }

    // ---- Additional approval tests ----

    #[test]
    fn test_approval_status_display() {
        assert_eq!(format!("{}", ApprovalStatus::Pending), "pending");
        assert_eq!(format!("{}", ApprovalStatus::Approved), "approved");
        assert_eq!(format!("{}", ApprovalStatus::Denied), "denied");
        assert_eq!(format!("{}", ApprovalStatus::Expired), "expired");
    }

    #[test]
    fn test_approval_config_default() {
        let config = ApprovalConfig::default();
        assert!(config.enabled);
        assert_eq!(config.timeout, Duration::from_secs(30));
        assert_eq!(config.min_risk_level, "MEDIUM");
        assert_eq!(config.dialog_width, 550);
        assert_eq!(config.dialog_height, 480);
        assert!(config.enable_sound);
        assert!(config.enable_animation);
    }

    #[test]
    fn test_multi_approval_request_validate_valid() {
        let req = MultiApprovalRequest {
            request_id: "req-1".to_string(),
            operation: "file_write".to_string(),
            target: "/tmp/test".to_string(),
            risk_level: "HIGH".to_string(),
            reason: "test reason".to_string(),
            context: HashMap::new(),
            timeout_seconds: 30,
            timestamp: chrono::Utc::now().timestamp(),
        };
        assert!(req.validate().is_ok());
    }

    #[test]
    fn test_multi_approval_request_validate_all_risk_levels() {
        for level in &["LOW", "MEDIUM", "HIGH", "CRITICAL"] {
            let req = MultiApprovalRequest {
                request_id: "req-1".to_string(),
                operation: "file_write".to_string(),
                target: "/tmp/test".to_string(),
                risk_level: level.to_string(),
                reason: "test".to_string(),
                context: HashMap::new(),
                timeout_seconds: 30,
                timestamp: 0,
            };
            assert!(req.validate().is_ok(), "risk_level {} should be valid", level);
        }
    }

    #[test]
    fn test_multi_approval_request_validate_empty_request_id() {
        let req = MultiApprovalRequest {
            request_id: "".to_string(),
            operation: "file_write".to_string(),
            target: "/tmp/test".to_string(),
            risk_level: "HIGH".to_string(),
            reason: "test".to_string(),
            context: HashMap::new(),
            timeout_seconds: 30,
            timestamp: 0,
        };
        assert!(req.validate().is_err());
        assert!(req.validate().unwrap_err().contains("request_id"));
    }

    #[test]
    fn test_multi_approval_request_validate_empty_operation() {
        let req = MultiApprovalRequest {
            request_id: "req-1".to_string(),
            operation: "".to_string(),
            target: "/tmp/test".to_string(),
            risk_level: "HIGH".to_string(),
            reason: "test".to_string(),
            context: HashMap::new(),
            timeout_seconds: 30,
            timestamp: 0,
        };
        assert!(req.validate().is_err());
        assert!(req.validate().unwrap_err().contains("operation"));
    }

    #[test]
    fn test_multi_approval_request_validate_empty_target() {
        let req = MultiApprovalRequest {
            request_id: "req-1".to_string(),
            operation: "file_write".to_string(),
            target: "".to_string(),
            risk_level: "HIGH".to_string(),
            reason: "test".to_string(),
            context: HashMap::new(),
            timeout_seconds: 30,
            timestamp: 0,
        };
        assert!(req.validate().is_err());
        assert!(req.validate().unwrap_err().contains("target"));
    }

    #[test]
    fn test_multi_approval_request_validate_empty_risk_level() {
        let req = MultiApprovalRequest {
            request_id: "req-1".to_string(),
            operation: "file_write".to_string(),
            target: "/tmp/test".to_string(),
            risk_level: "".to_string(),
            reason: "test".to_string(),
            context: HashMap::new(),
            timeout_seconds: 30,
            timestamp: 0,
        };
        assert!(req.validate().is_err());
        assert!(req.validate().unwrap_err().contains("risk_level"));
    }

    #[test]
    fn test_multi_approval_request_validate_invalid_risk_level() {
        let req = MultiApprovalRequest {
            request_id: "req-1".to_string(),
            operation: "file_write".to_string(),
            target: "/tmp/test".to_string(),
            risk_level: "INVALID".to_string(),
            reason: "test".to_string(),
            context: HashMap::new(),
            timeout_seconds: 30,
            timestamp: 0,
        };
        assert!(req.validate().is_err());
        assert!(req.validate().unwrap_err().contains("invalid risk_level"));
    }

    #[test]
    fn test_multi_approval_request_validate_zero_timeout() {
        let req = MultiApprovalRequest {
            request_id: "req-1".to_string(),
            operation: "file_write".to_string(),
            target: "/tmp/test".to_string(),
            risk_level: "HIGH".to_string(),
            reason: "test".to_string(),
            context: HashMap::new(),
            timeout_seconds: 0,
            timestamp: 0,
        };
        assert!(req.validate().is_err());
        assert!(req.validate().unwrap_err().contains("timeout_seconds"));
    }

    #[test]
    fn test_is_safe_operation_all_safe_ops() {
        assert!(is_safe_operation("file_read", "LOW"));
        assert!(is_safe_operation("dir_list", "LOW"));
        assert!(is_safe_operation("network_request", "LOW"));
        assert!(is_safe_operation("hardware_i2c", "LOW"));
        assert!(is_safe_operation("registry_read", "LOW"));
    }

    #[test]
    fn test_is_safe_operation_non_low_risk() {
        assert!(!is_safe_operation("file_read", "MEDIUM"));
        assert!(!is_safe_operation("file_read", "HIGH"));
        assert!(!is_safe_operation("file_read", "CRITICAL"));
    }

    #[test]
    fn test_is_safe_operation_dangerous_op() {
        assert!(!is_safe_operation("file_write", "LOW"));
        assert!(!is_safe_operation("process_exec", "LOW"));
        assert!(!is_safe_operation("file_delete", "LOW"));
        assert!(!is_safe_operation("registry_write", "LOW"));
    }

    #[test]
    fn test_operation_display_name_all_known_ops() {
        assert_eq!(operation_display_name("file_read"), "File Read");
        assert_eq!(operation_display_name("file_write"), "File Write");
        assert_eq!(operation_display_name("file_delete"), "File Delete");
        assert_eq!(operation_display_name("dir_read"), "Directory Listing");
        assert_eq!(operation_display_name("dir_list"), "Directory Listing");
        assert_eq!(operation_display_name("dir_create"), "Create Directory");
        assert_eq!(operation_display_name("dir_delete"), "Delete Directory");
        assert_eq!(operation_display_name("process_exec"), "Execute Command");
        assert_eq!(operation_display_name("process_spawn"), "Spawn Process");
        assert_eq!(operation_display_name("process_kill"), "Kill Process");
        assert_eq!(operation_display_name("network_request"), "Network Request");
        assert_eq!(operation_display_name("network_download"), "Download File");
        assert_eq!(operation_display_name("network_upload"), "Upload File");
        assert_eq!(operation_display_name("hardware_i2c"), "I2C Access");
        assert_eq!(operation_display_name("hardware_spi"), "SPI Access");
        assert_eq!(operation_display_name("hardware_gpio"), "GPIO Access");
        assert_eq!(operation_display_name("registry_read"), "Registry Read");
        assert_eq!(operation_display_name("registry_write"), "Registry Write");
        assert_eq!(operation_display_name("registry_delete"), "Registry Delete");
        assert_eq!(operation_display_name("system_shutdown"), "System Shutdown");
        assert_eq!(operation_display_name("system_reboot"), "System Reboot");
    }

    #[test]
    fn test_approval_manager_new_custom_timeout() {
        let mgr = ApprovalManager::new(60);
        // Should work normally
        let id = mgr.request_approval("file_write", "agent-1");
        let req = mgr.get(&id).unwrap();
        assert_eq!(req.status, ApprovalStatus::Pending);
    }

    #[test]
    fn test_approval_manager_request_generates_unique_ids() {
        let mgr = ApprovalManager::with_default_timeout();
        let id1 = mgr.request_approval("op1", "agent-1");
        let id2 = mgr.request_approval("op2", "agent-2");
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_approval_manager_deny_sets_reason() {
        let mgr = ApprovalManager::with_default_timeout();
        let id = mgr.request_approval("file_delete", "agent-1");
        mgr.deny(&id, "dangerous").unwrap();
        let req = mgr.get(&id).unwrap();
        assert_eq!(req.status, ApprovalStatus::Denied);
        assert_eq!(req.deny_reason.as_deref(), Some("dangerous"));
    }

    #[test]
    fn test_approval_manager_deny_nonexistent() {
        let mgr = ApprovalManager::with_default_timeout();
        assert!(mgr.deny("nonexistent", "reason").is_err());
    }

    #[test]
    fn test_approval_manager_deny_already_denied() {
        let mgr = ApprovalManager::with_default_timeout();
        let id = mgr.request_approval("op", "agent");
        mgr.deny(&id, "first").unwrap();
        assert!(mgr.deny(&id, "second").is_err());
    }

    #[test]
    fn test_approval_manager_total_count() {
        let mgr = ApprovalManager::with_default_timeout();
        assert_eq!(mgr.total_count(), 0);
        let id1 = mgr.request_approval("op1", "agent");
        let _id2 = mgr.request_approval("op2", "agent");
        assert_eq!(mgr.total_count(), 2);
        mgr.approve(&id1).unwrap();
        assert_eq!(mgr.total_count(), 2); // Still tracked
    }

    #[test]
    fn test_approval_manager_list_pending_after_approve() {
        let mgr = ApprovalManager::with_default_timeout();
        let id1 = mgr.request_approval("op1", "agent");
        let _id2 = mgr.request_approval("op2", "agent");
        assert_eq!(mgr.list_pending().len(), 2);
        mgr.approve(&id1).unwrap();
        assert_eq!(mgr.list_pending().len(), 1);
    }

    #[test]
    fn test_approval_manager_cleanup_does_not_affect_non_pending() {
        let mgr = ApprovalManager::new(0);
        let id = mgr.request_approval("op", "agent");
        mgr.approve(&id).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let expired = mgr.cleanup_expired();
        assert_eq!(expired, 0); // Already approved, not expired
    }

    #[test]
    fn test_multi_process_set_get_config() {
        let mgr = MultiProcessApprovalManager::new(30);
        let custom_config = ApprovalConfig {
            enabled: false,
            timeout: Duration::from_secs(60),
            min_risk_level: "HIGH".to_string(),
            dialog_width: 800,
            dialog_height: 600,
            enable_sound: false,
            enable_animation: false,
        };
        mgr.set_config(custom_config.clone());
        let retrieved = mgr.get_config();
        assert!(!retrieved.enabled);
        assert_eq!(retrieved.timeout, Duration::from_secs(60));
        assert_eq!(retrieved.min_risk_level, "HIGH");
        assert_eq!(retrieved.dialog_width, 800);
    }

    #[test]
    fn test_multi_process_default_timeout() {
        let mgr = MultiProcessApprovalManager::with_default_timeout();
        mgr.start().unwrap();
        assert!(mgr.is_running());
        mgr.stop().unwrap();
    }

    #[tokio::test]
    async fn test_multi_process_safe_ops_various() {
        let mgr = MultiProcessApprovalManager::new(30);
        mgr.start().unwrap();

        for (op, expected) in [
            ("file_read", true),
            ("dir_list", true),
            ("network_request", true),
            ("hardware_i2c", true),
            ("registry_read", true),
        ] {
            let req = MultiApprovalRequest {
                request_id: format!("test-{}", op),
                operation: op.to_string(),
                target: "/tmp/test".to_string(),
                risk_level: "LOW".to_string(),
                reason: "test".to_string(),
                context: HashMap::new(),
                timeout_seconds: 5,
                timestamp: chrono::Utc::now().timestamp(),
            };
            let resp = mgr.request_approval(&req).await.unwrap();
            assert_eq!(resp.approved, expected, "op {} should be approved={}", op, expected);
        }
    }

    #[tokio::test]
    async fn test_multi_process_dangerous_ops_various() {
        let mgr = MultiProcessApprovalManager::new(30);
        mgr.start().unwrap();

        for op in ["process_exec", "file_delete", "registry_write", "system_shutdown"] {
            let req = MultiApprovalRequest {
                request_id: format!("test-{}", op),
                operation: op.to_string(),
                target: "dangerous_target".to_string(),
                risk_level: "CRITICAL".to_string(),
                reason: "test".to_string(),
                context: HashMap::new(),
                timeout_seconds: 5,
                timestamp: chrono::Utc::now().timestamp(),
            };
            let resp = mgr.request_approval(&req).await.unwrap();
            assert!(!resp.approved, "op {} should be denied", op);
            assert!(!resp.timed_out);
        }
    }

    #[test]
    fn test_multi_approval_response_fields() {
        let resp = MultiApprovalResponse {
            request_id: "test-1".to_string(),
            approved: true,
            timed_out: false,
            duration_seconds: 1.5,
            response_time: 1700000000,
        };
        assert_eq!(resp.request_id, "test-1");
        assert!(resp.approved);
        assert!(!resp.timed_out);
        assert_eq!(resp.duration_seconds, 1.5);
    }

    #[test]
    fn test_multi_approval_request_context() {
        let mut ctx = HashMap::new();
        ctx.insert("source_ip".to_string(), "192.168.1.1".to_string());
        ctx.insert("session_id".to_string(), "abc123".to_string());
        let req = MultiApprovalRequest {
            request_id: "ctx-test".to_string(),
            operation: "file_write".to_string(),
            target: "/tmp/test".to_string(),
            risk_level: "HIGH".to_string(),
            reason: "test".to_string(),
            context: ctx,
            timeout_seconds: 30,
            timestamp: 0,
        };
        assert_eq!(req.context.len(), 2);
        assert_eq!(req.context.get("source_ip"), Some(&"192.168.1.1".to_string()));
    }

    // ============================================================
    // Additional tests for 95%+ coverage
    // ============================================================

    #[tokio::test]
    async fn test_multi_process_with_child_factory_popup_not_supported() {
        struct FailFactory;
        impl ChildProcessFactory for FailFactory {
            fn spawn_child(
                &self,
                _window_type: &str,
                _data: HashMap<String, serde_json::Value>,
            ) -> Result<(String, oneshot::Receiver<serde_json::Value>), String> {
                Err("popup not supported on this platform".to_string())
            }
        }

        let mgr = MultiProcessApprovalManager::new(30);
        mgr.set_child_factory(Arc::new(FailFactory));
        mgr.start().unwrap();

        let req = MultiApprovalRequest {
            request_id: "popup-fail".to_string(),
            operation: "file_write".to_string(),
            target: "/tmp/test".to_string(),
            risk_level: "HIGH".to_string(),
            reason: "test".to_string(),
            context: HashMap::new(),
            timeout_seconds: 5,
            timestamp: chrono::Utc::now().timestamp(),
        };
        let resp = mgr.request_approval(&req).await.unwrap();
        assert!(!resp.approved);
        assert!(!resp.timed_out);
    }

    #[tokio::test]
    async fn test_multi_process_with_child_factory_spawn_error() {
        struct ErrorFactory;
        impl ChildProcessFactory for ErrorFactory {
            fn spawn_child(
                &self,
                _window_type: &str,
                _data: HashMap<String, serde_json::Value>,
            ) -> Result<(String, oneshot::Receiver<serde_json::Value>), String> {
                Err("generic spawn error".to_string())
            }
        }

        let mgr = MultiProcessApprovalManager::new(30);
        mgr.set_child_factory(Arc::new(ErrorFactory));
        mgr.start().unwrap();

        let req = MultiApprovalRequest {
            request_id: "spawn-err".to_string(),
            operation: "file_write".to_string(),
            target: "/tmp/test".to_string(),
            risk_level: "HIGH".to_string(),
            reason: "test".to_string(),
            context: HashMap::new(),
            timeout_seconds: 5,
            timestamp: chrono::Utc::now().timestamp(),
        };
        let result = mgr.request_approval(&req).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("failed to create approval window"));
    }

    #[tokio::test]
    async fn test_multi_process_with_child_factory_approved() {
        struct ApproveFactory;
        impl ChildProcessFactory for ApproveFactory {
            fn spawn_child(
                &self,
                _window_type: &str,
                _data: HashMap<String, serde_json::Value>,
            ) -> Result<(String, oneshot::Receiver<serde_json::Value>), String> {
                let (tx, rx) = oneshot::channel();
                tx.send(serde_json::json!({"approved": true})).unwrap();
                Ok(("child-1".to_string(), rx))
            }
        }

        let mgr = MultiProcessApprovalManager::new(30);
        mgr.set_child_factory(Arc::new(ApproveFactory));
        mgr.start().unwrap();

        let req = MultiApprovalRequest {
            request_id: "factory-approve".to_string(),
            operation: "file_write".to_string(),
            target: "/tmp/test".to_string(),
            risk_level: "HIGH".to_string(),
            reason: "test".to_string(),
            context: HashMap::new(),
            timeout_seconds: 5,
            timestamp: chrono::Utc::now().timestamp(),
        };
        let resp = mgr.request_approval(&req).await.unwrap();
        assert!(resp.approved);
        assert!(!resp.timed_out);
        assert!(resp.duration_seconds >= 0.0);
    }

    #[tokio::test]
    async fn test_multi_process_with_child_factory_denied() {
        struct DenyFactory;
        impl ChildProcessFactory for DenyFactory {
            fn spawn_child(
                &self,
                _window_type: &str,
                _data: HashMap<String, serde_json::Value>,
            ) -> Result<(String, oneshot::Receiver<serde_json::Value>), String> {
                let (tx, rx) = oneshot::channel();
                tx.send(serde_json::json!({"approved": false})).unwrap();
                Ok(("child-1".to_string(), rx))
            }
        }

        let mgr = MultiProcessApprovalManager::new(30);
        mgr.set_child_factory(Arc::new(DenyFactory));
        mgr.start().unwrap();

        let req = MultiApprovalRequest {
            request_id: "factory-deny".to_string(),
            operation: "file_delete".to_string(),
            target: "/tmp/test".to_string(),
            risk_level: "HIGH".to_string(),
            reason: "test".to_string(),
            context: HashMap::new(),
            timeout_seconds: 5,
            timestamp: chrono::Utc::now().timestamp(),
        };
        let resp = mgr.request_approval(&req).await.unwrap();
        assert!(!resp.approved);
        assert!(!resp.timed_out);
    }

    #[tokio::test]
    async fn test_multi_process_with_child_factory_channel_dropped() {
        struct DropFactory;
        impl ChildProcessFactory for DropFactory {
            fn spawn_child(
                &self,
                _window_type: &str,
                _data: HashMap<String, serde_json::Value>,
            ) -> Result<(String, oneshot::Receiver<serde_json::Value>), String> {
                let (_tx, rx) = oneshot::channel();
                // Drop the sender so the receiver returns Err
                Ok(("child-1".to_string(), rx))
            }
        }

        let mgr = MultiProcessApprovalManager::new(30);
        mgr.set_child_factory(Arc::new(DropFactory));
        mgr.start().unwrap();

        let req = MultiApprovalRequest {
            request_id: "channel-drop".to_string(),
            operation: "file_write".to_string(),
            target: "/tmp/test".to_string(),
            risk_level: "HIGH".to_string(),
            reason: "test".to_string(),
            context: HashMap::new(),
            timeout_seconds: 5,
            timestamp: chrono::Utc::now().timestamp(),
        };
        let result = mgr.request_approval(&req).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty result"));
    }

    #[tokio::test]
    async fn test_multi_process_with_child_factory_timeout() {
        // We need the sender to stay alive (not dropped) but never send.
        // Use a shared sender that we keep alive.
        let sender = Arc::new(Mutex::new(None));
        let sender_clone = sender.clone();

        struct TimeoutFactory {
            sender: Arc<Mutex<Option<oneshot::Sender<serde_json::Value>>>>,
        }
        impl ChildProcessFactory for TimeoutFactory {
            fn spawn_child(
                &self,
                _window_type: &str,
                _data: HashMap<String, serde_json::Value>,
            ) -> Result<(String, oneshot::Receiver<serde_json::Value>), String> {
                let (tx, rx) = oneshot::channel();
                *self.sender.lock() = Some(tx);
                Ok(("child-1".to_string(), rx))
            }
        }

        let mgr = MultiProcessApprovalManager::new(1);
        mgr.set_child_factory(Arc::new(TimeoutFactory { sender: sender_clone }));
        mgr.start().unwrap();

        let req = MultiApprovalRequest {
            request_id: "timeout-test".to_string(),
            operation: "file_write".to_string(),
            target: "/tmp/test".to_string(),
            risk_level: "HIGH".to_string(),
            reason: "test".to_string(),
            context: HashMap::new(),
            timeout_seconds: 1,
            timestamp: chrono::Utc::now().timestamp(),
        };
        let resp = mgr.request_approval(&req).await.unwrap();
        assert!(!resp.approved);
        assert!(resp.timed_out);
        // Keep sender alive through the test
        let _ = sender;
    }

    #[tokio::test]
    async fn test_multi_process_with_child_factory_data_field_approved() {
        // Test the data.approved fallback path
        struct DataFieldFactory;
        impl ChildProcessFactory for DataFieldFactory {
            fn spawn_child(
                &self,
                _window_type: &str,
                _data: HashMap<String, serde_json::Value>,
            ) -> Result<(String, oneshot::Receiver<serde_json::Value>), String> {
                let (tx, rx) = oneshot::channel();
                // Use data.approved format instead of top-level approved
                tx.send(serde_json::json!({"data": {"approved": true}})).unwrap();
                Ok(("child-1".to_string(), rx))
            }
        }

        let mgr = MultiProcessApprovalManager::new(30);
        mgr.set_child_factory(Arc::new(DataFieldFactory));
        mgr.start().unwrap();

        let req = MultiApprovalRequest {
            request_id: "data-field".to_string(),
            operation: "file_write".to_string(),
            target: "/tmp/test".to_string(),
            risk_level: "HIGH".to_string(),
            reason: "test".to_string(),
            context: HashMap::new(),
            timeout_seconds: 5,
            timestamp: chrono::Utc::now().timestamp(),
        };
        let resp = mgr.request_approval(&req).await.unwrap();
        assert!(resp.approved);
    }

    #[tokio::test]
    async fn test_multi_process_with_child_factory_no_approved_field() {
        // When the result has no approved field, default to false
        struct NoApprovedFieldFactory;
        impl ChildProcessFactory for NoApprovedFieldFactory {
            fn spawn_child(
                &self,
                _window_type: &str,
                _data: HashMap<String, serde_json::Value>,
            ) -> Result<(String, oneshot::Receiver<serde_json::Value>), String> {
                let (tx, rx) = oneshot::channel();
                tx.send(serde_json::json!({"status": "ok"})).unwrap();
                Ok(("child-1".to_string(), rx))
            }
        }

        let mgr = MultiProcessApprovalManager::new(30);
        mgr.set_child_factory(Arc::new(NoApprovedFieldFactory));
        mgr.start().unwrap();

        let req = MultiApprovalRequest {
            request_id: "no-approved".to_string(),
            operation: "file_write".to_string(),
            target: "/tmp/test".to_string(),
            risk_level: "HIGH".to_string(),
            reason: "test".to_string(),
            context: HashMap::new(),
            timeout_seconds: 5,
            timestamp: chrono::Utc::now().timestamp(),
        };
        let resp = mgr.request_approval(&req).await.unwrap();
        // No approved field means default false
        assert!(!resp.approved);
    }

    #[test]
    fn test_approval_request_debug() {
        let req = ApprovalRequest {
            id: "req-123".to_string(),
            operation: "file_write".to_string(),
            requester: "agent-1".to_string(),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            status: ApprovalStatus::Pending,
            deny_reason: None,
        };
        let debug = format!("{:?}", req);
        assert!(debug.contains("req-123"));
        assert!(debug.contains("file_write"));
    }

    #[test]
    fn test_multi_approval_request_debug() {
        let req = MultiApprovalRequest {
            request_id: "multi-123".to_string(),
            operation: "file_delete".to_string(),
            target: "/tmp/test".to_string(),
            risk_level: "CRITICAL".to_string(),
            reason: "dangerous".to_string(),
            context: HashMap::new(),
            timeout_seconds: 30,
            timestamp: 1700000000,
        };
        let debug = format!("{:?}", req);
        assert!(debug.contains("multi-123"));
        assert!(debug.contains("CRITICAL"));
    }

    #[test]
    fn test_multi_approval_response_debug() {
        let resp = MultiApprovalResponse {
            request_id: "resp-1".to_string(),
            approved: true,
            timed_out: false,
            duration_seconds: 2.5,
            response_time: 1700000000,
        };
        let debug = format!("{:?}", resp);
        assert!(debug.contains("resp-1"));
    }

    #[test]
    fn test_approval_config_clone() {
        let config = ApprovalConfig::default();
        let cloned = config.clone();
        assert_eq!(cloned.enabled, config.enabled);
        assert_eq!(cloned.timeout, config.timeout);
        assert_eq!(cloned.min_risk_level, config.min_risk_level);
    }

    #[tokio::test]
    async fn test_multi_process_stop_when_not_started() {
        let mgr = MultiProcessApprovalManager::new(30);
        // Stop when not started should succeed
        mgr.stop().unwrap();
        assert!(!mgr.is_running());
    }

    #[tokio::test]
    async fn test_multi_process_start_idempotent() {
        let mgr = MultiProcessApprovalManager::new(30);
        mgr.start().unwrap();
        assert!(mgr.is_running());
        mgr.start().unwrap(); // Start again
        assert!(mgr.is_running());
        mgr.stop().unwrap();
    }

    #[test]
    fn test_approval_manager_get_nonexistent() {
        let mgr = ApprovalManager::with_default_timeout();
        assert!(mgr.get("nonexistent").is_none());
    }

    #[test]
    fn test_approval_manager_approve_after_deny_fails() {
        let mgr = ApprovalManager::with_default_timeout();
        let id = mgr.request_approval("op", "agent");
        mgr.deny(&id, "nope").unwrap();
        // Trying to approve a denied request should fail
        assert!(mgr.approve(&id).is_err());
    }

    #[test]
    fn test_approval_manager_deny_after_approve_fails() {
        let mgr = ApprovalManager::with_default_timeout();
        let id = mgr.request_approval("op", "agent");
        mgr.approve(&id).unwrap();
        // Trying to deny an approved request should fail
        assert!(mgr.deny(&id, "reason").is_err());
    }

    #[tokio::test]
    async fn test_multi_process_uses_request_timeout_when_set() {
        // Keep sender alive to trigger timeout rather than "empty result"
        let sender = Arc::new(Mutex::new(None));
        let sender_clone = sender.clone();

        struct TimeoutFactory {
            sender: Arc<Mutex<Option<oneshot::Sender<serde_json::Value>>>>,
        }
        impl ChildProcessFactory for TimeoutFactory {
            fn spawn_child(
                &self,
                _window_type: &str,
                _data: HashMap<String, serde_json::Value>,
            ) -> Result<(String, oneshot::Receiver<serde_json::Value>), String> {
                let (tx, rx) = oneshot::channel();
                *self.sender.lock() = Some(tx);
                Ok(("child-1".to_string(), rx))
            }
        }

        let mgr = MultiProcessApprovalManager::new(600); // 10 min default timeout
        mgr.set_child_factory(Arc::new(TimeoutFactory { sender: sender_clone }));
        mgr.start().unwrap();

        let req = MultiApprovalRequest {
            request_id: "custom-timeout".to_string(),
            operation: "file_write".to_string(),
            target: "/tmp/test".to_string(),
            risk_level: "HIGH".to_string(),
            reason: "test".to_string(),
            context: HashMap::new(),
            timeout_seconds: 1, // Use 1s timeout from request
            timestamp: chrono::Utc::now().timestamp(),
        };
        let start = std::time::Instant::now();
        let resp = mgr.request_approval(&req).await.unwrap();
        let elapsed = start.elapsed();
        assert!(resp.timed_out);
        assert!(elapsed.as_secs() < 5, "Should timeout in ~1s, not 600s (elapsed: {:?}", elapsed);
        let _ = sender;
    }
}
