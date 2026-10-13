//! Failover system for LLM providers.

use thiserror::Error;

/// Reason for provider failover.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FailoverReason {
    Auth,
    RateLimit,
    Billing,
    Timeout,
    Format,
    Overloaded,
    Unknown,
}

use serde::{Deserialize, Serialize};

/// Failover error with context.
#[derive(Debug, Clone, Error)]
pub enum FailoverError {
    #[error("auth failure for provider {provider}/{model}: status {status}")]
    Auth {
        provider: String,
        model: String,
        status: u16,
    },
    #[error("rate limited by provider {provider}/{model}")]
    RateLimit {
        provider: String,
        model: String,
        retry_after: Option<u64>,
    },
    #[error("billing issue with provider {provider}")]
    Billing { provider: String },
    #[error("timeout calling provider {provider}/{model}")]
    Timeout { provider: String, model: String },
    #[error("format error from provider {provider}: {message}")]
    Format { provider: String, message: String },
    #[error("provider {provider} is overloaded (status: {status})")]
    Overloaded { provider: String, status: u16 },
    #[error("unknown error from provider {provider}: {message}")]
    Unknown { provider: String, message: String },
}

impl FailoverError {
    /// Check if this error is retriable with a different provider.
    pub fn is_retriable(&self) -> bool {
        match self {
            FailoverError::RateLimit { .. }
            | FailoverError::Timeout { .. }
            | FailoverError::Overloaded { .. } => true,
            // 2026-10-13 诚实化配套：`transport_error` 的传输层连接失败
            // （DNS/连接拒绝/SYN 黑洞/中途断流）落 Unknown 但带 "connect
            // error" 前缀——与 Timeout 同属瞬态故障，fallback 链与路由
            // 必须照常轮转（此前伪装 Timeout 时 is_retriable 恒 true，
            // 诚实化若不补这条，主端点连不上就不再故障转移 = 回归）。
            // 其余 Unknown（HTTP 4xx/500 等应用层错误）维持不可重试。
            FailoverError::Unknown { message, .. } => message.starts_with("connect error"),
            _ => false,
        }
    }

    /// Get the failover reason.
    pub fn reason(&self) -> FailoverReason {
        match self {
            FailoverError::Auth { .. } => FailoverReason::Auth,
            FailoverError::RateLimit { .. } => FailoverReason::RateLimit,
            FailoverError::Billing { .. } => FailoverReason::Billing,
            FailoverError::Timeout { .. } => FailoverReason::Timeout,
            FailoverError::Format { .. } => FailoverReason::Format,
            FailoverError::Overloaded { .. } => FailoverReason::Overloaded,
            FailoverError::Unknown { .. } => FailoverReason::Unknown,
        }
    }

    /// Get the `retry_after` hint carried by this error (RateLimit only).
    ///
    /// J1：`RateLimit` 变体可携带服务端 `Retry-After` 秒数；转成 `Duration`
    /// 供 `CooldownTracker::mark_failure` 作退避 hint（其余变体恒 None）。
    pub fn retry_after_hint(&self) -> Option<std::time::Duration> {
        match self {
            FailoverError::RateLimit {
                retry_after: Some(secs),
                ..
            } => Some(std::time::Duration::from_secs(*secs)),
            _ => None,
        }
    }

    /// Create from HTTP status code.
    ///
    /// `retry_after`：调用方从响应头解析的 `Retry-After` 秒数（见
    /// [`retry_after_from_headers`]）；None = 响应未携带（退避走公式）。
    pub fn from_status(
        provider: &str,
        model: &str,
        status: u16,
        body: &str,
        retry_after: Option<u64>,
    ) -> Self {
        match status {
            401 | 403 => FailoverError::Auth {
                provider: provider.to_string(),
                model: model.to_string(),
                status,
            },
            429 => FailoverError::RateLimit {
                provider: provider.to_string(),
                model: model.to_string(),
                retry_after,
            },
            402 => FailoverError::Billing {
                provider: provider.to_string(),
            },
            503 | 502 => FailoverError::Overloaded {
                provider: provider.to_string(),
                // 真实上游状态码随错误携带（2026-10-13）：此前只带 provider，
                // 502（中转转发失败）与 503 分不清，归因盲点。Display 的
                // "(status: NNN)" 形态可被 llm_error_class::extract_http_status
                // 还原——展平文本回灌分类器时不丢状态码。
                status,
            },
            _ => FailoverError::Unknown {
                provider: provider.to_string(),
                message: format!(
                    "status {}: {}",
                    status,
                    body.chars().take(200).collect::<String>()
                ),
            },
        }
    }
}

