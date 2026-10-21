//! NemesisBot WASM 宿主运行时插件（plugin-wasm）。
//!
//! 纯 wasmtime 通用装配壳：引擎/编译缓存/epoch ticker、组件编译与实例化、
//! fuel/内存/墙钟预算、wasi p2 数据目录预开放、平台合同（nemesis:plugin
//! @0.1.0）宿主面声明与导出调用。**零策略语义**——全部宿主能力（egress/
//! vault/审计/帧纪律）经 [`abi::HostCallbacks`] 分发回宿主 exe 执行。
//!
//! 导出面（对象句柄 ABI，句柄 = 不透明 u64；错误槽线程局部）：
//!
//! | 导出 | 职责 |
//! |---|---|
//! | `nvwa_abi_version` | ABI 握手 |
//! | `nvwa_engine_init` | 引擎单例 + 编译缓存 + epoch ticker + 回调表注册 |
//! | `nvwa_component_new` | 编译（world 识别 + 导出索引解析） |
//! | `nvwa_component_drop` | 回收编译产物 |
//! | `nvwa_store_new` | fresh store（fuel/内存/表/墙钟/env 透传） |
//! | `nvwa_call_export` | 单次导出调用（消耗 store；get-metadata/execute/observe） |
//! | `nvwa_store_drop` | 释放 store（call_export 已消耗时为容忍的 no-op） |
//! | `nvwa_last_error` / `nvwa_free_string` | 线程局部错误槽 + 字符串所有权归还 |

mod abi;
mod call;
mod store;

use std::collections::HashMap;
use std::os::raw::{c_char, c_int, c_void};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Mutex, OnceLock};

use wasmtime::Store;

use abi::{callbacks, status, HostCallbacks, ENGINE_HANDLE};
use call::CompiledComponent;
use store::{EnvPtr, ShellCtx, StoreSpec};

/// 句柄表条目：store + 其组件引用（call_export 需要两者）。
struct StoreEntry {
    store: Store<ShellCtx>,
    comp: std::sync::Arc<CompiledComponent>,
}

#[derive(Default)]
struct Handles {
    next: u64,
    components: HashMap<u64, std::sync::Arc<CompiledComponent>>,
    stores: HashMap<u64, StoreEntry>,
}

fn handles() -> &'static Mutex<Handles> {
    static HANDLES: OnceLock<Mutex<Handles>> = OnceLock::new();
    HANDLES.get_or_init(|| Mutex::new(Handles::default()))
}

thread_local! {
    static LAST_ERROR: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

fn set_err(msg: impl std::fmt::Display) {
    LAST_ERROR.with(|e| *e.borrow_mut() = msg.to_string());
}

/// FFI 边界护栏：catch_unwind + 错误槽（导出统一形态）。
fn guard<T>(f: impl FnOnce() -> Result<T, (c_int, String)>) -> Result<T, (c_int, String)> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(p) => {
            let msg = if let Some(s) = p.downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = p.downcast_ref::<String>() {
                s.clone()
            } else {
                "opaque panic".into()
            };
            let m = format!("shell panic: {msg}");
            set_err(&m);
            Err((status::INTERNAL, m))
        }
    }
}

#[no_mangle]
pub extern "C" fn nvwa_abi_version() -> u32 {
    abi::ABI_VERSION
}

/// # Safety
/// `cache_root` 可空；`cb` 指向有效 [`HostCallbacks`]（进程存活期）。
#[no_mangle]
pub unsafe extern "C" fn nvwa_engine_init(
    cache_root: *const c_char,
    cb: *const HostCallbacks,
) -> u64 {
    match guard(|| {
        unsafe { abi::engine_init_impl(cache_root, cb) }.map_err(|e| (status::INTERNAL, e))
    }) {
        Ok(()) => ENGINE_HANDLE,
        Err(_) => 0,
    }
}

/// # Safety
/// `bytes/len` 指向合法 wasm 组件字节缓冲。
#[no_mangle]
pub unsafe extern "C" fn nvwa_component_new(engine: u64, bytes: *const u8, len: usize) -> u64 {
    let r = guard(|| {
        if engine != ENGINE_HANDLE {
            return Err((status::BAD_HANDLE, "invalid engine handle".into()));
        }
        let Some(eng) = abi::engine() else {
            return Err((status::BAD_HANDLE, "engine not initialized".into()));
        };
        if bytes.is_null() && len != 0 {
            return Err((status::BAD_ARGS, "null bytes".into()));
        }
        let wasm = if len == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(bytes, len) }
        };
        let comp = call::compile(eng, wasm).map_err(|e| (status::COMPILE, e))?;
        let mut h = handles().lock().expect("handles lock");
        h.next += 1;
        let handle = h.next;
        h.components.insert(handle, std::sync::Arc::new(comp));
        Ok(handle)
    });
    r.unwrap_or(0)
}

/// # Safety
/// 句柄必须来自 `nvwa_component_new`；重复 drop 为容忍的 no-op。
#[no_mangle]
pub unsafe extern "C" fn nvwa_component_drop(component: u64) {
    let _ = guard(|| {
        handles()
            .lock()
            .expect("handles lock")
            .components
            .remove(&component);
        Ok(())
    });
}

