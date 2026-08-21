//! Claude Code subagent delegation tool (H7 / U13 half, minimal spawn
//! version, dsh-alignment second batch).
//!
//! Delegates a self-contained task to the local Claude Code CLI:
//! `claude --print -p "<prompt>" --output-format text`, stdout collected as
//! the result. This is the MINIMAL half-item per the goal: no Codex, no
//! Agent-SDK deep integration, no nested dsh, no CC hooks, no permission
//! pass-through (the CLI's own permission config governs the child; see the
//! note at the dispatch site in loop.rs — this tool call itself passes
//! through the normal security pipeline like any other tool).
//!
//! Registration is OPT-IN: `claude_code_tool.enabled = true` in config
//! (default false), AND the CLI must be locatable at registration time —
//! absent CLI ⇒ tool simply not registered (graceful degradation).

use crate::context::RequestContext;
use crate::loop_tools::Tool;
use async_trait::async_trait;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

/// Default wall-clock budget for one delegation.
const DEFAULT_TIMEOUT_SECS: u64 = 300;

/// Locate the Claude Code CLI: `where claude` (Windows) / `which claude`.
/// Returns the resolved path, or None when not installed.
pub fn find_claude_cli() -> Option<String> {
    let finder = if cfg!(windows) { "where" } else { "which" };
    let out = std::process::Command::new(finder)
        .arg("claude")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    let first = s.lines().next()?.trim().to_string();
    if first.is_empty() {
        None
    } else {
        Some(first)
    }
}

/// The delegation tool. Constructed only when the CLI was found AND the
/// config enabled it.
pub struct ClaudeCodeTool {
    cli_path: String,
    timeout_secs: u64,
}

impl ClaudeCodeTool {
    pub fn new(cli_path: String, timeout_secs: Option<u64>) -> Self {
        Self {
            cli_path,
            timeout_secs: timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS),
        }
    }
}

#[async_trait]
impl Tool for ClaudeCodeTool {
    fn description(&self) -> String {
        "将任务委派给本机的 Claude Code CLI 执行并返回其最终答复。适合借用 Claude 的编码/工具能力处理自包含的子任务。输入应为无需额外上下文即可执行的完整任务描述。".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "Self-contained task description for Claude Code (include all context it needs: paths, goals, constraints)."
                }
            },
            "required": ["prompt"]
        })
    }

    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String> {
        let v: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid arguments: {}", e))?;
        let prompt = v
            .get("prompt")
            .and_then(|p| p.as_str())
            .ok_or("Missing 'prompt' argument")?;
        if prompt.trim().is_empty() {
            return Err("'prompt' must not be empty".to_string());
        }

        // Run in a real directory: the session_key is NOT a path in general
        // (e.g. "web:chat123"), so only use it as a cwd base when its parent
        // actually exists on disk; otherwise fall back to the process cwd.
        let cwd = {
            let p = std::path::Path::new(&context.session_key)
                .parent()
                .map(|p| p.to_path_buf());
            match p {
                Some(dir) if dir.is_dir() => dir,
                _ => std::env::current_dir()
                    .unwrap_or_else(|_| std::path::PathBuf::from(".")),
            }
        };

        // H7 minimal: `--print` runs non-interactively and prints the final
        // answer to stdout. No permission flags passed — the CLI's own
        // settings (e.g. --permission-mode in the user's claude config)
        // govern the child.
        let mut cmd = Command::new(&self.cli_path);
        cmd.arg("--print")
            .arg("-p")
            .arg(prompt)
            .arg("--output-format")
            .arg("text")
            .current_dir(&cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            // Never open a console window (project background-process rule).
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }

        let out = tokio::time::timeout(Duration::from_secs(self.timeout_secs), cmd.output())
            .await
            .map_err(|_| {
                format!(
                    "Error: claude_code delegation timed out after {}s",
                    self.timeout_secs
                )
            })?
            .map_err(|e| format!("Error: failed to spawn claude CLI: {}", e))?;

        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        if !out.status.success() {
            return Ok(format!(
                "Error: claude CLI exited with {}.\nstdout:\n{}\nstderr:\n{}",
                out.status, stdout, stderr
            ));
        }
        Ok(stdout.trim().to_string())
    }
}

#[cfg(test)]
mod tests;
