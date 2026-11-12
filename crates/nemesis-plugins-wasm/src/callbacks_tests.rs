//! cb_llm_complete 帧纪律回归锁（2026-10-24）。
//!
//! 契约：llm-complete **只有策略帧可用**（host_impl::LlmInvoker、runtime
//! CallFrame.llm、registry::set_llm 三处注释同口径）。回归前实现只拦
//! observe 帧——tool 帧经 build_frame 无差别注入的 llm 面即可调用，绕过
//! egress deny-by-default 形成未声明出站通道（manifest 零权限插件可把
//! 任意内容经 prompt 送外部模型端点）。修复后：`kind == Policy` 且非
//! observe 帧才放行；壳侧恒注册（link 兼容）不受影响。
//!
//! 直接驱动 extern "C" shim：CallCaps 经 `Box::into_raw` 作 env，调用
//! 区间同步封闭（同 runtime::CallSession 契约），无需 wasm fixture。
//!
//! 2026-11-03 自持写通道批追加：cb_secret_set / cb_config_set 帧纪律 +
//! namespace 闸矩阵（同驱动形态）——写面只 Panel 帧可用、声明闸、
//! writer 未装配诚实 unavailable、审计只记别名不记值。

use std::sync::Arc;

use crate::abi::host_err;
use crate::callbacks::{cb_config_set, cb_free_str, cb_llm_complete, cb_secret_set};
use crate::host_impl::{
    CallCaps, ConfigWriter, FrameBudget, InstanceConfig, LlmInvoker, PluginLogBuffer,
    SecretResolver, SecretWriter,
};
use crate::limits::PluginLimits;
use crate::manifest::PluginKind;

struct StubLlm;

impl LlmInvoker for StubLlm {
    fn complete(
        &self,
        _prompt: &str,
        _max_tokens: u32,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>>
    {
        Box::pin(async { Ok("llm-said-hi".to_string()) })
    }
}

struct NoSecrets;

impl SecretResolver for NoSecrets {
    fn resolve(&self, _alias: &str) -> Option<String> {
        None
    }
}

fn caps_for(kind: PluginKind, observer_frame: bool) -> CallCaps {
    CallCaps {
        slug: "t".to_string(),
        kind,
        observer_frame,
        frame: FrameBudget::new(4),
        logs: Arc::new(PluginLogBuffer::new(8, 256)),
        limits: PluginLimits::default(),
        config: Arc::new(InstanceConfig::default()),
        secrets: Arc::new(NoSecrets),
        workspace_root: std::env::temp_dir(),
        egress: Arc::new(crate::egress::EgressPolicy::default()),
        invoker: None,
        llm: Some(Arc::new(StubLlm)),
        secret_write: None,
        config_write: None,
        tokio_handle: None,
        audit: None,
        session_key: String::new(),
    }
}

/// env 生命周期：Box::into_raw → 同步调用 → Box::from_raw（回调只在
/// shim 执行期内运行，时序封闭）；out/detail 由本宿主分配、cb_free_str
/// 归还。返回 (status 标签, detail 文本)。
fn call_llm(caps: CallCaps, max_tokens: u32) -> (i32, Option<String>) {
    let env = Box::into_raw(Box::new(caps)) as *mut std::os::raw::c_void;
    let prompt = std::ffi::CString::new("hello").expect("static prompt is NUL-free");
    let mut out: *mut std::os::raw::c_char = std::ptr::null_mut();
    let mut detail: *mut std::os::raw::c_char = std::ptr::null_mut();
    let tag = unsafe { cb_llm_complete(env, prompt.as_ptr(), max_tokens, &mut out, &mut detail) };
    let detail_s = if detail.is_null() {
        None
    } else {
        Some(unsafe {
            std::ffi::CStr::from_ptr(detail)
                .to_string_lossy()
                .into_owned()
        })
    };
    unsafe { cb_free_str(out) };
    unsafe { cb_free_str(detail) };
    drop(unsafe { Box::from_raw(env as *mut CallCaps) });
    (tag, detail_s)
}

// 核心回归臂：修复前此用例得 OK（tool 帧被无差别注入 llm 面），修复后
// 恒 UNAVAILABLE。
#[test]
fn r70_tool_frame_llm_complete_denied() {
    let (tag, detail) = call_llm(caps_for(PluginKind::Tool, false), 64);
    assert_eq!(tag, host_err::UNAVAILABLE, "detail: {detail:?}");
    assert!(
        detail
            .unwrap_or_default()
            .contains("only available in policy or panel frames")
    );
}

#[test]
fn r70_observer_frame_llm_complete_denied() {
    let (tag, detail) = call_llm(caps_for(PluginKind::Policy, true), 64);
    assert_eq!(tag, host_err::UNAVAILABLE, "detail: {detail:?}");
}

#[test]
fn r70_observer_kind_frame_llm_complete_denied() {
    let (tag, _) = call_llm(caps_for(PluginKind::Observer, true), 64);
    assert_eq!(tag, host_err::UNAVAILABLE);
}

// 放行臂：策略帧（kind=Policy、非 observe）端到端拿到 StubLlm 结果。
// Handle::block_on 不允许 async 上下文内调用——在 runtime 上下文之外
// 驱动 shim，runtime 活到断言结束。
#[test]
fn r70_policy_frame_llm_complete_ok() {
    let rt = tokio::runtime::Runtime::new().expect("test runtime");
    let mut caps = caps_for(PluginKind::Policy, false);
    caps.tokio_handle = Some(rt.handle().clone());
    let (tag, detail) = call_llm(caps, 64);
    assert_eq!(tag, host_err::OK, "detail: {detail:?}");
}

#[test]
fn r70_zero_max_tokens_denied() {
    let rt = tokio::runtime::Runtime::new().expect("test runtime");
    let mut caps = caps_for(PluginKind::Policy, false);
    caps.tokio_handle = Some(rt.handle().clone());
    let (tag, detail) = call_llm(caps, 0);
    assert_eq!(tag, host_err::POLICY_DENIED, "detail: {detail:?}");
    assert!(
        detail
            .unwrap_or_default()
            .contains("max-tokens must be > 0")
    );
}

// ---------------------------------------------------------------------------
// cb_secret_set / cb_config_set 闸矩阵（自持写通道批）
// ---------------------------------------------------------------------------

struct StubSecretWriter {
    calls: std::sync::Mutex<Vec<(String, String)>>,
    fail: bool,
}

impl StubSecretWriter {
    fn wired(fail: bool) -> Arc<Self> {
        Arc::new(Self {
            calls: std::sync::Mutex::new(Vec::new()),
            fail,
        })
    }

