//! Security middleware for file/process/network/hardware operations.
//! Provides wrappers with permission presets for each operation type.
//!
//! Implements:
//! - SecureFileWrapper, SecureProcessWrapper, SecureNetworkWrapper, SecureHardwareWrapper
//! - SecurityMiddleware as a unified interface
//! - BatchOperationRequest for batched approval
//! - Permission presets (CLI, Web, Agent)
//! - Security status monitoring

use crate::auditor::{AuditFilter, OperationRequest, SecurityAuditor};
use crate::types::*;
use std::collections::HashMap;
use tracing::{debug, info, warn};

/// Build a `tokio::process::Command` that executes a shell command.
///
/// On Windows, uses `raw_arg` to avoid C-runtime quoting that garbles
/// cmd.exe's own quote handling (e.g. `if exist "path"` commands).
/// On Unix, uses standard `.arg()` which works correctly with `sh -c`.
fn shell_command(command: &str) -> tokio::process::Command {
    #[cfg(target_os = "windows")]
    {
        #[allow(unused_imports)]
        use std::os::windows::process::CommandExt;
        let mut cmd = tokio::process::Command::new("cmd");
        cmd.raw_arg(format!("/C {}", command));
        cmd
    }
    #[cfg(not(target_os = "windows"))]
    {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c").arg(command);
        cmd
    }
}

/// Permission preset for security operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionPreset {
    /// Read-only: only file_read, dir_read allowed
    ReadOnly,
    /// Standard: read + write + network_request
    Standard,
    /// Elevated: standard + process_exec + process_spawn
    Elevated,
    /// Unrestricted: all operations allowed (subject to ABAC)
    Unrestricted,
}

impl PermissionPreset {
    /// Check if an operation type is allowed under this preset.
    pub fn allows(&self, op: OperationType) -> bool {
        match self {
            Self::ReadOnly => matches!(
                op,
                OperationType::FileRead | OperationType::DirRead
            ),
            Self::Standard => matches!(
                op,
                OperationType::FileRead
                    | OperationType::FileWrite
                    | OperationType::DirRead
                    | OperationType::DirCreate
                    | OperationType::NetworkRequest
                    | OperationType::NetworkDownload
            ),
            Self::Elevated => matches!(
                op,
                OperationType::FileRead
                    | OperationType::FileWrite
                    | OperationType::FileDelete
                    | OperationType::DirRead
                    | OperationType::DirCreate
                    | OperationType::DirDelete
                    | OperationType::ProcessExec
                    | OperationType::ProcessSpawn
                    | OperationType::NetworkRequest
                    | OperationType::NetworkDownload
                    | OperationType::NetworkUpload
            ),
            Self::Unrestricted => true,
        }
    }
}

/// Security middleware - unified interface for all security-wrapped operations.
pub struct SecurityMiddleware {
    auditor: std::sync::Arc<SecurityAuditor>,
    user: String,
    source: String,
    workspace: String,
    preset: PermissionPreset,
}

impl SecurityMiddleware {
    pub fn new(
        auditor: std::sync::Arc<SecurityAuditor>,
        user: &str,
        source: &str,
        workspace: &str,
    ) -> Self {
        info!(
            user = user,
            source = source,
            workspace = workspace,
            preset = "Standard",
            "[Security] Middleware created",
        );
        Self {
            auditor,
            user: user.to_string(),
            source: source.to_string(),
            workspace: workspace.to_string(),
            preset: PermissionPreset::Standard,
        }
    }

    pub fn with_preset(
        auditor: std::sync::Arc<SecurityAuditor>,
        user: &str,
        source: &str,
        workspace: &str,
        preset: PermissionPreset,
    ) -> Self {
        info!(
            user = user,
            source = source,
            workspace = workspace,
            preset = ?preset,
            "[Security] Middleware created with preset",
        );
        Self {
            auditor,
            user: user.to_string(),
            source: source.to_string(),
            workspace: workspace.to_string(),
            preset,
        }
    }

    /// Get current permission preset.
    pub fn preset(&self) -> PermissionPreset {
        self.preset
    }

    /// Set permission preset.
    pub fn set_preset(&mut self, preset: PermissionPreset) {
        info!(
            old_preset = ?self.preset,
            new_preset = ?preset,
            user = %self.user,
            "[Security] Permission preset changed",
        );
        self.preset = preset;
    }

    /// Check if an operation type is allowed by the preset.
    pub fn is_operation_allowed(&self, op: OperationType) -> bool {
        self.preset.allows(op)
    }

    fn check_operation(&self, op: OperationType, target: &str) -> Result<String, String> {
        if !self.preset.allows(op) {
            warn!(
                operation = %op,
                target = target,
                preset = ?self.preset,
                user = %self.user,
                source = %self.source,
                "[Security] Operation denied by preset: operation={}, preset={:?}",
                op,
                self.preset,
            );
            return Err(format!(
                "operation {} not allowed under {:?} preset",
                op, self.preset
            ));
        }
        let req = OperationRequest {
            id: uuid::Uuid::new_v4().to_string(),
            op_type: op,
            danger_level: get_danger_level(op),
            user: self.user.clone(),
            source: self.source.clone(),
            target: target.to_string(),
            timestamp: Some(chrono::Utc::now()),
            ..Default::default()
        };
        let danger = req.danger_level;
        let (allowed, err, _) = self.auditor.request_permission(&req);
        if allowed {
            debug!(
                operation = %op,
                target = target,
                danger_level = %danger,
                user = %self.user,
                source = %self.source,
                "[Security] Operation approved: operation={}, target={}, danger={}",
                op,
                target,
                danger,
            );
            Ok(target.to_string())
        } else {
            warn!(
                operation = %op,
                target = target,
                danger_level = %danger,
                user = %self.user,
                source = %self.source,
                error = ?err,
                "[Security] Operation denied: operation={}, target={}, danger={}",
                op,
                target,
                danger,
            );
            Err(err.unwrap_or_else(|| "permission denied".to_string()))
        }
    }

    /// Get workspace.
    pub fn workspace(&self) -> &str {
        &self.workspace
    }

    /// Get user.
    pub fn user(&self) -> &str {
        &self.user
    }

