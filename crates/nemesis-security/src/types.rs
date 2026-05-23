//! Security types: OperationType, DangerLevel, etc.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

/// Operation type classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OperationType {
    // File operations
    FileRead,
    FileWrite,
    FileDelete,
    // Directory operations
    DirRead,
    DirCreate,
    DirDelete,
    // Process operations
    ProcessExec,
    ProcessSpawn,
    ProcessKill,
    ProcessSuspend,
    // Network operations
    NetworkDownload,
    NetworkUpload,
    NetworkRequest,
    // Hardware operations
    HardwareI2C,
    HardwareSPI,
    HardwareGPIO,
    // System operations
    SystemShutdown,
    SystemReboot,
    SystemConfig,
   SystemService,
    SystemInstall,
    // Registry operations
    RegistryRead,
    RegistryWrite,
    RegistryDelete,
}

impl fmt::Display for OperationType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::FileRead => "file_read",
            Self::FileWrite => "file_write",
            Self::FileDelete => "file_delete",
            Self::DirRead => "dir_read",
            Self::DirCreate => "dir_create",
            Self::DirDelete => "dir_delete",
            Self::ProcessExec => "process_exec",
            Self::ProcessSpawn => "process_spawn",
            Self::ProcessKill => "process_kill",
            Self::ProcessSuspend => "process_suspend",
            Self::NetworkDownload => "network_download",
            Self::NetworkUpload => "network_upload",
            Self::NetworkRequest => "network_request",
            Self::HardwareI2C => "hardware_i2c",
            Self::HardwareSPI => "hardware_spi",
            Self::HardwareGPIO => "hardware_gpio",
            Self::SystemShutdown => "system_shutdown",
            Self::SystemReboot => "system_reboot",
            Self::SystemConfig => "system_config",
            Self::SystemService => "system_service",
            Self::SystemInstall => "system_install",
            Self::RegistryRead => "registry_read",
            Self::RegistryWrite => "registry_write",
            Self::RegistryDelete => "registry_delete",
        };
        write!(f, "{}", s)
    }
}

/// Danger level for operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum DangerLevel {
    Low = 0,
    Medium = 1,
    High = 2,
    Critical = 3,
}

impl fmt::Display for DangerLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Low => write!(f, "LOW"),
            Self::Medium => write!(f, "MEDIUM"),
            Self::High => write!(f, "HIGH"),
            Self::Critical => write!(f, "CRITICAL"),
        }
    }
}

/// Get danger level for an operation type.
pub fn get_danger_level(op: OperationType) -> DangerLevel {
    match op {
        OperationType::FileRead | OperationType::DirRead => DangerLevel::Low,
        OperationType::NetworkDownload | OperationType::NetworkRequest => DangerLevel::Medium,
        OperationType::FileWrite | OperationType::FileDelete
        | OperationType::DirCreate | OperationType::DirDelete
        | OperationType::ProcessSpawn => DangerLevel::High,
        OperationType::ProcessExec | OperationType::ProcessKill
        | OperationType::SystemShutdown | OperationType::SystemReboot
        | OperationType::SystemConfig | OperationType::SystemService | OperationType::SystemInstall
        | OperationType::RegistryWrite | OperationType::RegistryDelete => DangerLevel::Critical,
        _ => DangerLevel::Medium,
    }
}

/// Security rule for ABAC evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityRule {
    pub pattern: String,
    pub action: String,
    #[serde(default)]
    pub comment: String,
}

/// Tool invocation for security checks.
#[derive(Debug, Clone)]
pub struct ToolInvocation {
    pub tool_name: String,
    pub args: serde_json::Value,
    pub user: String,
    pub source: String,
    pub metadata: std::collections::HashMap<String, String>,
}

/// Security decision result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecurityDecision {
    Allowed,
    Denied,
    RequireApproval,
}

impl fmt::Display for SecurityDecision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Allowed => write!(f, "allowed"),
            Self::Denied => write!(f, "denied"),
            Self::RequireApproval => write!(f, "require_approval"),
        }
    }
}

