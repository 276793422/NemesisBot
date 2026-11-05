//! 布局哨兵三臂识别回归锁（2026-11-03 自持写通道批）。
//!
//! 契约（abi.rs 布局演进纪律）：`struct_size` 哨兵恒在 8 函数之后的固定
//! offset，增补槽（llm-complete / secret-set / config-set）一律追加在哨兵
//! 之后；宿主按自身布局填整表字节数，壳先读哨兵判布局再读其余槽。回归
//! 隐患：若哨兵位随增补漂移，旧布局内存上读哨兵 = 越界读栈垃圾，三臂
//! 判定随机翻转（宿主/壳版本错配时从「诚实失败」劣化为「内存腐坏」）。
//!
//! 直测 [`crate::abi::recognize_callback_layout`] 纯函数——engine_init 的
//! 单例状态不被触碰。宿主短布局的模拟走哨兵形态（全尺寸结构 + 短布局
//! 的 struct_size = 识别分支读到的正是同布局前缀，全程有初始化内存，
//! 无越界读）。

use std::mem::offset_of;
use std::os::raw::{c_char, c_int, c_void};
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::abi::{HostCallbacks, HostCallbacksLegacy, HostCallbacksLlmOnly};

// -- no-op 桩：每槽一址。函数体互异（各自 fetch_add 独立静态）防链接器
//    identical-folding 把地址合并——地址比对才有意义。

unsafe extern "C" fn s_free_str(_s: *mut c_char) {
    FN_FREE_STR.fetch_add(1, Ordering::Relaxed);
}
unsafe extern "C" fn s_log(_env: *mut c_void, _level: c_int, _msg: *const c_char) {
    FN_LOG.fetch_add(1, Ordering::Relaxed);
}
unsafe extern "C" fn s_now_millis(_env: *mut c_void, _out_ms: *mut u64) -> c_int {
    FN_NOW.fetch_add(1, Ordering::Relaxed);
    0
}
unsafe extern "C" fn s_config_get(
    _env: *mut c_void,
    _key: *const c_char,
    _out_found: *mut c_int,
    _out_value: *mut *mut c_char,
    _out_detail: *mut *mut c_char,
) -> c_int {
    FN_CONFIG_GET.fetch_add(1, Ordering::Relaxed);
    0
}
unsafe extern "C" fn s_secret_get(
    _env: *mut c_void,
    _name: *const c_char,
    _out_value: *mut *mut c_char,
    _out_detail: *mut *mut c_char,
) -> c_int {
    FN_SECRET_GET.fetch_add(1, Ordering::Relaxed);
    0
}
unsafe extern "C" fn s_workspace_read(
    _env: *mut c_void,
    _rel: *const c_char,
    _out_b64: *mut *mut c_char,
    _out_detail: *mut *mut c_char,
) -> c_int {
    FN_WS_READ.fetch_add(1, Ordering::Relaxed);
    0
}
unsafe extern "C" fn s_data_dir_path(
    _env: *mut c_void,
    _out_path: *mut *mut c_char,
    _out_detail: *mut *mut c_char,
) -> c_int {
    FN_DATA_DIR.fetch_add(1, Ordering::Relaxed);
    0
}
unsafe extern "C" fn s_http_send(
    _env: *mut c_void,
    _req: *const c_char,
    _out_json: *mut *mut c_char,
    _out_detail: *mut *mut c_char,
) -> c_int {
    FN_HTTP.fetch_add(1, Ordering::Relaxed);
    0
}
unsafe extern "C" fn s_tool_invoke(
    _env: *mut c_void,
    _tool: *const c_char,
    _args: *const c_char,
    _out: *mut *mut c_char,
    _out_detail: *mut *mut c_char,
) -> c_int {
    FN_TOOL.fetch_add(1, Ordering::Relaxed);
    0
}
unsafe extern "C" fn s_llm_complete(
    _env: *mut c_void,
    _prompt: *const c_char,
    _max_tokens: u32,
    _out: *mut *mut c_char,
    _out_detail: *mut *mut c_char,
) -> c_int {
    FN_LLM.fetch_add(1, Ordering::Relaxed);
    0
}