    /// Get source.
    pub fn source(&self) -> &str {
        &self.source
    }

    // -----------------------------------------------------------------------
    // Wrapper accessors
    // -----------------------------------------------------------------------

    /// Get file operations wrapper.
    pub fn file(&self) -> SecureFileWrapper<'_> {
        SecureFileWrapper::new(self)
    }

    /// Get process operations wrapper.
    pub fn process(&self) -> SecureProcessWrapper<'_> {
        SecureProcessWrapper::new(self)
    }

    /// Get network operations wrapper.
    pub fn network(&self) -> SecureNetworkWrapper<'_> {
        SecureNetworkWrapper::new(self)
    }

    /// Get hardware operations wrapper.
    pub fn hardware(&self) -> SecureHardwareWrapper<'_> {
        SecureHardwareWrapper::new(self)
    }

    // -----------------------------------------------------------------------
    // Batch operations
    // -----------------------------------------------------------------------

    /// Request permission for a batch of operations at once.
    ///
    /// Equivalent to Go's `RequestBatchPermission()`. The batch is evaluated
    /// using the highest danger level from all operations. If approved,
    /// each individual operation is then checked.
    pub fn request_batch_permission(
        &self,
        batch: &BatchOperationRequest,
    ) -> Result<String, String> {
        if batch.operations.is_empty() {
            return Err("no operations in batch".to_string());
        }

        info!(
            batch_id = %batch.id,
            operation_count = batch.operations.len(),
            user = %self.user,
            source = %self.source,
            "[Security] Batch permission request: {} operations",
            batch.operations.len(),
        );

        // Find the highest danger level
        let mut max_danger = DangerLevel::Low;
        for op in &batch.operations {
            if op.danger_level > max_danger {
                max_danger = op.danger_level;
            }
            if !self.preset.allows(op.op_type) {
                warn!(
                    operation = %op.op_type,
                    target = %op.target,
                    preset = ?self.preset,
                    "[Security] Batch operation denied by preset: operation={}",
                    op.op_type,
                );
                return Err(format!(
                    "operation {} not allowed under {:?} preset",
                    op.op_type, self.preset
                ));
            }
        }

        // Create a summary request for the batch
        let summary_id = if batch.id.is_empty() {
            uuid::Uuid::new_v4().to_string()
        } else {
            batch.id.clone()
        };

        let summary_req = OperationRequest {
            id: summary_id.clone(),
            op_type: OperationType::ProcessExec, // batch_operation - use a reasonable default
            danger_level: max_danger,
            user: self.user.clone(),
            source: self.source.clone(),
            target: format!("{} operations", batch.operations.len()),
            timestamp: Some(chrono::Utc::now()),
            ..Default::default()
        };

        // Request permission for the batch summary
        let (allowed, err, _) = self.auditor.request_permission(&summary_req);
        if !allowed {
            warn!(
                batch_id = %summary_id,
                max_danger = %max_danger,
                "[Security] Batch permission denied",
            );
            return Err(err.unwrap_or_else(|| "batch permission denied".to_string()));
        }

        // If batch is approved, check all individual operations
        for op in &batch.operations {
            let mut individual_req = op.clone();
            individual_req.user = self.user.clone();
            individual_req.source = self.source.clone();
            let (allowed, err, _) = self.auditor.request_permission(&individual_req);
            if !allowed {
                warn!(
                    batch_id = %summary_id,
                    operation = %op.op_type,
                    target = %op.target,
                    "[Security] Individual operation in batch denied",
                );
                return Err(err.unwrap_or_else(|| "individual operation denied".to_string()));
            }
        }

        info!(
            batch_id = %summary_id,
            operation_count = batch.operations.len(),
            "[Security] Batch approved: {} operations",
            batch.operations.len(),
        );

        Ok(summary_id)
    }

    // -----------------------------------------------------------------------
    // Pending request management
    // -----------------------------------------------------------------------

    /// Approve a pending request (for user interaction).
    pub fn approve_pending_request(&self, request_id: &str) -> Result<(), String> {
        info!(
            request_id = request_id,
            user = %self.user,
            source = %self.source,
            "[Security] User approved operation: request_id={}",
            request_id,
        );
        self.auditor.approve_request(request_id, &self.user)
    }

    /// Deny a pending request (for user interaction).
    pub fn deny_pending_request(&self, request_id: &str, reason: &str) -> Result<(), String> {
        info!(
            request_id = request_id,
            user = %self.user,
            source = %self.source,
            reason = reason,
            "[Security] User rejected operation: request_id={}, reason={}",
            request_id,
            reason,
        );
        self.auditor.deny_request(request_id, &self.user, reason)
    }

    // -----------------------------------------------------------------------
    // Status / statistics
    // -----------------------------------------------------------------------

    /// Get a security summary including statistics and pending requests.
    ///
    /// Equivalent to Go's `GetSecuritySummary()`.
    pub fn get_security_summary(&self) -> serde_json::Value {
        let stats = self.auditor.get_statistics();
        let pending = self.auditor.get_pending_requests();

        let pending_summaries: Vec<serde_json::Value> = pending
            .iter()
            .map(|req| {
                serde_json::json!({
                    "id": req.id,
                    "type": req.op_type.to_string(),
                    "target": req.target,
                    "danger": req.danger_level.to_string(),
                    "timestamp": req.timestamp.map(|t| t.to_rfc3339()).unwrap_or_default(),
                })
            })
            .collect();

        serde_json::json!({
            "statistics": stats,
            "pending_requests": pending.len(),
            "pending": pending_summaries,
            "user": self.user,
            "source": self.source,
            "workspace": self.workspace,
        })
    }

    /// Export the audit log to a file.
    pub fn export_audit_log(&self, file_path: &str) -> Result<(), String> {
        info!(
            file_path = file_path,
            user = %self.user,
            "[Security] Exporting audit log to: {}",
            file_path,
        );
        self.auditor.export_audit_log(file_path)
    }

    /// Query the audit log with a filter.
    ///
    /// Equivalent to Go's `SecurityMiddleware.GetAuditLog()`.
    /// Delegates to the underlying auditor.
    pub fn get_audit_log(&self, filter: AuditFilter) -> Vec<crate::auditor::AuditEvent> {
        self.auditor.get_audit_log(filter)
    }
}

