//! C-ABI 合同常量与宿主回调表布局（权威源在本壳；宿主 exe 侧按同布局镜像声明）。
//!
//! 跨界纪律：字节串（UTF-8 + `\0` 结尾）+ 不透明句柄（u64）+ 数值 + host-error
//! 五值 tag，零 Rust 类型跨界。所有权纪律：回调返回的字符串一律由宿主分配、
//! 壳消费后经 `free_str` 归还宿主释放（谁分配谁释放，规避跨 CRT 分配器）；
//! 壳返回给宿主的字符串同理（宿主用 `free_string` 归还壳）。

use std::os::raw::{c_char, c_int, c_void};

/// ABI 代际（宿主 `abi_version` 握手；不匹配 = 拒绝装配）。
pub const ABI_VERSION: u32 = 1;

/// 调用状态码（`call_export` 返回；壳内把 wasmtime 错误归类，宿主映射回
/// 自身错误枚举）。
pub mod status {
    use std::os::raw::c_int;
    /// 成功（`out_json` 有效）。
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
    /// 宿主回调通道故障（回调不可用；正常策略拒绝走结果 JSON 不走此码）。
    pub const CALLBACK_ERROR: c_int = 6;
    /// 壳内部错误（含 store 复用等契约违例）。
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

/// 引擎句柄（进程单例；`engine_init` 成功恒返回此值）。
pub const ENGINE_HANDLE: u64 = 1;

/// 宿主回调表（字符串释放器 + 8 个宿主函数 + struct_size 哨兵 + 增补槽；
/// `#[repr(C)]`，布局由本壳权威）。
///
/// 布局演进纪律（批 3 实证）：**哨兵位固定**——`struct_size` 恒在 8 函数
/// 之后的固定 offset，新增槽一律追加在哨兵之后；宿主在哨兵处填整表字节
/// 数，壳先读哨兵判布局再读其余槽（哨兵若随增补漂移，旧布局内存上读它
/// = 越界读栈垃圾，判定随机翻转）。
///
/// - 全部策略语义在宿主实现里（egress/vault/审计/帧纪律），壳只分发不解释；
/// - 每个回调首参 `env` = `store_new` 传入的宿主 per-call 上下文指针，
///   壳只透传从不解引用；
/// - 每个回调**必须**在宿主内部 `catch_unwind`（FFI 边界禁 unwind 外泄），
///   panic 时返回 `host_err::UNAVAILABLE` + detail；
/// - 返回 `*mut c_char` 的出参：宿主分配（`CString::into_raw`），壳消费后
///   必经 `free_str` 释放；
/// - 返回 tag：`host_err::*`；detail 出参仅错误时非空。
#[repr(C)]
pub struct HostCallbacks {
    /// 释放宿主分配的字符串（`CString::into_raw` 的归还端）。
    pub free_str: unsafe extern "C" fn(s: *mut c_char),
    /// `log(level, message)`；WIT 无返回值，tag 恒 0（保留 int 形态统一
    /// panic 护栏）。level：0=trace 1=debug 2=info 3=warn 4=error。
    pub log: unsafe extern "C" fn(env: *mut c_void, level: c_int, message: *const c_char),
    /// `now-millis() -> u64`；预算耗尽宿主自行落 0（tag 恒 0）。
    pub now_millis: unsafe extern "C" fn(env: *mut c_void, out_ms: *mut u64) -> c_int,
    /// `config-get(key) -> result<option<string>, host-error>`；
    /// `out_found`=0/1；`out_value` 仅 found=1 时非空。
    pub config_get: unsafe extern "C" fn(
        env: *mut c_void,
        key: *const c_char,
        out_found: *mut c_int,
        out_value: *mut *mut c_char,
        out_detail: *mut *mut c_char,
    ) -> c_int,
    /// `secret-get(name) -> result<string, host-error>`；`out_value` 仅 ok 时非空。
    pub secret_get: unsafe extern "C" fn(
        env: *mut c_void,
        name: *const c_char,
        out_value: *mut *mut c_char,
        out_detail: *mut *mut c_char,
    ) -> c_int,
    /// `workspace-read(rel-path) -> result<list<u8>, host-error>`；
    /// 字节经 base64（标准字母表 + padding）。
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
    /// 布局哨兵：**位固定**——紧跟 8 函数之后，offset 不随后续增补槽漂移
    /// （宿主镜像在**此处**填 `std::mem::size_of::<宿主镜像>()` = 整表字节
    /// 数；壳必须先读本字段判定布局，才谈得上读其余槽）。壳接受两种布局：
    /// 本结构（含 llm-complete 增补槽）或 [`HostCallbacksLegacy`]（批 3 前
    /// 8 函数布局，两种布局中本字段同位同读）；其余值诚实失败而非内存腐坏。
    pub struct_size: usize,
    /// `llm-complete(prompt, max-tokens) -> result<string, host-error>`
    /// （2026-10-22 批 3 增补槽；**追加在哨兵之后**——哨兵位固定纪律：
    /// 未来新槽同 pattern 追加，永不插到哨兵之前）。`Option<fn>` 经空指针
    /// 优化与裸 fn 指针逐位相同——None = 空 = 槽缺失。宿主按
    /// [`HostCallbacksLegacy`] 旧布局注册时本槽不在表内，壳按 None 处置：
    /// guest 调 llm-complete 恒返 host-error unavailable。
    pub llm_complete: Option<
        unsafe extern "C" fn(
            env: *mut c_void,
            prompt: *const c_char,
            max_tokens: u32,
            out_content: *mut *mut c_char,
            out_detail: *mut *mut c_char,
        ) -> c_int,
    >,
}

/// 批 3 前回调表布局（free_str + 8 fn 指针 + struct_size；与升级前
/// [`HostCallbacks`] 逐字段相同）。仅用于 `engine_init` 识别旧宿主传入的
/// legacy 尺寸并换算成新结构（llm-complete = None），不参与其他路径。
#[repr(C)]
pub struct HostCallbacksLegacy {
    /// 同 [`HostCallbacks::free_str`]。
    pub free_str: unsafe extern "C" fn(s: *mut c_char),
    /// 同 [`HostCallbacks::log`]。
    pub log: unsafe extern "C" fn(env: *mut c_void, level: c_int, message: *const c_char),
    /// 同 [`HostCallbacks::now_millis`]。
    pub now_millis: unsafe extern "C" fn(env: *mut c_void, out_ms: *mut u64) -> c_int,
    /// 同 [`HostCallbacks::config_get`]。
    pub config_get: unsafe extern "C" fn(
        env: *mut c_void,
        key: *const c_char,
        out_found: *mut c_int,
        out_value: *mut *mut c_char,
        out_detail: *mut *mut c_char,
    ) -> c_int,
    /// 同 [`HostCallbacks::secret_get`]。
    pub secret_get: unsafe extern "C" fn(
        env: *mut c_void,
        name: *const c_char,
        out_value: *mut *mut c_char,
        out_detail: *mut *mut c_char,
    ) -> c_int,
    /// 同 [`HostCallbacks::workspace_read`]。
    pub workspace_read: unsafe extern "C" fn(
        env: *mut c_void,
        rel_path: *const c_char,
        out_b64: *mut *mut c_char,
        out_detail: *mut *mut c_char,
    ) -> c_int,
    /// 同 [`HostCallbacks::data_dir_path`]。
    pub data_dir_path: unsafe extern "C" fn(
        env: *mut c_void,
        out_path: *mut *mut c_char,
        out_detail: *mut *mut c_char,
    ) -> c_int,
    /// 同 [`HostCallbacks::http_send`]。
    pub http_send: unsafe extern "C" fn(
        env: *mut c_void,
        request_json: *const c_char,
        out_response_json: *mut *mut c_char,
        out_detail: *mut *mut c_char,
    ) -> c_int,
    /// 同 [`HostCallbacks::tool_invoke`]。
    pub tool_invoke: unsafe extern "C" fn(
        env: *mut c_void,
        tool: *const c_char,
        args_json: *const c_char,
        out_content: *mut *mut c_char,
        out_detail: *mut *mut c_char,
    ) -> c_int,
    /// 同 [`HostCallbacks::struct_size`]（legacy 尺寸）。
    pub struct_size: usize,
}

// 纯 fn 指针 + usize，自动 Send + Sync（static 存放合法；宿主保证回调
// 线程安全——exe 侧策略核心本就是 Send+Sync 面貌）。

/// 引擎槽（进程单例）。
pub(crate) struct EngineSlot {
    pub engine: wasmtime::Engine,
}

static ENGINE: std::sync::OnceLock<EngineSlot> = std::sync::OnceLock::new();
static CALLBACKS: std::sync::OnceLock<HostCallbacks> = std::sync::OnceLock::new();

/// epoch ticker 步进间隔（墙钟超时粒度；50ms 足够细且唤醒开销可忽略）。
pub(crate) const EPOCH_TICK_MS: u64 = 50;

/// 墙钟超时毫秒 → epoch deadline tick 数（至少 1 tick）。
pub(crate) fn epoch_deadline_ticks(timeout_ms: u64) -> u64 {
    (timeout_ms / EPOCH_TICK_MS).max(1)
}

/// 取引擎单例（`engine_init` 已成功的前提下）。
pub(crate) fn engine() -> Option<&'static wasmtime::Engine> {
    ENGINE.get().map(|s| &s.engine)
}

