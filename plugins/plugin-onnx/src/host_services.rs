//! Host services vtable — standalone copy for plugin use.
//!
//! This is a copy of `crates/nemesis-plugin/src/host_services.rs` (struct definition only).
//! Plugins include this file to access the HostServices vtable.
//! Keep in sync with the authoritative definition in nemesis-plugin.

use std::os::raw::c_char;

/// Host services vtable — host 导出给 plugin 的通用能力接口。
#[repr(C)]
pub struct HostServices {
    // ---- 版本 ----
    /// vtable 结构版本号，每新增一批函数 +1。
    pub version: u32,

    // ---- 日志 ----
    /// 通过 host 的 tracing 系统输出日志。
    /// level: 0=trace, 1=debug, 2=info, 3=warn, 4=error
    /// tag: 组件名 (如 "plugin-onnx")
    /// msg: 日志内容
    pub log: Option<extern "C" fn(level: i32, tag: *const c_char, msg: *const c_char)>,

    // ---- 路径解析 ----
    /// 获取 bot 的 workspace 根目录路径，写入 buf。
    /// 返回写入字节数（不含 \\0），负数为错误码。
    pub get_workspace_dir: Option<extern "C" fn(buf: *mut c_char, buf_len: usize) -> i32>,

    /// 获取 plugin 专属数据目录路径 (如 workspace/plugins/plugin-onnx/)。
    /// host 保证目录存在（自动创建）。
    /// 返回写入字节数（不含 \\0），负数为错误码。
    pub get_plugin_data_dir: Option<extern "C" fn(
        plugin_name: *const c_char,
        buf: *mut c_char,
        buf_len: usize,
    ) -> i32>,

    /// 获取 plugin 专属配置目录路径 (如 workspace/config/plugins/)。
    /// 返回写入字节数（不含 \\0），负数为错误码。
    pub get_plugin_config_dir: Option<extern "C" fn(buf: *mut c_char, buf_len: usize) -> i32>,

    // ---- 文件操作 ----
    /// 检查文件是否存在。1=存在, 0=不存在, 负数=错误。
    pub file_exists: Option<extern "C" fn(path: *const c_char) -> i32>,

    /// 获取文件大小（字节）。-1=文件不存在或错误。
    pub file_size: Option<extern "C" fn(path: *const c_char) -> i64>,

    /// 同步下载文件。host 负责重试、重定向、代理。
    /// 返回 0=成功, 负数=错误。
    pub download_file: Option<extern "C" fn(
        url: *const c_char,
        dest_path: *const c_char,
    ) -> i32>,

    // ---- 内存管理 ----
    /// 释放 host 分配的字符串内存。
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
