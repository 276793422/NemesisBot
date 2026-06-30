//! System tray management.
//!
//! Provides a platform-agnostic system tray abstraction with configurable menus,
//! action dispatch, and callback registration. Concrete platform integration
//! (Windows tray, Linux AppIndicator, macOS NSStatusItem) requires
//! platform-specific backend code that hooks into the types defined here.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
#[cfg(all(not(target_os = "windows"), not(target_os = "linux")))]
use std::sync::atomic::Ordering;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Configuration types
// ---------------------------------------------------------------------------

/// Configuration for creating a system tray instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrayConfig {
    /// Title displayed in the tray area (used on platforms that show text).
    pub title: String,
    /// Tooltip shown when hovering over the tray icon.
    pub tooltip: String,
    /// Ordered list of menu items to display in the context menu.
    pub menu_items: Vec<MenuItem>,
}

impl Default for TrayConfig {
    fn default() -> Self {
        Self {
            title: "NemesisBot".into(),
            tooltip: "NemesisBot - AI Agent".into(),
            menu_items: vec![
                MenuItem::new("start", "Start Service", true),
                MenuItem::new("stop", "Stop Service", true),
                MenuItem::new("webui", "Open Dashboard", true),
                MenuItem::new("version", "Version Info", false),
                MenuItem::new("quit", "Quit", true),
            ],
        }
    }
}

/// A single entry in the tray context menu.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MenuItem {
    /// Unique identifier for this menu item (used in callback dispatch).
    pub id: String,
    /// Display label shown to the user.
    pub label: String,
    /// Whether the item is clickable (grayed out when false).
    pub enabled: bool,
}

impl MenuItem {
    /// Creates a new menu item.
    pub fn new(id: impl Into<String>, label: impl Into<String>, enabled: bool) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            enabled,
        }
    }
}

// ---------------------------------------------------------------------------
// Tray actions
// ---------------------------------------------------------------------------

/// Actions that can be triggered from the system tray menu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrayAction {
    /// Start the bot service.
    StartService,
    /// Stop the bot service.
    StopService,
    /// Open the web dashboard in a browser.
    OpenWebUI,
    /// Display version information (usually a disabled informational item).
    Version,
    /// Quit the application entirely.
    Quit,
}

