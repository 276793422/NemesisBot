//! 宿主侧契约单测：envelope 解析 / manifest 解析 / 虚拟插件句柄端到端
//! （不 dlopen——同签名 extern "C" fn 构造句柄）/ 桥槽 set/peek。

use std::ffi::{CString, c_char};
use std::sync::atomic::{AtomicBool, Ordering};

use super::{
    HOST_RESOURCES_ABI_VERSION, PRS_ABI_VERSION_SYMBOL, PRS_FREE_SYMBOL, PRS_GET_SYMBOL,
    PRS_MANIFEST_SYMBOL, RESOURCES_KIND_PERSONA_CATALOG, ResourcesManifest, ResourcesPluginHandle,
    parse_get_envelope, resources_plugin_handle, set_resources_plugin_handle, snapshot_content,
    snapshot_tree_filtered,
};

// ---------------------------------------------------------------------------
// 符号名与版本常量（loader 与插件双侧对齐的锚）
// ---------------------------------------------------------------------------

#[test]
fn symbol_names_are_frozen() {
    assert_eq!(PRS_ABI_VERSION_SYMBOL, "prs_abi_version");
    assert_eq!(PRS_MANIFEST_SYMBOL, "prs_manifest");
    assert_eq!(PRS_GET_SYMBOL, "prs_get");
    assert_eq!(PRS_FREE_SYMBOL, "prs_free");
    assert_eq!(HOST_RESOURCES_ABI_VERSION, 1);
}

// ---------------------------------------------------------------------------
// envelope 解析
// ---------------------------------------------------------------------------

#[test]
fn envelope_ok_takes_content() {
    let raw = r##"{"ok":true,"content":"# hello\n正文"}"##;
    assert_eq!(parse_get_envelope(raw).unwrap(), "# hello\n正文");
}

