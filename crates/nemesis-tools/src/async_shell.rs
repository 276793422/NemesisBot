//! Async shell execution tool - starts applications and returns quickly.

use crate::registry::Tool;
use crate::types::ToolResult;
use async_trait::async_trait;
use regex::Regex;
use std::path::PathBuf;
use std::time::Duration;

/// Default dangerous command patterns (lowercase).
const DEFAULT_DENY_PATTERNS: &[&str] = &[
    "rm -rf /",
    "rm -rf /*",
    "rm -rf ~",
    "sudo rm",
    "sudo ",
    "mkfs.",
    "dd if=",
    ":(){ :|:& };:",
    "> /dev/sda",
    "chmod -R 777 /",
    "chown -R",
    "shutdown",
    "reboot",
    "halt",
    "poweroff",
    "init 0",
    "init 6",
    "format ",
    "del /f /s /q C:",
    "rd /s /q C:",
    "net user",
    "net localgroup administrators",
];

/// Compile the default deny patterns into Regex objects.
/// These use case-insensitive substring matching (same approach as the Go version
/// which lowercases both command and pattern before comparison).
fn default_deny_patterns() -> Vec<Regex> {
    DEFAULT_DENY_PATTERNS
        .iter()
        .filter_map(|p| Regex::new(&format!("(?i){}", regex::escape(p))).ok())
        .collect()
}

/// Async exec tool - starts applications asynchronously.
pub struct AsyncExecTool {
    working_dir: PathBuf,
    default_wait: Duration,
    restrict: bool,
    /// Compiled deny patterns (regex-based, matching Go version).
    deny_patterns: Vec<Regex>,
}

impl AsyncExecTool {
    /// Create a new async exec tool with default deny patterns.
    pub fn new(working_dir: &str, restrict: bool) -> Self {
        Self {
            working_dir: PathBuf::from(working_dir),
            default_wait: Duration::from_secs(3),
            restrict,
            deny_patterns: default_deny_patterns(),
        }
    }

    /// Create a new async exec tool with configuration options.
    ///
    /// Mirrors Go's `NewAsyncExecToolWithConfig`. Accepts optional custom deny
    /// patterns and an `enable_deny_patterns` flag. If `enable_deny_patterns`
    /// is false, all deny patterns are cleared (allowing all commands).
    /// If custom patterns are provided and non-empty, they replace the defaults.
    pub fn new_with_config(
        working_dir: &str,
        restrict: bool,
        custom_deny_patterns: Option<&[&str]>,
        enable_deny_patterns: bool,
    ) -> Self {
        let deny_patterns = if enable_deny_patterns {
            match custom_deny_patterns {
                Some(patterns) if !patterns.is_empty() => {
                    tracing::info!("Using custom deny patterns: {:?}", patterns);
                    patterns
                        .iter()
                        .filter_map(|p| {
                            let re = Regex::new(p);
                            match &re {
                                Ok(_) => re.ok(),
                                Err(e) => {
                                    tracing::warn!("Invalid custom deny pattern {:?}: {}", p, e);
                                    None
                                }
                            }
                        })
                        .collect()
                }
                _ => default_deny_patterns(),
            }
        } else {
            tracing::warn!("Deny patterns are disabled. All commands will be allowed.");
            Vec::new()
        };

        Self {
            working_dir: PathBuf::from(working_dir),
            default_wait: Duration::from_secs(3),
            restrict,
            deny_patterns,
        }
    }

    /// Set the default wait time for process startup confirmation.
    pub fn set_default_wait(&mut self, wait: Duration) {
        self.default_wait = wait;
    }

    /// Set whether to restrict commands to the workspace directory.
    pub fn set_restrict_to_workspace(&mut self, restrict: bool) {
        self.restrict = restrict;
    }

    /// Set allowed command patterns (additional patterns beyond the deny list).
    /// Currently the tool uses a deny-list approach; this is reserved for future
    /// allow-list support where only matching patterns would be permitted.
    pub fn set_allow_patterns(&mut self, _patterns: &[&str]) {
        // Reserved for future allow-list implementation
    }

    /// Check if a command contains dangerous patterns.
    fn is_dangerous(&self, command: &str) -> bool {
        self.deny_patterns
            .iter()
            .any(|p| p.is_match(command))
    }

