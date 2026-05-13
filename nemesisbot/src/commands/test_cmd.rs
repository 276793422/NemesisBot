//! Internal test commands (hidden from help).
//!
//! Tests the parent-child window architecture:
//! - Headless approval (no UI, auto-approve via WS)
//! - Real UI approval (popup window, user clicks approve/reject)
//! - Dashboard window via ProcessManager
//! - WebSocket server/client communication
//!
//! Usage:
//!   nemesisbot test approval-headless    — automated, no UI
//!   nemesisbot test approval-ui          — real popup, manual or automation
//!   nemesisbot test dashboard            — real dashboard window
//!   nemesisbot test ws                   — WS ping-pong

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum TestAction {
    /// Test headless approval flow (no UI, auto-approve via WS)
    ApprovalHeadless {
        /// Expected action result for assertion
        #[arg(long, default_value = "approved")]
        expected: String,
    },
    /// Test real UI approval window (blocks until window closes)
    ApprovalUi {
        /// Risk level for the approval request
        #[arg(long, default_value = "HIGH")]
        risk_level: String,
        /// Operation name
        #[arg(long, default_value = "file_write")]
        operation: String,
        /// Target path
        #[arg(long, default_value = "C:\\Temp\\test.txt")]
        target: String,
    },
    /// Test dashboard window via ProcessManager (blocks until window closes)
    Dashboard {
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long, default_value_t = 49000)]
        port: u16,
        #[arg(long, default_value = "276793422")]
        token: String,
    },
    /// Test WebSocket server/client communication
    Ws,
}

