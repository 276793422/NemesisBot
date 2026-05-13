//! NemesisBot Cluster P2P Integration Test Runner
//!
//! 1:1 port of Go's `test/cluster/p2p/p2p_test.go` (12 tests)
//! plus `test/cluster/integration_stress.go` (6 stress tests).
//!
//! Uses real TCP + UDP connections between P2P nodes.

use std::io::Write;
use std::net::{TcpStream, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nemesis_cluster::rpc::{RpcServer, RpcServerConfig};
use nemesis_cluster::task_manager::TaskManager;
use nemesis_cluster::transport::conn::{Connection, WireMessage};
use nemesis_cluster::transport::frame::{read_frame, write_frame};
use nemesis_cluster::discovery::{
    derive_key, encrypt_data, DiscoveryConfig, DiscoveryMessage, DiscoveryService,
};
use nemesis_types::cluster::TaskStatus;

use serde_json::json;

// ============================================================
// Test infrastructure
// ============================================================

struct TestResult {
    name: String,
    passed: bool,
    detail: String,
}

struct TestRunner {
    results: Vec<TestResult>,
}

impl TestRunner {
    fn new() -> Self {
        Self { results: vec![] }
    }

    fn run<F: FnOnce() -> Result<String, String>>(&mut self, name: &str, f: F) {
        print!("  {:50}", name);
        match f() {
            Ok(detail) => {
                let msg = detail.clone();
                self.results.push(TestResult {
                    name: name.to_string(),
                    passed: true,
                    detail,
                });
                println!("PASS  {}", msg);
            }
            Err(e) => {
                self.results.push(TestResult {
                    name: name.to_string(),
                    passed: false,
                    detail: e.clone(),
                });
                println!("FAIL  {}", e);
            }
        }
    }

    fn summary(&self) {
        let total = self.results.len();
        let passed = self.results.iter().filter(|r| r.passed).count();
        let failed = total - passed;
        println!("\n============================================================");
        println!("  TEST RESULTS");
        println!("============================================================");
        for r in &self.results {
            let icon = if r.passed { "PASS" } else { "FAIL" };
            println!("  [{}] {:50} {}", icon, r.name, r.detail);
        }
        println!("------------------------------------------------------------");
        println!("  Total: {} | Passed: {} | Failed: {}", total, passed, failed);
        println!("============================================================\n");
    }
}

// ============================================================
// Helper: start RPC server on dynamic port
// ============================================================

fn start_server(token: &str) -> (RpcServer, u16) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let config = RpcServerConfig {
        bind_address: "0.0.0.0:0".into(),
        ..Default::default()
    };
    let server = RpcServer::new(config);
    if !token.is_empty() {
        server.set_auth_token(token);
    }
    rt.block_on(server.start()).unwrap();
    let port = server.port();
    // Leak the runtime so the server stays alive
    std::mem::forget(rt);
    (server, port)
}

/// Send a WireMessage over a raw TCP connection and read the response.
fn tcp_send_recv(addr: &str, msg: &WireMessage) -> Result<WireMessage, String> {
    let data = msg.to_bytes().map_err(|e| e.to_string())?;
    let mut conn = Connection::connect(addr).map_err(|e| e.to_string())?;
    conn.send(&data).map_err(|e| e.to_string())?;
    let resp_data = conn.recv().map_err(|e| e.to_string())?;
    WireMessage::from_bytes(&resp_data).map_err(|e| e.to_string())
}

/// Send a WireMessage over a raw TCP connection with auth token.
fn tcp_send_recv_auth(addr: &str, msg: &WireMessage, token: &str) -> Result<WireMessage, String> {
    let mut stream =
        TcpStream::connect(addr).map_err(|e| format!("connect: {}", e))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .unwrap();

    // Send auth token line first
    if !token.is_empty() {
        stream
            .write_all(format!("{}\n", token).as_bytes())
            .map_err(|e| format!("send token: {}", e))?;
        stream.flush().unwrap();
    }

    // Send framed message
    let data = msg.to_bytes().map_err(|e| e.to_string())?;
    write_frame(&mut stream, &data).map_err(|e| format!("send frame: {}", e))?;

    // Read response frame
    let resp_data = read_frame(&mut stream).map_err(|e| format!("read frame: {}", e))?;
    WireMessage::from_bytes(&resp_data).map_err(|e| e.to_string())
}

/// Allocate a free UDP port.
fn allocate_udp_port() -> u16 {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.local_addr().unwrap().port()
}

/// Encrypted send discovery message via UDP unicast.
fn send_encrypted_udp(target_port: u16, key: &[u8; 32], msg: &DiscoveryMessage) {
    let msg_data = msg.to_bytes().unwrap();
    let encrypted = encrypt_data(key, &msg_data).unwrap();
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let _ = socket.send_to(&encrypted, format!("127.0.0.1:{}", target_port));
}

/// Mock ClusterCallbacks for discovery tests.
struct MockCallbacks {
    node_id: String,
    rpc_port: u16,
    discovered: Mutex<Vec<(String, String, Vec<String>, u16, String, String)>>,
    offline_events: Mutex<Vec<(String, String)>>,
}

