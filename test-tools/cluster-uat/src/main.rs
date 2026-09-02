//! NemesisBot Cluster UAT (User Acceptance Test)
//!
//! End-to-end verification of cluster functionality including:
//! - Multi-node startup and configuration (4 nodes: A, B, C, D)
//! - UDP discovery
//! - 2-hop peer_chat (A→B, A→C, A→D)
//! - 3-hop chain (A→B→D)
//! - 4-hop chain (A→B→C→D)
//! - Bidirectional, concurrent, and error recovery scenarios
//! - Board dispatch full chain (T15: coordinator → worker → callback writeback)
//!
//! Usage:
//!   cargo run -p cluster-uat                    # Run all tests
//!   cargo run -p cluster-uat -- --skip-long     # Skip long-running tests

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use test_harness::*;
use tokio_tungstenite::tungstenite::Message;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const AI_SERVER_PORT: u16 = 8080;
const AUTH_TOKEN: &str = "276793422";
// All 4 nodes MUST share the same cluster token. RPC frames are AEAD-encrypted
// (AES-256-GCM) with the token as the key derivation input — a per-node random
// token makes inter-node decryption impossible (logs show
// "Frame decrypt failed ... AES-GCM decrypt failed").
const SHARED_CLUSTER_TOKEN: &str = "uat-shared-cluster-token-0123456789abcdef";

struct NodeConfig {
    name: &'static str,
    /// Cluster identity role (peers.toml [node].role). Node-A is the
    /// coordinator — the board authority for T15's issue.dispatch; peer_chat
    /// (T4-T14) is role-agnostic, so this doesn't affect the hop tests.
    role: &'static str,
    web_port: u16,
    health_port: u16,
    udp_port: u16,
    rpc_port: u16,
    model: &'static str,
}

const NODES: [NodeConfig; 4] = [
    NodeConfig {
        name: "Node-A",
        role: "coordinator",
        web_port: 49000,
        // 18790 carries kernel-orphaned LISTEN sockets (ghost PID — a prior
        // run's gateways were taskkilled with pending connections; the handle
        // outlived the process). The health check then times out and aborts
        // the suite, so Node-A uses a port outside the ghost set. 18790 works
        // again after a reboot.
        health_port: 18794,
        udp_port: 11949,
        rpc_port: 21949,
        model: "test/testai-3.1",
    },
    NodeConfig {
        name: "Node-B",
        role: "worker",
        // 49001 is ghost-held (see Node-A health_port comment) — T7 connects
        // to B's web port, so it must be outside the ghost set.
        web_port: 49005,
        health_port: 18791,
        // Distinct UDP port per node — on Windows SO_REUSEADDR lets a later
        // bind *hijack* the port rather than sharing it, so 4 processes on the
        // same UDP port silently drop discovery on 3 of them. Static peers in
        // peers.toml (configured in setup_node) provide the cross-node links.
        udp_port: 11950,
        rpc_port: 21950,
        model: "test/testai-3.1",
    },
    NodeConfig {
        name: "Node-C",
        role: "worker",
        web_port: 49006,
        health_port: 18792,
        udp_port: 11951,
        rpc_port: 21951,
        model: "test/testai-3.1",
    },
    NodeConfig {
        name: "Node-D",
        role: "worker",
        web_port: 49003,
        health_port: 18793,
        udp_port: 11952,
        rpc_port: 21952,
        model: "test/testai-3.1",
    },
];

// ---------------------------------------------------------------------------
// Gateway process management
// ---------------------------------------------------------------------------

/// Managed gateway process. Both stdout and stderr are captured to the log file
/// for comprehensive multi-node tracing.
struct GatewayProcess {
    child: Option<tokio::process::Child>,
    name: &'static str,
    log_path: std::path::PathBuf,
}

impl GatewayProcess {
    fn spawn(name: &'static str, bin: &Path, cwd: &Path) -> Result<Self> {
        println!("  Starting {}...", name);
        // Redirect stderr to a log file for debugging.
        let log_path = cwd.join("gateway.log");
        let log_file = std::fs::File::create(&log_path)
            .with_context(|| format!("Cannot create log file for {}", name))?;
        let child = tokio::process::Command::new(bin)
            .args(["--local", "gateway", "--debug"])
            .env("RUST_LOG", "debug")
            .current_dir(cwd)
            .stdout(Stdio::from(log_file.try_clone()?))
            .stderr(Stdio::from(log_file))
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("Failed to spawn {}", name))?;
        println!(
            "  {} started (PID: {:?}, log: {})",
            name,
            child.id(),
            log_path.display()
        );
        Ok(Self {
            child: Some(child),
            name,
            log_path,
        })
    }

    async fn kill(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill().await;
            println!("  {} stopped", self.name);
        }
    }

    fn is_running(&mut self) -> bool {
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
}

impl Drop for GatewayProcess {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.start_kill();
        }
    }
}

// ---------------------------------------------------------------------------
// Configuration helpers
// ---------------------------------------------------------------------------

/// Modify config.json to set web server port, health check port, and debug logging.
fn configure_ports(home: &Path, web_port: u16, health_port: u16) -> Result<()> {
    let config_path = home.join("config.json");
    let raw = std::fs::read_to_string(&config_path)
        .with_context(|| format!("Reading {}", config_path.display()))?;
    let mut config: Value = serde_json::from_str(&raw)?;

    if let Some(obj) = config.as_object_mut() {
        // Set web server port (channels.web.port)
        if let Some(channels) = obj.get_mut("channels")
            && let Some(ch) = channels.as_object_mut()
        {
            if let Some(web) = ch.get_mut("web")
                && let Some(w) = web.as_object_mut()
            {
                w.insert("port".to_string(), json!(web_port));
            }
            // Disable standalone websocket channel — the web server already
            // handles WebSocket on the web port. Without this, the
            // websocket channel binds to its default port (49001), which
            // can conflict with a node's web port or a ghost listener.
            if let Some(ws) = ch.get_mut("websocket")
                && let Some(w) = ws.as_object_mut()
            {
                w.insert("enabled".to_string(), json!(false));
            }
        }
        // Set health check port (gateway.port)
        if let Some(gateway) = obj.get_mut("gateway")
            && let Some(gw) = gateway.as_object_mut()
        {
            gw.insert("port".to_string(), json!(health_port));
        }
        // Enable DEBUG level logging for detailed traces
        obj.insert(
            "logging".to_string(),
            json!({
                "general": {
                    "level": "DEBUG",
                    "enable_console": true,
                    "file": ""
                }
            }),
        );
    }

    std::fs::write(&config_path, serde_json::to_string_pretty(&config)?)?;
    Ok(())
}

/// Patch the `board` section into config.json — dispatch-timeout sweep with
/// test-speed values (production default 3600s/20s would make T18 wait an
/// hour). Must run before the gateway starts (sweep is armed at startup).
fn configure_board_sweep(home: &Path, timeout_secs: u64, interval_secs: u64) -> Result<()> {
    let config_path = home.join("config.json");
    let raw = std::fs::read_to_string(&config_path)
        .with_context(|| format!("Reading {}", config_path.display()))?;
    let mut config: Value = serde_json::from_str(&raw)?;
    if let Some(obj) = config.as_object_mut() {
        obj.insert(
            "board".to_string(),
            json!({
                "dispatch_timeout_secs": timeout_secs,
                "dispatch_sweep_interval_secs": interval_secs,
            }),
        );
    }
    std::fs::write(&config_path, serde_json::to_string_pretty(&config)?)?;
    Ok(())
}

/// Spawn a gateway and wait for full readiness: HTTP health → RPC port
/// listening → UDP re-discovery settle. Shared by the T9/T17/T18 restart
/// flows (kill is the caller's job — T9 probes offline state in between).
async fn start_gateway_and_wait(
    name: &'static str,
    bin: &Path,
    ws_path: &Path,
    node: &NodeConfig,
) -> Result<GatewayProcess, String> {
    let gw = GatewayProcess::spawn(name, bin, ws_path)
        .map_err(|e| format!("Cannot start {}: {}", name, e))?;

    // Wait for HTTP health check (gateway web server up)
    let health_url = format!("http://127.0.0.1:{}/health", node.health_port);
    wait_for_http(&health_url, Duration::from_secs(15))
        .await
        .map_err(|e| format!("{} not healthy after start: {}", name, e))?;

    // Wait for the RPC server to be listening
    let rpc_addr = format!("127.0.0.1:{}", node.rpc_port);
    let rpc_ready = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if tokio::net::TcpStream::connect(&rpc_addr).await.is_ok() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    })
    .await
    .unwrap_or(false);
    if !rpc_ready {
        return Err(format!("{} RPC server not ready at {}", name, rpc_addr));
    }
    println!(
        "    {} restarted and healthy (RPC on {})",
        name, node.rpc_port
    );

    // UDP announce has 0-5s jitter; broadcast_interval is 3s in tests, so
    // 15s covers jitter + processing before callers rely on discovery.
    println!("    Waiting for UDP discovery to propagate (15s)...");
    tokio::time::sleep(Duration::from_secs(15)).await;
    Ok(gw)
}