    fn calls(&self) -> Vec<(String, String)> {
        self.calls.lock().expect("stub lock").clone()
    }
}

impl SecretWriter for StubSecretWriter {
    fn write(&self, alias: &str, value: &str) -> Result<(), String> {
        self.calls
            .lock()
            .expect("stub lock")
            .push((alias.to_string(), value.to_string()));
        if self.fail {
            Err("vault locked (set NEMESISBOT_VAULT_PASSPHRASE)".to_string())
        } else {
            Ok(())
        }
    }
}

struct StubConfigWriter {
    calls: std::sync::Mutex<Vec<(String, String, String)>>,
}

impl StubConfigWriter {
    fn wired() -> Arc<Self> {
        Arc::new(Self {
            calls: std::sync::Mutex::new(Vec::new()),
        })
    }

    fn calls(&self) -> Vec<(String, String, String)> {
        self.calls.lock().expect("stub lock").clone()
    }
}

impl ConfigWriter for StubConfigWriter {
    fn set_config(&self, slug: &str, key: &str, value: &str) -> Result<(), String> {
        self.calls.lock().expect("stub lock").push((
            slug.to_string(),
            key.to_string(),
            value.to_string(),
        ));
        Ok(())
    }
}

/// 写面测试 caps：Panel 帧 + 声明表（base_url 普通键 / password x-secret
/// 键）+ 可选 writer 注入。audit None（闸矩阵不验审计落盘——shim 内
/// audit_event 对 None 诚实跳过）。
fn write_caps(
    secret_write: Option<Arc<StubSecretWriter>>,
    config_write: Option<Arc<StubConfigWriter>>,
) -> CallCaps {
    let mut cfg = InstanceConfig::default();
    cfg.declared_keys = vec!["base_url".to_string()];
    cfg.x_secret_keys = vec!["password".to_string()];
    CallCaps {
        slug: "t".to_string(),
        kind: PluginKind::Panel,
        observer_frame: false,
        frame: FrameBudget::new(4),
        logs: Arc::new(PluginLogBuffer::new(8, 256)),
        limits: PluginLimits::default(),
        config: Arc::new(cfg),
        secrets: Arc::new(NoSecrets),
        workspace_root: std::env::temp_dir(),
        egress: Arc::new(crate::egress::EgressPolicy::default()),
        invoker: None,
        llm: None,
        secret_write: secret_write.map(|w| w as Arc<dyn SecretWriter>),
        config_write: config_write.map(|w| w as Arc<dyn ConfigWriter>),
        tokio_handle: None,
        audit: None,
        session_key: String::new(),
    }
}

/// 驱动 cb_secret_set：返回 (status 标签, detail 文本)。
fn call_secret_set(caps: CallCaps, name: &str, value: &str) -> (i32, Option<String>) {
    let env = Box::into_raw(Box::new(caps)) as *mut std::os::raw::c_void;
    let name_c = std::ffi::CString::new(name).expect("test name is NUL-free");
    let value_c = std::ffi::CString::new(value).expect("test value is NUL-free");
    let mut detail: *mut std::os::raw::c_char = std::ptr::null_mut();
    let tag = unsafe { cb_secret_set(env, name_c.as_ptr(), value_c.as_ptr(), &mut detail) };
    let detail_s = if detail.is_null() {
        None
    } else {
        Some(unsafe {
            std::ffi::CStr::from_ptr(detail)
                .to_string_lossy()
                .into_owned()
        })
    };
    unsafe { cb_free_str(detail) };
    drop(unsafe { Box::from_raw(env as *mut CallCaps) });
    (tag, detail_s)
}

/// 驱动 cb_config_set：同 [`call_secret_set`] 形态。
fn call_config_set(caps: CallCaps, key: &str, value: &str) -> (i32, Option<String>) {
    let env = Box::into_raw(Box::new(caps)) as *mut std::os::raw::c_void;
    let key_c = std::ffi::CString::new(key).expect("test key is NUL-free");
    let value_c = std::ffi::CString::new(value).expect("test value is NUL-free");
    let mut detail: *mut std::os::raw::c_char = std::ptr::null_mut();
    let tag = unsafe { cb_config_set(env, key_c.as_ptr(), value_c.as_ptr(), &mut detail) };
    let detail_s = if detail.is_null() {
        None
    } else {
        Some(unsafe {
            std::ffi::CStr::from_ptr(detail)
                .to_string_lossy()
                .into_owned()
        })
    };
    unsafe { cb_free_str(detail) };
    drop(unsafe { Box::from_raw(env as *mut CallCaps) });
    (tag, detail_s)
}

// -- secret-set 帧纪律（写面只 Panel 帧可用——tool/observer 帧构造期即
//    None，调用期诚实 unavailable）。

#[test]
fn write_tool_frame_secret_set_denied() {
    let mut caps = write_caps(Some(StubSecretWriter::wired(false)), None);
    caps.kind = PluginKind::Tool;
    let (tag, detail) = call_secret_set(caps, "password", "v");
    assert_eq!(tag, host_err::UNAVAILABLE, "detail: {detail:?}");
    assert!(
        detail
            .unwrap_or_default()
            .contains("only available in panel frames")
    );
}

#[test]
fn write_observer_frame_secret_set_denied() {
    let mut caps = write_caps(Some(StubSecretWriter::wired(false)), None);
    caps.observer_frame = true;
    let (tag, detail) = call_secret_set(caps, "password", "v");
    assert_eq!(tag, host_err::UNAVAILABLE, "detail: {detail:?}");
}

// -- writer 未装配（standalone 宿主）= 诚实 unavailable 不假成功。

#[test]
fn secret_set_unwired_writer_denied() {
    let (tag, detail) = call_secret_set(write_caps(None, None), "password", "v");
    assert_eq!(tag, host_err::UNAVAILABLE, "detail: {detail:?}");
    assert!(
        detail
            .unwrap_or_default()
            .contains("secret writer not wired")
    );
}

// -- namespace 闸：未声明名 → policy-denied（供给闸的运行期镜像）。

#[test]
fn secret_set_undeclared_name_policy_denied() {
    let (tag, detail) = call_secret_set(
        write_caps(Some(StubSecretWriter::wired(false)), None),
        "other",
        "v",
    );
    assert_eq!(tag, host_err::POLICY_DENIED, "detail: {detail:?}");
    assert!(
        detail
            .unwrap_or_default()
            .contains("not declared in manifest x-secret")
    );
}

// -- 放行臂：写经裸别名 `plugin/<slug>/<name>`（vault: 前缀是组件侧视角
//    ——writer 收裸形态），值原样透传；stub 现场核对调用记录。

#[test]
fn secret_set_ok_writes_bare_alias() {
    let writer = StubSecretWriter::wired(false);
    let (tag, detail) =
        call_secret_set(write_caps(Some(writer.clone()), None), "password", "s3cret");
    assert_eq!(tag, host_err::OK, "detail: {detail:?}");
    let calls = writer.calls();
    assert_eq!(
        calls,
        vec![("plugin/t/password".to_string(), "s3cret".to_string())]
    );
}

// -- vault 侧失败（文件缺失/锁定）= unavailable 诚实报错（detail 带根因
//    文本供面板 remediation），不回退不重试。

#[test]
fn secret_set_writer_error_unavailable() {
    let (tag, detail) = call_secret_set(
        write_caps(Some(StubSecretWriter::wired(true)), None),
        "password",
        "v",
    );
    assert_eq!(tag, host_err::UNAVAILABLE, "detail: {detail:?}");
    assert!(detail.unwrap_or_default().contains("vault locked"));
}

// -- config-set 对偶矩阵：x-secret 键 → policy-denied（配置面/凭据面
//    分离——写凭据必须走 secret-set）；未声明键 → not-found（与 get 同
//    语义）；放行臂核对 (slug, key, value) 全透传。

#[test]
fn config_set_x_secret_key_policy_denied() {
    let (tag, detail) = call_config_set(
        write_caps(None, Some(StubConfigWriter::wired())),
        "password",
        "v",
    );
    assert_eq!(tag, host_err::POLICY_DENIED, "detail: {detail:?}");
    assert!(detail.unwrap_or_default().contains("use secret-set"));
}

#[test]
fn config_set_undeclared_key_not_found() {
    let (tag, detail) = call_config_set(
        write_caps(None, Some(StubConfigWriter::wired())),
        "other",
        "v",
    );
    assert_eq!(tag, host_err::NOT_FOUND, "detail: {detail:?}");
    assert!(detail.unwrap_or_default().contains("key not declared"));
}

#[test]
fn config_set_ok_records_call() {
    let writer = StubConfigWriter::wired();
    let (tag, detail) = call_config_set(
        write_caps(None, Some(writer.clone())),
        "base_url",
        "http://x",
    );
    assert_eq!(tag, host_err::OK, "detail: {detail:?}");
    assert_eq!(
        writer.calls(),
        vec![(
            "t".to_string(),
            "base_url".to_string(),
            "http://x".to_string()
        )]
    );
}

#[test]
fn config_set_tool_frame_denied() {
    let mut caps = write_caps(None, Some(StubConfigWriter::wired()));
    caps.kind = PluginKind::Tool;
    let (tag, detail) = call_config_set(caps, "base_url", "v");
    assert_eq!(tag, host_err::UNAVAILABLE, "detail: {detail:?}");
}

// ---------------------------------------------------------------------------
// 其余 shim 全矩阵（log / now-millis / config-get / secret-get /
// workspace-read / data-dir-path / http-send / tool-invoke / free-str /
// panic 护栏）
// ---------------------------------------------------------------------------

use crate::callbacks::test_only::{
    cb_config_get, cb_data_dir_path, cb_http_send, cb_log, cb_now_millis, cb_secret_get,
    cb_tool_invoke, cb_workspace_read,
};
use crate::host_impl::HostToolInvoker;

/// 全字段可调 caps（remaining shims 用）。
struct CapsBuilder {
    caps: CallCaps,
}

impl CapsBuilder {
    fn new() -> Self {
        let mut cfg = InstanceConfig::default();
        cfg.declared_keys = vec!["base_url".to_string()];
        cfg.x_secret_keys = vec!["password".to_string()];
        Self {
            caps: CallCaps {
                slug: "t".to_string(),
                kind: PluginKind::Tool,
                observer_frame: false,
                frame: FrameBudget::new(8),
                logs: Arc::new(PluginLogBuffer::new(8, 256)),
                limits: PluginLimits::default(),
                config: Arc::new(cfg),
                secrets: Arc::new(NoSecrets),
                workspace_root: std::env::temp_dir(),
                egress: Arc::new(crate::egress::EgressPolicy::default()),
                invoker: None,
                llm: None,
                secret_write: None,
                config_write: None,
                tokio_handle: None,
                audit: None,
                session_key: String::new(),
            },
        }
    }

