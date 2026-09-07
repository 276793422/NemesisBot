//! MCP stdio transport.
//!
//! Implements the `Transport` trait for subprocess-based communication using
//! newline-delimited JSON-RPC over stdin/stdout, as defined by the MCP
//! specification.

use async_trait::async_trait;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::time::{Duration, timeout};

use crate::transport::{Transport, TransportError, TransportRequest, TransportResponse};

// ---------------------------------------------------------------------------
// StdioTransport
// ---------------------------------------------------------------------------

/// Stdio-based MCP transport.
///
/// Spawns a child process and communicates via its stdin/stdout using
/// newline-delimited JSON-RPC messages.
pub struct StdioTransport {
    /// Command to execute.
    command: String,
    /// Arguments to pass to the command.
    args: Vec<String>,
    /// Environment variables ("KEY=VALUE").
    env: Vec<String>,
    /// The spawned child process.
    child: Option<Child>,
    /// Write handle to child's stdin (protected by mutex for concurrent sends).
    stdin: Option<Mutex<tokio::process::ChildStdin>>,
    /// Read handle to child's stdout (protected by mutex for sequential reads).
    stdout: Option<Mutex<BufReader<tokio::process::ChildStdout>>>,
    /// Whether the transport is currently connected.
    connected: bool,
}

impl StdioTransport {
    /// Create a new stdio transport for the given command.
    ///
    /// The subprocess is not started until `connect()` is called.
    pub fn new(command: impl Into<String>, args: Vec<String>, env: Vec<String>) -> Self {
        Self {
            command: command.into(),
            args,
            env,
            child: None,
            stdin: None,
            stdout: None,
            connected: false,
        }
    }

    /// Create from a `ServerConfig`.
    pub fn from_config(config: &crate::types::ServerConfig) -> Self {
        Self::new(&config.command, config.args.clone(), config.env.clone())
    }
}

// ---------------------------------------------------------------------------
// J3: stdout 行分类（响应 / 通知 / 垃圾行）
// ---------------------------------------------------------------------------

/// `classify_line` 的分类结果。抽成纯函数便于单测——真实的 subprocess
/// 读取循环没法直接构造。
enum LineKind {
    /// 带 `id` 的 JSON-RPC 响应（含错误响应——错误响应也带 id）。
    Response,
    /// progress 通知（`method` 含 "progress"）。
    Progress(String),
    /// 其他通知（log / cancelled / initialized 等）。
    Notification(String),
    /// 非 JSON 行（服务器往 stdout 打的 banner / 日志）。
    Garbage,
}

/// 按行内容分类。判定规则：JSON 且带 `id` = 响应；JSON 无 `id` = 通知
/// （JSON-RPC 通知就是无 id 请求）；其余 = 垃圾行。
fn classify_line(line: &str) -> LineKind {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
        return LineKind::Garbage;
    };
    if v.get("id").is_some() {
        return LineKind::Response;
    }
    let method = v
        .get("method")
        .and_then(|m| m.as_str())
        .unwrap_or("unknown")
        .to_string();
    if method.contains("progress") {
        LineKind::Progress(method)
    } else {
        LineKind::Notification(method)
    }
}

#[async_trait]
impl Transport for StdioTransport {
    async fn connect(&mut self) -> Result<(), TransportError> {
        if self.connected {
            return Ok(());
        }

        let mut cmd = Command::new(&self.command);
        cmd.args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Inject environment variables.
        for pair in &self.env {
            if let Some((k, v)) = pair.split_once('=') {
                cmd.env(k, v);
            }
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| TransportError::send_failed(format!("failed to spawn MCP server: {e}")))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| TransportError::send_failed("failed to get stdin pipe"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| TransportError::send_failed("failed to get stdout pipe"))?;

        self.child = Some(child);
        self.stdin = Some(Mutex::new(stdin));
        self.stdout = Some(Mutex::new(BufReader::new(stdout)));
        self.connected = true;

        Ok(())
    }

    async fn close(&mut self) -> Result<(), TransportError> {
        if !self.connected {
            return Ok(());
        }
        self.connected = false;

        // Drop stdin to signal EOF.
        self.stdin = None;

        // Kill the child process.
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        self.child = None;
        self.stdout = None;

        Ok(())
    }

    async fn send(
        &mut self,
        request: &TransportRequest,
        timeout_ms: u64,
    ) -> Result<TransportResponse, TransportError> {
        if !self.connected {
            return Err(TransportError::not_connected());
        }

        // Serialize the request.
        let mut line = serde_json::to_string(request).map_err(|e| {
            TransportError::send_failed(format!("failed to serialize request: {e}"))
        })?;
        line.push('\n');

        // Write to stdin.
        let stdin = self.stdin.as_ref().ok_or(TransportError::not_connected())?;
        {
            let mut writer = stdin.lock().await;
            writer.write_all(line.as_bytes()).await.map_err(|e| {
                TransportError::send_failed(format!("failed to write to stdin: {e}"))
            })?;
            writer
                .flush()
                .await
                .map_err(|e| TransportError::send_failed(format!("failed to flush stdin: {e}")))?;
        }

        // Read response from stdout with timeout.
        let stdout = self
            .stdout
            .as_ref()
            .ok_or(TransportError::not_connected())?;
        let effective_timeout = if timeout_ms == 0 {
            Duration::from_secs(30)
        } else {
            Duration::from_millis(timeout_ms)
        };

        // J3：逐行读直到拿到带 id 的响应行（整体 deadline 不变）。无 id 的
        // JSON-RPC 通知（progress/log 是 MCP 标准行为）不是本次请求的响应，
        // 记 trace 后跳过；非 JSON 行（服务器 banner）同样跳过。旧实现读
        // 一行就当响应解析：服务器在响应前先发通知会让通知行反序列化失败
        // （TransportResponse 要求 id 字段）→ 整个请求报 send_failed。
        let deadline = tokio::time::Instant::now() + effective_timeout;
        let response_line = {
            let mut reader = stdout.lock().await;
            loop {
                let mut buf = String::new();
                let remaining = deadline
                    .checked_duration_since(tokio::time::Instant::now())
                    .unwrap_or_default();
                if remaining.is_zero() {
                    return Err(TransportError::timeout());
                }
                timeout(remaining, reader.read_line(&mut buf))
                    .await
                    .map_err(|_| TransportError::timeout())?
                    .map_err(|e| {
                        TransportError::send_failed(format!("failed to read from stdout: {e}"))
                    })?;

                if buf.is_empty() {
                    return Err(TransportError::send_failed(
                        "connection closed (EOF from MCP server)",
                    ));
                }

                match classify_line(&buf) {
                    LineKind::Response => break buf,
                    LineKind::Progress(method) => {
                        tracing::trace!(
                            method = %method,
                            "[StdioTransport] MCP progress notification: {}",
                            buf.trim()
                        );
                    }
                    LineKind::Notification(method) => {
                        tracing::trace!(
                            method = %method,
                            "[StdioTransport] MCP notification skipped"
                        );
                    }
                    LineKind::Garbage => {
                        tracing::trace!("[StdioTransport] MCP non-JSON stdout line skipped");
                    }
                }
            }
        };

        // Parse the response.
        let response: TransportResponse = serde_json::from_str(response_line.trim())
            .map_err(|e| TransportError::send_failed(format!("failed to parse response: {e}")))?;

        Ok(response)
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    fn name(&self) -> &str {
        "stdio"
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        // Best-effort kill the child process on drop.
        if let Some(child) = self.child.as_mut() {
            let _ = child.start_kill();
        }
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests;