/// Parse a raw `Retry-After` header value into seconds (RFC 7231 §7.1.3).
///
/// 两种合法形态：延迟秒数（`"30"`）或 HTTP-date（`"Wed, 21 Oct 2026 07:28:00 GMT"`，
/// RFC 2822 格式，按当前墙钟换算成剩余秒数，已过去的日期 → `Some(0)` 立即可重试）。
/// 解析失败 / 值缺失 → `None`（退避走公式，不猜）。
pub fn parse_retry_after(value: Option<&str>) -> Option<u64> {
    let raw = value?.trim();
    if raw.is_empty() {
        return None;
    }
    // 形态 1：延迟秒数。
    if let Ok(secs) = raw.parse::<u64>() {
        return Some(secs);
    }
    // 形态 2：HTTP-date（IMF-fixdate，RFC 2822 同构）。
    let target = chrono::DateTime::parse_from_rfc2822(raw).ok()?;
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Some(target.timestamp().max(0) as u64).map(|t| t.saturating_sub(now_secs))
}

/// Extract + parse the `Retry-After` header from a response header map.
pub fn retry_after_from_headers(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    parse_retry_after(headers.get("retry-after").and_then(|v| v.to_str().ok()))
}

/// Map a reqwest transport error to a `FailoverError`, honestly.
///
/// 2026-10-13 根修（gpt-5.6-sol 风暴排查副产品）：此前各 lane 把**一切**
/// `send()`/流读错误一律 `.map_err(|_| Timeout)`——DNS 失败、连接拒绝、
/// SYN 黑洞（Windows TCP 重传 ~21s）全部伪装成「超时」，误导排查方向
/// （真超时=上游慢；连接失败=地址/网络错，处置与文案完全不同）。现在：
/// - `reqwest::Error::is_timeout()`（客户端级真超时，含 connect_timeout）
///   → `Timeout`（语义不变，瞬态可重试）；
/// - 其余传输错误 → `Unknown`，message 以 **"connect error: "** 前缀 +
///   完整 source 链（DNS/连接/OS 根因原文）。该前缀是 agent loop transient
///   词表成员（"connect error"），也是 `llm_error_class` 的早判分支
///   （先于 auth/format，防 URL 孤立数字误分类）——可重试归宿有双保险。
pub fn transport_error(provider: &str, model: &str, e: &reqwest::Error) -> FailoverError {
    if e.is_timeout() {
        return FailoverError::Timeout {
            provider: provider.to_string(),
            model: model.to_string(),
        };
    }
    // Unknown 变体只带 provider + message（Timeout 才带 model 槽位）——把
    // model 编进 message 保住可观测性与「空模型名回落 default_model」的
    // 可断言性（"connect error" 前缀保持在最前，词表/分类器锚定它）。
    FailoverError::Unknown {
        provider: provider.to_string(),
        message: format!("connect error (model {}): {}", model, error_chain(e)),
    }
}

/// Walk `std::error::Error::source()` chain and join with ": "——reqwest 的
/// `Display` 只有顶层（"error sending request for url (...)"），真实根因
/// （hyper/tcp/OS 文案）藏在 source 链里。
fn error_chain(e: &dyn std::error::Error) -> String {
    let mut parts = vec![e.to_string()];
    let mut src = e.source();
    while let Some(s) = src {
        parts.push(s.to_string());
        src = s.source();
    }
    parts.join(": ")
}

#[cfg(test)]
mod tests;