    fn budget(mut self, n: u32) -> Self {
        self.caps.frame = FrameBudget::new(n);
        self
    }

    fn observer(mut self, on: bool) -> Self {
        self.caps.observer_frame = on;
        self
    }

    fn secrets(mut self, s: Arc<dyn SecretResolver>) -> Self {
        self.caps.secrets = s;
        self
    }

    fn workspace(mut self, root: std::path::PathBuf) -> Self {
        self.caps.workspace_root = root;
        self
    }

    fn egress(mut self, p: crate::egress::EgressPolicy) -> Self {
        self.caps.egress = Arc::new(p);
        self
    }

    fn invoker(mut self, i: Arc<dyn HostToolInvoker>) -> Self {
        self.caps.invoker = Some(i);
        self
    }

    fn tokio(mut self, h: tokio::runtime::Handle) -> Self {
        self.caps.tokio_handle = Some(h);
        self
    }

    fn audit(mut self, a: crate::host_impl::SharedAuditLogger) -> Self {
        self.caps.audit = Some(a);
        self
    }

    fn build(self) -> CallCaps {
        self.caps
    }
}

/// 驱动任意 shim 的公共形态：Box<CallCaps> 作 env、C 串只读、出参串
/// 归还。返回 (tag, detail 文本)。
fn run_shim<F>(caps: CallCaps, f: F) -> (i32, Option<String>)
where
    F: FnOnce(*mut std::os::raw::c_void, &mut *mut std::os::raw::c_char) -> i32,
{
    let env = Box::into_raw(Box::new(caps)) as *mut std::os::raw::c_void;
    let mut detail: *mut std::os::raw::c_char = std::ptr::null_mut();
    let tag = f(env, &mut detail);
    let detail_s = if detail.is_null() {
        None
    } else {
        Some(unsafe {
            std::ffi::CStr::from_ptr(detail)
                .to_string_lossy()
                .into_owned()
        })
    };
    unsafe { cb_free_str(detail) };
    drop(unsafe { Box::from_raw(env as *mut CallCaps) });
    (tag, detail_s)
}

fn cstring(s: &str) -> std::ffi::CString {
    std::ffi::CString::new(s).expect("test string is NUL-free")
}

// -- cb_free_str：空指针容忍 + 真实归还 ---------------------------------------

#[test]
fn free_str_null_and_real_pointer() {
    // SAFETY: 空指针臂 = no-op（shim 契约）。
    unsafe { cb_free_str(std::ptr::null_mut()) };
    // 真实指针（data_dir_path 出参串 = 宿主分配的 CString::into_raw 产物）
    // 经 free_str 归还——无 double-free / 无泄漏即通过。
    let (tag, _detail) = run_shim(CapsBuilder::new().build(), |env, detail| {
        let mut out_path: *mut std::os::raw::c_char = std::ptr::null_mut();
        let t = unsafe { cb_data_dir_path(env, &mut out_path, detail) };
        assert!(!out_path.is_null());
        // SAFETY: out_path 来自 string_to_cstr（CString::into_raw）。
        unsafe { cb_free_str(out_path) };
        t
    });
    assert_eq!(tag, host_err::OK);
}

// -- cb_log：级别映射 + 写入环形缓冲（panic 静默）------------------------------

#[test]
fn log_writes_all_levels_into_ring() {
    let caps = CapsBuilder::new().build();
    let env = Box::into_raw(Box::new(caps)) as *mut std::os::raw::c_void;
    for (raw_level, want) in [
        (0, "trace"),
        (1, "debug"),
        (2, "info"),
        (3, "warn"),
        (9, "error"), // 词表外归 error
    ] {
        let msg = cstring(&format!("m{raw_level}"));
        // SAFETY: env 来自 Box::into_raw，调用区间同步封闭。
        unsafe { cb_log(env, raw_level, msg.as_ptr()) };
        let logs_ref = unsafe { &*(env as *const CallCaps) };
        let (lines, _) = logs_ref.logs.snapshot();
        assert_eq!(lines.last().expect("line").level, want);
        assert!(
            lines
                .last()
                .expect("line")
                .message
                .contains(&format!("m{raw_level}"))
        );
    }
    drop(unsafe { Box::from_raw(env as *mut CallCaps) });
}

// -- cb_now_millis：正常回毫秒；预算耗尽退化为 0 ------------------------------

#[test]
fn now_millis_ok_and_budget_exhausted() {
    // 正常：回当前 Unix 毫秒。
    let caps = CapsBuilder::new().budget(2).build();
    let env = Box::into_raw(Box::new(caps)) as *mut std::os::raw::c_void;
    let mut out_ms: u64 = 0;
    // SAFETY: env 同步封闭。
    let tag = unsafe { cb_now_millis(env, &mut out_ms) };
    assert_eq!(tag, host_err::OK);
    assert!(out_ms > 0, "正常路径回当前毫秒");
    unsafe { cb_now_millis(env, &mut out_ms) };
    unsafe { cb_now_millis(env, &mut out_ms) }; // 第三次：预算 2 已耗尽
    assert_eq!(out_ms, 0, "预算耗尽退化为 0（语义可辨的时间原点）");
    // out_ms 空指针容忍（无 result 返回型，写空指针臂）。
    let tag = unsafe { cb_now_millis(env, std::ptr::null_mut()) };
    assert_eq!(tag, host_err::OK);
    drop(unsafe { Box::from_raw(env as *mut CallCaps) });
}

// -- cb_config_get：预算/x-secret/未声明/已设/已声明未设 ------------------------

#[test]
fn config_get_full_matrix() {
    // 预算耗尽。
    let (tag, detail) = run_shim(CapsBuilder::new().budget(0).build(), |env, detail| {
        let key = cstring("base_url");
        let mut out_value: *mut std::os::raw::c_char = std::ptr::null_mut();
        let mut found: i32 = -1;
        unsafe { cb_config_get(env, key.as_ptr(), &mut found, &mut out_value, detail) }
    });
    assert_eq!(tag, host_err::BUDGET_EXCEEDED, "detail: {detail:?}");

    // x-secret 键 → policy-denied（配置面/凭据面分离）。
    let (tag, detail) = run_shim(CapsBuilder::new().build(), |env, detail| {
        let key = cstring("password");
        let mut out_value: *mut std::os::raw::c_char = std::ptr::null_mut();
        let mut found: i32 = -1;
        unsafe { cb_config_get(env, key.as_ptr(), &mut found, &mut out_value, detail) }
    });
    assert_eq!(tag, host_err::POLICY_DENIED, "detail: {detail:?}");
    assert!(detail.unwrap_or_default().contains("use secret-get"));

    // 未声明键 → not-found。
    let (tag, detail) = run_shim(CapsBuilder::new().build(), |env, detail| {
        let key = cstring("ghost");
        let mut out_value: *mut std::os::raw::c_char = std::ptr::null_mut();
        let mut found: i32 = -1;
        unsafe { cb_config_get(env, key.as_ptr(), &mut found, &mut out_value, detail) }
    });
    assert_eq!(tag, host_err::NOT_FOUND, "detail: {detail:?}");
    assert!(detail.unwrap_or_default().contains("key not declared"));

    // 已声明已设 → found=1 + 值原样。
    let mut caps = CapsBuilder::new().build();
    let cfg = Arc::get_mut(&mut caps.config).expect("unique");
    cfg.entries.insert("base_url".into(), "http://x".into());
    let (tag, detail) = run_shim(caps, |env, detail| {
        let key = cstring("base_url");
        let mut out_value: *mut std::os::raw::c_char = std::ptr::null_mut();
        let mut found: i32 = -1;
        let t = unsafe { cb_config_get(env, key.as_ptr(), &mut found, &mut out_value, detail) };
        assert_eq!(found, 1, "已设键 found=1");
        assert!(!out_value.is_null());
        let v = unsafe {
            std::ffi::CStr::from_ptr(out_value)
                .to_string_lossy()
                .into_owned()
        };
        unsafe { cb_free_str(out_value) };
        assert_eq!(v, "http://x");
        t
    });
    assert_eq!(tag, host_err::OK, "detail: {detail:?}");

    // 已声明未设 → OK 无 detail；found 出参不写（壳侧初始化为 0，shim 只在
    // 命中时写 1——传 -1 探针验证「未触碰」契约）。
    let (tag, detail) = run_shim(CapsBuilder::new().build(), |env, detail| {
        let key = cstring("base_url");
        let mut out_value: *mut std::os::raw::c_char = std::ptr::null_mut();
        let mut found: i32 = -1;
        let t = unsafe { cb_config_get(env, key.as_ptr(), &mut found, &mut out_value, detail) };
        assert_eq!(found, -1, "未设键 shim 不写 found（壳侧初始化 0）");
        assert!(out_value.is_null());
        t
    });
    assert_eq!(tag, host_err::OK);
    assert!(detail.is_none(), "OK 臂不写 detail");
}

// -- cb_secret_get：observer 帧 / 未声明 / 命中 / 缺值 + 审计三态 ---------------

struct OneSecret;
impl SecretResolver for OneSecret {
    fn resolve(&self, alias: &str) -> Option<String> {
        alias.ends_with("/password").then(|| "s3cr3t".to_string())
    }
}

fn audit_logger() -> (tempfile::TempDir, crate::host_impl::SharedAuditLogger) {
    let dir = tempfile::tempdir().expect("audit tmp");
    let logger = nemesis_security::audit_log::AuditLogger::new(
        nemesis_security::audit_log::AuditLogConfig {
            audit_log_dir: dir.path().to_path_buf(),
            enabled: true,
        },
    )
    .expect("logger");
    (dir, Arc::new(std::sync::Mutex::new(logger)))
}

#[test]
fn secret_get_observer_frame_denied() {
    let (tag, detail) = run_shim(CapsBuilder::new().observer(true).build(), |env, detail| {
        let name = cstring("password");
        let mut out_value: *mut std::os::raw::c_char = std::ptr::null_mut();
        unsafe { cb_secret_get(env, name.as_ptr(), &mut out_value, detail) }
    });
    assert_eq!(tag, host_err::UNAVAILABLE, "detail: {detail:?}");
    assert!(detail.unwrap_or_default().contains("observer frames"));
}

#[test]
fn secret_get_budget_and_undeclared() {
    let (tag, detail) = run_shim(CapsBuilder::new().budget(0).build(), |env, detail| {
        let name = cstring("password");
        let mut out_value: *mut std::os::raw::c_char = std::ptr::null_mut();
        unsafe { cb_secret_get(env, name.as_ptr(), &mut out_value, detail) }
    });
    assert_eq!(tag, host_err::BUDGET_EXCEEDED, "detail: {detail:?}");

    let (_dir, audit) = audit_logger();
    let (tag, detail) = run_shim(CapsBuilder::new().audit(audit).build(), |env, detail| {
        let name = cstring("not_declared");
        let mut out_value: *mut std::os::raw::c_char = std::ptr::null_mut();
        unsafe { cb_secret_get(env, name.as_ptr(), &mut out_value, detail) }
    });
    assert_eq!(tag, host_err::POLICY_DENIED, "detail: {detail:?}");
    assert!(
        detail
            .unwrap_or_default()
            .contains("not declared in manifest x-secret")
    );
}

#[test]
fn secret_get_hit_and_miss() {
    // 命中：原文经出参串回 guest（原文只在返回值内）。
    let (_dir, audit) = audit_logger();
    let (tag, detail) = run_shim(
        CapsBuilder::new()
            .secrets(Arc::new(OneSecret))
            .audit(audit)
            .build(),
        |env, detail| {
            let name = cstring("password");
            let mut out_value: *mut std::os::raw::c_char = std::ptr::null_mut();
            let t = unsafe { cb_secret_get(env, name.as_ptr(), &mut out_value, detail) };
            assert!(!out_value.is_null(), "命中臂写出参");
            let v = unsafe {
                std::ffi::CStr::from_ptr(out_value)
                    .to_string_lossy()
                    .into_owned()
            };
            unsafe { cb_free_str(out_value) };
            assert_eq!(v, "s3cr3t");
            t
        },
    );
    assert_eq!(tag, host_err::OK, "detail: {detail:?}");

    // 声明但 vault 无值 → NotFound + 审计 denied（与未声明的 PolicyDenied 分流）。
    let (_dir, audit) = audit_logger();
    let (tag, detail) = run_shim(CapsBuilder::new().audit(audit).build(), |env, detail| {
        let name = cstring("password");
        let mut out_value: *mut std::os::raw::c_char = std::ptr::null_mut();
        unsafe { cb_secret_get(env, name.as_ptr(), &mut out_value, detail) }
    });
    assert_eq!(tag, host_err::NOT_FOUND, "detail: {detail:?}");
    assert!(
        detail
            .unwrap_or_default()
            .contains("no secret stored for alias")
    );
}

// -- cb_workspace_read：base64 回传 + 越界拒绝 + 审计 ---------------------------

#[test]
fn workspace_read_ok_and_denied() {
    let dir = tempfile::tempdir().expect("ws tmp");
    std::fs::write(dir.path().join("ok.txt"), b"payload").expect("write");
    let (_audit_dir, audit) = audit_logger();
    // 根 canonicalize（与闸内 canonical 比较同形态）。
    let root = dir.path().canonicalize().expect("canonical");

    // 命中：base64 回传 + 审计 allowed。
    let (tag, detail) = run_shim(
        CapsBuilder::new()
            .workspace(root.clone())
            .audit(audit)
            .build(),
        |env, detail| {
            let rel = cstring("ok.txt");
            let mut out_b64: *mut std::os::raw::c_char = std::ptr::null_mut();
            let t = unsafe { cb_workspace_read(env, rel.as_ptr(), &mut out_b64, detail) };
            assert!(!out_b64.is_null());
            let b64 = unsafe {
                std::ffi::CStr::from_ptr(out_b64)
                    .to_string_lossy()
                    .into_owned()
            };
            unsafe { cb_free_str(out_b64) };
            let decoded = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &b64)
                .expect("b64");
            assert_eq!(decoded, b"payload");
            t
        },
    );
    assert_eq!(tag, host_err::OK, "detail: {detail:?}");