impl TrayAction {
    /// Maps a menu item id to the corresponding [`TrayAction`].
    ///
    /// Returns `None` if the id does not correspond to any known action.
    pub fn from_menu_id(id: &str) -> Option<Self> {
        match id {
            "start" => Some(Self::StartService),
            "stop" => Some(Self::StopService),
            "webui" => Some(Self::OpenWebUI),
            "version" => Some(Self::Version),
            "quit" => Some(Self::Quit),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Callback type
// ---------------------------------------------------------------------------

/// Callback invoked when a tray action is triggered.
pub type TrayCallback = Arc<dyn Fn(TrayAction) + Send + Sync>;

// ---------------------------------------------------------------------------
// SystemTray
// ---------------------------------------------------------------------------

/// Platform-agnostic system tray manager.
///
/// Holds the menu configuration and registered callbacks. A real platform
/// backend would read this state and create native tray widgets.
pub struct SystemTray {
    config: RwLock<TrayConfig>,
    callbacks: RwLock<HashMap<String, TrayCallback>>,
}

impl SystemTray {
    /// Creates a new system tray with the given configuration.
    pub fn new(config: TrayConfig) -> Self {
        tracing::info!(
            title = %config.title,
            menu_count = config.menu_items.len(),
            "[Desktop] System tray created"
        );
        Self {
            config: RwLock::new(config),
            callbacks: RwLock::new(HashMap::new()),
        }
    }

    /// Creates a system tray with the default configuration.
    pub fn default_tray() -> Self {
        Self::new(TrayConfig::default())
    }

    /// Returns a snapshot of the current tray configuration.
    pub fn config(&self) -> TrayConfig {
        self.config.read().expect("tray config lock poisoned").clone()
    }

    /// Returns the current number of menu items.
    pub fn menu_count(&self) -> usize {
        self.config.read().expect("tray config lock poisoned").menu_items.len()
    }

    /// Adds a menu item to the end of the menu.
    pub fn add_menu_item(&self, item: MenuItem) {
        self.config.write().expect("tray config lock poisoned").menu_items.push(item);
    }

    /// Registers a callback for the given menu item id.
    ///
    /// When [`Self::trigger_action`] is called with a matching id the
    /// registered callback will be invoked with the resolved [`TrayAction`].
    pub fn on_click(&self, menu_id: impl Into<String>, callback: TrayCallback) {
        let id = menu_id.into();
        tracing::debug!(menu_id = %id, "[Desktop] Callback registered for menu item");
        self.callbacks.write().expect("callbacks lock poisoned").insert(id, callback);
    }

    /// Programmatically triggers an action by menu item id.
    ///
    /// Resolves the id to a [`TrayAction`], looks up any registered callback,
    /// and invokes it. Returns `true` if a callback was found and invoked,
    /// `false` otherwise.
    pub fn trigger_action(&self, menu_id: &str) -> bool {
        let action = match TrayAction::from_menu_id(menu_id) {
            Some(a) => a,
            None => {
                tracing::warn!(menu_id = menu_id, "[Desktop] Unknown tray menu action");
                return false;
            }
        };

        let cb = self.callbacks.read().expect("callbacks lock poisoned").get(menu_id).cloned();
        match cb {
            Some(callback) => {
                tracing::info!(menu_id = menu_id, action = ?action, "[Desktop] Tray action triggered");
                callback(action);
                true
            }
            None => {
                tracing::debug!(menu_id = menu_id, "[Desktop] No callback registered for menu item");
                false
            }
        }
    }

    /// Updates the tooltip text.
    pub fn set_tooltip(&self, tooltip: impl Into<String>) {
        self.config.write().expect("tray config lock poisoned").tooltip = tooltip.into();
    }

    /// Enables or disables a menu item by id.
    ///
    /// Returns `true` if the item was found and updated.
    pub fn set_menu_enabled(&self, id: &str, enabled: bool) -> bool {
        let mut cfg = self.config.write().expect("tray config lock poisoned");
        for item in &mut cfg.menu_items {
            if item.id == id {
                item.enabled = enabled;
                return true;
            }
        }
        false
    }
}

// ---------------------------------------------------------------------------
// Platform tray implementation
// ---------------------------------------------------------------------------

// Thread-local storage for cluster menu items.
//
// `tray_icon::menu::MenuItem` is not `Send` (internally uses `Rc`), so it
// cannot be stored in `PlatformTray` (which must be `Send`). Instead, the
// items are created in `run_event_loop()` on the tray thread and stored here.
// The enable/disable functions below access them from callbacks that also run
// on the tray thread.
#[cfg(not(target_os = "linux"))]
thread_local! {
    static CLUSTER_MENU_ITEMS: std::cell::RefCell<Option<(
        tray_icon::menu::MenuItem,
        tray_icon::menu::MenuItem,
    )>> = std::cell::RefCell::new(None);
}

/// Enable the "集群启动" and "集群停止" tray menu items.
///
/// Safe to call from any thread; no-ops when called off the tray thread
/// (the thread-local will be `None`).
#[cfg(not(target_os = "linux"))]
pub fn enable_cluster_menu_items() {
    CLUSTER_MENU_ITEMS.with(|items| {
        if let Some((start, stop)) = items.borrow().as_ref() {
            start.set_enabled(true);
            stop.set_enabled(true);
            tracing::debug!("[Desktop] Cluster menu items enabled");
        }
    });
}

/// Disable the "集群启动" and "集群停止" tray menu items.
#[cfg(not(target_os = "linux"))]
pub fn disable_cluster_menu_items() {
    CLUSTER_MENU_ITEMS.with(|items| {
        if let Some((start, stop)) = items.borrow().as_ref() {
            start.set_enabled(false);
            stop.set_enabled(false);
            tracing::debug!("[Desktop] Cluster menu items disabled");
        }
    });
}

#[cfg(target_os = "linux")]
pub fn enable_cluster_menu_items() {
    // TODO: 通过 plugin-ui bridge 实现
}

#[cfg(target_os = "linux")]
pub fn disable_cluster_menu_items() {
    // TODO: 通过 plugin-ui bridge 实现
}

/// User event types forwarded from tray-icon to the winit event loop.
#[cfg(not(target_os = "linux"))]
#[derive(Debug, Clone)]
enum TrayUserEvent {
    /// A tray icon event (click, double-click, etc.).
    Icon(tray_icon::TrayIconEvent),
    /// A context menu event (menu item selected).
    Menu(tray_icon::menu::MenuEvent),
    /// External request to exit the event loop. On macOS this is sent from
    /// the gateway worker thread after cleanup (via [`main_thread_handoff`])
    /// so the main-thread tray loop unwinds for shutdowns that do not
    /// originate from the tray "Quit" menu item (e.g. Ctrl+C).
    #[cfg(target_os = "macos")]
    Exit,
}

/// macOS main-thread tray handoff.
///
/// winit's `EventLoop` must be created and run on the process main thread on
/// macOS (AppKit/NSApplication requirement; no `with_any_thread` escape hatch).
/// The gateway, however, runs on a tokio worker thread and assembles the tray
/// callbacks from gateway-local `Arc` state there. This module bridges the two:
///
/// 1. macOS `main()` calls [`main_thread_handoff::init`] to create the handoff
///    channel, then spawns the gateway on the runtime.
/// 2. The gateway worker, at the tray-setup step, calls
///    [`main_thread_handoff::deliver`] with the fully-configured tray.
/// 3. The main thread receives it and runs the event loop, which calls
///    [`main_thread_handoff::set_exit_proxy`] to register a proxy.
/// 4. On shutdown the worker calls [`main_thread_handoff::request_exit`] after
///    cleanup so the main-thread loop exits even for Ctrl+C.
#[cfg(target_os = "macos")]
pub mod main_thread_handoff {
    use super::{PlatformTray, TrayUserEvent};
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::sync::{Mutex, OnceLock};
    use winit::event_loop::EventLoopProxy;

    // A plain std channel (not tokio::mpsc): the tray handoff is a one-shot
    // delivery and the main thread receives it WITHOUT a tokio runtime.
    //
    // Held in a Mutex<Option<..>> (not OnceLock) so it can be dropped via
    // [`TrayChannelGuard`] when the gateway task exits — that unblocks the main
    // thread's receiver even if the gateway errored before tray setup.
    static TRAY_TX: Mutex<Option<Sender<PlatformTray>>> = Mutex::new(None);
    static EXIT_PROXY: OnceLock<EventLoopProxy<TrayUserEvent>> = OnceLock::new();

    /// Create the handoff channel. Called once from macOS `main()` before the
    /// gateway thread is spawned. Returns the receiver the main thread waits on.
    pub fn init() -> Receiver<PlatformTray> {
        let (tx, rx) = mpsc::channel::<PlatformTray>();
        *TRAY_TX.lock().expect("TRAY_TX lock") = Some(tx);
        rx
    }

    /// Deliver the configured tray to the main thread. Called from the gateway
    /// thread at the tray-setup step. No-ops if the channel was already closed.
    pub fn deliver(tray: PlatformTray) {
        if let Some(tx) = TRAY_TX.lock().expect("TRAY_TX lock").as_ref() {
            let _ = tx.send(tray);
        }
    }

    /// Drop the handoff sender so the main thread's `recv()` returns `Err`
    /// (i.e. "gateway finished without starting a tray"). Called by
    /// [`TrayChannelGuard::drop`].
    fn close() {
        *TRAY_TX.lock().expect("TRAY_TX lock") = None;
    }

    /// Store the event-loop proxy so the gateway thread can request exit. Called
    /// from `run_event_loop_native` on the main thread just before
    /// `event_loop.run()`. `pub(super)` because it is only used within the
    /// `systray` module (and exposes the private `TrayUserEvent`).
    pub(super) fn set_exit_proxy(proxy: EventLoopProxy<TrayUserEvent>) {
        let _ = EXIT_PROXY.set(proxy);
    }

    /// Request the main-thread tray loop to exit. Called from the gateway
    /// thread at the end of cleanup — covers Ctrl+C and any shutdown path,
    /// not just the tray "Quit" menu item.
    pub fn request_exit() {
        if let Some(proxy) = EXIT_PROXY.get() {
            let _ = proxy.send_event(TrayUserEvent::Exit);
        }
    }

    /// RAII guard that closes the handoff channel when dropped. Create it at
    /// the very top of the macOS gateway `run()` so that ANY exit path
    /// (including early errors before the tray-setup step) unblocks the main
    /// thread's receiver instead of deadlocking it.
    pub struct TrayChannelGuard;
    impl Drop for TrayChannelGuard {
        fn drop(&mut self) {
            close();
        }
    }

    /// Acquire the [`TrayChannelGuard`] for the current gateway run.
    pub fn channel_guard() -> TrayChannelGuard {
        TrayChannelGuard
    }
}

// ---------------------------------------------------------------------------
// Linux tray bridge — libloading 加载 plugin-ui.so
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
mod linux_tray {
    use std::ffi::{CStr, CString};
    use std::os::raw::c_char;

    /// 持有所有回调的容器，通过 Box::into_raw 泄漏到堆上。
    struct Callbacks {
        on_start: Option<Box<dyn Fn() + Send + Sync>>,
        on_stop: Option<Box<dyn Fn() + Send + Sync>>,
        on_cluster_start: Option<Box<dyn Fn() + Send + Sync>>,
        on_cluster_stop: Option<Box<dyn Fn() + Send + Sync>>,
        on_open_dashboard: Option<Box<dyn Fn() + Send + Sync>>,
        on_open_chat: Option<Box<dyn Fn() + Send + Sync>>,
        on_quit: Option<Box<dyn Fn() + Send + Sync>>,
    }

    /// extern "C" 回调桥接：plugin-ui 调用此函数通知菜单点击。
    extern "C" fn on_menu_click(
        user_data: *mut std::os::raw::c_void,
        menu_id: *const c_char,
    ) {
        if user_data.is_null() || menu_id.is_null() {
            return;
        }
        let cbs = unsafe { &*(user_data as *const Callbacks) };
        let id = unsafe { CStr::from_ptr(menu_id) }
            .to_string_lossy()
            .into_owned();

        match id.as_str() {
            "start" => { if let Some(ref cb) = cbs.on_start { cb(); } }
            "stop" => { if let Some(ref cb) = cbs.on_stop { cb(); } }
            "cluster_start" => { if let Some(ref cb) = cbs.on_cluster_start { cb(); } }
            "cluster_stop" => { if let Some(ref cb) = cbs.on_cluster_stop { cb(); } }
            "dashboard" => { if let Some(ref cb) = cbs.on_open_dashboard { cb(); } }
            "chat" => { if let Some(ref cb) = cbs.on_open_chat { cb(); } }
            "quit" => { if let Some(ref cb) = cbs.on_quit { cb(); } }
            _ => {}
        }
    }

    pub fn run_via_plugin_ui(
        on_start: Option<Box<dyn Fn() + Send + Sync>>,
        on_stop: Option<Box<dyn Fn() + Send + Sync>>,
        on_cluster_start: Option<Box<dyn Fn() + Send + Sync>>,
        on_cluster_stop: Option<Box<dyn Fn() + Send + Sync>>,
        on_open_dashboard: Option<Box<dyn Fn() + Send + Sync>>,
        on_open_chat: Option<Box<dyn Fn() + Send + Sync>>,
        on_quit: Option<Box<dyn Fn() + Send + Sync>>,
    ) {
        // 1. 查找 plugin-ui library
        let lib_path = match nemesis_utils::find_plugin_library("plugin_ui") {
            Some(p) => p,
            None => {
                let filename = nemesis_utils::plugin_library_filename("plugin_ui");
                eprintln!("[tray:linux] plugin-ui {} not found, tray disabled", filename);
                return;
            }
        };

        // 2. 加载 library
        let lib = unsafe {
            match libloading::Library::new(&lib_path) {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("[tray:linux] dlopen {} failed: {}", lib_path.display(), e);
                    return;
                }
            }
        };

        let tray_create: libloading::Symbol<
            unsafe extern "C" fn(*const c_char, *const nemesis_plugin::host_services::TrayCallbacks) -> i32
        > = unsafe {
            match lib.get(b"plugin_tray_create_indicator\0") {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("[tray:linux] symbol plugin_tray_create_indicator not found: {}", e);
                    return;
                }
            }
        };

        // 3. 加载图标 RGBA 数据
        let png_data = crate::icons::embedded_icon_png();
        let icon_data = match image::load_from_memory(png_data) {
            Ok(img) => {
                let rgba = img.to_rgba8();
                let (w, h) = rgba.dimensions();
                Some((rgba.into_raw(), w, h))
            }
            Err(e) => {
                eprintln!("[tray:linux] icon decode failed: {}", e);
                None
            }
        };

        // 4. 构造 config_json
        let menu_items = serde_json::json!([
            {"id": "start", "label": "启动服务", "enabled": true},
            {"id": "stop", "label": "停止服务", "enabled": true},
            {"id": "cluster_start", "label": "集群启动", "enabled": true},
            {"id": "cluster_stop", "label": "集群停止", "enabled": true},
            {"id": "dashboard", "label": "打开 Dashboard", "enabled": true},
            {"id": "chat", "label": "打开聊天", "enabled": true},
            {"id": "version", "label": "NemesisBot (linux)", "enabled": false},
            {"id": "quit", "label": "退出", "enabled": true},
        ]);

        let mut config = serde_json::json!({"menu_items": menu_items});
        if let Some((rgba, w, h)) = icon_data {
            config["icon_rgba"] = serde_json::json!(rgba);
            config["icon_width"] = serde_json::json!(w);
            config["icon_height"] = serde_json::json!(h);
        }

        let config_str = serde_json::to_string(&config).unwrap_or_default();
        let config_cstr = CString::new(config_str).unwrap_or_default();

        // 5. 构造 Callbacks 并泄漏到堆上（进程生命周期）
        let cbs = Box::new(Callbacks {
            on_start,
            on_stop,
            on_cluster_start,
            on_cluster_stop,
            on_open_dashboard,
            on_open_chat,
            on_quit,
        });
        let cbs_ptr = Box::into_raw(cbs);

        let tray_cbs = nemesis_plugin::host_services::TrayCallbacks {
            user_data: cbs_ptr as *mut std::os::raw::c_void,
            on_menu_click,
        };

        // 6+7. 调用 plugin_tray_create_indicator()，然后 forget lib
        // 注意：tray_create 借用 lib，必须先调用再 forget
        eprintln!("[tray:linux] creating tray indicator via plugin-ui.so");
        let rc = unsafe { tray_create(config_cstr.as_ptr(), &tray_cbs) };
        std::mem::forget(lib); // 防止 dlclose（tray 线程需要 so 持续加载）
        if rc != 0 {
            eprintln!("[tray:linux] plugin_tray_create_indicator failed: {}", rc);
            unsafe { let _ = Box::from_raw(cbs_ptr); }
        }
    }
}

/// Platform-native system tray using `tray-icon` + `winit`.
///
/// Creates a real system tray icon with a context menu on Windows, Linux,
/// and macOS. The tray runs on a dedicated thread with its own event loop.
///
/// # Menu structure (matches Go project)
///
/// ```text
/// 启动服务
/// 停止服务
/// 集群启动
/// 集群停止
/// ───────────
/// 打开 Dashboard
/// 打开聊天
/// ───────────
/// NemesisBot (windows)   [disabled]
/// ───────────
/// 退出
/// ```
///
/// Left-click double-click (400ms) opens the Dashboard.
pub struct PlatformTray {
    on_start: Option<Box<dyn Fn() + Send + Sync>>,
    on_stop: Option<Box<dyn Fn() + Send + Sync>>,
    on_cluster_start: Option<Box<dyn Fn() + Send + Sync>>,
    on_cluster_stop: Option<Box<dyn Fn() + Send + Sync>>,
    on_open_dashboard: Option<Box<dyn Fn() + Send + Sync>>,
    on_open_chat: Option<Box<dyn Fn() + Send + Sync>>,
    on_quit: Option<Box<dyn Fn() + Send + Sync>>,
}

impl PlatformTray {
    /// Creates a new `PlatformTray` with no callbacks registered.
    pub fn new() -> Self {
        Self {
            on_start: None,
            on_stop: None,
            on_cluster_start: None,
            on_cluster_stop: None,
            on_open_dashboard: None,
            on_open_chat: None,
            on_quit: None,
        }
    }

