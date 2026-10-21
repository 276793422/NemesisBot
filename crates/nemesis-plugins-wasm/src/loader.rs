//! 宿主 DLL 装载器（libloading；load-once-never-unload）。
//!
//! 搜索顺序：`NEMESIS_WASM_HOST_DLL` 环境变量 → exe 同目录 → exe 旁
//! `plugins/`（部署布局）→ 开发路径（`<repo>/plugins/plugin-wasm/target
//! /release/`）。装载失败 =
//! `PluginError::NotAvailable`（`PluginManager::new` 上抛 → gateway 侧
//! `build_plugin_stack` 警告并整体关闭 wasm 子系统——诚实降级，不崩）。
//!
//! 卸载纪律：进程内**永不** `Library::unload`（函数指针存活期 = 进程；
//! 卸载后悬垂指针 = UB，与 plugin-ui 的运行时加载同纪律）。

use std::os::raw::{c_char, c_int, c_void};
use std::sync::{Arc, OnceLock};

use crate::abi::{self, ENGINE_HANDLE, HostCallbacks, status};
use crate::error::PluginError;

/// 壳侧导出的函数指针集（符号在 open 期一次性解析）。
pub(crate) struct HostDll {
    #[allow(dead_code)] // 泄漏保活；字段仅防 Library 被 drop
    lib: &'static libloading::Library,
    pub(crate) abi_version: unsafe extern "C" fn() -> u32,
    pub(crate) engine_init:
        unsafe extern "C" fn(cache_root: *const c_char, cb: *const HostCallbacks) -> u64,
    pub(crate) component_new: unsafe extern "C" fn(u64, *const u8, usize) -> u64,
    pub(crate) component_drop: unsafe extern "C" fn(u64),
    pub(crate) store_new:
        unsafe extern "C" fn(u64, u64, *const c_char, u64, u64, u64, u64, *mut c_void) -> u64,
    pub(crate) call_export:
        unsafe extern "C" fn(u64, *const c_char, *const c_char, *mut *mut c_char) -> c_int,
    pub(crate) store_drop: unsafe extern "C" fn(u64),
    pub(crate) last_error: unsafe extern "C" fn() -> *mut c_char,
    pub(crate) free_string: unsafe extern "C" fn(*mut c_char),
}

/// 平台壳库名。
fn dll_file_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "plugin_wasm.dll"
    } else if cfg!(target_os = "macos") {
        "libplugin_wasm.dylib"
    } else {
        "libplugin_wasm.so"
    }
}

/// 候选路径序列（env → exe 同目录 → exe 旁 `plugins/` → 开发路径）。
fn candidate_paths() -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(p) = std::env::var("NEMESIS_WASM_HOST_DLL")
        && !p.trim().is_empty()
    {
        out.push(std::path::PathBuf::from(p));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        out.push(dir.join(dll_file_name()));
        // 部署布局：build 脚本把壳产物放 exe 旁 plugins/（与 plugin-ui/
        // eval-monitor 同目录纪律）。
        out.push(dir.join("plugins").join(dll_file_name()));
        // 开发形态：tests/gateway 从 target/{debug,release} 跑，壳产物在
        // plugins/plugin-wasm/target/release/（独立 CARGO_TARGET_DIR）。
        out.push(
            dir.join("../../plugins/plugin-wasm/target/release")
                .join(dll_file_name()),
        );
    }
    out
}

/// 进程级装载缓存（首个成功路径定终身；失败也缓存——重试无意义，失败
/// 原因写进错误文本）。
static HOST: OnceLock<Result<Arc<HostDll>, String>> = OnceLock::new();

impl HostDll {
    /// 装载（进程单例；重复调用返回缓存结论）。
    pub(crate) fn open() -> Result<Arc<HostDll>, PluginError> {
        match HOST.get_or_init(|| unsafe { open_impl() }) {
            Ok(h) => Ok(h.clone()),
            Err(e) => Err(PluginError::NotAvailable(e.clone())),
        }
    }
}

