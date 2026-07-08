//! Remote executor: run execution-class tools in a separate child process.
//!
//! Gateway-side bridge. Each MOVE tool (exec/file/grep/git/...) is wrapped in a
//! [`RemoteExecutorTool`] that delegates metadata (`description` / `parameters`
//! / `preview`) to the local tool impl — so the LLM sees byte-identical schemas
//! and the checkpoint (edit safety net) still snapshots file writes — but routes
//! `execute()` over a per-call stdio IPC to a freshly-spawned `nemesisbot` child
//! running in executor role (`NEMESISBOT_ROLE=executor`).
//!
//! See `docs/PLAN/2026-07-08_executor-separation.md`.
//!
//! Layer 2 (sandbox) is a single spawn-command fork: when `sandbox` is set the
//! child is launched via `Start.exe /box:NemesisBox nemesisbot.exe` instead of
//! `nemesisbot.exe` directly. Sandboxie does not isolate by process name — box
//! membership is assigned at spawn time by the driver — so the same binary runs
//! unsandboxed (gateway) and sandboxed (executor child) simultaneously. The
//! `start_exe` path stays `None` until the Sandboxie integration phase; until
//! then `sandbox=true` returns a clear error from `build_command`.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tracing::debug;

use crate::context::RequestContext;
use crate::r#loop::{FileChange, Tool};

/// Tools that run in the executor child (the MOVE set): execution-class tools
/// with side effects that don't need gateway in-memory state — exactly the
/// LLM-driven dangerous-ops surface the sandbox (Layer 2) will wrap.
///
/// Kept local (STAY) because they need gateway objects (bus / vector store /
/// scheduler / engine / registries): `message`, `cluster_rpc`, `cron`,
/// `memory_*`, `find_skills` / `install_skill` / `skill_manage`, `workflow_run`,
/// `forge_bridge`, `mcp_*`.
///
/// Note: `exec_async` (background processes) is intentionally NOT here — a
/// per-call child exits after one tool call, so a background-process handle it
/// returned would be orphaned. It stays local until a long-lived executor model
/// is introduced. `sleep` likewise stays local (no isolation value).
pub const MOVE_TOOLS: &[&str] = &[
    "exec",
    "read_file",
    "write_file",
    "list_dir",
    "edit_file",
    "append_file",
    "delete_file",
    "create_dir",
    "delete_dir",
    "grep",
    "git",
];

// ---------------------------------------------------------------------------
// Wire protocol — one JSON line per direction (newline-delimited)
// ---------------------------------------------------------------------------

/// Gateway → executor request. Mirrored on the child side by `exec_worker`.
#[derive(Serialize)]
struct ExecutorRequest<'a> {
    tool: &'a str,
    /// Raw tool-args JSON string — exactly what the LLM produced and what the
    /// local `Tool::execute` expects as `args: &str`.
    args: &'a str,
    /// Serialized [`RequestContext`] (`async_callback` is dropped via
    /// `#[serde(skip)]` on the field).
    context: serde_json::Value,
}

/// Executor → gateway response.
#[derive(Deserialize)]
struct ExecutorResponse {
    ok: bool,
    #[serde(default)]
    result: String,
    #[serde(default)]
    error: String,
}

// ---------------------------------------------------------------------------
// ExecutorChannel — spawns a fresh child per call, one request → one response
// ---------------------------------------------------------------------------

/// Spawn configuration for executor children. Holds no mutable state, so a
/// single `Arc<ExecutorChannel>` is shared by every `RemoteExecutorTool`.
pub struct ExecutorChannel {
    /// Path to the nemesisbot executable (the gateway's own exe).
    pub exe_path: PathBuf,
    /// Resolved workspace path, passed to the child via env so it does not
    /// re-run path resolution (which depends on `--local` / NEMESISBOT_HOME).
    pub workspace: String,
    /// Layer 2 switch: wrap the spawn with `Start.exe /box:`.
    pub sandbox: bool,
    /// Path to Sandboxie `Start.exe`. `None` until the Sandboxie integration
    /// phase; if `sandbox` is set while this is `None`, `build_command` errors
    /// with a clear message.
    pub start_exe: Option<PathBuf>,
    /// Sandboxie box name.
    pub box_name: String,
    /// Per-call hard timeout (the child must respond within this).
    pub timeout: Duration,
}

impl ExecutorChannel {
    /// Construct a channel. `start_exe` defaults to `None` (Layer 1); the
    /// Sandboxie integration phase fills it (likely from config).
    pub fn new(exe_path: PathBuf, workspace: String, sandbox: bool) -> Self {
        Self {
            exe_path,
            workspace,
            sandbox,
            start_exe: None,
            box_name: "NemesisBox".to_string(),
            timeout: Duration::from_secs(24 * 3600),
        }
    }