    /// Guard command against security policies.
    fn guard_command(&self, command: &str) -> Result<(), String> {
        let trimmed = command.trim();
        if trimmed.is_empty() {
            return Err("empty command".to_string());
        }
        if self.is_dangerous(trimmed) {
            return Err("Command blocked by safety guard (dangerous pattern detected)".to_string());
        }
        if self.restrict {
            let lower = trimmed.to_lowercase();
            if lower.contains("..\\") || lower.contains("../") {
                return Err(
                    "Command blocked by safety guard (path traversal detected)".to_string(),
                );
            }
        }
        Ok(())
    }

    /// Extract process name from a command string.
    fn extract_process_name(command: &str) -> String {
        let trimmed = command.trim().trim_matches('"');
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.is_empty() {
            return String::new();
        }
        let exec_name = parts[0];
        let name = std::path::Path::new(exec_name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(exec_name);
        name.to_string()
    }

    /// Check if a process is running by name (best-effort).
    async fn check_process_running(process_name: &str) -> bool {
        if cfg!(target_os = "windows") {
            // Use tasklist to check
            let result = tokio::process::Command::new("tasklist")
                .args(["/FI", &format!("IMAGENAME eq {}.exe", process_name), "/NH"])
                .output()
                .await;
            match result {
                Ok(output) => {
                    let stdout = String::from_utf8_lossy(&output.stdout).to_lowercase();
                    stdout.contains(&format!("{}.exe", process_name.to_lowercase()))
                }
                Err(_) => false,
            }
        } else {
            // Use pgrep on Unix
            let result = tokio::process::Command::new("pgrep")
                .args(["-x", process_name])
                .output()
                .await;
            match result {
                Ok(output) => !output.stdout.is_empty(),
                Err(_) => false,
            }
        }
    }
}

#[async_trait]
impl Tool for AsyncExecTool {
    fn name(&self) -> &str {
        "exec_async"
    }