// ---------------------------------------------------------------------------
// Batch operation request
// ---------------------------------------------------------------------------

/// Batch of operations to be approved together.
///
/// Equivalent to Go's `BatchOperationRequest`.
#[derive(Debug, Clone)]
pub struct BatchOperationRequest {
    /// Batch ID.
    pub id: String,
    /// Operations in the batch.
    pub operations: Vec<OperationRequest>,
    /// User who submitted the batch.
    pub user: String,
    /// Source of the batch request.
    pub source: String,
    /// Description of the batch.
    pub description: String,
}

impl Default for BatchOperationRequest {
    fn default() -> Self {
        Self {
            id: String::new(),
            operations: Vec::new(),
            user: String::new(),
            source: String::new(),
            description: String::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Permission factories
// ---------------------------------------------------------------------------

/// Create a CLI permission configuration (less restrictive).
///
/// Equivalent to Go's `CreateCLIPermission()`:
/// - Allows: file_read, file_write, file_delete, dir_read, dir_create,
///   process_exec, network_download, network_request
/// - Denied targets: /etc/sudoers, /etc/passwd, Windows hosts file
/// - Require approval: process_kill, system_shutdown, system_reboot
/// - Max danger level: High
pub fn create_cli_permission() -> Permission {
    let mut allowed = HashMap::new();
    allowed.insert(OperationType::FileRead, true);
    allowed.insert(OperationType::FileWrite, true);
    allowed.insert(OperationType::FileDelete, true);
    allowed.insert(OperationType::DirRead, true);
    allowed.insert(OperationType::DirCreate, true);
    allowed.insert(OperationType::ProcessExec, true);
    allowed.insert(OperationType::NetworkDownload, true);
    allowed.insert(OperationType::NetworkRequest, true);

    let mut require_approval = HashMap::new();
    require_approval.insert(OperationType::ProcessKill, true);
    require_approval.insert(OperationType::SystemShutdown, true);
    require_approval.insert(OperationType::SystemReboot, true);

    Permission {
        allowed_types: allowed,
        allowed_targets: Vec::new(),
        denied_targets: vec![
            "/etc/sudoers".to_string(),
            "/etc/passwd".to_string(),
            "C:/Windows/System32/drivers/etc/hosts".to_string(),
        ],
        require_approval,
        max_danger_level: DangerLevel::High,
    }
}

/// Create a Web permission configuration (more restrictive).
///
/// Equivalent to Go's `CreateWebPermission()`:
/// - Allows: file_read, file_write, dir_read, dir_create
/// - Require approval: file_delete, process_exec, network_download
/// - Max danger level: Medium
pub fn create_web_permission() -> Permission {
    let mut allowed = HashMap::new();
    allowed.insert(OperationType::FileRead, true);
    allowed.insert(OperationType::FileWrite, true);
    allowed.insert(OperationType::DirRead, true);
    allowed.insert(OperationType::DirCreate, true);

    let mut require_approval = HashMap::new();
    require_approval.insert(OperationType::FileDelete, true);
    require_approval.insert(OperationType::ProcessExec, true);
    require_approval.insert(OperationType::NetworkDownload, true);

    Permission {
        allowed_types: allowed,
        allowed_targets: Vec::new(),
        denied_targets: Vec::new(),
        require_approval,
        max_danger_level: DangerLevel::Medium,
    }
}

/// Create an Agent permission configuration (context-aware).
///
/// Equivalent to Go's `CreateAgentPermission(agentID)`:
/// - Allows: file_read, file_write, dir_read, dir_create, process_exec, network_request
/// - Require approval: file_delete, process_kill, system_shutdown, network_download
/// - Max danger level: High
pub fn create_agent_permission(_agent_id: &str) -> Permission {
    let mut allowed = HashMap::new();
    allowed.insert(OperationType::FileRead, true);
    allowed.insert(OperationType::FileWrite, true);
    allowed.insert(OperationType::DirRead, true);
    allowed.insert(OperationType::DirCreate, true);
    allowed.insert(OperationType::ProcessExec, true);
    allowed.insert(OperationType::NetworkRequest, true);

    let mut require_approval = HashMap::new();
    require_approval.insert(OperationType::FileDelete, true);
    require_approval.insert(OperationType::ProcessKill, true);
    require_approval.insert(OperationType::SystemShutdown, true);
    require_approval.insert(OperationType::NetworkDownload, true);

    Permission {
        allowed_types: allowed,
        allowed_targets: Vec::new(),
        denied_targets: Vec::new(),
        require_approval,
        max_danger_level: DangerLevel::High,
    }
}

// ---------------------------------------------------------------------------
// Wrappers
// ---------------------------------------------------------------------------

/// Secure file operation wrapper.
pub struct SecureFileWrapper<'a> {
    middleware: &'a SecurityMiddleware,
}

impl<'a> SecureFileWrapper<'a> {
    pub fn new(middleware: &'a SecurityMiddleware) -> Self {
        Self { middleware }
    }

    /// Check file read permission.
    pub fn check_file_read(&self, path: &str) -> Result<String, String> {
        let validated = validate_path(path, &self.middleware.workspace)?;
        self.middleware
            .check_operation(OperationType::FileRead, &validated)
    }

    /// Check file write permission.
    pub fn check_file_write(&self, path: &str) -> Result<String, String> {
        let validated = validate_path(path, &self.middleware.workspace)?;
        self.middleware
            .check_operation(OperationType::FileWrite, &validated)
    }

    /// Check file delete permission.
    pub fn check_file_delete(&self, path: &str) -> Result<String, String> {
        let validated = validate_path(path, &self.middleware.workspace)?;
        self.middleware
            .check_operation(OperationType::FileDelete, &validated)
    }

    /// Check directory read permission.
    pub fn check_dir_read(&self, path: &str) -> Result<String, String> {
        let validated = validate_path(path, &self.middleware.workspace)?;
        self.middleware
            .check_operation(OperationType::DirRead, &validated)
    }

    /// Check directory create permission.
    pub fn check_dir_create(&self, path: &str) -> Result<String, String> {
        let validated = validate_path(path, &self.middleware.workspace)?;
        self.middleware
            .check_operation(OperationType::DirCreate, &validated)
    }

    /// Check directory delete permission.
    pub fn check_dir_delete(&self, path: &str) -> Result<String, String> {
        let validated = validate_path(path, &self.middleware.workspace)?;
        self.middleware
            .check_operation(OperationType::DirDelete, &validated)
    }

    // -----------------------------------------------------------------------
    // Actual I/O operations (with security checks)
    // -----------------------------------------------------------------------

    /// Read file contents with security check.
    pub async fn read_file(&self, path: &str) -> Result<String, String> {
        let validated = self.check_file_read(path)?;
        debug!(
            path = path,
            user = %self.middleware.user,
            "[Security] File read: path={}",
            path,
        );
        tokio::fs::read_to_string(&validated)
            .await
            .map_err(|e| format!("failed to read file: {}", e))
    }

    /// Write content to a file with security check.
    pub async fn write_file(&self, path: &str, content: &str) -> Result<(), String> {
        let validated = self.check_file_write(path)?;
        info!(
            path = path,
            content_len = content.len(),
            user = %self.middleware.user,
            "[Security] File write: path={}, size={}",
            path,
            content.len(),
        );
        if let Some(parent) = std::path::Path::new(&validated).parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("failed to create directories: {}", e))?;
        }
        tokio::fs::write(&validated, content)
            .await
            .map_err(|e| format!("failed to write file: {}", e))
    }