/// Configure a single cluster node via CLI commands.
///
/// Each node gets its own UDP port (Windows SO_REUSEADDR semantics hijack
/// rather than share — see `NODES` comment) so UDP auto-discovery does not
/// link them. Instead, we seed each node's `peers.toml` with the other three
/// nodes' UDP addresses — gateway.rs derives the RPC port via the
/// `udp_port + 10000` convention (e.g., 11950→21950) and routes cluster_rpc
/// calls accordingly.
async fn setup_node(ws: &TestWorkspace, bin: &Path, node: &NodeConfig) -> Result<()> {
    let name = node.name;
    println!("\n  Configuring {}...", name);

    // 1. Onboard with default config
    let out = ws.run_cli(bin, &["onboard", "default"]).await;
    if !out.success() {
        bail!("{}: onboard failed: {}", name, out.stderr);
    }

    // 2. Set web/health ports in config.json
    configure_ports(&ws.home(), node.web_port, node.health_port)
        .with_context(|| format!("{}: configure_ports failed", name))?;

    // 3. Add AI model
    let out = ws
        .run_cli(
            bin,
            &[
                "model",
                "add",
                "--model",
                node.model,
                "--base",
                &format!("http://127.0.0.1:{}/v1", AI_SERVER_PORT),
                "--key",
                "test-key",
                "--default",
            ],
        )
        .await;
    if !out.success() {
        bail!("{}: model add failed: {}", name, out.stderr);
    }

    // 4. Initialize cluster (role from NODES: A=coordinator, rest=worker)
    let out = ws
        .run_cli(
            bin,
            &[
                "cluster",
                "init",
                "--name",
                name,
                "--role",
                node.role,
                "--category",
                "development",
            ],
        )
        .await;
    if !out.success() {
        bail!("{}: cluster init failed: {}", name, out.stderr);
    }

    // 4a. Override the per-node random token with the shared token.
    // cluster init generates a unique UUID per node, but RPC AEAD requires
    // every node to derive the same key from the same token.
    let out = ws
        .run_cli(bin, &["cluster", "token", "set", SHARED_CLUSTER_TOKEN])
        .await;
    if !out.success() {
        bail!("{}: cluster token set failed: {}", name, out.stderr);
    }

    // 5. Configure cluster ports (per-node UDP+RPC; short broadcast interval)
    let out = ws
        .run_cli(
            bin,
            &[
                "cluster",
                "config",
                "--udp-port",
                &node.udp_port.to_string(),
                "--rpc-port",
                &node.rpc_port.to_string(),
                "--broadcast-interval",
                "3",
            ],
        )
        .await;
    if !out.success() {
        bail!("{}: cluster config failed: {}", name, out.stderr);
    }

    // 6. Add the other three nodes as static peers.
    // gateway.rs convention: the `address` field holds the UDP host:port,
    // and the RPC port is derived as `udp_port + 10000` (e.g., 11950→21950).
    // Passing the RPC port here would cause gateway to derive rpc_port=rpc+10000
    // and cluster_rpc connections would fail with "peer not found".
    for peer in NODES.iter() {
        if peer.name == node.name {
            continue;
        }
        let out = ws
            .run_cli(
                bin,
                &[
                    "cluster",
                    "peers",
                    "add",
                    "--id",
                    peer.name,
                    "--name",
                    peer.name,
                    "--address",
                    &format!("127.0.0.1:{}", peer.udp_port),
                    "--role",
                    peer.role,
                ],
            )
            .await;
        if !out.success() {
            bail!("{}: peers add {} failed: {}", name, peer.name, out.stderr);
        }
    }

    // 7. Enable cluster
    let out = ws.run_cli(bin, &["cluster", "enable"]).await;
    if !out.success() {
        bail!("{}: cluster enable failed: {}", name, out.stderr);
    }

    println!(
        "  {} configured OK (static peers + UDP port {})",
        name, node.udp_port
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// WebSocket helpers
// ---------------------------------------------------------------------------

/// Connect to a gateway's WebSocket endpoint.
async fn ws_connect_gateway(
    port: u16,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
> {
    test_harness::ws_connect(port, AUTH_TOKEN).await
}

/// Send a chat message via WebSocket and wait for a response.
async fn ws_send_recv(
    stream: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    content: &str,
    timeout_secs: u64,
) -> Result<String> {
    test_harness::ws_send_and_recv(stream, content, timeout_secs).await
}

/// Send a message and wait for a chat.receive response matching a predicate.
/// Skips non-matching chat.receive messages. Returns the first matching response.
/// If timeout is reached without a match, returns Err.
async fn ws_send_recv_until<P: Fn(&str) -> bool>(
    stream: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    content: &str,
    timeout_secs: u64,
    predicate: P,
) -> Result<String> {
    let msg = json!({
        "type": "message",
        "module": "chat",
        "cmd": "send",
        "data": { "content": content },
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
                        if predicate(&content) {
                            return Ok(content);
                        }
                        // Skip non-matching message
                    }
                    if msg_type == "system" && module == "error" {
                        let err = v["data"]["content"]
                            .as_str()
                            .unwrap_or("unknown error")
                            .to_string();
                        return Err(anyhow::anyhow!("Server error: {}", err));
                    }
                }
            }
            Ok(Some(Ok(Message::Ping(_)))) => {
                let _ = stream.send(Message::Pong(vec![].into())).await;
            }
            Ok(Some(Ok(Message::Close(_)))) => {
                return Err(anyhow::anyhow!("WebSocket closed"));
            }
            Ok(Some(Ok(_))) => {} // Ignore Binary, Pong, Frame
            Ok(None) => return Err(anyhow::anyhow!("WebSocket stream ended")),
            Ok(Some(Err(e))) => return Err(anyhow::anyhow!("WebSocket error: {}", e)),
            Err(_) => {
                return Err(anyhow::anyhow!(
                    "Timeout after {}s (no matching response)",
                    timeout_secs
                ));
            }
        }
    }
}