impl MockCallbacks {
    fn new(node_id: &str, rpc_port: u16) -> Self {
        Self {
            node_id: node_id.to_string(),
            rpc_port,
            discovered: Mutex::new(vec![]),
            offline_events: Mutex::new(vec![]),
        }
    }

    fn get_discovered(&self) -> Vec<(String, String, Vec<String>, u16, String, String)> {
        self.discovered.lock().unwrap().clone()
    }

    fn get_offline_events(&self) -> Vec<(String, String)> {
        self.offline_events.lock().unwrap().clone()
    }
}

impl nemesis_cluster::discovery::ClusterCallbacks for MockCallbacks {
    fn node_id(&self) -> &str {
        &self.node_id
    }
    fn address(&self) -> &str {
        "127.0.0.1"
    }
    fn rpc_port(&self) -> u16 {
        self.rpc_port
    }
    fn all_local_ips(&self) -> Vec<String> {
        vec!["127.0.0.1".to_string()]
    }
    fn role(&self) -> &str {
        "worker"
    }
    fn category(&self) -> &str {
        "development"
    }
    fn tags(&self) -> Vec<String> {
        vec!["test".to_string()]
    }
    fn handle_discovered_node(
        &self,
        node_id: &str,
        name: &str,
        addresses: &[String],
        rpc_port: u16,
        role: &str,
        category: &str,
        _tags: &[String],
        _capabilities: &[String],
    ) {
        let mut g = self.discovered.lock().unwrap();
        g.push((
            node_id.to_string(),
            name.to_string(),
            addresses.to_vec(),
            rpc_port,
            role.to_string(),
            category.to_string(),
        ));
    }
    fn handle_node_offline(&self, node_id: &str, reason: &str) {
        self.offline_events
            .lock()
            .unwrap()
            .push((node_id.to_string(), reason.to_string()));
    }
    fn sync_to_disk(&self) -> Result<(), String> {
        Ok(())
    }
}

// ============================================================
// Test 1: Two-Node Ping
// ============================================================

fn test_two_node_ping() -> Result<String, String> {
    let (server, port) = start_server("");
    let addr = format!("127.0.0.1:{}", port);

    let req = WireMessage::new_request("node-B", "node-A", "Ping", json!({}));
    let resp = tcp_send_recv(&addr, &req)?;

    if resp.msg_type != "response" {
        return Err(format!("expected response, got {}", resp.msg_type));
    }
    let status = resp
        .payload
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if status != "pong" {
        return Err(format!("expected status 'pong', got '{}'", status));
    }

    server.stop().map_err(|e| e)?;
    Ok("B pinged A via TCP, got pong".into())
}

// ============================================================
// Test 2: Bidirectional Communication
// ============================================================

fn test_bidirectional() -> Result<String, String> {
    let (server_a, port_a) = start_server("");
    let (server_b, port_b) = start_server("");

    // Register unique handlers
    server_a.register_handler(
        "echo_A",
        Box::new(|payload| Ok(json!({"source": "A", "echo_id": payload["id"]}))),
    );
    server_b.register_handler(
        "echo_B",
        Box::new(|payload| Ok(json!({"source": "B", "echo_id": payload["id"]}))),
    );

    let addr_a = format!("127.0.0.1:{}", port_a);
    let addr_b = format!("127.0.0.1:{}", port_b);

    // A -> B (using thread to simulate concurrency)
    let addr_b_clone = addr_b.clone();
    let h_a_to_b = std::thread::spawn(move || -> Result<String, String> {
        let req = WireMessage::new_request("node-A", "node-B", "echo_B", json!({"id": "call-A-to-B"}));
        let resp = tcp_send_recv(&addr_b_clone, &req)?;
        let source = resp.payload.get("source").and_then(|v| v.as_str()).unwrap_or("");
        let echo_id = resp.payload.get("echo_id").and_then(|v| v.as_str()).unwrap_or("");
        if source != "B" || echo_id != "call-A-to-B" {
            Err(format!("A->B: source={}, echo_id={}", source, echo_id))
        } else {
            Ok("A->B OK".into())
        }
    });

    // B -> A
    let addr_a_clone = addr_a.clone();
    let h_b_to_a = std::thread::spawn(move || -> Result<String, String> {
        let req = WireMessage::new_request("node-B", "node-A", "echo_A", json!({"id": "call-B-to-A"}));
        let resp = tcp_send_recv(&addr_a_clone, &req)?;
        let source = resp.payload.get("source").and_then(|v| v.as_str()).unwrap_or("");
        let echo_id = resp.payload.get("echo_id").and_then(|v| v.as_str()).unwrap_or("");
        if source != "A" || echo_id != "call-B-to-A" {
            Err(format!("B->A: source={}, echo_id={}", source, echo_id))
        } else {
            Ok("B->A OK".into())
        }
    });

    let r1 = h_a_to_b.join().unwrap()?;
    let r2 = h_b_to_a.join().unwrap()?;

    server_a.stop().map_err(|e| e)?;
    server_b.stop().map_err(|e| e)?;
    Ok(format!("{}, {}", r1, r2))
}

// ============================================================
// Test 3: Task Dispatch and Callback
// ============================================================

