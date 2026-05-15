//! Shared test harness for NemesisBot integration tests.
//!
//! Provides utilities for:
//! - Isolated temporary workspace management
//! - AI Server and Gateway process lifecycle
//! - WebSocket client with message protocol support
//! - CLI command execution with output capture
//! - HTTP health check polling
//! - Assertion helpers

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const AI_SERVER_PORT: u16 = 8080;
pub const WEB_PORT: u16 = 49000;
pub const WS_PORT: u16 = 49000;
pub const HEALTH_PORT: u16 = 18790;
pub const AUTH_TOKEN: &str = "276793422";

// ---------------------------------------------------------------------------
// Process management
// ---------------------------------------------------------------------------

/// A managed child process that is killed on drop.
pub struct ManagedProcess {
    child: Option<tokio::process::Child>,
    name: &'static str,
}

impl ManagedProcess {
    /// Spawn a new managed process. stderr is inherited so error messages are visible.
    pub fn spawn(
        name: &'static str,
        program: &Path,
        args: &[&str],
        cwd: &Path,
    ) -> Result<Self> {
        println!("  Starting {}...", name);
        let child = tokio::process::Command::new(program)
            .args(args)
            .current_dir(cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("Failed to spawn {}: {}", name, program.display()))?;
        println!("  {} started (PID: {:?})", name, child.id());
        Ok(Self {
            child: Some(child),
            name,
        })
    }

    /// Check if the process is still running.
    pub async fn is_running(&mut self) -> bool {
        if let Some(child) = self.child.as_mut() {
            match child.try_wait() {
                Ok(Some(status)) => {
                    println!("  {} exited with: {}", self.name, status);
                    false
                }
                Ok(None) => true,
                Err(_) => false,
            }
        } else {
            false
        }
    }

    /// Kill the managed process.
    pub async fn kill(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill().await;
            println!("  {} stopped", self.name);
        }
    }
}

impl Drop for ManagedProcess {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.start_kill();
        }
    }
}

// ---------------------------------------------------------------------------
// Test workspace
// ---------------------------------------------------------------------------

/// An isolated test workspace with a .nemesisbot directory.
pub struct TestWorkspace {
    temp_dir: tempfile::TempDir,
}

impl TestWorkspace {
    /// Create a new isolated test workspace.
    pub fn new() -> Result<Self> {
        let temp_dir = tempfile::TempDir::new()?;
        Ok(Self { temp_dir })
    }

    /// Path to the workspace root (where nemesisbot commands run).
    pub fn path(&self) -> &Path {
        self.temp_dir.path()
    }

    /// Path to the .nemesisbot home directory.
    pub fn home(&self) -> PathBuf {
        self.temp_dir.path().join(".nemesisbot")
    }

    /// Path to config.json.
    pub fn config_path(&self) -> PathBuf {
        self.home().join("config.json")
    }

    /// Path to workspace directory.
    pub fn workspace(&self) -> PathBuf {
        self.home().join("workspace")
    }

    /// Path to forge directory.
    pub fn forge_dir(&self) -> PathBuf {
        self.workspace().join("forge")
    }

    /// Path to security config.
    pub fn security_config_path(&self) -> PathBuf {
        self.home()
            .join("workspace")
            .join("config")
            .join("config.security.json")
    }

