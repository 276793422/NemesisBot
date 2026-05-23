//! ABAC Security Auditor - Attribute-Based Access Control
//!
//! Implements the core security evaluation engine that:
//! - Evaluates operation requests against configured rules
//! - Manages approval workflows (approve/deny pending requests)
//! - Validates paths for workspace isolation
//! - Checks commands for dangerous patterns
//! - Tracks statistics and exports audit logs

use crate::matcher;
use crate::types::*;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Default deny patterns
// ---------------------------------------------------------------------------

/// Default deny patterns for dangerous operations, keyed by operation type.
///
/// Equivalent to Go's `DefaultDenyPatterns`.
pub static DEFAULT_DENY_PATTERNS: std::sync::LazyLock<HashMap<OperationType, Vec<&'static str>>> =
    std::sync::LazyLock::new(|| {
        let mut m = HashMap::new();
        m.insert(
            OperationType::ProcessExec,
            vec![
                r"\brm\s+-[rf]{1,2}\b",
                r"\bdel\s+/[fq]\b",
                r"\b(format|mkfs|diskpart)\b",
                r"\bdd\s+if=",
                r"\b(shutdown|reboot|poweroff)\b",
                r"\bsudo\b",
                r"\bchmod\s+[0-7]{3,4}\b",
                r"\bchown\b",
                r"\bpkill\b",
                r"\bkillall\b",
                r"\bkill\s+-[9]\b",
                r"\bcurl\b.*\|\s*(sh|bash)",
                r"\bwget\b.*\|\s*(sh|bash)",
                r"\beval\b",
                r"\bsource\s+.*\.sh\b",
            ],
        );
        m.insert(
            OperationType::FileWrite,
            vec![
                r"\.\.[/\\]",
                r"^/etc/",
                r"^/sys/",
                r"^/proc/",
                r"^/dev/",
                r"C:\\Windows\\System32",
                r"C:\\Windows\\System32\\drivers\\etc\\hosts",
            ],
        );
        m.insert(
            OperationType::NetworkDownload,
            vec![
                r"file://",
                r"ftp://",
            ],
        );
        m
    });

// ---------------------------------------------------------------------------
// ApprovalRequiredError
// ---------------------------------------------------------------------------

/// Error returned when an operation requires approval but no interactive
/// approval manager is available.
///
/// Equivalent to Go's `ApprovalRequiredError`.
#[derive(Debug, Clone)]
pub struct ApprovalRequiredError {
    /// The request ID that needs approval.
    pub request_id: String,
    /// Human-readable reason why approval is needed.
    pub reason: String,
}

impl std::fmt::Display for ApprovalRequiredError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "approval required: {} (request ID: {})",
            self.reason, self.request_id
        )
    }
}

impl std::error::Error for ApprovalRequiredError {}

impl ApprovalRequiredError {
    /// Always returns `true` — this type only exists for approval-required cases.
    pub fn is_approval_required(&self) -> bool {
        true
    }
}

/// Trait for approval manager integration.
///
/// Mirrors Go's `approval.ApprovalManager` interface. The auditor calls into
/// this when a `require_approval` decision is reached.
pub trait ApprovalManager: Send + Sync {
    /// Whether the approval manager is currently running and able to show dialogs.
    fn is_running(&self) -> bool;

    /// Request interactive approval. Returns `true` if approved, `false` if denied.
    fn request_approval_sync(
        &self,
        request_id: &str,
        operation: &str,
        target: &str,
        risk_level: &str,
        reason: &str,
        timeout_secs: u64,
    ) -> Result<bool, String>;
}

/// Security auditor configuration.
#[derive(Debug, Clone)]
pub struct AuditorConfig {
    pub enabled: bool,
    pub log_all_operations: bool,
    pub log_denials_only: bool,
    pub approval_timeout_secs: u64,
    pub max_pending_requests: usize,
    pub audit_log_retention_days: u32,
    pub audit_log_file_enabled: bool,
    pub audit_log_dir: Option<String>,
    pub default_action: String,
}