/// fresh store。`env` = 宿主 per-call 上下文（回调首参透传；宿主保证
/// store 存活期内有效，即 `store_drop`/`call_export` 完成前不可释放）。
///
/// # Safety
/// `data_dir` 指向合法 C 字符串；`env` 按上述契约。
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn nvwa_store_new(
    engine: u64,
    component: u64,
    data_dir: *const c_char,
    mem_limit: u64,
    table_limit: u64,
    fuel: u64,
    timeout_ms: u64,
    env: *mut c_void,
) -> u64 {
    let r = guard(|| {
        if engine != ENGINE_HANDLE {
            return Err((status::BAD_HANDLE, "invalid engine handle".into()));
        }
        let Some(eng) = abi::engine() else {
            return Err((status::BAD_HANDLE, "engine not initialized".into()));
        };
        let comp = handles()
            .lock()
            .expect("handles lock")
            .components
            .get(&component)
            .cloned()
            .ok_or((status::BAD_HANDLE, "invalid component handle".into()))?;
        let spec = StoreSpec {
            data_dir: unsafe { abi::cstr_to_string(data_dir) },
            memory_bytes: mem_limit.min(usize::MAX as u64) as usize,
            table_elements: table_limit,
            fuel,
            timeout_ms,
            env: EnvPtr(env),
        };
        let store = store::fresh_store(eng, &spec).map_err(|e| (status::INTERNAL, e))?;
        let mut h = handles().lock().expect("handles lock");
        h.next += 1;
        let handle = h.next;
        h.stores.insert(handle, StoreEntry { store, comp });
        Ok(handle)
    });
    r.unwrap_or(0)
}

/// 单次导出调用。`name` ∈ `get-metadata` / `execute` / `observe`；
/// `args_json`：execute = 完整 tool-input JSON，observe = 事件 JSON，
/// get-metadata 忽略。status=ok 时 `*out_json` = 结果 JSON（宿主用
/// `nvwa_free_string` 释放）；其余 status 时错误详情在 `nvwa_last_error`。
///
/// store 语义：一次性（fresh-store-per-call）——调用即消耗，无论成败；
/// 之后的 `nvwa_store_drop` 为容忍的 no-op。
///
/// # Safety
/// `name/args_json` 指向合法 C 字符串；`out_json` 可写。
#[no_mangle]
pub unsafe extern "C" fn nvwa_call_export(
    store: u64,
    name: *const c_char,
    args_json: *const c_char,
    out_json: *mut *mut c_char,
) -> c_int {
    let r = guard(|| {
        if callbacks().is_none() {
            return Err((status::CALLBACK_ERROR, "callbacks not initialized".into()));
        }
        let name_s = unsafe { abi::cstr_to_string(name) };
        let args_s = unsafe { abi::cstr_to_string(args_json) };
        // 摘出 store（一次性语义；全局锁只在摘取/放回瞬间持有，不横跨
        // 实例化与执行）。
        let entry = {
            let mut h = handles().lock().expect("handles lock");
            h.stores.remove(&store)
        };
        let Some(mut entry) = entry else {
            return Err((
                status::BAD_HANDLE,
                "invalid or consumed store handle".into(),
            ));
        };
        let result = call::call_export(&mut entry.store, &entry.comp, &name_s, &args_s);
        // entry.store 在此 drop（fresh-store-per-call 的壳侧对齐；env 指向
        // 的宿主上下文由宿主在 store_drop 之后才释放——契约）。
        match result {
            Ok(json) => {
                unsafe { *out_json = abi::string_to_cstr(&json) };
                Ok(status::OK)
            }
            Err((code, msg)) => {
                set_err(&msg);
                // 错误文本同样走 out_json（loader 侧统一从 out 取载荷）。
                unsafe { *out_json = abi::string_to_cstr(&msg) };
                Err((code, msg))
            }
        }
    });
    match r {
        Ok(code) => code,
        Err((code, _)) => code,
    }
}

/// # Safety
/// 句柄必须来自 `nvwa_store_new`；已被 call_export 消耗时为容忍的 no-op。
#[no_mangle]
pub unsafe extern "C" fn nvwa_store_drop(store: u64) {
    let _ = guard(|| {
        handles()
            .lock()
            .expect("handles lock")
            .stores
            .remove(&store);
        Ok(())
    });
}

/// 线程局部错误槽快照（调用方用 `nvwa_free_string` 释放）。
#[no_mangle]
pub extern "C" fn nvwa_last_error() -> *mut c_char {
    let msg = LAST_ERROR.with(|e| e.borrow().clone());
    abi::string_to_cstr(&msg)
}

/// 释放壳分配的字符串（`nvwa_call_export`/`nvwa_last_error` 返回值）。
///
/// # Safety
/// 指针必须来自本壳导出；空指针容忍。
#[no_mangle]
pub unsafe extern "C" fn nvwa_free_string(s: *mut c_char) {
    if !s.is_null() {
        drop(unsafe { std::ffi::CString::from_raw(s) });
    }
}
