//! Shared test harness for NemesisBot integration tests.
//!
//! Provides utilities for:
//! - Isolated temporary workspace management
//! - AI Server and Gateway process lifecycle
//! - WebSocket client with message protocol support
//! - CLI command execution with output capture
//! - HTTP health check polling
//! - Assertion helpers
//!
//! R9 additions: stdin-fed CLI runs ([`TestWorkspace::run_cli_with_stdin`])
//! and the scripted OpenAI-compatible responder ([`mock_ai::MockAiServer`]).

pub mod mock_ai;

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
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

// ---------------------------------------------------------------------------
// Coverage plumbing (R2 measurement reform)
// ---------------------------------------------------------------------------

/// Per-process LLVM profile path injected into spawned nemesisbot processes.
///
/// Set `NEMESISBOT_COVERAGE_DIR` in the outer `cargo test` environment to
/// make every [`ManagedProcess::spawn`] gateway write its counters to
/// `<dir>/<slug>-%p-%m.profraw`. The `%p` placeholder keeps concurrent
/// gateways from clobbering each other; a clean exit (graceful shutdown, not
/// TerminateProcess) is what actually flushes them — see
/// [`ManagedProcess::wait_for_exit`] and [`graceful_shutdown_gateway`].
///
/// Gateway long-run processes cover the bulk of nemesisbot's runtime surface
/// (web handlers / channels / agent loop / security pipeline all live in the
/// gateway process), which the plain-L2 runs previously measured as zero.
///
/// CLI child processes (via [`TestWorkspace::run_cli`]) additionally require
/// `NEMESISBOT_COVERAGE_CLI=1`: every instrumented process writes a full
/// (~55MB) counter image regardless of how little it executed, so collecting
/// the dozens of short-lived CLI invocations costs gigabytes of %TEMP%.
pub fn coverage_profile_file(slug: &str) -> Option<String> {
    let dir = std::env::var("NEMESISBOT_COVERAGE_DIR").ok()?;
    let safe: String = slug
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
        .collect();
    // A bare `%p-%m` pattern is NOT unique across a long measurement run:
    // Windows recycles PIDs aggressively, so among the hundreds of short-lived
    // CLI subprocesses spawned by one full bin-suite pass, later processes can
    // be handed the pid of an earlier one and silently overwrite that earlier
    // process's profraw (last-writer-wins). The R9 B1 leg lost most of its CLI
    // coverage this way (eval happened to survive; skills/cluster/scanner/
    // channel/dashboard/exec_worker all came back near-zero). Stamp every spawn
    // with a nanosecond tick + pid so filenames never collide.
    //
    // R10（2026-08-27 覆盖率终测）：模板里再去掉 `%m`（merge-pool 模式）。
    // `%m` 让同一条 LLVM_PROFILE_FILE 的多个写入方走 LLVM 内置池合并——但本
    // 工具链的父子继承场景会互相吃掉计数器（实证：dashboard r9_spawn_fail 里
    // CLI 子进程 start_and_wait 再 spawn 孙辈 gateway，两代继承同一文件名，
    // 终态文件里只剩孙辈 gateway.rs 的运行时计数，CLI 子进程自己的
    // dashboard.rs 计数全部丢失——llvm-profdata show 该函数零条目）。
    // 我们的协议本来就是离线 `llvm-profdata merge` 求和，每个进程写一份
    // 独立完整镜像即可；时间戳纳秒 + %p 已保证唯一，不需要池模式。
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    Some(format!(
        "{}\\{}-{}-%p.profraw",
        dir.trim_end_matches(['\\', '/']),
        safe,
        stamp
    ))
}

/// CLI-gated profile environment: empty unless BOTH `NEMESISBOT_COVERAGE_DIR`
/// and `NEMESISBOT_COVERAGE_CLI=1` are set (see [`coverage_profile_file`] for
/// why CLI collection is opt-in — every short-lived instrumented invocation
/// still writes a full counter image, so this is disk-budgeted per run).
pub fn coverage_cli_env() -> Vec<(String, String)> {
    if std::env::var("NEMESISBOT_COVERAGE_CLI").as_deref() != Ok("1") {
        return Vec::new();
    }
    match coverage_profile_file("cli") {
        Some(profile) => vec![("LLVM_PROFILE_FILE".to_string(), profile)],
        None => Vec::new(),
    }
}