static FN_FREE_STR: AtomicUsize = AtomicUsize::new(0);
static FN_LOG: AtomicUsize = AtomicUsize::new(0);
static FN_NOW: AtomicUsize = AtomicUsize::new(0);
static FN_CONFIG_GET: AtomicUsize = AtomicUsize::new(0);
static FN_SECRET_GET: AtomicUsize = AtomicUsize::new(0);
static FN_WS_READ: AtomicUsize = AtomicUsize::new(0);
static FN_DATA_DIR: AtomicUsize = AtomicUsize::new(0);
static FN_HTTP: AtomicUsize = AtomicUsize::new(0);
static FN_TOOL: AtomicUsize = AtomicUsize::new(0);
static FN_LLM: AtomicUsize = AtomicUsize::new(0);

/// 基础槽位期望表（三种布局的前缀字段同名同型——legacy 结构作基础形态，
/// 逐槽断言时按字段比对；fn 指针字段 `==` 即地址比对）。
const BASE: HostCallbacksLegacy = HostCallbacksLegacy {
    free_str: s_free_str,
    log: s_log,
    now_millis: s_now_millis,
    config_get: s_config_get,
    secret_get: s_secret_get,
    workspace_read: s_workspace_read,
    data_dir_path: s_data_dir_path,
    http_send: s_http_send,
    tool_invoke: s_tool_invoke,
    struct_size: 0,
};

/// llm 增补槽期望（Option 整值比对——不走 fn 指针裸 `==`，免
/// ambiguous_fn_ptr_comparisons 告警）。
const EXPECT_LLM: Option<
    unsafe extern "C" fn(
        *mut c_void,
        *const c_char,
        u32,
        *mut *mut c_char,
        *mut *mut c_char,
    ) -> c_int,
> = Some(s_llm_complete);

fn assert_base_mapped(got: &HostCallbacks) {
    // fn_addr_eq：双方同为 fn 指针字段（类型统一无碍）；地址比对在同
    // codegen unit 内有意义（桩函数体互异防 folding，见上）。逐点直呼
    // （泛型函数别名会钉死单一实例化，跨槽复用必炸）。
    assert!(
        std::ptr::fn_addr_eq(got.free_str, BASE.free_str),
        "free_str mis-mapped"
    );
    assert!(std::ptr::fn_addr_eq(got.log, BASE.log), "log mis-mapped");
    assert!(
        std::ptr::fn_addr_eq(got.now_millis, BASE.now_millis),
        "now_millis mis-mapped"
    );
    assert!(
        std::ptr::fn_addr_eq(got.config_get, BASE.config_get),
        "config_get mis-mapped"
    );
    assert!(
        std::ptr::fn_addr_eq(got.secret_get, BASE.secret_get),
        "secret_get mis-mapped"
    );
    assert!(
        std::ptr::fn_addr_eq(got.workspace_read, BASE.workspace_read),
        "workspace_read mis-mapped"
    );
    assert!(
        std::ptr::fn_addr_eq(got.data_dir_path, BASE.data_dir_path),
        "data_dir_path mis-mapped"
    );
    assert!(
        std::ptr::fn_addr_eq(got.http_send, BASE.http_send),
        "http_send mis-mapped"
    );
    assert!(
        std::ptr::fn_addr_eq(got.tool_invoke, BASE.tool_invoke),
        "tool_invoke mis-mapped"
    );
}

/// 哨兵同位性 + 三尺寸互异且递增（legacy < llm-only < full）。哨兵位
/// 漂移 = 本用例当场红，三臂判定不再可能读到栈垃圾。
#[test]
fn layout_sentinel_fixed_across_all_arms() {
    let legacy = offset_of!(HostCallbacksLegacy, struct_size);
    let mid = offset_of!(HostCallbacksLlmOnly, struct_size);
    let full = offset_of!(HostCallbacks, struct_size);
    assert_eq!(legacy, mid, "sentinel offset must match legacy arm");
    assert_eq!(legacy, full, "sentinel offset must match llm-only arm");

    let (l, m, f) = (
        std::mem::size_of::<HostCallbacksLegacy>(),
        std::mem::size_of::<HostCallbacksLlmOnly>(),
        std::mem::size_of::<HostCallbacks>(),
    );
    assert!(l < m, "legacy {l} < llm-only {m}");
    assert!(m < f, "llm-only {m} < full {f}");
}