    // 越界：POLICY_DENIED + 截断 detail。
    let (_audit_dir2, audit2) = audit_logger();
    let (tag, detail) = run_shim(
        CapsBuilder::new().workspace(root).audit(audit2).build(),
        |env, detail| {
            let rel = cstring("../escape.txt");
            let mut out_b64: *mut std::os::raw::c_char = std::ptr::null_mut();
            unsafe { cb_workspace_read(env, rel.as_ptr(), &mut out_b64, detail) }
        },
    );
    assert_eq!(tag, host_err::POLICY_DENIED, "detail: {detail:?}");
    assert!(detail.unwrap_or_default().contains("escapes workspace"));
}

// -- cb_data_dir_path：恒 "/data" --------------------------------------------

#[test]
fn data_dir_path_constant() {
    let (tag, detail) = run_shim(CapsBuilder::new().build(), |env, detail| {
        let mut out_path: *mut std::os::raw::c_char = std::ptr::null_mut();
        let t = unsafe { cb_data_dir_path(env, &mut out_path, detail) };
        assert!(!out_path.is_null());
        let p = unsafe {
            std::ffi::CStr::from_ptr(out_path)
                .to_string_lossy()
                .into_owned()
        };
        unsafe { cb_free_str(out_path) };
        assert_eq!(p, "/data");
        t
    });
    assert_eq!(tag, host_err::OK, "detail: {detail:?}");
}

