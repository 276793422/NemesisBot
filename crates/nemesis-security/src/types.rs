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
mod tests {
    use super::*;

    #[test]
    fn test_danger_levels() {
        assert_eq!(get_danger_level(OperationType::FileRead), DangerLevel::Low);
        assert_eq!(get_danger_level(OperationType::ProcessExec), DangerLevel::Critical);
        assert_eq!(get_danger_level(OperationType::FileWrite), DangerLevel::High);
    }

    #[test]
    fn test_tool_to_operation() {
        assert_eq!(tool_to_operation("read_file"), Some(OperationType::FileRead));
        assert_eq!(tool_to_operation("exec"), Some(OperationType::ProcessExec));
        assert_eq!(tool_to_operation("unknown"), None);
    }

    #[test]
    fn test_extract_target() {
        let args = serde_json::json!({"path": "/tmp/test.txt"});
        assert_eq!(extract_target("read_file", &args), "/tmp/test.txt");

        let args2 = serde_json::json!({"command": "ls -la"});
        assert_eq!(extract_target("exec", &args2), "ls -la");
    }

    #[test]
    fn test_is_safe_command() {
        assert!(is_safe_command("ls -la").0);
        assert!(is_safe_command("cat file.txt").0);
        assert!(!is_safe_command("rm -rf /").0);
        assert!(!is_safe_command("sudo apt install").0);
        assert!(!is_safe_command("shutdown -h now").0);
    }

    #[test]
    fn test_operation_type_display() {
        assert_eq!(OperationType::FileRead.to_string(), "file_read");
        assert_eq!(OperationType::ProcessExec.to_string(), "process_exec");
    }

    #[test]
    fn test_danger_level_ordering() {
        assert!(DangerLevel::Critical > DangerLevel::High);
        assert!(DangerLevel::High > DangerLevel::Medium);
        assert!(DangerLevel::Medium > DangerLevel::Low);
    }

    #[test]
    fn test_all_operation_types_display() {
        assert_eq!(OperationType::FileDelete.to_string(), "file_delete");
        assert_eq!(OperationType::DirRead.to_string(), "dir_read");
        assert_eq!(OperationType::DirCreate.to_string(), "dir_create");
        assert_eq!(OperationType::DirDelete.to_string(), "dir_delete");
        assert_eq!(OperationType::ProcessSpawn.to_string(), "process_spawn");
        assert_eq!(OperationType::ProcessKill.to_string(), "process_kill");
        assert_eq!(OperationType::ProcessSuspend.to_string(), "process_suspend");
        assert_eq!(OperationType::NetworkDownload.to_string(), "network_download");
        assert_eq!(OperationType::NetworkUpload.to_string(), "network_upload");
        assert_eq!(OperationType::HardwareI2C.to_string(), "hardware_i2c");
        assert_eq!(OperationType::HardwareSPI.to_string(), "hardware_spi");
        assert_eq!(OperationType::HardwareGPIO.to_string(), "hardware_gpio");
        assert_eq!(OperationType::SystemShutdown.to_string(), "system_shutdown");
        assert_eq!(OperationType::SystemReboot.to_string(), "system_reboot");
        assert_eq!(OperationType::RegistryRead.to_string(), "registry_read");
        assert_eq!(OperationType::RegistryWrite.to_string(), "registry_write");
        assert_eq!(OperationType::RegistryDelete.to_string(), "registry_delete");
    }

    #[test]
    fn test_danger_level_display() {
        assert_eq!(format!("{}", DangerLevel::Low), "LOW");
        assert_eq!(format!("{}", DangerLevel::Medium), "MEDIUM");
        assert_eq!(format!("{}", DangerLevel::High), "HIGH");
        assert_eq!(format!("{}", DangerLevel::Critical), "CRITICAL");
    }

    #[test]
    fn test_security_decision_display() {
        assert_eq!(format!("{}", SecurityDecision::Allowed), "allowed");
        assert_eq!(format!("{}", SecurityDecision::Denied), "denied");
        assert_eq!(format!("{}", SecurityDecision::RequireApproval), "require_approval");
    }

