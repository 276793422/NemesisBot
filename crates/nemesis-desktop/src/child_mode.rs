//! Child mode - Entry point for child process execution.
//!
//! Handles the child process side of the parent-child architecture.
//! Mirrors Go process/child_entry.go: RunChildMode, handshake, WS key exchange, window data.
//!
//! Protocol: anon-pipe-v1 (JSON over stdin/stdout pipes)
//! Flow: handshake → ws_key → window_data → run window

use std::collections::HashMap;
use std::env;
use std::io::{BufRead, Read, Write};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::process::handshake::{
    PipeMessage, HandshakeResult,
};

// ---------------------------------------------------------------------------
// CLI argument helpers
// ---------------------------------------------------------------------------

/// Check if the current process was spawned as a child process (`--multiple`).
pub fn has_child_mode_flag() -> bool {
    env::args().any(|arg| arg == "--multiple")
}

/// Extract the child ID from `--child-id <value>`.
pub fn get_child_id() -> Option<String> {
    let args: Vec<String> = env::args().collect();
    for i in 0..args.len() {
        if args[i] == "--child-id" && i + 1 < args.len() {
            return Some(args[i + 1].clone());
        }
    }
    None
}

/// Extract the window type from `--window-type <value>`.
pub fn get_window_type() -> Option<String> {
    let args: Vec<String> = env::args().collect();
    for i in 0..args.len() {
        if args[i] == "--window-type" && i + 1 < args.len() {
            return Some(args[i + 1].clone());
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Pipe I/O wrappers (mirrors Go ReadCloser/WriteCloser)
// ---------------------------------------------------------------------------

/// Read pipe wrapper — reads JSON messages from a reader (stdin).
pub struct PipeReader<R: Read> {
    reader: std::io::BufReader<R>,
}

impl<R: Read> PipeReader<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader: std::io::BufReader::new(reader),
        }
    }

    /// Read a single JSON PipeMessage. Blocks until a complete message is available.
    pub fn read_message(&mut self) -> Result<PipeMessage, String> {
        let mut line = String::new();
        self.reader.read_line(&mut line)
            .map_err(|e| format!("pipe read: {}", e))?;
        // serde_json can handle both line-delimited and pretty-printed
        // We read a full line and parse
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Err("empty message from pipe".to_string());
        }
        serde_json::from_str(trimmed)
            .map_err(|e| format!("pipe parse: {} (input: {:?})", e, trimmed))
    }
}

/// Write pipe wrapper — writes JSON messages to a writer (stdout).
pub struct PipeWriter<W: Write> {
    writer: W,
}

impl<W: Write> PipeWriter<W> {
    pub fn new(writer: W) -> Self {
        Self { writer }
    }

    /// Write a single JSON PipeMessage.
    pub fn write_message(&mut self, msg: &PipeMessage) -> Result<(), String> {
        let json = serde_json::to_string(msg)
            .map_err(|e| format!("pipe serialize: {}", e))?;
        self.writer.write_all(json.as_bytes())
            .map_err(|e| format!("pipe write: {}", e))?;
        self.writer.write_all(b"\n")
            .map_err(|e| format!("pipe newline: {}", e))?;
        self.writer.flush()
            .map_err(|e| format!("pipe flush: {}", e))
    }
}

// ---------------------------------------------------------------------------
// Handshake protocol functions (mirrors Go handshake.go exactly)
// ---------------------------------------------------------------------------

/// Child-side handshake. Reads from parent (stdin), writes to parent (stdout).
///
/// 1. Wait for handshake message from parent (with timeout)
/// 2. Send ACK back
///
/// Mirrors Go ChildHandshake.
pub fn child_handshake(reader: &mut impl Read, writer: &mut impl Write) -> Result<HandshakeResult, String> {
    let mut pipe_in = PipeReader::new(reader);
    let mut pipe_out = PipeWriter::new(writer);

    // Read handshake message
    let msg = pipe_in.read_message()?;

    if msg.msg_type != "handshake" {
        return Err(format!("expected handshake, got {}", msg.msg_type));
    }

    // Send ACK
    let ack = PipeMessage::ack();
    pipe_out.write_message(&ack)?;

    Ok(HandshakeResult {
        success: true,
        window_id: None,
        error: None,
    })
}

/// Parent-side handshake. Sends handshake to child, waits for ACK.
///
/// 1. Send handshake message to child
/// 2. Wait for ACK from child (with timeout)
///
/// Mirrors Go ParentHandshake.
pub fn parent_handshake(writer: &mut impl Write, reader: &mut impl Read) -> Result<HandshakeResult, String> {
    let mut pipe_out = PipeWriter::new(writer);
    let mut pipe_in = PipeReader::new(reader);

    // Send handshake
    let hs = PipeMessage::handshake();
    pipe_out.write_message(&hs)?;

    // Wait for ACK
    let ack = pipe_in.read_message()?;
    if ack.msg_type != "ack" {
        return Err(format!("expected ack, got {}", ack.msg_type));
    }

    Ok(HandshakeResult {
        success: true,
        window_id: None,
        error: None,
    })
}

/// Receive WebSocket key from parent (child side).
///
/// 1. Read ws_key message from stdin
/// 2. Send ACK back
///
/// Mirrors Go ReceiveWSKey.
pub fn receive_ws_key(reader: &mut impl Read, writer: &mut impl Write) -> Result<(String, u16, String), String> {
    let mut pipe_in = PipeReader::new(reader);
    let mut pipe_out = PipeWriter::new(writer);

    let msg = pipe_in.read_message()?;

    if msg.msg_type != "ws_key" {
        return Err(format!("expected ws_key, got {}", msg.msg_type));
    }

    let key = msg.data.get("key")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let port = msg.data.get("port")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u16;
    let path = msg.data.get("path")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Send ACK
    let ack = PipeMessage::ack();
    pipe_out.write_message(&ack)?;

    Ok((key, port, path))
}

/// Send WebSocket key to child (parent side).
///
/// 1. Send ws_key message to child
/// 2. Wait for ACK
///
/// Mirrors Go SendWSKey.
pub fn send_ws_key(writer: &mut impl Write, reader: &mut impl Read, key: &str, port: u16, path: &str) -> Result<(), String> {
    let mut pipe_out = PipeWriter::new(writer);
    let mut pipe_in = PipeReader::new(reader);

    let msg = PipeMessage::ws_key(key, port, path);
    pipe_out.write_message(&msg)?;

    // Wait for ACK
    let ack = pipe_in.read_message()?;
    if ack.msg_type != "ack" {
        return Err(format!("expected ack, got {}", ack.msg_type));
    }

    Ok(())
}

/// Receive window data from parent (child side).
///
/// 1. Read window_data message from stdin
/// 2. Send ACK back
///
/// Mirrors Go ReceiveWindowData.
pub fn receive_window_data(reader: &mut impl Read, writer: &mut impl Write) -> Result<serde_json::Value, String> {
    let mut pipe_in = PipeReader::new(reader);
    let mut pipe_out = PipeWriter::new(writer);

    let msg = pipe_in.read_message()?;

    if msg.msg_type != "window_data" {
        return Err(format!("expected window_data, got {}", msg.msg_type));
    }

    // Send ACK
    let ack = PipeMessage::ack();
    pipe_out.write_message(&ack)?;

    // Extract data
    msg.data.get("data")
        .cloned()
        .ok_or_else(|| "missing data field in window_data message".to_string())
}