    /// Edit a file by replacing a string with security check.
    pub async fn edit_file(&self, path: &str, old: &str, new: &str) -> Result<(), String> {
        let validated = self.check_file_write(path)?;
        let content = tokio::fs::read_to_string(&validated)
            .await
            .map_err(|e| format!("failed to read file for editing: {}", e))?;
        let updated = if old.contains('\n') || content.contains(old) {
            content.replacen(old, new, 1)
        } else {
            return Err("pattern not found in file".to_string());
        };
        tokio::fs::write(&validated, updated)
            .await
            .map_err(|e| format!("failed to write edited file: {}", e))
    }

    /// Append content to a file with security check.
    pub async fn append_file(&self, path: &str, content: &str) -> Result<(), String> {
        let validated = self.check_file_write(path)?;
        let existing = tokio::fs::read_to_string(&validated).await.unwrap_or_default();
        let new_content = if existing.is_empty() {
            content.to_string()
        } else if existing.ends_with('\n') {
            format!("{}{}", existing, content)
        } else {
            format!("{}\n{}", existing, content)
        };
        tokio::fs::write(&validated, new_content)
            .await
            .map_err(|e| format!("failed to append to file: {}", e))
    }

    /// Delete a file with security check.
    pub async fn delete_file(&self, path: &str) -> Result<(), String> {
        let validated = self.check_file_delete(path)?;
        info!(
            path = path,
            user = %self.middleware.user,
            "[Security] File delete: path={}",
            path,
        );
        tokio::fs::remove_file(&validated)
            .await
            .map_err(|e| format!("failed to delete file: {}", e))
    }

    /// Read directory contents with security check.
    pub async fn read_directory(&self, path: &str) -> Result<Vec<String>, String> {
        let validated = self.check_dir_read(path)?;
        let mut entries = Vec::new();
        let mut dir = tokio::fs::read_dir(&validated)
            .await
            .map_err(|e| format!("failed to read directory: {}", e))?;
        while let Some(entry) = dir
            .next_entry()
            .await
            .map_err(|e| format!("failed to read directory entry: {}", e))?
        {
            let name = entry.file_name().to_string_lossy().to_string();
            let ft = entry
                .file_type()
                .await
                .map_err(|e| format!("failed to get file type: {}", e))?;
            if ft.is_dir() {
                entries.push(format!("{}/", name));
            } else {
                entries.push(name);
            }
        }
        entries.sort();
        Ok(entries)
    }

    /// Create directory with security check.
    pub async fn create_directory(&self, path: &str) -> Result<(), String> {
        let validated = self.check_dir_create(path)?;
        tokio::fs::create_dir_all(&validated)
            .await
            .map_err(|e| format!("failed to create directory: {}", e))
    }

    /// Delete a directory (recursively) with security check.
    pub async fn delete_directory(&self, path: &str) -> Result<(), String> {
        let validated = self.check_dir_delete(path)?;
        tokio::fs::remove_dir_all(&validated)
            .await
            .map_err(|e| format!("failed to delete directory: {}", e))
    }

    /// Stat a file or directory, returning metadata.
    pub async fn stat(&self, path: &str) -> Result<FileMetadata, String> {
        let validated = {
            // Use file read as the permission check for stat
            let validated = validate_path(path, &self.middleware.workspace)?;
            self.middleware
                .check_operation(OperationType::FileRead, &validated)?;
            validated
        };
        let meta = tokio::fs::metadata(&validated)
            .await
            .map_err(|e| format!("failed to stat: {}", e))?;
        Ok(FileMetadata {
            is_file: meta.is_file(),
            is_dir: meta.is_dir(),
            len: meta.len(),
            readonly: meta.permissions().readonly(),
            modified: meta
                .modified()
                .ok()
                .map(|t| {
                    let dt: chrono::DateTime<chrono::Utc> = t.into();
                    dt.to_rfc3339()
                })
                .unwrap_or_default(),
        })
    }

    /// Open and read a file as raw bytes with security check.
    pub async fn open_file(&self, path: &str) -> Result<Vec<u8>, String> {
        let validated = self.check_file_read(path)?;
        tokio::fs::read(&validated)
            .await
            .map_err(|e| format!("failed to read file: {}", e))
    }