// -- cb_http_send：解码/预算/闸/无 handle/真实代理往返 --------------------------

fn http_request_json(body: Option<&str>) -> String {
    let body_part = body
        .map(|b| {
            format!(
                ",\"body\":\"{}\"",
                base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b)
            )
        })
        .unwrap_or_default();
    format!(r#"{{"method":"GET","url":"__URL__","headers":[["X-Custom","v1"]]{body_part}}}"#)
}

#[test]
fn http_send_decode_and_budget_failures() {
    // 入参 JSON 损坏 → UNAVAILABLE。
    let (tag, detail) = run_shim(CapsBuilder::new().build(), |env, detail| {
        let raw = cstring("{not json");
        let mut out: *mut std::os::raw::c_char = std::ptr::null_mut();
        unsafe { cb_http_send(env, raw.as_ptr(), &mut out, detail) }
    });
    assert_eq!(tag, host_err::UNAVAILABLE, "detail: {detail:?}");
    assert!(detail.unwrap_or_default().contains("decode failed"));

    // 预算耗尽 → BUDGET_EXCEEDED。
    let (tag, detail) = run_shim(CapsBuilder::new().budget(0).build(), |env, detail| {
        let raw = cstring("{}");
        let mut out: *mut std::os::raw::c_char = std::ptr::null_mut();
        unsafe { cb_http_send(env, raw.as_ptr(), &mut out, detail) }
    });
    assert_eq!(tag, host_err::BUDGET_EXCEEDED, "detail: {detail:?}");
}