    /// Build the spawn command, forking on the `sandbox` flag.
    ///
    /// - Layer 1 (`sandbox=false`): `nemesisbot.exe` directly — a normal process.
    /// - Layer 2 (`sandbox=true`):  `Start.exe /box:NemesisBox nemesisbot.exe` —
    ///   the driver marks the child as boxed at creation. Same binary, different
    ///   box membership, distinguished by launch command (NOT by exe name).
    fn build_command(&self) -> Result<Command, String> {
        let mut cmd = if self.sandbox {
            let start = self.start_exe.as_ref().ok_or_else(|| {
                "executor.sandbox=true but Sandboxie is not integrated yet \
                 (start_exe unset); set executor.sandbox=false or wait for the \
                 Sandboxie integration phase"
                    .to_string()
            })?;
            let mut c = Command::new(start);
            c.arg(format!("/box:{}", self.box_name));
            c.arg(&self.exe_path);
            c
        } else {
            Command::new(&self.exe_path)
        };
        cmd.env("NEMESISBOT_ROLE", "executor")
            .env("NEMESISBOT_EXECUTOR_WORKSPACE", &self.workspace);
        Ok(cmd)
    }

    /// Spawn a child, send one request line, read one response line, reap.
    pub async fn spawn_and_call(
        &self,
        tool: &str,
        args: &str,
        ctx: &RequestContext,
    ) -> Result<String, String> {
        let context_value =
            serde_json::to_value(ctx).map_err(|e| format!("serialize context: {e}"))?;
        let request = ExecutorRequest {
            tool,
            args,
            context: context_value,
        };
        let mut line = serde_json::to_string(&request)
            .map_err(|e| format!("serialize executor request: {e}"))?;
        line.push('\n');

        let mut cmd = self.build_command()?;
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("failed to spawn executor child: {e}"))?;

        // Drain stderr in the background so the child never blocks on a full
        // (~4KB) stderr pipe. Lines surface in gateway logs for diagnosis.
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    debug!("[executor stderr] {line}");
                }
            });
        }

        // Write the single request line, then drop stdin to signal EOF.
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(line.as_bytes()).await;
            let _ = stdin.flush().await;
            drop(stdin);
        }

        // Read the single response line (with timeout).
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "executor child has no stdout".to_string())?;
        let mut reader = BufReader::new(stdout).lines();
        let resp_line = match tokio::time::timeout(self.timeout, reader.next_line()).await {
            Err(_) => {
                let _ = child.start_kill();
                return Err(format!(
                    "executor timed out after {:?} (tool={tool})",
                    self.timeout
                ));
            }
            Ok(Err(e)) => {
                let _ = child.start_kill();
                return Err(format!("read executor response: {e}"));
            }
            Ok(Ok(None)) => {
                let _ = child.start_kill();
                return Err(format!(
                    "executor child exited without a response (tool={tool})"
                ));
            }
            Ok(Ok(Some(line))) => line,
        };

        // Reap the child (best-effort).
        let _ = child.wait().await;

        let resp: ExecutorResponse = serde_json::from_str(&resp_line)
            .map_err(|e| format!("parse executor response: {e}"))?;
        if resp.ok {
            Ok(resp.result)
        } else if resp.error.is_empty() {
            Err("executor returned an error".to_string())
        } else {
            Err(resp.error)
        }
    }
}

// ---------------------------------------------------------------------------
// RemoteExecutorTool — the Tool the agent loop sees
// ---------------------------------------------------------------------------

/// Gateway-side bridge: a normal `Tool` to the agent loop, but `execute()` is
/// proxied to an executor child. Metadata + `preview` delegate to the wrapped
/// local impl, so the LLM sees identical schemas and the checkpoint safety net
/// still snapshots file writes.
pub struct RemoteExecutorTool {
    name: String,
    local: Box<dyn Tool>,
    channel: std::sync::Arc<ExecutorChannel>,
}

impl RemoteExecutorTool {
    pub fn new(
        name: String,
        local: Box<dyn Tool>,
        channel: std::sync::Arc<ExecutorChannel>,
    ) -> Self {
        Self {
            name,
            local,
            channel,
        }
    }
}

#[async_trait]
impl Tool for RemoteExecutorTool {
    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String> {
        self.channel
            .spawn_and_call(&self.name, args, context)
            .await
            .map_err(|e| format!("executor unavailable: {e}"))
    }

    fn set_context(&self, channel: &str, chat_id: &str) {
        self.local.set_context(channel, chat_id);
    }

    fn description(&self) -> String {
        self.local.description()
    }

    fn parameters(&self) -> serde_json::Value {
        self.local.parameters()
    }

    fn preview(&self, args: &str) -> Option<FileChange> {
        self.local.preview(args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_command_real_env_succeeds() {
        let ch = ExecutorChannel::new(PathBuf::from("/x/nemesisbot.exe"), "/ws".into(), false);
        assert!(!ch.sandbox);
        assert!(ch.build_command().is_ok());
    }

    #[test]
    fn build_command_sandbox_without_start_exe_errors_clearly() {
        let ch = ExecutorChannel::new(PathBuf::from("/x/nemesisbot.exe"), "/ws".into(), true);
        let err = ch.build_command().unwrap_err();
        assert!(
            err.contains("Sandboxie"),
            "expected a Sandboxie-not-integrated error, got: {err}"
        );
    }

    #[test]
    fn move_tools_is_the_expected_set() {
        assert_eq!(
            MOVE_TOOLS,
            &[
                "exec",
                "read_file",
                "write_file",
                "list_dir",
                "edit_file",
                "append_file",
                "delete_file",
                "create_dir",
                "delete_dir",
                "grep",
                "git",
            ]
        );
    }
}
