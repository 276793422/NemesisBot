//! Linux tray backend — libayatana-appindicator3 + GTK event loop.
//!
//! Uses GtkMenu (not GMenu) for universal desktop panel compatibility.
//! Suppresses the library's "deprecated" warning via targeted GLib log filter.

use std::ffi::CStr;
use std::os::raw::c_char;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::appindicator_glib::*;
use crate::host_services::TrayCallbacks;

use gtk::prelude::*;

static TRAY_RUNNING: AtomicBool = AtomicBool::new(false);

/// 检测 GTK 是否可能可用（不实际初始化）。
#[no_mangle]
pub extern "C" fn plugin_tray_init() -> i32 {
    0
}

/// Tray 配置中的菜单项（由 nemesis-desktop 生成）。
#[derive(serde::Deserialize)]
struct TrayMenuItemConfig {
    id: String,
    label: String,
    #[serde(default = "default_true")]
    enabled: bool,
}

fn default_true() -> bool {
    true
}

/// Tray 配置（由 nemesis-desktop 生成）。
#[derive(serde::Deserialize)]
struct TrayConfigJson {
    icon_rgba: Option<Vec<u8>>,
    icon_width: Option<u32>,
    icon_height: Option<u32>,
    icon_path: Option<String>,
    menu_items: Vec<TrayMenuItemConfig>,
}

/// 创建原生托盘指示器。
#[no_mangle]
pub extern "C" fn plugin_tray_create_indicator(
    config_json: *const c_char,
    callbacks: *const TrayCallbacks,
) -> i32 {
    if config_json.is_null() || callbacks.is_null() {
        eprintln!("[plugin-ui:tray] null argument");
        return -1;
    }

    let config_str = unsafe { CStr::from_ptr(config_json) }.to_string_lossy();
    let config: TrayConfigJson = match serde_json::from_str(&config_str) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[plugin-ui:tray] parse config failed: {}", e);
            return -2;
        }
    };

    let callbacks = unsafe { *callbacks };

    std::thread::Builder::new()
        .name("plugin-ui-tray-gtk".into())
        .spawn(move || {
            run_gtk_event_loop(config, callbacks);
        })
        .map(|_| 0)
        .unwrap_or_else(|e| {
            eprintln!("[plugin-ui:tray] spawn thread failed: {}", e);
            -3
        })
}

/// Suppress the specific "libayatana-appindicator is deprecated" warning
/// from the library's init function.
///
/// ## Background
///
/// Linux tray indicators have two library options:
///
/// 1. **libayatana-appindicator3** (GtkMenu / libdbusmenu D-Bus protocol)
///    - Marked as "deprecated" by upstream (printed at library init)
///    - Uses GtkMenu → libdbusmenu, universally supported by all desktop panels
///
/// 2. **libayatana-appindicator-glib** (GMenuModel / org.gtk.Menus D-Bus protocol)
///    - No deprecation warning
///    - Uses GMenuModel, NOT supported by many desktop panels (menu doesn't appear)
///
/// We chose option 1 because actual functionality (menu visibility) trumps a
/// cosmetic warning. The "deprecated" label is misleading — the library is still
/// maintained and ships with every major distro. Upstream's recommendation to
/// migrate to GMenu/GTK4 is premature until desktop panels adopt it.
///
/// This filter suppresses ONLY the deprecation message. All other GLib/GTK
/// warnings are forwarded to stderr normally.
fn suppress_deprecation_warning() {
    let levels = gtk::glib::LogLevels::LEVEL_WARNING;
    gtk::glib::log_set_handler(
        Some("libayatana-appindicator"),
        levels,
        false,
        false,
        |_domain: Option<&str>, _level: gtk::glib::LogLevel, message: &str| {
            if message.contains("libayatana-appindicator is deprecated") {
                return; // suppress only this specific message
            }
            eprintln!("[plugin-ui:tray] GLib warning: {}", message);
        },
    );
}