impl Default for AuditorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            log_all_operations: true,
            log_denials_only: false,
            approval_timeout_secs: 300,
            max_pending_requests: 100,
            audit_log_retention_days: 90,
            audit_log_file_enabled: false,
            audit_log_dir: None,
            default_action: "deny".to_string(),
        }
    }
}

/// Operation request for security evaluation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OperationRequest {
    pub id: String,
    pub op_type: OperationType,
    pub danger_level: DangerLevel,
    pub user: String,
    pub source: String,
    pub target: String,
    pub timestamp: Option<chrono::DateTime<chrono::Utc>>,
    /// Who approved (if applicable).
    pub approver: Option<String>,
    /// When approved.
    pub approved_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Reason for denial (if denied).
    pub denied_reason: Option<String>,
}

impl Default for OperationRequest {
    fn default() -> Self {
        Self {
            id: String::new(),
            op_type: OperationType::FileRead,
            danger_level: DangerLevel::Low,
            user: String::new(),
            source: String::new(),
            target: String::new(),
            timestamp: None,
            approver: None,
            approved_at: None,
            denied_reason: None,
        }
    }
}

/// Security auditor - ABAC engine.
pub struct SecurityAuditor {
    rules: RwLock<HashMap<OperationType, Vec<SecurityRule>>>,
    default_action: RwLock<String>,
    active_requests: RwLock<HashMap<String, OperationRequest>>,
    config: AuditorConfig,
    enabled: RwLock<bool>,
    total_events: AtomicI64,
    allowed_count: AtomicI64,
    denied_count: AtomicI64,
    approved_count: AtomicI64,
    pending_count: AtomicI64,
    /// Optional approval manager for interactive approval dialogs.
    approval_manager: RwLock<Option<Arc<dyn ApprovalManager>>>,
    /// Optional explicit log file path for audit events (date-based).
    /// When set, audit events are appended to this file in JSON format.
    log_file_path: RwLock<Option<PathBuf>>,
}

impl SecurityAuditor {
    pub fn new(config: AuditorConfig) -> Self {
        Self {
            rules: RwLock::new(HashMap::new()),
            default_action: RwLock::new(config.default_action.clone()),
            active_requests: RwLock::new(HashMap::new()),
            enabled: RwLock::new(config.enabled),
            config,
            total_events: AtomicI64::new(0),
            allowed_count: AtomicI64::new(0),
            denied_count: AtomicI64::new(0),
            approved_count: AtomicI64::new(0),
            pending_count: AtomicI64::new(0),
            approval_manager: RwLock::new(None),
            log_file_path: RwLock::new(None),
        }
    }

    /// Set the audit log file path for date-based log file output.
    ///
    /// When configured, `log_audit_event()` will append events as JSON lines
    /// to the specified file path. This mirrors Go's behavior of writing audit
    /// events directly to a date-based log file.
    pub fn set_log_file(&self, path: &str) {
        *self.log_file_path.write() = Some(PathBuf::from(path));
    }

    /// Get the current audit log file path, if configured.
    pub fn get_log_file_path(&self) -> Option<PathBuf> {
        self.log_file_path.read().clone()
    }

    /// Set the approval manager for interactive approval dialogs.
    ///
    /// Equivalent to Go's `SecurityAuditor.SetApprovalManager()`.
    pub fn set_approval_manager(&self, mgr: Arc<dyn ApprovalManager>) {
        *self.approval_manager.write() = Some(mgr);
    }

    /// Get a reference to the current approval manager, if any.
    ///
    /// Equivalent to Go's `SecurityAuditor.GetApprovalManager()`.
    pub fn get_approval_manager(&self) -> Option<Arc<dyn ApprovalManager>> {
        self.approval_manager.read().clone()
    }

