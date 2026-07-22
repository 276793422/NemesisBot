//! Pure GTK + WebKitGTK window creation for Linux.
//!
//! Bypasses wry/tao's X11 reparenting which causes blank rendering
//! on some Linux setups. Uses GTK3 directly for window management
//! and WebKitGTK for web content rendering.

use std::time::Duration;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

use crate::{
    get_approval_result_value, is_close_requested, set_approval_result,
    take_bring_to_front_requested, WindowConfig, PLUGIN_ERR_WINDOW,
};

// Embedded brand icon (shared with window.rs)
const ICON_PNG: &[u8] = include_bytes!("../icons/icon.png");

/// Set up common window properties: centering, close handler, flag-check timeout.
/// Returns the SourceId for the periodic check (caller may remove on cleanup).
fn setup_common(window: &gtk::Window) {
    window.set_position(gtk::WindowPosition::Center);

    // Set window icon
    if let Some(pixbuf) = create_icon_pixbuf() {
        window.set_icon(Some(&pixbuf));
    }

    // close-request → quit main loop
    window.connect_delete_event(|_, _| {
        gtk::main_quit();
        glib::Propagation::Stop
    });

    // Periodic flag check — captures a cloned reference so we can call present()
    let win_clone = window.clone();
    glib::timeout_add_local(Duration::from_millis(100), move || {
        if take_bring_to_front_requested() {
            win_clone.present();
        }
        if is_close_requested() {
            gtk::main_quit();
        }
        glib::ControlFlow::Continue
    });
}

/// Decode the embedded PNG into a gdk::Pixbuf for the window icon.
fn create_icon_pixbuf() -> Option<gtk::gdk_pixbuf::Pixbuf> {
    let img = image::load_from_memory_with_format(ICON_PNG, image::ImageFormat::Png).ok()?;
    let rgba = img.into_rgba8();
    let (w, h) = rgba.dimensions();
    Some(gtk::gdk_pixbuf::Pixbuf::from_bytes(
        &glib::Bytes::from(rgba.as_raw()),
        gtk::gdk_pixbuf::Colorspace::Rgb,
        true,
        8,
        w as i32,
        h as i32,
        (w * 4) as i32,
    ))
}

// ---------------------------------------------------------------------------
// Dashboard window
// ---------------------------------------------------------------------------

/// Create a dashboard window using pure GTK + WebKitGTK.
pub fn create_dashboard_window(config: &WindowConfig) -> Result<(), i32> {
    use webkit2gtk::{
        SettingsExt, UserContentInjectedFrames, UserContentManagerExt, UserScript,
        UserScriptInjectionTime, WebViewExt,
    };

    let url = config.url.clone().unwrap_or_default();
    if url.is_empty() {
        eprintln!("[plugin-ui] dashboard: url is required");
        return Err(PLUGIN_ERR_WINDOW);
    }

    gtk::init().map_err(|e| {
        eprintln!("[plugin-ui] GTK init failed: {}", e);
        PLUGIN_ERR_WINDOW
    })?;

    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title(&config.title);
    window.set_default_size(config.width as i32, config.height as i32);

    let webview = webkit2gtk::WebView::new();

    if let Some(settings) = WebViewExt::settings(&webview) {
        settings.set_enable_developer_extras(true);
    }

    // Inject init script (e.g. window.__DASHBOARD_TOKEN__)
    if let Some(ref script) = config.init_script {
        if let Some(mgr) = webview.user_content_manager() {
            let user_script = UserScript::new(
                script,
                UserContentInjectedFrames::AllFrames,
                UserScriptInjectionTime::Start,
                &[],
                &[],
            );
            mgr.add_script(&user_script);
        }
    }

    webview.load_uri(&url);

    window.add(&webview);
    setup_common(&window);
    window.show_all();

    eprintln!("[plugin-ui] Dashboard (GTK+WebKitGTK) loading {}", url);
    gtk::main();
    eprintln!("[plugin-ui] Dashboard event loop exited");

    Ok(())
}

