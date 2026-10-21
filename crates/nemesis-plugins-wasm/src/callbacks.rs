//! 宿主回调 shim（8 个 `extern "C"` 函数 → [`CallCaps`] 策略核心）。
//!
//! 策略语义从原 bindgen Host 实现原样搬移（帧预算/observe 帧收窄/审计/脱敏），
//! 唯一形态变化：两 world 的 bindgen 类型差异由「统一 observer-frame 规则」
//! 收敛——tool world 的实现里 secret-get/tool_invoke 本就检查
//! `observer_frame → unavailable`，observer world 恒 `observer_frame=true`，
//! 一条规则同时复刻两个 world 的 bindgen 行为。
//!
//! FFI 纪律：每个 shim `catch_unwind`（panic → host_err::UNAVAILABLE +
//! detail，禁止 unwind 外泄）；入参 C 串只读不释放；出参串 `CString::into_raw`
//! 由壳经 `free_str` 归还。

use std::os::raw::{c_char, c_int, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};

use base64::Engine as _;

use crate::abi::{HostCallbacks, host_err};
use crate::host_impl::{CallCaps, unix_millis};

/// 回调表（engine_init 期传给壳；static 生命周期——宿主进程存活期有效）。
pub(crate) fn callbacks_table() -> HostCallbacks {
    HostCallbacks {
        free_str: cb_free_str,
        log: cb_log,
        now_millis: cb_now_millis,
        config_get: cb_config_get,
        secret_get: cb_secret_get,
        workspace_read: cb_workspace_read,
        data_dir_path: cb_data_dir_path,
        http_send: cb_http_send,
        tool_invoke: cb_tool_invoke,
        struct_size: std::mem::size_of::<HostCallbacks>(),
    }
}

// ---------------------------------------------------------------------------
// C 串帮助
// ---------------------------------------------------------------------------

/// `*const c_char` → String（空指针 = 空串；UTF-8 失败有损替换）。
///
/// # Safety
/// 指针必须指向合法 NUL 结尾缓冲（壳契约）。
unsafe fn cstr_to_string(p: *const c_char) -> String {
    unsafe {
        if p.is_null() {
            return String::new();
        }
        std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}

/// String → 壳侧堆串（消费方 = 壳，经 free_str 归还）。
fn string_to_cstr(s: &str) -> *mut c_char {
    let s = if s.contains('\0') {
        s.replace('\0', "\u{FFFD}")
    } else {
        s.to_string()
    };
    match std::ffi::CString::new(s) {
        Ok(c) => c.into_raw(),
        Err(_) => std::ffi::CString::new(String::new())
            .expect("empty string is valid CString")
            .into_raw(),
    }
}

/// env 指针 → CallCaps 引用（runtime 保证 store 存活期内 env 有效）。
///
/// # Safety
/// env 必须来自 `Box::into_raw(Box<CallCaps>)` 且未被回收。
unsafe fn caps_of<'a>(env: *mut c_void) -> &'a CallCaps {
    unsafe { &*(env as *const CallCaps) }
}

/// 写错误 detail 出参。
fn put_detail(out: *mut *mut c_char, detail: &str) {
    if !out.is_null() && !detail.is_empty() {
        unsafe { *out = string_to_cstr(detail) };
    }
}

/// panic 护栏包裹：Ok 走策略核心，Err(p) = UNAVAILABLE + panic 摘要。
fn guard(out_detail: *mut *mut c_char, f: impl FnOnce() -> (c_int, Option<String>)) -> c_int {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok((tag, detail)) => {
            if tag != host_err::OK
                && let Some(d) = detail.as_deref()
            {
                put_detail(out_detail, d);
            }
            tag
        }
        Err(p) => {
            let msg = if let Some(s) = p.downcast_ref::<&str>() {
                (*s).to_string()
            } else if let Some(s) = p.downcast_ref::<String>() {
                s.clone()
            } else {
                "opaque panic".into()
            };
            put_detail(out_detail, &format!("host callback panic: {msg}"));
            host_err::UNAVAILABLE
        }
    }
}