    /// Cleanup old audit logs.
    ///
    /// Equivalent to Go's `SecurityAuditor.CleanupOldAuditLogs()`.
    /// Events are persisted to file; this method is a no-op but provided
    /// for API parity.
    pub fn cleanup_old_audit_logs(&self) -> Result<(), String> {
        // No-op: events are persisted to the audit log file.
        // File-based retention can be handled externally by rotating the log file.
        Ok(())
    }

    /// Set rules for an operation type.
    pub fn set_rules(&self, op_type: OperationType, rules: Vec<SecurityRule>) {
        let mut r = self.rules.write();
        r.insert(op_type, rules);
    }

    /// Set the default action for unmatched requests.
    pub fn set_default_action(&self, action: &str) {
        *self.default_action.write() = action.to_string();
    }

    /// Check if enabled.
    pub fn is_enabled(&self) -> bool {
        *self.enabled.read()
    }

    /// Enable the auditor.
    pub fn enable(&self) {
        *self.enabled.write() = true;
        tracing::info!("[Security] Security auditor enabled");
    }

    /// Disable the auditor.
    pub fn disable(&self) {
        *self.enabled.write() = false;
        tracing::warn!("[Security] Security auditor DISABLED - all operations will be allowed!");
    }

    /// Request permission for an operation.
    /// Returns (allowed, error_message, request_id).
    pub fn request_permission(&self, req: &OperationRequest) -> (bool, Option<String>, String) {
        if !self.is_enabled() {
            return (true, None, req.id.clone());
        }

        self.total_events.fetch_add(1, Ordering::SeqCst);

        let (decision, reason, policy) = self.evaluate_request(req);

        let decision_str = match decision {
            SecurityDecision::Allowed => "allowed",
            SecurityDecision::Denied => "denied",
            SecurityDecision::RequireApproval => "pending",
        };

        // Log the audit event to persistent storage
        let event = AuditEvent {
            event_id: uuid::Uuid::new_v4().to_string(),
            request: req.clone(),
            decision: decision_str.to_string(),
            reason: reason.clone(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            policy_rule: policy.clone(),
        };
        self.log_audit_event(&event);

        match decision {
            SecurityDecision::Allowed => {
                self.allowed_count.fetch_add(1, Ordering::SeqCst);
                (true, None, req.id.clone())
            }
            SecurityDecision::Denied => {
                self.denied_count.fetch_add(1, Ordering::SeqCst);
                (
                    false,
                    Some(format!(
                        "Security policy denied {} on '{}' ({})",
                        req.op_type, req.target, reason
                    )),
                    req.id.clone(),
                )
            }
            SecurityDecision::RequireApproval => {
                self.pending_count.fetch_add(1, Ordering::SeqCst);

                // Try to use interactive approval manager if available (mirrors Go behavior)
                let mgr_opt = self.approval_manager.read().clone();
                if let Some(mgr) = mgr_opt {
                    if mgr.is_running() {
                        // Call the approval manager synchronously
                        match mgr.request_approval_sync(
                            &req.id,
                            &req.op_type.to_string(),
                            &req.target,
                            &req.danger_level.to_string(),
                            &reason,
                            self.config.approval_timeout_secs,
                        ) {
                            Ok(true) => {
                                // User approved the operation
                                self.pending_count.fetch_sub(1, Ordering::SeqCst);
                                self.approved_count.fetch_add(1, Ordering::SeqCst);
                                return (true, None, req.id.clone());
                            }
                            Ok(false) => {
                                // User explicitly denied or timed out
                                self.pending_count.fetch_sub(1, Ordering::SeqCst);
                                self.denied_count.fetch_add(1, Ordering::SeqCst);
                                return (
                                    false,
                                    Some(format!(
                                        "User rejected {} on '{}' ({})",
                                        req.op_type, req.target, reason
                                    )),
                                    req.id.clone(),
                                );
                            }
                            Err(_) => {
                                // Dialog failed, fall through to pending request storage
                            }
                        }
                    }
                }

                // No approval manager available or dialog failed, store as pending request
                let mut active = self.active_requests.write();
                active.insert(req.id.clone(), req.clone());
                (
                    false,
                    Some(format!(
                        "approval required: {} (request ID: {})",
                        reason, req.id
                    )),
                    req.id.clone(),
                )
            }
        }
    }

    /// Approve a pending operation request.
    ///
    /// Removes the request from the pending list, increments the approved counter,
    /// and records the approver information.
    pub fn approve_request(&self, request_id: &str, approver: &str) -> Result<(), String> {
        let mut active = self.active_requests.write();
        match active.get_mut(request_id) {
            Some(req) => {
                req.approver = Some(approver.to_string());
                req.approved_at = Some(chrono::Utc::now());
                let _reason = format!("Approved by {}", approver);
                tracing::info!(
                    request_id = request_id,
                    approver = approver,
                    operation = %req.op_type,
                    target = %req.target,
                    "[Security] Operation approved"
                );
                active.remove(request_id);
                self.pending_count.fetch_sub(1, Ordering::SeqCst);
                self.approved_count.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
            None => Err(format!("request not found: {}", request_id)),
        }
    }

    /// Deny a pending operation request.
    ///
    /// Removes the request from the pending list, increments the denied counter,
    /// and records the reason.
    pub fn deny_request(
        &self,
        request_id: &str,
        approver: &str,
        reason: &str,
    ) -> Result<(), String> {
        let mut active = self.active_requests.write();
        match active.get_mut(request_id) {
            Some(req) => {
                req.denied_reason = Some(reason.to_string());
                let _deny_reason = format!("Denied by {}: {}", approver, reason);
                tracing::info!(
                    request_id = request_id,
                    approver = approver,
                    reason = reason,
                    operation = %req.op_type,
                    "[Security] Operation denied"
                );
                active.remove(request_id);
                self.pending_count.fetch_sub(1, Ordering::SeqCst);
                self.denied_count.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
            None => Err(format!("request not found: {}", request_id)),
        }
    }

    /// Get pending request count.
    pub fn pending_count(&self) -> usize {
        self.active_requests.read().len()
    }

    /// Get all pending approval requests.
    pub fn get_pending_requests(&self) -> Vec<OperationRequest> {
        self.active_requests
            .read()
            .values()
            .cloned()
            .collect()
    }

    /// Get statistics as a HashMap of string keys to i64 values.
    pub fn statistics(&self) -> HashMap<String, i64> {
        let mut stats = HashMap::new();
        stats.insert("total_events".to_string(), self.total_events.load(Ordering::SeqCst));
        stats.insert("allowed".to_string(), self.allowed_count.load(Ordering::SeqCst));
        stats.insert("denied".to_string(), self.denied_count.load(Ordering::SeqCst));
        stats.insert("approved".to_string(), self.approved_count.load(Ordering::SeqCst));
        stats.insert("pending".to_string(), self.pending_count.load(Ordering::SeqCst));
        stats
    }

    /// Get full statistics as a HashMap of string keys to various types.
    ///
    /// Equivalent to Go's `GetStatistics()`. Returns richer data including
    /// active request count, enabled status, and rule type count.
    pub fn get_statistics(&self) -> HashMap<String, serde_json::Value> {
        let mut stats = HashMap::new();
        stats.insert("total_events".to_string(), serde_json::json!(self.total_events.load(Ordering::SeqCst)));
        stats.insert("allowed".to_string(), serde_json::json!(self.allowed_count.load(Ordering::SeqCst)));
        stats.insert("denied".to_string(), serde_json::json!(self.denied_count.load(Ordering::SeqCst)));
        stats.insert("approved".to_string(), serde_json::json!(self.approved_count.load(Ordering::SeqCst)));
        stats.insert("pending".to_string(), serde_json::json!(self.pending_count.load(Ordering::SeqCst)));
        stats.insert("active_requests".to_string(), serde_json::json!(self.active_requests.read().len()));
        stats.insert("enabled".to_string(), serde_json::json!(self.is_enabled()));
        stats.insert("rule_types".to_string(), serde_json::json!(self.rules.read().len()));
        stats
    }

    /// Export the audit log to a file.
    ///
    /// In the Go implementation, this copies the persistent audit log file to
    /// the specified path. Here, we create a summary export with the statistics.
    pub fn export_audit_log(&self, file_path: &str) -> Result<(), String> {
        let path = Path::new(file_path);

        // Create parent directory if needed
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create directory: {}", e))?;
        }

        // Write statistics as JSON
        let stats = self.get_statistics();
        let content = serde_json::to_string_pretty(&stats)
            .unwrap_or_else(|_| "{}".to_string());

        std::fs::write(path, content)
            .map_err(|e| format!("failed to write audit log export: {}", e))?;

        Ok(())
    }

    /// Validate that a path is within the workspace and safe.
    ///
    /// Equivalent to Go's `ValidatePath()`. Checks:
    /// 1. Path resolves to an absolute path
    /// 2. Path is within workspace (if workspace is specified)
    /// 3. Path does not access dangerous system paths
    pub fn validate_path(
        path: &str,
        workspace: &str,
        _operation: OperationType,
    ) -> Result<String, String> {
        validate_path_internal(path, workspace)
    }

    /// Check if a command is safe to execute.
    ///
    /// Equivalent to Go's `IsSafeCommand()`. Checks against a set of
    /// dangerous command patterns.
    pub fn is_safe_command(command: &str) -> (bool, String) {
        is_safe_command_internal(command)
    }

    /// Close the auditor and release resources.
    pub fn close(&self) -> Result<(), String> {
        // Clear active requests
        self.active_requests.write().clear();
        Ok(())
    }

    /// Query the audit log with a filter.
    ///
    /// Equivalent to Go's `SecurityAuditor.GetAuditLog()`.
    /// Events are persisted to file rather than held in memory, so this
    /// delegates to the free function `get_audit_log`.
    pub fn get_audit_log(&self, filter: AuditFilter) -> Vec<AuditEvent> {
        get_audit_log(self, &filter)
    }

    /// Get a reference to the auditor configuration.
    pub fn config(&self) -> &AuditorConfig {
        &self.config
    }

    /// Append an audit event to the persistent JSONL log file.
    ///
    /// If `audit_log_file_enabled` is true and `audit_log_dir` is set,
    /// the event is serialized as JSON and appended as a new line to
    /// `{audit_log_dir}/audit.jsonl`.
    ///
    /// Additionally, if `log_file_path` is configured via `set_log_file()`,
    /// the event is appended to that file as well (mirrors Go's date-based
    /// log file behavior).
    pub fn log_audit_event(&self, event: &AuditEvent) {
        // Write to the configured JSONL audit log directory
        if self.config.audit_log_file_enabled {
            if let Some(ref log_dir) = self.config.audit_log_dir {
                if !log_dir.is_empty() {
                    let log_path = Path::new(log_dir).join("audit.jsonl");

                    // Create directory if it doesn't exist
                    if let Some(parent) = log_path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }

                    match serde_json::to_string(event) {
                        Ok(line) => {
                            use std::io::Write;
                            // Open in append mode, create if not exists
                            match std::fs::OpenOptions::new()
                                .create(true)
                                .append(true)
                                .open(&log_path)
                            {
                                Ok(mut file) => {
                                    if let Err(e) = writeln!(file, "{}", line) {
                                        tracing::warn!(path = %log_path.display(), error = %e, "[Security] Failed to write audit event");
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!(path = %log_path.display(), error = %e, "[Security] Failed to open audit log for writing");
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "[Security] Failed to serialize audit event");
                        }
                    }
                }
            }
        }

        // Write to the explicit log_file_path if configured (mirrors Go's date-based log file)
        let log_path_opt = self.log_file_path.read().clone();
        if let Some(ref log_path) = log_path_opt {
            // Create parent directory if needed
            if let Some(parent) = log_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }

            match serde_json::to_string(event) {
                Ok(line) => {
                    use std::io::Write;
                    match std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(log_path)
                    {
                        Ok(mut file) => {
                            if let Err(e) = writeln!(file, "{}", line) {
                                tracing::warn!(path = %log_path.display(), error = %e, "[Security] Failed to write audit event to log file");
                            }
                        }
                        Err(e) => {
                            tracing::warn!(path = %log_path.display(), error = %e, "[Security] Failed to open log file for writing");
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "[Security] Failed to serialize audit event for log file");
                }
            }
        }
    }

    fn evaluate_request(&self, req: &OperationRequest) -> (SecurityDecision, String, String) {
        let rules = self.rules.read();
        let op_rules = match rules.get(&req.op_type) {
            Some(r) if !r.is_empty() => r,
            _ => {
                let action = self.default_action.read();
                return (
                    normalize_decision(&action),
                    "no rules configured, using default action".to_string(),
                    "default".to_string(),
                );
            }
        };

        for (i, rule) in op_rules.iter().enumerate() {
            let matched = match req.op_type {
                OperationType::FileRead
                | OperationType::FileWrite
                | OperationType::FileDelete
                | OperationType::DirRead
                | OperationType::DirCreate
                | OperationType::DirDelete
                | OperationType::RegistryRead
                | OperationType::RegistryWrite
                | OperationType::RegistryDelete => {
                    matcher::match_pattern(&rule.pattern, &req.target)
                }
                OperationType::ProcessExec
                | OperationType::ProcessSpawn
                | OperationType::ProcessKill
                | OperationType::ProcessSuspend => {
                    matcher::match_command_pattern(&rule.pattern, &req.target)
                }
                OperationType::NetworkDownload
                | OperationType::NetworkUpload
                | OperationType::NetworkRequest => {
                    matcher::match_domain_pattern(&rule.pattern, &req.target)
                }
                _ => {
                    rule.pattern == "*" || matcher::match_pattern(&rule.pattern, &req.target)
                }
            };

            if matched {
                let reason = format!("rule matched: pattern={}", rule.pattern);
                return (
                    normalize_decision(&rule.action),
                    reason,
                    format!("rule[{}]", i),
                );
            }
        }

        let action = self.default_action.read();
        (
            normalize_decision(&action),
            "no rules matched, using default action".to_string(),
            "default".to_string(),
        )
    }
}

// ---------------------------------------------------------------------------
// Internal helper functions
// ---------------------------------------------------------------------------

fn normalize_decision(action: &str) -> SecurityDecision {
    match action {
        "allow" | "allowed" => SecurityDecision::Allowed,
        "deny" | "denied" => SecurityDecision::Denied,
        "ask" | "require_approval" => SecurityDecision::RequireApproval,
        _ => SecurityDecision::Denied,
    }
}

/// Validate path is within workspace and safe.
fn validate_path_internal(path: &str, workspace: &str) -> Result<String, String> {
    let abs_path = Path::new(path)
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(path));

    if !workspace.is_empty() {
        let abs_workspace = Path::new(workspace)
            .canonicalize()
            .unwrap_or_else(|_| PathBuf::from(workspace));

        match abs_path.strip_prefix(&abs_workspace) {
            Ok(rel) => {
                if rel.starts_with("..") {
                    return Err("access denied: path outside workspace".to_string());
                }
            }
            Err(_) => {
                if !abs_path.starts_with(&abs_workspace) {
                    return Err("access denied: path outside workspace".to_string());
                }
            }
        }
    }

    // Check dangerous system paths
    let path_str = abs_path.to_string_lossy();
    let dangerous = [
        "/etc/passwd",
        "/etc/shadow",
        "/etc/sudoers",
        "C:\\Windows\\System32\\drivers\\etc\\hosts",
    ];
    for d in &dangerous {
        if path_str.starts_with(d) {
            return Err("access denied: protected system path".to_string());
        }
    }

    Ok(abs_path.to_string_lossy().to_string())
}

/// Check if a command is safe to execute.
fn is_safe_command_internal(command: &str) -> (bool, String) {
    use std::sync::OnceLock;
    static DANGEROUS: OnceLock<Vec<regex::Regex>> = OnceLock::new();
    let patterns = DANGEROUS.get_or_init(|| {
        let raw = [
            r"(?i)\brm\s+-[rf]{1,2}\b",
            r"(?i)\bdel\s+/[fq]\b",
            r"(?i)\b(format|mkfs)\b",
            r"(?i)\bdd\s+if=",
            r"(?i)\b(shutdown|reboot|poweroff)\b",
            r"(?i)\bsudo\b",
            r"(?i)\bchmod\s+[0-7]{3,4}\b",
            r"(?i)\bchown\b",
        ];
        raw.iter()
            .filter_map(|p| regex::Regex::new(p).ok())
            .collect()
    });

    for re in patterns {
        if re.is_match(command) {
            return (false, "command contains dangerous pattern".to_string());
        }
    }
    (true, String::new())
}

// ---------------------------------------------------------------------------
// Audit log types and querying
// ---------------------------------------------------------------------------

/// An audit log event recording a security decision.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AuditEvent {
    /// Unique event ID.
    pub event_id: String,
    /// The operation request that triggered this event.
    pub request: OperationRequest,
    /// Decision made: "allowed", "denied", "approved", "pending".
    pub decision: String,
    /// Human-readable reason for the decision.
    pub reason: String,
    /// When the decision was made (RFC 3339).
    pub timestamp: String,
    /// Which policy rule matched.
    pub policy_rule: String,
}

/// Filter for querying audit log events.
#[derive(Debug, Clone, Default)]
pub struct AuditFilter {
    /// Filter by operation type.
    pub operation_type: Option<OperationType>,
    /// Filter by user.
    pub user: Option<String>,
    /// Filter by source.
    pub source: Option<String>,
    /// Filter by decision ("allowed", "denied", "approved", "pending").
    pub decision: Option<String>,
    /// Filter events after this time (RFC 3339).
    pub start_time: Option<String>,
    /// Filter events before this time (RFC 3339).
    pub end_time: Option<String>,
}

impl AuditFilter {
    /// Check if the filter has no constraints.
    pub fn is_empty(&self) -> bool {
        self.operation_type.is_none()
            && self.user.is_none()
            && self.source.is_none()
            && self.decision.is_none()
            && self.start_time.is_none()
            && self.end_time.is_none()
    }