// ---------------------------------------------------------------------------
// Approval window
// ---------------------------------------------------------------------------

/// Create an approval window using pure GTK + WebKitGTK.
#[allow(deprecated)] // NavigationPolicyDecisionExt::request is deprecated since 2.6
pub fn create_approval_window(config: &WindowConfig) -> Result<(), i32> {
    use webkit2gtk::{
        NavigationPolicyDecision, NavigationPolicyDecisionExt, PolicyDecisionExt,
        PolicyDecisionType, SettingsExt, URIRequestExt, URISchemeRequestExt, WebContextExt,
        WebViewExt,
    };

    let html = config.html.clone().unwrap_or_default();
    if html.is_empty() {
        eprintln!("[plugin-ui] approval: html content is required");
        return Err(PLUGIN_ERR_WINDOW);
    }

    gtk::init().map_err(|e| {
        eprintln!("[plugin-ui] GTK init failed: {}", e);
        PLUGIN_ERR_WINDOW
    })?;

    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title(&config.title);
    window.set_default_size(750, 700);
    window.set_resizable(true);

    // Create WebContext with nemesis:// custom protocol
    let context = webkit2gtk::WebContext::new();

    // Register custom URI scheme for serving approval HTML
    let html_for_scheme = html.clone();
    context.register_uri_scheme("nemesis", move |request| {
        let uri = request.uri().map(|s| s.to_string()).unwrap_or_default();
        eprintln!("[plugin-ui] URI scheme request: {}", uri);

        if uri.contains("/__approval_page") {
            let bytes = glib::Bytes::from(html_for_scheme.as_bytes());
            let stream = gio::MemoryInputStream::from_bytes(&bytes);
            request.finish(
                &stream,
                html_for_scheme.len() as i64,
                Some("text/html; charset=utf-8"),
            );
            return;
        }

        let bytes = glib::Bytes::from(b"not found");
        let stream = gio::MemoryInputStream::from_bytes(&bytes);
        request.finish(&stream, 9i64, Some("text/plain"));
    });

    let webview = webkit2gtk::WebView::builder().web_context(&context).build();

    if let Some(settings) = WebViewExt::settings(&webview) {
        settings.set_enable_developer_extras(true);
    }

    // Intercept approval result navigation from JS
    webview.connect_decide_policy(move |_webview, decision, policy_type| {
        if policy_type == PolicyDecisionType::NavigationAction {
            if let Some(nav) = decision.dynamic_cast_ref::<NavigationPolicyDecision>() {
                if let Some(req) = nav.request() {
                    if let Some(uri) = req.uri() {
                        let uri_str = uri.as_str();
                        if uri_str.contains("/__approval_result") {
                            let action = uri_str
                                .split("action=")
                                .nth(1)
                                .map(|s| s.split('&').next().unwrap_or("rejected"))
                                .unwrap_or("rejected");
                            eprintln!("[plugin-ui] Approval result: {}", action);
                            set_approval_result(action);
                            decision.ignore();
                            return true;
                        }
                    }
                }
            }
        }
        false
    });

    // Load approval page via custom protocol
    webview.load_uri("nemesis://localhost/__approval_page");

    window.add(&webview);
    setup_common(&window);

    // Safety net timeout — auto-reject if user doesn't respond
    let timeout_secs = config.timeout_seconds;
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(timeout_secs));
        if get_approval_result_value().is_none() {
            eprintln!(
                "[plugin-ui] Approval timeout ({}s) — auto-rejecting",
                timeout_secs
            );
            set_approval_result("rejected");
        }
    });

    window.show_all();

    eprintln!("[plugin-ui] Approval window (GTK+WebKitGTK) created");
    gtk::main();
    eprintln!("[plugin-ui] Approval event loop exited");

    Ok(())
}
