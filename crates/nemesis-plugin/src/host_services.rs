//! Host services vtable — host 导出给 plugin 的通用能力接口。
//!
//! 这是 vtable 的**唯一权威定义**。Standalone plugin 通过复制此文件保持同步
//! （与 C header 模式相同）。
//!
//! 所有函数指针用 `Option` 包裹：
//! - `Some(fn)` = host 实现了此功能
//! - `None` = host 不支持（plugin 不应调用）
//!
//! 向前兼容：新函数追加在结构体末尾，旧字段位置不变。
//! Plugin 通过检查 `Option` 是否为 `None` 判断功能可用性。

use std::ffi::CStr;
use std::os::raw::c_char;
use std::path::Path;
use std::sync::OnceLock;

/// Host services vtable — host 导出给 plugin 的通用能力接口。
#[repr(C)]
pub struct HostServices {
    // ---- 版本 ----
    /// vtable 结构版本号，每新增一批函数 +1。
    /// Plugin 可检查此值判断 host 的新功能支持程度。
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

// ---------------------------------------------------------------------------
// Host-side implementation
// ---------------------------------------------------------------------------

/// Current vtable version. Increment when adding new functions.
pub const HOST_SERVICES_VERSION: u32 = 1;

// Static storage for path pointers (process-lifetime, set once)
// Wrap raw pointer in a Send+Sync newtype for static storage.
struct StaticPtr(*mut c_char);
unsafe impl Send for StaticPtr {}
unsafe impl Sync for StaticPtr {}

static WORKSPACE_DIR_PTR: OnceLock<StaticPtr> = OnceLock::new();
static CONFIG_DIR_PTR: OnceLock<StaticPtr> = OnceLock::new();

/// Build a `HostServices` vtable for the given workspace directory.
///
/// Stores the workspace and config paths in process-lifetime statics
/// and returns function pointers that read from them.
pub fn build_host_services(workspace_dir: &Path) -> HostServices {
    // Store workspace dir (leaked CString — valid for process lifetime)
    let ws_cstring = std::ffi::CString::new(
        workspace_dir.to_str().expect("workspace path is valid UTF-8")
    ).expect("no null bytes in workspace path");
    let _ = WORKSPACE_DIR_PTR.set(StaticPtr(ws_cstring.into_raw()));

    // Store config dir: workspace/config/plugins/
    let config_dir = workspace_dir.join("config").join("plugins");
    let config_cstring = std::ffi::CString::new(
        config_dir.to_str().expect("config path is valid UTF-8")
    ).expect("no null bytes in config path");
    let _ = CONFIG_DIR_PTR.set(StaticPtr(config_cstring.into_raw()));

    HostServices {
        version: HOST_SERVICES_VERSION,
        log: Some(host_log),
        get_workspace_dir: Some(host_get_workspace_dir),
        get_plugin_data_dir: Some(host_get_plugin_data_dir),
        get_plugin_config_dir: Some(host_get_plugin_config_dir),
        file_exists: Some(host_file_exists),
        file_size: Some(host_file_size),
        download_file: Some(host_download_file),
        free_string: Some(host_free_string),
    }
}

// ---- Log implementation ----

extern "C" fn host_log(level: i32, tag: *const c_char, msg: *const c_char) {
    if tag.is_null() || msg.is_null() {
        return;
    }
    let tag = unsafe { CStr::from_ptr(tag) }.to_string_lossy();
    let msg = unsafe { CStr::from_ptr(msg) }.to_string_lossy();
    match level {
        0 => tracing::trace!(tag = %tag, "{}", msg),
        1 => tracing::debug!(tag = %tag, "{}", msg),
        2 => tracing::info!(tag = %tag, "{}", msg),
        3 => tracing::warn!(tag = %tag, "{}", msg),
        _ => tracing::error!(tag = %tag, "{}", msg),
    }
}

// ---- Path implementations ----

extern "C" fn host_get_workspace_dir(buf: *mut c_char, buf_len: usize) -> i32 {
    let ptr = WORKSPACE_DIR_PTR.get().map(|s| s.0).unwrap_or(std::ptr::null_mut());
    write_cstr_to_buf(ptr, buf, buf_len)
}

extern "C" fn host_get_plugin_config_dir(buf: *mut c_char, buf_len: usize) -> i32 {
    let ptr = CONFIG_DIR_PTR.get().map(|s| s.0).unwrap_or(std::ptr::null_mut());
    write_cstr_to_buf(ptr, buf, buf_len)
}

extern "C" fn host_get_plugin_data_dir(
    plugin_name: *const c_char,
    buf: *mut c_char,
    buf_len: usize,
) -> i32 {
    if plugin_name.is_null() || buf.is_null() {
        return -1;
    }
    let plugin = unsafe { CStr::from_ptr(plugin_name) }.to_string_lossy();
    let ws_ptr = WORKSPACE_DIR_PTR.get().map(|s| s.0).unwrap_or(std::ptr::null_mut());
    let ws = if ws_ptr.is_null() {
        return -2;
    } else {
        unsafe { CStr::from_ptr(ws_ptr) }.to_string_lossy().to_string()
    };

    let data_dir = Path::new(&*ws).join("plugins").join(&*plugin);
    let _ = std::fs::create_dir_all(&data_dir);

    let cstr = match std::ffi::CString::new(data_dir.to_str().unwrap_or("")) {
        Ok(c) => c,
        Err(_) => return -3,
    };
    write_cstr_to_buf(cstr.as_ptr(), buf, buf_len)
}

// ---- File operations ----

extern "C" fn host_file_exists(path: *const c_char) -> i32 {
    if path.is_null() {
        return -1;
    }
    let p = unsafe { CStr::from_ptr(path) }.to_string_lossy();
    if Path::new(&*p).exists() { 1 } else { 0 }
}

extern "C" fn host_file_size(path: *const c_char) -> i64 {
    if path.is_null() {
        return -1;
    }
    let p = unsafe { CStr::from_ptr(path) }.to_string_lossy();
    match std::fs::metadata(&*p) {
        Ok(meta) => meta.len() as i64,
        Err(_) => -1,
    }
}

extern "C" fn host_download_file(url: *const c_char, dest_path: *const c_char) -> i32 {
    if url.is_null() || dest_path.is_null() {
        return -1;
    }
    let url_str = unsafe { CStr::from_ptr(url) }.to_string_lossy();
    let dest_str = unsafe { CStr::from_ptr(dest_path) }.to_string_lossy();

    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(_) => return -2,
    };