#[test]
fn http_send_egress_denied_writes_audit() {
    let (_dir, audit) = audit_logger();
    let raw = http_request_json(None).replace("__URL__", "http://api.example.com/");
    let (tag, detail) = run_shim(CapsBuilder::new().audit(audit).build(), |env, detail| {
        let raw = cstring(&raw);
        let mut out: *mut std::os::raw::c_char = std::ptr::null_mut();
        unsafe { cb_http_send(env, raw.as_ptr(), &mut out, detail) }
    });
    assert_eq!(tag, host_err::NO_PERMISSION, "detail: {detail:?}");
    assert!(
        detail.is_none(),
        "no-permission 臂无 detail（审计承担留痕）"
    );
}

#[test]
fn http_send_without_tokio_handle_fails_closed() {
    let p =
        crate::egress::EgressPolicy::from_permissions(vec!["api.example.com".to_string()], false);
    let raw = http_request_json(None).replace("__URL__", "http://api.example.com/");
    let (tag, detail) = run_shim(CapsBuilder::new().egress(p).build(), |env, detail| {
        let raw = cstring(&raw);
        let mut out: *mut std::os::raw::c_char = std::ptr::null_mut();
        unsafe { cb_http_send(env, raw.as_ptr(), &mut out, detail) }
    });
    assert_eq!(tag, host_err::UNAVAILABLE, "detail: {detail:?}");
    assert!(
        detail
            .unwrap_or_default()
            .contains("egress runtime unavailable")
    );
}

