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

/// TestAIServer 监听端口。默认 8080；被外部程序占用时用 TESTAI_PORT 环境变量
/// 整体改道（与 test-harness::ai_server_port 同一语义，本 runner 独立装配）。
fn ai_server_port() -> u16 {
    std::env::var("TESTAI_PORT")
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(8080)
}
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
        Self::spawn_with_env(name, bin, cwd, &[])
    }

    /// Spawn with extra process env vars（T-XFER 用 `NEMESISBOT_TRANSFER_CHUNK_BYTES`
    /// 压小块大小制造多块传输/续传窗口；空切片 = 与 spawn 完全同语义）。
    fn spawn_with_env(
        name: &'static str,
        bin: &Path,
        cwd: &Path,
        envs: &[(&str, &str)],
    ) -> Result<Self> {
        println!("  Starting {}...", name);
        // Redirect stderr to a log file for debugging.
        // 追加而非截断（2026-09-18 T26 排查教训）：File::create 语义下，
        // 套件中途任一节点重启都会把前一窗口的日志永久抹掉——B 的 T26
        // 窗口日志被 T-XFER 系列截断，只能靠外部快照抢救。追加后每段
        // 有 "Starting ..." 分界行 + tracing 时间戳，混窗仍可读。
        let log_path = cwd.join("gateway.log");
        let log_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .with_context(|| format!("Cannot open log file for {}", name))?;
        let mut cmd = tokio::process::Command::new(bin);
        cmd.args(["--local", "gateway", "--debug"])
            .env("RUST_LOG", "debug")
            .current_dir(cwd)
            .stdout(Stdio::from(log_file.try_clone()?))
            .stderr(Stdio::from(log_file))
            .kill_on_drop(true);
        for (k, v) in envs {
            cmd.env(k, v);
        }
        let child = cmd
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

/// Print the last `max_lines` lines of a gateway log to stdout (CI evidence:
/// stdout/stderr 都重定向在这个文件里，健康检查失败时倾倒尾部，让 CI 日志
/// 自带「为什么没起来」的直接证据，而不是无诊断价值的纯超时）。
fn dump_log_tail(path: &Path, max_lines: usize) {
    println!("--- gateway log tail: {} ---", path.display());
    match std::fs::read_to_string(path) {
        Ok(content) => {
            let lines: Vec<&str> = content.lines().collect();
            let skip = lines.len().saturating_sub(max_lines);
            if skip > 0 {
                println!("  (... {} earlier lines omitted ...)", skip);
            }
            for line in &lines[skip..] {
                println!("  {}", line);
            }
        }
        Err(e) => println!("  (cannot read log: {})", e),
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
        // Enable DEBUG level logging for detailed traces.
        // 2026-09-14 根修（T-XFER-1~6 全败）：此前整段替换 `logging`，把
        // config 模板里的 `logging.llm` 静默抹掉——cluster 请求日志（执行
        // 档案数据源）随之关闭，档案回传全链死。改为**合并**：只覆盖
        // general 段，llm 段原样保留。
        let logging = obj.entry("logging".to_string()).or_insert(json!({}));
        if let Some(l) = logging.as_object_mut() {
            l.insert(
                "general".to_string(),
                json!({
                    "level": "DEBUG",
                    "enable_console": true,
                    "file": ""
                }),
            );
        }
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
    start_gateway_and_wait_with_env(name, bin, ws_path, node, &[]).await
}

/// 同 [`start_gateway_and_wait`]，额外注入进程环境变量（T-XFER 块大小实验）。
async fn start_gateway_and_wait_with_env(
    name: &'static str,
    bin: &Path,
    ws_path: &Path,
    node: &NodeConfig,
    envs: &[(&str, &str)],
) -> Result<GatewayProcess, String> {
    let gw = GatewayProcess::spawn_with_env(name, bin, ws_path, envs)
        .map_err(|e| format!("Cannot start {}: {}", name, e))?;

    // Wait for HTTP health check (gateway web server up)
    // 30s（2026-09-12）：重启流发生在其余 3 个 gateway + 负载仍在跑时，CI
    // runner 上 15s 偶发不够（与 Phase 7 同一族 flake；本机余量充足）。
    let health_url = format!("http://127.0.0.1:{}/health", node.health_port);
    wait_for_http(&health_url, Duration::from_secs(30))
        .await
        .map_err(|e| format!("{} not healthy after start: {}", name, e))?;

    // Wait for the RPC server to be listening
    let rpc_addr = format!("127.0.0.1:{}", node.rpc_port);
    let rpc_ready = tokio::time::timeout(Duration::from_secs(30), async {
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
                &format!("http://127.0.0.1:{}/v1", ai_server_port()),
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
                        // user 回声帧（发送确认）不是回复——跳过，防止回声
                        // 抢跑被 predicate 误匹配。
                        if v["data"]["role"].as_str() == Some("user") {
                            continue;
                        }
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

/// Query `cluster.nodes.list` and return one node's `online` flag (T20).
/// Matched by human **name** ("Node-B") — registry `id` 是运行时生成的
/// `node-laptop-<host>-<uuid>`，跨次运行不稳定；name 才是稳定身份。
async fn node_online(stream: &mut WsStream, target: &str) -> Result<bool> {
    let nodes = ws_api_request(stream, "cluster", "nodes.list", json!({}), 10).await?;
    nodes
        .pointer("/nodes")
        .and_then(|v| v.as_array())
        .and_then(|arr| {
            arr.iter()
                .find(|n| n.get("name").and_then(|i| i.as_str()) == Some(target))
                .map(|n| n.get("online").and_then(|o| o.as_bool()))
        })
        .flatten()
        .ok_or_else(|| anyhow::anyhow!("node {} not in nodes.list", target))
}

/// Query `cluster.nodes.list` and return one node's runtime `id`
/// （`node-<host>-<uuid>` 形态）。D0 单一真相源统一（2026-09-13）后，派发
/// 账本 `worker_id` 与写回评论 author 都存运行时节点 id（dispatch target
/// 经 `canonical_peer_id` 归一化）——断言 worker 评论作者身份前，先用本
/// 辅助把人读名（"Node-B"）解析成运行时 id。
async fn node_runtime_id(stream: &mut WsStream, target: &str) -> Result<String> {
    let nodes = ws_api_request(stream, "cluster", "nodes.list", json!({}), 10).await?;
    nodes
        .pointer("/nodes")
        .and_then(|v| v.as_array())
        .and_then(|arr| {
            arr.iter()
                .find(|n| n.get("name").and_then(|i| i.as_str()) == Some(target))
                .and_then(|n| n.get("id").and_then(|i| i.as_str()))
                .map(|s| s.to_string())
        })
        .ok_or_else(|| anyhow::anyhow!("node {} not in nodes.list", target))
}

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
// Swarm M1 (T21/T22) helpers
// ---------------------------------------------------------------------------

/// 订阅网关 SSE 事件流，把第一条 `board.plan_ready` 事件的 data JSON 经
/// channel 回传。EventHub 事件只走 SSE（WS push 泵只转发 AgentEvent 的
/// tool_event 帧），浏览器 EventSource 的角色在 cluster-uat 里由 reqwest
/// bytes_stream 手工解析替代。必须在触发 `issue.plan` **之前** spawn ——
/// SSE 新连接只收连接之后的活事件（无 Last-Event-ID 不重放）。
/// 返回（就绪信号, 事件接收端）：就绪信号在收到服务端首字节（heartbeat
/// 帧）时触发——此时 EventHub subscribe 已在服务端就位，之后触发的
/// planner 事件不会因连接竞态丢失（planner 全程仅 ~15ms）。
async fn spawn_plan_ready_listener(
    port: u16,
) -> (
    tokio::sync::oneshot::Receiver<()>,
    tokio::sync::mpsc::Receiver<Value>,
) {
    let (tx, rx) = tokio::sync::mpsc::channel::<Value>(1);
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let client = reqwest::Client::new(); // 无总超时：SSE 长连接
        // F1 统一鉴权（2026-09-22）罩到 REST 全部路由后，SSE 裸连会拿 401
        // JSON（首字节还恰好触发 ready 信号 → 流随即结束 → 「监听器提前
        // 退出」四连败，CI extended-tests 2026-09-23 实录）。走中间件文档化
        // 的 EventSource 兜底：?token= 查询参数（与 ws_connect 同一 token）。
        let url = format!(
            "http://127.0.0.1:{}/api/events/stream?token={}",
            port, AUTH_TOKEN
        );
        let Ok(resp) = client.get(&url).send().await else {
            return; // 连接失败：接收端 timeout 会如实报失败
        };
        let mut stream = resp.bytes_stream();
        let mut ready_tx = Some(ready_tx);
        let mut buf = String::new();
        while let Some(Ok(chunk)) = stream.next().await {
            buf.push_str(&String::from_utf8_lossy(&chunk));
            if let Some(rt) = ready_tx.take() {
                let _ = rt.send(());
            }
            // SSE 事件以空行分隔；只处理完整落缓冲的块（跨 chunk 事件由
            // 累积缓冲自然拼齐）。
            while let Some(pos) = buf.find("\n\n") {
                let block: String = buf.drain(..pos + 2).collect();
                if !block.contains("board.plan_ready") {
                    continue;
                }
                let data_line = block
                    .lines()
                    .find(|l| l.starts_with("data:"))
                    .and_then(|l| l.strip_prefix("data:"))
                    .map(str::trim_start)
                    .unwrap_or("");
                if let Ok(v) = serde_json::from_str::<Value>(data_line) {
                    let _ = tx.send(v).await;
                    return;
                }
            }
        }
    });
    (ready_rx, rx)
}

/// 读取单个 issue 的状态（T21/T22 轮询用；自由函数避免闭包借用逃逸）。
async fn issue_status_of(ws: &mut WsStream, id: i64) -> Result<String, anyhow::Error> {
    let got = ws_api_request(ws, "board", "issue.get", json!({ "id": id }), 10).await?;
    Ok(got
        .pointer("/issue/status")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string())
}

/// T21/T22 共用：建父单 → `issue.plan` 一段（异步 planner）→ SSE 等
/// `board.plan_ready` → confirm 二段。返回（父单 id, 批内顺序子单 id 列表,
/// confirm 响应原文）。
async fn swarm_plan_and_confirm(
    ws: &mut WsStream,
    port: u16,
    marker: &str,
    ready_timeout_secs: u64,
) -> Result<(i64, Vec<i64>, Value), String> {
    // 1. 建父单（letters-only marker —— 数字串会触发 DLP credit_card 误报，
    //    见 T14 注释）。
    let created = ws_api_request(
        ws,
        "board",
        "issue.create",
        json!({
            "title": format!("{} 群体协作拆解 e2e", marker),
            "description": "cluster-uat Swarm M1 planner 全链验证。",
            "acceptance_criteria": "全部子任务完成。",
        }),
        15,
    )
    .await
    .map_err(|e| format!("issue.create failed: {e}"))?;
    let parent_id = created
        .pointer("/issue/id")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    if parent_id == 0 {
        return Err(format!("issue.create returned no id: {created}"));
    }

    // 2. 先挂 SSE 监听并等订阅就绪（heartbeat 首字节 = 服务端 EventHub
    //    subscribe 已就位），再触发 planner——plan_ready 在 issue.plan 后
    //    ~15ms 内就可能发出，不等待会与连接建立赛跑丢帧。
    let (plan_ready_up, mut rx) = spawn_plan_ready_listener(port).await;
    if tokio::time::timeout(Duration::from_secs(10), plan_ready_up)
        .await
        .is_err()
    {
        return Err("SSE 监听 10s 内未就绪（未收到 heartbeat）".to_string());
    }

    // 3. 一段：异步拆解，立即返回 planning + plan_id。
    let planning = ws_api_request(ws, "board", "issue.plan", json!({ "id": parent_id }), 15)
        .await
        .map_err(|e| format!("issue.plan (一段) failed: {e}"))?;
    if planning.get("status").and_then(|v| v.as_str()) != Some("planning") {
        return Err(format!("issue.plan 一段应返回 planning: {planning}"));
    }

    // 4. 等 board.plan_ready（planner 用 testai-planner-1.0 固定输出，秒级）。
    let ready = tokio::time::timeout(Duration::from_secs(ready_timeout_secs), rx.recv())
        .await
        .map_err(|_| format!("{ready_timeout_secs}s 内未收到 board.plan_ready SSE 事件"))?
        .ok_or("SSE 监听器提前退出，未捕获 plan_ready")?;
    if ready.get("issue_id").and_then(|v| v.as_i64()) != Some(parent_id) {
        return Err(format!("plan_ready 的 issue_id 不匹配: {ready}"));
    }
    let plan_id = ready
        .get("plan_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if plan_id.is_empty() {
        return Err(format!("plan_ready 缺 plan_id: {ready}"));
    }

    // 5. 二段 confirm（缓存一次性消费 → 落库 + 依赖闸派发波）。
    let confirmed = ws_api_request(
        ws,
        "board",
        "issue.plan",
        json!({ "id": parent_id, "plan_id": plan_id, "confirm": true }),
        60,
    )
    .await
    .map_err(|e| format!("issue.plan confirm failed: {e}"))?;
    let children: Vec<i64> = confirmed
        .get("created")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_i64()).collect())
        .unwrap_or_default();
    if children.len() != 3 {
        return Err(format!("confirm 应创建 3 个子任务: {confirmed}"));
    }
    Ok((parent_id, children, confirmed))
}

// ---------------------------------------------------------------------------
// Board archive transfer (T-XFER，P3 执行档案回传) helpers
// ---------------------------------------------------------------------------

/// T-XFER 共用发车流：建项目（可选）→ 建单 → 派发 Node-B → 直读 board.db
/// 取 task_id。返回 (issue_id, issue_number, project_dir（无项目=空串）, task_id)。
#[allow(clippy::too_many_arguments)]
async fn xfer_dispatch_to_b(
    ws: &mut WsStream,
    ws_a: &TestWorkspace,
    project_name: Option<&str>,
    title: &str,
    description: &str,
) -> Result<(i64, String, String, String)> {
    let mut project_id: Option<i64> = None;
    let mut project_dir = String::new();
    if let Some(pname) = project_name {
        let created = ws_api_request(
            ws,
            "board",
            "project.create",
            json!({ "name": pname, "auto_start": false }),
            15,
        )
        .await?;
        project_id = created.pointer("/project/id").and_then(|v| v.as_i64());
        project_dir = created
            .pointer("/directory")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if project_id.unwrap_or(0) == 0 || project_dir.is_empty() {
            anyhow::bail!("project.create 无 id/directory: {created}");
        }
    }
    let mut issue_data = json!({ "title": title, "description": description });
    if let Some(pid) = project_id {
        issue_data["project_id"] = json!(pid);
    }
    let created = ws_api_request(ws, "board", "issue.create", issue_data, 15).await?;
    let issue_id = created
        .pointer("/issue/id")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let issue_number = created
        .pointer("/issue/number")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if issue_id == 0 || issue_number.is_empty() {
        anyhow::bail!("issue.create 无 id/number: {created}");
    }
    ws_api_request(
        ws,
        "board",
        "issue.dispatch",
        json!({ "id": issue_id, "target": "Node-B" }),
        30,
    )
    .await?;
    // task_id 从派发账本取（board.db 权威证据；派发记录随 WSAPI 同步落库，
    // 少量重试只兜并发写延迟）。
    let db = ws_a.home().join("workspace").join("board").join("board.db");
    let store = nemesis_board::BoardStore::open(&db, "NB")
        .map_err(|e| anyhow::anyhow!("open board.db: {e}"))?;
    let mut task_id = String::new();
    for _ in 0..40 {
        if let Ok(ds) = store.list_dispatches(issue_id)
            && let Some(d) = ds.last()
        {
            task_id = d.task_id.clone();
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    if task_id.is_empty() {
        anyhow::bail!("派发记录 20s 未落库 issue={issue_id}");
    }
    Ok((issue_id, issue_number, project_dir, task_id))
}

/// B 侧发件箱条目在场（entry.json 存在 = 载荷复制完整、待推/推中/暂停留）。
fn b_outbox_entry(ws_b: &TestWorkspace, task_id: &str) -> Option<std::path::PathBuf> {
    let dir = ws_b
        .home()
        .join("workspace")
        .join("cluster")
        .join("outbox")
        .join(task_id);
    dir.join("entry.json").exists().then_some(dir)
}

/// 发件箱 entry.json 的 state 字段（读不到 = 空串）。
fn b_outbox_state(ws_b: &TestWorkspace, task_id: &str) -> String {
    b_outbox_entry(ws_b, task_id)
        .and_then(|dir| std::fs::read_to_string(dir.join("entry.json")).ok())
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|v| v.get("state").and_then(|s| s.as_str()).map(str::to_string))
        .unwrap_or_default()
}

/// A 侧收件箱条目在场（landed.json 存在 = 完整落地未安置）。
fn a_inbox_entry(ws_a: &TestWorkspace, task_id: &str) -> Option<std::path::PathBuf> {
    let dir = ws_a
        .home()
        .join("workspace")
        .join("cluster")
        .join("inbox")
        .join(task_id);
    dir.join("landed.json").exists().then_some(dir)
}

/// A 侧 .staging 下属于该 task 的传输暂存目录（transfer_id 以 task_id 开头）。
fn a_staging_dirs(ws_a: &TestWorkspace, task_id: &str) -> Vec<std::path::PathBuf> {
    let root = ws_a
        .home()
        .join("workspace")
        .join("cluster")
        .join("inbox")
        .join(".staging");
    std::fs::read_dir(&root)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.is_dir()
                        && p.file_name()
                            .map(|n| n.to_string_lossy().starts_with(task_id))
                            .unwrap_or(false)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// B 侧 cluster_logs 任务执行记录目录（任意设备段下，目录名以 `_{task_id}` 结尾）。
fn b_task_records(ws_b: &TestWorkspace, task_id: &str) -> Option<std::path::PathBuf> {
    let root = ws_b
        .home()
        .join("workspace")
        .join("logs")
        .join("cluster_logs");
    let devices = std::fs::read_dir(root).ok()?;
    for dev in devices.flatten() {
        if !dev.path().is_dir() {
            continue;
        }
        let Ok(tasks) = std::fs::read_dir(dev.path()) else {
            continue;
        };
        for t in tasks.flatten() {
            let name = t.file_name().to_string_lossy().to_string();
            if t.path().is_dir() && name.ends_with(&format!("_{task_id}")) {
                return Some(t.path());
            }
        }
    }
    None
}

/// 项目档案 execution 落地目录列表（records/<number>/execution/<ts>/）。
fn execution_dirs(project_dir: &str, issue_number: &str) -> Vec<std::path::PathBuf> {
    let root = std::path::Path::new(project_dir)
        .join("records")
        .join(issue_number)
        .join("execution");
    std::fs::read_dir(&root)
        .map(|rd| rd.flatten().map(|e| e.path()).collect())
        .unwrap_or_default()
}

/// 直接改 B 的 config.json `board.archive.max_transfer_bytes`（该键不在
/// board.config.set WSAPI 白名单——D4 护栏属部署级配置）。
fn patch_board_archive_limit(home: &Path, value: u64) -> Result<()> {
    let p = home.join("config.json");
    let raw = std::fs::read_to_string(&p)?;
    let mut cfg: Value = serde_json::from_str(&raw)?;
    let obj = cfg
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("config.json 非对象"))?;
    let board = obj.entry("board").or_insert_with(|| json!({}));
    board
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("board 段非对象"))?
        .insert(
            "archive".to_string(),
            json!({ "max_transfer_bytes": value }),
        );
    std::fs::write(&p, serde_json::to_string_pretty(&cfg)?)?;
    Ok(())
}

/// 切换 B 的默认模型（testai-board-1.0 ↔ testai-1.2；T5 用 30s 延迟模型
/// 制造确定性的「执行中」窗口）。只写配置，重启后生效。
async fn b_switch_model(ws_b: &TestWorkspace, gateway_bin: &Path, model: &str) -> Result<()> {
    let out = ws_b
        .run_cli(
            gateway_bin,
            &[
                "model",
                "add",
                "--model",
                model,
                "--base",
                &format!("http://127.0.0.1:{}/v1", ai_server_port()),
                "--key",
                "test-key",
                "--default",
            ],
        )
        .await;
    if !out.success() {
        anyhow::bail!("model add {model} failed: {}", out.stderr);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// P5 冲突漏斗 UAT 辅助（T-MRG-2~7 共用）
// ---------------------------------------------------------------------------

/// 读项目冻结态与待补合并队列长度（project.list 单一投影；Project serde
/// 直出 conflict_frozen / pending_merges）。返回 (frozen, pending 数)。
async fn project_freeze_state(ws: &mut WsStream, project_id: i64) -> anyhow::Result<(bool, usize)> {
    let got = ws_api_request(ws, "board", "project.list", json!({}), 10).await?;
    let projects = got
        .get("projects")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow::anyhow!("project.list 无 projects: {got}"))?;
    let p = projects
        .iter()
        .find(|p| p.get("id").and_then(|v| v.as_i64()) == Some(project_id))
        .ok_or_else(|| anyhow::anyhow!("project.list 缺项目 {project_id}"))?;
    Ok((
        p.get("conflict_frozen")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        p.get("pending_merges")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0),
    ))
}

/// 轮询决策流直到 issue 出现指定 decision（details 精确匹配
/// `"decision":"<kind>"`——`conflict` 与 `conflict_auto_resolve` 等前缀族
/// 靠闭合引号区分），返回 details 原文供 further 断言（mode/new_target…）。
async fn wait_audit_decision(
    ws: &mut WsStream,
    issue_id: i64,
    kind: &str,
    timeout_secs: u64,
) -> anyhow::Result<String> {
    let needle = format!("\"decision\":\"{kind}\"");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!("audit.list 等待 issue {issue_id} 的 {kind} 决策超时（{timeout_secs}s）");
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
        let got = ws_api_request(
            ws,
            "board",
            "audit.list",
            json!({ "limit": 300, "action": "auto_decide" }),
            10,
        )
        .await?;
        let hit = got
            .get("decisions")
            .and_then(|v| v.as_array())
            .and_then(|rows| {
                rows.iter()
                    .find(|r| {
                        r.get("issue_id").and_then(|v| v.as_i64()) == Some(issue_id)
                            && r.get("details")
                                .and_then(|v| v.as_str())
                                .is_some_and(|s| s.contains(&needle))
                    })
                    .and_then(|r| {
                        r.get("details")
                            .and_then(|v| v.as_str())
                            .map(str::to_string)
                    })
            });
        if let Some(details) = hit {
            return Ok(details);
        }
    }
}

/// 冲突测试建单：项目单 + 编辑桩标记直插 description。返回 (id, number)。
async fn create_conflict_issue(
    ws: &mut WsStream,
    project_id: i64,
    title: &str,
    markers: &str,
    acceptance_criteria: &str,
) -> anyhow::Result<(i64, String)> {
    let r = ws_api_request(
        ws,
        "board",
        "issue.create",
        json!({
            "title": title,
            "project_id": project_id,
            "description": format!("在工作副本内完成指定编辑：{markers}"),
            "acceptance_criteria": acceptance_criteria,
        }),
        15,
    )
    .await?;
    let id = r.pointer("/issue/id").and_then(|v| v.as_i64()).unwrap_or(0);
    let number = r
        .pointer("/issue/number")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if id == 0 {
        anyhow::bail!("issue.create 无 id: {r}");
    }
    Ok((id, number))
}

/// 等待两单分流成「胜者/败者」：恰好一单进入 {in_review, done}（先合并方）
/// 且另一单仍 in_progress（冲突被闸）。返回 (winner_id, loser_id)。
async fn wait_conflict_split(
    ws: &mut WsStream,
    id_a: i64,
    id_b: i64,
    timeout_secs: u64,
) -> anyhow::Result<(i64, i64)> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!(
                "等待冲突分流超时：{id_a}='{}' {id_b}='{}'",
                issue_status_of(ws, id_a).await.unwrap_or_default(),
                issue_status_of(ws, id_b).await.unwrap_or_default(),
            );
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        let sa = issue_status_of(ws, id_a).await.unwrap_or_default();
        let sb = issue_status_of(ws, id_b).await.unwrap_or_default();
        let advanced = |s: &str| matches!(s, "in_review" | "done");
        if (advanced(&sa) && sb == "in_progress") || (advanced(&sb) && sa == "in_progress") {
            let winner = if advanced(&sa) { id_a } else { id_b };
            let loser = if advanced(&sa) { id_b } else { id_a };
            return Ok((winner, loser));
        }
    }
}

/// 等待单据到达 done（轮询；异常终态立即 bail）。超时附诊断现场：单据
/// 评论（评审结论/转人工注释落点）+ 决策流该单行——失败免二次翻日志。
async fn wait_issue_done(
    ws: &mut WsStream,
    id: i64,
    number: &str,
    timeout_secs: u64,
) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        if tokio::time::Instant::now() >= deadline {
            let status = issue_status_of(ws, id).await.unwrap_or_default();
            let comments =
                ws_api_request(ws, "board", "comment.list", json!({ "issue_id": id }), 10)
                    .await
                    .ok()
                    .and_then(|r| r.get("comments").and_then(|v| v.as_array()).cloned())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|c| serde_json::to_string(c).ok())
                            .map(|s| format!("      {}", s.chars().take(260).collect::<String>()))
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_else(|| "      （comment.list 拉取失败）".to_string());
            let audit = ws_api_request(
                ws,
                "board",
                "audit.list",
                json!({ "limit": 300, "action": "auto_decide" }),
                10,
            )
            .await
            .ok()
            .and_then(|r| r.get("decisions").and_then(|v| v.as_array()).cloned())
            .map(|rows| {
                rows.iter()
                    .filter(|r| r.get("issue_id").and_then(|v| v.as_i64()) == Some(id))
                    .filter_map(|r| {
                        r.get("details")
                            .and_then(|v| v.as_str())
                            .map(|d| format!("      {}", d.chars().take(260).collect::<String>()))
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_else(|| "      （audit.list 拉取失败）".to_string());
            anyhow::bail!(
                "{timeout_secs}s 内单据 {number} 未 done（现状='{status}'）\n    --- 评论现场 ---\n{comments}\n    --- 决策流现场 ---\n{audit}"
            );
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
        match issue_status_of(ws, id).await.unwrap_or_default().as_str() {
            "done" => return Ok(()),
            "cancelled" => anyhow::bail!("单据 {number} 被取消"),
            _ => {}
        }
    }
}

/// 轮询等待项目档案 execution 落地（返回首个含 manifest.json 的 ts 目录；
/// 失败消息自带两侧传输状态现场，免二次翻日志）。
async fn wait_execution_landed(
    project_dir: &str,
    issue_number: &str,
    task_id: &str,
    ws_a: &TestWorkspace,
    ws_b: &TestWorkspace,
    timeout: Duration,
) -> Result<std::path::PathBuf, String> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "{}s 内档案未落地 task={task_id}（B 发件箱 state='{}'，A 收件箱在场={}）",
                timeout.as_secs(),
                b_outbox_state(ws_b, task_id),
                a_inbox_entry(ws_a, task_id).is_some(),
            ));
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
        if let Some(exec) = execution_dirs(project_dir, issue_number)
            .into_iter()
            .find(|d| d.join("manifest.json").exists())
        {
            return Ok(exec);
        }
    }
}

/// D6 落地凭据核验：manifest 字段自洽（chunk_count == ⌈total/chunk⌉）+
/// files/ 逐文件字节量与 sha256 一致。返回 (total, chunk_size, chunk_count, 文件数)。
fn verify_landed_manifest(exec_dir: &Path) -> Result<(u64, u64, u64, usize), String> {
    let raw = std::fs::read_to_string(exec_dir.join("manifest.json"))
        .map_err(|e| format!("manifest.json 不可读: {e}"))?;
    let mv: Value =
        serde_json::from_str(&raw).map_err(|e| format!("manifest.json 非法 JSON: {e}"))?;
    let total = mv.get("total_bytes").and_then(|v| v.as_u64()).unwrap_or(0);
    let chunk_size = mv.get("chunk_size").and_then(|v| v.as_u64()).unwrap_or(0);
    let chunk_count = mv.get("chunk_count").and_then(|v| v.as_u64()).unwrap_or(0);
    if chunk_size == 0 || chunk_count == 0 {
        return Err(format!(
            "manifest 分块字段非法（size={chunk_size} count={chunk_count}）"
        ));
    }
    let files = mv
        .get("files")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    if files.is_empty() {
        return Err("manifest 无文件清单".to_string());
    }
    // 分块按文件独立切（块不跨文件）：chunk_count == Σ ⌈size_i / chunk⌉。
    let expected: u64 = files
        .iter()
        .map(|f| {
            f.get("size")
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
                .div_ceil(chunk_size)
        })
        .sum();
    if chunk_count != expected {
        return Err(format!(
            "chunk_count={chunk_count} 与 Σ⌈file/chunk⌉ 推导 {expected} 不符"
        ));
    }
    for f in &files {
        let rel = f.get("path").and_then(|v| v.as_str()).unwrap_or_default();
        let size = f.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
        let sha = f.get("sha256").and_then(|v| v.as_str()).unwrap_or_default();
        // 安置布局是**平面**的（ingest_landed 契约：execution/<ts>/ 下直接
        // 是执行记录文件 + manifest.json + landed.json，无 files/ 夹层——
        // 夹层只存在于落地收件箱 inbox/<task>/files/）。
        let data = std::fs::read(exec_dir.join(rel.replace('/', "\\")))
            .map_err(|e| format!("文件 {rel} 不可读: {e}"))?;
        if data.len() as u64 != size {
            return Err(format!(
                "文件 {rel} 大小不符（盘 {} / 账 {size}）",
                data.len()
            ));
        }
        use sha2::Digest;
        let mut h = sha2::Sha256::new();
        h.update(&data);
        let got: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
        if got != sha {
            return Err(format!("文件 {rel} sha256 不符（D6 核验失败）"));
        }
    }
    Ok((total, chunk_size, chunk_count, files.len()))
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
    if let Some(Some(filt)) = TEST_FILTER.get()
        && !name.contains(filt.as_str())
    {
        println!("SKIP");
        return TestResult {
            name: name.to_string(),
            passed: true,
            message: "SKIP: filtered out (--filter)".to_string(),
        };
    }
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
    filter: Option<String>,
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
        filter,
    }
}