fn test_task_dispatch_callback() -> Result<String, String> {
    let (server_a, port_a) = start_server("");
    let (server_b, port_b) = start_server("");

    let callbacks: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(vec![]));
    let callbacks_clone = callbacks.clone();

    // A: peer_chat_callback handler
    server_a.register_handler(
        "peer_chat_callback",
        Box::new(move |payload| {
            callbacks_clone.lock().unwrap().push(payload.clone());
            Ok(json!({"status": "received"}))
        }),
    );

    let port_a_for_cb = port_a;
    let token_for_a = String::new();

    // B: peer_chat handler (ACK + async callback via real TCP)
    server_b.register_handler(
        "peer_chat",
        Box::new(move |payload| {
            let task_id = payload
                .get("task_id")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();

            let source_info = payload.get("_source").cloned().unwrap_or(json!({}));
            let addr = format!("127.0.0.1:{}", port_a_for_cb);
            let token = token_for_a.clone();
            let task_id_cb = task_id.clone();

            // Async callback via real TCP
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(100));
                let source_node_id = source_info.get("node_id").and_then(|v| v.as_str()).unwrap_or("");
                if !source_node_id.is_empty() {
                    let cb_msg = WireMessage::new_request(
                        "node-B",
                        source_node_id,
                        "peer_chat_callback",
                        json!({
                            "task_id": task_id_cb,
                            "status": "success",
                            "response": "Processed by node-B",
                        }),
                    );
                    let _ = tcp_send_recv_auth(&addr, &cb_msg, &token);
                }
            });

            // Immediate ACK
            Ok(json!({"status": "accepted", "task_id": task_id}))
        }),
    );

    // A calls B's peer_chat
    let addr_b = format!("127.0.0.1:{}", port_b);
    let task_id = format!("task-test-{}", chrono::Utc::now().timestamp_millis());
    let req = WireMessage::new_request(
        "node-A",
        "node-B",
        "peer_chat",
        json!({
            "content": "Hello from A",
            "type": "chat",
            "task_id": task_id,
            "_source": {"node_id": "node-A"},
        }),
    );

    let resp = tcp_send_recv(&addr_b, &req)?;
    let ack_status = resp
        .payload
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if ack_status != "accepted" {
        return Err(format!("expected ACK 'accepted', got '{}'", ack_status));
    }

    // Wait for async callback
    let start = std::time::Instant::now();
    loop {
        let count = callbacks.lock().unwrap().len();
        if count > 0 {
            break;
        }
        if start.elapsed() > Duration::from_secs(5) {
            return Err("timeout waiting for callback from B".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let cbs = callbacks.lock().unwrap();
    let cb = &cbs[0];
    let cb_task = cb.get("task_id").and_then(|v| v.as_str()).unwrap_or("");
    let cb_status = cb.get("status").and_then(|v| v.as_str()).unwrap_or("");
    let cb_resp = cb
        .get("response")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if cb_task != task_id {
        return Err(format!(
            "expected task_id '{}', got '{}'",
            task_id, cb_task
        ));
    }
    if cb_status != "success" {
        return Err(format!("expected 'success', got '{}'", cb_status));
    }
    if cb_resp != "Processed by node-B" {
        return Err(format!("unexpected response: {}", cb_resp));
    }

    server_a.stop().map_err(|e| e)?;
    server_b.stop().map_err(|e| e)?;
    Ok(format!(
        "ACK received, callback verified (task_id={})",
        task_id
    ))
}

// ============================================================
// Test 4: Task Manager Lifecycle (no network)
// ============================================================

fn test_task_status_lifecycle() -> Result<String, String> {
    let mut tm = TaskManager::new();
    tm.start();

    let completed: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));
    let completed_clone = completed.clone();
    tm.set_on_complete(Box::new(move |task_id: &str| {
        completed_clone.lock().unwrap().push(task_id.to_string());
    }));

    // --- Success path ---
    let task1 = tm.create_task("peer_chat", json!({"content": "test"}), "web", "chat-1");
    let got = tm.get_task(&task1.id).unwrap();
    if got.status != TaskStatus::Pending {
        return Err(format!(
            "expected Pending, got {:?}",
            got.status
        ));
    }

    tm.complete_task(&task1.id, json!("Done!"));
    let got = tm.get_task(&task1.id).unwrap();
    if got.status != TaskStatus::Completed {
        return Err(format!(
            "expected Completed, got {:?}",
            got.status
        ));
    }

    // Verify callback
    let cb_list = completed.lock().unwrap();
    if cb_list.len() != 1 || cb_list[0] != task1.id {
        return Err(format!("expected callback for {}, got {:?}", task1.id, *cb_list));
    }
    drop(cb_list);

    // --- Error path ---
    let task2 = tm.create_task("peer_chat", json!({}), "web", "chat-2");
    tm.fail_task(&task2.id, "connection refused");
    let got2 = tm.get_task(&task2.id).unwrap();
    if got2.status != TaskStatus::Failed {
        return Err(format!(
            "expected Failed, got {:?}",
            got2.status
        ));
    }

    // --- CompleteCallback ---
    let task3 = tm.create_task("peer_chat", json!({}), "web", "chat-3");
    tm.complete_callback(&task3.id, "success", "callback response", "");
    let got3 = tm.get_task(&task3.id).unwrap();
    if got3.status != TaskStatus::Completed {
        return Err(format!(
            "expected Completed from CompleteCallback, got {:?}",
            got3.status
        ));
    }

    tm.stop();
    Ok("3 lifecycle paths verified (success/fail/callback)".into())
}