/// A managed child process that is killed on drop.
pub struct ManagedProcess {
    child: Option<tokio::process::Child>,
    name: &'static str,
}

impl ManagedProcess {
    /// Spawn a new managed process. stderr is inherited so error messages are visible.
    pub fn spawn(name: &'static str, program: &Path, args: &[&str], cwd: &Path) -> Result<Self> {
        println!("  Starting {}...", name);
        let mut cmd = tokio::process::Command::new(program);
        cmd.args(args)
            .current_dir(cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        if let Some(profile) = coverage_profile_file(name) {
            cmd.env("LLVM_PROFILE_FILE", &profile);
        }
        let child = cmd
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

    /// Wait for the managed process to exit on its own (post graceful shutdown).
    ///
    /// Unlike [`Self::kill`] / the Drop terminator — both of which TerminateProcess
    /// the child and therefore skip its atexit handlers — this lets an LLVM
    /// coverage-instrumented binary flush its `.profraw` counters to disk.
    pub async fn wait_for_exit(&mut self, timeout: std::time::Duration) -> Result<()> {
        let Some(child) = self.child.as_mut() else {
            bail!("{} already stopped", self.name);
        };
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            match child.try_wait().context("try_wait failed")? {
                Some(status) => {
                    println!("  {} exited with: {}", self.name, status);
                    self.child = None;
                    return Ok(());
                }
                None => {
                    if tokio::time::Instant::now() >= deadline {
                        bail!(
                            "{} did not exit within {:?} after graceful shutdown",
                            self.name,
                            timeout
                        );
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }
            }
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
// Graceful gateway shutdown (coverage-safe teardown)
// ---------------------------------------------------------------------------

/// Ask a running nemesisbot gateway to shut down cleanly.
///
/// POSTs `{"cmd":"shutdown"}` to `/api/internal` — the same mpsc path Ctrl+C
/// takes (BUG #31 fix, quality-hardening goal S11e), so the process exits
/// through its normal atexit chain. A coverage-instrumented binary only
/// writes `.profraw` on that clean-exit path; [`ManagedProcess::kill`]/Drop
/// (TerminateProcess) would lose the counters.
///
/// Returns Ok once the server ack'd the command. Wait for actual process exit
/// with [`ManagedProcess::wait_for_exit`].
pub async fn graceful_shutdown_gateway(port: u16, token: &str) -> Result<()> {
    let url = format!("http://127.0.0.1:{}/api/internal", port);
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header("X-Auth-Token", token)
        .json(&serde_json::json!({"cmd": "shutdown"}))
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .with_context(|| format!("POST {url} failed"))?;
    if !resp.status().is_success() {
        bail!("POST {url} returned {}", resp.status());
    }
    println!("  graceful shutdown requested via {url}");
    Ok(())
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
    pub async fn run_cli(&self, nemesisbot_bin: &Path, args: &[&str]) -> CliOutput {
        self.run_cli_with_timeout(nemesisbot_bin, args, 15).await
    }

    /// `run_cli` variant that feeds `stdin_input` to the child's stdin and
    /// then closes it (the child sees the scripted bytes followed by EOF).
    ///
    /// This is what makes piped-stdin interactive flows testable: the REPL
    /// (rustyline falls back to `readline_direct` on a pipe — no TTY gating),
    /// the eval-rules wizard (`ask`/`ask_choice` are plain `print!` +
    /// `stdin().read_line`), and y/N confirms all walk their whole flow on a
    /// pipe. Tests drive them by scripting every answer, ending the input
    /// with the quit/EOF path where the flow expects it.
    pub async fn run_cli_with_stdin(
        &self,
        nemesisbot_bin: &Path,
        args: &[&str],
        stdin_input: &str,
        timeout_secs: u64,
    ) -> CliOutput {
        let mut full_args = vec!["--local"];
        full_args.extend(args);

        let spawned = tokio::process::Command::new(nemesisbot_bin)
            .args(&full_args)
            .current_dir(self.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::piped())
            .kill_on_drop(true)
            .envs(coverage_cli_env())
            .spawn();
        let mut child = match spawned {
            Ok(c) => c,
            Err(e) => {
                return CliOutput {
                    exit_code: -1,
                    stdout: String::new(),
                    stderr: format!("Failed to execute: {}", e),
                }
            }
        };
        // Write the script, then drop stdin so the child observes EOF —
        // that's the only way a REPL/wizard reading until EOF can terminate.
        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            let _ = stdin.write_all(stdin_input.as_bytes()).await;
            let _ = stdin.shutdown().await;
            drop(stdin);
        }
        // wait_with_output takes the Child by value; move it through an
        // Option so the async block owns it. kill_on_drop(true) means a
        // timeout (which drops the future, which drops the Child) kills
        // the child instead of leaking it.
        let mut child_opt = Some(child);
        match tokio::time::timeout(Duration::from_secs(timeout_secs), async {
            child_opt
                .take()
                .expect("child consumed twice")
                .wait_with_output()
                .await
        })
        .await
        {
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
                stderr: format!("Command timed out ({}s)", timeout_secs),
            },
        }
    }

    /// `run_cli` with an explicit per-command timeout (seconds) — for
    /// commands that legitimately take longer than the 15s default
    /// (e.g. `model catalog-update` fetching the online catalog).
    pub async fn run_cli_with_timeout(
        &self,
        nemesisbot_bin: &Path,
        args: &[&str],
        timeout_secs: u64,
    ) -> CliOutput {
        let mut full_args = vec!["--local"];
        full_args.extend(args);

        let result = tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            tokio::process::Command::new(nemesisbot_bin)
                .args(&full_args)
                .current_dir(self.path())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .envs(coverage_cli_env())
                .output(),
        )
        .await;

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
                stderr: format!("Command timed out ({}s)", timeout_secs),
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

/// The WebSocket stream type returned by [`ws_connect`] (named so callers can
/// hold one across await points without spelling out the full generic type).
pub type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Connect to WebSocket with auth token.
pub async fn ws_connect(port: u16, token: &str) -> Result<WsStream> {
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
        "timestamp": chrono::Local::now().to_rfc3339()
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
                        let content = v["data"]["content"].as_str().unwrap_or("").to_string();
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
            Err(_) => bail!("Timeout waiting for response ({}s)", timeout_secs),
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
    let exe_dir = std::env::current_exe()?.parent().unwrap().to_path_buf();

    // Try going up from test-tools/integration-test/target/release/.
    // 12 层覆盖 cargo llvm-cov 的嵌套产物目录（<covdir>/llvm-cov-target/debug/deps
    // 比标准 target/debug/deps 深 2 层；5 层上限会差一轮 check 不到 workspace root，
    // 曾致 llvm-cov 下 254 个 spawn 子进程的测试连环失败）。
    let mut dir = exe_dir.clone();
    for _ in 0..12 {
        if dir.join("Cargo.toml").exists()
            && std::fs::read_to_string(dir.join("Cargo.toml"))?.contains("[workspace]")
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
///
/// `NEMESISBOT_TEST_BIN` overrides the default target-dir resolution so a
/// coverage-instrumented build (`cargo llvm-cov -p nemesisbot --no-run`,
/// which lands in its own target dir) can be driven through the same L2
/// pipeline. Falls back to the normal lookup when unset or missing.
pub fn resolve_nemesisbot_bin() -> Result<PathBuf> {
    if let Ok(bin) = std::env::var("NEMESISBOT_TEST_BIN") {
        let bin = PathBuf::from(bin);
        if bin.exists() {
            return Ok(bin);
        }
        bail!(
            "NEMESISBOT_TEST_BIN={} does not exist; refusing silent fallback",
            bin.display()
        );
    }
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
    bail!(
        "AI server binary not found (checked test-tools/TestAIServer/testaiserver.exe and target/)"
    );
}

// ---------------------------------------------------------------------------
// Port cleanup (Windows)
// ---------------------------------------------------------------------------

/// Kill processes listening on the specified ports.
pub fn cleanup_ports(ports: &[u16]) {
    for port in ports {
        // Use netstat to find PIDs, then taskkill
        let output = std::process::Command::new("cmd")
            .args(&[
                "/c",
                &format!("netstat -ano | findstr :{} | findstr LISTENING", port),
            ])
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

#[cfg(test)]
mod tests;