/// Send window data to child (parent side).
///
/// 1. Send window_data message to child
/// 2. Wait for ACK
///
/// Mirrors Go SendWindowData.
pub fn send_window_data(writer: &mut impl Write, reader: &mut impl Read, data: &serde_json::Value) -> Result<(), String> {
    let mut pipe_out = PipeWriter::new(writer);
    let mut pipe_in = PipeReader::new(reader);

    let msg = PipeMessage::window_data(data);
    pipe_out.write_message(&msg)?;

    // Wait for ACK
    let ack = pipe_in.read_message()?;
    if ack.msg_type != "ack" {
        return Err(format!("expected ack, got {}", ack.msg_type));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Run child mode (full handshake flow)
// ---------------------------------------------------------------------------

/// Window data for approval windows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalWindowData {
    pub request_id: String,
    pub operation: String,
    pub operation_name: String,
    pub target: String,
    pub risk_level: String,
    pub reason: String,
    pub timeout_seconds: i32,
    #[serde(default)]
    pub context: HashMap<String, String>,
    pub timestamp: i64,
}

/// Window data for dashboard windows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardWindowData {
    pub token: String,
    pub web_port: u16,
    pub web_host: String,
}

/// Run the child process mode — full handshake flow.
///
/// Mirrors Go RunChildMode exactly:
/// 1. Parse args (child_id, window_type)
/// 2. Handshake with parent via stdin/stdout
/// 3. Receive WebSocket key
/// 4. Receive window data
/// 5. Run appropriate window handler
pub async fn run_child_mode() -> Result<(), String> {
    if !has_child_mode_flag() {
        return Err("not in child mode".to_string());
    }

    let child_id = get_child_id()
        .ok_or("child-id not specified")?;
    let window_type = get_window_type()
        .ok_or("window-type not specified")?;

    // Allow forcing headless mode via environment variable (for testing)
    let window_type = if env::var("NEMESISBOT_FORCE_HEADLESS").as_deref() == Ok("1") && window_type == "approval" {
        eprintln!("[Child] Forced headless mode via NEMESISBOT_FORCE_HEADLESS=1");
        "headless".to_string()
    } else {
        window_type
    };

    eprintln!("[Child] Child ID: {}, Window Type: {}", child_id, window_type);

    // 2. Create stdin/stdout pipe wrappers
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut stdin_lock = stdin.lock();
    let mut stdout_lock = stdout.lock();

    // 3. Handshake
    eprintln!("[Child] Waiting for handshake...");
    let result = child_handshake(&mut stdin_lock, &mut stdout_lock)?;
    if !result.success {
        return Err("handshake failed".to_string());
    }
    eprintln!("[Child] Handshake completed");

    // 4. Receive WebSocket key
    eprintln!("[Child] Waiting for WebSocket key...");
    let (key, port, path) = receive_ws_key(&mut stdin_lock, &mut stdout_lock)?;
    eprintln!("[Child] WS key received: key={}, port={}, path={}", key, port, path);

    // 5. Receive window data
    eprintln!("[Child] Waiting for window data...");
    let window_data = receive_window_data(&mut stdin_lock, &mut stdout_lock)?;
    eprintln!("[Child] Window data received");

    // 6. Run window based on type
    run_window(&child_id, &window_type, &window_data, key, port, path)
}

/// Run the appropriate window based on window_type.
/// Mirrors Go runWailsWindow.
///
/// For "dashboard" and "approval" window types, attempts to load `plugin-ui.dll`
/// (or `plugin_ui.dll` on Windows) from the `plugins/` subdirectory next to the executable.
/// If the DLL is not found, falls back to a log message (graceful degradation).
///
/// For "headless" mode, returns immediately (auto-approve).
///
/// Connects to the parent's WebSocket server and registers a `window.bring_to_front`
/// handler that calls `plugin_request_bring_to_front()` in the DLL, enabling the
/// parent to request the window be brought to the foreground for deduplication.
fn run_window(
    child_id: &str,
    window_type: &str,
    window_data: &serde_json::Value,
    ws_key: String,
    ws_port: u16,
    ws_path: String,
) -> Result<(), String> {
    match window_type {
        "approval" => {
            let data: ApprovalWindowData = serde_json::from_value(window_data.clone())
                .map_err(|e| format!("invalid approval window data: {}", e))?;
            eprintln!("[Child] Starting approval window for request {}", data.request_id);
            load_and_run_plugin_window(window_type, window_data, &ws_key, ws_port, &ws_path)
        }
        "headless" => {
            let data: ApprovalWindowData = serde_json::from_value(window_data.clone())
                .map_err(|e| format!("invalid headless window data: {}", e))?;
            eprintln!("[Child] Starting headless window (auto-approve) for request {}", data.request_id);
            run_headless_auto_approve(child_id, &data, &ws_key, ws_port, &ws_path)
        }
        "dashboard" => {
            let data: DashboardWindowData = serde_json::from_value(window_data.clone())
                .map_err(|e| format!("invalid dashboard window data: {}", e))?;
            eprintln!("[Child] Starting dashboard window (web={}:{})", data.web_host, data.web_port);
            load_and_run_plugin_window(window_type, window_data, &ws_key, ws_port, &ws_path)
        }
        _ => Err(format!("unknown window type: {}", window_type)),
    }
}