/// --filter 接线（2026-09-18 T26 排查）：单测试定点复跑。设置后仅执行
/// 名字含过滤串的测试，其余直接 SKIP（setup/节点装配不受影响）。
static TEST_FILTER: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    let args = parse_args();
    let _ = TEST_FILTER.set(args.filter);

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

    // 进程级前置清理（2026-09-18 第四轮 T26 幽灵污染教训）：上一轮套件/
    // 定点跑/手动实验残留的 nemesisbot.exe 若还活着，其 UDP discovery 会
    // 与本轮节点的 UDP bind 双重绑定（Windows 默认允许 UDP 同端口双绑，
    // TCP 则独占——所以幽灵表现为「只广播 announce、不接 RPC」的半死形态）。
    // 幽灵广播的 announce 身份来自它自己那份（可能残缺的）配置，会把本轮
    // 节点 registry 里的对端条目反复改写（第四轮实证：Node-A 条目在
    // name=id/role=worker 与 name=Node-A 之间横跳），worker 侧
    // board.sync 的 coordinator 查找因此永远落空。cleanup_ports 按 PID
    // 只杀端口占用者，覆盖不了这种 UDP 幽灵——套件开跑前按映像名整体
    // 清一次。测试机=独占测试环境，整体杀安全。
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let ghost_sweep = std::process::Command::new("taskkill")
            .args(["/IM", "nemesisbot.exe", "/F"])
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .output();
        match ghost_sweep {
            Ok(out) if out.status.success() => {
                println!("  Swept leftover nemesisbot.exe processes");
            }
            // 找不到进程（正常情况）taskkill 返回非零——静默即可。
            Ok(_) => println!("  No leftover nemesisbot.exe processes"),
            Err(e) => eprintln!("  WARN: ghost sweep taskkill failed: {}", e),
        }
        // taskkill 是异步通知式的，给内核一点回收端口/句柄的时间，
        // 避免下面的端口探活撞上垂死进程的残留监听。
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    }

    let all_ports: Vec<u16> = NODES
        .iter()
        .flat_map(|n| vec![n.web_port, n.health_port, n.udp_port, n.rpc_port])
        .chain(std::iter::once(ai_server_port()))
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

    let mut ai_server = ManagedProcess::spawn(
        "TestAIServer",
        &ai_server_bin,
        // 显式传端口（与 ai_server_port() 一致；此前裸启动依赖 Go 默认 8080）
        &["--port", &ai_server_port().to_string()],
        &root,
    )
    .expect("Cannot start TestAIServer");

    match wait_for_http(
        &format!("http://127.0.0.1:{}/v1/models", ai_server_port()),
        Duration::from_secs(10),
    )
    .await
    {
        Ok(_) => println!("  TestAIServer ready on port {}", ai_server_port()),
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

    // 2026-09-12 CI flake 根修：旧的「逐节点 15s 串行窗」在 CI runner 上
    // 偶发全灭（同 commit e139aaa 03:29 绿 / 05:25 红；4 个 debug gateway
    // 并发装配远慢于本机，且 A 慢会吃光 B/C/D 的窗口——旧实现里 D 实际
    // 等到 spawn 后 61s 仍未就绪）。改为 4 路并发轮询 + 共享 90s deadline；
    // 失败时倾倒各节点 gateway.log 尾部（stdout/stderr 都在里面）+ 进程
    // 存活状态——下一次红自带根因证据，不再是无诊断价值的纯超时。
    let health_handles: Vec<_> = NODES
        .iter()
        .map(|node| {
            let url = format!("http://127.0.0.1:{}/health", node.health_port);
            tokio::spawn(async move { wait_for_http(&url, Duration::from_secs(90)).await })
        })
        .collect();
    let mut all_healthy = true;
    for (i, handle) in health_handles.into_iter().enumerate() {
        match handle
            .await
            .unwrap_or_else(|e| Err(anyhow::anyhow!("health task failed: {}", e)))
        {
            Ok(_) => println!("  {} ready (health OK)", NODES[i].name),
            Err(e) => {
                eprintln!("  {} NOT ready: {}", NODES[i].name, e);
                all_healthy = false;
            }
        }
    }

    if !all_healthy {
        eprintln!("\nERROR: Not all gateways are healthy. Aborting.");
        // 证据倾倒（在 kill 之前）：进程退出状态（is_running 对已退出的
        // 节点打印 exit status）+ gateway.log 尾部，直接进 CI 日志。
        for gw in [&mut gw_a, &mut gw_b, &mut gw_c, &mut gw_d] {
            let _ = gw.is_running();
            dump_log_tail(&gw.log_path, 60);
        }
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
    println!("  Running Tests (T1-T22, 4-node full chain verification)");
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
                    // 占位升级（2026-09-18）：announce 到达后 [peers.人读名]
                    // 占位键会被重写为 [peers.真实运行时id]（表键即 peer_id，
                    // 人读名保留在 name 字段）——单机/回环拓扑下这在 T2 前
                    // 就会发生（U1-6 名匹配坍缩使 announce 也能触发升级）。
                    // 条目存在的判据因此是：人读名键仍在，或任一条目的
                    // name 字段 == 人读名。
                    let placeholder_key_present =
                        content.contains(&format!("[peers.{}]", sanitized));
                    let name_field_present = content
                        .parse::<toml::Value>()
                        .ok()
                        .and_then(|doc| doc.get("peers").and_then(|v| v.as_table()).cloned())
                        .map(|peers| {
                            peers.values().any(|entry| {
                                entry.get("name").and_then(|v| v.as_str()) == Some(other.name)
                            })
                        })
                        .unwrap_or(false);
                    if !placeholder_key_present && !name_field_present {
                        return fail(
                            "T2",
                            format!(
                                "{} peers.toml missing entry for {} (looked for [peers.{}] or name=\"{}\")",
                                node.name, other.name, sanitized, other.name
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
            // D0 单一真相源（2026-09-13）：写回评论 author = 运行时节点 id
            // （dispatch target 经 canonical_peer_id 归一化），先把人读名
            // 解析成 id 再断言。
            let b_worker_id = match node_runtime_id(&mut ws, "Node-B").await {
                Ok(id) => id,
                Err(e) => return fail("T15", format!("resolve Node-B runtime id failed: {}", e)),
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
                    // 批次 D 起结构化汇报走 ctype=delivery 首评；无格式回复
                    // 仍走 ✅ 降级前缀。两种完成形态都认（testai-3.1 回显
                    // dispatch prompt，prompt 内嵌四段模板会被从宽解析判为
                    // delivery——内容关联性由 marker 断言兜住）。
                    let ctype = c.get("ctype").and_then(|v| v.as_str()).unwrap_or("");
                    if kind == "agent"
                        && id == b_worker_id
                        && (ctype == "delivery" || content.starts_with("✅ worker 汇报完成"))
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
    // 步骤 3：重复取消幂等（B2/B3 停车场复活语义）——Ok{cancelled:true,
    // task_id:null}，不再断言旧契约的「没有进行中的派发」报错。
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
                        &format!("http://127.0.0.1:{}/v1", ai_server_port()),
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

            // 3. 重复取消幂等（B2/B3 停车场复活语义，2026-09-11）：已终态 +
            //    无在途派发 → Ok{cancelled:true, task_id:null, status 抵达
            //    cancelled}，不再报「没有进行中的派发」。
            let again = match ws_api_request(
                &mut ws,
                "board",
                "issue.cancel",
                json!({ "id": issue_id }),
                15,
            )
            .await
            {
                Ok(v) => v,
                Err(e) => return fail("T17", format!("second cancel not idempotent: {}", e)),
            };
            if again.get("cancelled").and_then(|v| v.as_bool()) != Some(true) {
                return fail("T17", format!("second cancel unexpected: {}", again));
            }
            let again_task = again.get("task_id").map(|v| v.is_null());
            if again_task != Some(true) {
                return fail(
                    "T17",
                    format!("second cancel task_id should be null: {}", again),
                );
            }
            let again_status = again
                .pointer("/issue/status")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if again_status != "cancelled" {
                return fail(
                    "T17",
                    format!("second cancel issue status '{}' != cancelled", again_status),
                );
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
                    "cancel 下行 OK：issue {} → cancelled；重复取消幂等（task_id=null）；\
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
                        &format!("http://127.0.0.1:{}/v1", ai_server_port()),
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

    // T19: A 崩溃 → 重启恢复（G5 端到端；2026-09-01 集群韧性 goal）。
    //
    // 场景：A 发起 peer_chat 后崩溃，B 在 A 宕机窗口内完成 —— 回调打到
    // 空处丢失，B 的结果落盘（G1 持久化）。A 重启后 first_start 从续行
    // 快照重建 TaskManager 登记项（G5 接线），恢复循环（120s tick，2min
    // 新鲜保护）查询 B 的 query_task_result → done → 回灌 bus → 续行把
    // B 的回复写进 session_log。
    //
    // 确定性设计：B 切 testai-1.2（固定 30s 延迟 + 固定回复「好的，
    // 我知道了」）。A 的快照文件落盘（rpc_cache/*.json 出现）后才 kill；
    // B 的真结果文件落盘（rpc_cache/results/*.json 且含非空 response/
    // error —— set_running 占位文件不算，否则 A 会在 B 完成前重启、
    // 回调路径短路恢复查询路径，见 has_real_result 注释）后才重启 A ——
    // 两步都以磁盘证据为闸，无 sleep 竞态。session_log 断言同文件内：
    // user 行含 marker（重启前写入，重启存活）+ assistant 行含 B 的固定
    // 回复（只可能来自恢复路径 —— A 的 testai-3.1 在 async ack 轮不会
    // 产生该文案，回调路径在 A 宕机时已丢失）。
    all_results.push(
        run_test("T19: A 崩溃 → 重启恢复（G5）", || async {
            let marker = "T19ARECOVERYMARKERUNIQXYZ";

            // 0. B 切 30s 延迟模型 + 重启。
            let out = ws_b
                .run_cli(
                    &gateway_bin,
                    &[
                        "model",
                        "add",
                        "--model",
                        "test/testai-1.2",
                        "--base",
                        &format!("http://127.0.0.1:{}/v1", ai_server_port()),
                        "--key",
                        "test-key",
                        "--default",
                    ],
                )
                .await;
            if !out.success() {
                return fail("T19", format!("B model switch failed: {}", out.stderr));
            }
            gw_b.kill().await;
            gw_b = match start_gateway_and_wait("Gateway-B", &gateway_bin, ws_b.path(), &NODES[1])
                .await
            {
                Ok(g) => g,
                Err(e) => return fail("T19", format!("B restart failed: {}", e)),
            };

            let a_cache = ws_a
                .home()
                .join("workspace")
                .join("cluster")
                .join("rpc_cache");
            let b_results = ws_b
                .home()
                .join("workspace")
                .join("cluster")
                .join("rpc_cache")
                .join("results");

            // 基线：A 的快照数 / B 的结果数（此前测试完成即清理，应为 0，
            // 但用增量判断以容忍残留）。
            let count_json = |dir: &std::path::Path| -> usize {
                std::fs::read_dir(dir)
                    .map(|entries| {
                        entries
                            .flatten()
                            .filter(|e| {
                                e.path().extension().and_then(|x| x.to_str()) == Some("json")
                            })
                            .count()
                    })
                    .unwrap_or(0)
            };
            let a_snapshots_before = count_json(&a_cache);
            let _b_results_before = count_json(&b_results); // 基线记录；闸用 has_real_result（内容级）
            // 内容级判断：只认「真结果」文件（result.response / result.error
            // 非空）。set_running 占位文件（{"status":"running"}）在任务创建
            // 瞬间就会落盘 —— 旧闸 count_json 曾被它提前满足 → A 在 B 的 30s
            // LLM 完成前就重启 → B 的回调重试撞上复活的 A 走回调路径成功，
            // 恢复查询路径从未被真正验证（T19 假阳性根因，真机 R1a 发现）。
            let has_real_result = |dir: &std::path::Path| -> bool {
                std::fs::read_dir(dir)
                    .map(|entries| {
                        entries.flatten().any(|e| {
                            let Ok(data) = std::fs::read_to_string(e.path()) else {
                                return false;
                            };
                            let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) else {
                                return false;
                            };
                            ["response", "error"].iter().any(|k| {
                                v["result"][k].as_str().is_some_and(|s| !s.is_empty())
                            })
                        })
                    })
                    .unwrap_or(false)
            };

            // 1. 发起 peer_chat（不等待回复 —— A 即将崩溃）。
            let mut ws = match ws_connect_gateway(NODES[0].web_port).await {
                Ok(s) => s,
                Err(e) => return fail("T19", format!("WS connect to A failed: {}", e)),
            };
            let send_msg = json!({
                "type": "message",
                "module": "chat",
                "cmd": "send",
                "data": { "content": format!(
                    r#"<PEER_CHAT>{{"peer_id":"Node-B","content":"{}"}}</PEER_CHAT>"#,
                    marker
                ) },
                "timestamp": chrono::Local::now().to_rfc3339()
            });
            if let Err(e) = ws.send(Message::Text(send_msg.to_string().into())).await {
                return fail("T19", format!("WS send failed: {}", e));
            }

            // 2. 等 A 的续行快照落盘（≤20s）—— 这是恢复的物质基础。
            let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
            loop {
                if count_json(&a_cache) > a_snapshots_before {
                    break;
                }
                if tokio::time::Instant::now() >= deadline {
                    return fail(
                        "T19",
                        format!("20s 内 A 的续行快照未出现在 {}", a_cache.display()),
                    );
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }

            // 3. 杀 A（此刻 B 仍在 30s LLM 延迟中）。
            gw_a.kill().await;

            // 4. 等 B 在 A 宕机窗口内完成（真结果文件落盘，≤90s from now）。
            //    内容级闸（见 has_real_result）：占位文件不算 —— 否则 A 会在
            //    B 的 LLM 完成前重启，恢复查询路径被回调路径短路（假阳性）。
            let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
            loop {
                if has_real_result(&b_results) {
                    break;
                }
                if tokio::time::Instant::now() >= deadline {
                    return fail(
                        "T19",
                        format!("90s 内 B 的结果未落盘于 {}（B 未在 A 宕机期间完成）", b_results.display()),
                    );
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }

            // 5. 重启 A → first_start 重建登记项 → 恢复循环接管。
            gw_a = match start_gateway_and_wait("Gateway-A", &gateway_bin, ws_a.path(), &NODES[0])
                .await
            {
                Ok(g) => g,
                Err(e) => return fail("T19", format!("A restart failed: {}", e)),
            };

            // 6. 轮询 session_logs（恢复循环 120s tick + 2min 新鲜保护，
            //    预计 2-4 分钟内完成；deadline 300s）。
            let session_logs_dir = ws_a
                .home()
                .join("workspace")
                .join("logs")
                .join("session_logs");
            let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
            let mut recovered = false;
            while tokio::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_secs(5)).await;
                let mut user_seen = false;
                let mut reply_seen = false;
                if let Ok(entries) = std::fs::read_dir(&session_logs_dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                            continue;
                        }
                        let Ok(content) = std::fs::read_to_string(&path) else {
                            continue;
                        };
                        if !content.contains(marker) {
                            continue;
                        }
                        for line in content.lines() {
                            if line.contains(marker)
                                && (line.contains(r#""role":"user""#)
                                    || line.contains(r#""role": "user""#))
                            {
                                user_seen = true;
                            }
                            if line.contains("好的，我知道了")
                                && (line.contains(r#""role":"assistant""#)
                                    || line.contains(r#""role": "assistant""#))
                            {
                                reply_seen = true;
                            }
                        }
                    }
                }
                if user_seen && reply_seen {
                    recovered = true;
                    break;
                }
            }
            if !recovered {
                return fail(
                    "T19",
                    format!(
                        "300s 内恢复未完成：session_logs 无 marker user 行 + 「好的，我知道了」assistant 行（dir={}）——检查 first_start 登记/恢复循环/回灌 bus 链路",
                        session_logs_dir.display()
                    ),
                );
            }
            pass(
                "T19",
                "A 崩溃重启后经恢复循环取回 B 的结果并写入 session_log（G5 端到端 OK）",
            )
        })
        .await,
    );

    // T20: 主动健康探针 Offline/Online 翻转（G2 端到端；2026-09-01 集群韧性
    // goal）。A 注入 test-speed 探针配置（2s 间隔 / 阈值 2）并重启；kill B →
    // 2 次探针失败（~4-8s）翻转 Offline（被动过期要 120s+，主动探针是本次
    // 验证点）；重启 B → announce upsert Online / 探针成功自愈。
    // 本测试最后跑：A 的探针配置会保留到 run 结束。
    all_results.push(
        run_test("T20: 主动健康探针翻转（G2）", || async {
            // 0. A 注入探针配置（AppConfig 读 workspace/config/config.cluster.json
            //    顶层键）+ 重启武装探针循环。
            let cluster_cfg = ws_a
                .home()
                .join("workspace")
                .join("config")
                .join("config.cluster.json");
            let raw = match std::fs::read_to_string(&cluster_cfg) {
                Ok(r) => r,
                Err(e) => {
                    return fail(
                        "T20",
                        format!("read {} failed: {}", cluster_cfg.display(), e),
                    );
                }
            };
            let mut cfg: Value = match serde_json::from_str(&raw) {
                Ok(v) => v,
                Err(e) => return fail("T20", format!("parse config.cluster.json failed: {}", e)),
            };
            if let Some(obj) = cfg.as_object_mut() {
                obj.insert("health_check_interval_secs".into(), json!(2));
                obj.insert("health_check_failure_threshold".into(), json!(2));
            }
            let pretty = match serde_json::to_string_pretty(&cfg) {
                Ok(s) => s,
                Err(e) => return fail("T20", format!("serialize config failed: {}", e)),
            };
            if let Err(e) = std::fs::write(&cluster_cfg, pretty) {
                return fail(
                    "T20",
                    format!("write {} failed: {}", cluster_cfg.display(), e),
                );
            }

            gw_a.kill().await;
            gw_a = match start_gateway_and_wait("Gateway-A", &gateway_bin, ws_a.path(), &NODES[0])
                .await
            {
                Ok(g) => g,
                Err(e) => return fail("T20", format!("A restart failed: {}", e)),
            };

            // 1. 基线：B 在线。
            let mut ws = match ws_connect_gateway(NODES[0].web_port).await {
                Ok(s) => s,
                Err(e) => return fail("T20", format!("WS connect to A failed: {}", e)),
            };
            match node_online(&mut ws, "Node-B").await {
                Ok(true) => {}
                Ok(false) => return fail("T20", "基线即 offline —— 前序测试未恢复或探针误报"),
                Err(e) => return fail("T20", format!("nodes.list failed: {}", e)),
            }

            // 2. kill B → 轮询 ≤40s 等 Offline（2 次失败 × 2s ≈ 4-8s）。
            gw_b.kill().await;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(40);
            let mut went_offline = false;
            while tokio::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_secs(2)).await;
                match node_online(&mut ws, "Node-B").await {
                    Ok(false) => {
                        went_offline = true;
                        break;
                    }
                    Ok(true) => {}
                    Err(e) => return fail("T20", format!("nodes.list failed: {}", e)),
                }
            }
            if !went_offline {
                return fail(
                    "T20",
                    "40s 内 B 未被主动探针标记 Offline（预期 2 次失败 × 2s 间隔 ≈ 4-8s）",
                );
            }

            // 3. 重启 B → announce upsert Online / 探针成功自愈（≤60s）。
            gw_b = match start_gateway_and_wait("Gateway-B", &gateway_bin, ws_b.path(), &NODES[1])
                .await
            {
                Ok(g) => g,
                Err(e) => return fail("T20", format!("B restart failed: {}", e)),
            };
            let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
            let mut healed = false;
            while tokio::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_secs(3)).await;
                match node_online(&mut ws, "Node-B").await {
                    Ok(true) => {
                        healed = true;
                        break;
                    }
                    Ok(false) => {}
                    Err(e) => return fail("T20", format!("nodes.list failed: {}", e)),
                }
            }
            if !healed {
                return fail(
                    "T20",
                    "60s 内 B 未恢复 Online（announce upsert / 探针自愈均未生效）",
                );
            }
            pass("T20", "主动探针 Offline/Online 双向翻转 OK（G2 端到端）")
        })
        .await,
    );

    // T21: Swarm M1 G1 —— 父单 AI 拆解（两段式）→ 依赖闸派发波 → worker
    // 完成 → 回流 in_review。
    //
    // 前置（本测试开头做，跑在套件最后所以改动不回滚）：
    //   a) A 切 testai-planner-1.0（固定合法拆解 JSON，见 TestAIServer
    //      models/planner_model.go）+ 重启 —— planner 经 A 的 agent_loop
    //      run_detached 裸提示词调用，用 A 的默认模型。
    //   b) board sweep 调宽（600s/10s）—— T15 配的 15s sweep 会把派到
    //      testai-1.2 慢模型 worker（30s LLM）的在途派发误标失败。
    // 链路：issue.plan 一段（异步 planner，SSE board.plan_ready 推预览）→
    // confirm 二段（落库+依赖闸派发波）→ 无依赖子单派出 → worker 完成写回
    // in_review + 完成评论；父单随首派 → in_progress。
    all_results.push(
        run_test("T21: Swarm G1 AI拆解两段式+自动派发全链", || async {
            // 0. 前置：planner 模型 + sweep 调宽，一次重启生效两改动。
            let out = ws_a
                .run_cli(
                    &gateway_bin,
                    &[
                        "model",
                        "add",
                        "--model",
                        "test/testai-planner-1.0",
                        "--base",
                        &format!("http://127.0.0.1:{}/v1", ai_server_port()),
                        "--key",
                        "test-key",
                        "--default",
                    ],
                )
                .await;
            if !out.success() {
                return fail("T21", format!("A planner model add failed: {}", out.stderr));
            }
            if let Err(e) = configure_board_sweep(&ws_a.home(), 600, 10) {
                return fail("T21", format!("A sweep reconfig failed: {e}"));
            }
            gw_a.kill().await;
            gw_a = match start_gateway_and_wait("Gateway-A", &gateway_bin, ws_a.path(), &NODES[0])
                .await
            {
                Ok(g) => g,
                Err(e) => return fail("T21", format!("A restart failed: {e}")),
            };

            let mut ws = match ws_connect_gateway(NODES[0].web_port).await {
                Ok(s) => s,
                Err(e) => return fail("T21", format!("WS connect to A failed: {e}")),
            };
            let (parent_id, children, confirm) = match swarm_plan_and_confirm(
                &mut ws,
                NODES[0].web_port,
                "T21SWARMPLANNERMARKER",
                60,
            )
            .await
            {
                Ok(v) => v,
                Err(e) => return fail("T21", e),
            };

            // 1. 派发波：3 子单中只派出无依赖的子0；子1/子2 依赖闸暂缓。
            if confirm.get("dispatched").and_then(|v| v.as_u64()) != Some(1) {
                return fail(
                    "T21",
                    format!("派发波应派出 1 张（无依赖子任务）: {confirm}"),
                );
            }
            let deferred = confirm
                .get("deferred")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            if deferred != 2 {
                return fail("T21", format!("依赖闸应暂缓 2 张: {confirm}"));
            }

            // 2. 状态断言：父单 in_progress（首派联动）；子0 in_progress；
            //    子1/子2 backlog。
            for (id, want, what) in [
                (parent_id, "in_progress", "父单"),
                (children[0], "in_progress", "子0（无依赖）"),
                (children[1], "backlog", "子1（依赖子0）"),
                (children[2], "backlog", "子2（依赖子1）"),
            ] {
                match issue_status_of(&mut ws, id).await {
                    Ok(s) if s == want => {}
                    Ok(s) => {
                        return fail("T21", format!("{what}应为 {want}，实际 {s}"));
                    }
                    Err(e) => return fail("T21", format!("issue.get({id}) failed: {e}")),
                }
            }

            // 3. 轮询子0 → in_review（worker 完成 + 写回；目标可能是
            //    testai-3.1 即时回显或 testai-1.2 慢 30s，上限 240s）。
            let deadline = tokio::time::Instant::now() + Duration::from_secs(240);
            let mut sub0_status = String::new();
            loop {
                if tokio::time::Instant::now() >= deadline {
                    return fail(
                        "T21",
                        format!(
                            "240s 内子0 未到 in_review（最后状态='{sub0_status}'）\
                             —— worker 完成写回链路未走通",
                        ),
                    );
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
                sub0_status = match issue_status_of(&mut ws, children[0]).await {
                    Ok(s) => s,
                    Err(e) => return fail("T21", format!("issue.get failed: {e}")),
                };
                if sub0_status == "in_review" {
                    break;
                }
            }

            // 4. 完成评论写回（gateway write_back_board_dispatch）。
            let comments = match ws_api_request(
                &mut ws,
                "board",
                "comment.list",
                json!({ "issue_id": children[0] }),
                10,
            )
            .await
            {
                Ok(v) => v,
                Err(e) => return fail("T21", format!("comment.list failed: {e}")),
            };
            let has_report = comments
                .get("comments")
                .and_then(|c| c.as_array())
                .map(|arr| {
                    // delivery 首评（批次 D）或 ✅ 降级前缀，都算完成写回。
                    arr.iter().any(|c| {
                        let ctype = c.get("ctype").and_then(|v| v.as_str()).unwrap_or("");
                        let content = c.get("content").and_then(|v| v.as_str()).unwrap_or("");
                        ctype == "delivery" || content.starts_with("✅ worker 汇报完成")
                    })
                })
                .unwrap_or(false);
            if !has_report {
                return fail(
                    "T21",
                    format!("子0 到 in_review 但无 worker 完成评论: {comments}"),
                );
            }
            pass(
                "T21",
                format!(
                    "Swarm G1 全链 OK：父单 {parent_id} 拆解 3 子单，子0 自动派出→完成 in_review\
                     （评论已写回），父单 in_progress",
                ),
            )
        })
        .await,
    );

    // T22: Swarm M1 G2 —— 依赖闸 + 完成后自动补派（无人工干预）。
    //
    // 场景：子1 depends_on 子0。confirm 波子1 留 backlog；人工验收子0
    // （in_review → done）后 on_issue_settled 触发器自动补派子1 —— 测试在
    // issue.status 之后**不做任何派发动作**，子1 转 in_progress 即为补派
    // 发生的直接证据。子2 depends_on 子1，保持 backlog；父单不收口。
    all_results.push(
        run_test("T22: Swarm G2 依赖闸+完成后自动补派", || async {
            let mut ws = match ws_connect_gateway(NODES[0].web_port).await {
                Ok(s) => s,
                Err(e) => return fail("T22", format!("WS connect to A failed: {e}")),
            };
            let (parent_id, children, _confirm) =
                match swarm_plan_and_confirm(&mut ws, NODES[0].web_port, "T22SWARMDEPMARKER", 60)
                    .await
                {
                    Ok(v) => v,
                    Err(e) => return fail("T22", e),
                };

            // 0. 等子0 完成到 in_review（同 T21，≤240s）。
            let deadline = tokio::time::Instant::now() + Duration::from_secs(240);
            let mut sub0_status = String::new();
            loop {
                if tokio::time::Instant::now() >= deadline {
                    return fail(
                        "T22",
                        format!("240s 内子0 未到 in_review（最后 '{sub0_status}'）"),
                    );
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
                sub0_status = match issue_status_of(&mut ws, children[0]).await {
                    Ok(s) => s,
                    Err(e) => return fail("T22", format!("issue.get failed: {e}")),
                };
                if sub0_status == "in_review" {
                    break;
                }
            }
            // 前置锚点：此刻子1 必须仍在 backlog（依赖闸生效）。
            match issue_status_of(&mut ws, children[1]).await {
                Ok(s) if s == "backlog" => {}
                Ok(s) => return fail("T22", format!("验收前子1 应留 backlog，实际 {s}")),
                Err(e) => return fail("T22", format!("issue.get failed: {e}")),
            }

            // 1. 人工验收签字：子0 → done。此后测试不再做任何派发动作。
            match ws_api_request(
                &mut ws,
                "board",
                "issue.status",
                json!({ "id": children[0], "status": "done" }),
                60,
            )
            .await
            {
                Ok(_) => {}
                Err(e) => return fail("T22", format!("issue.status done failed: {e}")),
            }

            // 2. 自动补派证据：子1 未经 issue.dispatch → in_progress
            //    （on_issue_settled 在 issue.status 内同步触发，轮询只兜
            //    时序抖动，≤30s）。
            let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
            let mut sub1_status = String::new();
            loop {
                if tokio::time::Instant::now() >= deadline {
                    return fail(
                        "T22",
                        format!(
                            "子0 done 后 30s 内子1 未被自动补派（状态 '{sub1_status}'）\
                             —— on_issue_settled 补派触发器未生效",
                        ),
                    );
                }
                sub1_status = match issue_status_of(&mut ws, children[1]).await {
                    Ok(s) => s,
                    Err(e) => return fail("T22", format!("issue.get failed: {e}")),
                };
                if sub1_status == "in_progress" {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }

            // 3. 子2（依赖子1）仍 backlog；父单未收口（in_progress）。
            for (id, want, what) in [
                (children[2], "backlog", "子2（依赖未完成）"),
                (parent_id, "in_progress", "父单（子1/子2 未终态）"),
            ] {
                match issue_status_of(&mut ws, id).await {
                    Ok(s) if s == want => {}
                    Ok(s) => return fail("T22", format!("{what}应为 {want}，实际 {s}")),
                    Err(e) => return fail("T22", format!("issue.get({id}) failed: {e}")),
                }
            }
            pass(
                "T22",
                "Swarm G2 OK：依赖闸拦住子1/子2；子0 验收 done 后子1 无人工干预自动派出",
            )
        })
        .await,
    );

    // ==================================================================
    // Swarm M3 批次 G（G3/G4/G5/G8/G9 出口判据；G12 幂等由 board_bus /
    // board handler 单测双臂钉死，dashboard post / writeback 已间接走幂
    // 等管线，不单独立项）。
    // ==================================================================

    /// 拉频道消息（board.channel.messages after_id 游标）。
    async fn channel_messages(ws: &mut WsStream, channel_id: i64) -> Result<Vec<Value>, String> {
        let r = ws_api_request(
            ws,
            "board",
            "channel.messages",
            json!({ "channel_id": channel_id, "after_id": 0, "limit": 500 }),
            10,
        )
        .await
        .map_err(|e| format!("channel.messages failed: {e}"))?;
        Ok(r.get("messages")
            .and_then(|m| m.as_array())
            .cloned()
            .unwrap_or_default())
    }

    /// 统计某节点的 agent 发言数（G3/G4 断言用）。
    fn agent_posts_of(messages: &[Value], node_id: &str) -> usize {
        messages
            .iter()
            .filter(|m| {
                m.pointer("/sender/kind").and_then(|v| v.as_str()) == Some("agent")
                    && m.pointer("/sender/id").and_then(|v| v.as_str()) == Some(node_id)
            })
            .count()
    }

    /// 从节点 home 的 peers.toml [node] 段读运行时节点 id。cluster init
    /// 生成 node-{host}-{uuid} 形态 id，名字（"Node-B"）≠id——讨论链路的
    /// sender 与裁决日志都记 id。
    fn read_node_runtime_id(ws: &TestWorkspace) -> Option<String> {
        let src = std::fs::read_to_string(
            ws.home()
                .join("workspace")
                .join("cluster")
                .join("peers.toml"),
        )
        .ok()?;
        let mut in_node = false;
        for line in src.lines() {
            let t = line.trim();
            if t.starts_with('[') {
                in_node = t == "[node]";
                continue;
            }
            if in_node && let Some(rest) = t.strip_prefix("id =") {
                let v = rest.trim().trim_matches('"').to_string();
                if !v.is_empty() {
                    return Some(v);
                }
            }
        }
        None
    }

    // T23: 定向投递 + worker 被动响应（G3+G4）。
    //
    // dashboard（admin）在 #dev 发「@Node-B …」→ 规则裁决器只点名 B →
    // wake.post 下发 → B 的 cluster agent 消费（testai-1.1 固定回复
    // 「好的，我知道了」，不含 [SILENT]，必然发言）→ comment.post 上行
    // 落 #dev。断言：B 有发言、C/D 无发言（定向性）、A 的 gateway 日志
    // 有 wake decision 审计行（G3 出口判据的「日志可查」）。
    all_results.push(
        run_test("T23: 定向投递 @Node-B + worker 被动响应（G3+G4）", || async {
            // 0. B 切 testai-1.1（T17 后 B=1.2 慢速；1.1 固定快速回复，
            //    且回复文本不含 "[SILENT]" —— 3.1 回显 prompt 会把指令里
            //    的 [SILENT] 字样带回，触发误判沉默）。
            let out = ws_b
                .run_cli(
                    &gateway_bin,
                    &[
                        "model",
                        "add",
                        "--model",
                        "test/testai-1.1",
                        "--base",
                        &format!("http://127.0.0.1:{}/v1", ai_server_port()),
                        "--key",
                        "test-key",
                        "--default",
                    ],
                )
                .await;
            if !out.success() {
                return fail("T23", format!("B model switch failed: {}", out.stderr));
            }
            gw_b.kill().await;
            gw_b = match start_gateway_and_wait("Gateway-B", &gateway_bin, ws_b.path(), &NODES[1])
                .await
            {
                Ok(g) => g,
                Err(e) => return fail("T23", e),
            };

            let mut ws = match ws_connect_gateway(NODES[0].web_port).await {
                Ok(s) => s,
                Err(e) => return fail("T23", format!("WS connect to A failed: {}", e)),
            };
            let b_node_id = match read_node_runtime_id(&ws_b) {
                Some(id) => id,
                None => return fail("T23", "无法从 B 的 peers.toml 读到 [node] id"),
            };
            let c_node_id = read_node_runtime_id(&ws_c);
            let d_node_id = read_node_runtime_id(&ws_d);

            // 1. 找 #dev 频道 + 记录 C/D 发言 baseline。
            let channels = match ws_api_request(&mut ws, "board", "channel.list", json!({}), 10)
                .await
            {
                Ok(v) => v,
                Err(e) => return fail("T23", format!("channel.list failed: {e}")),
            };
            let dev_id = channels
                .get("channels")
                .and_then(|c| c.as_array())
                .and_then(|arr| {
                    arr.iter()
                        .find(|c| c.get("name").and_then(|n| n.as_str()) == Some("#dev"))
                        .and_then(|c| c.get("id").and_then(|i| i.as_i64()))
                })
                .ok_or_else(|| "no #dev channel".to_string());
            let dev_id = match dev_id {
                Ok(id) => id,
                Err(e) => return fail("T23", e),
            };
            let baseline = match channel_messages(&mut ws, dev_id).await {
                Ok(m) => m,
                Err(e) => return fail("T23", e),
            };
            let count_of = |msgs: &[Value], id: Option<&String>| match id {
                Some(real) => agent_posts_of(msgs, real),
                None => 0,
            };
            let base_c = count_of(&baseline, c_node_id.as_ref());
            let base_d = count_of(&baseline, d_node_id.as_ref());

            // 2. dashboard 人工发言 @Node-B（走讨论总线管线：幂等/额度/裁决）。
            let posted = match ws_api_request(
                &mut ws,
                "board",
                "channel.post",
                json!({ "channel_id": dev_id, "content": "@Node-B T23DIRECT 请确认定向投递链路" }),
                15,
            )
            .await
            {
                Ok(v) => v,
                Err(e) => return fail("T23", format!("channel.post failed: {e}")),
            };
            if posted.pointer("/posted/message_id").and_then(|v| v.as_i64()).unwrap_or(0) == 0 {
                return fail("T23", format!("channel.post 未返回 message_id: {posted}"));
            }

            // 3. 轮询 ≤150s 等 B 的 agent 发言（wake → agent round → 上行）。
            let deadline = tokio::time::Instant::now() + Duration::from_secs(150);
            let b_posts = loop {
                if tokio::time::Instant::now() >= deadline {
                    return fail(
                        "T23",
                        "150s 内 Node-B 未在 #dev 发言（baseline 后增量 0）\
                         —— wake 投递或 worker 响应链路未走通",
                    );
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
                let msgs = match channel_messages(&mut ws, dev_id).await {
                    Ok(m) => m,
                    Err(e) => return fail("T23", e),
                };
                let now_b = agent_posts_of(&msgs, &b_node_id);
                if now_b > agent_posts_of(&baseline, &b_node_id) {
                    break now_b;
                }
            };

            // 4. 定向性：C/D 发言数不增。
            let msgs = match channel_messages(&mut ws, dev_id).await {
                Ok(m) => m,
                Err(e) => return fail("T23", e),
            };
            let (c_now, d_now) = (
                count_of(&msgs, c_node_id.as_ref()),
                count_of(&msgs, d_node_id.as_ref()),
            );
            if c_now != base_c || d_now != base_d {
                return fail(
                    "T23",
                    format!("非目标节点被唤醒发言：Node-C {base_c}→{c_now}, Node-D {base_d}→{d_now}"),
                );
            }

            // 5. 审计日志：A 的 gateway.log 有 wake decision 行且 woke 名单
            //    含 B 的真实节点 id（裁决按节点名 @提及匹配，日志记 id）。
            tokio::time::sleep(Duration::from_secs(2)).await; // 裁决 spawn 异步落日志
            let log = std::fs::read_to_string(&gw_a.log_path).unwrap_or_default();
            let decision = log
                .lines()
                .filter(|l| l.contains("wake decision"))
                .find(|l| l.contains(&b_node_id));
            if decision.is_none() {
                return fail(
                    "T23",
                    "A 日志未找到 woke 含 Node-B 真实 id 的 wake decision 审计行（G3 日志断言失败）",
                );
            }
            pass(
                "T23",
                format!(
                    "定向投递 OK：@Node-B 只唤醒 B（发言 ×{}），C/D 无辜；裁决日志在案",
                    b_posts
                ),
            )
        })
        .await,
    );

    // T24: worker 排队无丢失（G4 双 wake）。
    //
    // 连续两条 @Node-B（各自触发裁决 + wake.post）→ B 依次消费两条 wake
    // → 两条上行发言。断言 baseline 之后 B 的发言增量 ≥2。
    all_results.push(
        run_test("T24: 双 wake 排队无丢失（G4）", || async {
            let mut ws = match ws_connect_gateway(NODES[0].web_port).await {
                Ok(s) => s,
                Err(e) => return fail("T24", format!("WS connect to A failed: {}", e)),
            };
            let channels =
                match ws_api_request(&mut ws, "board", "channel.list", json!({}), 10).await {
                    Ok(v) => v,
                    Err(e) => return fail("T24", format!("channel.list failed: {e}")),
                };
            let dev_id = channels
                .get("channels")
                .and_then(|c| c.as_array())
                .and_then(|arr| {
                    arr.iter()
                        .find(|c| c.get("name").and_then(|n| n.as_str()) == Some("#dev"))
                        .and_then(|c| c.get("id").and_then(|i| i.as_i64()))
                })
                .ok_or_else(|| "no #dev channel".to_string());
            let dev_id = match dev_id {
                Ok(id) => id,
                Err(e) => return fail("T24", e),
            };
            let baseline = match channel_messages(&mut ws, dev_id).await {
                Ok(m) => m,
                Err(e) => return fail("T24", e),
            };
            let b_node_id = match read_node_runtime_id(&ws_b) {
                Some(id) => id,
                None => return fail("T24", "无法从 B 的 peers.toml 读到 [node] id"),
            };
            let base_b = agent_posts_of(&baseline, &b_node_id);

            for content in [
                "@Node-B T24QUEUEONE 第一条排队消息",
                "@Node-B T24QUEUETWO 第二条排队消息",
            ] {
                if let Err(e) = ws_api_request(
                    &mut ws,
                    "board",
                    "channel.post",
                    json!({ "channel_id": dev_id, "content": content }),
                    15,
                )
                .await
                {
                    return fail("T24", format!("channel.post({content}) failed: {e}"));
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }

            // 两条 wake 各一轮 agent（1.1 快速模型），150s 足够。
            let deadline = tokio::time::Instant::now() + Duration::from_secs(150);
            loop {
                if tokio::time::Instant::now() >= deadline {
                    let msgs = channel_messages(&mut ws, dev_id).await.unwrap_or_default();
                    return fail(
                        "T24",
                        format!(
                            "150s 内 Node-B 发言增量未达 2（当前 {}）——第二条 wake 丢失或未排队",
                            agent_posts_of(&msgs, &b_node_id) - base_b,
                        ),
                    );
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
                let msgs = match channel_messages(&mut ws, dev_id).await {
                    Ok(m) => m,
                    Err(e) => return fail("T24", e),
                };
                if agent_posts_of(&msgs, &b_node_id) >= base_b + 2 {
                    break;
                }
            }
            pass("T24", "排队 OK：连续两条 @Node-B 都被处理，发言无丢失")
        })
        .await,
    );

    // T25: 交付线程（G5）。
    //
    // B 切 testai-10.0（固定输出四段结构化汇报）→ dispatch → writeback
    // 解析四段成功 → ctype='delivery' 首评（无「✅ worker 汇报完成」降级
    // 前缀）+ issue in_progress→in_review。对照：T15 用 testai-3.1 的无
    // 格式回复走「✅」普通评论降级路径。
    all_results.push(
        run_test("T25: 结构化汇报 → delivery 首评（G5）", || async {
            // 0. B 切 testai-10.0 + 重启。
            let out = ws_b
                .run_cli(
                    &gateway_bin,
                    &[
                        "model",
                        "add",
                        "--model",
                        "test/testai-10.0",
                        "--base",
                        &format!("http://127.0.0.1:{}/v1", ai_server_port()),
                        "--key",
                        "test-key",
                        "--default",
                    ],
                )
                .await;
            if !out.success() {
                return fail("T25", format!("B model switch failed: {}", out.stderr));
            }
            gw_b.kill().await;
            gw_b = match start_gateway_and_wait("Gateway-B", &gateway_bin, ws_b.path(), &NODES[1])
                .await
            {
                Ok(g) => g,
                Err(e) => return fail("T25", e),
            };

            let mut ws = match ws_connect_gateway(NODES[0].web_port).await {
                Ok(s) => s,
                Err(e) => return fail("T25", format!("WS connect to A failed: {}", e)),
            };
            // D0 单一真相源（2026-09-13）：delivery 首评 author = 运行时
            // 节点 id（同 T15），先把人读名解析成 id 再断言。
            let b_worker_id = match node_runtime_id(&mut ws, "Node-B").await {
                Ok(id) => id,
                Err(e) => return fail("T25", format!("resolve Node-B runtime id failed: {}", e)),
            };

            let marker = "T25DELIVERYREPORT";
            let created = match ws_api_request(
                &mut ws,
                "board",
                "issue.create",
                json!({
                    "title": format!("{} 交付线程 e2e", marker),
                    "description": "cluster-uat T25：结构化汇报生成 delivery 首评。",
                    "acceptance_criteria": "汇报四段齐全。",
                }),
                15,
            )
            .await
            {
                Ok(v) => v,
                Err(e) => return fail("T25", format!("issue.create failed: {e}")),
            };
            let issue_id = created
                .pointer("/issue/id")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            if issue_id == 0 {
                return fail("T25", format!("issue.create 无 id: {created}"));
            }
            if let Err(e) = ws_api_request(
                &mut ws,
                "board",
                "issue.dispatch",
                json!({ "id": issue_id, "target": "Node-B" }),
                30,
            )
            .await
            {
                return fail("T25", format!("issue.dispatch failed: {e}"));
            }

            // 轮询 ≤240s 等 writeback 推进 in_review（同 T15 预算）。
            let deadline = tokio::time::Instant::now() + Duration::from_secs(240);
            let mut last_status = String::new();
            loop {
                if tokio::time::Instant::now() >= deadline {
                    return fail(
                        "T25",
                        format!("240s 内 issue 未到 in_review（最后 '{last_status}'）",),
                    );
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
                last_status = match issue_status_of(&mut ws, issue_id).await {
                    Ok(s) => s,
                    Err(e) => return fail("T25", format!("issue.get failed: {e}")),
                };
                if last_status == "in_review" {
                    break;
                }
            }

            // 断言 delivery 首评：ctype=delivery、四段齐全、无降级前缀。
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
                Err(e) => return fail("T25", format!("comment.list failed: {e}")),
            };
            let delivery = comments
                .get("comments")
                .and_then(|c| c.as_array())
                .and_then(|arr| {
                    arr.iter().find(|c| {
                        c.get("ctype").and_then(|t| t.as_str()) == Some("delivery")
                            && c.pointer("/author/id")
                                .and_then(|v| v.as_str())
                                .is_some_and(|id| id == b_worker_id)
                    })
                })
                .cloned();
            match delivery {
                Some(c) => {
                    let content = c.get("content").and_then(|v| v.as_str()).unwrap_or("");
                    let four_sections = [
                        "## 结论",
                        "## 交付物清单",
                        "## 自检结果",
                        "## 风险与未尽事项",
                    ]
                    .iter()
                    .all(|s| content.contains(s));
                    if !four_sections {
                        return fail(
                            "T25",
                            format!("delivery 评论四段不全：{}", trunc(content, 200)),
                        );
                    }
                    if content.starts_with("✅ worker 汇报完成") {
                        return fail("T25", "delivery 评论带降级前缀——格式解析应成功而非降级");
                    }
                    pass(
                        "T25",
                        format!(
                            "交付线程 OK：issue {} → in_review，delivery 首评四段齐全（{}）",
                            issue_id,
                            trunc(content, 100),
                        ),
                    )
                }
                None => fail(
                    "T25",
                    format!(
                        "in_review 但无 ctype=delivery 评论：{}",
                        trunc(&comments.to_string(), 400)
                    ),
                ),
            }
        })
        .await,
    );

    // T26: worker 离线补拉（G8）。
    //
    // B 下线窗口内在 #qa @Node-B（默认频道第三选，与 T23/T24 的 #dev 隔离
    // 额度）→ wake 投递失败/跳过（sync 兜底记账）→ B 重启（WorkerWakeState
    // 清零，board.sync 从 0 重拉）→ 60s 节拍内补拉命中「@我」→ 入队 →
    // 上行发言。B 保持 testai-10.0（非 [SILENT]）。
    all_results.push(
        run_test(
            "T26: worker 离线 → 上线 board.sync 补拉（G8）",
            || async {
                let mut ws = match ws_connect_gateway(NODES[0].web_port).await {
                    Ok(s) => s,
                    Err(e) => return fail("T26", format!("WS connect to A failed: {}", e)),
                };
                let channels =
                    match ws_api_request(&mut ws, "board", "channel.list", json!({}), 10).await {
                        Ok(v) => v,
                        Err(e) => return fail("T26", format!("channel.list failed: {e}")),
                    };
                let ops_id = channels
                    .get("channels")
                    .and_then(|c| c.as_array())
                    .and_then(|arr| {
                        arr.iter()
                            .find(|c| c.get("name").and_then(|n| n.as_str()) == Some("#qa"))
                            .and_then(|c| c.get("id").and_then(|i| i.as_i64()))
                    })
                    .ok_or_else(|| "no #qa channel".to_string());
                let ops_id = match ops_id {
                    Ok(id) => id,
                    Err(e) => return fail("T26", e),
                };
                let baseline = match channel_messages(&mut ws, ops_id).await {
                    Ok(m) => m,
                    Err(e) => return fail("T26", e),
                };
                let b_node_id = match read_node_runtime_id(&ws_b) {
                    Some(id) => id,
                    None => return fail("T26", "无法从 B 的 peers.toml 读到 [node] id"),
                };
                let base_b = agent_posts_of(&baseline, &b_node_id);

                // 0. B 切 testai-10.0（T23/T25 同款先例）：setup 默认的
                //    testai-3.1 是 echo——discussion prompt 的「沉默指令」
                //    含 [SILENT] 字样，echo 原样带回 → handle_discussion
                //    从宽子串判定误判沉默 → worker 永不发言。定点复跑
                //    （--filter T26，跳过 T25 的模型切换）时这里不补切
                //    必挂。10.0 固定五段汇报、不含 [SILENT]。幂等：全套
                //    件里 T25 已切过，重复 model add --default 无害。
                let out = ws_b
                    .run_cli(
                        &gateway_bin,
                        &[
                            "model",
                            "add",
                            "--model",
                            "test/testai-10.0",
                            "--base",
                            &format!("http://127.0.0.1:{}/v1", ai_server_port()),
                            "--key",
                            "test-key",
                            "--default",
                        ],
                    )
                    .await;
                if !out.success() {
                    return fail("T26", format!("B model switch failed: {}", out.stderr));
                }

                // 1. 杀 B → 确认离线窗口 → @Node-B。
                gw_b.kill().await;
                tokio::time::sleep(Duration::from_secs(3)).await;
                if let Err(e) = ws_api_request(
                    &mut ws,
                    "board",
                    "channel.post",
                    json!({ "channel_id": ops_id, "content": "@Node-B T26OFFLINE 离线补拉验证" }),
                    15,
                )
                .await
                {
                    return fail("T26", format!("channel.post failed: {e}"));
                }

                // 2. 重启 B → board.sync 60s 节拍补拉 → 发言。
                gw_b =
                    match start_gateway_and_wait("Gateway-B", &gateway_bin, ws_b.path(), &NODES[1])
                        .await
                    {
                        Ok(g) => g,
                        Err(e) => return fail("T26", e),
                    };

                // 预算：重启就绪 + 首 sync tick ≤60s + agent round + 上行 → 240s。
                let deadline = tokio::time::Instant::now() + Duration::from_secs(240);
                loop {
                    if tokio::time::Instant::now() >= deadline {
                        return fail(
                            "T26",
                            "240s 内 Node-B 未补拉处理后发言——board.sync 补拉链路未走通",
                        );
                    }
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    let msgs = match channel_messages(&mut ws, ops_id).await {
                        Ok(m) => m,
                        Err(e) => return fail("T26", e),
                    };
                    if agent_posts_of(&msgs, &b_node_id) > base_b {
                        break;
                    }
                }
                pass(
                    "T26",
                    "离线韧性 OK：B 下线窗口的 @ 消息在上线后由 board.sync 补拉处理",
                )
            },
        )
        .await,
    );

    // T27: 资产 HTTP 分发层（G9）。
    //
    // 测试装置直接构造（签发/登记是装置不是被测行为，单测层已全覆盖）：
    // 落文件到 A 的 board/assets + 直接登记 A 的 board.db + secret 签发
    // bundle。被测面=真 HTTP 端点：正向下载字节一致；过期 token 拒绝；
    // 篡改 token 拒绝。
    all_results.push(
        run_test("T27: 资产下载 正向/过期/篡改（G9）", || async {
            // gateway --local 的 home = temp/.nemesisbot，workspace 在其下。
            let workspace = ws_a.home().join("workspace");
            let secret = match nemesis_board::load_or_create_secret(
                &nemesis_path::resolve_asset_secret_path_in_workspace(&workspace),
            ) {
                Ok(s) => s,
                Err(e) => return fail("T27", format!("asset secret unavailable: {e}")),
            };
            let node_url = match std::fs::read_to_string(
                nemesis_path::resolve_asset_node_url_path_in_workspace(&workspace),
            ) {
                Ok(u) => u.trim().to_string(),
                Err(e) => return fail("T27", format!("node url file unavailable: {e}")),
            };
            if node_url.is_empty() {
                return fail("T27", "node url file 为空（gateway 未落盘 bind 地址）");
            }
            // 实际 GET 走 127.0.0.1：node_url 落盘的 host 是 gateway 选的
            // 非回环 IP（跨机拉取用），同机访问受防火墙策略影响。token 的
            // HMAC 只覆盖 ref_name + expires_at，host 不参与校验——换成
            // 127.0.0.1 不影响 token 语义。
            let base = format!("http://127.0.0.1:{}", NODES[0].web_port);

            // 装置：登记 + 落文件（与 gateway 下载端点共享同一 db/目录）。
            let ref_name = "uat-t27-asset.bin";
            let body = b"T27ASSETBODYUATTWENTYSEVEN0123456789";
            let assets_dir = nemesis_path::resolve_board_assets_dir_in_workspace(&workspace);
            if let Err(e) = std::fs::create_dir_all(&assets_dir) {
                return fail("T27", format!("create assets dir: {e}"));
            }
            let asset_path = assets_dir.join(ref_name);
            if let Err(e) = std::fs::write(&asset_path, body) {
                return fail("T27", format!("write asset file: {e}"));
            }
            let sha = match nemesis_board::sha256_file(&asset_path) {
                Ok(s) => s,
                Err(e) => return fail("T27", format!("sha256: {e}")),
            };
            let db_path = workspace.join("board").join("board.db");
            let store = match nemesis_board::BoardStore::open(&db_path, "NB") {
                Ok(s) => s,
                Err(e) => return fail("T27", format!("open board.db: {e}")),
            };
            if let Err(e) = store.register_asset(nemesis_board::NewAsset {
                ref_name: ref_name.to_string(),
                origin_issue: None,
                sha256: sha.clone(),
                size: body.len() as i64,
            }) {
                return fail("T27", format!("register_asset: {e}"));
            }

            let client = reqwest::Client::new();
            let url = |token: &str, expires: i64| {
                format!(
                    "{base}/api/board/asset/{ref_name}?asset_token={token}&expires_at={expires}"
                )
            };

            // 1. 正向：bundle 签发 → GET 200 + 字节一致。
            let bundle = nemesis_board::issue_asset_bundle(
                &secret,
                ref_name,
                &sha,
                body.len() as i64,
                &node_url,
                "",
                nemesis_board::DEFAULT_TOKEN_TTL_SECS,
            );
            let resp = match client
                .get(url(&bundle.asset_token, bundle.expires_at))
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => return fail("T27", format!("GET failed: {e}")),
            };
            if resp.status() != reqwest::StatusCode::OK {
                return fail("T27", format!("正向下载应为 200，got {}", resp.status()));
            }
            let got = match resp.bytes().await {
                Ok(b) => b,
                Err(e) => return fail("T27", format!("read body: {e}")),
            };
            if got.as_ref() != body {
                return fail("T27", "正向下载字节不一致");
            }

            // 2. 过期 token：HMAC 覆盖 expires_at，须签发时即给负 TTL
            //    （签名合法但已过期 → 走 Expired 分支拒绝）。
            let expired = nemesis_board::issue_asset_bundle(
                &secret,
                ref_name,
                &sha,
                body.len() as i64,
                &node_url,
                "",
                -100,
            );
            let resp = match client
                .get(url(&expired.asset_token, expired.expires_at))
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => return fail("T27", format!("GET(expired) failed: {e}")),
            };
            if !resp.status().is_client_error() {
                return fail("T27", format!("过期 token 应被拒绝，got {}", resp.status()));
            }

            // 3. 篡改 token：改首字符 → HMAC 失配拒绝。
            let mut tampered = bundle.asset_token.clone();
            let first = tampered.as_bytes()[0];
            tampered.replace_range(..1, if first == b'A' { "B" } else { "A" });
            let resp = match client.get(url(&tampered, bundle.expires_at)).send().await {
                Ok(r) => r,
                Err(e) => return fail("T27", format!("GET(tampered) failed: {e}")),
            };
            if !resp.status().is_client_error() {
                return fail("T27", format!("篡改 token 应被拒绝，got {}", resp.status()));
            }
            pass(
                "T27",
                "资产 HTTP OK：正向 200 字节一致；过期/篡改 token 均被拒绝",
            )
        })
        .await,
    );

    // T28: 自动验收三态处置 + FAIL 重派保险丝（Swarm M4 G6）。
    //
    // 前置：A 切 testai-review-1.0（评审 LLM；verdict 由验收标准里的锚点
    // 决定，见 TestAIServer models/review_model.go）+ 重启 —— 验收经 A 主
    // agent_loop run_detached 裸提示词调用（与 planner 同款），用 A 的默认
    // 模型。board 旗标走默认（auto_review=true / auto_accept=false /
    // max_redispatch=2）。worker=B（testai-10.0 固定四段汇报，T25 已切）。
    // 三子场景：
    //   ① FAIL 保险丝：验收标准埋 <REVIEW_FAIL> → FAIL→重派#1→FAIL→重派#2
    //      →FAIL→预算(2)耗尽→转人工。断言 dispatch=3、两条 ❌ 重派评论、
    //      🤷 转人工评论带差距锚点与预算耗尽说明、状态保持 in_review。
    //   ② PASS 待人工：正常标准 → PASS →「待人工确认」评论；dispatch=1、
    //      状态保持 in_review（默认不自动 done）。
    //   ③ UNSURE 转人工：<REVIEW_UNSURE> → 🤷 无法定案评论；dispatch=1、
    //      in_review 保持。
    all_results.push(
        run_test("T28: 自动验收三态处置 + FAIL 重派保险丝（G6）", || async {
            // 0. A 切验收模型 + 重启。
            let out = ws_a
                .run_cli(
                    &gateway_bin,
                    &[
                        "model",
                        "add",
                        "--model",
                        "test/testai-review-1.0",
                        "--base",
                        &format!("http://127.0.0.1:{}/v1", ai_server_port()),
                        "--key",
                        "test-key",
                        "--default",
                    ],
                )
                .await;
            if !out.success() {
                return fail("T28", format!("A review model add failed: {}", out.stderr));
            }
            gw_a.kill().await;
            gw_a = match start_gateway_and_wait("Gateway-A", &gateway_bin, ws_a.path(), &NODES[0])
                .await
            {
                Ok(g) => g,
                Err(e) => return fail("T28", format!("A restart failed: {e}")),
            };

            let mut ws = match ws_connect_gateway(NODES[0].web_port).await {
                Ok(s) => s,
                Err(e) => return fail("T28", format!("WS connect to A failed: {e}")),
            };
            // 直接开 A 的 board.db 数派发行（重派次数的权威证据）。gateway
            // 持有同一 db 并发写，瞬时 BUSY 读失败按 0 计（只影响轮询中间
            // 态，终态断言会重试取值）。
            let db_path = ws_a.home().join("workspace").join("board").join("board.db");
            let board_store = match nemesis_board::BoardStore::open(&db_path, "NB") {
                Ok(s) => s,
                Err(e) => return fail("T28", format!("open board.db: {e}")),
            };
            let dispatch_count = |id: i64| -> usize {
                board_store
                    .list_dispatches(id)
                    .map(|v| v.len())
                    .unwrap_or(0)
            };

            // 评论全文拼接（轮询终态信号 + 终态断言都靠它）。
            // WS 请求失败返回空串，由轮询重试兜底。
            async fn comments_text(ws: &mut WsStream, issue_id: i64) -> String {
                match ws_api_request(
                    ws,
                    "board",
                    "comment.list",
                    json!({ "issue_id": issue_id }),
                    10,
                )
                .await
                {
                    Ok(v) => v
                        .get("comments")
                        .and_then(|c| c.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|c| c.get("content").and_then(|v| v.as_str()))
                                .collect::<Vec<_>>()
                                .join("\n---\n")
                        })
                        .unwrap_or_default(),
                    Err(_) => String::new(),
                }
            }

            // ---- ① FAIL 保险丝：3 轮 FAIL → 2 次重派 → 预算耗尽转人工 ----
            let created = match ws_api_request(
                &mut ws,
                "board",
                "issue.create",
                json!({
                    "title": "T28FAILFUSE 验收保险丝 e2e",
                    "description": "cluster-uat T28①：FAIL→重派→预算耗尽转人工。",
                    "acceptance_criteria": "交付物必须包含 <REVIEW_FAIL> 验收锚点材料。",
                }),
                15,
            )
            .await
            {
                Ok(v) => v,
                Err(e) => return fail("T28", format!("issue.create(FAIL) failed: {e}")),
            };
            let fail_issue = created
                .pointer("/issue/id")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            if fail_issue == 0 {
                return fail("T28", format!("issue.create(FAIL) 无 id: {created}"));
            }
            if let Err(e) = ws_api_request(
                &mut ws,
                "board",
                "issue.dispatch",
                json!({ "id": fail_issue, "target": "Node-B" }),
                30,
            )
            .await
            {
                return fail("T28", format!("issue.dispatch(FAIL) failed: {e}"));
            }
            // 终态信号 = 🤷 转人工评论（评审 FAIL 循环收口）。3 轮 worker
            // 处理 + 3 次评审，预算 600s。
            let deadline = tokio::time::Instant::now() + Duration::from_secs(600);
            loop {
                if tokio::time::Instant::now() >= deadline {
                    let n = dispatch_count(fail_issue);
                    let st = issue_status_of(&mut ws, fail_issue).await.unwrap_or_default();
                    return fail(
                        "T28",
                        format!(
                            "600s 内 FAIL 场景未收口（dispatch={n}, status='{st}'）\
                             —— 自动验收触发、重派链或保险丝未走通",
                        ),
                    );
                }
                tokio::time::sleep(Duration::from_secs(5)).await;
                let t = comments_text(&mut ws, fail_issue).await;
                if t.contains("验收 agent 无法定案") {
                    break;
                }
            }
            // 终态断言（dispatch 数读 BUSY 瞬态 → 小重试）。
            let mut n = dispatch_count(fail_issue);
            for _ in 0..5 {
                if n == 3 { break; }
                tokio::time::sleep(Duration::from_secs(1)).await;
                n = dispatch_count(fail_issue);
            }
            let text = comments_text(&mut ws, fail_issue).await;
            let status = match issue_status_of(&mut ws, fail_issue).await {
                Ok(s) => s,
                Err(e) => return fail("T28", format!("issue.get(FAIL) failed: {e}")),
            };
            if n != 3 {
                return fail("T28", format!("FAIL 场景应重派至 3 次派发（1+2），实际 {n}"));
            }
            if !text.contains("第 1/2 次重派") || !text.contains("第 2/2 次重派") {
                return fail("T28", format!("FAIL 场景缺两条 ❌ 重派评论: {}", trunc(&text, 400)));
            }
            if !text.contains("重派预算已耗尽") || !text.contains("T28 差距锚点") {
                return fail(
                    "T28",
                    format!("转人工评论应带预算耗尽说明 + 差距锚点: {}", trunc(&text, 400)),
                );
            }
            if status != "in_review" {
                return fail("T28", format!("FAIL 预算耗尽后应保持 in_review，实际 '{status}'"));
            }

            // ---- ② PASS 待人工：默认不自动 done ----
            let created = match ws_api_request(
                &mut ws,
                "board",
                "issue.create",
                json!({
                    "title": "T28PASSMANUAL 验收通过待人工 e2e",
                    "description": "cluster-uat T28②：PASS→待人工确认，不自动 done。",
                    "acceptance_criteria": "汇报四段齐全即可。",
                }),
                15,
            )
            .await
            {
                Ok(v) => v,
                Err(e) => return fail("T28", format!("issue.create(PASS) failed: {e}")),
            };
            let pass_issue = created
                .pointer("/issue/id")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            if pass_issue == 0 {
                return fail("T28", format!("issue.create(PASS) 无 id: {created}"));
            }
            if let Err(e) = ws_api_request(
                &mut ws,
                "board",
                "issue.dispatch",
                json!({ "id": pass_issue, "target": "Node-B" }),
                30,
            )
            .await
            {
                return fail("T28", format!("issue.dispatch(PASS) failed: {e}"));
            }
            let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
            loop {
                if tokio::time::Instant::now() >= deadline {
                    let st = issue_status_of(&mut ws, pass_issue).await.unwrap_or_default();
                    return fail(
                        "T28",
                        format!("300s 内 PASS 场景未出验收意见（status='{st}'）"),
                    );
                }
                tokio::time::sleep(Duration::from_secs(5)).await;
                let t = comments_text(&mut ws, pass_issue).await;
                if t.contains("待人工确认") {
                    break;
                }
            }
            let status = match issue_status_of(&mut ws, pass_issue).await {
                Ok(s) => s,
                Err(e) => return fail("T28", format!("issue.get(PASS) failed: {e}")),
            };
            if dispatch_count(pass_issue) != 1 {
                return fail("T28", "PASS 场景不应发生重派（dispatch 应为 1）");
            }
            if status != "in_review" {
                return fail(
                    "T28",
                    format!("PASS + auto_accept=false 应保持 in_review，实际 '{status}'"),
                );
            }

            // ---- ③ UNSURE 转人工 ----
            let created = match ws_api_request(
                &mut ws,
                "board",
                "issue.create",
                json!({
                    "title": "T28UNSUREHUMAN 验收无法定案 e2e",
                    "description": "cluster-uat T28③：UNSURE→转人工。",
                    "acceptance_criteria": "按 <REVIEW_UNSURE> 锚点处理。",
                }),
                15,
            )
            .await
            {
                Ok(v) => v,
                Err(e) => return fail("T28", format!("issue.create(UNSURE) failed: {e}")),
            };
            let unsure_issue = created
                .pointer("/issue/id")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            if unsure_issue == 0 {
                return fail("T28", format!("issue.create(UNSURE) 无 id: {created}"));
            }
            if let Err(e) = ws_api_request(
                &mut ws,
                "board",
                "issue.dispatch",
                json!({ "id": unsure_issue, "target": "Node-B" }),
                30,
            )
            .await
            {
                return fail("T28", format!("issue.dispatch(UNSURE) failed: {e}"));
            }
            let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
            loop {
                if tokio::time::Instant::now() >= deadline {
                    let st = issue_status_of(&mut ws, unsure_issue).await.unwrap_or_default();
                    return fail(
                        "T28",
                        format!("300s 内 UNSURE 场景未转人工（status='{st}'）"),
                    );
                }
                tokio::time::sleep(Duration::from_secs(5)).await;
                let t = comments_text(&mut ws, unsure_issue).await;
                if t.contains("验收 agent 无法定案") {
                    break;
                }
            }
            let status = match issue_status_of(&mut ws, unsure_issue).await {
                Ok(s) => s,
                Err(e) => return fail("T28", format!("issue.get(UNSURE) failed: {e}")),
            };
            if dispatch_count(unsure_issue) != 1 {
                return fail("T28", "UNSURE 场景不应发生重派（dispatch 应为 1）");
            }
            if status != "in_review" {
                return fail(
                    "T28",
                    format!("UNSURE 转人工后应保持 in_review，实际 '{status}'"),
                );
            }
            pass(
                "T28",
                format!(
                    "自动验收 OK：FAIL 3 轮→重派×2→预算耗尽转人工（issue {fail_issue}，保持 in_review）；\
                     PASS→待人工确认不自动 done（issue {pass_issue}）；UNSURE→转人工（issue {unsure_issue}）"
                ),
            )
        })
        .await,
    );

    // T29: 集体记忆蒸馏→注入闭环（Swarm M4.5 G7）。
    //
    // 三段链路（模型切换均显式执行，不依赖前序测试残留态）：
    //   ①蒸馏：A 切 testai-review-1.0 / B 切 testai-10.0（五段汇报）→
    //      验收标准埋 <REVIEW_EXP> → 评审 PASS + experience 槽位 →
    //      team_memory 落库。断言走直开 board.db（权威证据，同 T28）：
    //      scope=t29auth / category=pitfall / content 含锚点 / use_count=0
    //      / source=来源单号。
    //   ②planner 注入：A 切 testai-planner-1.0 → 父单 description 含
    //      t29auth（scope 关键词命中）→ issue.plan 一段 → plan_ready
    //      payload 的子任务描述含 T29PLANEXP（planner 模型回显锚点 = 经验
    //      段真的进了 prompt）。meta 注入不记 use_count。
    //   ③派发注入：B 切 testai-3.1（terminal echo 回显整个 prompt）→
    //      issue3 description 含 t29auth → dispatch → 回声写回评论含
    //      「团队过往经验」段头与经验锚点 → use_count==1（只认发车派发）。
    //      A 此时停在 planner 模型，issue3 的自动验收解析失败转人工——
    //      与本测试断言无关，不理会。
    all_results.push(
        run_test("T29: 集体记忆蒸馏→planner/派发注入闭环（G7）", || async {
            // 0. 前置模型态。
            let out = ws_a
                .run_cli(
                    &gateway_bin,
                    &[
                        "model",
                        "add",
                        "--model",
                        "test/testai-review-1.0",
                        "--base",
                        &format!("http://127.0.0.1:{}/v1", ai_server_port()),
                        "--key",
                        "test-key",
                        "--default",
                    ],
                )
                .await;
            if !out.success() {
                return fail("T29", format!("A review model add failed: {}", out.stderr));
            }
            gw_a.kill().await;
            gw_a = match start_gateway_and_wait("Gateway-A", &gateway_bin, ws_a.path(), &NODES[0])
                .await
            {
                Ok(g) => g,
                Err(e) => return fail("T29", format!("A restart failed: {e}")),
            };
            let out = ws_b
                .run_cli(
                    &gateway_bin,
                    &[
                        "model",
                        "add",
                        "--model",
                        "test/testai-10.0",
                        "--base",
                        &format!("http://127.0.0.1:{}/v1", ai_server_port()),
                        "--key",
                        "test-key",
                        "--default",
                    ],
                )
                .await;
            if !out.success() {
                return fail("T29", format!("B report model add failed: {}", out.stderr));
            }
            gw_b.kill().await;
            gw_b = match start_gateway_and_wait("Gateway-B", &gateway_bin, ws_b.path(), &NODES[1])
                .await
            {
                Ok(g) => g,
                Err(e) => return fail("T29", format!("B restart failed: {e}")),
            };

            let mut ws = match ws_connect_gateway(NODES[0].web_port).await {
                Ok(s) => s,
                Err(e) => return fail("T29", format!("WS connect to A failed: {e}")),
            };
            let db_path = ws_a.home().join("workspace").join("board").join("board.db");
            let board_store = match nemesis_board::BoardStore::open(&db_path, "NB") {
                Ok(s) => s,
                Err(e) => return fail("T29", format!("open board.db: {e}")),
            };
            // 评论全文拼接（T28 同款：WS 失败返回空串由轮询重试兜底）。
            async fn comments_text(ws: &mut WsStream, issue_id: i64) -> String {
                match ws_api_request(
                    ws,
                    "board",
                    "comment.list",
                    json!({ "issue_id": issue_id }),
                    10,
                )
                .await
                {
                    Ok(v) => v
                        .get("comments")
                        .and_then(|c| c.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|c| c.get("content").and_then(|v| v.as_str()))
                                .collect::<Vec<_>>()
                                .join("\n---\n")
                        })
                        .unwrap_or_default(),
                    Err(_) => String::new(),
                }
            }

            // ---- ① 蒸馏：评审 experience 槽位 → team_memory ----
            let created = match ws_api_request(
                &mut ws,
                "board",
                "issue.create",
                json!({
                    "title": "T29EXPDISTILL 集体记忆蒸馏 e2e",
                    "description": "cluster-uat T29①：验收蒸馏经验入库。",
                    "acceptance_criteria": "交付物必须包含 <REVIEW_EXP> 经验锚点材料。",
                }),
                15,
            )
            .await
            {
                Ok(v) => v,
                Err(e) => return fail("T29", format!("issue.create(distill) failed: {e}")),
            };
            let distill_issue = created
                .pointer("/issue/id")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            if distill_issue == 0 {
                return fail("T29", format!("issue.create(distill) 无 id: {created}"));
            }
            if let Err(e) = ws_api_request(
                &mut ws,
                "board",
                "issue.dispatch",
                json!({ "id": distill_issue, "target": "Node-B" }),
                30,
            )
            .await
            {
                return fail("T29", format!("issue.dispatch(distill) failed: {e}"));
            }
            // 轮询 ≤300s 等 team_memory 出现 scope=t29auth 条目（评审异步）。
            let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
            let entry: nemesis_board::models::TeamMemoryEntry = loop {
                if tokio::time::Instant::now() >= deadline {
                    return fail("T29", "300s 内 team_memory 未出现 t29auth 条目（评审蒸馏链路断）");
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
                if let Ok(v) = board_store.list_team_memory(Some("t29auth"), false)
                    && let Some(e) = v.into_iter().next()
                {
                    break e;
                }            };
            if entry.category != "pitfall" {
                return fail("T29", format!("经验类别应 pitfall，实际 '{}'", entry.category));
            }
            if !entry.content.contains("T29 经验锚点") {
                return fail("T29", format!("经验内容缺锚点：{}", trunc(&entry.content, 200)));
            }
            if entry.use_count != 0 {
                return fail("T29", format!("入库时 use_count 应 0，实际 {}", entry.use_count));
            }
            if entry.deprecated {
                return fail("T29", "新入库条目不应是 deprecated");
            }
            if entry.source.is_empty() || entry.author.is_empty() {
                return fail(
                    "T29",
                    format!(
                        "source/author 应非空（来源单号/蒸馏节点），实际 '{}/{}'",
                        entry.source, entry.author
                    ),
                );
            }
            // 评审闭环旁证：PASS → 待人工确认评论在场。
            let text = comments_text(&mut ws, distill_issue).await;
            if !text.contains("待人工确认") {
                return fail("T29", format!("蒸馏 issue 缺「待人工确认」评审评论: {}", trunc(&text, 300)));
            }

            // ---- ② planner 注入：plan_ready payload 探针锚点 ----
            let out = ws_a
                .run_cli(
                    &gateway_bin,
                    &[
                        "model",
                        "add",
                        "--model",
                        "test/testai-planner-1.0",
                        "--base",
                        &format!("http://127.0.0.1:{}/v1", ai_server_port()),
                        "--key",
                        "test-key",
                        "--default",
                    ],
                )
                .await;
            if !out.success() {
                return fail("T29", format!("A planner model add failed: {}", out.stderr));
            }
            gw_a.kill().await;
            gw_a = match start_gateway_and_wait("Gateway-A", &gateway_bin, ws_a.path(), &NODES[0])
                .await
            {
                Ok(g) => g,
                Err(e) => return fail("T29", format!("A planner restart failed: {e}")),
            };
            let mut ws = match ws_connect_gateway(NODES[0].web_port).await {
                Ok(s) => s,
                Err(e) => return fail("T29", format!("WS reconnect to A failed: {e}")),
            };
            // SSE 监听必须先于 issue.plan（无重放）。
            let (plan_ready_up, mut plan_rx) =
                spawn_plan_ready_listener(NODES[0].web_port).await;
            if tokio::time::timeout(Duration::from_secs(10), plan_ready_up)
                .await
                .is_err()
            {
                return fail("T29", "SSE 监听 10s 未就绪");
            }
            let created = match ws_api_request(
                &mut ws,
                "board",
                "issue.create",
                json!({
                    "title": "T29PLANINJECT planner 经验注入 e2e",
                    "description": "cluster-uat T29②：拆解时按 t29auth 关键词注入团队经验。",
                    "acceptance_criteria": "拆解成功。",
                }),
                15,
            )
            .await
            {
                Ok(v) => v,
                Err(e) => return fail("T29", format!("issue.create(plan) failed: {e}")),
            };
            let plan_issue = created
                .pointer("/issue/id")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            if plan_issue == 0 {
                return fail("T29", format!("issue.create(plan) 无 id: {created}"));
            }
            if let Err(e) = ws_api_request(
                &mut ws,
                "board",
                "issue.plan",
                json!({ "id": plan_issue }),
                15,
            )
            .await
            {
                return fail("T29", format!("issue.plan failed: {e}"));
            }
            let plan_payload = match tokio::time::timeout(Duration::from_secs(30), plan_rx.recv())
                .await
            {
                Ok(Some(v)) => v,
                _ => return fail("T29", "30s 内未收到 board.plan_ready（planner 未跑或失败）"),
            };
            let sub1_desc = plan_payload
                .pointer("/subs/0/description")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if !sub1_desc.contains("T29PLANEXP") {
                return fail(
                    "T29",
                    format!(
                        "plan_ready 子任务描述缺 T29PLANEXP（经验段未进 planner prompt）：{}",
                        trunc(&plan_payload.to_string(), 300)
                    ),
                );
            }

            // ---- ③ 派发注入：echo 写回评论 + use_count ----
            let out = ws_b
                .run_cli(
                    &gateway_bin,
                    &[
                        "model",
                        "add",
                        "--model",
                        "test/testai-3.1",
                        "--base",
                        &format!("http://127.0.0.1:{}/v1", ai_server_port()),
                        "--key",
                        "test-key",
                        "--default",
                    ],
                )
                .await;
            if !out.success() {
                return fail("T29", format!("B echo model add failed: {}", out.stderr));
            }
            gw_b.kill().await;
            gw_b = match start_gateway_and_wait("Gateway-B", &gateway_bin, ws_b.path(), &NODES[1])
                .await
            {
                Ok(g) => g,
                Err(e) => return fail("T29", format!("B echo restart failed: {e}")),
            };
            let created = match ws_api_request(
                &mut ws,
                "board",
                "issue.create",
                json!({
                    "title": "T29DISPATCHINJECT 派发经验注入 e2e",
                    "description": "cluster-uat T29③：派发时按 t29auth 关键词注入团队经验。",
                    "acceptance_criteria": "回显包含经验段。",
                }),
                15,
            )
            .await
            {
                Ok(v) => v,
                Err(e) => return fail("T29", format!("issue.create(dispatch) failed: {e}")),
            };
            let inject_issue = created
                .pointer("/issue/id")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            if inject_issue == 0 {
                return fail("T29", format!("issue.create(dispatch) 无 id: {created}"));
            }
            if let Err(e) = ws_api_request(
                &mut ws,
                "board",
                "issue.dispatch",
                json!({ "id": inject_issue, "target": "Node-B" }),
                30,
            )
            .await
            {
                return fail("T29", format!("issue.dispatch(inject) failed: {e}"));
            }
            // 轮询 ≤300s 等 echo 写回（testai-3.1 终端节点回显整个 prompt）。
            let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
            let mut echo_text = String::new();
            loop {
                if tokio::time::Instant::now() >= deadline {
                    return fail(
                        "T29",
                        format!("300s 内未看到经验注入回显（最后: {}）", trunc(&echo_text, 300)),
                    );
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
                echo_text = comments_text(&mut ws, inject_issue).await;
                if echo_text.contains("团队过往经验") && echo_text.contains("T29 经验锚点") {
                    break;
                }
            }
            // 注入计数：只认实际发车的派发（planner meta 注入不计数）。
            let used = board_store
                .list_team_memory(Some("t29auth"), false)
                .ok()
                .and_then(|v| v.into_iter().next())
                .map(|e| e.use_count)
                .unwrap_or(-1);
            if used != 1 {
                return fail(
                    "T29",
                    format!("派发注入后 use_count 应 1，实际 {used}"),
                );
            }
            pass(
                "T29",
                format!(
                    "集体记忆 OK：①蒸馏入库（issue {distill_issue}，scope=t29auth/pitfall/锚点在）；\
                     ②planner 注入探针 T29PLANEXP 命中（issue {plan_issue}）；\
                     ③派发注入回显 + use_count=1（issue {inject_issue}）"
                ),
            )
        })
        .await,
    );

    // T30: 锚点双检 e2e（全自动流转 P2 B1 + P1 拓扑硬闸）。
    //
    // 三段（模型均显式切换，不依赖前序残留态）：
    //   ①全过正流：A/B 切 testai-board-1.0（planner/review/讨论组合桩，
    //     完全复用 planner-1.0/review-1.0 的全部锚点分支）→ 父单标题带
    //     <PLAN_REANCHOR>（planner 按标记产出**纯 re: 型**锚点子任务 AC
    //     ——P1 拓扑硬闸拒绝 file: 锚点远端派发，re: 型对 B 端 delivery
    //     文本实核、跨节点安全）→ swarm_plan_and_confirm 人工 confirm
    //     （auto_confirm 必须为 false，否则 plan 被自动消费）→ 依赖链
    //     0→1→2 逐个派发：B 端 worker 固定汇报文本交付 → A 端验收锚点
    //     先于 LLM：re: 锚点对 B 端 delivery 命中 → 全过进语义（review
    //     桩 PASS）→ auto_accept 收货 → 父单 auto_close 收口。断言
    //     per-anchor PASS 摘要评论（计数 + 交付文本命中明细）、零 FAIL
    //     评论、全家桶 done。
    //   ②拓扑硬闸契约：file: 锚点 + 远端目标 = issue.dispatch 诚实拒绝
    //     （⛔ 拒绝派发），不产生派发记录、issue 保持 backlog。
    //   ③FAIL 短路：re: 型不可能命中的锚点（FAIL needle）单直接 dispatch
    //     → 验收锚点确定性短路 FAIL（评论含失败锚点原文，不进 LLM）→
    //     重派×2 预算耗尽 → 转人工保持 in_review（同 T28① 的处置链，
    //     触发源换成客观锚点）。
    all_results.push(
        run_test("T30: 锚点双检 e2e：全过正流 + FAIL 短路重派（P2 B1）", || async {
            // 0. A 切组合模型 + 重启。
            let out = ws_a
                .run_cli(
                    &gateway_bin,
                    &[
                        "model",
                        "add",
                        "--model",
                        "test/testai-board-1.0",
                        "--base",
                        &format!("http://127.0.0.1:{}/v1", ai_server_port()),
                        "--key",
                        "test-key",
                        "--default",
                    ],
                )
                .await;
            if !out.success() {
                return fail("T30", format!("A board model add failed: {}", out.stderr));
            }
            gw_a.kill().await;
            gw_a = match start_gateway_and_wait("Gateway-A", &gateway_bin, ws_a.path(), &NODES[0])
                .await
            {
                Ok(g) => g,
                Err(e) => return fail("T30", format!("A restart failed: {e}")),
            };
            // B 同款（T29③ 把 B 停在 testai-3.1 回声桩上，必须显式切走）。
            let out = ws_b
                .run_cli(
                    &gateway_bin,
                    &[
                        "model",
                        "add",
                        "--model",
                        "test/testai-board-1.0",
                        "--base",
                        &format!("http://127.0.0.1:{}/v1", ai_server_port()),
                        "--key",
                        "test-key",
                        "--default",
                    ],
                )
                .await;
            if !out.success() {
                return fail("T30", format!("B board model add failed: {}", out.stderr));
            }
            gw_b.kill().await;
            gw_b = match start_gateway_and_wait("Gateway-B", &gateway_bin, ws_b.path(), &NODES[1])
                .await
            {
                Ok(g) => g,
                Err(e) => return fail("T30", format!("B restart failed: {e}")),
            };

            let mut ws = match ws_connect_gateway(NODES[0].web_port).await {
                Ok(s) => s,
                Err(e) => return fail("T30", format!("WS connect to A failed: {e}")),
            };
            // 派发计数权威证据（同 T28：BUSY 瞬态按 0 计，终态断言重试）。
            let db_path = ws_a.home().join("workspace").join("board").join("board.db");
            let board_store = match nemesis_board::BoardStore::open(&db_path, "NB") {
                Ok(s) => s,
                Err(e) => return fail("T30", format!("open board.db: {e}")),
            };
            let dispatch_count = |id: i64| -> usize {
                board_store
                    .list_dispatches(id)
                    .map(|v| v.len())
                    .unwrap_or(0)
            };
            async fn comments_text(ws: &mut WsStream, issue_id: i64) -> String {
                match ws_api_request(
                    ws,
                    "board",
                    "comment.list",
                    json!({ "issue_id": issue_id }),
                    10,
                )
                .await
                {
                    Ok(v) => v
                        .get("comments")
                        .and_then(|c| c.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|c| c.get("content").and_then(|v| v.as_str()))
                                .collect::<Vec<_>>()
                                .join("\n---\n")
                        })
                        .unwrap_or_default(),
                    Err(_) => String::new(),
                }
            }

            // 0.5 开关：auto_accept/auto_close 开、auto_confirm 关（①走人工
            // confirm 两段式；swarm_plan_and_confirm 的显式 confirm 会被
            // auto_confirm 抢消费）。
            for (key, value) in [
                ("plan.auto_confirm", json!(false)),
                ("auto_accept", json!(true)),
                ("auto_close_parent", json!(true)),
                ("unlimited_mode", json!(false)),
            ] {
                if let Err(e) = ws_api_request(
                    &mut ws,
                    "board",
                    "config.set",
                    json!({ "key": key, "value": value }),
                    10,
                )
                .await
                {
                    return fail("T30", format!("config.set({key}) failed: {e}"));
                }
            }

            // ① 全纯 re: 锚点：无需预置 workspace 锚点文件（P1 拓扑硬闸
            //    下 file: 锚点本就不可远端派发）。

            // ---- ① 全过正流：<PLAN_REANCHOR> 计划 → 依赖链 → 锚点全过 → 收口 ----
            let (parent_id, children, _confirmed) =
                match swarm_plan_and_confirm(&mut ws, NODES[0].web_port, "<PLAN_REANCHOR>", 60).await
                {
                    Ok(v) => v,
                    Err(e) => return fail("T30", format!("① plan/confirm failed: {e}")),
                };
            // 等全家桶 done（3 子单 done + 父单 done）。
            let deadline = tokio::time::Instant::now() + Duration::from_secs(600);
            loop {
                let list = ws_api_request(&mut ws, "board", "issue.list", json!({}), 10)
                    .await
                    .ok();
                let subs_done = list
                    .as_ref()
                    .and_then(|d| d.get("issues"))
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        let subs: Vec<&Value> = arr
                            .iter()
                            .filter(|i| {
                                i.get("parent_issue_id").and_then(|v| v.as_i64())
                                    == Some(parent_id)
                            })
                            .collect();
                        let st = |i: &Value| -> bool {
                            i.get("status").and_then(|v| v.as_str()) == Some("done")
                        };
                        subs.len() == 3
                            && subs.iter().all(|i| st(i))
                            && arr
                                .iter()
                                .any(|i| {
                                    i.get("id").and_then(|v| v.as_i64()) == Some(parent_id)
                                        && st(i)
                                })
                    })
                    .unwrap_or(false);
                if subs_done {
                    break;
                }
                if tokio::time::Instant::now() >= deadline {
                    let pst = issue_status_of(&mut ws, parent_id).await.unwrap_or_default();
                    return fail(
                        "T30",
                        format!(
                            "①600s 内全家桶未收口（parent='{pst}'）——锚点正流链未走通"
                        ),
                    );
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
            // per-anchor PASS 摘要评论 + 交付文本锚点对 B 端 delivery 命中。
            // REANCHOR 计划锚点数：子单1 = 2 条（集群协作状态正常 + 收到），
            // 子单2/3 各 1 条。
            for (idx, cid) in children.iter().enumerate() {
                let t = comments_text(&mut ws, *cid).await;
                if t.contains("客观锚点检查失败") {
                    return fail("T30", format!("①子单{} 不应有锚点 FAIL 评论", cid));
                }
                let expected = if idx == 0 { "（2 条）" } else { "（1 条）" };
                if !t.contains("客观锚点检查通过")
                    || !t.contains(expected)
                {
                    return fail(
                        "T30",
                        format!("①子单{} 缺 PASS 摘要（{expected}）: {}", cid, trunc(&t, 300)),
                    );
                }
                if idx == 0
                    && !t.contains("交付文本命中 /集群协作状态正常/")
                {
                    return fail(
                        "T30",
                        format!("①子单1 交付文本锚点未对 B 端 delivery 命中: {}", trunc(&t, 300)),
                    );
                }
            }

            // ---- ② 拓扑硬闸契约：file: 锚点 + 远端目标 = 诚实拒绝 ----
            let created = match ws_api_request(
                &mut ws,
                "board",
                "issue.create",
                json!({
                    "title": "T30GATE 拓扑硬闸 e2e",
                    "description": "cluster-uat T30②：file: 锚点远端派发必须被 P1 硬闸拒绝。",
                    "acceptance_criteria": "交付说明文本。\n[CHECK] file:uat-t2/e2e-missing.md exists\n[CHECK] re:集群协作状态正常",
                }),
                15,
            )
            .await
            {
                Ok(v) => v,
                Err(e) => return fail("T30", format!("②issue.create failed: {e}")),
            };
            let gate_issue = created
                .pointer("/issue/id")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            if gate_issue == 0 {
                return fail("T30", format!("②issue.create 无 id: {created}"));
            }
            match ws_api_request(
                &mut ws,
                "board",
                "issue.dispatch",
                json!({ "id": gate_issue, "target": "Node-B" }),
                30,
            )
            .await
            {
                Ok(v) => {
                    return fail("T30", format!("②file: 锚点远端派发应被拒绝，实际成功: {v}"));
                }
                Err(e) => {
                    let msg = e.to_string();
                    if !msg.contains("拒绝派发") || !msg.contains("file:") {
                        return fail("T30", format!("②拒绝理由应携带硬闸语义: {msg}"));
                    }
                }
            }
            if dispatch_count(gate_issue) != 0 {
                return fail(
                    "T30",
                    format!("②被拒派发不得产生派发记录，实际 {} 条", dispatch_count(gate_issue)),
                );
            }
            let gate_status = match issue_status_of(&mut ws, gate_issue).await {
                Ok(s) => s,
                Err(e) => return fail("T30", format!("②issue.get failed: {e}")),
            };
            if gate_status != "backlog" {
                return fail(
                    "T30",
                    format!("②被拒后 issue 应保持 backlog，实际 '{gate_status}'"),
                );
            }

            // ---- ③ FAIL 短路：re: 型不可能命中的锚点 → 重派×2 → 预算耗尽转人工 ----
            let created = match ws_api_request(
                &mut ws,
                "board",
                "issue.create",
                json!({
                    "title": "T30ANCHORFAIL 锚点短路 e2e",
                    "description": "cluster-uat T30③：交付文本锚点必不命中，验收先锚点短路 FAIL。",
                    "acceptance_criteria": "交付说明文本。\n[CHECK] re:UAT30FAILNEEDLE",
                }),
                15,
            )
            .await
            {
                Ok(v) => v,
                Err(e) => return fail("T30", format!("③issue.create failed: {e}")),
            };
            let fail_issue = created
                .pointer("/issue/id")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            if fail_issue == 0 {
                return fail("T30", format!("③issue.create 无 id: {created}"));
            }
            if let Err(e) = ws_api_request(
                &mut ws,
                "board",
                "issue.dispatch",
                json!({ "id": fail_issue, "target": "Node-B" }),
                30,
            )
            .await
            {
                return fail("T30", format!("③issue.dispatch failed: {e}"));
            }
            // 短路 FAIL 评论（含失败锚点原文）→ 预算耗尽转人工。
            let deadline = tokio::time::Instant::now() + Duration::from_secs(600);
            loop {
                if tokio::time::Instant::now() >= deadline {
                    let n = dispatch_count(fail_issue);
                    let st = issue_status_of(&mut ws, fail_issue).await.unwrap_or_default();
                    return fail(
                        "T30",
                        format!("③600s 内未收口（dispatch={n}, status='{st}'）——锚点短路或重派链未走通"),
                    );
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
                let t = comments_text(&mut ws, fail_issue).await;
                if t.contains("重派预算已耗尽") {
                    if !t.contains("客观锚点检查失败")
                        || !t.contains("UAT30FAILNEEDLE")
                    {
                        return fail(
                            "T30",
                            format!("③转人工评论应携带锚点失败明细: {}", trunc(&t, 400)),
                        );
                    }
                    break;
                }
            }
            let mut n = dispatch_count(fail_issue);
            for _ in 0..5 {
                if n == 3 { break; }
                tokio::time::sleep(Duration::from_secs(1)).await;
                n = dispatch_count(fail_issue);
            }
            let status = match issue_status_of(&mut ws, fail_issue).await {
                Ok(s) => s,
                Err(e) => return fail("T30", format!("③issue.get failed: {e}")),
            };
            if n != 3 {
                return fail("T30", format!("③应重派至 3 次派发（1+2），实际 {n}"));
            }
            if status != "in_review" {
                return fail("T30", format!("③预算耗尽后应保持 in_review，实际 '{status}'"));
            }

            // 开关复位（本测试是套件末位，防御性恢复默认态）。
            for (key, value) in [
                ("plan.auto_confirm", json!(false)),
                ("auto_accept", json!(false)),
                ("auto_close_parent", json!(false)),
                ("unlimited_mode", json!(false)),
            ] {
                let _ = ws_api_request(
                    &mut ws,
                    "board",
                    "config.set",
                    json!({ "key": key, "value": value }),
                    10,
                )
                .await;
            }
            pass(
                "T30",
                format!(
                    "锚点双检 OK：①<PLAN_REANCHOR> 三子单链纯 re: 锚点全过（交付文本锚点命中 B 端 delivery）\
                     → 语义 → 全家桶 done（parent {parent_id}）；\
                     ②file: 锚点远端派发被拓扑硬闸拒绝（issue {gate_issue} 保持 backlog）；\
                     ③re: FAIL 短路 → 重派×2 → 预算耗尽转人工（issue {fail_issue}）"
                ),
            )
        })
        .await,
    );

    // ==================================================================
    // T31: autopilot auto_plan 建单→自动拆解→发车 e2e（全自动流转 P3 A1/D2）
    // ==================================================================
    //
    // 前置：T30 已把 A/B 切到 testai-board-1.0（planner/review/讨论组合桩）。
    // 流程：auto_confirm=true → 建 auto_plan=true 规则（target 空=互斥约束）
    // → WSAPI autopilot.run（D2 路径：带 moderator 槽/home/hub/集群的
    // AutoPlanContext）→ 模板建父单 → 后台 plan 链（planner 桩出 3 子任务）
    // → auto_confirm 自动确认 → 派发波匹配器选节点（B 在线）→ 子单派发。
    // 断言：run 返回 auto_plan.status=planning；父单 3 子单落库；子单状态
    // 推进到 in_progress/in_review/done（派发+执行证据）。auto_accept 须同开：
    // 依赖闸只在子单 done 后放行后续——auto_accept=false 时子0 验收 PASS 停
    // in_review（人工验收池），子1/子2 永远等不到补派（T31 首跑实测假红）；
    // 同开后链式推进 0→1→2 全部 done。
    // 父单有 auto_confirm 发车系统评论。
    all_results.push(
        run_test(
            "T31: autopilot auto_plan 建单→自动拆解→发车（P3 A1/D2）",
            || async {
                let mut ws = match ws_connect_gateway(NODES[0].web_port).await {
                    Ok(s) => s,
                    Err(e) => return fail("T31", format!("WS connect to A failed: {}", e)),
                };

                // 0. auto_confirm 开（拆解后自动发车）+ auto_accept 开（验收
                //    PASS 自动 done，依赖闸放行后续子单——全自动链必需）。
                for (key, value) in [
                    ("plan.auto_confirm", json!(true)),
                    ("auto_accept", json!(true)),
                ] {
                    if let Err(e) = ws_api_request(
                        &mut ws,
                        "board",
                        "config.set",
                        json!({ "key": key, "value": value }),
                        10,
                    )
                    .await
                    {
                        return fail("T31", format!("config.set({key}) failed: {e}"));
                    }
                }

                // 1. 建 auto_plan 规则（cron 挑远期凌晨，测试窗内不会被 cron 误触；
                //    触发走显式 autopilot.run——确定性）。
                let marker = "T31AUTOPLAN";
                let created = match ws_api_request(
                    &mut ws,
                    "board",
                    "autopilot.create",
                    json!({
                        "name": "uat-autopilot-t31",
                        "cron": "0 3 * * *",
                        "title": format!("{} 周期单 {{date}}", marker),
                        "description": "cluster-uat T31 auto_plan 建单自动拆解发车",
                        "target": "",
                        "auto_plan": true,
                    }),
                    15,
                )
                .await
                {
                    Ok(v) => v,
                    Err(e) => return fail("T31", format!("autopilot.create failed: {e}")),
                };
                let ap_id = created
                    .pointer("/autopilot/id")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                if ap_id == 0 {
                    return fail("T31", format!("autopilot.create 无 id: {created}"));
                }
                if created
                    .pointer("/autopilot/auto_plan")
                    .and_then(|v| v.as_bool())
                    != Some(true)
                {
                    return fail(
                        "T31",
                        format!(
                            "autopilot.auto_plan 未落库（应 true）: {}",
                            trunc(&created.to_string(), 200)
                        ),
                    );
                }

                // 2. 显式 run：返回应带 auto_plan.status=planning（moderator 槽在
                //    A 主 agent 装配后已填）。
                let run_out = match ws_api_request(
                    &mut ws,
                    "board",
                    "autopilot.run",
                    json!({ "id": ap_id }),
                    15,
                )
                .await
                {
                    Ok(v) => v,
                    Err(e) => return fail("T31", format!("autopilot.run failed: {e}")),
                };
                let parent_id = run_out
                    .pointer("/issue_id")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                if parent_id == 0 {
                    return fail("T31", format!("autopilot.run 未建单: {run_out}"));
                }
                if run_out
                    .pointer("/auto_plan/status")
                    .and_then(|v| v.as_str())
                    != Some("planning")
                {
                    return fail(
                        "T31",
                        format!(
                            "auto_plan 应 planning（槽空会 skipped，说明上下文没接通）: {}",
                            trunc(&run_out.to_string(), 240)
                        ),
                    );
                }

                // 3. 轮询 ≤300s：3 子单落库 + 状态推进（派发+执行证据）。
                //    auto_accept 同开 → 依赖链 0→1→2 依次 done；轮询条件覆盖
                //    中间态（in_progress/in_review）与终态（done）。
                let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
                let mut subs_state: String;
                loop {
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    let list = ws_api_request(&mut ws, "board", "issue.list", json!({}), 10)
                        .await
                        .ok();
                    let subs: Vec<Value> = list
                        .as_ref()
                        .and_then(|d| d.get("issues"))
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter(|i| {
                                    i.get("parent_issue_id").and_then(|v| v.as_i64())
                                        == Some(parent_id)
                                })
                                .cloned()
                                .collect()
                        })
                        .unwrap_or_default();
                    subs_state = subs
                        .iter()
                        .map(|i| {
                            format!(
                                "#{}:{}",
                                i.get("id").and_then(|v| v.as_i64()).unwrap_or(0),
                                i.get("status").and_then(|v| v.as_str()).unwrap_or("?")
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(" ");
                    let all_dispatched = subs.len() == 3
                        && subs.iter().all(|i| {
                            matches!(
                                i.get("status").and_then(|v| v.as_str()),
                                Some("in_progress") | Some("in_review") | Some("done")
                            )
                        });
                    if all_dispatched {
                        break;
                    }
                    if tokio::time::Instant::now() >= deadline {
                        return fail(
                            "T31",
                            format!(
                                "300s 内 3 子单未发车（当前 [{subs_state}]）——plan 链或派发波断"
                            ),
                        );
                    }
                }

                // 4. 父单 auto_confirm 发车系统评论。
                let comments = ws_api_request(
                    &mut ws,
                    "board",
                    "comment.list",
                    json!({ "issue_id": parent_id }),
                    10,
                )
                .await
                .ok()
                .and_then(|v| v.get("comments").and_then(|c| c.as_array()).cloned())
                .unwrap_or_default();
                let ctext: String = comments
                    .iter()
                    .filter_map(|c| c.get("content").and_then(|v| v.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n");
                if !ctext.contains("auto_confirm") {
                    return fail(
                        "T31",
                        format!("父单缺 auto_confirm 发车系统评论: {}", trunc(&ctext, 240)),
                    );
                }

                // 5. 清理：删规则（防后续窗内 cron 误触）+ 取消全家 + 复位开关。
                let _ = ws_api_request(
                    &mut ws,
                    "board",
                    "autopilot.remove",
                    json!({ "id": ap_id }),
                    10,
                )
                .await;
                let list = ws_api_request(&mut ws, "board", "issue.list", json!({}), 10)
                    .await
                    .ok();
                let ids: Vec<i64> = list
                    .as_ref()
                    .and_then(|d| d.get("issues"))
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|i| {
                                let pid = i.get("parent_issue_id").and_then(|v| v.as_i64());
                                let id = i.get("id").and_then(|v| v.as_i64())?;
                                if pid == Some(parent_id) || id == parent_id {
                                    Some(id)
                                } else {
                                    None
                                }
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                for id in ids {
                    let _ =
                        ws_api_request(&mut ws, "board", "issue.cancel", json!({ "id": id }), 10)
                            .await;
                }
                for (key, value) in [
                    ("plan.auto_confirm", json!(false)),
                    ("auto_accept", json!(false)),
                ] {
                    if let Err(e) = ws_api_request(
                        &mut ws,
                        "board",
                        "config.set",
                        json!({ "key": key, "value": value }),
                        10,
                    )
                    .await
                    {
                        return fail(
                            "T31",
                            format!("config.set({key}) 复位失败（污染后续）: {e}"),
                        );
                    }
                }

                pass(
                    "T31",
                    format!(
                        "auto_plan OK：规则 {ap_id} run 建父单 {parent_id} → planning → \
                     3 子单发车（[{subs_state}]）→ auto_confirm 发车评论在"
                    ),
                )
            },
        )
        .await,
    );

    // ==================================================================
    // T32: 建项目即启动 + 项目状态机 e2e（全自动流转 P3 F1/F2 = UAT-T3-4/T3-5）
    // ==================================================================
    //
    // 前置：T31 后 A/B 仍在 testai-board-1.0，开关已复位。
    // 流程：auto_confirm+auto_accept 同开（T31 教训：依赖闸要 done）→
    // project.create（auto_start=true + 验收标准）→ 断言父单即时建出
    // （auto_start.issue_id，project_id 绑定）且项目 active → 非法转移
    // active→completed 被 project.update loud 拒绝（F2 转移表）→ plan 链
    // 自动发车（3 子单派 B）→ 首派成功联动项目 active→in_progress →
    // 依赖链 0→1→2 全 done → 项目保持 in_progress（completed 只归 F3/P4
    // 项目级收口，P3 不越级自动 completed）→ 清理（archived 合法转移）。
    all_results.push(
        run_test(
            "T32: 建项目即启动 + 项目状态机（P3 F1/F2）",
            || async {
                let mut ws = match ws_connect_gateway(NODES[0].web_port).await {
                    Ok(s) => s,
                    Err(e) => return fail("T32", format!("WS connect to A failed: {}", e)),
                };

                // 0. 开关同 T31：auto_confirm（拆解自动发车）+ auto_accept
                //    （验收 done 放行依赖闸）。
                for (key, value) in [
                    ("plan.auto_confirm", json!(true)),
                    ("auto_accept", json!(true)),
                ] {
                    if let Err(e) = ws_api_request(
                        &mut ws,
                        "board",
                        "config.set",
                        json!({ "key": key, "value": value }),
                        10,
                    )
                    .await
                    {
                        return fail("T32", format!("config.set({key}) failed: {e}"));
                    }
                }

                // 1. 建项目即启动：父单应即时返回且项目 active。
                let created = match ws_api_request(
                    &mut ws,
                    "board",
                    "project.create",
                    json!({
                        "name": "T32AUTOSTART 项目",
                        "description": "cluster-uat T32 建项目自动启动 e2e",
                        "acceptance_criteria": "全部父单完成。",
                        "auto_start": true,
                    }),
                    15,
                )
                .await
                {
                    Ok(v) => v,
                    Err(e) => return fail("T32", format!("project.create failed: {e}")),
                };
                let pid = created
                    .pointer("/project/id")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                if pid == 0 {
                    return fail("T32", format!("project.create 无 id: {created}"));
                }
                if created.pointer("/project/status").and_then(|v| v.as_str()) != Some("active") {
                    return fail(
                        "T32",
                        format!("新项目应 active: {}", trunc(&created.to_string(), 200)),
                    );
                }
                let parent_id = created
                    .pointer("/auto_start/issue_id")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                if parent_id == 0 {
                    return fail(
                        "T32",
                        format!("auto_start 未建父单: {}", trunc(&created.to_string(), 200)),
                    );
                }

                // 2. 非法转移 live 拒绝：active→completed 必须被 loud 拒
                //    （F2 转移表只放行 active→in_progress/archived）。
                if let Ok(v) = ws_api_request(
                    &mut ws,
                    "board",
                    "project.update",
                    json!({ "id": pid, "status": "completed" }),
                    10,
                )
                .await
                {
                    return fail(
                        "T32",
                        format!("active→completed 应被拒绝却成功: {}", trunc(&v.to_string(), 200)),
                    );
                }

                // 3. 轮询 ≤300s：父单下 3 子单发车 + 项目 active→in_progress
                //    （F2 首派联动）→ 依赖链推进全 done。
                let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
                let mut subs_state: String;
                let mut proj_status: String;
                loop {
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    let list = ws_api_request(&mut ws, "board", "issue.list", json!({}), 10)
                        .await
                        .ok();
                    let subs: Vec<Value> = list
                        .as_ref()
                        .and_then(|d| d.get("issues"))
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter(|i| {
                                    i.get("parent_issue_id").and_then(|v| v.as_i64())
                                        == Some(parent_id)
                                })
                                .cloned()
                                .collect()
                        })
                        .unwrap_or_default();
                    subs_state = subs
                        .iter()
                        .map(|i| {
                            format!(
                                "#{}:{}",
                                i.get("id").and_then(|v| v.as_i64()).unwrap_or(0),
                                i.get("status").and_then(|v| v.as_str()).unwrap_or("?")
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(" ");
                    proj_status = ws_api_request(&mut ws, "board", "project.list", json!({}), 10)
                        .await
                        .ok()
                        .and_then(|v| {
                            v.get("projects")
                                .and_then(|p| p.as_array())
                                .and_then(|arr| {
                                    arr.iter().find(|p| {
                                        p.get("id").and_then(|v| v.as_i64()) == Some(pid)
                                    })
                                })
                                .and_then(|p| {
                                    p.get("status").and_then(|v| v.as_str()).map(String::from)
                                })
                        })
                        .unwrap_or_else(|| "?".to_string());
                    let all_done = subs.len() == 3
                        && subs.iter().all(|i| {
                            i.get("status").and_then(|v| v.as_str()) == Some("done")
                        });
                    let proj_advanced = proj_status == "in_progress" || proj_status == "completed";
                    if all_done && proj_advanced {
                        break;
                    }
                    if tokio::time::Instant::now() >= deadline {
                        return fail(
                            "T32",
                            format!(
                                "300s 内项目链未走完（subs [{subs_state}] project={proj_status}）\
                                 ——auto_start/发车/状态联动断"
                            ),
                        );
                    }
                }
                // completed 只归 F3/P4 项目级收口：P3 阶段全 done 后必须仍
                // in_progress（自动越级 completed = 误判）。
                if proj_status != "in_progress" {
                    return fail(
                        "T32",
                        format!(
                            "全 done 后项目应保持 in_progress（completed 归 P4/F3），实际 {proj_status}"
                        ),
                    );
                }

                // 4. 非法转移再验：in_progress→active 拒绝（合法出边只有
                //    completed/archived）。
                if let Ok(v) = ws_api_request(
                    &mut ws,
                    "board",
                    "project.update",
                    json!({ "id": pid, "status": "active" }),
                    10,
                )
                .await
                {
                    return fail(
                        "T32",
                        format!("in_progress→active 应被拒绝却成功: {}", trunc(&v.to_string(), 200)),
                    );
                }

                // 5. 清理：取消全家 + 项目归档（in_progress→archived 合法）
                //    + 复位开关。
                let list = ws_api_request(&mut ws, "board", "issue.list", json!({}), 10)
                    .await
                    .ok();
                let mut cancel_ids: Vec<i64> = list
                    .as_ref()
                    .and_then(|d| d.get("issues"))
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|i| {
                                let pp = i.get("parent_issue_id").and_then(|v| v.as_i64());
                                let id = i.get("id").and_then(|v| v.as_i64())?;
                                if pp == Some(parent_id) || id == parent_id {
                                    Some(id)
                                } else {
                                    None
                                }
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                cancel_ids.sort_unstable();
                cancel_ids.dedup();
                for id in cancel_ids {
                    let _ =
                        ws_api_request(&mut ws, "board", "issue.cancel", json!({ "id": id }), 10)
                            .await;
                }
                let _ = ws_api_request(
                    &mut ws,
                    "board",
                    "project.update",
                    json!({ "id": pid, "status": "archived" }),
                    10,
                )
                .await;
                for (key, value) in [
                    ("plan.auto_confirm", json!(false)),
                    ("auto_accept", json!(false)),
                ] {
                    if let Err(e) = ws_api_request(
                        &mut ws,
                        "board",
                        "config.set",
                        json!({ "key": key, "value": value }),
                        10,
                    )
                    .await
                    {
                        return fail(
                            "T32",
                            format!("config.set({key}) 复位失败（污染后续）: {e}"),
                        );
                    }
                }

                pass(
                    "T32",
                    format!(
                        "auto_start OK：项目 {pid} 建父单 {parent_id} → 非法转移两次拒绝 → \
                     3 子单发车全 done（[{subs_state}]）→ 项目 active→in_progress 联动 → \
                     归档收尾"
                    ),
                )
            },
        )
        .await,
    );

    // ==================================================================
    // T33: 验收取证二段验收 e2e（全自动流转 P4 B2b）
    // ==================================================================
    //
    // 前置：T30 起 A/B 均在 testai-board-1.0（planner/review/master 组合
    // 桩），开关已被 T32 复位。
    // 流程：review.selfcheck + auto_accept 同开 → 建单（AC 带
    // <REVIEW_NEED_EVIDENCE>）派 Node-B → B 固定汇报 → 自动验收一段判
    // UNSURE + need_evidence → 评审向 B 发一轮取证（[取证请求
    // board_selfcheck: 前缀落在 B 既有 board:{n} 会话）→ B 桩回固定取证
    // 文本（含 <SELFCHK_EVIDENCE_OK>）→ 回调经 SelfcheckRegistry 路由回
    // issue → 二段验收证据命中 PASS → auto_accept 自动收货 done。
    // 断言：⏳ 验收暂缓 + 自动收货两条评论、终态 done、派发数恒 1
    //（取证走 peer_chat 不产生派发行——取证链路不得被计成重派）。
    all_results.push(
        run_test("T33: 验收取证二段验收 e2e（P4 B2b）", || async {
            let mut ws = match ws_connect_gateway(NODES[0].web_port).await {
                Ok(s) => s,
                Err(e) => return fail("T33", format!("WS connect to A failed: {e}")),
            };
            for (key, value) in [
                ("review.selfcheck", json!(true)),
                ("auto_accept", json!(true)),
                ("unlimited_mode", json!(false)),
            ] {
                if let Err(e) = ws_api_request(
                    &mut ws,
                    "board",
                    "config.set",
                    json!({ "key": key, "value": value }),
                    10,
                )
                .await
                {
                    return fail("T33", format!("config.set({key}) failed: {e}"));
                }
            }
            // 派发计数权威证据（同 T30：直接读 A 侧 board.db）。
            let db_path = ws_a.home().join("workspace").join("board").join("board.db");
            let board_store = match nemesis_board::BoardStore::open(&db_path, "NB") {
                Ok(s) => s,
                Err(e) => return fail("T33", format!("open board.db: {e}")),
            };
            async fn comments_text(ws: &mut WsStream, issue_id: i64) -> String {
                match ws_api_request(
                    ws,
                    "board",
                    "comment.list",
                    json!({ "issue_id": issue_id }),
                    10,
                )
                .await
                {
                    Ok(v) => v
                        .get("comments")
                        .and_then(|c| c.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|c| c.get("content").and_then(|v| v.as_str()))
                                .collect::<Vec<_>>()
                                .join("\n---\n")
                        })
                        .unwrap_or_default(),
                    Err(_) => String::new(),
                }
            }

            let created = match ws_api_request(
                &mut ws,
                "board",
                "issue.create",
                json!({
                    "title": "T33SELFCHECK 验收取证 e2e",
                    "description": "cluster-uat T33：验收证据不足 → 向执行节点取证 → 二段验收。",
                    "acceptance_criteria": "交付说明文本。\n<REVIEW_NEED_EVIDENCE>",
                }),
                15,
            )
            .await
            {
                Ok(v) => v,
                Err(e) => return fail("T33", format!("issue.create failed: {e}")),
            };
            let sc_issue = created
                .pointer("/issue/id")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            if sc_issue == 0 {
                return fail("T33", format!("issue.create 无 id: {created}"));
            }
            if let Err(e) = ws_api_request(
                &mut ws,
                "board",
                "issue.dispatch",
                json!({ "id": sc_issue, "target": "Node-B" }),
                30,
            )
            .await
            {
                return fail("T33", format!("issue.dispatch failed: {e}"));
            }

            // 轮询 ≤600s：⏳ 挂起评论落 → B 取证回报 → 二段 PASS → 收货 done。
            let deadline = tokio::time::Instant::now() + Duration::from_secs(600);
            let (text, n_dispatch) = loop {
                if tokio::time::Instant::now() >= deadline {
                    let st = issue_status_of(&mut ws, sc_issue).await.unwrap_or_default();
                    let n = board_store
                        .list_dispatches(sc_issue)
                        .map(|v| v.len())
                        .unwrap_or(0);
                    return fail(
                        "T33",
                        format!("600s 内取证二段链未收口（status='{st}', dispatch={n}）——取证/二段/收货链断"),
                    );
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
                let text = comments_text(&mut ws, sc_issue).await;
                if !text.contains("⏳ 验收暂缓（board.review.selfcheck）") {
                    continue; // 一段 UNSURE 挂起还没落
                }
                let status = issue_status_of(&mut ws, sc_issue).await.unwrap_or_default();
                if status == "done" {
                    break (
                        text,
                        board_store
                            .list_dispatches(sc_issue)
                            .map(|v| v.len())
                            .unwrap_or(0),
                    );
                }
            };
            if !text.contains("自动收货") {
                return fail(
                    "T33",
                    format!("终态 done 但缺 auto_accept 收货评论: {}", trunc(&text, 400)),
                );
            }
            if n_dispatch != 1 {
                return fail(
                    "T33",
                    format!("取证链不应产生重派（派发数应恒 1），实际 {n_dispatch}"),
                );
            }

            // 复位（T34/T35 不用 selfcheck）。
            for (key, value) in
                [("review.selfcheck", json!(false)), ("auto_accept", json!(false))]
            {
                let _ = ws_api_request(
                    &mut ws,
                    "board",
                    "config.set",
                    json!({ "key": key, "value": value }),
                    10,
                )
                .await;
            }
            pass(
                "T33",
                format!(
                    "验收取证 OK：issue {sc_issue} 一段 UNSURE+need_evidence → ⏳ 向 Node-B 取证 → \
                     证据回报（SELFCHK_EVIDENCE_OK）→ 二段 PASS → 自动收货 done（派发数恒 1）"
                ),
            )
        })
        .await,
    );

    // ==================================================================
    // T34: 连续同节点 FAIL 换节点重派 e2e（全自动流转 P4 D3）
    // ==================================================================
    //
    // 前置：A/B 在 testai-board-1.0，C/D 在 testai-3.1 回声桩（对任意输入
    // 回声——FAIL 由 AC 锚点决定，交付内容与断言无关，能触发写回即可）。
    // 流程：建单（AC 带 <REVIEW_FAIL>）派 Node-B → FAIL(第 1/2 次重派)
    // 连续=1 维持同节点重派 B → FAIL(第 2/2 次重派) 连续=2 → 换历史未用过
    // 的次优节点（C/D 择一）→ FAIL 第 3 轮重派预算(2)耗尽 → 转人工。
    // 断言：🔁 换节点评论、第 1/2 + 第 2/2 次重派评论、转人工评论、派发
    // 数=3、末次派发 worker != Node-B、状态 in_review。
    all_results.push(
        run_test("T34: 连续同节点 FAIL 换节点重派 e2e（P4 D3）", || async {
            let mut ws = match ws_connect_gateway(NODES[0].web_port).await {
                Ok(s) => s,
                Err(e) => return fail("T34", format!("WS connect to A failed: {e}")),
            };
            for (key, value) in
                [("unlimited_mode", json!(false)), ("auto_accept", json!(false))]
            {
                if let Err(e) = ws_api_request(
                    &mut ws,
                    "board",
                    "config.set",
                    json!({ "key": key, "value": value }),
                    10,
                )
                .await
                {
                    return fail("T34", format!("config.set({key}) failed: {e}"));
                }
            }
            let db_path = ws_a.home().join("workspace").join("board").join("board.db");
            let board_store = match nemesis_board::BoardStore::open(&db_path, "NB") {
                Ok(s) => s,
                Err(e) => return fail("T34", format!("open board.db: {e}")),
            };
            async fn comments_text(ws: &mut WsStream, issue_id: i64) -> String {
                match ws_api_request(
                    ws,
                    "board",
                    "comment.list",
                    json!({ "issue_id": issue_id }),
                    10,
                )
                .await
                {
                    Ok(v) => v
                        .get("comments")
                        .and_then(|c| c.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|c| c.get("content").and_then(|v| v.as_str()))
                                .collect::<Vec<_>>()
                                .join("\n---\n")
                        })
                        .unwrap_or_default(),
                    Err(_) => String::new(),
                }
            }

            let created = match ws_api_request(
                &mut ws,
                "board",
                "issue.create",
                json!({
                    "title": "T34SWITCH 换节点重派 e2e",
                    "description": "cluster-uat T34：连续同节点 FAIL → 换节点重派。",
                    "acceptance_criteria": "交付说明文本。\n<REVIEW_FAIL>",
                }),
                15,
            )
            .await
            {
                Ok(v) => v,
                Err(e) => return fail("T34", format!("issue.create failed: {e}")),
            };
            let d3_issue = created
                .pointer("/issue/id")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            if d3_issue == 0 {
                return fail("T34", format!("issue.create 无 id: {created}"));
            }
            if let Err(e) = ws_api_request(
                &mut ws,
                "board",
                "issue.dispatch",
                json!({ "id": d3_issue, "target": "Node-B" }),
                30,
            )
            .await
            {
                return fail("T34", format!("issue.dispatch failed: {e}"));
            }

            // 轮询 ≤600s：直到重派预算耗尽转人工。
            let deadline = tokio::time::Instant::now() + Duration::from_secs(600);
            let text = loop {
                if tokio::time::Instant::now() >= deadline {
                    let st = issue_status_of(&mut ws, d3_issue).await.unwrap_or_default();
                    let n = board_store
                        .list_dispatches(d3_issue)
                        .map(|v| v.len())
                        .unwrap_or(0);
                    return fail(
                        "T34",
                        format!("600s 内未走到预算耗尽转人工（status='{st}', dispatch={n}）——重派链断"),
                    );
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
                let t = comments_text(&mut ws, d3_issue).await;
                if t.contains("重派预算已耗尽") {
                    break t;
                }
            };
            if !text.contains("🔁 连续多轮未通过，本次重派换节点执行 → ") {
                return fail(
                    "T34",
                    format!("连续 2 轮 FAIL 应触发换节点评论: {}", trunc(&text, 400)),
                );
            }
            if !text.contains("第 1/2 次重派") || !text.contains("第 2/2 次重派") {
                return fail(
                    "T34",
                    format!("缺两轮 FAIL 重派评论（第 1/2 + 第 2/2）: {}", trunc(&text, 400)),
                );
            }
            if !text.contains("🤷 验收 agent 无法定案，请人工裁决") {
                return fail(
                    "T34",
                    format!("预算耗尽后应转人工: {}", trunc(&text, 400)),
                );
            }
            let dispatches = board_store.list_dispatches(d3_issue).unwrap_or_default();
            if dispatches.len() != 3 {
                return fail(
                    "T34",
                    format!("应恰好 3 次派发（1 首派 + 2 重派），实际 {}", dispatches.len()),
                );
            }
            let last_worker = dispatches
                .last()
                .map(|d| d.worker_id.clone())
                .unwrap_or_default();
            if last_worker == "Node-B" {
                return fail(
                    "T34",
                    format!("连续 2 轮 FAIL 后应换节点执行，末次派发仍落在 Node-B（{last_worker}）"),
                );
            }
            let status = issue_status_of(&mut ws, d3_issue).await.unwrap_or_default();
            if status != "in_review" {
                return fail("T34", format!("转人工后应保持 in_review，实际 '{status}'"));
            }
            pass(
                "T34",
                format!(
                    "换节点重派 OK：issue {d3_issue} FAIL×2 落 Node-B（第 1/2+第 2/2 次重派）→ \
                     🔁 换节点 → {last_worker} → 第 3 轮预算耗尽转人工（派发数 3，in_review）"
                ),
            )
        })
        .await,
    );

    // ==================================================================
    // T35: 预算保险丝 e2e（全自动流转 P4 E1）
    // ==================================================================
    //
    // 两段（budget.wall_clock_budget_secs=1 秒墙钟闸贯穿）：
    //   ①默认模式熔断：建单（AC 带 <REVIEW_FAIL>）→ 建后停 2s 保证墙钟
    //     必超 → 派 Node-B → 首轮 FAIL 即超预算 → 🛑 停止自动重派转人工。
    //     断言：🛑 + 超限项 评论、派发数恒 1、状态 in_review。
    //   ②无限模式降级：unlimited_mode=true → 预算超限降级为 WARN 继续
    //     （不落 🛑 评论）→ 重派持续；用 auto_review=false 停链（已过闸的
    //     在飞轮次至多再派 1 次，按计数稳定判定）。断言：派发数 ≥3 且稳定
    //     不涨、全程无 🛑 评论。
    all_results.push(
        run_test("T35: 预算保险丝 e2e（P4 E1）", || async {
            let mut ws = match ws_connect_gateway(NODES[0].web_port).await {
                Ok(s) => s,
                Err(e) => return fail("T35", format!("WS connect to A failed: {e}")),
            };
            for (key, value) in [
                ("budget.wall_clock_budget_secs", json!(1)),
                ("unlimited_mode", json!(false)),
                ("auto_accept", json!(false)),
            ] {
                if let Err(e) = ws_api_request(
                    &mut ws,
                    "board",
                    "config.set",
                    json!({ "key": key, "value": value }),
                    10,
                )
                .await
                {
                    return fail("T35", format!("config.set({key}) failed: {e}"));
                }
            }
            let db_path = ws_a.home().join("workspace").join("board").join("board.db");
            let board_store = match nemesis_board::BoardStore::open(&db_path, "NB") {
                Ok(s) => s,
                Err(e) => return fail("T35", format!("open board.db: {e}")),
            };
            async fn comments_text(ws: &mut WsStream, issue_id: i64) -> String {
                match ws_api_request(
                    ws,
                    "board",
                    "comment.list",
                    json!({ "issue_id": issue_id }),
                    10,
                )
                .await
                {
                    Ok(v) => v
                        .get("comments")
                        .and_then(|c| c.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|c| c.get("content").and_then(|v| v.as_str()))
                                .collect::<Vec<_>>()
                                .join("\n---\n")
                        })
                        .unwrap_or_default(),
                    Err(_) => String::new(),
                }
            }
            let dispatch_count = |id: i64| -> usize {
                board_store
                    .list_dispatches(id)
                    .map(|v| v.len())
                    .unwrap_or(0)
            };

            // ---- ① 墙钟熔断（默认模式）----
            let created = match ws_api_request(
                &mut ws,
                "board",
                "issue.create",
                json!({
                    "title": "T35FUSE1 预算熔断 e2e",
                    "description": "cluster-uat T35①：墙钟预算超限停止自动重派。",
                    "acceptance_criteria": "交付说明文本。\n<REVIEW_FAIL>",
                }),
                15,
            )
            .await
            {
                Ok(v) => v,
                Err(e) => return fail("T35", format!("①issue.create failed: {e}")),
            };
            let fuse1 = created
                .pointer("/issue/id")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            if fuse1 == 0 {
                return fail("T35", format!("①issue.create 无 id: {created}"));
            }
            // 建后停 2s：保证评审时 issue 存活必超 1s 墙钟（本地桩链路可能
            // 秒级走完，不能赌传输耗时）。
            tokio::time::sleep(Duration::from_secs(2)).await;
            if let Err(e) = ws_api_request(
                &mut ws,
                "board",
                "issue.dispatch",
                json!({ "id": fuse1, "target": "Node-B" }),
                30,
            )
            .await
            {
                return fail("T35", format!("①issue.dispatch failed: {e}"));
            }
            let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
            loop {
                if tokio::time::Instant::now() >= deadline {
                    let st = issue_status_of(&mut ws, fuse1).await.unwrap_or_default();
                    return fail(
                        "T35",
                        format!("①300s 内未熔断（status='{st}', dispatch={}）——预算保险丝未生效", dispatch_count(fuse1)),
                    );
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
                let t = comments_text(&mut ws, fuse1).await;
                if t.contains("🛑 自动重派预算超限") {
                    if !t.contains("超限项：") || !t.contains("wall_clock_budget_secs") {
                        return fail(
                            "T35",
                            format!("①熔断评论应携带超限项明细（墙钟）: {}", trunc(&t, 400)),
                        );
                    }
                    break;
                }
            }
            if dispatch_count(fuse1) != 1 {
                return fail(
                    "T35",
                    format!("①熔断后不应发生重派（派发数应恒 1），实际 {}", dispatch_count(fuse1)),
                );
            }
            let status = issue_status_of(&mut ws, fuse1).await.unwrap_or_default();
            if status != "in_review" {
                return fail("T35", format!("①熔断转人工应保持 in_review，实际 '{status}'"));
            }

            // ---- ② 无限模式：超限降级 WARN 继续，auto_review=false 停链 ----
            if let Err(e) = ws_api_request(
                &mut ws,
                "board",
                "config.set",
                json!({ "key": "unlimited_mode", "value": true }),
                10,
            )
            .await
            {
                return fail("T35", format!("②config.set(unlimited_mode) failed: {e}"));
            }
            let created = match ws_api_request(
                &mut ws,
                "board",
                "issue.create",
                json!({
                    "title": "T35FUSE2 无限模式预算降级 e2e",
                    "description": "cluster-uat T35②：预算超限 WARN 继续重派，无 🛑 评论。",
                    "acceptance_criteria": "交付说明文本。\n<REVIEW_FAIL>",
                }),
                15,
            )
            .await
            {
                Ok(v) => v,
                Err(e) => return fail("T35", format!("②issue.create failed: {e}")),
            };
            let fuse2 = created
                .pointer("/issue/id")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            if fuse2 == 0 {
                return fail("T35", format!("②issue.create 无 id: {created}"));
            }
            if let Err(e) = ws_api_request(
                &mut ws,
                "board",
                "issue.dispatch",
                json!({ "id": fuse2, "target": "Node-B" }),
                30,
            )
            .await
            {
                return fail("T35", format!("②issue.dispatch failed: {e}"));
            }
            // 等重派持续发生（≥3 次派发），然后关 auto_review 停链。
            let deadline = tokio::time::Instant::now() + Duration::from_secs(600);
            loop {
                if tokio::time::Instant::now() >= deadline {
                    return fail(
                        "T35",
                        format!("②600s 内未达 3 次派发（实际 {}）——超限 WARN 继续路径断", dispatch_count(fuse2)),
                    );
                }
                if dispatch_count(fuse2) >= 3 {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
            if let Err(e) = ws_api_request(
                &mut ws,
                "board",
                "config.set",
                json!({ "key": "auto_review", "value": false }),
                10,
            )
            .await
            {
                return fail("T35", format!("②config.set(auto_review=false) failed: {e}"));
            }
            // 计数稳定判定（在飞轮次至多再派 1 次；连续两采样相等即停稳）。
            let mut stable = 0;
            for _ in 0..10 {
                tokio::time::sleep(Duration::from_secs(6)).await;
                if dispatch_count(fuse2) == stable {
                    break;
                }
                stable = dispatch_count(fuse2);
            }
            let final_n = dispatch_count(fuse2);
            if final_n < 3 {
                return fail("T35", format!("②停链后派发数应 ≥3，实际 {final_n}"));
            }
            let text = comments_text(&mut ws, fuse2).await;
            if text.contains("🛑 自动重派预算超限") {
                return fail(
                    "T35",
                    format!("②无限模式下超限应降级 WARN 继续（不落 🛑 评论）: {}", trunc(&text, 400)),
                );
            }

            // 复位（套件末位测试，防御性恢复默认态）。
            for (key, value) in [
                ("unlimited_mode", json!(false)),
                ("auto_review", json!(true)),
                ("budget.wall_clock_budget_secs", json!(0)),
            ] {
                let _ = ws_api_request(
                    &mut ws,
                    "board",
                    "config.set",
                    json!({ "key": key, "value": value }),
                    10,
                )
                .await;
            }
            pass(
                "T35",
                format!(
                    "预算保险丝 OK：①墙钟超限即熔断（issue {fuse1} 派发数恒 1，🛑+超限项，in_review）；\
                     ②无限模式超限 WARN 继续（issue {fuse2} 派发 {final_n} 次无 🛑，auto_review 停链）"
                ),
            )
        })
        .await,
    );

    // ==================================================================
    // T36: 项目级收口验收 e2e（全自动流转 P4 F3 = UAT-T4-5）
    // ==================================================================
    //
    // 两段（auto_close_project=true 贯穿；A/B 在 testai-board-1.0，验收
    // 桩按 AC 锚点出三态）：
    //   ①PASS 收口：建项目（AC 无 FAIL 锚点，auto_start=false 手工建单）→
    //     2 张顶层父单逐一 issue.status=done（backlog→done 合法转移）→
    //     末张落定时聚合触发 → 汇总验收 PASS → 项目 completed。
    //   ②FAIL 留痕：建项目（AC 带 <REVIEW_FAIL>）→ 1 张父单 done → 汇总
    //     验收 FAIL → 缺口评论落父单（🏛 未定案 + 差距）；父单保持 done
    //     （不自动重开——F3 边界）、项目保持 active（completed→in_progress
    //     回退臂单测钉死，active 本就不动）。
    all_results.push(
        run_test("T36: 项目级收口验收 e2e（P4 F3）", || async {
            let mut ws = match ws_connect_gateway(NODES[0].web_port).await {
                Ok(s) => s,
                Err(e) => return fail("T36", format!("WS connect to A failed: {e}")),
            };
            for (key, value) in [
                ("review.auto_close_project", json!(true)),
                ("unlimited_mode", json!(false)),
                ("auto_accept", json!(false)),
            ] {
                if let Err(e) = ws_api_request(
                    &mut ws,
                    "board",
                    "config.set",
                    json!({ "key": key, "value": value }),
                    10,
                )
                .await
                {
                    return fail("T36", format!("config.set({key}) failed: {e}"));
                }
            }
            async fn project_status_of(ws: &mut WsStream, pid: i64) -> String {
                ws_api_request(ws, "board", "project.list", json!({}), 10)
                    .await
                    .ok()
                    .and_then(|v| {
                        v.get("projects")
                            .and_then(|p| p.as_array())
                            .and_then(|arr| {
                                arr.iter()
                                    .find(|p| p.get("id").and_then(|v| v.as_i64()) == Some(pid))
                            })
                            .and_then(|p| p.get("status").and_then(|v| v.as_str()))
                            .map(String::from)
                    })
                    .unwrap_or_else(|| "?".to_string())
            }

            // ---- ① PASS 收口 ----
            let created = match ws_api_request(
                &mut ws,
                "board",
                "project.create",
                json!({
                    "name": "T36PASS 项目",
                    "description": "cluster-uat T36①：全部父单 done → 汇总验收 PASS → completed。",
                    "acceptance_criteria": "全部父单完成。",
                    "auto_start": false,
                }),
                15,
            )
            .await
            {
                Ok(v) => v,
                Err(e) => return fail("T36", format!("①project.create failed: {e}")),
            };
            let pid_pass = created
                .pointer("/project/id")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            if pid_pass == 0 {
                return fail("T36", format!("①project.create 无 id: {created}"));
            }
            let mut parents_pass = Vec::new();
            for k in 1..=2 {
                let created = match ws_api_request(
                    &mut ws,
                    "board",
                    "issue.create",
                    json!({
                        "title": format!("T36PASS 父单{k}"),
                        "description": "cluster-uat T36① 手工父单。",
                        "project_id": pid_pass,
                    }),
                    15,
                )
                .await
                {
                    Ok(v) => v,
                    Err(e) => return fail("T36", format!("①issue.create({k}) failed: {e}")),
                };
                let id = created
                    .pointer("/issue/id")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                if id == 0 {
                    return fail("T36", format!("①issue.create({k}) 无 id: {created}"));
                }
                parents_pass.push(id);
            }
            for id in &parents_pass {
                if let Err(e) = ws_api_request(
                    &mut ws,
                    "board",
                    "issue.status",
                    json!({ "id": id, "status": "done" }),
                    10,
                )
                .await
                {
                    return fail("T36", format!("①issue.status({id}) failed: {e}"));
                }
            }
            // 轮询 ≤300s：项目 completed（聚合触发 → 汇总 PASS）。
            let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
            loop {
                if tokio::time::Instant::now() >= deadline {
                    return fail(
                        "T36",
                        format!(
                            "①300s 内项目未 completed（实际 '{}'）——F3 聚合触发/汇总验收断",
                            project_status_of(&mut ws, pid_pass).await
                        ),
                    );
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
                if project_status_of(&mut ws, pid_pass).await == "completed" {
                    break;
                }
            }

            // ---- ② FAIL 留痕不回开 ----
            let created = match ws_api_request(
                &mut ws,
                "board",
                "project.create",
                json!({
                    "name": "T36FAIL 项目",
                    "description": "cluster-uat T36②：汇总验收 FAIL → 缺口评论，不自动重开父单。",
                    "acceptance_criteria": "交付说明文本。\n<REVIEW_FAIL>",
                    "auto_start": false,
                }),
                15,
            )
            .await
            {
                Ok(v) => v,
                Err(e) => return fail("T36", format!("②project.create failed: {e}")),
            };
            let pid_fail = created
                .pointer("/project/id")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            if pid_fail == 0 {
                return fail("T36", format!("②project.create 无 id: {created}"));
            }
            let created = match ws_api_request(
                &mut ws,
                "board",
                "issue.create",
                json!({
                    "title": "T36FAIL 父单1",
                    "description": "cluster-uat T36② 手工父单。",
                    "project_id": pid_fail,
                }),
                15,
            )
            .await
            {
                Ok(v) => v,
                Err(e) => return fail("T36", format!("②issue.create failed: {e}")),
            };
            let parent_fail = created
                .pointer("/issue/id")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            if parent_fail == 0 {
                return fail("T36", format!("②issue.create 无 id: {created}"));
            }
            if let Err(e) = ws_api_request(
                &mut ws,
                "board",
                "issue.status",
                json!({ "id": parent_fail, "status": "done" }),
                10,
            )
            .await
            {
                return fail("T36", format!("②issue.status failed: {e}"));
            }
            // 轮询 ≤300s：缺口评论（🏛 未定案 + 差距）落父单。
            let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
            loop {
                if tokio::time::Instant::now() >= deadline {
                    return fail("T36", "②300s 内缺口评论未落父单——F3 FAIL 留痕断");
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
                let got = ws_api_request(
                    &mut ws,
                    "board",
                    "comment.list",
                    json!({ "issue_id": parent_fail }),
                    10,
                )
                .await
                .ok()
                .and_then(|v| {
                    v.get("comments")
                        .and_then(|c| c.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|c| c.get("content").and_then(|v| v.as_str()))
                                .collect::<Vec<_>>()
                                .join("\n---\n")
                        })
                })
                .unwrap_or_default();
                if got.contains("收口验收未定案") {
                    if !got.contains("差距：") || !got.contains("T28 差距锚点") {
                        return fail(
                            "T36",
                            format!("②缺口评论应携带差距明细: {}", trunc(&got, 400)),
                        );
                    }
                    break;
                }
            }
            // 不自动重开：父单保持 done；项目 active 不回退（completed 才回
            // 退，回退臂单测钉死）。
            let pst = issue_status_of(&mut ws, parent_fail).await.unwrap_or_default();
            if pst != "done" {
                return fail("T36", format!("②父单应保持 done（F3 不自动重开），实际 '{pst}'"));
            }
            let proj = project_status_of(&mut ws, pid_fail).await;
            if proj != "active" {
                return fail("T36", format!("②FAIL 后项目应保持 active，实际 '{proj}'"));
            }

            // 复位。
            let _ = ws_api_request(
                &mut ws,
                "board",
                "config.set",
                json!({ "key": "review.auto_close_project", "value": false }),
                10,
            )
            .await;
            pass(
                "T36",
                format!(
                    "项目收口 OK：①项目 {pid_pass} 全父单 done → 汇总 PASS → completed；\
                     ②项目 {pid_fail} FAIL → 缺口评论落父单（差距锚点命中），父单保持 done、项目保持 active"
                ),
            )
        })
        .await,
    );

    // ==================================================================
    // T37: worker 用量回传 + 审计回滚 + token 预算闸 e2e（全自动流转 P5 E1 二期/E2）
    // ==================================================================
    //
    // 前置：A/B 在 testai-board-10 组合桩（同 T33-T36 尾态）；桩非流式
    // 响应带真实 usage（prompt=countTokens>0）→ B 端 request_logs 有
    // 非零 token → 回传 delta 非零。
    // ①usage 回传：普通单派 Node-B → B 执行 → 回调带 usage → master
    //   记账（session_key=cluster_rpc:{worker}/{task_id} 精确键、
    //   model=cluster_delegate:*、provider=cluster、input+output>0）。
    //   断言直读 A 侧账本（workspace/data/nemesisbot_data.db，同 T33
    //   直读 board.db 的权威证据纪律）。
    // ②审计回滚 happy path：auto_accept 开 → PASS 自动收货 done →
    //   audit.list 过滤 auto_decide → audit.rollback done→in_review +
    //   系统评论 → 再回滚诚实拒绝（仅 done 可回滚）。
    // ③token 预算闸跨机：budget.max_tokens_per_parent=1 + <REVIEW_FAIL>
    //   → 首轮 FAIL 进重派路径 → token 维超限 → 🛑 停转人工，不重派
    //   （派发数恒 1）。
    all_results.push(
        run_test("T37: 用量回传+审计回滚+token 预算闸 e2e（P5 E1 二期/E2）", || async {
            let mut ws = match ws_connect_gateway(NODES[0].web_port).await {
                Ok(s) => s,
                Err(e) => return fail("T37", format!("WS connect to A failed: {e}")),
            };
            for (key, value) in [
                ("auto_accept", json!(false)),
                ("unlimited_mode", json!(false)),
                ("budget.max_tokens_per_parent", json!(0)),
            ] {
                if let Err(e) = ws_api_request(
                    &mut ws,
                    "board",
                    "config.set",
                    json!({ "key": key, "value": value }),
                    10,
                )
                .await
                {
                    return fail("T37", format!("config.set({key}) failed: {e}"));
                }
            }
            let db_path = ws_a.home().join("workspace").join("board").join("board.db");
            let board_store = match nemesis_board::BoardStore::open(&db_path, "NB") {
                Ok(s) => s,
                Err(e) => return fail("T37", format!("open board.db: {e}")),
            };
            // A 侧用量账本（master 记账落点；gateway Step 9b 同一路径）。
            let ledger_path = ws_a
                .home()
                .join("workspace")
                .join("data")
                .join("nemesisbot_data.db");
            let ledger = match nemesis_data::DataStore::open(&ledger_path) {
                Ok(s) => s,
                Err(e) => return fail("T37", format!("open nemesisbot_data.db: {e}")),
            };

            // ---- ① usage 回传 + master 记账 ----
            let created = match ws_api_request(
                &mut ws,
                "board",
                "issue.create",
                json!({
                    "title": "T37USAGE 用量回传",
                    "description": "cluster-uat T37①：worker 回调携带 usage，master 记账可查。"
                }),
                15,
            )
            .await
            {
                Ok(v) => v,
                Err(e) => return fail("T37", format!("①issue.create failed: {e}")),
            };
            let usage_issue = created
                .pointer("/issue/id")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            if usage_issue == 0 {
                return fail("T37", format!("①issue.create 无 id: {created}"));
            }
            if let Err(e) = ws_api_request(
                &mut ws,
                "board",
                "issue.dispatch",
                json!({ "id": usage_issue, "target": "Node-B" }),
                30,
            )
            .await
            {
                return fail("T37", format!("①issue.dispatch failed: {e}"));
            }
            // 轮询 ≤300s 等 B 完成回调（写回 in_review 即回调已收）。
            let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
            loop {
                if tokio::time::Instant::now() >= deadline {
                    let st = issue_status_of(&mut ws, usage_issue).await.unwrap_or_default();
                    return fail("T37", format!("①300s 内回调未收（status='{st}'）——回传链断"));
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
                if issue_status_of(&mut ws, usage_issue).await.unwrap_or_default() == "in_review" {
                    break;
                }
            }
            let dispatches = board_store
                .list_dispatches(usage_issue)
                .unwrap_or_default();
            let Some(disp) = dispatches.last() else {
                return fail("T37", "①无派发记录");
            };
            // 记账键形态 `cluster_rpc:{worker}/{task_id}`：worker 段 = 运行时
            // 节点 id（传输层 _rpc.from），派发记录存的是 peer 名（Node-B）——
            // 两者不同源，per-task 唯一性锚定 task_id（UUID），worker 段只断
            // 非空（首跑空段 bug 的回归位）。
            let task_suffix = format!("/{}", disp.task_id);
            let min_key_len = "cluster_rpc:".len() + 1 + task_suffix.len();
            let filter = nemesis_data::LogFilter {
                model: None,
                status: None,
                session_key: Some(disp.task_id.clone()),
            };
            let (logs, total) = ledger
                .query_logs(0, i64::MAX, 1, 100, &filter)
                .unwrap_or((Vec::new(), 0));
            let hit = logs.iter().find(|l| {
                l.session_key.starts_with("cluster_rpc:")
                    && l.session_key.len() >= min_key_len
                    && l.session_key.ends_with(&task_suffix)
            });
            match hit {
                Some(l)
                    if l.model.starts_with("cluster_delegate:")
                        && l.provider_type == "cluster"
                        && l.input_tokens + l.output_tokens > 0 =>
                {
                    pass(
                        "T37",
                        format!(
                            "①用量回传 OK：{} → model={} in={} out={}（账本共 {total} 条命中 task_id）",
                            l.session_key, l.model, l.input_tokens, l.output_tokens
                        ),
                    );
                }
                Some(l) => {
                    return fail(
                        "T37",
                        format!(
                            "①记账行字段不符：key={} model={} provider={} in={} out={}",
                            l.session_key, l.model, l.provider_type, l.input_tokens, l.output_tokens
                        ),
                    );
                }
                None => {
                    return fail(
                        "T37",
                        format!("①master 账本无 cluster_rpc:*{task_suffix} 形态记账行（命中 {total} 条 task_id 子串）——回传/记账断"),
                    );
                }
            }

            // ---- ② 审计回滚 happy path ----
            if let Err(e) = ws_api_request(
                &mut ws,
                "board",
                "config.set",
                json!({ "key": "auto_accept", "value": true }),
                10,
            )
            .await
            {
                return fail("T37", format!("②config.set(auto_accept) failed: {e}"));
            }
            let created = match ws_api_request(
                &mut ws,
                "board",
                "issue.create",
                json!({
                    "title": "T37AUDIT 审计回滚",
                    "description": "cluster-uat T37②：PASS 自动收货 → 决策流可回滚。"
                }),
                15,
            )
            .await
            {
                Ok(v) => v,
                Err(e) => return fail("T37", format!("②issue.create failed: {e}")),
            };
            let audit_issue = created
                .pointer("/issue/id")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            if audit_issue == 0 {
                return fail("T37", format!("②issue.create 无 id: {created}"));
            }
            if let Err(e) = ws_api_request(
                &mut ws,
                "board",
                "issue.dispatch",
                json!({ "id": audit_issue, "target": "Node-B" }),
                30,
            )
            .await
            {
                return fail("T37", format!("②issue.dispatch failed: {e}"));
            }
            // 轮询 ≤600s 等 PASS 自动收货 done。
            let deadline = tokio::time::Instant::now() + Duration::from_secs(600);
            loop {
                if tokio::time::Instant::now() >= deadline {
                    let st = issue_status_of(&mut ws, audit_issue).await.unwrap_or_default();
                    return fail("T37", format!("②600s 内未自动收货（status='{st}'）——验收/收货链断"));
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
                if issue_status_of(&mut ws, audit_issue).await.unwrap_or_default() == "done" {
                    break;
                }
            }
            // 决策流里找本单的 auto_accept 决策。
            let audit_list = ws_api_request(
                &mut ws,
                "board",
                "audit.list",
                json!({ "limit": 50, "action": "auto_decide" }),
                10,
            )
            .await
            .ok()
            .unwrap_or_default();
            let activity_id = audit_list
                .get("decisions")
                .and_then(|v| v.as_array())
                .and_then(|rows| {
                    rows.iter()
                        .find(|r| {
                            r.get("issue_id").and_then(|v| v.as_i64()) == Some(audit_issue)
                                && r.get("details")
                                    .and_then(|v| v.as_str())
                                    .is_some_and(|s| s.contains("auto_accept"))
                        })
                        .and_then(|r| r.get("id").and_then(|v| v.as_i64()))
                });
            let Some(activity_id) = activity_id else {
                return fail(
                    "T37",
                    format!("②决策流无本单 auto_accept 决策: {audit_list}"),
                );
            };
            // 回滚：done → in_review + 系统评论。
            let rolled = match ws_api_request(
                &mut ws,
                "board",
                "audit.rollback",
                json!({ "activity_id": activity_id }),
                10,
            )
            .await
            {
                Ok(v) => v,
                Err(e) => return fail("T37", format!("②audit.rollback failed: {e}")),
            };
            if rolled.get("rolled_back").and_then(|v| v.as_bool()) != Some(true) {
                return fail("T37", format!("②rollback 未生效: {rolled}"));
            }
            let st = issue_status_of(&mut ws, audit_issue).await.unwrap_or_default();
            if st != "in_review" {
                return fail("T37", format!("②回滚后应 in_review，实际 '{st}'"));
            }
            let comments = ws_api_request(
                &mut ws,
                "board",
                "comment.list",
                json!({ "issue_id": audit_issue }),
                10,
            )
            .await
            .ok()
            .and_then(|v| {
                v.get("comments")
                    .and_then(|c| c.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|c| c.get("content").and_then(|v| v.as_str()))
                            .collect::<Vec<_>>()
                            .join("\n---\n")
                    })
            })
            .unwrap_or_default();
            if !comments.contains("审计回滚") {
                return fail("T37", format!("②缺审计回滚系统评论: {}", trunc(&comments, 300)));
            }
            // 再回滚：已非 done → 诚实拒绝。
            let roll_again = ws_api_request(
                &mut ws,
                "board",
                "audit.rollback",
                json!({ "activity_id": activity_id }),
                10,
            )
            .await;
            match roll_again {
                Err(e) if e.to_string().contains("仅支持回滚已自动收货") => {}
                other => {
                    return fail(
                        "T37",
                        format!("②再回滚应拒绝（仅 done 可回滚），实际 {other:?}"),
                    );
                }
            }
            let _ = ws_api_request(
                &mut ws,
                "board",
                "config.set",
                json!({ "key": "auto_accept", "value": false }),
                10,
            )
            .await;
            pass(
                "T37",
                format!(
                    "②审计回滚 OK：issue {audit_issue} auto_accept 决策（activity {activity_id}）\
                     → 回滚 in_review + 系统评论 → 再回滚诚实拒绝"
                ),
            );

            // ---- ③ token 预算闸跨机 ----
            if let Err(e) = ws_api_request(
                &mut ws,
                "board",
                "config.set",
                json!({ "key": "budget.max_tokens_per_parent", "value": 1 }),
                10,
            )
            .await
            {
                return fail("T37", format!("③config.set(max_tokens) failed: {e}"));
            }
            let created = match ws_api_request(
                &mut ws,
                "board",
                "issue.create",
                json!({
                    "title": "T37BUDGET token 预算闸",
                    "description": "cluster-uat T37③：FAIL 重派路径上 token 维超限 → 停转人工。",
                    "acceptance_criteria": "交付说明文本。\n<REVIEW_FAIL>"
                }),
                15,
            )
            .await
            {
                Ok(v) => v,
                Err(e) => return fail("T37", format!("③issue.create failed: {e}")),
            };
            let budget_issue = created
                .pointer("/issue/id")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            if budget_issue == 0 {
                return fail("T37", format!("③issue.create 无 id: {created}"));
            }
            if let Err(e) = ws_api_request(
                &mut ws,
                "board",
                "issue.dispatch",
                json!({ "id": budget_issue, "target": "Node-B" }),
                30,
            )
            .await
            {
                return fail("T37", format!("③issue.dispatch failed: {e}"));
            }
            // 轮询 ≤300s：🛑 预算超限评论落单（FAIL → 重派路径 → token 维熔断）。
            let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
            loop {
                if tokio::time::Instant::now() >= deadline {
                    let n = board_store
                        .list_dispatches(budget_issue)
                        .map(|v| v.len())
                        .unwrap_or(0);
                    return fail(
                        "T37",
                        format!("③300s 内预算超限评论未落——token 闸断（dispatch={n}）"),
                    );
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
                let got = ws_api_request(
                    &mut ws,
                    "board",
                    "comment.list",
                    json!({ "issue_id": budget_issue }),
                    10,
                )
                .await
                .ok()
                .and_then(|v| {
                    v.get("comments")
                        .and_then(|c| c.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|c| c.get("content").and_then(|v| v.as_str()))
                                .collect::<Vec<_>>()
                                .join("\n---\n")
                        })
                })
                .unwrap_or_default();
                if got.contains("自动重派预算超限") && got.contains("max_tokens_per_parent") {
                    let n = board_store
                        .list_dispatches(budget_issue)
                        .map(|v| v.len())
                        .unwrap_or(0);
                    if n != 1 {
                        return fail(
                            "T37",
                            format!("③token 闸熔断后不应重派（派发数应恒 1），实际 {n}"),
                        );
                    }
                    break;
                }
            }
            let _ = ws_api_request(
                &mut ws,
                "board",
                "config.set",
                json!({ "key": "budget.max_tokens_per_parent", "value": 0 }),
                10,
            )
            .await;
            pass(
                "T37",
                format!(
                    "T37 全链 OK：worker usage 跨机回传记账 + 审计回滚 happy path + token 预算闸熔断（issue {budget_issue} 派发数恒 1）"
                ),
            )
        })
        .await,
    );

    // T-XFER-1: 执行档案分块回传正流（P3/D1 分块 + D3 双删 + D6 凭据核验）。
    all_results.push(
        run_test("T-XFER-1: 执行档案分块回传正流（4KiB 多块 + D6 逐字节核验 + 双删）", || async {
            // B 带 4KiB 块大小重启：压小块制造多块传输（默认 1MiB 时档案单块，
            // 测不出分块/续传语义）。
            gw_b.kill().await;
            gw_b = match start_gateway_and_wait_with_env(
                "Gateway-B",
                &gateway_bin,
                ws_b.path(),
                &NODES[1],
                &[("NEMESISBOT_TRANSFER_CHUNK_BYTES", "4096")],
            )
            .await
            {
                Ok(g) => g,
                Err(e) => return fail("T-XFER-1", format!("B 重启失败: {e}")),
            };
            let mut ws = match ws_connect_gateway(NODES[0].web_port).await {
                Ok(s) => s,
                Err(e) => return fail("T-XFER-1", format!("WS connect to A failed: {e}")),
            };
            let (issue_id, issue_number, project_dir, task_id) = match xfer_dispatch_to_b(
                &mut ws,
                &ws_a,
                Some("T-XFER1 分块回传"),
                "T-XFER1 执行档案分块回传",
                "派发后 B 的执行档案（cluster_logs）经分块通道回传 A，落入项目档案 records。",
            )
            .await
            {
                Ok(v) => v,
                Err(e) => return fail("T-XFER-1", format!("发车流失败: {e}")),
            };
            let _ = issue_id;
            println!("\n         issue={issue_number} task={task_id}");

            // 轮询 ≤240s：execution 落地（B 推送受 ~3 RPC/s 限速 + 回调排队）。
            let exec = match wait_execution_landed(
                &project_dir,
                &issue_number,
                &task_id,
                &ws_a,
                &ws_b,
                Duration::from_secs(240),
            )
            .await
            {
                Ok(d) => d,
                Err(e) => return fail("T-XFER-1", e),
            };
            // D6 凭据核验 + 分块确实发生（chunk_size 4096 且 ≥2 块）。
            let (total, chunk_size, chunk_count, file_count) = match verify_landed_manifest(&exec) {
                Ok(v) => v,
                Err(e) => return fail("T-XFER-1", e),
            };
            if chunk_size != 4096 {
                return fail("T-XFER-1", format!("chunk_size 应为 4096（env 注入），实际 {chunk_size}"));
            }
            if chunk_count < 2 {
                return fail("T-XFER-1", format!("分块未发生：chunk_count={chunk_count} total={total}"));
            }
            // 双删：A 收件箱已清（ingest 安置）、B 发件箱已删（收到 end ACK）。
            if a_inbox_entry(&ws_a, &task_id).is_some() {
                return fail("T-XFER-1", "A 收件箱未清（ingest 未安置）");
            }
            if b_outbox_entry(&ws_b, &task_id).is_some() {
                return fail(
                    "T-XFER-1",
                    format!("B 发件箱未删（state='{}'）", b_outbox_state(&ws_b, &task_id)),
                );
            }
            pass(
                "T-XFER-1",
                format!(
                    "正流 OK：issue {issue_number} task {task_id} 档案 {total}B / {chunk_count} 块（4KiB）落 execution，{file_count} 文件 D6 逐字节核验过，收件箱/发件箱双删"
                ),
            )
        })
        .await,
    );

    // T-XFER-2: worker 中途 kill 重启续推（P3/D2 发件箱持久化 + 断点续传）。
    all_results.push(
        run_test("T-XFER-2: worker kill 重启续推（D2 发件箱持久化）", || async {
            // B 仍带 4KiB 块（T-XFER-1 的 env 重启注入）：续传 transfer_id 嵌
            // 块大小，保持同块大小才能命中 master 侧 staging 续传。
            let mut ws = match ws_connect_gateway(NODES[0].web_port).await {
                Ok(s) => s,
                Err(e) => return fail("T-XFER-2", format!("WS connect to A failed: {e}")),
            };
            let (issue_id, issue_number, project_dir, task_id) = match xfer_dispatch_to_b(
                &mut ws,
                &ws_a,
                Some("T-XFER2 worker 续推"),
                "T-XFER2 worker 中途 kill 重启续推",
                "入队后强杀 worker，重启靠持久化发件箱续推完成回传。",
            )
            .await
            {
                Ok(v) => v,
                Err(e) => return fail("T-XFER-2", format!("发车流失败: {e}")),
            };
            // 等 in_review（回调已收 ≈ 档案已入队），立即 kill B——无论撞上
            // 推送中（pushing→pending 重置）还是已推完（cluster_logs 残留
            // 回填），重启都必须续上。
            let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
            loop {
                if tokio::time::Instant::now() >= deadline {
                    let st = issue_status_of(&mut ws, issue_id).await.unwrap_or_default();
                    return fail("T-XFER-2", format!("120s 内未 in_review（status='{st}'）——回调链断"));
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
                if issue_status_of(&mut ws, issue_id).await.unwrap_or_default() == "in_review" {
                    break;
                }
            }
            gw_b.kill().await;
            println!("         killed B mid-flow (outbox state='{}')", b_outbox_state(&ws_b, &task_id));
            gw_b = match start_gateway_and_wait_with_env(
                "Gateway-B",
                &gateway_bin,
                ws_b.path(),
                &NODES[1],
                &[("NEMESISBOT_TRANSFER_CHUNK_BYTES", "4096")],
            )
            .await
            {
                Ok(g) => g,
                Err(e) => return fail("T-XFER-2", format!("B 重启失败: {e}")),
            };
            // 轮询 ≤240s：档案落地（重启清扫入队 → 续推/重推 → 落地）。
            let exec = match wait_execution_landed(
                &project_dir,
                &issue_number,
                &task_id,
                &ws_a,
                &ws_b,
                Duration::from_secs(240),
            )
            .await
            {
                Ok(d) => d,
                Err(e) => return fail("T-XFER-2", e),
            };
            if let Err(e) = verify_landed_manifest(&exec) {
                return fail("T-XFER-2", e);
            }
            // 回填竞态容忍（P4 实测修正）：重启 sweep_startup 会把 cluster_logs
            // 残留（含本任务已交付记录）重新入队（D2 设计语义），条目会短暂
            // 重现——续推 + master 宽容接收后才再次删除。P4 起档案在 kill 前已
            // 落地合并，wait_execution_landed 瞬间通过，快照式断言会跑在回填
            // 重推收敛之前 → 假红。轮询等它删除（120s），超时才是真断链。
            let outbox_dl = tokio::time::Instant::now() + Duration::from_secs(120);
            loop {
                if b_outbox_entry(&ws_b, &task_id).is_none() {
                    break;
                }
                if tokio::time::Instant::now() >= outbox_dl {
                    return fail(
                        "T-XFER-2",
                        format!(
                            "120s 内 B 发件箱未收敛删除（state='{}'）——续推链断",
                            b_outbox_state(&ws_b, &task_id)
                        ),
                    );
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
            if a_inbox_entry(&ws_a, &task_id).is_some() {
                return fail("T-XFER-2", "A 收件箱未清");
            }
            pass(
                "T-XFER-2",
                format!(
                    "续推 OK：issue {issue_number} task {task_id} kill 后重启档案落地且凭据核验过，发件箱/收件箱双删"
                ),
            )
        })
        .await,
    );

    // T-XFER-3: master 中途 kill 重启续收（P3/D3 先落盘后 ACK）。
    all_results.push(
        run_test("T-XFER-3: master kill 重启续收（D3 staging 持久 + 续传合并）", || async {
            // B 带双 env 重启：4KiB 块（同 T-XFER-1/2，多块传输）+
            // CHUNK_DELAY_MS=250（chunk 间停顿钩子）。全速推 17 块仅 ~240ms，
            // staging 非空窗口 < 500ms 轮询周期，master 半程死亡场景永远造
            // 不出来（2026-09-18 第六轮取证：链路本身全绿，纯窗口竞争）。
            // 250ms × 17 ≈ 4.3s 半程窗口，稳定覆盖 kill 时机。
            gw_b.kill().await;
            gw_b = match start_gateway_and_wait_with_env(
                "Gateway-B",
                &gateway_bin,
                ws_b.path(),
                &NODES[1],
                &[
                    ("NEMESISBOT_TRANSFER_CHUNK_BYTES", "4096"),
                    ("NEMESISBOT_TRANSFER_CHUNK_DELAY_MS", "250"),
                ],
            )
            .await
            {
                Ok(g) => g,
                Err(e) => return fail("T-XFER-3", format!("B 重启失败: {e}")),
            };
            let mut ws = match ws_connect_gateway(NODES[0].web_port).await {
                Ok(s) => s,
                Err(e) => return fail("T-XFER-3", format!("WS connect to A failed: {e}")),
            };
            let (issue_id, issue_number, project_dir, task_id) = match xfer_dispatch_to_b(
                &mut ws,
                &ws_a,
                Some("T-XFER3 master 续收"),
                "T-XFER3 master 中途 kill 重启续收",
                "master 收块半程被杀，重启后 worker 续推剩余块完成落地。",
            )
            .await
            {
                Ok(v) => v,
                Err(e) => return fail("T-XFER-3", format!("发车流失败: {e}")),
            };
            let _ = issue_id;
            // 轮询 ≤120s：A 的 .staging 出现本任务且 ≥1 块落盘（推送已开跑；
            // 4KiB 块 + CHUNK_DELAY_MS=250 拉长的半程窗口给足 kill 时机）。
            let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
            loop {
                if tokio::time::Instant::now() >= deadline {
                    return fail(
                        "T-XFER-3",
                        format!("120s 内未见 master staging 分块落盘 task={task_id}——推送未开始"),
                    );
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
                let started = a_staging_dirs(&ws_a, &task_id)
                    .iter()
                    .any(|d| std::fs::read_dir(d).map(|rd| rd.count()).unwrap_or(0) > 0);
                if started {
                    break;
                }
            }
            let staged = a_staging_dirs(&ws_a, &task_id)
                .iter()
                .map(|d| std::fs::read_dir(d).map(|rd| rd.count()).unwrap_or(0))
                .sum::<usize>();
            // kill A（master 半程死亡；已 ACK 的块必须已在盘上）。
            gw_a.kill().await;
            gw_a = match start_gateway_and_wait("Gateway-A", &gateway_bin, ws_a.path(), &NODES[0])
                .await
            {
                Ok(g) => g,
                Err(e) => return fail("T-XFER-3", format!("A 重启失败: {e}")),
            };
            // 轮询 ≤300s：落地（B 15s tick 重试 → begin 返回 have 续传 → end → ingest）。
            let exec = match wait_execution_landed(
                &project_dir,
                &issue_number,
                &task_id,
                &ws_a,
                &ws_b,
                Duration::from_secs(300),
            )
            .await
            {
                Ok(d) => d,
                Err(e) => return fail("T-XFER-3", e),
            };
            if let Err(e) = verify_landed_manifest(&exec) {
                return fail("T-XFER-3", e);
            }
            if a_inbox_entry(&ws_a, &task_id).is_some() {
                return fail("T-XFER-3", "A 收件箱未清");
            }
            if b_outbox_entry(&ws_b, &task_id).is_some() {
                return fail("T-XFER-3", "B 发件箱未删（续收未闭环）");
            }
            pass(
                "T-XFER-3",
                format!(
                    "续收 OK：issue {issue_number} task {task_id} kill 前 {staged} 项已在 staging，重启后续收落地且凭据核验过"
                ),
            )
        })
        .await,
    );

    // T-XFER-4: 重复推送去重（P3/D3 幂等：档案在场 dedup 免传）。
    all_results.push(
        run_test("T-XFER-4: 重复推送去重（重启回填重推 → dedup 免传删条目）", || async {
            // B 恢复默认块（本测试不关心分块）。
            gw_b.kill().await;
            gw_b = match start_gateway_and_wait("Gateway-B", &gateway_bin, ws_b.path(), &NODES[1])
                .await
            {
                Ok(g) => g,
                Err(e) => return fail("T-XFER-4", format!("B 重启失败: {e}")),
            };
            let mut ws = match ws_connect_gateway(NODES[0].web_port).await {
                Ok(s) => s,
                Err(e) => return fail("T-XFER-4", format!("WS connect to A failed: {e}")),
            };
            // 无项目 → 档案落地后无处安置，以孤儿形态留守收件箱。
            let (issue_id, issue_number, _project_dir, task_id) = match xfer_dispatch_to_b(
                &mut ws,
                &ws_a,
                None,
                "T-XFER4 重复推送去重",
                "无项目绑定：档案落收件箱后 worker 重启回填重推，验证 dedup 幂等。",
            )
            .await
            {
                Ok(v) => v,
                Err(e) => return fail("T-XFER-4", format!("发车流失败: {e}")),
            };
            let _ = issue_id;
            // 轮询 ≤120s：A 收件箱出现本任务（孤儿留守）。
            let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
            loop {
                if tokio::time::Instant::now() >= deadline {
                    return fail(
                        "T-XFER-4",
                        format!("120s 内档案未达收件箱 task={task_id}（B outbox state='{}'）", b_outbox_state(&ws_b, &task_id)),
                    );
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
                if a_inbox_entry(&ws_a, &task_id).is_some() {
                    break;
                }
            }
            // B 重启 → 启动清扫回填 cluster_logs 残留 → 重推 → begin 命中
            // dedup（同 task+content_hash 且档案实体在场）→ 免传删条目。
            gw_b.kill().await;
            gw_b = match start_gateway_and_wait("Gateway-B", &gateway_bin, ws_b.path(), &NODES[1])
                .await
            {
                Ok(g) => g,
                Err(e) => return fail("T-XFER-4", format!("B 重启失败: {e}")),
            };
            // 轮询 ≤120s：A 网关日志出现本任务的 dedup 行 + B 发件箱清空。
            let log_path = ws_a.path().join("gateway.log");
            let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
            loop {
                if tokio::time::Instant::now() >= deadline {
                    // 失败现场：倾倒 A 日志尾部含 Transfer 的行（诊断日志
                    // 格式/链路断点，免二次翻文件）。
                    let log = std::fs::read_to_string(&log_path).unwrap_or_default();
                    let hits: Vec<&str> = log.lines().filter(|l| l.contains("Transfer")).collect();
                    let start = hits.len().saturating_sub(5);
                    let hint = hits[start..].join(" | ");
                    return fail(
                        "T-XFER-4",
                        format!(
                            "120s 内未见 dedup 免传（B outbox state='{}'，log={}，Transfer 行尾: {hint})",
                            b_outbox_state(&ws_b, &task_id),
                            log_path.display()
                        ),
                    );
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
                let log = std::fs::read_to_string(&log_path).unwrap_or_default();
                if log.contains("[Transfer] dedup") && log.contains(&task_id) {
                    break;
                }
            }
            if b_outbox_entry(&ws_b, &task_id).is_some() {
                return fail("T-XFER-4", "dedup 后 B 发件箱未清（应免传直接删条目）");
            }
            // 收件箱实体完好（dedup 不破坏原落地）。
            let Some(inbox) = a_inbox_entry(&ws_a, &task_id) else {
                return fail("T-XFER-4", "收件箱实体消失（dedup 误删原落地——D3 幂等被破坏）");
            };
            let files = std::fs::read_dir(inbox.join("files"))
                .map(|rd| rd.count())
                .unwrap_or(0);
            if files == 0 {
                return fail("T-XFER-4", "收件箱 files/ 为空");
            }
            pass(
                "T-XFER-4",
                format!(
                    "去重 OK：issue {issue_number} task {task_id} 重启回填重推被 dedup 免传（{files} 文件原落地完好），worker 条目免传清空"
                ),
            )
        })
        .await,
    );

    // T-XFER-5: 超限诚实失败（P3/D4 护栏：绝不截断 + 出卡转人工）。
    all_results.push(
        run_test("T-XFER-5: 超限诚实失败（D4 护栏 + 决策流/收件箱出卡 + 转人工回滚）", || async {
            // B 护栏压到 1KiB + 换 30s 延迟模型（确定性「执行中」窗口），
            // 重启生效（archive.max_transfer_bytes 不在 board.config.set 白名单，
            // 直接写 config.json；model add 同理须重启加载）。
            if let Err(e) = patch_board_archive_limit(&ws_b.home(), 1024) {
                return fail("T-XFER-5", format!("护栏写入失败: {e}"));
            }
            if let Err(e) = b_switch_model(&ws_b, &gateway_bin, "test/testai-1.2").await {
                return fail("T-XFER-5", format!("B 模型切换失败: {e}"));
            }
            gw_b.kill().await;
            gw_b = match start_gateway_and_wait("Gateway-B", &gateway_bin, ws_b.path(), &NODES[1])
                .await
            {
                Ok(g) => g,
                Err(e) => return fail("T-XFER-5", format!("B 重启失败: {e}")),
            };
            let mut ws = match ws_connect_gateway(NODES[0].web_port).await {
                Ok(s) => s,
                Err(e) => return fail("T-XFER-5", format!("WS connect to A failed: {e}")),
            };
            let (issue_id, issue_number, _project_dir, task_id) = match xfer_dispatch_to_b(
                &mut ws,
                &ws_a,
                Some("T-XFER5 超限护栏"),
                "T-XFER5 超限诚实失败",
                "档案超护栏 → 本地拒绝传输 → overlimit 通知 master 出卡转人工。",
            )
            .await
            {
                Ok(v) => v,
                Err(e) => return fail("T-XFER-5", format!("发车流失败: {e}")),
            };
            // 轮询 ≤30s：B 侧 cluster_logs 任务目录出现（LLM 已开跑，30s
            // 延迟窗口开启）→ 手动置 done（in_progress→done 合法；写回守卫
            // 尊重 Done 不翻转 → 完成回调照常入队发件箱）。
            let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
            loop {
                if tokio::time::Instant::now() >= deadline {
                    return fail("T-XFER-5", format!("30s 内 B 侧执行记录未落 task={task_id}"));
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
                if b_task_records(&ws_b, &task_id).is_some() {
                    break;
                }
            }
            if let Err(e) = ws_api_request(
                &mut ws,
                "board",
                "issue.status",
                json!({ "id": issue_id, "status": "done" }),
                10,
            )
            .await
            {
                return fail("T-XFER-5", format!("手动置 done 失败: {e}"));
            }
            // 轮询 ≤180s：完成回调 → 入队 → 推送 → 本地超限拒传 →
            // overlimit RPC → note_overlimit 出卡 + done→in_review 回滚。
            let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
            loop {
                if tokio::time::Instant::now() >= deadline {
                    return fail(
                        "T-XFER-5",
                        format!(
                            "180s 内超护栏评论未落 task={task_id}（B outbox state='{}'）",
                            b_outbox_state(&ws_b, &task_id)
                        ),
                    );
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
                let comments = ws_api_request(
                    &mut ws,
                    "board",
                    "comment.list",
                    json!({ "issue_id": issue_id }),
                    10,
                )
                .await
                .ok()
                .and_then(|v| {
                    v.get("comments")
                        .and_then(|c| c.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|c| c.get("content").and_then(|v| v.as_str()))
                                .collect::<Vec<_>>()
                                .join("\n---\n")
                        })
                })
                .unwrap_or_default();
                if comments.contains("超护栏") {
                    break;
                }
            }
            // 四件套断言：①回滚 in_review ②决策流卡 ③收件箱卡 ④B 暂留存。
            let st = issue_status_of(&mut ws, issue_id).await.unwrap_or_default();
            if st != "in_review" {
                return fail("T-XFER-5", format!("转人工回滚后应 in_review，实际 '{st}'"));
            }
            let audit = ws_api_request(
                &mut ws,
                "board",
                "audit.list",
                json!({ "limit": 100, "action": "archive_overlimit" }),
                10,
            )
            .await
            .ok()
            .unwrap_or_default();
            let has_audit = audit
                .get("decisions")
                .and_then(|v| v.as_array())
                .map(|rows| {
                    rows.iter()
                        .any(|r| r.get("issue_id").and_then(|v| v.as_i64()) == Some(issue_id))
                })
                .unwrap_or(false);
            if !has_audit {
                return fail("T-XFER-5", format!("决策流无 archive_overlimit 卡: {audit}"));
            }
            let inbox = ws_api_request(&mut ws, "board", "inbox.list", json!({}), 10)
                .await
                .ok()
                .unwrap_or_default();
            let has_notif = inbox
                .get("notifications")
                .and_then(|v| v.as_array())
                .map(|rows| {
                    rows.iter().any(|n| {
                        n.get("kind").and_then(|k| k.as_str()) == Some("archive_overlimit")
                            && n.get("issue_id").and_then(|v| v.as_i64()) == Some(issue_id)
                    })
                })
                .unwrap_or(false);
            if !has_notif {
                return fail("T-XFER-5", "收件箱无 archive_overlimit 通知");
            }
            let b_state = b_outbox_state(&ws_b, &task_id);
            if b_state != "over_limit" {
                return fail("T-XFER-5", format!("B 发件箱应暂停留 over_limit，实际 '{b_state}'"));
            }
            // 清理：护栏还原 2GiB + 模型还原 board-1.0 + 重启——over_limit
            // 条目重武装（total ≤ 新护栏）正常推送，落 T5 项目档案，不污染
            // 后续测试。
            if let Err(e) = patch_board_archive_limit(&ws_b.home(), 2_147_483_648) {
                return fail("T-XFER-5", format!("护栏还原失败: {e}"));
            }
            if let Err(e) = b_switch_model(&ws_b, &gateway_bin, "test/testai-board-1.0").await {
                return fail("T-XFER-5", format!("B 模型还原失败: {e}"));
            }
            gw_b.kill().await;
            gw_b = match start_gateway_and_wait("Gateway-B", &gateway_bin, ws_b.path(), &NODES[1])
                .await
            {
                Ok(g) => g,
                Err(e) => return fail("T-XFER-5", format!("B 还原重启失败: {e}")),
            };
            pass(
                "T-XFER-5",
                format!(
                    "超限 OK：issue {issue_number} task {task_id} 1KiB 护栏诚实拒传（未截断），决策流+收件箱出卡、done→in_review 转人工；清理后重武装推送"
                ),
            )
        })
        .await,
    );

    // T-XFER-6: D5 兜底拉取（档案丢失 → sweep 补拉 → worker 重推 → 重落）。
    all_results.push(
        run_test("T-XFER-6: master 档案丢失兜底拉取（D5 sweep + dedup 放行重传）", || async {
            // 防御性再武装（T-XFER-5 若中途 fail，其清理段不会执行——B 仍带
            // 1KiB 护栏 + 慢模型，正流载荷会被误超限）。无条件还原默认态。
            if let Err(e) = patch_board_archive_limit(&ws_b.home(), 2_147_483_648) {
                return fail("T-XFER-6", format!("护栏还原失败: {e}"));
            }
            if let Err(e) = b_switch_model(&ws_b, &gateway_bin, "test/testai-board-1.0").await {
                return fail("T-XFER-6", format!("B 模型还原失败: {e}"));
            }
            gw_b.kill().await;
            gw_b = match start_gateway_and_wait("Gateway-B", &gateway_bin, ws_b.path(), &NODES[1])
                .await
            {
                Ok(g) => g,
                Err(e) => return fail("T-XFER-6", format!("B 还原重启失败: {e}")),
            };
            let mut ws = match ws_connect_gateway(NODES[0].web_port).await {
                Ok(s) => s,
                Err(e) => return fail("T-XFER-6", format!("WS connect to A failed: {e}")),
            };
            let (issue_id, issue_number, project_dir, task_id) = match xfer_dispatch_to_b(
                &mut ws,
                &ws_a,
                Some("T-XFER6 兜底拉取"),
                "T-XFER6 master 档案丢失兜底拉取",
                "落地档案被删后 D5 sweep 主动补拉，worker 重推重落。",
            )
            .await
            {
                Ok(v) => v,
                Err(e) => return fail("T-XFER-6", format!("发车流失败: {e}")),
            };
            let _ = issue_id;
            // ① 正流先落一次。
            let _exec = match wait_execution_landed(
                &project_dir,
                &issue_number,
                &task_id,
                &ws_a,
                &ws_b,
                Duration::from_secs(240),
            )
            .await
            {
                Ok(d) => d,
                Err(e) => return fail("T-XFER-6", format!("①正流落地失败: {e}")),
            };
            // ② B 侧执行记录必须在场（重传的物质基础）。
            if b_task_records(&ws_b, &task_id).is_none() {
                return fail("T-XFER-6", "B 无执行记录残留——重传无从谈起");
            }
            // ③ 模拟档案丢失：删 records/<number>（sweep 判据=execution 缺失）。
            let records = std::path::Path::new(&project_dir)
                .join("records")
                .join(&issue_number);
            if let Err(e) = std::fs::remove_dir_all(&records) {
                return fail("T-XFER-6", format!("删除档案失败: {e}"));
            }
            // ④ 轮询 ≤300s：A sweep（60s 周期）→ transfer_pull → B 回填入队 →
            //    重推（dedup 放行——收件箱实体已被 ingest 搬走）→ ingest 重落。
            let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
            loop {
                if tokio::time::Instant::now() >= deadline {
                    return fail(
                        "T-XFER-6",
                        format!(
                            "300s 内档案未补回 task={task_id}（B outbox state='{}'，A 收件箱在场={}）——D5 链断",
                            b_outbox_state(&ws_b, &task_id),
                            a_inbox_entry(&ws_a, &task_id).is_some()
                        ),
                    );
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
                if records.join("execution").exists()
                    && execution_dirs(&project_dir, &issue_number)
                        .iter()
                        .any(|d| d.join("manifest.json").exists())
                {
                    break;
                }
            }
            if b_outbox_entry(&ws_b, &task_id).is_some() {
                return fail("T-XFER-6", "补拉重传后 B 发件箱未删");
            }
            if a_inbox_entry(&ws_a, &task_id).is_some() {
                return fail("T-XFER-6", "A 收件箱未清");
            }
            pass(
                "T-XFER-6",
                format!(
                    "兜底拉取 OK：issue {issue_number} task {task_id} 档案删除后 sweep 补拉重传重落，双删闭环"
                ),
            )
        })
        .await,
    );

    // T-MRG-1: common.h 自动三方合并 e2e（看板项目档案 goal P4/E4 实机判据）。
    //
    // 双 worker（B/C）并行改同一文件不同区域：
    //   ① B/C 切编辑桩 testai-board-edit-1.0（emit 真实 edit_file 工具调用）；
    //   ② 建项目（目录）→ 预置基线 common.h（10 个 SECTION 区块）；
    //   ③ 两张**无父子关系**的项目单（同父会撞 R-9 调度互斥——本测就是要
    //      并行）分别派 Node-B / Node-C，<EDIT_ANCHOR> 指令驱动各自在
    //      工作副本里改不同 SECTION；
    //   ④ 双变更集回传 → master 串行三方合并（第二笔以第一笔后的 HEAD 为
    //      ours、首基线为 ancestor）→ 双方改动都保留；
    //   ⑤ 断言：两单 done / common.h 双补丁齐全 + 10 区块无损 / git 历史
    //      ≥3 commit（首 commit + 两笔 merge）/ B/C 工作副本已清扫。
    all_results.push(
        run_test("T-MRG-1: common.h 三方合并 e2e（双 worker 并行改不同区域→双方保留+git log）", || async {
            let outcome: Result<String, anyhow::Error> = async {
                // 0. B/C 切编辑桩 + 重启（A 保持组合桩负责 review PASS）。
                b_switch_model(&ws_b, &gateway_bin, "test/testai-board-edit-1.0")
                    .await
                    .map_err(anyhow::Error::msg)?;
                gw_b.kill().await;
                gw_b = start_gateway_and_wait("Gateway-B", &gateway_bin, ws_b.path(), &NODES[1])
                    .await
                    .map_err(anyhow::Error::msg)?;
                b_switch_model(&ws_c, &gateway_bin, "test/testai-board-edit-1.0")
                    .await
                    .map_err(anyhow::Error::msg)?;
                gw_c.kill().await;
                gw_c = start_gateway_and_wait("Gateway-C", &gateway_bin, ws_c.path(), &NODES[2])
                    .await
                    .map_err(anyhow::Error::msg)?;

                let mut ws = ws_connect_gateway(NODES[0].web_port).await?;
                // 开关：auto_accept（验收 PASS 自动收货；前序测试可能动过，防御性重设）。
                ws_api_request(&mut ws, "board", "config.set", json!({ "key": "auto_accept", "value": true }), 10).await?;

                // 1. 建项目（目录）+ 预置基线 common.h（10 区块，锚点行唯一）。
                let created = ws_api_request(
                    &mut ws, "board", "project.create",
                    json!({ "name": "T-MRG1 common.h 三方合并", "auto_start": false }), 15,
                ).await?;
                let project_id = created.pointer("/project/id").and_then(|v| v.as_i64()).unwrap_or(0);
                let project_dir = created.pointer("/directory").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if project_id == 0 || project_dir.is_empty() {
                    anyhow::bail!("project.create 无 id/directory: {created}");
                }
                let mut baseline = String::from("// common.h baseline v1 (T-MRG1)\n#ifndef COMMON_H\n#define COMMON_H\n");
                for i in 1..=10 {
                    baseline.push_str(&format!("// SECTION-{i}-ANCHOR\n#define FEATURE_{i} {i}00\n"));
                }
                baseline.push_str("#endif\n");
                std::fs::write(std::path::Path::new(&project_dir).join("common.h"), &baseline)
                    .map_err(|e| anyhow::anyhow!("写基线 common.h 失败: {e}"))?;

                // 2. 两张独立项目单（无父子——R-9 互斥只拦同父，不拦本测的并行），
                //    <EDIT_ANCHOR> 指令由编辑桩解析成 edit_file 调用（改不同区块）。
                let mk_issue = |title: &str, anchor_old: &str, anchor_new: &str| {
                    json!({
                        "title": title,
                        "project_id": project_id,
                        "description": format!(
                            "在工作副本内完成指定编辑：<EDIT_ANCHOR>{anchor_old}|||{anchor_new}</EDIT_ANCHOR>"
                        ),
                        "acceptance_criteria": "[TOUCH] common.h\n[CHECK] re:FILE_EDIT_DONE",
                    })
                };
                let r1 = ws_api_request(&mut ws, "board", "issue.create",
                    mk_issue("T-MRG1 B侧：SECTION-3 加 PATCH_B", "// SECTION-3-ANCHOR", "// SECTION-3-ANCHOR\n#define PATCH_B_WORKER 303"), 15).await?;
                let id_b = r1.pointer("/issue/id").and_then(|v| v.as_i64()).unwrap_or(0);
                let num_b = r1.pointer("/issue/number").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let r2 = ws_api_request(&mut ws, "board", "issue.create",
                    mk_issue("T-MRG1 C侧：SECTION-7 加 PATCH_C", "// SECTION-7-ANCHOR", "// SECTION-7-ANCHOR\n#define PATCH_C_WORKER 707"), 15).await?;
                let id_c = r2.pointer("/issue/id").and_then(|v| v.as_i64()).unwrap_or(0);
                let num_c = r2.pointer("/issue/number").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if id_b == 0 || id_c == 0 {
                    anyhow::bail!("issue.create 无 id: {r1:?} / {r2:?}");
                }

                // 3. 并行派发（第二张的基线取决于时点：同 c1 或含 B 补丁的 c2——
                //    两种时序 E8+三方合并都自洽）。
                ws_api_request(&mut ws, "board", "issue.dispatch", json!({ "id": id_b, "target": "Node-B" }), 30).await?;
                ws_api_request(&mut ws, "board", "issue.dispatch", json!({ "id": id_c, "target": "Node-C" }), 30).await?;
                println!("\n         B={num_b} C={num_c} dir={project_dir}");

                // 4. 轮询 ≤360s 等双单 done（worker 编辑+回传+合并+评审链）。
                let deadline = tokio::time::Instant::now() + Duration::from_secs(360);
                loop {
                    if tokio::time::Instant::now() >= deadline {
                        anyhow::bail!(
                            "360s 内双单未 done：{num_b}='{}' {num_c}='{}'",
                            issue_status_of(&mut ws, id_b).await.unwrap_or_default(),
                            issue_status_of(&mut ws, id_c).await.unwrap_or_default(),
                        );
                    }
                    tokio::time::sleep(Duration::from_secs(4)).await;
                    let sb = issue_status_of(&mut ws, id_b).await.unwrap_or_default();
                    let sc = issue_status_of(&mut ws, id_c).await.unwrap_or_default();
                    if sb == "done" && sc == "done" {
                        break;
                    }
                    if matches!(sb.as_str(), "failed" | "cancelled")
                        || matches!(sc.as_str(), "failed" | "cancelled")
                    {
                        anyhow::bail!("单据异常终态：{num_b}='{sb}' {num_c}='{sc}'");
                    }
                }

                // 5. common.h：双补丁齐全 + 10 区块无损（三方合并未吞对方改动）。
                let merged_file = std::fs::read_to_string(std::path::Path::new(&project_dir).join("common.h"))
                    .map_err(|e| anyhow::anyhow!("读合并后 common.h 失败: {e}"))?;
                for needle in ["#define PATCH_B_WORKER 303", "#define PATCH_C_WORKER 707"] {
                    if !merged_file.contains(needle) {
                        anyhow::bail!("合并结果缺 {needle}（一方改动被吞）:\n{merged_file}");
                    }
                }
                for i in 1..=10 {
                    let anchor = format!("// SECTION-{i}-ANCHOR");
                    if !merged_file.contains(&anchor) {
                        anyhow::bail!("合并结果缺区块 {anchor}:\n{merged_file}");
                    }
                }

                // 6. git 历史：≥3 commit（首 commit + ≥2 笔 merge），HEAD 消息为
                //    merge 形态；线性可 diff。
                let repo = git2::Repository::open(&project_dir)
                    .map_err(|e| anyhow::anyhow!("打开项目仓库失败: {e}"))?;
                let head = repo.head().map_err(|e| anyhow::anyhow!("仓库无 HEAD: {e}"))?
                    .peel_to_commit()
                    .map_err(|e| anyhow::anyhow!("HEAD peel 失败: {e}"))?;
                let mut count = 0usize;
                let mut oid = head.id();
                loop {
                    count += 1;
                    let commit = repo.find_commit(oid)
                        .map_err(|e| anyhow::anyhow!("find_commit 失败: {e}"))?;
                    if commit.parent_count() == 0 {
                        break; // 首 commit
                    }
                    if commit.parent_count() != 1 {
                        anyhow::bail!("意外 multi-parent commit（merge 语义应单父线性）: {}", commit.id());
                    }
                    oid = commit.parent(0).map_err(|e| anyhow::anyhow!("parent 遍历失败: {e}"))?.id();
                }
                if count < 3 {
                    anyhow::bail!("git 历史仅 {count} 个 commit（应 ≥3：首 commit + 两笔合并）");
                }
                if !head.message().unwrap_or("").contains("merge:") {
                    anyhow::bail!("HEAD 消息非 merge 形态: {:?}", head.message());
                }

                // 7. B/C 工作副本已清扫（E3 生命周期闭环）。
                for (wsx, label) in [(&ws_b, "B"), (&ws_c, "C")] {
                    let exec_root = wsx.home().join("workspace").join("cluster").join("exec");
                    if exec_root.exists()
                        && std::fs::read_dir(&exec_root)
                            .map(|it| it.flatten().count())
                            .unwrap_or(0)
                            > 0
                    {
                        anyhow::bail!("{label} 工作副本未清扫: {}", exec_root.display());
                    }
                }

                let head_short = &head.id().to_string()[..12.min(head.id().to_string().len())];
                Ok(format!(
                    "三方合并 OK：{num_b}+{num_c} 双 done，common.h 双补丁+10 区块保全，git 历史 {count} commit（HEAD={head_short}）"
                ))
            }
            .await;
            match outcome {
                Ok(msg) => pass("T-MRG-1", msg),
                Err(e) => fail("T-MRG-1", format!("{e}")),
            }
        })
        .await,
    );

    // T-MRG-2: 冲突漏斗 human 档 e2e（P5/F1-F4）。
    //
    // 双 worker 并行改同一行（MODE 0→1 / 0→2）→ 恰好一单真冲突：
    //   ① 项目冻结 conflict_frozen + 审计 conflict(mode=human) + 冲突单停车
    //      不进评审（保持 in_progress）；胜者照常验收 done；
    //   ② 冻结闸：冻结项目的 issue.dispatch loud 拒绝（含「冲突冻结」）；
    //   ③ 不株连：另一项目同窗建单派发照常 done；
    //   ④ 人工决策：cancel 败者（其变更集已停车入档案 records/）+ 工作树
    //      追加人工落定标记；
    //   ⑤ resume dry_run 冻结预览 → resume 执行：manual conflict resolution
    //      commit + 补合并回放（冻结期在途交付不丢）+ 解冻 + 恢复派发。
    all_results.push(
        run_test("T-MRG-2: 冲突漏斗 human 档（冻结+不株连+冻结闸+cancel 败者+resume 解冻补合并）", || async {
            let outcome: Result<String, anyhow::Error> = async {
                // 0. 配置：human 档（防前序测试残留）+ PASS 自动收货。
                let mut ws = ws_connect_gateway(NODES[0].web_port).await?;
                for (key, value) in [
                    ("conflict_auto_resolve", json!(false)),
                    ("auto_accept", json!(true)),
                    ("budget.max_total_redispatch", json!(0)),
                ] {
                    ws_api_request(&mut ws, "board", "config.set", json!({ "key": key, "value": value }), 10).await?;
                }

                // 1. 冲突项目 P2 + 基线（MODE 行 + A/B 区块）。
                let created = ws_api_request(
                    &mut ws, "board", "project.create",
                    json!({ "name": "T-MRG2 冲突冻结人工档", "auto_start": false }), 15,
                ).await?;
                let pid = created.pointer("/project/id").and_then(|v| v.as_i64()).unwrap_or(0);
                let p2_dir = created.pointer("/directory").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if pid == 0 || p2_dir.is_empty() {
                    anyhow::bail!("project.create 无 id/directory: {created}");
                }
                let baseline = "// common.h baseline (T-MRG2)\n#define MODE 0\n// SECTION-A-ANCHOR\n#define FEATURE_A 1\n// SECTION-B-ANCHOR\n#define FEATURE_B 2\n";
                std::fs::write(std::path::Path::new(&p2_dir).join("common.h"), baseline)
                    .map_err(|e| anyhow::anyhow!("写基线失败: {e}"))?;

                // 2. 四张独立项目单（无父子——并行合法）：I1/I2 同行互斥（必有一冲突），
                //    I3/I4 不同区块（验证冻结期在途交付的登记语义）。
                let ac_h = "[TOUCH] common.h\n[CHECK] re:FILE_EDIT_DONE";
                let (id1, num1) = create_conflict_issue(&mut ws, pid, "T-MRG2 I1：MODE 0→1",
                    "<EDIT_ANCHOR>#define MODE 0|||#define MODE 1</EDIT_ANCHOR>", ac_h).await?;
                let (id2, num2) = create_conflict_issue(&mut ws, pid, "T-MRG2 I2：MODE 0→2",
                    "<EDIT_ANCHOR>#define MODE 0|||#define MODE 2</EDIT_ANCHOR>", ac_h).await?;
                let (id3, _num3) = create_conflict_issue(&mut ws, pid, "T-MRG2 I3：A 区块追加",
                    "<EDIT_ANCHOR>// SECTION-A-ANCHOR|||// SECTION-A-ANCHOR\n#define PATCH_A 11</EDIT_ANCHOR>", ac_h).await?;
                let (id4, _num4) = create_conflict_issue(&mut ws, pid, "T-MRG2 I4：B 区块追加",
                    "<EDIT_ANCHOR>// SECTION-B-ANCHOR|||// SECTION-B-ANCHOR\n#define PATCH_B 22</EDIT_ANCHOR>", ac_h).await?;
                ws_api_request(&mut ws, "board", "issue.dispatch", json!({ "id": id1, "target": "Node-B" }), 30).await?;
                ws_api_request(&mut ws, "board", "issue.dispatch", json!({ "id": id2, "target": "Node-C" }), 30).await?;
                ws_api_request(&mut ws, "board", "issue.dispatch", json!({ "id": id3, "target": "Node-B" }), 30).await?;
                ws_api_request(&mut ws, "board", "issue.dispatch", json!({ "id": id4, "target": "Node-C" }), 30).await?;
                println!("\n         I1={num1} I2={num2} dir={p2_dir}");

                // 3. 等冻结 + 冲突分流（胜者先合并进评审，败者停车保持 in_progress）。
                let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
                loop {
                    if tokio::time::Instant::now() >= deadline {
                        let (f, _) = project_freeze_state(&mut ws, pid).await.unwrap_or((false, 0));
                        anyhow::bail!("300s 内项目未冻结：{num1}='{}' {num2}='{}' frozen={f}",
                            issue_status_of(&mut ws, id1).await.unwrap_or_default(),
                            issue_status_of(&mut ws, id2).await.unwrap_or_default());
                    }
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    if project_freeze_state(&mut ws, pid).await?.0 {
                        break;
                    }
                }
                let (win_id, lose_id) = wait_conflict_split(&mut ws, id1, id2, 120).await?;
                let lose_st = issue_status_of(&mut ws, lose_id).await.unwrap_or_default();
                if lose_st != "in_progress" {
                    anyhow::bail!("冲突单应停车保持 in_progress，实际 '{lose_st}'（人工档不进评审）");
                }
                let details = wait_audit_decision(&mut ws, lose_id, "conflict", 60).await?;
                if !details.contains("\"mode\":\"human\"") {
                    anyhow::bail!("conflict 审计应 mode=human: {details}");
                }
                // 胜者照常走完验收（solver 桩不在位——A 保持 board-1.0 组合桩 → PASS）。
                wait_issue_done(&mut ws, win_id, "胜者", 240).await?;

                // 4. 冻结闸：冻结项目的派发 loud 拒绝。
                let (id5, _num5) = create_conflict_issue(&mut ws, pid, "T-MRG2 I5：冻结期新单",
                    "<EDIT_ANCHOR>// SECTION-A-ANCHOR|||// SECTION-A-ANCHOR\n#define PATCH_GATE 55</EDIT_ANCHOR>", ac_h).await?;
                let gate_err = ws_api_request(&mut ws, "board", "issue.dispatch", json!({ "id": id5, "target": "Node-B" }), 30)
                    .await
                    .err()
                    .ok_or_else(|| anyhow::anyhow!("冻结期派发应被拒绝"))?;
                if !gate_err.to_string().contains("冲突冻结") {
                    anyhow::bail!("冻结闸报错应含「冲突冻结」: {gate_err}");
                }

                // 5. 不株连：另一项目同窗建单派发照常。
                let created3 = ws_api_request(
                    &mut ws, "board", "project.create",
                    json!({ "name": "T-MRG2 孤岛项目", "auto_start": false }), 15,
                ).await?;
                let pid3 = created3.pointer("/project/id").and_then(|v| v.as_i64()).unwrap_or(0);
                let p3_dir = created3.pointer("/directory").and_then(|v| v.as_str()).unwrap_or("").to_string();
                std::fs::write(std::path::Path::new(&p3_dir).join("solo.h"), "// SOLO-ANCHOR\n#define SOLO 0\n")
                    .map_err(|e| anyhow::anyhow!("写孤岛基线失败: {e}"))?;
                let (id6, _num6) = create_conflict_issue(&mut ws, pid3, "T-MRG2 I6：孤岛单",
                    // <EDIT_FILE> 必带：编辑桩目标文件缺省 common.h（T-mrg-1
                    // 契约），不指定会把 solo.h 的锚打到不存在的 common.h 上
                    // → FILE_EDIT_FAILED → 锚点检查 FAIL → 重派耗尽转人工。
                    "<EDIT_FILE>/solo.h</EDIT_FILE><EDIT_ANCHOR>#define SOLO 0|||#define SOLO 1</EDIT_ANCHOR>",
                    "[TOUCH] solo.h\n[CHECK] re:FILE_EDIT_DONE").await?;
                ws_api_request(&mut ws, "board", "issue.dispatch", json!({ "id": id6, "target": "Node-B" }), 30).await?;
                wait_issue_done(&mut ws, id6, "I6 孤岛单", 240).await?;

                // 6. 人工决策：cancel 败者（其变更集已停车入 records/，人工放弃）。
                ws_api_request(&mut ws, "board", "issue.cancel", json!({ "id": lose_id }), 15).await?;
                let lose_st = issue_status_of(&mut ws, lose_id).await.unwrap_or_default();
                if lose_st != "cancelled" {
                    anyhow::bail!("败者应 cancelled，实际 '{lose_st}'");
                }

                // 7. resume dry_run：冻结预览，无副作用。
                let dry = ws_api_request(&mut ws, "board", "project.resume", json!({ "project_id": pid, "dry_run": true }), 30).await?;
                if dry.get("dry_run").and_then(|v| v.as_bool()) != Some(true)
                    || dry.get("frozen").and_then(|v| v.as_bool()) != Some(true)
                {
                    anyhow::bail!("dry_run 预览应 dry_run=true+frozen=true: {dry}");
                }

                // 8. 人工落定：工作树追加标记（保证 manual commit 非空）。
                let mut cur = std::fs::read_to_string(std::path::Path::new(&p2_dir).join("common.h"))
                    .map_err(|e| anyhow::anyhow!("读工作树失败: {e}"))?;
                cur.push_str("// manual-resolution (T-MRG2)\n");
                std::fs::write(std::path::Path::new(&p2_dir).join("common.h"), cur)
                    .map_err(|e| anyhow::anyhow!("写人工落定失败: {e}"))?;

                // 9. resume 执行：manual commit + 补合并回放 + 解冻 + 恢复派发。
                let resumed = ws_api_request(&mut ws, "board", "project.resume", json!({ "project_id": pid }), 60).await?;
                let replay = resumed
                    .pointer("/conflict_replay")
                    .ok_or_else(|| anyhow::anyhow!("resume 响应缺 conflict_replay: {resumed}"))?;
                if replay.get("unfrozen").and_then(|v| v.as_bool()) != Some(true) {
                    anyhow::bail!("回放后应已解冻: {replay}");
                }
                if replay.get("manual_commit").map(|v| v.is_null()).unwrap_or(true) {
                    anyhow::bail!("人工落定 commit 缺失（工作树改动未入库）: {replay}");
                }
                if resumed.get("dispatched").and_then(|v| v.as_u64()).unwrap_or(0) < 1 {
                    anyhow::bail!("resume 应恢复派发冻结期新单 I5: {resumed}");
                }

                // 10. 余单收口：I3/I4（冻结期交付→补合并或直合并）+ I5（resume 恢复派发）。
                for (iid, inum) in [(id3, "I3"), (id4, "I4"), (id5, "I5")] {
                    wait_issue_done(&mut ws, iid, inum, 300).await?;
                }
                let (frozen, pending) = project_freeze_state(&mut ws, pid).await?;
                if frozen || pending != 0 {
                    anyhow::bail!("终态应解冻+队列清空，实际 frozen={frozen} pending={pending}");
                }

                Ok(format!(
                    "human 档 OK：{num1}/{num2} 恰一冲突→冻结+mode=human 审计；冻结闸拒绝派发；孤岛项目不株连；cancel 败者 + resume 解冻补合并（merged={}）+ 恢复派发全部收口",
                    replay.get("merged").and_then(|v| v.as_u64()).unwrap_or(0)
                ))
            }
            .await;
            match outcome {
                Ok(msg) => pass("T-MRG-2", msg),
                Err(e) => fail("T-MRG-2", format!("{e}")),
            }
        })
        .await,
    );

    // T-MRG-3: 冲突漏斗 auto 档 AI 硬解成功 e2e（P5/F1+F5①）。
    //
    // A 切 testai-conflict-solver-1.0（硬解 + 评审委托双职能）：并行同行
    // 冲突 → solver merge 确定性落定 → 冲突单走完评审 done。断言：双 done /
    // 文件被确定性重写 / 审计 conflict_auto_resolve（resolutions 在场）/
    // HEAD commit message 带 conflict_auto_resolve 标记。
    all_results.push(
        run_test("T-MRG-3: 冲突漏斗 auto 档 AI 硬解成功（solver merge 落定→冲突单 done+审计+commit 标记）", || async {
            let outcome: Result<String, anyhow::Error> = async {
                // 0. A 切硬解桩（评审通道由内嵌 review 桩委托 PASS）+ 重启。
                b_switch_model(&ws_a, &gateway_bin, "test/testai-conflict-solver-1.0")
                    .await
                    .map_err(anyhow::Error::msg)?;
                gw_a.kill().await;
                gw_a = start_gateway_and_wait("Gateway-A", &gateway_bin, ws_a.path(), &NODES[0])
                    .await
                    .map_err(anyhow::Error::msg)?;

                let mut ws = ws_connect_gateway(NODES[0].web_port).await?;
                for (key, value) in [
                    ("conflict_auto_resolve", json!(true)),
                    ("auto_accept", json!(true)),
                    ("budget.max_total_redispatch", json!(0)),
                ] {
                    ws_api_request(&mut ws, "board", "config.set", json!({ "key": key, "value": value }), 10).await?;
                }

                // 1. 项目 + 基线 + 并行同行双单。
                let created = ws_api_request(
                    &mut ws, "board", "project.create",
                    json!({ "name": "T-MRG3 冲突AI硬解", "auto_start": false }), 15,
                ).await?;
                let pid = created.pointer("/project/id").and_then(|v| v.as_i64()).unwrap_or(0);
                let pdir = created.pointer("/directory").and_then(|v| v.as_str()).unwrap_or("").to_string();
                std::fs::write(std::path::Path::new(&pdir).join("common.h"), "// common.h baseline (T-MRG3)\n#define MODE 0\n")
                    .map_err(|e| anyhow::anyhow!("写基线失败: {e}"))?;
                let ac_h = "[TOUCH] common.h\n[CHECK] re:FILE_EDIT_DONE";
                let (id1, num1) = create_conflict_issue(&mut ws, pid, "T-MRG3 I1：MODE 0→1",
                    "<EDIT_ANCHOR>#define MODE 0|||#define MODE 1</EDIT_ANCHOR>", ac_h).await?;
                let (id2, num2) = create_conflict_issue(&mut ws, pid, "T-MRG3 I2：MODE 0→2",
                    "<EDIT_ANCHOR>#define MODE 0|||#define MODE 2</EDIT_ANCHOR>", ac_h).await?;
                ws_api_request(&mut ws, "board", "issue.dispatch", json!({ "id": id1, "target": "Node-B" }), 30).await?;
                ws_api_request(&mut ws, "board", "issue.dispatch", json!({ "id": id2, "target": "Node-C" }), 30).await?;

                // 2. 分流后双单都应 done（胜者常规验收；败者硬解→评审）。
                wait_conflict_split(&mut ws, id1, id2, 240).await?;
                wait_issue_done(&mut ws, id1, &num1, 400).await?;
                wait_issue_done(&mut ws, id2, &num2, 400).await?;

                // 3. 恰一单带 conflict_auto_resolve 审计（败者）。
                let a1 = wait_audit_decision(&mut ws, id1, "conflict_auto_resolve", 90).await.ok();
                let a2 = wait_audit_decision(&mut ws, id2, "conflict_auto_resolve", 90).await.ok();
                let details = match (&a1, &a2) {
                    (Some(d), None) => d.clone(),
                    (None, Some(d)) => d.clone(),
                    _ => anyhow::bail!("conflict_auto_resolve 审计应恰在一单上: a1={a1:?} a2={a2:?}"),
                };
                if !details.contains("\"action\":\"merge\"") || !details.contains("resolutions") {
                    anyhow::bail!("硬解审计应含 merge 处置与 resolutions: {details}");
                }

                // 4. 文件被确定性重写 + HEAD commit 带标记。
                let final_file = std::fs::read_to_string(std::path::Path::new(&pdir).join("common.h"))
                    .map_err(|e| anyhow::anyhow!("读最终文件失败: {e}"))?;
                if !final_file.contains("resolved-by-ai-stub: common.h") {
                    anyhow::bail!("文件应被 solver 确定性重写:\n{final_file}");
                }
                let repo = git2::Repository::open(&pdir).map_err(|e| anyhow::anyhow!("打开仓库失败: {e}"))?;
                let head = repo.head().map_err(|e| anyhow::anyhow!("无 HEAD: {e}"))?
                    .peel_to_commit().map_err(|e| anyhow::anyhow!("HEAD peel 失败: {e}"))?;
                let head_msg = head.message().unwrap_or("").to_string();
                if !head_msg.contains("conflict_auto_resolve") {
                    anyhow::bail!("HEAD 消息应带 conflict_auto_resolve 标记: {head_msg}");
                }

                Ok(format!(
                    "auto 硬解 OK：{num1}+{num2} 双 done，败者硬解落定（stub 重写+审计 resolutions），HEAD 带标记"
                ))
            }
            .await;
            match outcome {
                Ok(msg) => pass("T-MRG-3", msg),
                Err(e) => fail("T-MRG-3", format!("{e}")),
            }
        })
        .await,
    );

    // T-MRG-4: 冲突漏斗 auto 档硬解失败 → 重派原 worker e2e（P5/F5②）。
    //
    // 编辑桩锚点 new_text 内嵌 <CONFLICT_SOLVE_BAD>（文件内容级标记——只有
    // 硬解 prompt 能看到冲突三阶段文件内容，评审 prompt 不读文件 → 胜者
    // 评审不受污染）：solver 3 轮恒败 → t0 探针原 worker 在线 →
    // conflict_redispatch 重派 → 重派说明切 ANCHOR2（新基线重新表达意图）
    // → 干净合并 done。断言：双 done / 最终内容为败者 ANCHOR2 目标值 /
    // 审计 conflict_redispatch / 重派评论在场。
    all_results.push(
        run_test("T-MRG-4: 硬解失败重派原 worker（3 轮恒败→conflict_redispatch→ANCHOR2 干净合并）", || async {
            let outcome: Result<String, anyhow::Error> = async {
                let mut ws = ws_connect_gateway(NODES[0].web_port).await?;
                for (key, value) in [
                    ("conflict_auto_resolve", json!(true)),
                    ("auto_accept", json!(true)),
                    ("budget.max_total_redispatch", json!(0)),
                ] {
                    ws_api_request(&mut ws, "board", "config.set", json!({ "key": key, "value": value }), 10).await?;
                }
                let created = ws_api_request(
                    &mut ws, "board", "project.create",
                    json!({ "name": "T-MRG4 硬解失败重派", "auto_start": false }), 15,
                ).await?;
                let pid = created.pointer("/project/id").and_then(|v| v.as_i64()).unwrap_or(0);
                let pdir = created.pointer("/directory").and_then(|v| v.as_str()).unwrap_or("").to_string();
                std::fs::write(std::path::Path::new(&pdir).join("common.h"), "// common.h baseline (T-MRG4)\n#define MODE 0\n")
                    .map_err(|e| anyhow::anyhow!("写基线失败: {e}"))?;
                let ac_h = "[TOUCH] common.h\n[CHECK] re:FILE_EDIT_DONE";
                // 双向 ANCHOR2：败者重派时新基线 = 胜者落点（带 BAD 标记行）。
                let (id1, num1) = create_conflict_issue(&mut ws, pid, "T-MRG4 I1：MODE 0→1",
                    "<EDIT_ANCHOR>#define MODE 0|||#define MODE 1 <CONFLICT_SOLVE_BAD></EDIT_ANCHOR>\
                     <EDIT_ANCHOR2>#define MODE 2 <CONFLICT_SOLVE_BAD>|||#define MODE 8</EDIT_ANCHOR2>", ac_h).await?;
                let (id2, num2) = create_conflict_issue(&mut ws, pid, "T-MRG4 I2：MODE 0→2",
                    "<EDIT_ANCHOR>#define MODE 0|||#define MODE 2 <CONFLICT_SOLVE_BAD></EDIT_ANCHOR>\
                     <EDIT_ANCHOR2>#define MODE 1 <CONFLICT_SOLVE_BAD>|||#define MODE 9</EDIT_ANCHOR2>", ac_h).await?;
                ws_api_request(&mut ws, "board", "issue.dispatch", json!({ "id": id1, "target": "Node-B" }), 30).await?;
                ws_api_request(&mut ws, "board", "issue.dispatch", json!({ "id": id2, "target": "Node-C" }), 30).await?;

                wait_conflict_split(&mut ws, id1, id2, 240).await?;
                wait_issue_done(&mut ws, id1, &num1, 480).await?;
                wait_issue_done(&mut ws, id2, &num2, 480).await?;

                // 败者审计 + 重派评论。
                let a1 = wait_audit_decision(&mut ws, id1, "conflict_redispatch", 60).await.ok();
                let a2 = wait_audit_decision(&mut ws, id2, "conflict_redispatch", 60).await.ok();
                let (lose_id, details) = match (&a1, &a2) {
                    (Some(d), None) => (id1, d.clone()),
                    (None, Some(d)) => (id2, d.clone()),
                    _ => anyhow::bail!("conflict_redispatch 审计应恰在一单上: a1={a1:?} a2={a2:?}"),
                };
                // 重派审计的 worker 身份（D0 单一真相源归一后记真实运行时
                // id；占位兜底路径下是人读名——两形态都合法，只断言身份
                // 非空，不再硬编码 "Node-" 前缀（CI 真实 id 是 node-<host>-uuid）。
                let worker_recorded = serde_json::from_str::<serde_json::Value>(&details)
                    .ok()
                    .and_then(|v| {
                        v.pointer("/worker")
                            .and_then(|w| w.as_str())
                            .map(|s| s.to_string())
                    });
                match worker_recorded {
                    Some(w) if !w.is_empty() => {}
                    _ => anyhow::bail!("重派审计应记录原 worker 身份（worker 字段非空）: {details}"),
                }
                let comments = ws_api_request(&mut ws, "board", "comment.list", json!({ "issue_id": lose_id }), 10).await?;
                let joined = comments.to_string();
                if !joined.contains("已重派原 worker") {
                    anyhow::bail!("败者评论应含重派说明: {joined}");
                }

                // 最终内容 = 败者 ANCHOR2 目标值（互斥），BAD 标记被覆写清除。
                let final_file = std::fs::read_to_string(std::path::Path::new(&pdir).join("common.h"))
                    .map_err(|e| anyhow::anyhow!("读最终文件失败: {e}"))?;
                let has8 = final_file.contains("#define MODE 8");
                let has9 = final_file.contains("#define MODE 9");
                if has8 == has9 {
                    anyhow::bail!("最终内容应恰含 MODE 8 或 MODE 9 之一:\n{final_file}");
                }
                if final_file.contains("CONFLICT_SOLVE_BAD") {
                    anyhow::bail!("重派交付应覆写 BAD 标记行:\n{final_file}");
                }

                Ok(format!(
                    "重派原 worker OK：{num1}+{num2} 双 done，3 轮恒败→conflict_redispatch→ANCHOR2 干净合并（终值 {}）",
                    if has8 { "MODE 8" } else { "MODE 9" }
                ))
            }
            .await;
            match outcome {
                Ok(msg) => pass("T-MRG-4", msg),
                Err(e) => fail("T-MRG-4", format!("{e}")),
            }
        })
        .await,
    );

    // T-MRG-5: 冲突漏斗 auto 档原 worker 离线 → 三轮接触 → 换人 e2e（P5/F5③）。
    //
    // 同 T-MRG-4 的恒败构造；胜者合并落定瞬间 kill 双 worker（solver Delay=4s
    // 把硬解 3 轮失败窗口放大到 ~12s，覆盖轮询+kill 的操作间隙）→ t0/+60/+120
    // 三轮帧级探针全无应答 → conflict_switch_worker 换 Node-D（预切编辑桩）
    // → ANCHOR2 干净合并 done。E8：无迟到变更集（原 worker 已死，交付已消费）。
    all_results.push(
        run_test("T-MRG-5: 原 worker 离线三轮接触换人（kill B/C→probe 全败→conflict_switch_worker→Node-D 接手）", || async {
            let outcome: Result<String, anyhow::Error> = async {
                // 0. D 预切编辑桩 + 重启（接手节点）。
                b_switch_model(&ws_d, &gateway_bin, "test/testai-board-edit-1.0")
                    .await
                    .map_err(anyhow::Error::msg)?;
                gw_d.kill().await;
                gw_d = start_gateway_and_wait("Gateway-D", &gateway_bin, ws_d.path(), &NODES[3])
                    .await
                    .map_err(anyhow::Error::msg)?;

                let mut ws = ws_connect_gateway(NODES[0].web_port).await?;
                for (key, value) in [
                    ("conflict_auto_resolve", json!(true)),
                    ("auto_accept", json!(true)),
                    ("budget.max_total_redispatch", json!(0)),
                ] {
                    ws_api_request(&mut ws, "board", "config.set", json!({ "key": key, "value": value }), 10).await?;
                }
                let created = ws_api_request(
                    &mut ws, "board", "project.create",
                    json!({ "name": "T-MRG5 离线换人", "auto_start": false }), 15,
                ).await?;
                let pid = created.pointer("/project/id").and_then(|v| v.as_i64()).unwrap_or(0);
                let pdir = created.pointer("/directory").and_then(|v| v.as_str()).unwrap_or("").to_string();
                std::fs::write(std::path::Path::new(&pdir).join("common.h"), "// common.h baseline (T-MRG5)\n#define MODE 0\n")
                    .map_err(|e| anyhow::anyhow!("写基线失败: {e}"))?;
                let ac_h = "[TOUCH] common.h\n[CHECK] re:FILE_EDIT_DONE";
                let (id1, num1) = create_conflict_issue(&mut ws, pid, "T-MRG5 I1：MODE 0→1",
                    "<EDIT_ANCHOR>#define MODE 0|||#define MODE 1 <CONFLICT_SOLVE_BAD></EDIT_ANCHOR>\
                     <EDIT_ANCHOR2>#define MODE 2 <CONFLICT_SOLVE_BAD>|||#define MODE 8</EDIT_ANCHOR2>", ac_h).await?;
                let (id2, num2) = create_conflict_issue(&mut ws, pid, "T-MRG5 I2：MODE 0→2",
                    "<EDIT_ANCHOR>#define MODE 0|||#define MODE 2 <CONFLICT_SOLVE_BAD></EDIT_ANCHOR>\
                     <EDIT_ANCHOR2>#define MODE 1 <CONFLICT_SOLVE_BAD>|||#define MODE 9</EDIT_ANCHOR2>", ac_h).await?;
                ws_api_request(&mut ws, "board", "issue.dispatch", json!({ "id": id1, "target": "Node-B" }), 30).await?;
                ws_api_request(&mut ws, "board", "issue.dispatch", json!({ "id": id2, "target": "Node-C" }), 30).await?;

                // 1. 胜者先合并（in_review/done）→ 立刻 kill 双 worker：在硬解
                //    3 轮失败窗口（Delay=4s×3 ≈ 12s）内完成，t0 探针必扑空。
                wait_conflict_split(&mut ws, id1, id2, 240).await?;
                gw_b.kill().await;
                gw_c.kill().await;

                // 2. 换人链全程（3 轮探针 ~120s + Node-D 执行）→ 双 done。
                wait_issue_done(&mut ws, id1, &num1, 540).await?;
                wait_issue_done(&mut ws, id2, &num2, 540).await?;

                // 3. 复活 B/C（后续测试依赖；编辑桩配置已持久化）。放在审计
                //    断言之前——断言失败 bail 不得跳过复活毒化 T-MRG-6/7。
                gw_b = start_gateway_and_wait("Gateway-B", &gateway_bin, ws_b.path(), &NODES[1])
                    .await
                    .map_err(anyhow::Error::msg)?;
                gw_c = start_gateway_and_wait("Gateway-C", &gateway_bin, ws_c.path(), &NODES[2])
                    .await
                    .map_err(anyhow::Error::msg)?;

                // 4. 败者审计：conflict_switch_worker 恰在一单上，new_target
                //    非空且异于原 worker。rank_dispatch_candidates 返回节点
                //    runtime id 而非 name（T37 双身份坑位的显形——投影取
                //    base.id），实际接手由双 done + 终值互斥断言背书。
                let a1 = wait_audit_decision(&mut ws, id1, "conflict_switch_worker", 60).await.ok();
                let a2 = wait_audit_decision(&mut ws, id2, "conflict_switch_worker", 60).await.ok();
                let details = match (&a1, &a2) {
                    (Some(d), None) => d.clone(),
                    (None, Some(d)) => d.clone(),
                    _ => anyhow::bail!("conflict_switch_worker 审计应恰在一单上: a1={a1:?} a2={a2:?}"),
                };
                let audit_json: serde_json::Value = serde_json::from_str(&details)
                    .map_err(|e| anyhow::anyhow!("conflict_switch_worker details 非 JSON: {e}: {details}"))?;
                let new_target = audit_json.get("new_target").and_then(|v| v.as_str()).unwrap_or("");
                let orig_worker = audit_json.get("worker").and_then(|v| v.as_str()).unwrap_or("");
                if new_target.is_empty() || new_target == orig_worker {
                    anyhow::bail!("换人审计 new_target 应非空且异于原 worker（{orig_worker}）: {details}");
                }

                // 5. 终值断言（败者 ANCHOR2 目标值互斥）。
                let final_file = std::fs::read_to_string(std::path::Path::new(&pdir).join("common.h"))
                    .map_err(|e| anyhow::anyhow!("读最终文件失败: {e}"))?;
                let has8 = final_file.contains("#define MODE 8");
                let has9 = final_file.contains("#define MODE 9");
                if has8 == has9 || final_file.contains("CONFLICT_SOLVE_BAD") {
                    anyhow::bail!("终值应恰为 ANCHOR2 目标（8/9 互斥）且无 BAD 残留:\n{final_file}");
                }

                Ok(format!(
                    "离线换人 OK：{num1}+{num2} 双 done，kill 后三轮探针全败→conflict_switch_worker→{new_target} 接手（终值 {}）",
                    if has8 { "MODE 8" } else { "MODE 9" }
                ))
            }
            .await;
            match outcome {
                Ok(msg) => pass("T-MRG-5", msg),
                Err(e) => fail("T-MRG-5", format!("{e}")),
            }
        })
        .await,
    );

    // T-MRG-6: 冲突漏斗 auto 档二进制+锁文件混合冲突 e2e（P5/E5+F6）。
    //
    // 双 worker 各写一份不同字节 logo.bin（add/add 二进制冲突）+ 同行改
    // Cargo.lock（文本冲突，BIN+锚点两步桩形态）→ 败者冲突集含双文件 →
    // solver：Cargo.lock merge 确定性缝合 + logo.bin theirs 择边（不发明
    // 内容）。断言：双 done / Cargo.lock==stub 缝合 / logo.bin==败者版本 /
    // 审计 resolutions 含 theirs 择边与「建议重新生成」锁文件理由。
    all_results.push(
        run_test("T-MRG-6: 二进制择边+锁文件理由（add/add logo.bin theirs + Cargo.lock merge 缝合）", || async {
            let outcome: Result<String, anyhow::Error> = async {
                let mut ws = ws_connect_gateway(NODES[0].web_port).await?;
                for (key, value) in [
                    ("conflict_auto_resolve", json!(true)),
                    ("auto_accept", json!(true)),
                    ("budget.max_total_redispatch", json!(0)),
                ] {
                    ws_api_request(&mut ws, "board", "config.set", json!({ "key": key, "value": value }), 10).await?;
                }
                let created = ws_api_request(
                    &mut ws, "board", "project.create",
                    json!({ "name": "T-MRG6 二进制+锁文件", "auto_start": false }), 15,
                ).await?;
                let pid = created.pointer("/project/id").and_then(|v| v.as_i64()).unwrap_or(0);
                let pdir = created.pointer("/directory").and_then(|v| v.as_str()).unwrap_or("").to_string();
                std::fs::write(std::path::Path::new(&pdir).join("Cargo.lock"), "# T-MRG6 lockfile\nversion = 3\n")
                    .map_err(|e| anyhow::anyhow!("写基线失败: {e}"))?;
                let ac6 = "[TOUCH] Cargo.lock\n[TOUCH] assets/logo.bin\n[CHECK] re:FILE_EDIT_DONE";
                let (id1, num1) = create_conflict_issue(&mut ws, pid, "T-MRG6 I1：lock 3→4 + logo V1",
                    "<EDIT_FILE>Cargo.lock</EDIT_FILE><EDIT_ANCHOR>version = 3|||version = 4</EDIT_ANCHOR><BIN_EDIT>", ac6).await?;
                let (id2, num2) = create_conflict_issue(&mut ws, pid, "T-MRG6 I2：lock 3→9 + logo V2",
                    "<EDIT_FILE>Cargo.lock</EDIT_FILE><EDIT_ANCHOR>version = 3|||version = 9</EDIT_ANCHOR><BIN_EDIT_V2>", ac6).await?;
                ws_api_request(&mut ws, "board", "issue.dispatch", json!({ "id": id1, "target": "Node-B" }), 30).await?;
                ws_api_request(&mut ws, "board", "issue.dispatch", json!({ "id": id2, "target": "Node-C" }), 30).await?;

                wait_conflict_split(&mut ws, id1, id2, 240).await?;
                wait_issue_done(&mut ws, id1, &num1, 480).await?;
                wait_issue_done(&mut ws, id2, &num2, 480).await?;

                // 败者审计：择边 theirs + 锁文件理由含「建议重新生成」。
                let a1 = wait_audit_decision(&mut ws, id1, "conflict_auto_resolve", 90).await.ok();
                let a2 = wait_audit_decision(&mut ws, id2, "conflict_auto_resolve", 90).await.ok();
                let details = match (&a1, &a2) {
                    (Some(d), None) => d.clone(),
                    (None, Some(d)) => d.clone(),
                    _ => anyhow::bail!("conflict_auto_resolve 审计应恰在一单上: a1={a1:?} a2={a2:?}"),
                };
                if !details.contains("\"action\":\"theirs\"") || !details.contains("建议重新生成") {
                    anyhow::bail!("硬解审计应含 theirs 择边与锁文件理由: {details}");
                }

                // 文件终态：Cargo.lock 被 stub 缝合；logo.bin 恰为 V1/V2 之一。
                let lock = std::fs::read_to_string(std::path::Path::new(&pdir).join("Cargo.lock"))
                    .map_err(|e| anyhow::anyhow!("读 Cargo.lock 失败: {e}"))?;
                if !lock.contains("resolved-by-ai-stub: Cargo.lock") {
                    anyhow::bail!("Cargo.lock 应被 stub 缝合:\n{lock}");
                }
                let logo = std::fs::read(std::path::Path::new(&pdir).join("assets").join("logo.bin"))
                    .map_err(|e| anyhow::anyhow!("读 logo.bin 失败: {e}"))?;
                if logo != b"BIN\x00STUB\x00V1\x00" && logo != b"BIN\x00STUB\x00V2\x00\x00" {
                    anyhow::bail!("logo.bin 应为败者版本（V1/V2 之一），实际 {} 字节", logo.len());
                }

                Ok(format!(
                    "二进制+锁文件 OK：{num1}+{num2} 双 done，Cargo.lock stub 缝合 + logo.bin 择边 {} 字节 + theirs/建议重新生成 审计",
                    logo.len()
                ))
            }
            .await;
            match outcome {
                Ok(msg) => pass("T-MRG-6", msg),
                Err(e) => fail("T-MRG-6", format!("{e}")),
            }
        })
        .await,
    );

    // T-MRG-7: 冲突漏斗 auto 档预算打满回落 human 档 e2e（P5/F5.5）。
    //
    // 父单 + 两子单（手动建子无 touch_paths → R-9 互斥不拦并行）并行同行
    // 冲突，max_total_redispatch=1：败者硬解 3 轮恒败后预算检查 ——
    // chain_dispatch_count = 父单链累计派发数（0+1+1=2）> 1 → breach →
    // fallback_freeze（回落 human 档：冻结 + conflict(mode=auto_fallback)
    // 审计 + 停车），不无限循环。断言：冻结 + 审计 mode=auto_fallback +
    // 败者停车 in_progress + 胜者照常 done。
    all_results.push(
        run_test("T-MRG-7: 预算打满回落 human 档（父链派发数超限→fallback_freeze 冻结停车）", || async {
            let outcome: Result<String, anyhow::Error> = async {
                let mut ws = ws_connect_gateway(NODES[0].web_port).await?;
                for (key, value) in [
                    ("conflict_auto_resolve", json!(true)),
                    ("auto_accept", json!(true)),
                    ("budget.max_total_redispatch", json!(1)),
                ] {
                    ws_api_request(&mut ws, "board", "config.set", json!({ "key": key, "value": value }), 10).await?;
                }
                let created = ws_api_request(
                    &mut ws, "board", "project.create",
                    json!({ "name": "T-MRG7 预算保险丝", "auto_start": false }), 15,
                ).await?;
                let pid = created.pointer("/project/id").and_then(|v| v.as_i64()).unwrap_or(0);
                let pdir = created.pointer("/directory").and_then(|v| v.as_str()).unwrap_or("").to_string();
                std::fs::write(std::path::Path::new(&pdir).join("common.h"), "// common.h baseline (T-MRG7)\n#define MODE 0\n")
                    .map_err(|e| anyhow::anyhow!("写基线失败: {e}"))?;
                // 父单（不派发；预算链根）。
                let rp = ws_api_request(&mut ws, "board", "issue.create",
                    json!({ "title": "T-MRG7 父单", "project_id": pid, "description": "预算链根" }), 15).await?;
                let parent_id = rp.pointer("/issue/id").and_then(|v| v.as_i64()).unwrap_or(0);
                if parent_id == 0 {
                    anyhow::bail!("父单创建失败: {rp}");
                }
                // 两子单：手动建无 TOUCH（R-9 互斥不拦并行）；文件级 BAD 标记。
                let ac_plain = "[CHECK] re:FILE_EDIT_DONE";
                let mk_child = |title: &str, new_mode: &str| {
                    json!({
                        "title": title,
                        "project_id": pid,
                        "parent_issue_id": parent_id,
                        "description": format!("在工作副本内完成指定编辑：<EDIT_ANCHOR>#define MODE 0|||{new_mode}</EDIT_ANCHOR>"),
                        "acceptance_criteria": ac_plain,
                    })
                };
                let r1 = ws_api_request(&mut ws, "board", "issue.create",
                    mk_child("T-MRG7 C1：MODE 0→1", "#define MODE 1 <CONFLICT_SOLVE_BAD>"), 15).await?;
                let (id1, num1) = (r1.pointer("/issue/id").and_then(|v| v.as_i64()).unwrap_or(0),
                    r1.pointer("/issue/number").and_then(|v| v.as_str()).unwrap_or("").to_string());
                let r2 = ws_api_request(&mut ws, "board", "issue.create",
                    mk_child("T-MRG7 C2：MODE 0→2", "#define MODE 2 <CONFLICT_SOLVE_BAD>"), 15).await?;
                let (id2, num2) = (r2.pointer("/issue/id").and_then(|v| v.as_i64()).unwrap_or(0),
                    r2.pointer("/issue/number").and_then(|v| v.as_str()).unwrap_or("").to_string());
                if id1 == 0 || id2 == 0 {
                    anyhow::bail!("子单创建失败: {r1:?} / {r2:?}");
                }
                ws_api_request(&mut ws, "board", "issue.dispatch", json!({ "id": id1, "target": "Node-B" }), 30).await?;
                ws_api_request(&mut ws, "board", "issue.dispatch", json!({ "id": id2, "target": "Node-C" }), 30).await?;

                // 败者：硬解恒败 → 预算 breach（链累计 2 > 1）→ 冻结停车。
                // （breach 检查在探针时刻表之前——本测不付 120s 等待。）
                let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
                loop {
                    if tokio::time::Instant::now() >= deadline {
                        let (f, _) = project_freeze_state(&mut ws, pid).await.unwrap_or((false, 0));
                        anyhow::bail!("300s 内项目未冻结（预算 breach 未触发）：{num1}='{}' {num2}='{}' frozen={f}",
                            issue_status_of(&mut ws, id1).await.unwrap_or_default(),
                            issue_status_of(&mut ws, id2).await.unwrap_or_default());
                    }
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    if project_freeze_state(&mut ws, pid).await?.0 {
                        break;
                    }
                }
                let (win_id, lose_id) = wait_conflict_split(&mut ws, id1, id2, 120).await?;
                let lose_st = issue_status_of(&mut ws, lose_id).await.unwrap_or_default();
                if lose_st != "in_progress" {
                    anyhow::bail!("预算停车单应保持 in_progress，实际 '{lose_st}'");
                }
                let details = wait_audit_decision(&mut ws, lose_id, "conflict", 60).await?;
                if !details.contains("\"mode\":\"auto_fallback\"") {
                    anyhow::bail!("conflict 审计应 mode=auto_fallback: {details}");
                }
                if !details.contains("预算超限") {
                    anyhow::bail!("停车理由应含预算超限说明: {details}");
                }
                wait_issue_done(&mut ws, win_id, "胜者", 300).await?;

                // 复原：预算闸归零（防污染后续 UAT 轮次）。
                ws_api_request(&mut ws, "board", "config.set", json!({ "key": "budget.max_total_redispatch", "value": 0 }), 10).await?;

                Ok(format!(
                    "预算保险丝 OK：父链派发 2>1 → fallback_freeze 冻结 + auto_fallback 审计 + 败者停车 + 胜者 done（{num1}/{num2}）"
                ))
            }
            .await;
            match outcome {
                Ok(msg) => pass("T-MRG-7", msg),
                Err(e) => fail("T-MRG-7", format!("{e}")),
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
