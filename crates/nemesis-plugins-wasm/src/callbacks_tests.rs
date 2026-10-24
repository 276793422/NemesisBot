//! cb_llm_complete 帧纪律回归锁（2026-10-24 BUG-HUNT R70）。
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

use std::sync::Arc;

use crate::abi::host_err;
use crate::callbacks::{cb_free_str, cb_llm_complete};
use crate::host_impl::{
    CallCaps, FrameBudget, InstanceConfig, LlmInvoker, PluginLogBuffer, SecretResolver,
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
            .contains("only available in policy frames")
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