/// Attempt to load plugin-ui.dll and call plugin_create_window.
///
/// DLL lookup order:
/// 1. `<exe_dir>/plugins/plugin_ui.dll`
/// 2. `<exe_dir>/plugins/plugin-ui.dll`
///
/// The DLL is expected to export C ABI functions:
/// - `plugin_init(config_dir, host)` → i32 (unified interface, optional)
/// - `plugin_free()` (unified interface, optional)
/// - `plugin_create_window(config_json: *const i8) -> i32`
/// - `plugin_request_bring_to_front()` (optional, used for window dedup)
///
/// Before calling `plugin_create_window`, connects to the parent's WebSocket
/// server and registers a `window.bring_to_front` notification handler that
/// calls `plugin_request_bring_to_front()` in the DLL.
fn load_and_run_plugin_window(
    window_type: &str,
    window_data: &serde_json::Value,
    ws_key: &str,
    ws_port: u16,
    ws_path: &str,
) -> Result<(), String> {
    let exe_dir = std::env::current_exe()
        .map_err(|e| format!("get exe path: {}", e))?
        .parent()
        .ok_or("no parent dir for exe")?
        .to_path_buf();

    // Try both naming conventions under plugins/ directory
    let dll_candidates = [
        exe_dir.join("plugins").join("plugin_ui.dll"),
        exe_dir.join("plugins").join("plugin-ui.dll"),
    ];

    let dll_path = dll_candidates.iter()
        .find(|p| p.exists())
        .ok_or_else(|| {
            format!(
                "plugin-ui.dll not found in {} (searched: {:?})",
                exe_dir.display(),
                dll_candidates.iter().map(|p| p.display().to_string()).collect::<Vec<_>>()
            )
        })?;

    eprintln!("[Child] Loading plugin DLL: {}", dll_path.display());

    // Load the DLL
    let lib = unsafe {
        libloading::Library::new(dll_path)
            .map_err(|e| format!("load DLL '{}': {}", dll_path.display(), e))?
    };

    // Try to call plugin_init if the DLL exports it (unified interface)
    let _init_result: i32 = unsafe {
        if let Ok(plugin_init_fn) = lib.get::<libloading::Symbol<unsafe extern "C" fn(*const i8, *const std::ffi::c_void) -> i32>>(b"plugin_init\0") {
            let c_config_dir = std::ffi::CString::new(".").unwrap_or_default();
            let ret = plugin_init_fn(c_config_dir.as_ptr(), std::ptr::null());
            eprintln!("[Child] plugin_init returned: {}", ret);
            ret
        } else {
            0 // No unified interface, that's OK
        }
    };

    // Get the plugin_create_window symbol
    let create_window: libloading::Symbol<unsafe extern "C" fn(*const i8) -> i32> = unsafe {
        lib.get(b"plugin_create_window\0")
            .map_err(|e| format!("get plugin_create_window symbol: {}", e))?
    };

    // Try to get the plugin_request_bring_to_front symbol (optional)
    let bring_to_front_fn: Option<libloading::Symbol<unsafe extern "C" fn()>> = unsafe {
        lib.get(b"plugin_request_bring_to_front\0").ok()
    };

    // Try to get the plugin_get_approval_result symbol (optional, for approval windows)
    let get_approval_result_fn: Option<libloading::Symbol<unsafe extern "C" fn() -> *const i8>> = unsafe {
        lib.get(b"plugin_get_approval_result\0").ok()
    };

    // Store the function pointer globally so the WS handler can call it
    if let Some(ref f) = bring_to_front_fn {
        let raw_fn: unsafe extern "C" fn() = **f;
        BRING_TO_FRONT_FN_PTR.set(raw_fn);
    }

    // Connect to parent's WebSocket server and register bring_to_front handler
    let ws_handle = connect_ws_with_handler(ws_key, ws_port, ws_path, bring_to_front_fn.is_some());

    // Build config JSON for the DLL
    let config = build_plugin_config(window_type, window_data);
    let c_config = std::ffi::CString::new(config)
        .map_err(|e| format!("CString conversion: {}", e))?;

    eprintln!("[Child] Calling plugin_create_window (blocking)...");

    // Call the DLL — blocks until the window closes (uses run_return so it returns normally).
    let result = unsafe { create_window(c_config.as_ptr()) };

    eprintln!("[Child] plugin_create_window returned: {}", result);

    // --- Approval result: read from DLL and send via WS ---
    if window_type == "approval" {
        if let Some(ref get_result_fn) = get_approval_result_fn {
            let ptr = unsafe { (**get_result_fn)() };
            let action = if ptr.is_null() {
                eprintln!("[Child] Approval window closed without action, defaulting to rejected");
                "rejected".to_string()
            } else {
                let c_str = unsafe { std::ffi::CStr::from_ptr(ptr) };
                let action = c_str.to_str().unwrap_or("rejected").to_string();
                eprintln!("[Child] Approval result from DLL: {}", action);
                action
            };

            let request_id = window_data.get("request_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            // Send approval.submit notification to parent via WS (with retries)
            if let Some(handle) = ws_handle.as_ref() {
                eprintln!("[Child] WS client exists, attempting to send approval.submit...");
                let params = serde_json::json!({
                    "action": action,
                    "request_id": request_id,
                });
                for attempt in 0..10 {
                    match handle.client.notify("approval.submit", params.clone()) {
                        Ok(()) => {
                            eprintln!("[Child] Sent approval.submit notification (action={})", action);
                            break;
                        }
                        Err(e) => {
                            eprintln!("[Child] approval.submit attempt {} failed: {}", attempt, e);
                            if attempt < 9 {
                                std::thread::sleep(std::time::Duration::from_millis(200));
                            }
                        }
                    }
                }
            } else {
                eprintln!("[Child] No WS client — cannot send approval.submit!");
            }
        }
    }

    // Give WS background thread time to flush any pending messages
    std::thread::sleep(std::time::Duration::from_millis(300));

    // Close the WebSocket client and signal the background thread to exit
    if let Some(handle) = ws_handle.as_ref() {
        handle.close();
    }

    // Call plugin_free if available (unified interface)
    unsafe {
        if let Ok(plugin_free_fn) = lib.get::<libloading::Symbol<unsafe extern "C" fn()>>(b"plugin_free\0") {
            plugin_free_fn();
            eprintln!("[Child] plugin_free called");
        }
    }

    // lib is dropped here, unloading the DLL
    if result != 0 {
        return Err(format!("plugin_create_window returned error code: {}", result));
    }

    Ok(())
}

/// WebSocket client handle with shutdown signal.
///
/// Wraps the WebSocket client and a shutdown flag used to stop the
/// background tokio runtime thread when the window closes.
struct WsHandle {
    client: Arc<crate::websocket::client::WebSocketClient>,
    shutdown: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl WsHandle {
    fn close(&self) {
        self.client.close();
        self.shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Create a WebSocket client connected to the parent server and register
/// a `window.bring_to_front` notification handler.
///
/// The handler calls `plugin_request_bring_to_front()` in the DLL, which
/// sets a flag that the DLL's event loop checks to bring the window to the
/// foreground.
///
/// Returns `None` if the WebSocket key is empty (no WS server available).
fn connect_ws_with_handler(
    ws_key: &str,
    ws_port: u16,
    ws_path: &str,
    has_bring_to_front: bool,
) -> Option<WsHandle> {
    // Skip WS connection if key is empty (no server)
    if ws_key.is_empty() || ws_port == 0 {
        eprintln!("[Child] No WebSocket key provided, skipping WS connection");
        return None;
    }

    let ws_key_data = crate::websocket::client::WebSocketKey {
        key: ws_key.to_string(),
        port: ws_port,
        path: if ws_path.is_empty() { "/ws".to_string() } else { ws_path.to_string() },
    };

    let client = Arc::new(crate::websocket::client::WebSocketClient::new(&ws_key_data));

    // Register the bring_to_front notification handler
    if has_bring_to_front {
        eprintln!("[Child] Registered window.bring_to_front handler (will set DLL flag)");
        client.register_notification_handler("window.bring_to_front", move |_msg| {
            eprintln!("[Child] Received window.bring_to_front notification");
            BRING_TO_FRONT_FN_PTR.call();
        });
    }

    // Shutdown flag for the background thread
    let shutdown = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let shutdown_clone = shutdown.clone();

    // Start a background thread with a tokio runtime to run the WS client
    let client_clone = Arc::clone(&client);
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        if let Ok(rt) = rt {
            let _ = rt.block_on(async {
                if let Err(e) = client_clone.connect().await {
                    eprintln!("[Child] WebSocket connect failed: {}", e);
                } else {
                    eprintln!("[Child] WebSocket connected to parent");
                    // Poll shutdown flag instead of sleeping 24h.
                    // The main thread sets this flag after calling client.close().
                    while !shutdown_clone.load(std::sync::atomic::Ordering::Relaxed) {
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    }
                }
            });
        }
    });

    Some(WsHandle { client, shutdown })
}

/// Global function pointer for plugin_request_bring_to_front.
/// Set by `load_and_run_plugin_window` before the WS handler runs.
///
/// SAFETY: Only written once before any WS handler runs, then only read.
struct BringToFrontFn {
    ptr: std::sync::atomic::AtomicPtr<()>,
}

impl BringToFrontFn {
    const fn new() -> Self {
        Self {
            ptr: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
        }
    }

    fn set(&self, f: unsafe extern "C" fn()) {
        self.ptr.store(f as *mut (), std::sync::atomic::Ordering::SeqCst);
    }

    fn call(&self) {
        let ptr = self.ptr.load(std::sync::atomic::Ordering::SeqCst);
        if !ptr.is_null() {
            let f: unsafe extern "C" fn() = unsafe { std::mem::transmute(ptr) };
            unsafe { f() };
        }
    }
}

static BRING_TO_FRONT_FN_PTR: BringToFrontFn = BringToFrontFn::new();

/// Run headless auto-approve: connect WS, wait 1 second, send approval.submit, exit.
///
/// This is the test mode — no DLL or UI needed. The child process connects
/// to the parent's WS server, auto-approves after a delay, and sends the
/// result via the `approval.submit` notification.
fn run_headless_auto_approve(
    child_id: &str,
    data: &ApprovalWindowData,
    ws_key: &str,
    ws_port: u16,
    ws_path: &str,
) -> Result<(), String> {
    // Connect WS client (no bring_to_front needed for headless)
    let ws_handle = connect_ws_with_handler(ws_key, ws_port, ws_path, false);

    eprintln!("[Child:headless] Connected, waiting 1s before auto-approve...");

    // Wait 1 second before auto-approving
    std::thread::sleep(std::time::Duration::from_secs(1));

    // Send approval.submit notification
    if let Some(ref handle) = ws_handle {
        let result = serde_json::json!({
            "action": "approved",
            "request_id": data.request_id,
        });

        // Retry up to 10 times (2s) waiting for WS connection
        for attempt in 0..10 {
            match handle.client.notify("approval.submit", result.clone()) {
                Ok(()) => {
                    eprintln!("[Child:headless] Sent approval.submit (auto-approve) for request {}", data.request_id);
                    break;
                }
                Err(_) if attempt < 9 => {
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
                Err(e) => {
                    eprintln!("[Child:headless] Failed to send approval.submit after retries: {}", e);
                }
            }
        }
    } else {
        eprintln!("[Child:headless] No WS client, result not sent");
    }

    // Keep alive briefly to ensure result delivery
    std::thread::sleep(std::time::Duration::from_secs(2));

    // Close WS client and signal background thread to exit
    if let Some(handle) = ws_handle.as_ref() {
        handle.close();
    }

    eprintln!("[Child:headless] Completed for child {}", child_id);
    Ok(())
}

// ---------------------------------------------------------------------------
// Approval HTML rendering (business logic — belongs in the host process,
// not in the UI DLL)
// ---------------------------------------------------------------------------

/// Risk level color mapping for the approval window.
fn risk_color(level: &str) -> &'static str {
    match level.to_uppercase().as_str() {
        "CRITICAL" => "#dc3545",
        "HIGH" => "#fd7e14",
        "MEDIUM" => "#ffc107",
        "LOW" => "#28a745",
        _ => "#6c757d",
    }
}

/// Escape HTML special characters.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Generate the approval HTML page.
///
/// This is business logic that produces the complete HTML page for the
/// approval window. The DLL just renders it as-is — zero content generation
/// in the UI layer.
fn render_approval_html(data: &ApprovalWindowData) -> String {
    let risk_color = risk_color(&data.risk_level);

    let risk_badge = format!(
        r#"<span style="background:{};color:#fff;padding:2px 10px;border-radius:4px;font-size:14px;">{}</span>"#,
        risk_color, data.risk_level
    );

    let timeout_secs = data.timeout_seconds.max(30);

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Security Approval - NemesisBot</title>
<style>
  * {{ margin: 0; padding: 0; box-sizing: border-box; }}
  body {{ font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; background: #1a1a2e; color: #e0e0e0; padding: 24px; }}
  .container {{ max-width: 700px; margin: 0 auto; }}
  .header {{ text-align: center; margin-bottom: 24px; }}
  .header h1 {{ font-size: 24px; color: #fff; }}
  .header p {{ color: #888; font-size: 14px; margin-top: 4px; }}
  .card {{ background: #16213e; border-radius: 12px; padding: 24px; margin-bottom: 16px; border: 1px solid #0f3460; }}
  .detail-row {{ display: flex; justify-content: space-between; padding: 10px 0; border-bottom: 1px solid #0f3460; }}
  .detail-row:last-child {{ border-bottom: none; }}
  .detail-label {{ color: #888; font-size: 14px; }}
  .detail-value {{ color: #fff; font-size: 14px; font-weight: 500; text-align: right; max-width: 60%; word-break: break-all; }}
  .reason {{ background: #0f3460; border-radius: 8px; padding: 16px; margin-top: 12px; font-size: 14px; line-height: 1.5; }}
  .actions {{ display: flex; gap: 16px; margin-top: 24px; }}
  .btn {{ flex: 1; padding: 14px; border: none; border-radius: 8px; font-size: 16px; font-weight: 600; cursor: pointer; }}
  .btn-approve {{ background: #28a745; color: #fff; }}
  .btn-approve:hover {{ background: #218838; }}
  .btn-reject {{ background: #dc3545; color: #fff; }}
  .btn-reject:hover {{ background: #c82333; }}
  .timer {{ text-align: center; color: #888; font-size: 13px; margin-top: 16px; }}
</style>
</head>
<body>
<div class="container">
  <div class="header">
    <h1>Security Approval Required</h1>
    <p>A dangerous operation requires your approval</p>
  </div>
  <div class="card">
    <div class="detail-row">
      <span class="detail-label">Operation</span>
      <span class="detail-value">{operation_name}</span>
    </div>
    <div class="detail-row">
      <span class="detail-label">Target</span>
      <span class="detail-value" style="font-family: monospace;">{target}</span>
    </div>
    <div class="detail-row">
      <span class="detail-label">Risk Level</span>
      <span class="detail-value">{risk_badge}</span>
    </div>
    <div class="detail-row">
      <span class="detail-label">Request ID</span>
      <span class="detail-value" style="font-family: monospace; font-size: 12px;">{request_id}</span>
    </div>
  </div>
  <div class="card">
    <span class="detail-label">Reason</span>
    <div class="reason">{reason}</div>
  </div>
  <div class="actions">
    <button class="btn btn-reject" onclick="respond('rejected')">Reject</button>
    <button class="btn btn-approve" onclick="respond('approved')">Approve</button>
  </div>
  <div class="timer" id="timer"></div>
</div>
<script>
const TIMEOUT = {timeout_seconds};
let remaining = TIMEOUT;
const timerEl = document.getElementById('timer');
let responseSent = false;

function respond(action) {{
  if (responseSent) return;
  responseSent = true;
  document.querySelectorAll('.btn').forEach(b => b.disabled = true);
  timerEl.textContent = 'Response: ' + action.toUpperCase();
  document.title = 'APPROVAL_RESULT:' + action;
  window.location.href = 'nemesis://localhost/__approval_result?action=' + action;
}}

setInterval(function() {{
  if (responseSent) return;
  if (remaining > 0) {{
    remaining--;
    var min = Math.floor(remaining / 60);
    var sec = remaining % 60;
    timerEl.textContent = 'Auto-reject in ' + min + ':' + (sec < 10 ? '0' : '') + sec;
  }} else {{
    respond('rejected');
  }}
}}, 1000);
</script>
</body>
</html>"#,
        operation_name = html_escape(&data.operation_name),
        target = html_escape(&data.target),
        risk_badge = risk_badge,
        request_id = html_escape(&data.request_id),
        reason = html_escape(&data.reason),
        timeout_seconds = timeout_secs,
    )
}

/// Build the JSON config string for plugin_create_window.
///
/// Generates the appropriate config for each window type:
/// - Dashboard: `url` (direct HTTP) + `init_script` (token injection)
/// - Approval: `html` (generated approval page) + `timeout_seconds`
fn build_plugin_config(window_type: &str, window_data: &serde_json::Value) -> String {
    match window_type {
        "dashboard" => {
            let data = DashboardWindowData::deserialize(window_data);
            match data {
                Ok(d) => {
                    let url = format!("http://{}:{}", d.web_host, d.web_port);
                    let backend_host_port = format!("{}:{}", d.web_host, d.web_port);
                    // Sanitize token for JS string embedding
                    let safe_token = d.token.replace('\\', "\\\\").replace('"', "\\\"");
                    let safe_backend = backend_host_port.replace('\\', "\\\\").replace('"', "\\\"");
                    let init_script = format!(
                        r#"window.__DASHBOARD_TOKEN__="{}";window.__DASHBOARD_BACKEND__="{}";"#,
                        safe_token, safe_backend
                    );
                    serde_json::json!({
                        "window_type": "dashboard",
                        "title": "NemesisBot Dashboard",
                        "width": 1280.0,
                        "height": 800.0,
                        "url": url,
                        "init_script": init_script,
                    }).to_string()
                }
                Err(_) => serde_json::json!({
                    "window_type": "dashboard",
                    "title": "NemesisBot Dashboard",
                }).to_string(),
            }
        }
        "approval" => {
            let data = ApprovalWindowData::deserialize(window_data);
            match data {
                Ok(d) => {
                    let html = render_approval_html(&d);
                    let timeout_secs = d.timeout_seconds.max(30) as u64;
                    serde_json::json!({
                        "window_type": "approval",
                        "title": "Security Approval - NemesisBot",
                        "width": 750.0,
                        "height": 700.0,
                        "html": html,
                        "timeout_seconds": timeout_secs,
                    }).to_string()
                }
                Err(_) => serde_json::json!({
                    "window_type": "approval",
                    "title": "Security Approval - NemesisBot",
                }).to_string(),
            }
        }
        _ => serde_json::json!({
            "window_type": window_type,
        }).to_string(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_has_child_mode_flag() {
        // Test runner doesn't pass --multiple, so should be false
        assert!(!has_child_mode_flag());
    }

    #[test]
    fn test_child_handshake_success() {
        // Simulate parent sending handshake, child reading it
        let parent_msg = r#"{"type":"handshake","version":"1.0","data":{"protocol":"anon-pipe-v1","version":"1.0"}}"#;
        let mut input = Cursor::new(parent_msg.to_string());
        let mut output = Vec::new();

        let result = child_handshake(&mut input, &mut output).unwrap();
        assert!(result.success);

        // Verify ACK was written
        let output_str = String::from_utf8(output).unwrap();
        let ack: PipeMessage = serde_json::from_str(output_str.trim()).unwrap();
        assert_eq!(ack.msg_type, "ack");
    }

    #[test]
    fn test_child_handshake_wrong_type() {
        let parent_msg = r#"{"type":"ws_key","version":"1.0","data":{}}"#;
        let mut input = Cursor::new(parent_msg.to_string());
        let mut output = Vec::new();

        let result = child_handshake(&mut input, &mut output);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expected handshake"));
    }

    #[test]
    fn test_parent_handshake_success() {
        // Parent writes handshake, then reads ACK
        let ack_response = r#"{"type":"ack","version":"1.0","data":{"status":"ok"}}"#;
        let mut input = Cursor::new(ack_response.to_string());
        let mut output = Vec::new();

        let result = parent_handshake(&mut output, &mut input).unwrap();
        assert!(result.success);

        // Verify handshake was written
        let output_str = String::from_utf8(output).unwrap();
        let hs: PipeMessage = serde_json::from_str(output_str.trim()).unwrap();
        assert_eq!(hs.msg_type, "handshake");
    }

    #[test]
    fn test_receive_ws_key() {
        let ws_msg = r#"{"type":"ws_key","version":"1.0","data":{"key":"abc123","port":8080,"path":"/ws"}}"#;
        let mut input = Cursor::new(ws_msg.to_string());
        let mut output = Vec::new();

        let (key, port, path) = receive_ws_key(&mut input, &mut output).unwrap();
        assert_eq!(key, "abc123");
        assert_eq!(port, 8080);
        assert_eq!(path, "/ws");

        // Verify ACK was written
        let output_str = String::from_utf8(output).unwrap();
        let ack: PipeMessage = serde_json::from_str(output_str.trim()).unwrap();
        assert!(ack.is_ack());
    }

    #[test]
    fn test_send_ws_key() {
        let ack_response = r#"{"type":"ack","version":"1.0","data":{"status":"ok"}}"#;
        let mut input = Cursor::new(ack_response.to_string());
        let mut output = Vec::new();

        send_ws_key(&mut output, &mut input, "test-key", 9090, "/api").unwrap();

        let output_str = String::from_utf8(output).unwrap();
        let msg: PipeMessage = serde_json::from_str(output_str.trim()).unwrap();
        assert!(msg.is_ws_key());
        assert_eq!(msg.data["key"], serde_json::json!("test-key"));
        assert_eq!(msg.data["port"], serde_json::json!(9090));
    }

    #[test]
    fn test_receive_window_data() {
        let wd_msg = r#"{"type":"window_data","version":"1.0","data":{"data":{"request_id":"r1","operation":"file_write","operation_name":"Write File","target":"test.txt","risk_level":"HIGH","reason":"test","timeout_seconds":30,"context":{},"timestamp":1234567890}}}"#;
        let mut input = Cursor::new(wd_msg.to_string());
        let mut output = Vec::new();

        let data = receive_window_data(&mut input, &mut output).unwrap();
        assert_eq!(data["request_id"], "r1");
        assert_eq!(data["risk_level"], "HIGH");

        // Verify ACK
        let output_str = String::from_utf8(output).unwrap();
        let ack: PipeMessage = serde_json::from_str(output_str.trim()).unwrap();
        assert!(ack.is_ack());
    }

    #[test]
    fn test_send_window_data() {
        let ack_response = r#"{"type":"ack","version":"1.0","data":{"status":"ok"}}"#;
        let mut input = Cursor::new(ack_response.to_string());
        let mut output = Vec::new();

        let data = serde_json::json!({"title": "Test Window"});
        send_window_data(&mut output, &mut input, &data).unwrap();

        let output_str = String::from_utf8(output).unwrap();
        let msg: PipeMessage = serde_json::from_str(output_str.trim()).unwrap();
        assert!(msg.is_window_data());
    }

    #[test]
    fn test_full_handshake_flow() {
        // Simulate full parent-child handshake flow:
        // Parent writes handshake → Child reads handshake → Child writes ACK → Parent reads ACK
        let mut parent_to_child = Vec::new();
        let mut child_to_parent = Vec::new();

        // Parent sends handshake
        {
            let mut writer = PipeWriter::new(&mut parent_to_child);
            writer.write_message(&PipeMessage::handshake()).unwrap();
        }

        // Child receives handshake and sends ACK
        {
            let mut reader = PipeReader::new(Cursor::new(String::from_utf8(parent_to_child.clone()).unwrap()));
            let mut writer = PipeWriter::new(&mut child_to_parent);
            let msg = reader.read_message().unwrap();
            assert!(msg.is_handshake());
            writer.write_message(&PipeMessage::ack()).unwrap();
        }

        // Parent reads ACK
        {
            let mut reader = PipeReader::new(Cursor::new(String::from_utf8(child_to_parent.clone()).unwrap()));
            let ack = reader.read_message().unwrap();
            assert!(ack.is_ack());
        }
    }

    #[test]
    fn test_full_ws_key_exchange() {
        let mut parent_to_child = Vec::new();
        let mut child_to_parent = Vec::new();

        // Parent sends ws_key
        {
            let mut writer = PipeWriter::new(&mut parent_to_child);
            writer.write_message(&PipeMessage::ws_key("my-key", 8080, "/ws")).unwrap();
        }

        // Child receives ws_key and sends ACK
        {
            let mut reader = PipeReader::new(Cursor::new(String::from_utf8(parent_to_child.clone()).unwrap()));
            let mut writer = PipeWriter::new(&mut child_to_parent);
            let msg = reader.read_message().unwrap();
            assert!(msg.is_ws_key());
            assert_eq!(msg.data["key"], serde_json::json!("my-key"));
            writer.write_message(&PipeMessage::ack()).unwrap();
        }

        // Parent reads ACK
        {
            let mut reader = PipeReader::new(Cursor::new(String::from_utf8(child_to_parent.clone()).unwrap()));
            let ack = reader.read_message().unwrap();
            assert!(ack.is_ack());
        }
    }

    #[test]
    fn test_approval_window_data_serde() {
        let data = ApprovalWindowData {
            request_id: "r1".to_string(),
            operation: "file_write".to_string(),
            operation_name: "Write File".to_string(),
            target: "test.txt".to_string(),
            risk_level: "HIGH".to_string(),
            reason: "test reason".to_string(),
            timeout_seconds: 30,
            context: HashMap::new(),
            timestamp: 1234567890,
        };
        let json = serde_json::to_string(&data).unwrap();
        let parsed: ApprovalWindowData = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.request_id, "r1");
        assert_eq!(parsed.risk_level, "HIGH");
    }

    #[test]
    fn test_dashboard_window_data_serde() {
        let data = DashboardWindowData {
            token: "tok123".to_string(),
            web_port: 8080,
            web_host: "0.0.0.0".to_string(),
        };
        let json = serde_json::to_string(&data).unwrap();
        let parsed: DashboardWindowData = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.token, "tok123");
        assert_eq!(parsed.web_port, 8080);
    }

    #[test]
    fn test_pipe_message_roundtrip() {
        let msg = PipeMessage::handshake();
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: PipeMessage = serde_json::from_str(&json).unwrap();
        assert!(parsed.is_handshake());
        assert_eq!(parsed.version, "1.0");
    }

    #[test]
    fn test_pipe_reader_empty_input() {
        let input = Cursor::new(String::new());
        let mut reader = PipeReader::new(input);
        let result = reader.read_message();
        assert!(result.is_err());
    }

    #[test]
    fn test_pipe_reader_empty_line() {
        let input = Cursor::new("\n\n".to_string());
        let mut reader = PipeReader::new(input);
        let result = reader.read_message();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty message"));
    }

    #[test]
    fn test_pipe_reader_invalid_json() {
        let input = Cursor::new("not json\n".to_string());
        let mut reader = PipeReader::new(input);
        let result = reader.read_message();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("pipe parse"));
    }

    #[test]
    fn test_pipe_writer_writes_json() {
        let mut output = Vec::new();
        let mut writer = PipeWriter::new(&mut output);
        writer.write_message(&PipeMessage::ack()).unwrap();
        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.contains("ack"));
        assert!(output_str.ends_with('\n'));
    }

    #[test]
    fn test_pipe_writer_multiple_messages() {
        let mut output = Vec::new();
        let mut writer = PipeWriter::new(&mut output);
        writer.write_message(&PipeMessage::handshake()).unwrap();
        writer.write_message(&PipeMessage::ack()).unwrap();
        let output_str = String::from_utf8(output).unwrap();
        let lines: Vec<&str> = output_str.lines().collect();
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn test_get_child_id_not_set() {
        // Test runner doesn't pass --child-id, so should be None
        assert!(get_child_id().is_none());
    }

    #[test]
    fn test_get_window_type_not_set() {
        // Test runner doesn't pass --window-type, so should be None
        assert!(get_window_type().is_none());
    }

    #[test]
    fn test_child_handshake_eof() {
        let mut input = Cursor::new(String::new());
        let mut output = Vec::new();
        let result = child_handshake(&mut input, &mut output);
        assert!(result.is_err());
    }

    #[test]
    fn test_parent_handshake_wrong_response() {
        let wrong_response = r#"{"type":"handshake","version":"1.0","data":{}}"#;
        let mut input = Cursor::new(wrong_response.to_string());
        let mut output = Vec::new();
        let result = parent_handshake(&mut output, &mut input);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expected ack"));
    }

    #[test]
    fn test_receive_ws_key_wrong_type() {
        let wrong_msg = r#"{"type":"handshake","version":"1.0","data":{}}"#;
        let mut input = Cursor::new(wrong_msg.to_string());
        let mut output = Vec::new();
        let result = receive_ws_key(&mut input, &mut output);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expected ws_key"));
    }

    #[test]
    fn test_receive_ws_key_defaults() {
        let ws_msg = r#"{"type":"ws_key","version":"1.0","data":{}}"#;
        let mut input = Cursor::new(ws_msg.to_string());
        let mut output = Vec::new();
        let (key, port, path) = receive_ws_key(&mut input, &mut output).unwrap();
        assert_eq!(key, "");
        assert_eq!(port, 0);
        assert_eq!(path, "");
    }

    #[test]
    fn test_receive_window_data_wrong_type() {
        let wrong_msg = r#"{"type":"handshake","version":"1.0","data":{}}"#;
        let mut input = Cursor::new(wrong_msg.to_string());
        let mut output = Vec::new();
        let result = receive_window_data(&mut input, &mut output);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expected window_data"));
    }

    #[test]
    fn test_receive_window_data_missing_data_field() {
        let msg = r#"{"type":"window_data","version":"1.0","data":{}}"#;
        let mut input = Cursor::new(msg.to_string());
        let mut output = Vec::new();
        let result = receive_window_data(&mut input, &mut output);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("missing data field"));
    }

    #[test]
    fn test_approval_window_data_with_context() {
        let mut context = HashMap::new();
        context.insert("user".to_string(), "alice".to_string());
        context.insert("channel".to_string(), "web".to_string());
        let data = ApprovalWindowData {
            request_id: "req-1".to_string(),
            operation: "file_write".to_string(),
            operation_name: "Write".to_string(),
            target: "/tmp/test.txt".to_string(),
            risk_level: "MEDIUM".to_string(),
            reason: "user request".to_string(),
            timeout_seconds: 60,
            context,
            timestamp: 1700000000,
        };
        let json = serde_json::to_string(&data).unwrap();
        let parsed: ApprovalWindowData = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.context.get("user").unwrap(), "alice");
        assert_eq!(parsed.context.get("channel").unwrap(), "web");
    }

    #[test]
    fn test_run_window_unknown_type() {
        let data = serde_json::json!({});
        let result = run_window("child-1", "unknown_type", &data, "key".to_string(), 8080, "/ws".to_string());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown window type"));
    }

    #[test]
    fn test_run_window_approval() {
        let data = serde_json::json!({
            "request_id": "r1",
            "operation": "file_write",
            "operation_name": "Write",
            "target": "test.txt",
            "risk_level": "HIGH",
            "reason": "test",
            "timeout_seconds": 30,
            "context": {},
            "timestamp": 1234567890
        });
        let result = run_window("child-1", "approval", &data, "key".to_string(), 8080, "/ws".to_string());
        // Without plugin-ui.dll, expect "not found" error
        // With plugin-ui.dll, expect Ok(()) or a runtime error from the DLL
        match result {
            Ok(()) => {},
            Err(e) => assert!(e.contains("plugin") || e.contains("not found") || e.contains("DLL"),
                "unexpected error: {}", e),
        }
    }

    #[test]
    fn test_run_window_headless() {
        let data = serde_json::json!({
            "request_id": "r2",
            "operation": "file_read",
            "operation_name": "Read",
            "target": "test.txt",
            "risk_level": "LOW",
            "reason": "auto",
            "timeout_seconds": 10,
            "context": {},
            "timestamp": 1234567890
        });
        let result = run_window("child-2", "headless", &data, "key".to_string(), 8080, "/ws".to_string());
        assert!(result.is_ok());
    }

    #[test]
    fn test_run_window_dashboard() {
        let data = serde_json::json!({
            "token": "tok123",
            "web_port": 8080,
            "web_host": "0.0.0.0"
        });
        let result = run_window("child-3", "dashboard", &data, "key".to_string(), 8080, "/ws".to_string());
        // Without plugin-ui.dll, expect "not found" error
        // With plugin-ui.dll, expect Ok(()) or a runtime error from the DLL
        match result {
            Ok(()) => {},
            Err(e) => assert!(e.contains("plugin") || e.contains("not found") || e.contains("DLL"),
                "unexpected error: {}", e),
        }
    }

    #[test]
    fn test_run_window_approval_invalid_data() {
        let data = serde_json::json!({"invalid": "data"});
        let result = run_window("child-1", "approval", &data, "key".to_string(), 8080, "/ws".to_string());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid approval window data"));
    }

    #[test]
    fn test_run_window_headless_invalid_data() {
        let data = serde_json::json!({"invalid": "data"});
        let result = run_window("child-1", "headless", &data, "key".to_string(), 8080, "/ws".to_string());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid headless window data"));
    }

    #[test]
    fn test_run_window_dashboard_invalid_data() {
        let data = serde_json::json!({"invalid": "data"});
        let result = run_window("child-1", "dashboard", &data, "key".to_string(), 8080, "/ws".to_string());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid dashboard window data"));
    }

    #[test]
    fn test_build_plugin_config_dashboard() {
        let data = serde_json::json!({
            "token": "mytoken",
            "web_port": 49000,
            "web_host": "127.0.0.1"
        });
        let config = build_plugin_config("dashboard", &data);
        let parsed: serde_json::Value = serde_json::from_str(&config).unwrap();
        assert_eq!(parsed["window_type"], "dashboard");
        assert_eq!(parsed["title"], "NemesisBot Dashboard");
        assert_eq!(parsed["url"], "http://127.0.0.1:49000");
        assert!(parsed["init_script"].as_str().unwrap().contains("mytoken"));
        assert!(parsed["init_script"].as_str().unwrap().contains("127.0.0.1:49000"));
        assert_eq!(parsed["width"], 1280.0);
        assert_eq!(parsed["height"], 800.0);
        // Old fields should NOT be present
        assert!(parsed.get("backend_url").is_none());
        assert!(parsed.get("auth_token").is_none());
    }

    #[test]
    fn test_build_plugin_config_approval() {
        let data = serde_json::json!({
            "request_id": "req-1",
            "operation": "file_write",
            "operation_name": "Write File",
            "target": "/tmp/test.txt",
            "risk_level": "HIGH",
            "reason": "user requested",
            "timeout_seconds": 60,
            "context": {},
            "timestamp": 1234567890
        });
        let config = build_plugin_config("approval", &data);
        let parsed: serde_json::Value = serde_json::from_str(&config).unwrap();
        assert_eq!(parsed["window_type"], "approval");
        assert_eq!(parsed["title"], "Security Approval - NemesisBot");
        assert_eq!(parsed["width"], 750.0);
        assert_eq!(parsed["height"], 700.0);
        // HTML content should be generated
        let html = parsed["html"].as_str().unwrap();
        assert!(html.contains("req-1"));
        assert!(html.contains("Write File"));
        assert!(html.contains("/tmp/test.txt"));
        assert!(html.contains("HIGH"));
        assert!(html.contains("__approval_result"));
        // Old field should NOT be present
        assert!(parsed.get("approval_data").is_none());
    }

    #[test]
    fn test_load_and_run_plugin_window_dll_not_found() {
        let data = serde_json::json!({
            "token": "test",
            "web_port": 8080,
            "web_host": "127.0.0.1"
        });
        let result = load_and_run_plugin_window("dashboard", &data, "key", 8080, "/ws");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("plugin") || err.contains("not found") || err.contains("DLL"),
            "unexpected error: {}", err);
    }

    #[test]
    fn test_send_ws_key_wrong_ack() {
        let wrong_ack = r#"{"type":"handshake","version":"1.0","data":{}}"#;
        let mut input = Cursor::new(wrong_ack.to_string());
        let mut output = Vec::new();
        let result = send_ws_key(&mut output, &mut input, "key", 8080, "/ws");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expected ack"));
    }

    #[test]
    fn test_send_window_data_wrong_ack() {
        let wrong_ack = r#"{"type":"handshake","version":"1.0","data":{}}"#;
        let mut input = Cursor::new(wrong_ack.to_string());
        let mut output = Vec::new();
        let data = serde_json::json!({"test": true});
        let result = send_window_data(&mut output, &mut input, &data);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expected ack"));
    }

    #[test]
    fn test_receive_ws_key_partial_data() {
        let ws_msg = r#"{"type":"ws_key","version":"1.0","data":{"key":"only-key"}}"#;
        let mut input = Cursor::new(ws_msg.to_string());
        let mut output = Vec::new();
        let (key, port, path) = receive_ws_key(&mut input, &mut output).unwrap();
        assert_eq!(key, "only-key");
        assert_eq!(port, 0); // missing port defaults to 0
        assert_eq!(path, ""); // missing path defaults to empty
    }

    #[test]
    fn test_parent_handshake_eof() {
        let mut input = Cursor::new(String::new());
        let mut output = Vec::new();
        let result = parent_handshake(&mut output, &mut input);
        assert!(result.is_err());
    }

    #[test]
    fn test_pipe_reader_multiple_lines() {
        let input = Cursor::new(
            r#"{"type":"handshake","version":"1.0","data":{}}
{"type":"ack","version":"1.0","data":{}}
"#.to_string()
        );
        let mut reader = PipeReader::new(input);
        let msg1 = reader.read_message().unwrap();
        assert!(msg1.is_handshake());
        let msg2 = reader.read_message().unwrap();
        assert!(msg2.is_ack());
    }

    #[test]
    fn test_approval_window_data_default_fields() {
        let json = r#"{"request_id":"r1","operation":"file_write","operation_name":"","target":"test.txt","risk_level":"HIGH","reason":"","timeout_seconds":0,"timestamp":0}"#;
        let data: ApprovalWindowData = serde_json::from_str(json).unwrap();
        assert_eq!(data.request_id, "r1");
        assert_eq!(data.operation_name, "");
        assert_eq!(data.reason, "");
        assert_eq!(data.timeout_seconds, 0);
        assert!(data.context.is_empty());
        assert_eq!(data.timestamp, 0);
    }

    #[test]
    fn test_dashboard_window_data_from_json() {
        let json = r#"{"token":"abc","web_port":9090,"web_host":"localhost"}"#;
        let data: DashboardWindowData = serde_json::from_str(json).unwrap();
        assert_eq!(data.token, "abc");
        assert_eq!(data.web_port, 9090);
        assert_eq!(data.web_host, "localhost");
    }

    #[test]
    fn test_child_handshake_eof_reads_empty() {
        // Empty stdin → read_line returns 0 → error
        let mut input = Cursor::new(String::new());
        let mut output = Vec::new();
        let result = child_handshake(&mut input, &mut output);
        assert!(result.is_err());
    }

    #[test]
    fn test_bring_to_front_fn_ptr_null() {
        // Without a DLL loaded, calling should be a no-op (ptr is null)
        BRING_TO_FRONT_FN_PTR.call();
        // Should not panic
    }

    #[test]
    fn test_connect_ws_with_handler_no_key() {
        // Empty key should return None
        let result = connect_ws_with_handler("", 0, "", false);
        assert!(result.is_none());
    }

    #[test]
    fn test_connect_ws_with_handler_zero_port() {
        let result = connect_ws_with_handler("some-key", 0, "/ws", false);
        assert!(result.is_none());
    }

    // --- Approval HTML rendering tests ---

    #[test]
    fn test_risk_color() {
        assert_eq!(risk_color("CRITICAL"), "#dc3545");
        assert_eq!(risk_color("HIGH"), "#fd7e14");
        assert_eq!(risk_color("MEDIUM"), "#ffc107");
        assert_eq!(risk_color("LOW"), "#28a745");
        assert_eq!(risk_color("unknown"), "#6c757d");
        assert_eq!(risk_color("high"), "#fd7e14"); // case insensitive
    }

    #[test]
    fn test_html_escape() {
        assert_eq!(html_escape("<script>alert('xss')</script>"),
            "&lt;script&gt;alert(&#39;xss&#39;)&lt;/script&gt;");
        assert_eq!(html_escape("a&b<c>d\"e'f"),
            "a&amp;b&lt;c&gt;d&quot;e&#39;f");
        assert_eq!(html_escape("normal text"), "normal text");
    }

    #[test]
    fn test_render_approval_html_basic() {
        let data = ApprovalWindowData {
            request_id: "req-1".to_string(),
            operation: "file_write".to_string(),
            operation_name: "Write File".to_string(),
            target: "/tmp/test.txt".to_string(),
            risk_level: "HIGH".to_string(),
            reason: "User requested write".to_string(),
            timeout_seconds: 10,
            context: HashMap::new(),
            timestamp: 1234567890,
        };
        let html = render_approval_html(&data);
        assert!(html.contains("req-1"));
        assert!(html.contains("Write File"));
        assert!(html.contains("/tmp/test.txt"));
        assert!(html.contains("HIGH"));
        assert!(html.contains("User requested write"));
        assert!(html.contains("#fd7e14")); // HIGH risk color
        assert!(html.contains("respond('approved')"));
        assert!(html.contains("respond('rejected')"));
        assert!(html.contains("__approval_result"));
        assert!(html.contains("TIMEOUT = 30")); // min 30 seconds
    }

    #[test]
    fn test_render_approval_html_critical_risk() {
        let data = ApprovalWindowData {
            request_id: "req-crit".to_string(),
            operation: "process_exec".to_string(),
            operation_name: "Execute".to_string(),
            target: "cmd.exe".to_string(),
            risk_level: "CRITICAL".to_string(),
            reason: "Dangerous".to_string(),
            timeout_seconds: 30,
            context: HashMap::new(),
            timestamp: 1234567890,
        };
        let html = render_approval_html(&data);
        assert!(html.contains("#dc3545")); // CRITICAL risk color (red)
    }

    #[test]
    fn test_render_approval_html_xss_protection() {
        let data = ApprovalWindowData {
            request_id: "req-xss".to_string(),
            operation: "file_write".to_string(),
            operation_name: "<script>alert(1)</script>".to_string(),
            target: "<img onerror=alert(1) src=x>".to_string(),
            risk_level: "HIGH".to_string(),
            reason: "\"injection\" attempt".to_string(),
            timeout_seconds: 30,
            context: HashMap::new(),
            timestamp: 1234567890,
        };
        let html = render_approval_html(&data);
        // Should NOT contain raw HTML tags from input
        assert!(!html.contains("<script>alert(1)</script>"));
        assert!(!html.contains("<img onerror"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("&lt;img"));
    }
}