    /// List directory contents with full entry info (name, is_dir, size).
    pub async fn list_dir(&self, path: &str) -> Result<Vec<DirEntry>, String> {
        let validated = self.check_dir_read(path)?;
        let mut entries = Vec::new();
        let mut dir = tokio::fs::read_dir(&validated)
            .await
            .map_err(|e| format!("failed to read directory: {}", e))?;
        while let Some(entry) = dir
            .next_entry()
            .await
            .map_err(|e| format!("failed to read directory entry: {}", e))?
        {
            let name = entry.file_name().to_string_lossy().to_string();
            let ft = entry
                .file_type()
                .await
                .map_err(|e| format!("failed to get file type: {}", e))?;
            let meta = entry
                .metadata()
                .await
                .map_err(|e| format!("failed to get metadata: {}", e))?;
            entries.push(DirEntry {
                name,
                is_dir: ft.is_dir(),
                size: meta.len(),
            });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }

    /// Remove a file with security check (alias for delete_file).
    pub async fn remove_file(&self, path: &str) -> Result<(), String> {
        self.delete_file(path).await
    }

    /// Remove a directory with security check (alias for delete_directory).
    pub async fn remove_dir(&self, path: &str) -> Result<(), String> {
        self.delete_directory(path).await
    }

    /// Create a new directory with security check (alias for create_directory).
    pub async fn create_dir(&self, path: &str) -> Result<(), String> {
        self.create_directory(path).await
    }
}

/// File metadata returned by stat operations.
#[derive(Debug, Clone)]
pub struct FileMetadata {
    /// Whether the path is a regular file.
    pub is_file: bool,
    /// Whether the path is a directory.
    pub is_dir: bool,
    /// Size in bytes.
    pub len: u64,
    /// Whether the file is read-only.
    pub readonly: bool,
    /// Last modified time (RFC 3339).
    pub modified: String,
}

/// Directory entry with metadata.
#[derive(Debug, Clone)]
pub struct DirEntry {
    /// Entry name.
    pub name: String,
    /// Whether the entry is a directory.
    pub is_dir: bool,
    /// File size in bytes (0 for directories).
    pub size: u64,
}

/// Secure process execution wrapper.
pub struct SecureProcessWrapper<'a> {
    middleware: &'a SecurityMiddleware,
}

impl<'a> SecureProcessWrapper<'a> {
    pub fn new(middleware: &'a SecurityMiddleware) -> Self {
        Self { middleware }
    }

    /// Check process execution permission.
    pub fn check_process_exec(&self, command: &str) -> Result<(), String> {
        let (safe, reason) = is_safe_command(command);
        if !safe {
            warn!(
                command = command,
                reason = %reason,
                user = %self.middleware.user,
                source = %self.middleware.source,
                "[Security] Dangerous command blocked: command={}, reason={}",
                command,
                reason,
            );
            return Err(format!("command blocked: {}", reason));
        }
        self.middleware
            .check_operation(OperationType::ProcessExec, command)?;
        Ok(())
    }

    /// Check process spawn permission.
    pub fn check_process_spawn(&self, command: &str) -> Result<(), String> {
        let (safe, reason) = is_safe_command(command);
        if !safe {
            warn!(
                command = command,
                reason = %reason,
                user = %self.middleware.user,
                source = %self.middleware.source,
                "[Security] Dangerous spawn command blocked: command={}, reason={}",
                command,
                reason,
            );
            return Err(format!("command blocked: {}", reason));
        }
        self.middleware
            .check_operation(OperationType::ProcessSpawn, command)?;
        Ok(())
    }

    /// Check process kill permission.
    pub fn check_process_kill(&self, pid: &str) -> Result<(), String> {
        self.middleware
            .check_operation(OperationType::ProcessKill, pid)?;
        Ok(())
    }

