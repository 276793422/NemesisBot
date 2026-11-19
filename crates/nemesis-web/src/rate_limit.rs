//! #25 网关流控（W1 安全图谱批，2026-11-19）。
//!
//! 网关 HTTP 面的两级流控，配置面 `security.rate_limit`
//! （单一真相源 [`nemesis_config::resolve_rate_limit`]；`enabled` 缺省
//! **false** = 全关，既有部署零行为变化）：
//!
//! - **令牌桶 QPS**：`/api/*` REST 控制面全局限流（qps/burst，429 +
//!   `Retry-After`）。挂载在 auth 中间件**外层**——洪水先于鉴权被拒，
//!   不给未认证请求消耗鉴权/处理器资源的机会（CORS 再外一层，
//!   preflight OPTIONS 不计费）。
//! - **SSE 流并发闸**：流端点（chat/events stream）在途并发上限
//!   （`max_concurrent_streams`，0 = 不限）。额度在流体真正结束/断连
//!   时归还（中间件 await 只到响应头，SSE 长连接的 body 在其后继续
//!   流——直接在中间件归还会提前释放，闸形同虚设）。
//!
//! 诚实边界：
//! - **全局桶，非 per-IP**——本网关定位本地单用户；反代场景下 per-IP
//!   信任面复杂（`X-Forwarded-For` 可伪造）。频控目标是意外风暴/脚本
//!   误用（如 MCP 客户端轮询失控），不是对抗性分布式压测。
//! - **WS 消息面不在本层**——WebSocket 长连接里的 `chat.send` 洪水是
//!   websocket_handler 语义（W1 不动，挂账）。
//! - `/health` 与静态资源不计费（前者是外部探活，限它等于自伤探针）。

use futures::StreamExt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

/// 并发闸适用的 SSE 流端点（精确匹配）。
const STREAM_PATHS: [&str; 2] = ["/api/chat/stream", "/api/events/stream"];

/// 桶状态（tokens + 上次补给时刻）。
#[derive(Debug)]
struct BucketState {
    tokens: f64,
    last: Instant,
}

/// 网关级流控器（全局单例；gateway 装配注入 WebServer）。
#[derive(Debug)]
pub struct GatewayRateLimiter {
    enabled: bool,
    qps: f64,
    burst: f64,
    bucket: parking_lot::Mutex<BucketState>,
    /// SSE 流在途计数。
    inflight_streams: AtomicUsize,
    /// 并发上限（0 = 不限）。
    max_streams: u32,
}

/// SSE 在途额度租约：Drop 归还（流体被 drop = 客户端断连/流结束/错误
/// 中断，三态统一走这里——不依赖 handler 记得归还）。
struct StreamLease {
    limiter: Option<std::sync::Arc<GatewayRateLimiter>>,
}

impl Drop for StreamLease {
    fn drop(&mut self) {
        if let Some(l) = self.limiter.take() {
            l.leave_stream();
        }
    }
}

impl GatewayRateLimiter {
    /// 从解析后的配置构造（qps/burst 已在 resolve 层钳下界 1）。
    pub fn new(settings: nemesis_config::RateLimitSettings) -> Self {
        Self {
            enabled: settings.enabled,
            qps: f64::from(settings.qps),
            burst: f64::from(settings.burst),
            bucket: parking_lot::Mutex::new(BucketState {
                tokens: f64::from(settings.burst),
                last: Instant::now(),
            }),
            inflight_streams: AtomicUsize::new(0),
            max_streams: settings.max_concurrent_streams,
        }
    }

    /// 流控总开关（未注入/未启用时 build_router 不挂中间件）。
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// 惰性补给 + 取一个令牌（`/api/*` 每请求一令牌）。
    pub fn allow_request(&self) -> bool {
        if !self.enabled {
            return true;
        }
        let mut s = self.bucket.lock();
        let now = Instant::now();
        let elapsed = now.duration_since(s.last).as_secs_f64();
        if elapsed > 0.0 {
            s.tokens = (s.tokens + elapsed * self.qps).min(self.burst);
            s.last = now;
        }
        if s.tokens >= 1.0 {
            s.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// SSE 并发闸入场（CAS——并发入场不超卖）。`max_streams == 0` =
    /// 不限但仍计数（观测面 + 租约 drop 的 `leave_stream` 必须有对称
    /// 的入场计数，否则 fetch_sub 回绕——不限 ≠ 不计）。
    pub fn try_enter_stream(&self) -> bool {
        if !self.enabled {
            return true;
        }
        let max = self.max_streams as usize;
        let mut cur = self.inflight_streams.load(Ordering::SeqCst);
        loop {
            if self.max_streams > 0 && cur >= max {
                return false;
            }
            match self.inflight_streams.compare_exchange_weak(
                cur,
                cur + 1,
                Ordering::SeqCst,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(actual) => cur = actual,
            }
        }
    }

    /// SSE 并发闸出场（只在 `try_enter_stream` 成功后调用；实际由
    /// [`StreamLease`] Drop 触发）。
    fn leave_stream(&self) {
        self.inflight_streams.fetch_sub(1, Ordering::SeqCst);
    }

    /// 在途 SSE 流数（观测面/测试）。
    pub fn inflight_streams(&self) -> usize {
        self.inflight_streams.load(Ordering::SeqCst)
    }
}

/// 429 响应（`Retry-After: 1` + JSON error 体——前端/脚本可编程消费）。
fn too_many_requests(reason: &str) -> axum::response::Response {
    use axum::response::IntoResponse;
    (
        axum::http::StatusCode::TOO_MANY_REQUESTS,
        [
            (http::header::RETRY_AFTER, "1"),
            (http::header::CONTENT_TYPE, "application/json"),
        ],
        serde_json::json!({ "error": "rate_limited", "reason": reason }).to_string(),
    )
        .into_response()
}

/// 流控中间件本体（`build_router` 以闭包捕获 limiter 挂 `route_layer`）。
pub(crate) async fn rate_limit_middleware(
    limiter: std::sync::Arc<GatewayRateLimiter>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let path = req.uri().path();
    // QPS：/api/* 全 REST 面（静态资源与 /health 不计费）。
    if path.starts_with("/api/") && !limiter.allow_request() {
        tracing::warn!(path = %path, "[WebServer] rate limit: QPS exceeded, 429");
        return too_many_requests("gateway QPS limit exceeded");
    }
    // 并发闸：只对 SSE 流端点。
    let is_stream = STREAM_PATHS.contains(&path);
    if is_stream && !limiter.try_enter_stream() {
        tracing::warn!(path = %path, "[WebServer] rate limit: stream concurrency exceeded, 429");
        return too_many_requests("gateway stream concurrency limit exceeded");
    }
    if !is_stream {
        return next.run(req).await;
    }
    // SSE 长连接：中间件 await 到响应头即返回，body 在其后继续流——
    // 把租约搬进 body 流的闭包环境里，流 drop 时租约 drop 归还额度。
    let resp = next.run(req).await;
    let (parts, body) = resp.into_parts();
    let lease = StreamLease {
        limiter: Some(limiter),
    };
    let stream = body.into_data_stream().map(move |item| {
        let _ = &lease; // 保活捕获：闭包随流 drop，租约随之归还
        item
    });
    axum::response::Response::from_parts(parts, axum::body::Body::from_stream(stream))
}

#[cfg(test)]
mod tests;