// ---------------------------------------------------------------------------
// 8 个 shim
// ---------------------------------------------------------------------------

/// # Safety
/// 壳契约：s 必须来自本宿主 `string_to_cstr`。
pub(crate) unsafe extern "C" fn cb_free_str(s: *mut c_char) {
    unsafe {
        if !s.is_null() {
            drop(std::ffi::CString::from_raw(s));
        }
    }
}

unsafe extern "C" fn cb_log(env: *mut c_void, level: c_int, message: *const c_char) {
    unsafe {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let caps = caps_of(env);
            let level_s = match level {
                0 => "trace",
                1 => "debug",
                2 => "info",
                3 => "warn",
                _ => "error",
            };
            caps.log_line(level_s, &cstr_to_string(message));
        }));
        // log 无返回通道；panic 只能静默（tracing 留痕除外——同样可能二次 panic，
        // 保持静默与旧 bindgen 行为一致）。
        let _ = r;
    }
}

unsafe extern "C" fn cb_now_millis(env: *mut c_void, out_ms: *mut u64) -> c_int {
    unsafe {
        let ms = catch_unwind(AssertUnwindSafe(|| {
            let caps = caps_of(env);
            if caps.frame.consume().is_err() {
                // 预算耗尽：退化为 0（guest 拿到语义可辨的时间原点）——
                // 无 result 返回型的函数不能在此回 host-error。
                0
            } else {
                unix_millis()
            }
        }))
        .unwrap_or(0);
        if !out_ms.is_null() {
            *out_ms = ms;
        }
        host_err::OK
    }
}

unsafe extern "C" fn cb_config_get(
    env: *mut c_void,
    key: *const c_char,
    out_found: *mut c_int,
    out_value: *mut *mut c_char,
    out_detail: *mut *mut c_char,
) -> c_int {
    guard(out_detail, || unsafe {
        let caps = caps_of(env);
        let key = cstr_to_string(key);
        // ↓↓↓ 策略核心 = 原 host_impl config_get（两 world 同语义）。
        if let Err(e) = caps.frame.consume() {
            return (host_err::BUDGET_EXCEEDED, Some(e));
        }
        if caps.config.x_secret_keys.contains(&key) {
            return (
                host_err::POLICY_DENIED,
                Some("x-secret key is not readable via config-get; use secret-get".to_string()),
            );
        }
        if !caps.config.declared_keys.contains(&key) {
            return (
                host_err::NOT_FOUND,
                Some(format!("key not declared: {key}")),
            );
        }
        match caps.config.entries.get(&key) {
            Some(v) => {
                if !out_value.is_null() {
                    *out_value = string_to_cstr(v);
                }
                if !out_found.is_null() {
                    *out_found = 1;
                }
                (host_err::OK, None)
            }
            None => (host_err::OK, None), // declared but unset = found=0
        }
    })
}

unsafe extern "C" fn cb_secret_get(
    env: *mut c_void,
    name: *const c_char,
    out_value: *mut *mut c_char,
    out_detail: *mut *mut c_char,
) -> c_int {
    guard(out_detail, || unsafe {
        let caps = caps_of(env);
        let name = cstr_to_string(name);
        // 统一 observer-frame 规则（tool world 显式检查 + observer world 恒
        // 真的合并形态）。
        if let Err(e) = caps.frame.consume() {
            return (host_err::BUDGET_EXCEEDED, Some(e));
        }
        if caps.observer_frame {
            return (
                host_err::UNAVAILABLE,
                Some("secret-get is unavailable in observer frames".to_string()),
            );
        }
        if !caps.config.x_secret_keys.contains(&name) {
            caps.audit_event(
                "plugin_secret_get",
                "denied",
                &format!("plugin/{}/{}", caps.slug, name),
                "MEDIUM",
                "secret name not declared in manifest x-secret",
            );
            return (
                host_err::POLICY_DENIED,
                Some(format!(
                    "secret name not declared in manifest x-secret: {name}"
                )),
            );
        }
        let alias = format!("vault:plugin/{}/{}", caps.slug, name);
        match caps.secrets.resolve(&alias) {
            Some(v) => {
                // 审计只记别名不记原文。
                caps.audit_event(
                    "plugin_secret_get",
                    "allowed",
                    &alias,
                    "MEDIUM",
                    "secret resolved for wasm plugin",
                );
                if !out_value.is_null() {
                    *out_value = string_to_cstr(&v);
                }
                (host_err::OK, None)
            }
            None => {
                caps.audit_event(
                    "plugin_secret_get",
                    "denied",
                    &alias,
                    "MEDIUM",
                    "no secret stored for alias",
                );
                (
                    host_err::NOT_FOUND,
                    Some(format!("no secret stored for alias {alias}")),
                )
            }
        }
    })
}