    /// Execute a command with security check and return output.
    pub async fn execute_command(&self, command: &str, timeout_secs: u64) -> Result<String, String> {
        self.check_process_exec(command)?;

        info!(
            command = command,
            timeout_secs = timeout_secs,
            user = %self.middleware.user,
            "[Security] Executing command: cmd={}, timeout={}s",
            command,
            timeout_secs,
        );

        let timeout = std::time::Duration::from_secs(timeout_secs.min(600));
        let result = tokio::time::timeout(
            timeout,
            shell_command(command).output(),
        )
        .await;

        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                if output.status.success() {
                    Ok(stdout.trim().to_string())
                } else {
                    Err(format!(
                        "exit code {}: {}",
                        output.status.code().unwrap_or(-1),
                        stderr.trim()
                    ))
                }
            }
            Ok(Err(e)) => Err(format!("failed to execute command: {}", e)),
            Err(_) => Err(format!("command timed out after {}s", timeout_secs)),
        }
    }
    /// Spawn a process and return its PID without waiting for completion.
    ///
    /// Performs security checks (dangerous pattern scan + ABAC), then spawns
    /// the command as a detached child process. Returns the OS-assigned PID.
    pub async fn spawn(&self, command: &str) -> Result<u32, String> {
        self.check_process_spawn(command)?;

        info!(
            command = command,
            user = %self.middleware.user,
            "[Security] Spawning process: cmd={}",
            command,
        );

        let mut child = shell_command(command)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("failed to spawn process: {}", e))?;

        let pid = child.id().unwrap_or(0);
        // Detach: let the child run independently.
        // We must not drop the Child without awaiting, otherwise it is killed.
        // Instead, spawn a background task that waits and discards output.
        tokio::spawn(async move {
            let _ = child.wait().await;
        });

        Ok(pid)
    }

    /// Kill a process by PID with security check.
    ///
    /// On Windows, calls `taskkill /F /PID <pid>`.
    /// On Unix, sends `SIGKILL` via `kill -9 <pid>`.
    pub async fn kill(&self, pid: u32) -> Result<(), String> {
        self.check_process_kill(&pid.to_string())?;

        warn!(
            pid = pid,
            user = %self.middleware.user,
            "[Security] Killing process: pid={}",
            pid,
        );

        let kill_cmd = if cfg!(target_os = "windows") {
            format!("taskkill /F /PID {}", pid)
        } else {
            format!("kill -9 {}", pid)
        };

        let output = shell_command(&kill_cmd)
            .output()
            .await
            .map_err(|e| format!("failed to execute kill command: {}", e))?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // On Unix, kill may report "no such process" if it already exited.
            if stderr.contains("no such process") || stderr.contains("not found") {
                Ok(())
            } else {
                Err(format!(
                    "failed to kill process {}: {}",
                    pid,
                    stderr.trim()
                ))
            }
        }
    }

    /// Terminate a process by PID with security check (graceful termination).
    ///
    /// On Windows, calls `taskkill /PID <pid>` (without /F).
    /// On Unix, sends `SIGTERM` via `kill <pid>`.
    pub async fn terminate(&self, pid: u32) -> Result<(), String> {
        self.check_process_kill(&pid.to_string())?;

        warn!(
            pid = pid,
            user = %self.middleware.user,
            "[Security] Terminating process: pid={}",
            pid,
        );

        let term_cmd = if cfg!(target_os = "windows") {
            format!("taskkill /PID {}", pid)
        } else {
            format!("kill {}", pid)
        };

        let output = shell_command(&term_cmd)
            .output()
            .await
            .map_err(|e| format!("failed to execute terminate command: {}", e))?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(format!(
                "failed to terminate process {}: {}",
                pid,
                stderr.trim()
            ))
        }
    }

    /// Wait for a process to complete (by PID) with security check.
    ///
    /// This uses a platform-specific wait command. On Unix, `wait <pid>` only
    /// works for children of the current shell, so this method polls for
    /// process existence instead.
    ///
    /// Returns the exit code if available, or -1 on error/timeout.
    pub async fn wait(&self, pid: u32, timeout_secs: u64) -> Result<i32, String> {
        self.middleware
            .check_operation(OperationType::ProcessExec, &format!("wait:{}", pid))?;

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);

        loop {
            // Check if process is still running
            let check_cmd = if cfg!(target_os = "windows") {
                format!("tasklist /FI \"PID eq {}\" /NH", pid)
            } else {
                format!("ps -p {} -o pid=", pid)
            };

            let output = shell_command(&check_cmd)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .output()
                .await
                .map_err(|e| format!("failed to check process: {}", e))?;

            let stdout = String::from_utf8_lossy(&output.stdout);
            let is_running = if cfg!(target_os = "windows") {
                stdout.contains(&pid.to_string())
            } else {
                stdout.trim().contains(&pid.to_string())
            };

            if !is_running {
                // Process has exited. Try to get exit code on Windows.
                if cfg!(target_os = "windows") {
                    // Windows: no portable way to get exit code of non-child process
                    return Ok(0);
                }
                // On Unix, `waitpid` only works for our children. Return 0 for
                // non-child processes that have exited.
                return Ok(0);
            }

            if tokio::time::Instant::now() >= deadline {
                return Err(format!("timeout waiting for process {} after {}s", pid, timeout_secs));
            }

            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    }

    /// Send a signal to a process with security check (Unix only).
    ///
    /// On Windows, this returns an error since Windows does not support
    /// POSIX signals. Use `kill()` or `terminate()` instead.
    pub async fn signal(&self, pid: u32, signal: i32) -> Result<(), String> {
        self.middleware
            .check_operation(OperationType::ProcessExec, &format!("signal:{}:{}", pid, signal))?;

        if cfg!(target_os = "windows") {
            return Err("POSIX signals are not supported on Windows; use kill() or terminate()".to_string());
        }

        let kill_cmd = format!("kill -{} {}", signal, pid);
        let output = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&kill_cmd)
            .output()
            .await
            .map_err(|e| format!("failed to send signal: {}", e))?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(format!(
                "failed to send signal {} to process {}: {}",
                signal, pid, stderr.trim()
            ))
        }
    }

    /// Get the stdout/stderr output from a completed command execution.
    ///
    /// This runs the command, waits for completion, and returns combined output.
    /// Essentially a convenience wrapper around `execute_command` that returns
    /// both stdout and stderr.
    pub async fn get_output(
        &self,
        command: &str,
        timeout_secs: u64,
    ) -> Result<ProcessOutput, String> {
        self.check_process_exec(command)?;

        let timeout = std::time::Duration::from_secs(timeout_secs.min(600));
        let result = tokio::time::timeout(
            timeout,
            shell_command(command).output(),
        )
        .await;

        match result {
            Ok(Ok(output)) => Ok(ProcessOutput {
                stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
                exit_code: output.status.code(),
                success: output.status.success(),
            }),
            Ok(Err(e)) => Err(format!("failed to execute command: {}", e)),
            Err(_) => Err(format!("command timed out after {}s", timeout_secs)),
        }
    }
}

/// Output from a process execution.
#[derive(Debug, Clone)]
pub struct ProcessOutput {
    /// Standard output.
    pub stdout: String,
    /// Standard error.
    pub stderr: String,
    /// Exit code, if available.
    pub exit_code: Option<i32>,
    /// Whether the process exited successfully (exit code 0).
    pub success: bool,
}

/// Secure network operation wrapper.
pub struct SecureNetworkWrapper<'a> {
    middleware: &'a SecurityMiddleware,
}

impl<'a> SecureNetworkWrapper<'a> {
    pub fn new(middleware: &'a SecurityMiddleware) -> Self {
        Self { middleware }
    }

    /// Check network request permission.
    pub fn check_network_request(&self, url: &str) -> Result<String, String> {
        self.middleware
            .check_operation(OperationType::NetworkRequest, url)
    }

