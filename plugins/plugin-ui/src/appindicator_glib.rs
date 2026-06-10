//! FFI bindings to libayatana-appindicator3 (GTK3-based).
//!
//! Uses GtkMenu for the tray menu — universally supported by desktop panels.

use std::os::raw::c_void;

#[repr(C)]
pub struct AppIndicator {
    _private: [u8; 0],
}

// AppIndicatorCategory
pub const APP_INDICATOR_CATEGORY_APPLICATION_STATUS: i32 = 0;

// AppIndicatorStatus
pub const APP_INDICATOR_STATUS_ACTIVE: i32 = 1;

extern "C" {
    pub fn app_indicator_new_with_path(
        id: *const std::os::raw::c_char,
        icon_name: *const std::os::raw::c_char,
        category: i32,
        icon_theme_path: *const std::os::raw::c_char,
    ) -> *mut AppIndicator;

    pub fn app_indicator_set_status(self_: *mut AppIndicator, status: i32);
    pub fn app_indicator_set_menu(self_: *mut AppIndicator, menu: *mut c_void);
    pub fn app_indicator_set_title(self_: *mut AppIndicator, title: *const std::os::raw::c_char);
}
