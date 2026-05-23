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
        Self::new(&config.command, config.args.clone(), config.env.clone().unwrap_or_default())
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

        let mut child = cmd.spawn().map_err(|e| {
            TransportError::send_failed(format!("failed to spawn MCP server: {e}"))
        })?;

        let stdin = child.stdin.take().ok_or_else(|| {
            TransportError::send_failed("failed to get stdin pipe")
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            TransportError::send_failed("failed to get stdout pipe")
        })?;

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
            writer.flush().await.map_err(|e| {
                TransportError::send_failed(format!("failed to flush stdin: {e}"))
            })?;
        }

        // Read response from stdout with timeout.
        let stdout = self.stdout.as_ref().ok_or(TransportError::not_connected())?;
        let effective_timeout = if timeout_ms == 0 {
            Duration::from_secs(30)
        } else {
            Duration::from_millis(timeout_ms)
        };

        let response_line = {
            let mut reader = stdout.lock().await;
            let mut buf = String::new();
            let read_future = reader.read_line(&mut buf);

            timeout(effective_timeout, read_future)
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

            buf
        };

        // Parse the response.
        let response: TransportResponse = serde_json::from_str(response_line.trim()).map_err(
            |e| TransportError::send_failed(format!("failed to parse response: {e}")),
        )?;

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