    /// Check network download permission.
    pub fn check_network_download(&self, url: &str) -> Result<String, String> {
        // Validate URL scheme
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err("only http/https URLs are allowed".to_string());
        }
        self.middleware
            .check_operation(OperationType::NetworkDownload, url)
    }

    /// Check network upload permission.
    pub fn check_network_upload(&self, url: &str) -> Result<String, String> {
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err("only http/https URLs are allowed".to_string());
        }
        self.middleware
            .check_operation(OperationType::NetworkUpload, url)
    }

    /// Download a URL with security check.
    pub async fn download_url(&self, url: &str, max_size: usize) -> Result<Vec<u8>, String> {
        let validated = self.check_network_download(url)?;

        info!(
            url = url,
            max_size = max_size,
            user = %self.middleware.user,
            "[Security] Downloading URL: url={}, max_size={}",
            url,
            max_size,
        );

        let response = reqwest::get(&validated).await.map_err(|e| format!("request failed: {}", e))?;

        let status = response.status();
        if !status.is_success() {
            return Err(format!("HTTP {} {}", status.as_u16(), status.canonical_reason().unwrap_or("Unknown")));
        }

        let bytes = response.bytes().await.map_err(|e| format!("failed to read response: {}", e))?;
        if bytes.len() > max_size {
            return Err(format!("response too large: {} bytes (limit: {})", bytes.len(), max_size));
        }

        Ok(bytes.to_vec())
    }

    /// Perform an HTTP GET request with security check.
    ///
    /// Returns the response body as a string.
    pub async fn get(&self, url: &str) -> Result<HttpResponse, String> {
        let validated = self.check_network_request(url)?;

        let response = reqwest::get(&validated)
            .await
            .map_err(|e| format!("GET request failed: {}", e))?;

        let status_code = response.status().as_u16();
        let success = response.status().is_success();

        let body = response
            .text()
            .await
            .map_err(|e| format!("failed to read response body: {}", e))?;

        Ok(HttpResponse {
            status_code,
            body,
            success,
        })
    }

    /// Perform an HTTP POST request with security check.
    ///
    /// Sends the provided body with the given content type header.
    pub async fn post(
        &self,
        url: &str,
        body: &str,
        content_type: &str,
    ) -> Result<HttpResponse, String> {
        self.check_network_upload(url)?;

        let client = reqwest::Client::new();
        let response = client
            .post(url)
            .header("Content-Type", content_type)
            .body(body.to_string())
            .send()
            .await
            .map_err(|e| format!("POST request failed: {}", e))?;

        let status_code = response.status().as_u16();
        let success = response.status().is_success();

        let resp_body = response
            .text()
            .await
            .map_err(|e| format!("failed to read response body: {}", e))?;

        Ok(HttpResponse {
            status_code,
            body: resp_body,
            success,
        })
    }

    /// Perform a generic HTTP request with security check.
    ///
    /// Supports GET, POST, PUT, DELETE, PATCH, HEAD methods.
    /// Headers are provided as a slice of (key, value) tuples.
    /// An optional body can be included.
    pub async fn do_request(&self, req: &HttpRequest) -> Result<HttpResponse, String> {
        self.check_network_request(&req.url)?;

        let client = reqwest::Client::new();
        let method = match req.method.to_uppercase().as_str() {
            "GET" => reqwest::Method::GET,
            "POST" => reqwest::Method::POST,
            "PUT" => reqwest::Method::PUT,
            "DELETE" => reqwest::Method::DELETE,
            "PATCH" => reqwest::Method::PATCH,
            "HEAD" => reqwest::Method::HEAD,
            other => return Err(format!("unsupported HTTP method: {}", other)),
        };

        let mut builder = client.request(method, &req.url);

        for (key, value) in &req.headers {
            builder = builder.header(key.as_str(), value.as_str());
        }

        if let Some(body) = &req.body {
            builder = builder.body(body.clone());
        }

        if let Some(timeout) = req.timeout_secs {
            builder = builder.timeout(std::time::Duration::from_secs(timeout));
        }

        let response = builder
            .send()
            .await
            .map_err(|e| format!("HTTP request failed: {}", e))?;

        let status_code = response.status().as_u16();
        let success = response.status().is_success();

        let resp_body = response
            .text()
            .await
            .map_err(|e| format!("failed to read response body: {}", e))?;

        Ok(HttpResponse {
            status_code,
            body: resp_body,
            success,
        })
    }

    /// Establish an HTTP connection (dial) and return the response status/headers.
    ///
    /// Performs a HEAD request to verify the target is reachable, returning
    /// the status code. This is a lightweight connectivity check with security
    /// policy enforcement.
    pub async fn dial_http(&self, url: &str) -> Result<u16, String> {
        let validated = self.check_network_request(url)?;

        let client = reqwest::Client::new();
        let response = client
            .head(&validated)
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| format!("dial HTTP failed: {}", e))?;

        Ok(response.status().as_u16())
    }
}

/// HTTP response from network operations.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    /// HTTP status code.
    pub status_code: u16,
    /// Response body as a string.
    pub body: String,
    /// Whether the status code indicates success (2xx).
    pub success: bool,
}

/// HTTP request parameters for `do_request`.
#[derive(Debug, Clone)]
pub struct HttpRequest {
    /// URL to request.
    pub url: String,
    /// HTTP method (GET, POST, PUT, DELETE, PATCH, HEAD).
    pub method: String,
    /// Request headers as (key, value) pairs.
    pub headers: Vec<(String, String)>,
    /// Optional request body.
    pub body: Option<String>,
    /// Optional timeout in seconds.
    pub timeout_secs: Option<u64>,
}

impl Default for HttpRequest {
    fn default() -> Self {
        Self {
            url: String::new(),
            method: "GET".to_string(),
            headers: Vec::new(),
            body: None,
            timeout_secs: None,
        }
    }
}

/// Secure hardware operation wrapper.
pub struct SecureHardwareWrapper<'a> {
    middleware: &'a SecurityMiddleware,
}

impl<'a> SecureHardwareWrapper<'a> {
    pub fn new(middleware: &'a SecurityMiddleware) -> Self {
        Self { middleware }
    }

    /// Check I2C permission.
    pub fn check_i2c(&self, device: &str) -> Result<String, String> {
        self.middleware
            .check_operation(OperationType::HardwareI2C, device)
    }

    /// Check SPI permission.
    pub fn check_spi(&self, device: &str) -> Result<String, String> {
        self.middleware
            .check_operation(OperationType::HardwareSPI, device)
    }

    /// Perform SPI write with security check.
    ///
    /// Mirrors Go's `SecureHardwareWrapper.SPIWrite`. Validates permission
    /// through the security auditor. The actual SPI hardware I/O is not
    /// performed here — this method only enforces the security gate.
    pub fn spi_write(&self, device: &str, _data: &[u8]) -> Result<(), String> {
        self.check_spi(&format!("spidev{}", device))?;
        Ok(())
    }

    /// Check GPIO permission.
    pub fn check_gpio(&self, pin: &str) -> Result<String, String> {
        self.middleware
            .check_operation(OperationType::HardwareGPIO, pin)
    }

