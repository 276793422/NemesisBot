//! System tray management.
//!
//! Provides a platform-agnostic system tray abstraction with configurable menus,
//! action dispatch, and callback registration. Concrete platform integration
//! (Windows tray, Linux AppIndicator, macOS NSStatusItem) requires
//! platform-specific backend code that hooks into the types defined here.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
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
        self.callbacks.write().expect("callbacks lock poisoned").insert(menu_id.into(), callback);
    }

    /// Programmatically triggers an action by menu item id.
    ///
    /// Resolves the id to a [`TrayAction`], looks up any registered callback,
    /// and invokes it. Returns `true` if a callback was found and invoked,
    /// `false` otherwise.
    pub fn trigger_action(&self, menu_id: &str) -> bool {
        let action = match TrayAction::from_menu_id(menu_id) {
            Some(a) => a,
            None => return false,
        };

        let cb = self.callbacks.read().expect("callbacks lock poisoned").get(menu_id).cloned();
        match cb {
            Some(callback) => {
                callback(action);
                true
            }
            None => false,
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

/// User event types forwarded from tray-icon to the winit event loop.
#[derive(Debug, Clone)]
enum TrayUserEvent {
    /// A tray icon event (click, double-click, etc.).
    Icon(tray_icon::TrayIconEvent),
    /// A context menu event (menu item selected).
    Menu(tray_icon::menu::MenuEvent),
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
            on_open_dashboard: None,
            on_open_chat: None,
            on_quit: None,
        }
    }

    /// Set callback for "Start Service" menu item.
    pub fn set_on_start(&mut self, cb: Box<dyn Fn() + Send + Sync>) {
        self.on_start = Some(cb);
    }

    /// Set callback for "Stop Service" menu item.
    pub fn set_on_stop(&mut self, cb: Box<dyn Fn() + Send + Sync>) {
        self.on_stop = Some(cb);
    }

    /// Set callback for "Open Dashboard" menu item and double-click.
    pub fn set_on_open_dashboard(&mut self, cb: Box<dyn Fn() + Send + Sync>) {
        self.on_open_dashboard = Some(cb);
    }

    /// Set callback for "Open Chat" menu item.
    pub fn set_on_open_chat(&mut self, cb: Box<dyn Fn() + Send + Sync>) {
        self.on_open_chat = Some(cb);
    }

    /// Set callback for "Quit" menu item.
    pub fn set_on_quit(&mut self, cb: Box<dyn Fn() + Send + Sync>) {
        self.on_quit = Some(cb);
    }

    /// Start the system tray on a dedicated thread.
    ///
    /// Returns the thread `JoinHandle`. The tray runs until the user
    /// clicks "Quit" (which calls `el.exit()`) or the process terminates.
    pub fn run(self) -> std::thread::JoinHandle<()> {
        std::thread::Builder::new()
            .name("nemesisbot-tray".into())
            .spawn(move || {
                self.run_event_loop();
            })
            .expect("failed to spawn tray thread")
    }

    fn run_event_loop(self) {
        #[cfg(not(target_os = "windows"))]
        use std::sync::atomic::AtomicU64;
        #[cfg(not(target_os = "windows"))]
        use std::sync::Arc;
        use winit::event::{Event, StartCause};
        use winit::event_loop::EventLoop;

        // Create winit event loop with user event support.
        // On Windows, allow creation on a non-main thread (tray runs on a
        // dedicated "nemesisbot-tray" thread).
        #[cfg(target_os = "windows")]
        use winit::platform::windows::EventLoopBuilderExtWindows;

        let event_loop = {
            #[cfg(target_os = "windows")]
            {
                let mut builder = EventLoop::<TrayUserEvent>::with_user_event();
                builder.with_any_thread(true);
                builder.build()
            }
            #[cfg(not(target_os = "windows"))]
            {
                EventLoop::<TrayUserEvent>::with_user_event().build()
            }
        };
        let event_loop = match event_loop {
            Ok(el) => el,
            Err(e) => {
                eprintln!("[tray] ERROR: Failed to create event loop: {}", e);
                tracing::error!("Failed to create tray event loop: {}", e);
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

        // Load icon
        let icon = match crate::icons::load_tray_icon_checked() {
            Ok(ic) => ic,
            Err(e) => {
                eprintln!("[tray] ERROR: Failed to load tray icon: {}", e);
                tracing::error!("Failed to load tray icon: {}", e);
                return;
            }
        };

        // Build menu (matching Go project structure)
        let menu = tray_icon::menu::Menu::new();
        let start_item = tray_icon::menu::MenuItem::with_id("start", "启动服务", true, None);
        let stop_item = tray_icon::menu::MenuItem::with_id("stop", "停止服务", true, None);
        let sep1 = tray_icon::menu::PredefinedMenuItem::separator();
        let dashboard_item = tray_icon::menu::MenuItem::with_id("dashboard", "打开 Dashboard", true, None);
        let chat_item = tray_icon::menu::MenuItem::with_id("chat", "打开聊天", true, None);
        let sep2 = tray_icon::menu::PredefinedMenuItem::separator();
        let version_item = tray_icon::menu::MenuItem::with_id("version", "NemesisBot (windows)", false, None);
        let sep3 = tray_icon::menu::PredefinedMenuItem::separator();
        let quit_item = tray_icon::menu::MenuItem::with_id("quit", "退出", true, None);

        let _ = menu.append(&start_item);
        let _ = menu.append(&stop_item);
        let _ = menu.append(&sep1);
        let _ = menu.append(&dashboard_item);
        let _ = menu.append(&chat_item);
        let _ = menu.append(&sep2);
        let _ = menu.append(&version_item);
        let _ = menu.append(&sep3);
        let _ = menu.append(&quit_item);

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
                tracing::error!("Failed to build tray icon: {:?}", e);
                return;
            }
        };

        eprintln!("[tray] System tray icon created successfully");
        tracing::info!("System tray icon created and running");

        // Double-click detection state for non-Windows platforms (400ms timer)
        #[cfg(not(target_os = "windows"))]
        let last_click_time = Arc::new(AtomicU64::new(0));
        #[cfg(not(target_os = "windows"))]
        let last_click_time_clone = last_click_time.clone();

        // Move callbacks into the event loop closure
        let on_start = self.on_start;
        let on_stop = self.on_stop;
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
                    match id {
                        "start" => {
                            if let Some(ref cb) = on_start { cb(); }
                        }
                        "stop" => {
                            if let Some(ref cb) = on_stop { cb(); }
                        }
                        "dashboard" => {
                            if let Some(ref cb) = on_open_dashboard { cb(); }
                        }
                        "chat" => {
                            if let Some(ref cb) = on_open_chat { cb(); }
                        }
                        "quit" => {
                            if let Some(ref cb) = on_quit { cb(); }
                            el.exit();
                        }
                        _ => {}
                    }
                }
                Event::UserEvent(TrayUserEvent::Icon(icon_event)) => {
                    match &icon_event {
                        tray_icon::TrayIconEvent::DoubleClick { .. } => {
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
                _ => {}
            }
        }).expect("tray event loop error");
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
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn test_tray_config_default() {
        let cfg = TrayConfig::default();
        assert_eq!(cfg.title, "NemesisBot");
        assert_eq!(cfg.tooltip, "NemesisBot - AI Agent");
        assert_eq!(cfg.menu_items.len(), 5);
        assert_eq!(cfg.menu_items[0].id, "start");
        assert!(cfg.menu_items[0].enabled);
        assert_eq!(cfg.menu_items[3].id, "version");
        assert!(!cfg.menu_items[3].enabled);
    }

    #[test]
    fn test_add_menu_item_and_trigger() {
        let tray = SystemTray::default_tray();
        assert_eq!(tray.menu_count(), 5);

        // Add an extra item with a known action id
        tray.add_menu_item(MenuItem::new("quit", "Quit (extra)", true));
        assert_eq!(tray.menu_count(), 6);

        let invoked = Arc::new(AtomicUsize::new(0));
        let invoked_clone = invoked.clone();
        tray.on_click("quit", Arc::new(move |action| {
            assert_eq!(action, TrayAction::Quit);
            invoked_clone.fetch_add(1, Ordering::SeqCst);
        }));

        assert!(tray.trigger_action("quit"));
        assert_eq!(invoked.load(Ordering::SeqCst), 1);

        // Unknown id returns false (no TrayAction mapping)
        assert!(!tray.trigger_action("nonexistent"));
        assert_eq!(invoked.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_set_menu_enabled() {
        let tray = SystemTray::default_tray();
        let cfg = tray.config();
        let version_item = cfg.menu_items.iter().find(|m| m.id == "version").unwrap();
        assert!(!version_item.enabled);

        drop(cfg); // release read lock

        assert!(tray.set_menu_enabled("version", true));
        let cfg = tray.config();
        let version_item = cfg.menu_items.iter().find(|m| m.id == "version").unwrap();
        assert!(version_item.enabled);

        assert!(!tray.set_menu_enabled("nonexistent", true));
    }

    #[test]
    fn test_trigger_known_actions_with_callbacks() {
        let tray = SystemTray::default_tray();
        let quit_count = Arc::new(AtomicUsize::new(0));
        let quit_clone = quit_count.clone();

        tray.on_click("quit", Arc::new(move |action| {
            assert_eq!(action, TrayAction::Quit);
            quit_clone.fetch_add(1, Ordering::SeqCst);
        }));

        assert!(tray.trigger_action("quit"));
        assert_eq!(quit_count.load(Ordering::SeqCst), 1);

        // start has no callback registered, returns false
        assert!(!tray.trigger_action("start"));
    }

    // ---- New tests ----

    #[test]
    fn test_menu_item_new() {
        let item = MenuItem::new("custom", "Custom Action", true);
        assert_eq!(item.id, "custom");
        assert_eq!(item.label, "Custom Action");
        assert!(item.enabled);
    }

    #[test]
    fn test_menu_item_disabled() {
        let item = MenuItem::new("info", "Info", false);
        assert!(!item.enabled);
    }

    #[test]
    fn test_tray_action_from_menu_id_all() {
        assert_eq!(TrayAction::from_menu_id("start"), Some(TrayAction::StartService));
        assert_eq!(TrayAction::from_menu_id("stop"), Some(TrayAction::StopService));
        assert_eq!(TrayAction::from_menu_id("webui"), Some(TrayAction::OpenWebUI));
        assert_eq!(TrayAction::from_menu_id("version"), Some(TrayAction::Version));
        assert_eq!(TrayAction::from_menu_id("quit"), Some(TrayAction::Quit));
    }

    #[test]
    fn test_tray_action_from_menu_id_unknown() {
        assert_eq!(TrayAction::from_menu_id("unknown"), None);
        assert_eq!(TrayAction::from_menu_id(""), None);
        assert_eq!(TrayAction::from_menu_id("START"), None); // case-sensitive
    }

    #[test]
    fn test_tray_action_equality() {
        assert_eq!(TrayAction::StartService, TrayAction::StartService);
        assert_ne!(TrayAction::StartService, TrayAction::StopService);
    }

    #[test]
    fn test_tray_action_serialization() {
        let action = TrayAction::Quit;
        let json = serde_json::to_string(&action).unwrap();
        let parsed: TrayAction = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, TrayAction::Quit);
    }

    #[test]
    fn test_tray_action_all_variants_serde() {
        for action in [TrayAction::StartService, TrayAction::StopService, TrayAction::OpenWebUI, TrayAction::Version, TrayAction::Quit] {
            let json = serde_json::to_string(&action).unwrap();
            let parsed: TrayAction = serde_json::from_str(&json).unwrap();
            assert_eq!(action, parsed);
        }
    }

    #[test]
    fn test_system_tray_new_custom_config() {
        let config = TrayConfig {
            title: "Custom".into(),
            tooltip: "Custom tooltip".into(),
            menu_items: vec![MenuItem::new("a", "Action A", true)],
        };
        let tray = SystemTray::new(config);
        assert_eq!(tray.config().title, "Custom");
        assert_eq!(tray.menu_count(), 1);
    }

    #[test]
    fn test_system_tray_default_tray() {
        let tray = SystemTray::default_tray();
        assert_eq!(tray.config().title, "NemesisBot");
        assert_eq!(tray.menu_count(), 5);
    }

    #[test]
    fn test_set_tooltip() {
        let tray = SystemTray::default_tray();
        tray.set_tooltip("New Tooltip");
        assert_eq!(tray.config().tooltip, "New Tooltip");
    }

    #[test]
    fn test_add_multiple_menu_items() {
        let tray = SystemTray::default_tray();
        assert_eq!(tray.menu_count(), 5);
        tray.add_menu_item(MenuItem::new("custom1", "Custom 1", true));
        tray.add_menu_item(MenuItem::new("custom2", "Custom 2", false));
        assert_eq!(tray.menu_count(), 7);
    }

    #[test]
    fn test_set_menu_enabled_not_found() {
        let tray = SystemTray::default_tray();
        assert!(!tray.set_menu_enabled("nonexistent_item", true));
    }

    #[test]
    fn test_trigger_action_no_callback() {
        let tray = SystemTray::default_tray();
        // "start" has a TrayAction mapping but no callback registered
        assert!(!tray.trigger_action("start"));
    }

    #[test]
    fn test_trigger_action_multiple_callbacks() {
        let tray = SystemTray::default_tray();
        let count = Arc::new(AtomicUsize::new(0));

        let c1 = count.clone();
        tray.on_click("start", Arc::new(move |_| { c1.fetch_add(1, Ordering::SeqCst); }));
        let c2 = count.clone();
        tray.on_click("stop", Arc::new(move |_| { c2.fetch_add(10, Ordering::SeqCst); }));

        assert!(tray.trigger_action("start"));
        assert_eq!(count.load(Ordering::SeqCst), 1);

        assert!(tray.trigger_action("stop"));
        assert_eq!(count.load(Ordering::SeqCst), 11);
    }

    #[test]
    fn test_menu_item_serialization() {
        let item = MenuItem::new("test", "Test Item", true);
        let json = serde_json::to_string(&item).unwrap();
        let parsed: MenuItem = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, "test");
        assert_eq!(parsed.label, "Test Item");
        assert!(parsed.enabled);
    }

    #[test]
    fn test_tray_config_serialization() {
        let config = TrayConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let parsed: TrayConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.title, "NemesisBot");
        assert_eq!(parsed.menu_items.len(), 5);
    }

    #[test]
    fn test_tray_action_debug() {
        let debug = format!("{:?}", TrayAction::StartService);
        assert!(debug.contains("StartService"));
    }

    #[test]
    fn test_menu_item_debug() {
        let item = MenuItem::new("x", "X", true);
        let debug = format!("{:?}", item);
        assert!(debug.contains("x"));
    }

    #[test]
    fn test_set_menu_enabled_toggle_multiple() {
        let tray = SystemTray::default_tray();
        assert!(tray.set_menu_enabled("start", false));
        assert!(!tray.config().menu_items[0].enabled);
        assert!(tray.set_menu_enabled("start", true));
        assert!(tray.config().menu_items[0].enabled);
    }

    // ============================================================
    // PlatformTray tests (feature-gated, no display server needed)
    // ============================================================

    mod platform_tray_tests {
        use super::*;
        use std::sync::atomic::{AtomicBool, Ordering};

        #[test]
        fn test_platform_tray_new_default() {
            let tray = PlatformTray::new();
            // Just verify construction doesn't panic
            let _ = tray;
        }

        #[test]
        fn test_platform_tray_default_trait() {
            let tray = PlatformTray::default();
            let _ = tray;
        }

        #[test]
        fn test_platform_tray_set_on_start() {
            let mut tray = PlatformTray::new();
            let called = Arc::new(AtomicBool::new(false));
            let called_clone = called.clone();
            tray.set_on_start(Box::new(move || {
                called_clone.store(true, Ordering::SeqCst);
            }));
            // Callback was set (we can't call it directly, but we verify no panic)
            let _ = tray;
        }

        #[test]
        fn test_platform_tray_set_on_stop() {
            let mut tray = PlatformTray::new();
            let called = Arc::new(AtomicBool::new(false));
            let called_clone = called.clone();
            tray.set_on_stop(Box::new(move || {
                called_clone.store(true, Ordering::SeqCst);
            }));
            let _ = tray;
        }

        #[test]
        fn test_platform_tray_set_on_open_dashboard() {
            let mut tray = PlatformTray::new();
            let called = Arc::new(AtomicBool::new(false));
            let called_clone = called.clone();
            tray.set_on_open_dashboard(Box::new(move || {
                called_clone.store(true, Ordering::SeqCst);
            }));
            let _ = tray;
        }

        #[test]
        fn test_platform_tray_set_on_open_chat() {
            let mut tray = PlatformTray::new();
            let called = Arc::new(AtomicBool::new(false));
            let called_clone = called.clone();
            tray.set_on_open_chat(Box::new(move || {
                called_clone.store(true, Ordering::SeqCst);
            }));
            let _ = tray;
        }

        #[test]
        fn test_platform_tray_set_on_quit() {
            let mut tray = PlatformTray::new();
            let called = Arc::new(AtomicBool::new(false));
            let called_clone = called.clone();
            tray.set_on_quit(Box::new(move || {
                called_clone.store(true, Ordering::SeqCst);
            }));
            let _ = tray;
        }

        #[test]
        fn test_platform_tray_all_callbacks() {
            let mut tray = PlatformTray::new();
            let counter = Arc::new(AtomicUsize::new(0));

            let c = counter.clone();
            tray.set_on_start(Box::new(move || { c.fetch_add(1, Ordering::SeqCst); }));
            let c = counter.clone();
            tray.set_on_stop(Box::new(move || { c.fetch_add(10, Ordering::SeqCst); }));
            let c = counter.clone();
            tray.set_on_open_dashboard(Box::new(move || { c.fetch_add(100, Ordering::SeqCst); }));
            let c = counter.clone();
            tray.set_on_open_chat(Box::new(move || { c.fetch_add(1000, Ordering::SeqCst); }));
            let c = counter.clone();
            tray.set_on_quit(Box::new(move || { c.fetch_add(10000, Ordering::SeqCst); }));

            let _ = tray;
            // Callbacks are set but not invoked (requires event loop).
            // Counter should still be 0.
            assert_eq!(counter.load(Ordering::SeqCst), 0);
        }

        #[test]
        fn test_platform_tray_overwrite_callback() {
            let mut tray = PlatformTray::new();
            // Set first callback
            tray.set_on_start(Box::new(|| {}));
            // Overwrite with second
            tray.set_on_start(Box::new(|| {}));
            let _ = tray;
        }
    }
}