    let pool = nemesis_http_pool::pool::shared_pool();
    match rt.block_on(pool.download_file(&url_str, &dest_str)) {
        Ok(()) => 0,
        Err(_) => -3,
    }
}

extern "C" fn host_free_string(ptr: *mut c_char) {
    if !ptr.is_null() {
        unsafe { let _ = std::ffi::CString::from_raw(ptr); }
    }
}

// ---- Helpers ----

/// Write a C string (src) into a caller-provided buffer (buf of buf_len bytes).
/// Returns the number of bytes written (excluding null terminator), or negative on error.
fn write_cstr_to_buf(src: *const c_char, buf: *mut c_char, buf_len: usize) -> i32 {
    if buf.is_null() || buf_len == 0 {
        return -1;
    }
    let src_len = unsafe { libc_strlen(src) };
    if src_len + 1 > buf_len {
        return -(src_len as i32 + 1);
    }
    unsafe {
        std::ptr::copy_nonoverlapping(src as *const u8, buf as *mut u8, src_len + 1);
    }
    src_len as i32
}

/// Calculate the length of a C string (like libc strlen).
unsafe fn libc_strlen(ptr: *const c_char) -> usize {
    if ptr.is_null() {
        return 0;
    }
    let mut len = 0usize;
    unsafe {
        while *ptr.add(len) != 0 {
            len += 1;
        }
    }
    len
}