unsafe extern "C" fn cb_workspace_read(
    env: *mut c_void,
    rel_path: *const c_char,
    out_b64: *mut *mut c_char,
    out_detail: *mut *mut c_char,
) -> c_int {
    guard(out_detail, || unsafe {
        let caps = caps_of(env);
        let rel = cstr_to_string(rel_path);
        if let Err(e) = caps.frame.consume() {
            return (host_err::BUDGET_EXCEEDED, Some(e));
        }
        match crate::host_impl::read_workspace_rel(
            &caps.workspace_root,
            &rel,
            caps.limits.workspace_read_max_bytes,
        ) {
            Ok(bytes) => {
                caps.audit_event(
                    "plugin_workspace_read",
                    "allowed",
                    &rel,
                    "LOW",
                    "workspace file read by wasm plugin",
                );
                if !out_b64.is_null() {
                    *out_b64 =
                        string_to_cstr(&base64::engine::general_purpose::STANDARD.encode(&bytes));
                }
                (host_err::OK, None)
            }
            Err(e) => {
                caps.audit_event(
                    "plugin_workspace_read",
                    "denied",
                    &rel,
                    "LOW",
                    &crate::host_impl::truncate_err(&e, 100),
                );
                (host_err::POLICY_DENIED, Some(e))
            }
        }
    })
}

unsafe extern "C" fn cb_data_dir_path(
    env: *mut c_void,
    out_path: *mut *mut c_char,
    out_detail: *mut *mut c_char,
) -> c_int {
    guard(out_detail, || unsafe {
        let caps = caps_of(env);
        if let Err(e) = caps.frame.consume() {
            return (host_err::BUDGET_EXCEEDED, Some(e));
        }
        if !out_path.is_null() {
            *out_path = string_to_cstr("/data");
        }
        (host_err::OK, None)
    })
}

