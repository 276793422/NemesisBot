//! Window creation logic for Dashboard and Approval windows.
//!
//! Pure WebView rendering — zero business logic.
//! All content (URLs, HTML, init scripts) is provided via WindowConfig
//! by the host process (nemesisbot).
//!
//! Dashboard: loads a URL directly + optional init script for token injection.
//! Approval: serves inline HTML via custom protocol + captures user response.

use crate::{
    is_close_requested,
    set_active_hwnd, set_approval_result, take_bring_to_front_requested, bring_window_to_foreground,
    WindowConfig,
    PLUGIN_ERR_WINDOW,
};

use tao::dpi::{LogicalSize, PhysicalPosition};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::platform::run_return::EventLoopExtRunReturn;
use tao::window::WindowBuilder;
use wry::{WebContext, WebViewBuilder};

// ---------------------------------------------------------------------------
// Dashboard window
// ---------------------------------------------------------------------------

/// Create a Dashboard window that loads a URL directly.
///
/// The host process provides the URL and an optional initialization script.
/// No reverse proxy or token injection happens in the DLL — that's all
/// handled by the host process.
pub fn create_dashboard_window(config: &WindowConfig) -> Result<(), i32> {
    let url = config.url.clone().unwrap_or_default();

    if url.is_empty() {
        eprintln!("[plugin-ui] dashboard: url is required");
        return Err(PLUGIN_ERR_WINDOW);
    }

    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title(&config.title)
        .with_inner_size(LogicalSize::new(config.width, config.height))
        .build(&event_loop)
        .map_err(|e| {
            eprintln!("[plugin-ui] failed to create window: {:?}", e);
            PLUGIN_ERR_WINDOW
        })?;

    // Center the window
    center_window(&window, config.width, config.height);

    // Capture HWND for bring-to-front dedup
    capture_hwnd(&window);

    // WebContext: data directory in %TEMP% so we don't pollute the exe directory
    let (mut web_context, data_dir) = create_web_context("dashboard");

    let mut builder = WebViewBuilder::with_web_context(&mut web_context)
        .with_url(&url)
        .with_devtools(true);

    // Inject init script (e.g. window.__DASHBOARD_TOKEN__) before page loads
    if let Some(ref script) = config.init_script {
        builder = builder.with_initialization_script(script);
    }

    let _webview = builder.build(&window).map_err(|e| {
        eprintln!("[plugin-ui] failed to create webview: {:?}", e);
        PLUGIN_ERR_WINDOW
    })?;

    eprintln!("[plugin-ui] Dashboard window created, loading {}", url);

    // Run event loop (blocking)
    run_event_loop(event_loop);

    // Clean up WebView2 data directory
    cleanup_web_context(&data_dir);

    Ok(())
}

// ---------------------------------------------------------------------------
// Approval window
// ---------------------------------------------------------------------------

/// Create an Approval window with inline HTML content.
///
/// The host process provides the complete HTML page. The DLL serves it via
/// a custom `nemesis://` protocol and captures the user's response when
/// the JS navigates to `nemesis://localhost/__approval_result?action=...`.
pub fn create_approval_window(config: &WindowConfig) -> Result<(), i32> {
    let html = config.html.clone().unwrap_or_default();

    if html.is_empty() {
        eprintln!("[plugin-ui] approval: html content is required");
        return Err(PLUGIN_ERR_WINDOW);
    }

    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title(&config.title)
        .with_inner_size(LogicalSize::new(750.0, 700.0))
        .with_resizable(true)
        .build(&event_loop)
        .map_err(|e| {
            eprintln!("[plugin-ui] failed to create window: {:?}", e);
            PLUGIN_ERR_WINDOW
        })?;

    // Capture HWND for bring-to-front dedup
    capture_hwnd(&window);

    // Approval windows are ephemeral — use temp data directory
    let (mut web_context, data_dir) = create_web_context("approval");

    // Use custom protocol to serve HTML (gives proper origin, unlike data: URIs)
    let html_for_protocol = html.clone();
    let _webview = WebViewBuilder::with_web_context(&mut web_context)
        .with_url("nemesis://localhost/__approval_page")
        .with_navigation_handler(|url: String| -> bool {
            // Intercept approval result navigation from JS
            if url.contains("/__approval_result") {
                let action = url.split("action=")
                    .nth(1)
                    .map(|s| s.split('&').next().unwrap_or("rejected"))
                    .unwrap_or("rejected");
                eprintln!("[plugin-ui] Navigation handler: approval result = {}", action);
                set_approval_result(action);
                return false; // Block navigation, we handled it
            }
            true // Allow all other navigations
        })
        .with_asynchronous_custom_protocol(
            "nemesis".into(),
            move |_webview_id, request, responder| {
                let uri = request.uri().path();

                // Serve the approval HTML
                if uri == "/__approval_page" {
                    responder.respond(
                        wry::http::Response::builder()
                            .status(200)
                            .header("Content-Type", "text/html; charset=utf-8")
                            .body(html_for_protocol.as_bytes().to_vec())
                            .unwrap(),
                    );
                    return;
                }

                if uri.starts_with("/__approval_result") {
                    let action = request.uri().query()
                        .and_then(|q| {
                            q.split('&')
                                .find_map(|pair| {
                                    let mut parts = pair.splitn(2, '=');
                                    let key = parts.next()?;
                                    let value = parts.next()?;
                                    if key == "action" { Some(value.to_string()) } else { None }
                                })
                        })
                        .unwrap_or_else(|| "rejected".to_string());

                    eprintln!("[plugin-ui] Approval result: {}", action);
                    set_approval_result(&action);

                    responder.respond(
                        wry::http::Response::builder()
                            .status(200)
                            .body("ok".as_bytes().to_vec())
                            .unwrap(),
                    );
                    return;
                }

                responder.respond(
                    wry::http::Response::builder()
                        .status(404)
                        .body("not found".as_bytes().to_vec())
                        .unwrap(),
                );
            },
        )
        .with_devtools(true)
        .build(&window)
        .map_err(|e| {
            eprintln!("[plugin-ui] failed to create webview: {:?}", e);
            PLUGIN_ERR_WINDOW
        })?;

    eprintln!("[plugin-ui] Approval window created");

    // Safety net timeout thread — auto-reject if user doesn't respond.
    // The HTML JS countdown fires first; this is a fallback.
    let timeout_secs = config.timeout_seconds;
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(timeout_secs));
        // Only auto-reject if no result has been set yet
        if crate::get_approval_result_value().is_none() {
            eprintln!("[plugin-ui] Approval timeout ({}s) — auto-rejecting", timeout_secs);
            set_approval_result("rejected");
        }
    });

    // Run event loop (blocking) — returns when ControlFlow::Exit is set
    eprintln!("[plugin-ui] Entering event loop...");
    run_event_loop(event_loop);
    eprintln!("[plugin-ui] Event loop exited");

    // Clean up WebView2 data directory
    cleanup_web_context(&data_dir);

    eprintln!("[plugin-ui] create_approval_window returning Ok");
    Ok(())
}