    #[test]
    fn test_tool_to_operation_all_mappings() {
        assert_eq!(tool_to_operation("write_file"), Some(OperationType::FileWrite));
        assert_eq!(tool_to_operation("edit_file"), Some(OperationType::FileWrite));
        assert_eq!(tool_to_operation("append_file"), Some(OperationType::FileWrite));
        assert_eq!(tool_to_operation("delete_file"), Some(OperationType::FileDelete));
        assert_eq!(tool_to_operation("list_directory"), Some(OperationType::DirRead));
        assert_eq!(tool_to_operation("list_dir"), Some(OperationType::DirRead));
        assert_eq!(tool_to_operation("create_directory"), Some(OperationType::DirCreate));
        assert_eq!(tool_to_operation("create_dir"), Some(OperationType::DirCreate));
        assert_eq!(tool_to_operation("delete_directory"), Some(OperationType::DirDelete));
        assert_eq!(tool_to_operation("delete_dir"), Some(OperationType::DirDelete));
        assert_eq!(tool_to_operation("spawn"), Some(OperationType::ProcessSpawn));
        assert_eq!(tool_to_operation("kill"), Some(OperationType::ProcessKill));
        assert_eq!(tool_to_operation("kill_process"), Some(OperationType::ProcessKill));
        assert_eq!(tool_to_operation("download"), Some(OperationType::NetworkDownload));
        assert_eq!(tool_to_operation("upload"), Some(OperationType::NetworkUpload));
        assert_eq!(tool_to_operation("http_request"), Some(OperationType::NetworkRequest));
        assert_eq!(tool_to_operation("web_request"), Some(OperationType::NetworkRequest));
    }

    #[test]
    fn test_extract_target_directory_ops() {
        let args = serde_json::json!({"path": "/tmp/mydir"});
        assert_eq!(extract_target("list_dir", &args), "/tmp/mydir");
        assert_eq!(extract_target("create_dir", &args), "/tmp/mydir");
        assert_eq!(extract_target("delete_dir", &args), "/tmp/mydir");
    }

    #[test]
    fn test_extract_target_network_ops() {
        let args = serde_json::json!({"url": "https://example.com"});
        assert_eq!(extract_target("download", &args), "https://example.com");
        assert_eq!(extract_target("upload", &args), "https://example.com");
        assert_eq!(extract_target("http_request", &args), "https://example.com");
    }

    #[test]
    fn test_extract_target_unknown_tool() {
        let args = serde_json::json!({"path": "/tmp/test"});
        assert_eq!(extract_target("unknown_tool", &args), "");
    }

    #[test]
    fn test_extract_target_missing_field() {
        let args = serde_json::json!({"other": "value"});
        assert_eq!(extract_target("read_file", &args), "");
        assert_eq!(extract_target("exec", &args), "");
    }

    #[test]
    fn test_extract_url() {
        let args = serde_json::json!({"url": "https://example.com/api"});
        assert_eq!(extract_url("download", &args), "https://example.com/api");
        assert_eq!(extract_url("http_request", &args), "https://example.com/api");
        assert_eq!(extract_url("exec", &args), "");
    }

    #[test]
    fn test_is_safe_command_safe() {
        assert!(is_safe_command("echo hello").0);
        assert!(is_safe_command("pwd").0);
        assert!(is_safe_command("git status").0);
        assert!(is_safe_command("python script.py").0);
        assert!(is_safe_command("npm install").0);
    }

    #[test]
    fn test_is_safe_command_dangerous() {
        assert!(!is_safe_command("rm -rf /").0);
        assert!(!is_safe_command("sudo rm -rf /").0);
        assert!(!is_safe_command("shutdown -h now").0);
        assert!(!is_safe_command("reboot").0);
        assert!(!is_safe_command("format C:").0);
        assert!(!is_safe_command("chmod 777 file").0);
        assert!(!is_safe_command("chown root file").0);
    }

    #[test]
    fn test_is_safe_command_case_insensitive() {
        assert!(!is_safe_command("RM -RF /").0);
        assert!(!is_safe_command("SUDO ls").0);
        assert!(!is_safe_command("SHUTDOWN now").0);
    }

