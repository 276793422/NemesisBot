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
mod tests {
    use super::*;
    use crate::types::JSONRPC_VERSION;

    #[test]
    fn create_transport() {
        let t = StdioTransport::new("echo", vec![], vec![]);
        assert_eq!(t.name(), "stdio");
        assert!(!t.is_connected());
    }

    #[test]
    fn create_from_config() {
        let config = crate::types::ServerConfig::new("test", "node")
            .arg("server.js")
            .env("FOO=bar")
            .timeout(10);

        let t = StdioTransport::from_config(&config);
        assert_eq!(t.command, "node");
        assert_eq!(t.args, vec!["server.js"]);
        assert_eq!(t.env, vec!["FOO=bar"]);
        assert!(!t.is_connected());
    }

    /// Test connect/close lifecycle with a simple echo-like program.
    /// On Windows, `cmd /C echo` exits immediately, so we just test that
    /// connect succeeds and close cleans up.
    #[tokio::test]
    async fn connect_and_close_lifecycle() {
        // Use a long-running command so the process stays alive during the test.
        // `ping -t localhost` on Windows runs indefinitely.
        #[cfg(target_os = "windows")]
        let mut t = StdioTransport::new("ping", vec!["-t".to_string(), "localhost".to_string()], vec![]);
        #[cfg(not(target_os = "windows"))]
        let mut t = StdioTransport::new("sleep", vec!["60".to_string()], vec![]);

        assert!(!t.is_connected());

        // Connect should succeed.
        t.connect().await.unwrap();
        assert!(t.is_connected());

        // Close should succeed.
        t.close().await.unwrap();
        assert!(!t.is_connected());

        // Double close should be fine.
        t.close().await.unwrap();
        assert!(!t.is_connected());
    }

    #[tokio::test]
    async fn send_when_not_connected_fails() {
        let mut t = StdioTransport::new("nonexistent", vec![], vec![]);
        let req = TransportRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(serde_json::Value::Number(1.into())),
            method: "ping".to_string(),
            params: None,
        };
        let result = t.send(&req, 1000).await;
        assert!(result.is_err());
    }

    /// End-to-end test: spawn a simple JSON-RPC echo server using Python,
    /// send a request, and verify the response. Skips if Python is unavailable.
    #[tokio::test]
    async fn e2e_jsonrpc_echo() {
        // Simple Python script that reads a JSON-RPC request from stdin and
        // echoes back a response with the same id.
        let python_script = r#"
import sys, json
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    try:
        req = json.loads(line)
        resp = {"jsonrpc": "2.0", "id": req.get("id"), "result": {"echo": req.get("method")}}
        sys.stdout.write(json.dumps(resp) + "\n")
        sys.stdout.flush()
    except Exception:
        break
"#;

        let mut t = StdioTransport::new(
            "python",
            vec!["-c".to_string(), python_script.to_string()],
            vec![],
        );

        // Skip if python is not available.
        if t.connect().await.is_err() {
            eprintln!("Skipping e2e test: python not available");
            return;
        }

        let req = TransportRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(serde_json::Value::Number(42.into())),
            method: "test/method".to_string(),
            params: None,
        };

        let resp = t.send(&req, 5000).await.unwrap();
        assert_eq!(resp.id, serde_json::Value::Number(42.into()));
        assert!(resp.result.is_some());
        assert_eq!(resp.result.unwrap()["echo"], "test/method");

        t.close().await.unwrap();
    }

    // ---- New tests ----

    #[test]
    fn transport_name_is_stdio() {
        let t = StdioTransport::new("test", vec![], vec![]);
        assert_eq!(t.name(), "stdio");
    }

    #[test]
    fn new_transport_not_connected() {
        let t = StdioTransport::new("test", vec!["arg1".to_string()], vec!["KEY=VAL".to_string()]);
        assert!(!t.is_connected());
        assert_eq!(t.command, "test");
        assert_eq!(t.args, vec!["arg1"]);
        assert_eq!(t.env, vec!["KEY=VAL"]);
    }

    #[tokio::test]
    async fn close_without_connect_is_ok() {
        let mut t = StdioTransport::new("test", vec![], vec![]);
        t.close().await.unwrap();
        assert!(!t.is_connected());
    }

    #[tokio::test]
    async fn connect_nonexistent_command_fails() {
        let mut t = StdioTransport::new(
            "/absolutely/nonexistent/command/that/does/not/exist",
            vec![],
            vec![],
        );
        let result = t.connect().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn double_connect_is_ok() {
        #[cfg(target_os = "windows")]
        let mut t = StdioTransport::new("ping", vec!["-t".to_string(), "localhost".to_string()], vec![]);
        #[cfg(not(target_os = "windows"))]
        let mut t = StdioTransport::new("sleep", vec!["60".to_string()], vec![]);

        t.connect().await.unwrap();
        assert!(t.is_connected());
        t.connect().await.unwrap(); // Second connect is a no-op
        assert!(t.is_connected());
        t.close().await.unwrap();
    }

    #[test]
    fn from_config_preserves_fields() {
        let config = crate::types::ServerConfig::new("my-server", "/usr/bin/node")
            .arg("index.js")
            .arg("--verbose")
            .env("NODE_ENV=production")
            .env("PORT=3000")
            .timeout(60);

        let t = StdioTransport::from_config(&config);
        assert_eq!(t.command, "/usr/bin/node");
        assert_eq!(t.args, vec!["index.js", "--verbose"]);
        assert_eq!(t.env.len(), 2);
    }

    #[test]
    fn from_config_no_env() {
        let config = crate::types::ServerConfig::new("srv", "cmd");
        let t = StdioTransport::from_config(&config);
        assert!(t.env.is_empty());
    }

    #[tokio::test]
    async fn send_after_close_fails() {
        #[cfg(target_os = "windows")]
        let mut t = StdioTransport::new("ping", vec!["-t".to_string(), "localhost".to_string()], vec![]);
        #[cfg(not(target_os = "windows"))]
        let mut t = StdioTransport::new("sleep", vec!["60".to_string()], vec![]);

        t.connect().await.unwrap();
        t.close().await.unwrap();

        let req = TransportRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(serde_json::Value::Number(1.into())),
            method: "test".to_string(),
            params: None,
        };
        let result = t.send(&req, 1000).await;
        assert!(result.is_err());
    }
}