    /// Check if an event matches this filter.
    pub fn matches(&self, event: &AuditEvent) -> bool {
        if let Some(ref op_type) = self.operation_type {
            if event.request.op_type != *op_type {
                return false;
            }
        }
        if let Some(ref user) = self.user {
            if event.request.user != *user {
                return false;
            }
        }
        if let Some(ref source) = self.source {
            if event.request.source.is_empty() || !event.request.source.contains(source) {
                return false;
            }
        }
        if let Some(ref decision) = self.decision {
            if event.decision != *decision {
                return false;
            }
        }
        if let Some(ref start) = self.start_time {
            if event.timestamp.as_str() < start.as_str() {
                return false;
            }
        }
        if let Some(ref end) = self.end_time {
            if event.timestamp.as_str() > end.as_str() {
                return false;
            }
        }
        true
    }
}

// ---------------------------------------------------------------------------
// Global auditor singleton
// ---------------------------------------------------------------------------

/// Global auditor singleton.
///
/// Equivalent to Go's `globalAuditor` / `auditorOnce` pattern.
static GLOBAL_AUDITOR: std::sync::OnceLock<Arc<SecurityAuditor>> = std::sync::OnceLock::new();

/// Initialize the global security auditor.
///
/// Returns the global auditor. If already initialized, returns the existing
/// instance (ignoring the provided config).
///
/// Equivalent to Go's `InitGlobalAuditor()`.
pub fn init_global_auditor(config: AuditorConfig) -> Arc<SecurityAuditor> {
    GLOBAL_AUDITOR
        .get_or_init(|| Arc::new(SecurityAuditor::new(config)))
        .clone()
}

/// Get the global security auditor.
///
/// If the global auditor has not been initialized yet, initializes it with
/// default configuration.
///
/// Equivalent to Go's `GetGlobalAuditor()`.
pub fn get_global_auditor() -> Arc<SecurityAuditor> {
    GLOBAL_AUDITOR
        .get_or_init(|| Arc::new(SecurityAuditor::new(AuditorConfig::default())))
        .clone()
}

/// Reset the global auditor (for testing purposes only).
///
/// This is not exposed publicly; it is only available within the crate for
/// test cleanup.
#[cfg(test)]
fn _reset_global_auditor() {
    // parking_lot::OnceLock does not support reset. In tests we just
    // re-initialize a new auditor each time via init_global_auditor
    // (which returns the existing one if already set).
    // NOTE: std::sync::OnceLock also does not support reset.
    // Each test should create its own auditor instance for isolation.
}

// ---------------------------------------------------------------------------
// Security status monitoring
// ---------------------------------------------------------------------------

/// Monitor security status continuously, logging statistics at regular intervals.
///
/// This function runs until the `shutdown` future resolves. At each `interval`,
/// it logs the current security statistics and attempts to clean up old audit
/// log data.
///
/// Equivalent to Go's `MonitorSecurityStatus()`.
pub async fn monitor_security_status(
    auditor: Arc<SecurityAuditor>,
    interval_secs: u64,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(interval_secs));

