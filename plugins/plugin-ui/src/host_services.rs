//! Host services vtable — standalone copy for plugin-ui use.
//!
//! This is a copy of `crates/nemesis-plugin/src/host_services.rs` (struct definition only).
//! Plugins include this file to access the HostServices vtable.
//! Keep in sync with the authoritative definition in nemesis-plugin.

use std::os::raw::c_char;

/// Host services vtable — host 导出给 plugin 的通用能力接口。
#[repr(C)]
pub struct HostServices {
    // ---- 版本 ----
    pub version: u32,

    // ---- 日志 ----
    pub log: Option<extern "C" fn(level: i32, tag: *const c_char, msg: *const c_char)>,

    // ---- 路径解析 ----
    pub get_workspace_dir: Option<extern "C" fn(buf: *mut c_char, buf_len: usize) -> i32>,
    pub get_plugin_data_dir: Option<extern "C" fn(
        plugin_name: *const c_char,
        buf: *mut c_char,
        buf_len: usize,
    ) -> i32>,
    pub get_plugin_config_dir: Option<extern "C" fn(buf: *mut c_char, buf_len: usize) -> i32>,

    // ---- 文件操作 ----
    pub file_exists: Option<extern "C" fn(path: *const c_char) -> i32>,
    pub file_size: Option<extern "C" fn(path: *const c_char) -> i64>,
    pub download_file: Option<extern "C" fn(
        url: *const c_char,
        dest_path: *const c_char,
    ) -> i32>,

    // ---- 内存管理 ----
    pub free_string: Option<extern "C" fn(ptr: *mut c_char)>,
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
