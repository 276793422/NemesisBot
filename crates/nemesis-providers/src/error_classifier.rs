//! Error classification for failover decisions.
//!
//! T2a 下沉（扩展计划 D4-2a）：模式表与纯分类逻辑单点在
//! [`nemesis_utils::llm_error_class`]（agent loop 生产代码同源消费——本 crate
//! 对 nemesis-agent 只有 dev-dependency，分类逻辑若留在这里，loop 只能继续
//! 用自己的词表）。本模块只保留 FailoverError 富化（provider/model/status
//! 载荷）与既有公开 API 形状。

use crate::failover::FailoverError;
use crate::failover::FailoverReason;
use nemesis_utils::llm_error_class::LlmErrorClass;
use nemesis_utils::llm_error_class::classify_llm_error;

/// Classify an error string into a `FailoverError` with reason.
/// Returns `None` if the error is not classifiable (unknown errors should not trigger fallback).
pub fn classify_error(error_msg: &str, provider: &str, model: &str) -> Option<FailoverError> {
    let class = classify_llm_error(error_msg)?;
    let msg = error_msg.to_lowercase();
    Some(match class {
        LlmErrorClass::Auth => FailoverError::Auth {
            provider: provider.to_string(),
            model: model.to_string(),
            // 状态码形态带真值；纯文案形态（原实现约定）= 0。
            status: extract_http_status(&msg).unwrap_or(0),
        },
        LlmErrorClass::RateLimit => FailoverError::RateLimit {
            provider: provider.to_string(),
            model: model.to_string(),
            // J1：此处 status 是从错误消息文本里抠出来的，拿不到响应头 → retry_after
            // 维持 None（能拿到 header 的 HTTP 路径已由 retry_after_from_headers 填充）。
            retry_after: None,
        },
        LlmErrorClass::Billing => FailoverError::Billing {
            provider: provider.to_string(),
        },
        LlmErrorClass::Timeout => FailoverError::Timeout {
            provider: provider.to_string(),
            model: model.to_string(),
        },
        LlmErrorClass::Format => FailoverError::Format {
            provider: provider.to_string(),
            message: error_msg.to_string(),
        },
        LlmErrorClass::Overloaded => FailoverError::Overloaded {
            provider: provider.to_string(),
        },
    })
}

/// Get the `FailoverReason` for a classified error message.
/// Returns `None` if not classifiable.
pub fn classify_reason(error_msg: &str) -> Option<FailoverReason> {
    Some(match classify_llm_error(error_msg)? {
        // Overloaded treated as rate_limit (existing convention).
        LlmErrorClass::RateLimit | LlmErrorClass::Overloaded => FailoverReason::RateLimit,
        LlmErrorClass::Billing => FailoverReason::Billing,
        LlmErrorClass::Timeout => FailoverReason::Timeout,
        LlmErrorClass::Auth => FailoverReason::Auth,
        LlmErrorClass::Format => FailoverReason::Format,
    })
}

/// Check if the message indicates an image dimension error.
pub fn is_image_dimension_error(msg: &str) -> bool {
    nemesis_utils::llm_error_class::is_image_dimension_error(msg)
}

/// Check if the message indicates an image file size error.
pub fn is_image_size_error(msg: &str) -> bool {
    nemesis_utils::llm_error_class::is_image_size_error(msg)
}

/// Extract HTTP status code from error message.
/// （分类器 tests 子模块直测；富化 Auth.status 也用。）
fn extract_http_status(msg: &str) -> Option<u16> {
    nemesis_utils::llm_error_class::extract_http_status(msg)
}

#[cfg(test)]
mod cov_tests;
#[cfg(test)]
mod tests;