/// Fine-grained permission configuration controlling allowed operation types,
/// target patterns, and approval requirements.
#[derive(Debug, Clone)]
pub struct Permission {
    /// Allowed operation types (true = allowed).
    pub allowed_types: HashMap<OperationType, bool>,
    /// Target patterns that are explicitly allowed.
    pub allowed_targets: Vec<String>,
    /// Target patterns that are explicitly denied.
    pub denied_targets: Vec<String>,
    /// Operation types that require human approval.
    pub require_approval: HashMap<OperationType, bool>,
    /// Maximum danger level that is permitted.
    pub max_danger_level: DangerLevel,
}

impl Permission {
    /// Create a new default permission with everything denied.
    pub fn new() -> Self {
        Self {
            allowed_types: HashMap::new(),
            allowed_targets: Vec::new(),
            denied_targets: Vec::new(),
            require_approval: HashMap::new(),
            max_danger_level: DangerLevel::Low,
        }
    }

    /// Check whether a specific operation type is allowed.
    pub fn is_operation_allowed(&self, op_type: &OperationType) -> bool {
        self.allowed_types.get(op_type).copied().unwrap_or(false)
    }

    /// Check whether a specific operation type requires approval.
    pub fn requires_approval(&self, op_type: &OperationType) -> bool {
        self.require_approval.get(op_type).copied().unwrap_or(false)
    }

    /// Check whether a target string matches any denied pattern.
    pub fn is_target_denied(&self, target: &str) -> bool {
        self.denied_targets.iter().any(|pattern| {
            target.contains(pattern) || matches_pattern(target, pattern)
        })
    }

    /// Check whether a target string matches any allowed pattern.
    pub fn is_target_allowed(&self, target: &str) -> bool {
        self.allowed_targets.iter().any(|pattern| {
            target.contains(pattern) || matches_pattern(target, pattern)
        })
    }
}

impl Default for Permission {
    fn default() -> Self {
        Self::new()
    }
}

/// A single policy rule with attribute-based matching.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyRule {
    /// Rule name for identification.
    pub name: String,
    /// Match by operation type (None = match all).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_op_type: Option<OperationType>,
    /// Match by target pattern (None = match all).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_target: Option<String>,
    /// Match by user (None = match all).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_user: Option<String>,
    /// Match by source (None = match all).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_source: Option<String>,
    /// Minimum danger level to match (None = match all).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_danger: Option<DangerLevel>,
    /// Action to take when matched: "allow", "deny", "ask".
    pub action: String,
    /// Human-readable reason for the action.
    #[serde(default)]
    pub reason: String,
}

/// A security policy containing a set of rules.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Policy {
    /// Policy name.
    pub name: String,
    /// Human-readable description.
    #[serde(default)]
    pub description: String,
    /// Whether this policy is active.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Ordered list of rules to evaluate.
    pub rules: Vec<PolicyRule>,
    /// Default action when no rule matches: "allow", "deny", "ask".
    #[serde(default = "default_action_deny")]
    pub default_action: String,
    /// If true, only log violations without blocking.
    #[serde(default)]
    pub log_only: bool,
    /// Whether multi-factor authentication is required.
    #[serde(default)]
    pub require_mfa: bool,
}

fn default_true() -> bool {
    true
}

fn default_action_deny() -> String {
    "deny".to_string()
}

/// Simple glob-style pattern match for permission target checks.
/// Supports `*` as a wildcard that matches any sequence of characters.
pub fn matches_pattern(target: &str, pattern: &str) -> bool {
    // Use the crate-level matcher for wildcard patterns
    crate::matcher::match_pattern(pattern, target)
}

/// Map tool name to operation type.
pub fn tool_to_operation(tool_name: &str) -> Option<OperationType> {
    match tool_name {
        "read_file" => Some(OperationType::FileRead),
        "write_file" | "edit_file" | "append_file" => Some(OperationType::FileWrite),
        "delete_file" => Some(OperationType::FileDelete),
        "list_directory" | "list_dir" => Some(OperationType::DirRead),
        "create_directory" | "create_dir" => Some(OperationType::DirCreate),
        "delete_directory" | "delete_dir" => Some(OperationType::DirDelete),
        "exec" | "execute_command" | "shell" | "exec_async" | "cron" => Some(OperationType::ProcessExec),
        "spawn" => Some(OperationType::ProcessSpawn),
        "kill" | "kill_process" => Some(OperationType::ProcessKill),
        "download" | "install_skill" => Some(OperationType::NetworkDownload),
        "upload" => Some(OperationType::NetworkUpload),
        "http_request" | "web_request" | "web_fetch" | "web_search"
        | "cluster_rpc" | "find_skills" => Some(OperationType::NetworkRequest),
        "screen_capture" => Some(OperationType::FileWrite),
        _ => None,
    }
}

