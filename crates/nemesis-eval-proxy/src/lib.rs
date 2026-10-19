//! nemesis-eval-proxy — local LLM proxy for the `nemesisbot eval` sandbox.
//!
//! Purpose (plan Step 3): let the sandboxed eval-agent call the real cloud
//! LLM with a FAKE key. The proxy is the only holder of the real key (in
//! memory), transparently swapping the auth headers on every request:
//!
//! ```text
//! sandbox agent (fake key) → 127.0.0.1:{port} → [proxy] → real endpoint (real key)
//! ```
//!
//! Design constraints (user-approved):
//! - **Pure pass-through**: path, query, body and headers are forwarded
//!   verbatim; the proxy never parses the protocol, so it works with any
//!   provider (OpenAI-compatible, Anthropic native, ...). SSE responses are
//!   forwarded byte-by-byte.
//! - **Not an open proxy**: only the single real endpoint configured at
//!   construction is ever contacted — a malicious skill inside the sandbox
//!   cannot use it as an SSRF springboard.
//! - **Per-launch client token** (A3): the proxy enforces that every request
//!   carries the per-launch random fake key issued for this eval run —
//!   missing or mismatching auth is 401'd without touching the upstream.
//!   Previously the proxy accepted ANY (or no) auth and statically hardcoded
//!   `"eval-fake-key"` on the agent side, so any local process that saw the
//!   loopback port could ride the real key for the eval's lifetime.
//! - Lifecycle == eval lifecycle: dropped when eval ends; the real key never
//!   touches disk.

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderName, HeaderValue, Method, Request, Response, StatusCode};
use axum::routing::any;
use futures::TryStreamExt;

/// Headers stripped from the incoming request before forwarding: hop-by-hop
/// headers plus the fake auth headers (replaced with the real key).
const STRIP_HEADERS: [&str; 6] = [
    "host",
    "authorization",
    "x-api-key",
    "content-length",
    "connection",
    "transfer-encoding",
];

/// Auth headers that carry provider credentials — replaced with the real key.
const AUTH_HEADERS: [&str; 2] = ["authorization", "x-api-key"];

#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    #[error("proxy io: {0}")]
    Io(#[from] std::io::Error),
    #[error("upstream request failed: {0}")]
    Upstream(#[from] reqwest::Error),
    #[error("invalid upstream response: {0}")]
    BadUpstream(&'static str),
}

/// A running proxy handle. Drop semantics: callers should call `shutdown()`
/// (or drop the handle) when the eval finishes.
pub struct ProxyHandle {
    pub port: u16,
    shutdown: tokio::sync::oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
}

impl ProxyHandle {
    /// The `api_base` value to write into the sandboxed agent's config.
    ///
    /// 带 `/v1` 后缀是 HttpCompat（chat/completions）lane 的约定；Anthropic
    /// lane（provider 拼 `/v1/messages`）与 Codex lane（拼 `/responses`）写入
    /// 盒内 config 前必须去掉 `/v1`（否则盒内路径 `/v1/v1/messages` → 上游
    /// 404，2026-09-11 r10 真链路红的根因）。lane 裁决见 eval.rs `box_api_base`。
    pub fn api_base(&self) -> String {
        format!("http://127.0.0.1:{}/v1", self.port)
    }

    /// Stop the proxy and wait for the task to finish.
    pub async fn shutdown(self) {
        let _ = self.shutdown.send(());
        let _ = self.task.await;
    }
}

/// Shared immutable proxy state: the one real endpoint and the real key.
#[derive(Clone)]
struct ProxyState {
    /// Base URL of the real endpoint, e.g. `https://api.example.com` —
    /// WITHOUT a trailing slash. The incoming path+query is appended verbatim.
    real_base: String,
    /// The real API key (memory only — never written to disk).
    api_key: String,
    /// Per-launch client token (A3)：本次 eval 发给盒内 agent 的随机 fake
    /// key——incoming auth 必须精确匹配它才转发（缺失/不匹配 = 401）。
    expected_client_key: String,
    client: reqwest::Client,
}

/// Start the proxy on a random loopback port.
///
/// - `real_base`: scheme://host[:port] of the real LLM endpoint (no path).
/// - `api_key`:   the real key, substituted into auth headers.
/// - `expected_client_key`: the per-launch random fake key issued to the
///   sandboxed agent (A3) — requests not carrying it are rejected 401.
pub async fn start(
    real_base: String,
    api_key: String,
    expected_client_key: String,
) -> Result<ProxyHandle, ProxyError> {
    let state = ProxyState {
        real_base: real_base.trim_end_matches('/').to_string(),
        api_key,
        expected_client_key,
        client: reqwest::Client::builder().build()?,
    };

    let app = Router::new()
        .route("/{*path}", any(proxy_handler))
        .fallback(any(proxy_handler)) // root path without a trailing segment
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();

    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        let server = axum::serve(listener, app);
        tokio::select! {
            _ = server => {}
            _ = rx => {}
        }
    });

    Ok(ProxyHandle {
        port,
        shutdown: tx,
        task,
    })
}

