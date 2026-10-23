//! Store 组装（fresh-store-per-call）与宿主函数 Linker 面。
//!
//! 对齐宿主旧 runtime.rs 的语义：per-plugin 数据目录 preopen `/data`（RW，
//! guest 唯一文件面；其余 fs/socket 全 deny）、fuel + epoch 双预算、内存/表
//! limiter（Store 级累计计量）。

use std::os::raw::{c_char, c_void};

use wasmtime::component::{Linker, ResourceTable};
use wasmtime::Store;
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

use crate::abi::{self, callbacks};

/// Store 数据（对齐宿主旧 PluginCtx；caps/策略全在宿主侧，壳只持透传指针
/// 与资源计量器）。
pub(crate) struct ShellCtx {
    pub(crate) wasi: WasiCtx,
    pub(crate) table: ResourceTable,
    /// 宿主 per-call 上下文（不透明；回调首参透传）。
    pub(crate) env: EnvPtr,
    pub(crate) limiter: ShellLimiter,
}

/// `*mut c_void` 的 Send 包装（Store<ShellCtx> 需要 Send 才能跨线程移动；
/// 指向的宿主上下文由宿主保证 store 存活期内有效——store_drop 契约）。
#[derive(Clone, Copy)]
pub(crate) struct EnvPtr(pub(crate) *mut c_void);
unsafe impl Send for EnvPtr {}

/// 内存/表资源 limiter（对齐宿主旧 PluginResourceLimiter：Store 存活期
/// 累计计量、单调不减；超限 = 分配失败 = guest trap 面）。
pub(crate) struct ShellLimiter {
    memory_bytes: usize,
    table_elements: u64,
    memory_total: usize,
    table_total: u64,
}

impl ShellLimiter {
    fn new(memory_bytes: usize, table_elements: u64) -> Self {
        Self {
            memory_bytes,
            table_elements,
            memory_total: 0,
            table_total: 0,
        }
    }
}

impl wasmtime::ResourceLimiter for ShellLimiter {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> Result<bool, wasmtime::Error> {
        let new_total = self
            .memory_total
            .saturating_add(desired.saturating_sub(current));
        if new_total > self.memory_bytes {
            return Err(wasmtime::Error::msg(format!(
                "memory limit exceeded: cumulative {new_total} > {}",
                self.memory_bytes
            )));
        }
        self.memory_total = new_total;
        Ok(true)
    }