    /// Run a nemesisbot CLI command in this workspace (--local mode).
    /// Returns CliOutput with exit_code=-1 if the process fails to start.
    /// Includes a 15-second timeout to prevent hanging on interactive commands.
    pub async fn run_cli(
        &self,
        nemesisbot_bin: &Path,
        args: &[&str],
    ) -> CliOutput {
        let mut full_args = vec!["--local"];
        full_args.extend(args);

        let result = tokio::time::timeout(
            Duration::from_secs(15),
            tokio::process::Command::new(nemesisbot_bin)
                .args(&full_args)
                .current_dir(self.path())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output(),
        ).await;

        match result {
            Ok(Ok(output)) => CliOutput {
                exit_code: output.status.code().unwrap_or(-1),
                stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            },
            Ok(Err(e)) => CliOutput {
                exit_code: -1,
                stdout: String::new(),
                stderr: format!("Failed to execute: {}", e),
            },
            Err(_) => CliOutput {
                exit_code: -2,
                stdout: String::new(),
                stderr: "Command timed out (15s)".to_string(),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// CLI output
// ---------------------------------------------------------------------------

/// Result of a CLI command execution.
#[derive(Debug, Clone)]
pub struct CliOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CliOutput {
    /// Check if the command succeeded (exit code 0).
    pub fn success(&self) -> bool {
        self.exit_code == 0
    }

    /// Check if stdout contains the given text.
    pub fn stdout_contains(&self, text: &str) -> bool {
        self.stdout.contains(text)
    }

    /// Check if stderr contains the given text.
    pub fn stderr_contains(&self, text: &str) -> bool {
        self.stderr.contains(text)
    }

    /// Get the first line of stdout (trimmed), truncated to max_len chars.
    pub fn stdout_first_line(&self) -> String {
        self.stdout
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .chars()
            .take(120)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// HTTP helpers
// ---------------------------------------------------------------------------

/// Create an HTTP client with reasonable timeouts.
pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap()
}

/// Poll an HTTP endpoint until it returns 200 or timeout.
pub async fn wait_for_http(url: &str, timeout: Duration) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()?;
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Ok(resp) = client.get(url).send().await {
            if resp.status().is_success() {
                return Ok(());
            }
        }
        if tokio::time::Instant::now() > deadline {
            bail!("Timeout waiting for {}", url);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

// ---------------------------------------------------------------------------
// WebSocket helpers
// ---------------------------------------------------------------------------

/// Connect to WebSocket with auth token.
pub async fn ws_connect(
    port: u16,
    token: &str,
) -> Result<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>>
{
    let url = format!("ws://127.0.0.1:{}/ws?token={}", port, token);
    let (stream, _) = tokio_tungstenite::connect_async(&url)
        .await
        .with_context(|| format!("WebSocket connect failed: {}", url))?;
    Ok(stream)
}

/// Send a chat message via WebSocket and wait for a response.
pub async fn ws_send_and_recv(
    stream: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    content: &str,
    timeout_secs: u64,
) -> Result<String> {
    let msg = json!({
        "type": "message",
        "module": "chat",
        "cmd": "send",
        "data": {
            "content": content
        },
        "timestamp": chrono::Utc::now().to_rfc3339()
    });
    stream.send(Message::Text(msg.to_string().into())).await?;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        let resp = tokio::time::timeout_at(deadline, stream.next()).await;
        match resp {
            Ok(Some(Ok(Message::Text(text)))) => {
                let text = text.to_string();
                if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    let msg_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    let module = v.get("module").and_then(|m| m.as_str()).unwrap_or("");
                    let cmd = v.get("cmd").and_then(|c| c.as_str()).unwrap_or("");

                    if msg_type == "message" && module == "chat" && cmd == "receive" {
                        let content = v["data"]["content"]
                            .as_str()
                            .unwrap_or("")
                            .to_string();
                        return Ok(content);
                    }
                    if msg_type == "system" && module == "error" {
                        let err = v["data"]["content"]
                            .as_str()
                            .unwrap_or("unknown error")
                            .to_string();
                        bail!("Error response: {}", err);
                    }
                    continue;
                }
                return Ok(text);
            }
            Ok(Some(Ok(Message::Ping(_)))) => continue,
            Ok(Some(Ok(Message::Pong(_)))) => continue,
            Ok(Some(Ok(other))) => return Ok(other.to_string()),
            Ok(Some(Err(e))) => bail!("WebSocket error: {}", e),
            Ok(None) => bail!("WebSocket closed"),
            Err(_) => bail!(
                "Timeout waiting for response ({}s)",
                timeout_secs
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Test result tracking
// ---------------------------------------------------------------------------

use std::sync::atomic::{AtomicUsize, Ordering};

static PASSED: AtomicUsize = AtomicUsize::new(0);
static FAILED: AtomicUsize = AtomicUsize::new(0);
static SKIPPED: AtomicUsize = AtomicUsize::new(0);

/// Reset the global test counters.
pub fn reset_counters() {
    PASSED.store(0, Ordering::SeqCst);
    FAILED.store(0, Ordering::SeqCst);
    SKIPPED.store(0, Ordering::SeqCst);
}

/// Get the current test counters.
pub fn get_counters() -> (usize, usize, usize) {
    (
        PASSED.load(Ordering::SeqCst),
        FAILED.load(Ordering::SeqCst),
        SKIPPED.load(Ordering::SeqCst),
    )
}

/// A single test result.
#[derive(Debug, Clone)]
pub struct TestResult {
    pub name: String,
    pub passed: bool,
    pub message: String,
}

/// Record a passing test.
pub fn pass(name: &str, msg: impl Into<String>) -> TestResult {
    PASSED.fetch_add(1, Ordering::SeqCst);
    TestResult {
        name: name.to_string(),
        passed: true,
        message: msg.into(),
    }
}

/// Record a failing test.
pub fn fail(name: &str, msg: impl Into<String>) -> TestResult {
    FAILED.fetch_add(1, Ordering::SeqCst);
    TestResult {
        name: name.to_string(),
        passed: false,
        message: msg.into(),
    }
}

/// Record a skipped test.
pub fn skip(name: &str, msg: impl Into<String>) -> TestResult {
    SKIPPED.fetch_add(1, Ordering::SeqCst);
    TestResult {
        name: name.to_string(),
        passed: true,
        message: format!("SKIP: {}", msg.into()),
    }
}

// ---------------------------------------------------------------------------
// Binary resolution
// ---------------------------------------------------------------------------

/// Resolve the project root directory from the current executable location.
pub fn resolve_project_root() -> Result<PathBuf> {
    let exe_dir = std::env::current_exe()?
        .parent()
        .unwrap()
        .to_path_buf();

    // Try going up from test-tools/integration-test/target/release/
    let mut dir = exe_dir.clone();
    for _ in 0..5 {
        if dir.join("Cargo.toml").exists()
            && std::fs::read_to_string(dir.join("Cargo.toml"))?
                .contains("[workspace]")
        {
            return Ok(dir);
        }
        if !dir.pop() {
            break;
        }
    }
    bail!("Could not find workspace root from {}", exe_dir.display());
}

/// Resolve the nemesisbot binary path.
pub fn resolve_nemesisbot_bin() -> Result<PathBuf> {
    let root = resolve_project_root()?;
    let bin = root.join("target/release/nemesisbot.exe");
    if bin.exists() {
        return Ok(bin);
    }
    let bin = root.join("target/debug/nemesisbot.exe");
    if bin.exists() {
        return Ok(bin);
    }
    bail!("nemesisbot binary not found in target/release or target/debug");
}

/// Resolve the AI server binary path (Go TestAIServer).
pub fn resolve_ai_server_bin() -> Result<PathBuf> {
    let root = resolve_project_root()?;
    // Go TestAIServer in test-tools/
    let bin = root.join("test-tools/TestAIServer/testaiserver.exe");
    if bin.exists() {
        return Ok(bin);
    }
    // Fallback: check target/ for any legacy builds
    let bin = root.join("target/release/ai-server.exe");
    if bin.exists() {
        return Ok(bin);
    }
    let bin = root.join("target/debug/ai-server.exe");
    if bin.exists() {
        return Ok(bin);
    }
    bail!("AI server binary not found (checked test-tools/TestAIServer/testaiserver.exe and target/)");
}

// ---------------------------------------------------------------------------
// Port cleanup (Windows)
// ---------------------------------------------------------------------------

/// Kill processes listening on the specified ports.
pub fn cleanup_ports(ports: &[u16]) {
    for port in ports {
        // Use netstat to find PIDs, then taskkill
        let output = std::process::Command::new("cmd")
            .args(&["/c", &format!(
                "netstat -ano | findstr :{} | findstr LISTENING",
                port
            )])
            .output();
        if let Ok(out) = output {
            let stdout = String::from_utf8_lossy(&out.stdout);
            for line in stdout.lines() {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if let Some(pid) = parts.last() {
                    if let Ok(pid_num) = pid.parse::<u32>() {
                        let _ = std::process::Command::new("taskkill")
                            .args(&["/F", "/PID", &pid_num.to_string()])
                            .output();
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Print helpers
// ---------------------------------------------------------------------------

/// Print a test suite header.
pub fn print_suite_header(name: &str) {
    println!("\n--- {} ---", name);
}

/// Print test results summary and return whether all passed.
pub fn print_results(results: &[TestResult]) -> bool {
    let mut pass_count = 0;
    let mut fail_count = 0;
    let mut skip_count = 0;

    for result in results {
        let status = if result.message.starts_with("SKIP:") {
            skip_count += 1;
            "SKIP"
        } else if result.passed {
            pass_count += 1;
            "PASS"
        } else {
            fail_count += 1;
            "FAIL"
        };
        println!("  [{:<4}] {} - {}", status, result.name, result.message);
    }

    println!("{}", "-".repeat(60));
    println!(
        "  Total: {} | Passed: {} | Failed: {} | Skipped: {}",
        pass_count + fail_count + skip_count,
        pass_count,
        fail_count,
        skip_count
    );

    fail_count == 0
}