/// http-request 入参 JSON 形态（与壳 `call_http_send` 的序列化对偶）：
/// `{"method","url","headers":[[k,v],…],"body":b64|null}`。
unsafe extern "C" fn cb_http_send(
    env: *mut c_void,
    request_json: *const c_char,
    out_response_json: *mut *mut c_char,
    out_detail: *mut *mut c_char,
) -> c_int {
    guard(out_detail, || unsafe {
        let caps = caps_of(env);
        let raw = cstr_to_string(request_json);
        let req: serde_json::Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => {
                return (
                    host_err::UNAVAILABLE,
                    Some(format!("http request decode failed: {e}")),
                );
            }
        };
        let method = req
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("GET")
            .to_string();
        let url = req
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let mut headers: Vec<(String, String)> = Vec::new();
        if let Some(hs) = req.get("headers").and_then(|v| v.as_array()) {
            for h in hs {
                if let Some(pair) = h.as_array() {
                    let k = pair.first().and_then(|v| v.as_str()).unwrap_or_default();
                    let w = pair.get(1).and_then(|v| v.as_str()).unwrap_or_default();
                    headers.push((k.to_string(), w.to_string()));
                }
            }
        }
        let body = req
            .get("body")
            .and_then(|v| v.as_str())
            .and_then(|s| base64::engine::general_purpose::STANDARD.decode(s).ok());
        if let Err(e) = caps.frame.consume() {
            return (host_err::BUDGET_EXCEEDED, Some(e));
        }
        let policy = caps.egress.clone();
        let limits = caps.limits.clone();
        let handle = caps.tokio_handle.clone();
        if !policy.enabled {
            caps.audit_event(
                "plugin_http_send",
                "denied",
                &url,
                "MEDIUM",
                "egress not granted (deny-by-default)",
            );
            return (host_err::NO_PERMISSION, None);
        }
        let Some(h) = handle else {
            // 无 tokio 上下文（罕见：headless 装配遗漏）；fail-closed。
            return (
                host_err::UNAVAILABLE,
                Some("egress runtime unavailable".to_string()),
            );
        };
        let fut = crate::egress::proxy_request(
            &policy,
            &limits,
            crate::egress::EgressRequest {
                method,
                url: url.clone(),
                headers,
                body,
            },
        );
        match h.block_on(fut) {
            Ok(resp) => {
                caps.audit_event(
                    "plugin_http_send",
                    "allowed",
                    &url,
                    "MEDIUM",
                    &format!(
                        "egress via wasm plugin{}, status {}",
                        if caps.observer_frame {
                            " (observer)"
                        } else {
                            ""
                        },
                        resp.status
                    ),
                );
                let headers: Vec<Vec<&str>> = resp
                    .headers
                    .iter()
                    .map(|(k, v)| vec![k.as_str(), v.as_str()])
                    .collect();
                let body_b64 = base64::engine::general_purpose::STANDARD.encode(&resp.body);
                let json = serde_json::json!({
                    "status": resp.status,
                    "headers": headers,
                    "body": body_b64,
                });
                if !out_response_json.is_null() {
                    *out_response_json = string_to_cstr(&json.to_string());
                }
                (host_err::OK, None)
            }
            Err(e) => {
                caps.audit_event(
                    "plugin_http_send",
                    "denied",
                    &url,
                    "MEDIUM",
                    &crate::host_impl::truncate_err(&e, 100),
                );
                (host_err::POLICY_DENIED, Some(e))
            }
        }
    })
}

unsafe extern "C" fn cb_tool_invoke(
    env: *mut c_void,
    tool: *const c_char,
    args_json: *const c_char,
    out_content: *mut *mut c_char,
    out_detail: *mut *mut c_char,
) -> c_int {
    guard(out_detail, || unsafe {
        let caps = caps_of(env);
        let tool = cstr_to_string(tool);
        let args = cstr_to_string(args_json);
        if let Err(e) = caps.frame.consume() {
            return (host_err::BUDGET_EXCEEDED, Some(e));
        }
        if caps.observer_frame {
            return (
                host_err::UNAVAILABLE,
                Some("tool-invoke is unavailable in observer frames".to_string()),
            );
        }
        if tool.starts_with(crate::PLUGIN_TOOL_PREFIX) {
            return (
                host_err::POLICY_DENIED,
                Some("plugin tools cannot invoke plugin tools (depth 1)".to_string()),
            );
        }
        let Some(invoker) = caps.invoker.clone() else {
            return (
                host_err::UNAVAILABLE,
                Some("host tool invoker not wired (plugin host standalone)".to_string()),
            );
        };
        let Some(handle) = caps.tokio_handle.clone() else {
            return (
                host_err::UNAVAILABLE,
                Some("no tokio runtime for tool dispatch".to_string()),
            );
        };
        match handle.block_on(invoker.invoke(&tool, &args)) {
            Ok(text) => {
                if !out_content.is_null() {
                    *out_content = string_to_cstr(&text);
                }
                (host_err::OK, None)
            }
            Err(e) => (
                host_err::UNAVAILABLE,
                Some(crate::host_impl::truncate_err(&e, 300)),
            ),
        }
    })
}