// 本地 server 回环 + 壳回调内 Handle::block_on 在插桩环境挂起（正常
// cargo test 直跑可过）；覆盖率由 egress.rs 的 proxy_request 本地往返 +
// http_send 的 deny/unavailable 臂覆盖，此处跳过。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "本地 server 回环在插桩环境挂起（Handle::block_on 死锁面）；正常 cargo test 可跑"]
async fn http_send_roundtrip_via_local_server_and_denial_path() {
    let body: &'static [u8] = b"egress-ok";
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = tokio::task::spawn_blocking(move || {
        if let Ok((stream, _)) = listener.accept() {
            use std::io::{Read, Write};
            let mut s = stream;
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf);
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = s.write_all(head.as_bytes());
            let _ = s.write_all(body);
        }
    });

    let (_audit_dir, audit) = audit_logger();
    let p = crate::egress::EgressPolicy::from_permissions(
        vec![format!("127.0.0.1:{}", addr.port())],
        true,
    );
    let mut caps = CapsBuilder::new().egress(p).audit(audit).build();
    caps.tokio_handle = Some(tokio::runtime::Handle::current());
    let raw = http_request_json(Some("req-body"))
        .replace("__URL__", &format!("http://127.0.0.1:{}/data", addr.port()));
    let (tag, detail) = run_shim(caps, |env, detail| {
        let raw = cstring(&raw);
        let mut out: *mut std::os::raw::c_char = std::ptr::null_mut();
        let t = unsafe { cb_http_send(env, raw.as_ptr(), &mut out, detail) };
        assert!(!out.is_null(), "成功臂写响应 JSON");
        let resp = unsafe { std::ffi::CStr::from_ptr(out).to_string_lossy().into_owned() };
        unsafe { cb_free_str(out) };
        let v: serde_json::Value = serde_json::from_str(&resp).expect("resp json");
        assert_eq!(v["status"], 200);
        let decoded = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            v["body"].as_str().expect("b64 body"),
        )
        .expect("decode");
        assert_eq!(decoded, body);
        t
    });
    assert_eq!(tag, host_err::OK, "detail: {detail:?}");

    // deny 路径：allowlist 外目标 → POLICY_DENIED + 审计 denied。
    let (_audit_dir2, audit2) = audit_logger();
    let p2 =
        crate::egress::EgressPolicy::from_permissions(vec!["api.example.com".to_string()], true);
    let mut caps2 = CapsBuilder::new().egress(p2).audit(audit2).build();
    caps2.tokio_handle = Some(tokio::runtime::Handle::current());
    let raw2 = http_request_json(None).replace("__URL__", "http://127.0.0.1:1/x");
    let (tag, detail) = run_shim(caps2, |env, detail| {
        let raw = cstring(&raw2);
        let mut out: *mut std::os::raw::c_char = std::ptr::null_mut();
        unsafe { cb_http_send(env, raw.as_ptr(), &mut out, detail) }
    });
    assert_eq!(tag, host_err::POLICY_DENIED, "detail: {detail:?}");
    server.await.expect("server joins");
}

// -- cb_tool_invoke：预算/observer/深度闸/未接线/无 handle/往返 ----------------

struct StubInvoker {
    ok: bool,
}

