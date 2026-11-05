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