// ---------------------------------------------------------------------------
// Event loop
// ---------------------------------------------------------------------------

/// Run the event loop until the window closes or close is requested.
///
/// Uses `WaitUntil` with a 100ms interval so the event loop periodically
/// checks `CLOSE_REQUESTED` and `BRING_TO_FRONT_REQUESTED` flags.
fn run_event_loop(mut event_loop: EventLoop<()>) {
    event_loop.run_return(move |event, _, control_flow| {
        *control_flow = ControlFlow::WaitUntil(
            std::time::Instant::now() + std::time::Duration::from_millis(100)
        );

        // Check for bring-to-front request (from parent via WebSocket)
        if take_bring_to_front_requested() {
            bring_window_to_foreground();
        }

        // Check for close request (set by set_approval_result or timeout thread)
        if is_close_requested() {
            eprintln!("[plugin-ui] Event loop: close requested, exiting");
            *control_flow = ControlFlow::Exit;
            return;
        }

        match event {
            Event::NewEvents(tao::event::StartCause::Init) => {
                if take_bring_to_front_requested() {
                    bring_window_to_foreground();
                }
            }
            Event::MainEventsCleared => {
                if take_bring_to_front_requested() {
                    bring_window_to_foreground();
                }
                if is_close_requested() {
                    eprintln!("[plugin-ui] MainEventsCleared: close requested, exiting");
                    *control_flow = ControlFlow::Exit;
                }
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                eprintln!("[plugin-ui] CloseRequested event, exiting");
                *control_flow = ControlFlow::Exit;
            }
            Event::WindowEvent {
                event: WindowEvent::Destroyed,
                ..
            } => {
                eprintln!("[plugin-ui] Destroyed event, exiting");
                *control_flow = ControlFlow::Exit;
            }
            _ => {}
        }
    });
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract the native window handle (HWND on Windows) and store it globally.
fn capture_hwnd(window: &tao::window::Window) {
    #[cfg(target_os = "windows")]
    {
        use raw_window_handle::HasWindowHandle;
        if let Ok(handle) = window.window_handle() {
            if let raw_window_handle::RawWindowHandle::Win32(win32_handle) = handle.as_raw() {
                let hwnd = win32_handle.hwnd.get() as isize;
                set_active_hwnd(hwnd);
            }
        }
    }
}

/// Create a WebContext with a unique data directory in %TEMP%.
fn create_web_context(window_type: &str) -> (WebContext, std::path::PathBuf) {
    let id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);

    let data_dir = std::env::temp_dir()
        .join("nemesisbot-webview2")
        .join(format!("{}-{}", window_type, id));

    let _ = std::fs::create_dir_all(&data_dir);

    let path = data_dir.clone();
    (WebContext::new(Some(data_dir)), path)
}

/// Remove the WebView2 data directory created for a window session.
fn cleanup_web_context(data_dir: &std::path::Path) {
    if data_dir.exists() {
        match std::fs::remove_dir_all(data_dir) {
            Ok(()) => eprintln!("[plugin-ui] cleaned up WebView2 data dir: {:?}", data_dir),
            Err(e) => eprintln!("[plugin-ui] warning: failed to clean up WebView2 data dir {:?}: {}", data_dir, e),
        }
    }
}

/// Center a window on the primary monitor.
fn center_window(window: &tao::window::Window, width: f64, height: f64) {
    if let Some(monitor) = window.primary_monitor().or_else(|| window.current_monitor()) {
        let monitor_size = monitor.size();
        let scale = monitor.scale_factor();
        let x = (monitor_size.width as f64 / scale - width) / 2.0;
        let y = (monitor_size.height as f64 / scale - height) / 2.0;
        let _ = window.set_outer_position(PhysicalPosition::new(
            x.max(0.0) as i32,
            y.max(0.0) as i32,
        ));
    }
}