// ============================================================
// Test 5: Concurrent Multi-Task
// ============================================================

fn test_concurrent_multi_task() -> Result<String, String> {
    let (server, port) = start_server("");
    let addr = format!("127.0.0.1:{}", port);

    server.register_handler(
        "work",
        Box::new(|payload| {
            std::thread::sleep(Duration::from_millis(100));
            Ok(json!({"status": "done", "work_id": payload["id"]}))
        }),
    );

    let num_tasks = 5;
    let mut handles = vec![];

    for i in 0..num_tasks {
        let addr = addr.clone();
        let work_id = format!("work-{}", i);
        handles.push(std::thread::spawn(move || -> Result<String, String> {
            let req = WireMessage::new_request(
                &format!("client-{}", i),
                "node-A",
                "work",
                json!({"id": work_id}),
            );
            let resp = tcp_send_recv(&addr, &req)?;
            let returned_id = resp
                .payload
                .get("work_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if returned_id != format!("work-{}", i) {
                Err(format!(
                    "cross-contamination: expected {}, got {}",
                    i, returned_id
                ))
            } else {
                Ok(format!("work-{} OK", i))
            }
        }));
    }

    let mut errors = vec![];
    for h in handles {
        match h.join().unwrap() {
            Ok(_) => {}
            Err(e) => errors.push(e),
        }
    }

    server.stop().map_err(|e| e)?;
    if errors.is_empty() {
        Ok(format!("{} concurrent tasks, no cross-contamination", num_tasks))
    } else {
        Err(errors.join("; "))
    }
}

// ============================================================
// Test 6: Auth Token Enforcement (4 sub-tests)
// ============================================================

fn test_auth_token_enforcement() -> Result<String, String> {
    let mut sub_results = vec![];

    // Sub-test 1: Same token -> success
    {
        let (server, port) = start_server("shared-secret");
        server.register_handler(
            "echo",
            Box::new(|_| Ok(json!({"status": "ok"}))),
        );
        let addr = format!("127.0.0.1:{}", port);
        let req = WireMessage::new_request("node-A", "node-B", "echo", json!({}));
        match tcp_send_recv_auth(&addr, &req, "shared-secret") {
            Ok(resp) => {
                let status = resp.payload.get("status").and_then(|v| v.as_str()).unwrap_or("");
                sub_results.push(format!(
                    "same_token: {}",
                    if status == "ok" { "OK" } else { "FAIL" }
                ));
            }
            Err(e) => sub_results.push(format!("same_token: FAIL ({})", e)),
        }
        server.stop().map_err(|e| e)?;
    }

    // Sub-test 2: Different token -> failure
    {
        let (server, port) = start_server("server-token");
        server.register_handler(
            "echo",
            Box::new(|_| Ok(json!({"status": "ok"}))),
        );
        let addr = format!("127.0.0.1:{}", port);
        let req = WireMessage::new_request("node-A", "node-B", "echo", json!({}));
        match tcp_send_recv_auth(&addr, &req, "wrong-token") {
            Ok(_) => sub_results.push("different_token: FAIL (should have failed)".into()),
            Err(_) => sub_results.push("different_token: OK (rejected)".into()),
        }
        server.stop().map_err(|e| e)?;
    }

    // Sub-test 3: Client no token + server has token -> failure
    {
        let (server, port) = start_server("server-token");
        server.register_handler(
            "echo",
            Box::new(|_| Ok(json!({"status": "ok"}))),
        );
        let addr = format!("127.0.0.1:{}", port);
        let req = WireMessage::new_request("node-A", "node-B", "echo", json!({}));
        // No token sent
        match tcp_send_recv(&addr, &req) {
            Ok(_) => sub_results.push("client_no_token: FAIL (should have failed)".into()),
            Err(_) => sub_results.push("client_no_token: OK (rejected)".into()),
        }
        server.stop().map_err(|e| e)?;
    }

    // Sub-test 4: Both no token -> success
    {
        let (server, port) = start_server("");
        server.register_handler(
            "echo",
            Box::new(|_| Ok(json!({"status": "ok"}))),
        );
        let addr = format!("127.0.0.1:{}", port);
        let req = WireMessage::new_request("node-A", "node-B", "echo", json!({}));
        match tcp_send_recv(&addr, &req) {
            Ok(_) => sub_results.push("both_no_token: OK".into()),
            Err(e) => sub_results.push(format!("both_no_token: FAIL ({})", e)),
        }
        server.stop().map_err(|e| e)?;
    }

    let all_ok = sub_results.iter().all(|r| r.contains(": OK"));
    if all_ok {
        Ok(sub_results.join(", "))
    } else {
        Err(sub_results.join(", "))
    }
}

// ============================================================
// Test 7: Role Capabilities
// ============================================================