    fn table_growing(
        &mut self,
        current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> Result<bool, wasmtime::Error> {
        let new_total = self
            .table_total
            .saturating_add(desired.saturating_sub(current) as u64);
        if new_total > self.table_elements {
            return Err(wasmtime::Error::msg(format!(
                "table limit exceeded: cumulative {new_total} > {}",
                self.table_elements
            )));
        }
        self.table_total = new_total;
        Ok(true)
    }
}

impl WasiView for ShellCtx {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

/// fresh store 组装参数。
pub(crate) struct StoreSpec {
    pub(crate) data_dir: String,
    pub(crate) memory_bytes: usize,
    pub(crate) table_elements: u64,
    pub(crate) fuel: u64,
    pub(crate) timeout_ms: u64,
    pub(crate) env: EnvPtr,
}

/// 组装 fresh store（对齐宿主旧 fresh_store 语义）。
pub(crate) fn fresh_store(
    engine: &wasmtime::Engine,
    spec: &StoreSpec,
) -> Result<Store<ShellCtx>, String> {
    let data_dir = std::path::PathBuf::from(&spec.data_dir);
    let mut builder = wasmtime_wasi::WasiCtx::builder();
    builder
        .preopened_dir(&data_dir, "/data", wasmtime_wasi::FsPerms::ReadWrite)
        .map_err(|e| format!("preopen data dir: {e}"))?;
    let wasi = builder.build();
    let mut store = Store::new(
        engine,
        ShellCtx {
            wasi,
            table: ResourceTable::new(),
            env: spec.env,
            limiter: ShellLimiter::new(spec.memory_bytes, spec.table_elements),
        },
    );
    store
        .set_fuel(spec.fuel)
        .map_err(|e| format!("set fuel: {e}"))?;
    store.set_epoch_deadline(abi::epoch_deadline_ticks(spec.timeout_ms));
    store.limiter(|s| &mut s.limiter);
    Ok(store)
}

/// fresh linker：宿主能力面（9 函数 → C 回调分发）+ wasi p2 空壳。
///
/// 三个 world 的 host 接口同名同型（`nemesis:plugin/host@0.1.0`），组件只
/// 绑定自己导入的；一次注册覆盖全部 world。llm-complete 恒注册（不按
/// world 区分）——新版 bindgen 产出的组件可能无论 world 都 import 该函数
///（import 裁剪不保证），缺席会让无辜组件实例化失败。
pub(crate) fn fresh_linker(engine: &wasmtime::Engine) -> Linker<ShellCtx> {
    let mut linker = Linker::new(engine);
    register_host_face(&mut linker);
    // wasi 空壳（组件经 std 引入的 wasi:cli/random/clocks 导入兜底；无
    // preopen 之外的 fs、无网络、无参数——宿主能力面之外 guest 什么也够不着）。
    if let Err(e) = wasmtime_wasi::p2::add_to_linker_sync(&mut linker) {
        eprintln!("[plugin-wasm] wasi p2 linker register failed: {e}");
    }
    linker
}

/// WIT host 接口实例名（package nemesis:plugin@0.1.0 的 interface host）。
const HOST_INSTANCE: &str = "nemesis:plugin/host@0.1.0";

/// 把 9 个宿主函数以动态 func_new 注册进 linker（类型在实例化期按组件
/// 导入声明匹配——壳不声明 Canonical ABI 类型，只按 WIT 形状取参写果）。
fn register_host_face(linker: &mut Linker<ShellCtx>) {
    // linker.instance 在 wasmtime 48 返回 Result（实例名重注册会 Err；此处
    // 每调用 fresh linker，恒 Ok）。
    let mut inst = match linker.instance(HOST_INSTANCE) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("[plugin-wasm] host instance register failed: {e}");
            return;
        }
    };
    let _ = inst.func_new("log", |store, _f, params, _results| {
        let env = store.data().env.0;
        let level = log_level_int(&params[0])?;
        let message = val_string(&params[1])?;
        unsafe {
            if let Some(cb) = callbacks() {
                // message 由壳分配、宿主按 const 只读消费；壳负责归还。
                let msg_c = crate::abi::string_to_cstr(&message);
                (cb.log)(env, level, msg_c);
                take_back_cstr(msg_c);
            }
        }
        Ok(())
    });
    let _ = inst.func_new("now-millis", |store, _f, _params, results| {
        let env = store.data().env.0;
        let mut ms: u64 = 0;
        unsafe {
            if let Some(cb) = callbacks() {
                let _tag = (cb.now_millis)(env, &mut ms);
            }
        }
        results[0] = wasmtime::component::Val::U64(ms);
        Ok(())
    });
    let _ = inst.func_new("config-get", |store, _f, params, results| {
        let env = store.data().env.0;
        let key = val_string(&params[0])?;
        results[0] = unsafe { call_config_get(env, &key) };
        Ok(())
    });
    let _ = inst.func_new("secret-get", |store, _f, params, results| {
        let env = store.data().env.0;
        let name = val_string(&params[0])?;
        results[0] = unsafe { call_secret_get(env, &name) };
        Ok(())
    });
    let _ = inst.func_new("workspace-read", |store, _f, params, results| {
        let env = store.data().env.0;
        let rel = val_string(&params[0])?;
        results[0] = unsafe { call_workspace_read(env, &rel) };
        Ok(())
    });
    let _ = inst.func_new("data-dir-path", |store, _f, _params, results| {
        let env = store.data().env.0;
        results[0] = unsafe { call_data_dir_path(env) };
        Ok(())
    });
    let _ = inst.func_new("http-send", |store, _f, params, results| {
        let env = store.data().env.0;
        let req = val_to_json(&params[0]);
        results[0] = unsafe { call_http_send(env, &req) };
        Ok(())
    });
    let _ = inst.func_new("tool-invoke", |store, _f, params, results| {
        let env = store.data().env.0;
        let tool = val_string(&params[0])?;
        let args = val_string(&params[1])?;
        results[0] = unsafe { call_tool_invoke(env, &tool, &args) };
        Ok(())
    });
    let _ = inst.func_new("llm-complete", |store, _f, params, results| {
        let env = store.data().env.0;
        let prompt = val_string(&params[0])?;
        let max_tokens = val_u32(&params[1])?;
        results[0] = unsafe { call_llm_complete(env, &prompt, max_tokens) };
        Ok(())
    });
}

