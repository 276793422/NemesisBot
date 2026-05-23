//! Shell command execution tool with safety validation.
//!
//! Features regex-based deny patterns, Windows command preprocessing,
//! workspace path guarding, allowlist mode, and output truncation.

use crate::registry::Tool;
use crate::types::ToolResult;
use async_trait::async_trait;
use regex::Regex;
use std::path::PathBuf;
use std::time::Duration;

/// Maximum output length before truncation.
const MAX_OUTPUT_LEN: usize = 10_000;

/// Default deny patterns compiled from regex strings.
/// These mirror the Go version's `defaultDenyPatterns`.
fn default_deny_patterns() -> Vec<Regex> {
    let patterns: &[&str] = &[
        // Destructive file operations
        r"\brm\s+-[rf]{1,2}\b",
        r"\bdel\s+/[fq]\b",
        r"\brmdir\s+/s\b",
        r"\b(format|mkfs|diskpart)\b\s",
        r"\bdd\s+if=",
        r">\s*/dev/sd[a-z]\b",
        // System control
        r"\b(shutdown|reboot|poweroff)\b",
        // Fork bomb
        r":\(\)\s*\{.*\};\s*:",
        // Command substitution
        r"\$\([^)]+\)",
        r"\$\{[^}]+\}",
        r"`[^`]+`",
        // Pipe to shell
        r"\|\s*sh\b",
        r"\|\s*bash\b",
        // Chained rm
        r";\s*rm\s+-[rf]",
        r"&&\s*rm\s+-[rf]",
        r"\|\|\s*rm\s+-[rf]",
        // Redirection exploits
        r">\s*/dev/null\s*>&?\s*\d?",
        r"<<\s*(?i)EOF",
        // Subshell data exfiltration
        r"\$\(\s*cat\s+",
        r"\$\(\s*curl\s+",
        r"\$\(\s*wget\s+",
        r"\$\(\s*which\s+",
        // Privilege escalation
        r"\bsudo\b",
        r"\bchmod\s+[0-7]{3,4}\b",
        r"\bchown\b",
        r"\bpkill\b",
        r"\bkillall\b",
        r"\bkill\s+-[9]\b",
        // Remote code execution
        r"\bcurl\b.*\|\s*(sh|bash)",
        r"\bwget\b.*\|\s*(sh|bash)",
        // Package management
        r"\bnpm\s+install\s+-g\b",
        r"\bpip\s+install\s+--user\b",
        r"\bapt\s+(install|remove|purge)\b",
        r"\byum\s+(install|remove)\b",
        r"\bdnf\s+(install|remove)\b",
        // Container escape
        r"\bdocker\s+run\b",
        r"\bdocker\s+exec\b",
        // Git force push
        r"\bgit\s+push\b",
        r"\bgit\s+force\b",
        // Remote access
        r"\bssh\b.*@",
        // Code execution
        r"\beval\b",
        r"\bsource\s+.*\.sh\b",
    ];

    patterns
        .iter()
        .filter_map(|p| Regex::new(p).ok())
        .collect()
}

/// Shell command execution tool with safety validation.
pub struct ShellTool {
    /// Working directory for command execution.
    workspace: PathBuf,
    /// Whether to restrict commands to the workspace.
    restrict: bool,
    /// Default timeout for command execution.
    default_timeout: Duration,
    /// Compiled deny patterns.
    deny_patterns: Vec<Regex>,
    /// Compiled allow patterns (if allowlist mode is active).
    allow_patterns: Vec<Regex>,
}

impl ShellTool {
    /// Create a new shell tool with default deny patterns.
    pub fn new(workspace: &str, restrict: bool) -> Self {
        Self {
            workspace: PathBuf::from(workspace),
            restrict,
            default_timeout: Duration::from_secs(60),
            deny_patterns: default_deny_patterns(),
            allow_patterns: Vec::new(),
        }
    }

    /// Create with a custom timeout.
    pub fn with_timeout(workspace: &str, restrict: bool, timeout: Duration) -> Self {
        Self {
            workspace: PathBuf::from(workspace),
            restrict,
            default_timeout: timeout,
            deny_patterns: default_deny_patterns(),
            allow_patterns: Vec::new(),
        }
    }