    fn description(&self) -> &str {
        "Start an application asynchronously and return quickly after confirming startup"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "description": "The application command to start"},
                "working_dir": {"type": "string", "description": "Working directory (optional)"},
                "wait_seconds": {
                    "type": "integer",
                    "description": "Seconds to wait for startup confirmation (1-10, default: 3)",
                    "default": 3,
                    "minimum": 1,
                    "maximum": 10
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let command = match args["command"].as_str() {
            Some(c) => c,
            None => return ToolResult::error("command is required"),
        };

        // Resolve working directory
        let cwd = match args["working_dir"].as_str() {
            Some(d) if !d.is_empty() => PathBuf::from(d),
            _ => self.working_dir.clone(),
        };

        // Guard
        if let Err(e) = self.guard_command(command) {
            return ToolResult::error(&e);
        }

        // Parse wait time
        let wait_secs = args["wait_seconds"]
            .as_u64()
            .unwrap_or(self.default_wait.as_secs())
            .clamp(1, 10);
        let wait = Duration::from_secs(wait_secs);

        let process_name = Self::extract_process_name(command);

        // Launch the process
        let (shell, flag): (&str, &str) = if cfg!(target_os = "windows") {
            ("cmd", "/C")
        } else {
            ("sh", "-c")
        };

        let launch_result = tokio::process::Command::new(shell)
            .arg(flag)
            .arg(command)
            .current_dir(&cwd)
            .spawn();

        let mut child = match launch_result {
            Ok(c) => c,
            Err(e) => {
                return ToolResult::error(&format!("Failed to start command: {}", e));
            }
        };

        // Wait a bit for the process to start
        tokio::time::sleep(wait).await;

        // Check if process is still running
        let still_running = match child.try_wait() {
            Ok(Some(_status)) => {
                // Process already exited
                false
            }
            Ok(None) => {
                // Still running
                true
            }
            Err(_) => {
                // Cannot determine status, check by name
                Self::check_process_running(&process_name).await
            }
        };

        // Also check by name if the child still appears running
        let confirmed = if still_running {
            true
        } else {
            Self::check_process_running(&process_name).await
        };

        if confirmed {
            ToolResult::success(&format!(
                "Application started successfully: {}\nProcess is running",
                command
            ))
        } else {
            ToolResult::error(&format!(
                "Application started but exited quickly (may have crashed): {}",
                command
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dangerous_detection() {
        let tool = AsyncExecTool::new(".", false);
        assert!(tool.is_dangerous("rm -rf /"));
        assert!(tool.is_dangerous("sudo apt install foo"));
        assert!(tool.is_dangerous("shutdown"));
        assert!(!tool.is_dangerous("echo hello"));
        assert!(!tool.is_dangerous("notepad.exe"));
    }

    #[test]
    fn test_extract_process_name() {
        assert_eq!(AsyncExecTool::extract_process_name("notepad.exe"), "notepad");
        assert_eq!(
            AsyncExecTool::extract_process_name("notepad.exe README.md"),
            "notepad"
        );
        assert_eq!(AsyncExecTool::extract_process_name("  calc.exe  "), "calc");
        assert_eq!(AsyncExecTool::extract_process_name(""), "");
    }

    #[tokio::test]
    async fn test_empty_command_rejected() {
        let tool = AsyncExecTool::new(".", false);
        let result = tool
            .execute(&serde_json::json!({"command": "  "}))
            .await;
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_dangerous_command_rejected() {
        let tool = AsyncExecTool::new(".", false);
        let result = tool
            .execute(&serde_json::json!({"command": "rm -rf /"}))
            .await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("dangerous"));
    }

    #[tokio::test]
    async fn test_missing_command() {
        let tool = AsyncExecTool::new(".", false);
        let result = tool
            .execute(&serde_json::json!({}))
            .await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("required"));
    }

    #[tokio::test]
    async fn test_launch_simple_command() {
        let tool = AsyncExecTool::new(".", false);
        let result = tool
            .execute(&serde_json::json!({
                "command": "echo hello_world",
                "wait_seconds": 1
            }))
            .await;
        // echo exits very fast, but the check by name may or may not find it
        // We just verify it doesn't panic or error on command rejection
        assert!(
            !result.for_llm.contains("blocked"),
            "Should not block echo: {}",
            result.for_llm
        );
    }

    // ============================================================
    // Additional tests for missing coverage
    // ============================================================

    #[test]
    fn test_is_dangerous_all_patterns() {
        let tool = AsyncExecTool::new(".", false);
        // Test all known dangerous patterns
        assert!(tool.is_dangerous("rm -rf /"));
        assert!(tool.is_dangerous("rm -rf /*"));
        assert!(tool.is_dangerous("rm -rf ~"));
        assert!(tool.is_dangerous("sudo rm file"));
        assert!(tool.is_dangerous("sudo something"));
        assert!(tool.is_dangerous("mkfs.ext4"));
        assert!(tool.is_dangerous("dd if=/dev/zero"));
        assert!(tool.is_dangerous(":(){ :|:& };:"));
        assert!(tool.is_dangerous("> /dev/sda"));
        assert!(tool.is_dangerous("chmod -R 777 /"));
        assert!(tool.is_dangerous("chown -R user /"));
        assert!(tool.is_dangerous("shutdown"));
        assert!(tool.is_dangerous("reboot"));
        assert!(tool.is_dangerous("halt"));
        assert!(tool.is_dangerous("poweroff"));
        assert!(tool.is_dangerous("init 0"));
        assert!(tool.is_dangerous("init 6"));
        assert!(tool.is_dangerous("format C:"));
        assert!(tool.is_dangerous("del /f /s /q C:"));
        assert!(tool.is_dangerous("rd /s /q C:"));
        assert!(tool.is_dangerous("net user"));
        assert!(tool.is_dangerous("net localgroup administrators"));
    }

    #[test]
    fn test_is_dangerous_safe_commands() {
        let tool = AsyncExecTool::new(".", false);
        assert!(!tool.is_dangerous("echo hello"));
        assert!(!tool.is_dangerous("notepad.exe"));
        assert!(!tool.is_dangerous("python script.py"));
        assert!(!tool.is_dangerous("cargo build"));
        assert!(!tool.is_dangerous("ls -la"));
    }

    #[test]
    fn test_is_dangerous_case_insensitive() {
        let tool = AsyncExecTool::new(".", false);
        assert!(tool.is_dangerous("SHUTDOWN"));
        assert!(tool.is_dangerous("Sudo something"));
        assert!(tool.is_dangerous("REBOOT"));
    }

    #[test]
    fn test_extract_process_name_various() {
        assert_eq!(AsyncExecTool::extract_process_name("notepad.exe"), "notepad");
        assert_eq!(AsyncExecTool::extract_process_name("notepad.exe file.txt"), "notepad");
        assert_eq!(AsyncExecTool::extract_process_name("  calc.exe  "), "calc");
        assert_eq!(AsyncExecTool::extract_process_name(""), "");
        assert_eq!(AsyncExecTool::extract_process_name("python"), "python");
        assert_eq!(AsyncExecTool::extract_process_name("python script.py --arg"), "python");
        assert_eq!(AsyncExecTool::extract_process_name("code ."), "code");
        // Quoted path with spaces: first token after quote stripping is "C:\Program" (split by space)
        assert_eq!(AsyncExecTool::extract_process_name("\"C:\\Program Files\\app.exe\""), "Program");
    }

    #[test]
    fn test_guard_command_restricts_path_traversal() {
        let mut tool = AsyncExecTool::new(".", true);
        tool.set_restrict_to_workspace(true);

        let result = tool.guard_command("cat ../etc/passwd");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("path traversal"));

        let result = tool.guard_command("cat ..\\windows\\system32");
        assert!(result.is_err());
    }

    #[test]
    fn test_guard_command_allows_when_unrestricted() {
        let tool = AsyncExecTool::new(".", false);
        let result = tool.guard_command("cat ../somefile");
        assert!(result.is_ok());
    }

    #[test]
    fn test_tool_interface() {
        let tool = AsyncExecTool::new(".", false);
        assert_eq!(tool.name(), "exec_async");
        assert!(!tool.description().is_empty());
        let params = tool.parameters();
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["command"].is_object());
    }

    #[tokio::test]
    async fn test_set_default_wait() {
        let mut tool = AsyncExecTool::new(".", false);
        tool.set_default_wait(std::time::Duration::from_secs(5));
        // Verify tool still works
        let result = tool
            .execute(&serde_json::json!({"command": "echo test", "wait_seconds": 1}))
            .await;
        assert!(!result.for_llm.contains("blocked"));
    }

    #[tokio::test]
    async fn test_sudo_command_rejected() {
        let tool = AsyncExecTool::new(".", false);
        let result = tool
            .execute(&serde_json::json!({"command": "sudo apt install something"}))
            .await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("dangerous"));
    }

