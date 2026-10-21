//! C-ABI 合同镜像（权威源在 `plugins/plugin-wasm` 壳；本文件按同布局镜像
//! 声明，`engine_init` 握手时壳侧校验 `struct_size`——布局漂移诚实失败）。
//!
//! 跨界纪律：字节串（UTF-8 + `\0` 结尾）+ 不透明句柄（u64）+ 数值 + host-error
//! 五值 tag，零 Rust 类型跨界。所有权纪律：回调返回的字符串一律由宿主分配、
//! 壳消费后经 `free_str` 归还宿主释放；壳返回给宿主的字符串由宿主经
//! `nvwa_free_string` 归还壳。

use std::os::raw::{c_char, c_int, c_void};

/// ABI 代际（`nvwa_abi_version` 握手；不匹配 = 拒绝装配）。
pub const ABI_VERSION: u32 = 1;

/// 调用状态码（`nvwa_call_export` 返回；壳内错误归类）。
pub mod status {
    use std::os::raw::c_int;
    pub const OK: c_int = 0;
    /// 墙钟/fuel 预算耗尽（epoch 或 fuel 归并一类）。
    pub const TIMEOUT: c_int = 1;
    /// guest trap（非预算类）。
    pub const TRAP: c_int = 2;
    /// 组件编译失败。
    pub const COMPILE: c_int = 3;
    /// 实例化失败（导入失配等）。
    pub const INSTANTIATE: c_int = 4;
    /// 句柄无效或已失效。
    pub const BAD_HANDLE: c_int = 5;
    /// 宿主回调通道故障。
    pub const CALLBACK_ERROR: c_int = 6;
    /// 壳内部错误（宿主侧归并入 catch-all 臂，不单独映射；镜像完备性保留）。
    #[allow(dead_code)]
    pub const INTERNAL: c_int = 7;
    /// 入参非法（未知导出名 / JSON 解析失败）。
    pub const BAD_ARGS: c_int = 8;
}

/// 宿主回调的 host-error 五值 tag（对齐 WIT `host-error` 变体词表）。
pub mod host_err {
    use std::os::raw::c_int;
    pub const OK: c_int = 0;
    pub const NO_PERMISSION: c_int = 1;
    pub const POLICY_DENIED: c_int = 2;
    pub const NOT_FOUND: c_int = 3;
    pub const BUDGET_EXCEEDED: c_int = 4;
    pub const UNAVAILABLE: c_int = 5;
}

/// 引擎句柄（进程单例；`nvwa_engine_init` 成功恒返回此值）。
pub const ENGINE_HANDLE: u64 = 1;

/// 宿主回调表镜像（`#[repr(C)]`，布局以壳为权威；末位 `struct_size`
/// 哨兵字段由壳在握手期校验）。字段语义详见壳侧 `plugins/plugin-wasm/src
/// /abi.rs`——两处必须逐字段同步修改。
#[repr(C)]
pub struct HostCallbacks {
    /// 释放宿主分配的字符串（`CString::into_raw` 的归还端）。
    pub free_str: unsafe extern "C" fn(s: *mut c_char),
    /// `log(level, message)`；level：0=trace 1=debug 2=info 3=warn 4=error。
    pub log: unsafe extern "C" fn(env: *mut c_void, level: c_int, message: *const c_char),
    /// `now-millis() -> u64`；预算耗尽宿主自行落 0。
    pub now_millis: unsafe extern "C" fn(env: *mut c_void, out_ms: *mut u64) -> c_int,
    /// `config-get(key) -> result<option<string>, host-error>`。
    pub config_get: unsafe extern "C" fn(
        env: *mut c_void,
        key: *const c_char,
        out_found: *mut c_int,
        out_value: *mut *mut c_char,
        out_detail: *mut *mut c_char,
    ) -> c_int,
    /// `secret-get(name) -> result<string, host-error>`。
    pub secret_get: unsafe extern "C" fn(
        env: *mut c_void,
        name: *const c_char,
        out_value: *mut *mut c_char,
        out_detail: *mut *mut c_char,
    ) -> c_int,
    /// `workspace-read(rel-path) -> result<list<u8>, host-error>`（base64）。
    pub workspace_read: unsafe extern "C" fn(
        env: *mut c_void,
        rel_path: *const c_char,
        out_b64: *mut *mut c_char,
        out_detail: *mut *mut c_char,
    ) -> c_int,
    /// `data-dir-path() -> result<string, host-error>`（恒 "/data"）。
    pub data_dir_path: unsafe extern "C" fn(
        env: *mut c_void,
        out_path: *mut *mut c_char,
        out_detail: *mut *mut c_char,
    ) -> c_int,
    /// `http-send(request) -> result<http-response, host-error>`；
    /// JSON 形态：`{"method","url","headers":[[k,v],…],"body":b64|null}`。
    pub http_send: unsafe extern "C" fn(
        env: *mut c_void,
        request_json: *const c_char,
        out_response_json: *mut *mut c_char,
        out_detail: *mut *mut c_char,
    ) -> c_int,
    /// `tool-invoke(tool, args-json) -> result<json-string, host-error>`。
    pub tool_invoke: unsafe extern "C" fn(
        env: *mut c_void,
        tool: *const c_char,
        args_json: *const c_char,
        out_content: *mut *mut c_char,
        out_detail: *mut *mut c_char,
    ) -> c_int,
    /// 布局哨兵：`std::mem::size_of::<HostCallbacks>()`，壳握手校验。
    pub struct_size: usize,
}
