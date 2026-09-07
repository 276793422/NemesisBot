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
#[derive(Debug, Error)]
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
    #[error("provider {provider} is overloaded")]
    Overloaded { provider: String },
    #[error("unknown error from provider {provider}: {message}")]
    Unknown { provider: String, message: String },
}

impl FailoverError {
    /// Check if this error is retriable with a different provider.
    pub fn is_retriable(&self) -> bool {
        matches!(
            self,
            FailoverError::RateLimit { .. }
                | FailoverError::Timeout { .. }
                | FailoverError::Overloaded { .. }
        )
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

#[cfg(test)]
mod tests;