// ---------------------------------------------------------------------------
// 回调分发 helper（C tag + 出参 → WIT result<_, host-error> Val）
// ---------------------------------------------------------------------------

/// 分配两个出参字符串并调回调的通用形态（返回 tag）。
unsafe fn call_cb_1s_2out(
    env: *mut c_void,
    arg: &str,
    f: unsafe extern "C" fn(*mut c_void, *const c_char, *mut *mut c_char, *mut *mut c_char) -> i32,
) -> (i32, Option<String>, Option<String>) {
    let mut out: *mut c_char = std::ptr::null_mut();
    let mut detail: *mut c_char = std::ptr::null_mut();
    let arg_c = crate::abi::string_to_cstr(arg);
    let tag = f(env, arg_c, &mut out, &mut detail);
    // arg_c 是壳分配、宿主按 const 只读——壳自释放。
    take_back_cstr(arg_c);
    (tag, take_out(out), take_out(detail))
}

/// 消费宿主返回的字符串出参（String 化 + free_str 归还）。
unsafe fn take_out(p: *mut c_char) -> Option<String> {
    if p.is_null() {
        return None;
    }
    let s = std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned();
    if let Some(cb) = callbacks() {
        (cb.free_str)(p);
    }
    Some(s)
}

/// 归还壳自己分配、宿主只读消费的字符串。
unsafe fn take_back_cstr(p: *mut c_char) {
    if !p.is_null() {
        drop(std::ffi::CString::from_raw(p));
    }
}

/// tag + 载荷 → `result<T, host-error>` Val。
fn result_val(
    ok: Option<wasmtime::component::Val>,
    tag: i32,
    detail: Option<String>,
) -> wasmtime::component::Val {
    use wasmtime::component::Val;
    if tag == abi::host_err::OK {
        return Val::Result(Ok(ok.map(Box::new)));
    }
    let (name, payload): (&str, Option<String>) = match tag {
        abi::host_err::NO_PERMISSION => ("no-permission", None),
        abi::host_err::POLICY_DENIED => ("policy-denied", detail),
        abi::host_err::NOT_FOUND => ("not-found", detail),
        abi::host_err::BUDGET_EXCEEDED => ("budget-exceeded", detail),
        _ => ("unavailable", detail),
    };
    let inner = payload
        .map(|d| {
            Box::new(Val::Variant(
                name.to_string(),
                Some(Box::new(Val::String(d))),
            ))
        })
        .unwrap_or_else(|| Box::new(Val::Variant(name.to_string(), None)));
    Val::Result(Err(Some(inner)))
}