    /// Set callback for "Start Service" menu item.
    pub fn set_on_start(&mut self, cb: Box<dyn Fn() + Send + Sync>) {
        tracing::debug!("[Desktop] Start Service callback set");
        self.on_start = Some(cb);
    }

    /// Set callback for "Stop Service" menu item.
    pub fn set_on_stop(&mut self, cb: Box<dyn Fn() + Send + Sync>) {
        tracing::debug!("[Desktop] Stop Service callback set");
        self.on_stop = Some(cb);
    }

    /// Set callback for "Open Dashboard" menu item and double-click.
    pub fn set_on_open_dashboard(&mut self, cb: Box<dyn Fn() + Send + Sync>) {
        tracing::debug!("[Desktop] Open Dashboard callback set");
        self.on_open_dashboard = Some(cb);
    }

    /// Set callback for "Open Chat" menu item.
    pub fn set_on_open_chat(&mut self, cb: Box<dyn Fn() + Send + Sync>) {
        tracing::debug!("[Desktop] Open Chat callback set");
        self.on_open_chat = Some(cb);
    }

    /// Set callback for "Quit" menu item.
    pub fn set_on_quit(&mut self, cb: Box<dyn Fn() + Send + Sync>) {
        tracing::debug!("[Desktop] Quit callback set");
        self.on_quit = Some(cb);
    }