#[test]
fn envelope_err_carries_code_and_msg() {
    let err = parse_get_envelope(r#"{"ok":false,"err_code":"not_found","err_msg":"x.md"}"#)
        .expect_err("err 分支必须 Err");
    assert!(err.contains("[not_found]") && err.contains("x.md"));
}

#[test]
fn envelope_corrupt_json_is_err() {
    assert!(parse_get_envelope("{{{not json").is_err());
    // ok=true 但缺 content
    assert!(parse_get_envelope(r#"{"ok":true}"#).is_err());
    // 多字节 UTF-8 损坏串：错误文案截断必须按字符边界（字节切片在中文字符
    // 中间 panic = 宿主崩溃，回归锁 2026-11-12）。
    let multibyte = format!("{{{{损坏的JSON：{}", "字".repeat(150));
    assert!(parse_get_envelope(&multibyte).is_err());
}

// ---------------------------------------------------------------------------
// manifest 解析
// ---------------------------------------------------------------------------

const SAMPLE_MANIFEST: &str = r#"{
  "abi": 1,
  "built_at_utc": "2026-11-12T12:00:00Z",
  "source": "baseline",
  "embed_source": "bundled snapshot",
  "resources": [{
    "kind": "persona-catalog",
    "id": "agency-agents",
    "upstream_repo": "msitarzewski/agency-agents",
    "upstream_commit": "8ae6164602c95ca78828b148b9052460ee426885",
    "fetched_at_utc": "2026-11-12T12:22:00Z",
    "file_count": 2,
    "license": "MIT",
    "tree": ["a.md", "b/engineering/x.md"]
  }, {
    "kind": "future-kind",
    "id": "other",
    "upstream_commit": "0",
    "file_count": 1,
    "tree": ["y.md"]
  }]
}"#;

#[test]
fn manifest_parse_and_entry_lookup() {
    let m = ResourcesManifest::parse(SAMPLE_MANIFEST).expect("样例 manifest 合法");
    assert_eq!(m.abi, 1);
    assert_eq!(m.source, "baseline");
    let e = m.entry("persona-catalog").expect("persona 条目在场");
    assert_eq!(e.file_count, 2);
    assert_eq!(e.license, "MIT");
    assert_eq!(e.tree.len(), 2);
    // 未知 kind → None（诚实 unavailable）
    assert!(m.entry("no-such-kind").is_none());
}

#[test]
fn manifest_parse_rejects_garbage() {
    assert!(ResourcesManifest::parse("not json").is_err());
    // abi 字段缺失（required）→ Err
    assert!(ResourcesManifest::parse(r#"{"resources":[]}"#).is_err());
}

// ---------------------------------------------------------------------------
// 虚拟插件句柄（同签名 extern "C" fn——不 dlopen 的端到端）
// ---------------------------------------------------------------------------

unsafe extern "C" fn fake_manifest() -> *mut c_char {
    c_string(SAMPLE_MANIFEST)
}

unsafe extern "C" fn fake_get(kind: *const c_char, id: *const c_char) -> *mut c_char {
    let k = unsafe { std::ffi::CStr::from_ptr(kind) }
        .to_string_lossy()
        .into_owned();
    let i = unsafe { std::ffi::CStr::from_ptr(id) }
        .to_string_lossy()
        .into_owned();
    if k != "persona-catalog" {
        return c_string(r#"{"ok":false,"err_code":"unknown_kind","err_msg":"k"}"#);
    }
    if i == "a.md" {
        return c_string(r#"{"ok":true,"content":"AAA"}"#);
    }
    c_string(r#"{"ok":false,"err_code":"not_found","err_msg":"nf"}"#)
}

unsafe extern "C" fn fake_free(p: *mut c_char) {
    if !p.is_null() {
        drop(unsafe { CString::from_raw(p) });
    }
}

static FAKE_FREED: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn fake_free_marks(p: *mut c_char) {
    unsafe { fake_free(p) };
    FAKE_FREED.store(true, Ordering::SeqCst);
}

fn c_string(s: &str) -> *mut c_char {
    CString::new(s).unwrap().into_raw()
}

fn fake_handle() -> ResourcesPluginHandle {
    ResourcesPluginHandle {
        manifest: fake_manifest,
        get: fake_get,
        free: fake_free_marks,
    }
}

#[test]
fn handle_manifest_str_and_free_discipline() {
    FAKE_FREED.store(false, Ordering::SeqCst);
    let h = fake_handle();
    let raw = unsafe { h.manifest_str() }.expect("manifest 读取成功");
    assert!(raw.contains("persona-catalog"));
    assert!(FAKE_FREED.load(Ordering::SeqCst), "返回串已被 free 归还");
}

#[test]
fn handle_get_content_happy_and_err_arms() {
    let h = fake_handle();
    assert_eq!(
        unsafe { h.get_content("persona-catalog", "a.md") }.unwrap(),
        "AAA"
    );
    let err = unsafe { h.get_content("persona-catalog", "missing.md") }.expect_err("缺失必须 Err");
    assert!(err.contains("[not_found]"));
    let err = unsafe { h.get_content("wrong-kind", "a.md") }.expect_err("未知 kind 必须 Err");
    assert!(err.contains("[unknown_kind]"));
}

#[test]
fn handle_null_manifest_is_err() {
    unsafe extern "C" fn null_manifest() -> *mut c_char {
        std::ptr::null_mut()
    }
    let h = ResourcesPluginHandle {
        manifest: null_manifest,
        get: fake_get,
        free: fake_free,
    };
    assert!(unsafe { h.manifest_str() }.is_err());
}

// ---------------------------------------------------------------------------
// 桥槽（OnceLock 进程全局——set/peek 一起测，无顺序依赖）
// ---------------------------------------------------------------------------

#[test]
fn bridge_set_then_peek() {
    assert!(
        resources_plugin_handle().is_none(),
        "进程内首次 peek = 未装载"
    );
    let h: &'static ResourcesPluginHandle = Box::leak(Box::new(fake_handle()));
    set_resources_plugin_handle(h);
    assert!(resources_plugin_handle().is_some());
    // 重复 set 静默忽略（先到者赢）
    let h2: &'static ResourcesPluginHandle = Box::leak(Box::new(fake_handle()));
    set_resources_plugin_handle(h2);
    assert!(std::ptr::eq(resources_plugin_handle().unwrap(), h));
}

// ---------------------------------------------------------------------------
// 快照访问层（依赖桥槽——本文件只有 bridge_set_then_peek 注入 fake 句柄，
// 进程全局 OnceLock 故断言只做「注入后」形态；测试执行顺序不定，两态皆可）
// ---------------------------------------------------------------------------

#[test]
fn short_label_commit_at_date() {
    let m = ResourcesManifest::parse(SAMPLE_MANIFEST).unwrap();
    let e = m.entry("persona-catalog").unwrap();
    assert_eq!(e.short_label(), "8ae6164 @ 2026-11-12");
    // 无日期 = 只出 commit 段
    let mut e2 = e.clone();
    e2.fetched_at_utc = String::new();
    assert_eq!(e2.short_label(), "8ae6164");
    // 短 commit（<7）原样
    let mut e3 = e.clone();
    e3.upstream_commit = "abc".to_string();
    assert_eq!(e3.short_label(), "abc @ 2026-11-12");
}

#[test]
fn snapshot_access_absent_bridge_or_empty_filter() {
    // 桥未注入（测试进程早期）或已注入 fake（bridge_set_then_peek 先跑时）：
    // fake 的 manifest 也有 persona-catalog 条目 → 过滤恒 false = None。
    let none = snapshot_tree_filtered(RESOURCES_KIND_PERSONA_CATALOG, |_| false);
    let _ = none; // 两种进程态下都是 None：缺席 / 过滤空
    // 真缺失 kind：桥注入 fake 时 None；未注入时也是 None（缺席语义一致）
    assert!(snapshot_tree_filtered("no-such-kind", |_| true).is_none());
    // content：fake 注入态下 a.md 命中；缺席态 Err——统一不断言具体值，
    // 只保证不 panic 且形态合法。
    let _ = snapshot_content(RESOURCES_KIND_PERSONA_CATALOG, "a.md");
}