    #[test]
    fn test_get_danger_level_all_types() {
        assert_eq!(get_danger_level(OperationType::FileRead), DangerLevel::Low);
        assert_eq!(get_danger_level(OperationType::DirRead), DangerLevel::Low);
        assert_eq!(get_danger_level(OperationType::NetworkRequest), DangerLevel::Medium);
        assert_eq!(get_danger_level(OperationType::NetworkDownload), DangerLevel::Medium);
        assert_eq!(get_danger_level(OperationType::HardwareI2C), DangerLevel::Medium);
        assert_eq!(get_danger_level(OperationType::FileWrite), DangerLevel::High);
        assert_eq!(get_danger_level(OperationType::FileDelete), DangerLevel::High);
        assert_eq!(get_danger_level(OperationType::DirCreate), DangerLevel::High);
        assert_eq!(get_danger_level(OperationType::DirDelete), DangerLevel::High);
        assert_eq!(get_danger_level(OperationType::ProcessSpawn), DangerLevel::High);
        assert_eq!(get_danger_level(OperationType::ProcessExec), DangerLevel::Critical);
        assert_eq!(get_danger_level(OperationType::ProcessKill), DangerLevel::Critical);
        assert_eq!(get_danger_level(OperationType::SystemShutdown), DangerLevel::Critical);
        assert_eq!(get_danger_level(OperationType::RegistryWrite), DangerLevel::Critical);
        assert_eq!(get_danger_level(OperationType::RegistryDelete), DangerLevel::Critical);
    }

    // ---- Permission tests ----

    #[test]
    fn test_permission_default_denies_all() {
        let perm = Permission::default();
        assert!(!perm.is_operation_allowed(&OperationType::FileRead));
        assert!(!perm.is_operation_allowed(&OperationType::FileWrite));
        assert!(!perm.is_operation_allowed(&OperationType::ProcessExec));
        assert!(!perm.is_operation_allowed(&OperationType::NetworkRequest));
    }

    #[test]
    fn test_permission_allowed_types() {
        let mut perm = Permission::new();
        perm.allowed_types.insert(OperationType::FileRead, true);
        perm.allowed_types.insert(OperationType::FileWrite, true);
        assert!(perm.is_operation_allowed(&OperationType::FileRead));
        assert!(perm.is_operation_allowed(&OperationType::FileWrite));
        assert!(!perm.is_operation_allowed(&OperationType::ProcessExec));
    }

    #[test]
    fn test_permission_explicit_false() {
        let mut perm = Permission::new();
        perm.allowed_types.insert(OperationType::FileRead, false);
        assert!(!perm.is_operation_allowed(&OperationType::FileRead));
    }

    #[test]
    fn test_permission_require_approval() {
        let mut perm = Permission::new();
        perm.require_approval.insert(OperationType::ProcessExec, true);
        assert!(perm.requires_approval(&OperationType::ProcessExec));
        assert!(!perm.requires_approval(&OperationType::FileRead));
    }

    #[test]
    fn test_permission_target_denied() {
        let mut perm = Permission::new();
        perm.denied_targets.push("/etc/passwd".to_string());
        assert!(perm.is_target_denied("/etc/passwd"));
        assert!(perm.is_target_denied("/etc/passwd.bak")); // contains
        assert!(!perm.is_target_denied("/tmp/test.txt"));
    }

    #[test]
    fn test_permission_target_allowed() {
        let mut perm = Permission::new();
        perm.allowed_targets.push("/workspace/*".to_string());
        assert!(perm.is_target_allowed("/workspace/file.txt"));
        assert!(!perm.is_target_allowed("/etc/passwd"));
    }

    #[test]
    fn test_permission_max_danger_level_default() {
        let perm = Permission::default();
        assert_eq!(perm.max_danger_level, DangerLevel::Low);
    }

    #[test]
    fn test_permission_max_danger_level_set() {
        let mut perm = Permission::new();
        perm.max_danger_level = DangerLevel::High;
        assert_eq!(perm.max_danger_level, DangerLevel::High);
    }

    // ---- Policy / PolicyRule serialization ----

