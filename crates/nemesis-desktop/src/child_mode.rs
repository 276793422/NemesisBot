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

use crate::process::handshake::{HandshakeResult, PipeMessage};

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
        self.reader
            .read_line(&mut line)
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
        let json = serde_json::to_string(msg).map_err(|e| format!("pipe serialize: {}", e))?;
        self.writer
            .write_all(json.as_bytes())
            .map_err(|e| format!("pipe write: {}", e))?;
        self.writer
            .write_all(b"\n")
            .map_err(|e| format!("pipe newline: {}", e))?;
        self.writer
            .flush()
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
pub fn child_handshake(
    reader: &mut impl Read,
    writer: &mut impl Write,
) -> Result<HandshakeResult, String> {
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
pub fn parent_handshake(
    writer: &mut impl Write,
    reader: &mut impl Read,
) -> Result<HandshakeResult, String> {
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
pub fn receive_ws_key(
    reader: &mut impl Read,
    writer: &mut impl Write,
) -> Result<(String, u16, String), String> {
    let mut pipe_in = PipeReader::new(reader);
    let mut pipe_out = PipeWriter::new(writer);

    let msg = pipe_in.read_message()?;

    if msg.msg_type != "ws_key" {
        return Err(format!("expected ws_key, got {}", msg.msg_type));
    }

    let key = msg
        .data
        .get("key")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let port = msg.data.get("port").and_then(|v| v.as_u64()).unwrap_or(0) as u16;
    let path = msg
        .data
        .get("path")
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
pub fn send_ws_key(
    writer: &mut impl Write,
    reader: &mut impl Read,
    key: &str,
    port: u16,
    path: &str,
) -> Result<(), String> {
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
pub fn receive_window_data(
    reader: &mut impl Read,
    writer: &mut impl Write,
) -> Result<serde_json::Value, String> {
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
    msg.data
        .get("data")
        .cloned()
        .ok_or_else(|| "missing data field in window_data message".to_string())
}

/// Send window data to child (parent side).
///
/// 1. Send window_data message to child
/// 2. Wait for ACK
///
/// Mirrors Go SendWindowData.
pub fn send_window_data(
    writer: &mut impl Write,
    reader: &mut impl Read,
    data: &serde_json::Value,
) -> Result<(), String> {
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

    let child_id = get_child_id().ok_or("child-id not specified")?;
    let window_type = get_window_type().ok_or("window-type not specified")?;

    // Allow forcing headless mode via environment variable (for testing)
    let window_type = if env::var("NEMESISBOT_FORCE_HEADLESS").as_deref() == Ok("1")
        && window_type == "approval"
    {
        eprintln!("[Child] Forced headless mode via NEMESISBOT_FORCE_HEADLESS=1");
        "headless".to_string()
    } else {
        window_type
    };

    eprintln!(
        "[Child] Child ID: {}, Window Type: {}",
        child_id, window_type
    );

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
    eprintln!(
        "[Child] WS key received: key={}, port={}, path={}",
        key, port, path
    );

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
            eprintln!(
                "[Child] Starting approval window for request {}",
                data.request_id
            );
            load_and_run_plugin_window(window_type, window_data, &ws_key, ws_port, &ws_path)
        }
        "headless" => {
            let data: ApprovalWindowData = serde_json::from_value(window_data.clone())
                .map_err(|e| format!("invalid headless window data: {}", e))?;
            eprintln!(
                "[Child] Starting headless window (auto-approve) for request {}",
                data.request_id
            );
            run_headless_auto_approve(child_id, &data, &ws_key, ws_port, &ws_path)
        }
        "dashboard" => {
            let data: DashboardWindowData = serde_json::from_value(window_data.clone())
                .map_err(|e| format!("invalid dashboard window data: {}", e))?;
            eprintln!(
                "[Child] Starting dashboard window (web={}:{})",
                data.web_host, data.web_port
            );
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
    let exe = std::env::current_exe().map_err(|e| format!("get exe path: {}", e))?;
    let exe_dir = exe.parent().ok_or("no parent dir for exe")?;

    let lib_path =
        nemesis_utils::find_plugin_library_in(exe_dir, "plugin_ui").ok_or_else(|| {
            let filename = nemesis_utils::plugin_library_filename("plugin_ui");
            format!(
                "plugin-ui library not found in {}/plugins/ (expected: {})",
                exe_dir.display(),
                filename
            )
        })?;

    eprintln!("[Child] Loading plugin library: {}", lib_path.display());

    // WebKitGTK on Linux: set env vars BEFORE dlopen'ing the library.
    // These must be set before libwebkit2gtk is loaded to take effect.
    #[cfg(target_os = "linux")]
    unsafe {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }

    // Load the library
    let lib = unsafe {
        libloading::Library::new(&lib_path)
            .map_err(|e| format!("load library '{}': {}", lib_path.display(), e))?
    };

    // Try to call plugin_init if the library exports it (unified interface)
    let _init_result: i32 = unsafe {
        if let Ok(plugin_init_fn) = lib.get::<libloading::Symbol<
            unsafe extern "C" fn(
                *const std::ffi::c_char,
                *const nemesis_plugin::HostServices,
            ) -> i32,
        >>(b"plugin_init\0")
        {
            let c_config_dir = std::ffi::CString::new(".").unwrap_or_default();

            // Build HostServices with decode_png support
            #[allow(unused_mut)]
            let mut host =
                nemesis_plugin::build_host_services(&std::env::current_dir().unwrap_or_default());
            #[cfg(not(target_os = "android"))]
            {
                host.decode_png = Some(host_decode_png);
            }
            // Leak the HostServices so it lives for the DLL's lifetime
            let host_box = Box::new(host);
            let host_ptr = Box::into_raw(host_box);

            let ret = plugin_init_fn(c_config_dir.as_ptr() as *const std::ffi::c_char, host_ptr);
            eprintln!("[Child] plugin_init returned: {}", ret);
            ret
        } else {
            0 // No unified interface, that's OK
        }
    };

    // Get the plugin_create_window symbol
    let create_window: libloading::Symbol<unsafe extern "C" fn(*const std::ffi::c_char) -> i32> = unsafe {
        lib.get(b"plugin_create_window\0")
            .map_err(|e| format!("get plugin_create_window symbol: {}", e))?
    };

    // Try to get the plugin_request_bring_to_front symbol (optional)
    let bring_to_front_fn: Option<libloading::Symbol<unsafe extern "C" fn()>> =
        unsafe { lib.get(b"plugin_request_bring_to_front\0").ok() };

    // Try to get the plugin_get_approval_result symbol (optional, for approval windows)
    let get_approval_result_fn: Option<
        libloading::Symbol<unsafe extern "C" fn() -> *const std::ffi::c_char>,
    > = unsafe { lib.get(b"plugin_get_approval_result\0").ok() };

    // Store the function pointer globally so the WS handler can call it
    if let Some(ref f) = bring_to_front_fn {
        let raw_fn: unsafe extern "C" fn() = **f;
        BRING_TO_FRONT_FN_PTR.set(raw_fn);
    }

    // Connect to parent's WebSocket server and register bring_to_front handler
    let ws_handle = connect_ws_with_handler(ws_key, ws_port, ws_path, bring_to_front_fn.is_some());

    // Build config JSON for the DLL
    let config = build_plugin_config(window_type, window_data);
    let c_config =
        std::ffi::CString::new(config).map_err(|e| format!("CString conversion: {}", e))?;

    eprintln!("[Child] Calling plugin_create_window (blocking)...");

    // Call the DLL — blocks until the window closes (uses run_return so it returns normally).
    let result = unsafe { create_window(c_config.as_ptr() as *const std::ffi::c_char) };

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

            let request_id = window_data
                .get("request_id")
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
                            eprintln!(
                                "[Child] Sent approval.submit notification (action={})",
                                action
                            );
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
        if let Ok(plugin_free_fn) =
            lib.get::<libloading::Symbol<unsafe extern "C" fn()>>(b"plugin_free\0")
        {
            plugin_free_fn();
            eprintln!("[Child] plugin_free called");
        }
    }

    // lib is dropped here, unloading the DLL
    if result != 0 {
        return Err(format!(
            "plugin_create_window returned error code: {}",
            result
        ));
    }

    Ok(())
}

/// Host-side decode_png implementation using the `image` crate.
/// Decodes PNG bytes into RGBA pixel data for plugin use (e.g. window icons).
#[cfg(not(target_os = "android"))]
extern "C" fn host_decode_png(
    png_data: *const u8,
    png_len: usize,
    out_rgba: *mut u8,
    out_rgba_len: usize,
    out_width: *mut u32,
    out_height: *mut u32,
) -> i32 {
    if png_data.is_null() || out_width.is_null() || out_height.is_null() {
        return -1;
    }
    let data = unsafe { std::slice::from_raw_parts(png_data, png_len) };
    let img = match image::load_from_memory_with_format(data, image::ImageFormat::Png) {
        Ok(img) => img.into_rgba8(),
        Err(_) => return -2,
    };
    let (w, h) = img.dimensions();
    let needed = (w * h * 4) as usize;
    unsafe {
        *out_width = w;
        *out_height = h;
    }
    // Query mode: caller passes null buffer to get dimensions only
    if out_rgba.is_null() {
        return -3;
    }
    if out_rgba_len < needed {
        return -3;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(img.as_raw().as_ptr(), out_rgba, needed);
    }
    0
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
        self.shutdown
            .store(true, std::sync::atomic::Ordering::SeqCst);
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
        path: if ws_path.is_empty() {
            "/ws".to_string()
        } else {
            ws_path.to_string()
        },
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
        self.ptr
            .store(f as *mut (), std::sync::atomic::Ordering::SeqCst);
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
                    eprintln!(
                        "[Child:headless] Sent approval.submit (auto-approve) for request {}",
                        data.request_id
                    );
                    break;
                }
                Err(_) if attempt < 9 => {
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
                Err(e) => {
                    eprintln!(
                        "[Child:headless] Failed to send approval.submit after retries: {}",
                        e
                    );
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
                    })
                    .to_string()
                }
                Err(_) => serde_json::json!({
                    "window_type": "dashboard",
                    "title": "NemesisBot Dashboard",
                })
                .to_string(),
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
                    })
                    .to_string()
                }
                Err(_) => serde_json::json!({
                    "window_type": "approval",
                    "title": "Security Approval - NemesisBot",
                })
                .to_string(),
            }
        }
        _ => serde_json::json!({
            "window_type": window_type,
        })
        .to_string(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