fn test_role_capabilities() -> Result<String, String> {
    let (server, port) = start_server("");
    let addr = format!("127.0.0.1:{}", port);

    // Query capabilities
    let req = WireMessage::new_request("node-B", "node-A", "GetCapabilities", json!({}));
    let resp = tcp_send_recv(&addr, &req)?;
    let caps = resp.payload.get("capabilities").cloned().unwrap_or(json!([]));
    if caps.as_array().map_or(true, |a| a.is_empty()) {
        return Err("expected non-empty capabilities".into());
    }

    // Query info
    let req2 = WireMessage::new_request("node-B", "node-A", "GetInfo", json!({}));
    let resp2 = tcp_send_recv(&addr, &req2)?;
    let status = resp2
        .payload
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if status != "online" {
        return Err(format!("expected status 'online', got '{}'", status));
    }

    server.stop().map_err(|e| e)?;
    Ok("get_capabilities + get_info verified".into())
}

// ============================================================
// Test 8: Encrypted Discovery
// ============================================================

fn test_encrypted_discovery() -> Result<String, String> {
    let enc_key = derive_key("test-cluster-secret");
    let port = allocate_udp_port();

    let cb = Arc::new(MockCallbacks::new("node-A", 9999));
    let cb_clone = cb.clone();

    let config = DiscoveryConfig::with_encryption(
        port,
        Duration::from_secs(30),
        "test-cluster-secret",
    );
    let rt = tokio::runtime::Runtime::new().unwrap();
    let disc = rt.block_on(async {
        let d = DiscoveryService::new(cb_clone, config).map_err(|e| e.to_string())?;
        d.start().map_err(|e| e.to_string())?;
        Ok::<DiscoveryService, String>(d)
    })?;

    std::thread::sleep(Duration::from_millis(100));

    // Send encrypted announce via unicast
    let announce = DiscoveryMessage::new_announce(
        "node-B",
        "Node B",
        vec!["192.168.1.2".to_string()],
        9900,
        "worker",
        "development",
        vec![],
        vec![],
    );
    send_encrypted_udp(port, &enc_key, &announce);

    // Wait for discovery
    let start = std::time::Instant::now();
    loop {
        let nodes = cb.get_discovered();
        if !nodes.is_empty() {
            let (node_id, _, _, rpc_port, role, _category) = &nodes[0];
            if node_id != "node-B" {
                return Err(format!("expected node-B, got {}", node_id));
            }
            if *rpc_port != 9900 {
                return Err(format!("expected rpc_port 9900, got {}", rpc_port));
            }
            if role != "worker" {
                return Err(format!("expected role 'worker', got {}", role));
            }
            break;
        }
        if start.elapsed() > Duration::from_secs(3) {
            return Err("timeout waiting for encrypted discovery".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    disc.stop().map_err(|e| e.to_string())?;

    // --- Different key: message silently dropped ---
    let port2 = allocate_udp_port();
    let cb2 = Arc::new(MockCallbacks::new("node-C", 0));
    let cb2_clone = cb2.clone();

    let config2 = DiscoveryConfig::with_encryption(port2, Duration::from_secs(30), "wrong-key");
    let rt2 = tokio::runtime::Runtime::new().unwrap();
    let disc2 = rt2.block_on(async {
        let d = DiscoveryService::new(cb2_clone, config2).map_err(|e| e.to_string())?;
        d.start().map_err(|e| e.to_string())?;
        Ok::<DiscoveryService, String>(d)
    })?;

    std::thread::sleep(Duration::from_millis(100));

    // Encrypt with the ORIGINAL key — listener has a DIFFERENT key
    send_encrypted_udp(port2, &enc_key, &announce);

    std::thread::sleep(Duration::from_millis(500));

    let nodes2 = cb2.get_discovered();
    disc2.stop().map_err(|e| e.to_string())?;

    if !nodes2.is_empty() {
        return Err("should NOT decrypt with wrong key".into());
    }

    Ok("same_key=discovered, wrong_key=silently_dropped".into())
}

// ============================================================
// Test 9: Node Offline Bye Message
// ============================================================

fn test_node_offline_bye() -> Result<String, String> {
    let enc_key = derive_key("test-cluster-secret");
    let port = allocate_udp_port();

    let cb = Arc::new(MockCallbacks::new("node-A", 0));
    let cb_clone = cb.clone();

    let config = DiscoveryConfig::with_encryption(port, Duration::from_secs(30), "test-cluster-secret");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let disc = rt.block_on(async {
        let d = DiscoveryService::new(cb_clone, config).map_err(|e| e.to_string())?;
        d.start().map_err(|e| e.to_string())?;
        Ok::<DiscoveryService, String>(d)
    })?;

    std::thread::sleep(Duration::from_millis(100));

    // Send encrypted bye message
    let bye = DiscoveryMessage::new_bye("node-B");
    send_encrypted_udp(port, &enc_key, &bye);

    // Wait for offline event
    let start = std::time::Instant::now();
    loop {
        let events = cb.get_offline_events();
        if !events.is_empty() {
            let (node_id, reason) = &events[0];
            if node_id != "node-B" {
                return Err(format!("expected node-B, got {}", node_id));
            }
            disc.stop().map_err(|e| e.to_string())?;
            return Ok(format!("offline event: node={}, reason={}", node_id, reason));
        }
        if start.elapsed() > Duration::from_secs(3) {
            disc.stop().map_err(|e| e.to_string())?;
            return Err("timeout waiting for HandleNodeOffline".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

// ============================================================
// Test 10: Error Handling — Invalid Action
// ============================================================

fn test_invalid_action() -> Result<String, String> {
    let (server, port) = start_server("");
    let addr = format!("127.0.0.1:{}", port);

    let req = WireMessage::new_request(
        "node-B",
        "node-A",
        "nonexistent_action",
        json!({"data": "test"}),
    );
    let resp = tcp_send_recv(&addr, &req)?;

    if resp.msg_type != "error" {
        return Err(format!(
            "expected error type, got '{}' with payload {:?}",
            resp.msg_type, resp.payload
        ));
    }
    if resp.error.is_empty() {
        return Err("expected error message".into());
    }
    if !resp.error.contains("no handler") {
        return Err(format!(
            "expected 'no handler' in error, got '{}'",
            resp.error
        ));
    }

    server.stop().map_err(|e| e)?;
    Ok(format!("no_handler error: {}", resp.error))
}

// ============================================================
// Test 11: Message Validation (raw TCP)
// ============================================================

fn test_message_validation() -> Result<String, String> {
    let (server, port) = start_server("");
    let addr = format!("127.0.0.1:{}", port);

    // Sub-test 1: Message with empty "action" field
    // (WireMessage requires all fields for deserialization, so empty action
    // triggers "no handler" error from the server)
    {
        let invalid = WireMessage::new_request("test-node", "node-A", "", json!({}));
        let resp = tcp_send_recv(&addr, &invalid)?;

        if resp.msg_type != "error" {
            return Err(format!(
                "expected error response, got type '{}'",
                resp.msg_type
            ));
        }
        if !resp.error.contains("no handler") {
            return Err(format!("unexpected error: {}", resp.error));
        }
    }

    // Sub-test 2: Valid ping with null payload
    {
        let ping_msg = WireMessage::new_request("test-node", "node-A", "Ping", json!(null));
        let resp = tcp_send_recv(&addr, &ping_msg)?;

        if resp.msg_type == "error" {
            return Err(format!(
                "ping should accept null payload, got error: {}",
                resp.error
            ));
        }
    }

    server.stop().map_err(|e| e)?;
    Ok("empty_action=error, valid_ping=response".into())
}

// ============================================================
// Test 12: Full End-to-End (Discovery + Auth + peer_chat + callback)
// ============================================================

fn test_full_e2e() -> Result<String, String> {
    let shared_token = "cluster-shared-secret-token";
    let enc_key = derive_key(shared_token);

    // Phase 1: Encrypted discovery setup
    let disc_port = allocate_udp_port();
    let cb = Arc::new(MockCallbacks::new("node-A", 0));
    let cb_clone = cb.clone();

    let disc_config = DiscoveryConfig::with_encryption(disc_port, Duration::from_secs(30), shared_token);
    let rt_disc = tokio::runtime::Runtime::new().unwrap();
    let disc = rt_disc.block_on(async {
        let d = DiscoveryService::new(cb_clone, disc_config).map_err(|e| e.to_string())?;
        d.start().map_err(|e| e.to_string())?;
        Ok::<DiscoveryService, String>(d)
    })?;

    // Phase 2: Authenticated RPC for both nodes
    let (server_a, port_a) = start_server(shared_token);
    let (server_b, port_b) = start_server(shared_token);

    let callbacks: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(vec![]));
    let callbacks_clone = callbacks.clone();

    server_a.register_handler(
        "peer_chat_callback",
        Box::new(move |payload| {
            callbacks_clone.lock().unwrap().push(payload.clone());
            Ok(json!({"status": "received"}))
        }),
    );

    let port_a_for_cb = port_a;
    let token_a = shared_token.to_string();

    server_b.register_handler(
        "peer_chat",
        Box::new(move |payload| {
            let task_id = payload
                .get("task_id")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let source_info = payload.get("_source").cloned().unwrap_or(json!({}));
            let addr = format!("127.0.0.1:{}", port_a_for_cb);
            let token = token_a.clone();
            let tid = task_id.clone();

            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(100));
                let source_node_id = source_info.get("node_id").and_then(|v| v.as_str()).unwrap_or("");
                if !source_node_id.is_empty() {
                    let cb_msg = WireMessage::new_request(
                        "node-B",
                        source_node_id,
                        "peer_chat_callback",
                        json!({
                            "task_id": tid,
                            "status": "success",
                            "response": "E2E: Processed by node-B",
                        }),
                    );
                    let _ = tcp_send_recv_auth(&addr, &cb_msg, &token);
                }
            });

            Ok(json!({"status": "accepted", "task_id": task_id}))
        }),
    );

    std::thread::sleep(Duration::from_millis(100));

    // Send encrypted announce from "node-B" to discovery A
    let announce = DiscoveryMessage::new_announce(
        "node-B",
        "Node B",
        vec!["127.0.0.1".to_string()],
        port_b,
        "worker",
        "development",
        vec![],
        vec!["code_generation".to_string(), "testing".to_string()],
    );
    send_encrypted_udp(disc_port, &enc_key, &announce);

    // Wait for discovery
    let start = std::time::Instant::now();
    loop {
        let nodes = cb.get_discovered();
        if !nodes.is_empty() {
            if nodes[0].0 != "node-B" {
                return Err(format!("expected node-B, got {}", nodes[0].0));
            }
            break;
        }
        if start.elapsed() > Duration::from_secs(3) {
            return Err("timeout: discovery did not find node-B".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    // Phase 3: Authenticated RPC — A calls B
    let addr_b = format!("127.0.0.1:{}", port_b);
    let task_id = format!("e2e-task-{}", chrono::Utc::now().timestamp_millis());
    let req = WireMessage::new_request(
        "node-A",
        "node-B",
        "peer_chat",
        json!({
            "content": "E2E test message",
            "type": "chat",
            "task_id": task_id,
            "_source": {"node_id": "node-A"},
        }),
    );

    let resp = tcp_send_recv_auth(&addr_b, &req, shared_token)?;
    let ack_status = resp
        .payload
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if ack_status != "accepted" {
        return Err(format!("expected 'accepted', got '{}'", ack_status));
    }

    // Phase 4: Verify callback
    let start = std::time::Instant::now();
    loop {
        let count = callbacks.lock().unwrap().len();
        if count > 0 {
            break;
        }
        if start.elapsed() > Duration::from_secs(5) {
            return Err("timeout waiting for callback".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let cbs = callbacks.lock().unwrap();
    let cb_task = cbs[0].get("task_id").and_then(|v| v.as_str()).unwrap_or("");
    let cb_status = cbs[0].get("status").and_then(|v| v.as_str()).unwrap_or("");
    let cb_resp = cbs[0].get("response").and_then(|v| v.as_str()).unwrap_or("");

    if cb_task != task_id {
        return Err(format!("expected task_id '{}', got '{}'", task_id, cb_task));
    }
    if cb_status != "success" {
        return Err(format!("expected 'success', got '{}'", cb_status));
    }
    if cb_resp != "E2E: Processed by node-B" {
        return Err(format!("unexpected response: {}", cb_resp));
    }

    disc.stop().map_err(|e| e.to_string())?;
    server_a.stop().map_err(|e| e)?;
    server_b.stop().map_err(|e| e)?;
    Ok("discovery->auth->peer_chat->callback verified".into())
}

// ============================================================
// STRESS TEST 1: Basic RPC Communication
// ============================================================

fn stress_basic_rpc() -> Result<String, String> {
    let (server, port) = start_server("");
    let addr = format!("127.0.0.1:{}", port);

    server.register_handler(
        "echo",
        Box::new(|payload| Ok(payload)),
    );

    let req = WireMessage::new_request("client", "server", "echo", json!({"message": "hello"}));
    let resp = tcp_send_recv(&addr, &req)?;

    if resp.msg_type != "response" {
        return Err(format!("wrong type: {}", resp.msg_type));
    }

    server.stop().map_err(|e| e)?;
    Ok("basic echo RPC succeeded".into())
}

// ============================================================
// STRESS TEST 2: Concurrent RPC (10 simultaneous)
// ============================================================

fn stress_concurrent_rpc() -> Result<String, String> {
    let (server, port) = start_server("");
    let addr = format!("127.0.0.1:{}", port);

    server.register_handler(
        "ping",
        Box::new(|_| {
            std::thread::sleep(Duration::from_millis(100));
            Ok(json!({"status": "ok"}))
        }),
    );

    let num = 10;
    let mut handles = vec![];
    for i in 0..num {
        let addr = addr.clone();
        handles.push(std::thread::spawn(move || -> bool {
            let req = WireMessage::new_request(
                &format!("client-{}", i),
                "server",
                "ping",
                json!(null),
            );
            match tcp_send_recv(&addr, &req) {
                Ok(resp) => resp.msg_type == "response",
                Err(_) => false,
            }
        }));
    }

    let success_count = handles
        .into_iter()
        .map(|h| h.join().unwrap())
        .filter(|&ok| ok)
        .count();

    server.stop().map_err(|e| e)?;
    if success_count == num {
        Ok(format!("{}/{} concurrent calls succeeded", success_count, num))
    } else {
        Err(format!(
            "{}/{} concurrent calls succeeded",
            success_count, num
        ))
    }
}

// ============================================================
// STRESS TEST 3: Sequential RPC (50 calls)
// ============================================================

fn stress_sequential_rpc() -> Result<String, String> {
    let (server, port) = start_server("");
    let addr = format!("127.0.0.1:{}", port);

    server.register_handler(
        "counter",
        Box::new(|payload| Ok(json!({"count": payload["n"]}))),
    );

    let total = 50;
    let mut success = 0;

    for i in 0..total {
        let req = WireMessage::new_request(
            "client",
            "server",
            "counter",
            json!({"n": i}),
        );
        match tcp_send_recv(&addr, &req) {
            Ok(resp) if resp.msg_type == "response" => success += 1,
            _ => {}
        }
        if i % 10 == 9 {
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    server.stop().map_err(|e| e)?;
    let min_expected = 45;
    if success >= min_expected {
        Ok(format!("{}/{} sequential calls succeeded", success, total))
    } else {
        Err(format!(
            "only {}/{} sequential calls succeeded (min {})",
            success, total, min_expected
        ))
    }
}

// ============================================================
// STRESS TEST 4: Large Payload (1MB)
// ============================================================

fn stress_large_payload() -> Result<String, String> {
    let (server, port) = start_server("");
    let addr = format!("127.0.0.1:{}", port);

    server.register_handler(
        "large",
        Box::new(|payload| {
            let size = serde_json::to_string(&payload).map(|s| s.len()).unwrap_or(0);
            Ok(json!({"status": "ok", "received_size": size}))
        }),
    );

    // Create ~100KB payload (1MB JSON would be very slow)
    let large_data: Vec<u8> = (0..100_000).map(|i| (i % 256) as u8).collect();
    let data_str = serde_json::to_string(&large_data).unwrap_or_default();

    let start = std::time::Instant::now();
    let req = WireMessage::new_request(
        "client",
        "server",
        "large",
        json!({"data": data_str}),
    );
    let resp = tcp_send_recv(&addr, &req)?;
    let elapsed = start.elapsed();

    if resp.msg_type != "response" {
        return Err(format!("wrong type: {}", resp.msg_type));
    }

    server.stop().map_err(|e| e)?;
    Ok(format!("100KB payload transfer: {:?}", elapsed))
}

// ============================================================
// STRESS TEST 5: Timeout Handling
// ============================================================

fn stress_timeout() -> Result<String, String> {
    let (server, port) = start_server("");
    let addr = format!("127.0.0.1:{}", port);

    server.register_handler("slow", Box::new(|_| {
        std::thread::sleep(Duration::from_secs(5));
        Ok(json!({"status": "ok"}))
    }));

    // Connect with short timeout
    let stream = TcpStream::connect(&addr).map_err(|e| format!("connect: {}", e))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();

    let req = WireMessage::new_request("client", "server", "slow", json!(null));
    let msg_data = req.to_bytes().map_err(|e| e.to_string())?;
    write_frame(&mut &stream, &msg_data).map_err(|e| format!("send: {}", e))?;

    let result = read_frame(&mut &stream);
    server.stop().map_err(|e| e)?;

    match result {
        Err(_) => Ok("timeout detected as expected".into()),
        Ok(_) => Err("expected timeout but got response".into()),
    }
}

// ============================================================
// STRESS TEST 6: Connection Pool
// ============================================================

fn stress_connection_pool() -> Result<String, String> {
    let (server, port) = start_server("");
    let addr = format!("127.0.0.1:{}", port);

    server.register_handler(
        "pool_test",
        Box::new(|payload| Ok(json!({"status": "ok", "conn_id": payload["id"]}))),
    );

    let pool = nemesis_cluster::transport::pool::ConnectionPool::new(
        nemesis_cluster::transport::pool::PoolConfig::default(),
    );

    let mut success = 0;
    for i in 0..5 {
        match pool.get_or_connect(&addr) {
            Ok(mut conn) => {
                let req = WireMessage::new_request(
                    "client",
                    "server",
                    "pool_test",
                    json!({"id": i}),
                );
                let data = req.to_bytes().map_err(|e| e.to_string())?;
                if conn.send(&data).is_ok() {
                    if let Ok(resp_data) = conn.recv() {
                        if let Ok(resp) = WireMessage::from_bytes(&resp_data) {
                            if resp.msg_type == "response" {
                                success += 1;
                            }
                        }
                    }
                }
                pool.return_connection(&addr, conn);
            }
            Err(_) => {}
        }
    }

    pool.close_all();
    server.stop().map_err(|e| e)?;

    if success >= 4 {
        Ok(format!("{}/5 pool connections succeeded", success))
    } else {
        Err(format!("only {}/5 pool connections succeeded", success))
    }
}

// ============================================================
// Main
// ============================================================

fn main() {
    println!("\n============================================================");
    println!("  NemesisBot Cluster P2P Integration Test Runner (Rust)");
    println!("  1:1 port of Go p2p_test.go + integration_stress.go");
    println!("============================================================\n");

    let mut runner = TestRunner::new();

    println!("--- P2P Tests (12) ---");
    runner.run("p2p/two_node_ping", test_two_node_ping);
    runner.run("p2p/bidirectional", test_bidirectional);
    runner.run("p2p/task_dispatch_callback", test_task_dispatch_callback);
    runner.run("p2p/task_status_lifecycle", test_task_status_lifecycle);
    runner.run("p2p/concurrent_multi_task", test_concurrent_multi_task);
    runner.run("p2p/auth_token_enforcement", test_auth_token_enforcement);
    runner.run("p2p/role_capabilities", test_role_capabilities);
    runner.run("p2p/encrypted_discovery", test_encrypted_discovery);
    runner.run("p2p/node_offline_bye", test_node_offline_bye);
    runner.run("p2p/invalid_action", test_invalid_action);
    runner.run("p2p/message_validation", test_message_validation);
    runner.run("p2p/full_e2e_encrypted_auth", test_full_e2e);

    println!("\n--- Stress Tests (6) ---");
    runner.run("stress/basic_rpc", stress_basic_rpc);
    runner.run("stress/concurrent_10", stress_concurrent_rpc);
    runner.run("stress/sequential_50", stress_sequential_rpc);
    runner.run("stress/large_payload", stress_large_payload);
    runner.run("stress/timeout", stress_timeout);
    runner.run("stress/connection_pool", stress_connection_pool);

    runner.summary();

    let failed = runner.results.iter().filter(|r| !r.passed).count();
    if failed > 0 {
        std::process::exit(1);
    }
}