unsafe fn call_config_get(env: *mut c_void, key: &str) -> wasmtime::component::Val {
    use wasmtime::component::Val;
    let Some(cb) = callbacks() else {
        return result_val(
            None,
            abi::host_err::UNAVAILABLE,
            Some("callbacks not initialized".into()),
        );
    };
    let mut found: i32 = 0;
    let mut out: *mut c_char = std::ptr::null_mut();
    let mut detail: *mut c_char = std::ptr::null_mut();
    let key_c = crate::abi::string_to_cstr(key);
    let tag = (cb.config_get)(env, key_c, &mut found, &mut out, &mut detail);
    take_back_cstr(key_c);
    let value = take_out(out);
    let detail = take_out(detail);
    if tag == abi::host_err::OK {
        let opt = if found != 0 {
            Some(Box::new(Val::Option(Some(Box::new(Val::String(
                value.unwrap_or_default(),
            ))))))
        } else {
            Some(Box::new(Val::Option(None)))
        };
        return Val::Result(Ok(opt));
    }
    result_val(None, tag, detail)
}

unsafe fn call_secret_get(env: *mut c_void, name: &str) -> wasmtime::component::Val {
    let Some(cb) = callbacks() else {
        return result_val(
            None,
            abi::host_err::UNAVAILABLE,
            Some("callbacks not initialized".into()),
        );
    };
    let mut out: *mut c_char = std::ptr::null_mut();
    let mut detail: *mut c_char = std::ptr::null_mut();
    let name_c = crate::abi::string_to_cstr(name);
    let tag = (cb.secret_get)(env, name_c, &mut out, &mut detail);
    take_back_cstr(name_c);
    let value = take_out(out);
    let detail = take_out(detail);
    result_val(value.map(wasmtime::component::Val::String), tag, detail)
}

unsafe fn call_workspace_read(env: *mut c_void, rel: &str) -> wasmtime::component::Val {
    use base64::Engine as _;
    use wasmtime::component::Val;
    let Some(cb) = callbacks() else {
        return result_val(
            None,
            abi::host_err::UNAVAILABLE,
            Some("callbacks not initialized".into()),
        );
    };
    let (tag, b64, detail) = call_cb_1s_2out(env, rel, cb.workspace_read);
    if tag != abi::host_err::OK {
        return result_val(None, tag, detail);
    }
    let bytes = b64
        .as_deref()
        .and_then(|s| base64::engine::general_purpose::STANDARD.decode(s).ok())
        .unwrap_or_default();
    let list = Val::List(bytes.into_iter().map(Val::U8).collect());
    result_val(Some(list), tag, detail)
}

unsafe fn call_data_dir_path(env: *mut c_void) -> wasmtime::component::Val {
    let Some(cb) = callbacks() else {
        return result_val(
            None,
            abi::host_err::UNAVAILABLE,
            Some("callbacks not initialized".into()),
        );
    };
    let mut out: *mut c_char = std::ptr::null_mut();
    let mut detail: *mut c_char = std::ptr::null_mut();
    let tag = (cb.data_dir_path)(env, &mut out, &mut detail);
    let path = take_out(out);
    let detail = take_out(detail);
    result_val(path.map(wasmtime::component::Val::String), tag, detail)
}