    loop {
        tokio::select! {
            _ = interval.tick() => {
                let stats = auditor.get_statistics();
                tracing::info!(
                    total_events = %stats.get("total_events").unwrap_or(&serde_json::json!(0)),
                    allowed = %stats.get("allowed").unwrap_or(&serde_json::json!(0)),
                    denied = %stats.get("denied").unwrap_or(&serde_json::json!(0)),
                    pending = %stats.get("pending").unwrap_or(&serde_json::json!(0)),
                    enabled = %stats.get("enabled").unwrap_or(&serde_json::json!(false)),
                    "[Security] Security status monitor tick"
                );
            }
            _ = shutdown.changed() => {
                tracing::info!("[Security] Security status monitor shutting down");
                return;
            }
        }
    }
}

/// Query the audit log with optional filtering.
///
/// Since events are persisted to a file (if `audit_log_file_enabled` and
/// `audit_log_dir` are configured), this function reads the JSONL audit log
/// file and applies the provided filter. If no audit log file is configured,
/// returns an empty vector.
///
/// Equivalent to Go's `GetAuditLog()`.
pub fn get_audit_log(
    auditor: &SecurityAuditor,
    filter: &AuditFilter,
) -> Vec<AuditEvent> {
    let config = &auditor.config;
    if !config.audit_log_file_enabled {
        return Vec::new();
    }

    let log_dir = match &config.audit_log_dir {
        Some(d) if !d.is_empty() => d,
        _ => return Vec::new(),
    };

    let log_path = Path::new(log_dir).join("audit.jsonl");
    if !log_path.exists() {
        return Vec::new();
    }

    let content = match std::fs::read_to_string(&log_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(path = %log_path.display(), error = %e, "[Security] Failed to read audit log");
            return Vec::new();
        }
    };

    let mut events = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match serde_json::from_str::<AuditEvent>(line) {
            Ok(event) => {
                if filter.is_empty() || filter.matches(&event) {
                    events.push(event);
                }
            }
            Err(e) => {
                tracing::trace!(line = line, error = %e, "[Security] Skipping malformed audit log line");
            }
        }
    }

    events
}

#[cfg(test)]
mod tests;
