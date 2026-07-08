//! Remote executor: run execution-class tools in a separate child process.
//!
//! Gateway-side bridge. Each MOVE tool (exec/file/grep/git/...) is wrapped in a
//! [`RemoteExecutorTool`] that delegates metadata (`description` / `parameters`
//! / `preview`) to the local tool impl — so the LLM sees byte-identical schemas
//! and the checkpoint (edit safety net) still snapshots file writes — but routes
//! `execute()` to a freshly-spawned `nemesisbot` child running in executor role.
//!
//! Two transports, picked by `sandbox`:
//! - **stdio** (sandbox=false, Layer 1): spawn child, exchange JSON over its
//!   stdin/stdout.
//! - **named pipe** (sandbox=true, Layer 2): `Start.exe` does not forward stdio
//!   across the box boundary, so the sandboxed path uses a Windows named pipe
//!   `\\.\pipe\NemesisBox_<id>` instead.
//!
//! The spawn command is controlled INDEPENDENTLY by `start_exe`:
//! - `None` → spawn the executor directly (Layer 1, or L2.1 transport testing).
//! - `Some` → wrap with `Start.exe /box:<box>` (L2.2 real Sandboxie containment).
//!
//! See `docs/PLAN/2026-07-08_executor-separation.md` (Layer 1) and
//! `docs/PLAN/2026-07-09_sandboxie-integration.md` (Layer 2).

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
    /// Transport switch: true → named-pipe transport (sandbox mode); false →
    /// stdio transport (Layer 1). NOTE: this is the TRANSPORT choice, not the
    /// spawn-wrap choice — see `start_exe`.
    pub sandbox: bool,
    /// Sandboxie `Start.exe` path. `Some` → spawn via `Start.exe /box:<box>`
    /// (real containment, L2.2). `None` → spawn the executor directly (Layer 1,
    /// or L2.1 transport testing without the box).
    pub start_exe: Option<PathBuf>,
    /// Sandboxie box name.
    pub box_name: String,
    /// Per-call hard timeout (the child must respond within this).
    pub timeout: Duration,
}

impl ExecutorChannel {
    /// Construct a channel in Layer-1 / L2.1 mode (direct spawn, no Start.exe
    /// wrap). L2.2 sets `start_exe` via the `with_start_exe` builder.
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

    /// Set the Sandboxie `Start.exe` path (L2.2: wraps the spawn for real box).
    #[allow(dead_code)]
    pub fn with_start_exe(mut self, start_exe: PathBuf) -> Self {
        self.start_exe = Some(start_exe);
        self
    }

    /// Set the per-call timeout (use a short one in tests).
    #[allow(dead_code)]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Build the spawn command. The wrap is controlled by `start_exe`:
    /// - `Some` → `Start.exe /box:<box> nemesisbot.exe` (L2.2 real box).
    /// - `None` → `nemesisbot.exe` directly (Layer 1 / L2.1 transport-only).
    fn build_command(&self) -> Command {
        let mut cmd = if let Some(start) = &self.start_exe {
            let mut c = Command::new(start);
            c.arg(format!("/box:{}", self.box_name));
            c.arg(&self.exe_path);
            c
        } else {
            Command::new(&self.exe_path)
        };
        cmd.env("NEMESISBOT_ROLE", "executor")
            .env("NEMESISBOT_EXECUTOR_WORKSPACE", &self.workspace);
        cmd
    }

    /// Spawn a child, send one request, read one response, reap.
    pub async fn spawn_and_call(
        &self,
        tool: &str,
        args: &str,
        ctx: &RequestContext,
    ) -> Result<String, String> {
        let request_line = self.build_request_line(tool, args, ctx)?;
        if self.sandbox {
            #[cfg(windows)]
            {
                return self.spawn_and_call_pipe(tool, &request_line).await;
            }
            #[cfg(not(windows))]
            {
                let _ = request_line;
                return Err(
                    "sandbox (named-pipe) transport is only supported on Windows".to_string(),
                );
            }
        }
        self.spawn_and_call_stdio(tool, &request_line).await
    }

    fn build_request_line(
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
        Ok(line)
    }

    fn parse_response(resp_line: &str) -> Result<String, String> {
        let resp: ExecutorResponse = serde_json::from_str(resp_line)
            .map_err(|e| format!("parse executor response: {e}"))?;
        if resp.ok {
            Ok(resp.result)
        } else if resp.error.is_empty() {
            Err("executor returned an error".to_string())
        } else {
            Err(resp.error)
        }
    }