    /// Set callback for "Cluster Start" menu item.
    pub fn set_on_cluster_start(&mut self, cb: Box<dyn Fn() + Send + Sync>) {
        tracing::debug!("[Desktop] Cluster Start callback set");
        self.on_cluster_start = Some(cb);
    }

    /// Set callback for "Cluster Stop" menu item.
    pub fn set_on_cluster_stop(&mut self, cb: Box<dyn Fn() + Send + Sync>) {
        tracing::debug!("[Desktop] Cluster Stop callback set");
        self.on_cluster_stop = Some(cb);
    }

    /// Start the system tray on a dedicated thread.
    ///
    /// Returns the thread `JoinHandle`. The tray runs until the user
    /// clicks "Quit" (which calls `el.exit()`) or the process terminates.
    pub fn run(self) -> std::thread::JoinHandle<()> {
        std::thread::Builder::new()
            .name("nemesisbot-tray".into())
            .spawn(move || {
                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    self.run_event_loop();
                })) {
                    Ok(()) => {}
                    Err(e) => {
                        eprintln!("[tray] WARNING: Tray thread exited: {:?}", e);
                        tracing::warn!("[Desktop] Tray thread exited due to error: {:?}", e);
                    }
                }
            })
            .expect("failed to spawn tray thread")
    }

    fn run_event_loop(self) {
        #[cfg(target_os = "linux")]
        {
            linux_tray::run_via_plugin_ui(
                self.on_start,
                self.on_stop,
                self.on_cluster_start,
                self.on_cluster_stop,
                self.on_open_dashboard,
                self.on_open_chat,
                self.on_quit,
            );
            return;
        }

        #[cfg(not(target_os = "linux"))]
        {
            self.run_event_loop_native();
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn run_event_loop_native(self) {
        #[cfg(not(target_os = "windows"))]
        use std::sync::atomic::AtomicU64;
        #[cfg(not(target_os = "windows"))]
        use std::sync::Arc;
        use winit::event::{Event, StartCause};
        use winit::event_loop::EventLoop;

        // Create winit event loop with user event support.
        // On Windows and Linux, allow creation on a non-main thread (tray runs
        // on a dedicated "nemesisbot-tray" thread).
        #[cfg(target_os = "windows")]
        use winit::platform::windows::EventLoopBuilderExtWindows;
        #[cfg(target_os = "linux")]
        use winit::platform::wayland::EventLoopBuilderExtWayland;

        let event_loop = {
            #[cfg(target_os = "windows")]
            {
                let mut builder = EventLoop::<TrayUserEvent>::with_user_event();
                builder.with_any_thread(true);
                builder.build()
            }
            #[cfg(target_os = "linux")]
            {
                let mut builder = EventLoop::<TrayUserEvent>::with_user_event();
                // Both X11 and Wayland traits provide with_any_thread, but they
                // set the same internal flag. Use fully-qualified call to disambiguate.
                EventLoopBuilderExtWayland::with_any_thread(&mut builder, true);
                builder.build()
            }
            #[cfg(not(any(target_os = "windows", target_os = "linux")))]
            {
                EventLoop::<TrayUserEvent>::with_user_event().build()
            }
        };
        let event_loop = match event_loop {
            Ok(el) => el,
            Err(e) => {
                eprintln!("[tray] ERROR: Failed to create event loop: {}", e);
                tracing::error!("[Desktop] Failed to create tray event loop: {}", e);
                return;
            }
        };

        // Forward tray icon events to the winit event loop
        let proxy = event_loop.create_proxy();
        tray_icon::TrayIconEvent::set_event_handler(Some(move |event| {
            let _ = proxy.send_event(TrayUserEvent::Icon(event));
        }));

        let proxy = event_loop.create_proxy();
        tray_icon::menu::MenuEvent::set_event_handler(Some(move |event| {
            let _ = proxy.send_event(TrayUserEvent::Menu(event));
        }));

        // macOS: register a proxy so the gateway worker can request loop exit
        // after cleanup (covers Ctrl+C and other non-tray-initiated shutdown).
        #[cfg(target_os = "macos")]
        main_thread_handoff::set_exit_proxy(event_loop.create_proxy());

        // On Linux, initialize GTK before creating tray icon.
        // tray-icon uses libayatana-appindicator which depends on GTK,
        // and GTK must be initialized before any GTK operations.
        #[cfg(target_os = "linux")]
        if let Err(e) = try_init_gtk() {
            tracing::warn!("[Desktop] GTK init failed: {}, system tray disabled", e);
            eprintln!("[tray] WARNING: GTK init failed ({}) — tray icon disabled", e);
            return;
        }

        // Load icon
        let icon = match crate::icons::load_tray_icon_checked() {
            Ok(ic) => ic,
            Err(e) => {
                eprintln!("[tray] ERROR: Failed to load tray icon: {}", e);
                tracing::error!("[Desktop] Failed to load tray icon: {}", e);
                return;
            }
        };

        // Build menu (matching Go project structure)
        let menu = tray_icon::menu::Menu::new();
        let start_item = tray_icon::menu::MenuItem::with_id("start", "启动服务", true, None);
        let stop_item = tray_icon::menu::MenuItem::with_id("stop", "停止服务", true, None);
        let cluster_start_menu = tray_icon::menu::MenuItem::with_id("cluster_start", "集群启动", true, None);
        let cluster_stop_menu = tray_icon::menu::MenuItem::with_id("cluster_stop", "集群停止", true, None);
        let sep1 = tray_icon::menu::PredefinedMenuItem::separator();
        let dashboard_item = tray_icon::menu::MenuItem::with_id("dashboard", "打开 Dashboard", true, None);
        let chat_item = tray_icon::menu::MenuItem::with_id("chat", "打开聊天", true, None);
        let sep2 = tray_icon::menu::PredefinedMenuItem::separator();
        let version_item = tray_icon::menu::MenuItem::with_id("version", "NemesisBot (windows)", false, None);
        let sep3 = tray_icon::menu::PredefinedMenuItem::separator();
        let quit_item = tray_icon::menu::MenuItem::with_id("quit", "退出", true, None);

        let _ = menu.append(&start_item);
        let _ = menu.append(&stop_item);
        let _ = menu.append(&cluster_start_menu);
        let _ = menu.append(&cluster_stop_menu);
        let _ = menu.append(&sep1);
        let _ = menu.append(&dashboard_item);
        let _ = menu.append(&chat_item);
        let _ = menu.append(&sep2);
        let _ = menu.append(&version_item);
        let _ = menu.append(&sep3);
        let _ = menu.append(&quit_item);

        // Populate thread-local so enable/disable functions can reach these items
        CLUSTER_MENU_ITEMS.with(|items| {
            *items.borrow_mut() = Some((cluster_start_menu, cluster_stop_menu));
        });

        // Create tray icon with menu bound, but disable left-click popup.
        // Only right-click shows the menu; left-double-click opens Dashboard.
        let _tray_icon = match tray_icon::TrayIconBuilder::new()
            .with_icon(icon)
            .with_tooltip("NemesisBot - AI Agent")
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()
        {
            Ok(ti) => ti,
            Err(e) => {
                eprintln!("[tray] ERROR: Failed to build tray icon: {:?}", e);
                tracing::error!("[Desktop] Failed to build tray icon: {:?}", e);
                return;
            }
        };

        eprintln!("[tray] System tray icon created successfully");
        tracing::info!("[Desktop] System tray icon created and running");

        // Double-click detection state for non-Windows platforms (400ms timer)
        #[cfg(not(target_os = "windows"))]
        let last_click_time = Arc::new(AtomicU64::new(0));
        #[cfg(not(target_os = "windows"))]
        let last_click_time_clone = last_click_time.clone();

        // Move callbacks into the event loop closure
        let on_start = self.on_start;
        let on_stop = self.on_stop;
        let on_cluster_start = self.on_cluster_start;
        let on_cluster_stop = self.on_cluster_stop;
        let on_open_dashboard = self.on_open_dashboard;
        let on_open_chat = self.on_open_chat;
        let on_quit = self.on_quit;

        #[allow(deprecated)]
        event_loop.run(|event, el| {
            match event {
                Event::NewEvents(StartCause::Init) => {
                    // Event loop started
                }
                Event::UserEvent(TrayUserEvent::Menu(menu_event)) => {
                    let id = menu_event.id().as_ref();
                    tracing::debug!(menu_id = id, "[Desktop] Tray menu item selected");
                    match id {
                        "start" => {
                            tracing::info!("[Desktop] Start Service clicked");
                            if let Some(ref cb) = on_start { cb(); }
                        }
                        "stop" => {
                            tracing::info!("[Desktop] Stop Service clicked");
                            if let Some(ref cb) = on_stop { cb(); }
                        }
                        "cluster_start" => {
                            tracing::info!("[Desktop] Cluster Start clicked");
                            if let Some(ref cb) = on_cluster_start { cb(); }
                        }
                        "cluster_stop" => {
                            tracing::info!("[Desktop] Cluster Stop clicked");
                            if let Some(ref cb) = on_cluster_stop { cb(); }
                        }
                        "dashboard" => {
                            tracing::info!("[Desktop] Open Dashboard clicked");
                            if let Some(ref cb) = on_open_dashboard { cb(); }
                        }
                        "chat" => {
                            tracing::info!("[Desktop] Open Chat clicked");
                            if let Some(ref cb) = on_open_chat { cb(); }
                        }
                        "quit" => {
                            tracing::info!("[Desktop] Quit clicked, exiting tray event loop");
                            if let Some(ref cb) = on_quit { cb(); }
                            el.exit();
                        }
                        _ => {}
                    }
                }
                Event::UserEvent(TrayUserEvent::Icon(icon_event)) => {
                    match &icon_event {
                        tray_icon::TrayIconEvent::DoubleClick { .. } => {
                            tracing::debug!("[Desktop] Tray icon double-clicked, opening Dashboard");
                            if let Some(ref cb) = on_open_dashboard { cb(); }
                        }
                        tray_icon::TrayIconEvent::Click { button, .. } => {
                            // On non-Windows platforms, use 400ms double-click detection
                            // for left-click to open Dashboard.
                            #[cfg(not(target_os = "windows"))]
                            if *button == tray_icon::MouseButton::Left {
                                let now = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_millis() as u64;
                                let prev = last_click_time_clone.load(Ordering::SeqCst);
                                if prev > 0 && now.saturating_sub(prev) < 400 {
                                    if let Some(ref cb) = on_open_dashboard { cb(); }
                                    last_click_time_clone.store(0, Ordering::SeqCst);
                                } else {
                                    last_click_time_clone.store(now, Ordering::SeqCst);
                                }
                            }
                            #[cfg(target_os = "windows")]
                            let _ = button;
                        }
                        _ => {}
                    }
                }
                #[cfg(target_os = "macos")]
                Event::UserEvent(TrayUserEvent::Exit) => {
                    tracing::info!("[Desktop] Tray exit requested, exiting event loop");
                    el.exit();
                }
                _ => {}
            }
        }).expect("tray event loop error");
    }
}

#[cfg(target_os = "macos")]
impl PlatformTray {
    /// Run the tray event loop on the CURRENT thread (macOS: the main thread).
    ///
    /// On macOS, winit's `EventLoop` must be created and run on the process
    /// main thread. The gateway worker hands the configured tray to the main
    /// thread via [`main_thread_handoff`], which calls this. Blocks until the
    /// loop exits (quit menu item, or an external `request_exit()`).
    pub fn run_on_current_thread(self) {
        self.run_event_loop_native();
    }
}

impl Default for PlatformTray {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