/// Extract target from tool arguments.
pub fn extract_target(tool_name: &str, args: &serde_json::Value) -> String {
    match tool_name {
        "read_file" | "write_file" | "edit_file" | "append_file" | "delete_file" => {
            args.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string()
        }
        "list_directory" | "list_dir" | "create_directory" | "create_dir" | "delete_directory" | "delete_dir" => {
            args.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string()
        }
        "exec" | "execute_command" | "spawn" | "shell" | "exec_async" => {
            args.get("command").and_then(|v| v.as_str()).unwrap_or("").to_string()
        }
        "download" | "upload" | "http_request" | "web_request" | "web_fetch" | "web_search"
        | "find_skills" => {
            args.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string()
        }
        "cluster_rpc" => {
            args.get("peer_id").and_then(|v| v.as_str()).unwrap_or("").to_string()
        }
        "cron" => {
            args.get("command").or_else(|| args.get("message"))
                .and_then(|v| v.as_str()).unwrap_or("").to_string()
        }
        "screen_capture" => {
            args.get("save_path").or_else(|| args.get("path"))
                .and_then(|v| v.as_str()).unwrap_or("").to_string()
        }
        "install_skill" => {
            args.get("url").or_else(|| args.get("source"))
                .and_then(|v| v.as_str()).unwrap_or("").to_string()
        }
        _ => String::new(),
    }
}

/// Extract URL from tool arguments.
pub fn extract_url(tool_name: &str, args: &serde_json::Value) -> String {
    match tool_name {
        "download" | "upload" | "http_request" | "web_request" | "web_fetch" | "web_search"
        | "install_skill" | "find_skills" => {
            args.get("url").or_else(|| args.get("source"))
                .and_then(|v| v.as_str()).unwrap_or("").to_string()
        }
        "cluster_rpc" => {
            args.get("peer_id").and_then(|v| v.as_str()).unwrap_or("").to_string()
        }
        _ => String::new(),
    }
}

/// Check if a command is safe.
pub fn is_safe_command(command: &str) -> (bool, String) {
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
        raw.iter().filter_map(|p| regex::Regex::new(p).ok()).collect()
    });

    for re in patterns {
        if re.is_match(command) {
            return (false, "command contains dangerous pattern".to_string());
        }
    }
    (true, String::new())
}

/// Validate path is within workspace and safe.
pub fn validate_path(path: &str, workspace: &str) -> Result<String, String> {
    let abs_path = std::path::Path::new(path)
        .canonicalize()
        .unwrap_or_else(|_| std::path::PathBuf::from(path));

    if !workspace.is_empty() {
        let abs_workspace = std::path::Path::new(workspace)
            .canonicalize()
            .unwrap_or_else(|_| std::path::PathBuf::from(workspace));

        match abs_path.strip_prefix(&abs_workspace) {
            Ok(rel) => {
                if rel.starts_with("..") {
                    return Err("access denied: path outside workspace".to_string());
                }
            }
            Err(_) => {
                // If strip_prefix fails, check if the path starts with workspace
                if !abs_path.starts_with(&abs_workspace) {
                    return Err("access denied: path outside workspace".to_string());
                }
            }
        }
    }

    // Check dangerous system paths
    let path_str = abs_path.to_string_lossy();
    let dangerous = [
        "/etc/passwd", "/etc/shadow", "/etc/sudoers",
        "C:\\Windows\\System32\\drivers\\etc\\hosts",
    ];
    for d in &dangerous {
        if path_str.starts_with(d) {
            return Err("access denied: protected system path".to_string());
        }
    }

    Ok(abs_path.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests;