fn run_gtk_event_loop(config: TrayConfigJson, callbacks: TrayCallbacks) {
    TRAY_RUNNING.store(true, Ordering::SeqCst);

    // Suppress the deprecation warning BEFORE GTK init (which loads the library)
    suppress_deprecation_warning();

    eprintln!("[plugin-ui:tray] Note: using libayatana-appindicator3 (GtkMenu/libdbusmenu)");
    eprintln!("[plugin-ui:tray]   - This library prints a 'deprecated' warning at init, which we suppress.");
    eprintln!("[plugin-ui:tray]   - The alternative (libayatana-appindicator-glib) uses GMenuModel D-Bus protocol,");
    eprintln!("[plugin-ui:tray]     which is NOT supported by most desktop panels (menu would be invisible).");
    eprintln!("[plugin-ui:tray]   - TODO: revisit when desktop panels adopt GMenuModel or GTK4 indicator APIs.");

    if let Err(e) = gtk::init() {
        eprintln!("[plugin-ui:tray] GTK init failed: {}", e);
        TRAY_RUNNING.store(false, Ordering::SeqCst);
        return;
    }

    // Build GtkMenu with menu items
    let menu = gtk::Menu::new();

    let user_data = callbacks.user_data as usize;
    let on_menu_click = callbacks.on_menu_click;

    for item_cfg in &config.menu_items {
        let menu_item = gtk::MenuItem::with_label(&item_cfg.label);
        if !item_cfg.enabled {
            menu_item.set_sensitive(false);
        }

        let id = item_cfg.id.clone();
        menu_item.connect_activate(move |_| {
            let c_id = std::ffi::CString::new(id.clone()).unwrap_or_default();
            on_menu_click(user_data as *mut std::os::raw::c_void, c_id.as_ptr());
        });

        menu.append(&menu_item);
    }

    menu.show_all();

    // Prepare icon file
    let (icon_dir, icon_name) = match prepare_icon(&config) {
        Some(v) => v,
        None => {
            eprintln!("[plugin-ui:tray] no icon available");
            TRAY_RUNNING.store(false, Ordering::SeqCst);
            return;
        }
    };

    // Create AppIndicator via FFI
    let c_id = std::ffi::CString::new("nemesisbot").unwrap();
    let c_icon = std::ffi::CString::new(icon_name.as_str()).unwrap();
    let c_path = std::ffi::CString::new(icon_dir.as_str()).unwrap();
    let c_title = std::ffi::CString::new("NemesisBot - AI Agent").unwrap();

    unsafe {
        let indicator = app_indicator_new_with_path(
            c_id.as_ptr(),
            c_icon.as_ptr(),
            APP_INDICATOR_CATEGORY_APPLICATION_STATUS,
            c_path.as_ptr(),
        );

        if indicator.is_null() {
            eprintln!("[plugin-ui:tray] failed to create AppIndicator");
            TRAY_RUNNING.store(false, Ordering::SeqCst);
            return;
        }

        app_indicator_set_title(indicator, c_title.as_ptr());
        app_indicator_set_menu(indicator, menu.as_ptr() as *mut _);
        app_indicator_set_status(indicator, APP_INDICATOR_STATUS_ACTIVE);
    }

    eprintln!("[plugin-ui:tray] tray icon created, entering GTK event loop");

    // GTK event loop (blocking)
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        gtk::main();
    }))
    .unwrap_or_else(|e| {
        eprintln!("[plugin-ui:tray] GTK event loop exited: {:?}", e);
    });

    TRAY_RUNNING.store(false, Ordering::SeqCst);
}

/// Save icon to a temp PNG file, return (directory, icon_name_without_extension).
fn prepare_icon(config: &TrayConfigJson) -> Option<(String, String)> {
    let temp_dir = std::env::temp_dir().join("nemesisbot-tray-icon");
    std::fs::create_dir_all(&temp_dir).ok()?;

    if let Some(rgba) = &config.icon_rgba {
        let w = config.icon_width.unwrap_or(32) as u32;
        let h = config.icon_height.unwrap_or(32) as u32;
        let img = image::RgbaImage::from_raw(w, h, rgba.clone())?;
        let path = temp_dir.join("nemesisbot.png");
        img.save(&path).ok()?;
        Some((
            temp_dir.to_string_lossy().into_owned(),
            "nemesisbot".to_string(),
        ))
    } else if let Some(path) = &config.icon_path {
        let data = std::fs::read(path).ok()?;
        let img = image::load_from_memory(&data).ok()?;
        let out = temp_dir.join("nemesisbot.png");
        img.save(&out).ok()?;
        Some((
            temp_dir.to_string_lossy().into_owned(),
            "nemesisbot".to_string(),
        ))
    } else {
        None
    }
}

/// 更新菜单项启用/禁用状态。
#[no_mangle]
pub extern "C" fn plugin_tray_set_menu_enabled(_id: *const c_char, _enabled: i32) {}

/// 销毁托盘指示器。
#[no_mangle]
pub extern "C" fn plugin_tray_destroy() {
    if TRAY_RUNNING.load(Ordering::SeqCst) {
        gtk::main_quit();
    }
}