unsafe fn call_http_send(env: *mut c_void, request_json: &str) -> wasmtime::component::Val {
    use base64::Engine as _;
    use wasmtime::component::Val;
    let Some(cb) = callbacks() else {
        return result_val(
            None,
            abi::host_err::UNAVAILABLE,
            Some("callbacks not initialized".into()),
        );
    };
    let mut out: *mut c_char = std::ptr::null_mut();
    let mut detail: *mut c_char = std::ptr::null_mut();
    let req_c = crate::abi::string_to_cstr(request_json);
    let tag = (cb.http_send)(env, req_c, &mut out, &mut detail);
    take_back_cstr(req_c);
    let resp = take_out(out);
    let detail = take_out(detail);
    if tag != abi::host_err::OK {
        return result_val(None, tag, detail);
    }
    // 响应 JSON → http-response record。
    let Ok(v) = serde_json::from_str::<serde_json::Value>(resp.as_deref().unwrap_or("{}")) else {
        return result_val(
            None,
            abi::host_err::UNAVAILABLE,
            Some("http response decode failed".into()),
        );
    };
    let status = v
        .get("status")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as u16;
    let mut headers: Vec<Val> = Vec::new();
    if let Some(hs) = v.get("headers").and_then(serde_json::Value::as_array) {
        for h in hs {
            if let Some(pair) = h.as_array() {
                let k = pair
                    .first()
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                let w = pair
                    .get(1)
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                headers.push(Val::Tuple(vec![
                    Val::String(k.into()),
                    Val::String(w.into()),
                ]));
            }
        }
    }
    let body = v
        .get("body")
        .and_then(serde_json::Value::as_str)
        .and_then(|s| base64::engine::general_purpose::STANDARD.decode(s).ok())
        .unwrap_or_default();
    let record = Val::Record(vec![
        ("status".into(), Val::U16(status)),
        ("headers".into(), Val::List(headers)),
        (
            "body".into(),
            Val::List(body.into_iter().map(Val::U8).collect()),
        ),
    ]);
    Val::Result(Ok(Some(Box::new(record))))
}

unsafe fn call_tool_invoke(env: *mut c_void, tool: &str, args: &str) -> wasmtime::component::Val {
    let Some(cb) = callbacks() else {
        return result_val(
            None,
            abi::host_err::UNAVAILABLE,
            Some("callbacks not initialized".into()),
        );
    };
    let mut out: *mut c_char = std::ptr::null_mut();
    let mut detail: *mut c_char = std::ptr::null_mut();
    let tool_c = crate::abi::string_to_cstr(tool);
    let args_c = crate::abi::string_to_cstr(args);
    let tag = (cb.tool_invoke)(env, tool_c, args_c, &mut out, &mut detail);
    take_back_cstr(tool_c);
    take_back_cstr(args_c);
    let content = take_out(out);
    let detail = take_out(detail);
    result_val(content.map(wasmtime::component::Val::String), tag, detail)
}

/// llm-complete 分发（2026-10-22 批 3 增补）：槽缺失（legacy 宿主回调表）=
/// 诚实 unavailable——fail-closed，绝不静默假装成功。
unsafe fn call_llm_complete(
    env: *mut c_void,
    prompt: &str,
    max_tokens: u32,
) -> wasmtime::component::Val {
    use wasmtime::component::Val;
    let Some(cb) = callbacks() else {
        return result_val(
            None,
            abi::host_err::UNAVAILABLE,
            Some("callbacks not initialized".into()),
        );
    };
    let Some(llm) = cb.llm_complete else {
        return result_val(
            None,
            abi::host_err::UNAVAILABLE,
            Some("llm-complete callback not available in this host".into()),
        );
    };
    let mut out: *mut c_char = std::ptr::null_mut();
    let mut detail: *mut c_char = std::ptr::null_mut();
    let prompt_c = crate::abi::string_to_cstr(prompt);
    let tag = llm(env, prompt_c, max_tokens, &mut out, &mut detail);
    take_back_cstr(prompt_c);
    let content = take_out(out);
    let detail = take_out(detail);
    result_val(content.map(Val::String), tag, detail)
}

// ---------------------------------------------------------------------------
// Val 取参 helper
// ---------------------------------------------------------------------------

/// Val → string（类型失配诚实报错——实例化期类型匹配后不该发生）。
fn val_string(v: &wasmtime::component::Val) -> Result<String, wasmtime::Error> {
    match v {
        wasmtime::component::Val::String(s) => Ok(s.clone()),
        other => Err(wasmtime::Error::msg(format!(
            "host func param type mismatch: expected string, got {other:?}"
        ))),
    }
}

