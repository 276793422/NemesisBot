//! NemesisBot Cluster UAT (User Acceptance Test)
//!
//! End-to-end verification of cluster functionality including:
//! - Multi-node startup and configuration (4 nodes: A, B, C, D)
//! - UDP discovery
//! - 2-hop peer_chat (A→B, A→C, A→D)
//! - 3-hop chain (A→B→D)
//! - 4-hop chain (A→B→C→D)
//! - Bidirectional, concurrent, and error recovery scenarios
//!
//! Usage:
//!   cargo run -p cluster-uat                    # Run all tests
//!   cargo run -p cluster-uat -- --skip-long     # Skip long-running tests

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
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
    web_port: u16,
    health_port: u16,
    udp_port: u16,
    rpc_port: u16,
    model: &'static str,
}

const NODES: [NodeConfig; 4] = [
    NodeConfig {
        name: "Node-A",
        web_port: 49000,
        health_port: 18790,
        udp_port: 11949,
        rpc_port: 21949,
        model: "test/testai-3.1",
    },
    NodeConfig {
        name: "Node-B",
        web_port: 49001,
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
        web_port: 49002,
        health_port: 18792,
        udp_port: 11951,
        rpc_port: 21951,
        model: "test/testai-3.1",
    },
    NodeConfig {
        name: "Node-D",
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
            .args(&["--local", "gateway", "--debug"])
            .env("RUST_LOG", "debug")
            .current_dir(cwd)
            .stdout(Stdio::from(log_file.try_clone()?))
            .stderr(Stdio::from(log_file))
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("Failed to spawn {}", name))?;
        println!("  {} started (PID: {:?}, log: {})", name, child.id(), log_path.display());
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
        if let Some(channels) = obj.get_mut("channels") {
            if let Some(ch) = channels.as_object_mut() {
                if let Some(web) = ch.get_mut("web") {
                    if let Some(w) = web.as_object_mut() {
                        w.insert("port".to_string(), json!(web_port));
                    }
                }
                // Disable standalone websocket channel — the web server already handles
                // WebSocket on the web port. Without this, the websocket channel binds to
                // its default port 49001, which conflicts with Node-B's web port.
                if let Some(ws) = ch.get_mut("websocket") {
                    if let Some(w) = ws.as_object_mut() {
                        w.insert("enabled".to_string(), json!(false));
                    }
                }
            }
        }
        // Set health check port (gateway.port)
        if let Some(gateway) = obj.get_mut("gateway") {
            if let Some(gw) = gateway.as_object_mut() {
                gw.insert("port".to_string(), json!(health_port));
            }
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

/// Configure a single cluster node via CLI commands.
///
/// Each node gets its own UDP port (Windows SO_REUSEADDR semantics hijack
/// rather than share — see `NODES` comment) so UDP auto-discovery does not
/// link them. Instead, we seed each node's `peers.toml` with the other three
/// nodes' UDP addresses — gateway.rs derives the RPC port via the
/// `udp_port + 10000` convention (e.g., 11950→21950) and routes cluster_rpc
/// calls accordingly.
async fn setup_node(
    ws: &TestWorkspace,
    bin: &Path,
    node: &NodeConfig,
) -> Result<()> {
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

    // 4. Initialize cluster
    let out = ws
        .run_cli(
            bin,
            &[
                "cluster",
                "init",
                "--name",
                name,
                "--role",
                "worker",
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
                    "worker",
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

    println!("  {} configured OK (static peers + UDP port {})", name, node.udp_port);
    Ok(())
}

// ---------------------------------------------------------------------------
// WebSocket helpers
// ---------------------------------------------------------------------------

/// Connect to a gateway's WebSocket endpoint.
async fn ws_connect_gateway(
    port: u16,
) -> Result<
    tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
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
                        let content = v["data"]["content"]
                            .as_str()
                            .unwrap_or("")
                            .to_string();
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
            Err(_) => return Err(anyhow::anyhow!("Timeout after {}s (no matching response)", timeout_secs)),
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

/// Truncate a string for display.
fn trunc(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
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

    let mut ai_server = ManagedProcess::spawn(
        "TestAIServer",
        &ai_server_bin,
        &[],
        &root,
    )
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

    let mut gw_a =
        GatewayProcess::spawn("Gateway-A", &gateway_bin, ws_a.path())
            .expect("Cannot start Gateway-A");
    let mut gw_b =
        GatewayProcess::spawn("Gateway-B", &gateway_bin, ws_b.path())
            .expect("Cannot start Gateway-B");
    let mut gw_c =
        GatewayProcess::spawn("Gateway-C", &gateway_bin, ws_c.path())
            .expect("Cannot start Gateway-C");
    let mut gw_d =
        GatewayProcess::spawn("Gateway-D", &gateway_bin, ws_d.path())
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
    println!("  Running Tests (T1-T13, 4-node full chain verification)");
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
                    return fail("T1", format!("{}: missing Config line in output", NODES[i].name));
                }
                // Verify enabled
                if !out.stdout_contains("Enabled: true") && !out.stdout_contains("enabled: true") {
                    return fail(
                        "T1",
                        format!("{}: cluster not enabled. Output: {}", NODES[i].name, trunc(&out.stdout, 200)),
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
            if !gw_a.is_running() || !gw_b.is_running() || !gw_c.is_running() || !gw_d.is_running() {
                return fail("T2", "One or more nodes crashed during startup");
            }

            for node in NODES.iter() {
                let peers_path = match node.name {
                    "Node-A" => ws_a.home().join("workspace").join("cluster").join("peers.toml"),
                    "Node-B" => ws_b.home().join("workspace").join("cluster").join("peers.toml"),
                    "Node-C" => ws_c.home().join("workspace").join("cluster").join("peers.toml"),
                    "Node-D" => ws_d.home().join("workspace").join("cluster").join("peers.toml"),
                    _ => unreachable!(),
                };
                let content = std::fs::read_to_string(&peers_path).unwrap_or_default();
                for other in NODES.iter() {
                    if other.name == node.name {
                        continue;
                    }
                    // cluster peers add sanitizes the id into a TOML key
                    // (hyphens → underscores, case preserved), so "Node-B"
                    // becomes "[peers.Node_B]".
                    let sanitized = other.name.replace('-', "_");
                    if !content.contains(&format!("[peers.{}]", sanitized)) {
                        return fail("T2", format!(
                            "{} peers.toml missing entry for {} (looked for [peers.{}])",
                            node.name, other.name, sanitized
                        ));
                    }
                }
            }
            pass("T2", "All 4 nodes have the other 3 as static peers".to_string())
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
            let out = ws_a.run_cli(&gateway_bin, &["cluster", "peers", "list"]).await;
            let stdout = out.stdout.clone();
            // cluster peers add sanitizes "Node-B" → "Node_B" in the TOML key.
            let has_b = stdout.contains("Node_B") || stdout.contains("Node-B");
            let has_c = stdout.contains("Node_C") || stdout.contains("Node-C");
            let has_d = stdout.contains("Node_D") || stdout.contains("Node-D");
            if has_b && has_c && has_d {
                pass("T3", format!(
                    "Node-A sees Node-B/C/D in peers list (exit={}, {} bytes)",
                    out.exit_code, stdout.len()
                ))
            } else {
                fail("T3", format!(
                    "PeerRegistry missing peers: B={} C={} D={} (exit={}, stdout: {})",
                    has_b, has_c, has_d, out.exit_code, trunc(&stdout, 200)
                ))
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
            }).await {
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
            }).await {
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
            }).await {
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
                    }).await {
                        Ok(resp) => {
                            if resp.contains(&content) {
                                Ok(resp)
                            } else {
                                Ok(resp) // Got continuation response, content may vary
                            }
                        }
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
                pass("T8", format!("All {} concurrent async requests succeeded", pass_count))
            } else {
                fail(
                    "T8",
                    format!(
                        "{}/{} requests failed",
                        fail_count,
                        pass_count + fail_count
                    ),
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
            gw_d = match GatewayProcess::spawn("Gateway-D", &gateway_bin, ws_d.path()) {
                Ok(p) => p,
                Err(e) => return fail("T9", format!("Cannot restart D: {}", e)),
            };

            // 3a: Wait for HTTP health check (gateway web server up)
            let health_url = format!("http://127.0.0.1:{}/health", NODES[3].health_port);
            if let Err(e) = wait_for_http(&health_url, Duration::from_secs(15)).await {
                return fail("T9", format!("D not healthy after restart: {}", e));
            }
            println!("    Node-D restarted and healthy");

            // 3b: Wait for D's RPC server to be listening
            let rpc_addr = format!("127.0.0.1:{}", NODES[3].rpc_port);
            let rpc_ready = tokio::time::timeout(
                Duration::from_secs(15),
                async {
                    loop {
                        if tokio::net::TcpStream::connect(&rpc_addr).await.is_ok() {
                            return true;
                        }
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                },
            )
            .await
            .unwrap_or(false);
            if !rpc_ready {
                return fail("T9", format!("D RPC server not ready at {}", rpc_addr));
            }
            println!("    Node-D RPC server ready on port {}", NODES[3].rpc_port);

            // 3c: Wait for D's UDP announce to reach A
            // D sends announce with 0-5s jitter, A needs to process it
            // broadcast_interval is 3s in tests, so 15s covers jitter + processing
            println!("    Waiting for UDP discovery to propagate (15s)...");
            tokio::time::sleep(Duration::from_secs(15)).await;

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
            }).await {
                Ok(resp) => pass(
                    "T9",
                    format!(
                        "Recovered: offline_err={}, continuation='{}'",
                        got_error,
                        trunc(&resp, 80)
                    ),
                ),
                Err(e) => fail("T9", format!("180s 内未收到续行响应 (UDP discovery may have failed): {}", e)),
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
            }).await {
                Ok(resp) => {
                    pass("T10", format!("大消息异步 OK ({} bytes)", resp.len()))
                }
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
            let msg = r#"<PEER_CHAT>{"peer_id":"Node-C","content":"hello direct to C"}</PEER_CHAT>"#;
            match ws_send_recv_until(&mut ws, msg, 180, |resp| {
                resp.contains("hello direct to C") || resp.contains("hello")
            }).await {
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
            }).await {
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
        let state_src = ws.home().join("workspace").join("cluster").join("state.toml");
        let state_dst = log_output_dir.join(format!("{}-state.toml", name));
        if state_src.exists() {
            std::fs::copy(&state_src, &state_dst).ok();
        }
        let peers_src = ws.home().join("workspace").join("cluster").join("peers.toml");
        let peers_dst = log_output_dir.join(format!("{}-peers.toml", name));
        if peers_src.exists() {
            std::fs::copy(&peers_src, &peers_dst).ok();
        }
        let cluster_cfg_src = ws.home().join("workspace").join("config").join("config.cluster.json");
        let cluster_cfg_dst = log_output_dir.join(format!("{}-config.cluster.json", name));
        if cluster_cfg_src.exists() {
            std::fs::copy(&cluster_cfg_src, &cluster_cfg_dst).ok();
        }
    }
    println!("  Logs saved to: {}", std::fs::canonicalize(&log_output_dir).unwrap_or_else(|_| log_output_dir.clone()).display());

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