    /// Create a new shell tool with configuration options.
    ///
    /// Mirrors Go's `NewExecToolWithConfig`. Accepts optional custom deny patterns
    /// and an enable_deny_patterns flag. If enable_deny_patterns is false, all
    /// deny patterns are cleared (allowing all commands).
    pub fn new_with_config(
        workspace: &str,
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
            tracing::warn!("[Shell] Warning: deny patterns are disabled. All commands will be allowed.");
            Vec::new()
        };

        Self {
            workspace: PathBuf::from(workspace),
            restrict,
            default_timeout: Duration::from_secs(60),
            deny_patterns,
            allow_patterns: Vec::new(),
        }
    }

    /// Set the default timeout for command execution.
    ///
    /// A timeout of zero means no timeout (wait indefinitely).
    /// Mirrors Go's `ExecTool.SetTimeout()`.
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.default_timeout = timeout;
    }

    /// Set whether to restrict commands to the workspace directory.
    ///
    /// Mirrors Go's `ExecTool.SetRestrictToWorkspace()`.
    pub fn set_restrict_to_workspace(&mut self, restrict: bool) {
        self.restrict = restrict;
    }

    /// Get the short path (8.3 format) for a long Windows path.
    ///
    /// On Windows, this escapes spaces with the cmd.exe escape character `^`
    /// for paths that don't handle quoted paths well.
    /// On non-Windows, returns the path unchanged.
    /// Mirrors Go's `ExecTool.getShortPath()`.
    pub fn get_short_path(long_path: &str) -> Result<String, String> {
        if long_path.contains(' ') {
            // cmd.exe escape character for spaces
            Ok(long_path.replace(' ', "^ "))
        } else {
            Ok(long_path.to_string())
        }
    }

    /// Set custom deny patterns from regex strings.
    /// Invalid patterns are logged and skipped.
    pub fn set_deny_patterns(&mut self, patterns: &[&str]) {
        self.deny_patterns = patterns
            .iter()
            .filter_map(|p| {
                let re = Regex::new(p);
                match &re {
                    Ok(_) => re.ok(),
                    Err(e) => {
                        tracing::warn!("[Shell] Invalid deny pattern {:?}: {}", p, e);
                        None
                    }
                }
            })
            .collect();
    }

    /// Enable allowlist mode with the given patterns.
    /// When allowlist is non-empty, commands must match at least one allow pattern.
    pub fn set_allow_patterns(&mut self, patterns: &[&str]) -> Result<(), String> {
        let mut compiled = Vec::with_capacity(patterns.len());
        for p in patterns {
            let re = Regex::new(p).map_err(|e| format!("invalid allow pattern {:?}: {}", p, e))?;
            compiled.push(re);
        }
        self.allow_patterns = compiled;
        Ok(())
    }

    /// Disable all deny patterns (allowing all commands).
    pub fn clear_deny_patterns(&mut self) {
        self.deny_patterns.clear();
    }

    /// Guard a command against dangerous patterns and workspace violations.
    /// Returns an error message string if the command should be blocked.
    fn guard_command(&self, command: &str, cwd: &std::path::Path) -> Result<(), String> {
        let cmd = command.trim();
        let lower = cmd.to_lowercase();

        // Check deny patterns
        for pattern in &self.deny_patterns {
            if pattern.is_match(&lower) {
                return Err(
                    "command blocked by safety guard (dangerous pattern detected)".to_string()
                );
            }
        }

        // Check allowlist mode
        if !self.allow_patterns.is_empty() {
            let allowed = self.allow_patterns.iter().any(|p| p.is_match(&lower));
            if !allowed {
                return Err("command blocked by safety guard (not in allowlist)".to_string());
            }
        }

        // Workspace path guard
        if self.restrict {
            // Path traversal detection
            if cmd.contains("..\\") || cmd.contains("../") {
                return Err(
                    "command blocked by safety guard (path traversal detected)".to_string()
                );
            }

            // Extract paths from the command and validate they are within workspace
            let path_pattern =
                Regex::new(r#"[A-Za-z]:[\\/][^\s"'<>]+|/[^\s"'<>]+"#).unwrap();
            let cwd_abs = std::path::Path::new(cwd)
                .canonicalize()
                .unwrap_or_else(|_| cwd.to_path_buf());

            for raw in path_pattern.find_iter(cmd) {
                let raw_str = raw.as_str();
                let p = std::path::Path::new(raw_str);
                let abs_path = if p.is_absolute() {
                    p.to_path_buf()
                } else {
                    cwd_abs.join(p)
                };

                // Try to canonicalize; if it fails, use the joined path as-is
                let abs_canonical = abs_path.canonicalize().unwrap_or(abs_path);

                // Check if the absolute path starts with the workspace
                if let Ok(rel) = abs_canonical.strip_prefix(&cwd_abs) {
                    let rel_str = rel.to_string_lossy();
                    if rel_str.starts_with("..") {
                        return Err(
                            "command blocked by safety guard (path outside working dir)"
                                .to_string(),
                        );
                    }
                }
            }
        }

        Ok(())
    }

    /// Preprocess commands for Windows execution.
    /// - Replaces 'curl' with 'curl.exe'
    /// - Adds --max-time to curl commands
    /// - Normalizes Windows paths
    /// - Fixes path quoting issues
    pub fn preprocess_windows_command(command: &str) -> String {
        let mut result = command.to_string();

        // 1. Replace curl -> curl.exe (avoid PowerShell alias)
        let curl_exe_re = Regex::new(r"\bcurl\.exe\b").unwrap();
        if !curl_exe_re.is_match(&result) {
            let bare_curl_re = Regex::new(r"\bcurl\b").unwrap();
            result = bare_curl_re.replace_all(&result, "curl.exe").to_string();
        }

        // 2. Add --max-time to curl if not already present
        let lower = result.to_lowercase();
        if lower.contains("curl.exe") {
            let has_max_time = Regex::new(r"(?:--max-time\s*\d+|-m\s*\d+)")
                .unwrap()
                .is_match(&result);
            if !has_max_time {
                let curl_exe_re2 = Regex::new(r"\bcurl\.exe\b").unwrap();
                result = curl_exe_re2
                    .replace_all(&result, "curl.exe --max-time 300")
                    .to_string();
            }
        }

        // 3. Normalize Windows file paths
        result = Self::normalize_windows_paths(&result);

        // 4. Fix path quoting for problematic Windows commands
        result = Self::fix_windows_path_quoting(&result);

        result
    }

    /// Normalize forward slashes to backslashes in Windows file paths,
    /// preserving URLs, SSH URLs, and UNC paths.
    pub fn normalize_windows_paths(command: &str) -> String {
        let mut placeholders: Vec<(String, String)> = Vec::new();
        let mut placeholder_index = 0;

        let mut extract_and_protect = |re: &Regex, input: &str| -> String {
            let matches: Vec<String> = re.find_iter(input).map(|m| m.as_str().to_string()).collect();
            let mut result = input.to_string();
            for m in matches {
                let placeholder = format!("___URL_PLACEHOLDER_{}___", placeholder_index);
                placeholders.push((placeholder.clone(), m.clone()));
                result = result.replacen(m.as_str(), &placeholder, 1);
                placeholder_index += 1;
            }
            result
        };

        // Protect standard URL protocols (http, https, ftp, ws, etc.)
        let url_re = Regex::new(r#"(?:https?|ftps?|sftp|wss?|file|git|ssh)://[^\s"'<>]+"#).unwrap();
        let mut protected = extract_and_protect(&url_re, command);

        // Protect git SSH URLs (git@host:path)
        let git_ssh_re = Regex::new(r#"git@[^\s"'<>]+"#).unwrap();
        protected = extract_and_protect(&git_ssh_re, &protected);

        // Protect UNC network paths (\\server\share or //server/share)
        let unc_re = Regex::new(r#"(?:\\\\|//)[^\s"'<>]+"#).unwrap();
        protected = extract_and_protect(&unc_re, &protected);

        // Convert C:/path -> C:\path
        let path_re = Regex::new(r#"([A-Za-z]):((?:/[^/\s"']*)+)"#).unwrap();
        protected = path_re
            .replace_all(&protected, |caps: &regex::Captures| {
                let drive = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                let path = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                let normalized = path.replace('/', "\\");
                format!("{}:{}", drive, normalized)
            })
            .to_string();

        // Restore protected strings
        for (placeholder, original) in &placeholders {
            protected = protected.replacen(placeholder.as_str(), original.as_str(), 1);
        }

        protected
    }

    /// Fix path quoting for Windows commands that don't handle quoted paths well.
    fn fix_windows_path_quoting(command: &str) -> String {
        let problematic_commands = ["type", "dir", "copy", "move", "del", "ren", "md", "rd"];

        for cmd in &problematic_commands {
            let pattern_str = format!(r#"(?i)^\s*{}\s+"([^"]+)"(.*)$"#, regex::escape(cmd));
            if let Ok(re) = Regex::new(&pattern_str) {
                if let Some(caps) = re.captures(command) {
                    let path = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                    let rest = caps.get(2).map(|m| m.as_str()).unwrap_or("");

                    // If path has no spaces, remove quotes
                    if !path.contains(' ') {
                        return format!("{} {}{}", cmd, path, rest);
                    }

                    // For paths with spaces, escape with ^ (cmd.exe escape character)
                    let escaped = path.replace(' ', "^ ");
                    return format!("{} {}{}", cmd, escaped, rest);
                }
            }
        }

        command.to_string()
    }

    /// Truncate output to MAX_OUTPUT_LEN characters.
    fn truncate_output(output: &str) -> String {
        if output.len() > MAX_OUTPUT_LEN {
            let truncated = &output[..MAX_OUTPUT_LEN];
            let remaining = output.len() - MAX_OUTPUT_LEN;
            format!("{}\n... (truncated, {} more chars)", truncated, remaining)
        } else {
            output.to_string()
        }
    }
}

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        "Execute a shell command and return its output"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in seconds (default: 60)"
                },
                "cwd": {
                    "type": "string",
                    "description": "Working directory for the command (default: workspace)"
                },
                "env": {
                    "type": "object",
                    "description": "Environment variables to set (key-value pairs)",
                    "additionalProperties": { "type": "string" }
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let mut command = match args["command"].as_str() {
            Some(c) => c.to_string(),
            None => return ToolResult::error("missing 'command' argument"),
        };

        // Check for empty command
        if command.trim().is_empty() {
            return ToolResult::error("empty command");
        }

        // Determine cwd
        let cwd = match args["cwd"].as_str() {
            Some(dir) => {
                let p = PathBuf::from(dir);
                if self.restrict {
                    let ws = self.workspace.to_string_lossy();
                    let target = if p.is_absolute() {
                        p
                    } else {
                        self.workspace.join(&p)
                    };
                    let target_str = target.to_string_lossy();
                    if !target_str.starts_with(ws.as_ref()) {
                        return ToolResult::error(&format!(
                            "cwd '{}' is outside workspace",
                            dir
                        ));
                    }
                    target
                } else if p.is_absolute() {
                    p
                } else {
                    self.workspace.join(&p)
                }
            }
            None => self.workspace.clone(),
        };

        // Guard the command against dangerous patterns and workspace violations
        if let Err(e) = self.guard_command(&command, &cwd) {
            tracing::warn!(
                command = %command.chars().take(100).collect::<String>(),
                reason = %e,
                "[Tools/Shell] Command blocked by safety guard"
            );
            return ToolResult::error(&e);
        }

        // Preprocess command for Windows
        if cfg!(target_os = "windows") {
            command = Self::preprocess_windows_command(&command);
        }

        // Parse timeout
        let timeout_secs = args["timeout"].as_u64().unwrap_or(self.default_timeout.as_secs());
        let timeout = Duration::from_secs(timeout_secs.min(600)); // Cap at 10 minutes

        // Determine shell based on platform
        let (shell, flag) = if cfg!(target_os = "windows") {
            ("cmd", "/C")
        } else {
            ("sh", "-c")
        };

        // Build command with optional env vars
        let mut cmd = tokio::process::Command::new(shell);
        cmd.arg(flag).arg(&command).current_dir(&cwd);

        if let Some(env_obj) = args["env"].as_object() {
            for (key, value) in env_obj {
                if let Some(val_str) = value.as_str() {
                    cmd.env(key, val_str);
                }
            }
        }

        // Execute the command
        let result = tokio::time::timeout(timeout, cmd.output()).await;

        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);

                let mut response = if stderr.is_empty() {
                    stdout.to_string()
                } else {
                    format!("{}\nSTDERR:\n{}", stdout, stderr)
                };

                if output.status.success() {
                    if response.trim().is_empty() {
                        response = "(no output)".to_string();
                    }
                    ToolResult::success(&Self::truncate_output(response.trim()))
                } else {
                    response.push_str(&format!(
                        "\nExit code: {}",
                        output.status.code().unwrap_or(-1)
                    ));
                    ToolResult::error(&Self::truncate_output(&response))
                }
            }
            Ok(Err(e)) => ToolResult::error(&format!("failed to execute command: {}", e)),
            Err(_) => ToolResult::error(&format!(
                "command timed out after {}s",
                timeout.as_secs()
            )),
        }
    }
}

#[cfg(test)]
mod tests;
