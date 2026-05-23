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
                    tracing::info!("[Shell] Using custom deny patterns: {:?}", patterns);
                    patterns
                        .iter()
                        .filter_map(|p| {
                            let re = Regex::new(p);
                            match &re {
                                Ok(_) => re.ok(),
                                Err(e) => {
                                    tracing::warn!("[Shell] Invalid custom deny pattern {:?}: {}", p, e);
                                    None
                                }
                            }
                        })
                        .collect()
                }
                _ => default_deny_patterns(),
            }
        } else {
            tracing::warn!("[Shell] Deny patterns are disabled. All commands will be allowed.");
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
mod tests;