/// # Safety
/// 装载任意动态库即执行其初始化代码；路径来自受控搜索序列。
unsafe fn open_impl() -> Result<Arc<HostDll>, String> {
    unsafe {
        let mut tried = Vec::new();
        for path in candidate_paths() {
            if !path.is_file() {
                tried.push(format!("{}（不存在）", path.display()));
                continue;
            }
            let lib = match libloading::Library::new(&path) {
                Ok(l) => l,
                Err(e) => {
                    tried.push(format!("{}（{e}）", path.display()));
                    continue;
                }
            };
            let dll = bind_symbols(lib)?;
            // ABI 握手（代际不匹配 = 拒绝装配，与 manifest api-version 同纪律）。
            let v = (dll.abi_version)();
            if v != abi::ABI_VERSION {
                return Err(format!(
                    "host dll abi version mismatch: dll {v}, host {} ({})",
                    abi::ABI_VERSION,
                    path.display()
                ));
            }
            tracing::info!(path = %path.display(), "[WasmPlugin] 宿主壳已装载");
            return Ok(Arc::new(dll));
        }
        Err(format!(
            "wasm host dll not found（candidates: {}）",
            tried.join("; ")
        ))
    }
}

/// # Safety
/// lib 必须是有效装载的壳库。
unsafe fn bind_symbols(lib: libloading::Library) -> Result<HostDll, String> {
    // 符号地址直取（*mut c_void）；transmute 只在符号存在性验证后做。
    let ptr = |name: &[u8]| -> Result<*mut c_void, String> {
        unsafe { lib.get::<*mut c_void>(name) }
            .map(|s| *s)
            .map_err(|e| {
                format!(
                    "symbol {}: {e}",
                    String::from_utf8_lossy(&name[..name.len() - 1])
                )
            })
    };
    macro_rules! fptr {
        ($name:expr) => {{
            let p = ptr($name)?;
            assert!(!p.is_null(), "symbol {} null", stringify!($name));
            p
        }};
    }
    let dll = unsafe {
        HostDll {
            abi_version: std::mem::transmute::<*mut c_void, unsafe extern "C" fn() -> u32>(fptr!(
                b"nvwa_abi_version\0"
            )),
            engine_init: std::mem::transmute::<
                *mut c_void,
                unsafe extern "C" fn(*const c_char, *const HostCallbacks) -> u64,
            >(fptr!(b"nvwa_engine_init\0")),
            component_new: std::mem::transmute::<
                *mut c_void,
                unsafe extern "C" fn(u64, *const u8, usize) -> u64,
            >(fptr!(b"nvwa_component_new\0")),
            component_drop: std::mem::transmute::<*mut c_void, unsafe extern "C" fn(u64)>(fptr!(
                b"nvwa_component_drop\0"
            )),
            store_new: std::mem::transmute::<
                *mut c_void,
                unsafe extern "C" fn(
                    u64,
                    u64,
                    *const c_char,
                    u64,
                    u64,
                    u64,
                    u64,
                    *mut c_void,
                ) -> u64,
            >(fptr!(b"nvwa_store_new\0")),
            call_export: std::mem::transmute::<
                *mut c_void,
                unsafe extern "C" fn(u64, *const c_char, *const c_char, *mut *mut c_char) -> c_int,
            >(fptr!(b"nvwa_call_export\0")),
            store_drop: std::mem::transmute::<*mut c_void, unsafe extern "C" fn(u64)>(fptr!(
                b"nvwa_store_drop\0"
            )),
            last_error: std::mem::transmute::<*mut c_void, unsafe extern "C" fn() -> *mut c_char>(
                fptr!(b"nvwa_last_error\0"),
            ),
            free_string: std::mem::transmute::<*mut c_void, unsafe extern "C" fn(*mut c_char)>(
                fptr!(b"nvwa_free_string\0"),
            ),
            lib: Box::leak(Box::new(lib)),
        }
    };
    Ok(dll)
}