    #[test]
    fn test_policy_rule_serialization() {
        let rule = PolicyRule {
            name: "deny_etc".to_string(),
            match_op_type: Some(OperationType::FileRead),
            match_target: Some("/etc/*".to_string()),
            match_user: None,
            match_source: None,
            min_danger: None,
            action: "deny".to_string(),
            reason: "system files".to_string(),
        };
        let json = serde_json::to_string(&rule).unwrap();
        assert!(json.contains("deny_etc"));
        assert!(json.contains("deny"));
        assert!(json.contains("FileRead"));

        let de: PolicyRule = serde_json::from_str(&json).unwrap();
        assert_eq!(de.name, "deny_etc");
        assert_eq!(de.match_op_type, Some(OperationType::FileRead));
    }

    #[test]
    fn test_policy_serialization() {
        let policy = Policy {
            name: "test_policy".to_string(),
            description: "test".to_string(),
            enabled: true,
            rules: vec![PolicyRule {
                name: "r1".to_string(),
                match_op_type: None,
                match_target: None,
                match_user: None,
                match_source: None,
                min_danger: None,
                action: "allow".to_string(),
                reason: String::new(),
            }],
            default_action: "deny".to_string(),
            log_only: false,
            require_mfa: false,
        };
        let json = serde_json::to_string(&policy).unwrap();
        assert!(json.contains("test_policy"));
        let de: Policy = serde_json::from_str(&json).unwrap();
        assert!(de.enabled);
        assert_eq!(de.rules.len(), 1);
    }

    #[test]
    fn test_policy_default_action() {
        let json = r#"{"name":"p","rules":[]}"#;
        let policy: Policy = serde_json::from_str(json).unwrap();
        assert_eq!(policy.default_action, "deny");
        assert!(policy.enabled);
        assert!(!policy.log_only);
        assert!(!policy.require_mfa);
    }

    #[test]
    fn test_security_rule_serialization() {
        let rule = SecurityRule {
            pattern: "/tmp/*".to_string(),
            action: "allow".to_string(),
            comment: "tmp access".to_string(),
        };
        let json = serde_json::to_string(&rule).unwrap();
        let de: SecurityRule = serde_json::from_str(&json).unwrap();
        assert_eq!(de.pattern, "/tmp/*");
        assert_eq!(de.action, "allow");
    }

    // ---- is_safe_command edge cases ----

    #[test]
    fn test_is_safe_command_del_flag() {
        assert!(!is_safe_command("del /f /q file.txt").0);
        assert!(!is_safe_command("del /q file.txt").0);
    }

    #[test]
    fn test_is_safe_command_dd() {
        assert!(!is_safe_command("dd if=/dev/zero of=/dev/sda").0);
    }

    #[test]
    fn test_is_safe_command_mixed_case_dd() {
        assert!(!is_safe_command("DD if=/dev/zero of=disk").0);
    }

    #[test]
    fn test_is_safe_command_chmod_variations() {
        assert!(!is_safe_command("chmod 777 /etc/shadow").0);
        assert!(!is_safe_command("chmod 4755 binary").0);
        assert!(!is_safe_command("chmod 644").0);
    }

    #[test]
    fn test_is_safe_command_poweroff() {
        assert!(!is_safe_command("poweroff").0);
        assert!(!is_safe_command("reboot now").0);
    }

    // ---- validate_path tests ----

