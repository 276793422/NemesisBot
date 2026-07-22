//! Host services vtable — standalone copy for plugin-ui use.
//!
//! This is a copy of `crates/nemesis-plugin/src/host_services.rs` (struct definition only).
//! Plugins include this file to access the HostServices vtable.
//! Keep in sync with the authoritative definition in nemesis-plugin.

use std::os::raw::c_char;
#[cfg(target_os = "linux")]
use std::os::raw::c_void;

/// Host services vtable — host 导出给 plugin 的通用能力接口。
#[repr(C)]
pub struct HostServices {
    // ---- 版本 ----
    pub version: u32,

    // ---- 日志 ----
    pub log: Option<extern "C" fn(level: i32, tag: *const c_char, msg: *const c_char)>,

    // ---- 路径解析 ----
    pub get_workspace_dir: Option<extern "C" fn(buf: *mut c_char, buf_len: usize) -> i32>,
    pub get_plugin_data_dir:
        Option<extern "C" fn(plugin_name: *const c_char, buf: *mut c_char, buf_len: usize) -> i32>,
    pub get_plugin_config_dir: Option<extern "C" fn(buf: *mut c_char, buf_len: usize) -> i32>,

    // ---- 文件操作 ----
    pub file_exists: Option<extern "C" fn(path: *const c_char) -> i32>,
    pub file_size: Option<extern "C" fn(path: *const c_char) -> i64>,
    pub download_file: Option<extern "C" fn(url: *const c_char, dest_path: *const c_char) -> i32>,

    // ---- 内存管理 ----
    pub free_string: Option<extern "C" fn(ptr: *mut c_char)>,

    // ---- 图像解码 ----
    /// 解码 PNG 数据为 RGBA 像素。
    /// png_data/png_len: 输入 PNG 字节
    /// out_rgba: 输出缓冲区（调用者分配），写入 RGBA 数据
    /// out_rgba_len: 输出缓冲区大小（字节）
    /// out_width/out_height: 写入图像尺寸
    /// 返回: 0=成功, 负数=错误（-1=参数无效, -2=解码失败, -3=缓冲区不足）
    pub decode_png: Option<
        extern "C" fn(
            png_data: *const u8,
            png_len: usize,
            out_rgba: *mut u8,
            out_rgba_len: usize,
            out_width: *mut u32,
            out_height: *mut u32,
        ) -> i32,
    >,
}

/// Helper: call host log if available.
pub fn host_log(host: Option<&HostServices>, level: i32, tag: &str, msg: &str) {
    if let Some(h) = host {
        if let Some(log_fn) = h.log {
            let c_tag = std::ffi::CString::new(tag).unwrap_or_default();
            let c_msg = std::ffi::CString::new(msg).unwrap_or_default();
            log_fn(level, c_tag.as_ptr(), c_msg.as_ptr());
        }
    }
}

// ---------------------------------------------------------------------------
// Tray callbacks — plugin-ui → nemesis-desktop 的事件通知
// ---------------------------------------------------------------------------

/// Tray 菜单点击回调表（plugin-ui 副本，与 nemesis-plugin 权威定义保持同步）。
#[cfg(target_os = "linux")]
#[repr(C)]
pub struct TrayCallbacks {
    pub user_data: *mut c_void,
    pub on_menu_click: extern "C" fn(user_data: *mut c_void, menu_id: *const c_char),
}

#[cfg(target_os = "linux")]
unsafe impl Send for TrayCallbacks {}
#[cfg(target_os = "linux")]
unsafe impl Sync for TrayCallbacks {}
#[cfg(target_os = "linux")]
impl Copy for TrayCallbacks {}
#[cfg(target_os = "linux")]
impl Clone for TrayCallbacks {
    fn clone(&self) -> Self {
        *self
    }
}