#[test]
fn recognize_full_layout_passthrough() {
    let cb = HostCallbacks {
        free_str: BASE.free_str,
        log: BASE.log,
        now_millis: BASE.now_millis,
        config_get: BASE.config_get,
        secret_get: BASE.secret_get,
        workspace_read: BASE.workspace_read,
        data_dir_path: BASE.data_dir_path,
        http_send: BASE.http_send,
        tool_invoke: BASE.tool_invoke,
        struct_size: std::mem::size_of::<HostCallbacks>(),
        llm_complete: Some(s_llm_complete),
        secret_set: None,
        config_set: None,
    };
    let got = crate::abi::recognize_callback_layout(&cb).expect("full layout recognized");
    assert_base_mapped(&got);
    assert_eq!(
        got.struct_size,
        std::mem::size_of::<HostCallbacks>(),
        "sentinel passthrough"
    );
    assert!(
        got.llm_complete
            .is_some_and(|f| std::ptr::fn_addr_eq(f, EXPECT_LLM.unwrap())),
        "full arm keeps llm slot"
    );
}

#[test]
fn recognize_llm_only_layout_maps_llm_fills_writes_none() {
    // 宿主传入短布局 = 全尺寸 backing 上的哨兵读出 mid 尺寸：识别必须走
    // mid 臂（整表换算），llm 槽保留、自持写两槽按 None 归一。
    let cb = HostCallbacks {
        free_str: BASE.free_str,
        log: BASE.log,
        now_millis: BASE.now_millis,
        config_get: BASE.config_get,
        secret_get: BASE.secret_get,
        workspace_read: BASE.workspace_read,
        data_dir_path: BASE.data_dir_path,
        http_send: BASE.http_send,
        tool_invoke: BASE.tool_invoke,
        struct_size: std::mem::size_of::<HostCallbacksLlmOnly>(),
        llm_complete: Some(s_llm_complete),
        secret_set: None,
        config_set: None,
    };
    let got = crate::abi::recognize_callback_layout(&cb).expect("llm-only layout recognized");
    assert_base_mapped(&got);
    assert_eq!(got.struct_size, std::mem::size_of::<HostCallbacksLlmOnly>());
    assert!(
        got.llm_complete
            .is_some_and(|f| std::ptr::fn_addr_eq(f, EXPECT_LLM.unwrap())),
        "llm-only arm preserves llm slot"
    );
    assert!(got.secret_set.is_none(), "write slots absent in mid layout");
    assert!(got.config_set.is_none(), "write slots absent in mid layout");
}

#[test]
fn recognize_legacy_layout_fills_all_appended_slots_none() {
    let cb = HostCallbacks {
        free_str: BASE.free_str,
        log: BASE.log,
        now_millis: BASE.now_millis,
        config_get: BASE.config_get,
        secret_get: BASE.secret_get,
        workspace_read: BASE.workspace_read,
        data_dir_path: BASE.data_dir_path,
        http_send: BASE.http_send,
        tool_invoke: BASE.tool_invoke,
        struct_size: std::mem::size_of::<HostCallbacksLegacy>(),
        llm_complete: None,
        secret_set: None,
        config_set: None,
    };
    let got = crate::abi::recognize_callback_layout(&cb).expect("legacy layout recognized");
    assert_base_mapped(&got);
    assert_eq!(got.struct_size, std::mem::size_of::<HostCallbacksLegacy>());
    assert!(got.llm_complete.is_none(), "legacy arm has no llm slot");
    assert!(got.secret_set.is_none());
    assert!(got.config_set.is_none());
}

#[test]
fn recognize_unknown_size_honest_failure() {
    let mut cb = HostCallbacks {
        free_str: BASE.free_str,
        log: BASE.log,
        now_millis: BASE.now_millis,
        config_get: BASE.config_get,
        secret_get: BASE.secret_get,
        workspace_read: BASE.workspace_read,
        data_dir_path: BASE.data_dir_path,
        http_send: BASE.http_send,
        tool_invoke: BASE.tool_invoke,
        struct_size: 3, // 三臂都不认的垃圾哨兵
        llm_complete: None,
        secret_set: None,
        config_set: None,
    };
    let Err(err) = crate::abi::recognize_callback_layout(&cb) else {
        panic!("unknown sentinel must fail honestly");
    };
    assert!(err.contains("struct size mismatch"), "{err}");
    // 哨兵漂移位（host 半更新：比 full 大但不等于任一已知臂）同样拒——
    // 这是哨兵纪律要防的「增补槽插到哨兵之前」形态。
    cb.struct_size = std::mem::size_of::<HostCallbacks>() + 8;
    let Err(err) = crate::abi::recognize_callback_layout(&cb) else {
        panic!("half-updated host sentinel must fail honestly");
    };
    assert!(err.contains("struct size mismatch"), "{err}");
}