impl HostToolInvoker for StubInvoker {
    fn invoke(
        &self,
        tool: &str,
        _args_json: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>>
    {
        let tool = tool.to_string();
        Box::pin(async move {
            if self.ok {
                Ok(format!("invoked:{tool}"))
            } else {
                Err("tool exploded".to_string())
            }
        })
    }
}

struct PanickingInvoker;
impl HostToolInvoker for PanickingInvoker {
    fn invoke(
        &self,
        _tool: &str,
        _args_json: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>>
    {
        Box::pin(async { panic!("invoker exploded") })
    }
}

#[test]
fn tool_invoke_gates() {
    // 预算。
    let (tag, detail) = run_shim(CapsBuilder::new().budget(0).build(), |env, detail| {
        let tool = cstring("exec");
        let args = cstring("{}");
        let mut out: *mut std::os::raw::c_char = std::ptr::null_mut();
        unsafe { cb_tool_invoke(env, tool.as_ptr(), args.as_ptr(), &mut out, detail) }
    });
    assert_eq!(tag, host_err::BUDGET_EXCEEDED, "detail: {detail:?}");

    // observer 帧。
    let (tag, detail) = run_shim(CapsBuilder::new().observer(true).build(), |env, detail| {
        let tool = cstring("exec");
        let args = cstring("{}");
        let mut out: *mut std::os::raw::c_char = std::ptr::null_mut();
        unsafe { cb_tool_invoke(env, tool.as_ptr(), args.as_ptr(), &mut out, detail) }
    });
    assert_eq!(tag, host_err::UNAVAILABLE, "detail: {detail:?}");
    assert!(detail.unwrap_or_default().contains("observer frames"));

    // 深度 1：plugin. 前缀目标拒绝。
    let (tag, detail) = run_shim(CapsBuilder::new().build(), |env, detail| {
        let tool = cstring("plugin.other.tool");
        let args = cstring("{}");
        let mut out: *mut std::os::raw::c_char = std::ptr::null_mut();
        unsafe { cb_tool_invoke(env, tool.as_ptr(), args.as_ptr(), &mut out, detail) }
    });
    assert_eq!(tag, host_err::POLICY_DENIED, "detail: {detail:?}");
    assert!(detail.unwrap_or_default().contains("depth 1"));

    // 未接线。
    let (tag, detail) = run_shim(CapsBuilder::new().build(), |env, detail| {
        let tool = cstring("exec");
        let args = cstring("{}");
        let mut out: *mut std::os::raw::c_char = std::ptr::null_mut();
        unsafe { cb_tool_invoke(env, tool.as_ptr(), args.as_ptr(), &mut out, detail) }
    });
    assert_eq!(tag, host_err::UNAVAILABLE, "detail: {detail:?}");
    assert!(detail.unwrap_or_default().contains("not wired"));

    // 接线但无 tokio runtime。
    let (tag, detail) = run_shim(
        CapsBuilder::new()
            .invoker(Arc::new(StubInvoker { ok: true }))
            .build(),
        |env, detail| {
            let tool = cstring("exec");
            let args = cstring("{}");
            let mut out: *mut std::os::raw::c_char = std::ptr::null_mut();
            unsafe { cb_tool_invoke(env, tool.as_ptr(), args.as_ptr(), &mut out, detail) }
        },
    );
    assert_eq!(tag, host_err::UNAVAILABLE, "detail: {detail:?}");
    assert!(detail.unwrap_or_default().contains("no tokio runtime"));
}

#[test]
fn tool_invoke_roundtrip_and_error_text() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    // 成功：结果文本经出参串回传。
    let (tag, detail) = run_shim(
        CapsBuilder::new()
            .invoker(Arc::new(StubInvoker { ok: true }))
            .tokio(rt.handle().clone())
            .build(),
        |env, detail| {
            let tool = cstring("weather");
            let args = cstring(r#"{"city":"x"}"#);
            let mut out: *mut std::os::raw::c_char = std::ptr::null_mut();
            let t = unsafe { cb_tool_invoke(env, tool.as_ptr(), args.as_ptr(), &mut out, detail) };
            assert!(!out.is_null());
            let text = unsafe { std::ffi::CStr::from_ptr(out).to_string_lossy().into_owned() };
            unsafe { cb_free_str(out) };
            assert_eq!(text, "invoked:weather");
            t
        },
    );
    assert_eq!(tag, host_err::OK, "detail: {detail:?}");

    // 失败：错误文本截断后转 UNAVAILABLE detail。
    let (tag, detail) = run_shim(
        CapsBuilder::new()
            .invoker(Arc::new(StubInvoker { ok: false }))
            .tokio(rt.handle().clone())
            .build(),
        |env, detail| {
            let tool = cstring("exec");
            let args = cstring("{}");
            let mut out: *mut std::os::raw::c_char = std::ptr::null_mut();
            unsafe { cb_tool_invoke(env, tool.as_ptr(), args.as_ptr(), &mut out, detail) }
        },
    );
    assert_eq!(tag, host_err::UNAVAILABLE, "detail: {detail:?}");
    assert!(detail.unwrap_or_default().contains("tool exploded"));
}

#[test]
fn guard_converts_panic_to_unavailable() {
    // invoker panic → catch_unwind 护栏 → UNAVAILABLE + panic 摘要（不 unwind
    // 外泄，壳侧不崩）。
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let (tag, detail) = run_shim(
        CapsBuilder::new()
            .invoker(Arc::new(PanickingInvoker))
            .tokio(rt.handle().clone())
            .build(),
        |env, detail| {
            let tool = cstring("exec");
            let args = cstring("{}");
            let mut out: *mut std::os::raw::c_char = std::ptr::null_mut();
            unsafe { cb_tool_invoke(env, tool.as_ptr(), args.as_ptr(), &mut out, detail) }
        },
    );
    assert_eq!(tag, host_err::UNAVAILABLE);
    let d = detail.unwrap_or_default();
    assert!(d.contains("host callback panic"), "panic 摘要: {d}");
    assert!(d.contains("invoker exploded"));
}

// -- C 串帮助：出参串 NUL 替换 + 入参空指针 -------------------------------------

#[test]
fn output_string_nul_replaced_and_null_args_tolerated() {
    // string_to_cstr：内嵌 NUL → 替换符（CString 不能带内嵌 NUL）。经
    // config_get 的值回传面驱动（值含 \0 → 出参串出现替换符）。
    let mut caps = CapsBuilder::new().build();
    let cfg = Arc::get_mut(&mut caps.config).expect("unique");
    cfg.entries
        .insert("base_url".into(), "before\0after".into());
    let (tag, _detail) = run_shim(caps, |env, _detail| {
        let key = cstring("base_url");
        let mut found: i32 = -1;
        let mut out_value: *mut std::os::raw::c_char = std::ptr::null_mut();
        let t = unsafe { cb_config_get(env, key.as_ptr(), &mut found, &mut out_value, _detail) };
        assert_eq!(found, 1);
        assert!(!out_value.is_null());
        // SAFETY: out_value 来自 string_to_cstr（NUL 结尾缓冲）。
        let v = unsafe {
            std::ffi::CStr::from_ptr(out_value)
                .to_string_lossy()
                .into_owned()
        };
        unsafe { cb_free_str(out_value) };
        assert_eq!(v, "before\u{FFFD}after");
        t
    });
    assert_eq!(tag, host_err::OK);

    // cstr_to_string：空指针入参 = 空串（经 shim 空键驱动，不 panic）。
    let (tag, detail) = run_shim(CapsBuilder::new().build(), |env, detail| {
        let mut found: i32 = -1;
        let mut out_value: *mut std::os::raw::c_char = std::ptr::null_mut();
        let detail_ptr: *mut *mut std::os::raw::c_char = detail;
        // SAFETY: 全空指针入参走各空指针早退臂（cstr_to_string(null) = 空串）。
        unsafe {
            cb_config_get(
                env,
                std::ptr::null(),
                &mut found,
                &mut out_value,
                detail_ptr,
            )
        }
    });
    assert_eq!(tag, host_err::NOT_FOUND, "空串键 = 未声明 → not-found");
    assert!(detail.unwrap_or_default().contains("key not declared: "));
}