    /// Drain child stderr in the background (prevents a ~4KB pipe block).
    fn drain_stderr(child: &mut tokio::process::Child) {
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    debug!("[executor stderr] {line}");
                }
            });
        }
    }

    /// stdio transport (sandbox=false): write stdin, read stdout.
    async fn spawn_and_call_stdio(
        &self,
        tool: &str,
        request_line: &str,
    ) -> Result<String, String> {
        let mut cmd = self.build_command();
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("failed to spawn executor child: {e}"))?;
        Self::drain_stderr(&mut child);

        // Write the single request line, then drop stdin to signal EOF.
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(request_line.as_bytes()).await;
            let _ = stdin.flush().await;
            drop(stdin);
        }

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

        let _ = child.wait().await;
        Self::parse_response(&resp_line)
    }

    /// Named-pipe transport (sandbox=true). L2.1: works with or without the box
    /// — `start_exe=None` spawns directly (transport test); `start_exe=Some`
    /// wraps with Start.exe (real box, L2.2).
    #[cfg(windows)]
    async fn spawn_and_call_pipe(
        &self,
        tool: &str,
        request_line: &str,
    ) -> Result<String, String> {
        use crate::executor_pipe;

        let id = executor_pipe::unique_pipe_id();
        let pipe = executor_pipe::pipe_name(&id);

        // 1. Create the named pipe BEFORE spawn so the child can connect to it.
        let mut server = executor_pipe::create_server(&pipe)
            .map_err(|e| format!("create executor pipe: {e}"))?;

        // 2. Spawn the child with the pipe env. stdio is unused for transport
        //    (the pipe carries the JSON); null stdin/stdout, piped stderr for
        //    diagnosis.
        let mut cmd = self.build_command();
        cmd.env("NEMESISBOT_EXECUTOR_PIPE", &pipe)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("failed to spawn executor child: {e}"))?;
        Self::drain_stderr(&mut child);

        // 3. Wait for the child to connect (timeout — child must start + connect).
        match tokio::time::timeout(Duration::from_secs(30), server.connect()).await {
            Err(_) => {
                let _ = child.start_kill();
                return Err(format!(
                    "executor child did not connect to pipe within 30s (tool={tool})"
                ));
            }
            Ok(Err(e)) => {
                let _ = child.start_kill();
                return Err(format!("executor pipe connect failed: {e}"));
            }
            Ok(Ok(())) => {}
        }

        // 4. Write request line.
        server
            .write_all(request_line.as_bytes())
            .await
            .map_err(|e| format!("write executor pipe: {e}"))?;
        server
            .flush()
            .await
            .map_err(|e| format!("flush executor pipe: {e}"))?;

        // 5. Read response line (timeout).
        let resp_line = {
            let mut reader = BufReader::new(&mut server).lines();
            match tokio::time::timeout(self.timeout, reader.next_line()).await {
                Err(_) => {
                    let _ = child.start_kill();
                    return Err(format!(
                        "executor timed out after {:?} (tool={tool})",
                        self.timeout
                    ));
                }
                Ok(Err(e)) => {
                    let _ = child.start_kill();
                    return Err(format!("read executor pipe: {e}"));
                }
                Ok(Ok(None)) => {
                    let _ = child.start_kill();
                    return Err(format!(
                        "executor child closed pipe without a response (tool={tool})"
                    ));
                }
                Ok(Ok(Some(line))) => line,
            }
        };

        // 6. Close our end so the child's loop sees EOF and exits; then reap.
        //    (The per-call child loops waiting for the next request; without
        //    closing the pipe it would block forever and child.wait() hangs.)
        drop(server);
        let _ = child.wait().await;
        Self::parse_response(&resp_line)
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
    fn build_command_no_start_exe_is_direct_spawn() {
        let ch = ExecutorChannel::new(PathBuf::from("/x/nemesisbot.exe"), "/ws".into(), false);
        assert!(ch.start_exe.is_none());
        // No error — direct spawn command is built.
        let _ = ch.build_command();
    }

    #[test]
    fn build_command_with_start_exe_wraps() {
        let ch = ExecutorChannel::new(PathBuf::from("/x/nemesisbot.exe"), "/ws".into(), true)
            .with_start_exe(PathBuf::from("/x/Start.exe"));
        assert!(ch.start_exe.is_some());
        // No error — Start.exe wrap command is built (L2.2 form).
        let _ = ch.build_command();
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