    #[tokio::test]
    async fn test_reboot_command_rejected() {
        let tool = AsyncExecTool::new(".", false);
        let result = tool
            .execute(&serde_json::json!({"command": "reboot"}))
            .await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("dangerous"));
    }

    #[tokio::test]
    async fn test_custom_working_dir() {
        let dir = tempfile::tempdir().unwrap();
        let tool = AsyncExecTool::new(".", false);

        let result = tool
            .execute(&serde_json::json!({
                "command": "echo custom_wd",
                "working_dir": dir.path().to_string_lossy().to_string(),
                "wait_seconds": 1
            }))
            .await;
        assert!(!result.for_llm.contains("blocked"), "Should not block: {}", result.for_llm);
    }

    #[test]
    fn test_set_allow_patterns_no_op() {
        let mut tool = AsyncExecTool::new(".", false);
        // Should not panic or change behavior
        tool.set_allow_patterns(&["echo"]);
        assert!(tool.guard_command("echo hello").is_ok());
    }

    #[test]
    fn test_new_with_config_default_patterns() {
        let tool = AsyncExecTool::new_with_config(".", false, None, true);
        // Default patterns should block dangerous commands
        assert!(tool.is_dangerous("shutdown"));
        assert!(tool.is_dangerous("rm -rf /"));
        assert!(!tool.is_dangerous("echo hello"));
    }

    #[test]
    fn test_new_with_config_custom_patterns() {
        let tool = AsyncExecTool::new_with_config(
            ".", false,
            Some(&[r"shutdown", r"reboot"]),
            true,
        );
        // Custom patterns: should match the custom regex patterns
        assert!(tool.is_dangerous("shutdown"));
        assert!(tool.is_dangerous("reboot"));
        // "rm -rf /" is NOT in the custom patterns list, so it should be allowed
        assert!(!tool.is_dangerous("rm -rf /"));
    }

    #[test]
    fn test_new_with_config_disabled_patterns() {
        let tool = AsyncExecTool::new_with_config(".", false, None, false);
        // When patterns are disabled, nothing should be dangerous
        assert!(!tool.is_dangerous("shutdown"));
        assert!(!tool.is_dangerous("rm -rf /"));
        assert!(!tool.is_dangerous("reboot"));
    }
}