/// Run a test subcommand.
pub async fn run(action: TestAction) -> Result<()> {
    match action {
        TestAction::ApprovalHeadless { expected } => {
            run_approval_headless(&expected).await
        }
        TestAction::ApprovalUi { risk_level, operation, target } => {
            run_approval_ui(&risk_level, &operation, &target).await
        }
        TestAction::Dashboard { host, port, token } => {
            run_dashboard(&host, port, &token).await
        }
        TestAction::Ws => {
            run_ws_test().await
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Create and start a ProcessManager, returning it with its WS port.
async fn create_process_manager() -> Result<Arc<nemesis_desktop::process::ProcessManager>> {
    let pm = Arc::new(nemesis_desktop::process::ProcessManager::new());
    pm.start().await.map_err(|e| anyhow::anyhow!("ProcessManager start failed: {}", e))?;
    Ok(pm)
}

/// Build approval window data JSON.
fn make_approval_data(request_id: &str, operation: &str, risk_level: &str, target: &str) -> serde_json::Value {
    serde_json::json!({
        "request_id": request_id,
        "operation": operation,
        "operation_name": operation,
        "target": target,
        "risk_level": risk_level,
        "reason": format!("Test approval ({}) via nemesisbot test", operation),
        "timeout_seconds": 30,
        "context": {},
        "timestamp": chrono::Utc::now().timestamp(),
    })
}

/// Check if plugin-ui.dll exists next to the executable.
fn check_dll_exists() -> Result<std::path::PathBuf> {
    let exe = std::env::current_exe()?;
    let exe_dir = exe.parent().ok_or(anyhow::anyhow!("no exe dir"))?;
    let candidates = [
        exe_dir.join("plugins").join("plugin_ui.dll"),
        exe_dir.join("plugins").join("plugin-ui.dll"),
    ];
    for p in &candidates {
        if p.exists() {
            return Ok(p.clone());
        }
    }
    Err(anyhow::anyhow!(
        "plugin-ui.dll not found (searched: {:?})",
        candidates.iter().map(|p| p.display().to_string()).collect::<Vec<_>>()
    ))
}

/// Print a test result banner.
fn print_result(pass: bool, message: &str) {
    println!();
    if pass {
        println!("  PASS: {}", message);
    } else {
        println!("  FAIL: {}", message);
    }
    println!();
}

// ---------------------------------------------------------------------------
// Test: approval-headless
// ---------------------------------------------------------------------------

async fn run_approval_headless(expected: &str) -> Result<()> {
    println!("=== Headless Approval Test ===");
    println!();
    println!("  Mode:      headless (no UI)");
    println!("  Expected:  {}", expected);
    println!();

    // 1. Create and start ProcessManager
    let pm = create_process_manager().await?;
    println!("  [1/5] ProcessManager started (WS port: {})", pm.ws_port());

    // 2. Set headless env var so child process skips DLL/UI
    // SAFETY: This is a test command. Setting env vars is safe in this context.
    unsafe { std::env::set_var("NEMESISBOT_FORCE_HEADLESS", "1"); }
    println!("  [2/5] NEMESISBOT_FORCE_HEADLESS=1 set");

    // 3. Build approval data
    let request_id = format!("headless-{}", chrono::Utc::now().timestamp_millis());
    let data = make_approval_data(&request_id, "file_write", "HIGH", "C:\\Temp\\test.txt");
    println!("  [3/5] Approval data prepared (request_id: {})", request_id);

    // 4. Spawn child process
    let (child_id, result_rx) = pm.spawn_child("approval", &data)
        .map_err(|e| anyhow::anyhow!("spawn_child failed: {}", e))?;
    println!("  [4/5] Child spawned: {}", child_id);

    // 5. Wait for result
    println!("  [5/5] Waiting for result (timeout: 15s)...");
    let result = match result_rx {
        Some(rx) => {
            tokio::time::timeout(Duration::from_secs(15), rx).await
        }
        None => {
            print_result(false, "No result channel returned from spawn_child");
            let _ = pm.stop();
            unsafe { std::env::remove_var("NEMESISBOT_FORCE_HEADLESS"); }
            return Err(anyhow::anyhow!("no result channel"));
        }
    };

    // Cleanup
    let _ = pm.stop();

    match result {
        Ok(Ok(value)) => {
            let action = value.get("action").and_then(|v| v.as_str()).unwrap_or("unknown");
            let req_id = value.get("request_id").and_then(|v| v.as_str()).unwrap_or("?");
            println!();
            println!("  Result received:");
            println!("    action:      {}", action);
            println!("    request_id:  {}", req_id);
            println!("    raw:         {}", value);

            let pass = action == expected && req_id == request_id;
            print_result(pass, &format!(
                "Headless approval test (action={}, expected={}, request_id match={})",
                action, expected, req_id == request_id
            ));

            if pass { Ok(()) } else { Err(anyhow::anyhow!("test assertion failed")) }
        }
        Ok(Err(_)) => {
            print_result(false, "Result channel closed without value");
            Err(anyhow::anyhow!("channel closed"))
        }
        Err(_) => {
            print_result(false, "Timeout waiting for result (15s)");
            Err(anyhow::anyhow!("timeout"))
        }
    }
}

// ---------------------------------------------------------------------------
// Test: approval-ui
// ---------------------------------------------------------------------------

async fn run_approval_ui(risk_level: &str, operation: &str, target: &str) -> Result<()> {
    println!("=== UI Approval Test ===");
    println!();

    // Check DLL
    let dll_path = check_dll_exists()?;
    println!("  DLL:  {}", dll_path.display());

    // Create ProcessManager
    let pm = create_process_manager().await?;
    println!("  PM:   ProcessManager started (WS port: {})", pm.ws_port());
    println!();

    // Build approval data
    let request_id = format!("ui-{}", chrono::Utc::now().timestamp_millis());
    let data = make_approval_data(&request_id, operation, risk_level, target);

    println!("  Request ID:  {}", request_id);
    println!("  Operation:   {}", operation);
    println!("  Risk Level:  {}", risk_level);
    println!("  Target:      {}", target);
    println!();
    println!("  >>> Spawning approval window — click Approve or Reject <<<");
    println!("  >>> Timeout: 120 seconds <<<");
    println!();

    // Spawn child (real UI)
    let (child_id, result_rx) = pm.spawn_child("approval", &data)
        .map_err(|e| anyhow::anyhow!("spawn_child failed: {}", e))?;
    println!("  Child spawned: {}", child_id);

    // Wait for result (longer timeout — user needs to interact)
    let result = match result_rx {
        Some(rx) => {
            tokio::time::timeout(Duration::from_secs(120), rx).await
        }
        None => {
            print_result(false, "No result channel returned");
            let _ = pm.stop();
            return Err(anyhow::anyhow!("no result channel"));
        }
    };

    match result {
        Ok(Ok(value)) => {
            let action = value.get("action").and_then(|v| v.as_str()).unwrap_or("unknown");
            let req_id = value.get("request_id").and_then(|v| v.as_str()).unwrap_or("?");
            println!();
            println!("  Result received:");
            println!("    action:      {}", action);
            println!("    request_id:  {}", req_id);
            println!("    raw:         {}", value);

            let pass = !action.is_empty();
            print_result(pass, &format!("UI approval test completed (action={})", action));

            // Stop PM after printing result — child process cleanup may send
            // console signals (Ctrl+C) that could kill this process.
            let _ = pm.stop();

            if pass { Ok(()) } else { Err(anyhow::anyhow!("empty action")) }
        }
        Ok(Err(_)) => {
            print_result(false, "Result channel closed without value");
            let _ = pm.stop();
            Err(anyhow::anyhow!("channel closed"))
        }
        Err(_) => {
            print_result(false, "Timeout waiting for UI result (120s)");
            let _ = pm.stop();
            Err(anyhow::anyhow!("timeout"))
        }
    }
}

// ---------------------------------------------------------------------------
// Test: dashboard
// ---------------------------------------------------------------------------

async fn run_dashboard(host: &str, port: u16, token: &str) -> Result<()> {
    println!("=== Dashboard Window Test ===");
    println!();

    // Check DLL
    let dll_path = check_dll_exists()?;
    println!("  DLL:  {}", dll_path.display());

    // Create ProcessManager
    let pm = create_process_manager().await?;
    println!("  PM:   ProcessManager started (WS port: {})", pm.ws_port());
    println!();

    let backend_url = format!("http://{}:{}", host, port);
    println!("  Backend:    {}", backend_url);
    println!("  Token:      {}...", &token[..token.len().min(4)]);
    println!();

    // Dashboard data
    let data = serde_json::json!({
        "token": token,
        "web_port": port,
        "web_host": host,
    });

    // Spawn child (dashboard is persistent — no result_rx)
    let (child_id, result_rx) = pm.spawn_child("dashboard", &data)
        .map_err(|e| anyhow::anyhow!("spawn_child failed: {}", e))?;

    println!("  Dashboard child spawned: {}", child_id);
    println!("  Note: Dashboard windows are persistent (no auto-close).");
    println!("  Press Ctrl+C to exit.");

    // Dashboard is persistent — just wait for shutdown
    if result_rx.is_none() {
        // Wait indefinitely (user must Ctrl+C)
        tokio::signal::ctrl_c().await?;
        println!();
        println!("  Stopping...");
    }

    let _ = pm.stop();
    println!("  Dashboard test ended.");
    Ok(())
}

// ---------------------------------------------------------------------------
// Test: ws
// ---------------------------------------------------------------------------

async fn run_ws_test() -> Result<()> {
    println!("=== WebSocket Server/Client Test ===");
    println!();

    // 1. Start WS server
    let pm = create_process_manager().await?;
    println!("  [1/4] WS server started on port {}", pm.ws_port());

    // 2. Generate a test key
    let key_gen = pm.ws_server().key_generator();
    let test_key = key_gen.generate("test-child", 12345);
    println!("  [2/4] Test key generated");

    // 3. Create and connect WS client
    let ws_key_data = nemesis_desktop::websocket::client::WebSocketKey {
        key: test_key.clone(),
        port: pm.ws_port(),
        path: format!("/child/{}", test_key),
    };

    let client = Arc::new(nemesis_desktop::websocket::client::WebSocketClient::new(&ws_key_data));

    // Track notification receipt
    let received = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let received_clone = received.clone();
    client.register_notification_handler("test.ping", move |_msg| {
        eprintln!("  [WS client] Received test.ping notification!");
        received_clone.store(true, std::sync::atomic::Ordering::SeqCst);
    });

    println!("  [3/4] Connecting client...");
    client.connect().await.map_err(|e| anyhow::anyhow!("WS connect failed: {}", e))?;
    println!("  [3/4] Client connected");

    // 4. Give the server a moment to register the connection, then send notification
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Find the connection by child_id and verify it exists
    let conn = pm.ws_server().get_connection("test-child");
    match conn {
        Some(_) => println!("  [4/4] Connection found on server side"),
        None => {
            // Try by key
            let conn_by_key = pm.ws_server().get_connection(&test_key);
            if conn_by_key.is_some() {
                println!("  [4/4] Connection found by key");
            } else {
                print_result(false, "Connection not found on server side");
                let _ = pm.stop();
                return Err(anyhow::anyhow!("connection not found"));
            }
        }
    }

    // Send notification from server to client
    pm.ws_server().send_notification("test-child", "test.ping", serde_json::json!({"msg": "hello"}))
        .or_else(|_| pm.ws_server().send_notification(&test_key, "test.ping", serde_json::json!({"msg": "hello"})))
        .map_err(|e| anyhow::anyhow!("send_notification failed: {}", e))?;

    println!("  [4/4] Sent test.ping notification");

    // Wait for client to receive
    tokio::time::sleep(Duration::from_secs(2)).await;

    let got_it = received.load(std::sync::atomic::Ordering::SeqCst);

    client.close();
    let _ = pm.stop();

    print_result(got_it, &format!("WS notification test (received={})", got_it));

    if got_it { Ok(()) } else { Err(anyhow::anyhow!("notification not received")) }
}