/// Val → u32（同 [`val_string`] 纪律）。
fn val_u32(v: &wasmtime::component::Val) -> Result<u32, wasmtime::Error> {
    match v {
        wasmtime::component::Val::U32(n) => Ok(*n),
        other => Err(wasmtime::Error::msg(format!(
            "host func param type mismatch: expected u32, got {other:?}"
        ))),
    }
}

/// log-level 枚举 Val（bindgen 形态 = `Val::Enum("trace"|…)`）→ C 整数档位。
fn log_level_int(v: &wasmtime::component::Val) -> Result<i32, wasmtime::Error> {
    let w = |m: &str| wasmtime::Error::msg(m.to_string());
    match v {
        wasmtime::component::Val::Enum(name) => match name.as_str() {
            "trace" => Ok(0),
            "debug" => Ok(1),
            "info" => Ok(2),
            "warn" => Ok(3),
            "error" => Ok(4),
            other => Err(w(&format!("unknown log level: {other}"))),
        },
        other => Err(w(&format!(
            "host func param type mismatch: expected enum log-level, got {other:?}"
        ))),
    }
}

/// 泛型 Val → JSON（http-request record 参数形态：字符串/数字/数组/对象/
/// option/null；bytes 假定为 base64 string——由宿主 JSON 形态约定）。
pub(crate) fn val_to_json(v: &wasmtime::component::Val) -> String {
    let value = val_to_json_value(v);
    value.to_string()
}

pub(crate) fn val_to_json_value(v: &wasmtime::component::Val) -> serde_json::Value {
    use wasmtime::component::Val;
    match v {
        Val::Bool(b) => serde_json::Value::Bool(*b),
        Val::S8(n) => (*n as i64).into(),
        Val::U8(n) => (*n as u64).into(),
        Val::S16(n) => (*n as i64).into(),
        Val::U16(n) => (*n as u64).into(),
        Val::S32(n) => (*n as i64).into(),
        Val::U32(n) => (*n as u64).into(),
        Val::S64(n) => (*n).into(),
        Val::U64(n) => (*n).into(),
        Val::Float32(f) => serde_json::Value::from(*f),
        Val::Float64(f) => serde_json::Value::from(*f),
        Val::Char(c) => serde_json::Value::String(c.to_string()),
        Val::String(s) => serde_json::Value::String(s.clone()),
        Val::List(items) => serde_json::Value::Array(items.iter().map(val_to_json_value).collect()),
        Val::Tuple(items) => {
            serde_json::Value::Array(items.iter().map(val_to_json_value).collect())
        }
        Val::Record(fields) => {
            let mut m = serde_json::Map::new();
            for (k, v) in fields {
                m.insert(k.clone(), val_to_json_value(v));
            }
            serde_json::Value::Object(m)
        }
        Val::Option(Some(inner)) => val_to_json_value(inner),
        Val::Option(None) => serde_json::Value::Null,
        Val::Result(Ok(inner)) => inner
            .as_ref()
            .map(|b| val_to_json_value(b))
            .unwrap_or(serde_json::Value::Null),
        Val::Result(Err(inner)) => inner
            .as_ref()
            .map(|b| val_to_json_value(b))
            .unwrap_or(serde_json::Value::Null),
        // 变体在合同出参面只以 host-error 出现，已在 result 层特判；
        // 这里给保守投影（tag + detail）。
        Val::Variant(name, payload) => {
            let mut m = serde_json::Map::new();
            m.insert("tag".into(), serde_json::Value::String(name.clone()));
            m.insert(
                "detail".into(),
                payload
                    .as_ref()
                    .map(|b| val_to_json_value(b))
                    .unwrap_or(serde_json::Value::Null),
            );
            serde_json::Value::Object(m)
        }
        Val::Enum(name) => serde_json::Value::String(name.clone()),
        Val::Flags(bits) => serde_json::Value::Array(
            bits.iter()
                .map(|f| serde_json::Value::String(f.clone()))
                .collect(),
        ),
        _ => serde_json::Value::Null,
    }
}
