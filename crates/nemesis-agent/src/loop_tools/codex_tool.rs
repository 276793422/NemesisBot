//! Codex CLI delegation tool (I4 / U13 other half, minimal spawn version,
//! dsh-alignment third batch).
//!
//! Delegates a self-contained task to the local OpenAI Codex CLI:
//! `codex exec "<prompt>"` (one-shot non-interactive execution; stdout
//! collected as the result). VERIFICATION STATUS: the codex CLI was NOT
//! present on this machine when this was written, so the exec sub-command
//! shape follows Codex CLI's public documentation (`codex exec` runs a
//! prompt non-interactively); if your codex build differs, adjust the args
//! below — the tool degrades to a structured error, never panics.
//!
//! Same minimal scope as its H7 claude_code sibling: no app-server
//! JSON-RPC session, no permission pass-through (the CLI's own config
//! governs the child). Registration is OPT-IN (`agents.codex_tool.enabled`,
//! default false) AND requires the CLI on PATH at registration time.

use crate::context::RequestContext;
use crate::loop_tools::Tool;
use async_trait::async_trait;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

/// Default wall-clock budget for one delegation.
const DEFAULT_TIMEOUT_SECS: u64 = 300;

/// Locate the Codex CLI: `where codex` (Windows) / `which codex`.
/// Returns the resolved path, or None when not installed.
pub fn find_codex_cli() -> Option<String> {
    let finder = if cfg!(windows) { "where" } else { "which" };
    let out = std::process::Command::new(finder)
        .arg("codex")
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
pub struct CodexTool {
    cli_path: String,
    timeout_secs: u64,
}

impl CodexTool {
    pub fn new(cli_path: String, timeout_secs: Option<u64>) -> Self {
        Self {
            cli_path,
            timeout_secs: timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS),
        }
    }
}

#[async_trait]
impl Tool for CodexTool {
    fn description(&self) -> String {
        "将任务委派给本机的 OpenAI Codex CLI 执行并返回其最终答复。适合借用 Codex 的编码/代理能力处理自包含的子任务。输入应为无需额外上下文即可执行的完整任务描述。".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "Self-contained task description for Codex (include all context it needs: paths, goals, constraints)."
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

        // I4 minimal: `codex exec` runs the prompt non-interactively and
        // prints the final answer to stdout. No permission flags passed —
        // the CLI's own config governs the child. (See the module doc:
        // exec-shape per public docs, CLI absent on the dev machine.)
        //
        // Round-5 fix: `--skip-git-repo-check` — codex exec refuses to run
        // outside a git repository by default, and the gateway's cwd is
        // typically the (non-git) workspace home, which would make EVERY
        // delegation fail with codex's not-inside-a-git-repo error. The
        // in-repo codex_cli provider (nemesis-providers/src/codex_cli.rs)
        // passes the same flag.
        let mut cmd = Command::new(&self.cli_path);
        // Timeout safety: if tokio::time::timeout drops the output() future
        // at the deadline, the spawned child must die with it — without
        // kill_on_drop the CLI process would outlive the tool call as an
        // orphan (second-pass review fix).
        cmd.kill_on_drop(true);
        cmd.arg("exec")
            .arg("--skip-git-repo-check")
            .arg(prompt)
            .current_dir(&cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            // Never open a console window (project background-process rule).
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }

        // Spawn explicitly so a timeout KILLS THE PROCESS TREE: killing
        // only the direct child (bat wrapper) leaves grandchildren holding
        // the inherited stdout/stderr pipes open, and output() futures hang
        // on pipe close until every holder exits (Windows inheritance
        // classic — this is why the earlier kill_on_drop fix alone did not
        // shorten the timeout path).
        let child = cmd
            .spawn()
            .map_err(|e| format!("Error: failed to spawn codex CLI: {}", e))?;
        // Capture the pid BEFORE moving `child` into wait_with_output (the
        // timeout arm needs it for the tree kill).
        let child_pid = child.id();
        let out = match tokio::time::timeout(Duration::from_secs(self.timeout_secs), child.wait_with_output()).await {
            Ok(r) => r.map_err(|e| format!("Error: codex CLI output wait failed: {}", e))?,
            Err(_) => {
                #[cfg(windows)]
                {
                    // Tree-kill (the CLI may spawn workers that inherit the
                    // pipes).
                    use std::os::windows::process::CommandExt;
                    let _ = std::process::Command::new("taskkill")
                        .args(["/PID", &child_pid.unwrap_or_default().to_string(), "/T", "/F"])
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .creation_flags(0x0800_0000)
                        .status();
                }
                #[cfg(not(windows))]
                {
                    // POSIX: `child` was moved into wait_with_output;
                    // kill_on_drop(true) (set at spawn) kills it when the
                    // dropped future's Child reaper runs. Worker reaping is
                    // the CLI's own responsibility on POSIX.
                }
                return Err(format!(
                    "Error: codex delegation timed out after {}s",
                    self.timeout_secs
                ));
            }
        };

        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        if !out.status.success() {
            return Ok(format!(
                "Error: codex CLI exited with {}.\nstdout:\n{}\nstderr:\n{}",
                out.status, stdout, stderr
            ));
        }
        Ok(stdout.trim().to_string())
    }
}

#[cfg(test)]
mod tests;