/// 取宿主回调表。
pub(crate) fn callbacks() -> Option<&'static HostCallbacks> {
    CALLBACKS.get()
}

/// 初始化引擎单例 + 注册回调表（幂等；首个成功者定缓存配置与回调表）。
///
/// 回调表布局按 `struct_size` 哨兵识别：新布局（含 llm-complete 槽）或
/// [`HostCallbacksLegacy`]（批 3 前布局，llm-complete = None）均可；其余
/// 尺寸诚实失败。
///
/// # Safety
///
/// `cache_root` 可为空（无缓存）；`cb` 必须指向有效的 [`HostCallbacks`]
/// 或 [`HostCallbacksLegacy`] 布局内存（按其 `struct_size` 判定），指向的
/// 内存在进程存活期有效。
pub(crate) unsafe fn engine_init_impl(
    cache_root: *const c_char,
    cb: *const HostCallbacks,
) -> Result<(), String> {
    if cb.is_null() {
        return Err("callbacks pointer is null".into());
    }
    let cb = &*cb;
    let shell_size = std::mem::size_of::<HostCallbacks>();
    // 布局哨兵（位固定，两种布局同位可安全首读）：只接受已知尺寸，杜绝
    // 按错误布局整读越界（llm-complete 槽在 legacy 布局中不存在，必须整
    // 表换算而非直接整读）。
    let stored = if cb.struct_size == shell_size {
        unsafe { std::ptr::read(cb) }
    } else if cb.struct_size == std::mem::size_of::<HostCallbacksLegacy>() {
        let old: HostCallbacksLegacy = unsafe { std::ptr::read(std::ptr::from_ref(cb).cast()) };
        HostCallbacks {
            free_str: old.free_str,
            log: old.log,
            now_millis: old.now_millis,
            config_get: old.config_get,
            secret_get: old.secret_get,
            workspace_read: old.workspace_read,
            data_dir_path: old.data_dir_path,
            http_send: old.http_send,
            tool_invoke: old.tool_invoke,
            struct_size: old.struct_size,
            llm_complete: None,
        }
    } else {
        return Err(format!(
            "callbacks struct size mismatch: host {} != shell {} or legacy {}",
            cb.struct_size,
            shell_size,
            std::mem::size_of::<HostCallbacksLegacy>()
        ));
    };
    let _ = CALLBACKS.set(stored);

    if ENGINE.get().is_some() {
        return Ok(());
    }
    let mut cfg = wasmtime::Config::new();
    cfg.wasm_component_model(true);
    cfg.consume_fuel(true);
    cfg.epoch_interruption(true);
    if !cache_root.is_null() {
        let root = cstr_to_string(cache_root);
        match enable_workspace_cache(std::path::Path::new(&root)) {
            Ok(cache) => {
                cfg.cache(Some(cache));
            }
            Err(e) => {
                // 缓存是性能优化不是正确性依赖；失败回退无缓存（对齐宿主
                // 旧 engine.rs 行为）。
                eprintln!("[plugin-wasm] cache init failed, fallback no-cache: {e}");
            }
        }
    }
    let engine = wasmtime::Engine::new(&cfg).map_err(|e| format!("engine: {e}"))?;
    start_epoch_ticker(engine.clone())?;
    let _ = ENGINE.set(EngineSlot { engine });
    Ok(())
}