/// Gateway WebSocket stream type (matches `test_harness::ws_connect`).
type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Send a WS API request (`type=request`) and wait for the matching response
/// (correlated by `reqId`; non-matching frames — chat.receive, pushes — are
/// skipped). Returns the response `data` payload. A non-null `error` field is
/// surfaced as Err.
async fn ws_api_request(
    stream: &mut WsStream,
    module: &str,
    cmd: &str,
    data: Value,
    timeout_secs: u64,
) -> Result<Value> {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(1);
    let req_id = format!(
        "uat-{}-{}",
        cmd.replace('.', "-"),
        SEQ.fetch_add(1, Ordering::Relaxed)
    );
    let msg = json!({
        "type": "request",
        "module": module,
        "cmd": cmd,
        "reqId": req_id,
        "data": data,
        "timestamp": chrono::Local::now().to_rfc3339()
    });
    stream.send(Message::Text(msg.to_string().into())).await?;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        let resp = tokio::time::timeout_at(deadline, stream.next()).await;
        match resp {
            Ok(Some(Ok(Message::Text(text)))) => {
                let Ok(v) = serde_json::from_str::<Value>(text.as_ref()) else {
                    continue;
                };
                if v.get("type").and_then(|t| t.as_str()) != Some("response") {
                    continue; // chat.receive / push / heartbeat — not ours
                }
                if v.get("reqId").and_then(|r| r.as_str()) != Some(req_id.as_str()) {
                    continue;
                }
                if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
                    return Err(anyhow::anyhow!("{} {} failed: {}", module, cmd, err));
                }
                return Ok(v.get("data").cloned().unwrap_or(Value::Null));
            }
            Ok(Some(Ok(Message::Ping(_)))) => {
                let _ = stream.send(Message::Pong(vec![].into())).await;
            }
            Ok(Some(Ok(Message::Close(_)))) => {
                return Err(anyhow::anyhow!("WebSocket closed"));
            }
            Ok(Some(Ok(_))) => {} // Ignore Binary, Pong, Frame
            Ok(None) => return Err(anyhow::anyhow!("WebSocket stream ended")),
            Ok(Some(Err(e))) => return Err(anyhow::anyhow!("WebSocket error: {}", e)),
            Err(_) => {
                return Err(anyhow::anyhow!(
                    "Timeout after {}s waiting for {} {} response",
                    timeout_secs,
                    module,
                    cmd
                ));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Test runner
// ---------------------------------------------------------------------------

/// Execute a single named test and print the outcome.
async fn run_test<F, Fut>(name: &'static str, f: F) -> TestResult
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = TestResult>,
{
    print!("\n  [TEST] {} ... ", name);
    let result = f().await;
    let status = if result.message.starts_with("SKIP:") {
        "SKIP"
    } else if result.passed {
        "PASS"
    } else {
        "FAIL"
    };
    println!("{}", status);
    if !result.passed && !result.message.is_empty() {
        println!("         {}", result.message);
    }
    result
}

/// Truncate a string for display (char-boundary safe — byte-slicing Chinese
/// text panics, see docs BUG str-slice family).
fn trunc(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut end = max;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &s[..end])
    }
}

// ---------------------------------------------------------------------------
// CLI argument parsing
// ---------------------------------------------------------------------------

struct Args {
    _skip_long: bool,
    _filter: Option<String>,
}

fn parse_args() -> Args {
    let args: Vec<String> = std::env::args().collect();
    let mut skip_long = false;
    let mut filter = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--skip-long" => skip_long = true,
            "--filter" => {
                i += 1;
                if i < args.len() {
                    filter = Some(args[i].clone());
                }
            }
            _ => {}
        }
        i += 1;
    }
    Args {
        _skip_long: skip_long,
        _filter: filter,
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    let _args = parse_args();

    println!("========================================");
    println!("  NemesisBot Cluster UAT Test Suite");
    println!("========================================");

    let mut all_results: Vec<TestResult> = Vec::new();

    // ------------------------------------------------------------------
    // Phase 1: Resolve binaries
    // ------------------------------------------------------------------
    println!("\n--- Phase 1: Resolve binaries ---");

    let root = match resolve_project_root() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("ERROR: Cannot find project root: {}", e);
            std::process::exit(1);
        }
    };
    println!("  Project root: {}", root.display());

    let gateway_bin =
        resolve_nemesisbot_bin().unwrap_or_else(|_| root.join("target/release/nemesisbot.exe"));
    let ai_server_bin = resolve_ai_server_bin()
        .unwrap_or_else(|_| root.join("test-tools/TestAIServer/testaiserver.exe"));

    if !gateway_bin.exists() {
        eprintln!(
            "ERROR: nemesisbot binary not found at {}",
            gateway_bin.display()
        );
        std::process::exit(1);
    }
    if !ai_server_bin.exists() {
        eprintln!(
            "ERROR: TestAIServer binary not found at {}",
            ai_server_bin.display()
        );
        std::process::exit(1);
    }
    println!("  Gateway: {}", gateway_bin.display());
    println!("  AI Server: {}", ai_server_bin.display());

    // ------------------------------------------------------------------
    // Phase 2: Cleanup ports
    // ------------------------------------------------------------------
    println!("\n--- Phase 2: Cleanup ports ---");

    let all_ports: Vec<u16> = NODES
        .iter()
        .flat_map(|n| vec![n.web_port, n.health_port, n.udp_port, n.rpc_port])
        .chain(std::iter::once(AI_SERVER_PORT))
        .collect();
    cleanup_ports(&all_ports);
    println!("  Cleaned {} ports", all_ports.len());

    // Pre-flight probe: any port that still ACCEPTS connections after
    // cleanup is held by something cleanup_ports can't kill — typically a
    // ghost listener (kernel-orphaned socket whose owning PID is gone; only
    // a reboot clears it). Aborting here with a clear message beats a test
    // hanging on a WebSocket handshake that never completes.
    let mut ghost_ports: Vec<u16> = Vec::new();
    for port in &all_ports {
        if tokio::net::TcpStream::connect(("127.0.0.1", *port))
            .await
            .is_ok()
        {
            ghost_ports.push(*port);
        }
    }
    if !ghost_ports.is_empty() {
        eprintln!(
            "\nERROR: ports still accepting connections after cleanup: {:?}. \
             Ghost listeners cannot be killed (owning PID is gone). \
             Reboot, or edit NODES to use free ports.",
            ghost_ports
        );
        std::process::exit(1);
    }

    // ------------------------------------------------------------------
    // Phase 3: Create isolated workspaces
    // ------------------------------------------------------------------
    println!("\n--- Phase 3: Create workspaces ---");

    let ws_a = TestWorkspace::new().expect("Cannot create workspace A");
    let ws_b = TestWorkspace::new().expect("Cannot create workspace B");
    let ws_c = TestWorkspace::new().expect("Cannot create workspace C");
    let ws_d = TestWorkspace::new().expect("Cannot create workspace D");
    println!("  Workspace A: {}", ws_a.path().display());
    println!("  Workspace B: {}", ws_b.path().display());
    println!("  Workspace C: {}", ws_c.path().display());
    println!("  Workspace D: {}", ws_d.path().display());

    // ------------------------------------------------------------------
    // Phase 4: Configure cluster nodes
    // ------------------------------------------------------------------
    println!("\n--- Phase 4: Configure nodes ---");

    // Configure each node — no static peers, pure UDP discovery
    if let Err(e) = setup_node(&ws_a, &gateway_bin, &NODES[0]).await {
        eprintln!("ERROR: {}", e);
        std::process::exit(1);
    }
    // T18 (worker offline/timeout sweep) needs a fast sweep on the
    // coordinator — production default (3600s timeout / 20s interval) would
    // stall the test for an hour. 15s timeout / 2s interval ⇒ failure lands
    // within ~17s of dispatch.
    if let Err(e) = configure_board_sweep(&ws_a.home(), 15, 2) {
        eprintln!("ERROR: configure_board_sweep for Node-A: {}", e);
        std::process::exit(1);
    }

    if let Err(e) = setup_node(&ws_b, &gateway_bin, &NODES[1]).await {
        eprintln!("ERROR: {}", e);
        std::process::exit(1);
    }

    if let Err(e) = setup_node(&ws_c, &gateway_bin, &NODES[2]).await {
        eprintln!("ERROR: {}", e);
        std::process::exit(1);
    }

    if let Err(e) = setup_node(&ws_d, &gateway_bin, &NODES[3]).await {
        eprintln!("ERROR: {}", e);
        std::process::exit(1);
    }

    // ------------------------------------------------------------------
    // Phase 5: Start TestAIServer
    // ------------------------------------------------------------------
    println!("\n--- Phase 5: Start TestAIServer ---");

    let mut ai_server = ManagedProcess::spawn("TestAIServer", &ai_server_bin, &[], &root)
        .expect("Cannot start TestAIServer");

    match wait_for_http(
        &format!("http://127.0.0.1:{}/v1/models", AI_SERVER_PORT),
        Duration::from_secs(10),
    )
    .await
    {
        Ok(_) => println!("  TestAIServer ready on port {}", AI_SERVER_PORT),
        Err(e) => {
            eprintln!("ERROR: TestAIServer not ready: {}", e);
            ai_server.kill().await;
            std::process::exit(1);
        }
    }

    // ------------------------------------------------------------------
    // Phase 6: Start gateway processes
    // ------------------------------------------------------------------
    println!("\n--- Phase 6: Start gateways ---");

    let mut gw_a = GatewayProcess::spawn("Gateway-A", &gateway_bin, ws_a.path())
        .expect("Cannot start Gateway-A");
    let mut gw_b = GatewayProcess::spawn("Gateway-B", &gateway_bin, ws_b.path())
        .expect("Cannot start Gateway-B");
    let mut gw_c = GatewayProcess::spawn("Gateway-C", &gateway_bin, ws_c.path())
        .expect("Cannot start Gateway-C");
    let mut gw_d = GatewayProcess::spawn("Gateway-D", &gateway_bin, ws_d.path())
        .expect("Cannot start Gateway-D");

    // ------------------------------------------------------------------
    // Phase 7: Wait for health checks
    // ------------------------------------------------------------------
    println!("\n--- Phase 7: Health checks ---");

    let mut all_healthy = true;
    for (i, _gw) in [&mut gw_a, &mut gw_b, &mut gw_c, &mut gw_d]
        .iter_mut()
        .enumerate()
    {
        let url = format!("http://127.0.0.1:{}/health", NODES[i].health_port);
        match wait_for_http(&url, Duration::from_secs(15)).await {
            Ok(_) => println!("  {} ready (health OK)", NODES[i].name),
            Err(e) => {
                eprintln!("  {} NOT ready: {}", NODES[i].name, e);
                all_healthy = false;
            }
        }
    }

    if !all_healthy {
        eprintln!("\nERROR: Not all gateways are healthy. Aborting.");
        gw_d.kill().await;
        gw_c.kill().await;
        gw_b.kill().await;
        gw_a.kill().await;
        ai_server.kill().await;
        std::process::exit(1);
    }

    // ==================================================================
    // Run Tests
    // ==================================================================
    println!("\n========================================");
    println!("  Running Tests (T1-T18, 4-node full chain verification)");
    println!("========================================");

    // T1: Node startup and configuration verification
    all_results.push(
        run_test("T1: Node startup & config", || async {
            for (i, ws) in [&ws_a, &ws_b, &ws_c, &ws_d].iter().enumerate() {
                let out = ws.run_cli(&gateway_bin, &["cluster", "status"]).await;
                if !out.success() {
                    return fail(
                        "T1",
                        format!("{}: cluster status failed: {}", NODES[i].name, out.stderr),
                    );
                }
                if !out.stdout_contains("Config:") {
                    return fail(
                        "T1",
                        format!("{}: missing Config line in output", NODES[i].name),
                    );
                }
                // Verify enabled
                if !out.stdout_contains("Enabled: true") && !out.stdout_contains("enabled: true") {
                    return fail(
                        "T1",
                        format!(
                            "{}: cluster not enabled. Output: {}",
                            NODES[i].name,
                            trunc(&out.stdout, 200)
                        ),
                    );
                }
            }
            pass("T1", "All 4 nodes configured and reporting enabled")
        })
        .await,
    );

    // T2: Peer graph established (static peers configured per-node)
    // We use per-node UDP ports (Windows SO_REUSEADDR semantics differ from
    // Linux), so cross-node links come from peers.toml rather than UDP
    // announce. The test verifies each node's peers.toml lists the other three.
    all_results.push(
        run_test("T2: Peer graph (static peers)", || async {
            // Verify nodes are still running
            if !gw_a.is_running() || !gw_b.is_running() || !gw_c.is_running() || !gw_d.is_running()
            {
                return fail("T2", "One or more nodes crashed during startup");
            }

            for node in NODES.iter() {
                let peers_path = match node.name {
                    "Node-A" => ws_a
                        .home()
                        .join("workspace")
                        .join("cluster")
                        .join("peers.toml"),
                    "Node-B" => ws_b
                        .home()
                        .join("workspace")
                        .join("cluster")
                        .join("peers.toml"),
                    "Node-C" => ws_c
                        .home()
                        .join("workspace")
                        .join("cluster")
                        .join("peers.toml"),
                    "Node-D" => ws_d
                        .home()
                        .join("workspace")
                        .join("cluster")
                        .join("peers.toml"),
                    _ => unreachable!(),
                };
                let content = std::fs::read_to_string(&peers_path).unwrap_or_default();
                for other in NODES.iter() {
                    if other.name == node.name {
                        continue;
                    }
                    // cluster peers add sanitizes the id into a TOML key.
                    // Per TOML v1.0.0, `-` is a legal bare key char so it's
                    // preserved as-is. Only `.` and `:` get replaced with `_`.
                    let sanitized = other.name.replace(['.', ':'], "_");
                    if !content.contains(&format!("[peers.{}]", sanitized)) {
                        return fail(
                            "T2",
                            format!(
                                "{} peers.toml missing entry for {} (looked for [peers.{}])",
                                node.name, other.name, sanitized
                            ),
                        );
                    }
                }
            }
            pass(
                "T2",
                "All 4 nodes have the other 3 as static peers".to_string(),
            )
        })
        .await,
    );

    // T3: Static peers loaded into Node-A's PeerRegistry
    // After Node-A's gateway has been running, query its peers list via CLI
    // and verify all three peers (Node-B/C/D) are visible. This validates
    // that peers.toml was correctly loaded by the runtime. The CLI prints
    // the file content, so peer ids appear in their sanitized form (Node_B).
    all_results.push(
        run_test("T3: PeerRegistry loaded from peers.toml", || async {
            let out = ws_a
                .run_cli(&gateway_bin, &["cluster", "peers", "list"])
                .await;
            let stdout = out.stdout.clone();
            // cluster peers add sanitizes "Node-B" → "Node_B" in the TOML key.
            let has_b = stdout.contains("Node_B") || stdout.contains("Node-B");
            let has_c = stdout.contains("Node_C") || stdout.contains("Node-C");
            let has_d = stdout.contains("Node_D") || stdout.contains("Node-D");
            if has_b && has_c && has_d {
                pass(
                    "T3",
                    format!(
                        "Node-A sees Node-B/C/D in peers list (exit={}, {} bytes)",
                        out.exit_code,
                        stdout.len()
                    ),
                )
            } else {
                fail(
                    "T3",
                    format!(
                        "PeerRegistry missing peers: B={} C={} D={} (exit={}, stdout: {})",
                        has_b,
                        has_c,
                        has_d,
                        out.exit_code,
                        trunc(&stdout, 200)
                    ),
                )
            }
        })
        .await,
    );

    // T4: User → A → B (2-hop peer_chat with full async chain)
    // Use ws_send_recv_until to skip intermediate messages and match the continuation response.
    // The number of intermediate messages varies depending on LLM behavior.
    all_results.push(
        run_test("T4: 2-hop A→B (full async chain)", || async {
            let mut ws = match ws_connect_gateway(NODES[0].web_port).await {
                Ok(s) => s,
                Err(e) => return fail("T4", format!("WS connect to A failed: {}", e)),
            };
            let msg = r#"<PEER_CHAT>{"peer_id":"Node-B","content":"hello from A"}</PEER_CHAT>"#;
            match ws_send_recv_until(&mut ws, msg, 180, |resp| {
                resp.contains("hello from A") || resp.contains("echo")
            })
            .await
            {
                Ok(resp) => {
                    if resp.contains("hello from A") {
                        pass("T4", format!("完整异步 2-hop A→B: {}", trunc(&resp, 100)))
                    } else {
                        pass("T4", format!("2-hop A→B 响应: {}", trunc(&resp, 100)))
                    }
                }
                Err(e) => fail("T4", format!("180s 内未收到续行响应: {}", e)),
            }
        })
        .await,
    );

    // T5: User → A → D (2-hop, D uses testai-3.1 which echoes content back)
    all_results.push(
        run_test("T5: 2-hop A→D (full async chain)", || async {
            let mut ws = match ws_connect_gateway(NODES[0].web_port).await {
                Ok(s) => s,
                Err(e) => return fail("T5", format!("WS connect to A failed: {}", e)),
            };
            let msg = r#"<PEER_CHAT>{"peer_id":"Node-D","content":"hello to D"}</PEER_CHAT>"#;
            match ws_send_recv_until(&mut ws, msg, 180, |resp| {
                resp.contains("hello to D") || resp.contains("hello")
            })
            .await
            {
                Ok(resp) => pass("T5", format!("完整异步 2-hop A→D: {}", trunc(&resp, 100))),
                Err(e) => fail("T5", format!("180s 内未收到续行响应: {}", e)),
            }
        })
        .await,
    );

    // T6: 3-hop A→B→D — route format for multi-hop.
    // testai-3.1 extracts route[0] (Node-B), passes remaining route [Node-D] to B.
    // B extracts route[0] (Node-D), passes content to D. D echoes back.
    all_results.push(
        run_test("T6: 3-hop A→B→D (route format)", || async {
            let mut ws = match ws_connect_gateway(NODES[0].web_port).await {
                Ok(s) => s,
                Err(e) => return fail("T6", format!("WS connect to A failed: {}", e)),
            };
            // Route format: A→B→D
            let msg = r#"<PEER_CHAT>{"route":["Node-B","Node-D"],"content":"hello from A via B"}</PEER_CHAT>"#;
            match ws_send_recv_until(&mut ws, msg, 300, |resp| {
                resp.contains("hello from A via B") || resp.contains("hello")
            }).await {
                Ok(content) => {
                    if !content.is_empty() {
                        pass("T6", format!("3-hop response received ({} chars): {}", content.len(), trunc(&content, 200)))
                    } else {
                        fail("T6", String::from("Response was empty"))
                    }
                }
                Err(e) => fail("T6", format!("300s 内未收到 3-hop 续行响应: {}", e)),
            }
        })
        .await,
    );

    // T7: Bidirectional B → A (full async chain)
    all_results.push(
        run_test("T7: Bidirectional B→A (full async chain)", || async {
            let mut ws = match ws_connect_gateway(NODES[1].web_port).await {
                Ok(s) => s,
                Err(e) => return fail("T7", format!("WS connect to B failed: {}", e)),
            };
            let msg = r#"<PEER_CHAT>{"peer_id":"Node-A","content":"hello from B"}</PEER_CHAT>"#;
            match ws_send_recv_until(&mut ws, msg, 180, |resp| {
                resp.contains("hello from B") || resp.contains("echo")
            })
            .await
            {
                Ok(resp) => {
                    if resp.contains("hello from B") {
                        pass("T7", format!("完整双向 B→A: {}", trunc(&resp, 100)))
                    } else {
                        pass("T7", format!("双向 B→A 响应: {}", trunc(&resp, 100)))
                    }
                }
                Err(e) => fail("T7", format!("180s 内未收到续行响应: {}", e)),
            }
        })
        .await,
    );

    // T8: Concurrent requests (full async chain — each goes through real LLM + continuation)
    all_results.push(
        run_test("T8: Concurrent requests (x3, full async)", || async {
            let mut handles = Vec::new();
            for i in 0..3u32 {
                let port = NODES[0].web_port;
                let content = format!("concurrent-msg-{}", i);
                let handle = tokio::spawn(async move {
                    let mut ws = match ws_connect_gateway(port).await {
                        Ok(s) => s,
                        Err(e) => return Err(format!("WS connect failed: {}", e)),
                    };
                    let msg = format!(
                        r#"<PEER_CHAT>{{"peer_id":"Node-B","content":"{}"}}</PEER_CHAT>"#,
                        content
                    );
                    match ws_send_recv_until(&mut ws, &msg, 180, |resp| {
                        resp.contains(&content) || resp.contains("concurrent-msg")
                    })
                    .await
                    {
                        // Either the echoed content or the continuation response is
                        // acceptable; both are returned as-is.
                        Ok(resp) => Ok(resp),
                        Err(e) => Err(format!("无续行响应: {}", e)),
                    }
                });
                handles.push(handle);
            }

            let mut pass_count = 0usize;
            let mut fail_count = 0usize;
            for handle in handles {
                match handle.await {
                    Ok(Ok(_resp)) => pass_count += 1,
                    Ok(Err(e)) => {
                        fail_count += 1;
                        println!("         Concurrent error: {}", e);
                    }
                    Err(e) => {
                        fail_count += 1;
                        println!("         Task join error: {}", e);
                    }
                }
            }

            if fail_count == 0 {
                pass(
                    "T8",
                    format!("All {} concurrent async requests succeeded", pass_count),
                )
            } else {
                fail(
                    "T8",
                    format!("{}/{} requests failed", fail_count, pass_count + fail_count),
                )
            }
        })
        .await,
    );

    // T9: Node offline and recovery (full async chain)
    //
    // Recovery flow:
    // 1. Kill D → A still has D in registry (no "bye" sent on kill)
    // 2. Offline test → cluster_rpc to D fails (TCP refused)
    // 3. Restart D → D sends UDP announce (0-5s jitter) → A marks D Online
    // 4. Retry → full async chain works
    //
    // Key timing: after D restarts, we must wait for:
    //   a) D's RPC server to be listening (TCP port check)
    //   b) D's UDP announce to reach A (broadcast_interval + jitter)
    all_results.push(
        run_test("T9: Node offline & recovery (full async)", || async {
            // Step 1: Stop node D
            gw_d.kill().await;
            println!("    Node-D stopped");
            tokio::time::sleep(Duration::from_secs(2)).await;

            // Verify A is still running
            if !gw_a.is_running() {
                return fail("T9", "Node-A crashed after D went offline");
            }

            // Step 2: Try sending to D while offline — should get an error response
            let mut ws = match ws_connect_gateway(NODES[0].web_port).await {
                Ok(s) => s,
                Err(e) => return fail("T9", format!("WS connect failed: {}", e)),
            };
            let msg = r#"<PEER_CHAT>{"peer_id":"Node-D","content":"offline test"}</PEER_CHAT>"#;
            let result = ws_send_recv(&mut ws, msg, 30).await;
            let got_error = result.is_err();
            println!(
                "    Offline test: {}",
                if got_error {
                    "error/timeout as expected"
                } else {
                    "got response (intermediate msg before RPC failure)"
                }
            );

            // Step 3: Restart D and wait for full readiness
            // (health → RPC port → UDP re-discovery, shared helper)
            gw_d = match start_gateway_and_wait("Gateway-D", &gateway_bin, ws_d.path(), &NODES[3])
                .await
            {
                Ok(g) => g,
                Err(e) => return fail("T9", e),
            };

            // Step 4: Retry — should succeed with full async chain
            let mut ws2 = match ws_connect_gateway(NODES[0].web_port).await {
                Ok(s) => s,
                Err(e) => return fail("T9", format!("WS connect after restart failed: {}", e)),
            };
            // Use ws_send_recv_until to skip intermediate messages and wait for
            // the actual continuation response containing D's LLM output.
            // D uses testai-3.1 which echoes content back.
            match ws_send_recv_until(&mut ws2, msg, 180, |resp| {
                resp.contains("offline test") || resp.contains("hello")
            })
            .await
            {
                Ok(resp) => pass(
                    "T9",
                    format!(
                        "Recovered: offline_err={}, continuation='{}'",
                        got_error,
                        trunc(&resp, 80)
                    ),
                ),
                Err(e) => fail(
                    "T9",
                    format!(
                        "180s 内未收到续行响应 (UDP discovery may have failed): {}",
                        e
                    ),
                ),
            }
        })
        .await,
    );

    // T10: Large payload (4KB, full async chain)
    // Uses ws_send_recv_until to skip intermediate messages and wait for the
    // actual continuation response that contains the echoed large payload.
    all_results.push(
        run_test("T10: Large payload (4KB, full async)", || async {
            let mut ws = match ws_connect_gateway(NODES[0].web_port).await {
                Ok(s) => s,
                Err(e) => return fail("T10", format!("WS connect failed: {}", e)),
            };
            let large_content = "X".repeat(4096);
            let msg = format!(
                r#"<PEER_CHAT>{{"peer_id":"Node-B","content":"{}"}}</PEER_CHAT>"#,
                large_content
            );
            // Wait for a response that is clearly the continuation (contains "X" and is large),
            // skipping the intermediate "已发送请求..." message.
            match ws_send_recv_until(&mut ws, &msg, 180, |resp| {
                resp.contains("X") && resp.len() > 100
            })
            .await
            {
                Ok(resp) => pass("T10", format!("大消息异步 OK ({} bytes)", resp.len())),
                Err(e) => fail("T10", format!("180s 内未收到匹配的续行响应: {}", e)),
            }
        })
        .await,
    );

    // T11: 4-hop A→B→C→D — route format for multi-hop chain call.
    // testai-3.1 extracts route[0] at each hop, forwards remaining route.
    // A→B→C→D: A extracts B, B extracts C, C extracts D, D echoes content.
    // Callbacks chain back: D→C→B→A.
    all_results.push(
        run_test("T11: 4-hop A→B→C→D (route format)", || async {
            let mut ws = match ws_connect_gateway(NODES[0].web_port).await {
                Ok(s) => s,
                Err(e) => return fail("T11", format!("WS connect to A failed: {}", e)),
            };
            // Route format: A→B→C→D
            let msg = r#"<PEER_CHAT>{"route":["Node-B","Node-C","Node-D"],"content":"hello from A via B via C"}</PEER_CHAT>"#;
            match ws_send_recv_until(&mut ws, msg, 420, |resp| {
                resp.contains("hello from A via B via C") || resp.contains("hello")
            }).await {
                Ok(content) => {
                    if !content.is_empty() {
                        pass("T11", format!("4-hop response received ({} chars): {}", content.len(), trunc(&content, 200)))
                    } else {
                        fail("T11", String::from("Response was empty"))
                    }
                }
                Err(e) => fail("T11", format!("420s 内未收到 4-hop 续行响应: {}", e)),
            }
        })
        .await,
    );

    // T12: 2-hop A→C (C uses testai-3.1, echoes back content)
    all_results.push(
        run_test("T12: 2-hop A→C (full async chain)", || async {
            let mut ws = match ws_connect_gateway(NODES[0].web_port).await {
                Ok(s) => s,
                Err(e) => return fail("T12", format!("WS connect to A failed: {}", e)),
            };
            let msg =
                r#"<PEER_CHAT>{"peer_id":"Node-C","content":"hello direct to C"}</PEER_CHAT>"#;
            match ws_send_recv_until(&mut ws, msg, 180, |resp| {
                resp.contains("hello direct to C") || resp.contains("hello")
            })
            .await
            {
                Ok(resp) => pass("T12", format!("完整异步 2-hop A→C: {}", trunc(&resp, 100))),
                Err(e) => fail("T12", format!("180s 内未收到续行响应: {}", e)),
            }
        })
        .await,
    );

    // T13: Bidirectional D → A (from D's WebSocket to A)
    all_results.push(
        run_test("T13: Bidirectional D→A (full async chain)", || async {
            let mut ws = match ws_connect_gateway(NODES[3].web_port).await {
                Ok(s) => s,
                Err(e) => return fail("T13", format!("WS connect to D failed: {}", e)),
            };
            let msg = r#"<PEER_CHAT>{"peer_id":"Node-A","content":"hello from D"}</PEER_CHAT>"#;
            match ws_send_recv_until(&mut ws, msg, 180, |resp| {
                resp.contains("hello from D") || resp.contains("echo")
            })
            .await
            {
                Ok(resp) => {
                    if resp.contains("hello from D") {
                        pass("T13", format!("完整双向 D→A: {}", trunc(&resp, 100)))
                    } else {
                        pass("T13", format!("双向 D→A 响应: {}", trunc(&resp, 100)))
                    }
                }
                Err(e) => fail("T13", format!("180s 内未收到续行响应: {}", e)),
            }
        })
        .await,
    );

    // T14: cluster_continuation persists final reply to Node-A session_logs
    //
    // Regression test for the bug where handle_cluster_continuation sent the
    // AI's final reply through outbound_tx but never wrote it to
    // session_logs/ — the user could see the reply in the dashboard but the
    // JSONL history skipped it.
    //
    // Strategy:
    //   1. Send a PEER_CHAT to Node-B with a unique marker in the content
    //   2. Wait for the continuation response (testai-3.1 echoes content)
    //   3. Scan ws_a's session_logs/*.jsonl for any file containing the marker
    //   4. Verify that file has BOTH a "user" row AND an "assistant" row
    //      containing the marker — i.e. the continuation reply was persisted
    //      under the same session_key as the user message
    //
    // If this test fails on the assistant-row assertion but passes on the
    // user-row one, the regression is back: handle_cluster_continuation is
    // skipping log writes (empty session_key guard, or the session_key
    // plumbing through ContinuationData broke).
    all_results.push(
        run_test("T14: session_log persists continuation reply", || async {
            // Non-numeric marker. A previous version used a 13-digit millis
            // timestamp, which tripped the DLP credit_card rule (dlp.rs) —
            // cluster_rpc got blocked as "sensitive data", the continuation
            // never happened, and the test timed out at 180s. Letters only so
            // DLP doesn't flag it; still unique within the run (no other test
            // uses this marker, and each run gets a fresh temp workspace).
            let marker = "T14_SESSIONLOG_MARKER_UNIQUETESTXYZ".to_string();
            let user_payload = format!(
                r#"<PEER_CHAT>{{"peer_id":"Node-B","content":"{}"}}</PEER_CHAT>"#,
                marker
            );

            let mut ws = match ws_connect_gateway(NODES[0].web_port).await {
                Ok(s) => s,
                Err(e) => return fail("T14", format!("WS connect to A failed: {}", e)),
            };
            // Wait for the continuation response. testai-3.1 echoes the
            // content back, so the marker should reappear in the assistant
            // reply.
            match ws_send_recv_until(&mut ws, &user_payload, 180, |resp| resp.contains(&marker))
                .await
            {
                Ok(_resp) => {
                    // Now scan Node-A's session_logs directory.
                    let session_logs_dir = ws_a
                        .home()
                        .join("workspace")
                        .join("logs")
                        .join("session_logs");

                    // Give the filesystem a moment to flush, then scan.
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    let mut matching_files: Vec<String> = Vec::new();
                    let mut user_seen = false;
                    let mut assistant_seen = false;
                    let mut sample_line = String::new();

                    let entries = match std::fs::read_dir(&session_logs_dir) {
                        Ok(e) => e,
                        Err(e) => {
                            return fail(
                                "T14",
                                format!(
                                    "session_logs dir not readable at {}: {}",
                                    session_logs_dir.display(),
                                    e
                                ),
                            );
                        }
                    };
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                            continue;
                        }
                        let content = match std::fs::read_to_string(&path) {
                            Ok(c) => c,
                            Err(_) => continue,
                        };
                        if !content.contains(&marker) {
                            continue;
                        }
                        matching_files.push(path.display().to_string());
                        for line in content.lines() {
                            if !line.contains(&marker) {
                                continue;
                            }
                            let is_user = line.contains(r#""role":"user""#)
                                || line.contains(r#""role": "user""#);
                            let is_assistant = line.contains(r#""role":"assistant""#)
                                || line.contains(r#""role": "assistant""#);
                            if is_user {
                                user_seen = true;
                            }
                            if is_assistant {
                                assistant_seen = true;
                                sample_line = line.to_string();
                            }
                        }
                    }

                    if matching_files.is_empty() {
                        return fail(
                            "T14",
                            format!(
                                "no session_log file in {} contains marker {}; \
                                 regression: continuation reply not persisted",
                                session_logs_dir.display(),
                                marker
                            ),
                        );
                    }
                    if !user_seen {
                        return fail(
                            "T14",
                            format!(
                                "marker found in {:?} but no user row — \
                                 unexpected; user message should always be logged",
                                matching_files
                            ),
                        );
                    }
                    if !assistant_seen {
                        return fail(
                            "T14",
                            format!(
                                "REGRESSION: marker found in {:?} with user row \
                                 but NO assistant row — handle_cluster_continuation \
                                 is skipping session_log writes",
                                matching_files
                            ),
                        );
                    }
                    pass(
                        "T14",
                        format!(
                            "continuation reply persisted: files={:?}, sample assistant line: {}",
                            matching_files,
                            trunc(&sample_line, 120)
                        ),
                    )
                }
                Err(e) => fail("T14", format!("180s 内未收到续行响应: {}", e)),
            }
        })
        .await,
    );

    // T15: board dispatch full chain (W2 P2)
    //
    // Coordinator Node-A creates a board issue and dispatches it to worker
    // Node-B via `board issue.dispatch` (peer_chat RPC, task_id ↔ issue
    // binding in issue_dispatch). B's agent (TestAIServer echo model)
    // processes the prompt and reports back through peer_chat_callback; A's
    // gateway writes the result back to the board:
    //   - comment "✅ worker 汇报完成：..." authored by agent Node-B
    //   - issue transitions in_progress → in_review (awaiting acceptance)
    //
    // Failure modes this catches: dispatch validation wiring, RPC delivery,
    // callback routing (task_id → issue_dispatch), and the writeback itself.
    all_results.push(
        run_test("T15: board dispatch A→B full chain", || async {
            let mut ws = match ws_connect_gateway(NODES[0].web_port).await {
                Ok(s) => s,
                Err(e) => return fail("T15", format!("WS connect to A failed: {}", e)),
            };

            // 1. Create the issue on the coordinator (letters-only marker —
            //    digit-heavy strings trip the DLP credit_card rule, see T14).
            let marker = "T15BOARDDISPATCHMARKER";
            let created = match ws_api_request(
                &mut ws,
                "board",
                "issue.create",
                json!({
                    "title": format!("{} board dispatch e2e", marker),
                    "description": "Dispatched by cluster-uat T15 to worker Node-B.",
                    "acceptance_criteria": "Worker replies with a completion report.",
                }),
                15,
            )
            .await
            {
                Ok(v) => v,
                Err(e) => return fail("T15", format!("issue.create failed: {}", e)),
            };
            let issue_id = created
                .pointer("/issue/id")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            if issue_id == 0 {
                return fail(
                    "T15",
                    format!("issue.create returned no issue id: {}", created),
                );
            }

            // 2. Dispatch to worker Node-B → issue must land in in_progress.
            let disp = match ws_api_request(
                &mut ws,
                "board",
                "issue.dispatch",
                json!({ "id": issue_id, "target": "Node-B" }),
                30,
            )
            .await
            {
                Ok(v) => v,
                Err(e) => return fail("T15", format!("issue.dispatch failed: {}", e)),
            };
            let task_id = disp
                .get("task_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let status_now = disp
                .pointer("/issue/status")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if disp.get("dispatched").and_then(|v| v.as_bool()) != Some(true) || task_id.is_empty()
            {
                return fail(
                    "T15",
                    format!("issue.dispatch unexpected response: {}", disp),
                );
            }
            if status_now != "in_progress" {
                return fail(
                    "T15",
                    format!(
                        "issue not in_progress after dispatch (got '{}')",
                        status_now
                    ),
                );
            }
            println!(
                "\n         T15 dispatched (issue_id={}, task_id={})",
                issue_id, task_id
            );

            // 3. Poll until the callback writeback moves the issue to
            //    in_review. The worker chain is: RPC ACK → agent LLM
            //    (TestAIServer echo) → peer_chat_callback → writeback.
            let deadline = tokio::time::Instant::now() + Duration::from_secs(240);
            let mut last_status = String::from(status_now);
            loop {
                if tokio::time::Instant::now() >= deadline {
                    return fail(
                        "T15",
                        format!(
                            "240s 内 issue 未到 in_review（最后状态='{}', task_id={}）\
                             —— callback 写回链路未走通",
                            last_status, task_id
                        ),
                    );
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
                let got = match ws_api_request(
                    &mut ws,
                    "board",
                    "issue.get",
                    json!({ "id": issue_id }),
                    10,
                )
                .await
                {
                    Ok(v) => v,
                    Err(e) => return fail("T15", format!("issue.get failed: {}", e)),
                };
                last_status = got
                    .pointer("/issue/status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if last_status == "in_review" {
                    break;
                }
            }

            // 4. The worker's completion report must be on the issue as a
            //    comment authored by agent Node-B with the fixed success
            //    prefix (gateway write_back_board_dispatch), and it must
            //    contain the issue marker — testai-3.1 echoes the prompt
            //    verbatim, so the marker proves the report is the worker's
            //    actual reply to *this* issue (end-to-end data flow).
            let comments = match ws_api_request(
                &mut ws,
                "board",
                "comment.list",
                json!({ "issue_id": issue_id }),
                10,
            )
            .await
            {
                Ok(v) => v,
                Err(e) => return fail("T15", format!("comment.list failed: {}", e)),
            };
            let mut found: Option<String> = None;
            if let Some(arr) = comments.get("comments").and_then(|c| c.as_array()) {
                for c in arr {
                    let kind = c
                        .pointer("/author/kind")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let id = c
                        .pointer("/author/id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let content = c.get("content").and_then(|v| v.as_str()).unwrap_or("");
                    if kind == "agent"
                        && id == "Node-B"
                        && content.starts_with("✅ worker 汇报完成")
                        && content.contains(marker)
                    {
                        found = Some(content.to_string());
                    }
                }
            }
            match found {
                Some(content) => pass(
                    "T15",
                    format!(
                        "board dispatch 全链路 OK：issue {} in_progress→in_review，\
                         worker 回报已写回（{}）",
                        issue_id,
                        trunc(&content, 120)
                    ),
                ),
                None => fail(
                    "T15",
                    format!(
                        "issue 到达 in_review 但未找到 agent Node-B 的完成评论;\
                         comments={}",
                        comments
                    ),
                ),
            }
        })
        .await,
    );

    // T16: autopilot 定时触发（开发计划 §6-T5，W2 P4 autopilot）。
    //
    // 建每分钟规则（target=Node-B）→ 等 cron 到点 → fire_autopilot 模板建单
    // + 派发 → autopilot.runs 出现带 marker 的 issue 且状态 in_progress。
    // 等 cron 最坏 ~70s（分钟边界），轮询上限 120s。
    // 本测试先于 T17 cancel 跑（T17 会杀掉 Node-B 做竞态控制）。
    all_results.push(
        run_test("T16: autopilot cron 触发建单+派发", || async {
            let mut ws = match ws_connect_gateway(NODES[0].web_port).await {
                Ok(s) => s,
                Err(e) => return fail("T16", format!("WS connect to A failed: {}", e)),
            };

            let marker = "T16AUTOPILOTMARKER";
            let created = match ws_api_request(
                &mut ws,
                "board",
                "autopilot.create",
                json!({
                    "name": "uat-autopilot-t16",
                    "cron": "* * * * *",
                    "title": format!("{} 定时站会 {{date}}", marker),
                    "description": "cluster-uat T16 每分钟规则",
                    "target": "Node-B",
                }),
                15,
            )
            .await
            {
                Ok(v) => v,
                Err(e) => return fail("T16", format!("autopilot.create failed: {}", e)),
            };
            let ap_id = created
                .pointer("/autopilot/id")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            if ap_id == 0 {
                return fail(
                    "T16",
                    format!("autopilot.create returned no id: {}", created),
                );
            }
            // WSAPI create 即时挂载 cron_job。
            if created.pointer("/autopilot/cron_job_id").is_none() {
                return fail(
                    "T16",
                    format!("autopilot not armed (no cron_job_id): {}", created),
                );
            }

            // 等 cron 到点：轮询 runs 直到带 marker 的 issue 出现且 in_progress。
            let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
            let mut found: Option<(i64, String, String)> = None;
            while tokio::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_secs(5)).await;
                let runs = match ws_api_request(
                    &mut ws,
                    "board",
                    "autopilot.runs",
                    json!({ "id": ap_id }),
                    10,
                )
                .await
                {
                    Ok(v) => v,
                    Err(e) => return fail("T16", format!("autopilot.runs failed: {}", e)),
                };
                if let Some(arr) = runs.get("issues").and_then(|i| i.as_array()) {
                    for iss in arr {
                        let title = iss.get("title").and_then(|t| t.as_str()).unwrap_or("");
                        if !title.contains(marker) {
                            continue;
                        }
                        let status = iss.get("status").and_then(|s| s.as_str()).unwrap_or("");
                        let id = iss.get("id").and_then(|i| i.as_i64()).unwrap_or(0);
                        // in_progress 证明派发已落；若 worker 在两次轮询之间
                        // 已跑完（echo 链路快），in_review/done 同样是派发发生
                        // 的证据（backlog/todo 才是只建单未派发）。
                        if matches!(status, "in_progress" | "in_review" | "done") {
                            found = Some((id, title.to_string(), status.to_string()));
                            break;
                        }
                    }
                }
                if found.is_some() {
                    break;
                }
            }
            // 清理：删规则（防后续轮次重复触发建单）。
            let _ = ws_api_request(
                &mut ws,
                "board",
                "autopilot.remove",
                json!({ "id": ap_id }),
                10,
            )
            .await;
            match found {
                Some((id, title, status)) => pass(
                    "T16",
                    format!(
                        "autopilot 定时触发 OK：规则 {} cron 到点建单 issue {}（状态 {}，{}）",
                        ap_id,
                        id,
                        status,
                        trunc(&title, 50)
                    ),
                ),
                None => fail(
                    "T16",
                    format!(
                        "120s 内 cron 未触发建单+派发（rule {}）——检查 gateway on_job 挂载",
                        ap_id
                    ),
                ),
            }
        })
        .await,
    );

    // T17: board cancel 下行（开发计划 §6-T4，W2 P4 per-task cancel）。
    //
    // 本地 echo 链路（testai-3.1）<2s 跑完全链——cancel 与写回是真实竞态，
    // kill-after-ACK 追不上（run2 实测两连败"该 issue 没有进行中的派发"：
    // 写回先落账，dispatch 已终结）。确定性方案：把 B 的默认模型切成
    // testai-1.2（固定 30s 延迟）并重启 B → worker ACK 后挂在 LLM 上 30s，
    // cancel（~+1s）必赢竞态：
    //   cancel_dispatch（竞态守卫）→ issue → cancelled（终态）
    //   → fire-and-forget task_cancel 送达活着的 B → worker 被取消，无写回。
    // 之后等过 sweep 截止线（15s 超时 + 2s 间隔）：cancelled 记录不得被
    // sweep 误标（无"派发超时"评论）。送达失败 ⛔ 评论路径由单测覆盖，
    // 本 e2e 送达成功（B 活着），不断言。
    // 注：T17 后 B 保持 testai-1.2（套件后续无测试使用 B 的 LLM）。
    all_results.push(
        run_test("T17: board cancel 下行 (issue.cancel)", || async {
            // 0. B 切慢模型 + 重启（拿确定性的 30s LLM 窗口）。
            let out = ws_b
                .run_cli(
                    &gateway_bin,
                    &[
                        "model",
                        "add",
                        "--model",
                        "test/testai-1.2",
                        "--base",
                        &format!("http://127.0.0.1:{}/v1", AI_SERVER_PORT),
                        "--key",
                        "test-key",
                        "--default",
                    ],
                )
                .await;
            if !out.success() {
                return fail("T17", format!("B model switch failed: {}", out.stderr));
            }
            gw_b.kill().await;
            gw_b = match start_gateway_and_wait("Gateway-B", &gateway_bin, ws_b.path(), &NODES[1])
                .await
            {
                Ok(g) => g,
                Err(e) => return fail("T17", e),
            };

            let mut ws = match ws_connect_gateway(NODES[0].web_port).await {
                Ok(s) => s,
                Err(e) => return fail("T17", format!("WS connect to A failed: {}", e)),
            };

            // 1. Create + dispatch to Node-B（ACK 即回，worker 随后挂 30s LLM）。
            let created = match ws_api_request(
                &mut ws,
                "board",
                "issue.create",
                json!({
                    "title": "T17CANCELMARKER cancel e2e",
                    "description": "Dispatched then cancelled by cluster-uat T17.",
                }),
                15,
            )
            .await
            {
                Ok(v) => v,
                Err(e) => return fail("T17", format!("issue.create failed: {}", e)),
            };
            let issue_id = created
                .pointer("/issue/id")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            if issue_id == 0 {
                return fail("T17", format!("issue.create returned no id: {}", created));
            }
            let disp = match ws_api_request(
                &mut ws,
                "board",
                "issue.dispatch",
                json!({ "id": issue_id, "target": "Node-B" }),
                30,
            )
            .await
            {
                Ok(v) => v,
                Err(e) => return fail("T17", format!("issue.dispatch failed: {}", e)),
            };
            if disp.get("dispatched").and_then(|v| v.as_bool()) != Some(true) {
                return fail("T17", format!("issue.dispatch unexpected: {}", disp));
            }

            // 2. Cancel：worker 挂在 30s LLM 上，dispatch 仍 active，必赢竞态。
            let cancel = match ws_api_request(
                &mut ws,
                "board",
                "issue.cancel",
                json!({ "id": issue_id }),
                15,
            )
            .await
            {
                Ok(v) => v,
                Err(e) => return fail("T17", format!("issue.cancel failed: {}", e)),
            };
            if cancel.get("cancelled").and_then(|v| v.as_bool()) != Some(true) {
                return fail("T17", format!("issue.cancel unexpected: {}", cancel));
            }
            let status = cancel
                .pointer("/issue/status")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if status != "cancelled" {
                return fail(
                    "T17",
                    format!("issue not cancelled after cancel (got '{}')", status),
                );
            }

            // 3. 重复取消被竞态守卫诚实拒绝（dispatch 已终结）。
            match ws_api_request(
                &mut ws,
                "board",
                "issue.cancel",
                json!({ "id": issue_id }),
                15,
            )
            .await
            {
                Ok(_) => return fail("T17", "second cancel unexpectedly succeeded"),
                Err(e) => {
                    let msg = e.to_string();
                    if !(msg.contains("没有进行中的派发") || msg.contains("派发已终结"))
                    {
                        return fail("T17", format!("second cancel wrong error: {}", msg));
                    }
                }
            }

            // 4. 等过 sweep 截止线（15s 超时 + 2s 间隔 → 25s 轮询覆盖）：
            //    cancelled 记录不被 sweep 误伤（无"派发超时"评论）。
            let deadline = tokio::time::Instant::now() + Duration::from_secs(25);
            while tokio::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_secs(3)).await;
                let comments = match ws_api_request(
                    &mut ws,
                    "board",
                    "comment.list",
                    json!({ "issue_id": issue_id }),
                    10,
                )
                .await
                {
                    Ok(v) => v,
                    Err(e) => return fail("T17", format!("comment.list failed: {}", e)),
                };
                if let Some(arr) = comments.get("comments").and_then(|c| c.as_array()) {
                    for c in arr {
                        let content = c.get("content").and_then(|v| v.as_str()).unwrap_or("");
                        if content.contains("派发超时") {
                            return fail(
                                "T17",
                                format!("cancelled 记录被 sweep 误标失败：{}", content),
                            );
                        }
                    }
                }
            }
            pass(
                "T17",
                format!(
                    "cancel 下行 OK：issue {} → cancelled；重复取消被竞态守卫拒绝；\
                     过 sweep 截止线无派发超时误标",
                    issue_id
                ),
            )
        })
        .await,
    );

    // T18: worker 离线/超时 → sweep 标失败（开发计划 §6-T3，W2 P4 鲁棒性）。
    //
    // 与 T17 同一确定性前提：echo 链路（testai-3.1）<2s 完成，写回会赶在
    // kill 前落账（run2 实测 sweep 无账可查）。把 D 的默认模型切成
    // testai-1.2（固定 30s 延迟）并重启 → 派发 ACK 后 kill（+2s）落在
    // LLM 中段：写回永不抵达 → Node-A 的 dispatch sweep（config board:
    // 15s 超时 / 2s 间隔，见 configure_board_sweep）在 ~17s 内把 dispatch
    // 记录标 failed：issue 上出现 ⛔ system 评论，admin 收到 dispatch_failed
    // 通知。注意：sweep 是 MVP 策略（abort+notify，不自动 retry），issue
    // 状态保持在 in_progress 由人工重派/取消。
    // 本测试最后跑；B 已在 T17 切慢模型（无影响，T18 不用 B 的 LLM）。
    all_results.push(
        run_test("T18: worker 离线 → sweep 标失败", || async {
            // 0. D 切慢模型 + 重启（写回赶在 kill 前落账的竞态由此消除）。
            let out = ws_d
                .run_cli(
                    &gateway_bin,
                    &[
                        "model",
                        "add",
                        "--model",
                        "test/testai-1.2",
                        "--base",
                        &format!("http://127.0.0.1:{}/v1", AI_SERVER_PORT),
                        "--key",
                        "test-key",
                        "--default",
                    ],
                )
                .await;
            if !out.success() {
                return fail("T18", format!("D model switch failed: {}", out.stderr));
            }
            gw_d.kill().await;
            gw_d = match start_gateway_and_wait("Gateway-D", &gateway_bin, ws_d.path(), &NODES[3])
                .await
            {
                Ok(g) => g,
                Err(e) => return fail("T18", e),
            };

            let mut ws = match ws_connect_gateway(NODES[0].web_port).await {
                Ok(s) => s,
                Err(e) => return fail("T18", format!("WS connect to A failed: {}", e)),
            };

            // 1. Create + dispatch to Node-B.
            let created = match ws_api_request(
                &mut ws,
                "board",
                "issue.create",
                json!({
                    "title": "T18OFFLINEMARKER offline sweep e2e",
                    "description": "Dispatched then worker killed by cluster-uat T18.",
                }),
                15,
            )
            .await
            {
                Ok(v) => v,
                Err(e) => return fail("T18", format!("issue.create failed: {}", e)),
            };
            let issue_id = created
                .pointer("/issue/id")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            if issue_id == 0 {
                return fail("T18", format!("issue.create returned no id: {}", created));
            }
            let disp = match ws_api_request(
                &mut ws,
                "board",
                "issue.dispatch",
                json!({ "id": issue_id, "target": "Node-D" }),
                30,
            )
            .await
            {
                Ok(v) => v,
                Err(e) => return fail("T18", format!("issue.dispatch failed: {}", e)),
            };
            if disp.get("dispatched").and_then(|v| v.as_bool()) != Some(true) {
                return fail("T18", format!("issue.dispatch unexpected: {}", disp));
            }

            // 2. 给 worker 2s 进入执行，然后杀掉 Gateway-D（写回永不抵达）。
            tokio::time::sleep(Duration::from_secs(2)).await;
            gw_d.kill().await;

            // 3. 轮询 issue 评论，等 ⛔ system 评论（15s 超时 + 2s sweep ≤ ~17s，
            //    留 60s 余量给轮询抖动）。
            let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
            let mut sweep_comment: Option<String> = None;
            while tokio::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_secs(3)).await;
                let comments = match ws_api_request(
                    &mut ws,
                    "board",
                    "comment.list",
                    json!({ "issue_id": issue_id }),
                    10,
                )
                .await
                {
                    Ok(v) => v,
                    Err(e) => return fail("T18", format!("comment.list failed: {}", e)),
                };
                if let Some(arr) = comments.get("comments").and_then(|c| c.as_array()) {
                    for c in arr {
                        let kind = c.pointer("/author/kind").and_then(|v| v.as_str()).unwrap_or("");
                        let id = c.pointer("/author/id").and_then(|v| v.as_str()).unwrap_or("");
                        let content = c.get("content").and_then(|v| v.as_str()).unwrap_or("");
                        if kind == "system" && id == "board" && content.starts_with('⛔') {
                            sweep_comment = Some(content.to_string());
                            break;
                        }
                    }
                }
                if sweep_comment.is_some() {
                    break;
                }
            }
            let Some(comment) = sweep_comment else {
                return fail(
                    "T18",
                    format!("60s 内 sweep 未标失败（issue {} 无 ⛔ system 评论）——检查 board sweep 配置/挂载", issue_id),
                );
            };

            // 4. dispatch_failed 通知到达 admin 收件箱。
            let inbox = match ws_api_request(&mut ws, "board", "inbox.list", json!({}), 10).await {
                Ok(v) => v,
                Err(e) => return fail("T18", format!("inbox.list failed: {}", e)),
            };
            let notified = inbox
                .get("notifications")
                .and_then(|n| n.as_array())
                .map(|arr| {
                    arr.iter().any(|n| {
                        n.get("kind").and_then(|k| k.as_str()) == Some("dispatch_failed")
                            && n.get("issue_id").and_then(|i| i.as_i64()) == Some(issue_id)
                    })
                })
                .unwrap_or(false);
            if !notified {
                return fail(
                    "T18",
                    format!("sweep 已标失败但 admin 未收到 dispatch_failed 通知：{}", inbox),
                );
            }

            // 5. MVP 语义：issue 停在 in_progress（不自动 retry/转移）。
            let got = match ws_api_request(&mut ws, "board", "issue.get", json!({ "id": issue_id }), 10).await {
                Ok(v) => v,
                Err(e) => return fail("T18", format!("issue.get failed: {}", e)),
            };
            let status = got
                .pointer("/issue/status")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if status != "in_progress" {
                return fail(
                    "T18",
                    format!("sweep 后 issue 状态应为 in_progress（MVP 不自动转移），got '{}'", status),
                );
            }
            pass(
                "T18",
                format!("离线 sweep OK：{}（issue {} 保持 in_progress 待人工处置，通知已达）", trunc(&comment, 80), issue_id),
            )
        })
        .await,
    );

    // ==================================================================
    // Cleanup
    // ==================================================================
    println!("\n--- Cleanup ---");
    gw_d.kill().await;
    gw_c.kill().await;
    gw_b.kill().await;
    gw_a.kill().await;
    ai_server.kill().await;

    // Save gateway logs to a persistent directory before temp dirs are cleaned up
    let log_output_dir = std::path::PathBuf::from("cluster-uat-logs");
    std::fs::create_dir_all(&log_output_dir).ok();
    for (gw, ws, name) in [
        (&gw_a, &ws_a, "Node-A"),
        (&gw_b, &ws_b, "Node-B"),
        (&gw_c, &ws_c, "Node-C"),
        (&gw_d, &ws_d, "Node-D"),
    ] {
        let src = gw.log_path.clone();
        let dst = log_output_dir.join(format!("{}.log", name));
        if src.exists() {
            match std::fs::copy(&src, &dst) {
                Ok(_) => println!("  Saved {} log to {}", name, dst.display()),
                Err(e) => println!("  Failed to save {} log: {}", name, e),
            }
        } else {
            println!("  {} log not found at {}", name, src.display());
        }
        // Also copy state.toml, peers.toml and config.cluster.json
        let state_src = ws
            .home()
            .join("workspace")
            .join("cluster")
            .join("state.toml");
        let state_dst = log_output_dir.join(format!("{}-state.toml", name));
        if state_src.exists() {
            std::fs::copy(&state_src, &state_dst).ok();
        }
        let peers_src = ws
            .home()
            .join("workspace")
            .join("cluster")
            .join("peers.toml");
        let peers_dst = log_output_dir.join(format!("{}-peers.toml", name));
        if peers_src.exists() {
            std::fs::copy(&peers_src, &peers_dst).ok();
        }
        let cluster_cfg_src = ws
            .home()
            .join("workspace")
            .join("config")
            .join("config.cluster.json");
        let cluster_cfg_dst = log_output_dir.join(format!("{}-config.cluster.json", name));
        if cluster_cfg_src.exists() {
            std::fs::copy(&cluster_cfg_src, &cluster_cfg_dst).ok();
        }
    }
    println!(
        "  Logs saved to: {}",
        std::fs::canonicalize(&log_output_dir)
            .unwrap_or_else(|_| log_output_dir.clone())
            .display()
    );

    // Final port cleanup
    cleanup_ports(&all_ports);

    // ==================================================================
    // Results
    // ==================================================================
    println!("\n========================================");
    println!("  Cluster UAT Results");
    println!("========================================");
    let all_passed = print_results(&all_results);

    std::process::exit(if all_passed { 0 } else { 1 });
}