impl HostDll {
    /// 引擎初始化（幂等；回调表 = [`crate::callbacks::callbacks_table`]）。
    pub(crate) fn engine_init(
        &self,
        cache_root: Option<&std::path::Path>,
    ) -> Result<(), PluginError> {
        let cache_c = cache_root.map(|p| {
            std::ffi::CString::new(p.to_string_lossy().as_bytes())
                .unwrap_or_else(|_| std::ffi::CString::new(String::new()).unwrap())
        });
        let cb = crate::callbacks::callbacks_table();
        let h = unsafe {
            (self.engine_init)(
                cache_c.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()),
                &cb,
            )
        };
        if h != ENGINE_HANDLE {
            return Err(PluginError::NotAvailable(format!(
                "wasm host engine init failed: {}",
                self.take_last_error()
            )));
        }
        Ok(())
    }

    /// 编译组件 → 句柄（0 = 失败）。
    pub(crate) fn component_new(&self, wasm: &[u8]) -> Result<u64, PluginError> {
        let h = unsafe { (self.component_new)(ENGINE_HANDLE, wasm.as_ptr(), wasm.len()) };
        if h == 0 {
            return Err(PluginError::Compile(self.take_last_error()));
        }
        Ok(h)
    }

    /// 释放组件句柄（0 / 重复 = 容忍的 no-op）。
    pub(crate) fn component_drop(&self, component: u64) {
        if component != 0 {
            unsafe { (self.component_drop)(component) };
        }
    }

    /// fresh store（`env` = Box<CallCaps> 裸指针，store 存活期内有效）。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn store_new(
        &self,
        component: u64,
        data_dir: &std::path::Path,
        memory_bytes: usize,
        table_elements: usize,
        fuel: u64,
        timeout_ms: u64,
        env: *mut c_void,
    ) -> Result<u64, PluginError> {
        let dir_c = std::ffi::CString::new(data_dir.to_string_lossy().as_bytes())
            .unwrap_or_else(|_| std::ffi::CString::new(String::new()).unwrap());
        let h = unsafe {
            (self.store_new)(
                ENGINE_HANDLE,
                component,
                dir_c.as_ptr(),
                memory_bytes as u64,
                table_elements as u64,
                fuel,
                timeout_ms,
                env,
            )
        };
        if h == 0 {
            return Err(PluginError::Host(format!(
                "store init: {}",
                self.take_last_error()
            )));
        }
        Ok(h)
    }

    /// 单次导出调用。Ok(json) = status OK；Err(PluginError) 按状态码归类。
    pub(crate) fn call_export(
        &self,
        store: u64,
        name: &str,
        args_json: &str,
        timeout_ms: u64,
    ) -> Result<String, PluginError> {
        let name_c = std::ffi::CString::new(name).unwrap_or_default();
        let args_c = std::ffi::CString::new(args_json.as_bytes().to_vec()).unwrap_or_default();
        let mut out: *mut c_char = std::ptr::null_mut();
        let code = unsafe { (self.call_export)(store, name_c.as_ptr(), args_c.as_ptr(), &mut out) };
        let payload = unsafe { take_out_string(&self.free_string, out) };
        match code {
            status::OK => Ok(payload.unwrap_or_else(|| "null".to_string())),
            status::TIMEOUT => Err(PluginError::Timeout { ms: timeout_ms }),
            status::TRAP => Err(PluginError::Trap(payload.unwrap_or_default())),
            status::COMPILE => Err(PluginError::Compile(payload.unwrap_or_default())),
            status::INSTANTIATE => Err(PluginError::Compile(format!(
                "instantiate: {}",
                crate::host_impl::truncate_err(&payload.unwrap_or_default(), 500)
            ))),
            status::BAD_HANDLE => Err(PluginError::Trap(format!(
                "invalid store handle: {}",
                payload.unwrap_or_default()
            ))),
            status::CALLBACK_ERROR => Err(PluginError::Host(format!(
                "callback channel failure: {}",
                payload.unwrap_or_default()
            ))),
            status::BAD_ARGS => Err(PluginError::Host(format!(
                "bad call args: {}",
                payload.unwrap_or_default()
            ))),
            _ => Err(PluginError::Trap(format!(
                "shell internal: {}",
                payload.unwrap_or_default()
            ))),
        }
    }

    /// 释放 store（已被 call_export 消耗时 = 容忍的 no-op）。
    pub(crate) fn store_drop(&self, store: u64) {
        if store != 0 {
            unsafe { (self.store_drop)(store) };
        }
    }

    /// 取走线程局部错误槽（消费后壳侧自清由下次调用覆盖语义承担）。
    pub(crate) fn take_last_error(&self) -> String {
        let p = unsafe { (self.last_error)() };
        unsafe { take_out_string(&self.free_string, p) }.unwrap_or_default()
    }
}

/// 消费壳返回的字符串（String 化 + free_string 归还壳）。
///
/// # Safety
/// p 必须来自壳导出（call_export out / last_error）；`free` 必须是壳的
/// `nvwa_free_string`。
unsafe fn take_out_string(
    free: &unsafe extern "C" fn(*mut c_char),
    p: *mut c_char,
) -> Option<String> {
    unsafe {
        if p.is_null() {
            return None;
        }
        let s = std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned();
        free(p);
        Some(s)
    }
}
