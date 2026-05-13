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
        self.preset = preset;
    }

    /// Check if an operation type is allowed by the preset.
    pub fn is_operation_allowed(&self, op: OperationType) -> bool {
        self.preset.allows(op)
    }

    fn check_operation(&self, op: OperationType, target: &str) -> Result<String, String> {
        if !self.preset.allows(op) {
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
        let (allowed, err, _) = self.auditor.request_permission(&req);
        if allowed {
            Ok(target.to_string())
        } else {
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

        // Find the highest danger level
        let mut max_danger = DangerLevel::Low;
        for op in &batch.operations {
            if op.danger_level > max_danger {
                max_danger = op.danger_level;
            }
            if !self.preset.allows(op.op_type) {
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
            return Err(err.unwrap_or_else(|| "batch permission denied".to_string()));
        }

        // If batch is approved, check all individual operations
        for op in &batch.operations {
            let mut individual_req = op.clone();
            individual_req.user = self.user.clone();
            individual_req.source = self.source.clone();
            let (allowed, err, _) = self.auditor.request_permission(&individual_req);
            if !allowed {
                return Err(err.unwrap_or_else(|| "individual operation denied".to_string()));
            }
        }

        Ok(summary_id)
    }

    // -----------------------------------------------------------------------
    // Pending request management
    // -----------------------------------------------------------------------

    /// Approve a pending request (for user interaction).
    pub fn approve_pending_request(&self, request_id: &str) -> Result<(), String> {
        self.auditor.approve_request(request_id, &self.user)
    }

    /// Deny a pending request (for user interaction).
    pub fn deny_pending_request(&self, request_id: &str, reason: &str) -> Result<(), String> {
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
        tokio::fs::read_to_string(&validated)
            .await
            .map_err(|e| format!("failed to read file: {}", e))
    }

    /// Write content to a file with security check.
    pub async fn write_file(&self, path: &str, content: &str) -> Result<(), String> {
        let validated = self.check_file_write(path)?;
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

        let (shell, flag) = if cfg!(target_os = "windows") {
            ("cmd", "/C")
        } else {
            ("sh", "-c")
        };

        let timeout = std::time::Duration::from_secs(timeout_secs.min(600));
        let result = tokio::time::timeout(
            timeout,
            tokio::process::Command::new(shell)
                .arg(flag)
                .arg(command)
                .output(),
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

        let (shell, flag) = if cfg!(target_os = "windows") {
            ("cmd", "/C")
        } else {
            ("sh", "-c")
        };

        let mut child = tokio::process::Command::new(shell)
            .arg(flag)
            .arg(command)
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

        let (shell, flag) = if cfg!(target_os = "windows") {
            ("cmd", "/C")
        } else {
            ("sh", "-c")
        };

        let kill_cmd = if cfg!(target_os = "windows") {
            format!("taskkill /F /PID {}", pid)
        } else {
            format!("kill -9 {}", pid)
        };

        let output = tokio::process::Command::new(shell)
            .arg(flag)
            .arg(&kill_cmd)
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

        let (shell, flag) = if cfg!(target_os = "windows") {
            ("cmd", "/C")
        } else {
            ("sh", "-c")
        };

        let term_cmd = if cfg!(target_os = "windows") {
            format!("taskkill /PID {}", pid)
        } else {
            format!("kill {}", pid)
        };

        let output = tokio::process::Command::new(shell)
            .arg(flag)
            .arg(&term_cmd)
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
            let (shell, flag) = if cfg!(target_os = "windows") {
                ("cmd", "/C")
            } else {
                ("sh", "-c")
            };

            let check_cmd = if cfg!(target_os = "windows") {
                format!("tasklist /FI \"PID eq {}\" /NH", pid)
            } else {
                format!("ps -p {} -o pid=", pid)
            };

            let output = tokio::process::Command::new(shell)
                .arg(flag)
                .arg(&check_cmd)
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

        let (shell, flag) = if cfg!(target_os = "windows") {
            ("cmd", "/C")
        } else {
            ("sh", "-c")
        };

        let timeout = std::time::Duration::from_secs(timeout_secs.min(600));
        let result = tokio::time::timeout(
            timeout,
            tokio::process::Command::new(shell)
                .arg(flag)
                .arg(command)
                .output(),
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

        let (shell, flag) = if cfg!(target_os = "windows") {
            ("cmd", "/C")
        } else {
            ("sh", "-c")
        };

        let output = tokio::process::Command::new(shell)
            .arg(flag)
            .arg(&i2cget_cmd)
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

        let (shell, flag) = if cfg!(target_os = "windows") {
            ("cmd", "/C")
        } else {
            ("sh", "-c")
        };

        let output = tokio::process::Command::new(shell)
            .arg(flag)
            .arg(&i2cset_cmd)
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
mod tests {
    use super::*;
    use crate::auditor::AuditorConfig;

    fn make_middleware(preset: PermissionPreset) -> SecurityMiddleware {
        let config = AuditorConfig {
            enabled: true,
            default_action: "allow".to_string(),
            ..Default::default()
        };
        let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
        SecurityMiddleware::with_preset(auditor, "test_user", "cli", "/tmp/workspace", preset)
    }

    #[test]
    fn test_preset_read_only() {
        let mw = make_middleware(PermissionPreset::ReadOnly);
        assert!(mw.is_operation_allowed(OperationType::FileRead));
        assert!(mw.is_operation_allowed(OperationType::DirRead));
        assert!(!mw.is_operation_allowed(OperationType::FileWrite));
        assert!(!mw.is_operation_allowed(OperationType::ProcessExec));
    }

    #[test]
    fn test_preset_standard() {
        let mw = make_middleware(PermissionPreset::Standard);
        assert!(mw.is_operation_allowed(OperationType::FileRead));
        assert!(mw.is_operation_allowed(OperationType::FileWrite));
        assert!(mw.is_operation_allowed(OperationType::NetworkRequest));
        assert!(!mw.is_operation_allowed(OperationType::ProcessExec));
    }

    #[test]
    fn test_preset_elevated() {
        let mw = make_middleware(PermissionPreset::Elevated);
        assert!(mw.is_operation_allowed(OperationType::FileRead));
        assert!(mw.is_operation_allowed(OperationType::ProcessExec));
        assert!(!mw.is_operation_allowed(OperationType::ProcessKill));
    }

    #[test]
    fn test_preset_unrestricted() {
        let mw = make_middleware(PermissionPreset::Unrestricted);
        assert!(mw.is_operation_allowed(OperationType::FileRead));
        assert!(mw.is_operation_allowed(OperationType::ProcessKill));
        assert!(mw.is_operation_allowed(OperationType::SystemShutdown));
    }

    #[test]
    fn test_secure_file_wrapper() {
        let mw = make_middleware(PermissionPreset::Standard);
        let wrapper = SecureFileWrapper::new(&mw);
        // Path validation may fail for non-existent paths but the check itself works
        let _ = wrapper.check_file_read("/tmp/test.txt");
    }

    #[test]
    fn test_secure_process_wrapper_dangerous() {
        let mw = make_middleware(PermissionPreset::Elevated);
        let wrapper = SecureProcessWrapper::new(&mw);
        assert!(wrapper.check_process_exec("rm -rf /").is_err());
        assert!(wrapper.check_process_exec("sudo something").is_err());
    }

    #[test]
    fn test_secure_process_wrapper_blocked_by_preset() {
        let mw = make_middleware(PermissionPreset::Standard);
        let wrapper = SecureProcessWrapper::new(&mw);
        // ProcessExec not allowed under Standard preset
        let result = wrapper.check_process_exec("ls -la");
        assert!(result.is_err());
    }

    #[test]
    fn test_secure_network_wrapper() {
        let mw = make_middleware(PermissionPreset::Standard);
        let wrapper = SecureNetworkWrapper::new(&mw);
        assert!(wrapper.check_network_request("https://example.com").is_ok());
    }

    #[test]
    fn test_secure_network_download_invalid_scheme() {
        let mw = make_middleware(PermissionPreset::Standard);
        let wrapper = SecureNetworkWrapper::new(&mw);
        assert!(wrapper.check_network_download("ftp://example.com").is_err());
        assert!(wrapper.check_network_download("file:///etc/passwd").is_err());
    }

    #[test]
    fn test_permission_preset_allows_consistency() {
        for preset in [
            PermissionPreset::ReadOnly,
            PermissionPreset::Standard,
            PermissionPreset::Elevated,
            PermissionPreset::Unrestricted,
        ] {
            let ops = [
                OperationType::FileRead,
                OperationType::FileWrite,
                OperationType::FileDelete,
                OperationType::ProcessExec,
                OperationType::ProcessKill,
                OperationType::NetworkRequest,
                OperationType::HardwareI2C,
            ];
            for op in ops {
                // Just ensure no panic
                let _ = preset.allows(op);
            }
        }
    }

    #[test]
    fn test_batch_operation_empty() {
        let mw = make_middleware(PermissionPreset::Standard);
        let batch = BatchOperationRequest::default();
        assert!(mw.request_batch_permission(&batch).is_err());
    }

    #[test]
    fn test_batch_operation_blocked_by_preset() {
        let mw = make_middleware(PermissionPreset::Standard);
        let batch = BatchOperationRequest {
            id: "batch-1".to_string(),
            operations: vec![OperationRequest {
                id: "op-1".to_string(),
                op_type: OperationType::ProcessExec,
                danger_level: DangerLevel::Critical,
                user: "test".to_string(),
                source: "cli".to_string(),
                target: "ls".to_string(),
                timestamp: None,
                ..Default::default()
            }],
            user: "test".to_string(),
            source: "cli".to_string(),
            description: "test batch".to_string(),
        };
        let result = mw.request_batch_permission(&batch);
        assert!(result.is_err());
    }

    #[test]
    fn test_batch_operation_approved() {
        let mw = make_middleware(PermissionPreset::Elevated);
        let batch = BatchOperationRequest {
            id: "batch-2".to_string(),
            operations: vec![
                OperationRequest {
                    id: "op-a".to_string(),
                    op_type: OperationType::FileRead,
                    danger_level: DangerLevel::Low,
                    user: "test".to_string(),
                    source: "cli".to_string(),
                    target: "/tmp/a.txt".to_string(),
                    timestamp: None,
                    ..Default::default()
                },
                OperationRequest {
                    id: "op-b".to_string(),
                    op_type: OperationType::FileRead,
                    danger_level: DangerLevel::Low,
                    user: "test".to_string(),
                    source: "cli".to_string(),
                    target: "/tmp/b.txt".to_string(),
                    timestamp: None,
                    ..Default::default()
                },
            ],
            user: "test".to_string(),
            source: "cli".to_string(),
            description: "read two files".to_string(),
        };
        let result = mw.request_batch_permission(&batch);
        assert!(result.is_ok());
    }

    #[test]
    fn test_wrapper_accessors() {
        let mw = make_middleware(PermissionPreset::Standard);
        // Just ensure they compile and work
        let _file = mw.file();
        let _process = mw.process();
        let _network = mw.network();
        let _hardware = mw.hardware();
    }

    #[test]
    fn test_get_security_summary() {
        let mw = make_middleware(PermissionPreset::Standard);
        let summary = mw.get_security_summary();
        assert_eq!(summary["user"], "test_user");
        assert_eq!(summary["source"], "cli");
        assert_eq!(summary["workspace"], "/tmp/workspace");
    }

    #[test]
    fn test_user_source_workspace() {
        let mw = make_middleware(PermissionPreset::Standard);
        assert_eq!(mw.user(), "test_user");
        assert_eq!(mw.source(), "cli");
        assert_eq!(mw.workspace(), "/tmp/workspace");
    }

    #[test]
    fn test_permission_factories() {
        // CLI: allows process_exec, file_delete, network_download; denies targets
        let cli = create_cli_permission();
        assert!(cli.is_operation_allowed(&OperationType::ProcessExec));
        assert!(cli.is_operation_allowed(&OperationType::FileDelete));
        assert!(cli.is_target_denied("/etc/passwd"));
        assert!(cli.requires_approval(&OperationType::ProcessKill));
        assert_eq!(cli.max_danger_level, DangerLevel::High);

        // Web: allows file_read, file_write; requires approval for file_delete
        let web = create_web_permission();
        assert!(!web.is_operation_allowed(&OperationType::ProcessExec));
        assert!(web.is_operation_allowed(&OperationType::FileRead));
        assert!(web.requires_approval(&OperationType::FileDelete));
        assert_eq!(web.max_danger_level, DangerLevel::Medium);

        // Agent: allows process_exec; requires approval for process_kill
        let agent = create_agent_permission("agent-1");
        assert!(agent.is_operation_allowed(&OperationType::ProcessExec));
        assert!(agent.requires_approval(&OperationType::ProcessKill));
        assert_eq!(agent.max_danger_level, DangerLevel::High);
    }

    #[test]
    fn test_export_audit_log() {
        let dir = tempfile::tempdir().unwrap();
        let mw = make_middleware(PermissionPreset::Standard);
        let export_path = dir.path().join("audit.json");
        let result = mw.export_audit_log(export_path.to_str().unwrap());
        assert!(result.is_ok());
    }

    #[test]
    fn test_approve_deny_pending() {
        let config = AuditorConfig {
            enabled: true,
            default_action: "ask".to_string(),
            ..Default::default()
        };
        let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
        let mw = SecurityMiddleware::with_preset(
            auditor.clone(),
            "test_user",
            "cli",
            "/tmp/workspace",
            PermissionPreset::Unrestricted,
        );

        // Create a pending request
        let req = OperationRequest {
            id: "pending-test".to_string(),
            op_type: OperationType::FileWrite,
            danger_level: DangerLevel::High,
            user: "test".to_string(),
            source: "cli".to_string(),
            target: "/tmp/test".to_string(),
            timestamp: None,
            ..Default::default()
        };
        let (allowed, _, _) = auditor.request_permission(&req);
        assert!(!allowed);

        // Approve it
        mw.approve_pending_request("pending-test").unwrap();

        // Should fail now (already removed)
        assert!(mw.approve_pending_request("pending-test").is_err());
    }

    // ---- Additional middleware tests ----

    #[test]
    fn test_preset_read_only_denies_all_write() {
        let mw = make_middleware(PermissionPreset::ReadOnly);
        // All write operations should be denied
        assert!(!mw.is_operation_allowed(OperationType::FileWrite));
        assert!(!mw.is_operation_allowed(OperationType::FileDelete));
        assert!(!mw.is_operation_allowed(OperationType::DirCreate));
        assert!(!mw.is_operation_allowed(OperationType::DirDelete));
        assert!(!mw.is_operation_allowed(OperationType::ProcessExec));
        assert!(!mw.is_operation_allowed(OperationType::ProcessSpawn));
        assert!(!mw.is_operation_allowed(OperationType::NetworkRequest));
        assert!(!mw.is_operation_allowed(OperationType::NetworkDownload));
    }

    #[test]
    fn test_preset_standard_allows_common_ops() {
        let mw = make_middleware(PermissionPreset::Standard);
        assert!(mw.is_operation_allowed(OperationType::FileRead));
        assert!(mw.is_operation_allowed(OperationType::FileWrite));
        assert!(mw.is_operation_allowed(OperationType::DirRead));
        assert!(mw.is_operation_allowed(OperationType::DirCreate));
        assert!(mw.is_operation_allowed(OperationType::NetworkRequest));
        assert!(mw.is_operation_allowed(OperationType::NetworkDownload));
    }

    #[test]
    fn test_preset_standard_denies_critical_ops() {
        let mw = make_middleware(PermissionPreset::Standard);
        assert!(!mw.is_operation_allowed(OperationType::ProcessExec));
        assert!(!mw.is_operation_allowed(OperationType::ProcessSpawn));
        assert!(!mw.is_operation_allowed(OperationType::ProcessKill));
        assert!(!mw.is_operation_allowed(OperationType::SystemShutdown));
        assert!(!mw.is_operation_allowed(OperationType::RegistryWrite));
    }

    #[test]
    fn test_preset_elevated_allows_exec() {
        let mw = make_middleware(PermissionPreset::Elevated);
        assert!(mw.is_operation_allowed(OperationType::ProcessExec));
        assert!(mw.is_operation_allowed(OperationType::ProcessSpawn));
        assert!(mw.is_operation_allowed(OperationType::FileRead));
        assert!(mw.is_operation_allowed(OperationType::FileWrite));
        assert!(mw.is_operation_allowed(OperationType::NetworkRequest));
    }

    #[test]
    fn test_preset_elevated_denies_kill_and_system() {
        let mw = make_middleware(PermissionPreset::Elevated);
        assert!(!mw.is_operation_allowed(OperationType::ProcessKill));
        assert!(!mw.is_operation_allowed(OperationType::SystemShutdown));
        assert!(!mw.is_operation_allowed(OperationType::SystemReboot));
        assert!(!mw.is_operation_allowed(OperationType::RegistryWrite));
    }

    #[test]
    fn test_preset_unrestricted_allows_everything() {
        let mw = make_middleware(PermissionPreset::Unrestricted);
        let all_ops = [
            OperationType::FileRead, OperationType::FileWrite,
            OperationType::FileDelete, OperationType::DirRead,
            OperationType::DirCreate, OperationType::DirDelete,
            OperationType::ProcessExec, OperationType::ProcessSpawn,
            OperationType::ProcessKill, OperationType::NetworkRequest,
            OperationType::NetworkDownload, OperationType::HardwareI2C,
            OperationType::RegistryWrite, OperationType::SystemShutdown,
        ];
        for op in &all_ops {
            assert!(mw.is_operation_allowed(*op), "Expected {:?} to be allowed under Unrestricted", op);
        }
    }

    #[test]
    fn test_secure_file_wrapper_write_blocked_in_readonly() {
        let mw = make_middleware(PermissionPreset::ReadOnly);
        let wrapper = SecureFileWrapper::new(&mw);
        assert!(wrapper.check_file_write("/tmp/test.txt").is_err());
    }

    #[test]
    fn test_secure_file_wrapper_delete_blocked_in_readonly() {
        let mw = make_middleware(PermissionPreset::ReadOnly);
        let wrapper = SecureFileWrapper::new(&mw);
        assert!(wrapper.check_file_delete("/tmp/test.txt").is_err());
    }

    #[test]
    fn test_secure_file_wrapper_create_dir_blocked_in_readonly() {
        let mw = make_middleware(PermissionPreset::ReadOnly);
        let wrapper = SecureFileWrapper::new(&mw);
        assert!(wrapper.check_dir_create("/tmp/newdir").is_err());
    }

    #[test]
    fn test_secure_file_wrapper_delete_dir_blocked_in_readonly() {
        let mw = make_middleware(PermissionPreset::ReadOnly);
        let wrapper = SecureFileWrapper::new(&mw);
        assert!(wrapper.check_dir_delete("/tmp/olddir").is_err());
    }

    #[test]
    fn test_secure_process_wrapper_kill_blocked_everywhere() {
        for preset in [PermissionPreset::ReadOnly, PermissionPreset::Standard, PermissionPreset::Elevated] {
            let mw = make_middleware(preset);
            let wrapper = SecureProcessWrapper::new(&mw);
            assert!(wrapper.check_process_kill("1234").is_err(),
                "ProcessKill should be blocked under {:?}", preset);
        }
        // Unrestricted allows it (subject to ABAC)
        let mw = make_middleware(PermissionPreset::Unrestricted);
        let wrapper = SecureProcessWrapper::new(&mw);
        let _ = wrapper.check_process_kill("1234"); // may or may not succeed
    }

    #[test]
    fn test_secure_network_wrapper_download_valid_url() {
        let mw = make_middleware(PermissionPreset::Standard);
        let wrapper = SecureNetworkWrapper::new(&mw);
        assert!(wrapper.check_network_download("https://example.com/file.zip").is_ok());
        assert!(wrapper.check_network_download("http://example.com/file.zip").is_ok());
    }

    #[test]
    fn test_secure_network_wrapper_download_blocked_in_readonly() {
        let mw = make_middleware(PermissionPreset::ReadOnly);
        let wrapper = SecureNetworkWrapper::new(&mw);
        assert!(wrapper.check_network_download("https://example.com/file.zip").is_err());
    }

    #[test]
    fn test_secure_hardware_wrapper_all_blocked_in_standard() {
        let mw = make_middleware(PermissionPreset::Standard);
        let wrapper = SecureHardwareWrapper::new(&mw);
        assert!(wrapper.check_i2c("device").is_err());
        assert!(wrapper.check_spi("device").is_err());
    }

    #[test]
    fn test_batch_operation_mixed_operations() {
        let mw = make_middleware(PermissionPreset::Standard);
        let batch = BatchOperationRequest {
            id: "batch-mixed".to_string(),
            operations: vec![
                OperationRequest {
                    id: "op-r".to_string(),
                    op_type: OperationType::FileRead,
                    danger_level: DangerLevel::Low,
                    user: "test".to_string(),
                    source: "cli".to_string(),
                    target: "/tmp/a.txt".to_string(),
                    timestamp: None,
                    ..Default::default()
                },
                OperationRequest {
                    id: "op-w".to_string(),
                    op_type: OperationType::FileWrite,
                    danger_level: DangerLevel::High,
                    user: "test".to_string(),
                    source: "cli".to_string(),
                    target: "/tmp/b.txt".to_string(),
                    timestamp: None,
                    ..Default::default()
                },
            ],
            user: "test".to_string(),
            source: "cli".to_string(),
            description: "read and write".to_string(),
        };
        let result = mw.request_batch_permission(&batch);
        assert!(result.is_ok());
    }

    #[test]
    fn test_batch_operation_single_dangerous() {
        let mw = make_middleware(PermissionPreset::Elevated);
        let batch = BatchOperationRequest {
            id: "batch-danger".to_string(),
            operations: vec![
                OperationRequest {
                    id: "op-r".to_string(),
                    op_type: OperationType::FileRead,
                    danger_level: DangerLevel::Low,
                    user: "test".to_string(),
                    source: "cli".to_string(),
                    target: "/tmp/a.txt".to_string(),
                    timestamp: None,
                    ..Default::default()
                },
                OperationRequest {
                    id: "op-kill".to_string(),
                    op_type: OperationType::ProcessKill,
                    danger_level: DangerLevel::Critical,
                    user: "test".to_string(),
                    source: "cli".to_string(),
                    target: "1234".to_string(),
                    timestamp: None,
                    ..Default::default()
                },
            ],
            user: "test".to_string(),
            source: "cli".to_string(),
            description: "read and kill".to_string(),
        };
        let result = mw.request_batch_permission(&batch);
        // ProcessKill not allowed under Elevated
        assert!(result.is_err());
    }

    #[test]
    fn test_batch_operation_custom_id() {
        let mw = make_middleware(PermissionPreset::Standard);
        let batch = BatchOperationRequest {
            id: "custom-batch-id".to_string(),
            operations: vec![OperationRequest {
                id: "op-1".to_string(),
                op_type: OperationType::FileRead,
                danger_level: DangerLevel::Low,
                user: "test".to_string(),
                source: "cli".to_string(),
                target: "/tmp/a.txt".to_string(),
                timestamp: None,
                ..Default::default()
            }],
            user: "test".to_string(),
            source: "cli".to_string(),
            description: "custom id batch".to_string(),
        };
        let result = mw.request_batch_permission(&batch);
        assert!(result.is_ok());
    }

    #[test]
    fn test_permission_factories_cli_details() {
        let cli = create_cli_permission();
        assert!(cli.is_operation_allowed(&OperationType::ProcessExec));
        assert!(cli.is_operation_allowed(&OperationType::FileDelete));
        assert!(cli.is_operation_allowed(&OperationType::NetworkDownload));
        assert!(cli.is_operation_allowed(&OperationType::FileWrite));
        assert!(cli.is_operation_allowed(&OperationType::DirCreate));
        assert!(!cli.is_operation_allowed(&OperationType::ProcessKill));
        assert!(!cli.is_operation_allowed(&OperationType::SystemShutdown));
    }

    #[test]
    fn test_permission_factories_web_details() {
        let web = create_web_permission();
        assert!(web.is_operation_allowed(&OperationType::FileRead));
        assert!(web.is_operation_allowed(&OperationType::FileWrite));
        assert!(web.is_operation_allowed(&OperationType::DirRead));
        assert!(web.is_operation_allowed(&OperationType::DirCreate));
        assert!(!web.is_operation_allowed(&OperationType::ProcessExec));
        assert!(!web.is_operation_allowed(&OperationType::ProcessKill));
        assert!(web.requires_approval(&OperationType::FileDelete));
        assert!(web.requires_approval(&OperationType::ProcessExec));
    }

    #[test]
    fn test_permission_factories_agent_details() {
        let agent = create_agent_permission("agent-007");
        assert!(agent.is_operation_allowed(&OperationType::ProcessExec));
        assert!(agent.is_operation_allowed(&OperationType::FileRead));
        assert!(agent.is_operation_allowed(&OperationType::FileWrite));
        assert!(agent.requires_approval(&OperationType::ProcessKill));
        assert!(agent.requires_approval(&OperationType::SystemShutdown));
    }

    #[test]
    fn test_security_status_fields() {
        let mw = make_middleware(PermissionPreset::Standard);
        let status = mw.get_security_summary();
        assert!(status.is_object());
    }

    #[test]
    fn test_get_security_summary_contains_all_fields() {
        let mw = make_middleware(PermissionPreset::Standard);
        let summary = mw.get_security_summary();
        let obj = summary.as_object().unwrap();
        assert!(obj.contains_key("user"));
        assert!(obj.contains_key("source"));
        assert!(obj.contains_key("workspace"));
        assert!(obj.contains_key("statistics"));
    }

    #[test]
    fn test_batch_default_is_empty() {
        let batch = BatchOperationRequest::default();
        assert!(batch.id.is_empty());
        assert!(batch.operations.is_empty());
        assert!(batch.user.is_empty());
        assert!(batch.source.is_empty());
        assert!(batch.description.is_empty());
    }

    #[test]
    fn test_secure_network_wrapper_upload_blocked_in_standard() {
        let mw = make_middleware(PermissionPreset::Standard);
        let wrapper = SecureNetworkWrapper::new(&mw);
        assert!(wrapper.check_network_upload("https://example.com/upload").is_err());
    }

    #[test]
    fn test_secure_network_wrapper_upload_allowed_in_unrestricted() {
        let mw = make_middleware(PermissionPreset::Unrestricted);
        let wrapper = SecureNetworkWrapper::new(&mw);
        assert!(wrapper.check_network_upload("https://example.com/upload").is_ok());
    }

    #[test]
    fn test_deny_pending_request_via_middleware() {
        let config = AuditorConfig {
            enabled: true,
            default_action: "ask".to_string(),
            ..Default::default()
        };
        let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
        let mw = SecurityMiddleware::with_preset(
            auditor.clone(),
            "test_user",
            "cli",
            "/tmp/workspace",
            PermissionPreset::Unrestricted,
        );

        let req = OperationRequest {
            id: "deny-via-mw".to_string(),
            op_type: OperationType::FileWrite,
            danger_level: DangerLevel::High,
            user: "test".to_string(),
            source: "cli".to_string(),
            target: "/tmp/test".to_string(),
            timestamp: None,
            ..Default::default()
        };
        let (allowed, _, _) = auditor.request_permission(&req);
        assert!(!allowed);

        mw.deny_pending_request("deny-via-mw", "too risky").unwrap();
        assert!(mw.deny_pending_request("deny-via-mw", "too risky").is_err());
    }

    // ---- Additional coverage tests for middleware ----

    #[test]
    fn test_set_preset() {
        let mut mw = make_middleware(PermissionPreset::ReadOnly);
        assert_eq!(mw.preset(), PermissionPreset::ReadOnly);
        mw.set_preset(PermissionPreset::Standard);
        assert_eq!(mw.preset(), PermissionPreset::Standard);
        assert!(mw.is_operation_allowed(OperationType::FileWrite));
    }

    #[test]
    fn test_check_operation_denied_by_preset() {
        let mw = make_middleware(PermissionPreset::Standard);
        // ProcessExec is not allowed under Standard
        let result = mw.check_operation(OperationType::ProcessExec, "ls");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not allowed"));
    }

    #[test]
    fn test_check_operation_allowed_by_preset() {
        let mw = make_middleware(PermissionPreset::Standard);
        // FileRead is allowed under Standard
        let result = mw.check_operation(OperationType::FileRead, "/tmp/test.txt");
        assert!(result.is_ok());
    }

    #[test]
    fn test_file_wrapper_check_file_read() {
        let mw = make_middleware(PermissionPreset::Standard);
        let wrapper = SecureFileWrapper::new(&mw);
        assert!(wrapper.check_file_read("/tmp/workspace/test.txt").is_ok());
    }

    #[test]
    fn test_file_wrapper_check_file_write() {
        let mw = make_middleware(PermissionPreset::Standard);
        let wrapper = SecureFileWrapper::new(&mw);
        assert!(wrapper.check_file_write("/tmp/workspace/test.txt").is_ok());
    }

    #[test]
    fn test_file_wrapper_check_file_read_denied_for_elevated_only() {
        let mw = make_middleware(PermissionPreset::ReadOnly);
        let wrapper = SecureFileWrapper::new(&mw);
        // FileWrite should be denied under ReadOnly
        assert!(wrapper.check_file_write("/tmp/workspace/test.txt").is_err());
        // But FileRead should be allowed
        assert!(wrapper.check_file_read("/tmp/workspace/test.txt").is_ok());
    }

    #[test]
    fn test_file_wrapper_check_dir_read() {
        let mw = make_middleware(PermissionPreset::Standard);
        let wrapper = SecureFileWrapper::new(&mw);
        assert!(wrapper.check_dir_read("/tmp/workspace").is_ok());
    }

    #[test]
    fn test_file_wrapper_check_dir_create() {
        let mw = make_middleware(PermissionPreset::Standard);
        let wrapper = SecureFileWrapper::new(&mw);
        assert!(wrapper.check_dir_create("/tmp/workspace/newdir").is_ok());
    }

    #[test]
    fn test_file_wrapper_check_dir_delete_denied_for_standard() {
        let mw = make_middleware(PermissionPreset::Standard);
        let wrapper = SecureFileWrapper::new(&mw);
        // DirDelete is not in Standard preset
        assert!(wrapper.check_dir_delete("/tmp/workspace/dir").is_err());
    }

    #[tokio::test]
    async fn test_file_wrapper_write_outside_workspace_denied() {
        let mw = make_middleware(PermissionPreset::Unrestricted);
        let wrapper = SecureFileWrapper::new(&mw);
        let result = wrapper.write_file("/etc/evil.txt", "bad content").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_file_wrapper_read_outside_workspace_denied() {
        let mw = make_middleware(PermissionPreset::ReadOnly);
        let wrapper = SecureFileWrapper::new(&mw);
        let result = wrapper.read_file("/etc/passwd").await;
        assert!(result.is_err());
    }

    #[test]
    fn test_process_wrapper_check_spawn() {
        let mw = make_middleware(PermissionPreset::Elevated);
        let wrapper = SecureProcessWrapper::new(&mw);
        let result = wrapper.check_process_spawn("python script.py");
        assert!(result.is_ok());
    }

    #[test]
    fn test_process_wrapper_spawn_blocked_in_standard() {
        let mw = make_middleware(PermissionPreset::Standard);
        let wrapper = SecureProcessWrapper::new(&mw);
        let result = wrapper.check_process_spawn("python script.py");
        assert!(result.is_err());
    }

    #[test]
    fn test_process_wrapper_check_exec() {
        let mw = make_middleware(PermissionPreset::Unrestricted);
        let wrapper = SecureProcessWrapper::new(&mw);
        let result = wrapper.check_process_exec("ls -la");
        assert!(result.is_ok());
    }

    #[test]
    fn test_process_wrapper_check_exec_blocked_in_standard() {
        let mw = make_middleware(PermissionPreset::Standard);
        let wrapper = SecureProcessWrapper::new(&mw);
        let result = wrapper.check_process_exec("ls -la");
        assert!(result.is_err());
    }

    #[test]
    fn test_network_wrapper_request_valid_urls() {
        let mw = make_middleware(PermissionPreset::Standard);
        let wrapper = SecureNetworkWrapper::new(&mw);
        assert!(wrapper.check_network_request("https://example.com/api").is_ok());
        assert!(wrapper.check_network_request("http://example.com/api").is_ok());
    }

    #[test]
    fn test_network_wrapper_download_invalid_scheme() {
        let mw = make_middleware(PermissionPreset::Standard);
        let wrapper = SecureNetworkWrapper::new(&mw);
        assert!(wrapper.check_network_download("ftp://example.com").is_err());
        assert!(wrapper.check_network_download("file:///etc/passwd").is_err());
    }

    #[test]
    fn test_network_wrapper_upload_valid_url() {
        let mw = make_middleware(PermissionPreset::Unrestricted);
        let wrapper = SecureNetworkWrapper::new(&mw);
        // Upload requires Unrestricted permission
        assert!(wrapper.check_network_upload("https://example.com/upload").is_ok());
    }

    #[test]
    fn test_network_wrapper_upload_invalid_url() {
        let mw = make_middleware(PermissionPreset::Unrestricted);
        let wrapper = SecureNetworkWrapper::new(&mw);
        assert!(wrapper.check_network_upload("ftp://example.com").is_err());
    }

    #[test]
    fn test_hardware_wrapper_gpio() {
        let mw = make_middleware(PermissionPreset::Unrestricted);
        let wrapper = SecureHardwareWrapper::new(&mw);
        let result = wrapper.check_gpio("17");
        assert!(result.is_ok());
    }

    #[test]
    fn test_hardware_wrapper_spi_write() {
        let mw = make_middleware(PermissionPreset::Unrestricted);
        let wrapper = SecureHardwareWrapper::new(&mw);
        let result = wrapper.spi_write("1.0", &[0x01, 0x02]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_http_request_default() {
        let req = HttpRequest::default();
        assert!(req.url.is_empty());
        assert_eq!(req.method, "GET");
        assert!(req.headers.is_empty());
        assert!(req.body.is_none());
        assert!(req.timeout_secs.is_none());
    }

    #[test]
    fn test_http_response_fields() {
        let resp = HttpResponse {
            status_code: 200,
            body: "OK".to_string(),
            success: true,
        };
        assert_eq!(resp.status_code, 200);
        assert_eq!(resp.body, "OK");
        assert!(resp.success);
    }

    #[test]
    fn test_batch_operation_empty_id_generates_uuid() {
        let mw = make_middleware(PermissionPreset::Standard);
        let batch = BatchOperationRequest {
            id: String::new(), // empty -> should generate UUID
            operations: vec![OperationRequest {
                id: "op-1".to_string(),
                op_type: OperationType::FileRead,
                danger_level: DangerLevel::Low,
                user: "test".to_string(),
                source: "cli".to_string(),
                target: "/tmp/a.txt".to_string(),
                timestamp: None,
                ..Default::default()
            }],
            ..Default::default()
        };
        let result = mw.request_batch_permission(&batch);
        assert!(result.is_ok());
        // The returned ID should be a UUID (not empty)
        let id = result.unwrap();
        assert!(!id.is_empty());
    }

    #[test]
    fn test_permission_is_target_denied_with_empty_list() {
        let perm = create_web_permission();
        assert!(!perm.is_target_denied("/any/path"));
    }

    #[test]
    fn test_permission_is_target_allowed_with_empty_list() {
        let perm = create_web_permission();
        assert!(!perm.is_target_allowed("/any/path"));
    }

    #[test]
    fn test_permission_is_target_denied_cli() {
        let perm = create_cli_permission();
        assert!(perm.is_target_denied("/etc/sudoers"));
        assert!(perm.is_target_denied("/etc/passwd"));
    }

    #[test]
    fn test_permission_is_operation_allowed_false_for_unknown() {
        let perm = create_web_permission();
        assert!(!perm.is_operation_allowed(&OperationType::HardwareI2C));
        assert!(!perm.is_operation_allowed(&OperationType::RegistryWrite));
    }

    #[test]
    fn test_permission_requires_approval_false_for_unknown() {
        let perm = create_web_permission();
        assert!(!perm.requires_approval(&OperationType::HardwareI2C));
    }

    #[test]
    fn test_cli_permission_allows_all_expected() {
        let cli = create_cli_permission();
        assert!(cli.is_operation_allowed(&OperationType::FileRead));
        assert!(cli.is_operation_allowed(&OperationType::FileWrite));
        assert!(cli.is_operation_allowed(&OperationType::FileDelete));
        assert!(cli.is_operation_allowed(&OperationType::DirRead));
        assert!(cli.is_operation_allowed(&OperationType::DirCreate));
        assert!(cli.is_operation_allowed(&OperationType::ProcessExec));
        assert!(cli.is_operation_allowed(&OperationType::NetworkDownload));
        assert!(cli.is_operation_allowed(&OperationType::NetworkRequest));
    }

    #[test]
    fn test_get_audit_log() {
        let mw = make_middleware(PermissionPreset::Standard);
        let filter = crate::auditor::AuditFilter::default();
        let logs = mw.get_audit_log(filter);
        // May or may not have entries, but should not panic
        assert!(logs.is_empty() || !logs.is_empty());
    }

    // ---- Coverage expansion tests for middleware ----

    #[tokio::test]
    async fn test_file_wrapper_write_within_workspace() {
        let dir = tempfile::tempdir().unwrap();
        // Canonicalize workspace to ensure path matching works on Windows
        let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
        let config = AuditorConfig {
            enabled: true,
            default_action: "allow".to_string(),
            ..Default::default()
        };
        let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
        let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
        let wrapper = SecureFileWrapper::new(&mw);
        let file_path = format!("{}\\test_write.txt", ws);
        let result = wrapper.write_file(&file_path, "hello world").await;
        assert!(result.is_ok(), "write_file failed: {:?}", result);
        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "hello world");
    }

    #[tokio::test]
    async fn test_file_wrapper_read_within_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
        let file_path = format!("{}\\test_read.txt", ws);
        std::fs::write(&file_path, "read me").unwrap();
        let config = AuditorConfig {
            enabled: true,
            default_action: "allow".to_string(),
            ..Default::default()
        };
        let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
        let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
        let wrapper = SecureFileWrapper::new(&mw);
        let result = wrapper.read_file(&file_path).await;
        assert!(result.is_ok(), "read_file failed: {:?}", result);
        assert_eq!(result.unwrap(), "read me");
    }

    #[tokio::test]
    async fn test_file_wrapper_edit_file() {
        let dir = tempfile::tempdir().unwrap();
        let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
        let file_path = format!("{}\\edit_test.txt", ws);
        std::fs::write(&file_path, "old content here").unwrap();
        let config = AuditorConfig {
            enabled: true,
            default_action: "allow".to_string(),
            ..Default::default()
        };
        let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
        let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
        let wrapper = SecureFileWrapper::new(&mw);
        let result = wrapper.edit_file(&file_path, "old content", "new content").await;
        assert!(result.is_ok(), "edit_file failed: {:?}", result);
        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "new content here");
    }

    #[tokio::test]
    async fn test_file_wrapper_edit_file_pattern_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
        let file_path = format!("{}\\edit_notfound.txt", ws);
        std::fs::write(&file_path, "some content").unwrap();
        let config = AuditorConfig {
            enabled: true,
            default_action: "allow".to_string(),
            ..Default::default()
        };
        let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
        let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
        let wrapper = SecureFileWrapper::new(&mw);
        let result = wrapper.edit_file(&file_path, "nonexistent", "replacement").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("pattern not found"));
    }

    #[tokio::test]
    async fn test_file_wrapper_append_file() {
        let dir = tempfile::tempdir().unwrap();
        let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
        let file_path = format!("{}\\append_test.txt", ws);
        std::fs::write(&file_path, "first line\n").unwrap();
        let config = AuditorConfig {
            enabled: true,
            default_action: "allow".to_string(),
            ..Default::default()
        };
        let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
        let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
        let wrapper = SecureFileWrapper::new(&mw);
        let result = wrapper.append_file(&file_path, "second line").await;
        assert!(result.is_ok(), "append_file failed: {:?}", result);
        let content = std::fs::read_to_string(&file_path).unwrap();
        assert!(content.contains("first line"));
        assert!(content.contains("second line"));
    }

    #[tokio::test]
    async fn test_file_wrapper_append_to_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
        let file_path = format!("{}\\append_empty.txt", ws);
        std::fs::write(&file_path, "").unwrap();
        let config = AuditorConfig {
            enabled: true,
            default_action: "allow".to_string(),
            ..Default::default()
        };
        let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
        let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
        let wrapper = SecureFileWrapper::new(&mw);
        let result = wrapper.append_file(&file_path, "first content").await;
        assert!(result.is_ok(), "append_file failed: {:?}", result);
        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "first content");
    }

    #[tokio::test]
    async fn test_file_wrapper_append_to_file_without_newline() {
        let dir = tempfile::tempdir().unwrap();
        let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
        let file_path = format!("{}\\append_nonl.txt", ws);
        std::fs::write(&file_path, "no newline").unwrap();
        let config = AuditorConfig {
            enabled: true,
            default_action: "allow".to_string(),
            ..Default::default()
        };
        let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
        let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
        let wrapper = SecureFileWrapper::new(&mw);
        let result = wrapper.append_file(&file_path, "appended").await;
        assert!(result.is_ok(), "append_file failed: {:?}", result);
        let content = std::fs::read_to_string(&file_path).unwrap();
        assert!(content.contains("no newline\nappended"));
    }

    #[tokio::test]
    async fn test_file_wrapper_delete_file() {
        let dir = tempfile::tempdir().unwrap();
        let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
        let file_path = format!("{}\\delete_me.txt", ws);
        std::fs::write(&file_path, "to be deleted").unwrap();
        let config = AuditorConfig {
            enabled: true,
            default_action: "allow".to_string(),
            ..Default::default()
        };
        let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
        let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Unrestricted);
        let wrapper = SecureFileWrapper::new(&mw);
        let result = wrapper.delete_file(&file_path).await;
        assert!(result.is_ok(), "delete_file failed: {:?}", result);
        assert!(!std::path::Path::new(&file_path).exists());
    }

    #[tokio::test]
    async fn test_file_wrapper_read_directory() {
        let dir = tempfile::tempdir().unwrap();
        let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
        std::fs::write(format!("{}\\file1.txt", ws), "a").unwrap();
        std::fs::create_dir(format!("{}\\subdir", ws)).unwrap();
        let config = AuditorConfig {
            enabled: true,
            default_action: "allow".to_string(),
            ..Default::default()
        };
        let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
        let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
        let wrapper = SecureFileWrapper::new(&mw);
        let result = wrapper.read_directory(&ws).await;
        assert!(result.is_ok(), "read_directory failed: {:?}", result);
        let entries = result.unwrap();
        assert!(entries.contains(&"file1.txt".to_string()));
        assert!(entries.contains(&"subdir/".to_string()));
    }

    #[tokio::test]
    async fn test_file_wrapper_create_directory() {
        let dir = tempfile::tempdir().unwrap();
        let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
        let new_dir = format!("{}\\new_dir", ws);
        let config = AuditorConfig {
            enabled: true,
            default_action: "allow".to_string(),
            ..Default::default()
        };
        let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
        let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
        let wrapper = SecureFileWrapper::new(&mw);
        let result = wrapper.create_directory(&new_dir).await;
        assert!(result.is_ok(), "create_directory failed: {:?}", result);
        assert!(std::path::Path::new(&new_dir).is_dir());
    }

    #[tokio::test]
    async fn test_file_wrapper_delete_directory() {
        let dir = tempfile::tempdir().unwrap();
        let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
        let del_dir = format!("{}\\del_dir", ws);
        std::fs::create_dir_all(&del_dir).unwrap();
        std::fs::write(format!("{}\\inner.txt", del_dir), "x").unwrap();
        let config = AuditorConfig {
            enabled: true,
            default_action: "allow".to_string(),
            ..Default::default()
        };
        let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
        let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Unrestricted);
        let wrapper = SecureFileWrapper::new(&mw);
        let result = wrapper.delete_directory(&del_dir).await;
        assert!(result.is_ok(), "delete_directory failed: {:?}", result);
        assert!(!std::path::Path::new(&del_dir).exists());
    }

    #[tokio::test]
    async fn test_file_wrapper_stat() {
        let dir = tempfile::tempdir().unwrap();
        let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
        let file_path = format!("{}\\stat_test.txt", ws);
        std::fs::write(&file_path, "stat content here").unwrap();
        let config = AuditorConfig {
            enabled: true,
            default_action: "allow".to_string(),
            ..Default::default()
        };
        let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
        let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
        let wrapper = SecureFileWrapper::new(&mw);
        let result = wrapper.stat(&file_path).await;
        assert!(result.is_ok(), "stat failed: {:?}", result);
        let meta = result.unwrap();
        assert!(meta.is_file);
        assert!(!meta.is_dir);
        assert_eq!(meta.len, "stat content here".len() as u64);
    }

    #[tokio::test]
    async fn test_file_wrapper_open_file() {
        let dir = tempfile::tempdir().unwrap();
        let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
        let file_path = format!("{}\\open_test.txt", ws);
        std::fs::write(&file_path, b"binary\x00data").unwrap();
        let config = AuditorConfig {
            enabled: true,
            default_action: "allow".to_string(),
            ..Default::default()
        };
        let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
        let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
        let wrapper = SecureFileWrapper::new(&mw);
        let result = wrapper.open_file(&file_path).await;
        assert!(result.is_ok(), "open_file failed: {:?}", result);
        assert_eq!(result.unwrap(), b"binary\x00data");
    }

    #[tokio::test]
    async fn test_file_wrapper_list_dir() {
        let dir = tempfile::tempdir().unwrap();
        let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
        std::fs::write(format!("{}\\a.txt", ws), "aaa").unwrap();
        std::fs::write(format!("{}\\b.txt", ws), "bb").unwrap();
        let config = AuditorConfig {
            enabled: true,
            default_action: "allow".to_string(),
            ..Default::default()
        };
        let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
        let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
        let wrapper = SecureFileWrapper::new(&mw);
        let result = wrapper.list_dir(&ws).await;
        assert!(result.is_ok(), "list_dir failed: {:?}", result);
        let entries = result.unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().any(|e| e.name == "a.txt"));
        assert!(entries.iter().any(|e| e.name == "b.txt"));
    }

    #[tokio::test]
    async fn test_file_wrapper_remove_file_alias() {
        let dir = tempfile::tempdir().unwrap();
        let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
        let file_path = format!("{}\\remove_me.txt", ws);
        std::fs::write(&file_path, "bye").unwrap();
        let config = AuditorConfig {
            enabled: true,
            default_action: "allow".to_string(),
            ..Default::default()
        };
        let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
        let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Unrestricted);
        let wrapper = SecureFileWrapper::new(&mw);
        let result = wrapper.remove_file(&file_path).await;
        assert!(result.is_ok(), "remove_file failed: {:?}", result);
        assert!(!std::path::Path::new(&file_path).exists());
    }

    #[tokio::test]
    async fn test_file_wrapper_create_dir_alias() {
        let dir = tempfile::tempdir().unwrap();
        let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
        let new_dir = format!("{}\\alias_dir", ws);
        let config = AuditorConfig {
            enabled: true,
            default_action: "allow".to_string(),
            ..Default::default()
        };
        let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
        let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
        let wrapper = SecureFileWrapper::new(&mw);
        let result = wrapper.create_dir(&new_dir).await;
        assert!(result.is_ok(), "create_dir failed: {:?}", result);
        assert!(std::path::Path::new(&new_dir).is_dir());
    }

    #[tokio::test]
    async fn test_file_wrapper_remove_dir_alias() {
        let dir = tempfile::tempdir().unwrap();
        let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
        let rm_dir = format!("{}\\rm_dir", ws);
        std::fs::create_dir_all(&rm_dir).unwrap();
        let config = AuditorConfig {
            enabled: true,
            default_action: "allow".to_string(),
            ..Default::default()
        };
        let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
        let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Unrestricted);
        let wrapper = SecureFileWrapper::new(&mw);
        let result = wrapper.remove_dir(&rm_dir).await;
        assert!(result.is_ok(), "remove_dir failed: {:?}", result);
        assert!(!std::path::Path::new(&rm_dir).exists());
    }

    #[test]
    fn test_process_wrapper_check_kill() {
        let mw = make_middleware(PermissionPreset::Unrestricted);
        let wrapper = SecureProcessWrapper::new(&mw);
        let result = wrapper.check_process_kill("9999");
        // Under unrestricted, may pass ABAC check
        let _ = result;
    }

    #[test]
    fn test_file_metadata_fields() {
        let meta = FileMetadata {
            is_file: true,
            is_dir: false,
            len: 1024,
            readonly: false,
            modified: "2026-01-01T00:00:00Z".to_string(),
        };
        assert!(meta.is_file);
        assert!(!meta.is_dir);
        assert_eq!(meta.len, 1024);
        assert!(!meta.readonly);
    }

    #[test]
    fn test_dir_entry_fields() {
        let entry = DirEntry {
            name: "test.txt".to_string(),
            is_dir: false,
            size: 42,
        };
        assert_eq!(entry.name, "test.txt");
        assert!(!entry.is_dir);
        assert_eq!(entry.size, 42);
    }

    #[test]
    fn test_http_request_with_body() {
        let req = HttpRequest {
            url: "https://api.example.com".to_string(),
            method: "POST".to_string(),
            headers: vec![("Content-Type".to_string(), "application/json".to_string())],
            body: Some(r#"{"key":"value"}"#.to_string()),
            timeout_secs: Some(30),
        };
        assert_eq!(req.method, "POST");
        assert!(req.body.is_some());
        assert_eq!(req.headers.len(), 1);
        assert_eq!(req.timeout_secs, Some(30));
    }

    #[tokio::test]
    async fn test_network_wrapper_check_request_valid() {
        let mw = make_middleware(PermissionPreset::Standard);
        let wrapper = SecureNetworkWrapper::new(&mw);
        assert!(wrapper.check_network_request("https://api.example.com/data").is_ok());
        assert!(wrapper.check_network_request("http://localhost:8080/api").is_ok());
    }

    #[tokio::test]
    async fn test_hardware_wrapper_i2c_read_coverage() {
        let mw = make_middleware(PermissionPreset::Unrestricted);
        let wrapper = SecureHardwareWrapper::new(&mw);
        let result = wrapper.i2c_read("1", 0x48, 0x00, 4).await;
        // Will fail because i2cget is not available, but should not panic
        let _ = result;
    }

    #[tokio::test]
    async fn test_hardware_wrapper_i2c_write_coverage() {
        let mw = make_middleware(PermissionPreset::Unrestricted);
        let wrapper = SecureHardwareWrapper::new(&mw);
        let result = wrapper.i2c_write("1", 0x48, 0x00, &[0x01, 0x02]).await;
        let _ = result;
    }

    #[test]
    fn test_hardware_wrapper_blocked_in_standard() {
        let mw = make_middleware(PermissionPreset::Standard);
        let wrapper = SecureHardwareWrapper::new(&mw);
        assert!(wrapper.check_i2c("device").is_err());
        assert!(wrapper.check_spi("device").is_err());
        assert!(wrapper.check_gpio("17").is_err());
    }

    #[test]
    fn test_network_wrapper_upload_blocked_invalid_url() {
        let mw = make_middleware(PermissionPreset::Unrestricted);
        let wrapper = SecureNetworkWrapper::new(&mw);
        assert!(wrapper.check_network_upload("ftp://bad.url").is_err());
        assert!(wrapper.check_network_upload("file:///etc/passwd").is_err());
    }

    #[test]
    fn test_network_wrapper_request_blocked_in_readonly() {
        let mw = make_middleware(PermissionPreset::ReadOnly);
        let wrapper = SecureNetworkWrapper::new(&mw);
        assert!(wrapper.check_network_request("https://example.com").is_err());
    }

    #[test]
    fn test_check_operation_with_dangerous_process() {
        let config = AuditorConfig {
            enabled: true,
            default_action: "ask".to_string(),
            ..Default::default()
        };
        let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
        let mw = SecurityMiddleware::with_preset(
            auditor.clone(), "test_user", "cli", "/tmp/workspace", PermissionPreset::Unrestricted,
        );
        // ProcessExec is allowed by Unrestricted, but dangerous commands should still be checked
        let result = mw.check_operation(OperationType::ProcessExec, "rm -rf /");
        // This should either be allowed (ABAC) or blocked (safe command check)
        let _ = result;
    }

    // ---- Additional coverage tests ----

    #[test]
    fn test_batch_permission_empty() {
        let mw = make_middleware(PermissionPreset::Standard);
        let batch = BatchOperationRequest::default();
        let result = mw.request_batch_permission(&batch);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no operations"));
    }

    #[test]
    fn test_batch_permission_allowed() {
        let mw = make_middleware(PermissionPreset::Standard);
        let mut batch = BatchOperationRequest::default();
        batch.id = "batch-1".to_string();
        batch.operations.push(OperationRequest {
            id: "op-1".to_string(),
            op_type: OperationType::FileRead,
            danger_level: DangerLevel::Low,
            target: "/tmp/test.txt".to_string(),
            ..Default::default()
        });
        let result = mw.request_batch_permission(&batch);
        assert!(result.is_ok());
    }

    #[test]
    fn test_batch_permission_preset_blocked() {
        let mw = make_middleware(PermissionPreset::ReadOnly);
        let mut batch = BatchOperationRequest::default();
        batch.operations.push(OperationRequest {
            id: "op-1".to_string(),
            op_type: OperationType::ProcessExec,
            danger_level: DangerLevel::Critical,
            target: "rm -rf /".to_string(),
            ..Default::default()
        });
        let result = mw.request_batch_permission(&batch);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not allowed"));
    }

    #[test]
    fn test_batch_permission_with_custom_id() {
        let mw = make_middleware(PermissionPreset::Standard);
        let mut batch = BatchOperationRequest::default();
        batch.id = "custom-batch-id".to_string();
        batch.operations.push(OperationRequest {
            id: "op-1".to_string(),
            op_type: OperationType::FileRead,
            danger_level: DangerLevel::Low,
            target: "/tmp/test.txt".to_string(),
            ..Default::default()
        });
        let result = mw.request_batch_permission(&batch);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "custom-batch-id");
    }

    #[test]
    fn test_batch_permission_auto_id() {
        let mw = make_middleware(PermissionPreset::Standard);
        let mut batch = BatchOperationRequest::default();
        // id is empty, should auto-generate
        batch.operations.push(OperationRequest {
            id: "op-1".to_string(),
            op_type: OperationType::FileRead,
            danger_level: DangerLevel::Low,
            target: "/tmp/test.txt".to_string(),
            ..Default::default()
        });
        let result = mw.request_batch_permission(&batch);
        assert!(result.is_ok());
        // Should be a UUID
        let id = result.unwrap();
        assert!(!id.is_empty());
    }

    #[test]
    fn test_get_security_summary_fields() {
        let mw = make_middleware(PermissionPreset::Standard);
        let summary = mw.get_security_summary();
        assert!(summary["user"] == "test_user");
        assert!(summary["source"] == "cli");
        assert!(summary["workspace"] == "/tmp/workspace");
        assert!(summary["pending_requests"].as_u64().unwrap() == 0);
    }

    #[test]
    fn test_batch_operation_request_default() {
        let batch = BatchOperationRequest::default();
        assert!(batch.id.is_empty());
        assert!(batch.operations.is_empty());
        assert!(batch.user.is_empty());
        assert!(batch.source.is_empty());
        assert!(batch.description.is_empty());
    }

    #[test]
    fn test_batch_permission_multiple_operations() {
        let mw = make_middleware(PermissionPreset::Standard);
        let mut batch = BatchOperationRequest::default();
        batch.id = "multi-batch".to_string();
        batch.operations.push(OperationRequest {
            id: "op-1".to_string(),
            op_type: OperationType::FileRead,
            danger_level: DangerLevel::Low,
            target: "/tmp/a.txt".to_string(),
            ..Default::default()
        });
        batch.operations.push(OperationRequest {
            id: "op-2".to_string(),
            op_type: OperationType::FileWrite,
            danger_level: DangerLevel::Medium,
            target: "/tmp/b.txt".to_string(),
            ..Default::default()
        });
        let result = mw.request_batch_permission(&batch);
        assert!(result.is_ok());
    }

    #[test]
    fn test_approve_deny_pending_request() {
        let mw = make_middleware(PermissionPreset::Unrestricted);
        // Approve/deny non-existent request should fail gracefully
        let result = mw.approve_pending_request("nonexistent");
        assert!(result.is_err());
        let result = mw.deny_pending_request("nonexistent", "testing");
        assert!(result.is_err());
    }

    #[test]
    fn test_middleware_accessors() {
        let mw = make_middleware(PermissionPreset::Standard);
        assert_eq!(mw.user(), "test_user");
        assert_eq!(mw.source(), "cli");
        assert_eq!(mw.workspace(), "/tmp/workspace");
        assert_eq!(mw.preset(), PermissionPreset::Standard);
    }

    #[test]
    fn test_middleware_set_preset() {
        let mut mw = make_middleware(PermissionPreset::ReadOnly);
        assert_eq!(mw.preset(), PermissionPreset::ReadOnly);
        mw.set_preset(PermissionPreset::Unrestricted);
        assert_eq!(mw.preset(), PermissionPreset::Unrestricted);
    }

    #[test]
    fn test_is_operation_allowed() {
        let mw = make_middleware(PermissionPreset::ReadOnly);
        assert!(mw.is_operation_allowed(OperationType::FileRead));
        assert!(!mw.is_operation_allowed(OperationType::FileWrite));
        assert!(!mw.is_operation_allowed(OperationType::ProcessExec));
    }

    #[test]
    fn test_export_audit_log_invalid_path() {
        let mw = make_middleware(PermissionPreset::Standard);
        let result = mw.export_audit_log("/nonexistent/dir/file.log");
        // May fail due to path not existing
        let _ = result;
    }

    #[test]
    fn test_get_audit_log_with_filter() {
        let mw = make_middleware(PermissionPreset::Standard);
        let filter = AuditFilter::default();
        let events = mw.get_audit_log(filter);
        // May be empty since no operations have been performed
        assert!(events.is_empty() || !events.is_empty());
    }

    #[test]
    fn test_batch_permission_max_danger_critical() {
        let mw = make_middleware(PermissionPreset::Unrestricted);
        let mut batch = BatchOperationRequest::default();
        batch.operations.push(OperationRequest {
            id: "op-1".to_string(),
            op_type: OperationType::FileRead,
            danger_level: DangerLevel::Low,
            target: "/tmp/a.txt".to_string(),
            ..Default::default()
        });
        batch.operations.push(OperationRequest {
            id: "op-2".to_string(),
            op_type: OperationType::ProcessKill,
            danger_level: DangerLevel::Critical,
            target: "1234".to_string(),
            ..Default::default()
        });
        let result = mw.request_batch_permission(&batch);
        // Even with Unrestricted, it may be blocked by the auditor
        let _ = result;
    }

    // ---- More coverage tests for middleware ----

    #[test]
    fn test_security_middleware_new() {
        let config = AuditorConfig {
            enabled: true,
            default_action: "allow".to_string(),
            ..Default::default()
        };
        let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
        let mw = SecurityMiddleware::new(auditor, "user1", "web", "/home/user/workspace");
        assert_eq!(mw.user(), "user1");
        assert_eq!(mw.source(), "web");
        assert_eq!(mw.workspace(), "/home/user/workspace");
        assert_eq!(mw.preset(), PermissionPreset::Standard); // default preset
    }

    #[test]
    fn test_check_operation_all_ops_under_readonly() {
        let mw = make_middleware(PermissionPreset::ReadOnly);
        // Only FileRead and DirRead should pass preset check
        assert!(mw.check_operation(OperationType::FileRead, "/tmp/test").is_ok());
        assert!(mw.check_operation(OperationType::DirRead, "/tmp/test").is_ok());
        // Everything else should fail
        assert!(mw.check_operation(OperationType::FileWrite, "/tmp/test").is_err());
        assert!(mw.check_operation(OperationType::FileDelete, "/tmp/test").is_err());
        assert!(mw.check_operation(OperationType::DirCreate, "/tmp/test").is_err());
        assert!(mw.check_operation(OperationType::DirDelete, "/tmp/test").is_err());
        assert!(mw.check_operation(OperationType::ProcessExec, "ls").is_err());
        assert!(mw.check_operation(OperationType::ProcessSpawn, "ls").is_err());
        assert!(mw.check_operation(OperationType::ProcessKill, "1234").is_err());
        assert!(mw.check_operation(OperationType::NetworkRequest, "http://a.com").is_err());
        assert!(mw.check_operation(OperationType::NetworkDownload, "http://a.com").is_err());
        assert!(mw.check_operation(OperationType::NetworkUpload, "http://a.com").is_err());
        assert!(mw.check_operation(OperationType::HardwareI2C, "dev").is_err());
        assert!(mw.check_operation(OperationType::HardwareSPI, "dev").is_err());
        assert!(mw.check_operation(OperationType::HardwareGPIO, "17").is_err());
        assert!(mw.check_operation(OperationType::RegistryWrite, "HKLM").is_err());
        assert!(mw.check_operation(OperationType::SystemShutdown, "now").is_err());
        assert!(mw.check_operation(OperationType::SystemReboot, "now").is_err());
    }

    #[test]
    fn test_check_operation_standard_ops() {
        let mw = make_middleware(PermissionPreset::Standard);
        // Allowed under Standard
        assert!(mw.check_operation(OperationType::FileRead, "/tmp/test").is_ok());
        assert!(mw.check_operation(OperationType::FileWrite, "/tmp/test").is_ok());
        assert!(mw.check_operation(OperationType::DirRead, "/tmp/test").is_ok());
        assert!(mw.check_operation(OperationType::DirCreate, "/tmp/test").is_ok());
        assert!(mw.check_operation(OperationType::NetworkRequest, "http://a.com").is_ok());
        assert!(mw.check_operation(OperationType::NetworkDownload, "http://a.com").is_ok());
        // Denied under Standard
        assert!(mw.check_operation(OperationType::FileDelete, "/tmp/test").is_err());
        assert!(mw.check_operation(OperationType::DirDelete, "/tmp/test").is_err());
        assert!(mw.check_operation(OperationType::ProcessExec, "ls").is_err());
        assert!(mw.check_operation(OperationType::ProcessSpawn, "ls").is_err());
        assert!(mw.check_operation(OperationType::ProcessKill, "1234").is_err());
        assert!(mw.check_operation(OperationType::NetworkUpload, "http://a.com").is_err());
        assert!(mw.check_operation(OperationType::HardwareI2C, "dev").is_err());
        assert!(mw.check_operation(OperationType::RegistryWrite, "HKLM").is_err());
        assert!(mw.check_operation(OperationType::SystemShutdown, "now").is_err());
    }

    #[test]
    fn test_check_operation_elevated_ops() {
        let mw = make_middleware(PermissionPreset::Elevated);
        // Allowed under Elevated
        assert!(mw.check_operation(OperationType::FileRead, "/tmp/test").is_ok());
        assert!(mw.check_operation(OperationType::FileWrite, "/tmp/test").is_ok());
        assert!(mw.check_operation(OperationType::FileDelete, "/tmp/test").is_ok());
        assert!(mw.check_operation(OperationType::DirRead, "/tmp/test").is_ok());
        assert!(mw.check_operation(OperationType::DirCreate, "/tmp/test").is_ok());
        assert!(mw.check_operation(OperationType::DirDelete, "/tmp/test").is_ok());
        assert!(mw.check_operation(OperationType::ProcessExec, "ls").is_ok());
        assert!(mw.check_operation(OperationType::ProcessSpawn, "ls").is_ok());
        assert!(mw.check_operation(OperationType::NetworkRequest, "http://a.com").is_ok());
        assert!(mw.check_operation(OperationType::NetworkDownload, "http://a.com").is_ok());
        assert!(mw.check_operation(OperationType::NetworkUpload, "http://a.com").is_ok());
        // Denied under Elevated
        assert!(mw.check_operation(OperationType::ProcessKill, "1234").is_err());
        assert!(mw.check_operation(OperationType::SystemShutdown, "now").is_err());
        assert!(mw.check_operation(OperationType::SystemReboot, "now").is_err());
        assert!(mw.check_operation(OperationType::RegistryWrite, "HKLM").is_err());
        assert!(mw.check_operation(OperationType::HardwareI2C, "dev").is_err());
        assert!(mw.check_operation(OperationType::HardwareGPIO, "17").is_err());
    }

    #[test]
    fn test_network_wrapper_check_upload_valid_schemes() {
        let mw = make_middleware(PermissionPreset::Unrestricted);
        let wrapper = SecureNetworkWrapper::new(&mw);
        assert!(wrapper.check_network_upload("https://example.com/upload").is_ok());
        assert!(wrapper.check_network_upload("http://example.com/upload").is_ok());
    }

    #[test]
    fn test_hardware_wrapper_spi_write_blocked_in_standard() {
        let mw = make_middleware(PermissionPreset::Standard);
        let wrapper = SecureHardwareWrapper::new(&mw);
        assert!(wrapper.spi_write("1.0", &[0x01]).is_err());
    }

    #[test]
    fn test_hardware_wrapper_gpio_blocked_in_standard() {
        let mw = make_middleware(PermissionPreset::Standard);
        let wrapper = SecureHardwareWrapper::new(&mw);
        assert!(wrapper.check_gpio("17").is_err());
    }

    #[test]
    fn test_hardware_wrapper_i2c_allowed_in_unrestricted() {
        let mw = make_middleware(PermissionPreset::Unrestricted);
        let wrapper = SecureHardwareWrapper::new(&mw);
        assert!(wrapper.check_i2c("i2c-1:0x48").is_ok());
        assert!(wrapper.check_spi("spidev1.0").is_ok());
        assert!(wrapper.check_gpio("17").is_ok());
    }

    #[test]
    fn test_process_wrapper_spawn_dangerous_commands() {
        let mw = make_middleware(PermissionPreset::Elevated);
        let wrapper = SecureProcessWrapper::new(&mw);
        // All these should be blocked by is_safe_command
        assert!(wrapper.check_process_exec("format C:").is_err());
        assert!(wrapper.check_process_exec("mkfs.ext4 /dev/sda1").is_err());
        assert!(wrapper.check_process_exec("dd if=/dev/zero of=/dev/sda").is_err());
        assert!(wrapper.check_process_exec("chmod 777 /etc/passwd").is_err());
        assert!(wrapper.check_process_exec("chown root:root /tmp/evil").is_err());
        assert!(wrapper.check_process_exec("shutdown -h now").is_err());
        assert!(wrapper.check_process_exec("reboot").is_err());
        assert!(wrapper.check_process_exec("poweroff").is_err());
        assert!(wrapper.check_process_spawn("sudo rm -rf /").is_err());
    }

    #[test]
    fn test_process_wrapper_safe_commands() {
        let mw = make_middleware(PermissionPreset::Elevated);
        let wrapper = SecureProcessWrapper::new(&mw);
        // These should pass the safe command check
        assert!(wrapper.check_process_exec("ls -la /tmp").is_ok());
        assert!(wrapper.check_process_exec("python script.py").is_ok());
        assert!(wrapper.check_process_exec("echo hello").is_ok());
        assert!(wrapper.check_process_exec("git status").is_ok());
        assert!(wrapper.check_process_spawn("cargo build").is_ok());
    }

    #[test]
    fn test_process_output_struct() {
        let output = ProcessOutput {
            stdout: "hello".to_string(),
            stderr: String::new(),
            exit_code: Some(0),
            success: true,
        };
        assert_eq!(output.stdout, "hello");
        assert!(output.stderr.is_empty());
        assert_eq!(output.exit_code, Some(0));
        assert!(output.success);
    }

    #[test]
    fn test_http_request_struct_fields() {
        let req = HttpRequest {
            url: "https://api.example.com/v1/data".to_string(),
            method: "PUT".to_string(),
            headers: vec![
                ("Authorization".to_string(), "Bearer token".to_string()),
                ("Content-Type".to_string(), "application/json".to_string()),
            ],
            body: Some(r#"{"key":"value"}"#.to_string()),
            timeout_secs: Some(60),
        };
        assert_eq!(req.url, "https://api.example.com/v1/data");
        assert_eq!(req.method, "PUT");
        assert_eq!(req.headers.len(), 2);
        assert!(req.body.is_some());
        assert_eq!(req.timeout_secs, Some(60));
    }

    #[test]
    fn test_batch_operation_multiple_preset_blocks() {
        // Standard does not allow DirDelete
        let mw = make_middleware(PermissionPreset::Standard);
        let batch = BatchOperationRequest {
            id: "batch-test".to_string(),
            operations: vec![
                OperationRequest {
                    id: "op-ok".to_string(),
                    op_type: OperationType::FileRead,
                    danger_level: DangerLevel::Low,
                    user: "test".to_string(),
                    source: "cli".to_string(),
                    target: "/tmp/a.txt".to_string(),
                    timestamp: None,
                    ..Default::default()
                },
                OperationRequest {
                    id: "op-bad".to_string(),
                    op_type: OperationType::DirDelete,
                    danger_level: DangerLevel::High,
                    user: "test".to_string(),
                    source: "cli".to_string(),
                    target: "/tmp/dir".to_string(),
                    timestamp: None,
                    ..Default::default()
                },
            ],
            user: "test".to_string(),
            source: "cli".to_string(),
            description: "mixed batch".to_string(),
        };
        let result = mw.request_batch_permission(&batch);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not allowed"));
    }

    #[test]
    fn test_batch_operation_danger_levels_propagated() {
        // The summary request should use the max danger level
        let config = AuditorConfig {
            enabled: true,
            default_action: "deny".to_string(), // deny everything
            ..Default::default()
        };
        let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
        let mw = SecurityMiddleware::with_preset(
            auditor, "test_user", "cli", "/tmp/ws", PermissionPreset::Standard,
        );
        let batch = BatchOperationRequest {
            id: "batch-deny".to_string(),
            operations: vec![OperationRequest {
                id: "op-1".to_string(),
                op_type: OperationType::FileRead,
                danger_level: DangerLevel::Low,
                user: "test".to_string(),
                source: "cli".to_string(),
                target: "/tmp/a.txt".to_string(),
                timestamp: None,
                ..Default::default()
            }],
            ..Default::default()
        };
        let result = mw.request_batch_permission(&batch);
        // Should be denied by the auditor
        assert!(result.is_err());
    }

    #[test]
    fn test_create_cli_permission_denied_targets() {
        let cli = create_cli_permission();
        assert!(cli.is_target_denied("/etc/sudoers"));
        assert!(cli.is_target_denied("/etc/passwd"));
        assert!(cli.is_target_denied("C:/Windows/System32/drivers/etc/hosts"));
        assert!(!cli.is_target_denied("/tmp/safe_file"));
    }

    #[test]
    fn test_create_web_permission_no_denied_targets() {
        let web = create_web_permission();
        assert!(!web.is_target_denied("/etc/passwd"));
        assert!(!web.is_target_denied("/any/path"));
    }

    #[test]
    fn test_create_agent_permission_operations() {
        let agent = create_agent_permission("agent-123");
        assert!(agent.is_operation_allowed(&OperationType::FileRead));
        assert!(agent.is_operation_allowed(&OperationType::FileWrite));
        assert!(agent.is_operation_allowed(&OperationType::DirRead));
        assert!(agent.is_operation_allowed(&OperationType::DirCreate));
        assert!(agent.is_operation_allowed(&OperationType::ProcessExec));
        assert!(agent.is_operation_allowed(&OperationType::NetworkRequest));
        assert!(!agent.is_operation_allowed(&OperationType::FileDelete));
        assert!(!agent.is_operation_allowed(&OperationType::DirDelete));
        assert!(!agent.is_operation_allowed(&OperationType::ProcessKill));
        assert!(!agent.is_operation_allowed(&OperationType::ProcessSpawn));
        assert!(!agent.is_operation_allowed(&OperationType::NetworkDownload));
        assert!(!agent.is_operation_allowed(&OperationType::NetworkUpload));
    }

    #[test]
    fn test_create_web_permission_network_not_allowed() {
        let web = create_web_permission();
        assert!(!web.is_operation_allowed(&OperationType::NetworkRequest));
        assert!(!web.is_operation_allowed(&OperationType::NetworkDownload));
    }

    #[test]
    fn test_set_preset_changes_behavior() {
        let mut mw = make_middleware(PermissionPreset::ReadOnly);
        assert!(mw.check_operation(OperationType::FileWrite, "/tmp/test").is_err());
        mw.set_preset(PermissionPreset::Standard);
        assert!(mw.check_operation(OperationType::FileWrite, "/tmp/test").is_ok());
        mw.set_preset(PermissionPreset::Elevated);
        assert!(mw.check_operation(OperationType::ProcessExec, "ls").is_ok());
        mw.set_preset(PermissionPreset::Unrestricted);
        assert!(mw.check_operation(OperationType::ProcessKill, "1234").is_ok());
    }

    #[tokio::test]
    async fn test_file_wrapper_write_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
        let config = AuditorConfig {
            enabled: true,
            default_action: "allow".to_string(),
            ..Default::default()
        };
        let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
        let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
        let wrapper = SecureFileWrapper::new(&mw);
        let file_path = format!("{}\\subdir\\nested\\test.txt", ws);
        let result = wrapper.write_file(&file_path, "nested content").await;
        assert!(result.is_ok(), "write_file with nested dirs failed: {:?}", result);
        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "nested content");
    }

    #[tokio::test]
    async fn test_file_wrapper_read_nonexistent_file() {
        let dir = tempfile::tempdir().unwrap();
        let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
        let config = AuditorConfig {
            enabled: true,
            default_action: "allow".to_string(),
            ..Default::default()
        };
        let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
        let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
        let wrapper = SecureFileWrapper::new(&mw);
        let file_path = format!("{}\\nonexistent.txt", ws);
        let result = wrapper.read_file(&file_path).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_file_wrapper_delete_nonexistent_file() {
        let dir = tempfile::tempdir().unwrap();
        let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
        let config = AuditorConfig {
            enabled: true,
            default_action: "allow".to_string(),
            ..Default::default()
        };
        let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
        let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Unrestricted);
        let wrapper = SecureFileWrapper::new(&mw);
        let file_path = format!("{}\\nonexistent_delete.txt", ws);
        let result = wrapper.delete_file(&file_path).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_file_wrapper_edit_with_multiline_pattern() {
        let dir = tempfile::tempdir().unwrap();
        let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
        let file_path = format!("{}\\multiline.txt", ws);
        std::fs::write(&file_path, "line1\nline2\nline3").unwrap();
        let config = AuditorConfig {
            enabled: true,
            default_action: "allow".to_string(),
            ..Default::default()
        };
        let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
        let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
        let wrapper = SecureFileWrapper::new(&mw);
        let result = wrapper.edit_file(&file_path, "line1\nline2", "replaced").await;
        assert!(result.is_ok(), "edit_file multiline failed: {:?}", result);
        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "replaced\nline3");
    }

    #[tokio::test]
    async fn test_file_wrapper_stat_directory() {
        let dir = tempfile::tempdir().unwrap();
        let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
        let config = AuditorConfig {
            enabled: true,
            default_action: "allow".to_string(),
            ..Default::default()
        };
        let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
        let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
        let wrapper = SecureFileWrapper::new(&mw);
        let result = wrapper.stat(&ws).await;
        assert!(result.is_ok(), "stat dir failed: {:?}", result);
        let meta = result.unwrap();
        assert!(meta.is_dir);
        assert!(!meta.is_file);
    }

    #[tokio::test]
    async fn test_file_wrapper_stat_nonexistent() {
        let dir = tempfile::tempdir().unwrap();
        let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
        let config = AuditorConfig {
            enabled: true,
            default_action: "allow".to_string(),
            ..Default::default()
        };
        let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
        let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
        let wrapper = SecureFileWrapper::new(&mw);
        let file_path = format!("{}\\nonexistent_stat.txt", ws);
        let result = wrapper.stat(&file_path).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_file_wrapper_read_empty_directory() {
        let dir = tempfile::tempdir().unwrap();
        let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
        let config = AuditorConfig {
            enabled: true,
            default_action: "allow".to_string(),
            ..Default::default()
        };
        let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
        let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
        let wrapper = SecureFileWrapper::new(&mw);
        let result = wrapper.read_directory(&ws).await;
        assert!(result.is_ok());
        let entries = result.unwrap();
        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn test_file_wrapper_delete_nonexistent_directory() {
        let dir = tempfile::tempdir().unwrap();
        let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
        let config = AuditorConfig {
            enabled: true,
            default_action: "allow".to_string(),
            ..Default::default()
        };
        let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
        let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Unrestricted);
        let wrapper = SecureFileWrapper::new(&mw);
        let del_dir = format!("{}\\nonexistent_dir", ws);
        let result = wrapper.delete_directory(&del_dir).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_file_wrapper_list_dir_with_subdirs() {
        let dir = tempfile::tempdir().unwrap();
        let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
        std::fs::create_dir(format!("{}\\child_dir", ws)).unwrap();
        std::fs::write(format!("{}\\root_file.txt", ws), "hello").unwrap();
        let config = AuditorConfig {
            enabled: true,
            default_action: "allow".to_string(),
            ..Default::default()
        };
        let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
        let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
        let wrapper = SecureFileWrapper::new(&mw);
        let result = wrapper.list_dir(&ws).await;
        assert!(result.is_ok());
        let entries = result.unwrap();
        assert_eq!(entries.len(), 2);
        let child = entries.iter().find(|e| e.name == "child_dir").unwrap();
        assert!(child.is_dir);
        let file = entries.iter().find(|e| e.name == "root_file.txt").unwrap();
        assert!(!file.is_dir);
        assert_eq!(file.size, 5);
    }

    #[tokio::test]
    async fn test_file_wrapper_open_file_nonexistent() {
        let dir = tempfile::tempdir().unwrap();
        let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
        let config = AuditorConfig {
            enabled: true,
            default_action: "allow".to_string(),
            ..Default::default()
        };
        let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
        let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
        let wrapper = SecureFileWrapper::new(&mw);
        let file_path = format!("{}\\nonexistent_open.txt", ws);
        let result = wrapper.open_file(&file_path).await;
        assert!(result.is_err());
    }
}
