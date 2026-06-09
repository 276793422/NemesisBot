//! Linux tray backend — GTK init + libayatana-appindicator + GTK event loop.
//!
//! 只做三件事：
//! 1. 在 GTK 线程内初始化 GTK
//! 2. 创建原生托盘图标 + 菜单
//! 3. 用户点击时通过 on_menu_click 回调通知 nemesis-desktop
//!
//! 关键约束：gtk::init() 和 gtk::main() 必须在同一线程。
//! 所以 GTK init 放在 plugin_tray_create_indicator 创建的线程内，
//! 而不是单独的 plugin_tray_init 调用。

use std::ffi::CStr;
use std::os::raw::c_char;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::host_services::TrayCallbacks;

static TRAY_RUNNING: AtomicBool = AtomicBool::new(false);

/// 检测 GTK 是否可能可用（不实际初始化）。
///
/// 用于 nemesis-desktop 在加载 so 前做预检。
/// 实际 GTK 初始化在 plugin_tray_create_indicator 创建的线程内完成。
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

fn default_true() -> bool { true }

/// Tray 配置（由 nemesis-desktop 生成）。
#[derive(serde::Deserialize)]
struct TrayConfigJson {
    /// 图标 RGBA 数据（可选，优先于 icon_path）
    icon_rgba: Option<Vec<u8>>,
    icon_width: Option<u32>,
    icon_height: Option<u32>,
    /// 图标文件路径（备选方案）
    icon_path: Option<String>,
    /// 菜单项列表
    menu_items: Vec<TrayMenuItemConfig>,
}

/// 创建原生托盘指示器。
///
/// 非阻塞 — 内部创建线程运行 GTK 事件循环。
/// GTK 初始化在同一线程内完成（gtk::init + gtk::main 必须同线程）。
/// 用户点击菜单项时通过 callbacks.on_menu_click 通知 nemesis-desktop。
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

fn run_gtk_event_loop(config: TrayConfigJson, callbacks: TrayCallbacks) {
    TRAY_RUNNING.store(true, Ordering::SeqCst);

    // 在此线程内初始化 GTK（gtk::init 和 gtk::main 必须同线程）
    if let Err(e) = gtk::init() {
        eprintln!("[plugin-ui:tray] GTK init failed: {}", e);
        TRAY_RUNNING.store(false, Ordering::SeqCst);
        return;
    }

    // 构建菜单
    let menu = tray_icon::menu::Menu::new();
    let mut menu_items: Vec<(String, tray_icon::menu::MenuItem)> = Vec::new();

    for item_cfg in &config.menu_items {
        let item = tray_icon::menu::MenuItem::with_id(
            &item_cfg.id,
            &item_cfg.label,
            item_cfg.enabled,
            None,
        );
        let _ = menu.append(&item);
        menu_items.push((item_cfg.id.clone(), item));
    }

    // 构建图标
    let icon = build_icon(&config);

    // 创建 tray icon
    let _tray_icon = match tray_icon::TrayIconBuilder::new()
        .with_icon(match icon {
            Some(ic) => ic,
            None => {
                eprintln!("[plugin-ui:tray] no icon available");
                TRAY_RUNNING.store(false, Ordering::SeqCst);
                return;
            }
        })
        .with_tooltip("NemesisBot - AI Agent")
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()
    {
        Ok(ti) => ti,
        Err(e) => {
            eprintln!("[plugin-ui:tray] build tray icon failed: {:?}", e);
            TRAY_RUNNING.store(false, Ordering::SeqCst);
            return;
        }
    };

    eprintln!("[plugin-ui:tray] tray icon created, entering GTK event loop");

    // 设置菜单事件处理（在 gtk::main 之前）
    // 将裸指针转为 usize（Send+Sync）以通过闭包的 trait 约束
    let user_data = callbacks.user_data as usize;
    let on_menu_click = callbacks.on_menu_click;
    tray_icon::menu::MenuEvent::set_event_handler(Some(move |event: tray_icon::menu::MenuEvent| {
        let id = event.id().as_ref();
        let c_id = std::ffi::CString::new(id).unwrap_or_default();
        on_menu_click(user_data as *mut std::os::raw::c_void, c_id.as_ptr());
    }));

    // GTK 事件循环（阻塞）
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        gtk::main();
    })).unwrap_or_else(|e| {
        eprintln!("[plugin-ui:tray] GTK event loop exited: {:?}", e);
    });

    TRAY_RUNNING.store(false, Ordering::SeqCst);
}

fn build_icon(config: &TrayConfigJson) -> Option<tray_icon::Icon> {
    if let Some(rgba) = &config.icon_rgba {
        let w = config.icon_width.unwrap_or(32);
        let h = config.icon_height.unwrap_or(32);
        return tray_icon::Icon::from_rgba(rgba.clone(), w, h).ok();
    }
    if let Some(path) = &config.icon_path {
        let data = std::fs::read(path).ok()?;
        let img = image::load_from_memory(&data).ok()?;
        let rgba = img.to_rgba8();
        let (w, h) = rgba.dimensions();
        return tray_icon::Icon::from_rgba(rgba.into_raw(), w, h).ok();
    }
    None
}

/// 更新菜单项启用/禁用状态。
#[no_mangle]
pub extern "C" fn plugin_tray_set_menu_enabled(
    _id: *const c_char,
    _enabled: i32,
) {
}

/// 销毁托盘指示器。
#[no_mangle]
pub extern "C" fn plugin_tray_destroy() {
    if TRAY_RUNNING.load(Ordering::SeqCst) {
        gtk::main_quit();
    }
}