    #[test]
    fn test_validate_path_empty_workspace() {
        let result = validate_path("/tmp/test.txt", "");
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_path_system_paths() {
        // These are always blocked regardless of workspace
        assert!(validate_path("/etc/passwd", "/home/user").is_err());
        assert!(validate_path("/etc/shadow", "/home/user").is_err());
        assert!(validate_path("/etc/sudoers", "/home/user").is_err());
    }

    // ---- extract_url tests ----

    #[test]
    fn test_extract_url_all_tools() {
        let args = serde_json::json!({"url": "https://api.example.com/v1"});
        assert_eq!(extract_url("download", &args), "https://api.example.com/v1");
        assert_eq!(extract_url("upload", &args), "https://api.example.com/v1");
        assert_eq!(extract_url("http_request", &args), "https://api.example.com/v1");
        assert_eq!(extract_url("web_request", &args), "https://api.example.com/v1");
    }

    #[test]
    fn test_extract_url_missing() {
        let args = serde_json::json!({"path": "/tmp"});
        assert_eq!(extract_url("read_file", &args), "");
        assert_eq!(extract_url("download", &args), "");
    }

    // ---- ToolInvocation ----

    #[test]
    fn test_tool_invocation_fields() {
        let inv = ToolInvocation {
            tool_name: "exec".to_string(),
            args: serde_json::json!({"command": "ls"}),
            user: "admin".to_string(),
            source: "cli".to_string(),
            metadata: {
                let mut m = HashMap::new();
                m.insert("key".to_string(), "value".to_string());
                m
            },
        };
        assert_eq!(inv.tool_name, "exec");
        assert_eq!(inv.user, "admin");
        assert_eq!(inv.source, "cli");
        assert_eq!(inv.metadata.get("key"), Some(&"value".to_string()));
    }

    // ---- SecurityDecision ----

    #[test]
    fn test_security_decision_equality() {
        assert_eq!(SecurityDecision::Allowed, SecurityDecision::Allowed);
        assert_ne!(SecurityDecision::Allowed, SecurityDecision::Denied);
        assert_ne!(SecurityDecision::Denied, SecurityDecision::RequireApproval);
    }

    // ---- DangerLevel ordering ----

    #[test]
    fn test_danger_level_ord_total() {
        assert!(DangerLevel::Low < DangerLevel::Medium);
        assert!(DangerLevel::Medium < DangerLevel::High);
        assert!(DangerLevel::High < DangerLevel::Critical);
        assert!(DangerLevel::Low < DangerLevel::Critical);
    }

    // ---- OperationType serde ----

    #[test]
    fn test_operation_type_serde_roundtrip() {
        for op in [
            OperationType::FileRead, OperationType::FileWrite,
            OperationType::ProcessExec, OperationType::NetworkRequest,
            OperationType::RegistryWrite, OperationType::SystemShutdown,
        ] {
            let json = serde_json::to_string(&op).unwrap();
            let de: OperationType = serde_json::from_str(&json).unwrap();
            assert_eq!(op, de);
        }
    }

    // ---- DangerLevel serde ----

    #[test]
    fn test_danger_level_serde_roundtrip() {
        for dl in [DangerLevel::Low, DangerLevel::Medium, DangerLevel::High, DangerLevel::Critical] {
            let json = serde_json::to_string(&dl).unwrap();
            let de: DangerLevel = serde_json::from_str(&json).unwrap();
            assert_eq!(dl, de);
        }
    }

    // ---- NetworkUpload danger level ----

    #[test]
    fn test_network_upload_danger_level() {
        assert_eq!(get_danger_level(OperationType::NetworkUpload), DangerLevel::Medium);
    }

    #[test]
    fn test_hardware_operations_danger_level() {
        assert_eq!(get_danger_level(OperationType::HardwareSPI), DangerLevel::Medium);
        assert_eq!(get_danger_level(OperationType::HardwareGPIO), DangerLevel::Medium);
    }

    #[test]
    fn test_system_operations_danger_level() {
        assert_eq!(get_danger_level(OperationType::SystemReboot), DangerLevel::Critical);
        assert_eq!(get_danger_level(OperationType::SystemConfig), DangerLevel::Critical);
        assert_eq!(get_danger_level(OperationType::SystemService), DangerLevel::Critical);
        assert_eq!(get_danger_level(OperationType::SystemInstall), DangerLevel::Critical);
    }

    #[test]
    fn test_registry_read_danger_level() {
        assert_eq!(get_danger_level(OperationType::RegistryRead), DangerLevel::Medium);
    }

    #[test]
    fn test_process_suspend_danger_level() {
        assert_eq!(get_danger_level(OperationType::ProcessSuspend), DangerLevel::Medium);
    }

    // ---- New tool mappings (shell, exec_async, web_fetch, etc.) ----

    #[test]
    fn test_tool_to_operation_shell_and_async() {
        assert_eq!(tool_to_operation("shell"), Some(OperationType::ProcessExec));
        assert_eq!(tool_to_operation("exec_async"), Some(OperationType::ProcessExec));
    }

    #[test]
    fn test_tool_to_operation_web_tools() {
        assert_eq!(tool_to_operation("web_fetch"), Some(OperationType::NetworkRequest));
        assert_eq!(tool_to_operation("web_search"), Some(OperationType::NetworkRequest));
    }

    #[test]
    fn test_tool_to_operation_screen_capture() {
        assert_eq!(tool_to_operation("screen_capture"), Some(OperationType::FileWrite));
    }

    #[test]
    fn test_tool_to_operation_install_skill() {
        assert_eq!(tool_to_operation("install_skill"), Some(OperationType::NetworkDownload));
    }

    #[test]
    fn test_extract_target_shell() {
        let args = serde_json::json!({"command": "run ./malware.exe"});
        assert_eq!(extract_target("shell", &args), "run ./malware.exe");
    }

    #[test]
    fn test_extract_target_exec_async() {
        let args = serde_json::json!({"command": "python script.py"});
        assert_eq!(extract_target("exec_async", &args), "python script.py");
    }

    #[test]
    fn test_extract_target_web_fetch() {
        let args = serde_json::json!({"url": "http://example.com/payload"});
        assert_eq!(extract_target("web_fetch", &args), "http://example.com/payload");
    }

    #[test]
    fn test_extract_target_web_search() {
        let args = serde_json::json!({"url": "http://search.example.com"});
        assert_eq!(extract_target("web_search", &args), "http://search.example.com");
    }

    #[test]
    fn test_extract_target_screen_capture() {
        let args = serde_json::json!({"save_path": "/tmp/cap.png"});
        assert_eq!(extract_target("screen_capture", &args), "/tmp/cap.png");

        let args2 = serde_json::json!({"path": "/tmp/cap2.png"});
        assert_eq!(extract_target("screen_capture", &args2), "/tmp/cap2.png");
    }

    #[test]
    fn test_extract_target_install_skill() {
        let args = serde_json::json!({"url": "https://github.com/user/skill"});
        assert_eq!(extract_target("install_skill", &args), "https://github.com/user/skill");

        let args2 = serde_json::json!({"source": "https://github.com/user/skill2"});
        assert_eq!(extract_target("install_skill", &args2), "https://github.com/user/skill2");
    }

    #[test]
    fn test_extract_url_web_tools() {
        let args = serde_json::json!({"url": "https://api.example.com/v1"});
        assert_eq!(extract_url("web_fetch", &args), "https://api.example.com/v1");
        assert_eq!(extract_url("web_search", &args), "https://api.example.com/v1");

        let args2 = serde_json::json!({"source": "https://github.com/skill"});
        assert_eq!(extract_url("install_skill", &args2), "https://github.com/skill");
    }

    // ---- cluster_rpc, find_skills, cron mappings ----

    #[test]
    fn test_tool_to_operation_cluster_rpc() {
        assert_eq!(tool_to_operation("cluster_rpc"), Some(OperationType::NetworkRequest));
    }

    #[test]
    fn test_tool_to_operation_find_skills() {
        assert_eq!(tool_to_operation("find_skills"), Some(OperationType::NetworkRequest));
    }

    #[test]
    fn test_tool_to_operation_cron() {
        assert_eq!(tool_to_operation("cron"), Some(OperationType::ProcessExec));
    }

    #[test]
    fn test_extract_target_cluster_rpc() {
        let args = serde_json::json!({"peer_id": "bot-2", "action": "ping"});
        assert_eq!(extract_target("cluster_rpc", &args), "bot-2");
    }

    #[test]
    fn test_extract_target_find_skills() {
        let args = serde_json::json!({"query": "docker"});
        assert_eq!(extract_target("find_skills", &args), "");
    }

    #[test]
    fn test_extract_target_cron() {
        let args = serde_json::json!({"command": "rm -rf /tmp/old", "action": "add"});
        assert_eq!(extract_target("cron", &args), "rm -rf /tmp/old");

        let args2 = serde_json::json!({"message": "reminder text", "action": "add"});
        assert_eq!(extract_target("cron", &args2), "reminder text");
    }

    #[test]
    fn test_extract_url_cluster_rpc() {
        let args = serde_json::json!({"peer_id": "bot-3", "action": "chat"});
        assert_eq!(extract_url("cluster_rpc", &args), "bot-3");
    }
}