/// 把缓存目录接进 wasmtime Engine 配置（对齐宿主旧 enable_workspace_cache）。
fn enable_workspace_cache(root: &std::path::Path) -> Result<wasmtime::Cache, String> {
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let dir = root.join("entries");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let cfg_path = root.join("wasmtime-cache.toml");
    // wasmtime 48 CacheConfig `deny_unknown_fields`：目录键名 `directory`、
    // 无 `enabled` 开关；路径剥 `\\?\` verbatim 前缀并换正斜杠。
    let dir_str = dir.to_string_lossy();
    let dir_str = dir_str
        .strip_prefix(r"\\?\")
        .unwrap_or(dir_str.as_ref())
        .replace('\\', "/");
    let body = format!("[cache]\ndirectory = '{dir_str}'\n");
    std::fs::write(&cfg_path, body).map_err(|e| e.to_string())?;
    wasmtime::Cache::from_file(Some(&cfg_path)).map_err(|e| format!("load cache config: {e}"))
}

/// 进程级 epoch ticker（独立 OS 线程；不依赖任何 runtime 在场）。
fn start_epoch_ticker(engine: wasmtime::Engine) -> Result<(), String> {
    std::thread::Builder::new()
        .name("plugin-wasm-epoch-ticker".into())
        .spawn(move || {
            let tick = std::time::Duration::from_millis(EPOCH_TICK_MS);
            loop {
                std::thread::sleep(tick);
                engine.increment_epoch();
            }
        })
        .map(|_| ())
        .map_err(|e| format!("epoch ticker thread: {e}"))
}

/// `*const c_char` → String（空指针 = 空串；UTF-8 失败有损替换）。
///
/// # Safety
/// 指针必须指向合法 NUL 结尾缓冲（宿主契约）。
pub(crate) unsafe fn cstr_to_string(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
}

/// String → 宿主堆上字符串（`CString::into_raw`；消费方负责 `free_str`）。
pub(crate) fn string_to_cstr(s: &str) -> *mut c_char {
    // 内嵌 NUL 的字符串无法成为 CStr——替换为 U+FFFD（合同禁内嵌 NUL；
    // 防御性不 panic）。
    let s = if s.contains('\0') {
        s.replace('\0', "\u{FFFD}")
    } else {
        s.to_string()
    };
    // CString::new 仅在内嵌 NUL 时失败，上面已排除。
    match std::ffi::CString::new(s) {
        Ok(c) => c.into_raw(),
        Err(_) => std::ffi::CString::new(String::new())
            .expect("empty string is valid CString")
            .into_raw(),
    }
}
