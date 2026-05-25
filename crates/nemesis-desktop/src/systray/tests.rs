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
// PlatformTray tests (desktop only — requires tray-icon)
// ============================================================

#[cfg(not(target_os = "android"))]
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