    // -----------------------------------------------------------------------
    // Actual hardware I/O operations (with security checks)
    // -----------------------------------------------------------------------
    //
    // Hardware I/O is inherently platform-specific and device-dependent.
    // On Linux, I2C and GPIO are accessed via sysfs or character devices.
    // On Windows, these typically require specialized drivers or are not
    // available at all.
    //
    // The implementations below provide the security-wrapped access that
    // delegates to the platform's native file/device I/O. If the underlying
    // device path does not exist, the I/O call returns an error.

    /// Read data from an I2C device with security check.
    ///
    /// `bus` is the I2C bus identifier (e.g., `"1"` or `"/dev/i2c-1"`).
    /// `address` is the 7-bit I2C slave address.
    /// `register` is the register address to read from.
    /// `length` is the number of bytes to read.
    ///
    /// This uses Linux sysfs I2C access when available. On platforms where
    /// raw I2C is not directly accessible, returns an error.
    pub async fn i2c_read(
        &self,
        bus: &str,
        address: u8,
        register: u8,
        length: usize,
    ) -> Result<Vec<u8>, String> {
        let device = if bus.starts_with('/') {
            bus.to_string()
        } else {
            format!("i2c-{}:0x{:02x}", bus, address)
        };

        let _validated = self.check_i2c(&device)?;

        // On Linux, we could access /dev/i2c-N via ioctl. However, since we
        // cannot use platform-specific APIs (libc/unix) in a cross-platform
        // crate, we use a shell command approach for the actual I/O.
        //
        // For embedded Linux systems, i2c-tools (i2cget) is typically available.
        // If the tool is not present, this will return an appropriate error.

        let bus_num = bus.trim_start_matches("/dev/i2c-");
        let i2cget_cmd = format!(
            "i2cget -y {} 0x{:02x} 0x{:02x} w",
            bus_num, address, register
        );

        let output = shell_command(&i2cget_cmd)
            .output()
            .await
            .map_err(|e| format!("failed to execute i2cget: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "i2c read failed (bus={}, addr=0x{:02x}, reg=0x{:02x}): {}",
                bus_num,
                address,
                register,
                stderr.trim()
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();

        // Parse hex output from i2cget (e.g., "0x1a" or "0x001a")
        // For multi-byte reads, we may need i2cdump instead. For simplicity,
        // parse single register value and pad to requested length.
        let value = u16::from_str_radix(stdout.trim_start_matches("0x"), 16)
            .map_err(|e| format!("failed to parse i2cget output '{}': {}", stdout, e))?;

        let mut result = vec![0u8; length];
        let bytes = value.to_be_bytes();
        let copy_len = bytes.len().min(length);
        result[..copy_len].copy_from_slice(&bytes[..copy_len]);

        Ok(result)
    }

    /// Write data to an I2C device with security check.
    ///
    /// `bus` is the I2C bus identifier (e.g., `"1"` or `"/dev/i2c-1"`).
    /// `address` is the 7-bit I2C slave address.
    /// `register` is the register address to write to.
    /// `data` is the bytes to write.
    pub async fn i2c_write(
        &self,
        bus: &str,
        address: u8,
        register: u8,
        data: &[u8],
    ) -> Result<(), String> {
        let device = if bus.starts_with('/') {
            bus.to_string()
        } else {
            format!("i2c-{}:0x{:02x}", bus, address)
        };

        let _validated = self.check_i2c(&device)?;

        let bus_num = bus.trim_start_matches("/dev/i2c-");

        // Use i2cset from i2c-tools for the actual write.
        // i2cset -y <bus> <addr> <register> <value> [mode]
        if data.is_empty() {
            return Err("no data to write".to_string());
        }

        // Write first byte using i2cset
        let value = data[0];
        let i2cset_cmd = format!(
            "i2cset -y {} 0x{:02x} 0x{:02x} 0x{:02x}",
            bus_num, address, register, value
        );

        let output = shell_command(&i2cset_cmd)
            .output()
            .await
            .map_err(|e| format!("failed to execute i2cset: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "i2c write failed (bus={}, addr=0x{:02x}, reg=0x{:02x}): {}",
                bus_num,
                address,
                register,
                stderr.trim()
            ));
        }

        Ok(())
    }

    /// Read the value of a GPIO pin with security check.
    ///
    /// `pin` is the GPIO pin identifier (e.g., `"17"` or `"GPIO17"`).
    ///
    /// On Linux, reads from `/sys/class/gpio/gpio{pin}/value`.
    /// Returns `"0"` (low) or `"1"` (high).
    pub async fn gpio_read(&self, pin: &str) -> Result<String, String> {
        // Normalize pin name: strip "GPIO" prefix if present
        let pin_num = pin.trim_start_matches("GPIO").trim_start_matches("gpio");

        let _validated = self.check_gpio(&format!("GPIO{}", pin_num))?;

        // Try Linux sysfs path first
        let sysfs_path = format!("/sys/class/gpio/gpio{}/value", pin_num);
        match tokio::fs::read_to_string(&sysfs_path).await {
            Ok(content) => {
                let value = content.trim().to_string();
                if value == "0" || value == "1" {
                    Ok(value)
                } else {
                    Err(format!("unexpected GPIO value: '{}'", value))
                }
            }
            Err(_) => {
                // Fallback: try character device gpiochip (newer Linux GPIO API)
                // For simplicity, return error if sysfs is not available.
                Err(format!(
                    "GPIO read for pin {} failed: sysfs path not accessible ({})",
                    pin_num, sysfs_path
                ))
            }
        }
    }

    /// Write a value to a GPIO pin with security check.
    ///
    /// `pin` is the GPIO pin identifier.
    /// `value` should be `"0"` (low) or `"1"` (high).
    pub async fn gpio_write(&self, pin: &str, value: &str) -> Result<(), String> {
        let pin_num = pin.trim_start_matches("GPIO").trim_start_matches("gpio");

        // Validate value
        if value != "0" && value != "1" {
            return Err(format!(
                "invalid GPIO value '{}': must be '0' or '1'",
                value
            ));
        }

        let _validated = self.check_gpio(&format!("GPIO{}", pin_num))?;

        let sysfs_path = format!("/sys/class/gpio/gpio{}/value", pin_num);
        tokio::fs::write(&sysfs_path, value)
            .await
            .map_err(|e| format!("failed to write GPIO {} value: {}", pin_num, e))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests;