/// Core handler: pure pass-through with auth-header substitution.
async fn proxy_handler(State(state): State<ProxyState>, req: Request<Body>) -> Response<Body> {
    // Split into parts first so headers survive the body extraction.
    let (parts, body) = req.into_parts();
    match forward_with_headers(
        &state,
        &parts.method,
        parts.uri.path(),
        parts.uri.query(),
        &parts.headers,
        body,
    )
    .await
    {
        Ok(resp) => resp,
        Err(e) => {
            tracing::warn!("[eval-proxy] forward error: {e}");
            Response::builder()
                .status(StatusCode::BAD_GATEWAY)
                .body(Body::from(format!("eval-proxy: {e}")))
                .unwrap()
        }
    }
}

/// 从 incoming headers 提取客户端凭据（authorization 的 Bearer token 或
/// x-api-key 裸值；A3 闸比对用）。
fn client_key_from_headers(headers: &axum::http::HeaderMap) -> Option<String> {
    for h in AUTH_HEADERS {
        if let Some(v) = headers.get(h).and_then(|v| v.to_str().ok()) {
            let bare = v.strip_prefix("Bearer ").unwrap_or(v);
            return Some(bare.to_string());
        }
    }
    None
}

async fn forward_with_headers(
    state: &ProxyState,
    method: &Method,
    uri_path: &str,
    uri_query: Option<&str>,
    headers: &axum::http::HeaderMap,
    body: Body,
) -> Result<Response<Body>, ProxyError> {
    // A3 闸：incoming auth 必须携带本次 eval 的每启动随机 fake key——缺失
    // 或不匹配一律 401，不触碰上游。防本地旁观进程摸到环回端口蹭真 key
    // （此前对任意/缺失 auth 无条件放行转发）。
    match client_key_from_headers(headers) {
        Some(k) if k == state.expected_client_key => {}
        _ => {
            tracing::warn!("[eval-proxy] request without valid per-launch token — 401");
            return Ok(Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .body(Body::from("eval-proxy: unauthorized"))
                .map_err(|_| ProxyError::BadUpstream("build response"))?);
        }
    }

    let query = uri_query.map(|q| format!("?{q}")).unwrap_or_default();
    let url = format!("{}{}{}", state.real_base, uri_path, query);

    let req_method = reqwest::Method::from_bytes(method.as_str().as_bytes())
        .map_err(|_| ProxyError::BadUpstream("method"))?;

    let mut out_req = state.client.request(req_method, &url);

    // Copy headers verbatim, stripping hop-by-hop + fake auth headers.
    for (name, value) in headers.iter() {
        let lower = name.as_str().to_ascii_lowercase();
        if STRIP_HEADERS.contains(&lower.as_str()) {
            continue;
        }
        if let Ok(v) = HeaderValue::from_bytes(value.as_bytes()) {
            out_req = out_req.header(name.as_str(), v);
        }
    }

    // Substitute the REAL key into the auth headers. Authorization is always
    // rewritten as "Bearer <real key>" (OpenAI-compatible providers expect
    // the Bearer scheme; sending the bare key earned 401s). x-api-key
    // (Anthropic-style) carries the bare key. （A3 闸已保证两个 auth 头至
    // 少一个在场——此前的「无 auth 头自动补」分支随闸移除。）
    for h in AUTH_HEADERS {
        if headers.contains_key(h) {
            let value = if h == "authorization" {
                format!("Bearer {}", state.api_key)
            } else {
                state.api_key.clone()
            };
            out_req = out_req.header(h, value);
        }
    }

    // Body as a byte stream (keeps pass-through semantics for any content).
    let stream = body.into_data_stream().map_err(std::io::Error::other);
    out_req = out_req.body(reqwest::Body::wrap_stream(stream));

    let resp = out_req.send().await?;

    // Convert the upstream response back into an axum response, streaming the
    // body (SSE works because nothing buffers the whole payload).
    let mut builder =
        Response::builder().status(StatusCode::from_u16(resp.status().as_u16()).unwrap());
    for (name, value) in resp.headers().iter() {
        let lower = name.as_str().to_ascii_lowercase();
        if STRIP_HEADERS.contains(&lower.as_str()) {
            continue;
        }
        if let (Ok(n), Ok(v)) = (
            HeaderName::from_bytes(name.as_str().as_bytes()),
            HeaderValue::from_bytes(value.as_bytes()),
        ) {
            builder = builder.header(n, v);
        }
    }

    let body_stream = resp.bytes_stream().map_err(std::io::Error::other);
    let body = Body::from_stream(body_stream);

    builder
        .body(body)
        .map_err(|_| ProxyError::BadUpstream("build response"))
}

// Convenience re-export for tests / future monitoring (v2).
#[cfg(test)]
mod tests;
