//! Error classification for failover decisions.

use crate::failover::FailoverError;
use crate::failover::FailoverReason;

/// Classify an error string into a `FailoverError` with reason.
/// Returns `None` if the error is not classifiable (unknown errors should not trigger fallback).
pub fn classify_error(error_msg: &str, provider: &str, model: &str) -> Option<FailoverError> {
    let msg = error_msg.to_lowercase();

    // Image dimension/size errors: non-retriable format error.
    if is_image_dimension_error(&msg) || is_image_size_error(&msg) {
        return Some(FailoverError::Format {
            provider: provider.to_string(),
            message: error_msg.to_string(),
        });
    }

    // Try HTTP status code extraction first.
    if let Some(status) = extract_http_status(&msg) {
        if let Some(err) = classify_by_status(status, provider, model) {
            return Some(err);
        }
    }

    // Message pattern matching (priority order).
    classify_by_message(&msg, provider, model)
}

/// Classify by HTTP status code.
fn classify_by_status(status: u16, provider: &str, model: &str) -> Option<FailoverError> {
    match status {
        401 | 403 => Some(FailoverError::Auth {
            provider: provider.to_string(),
            model: model.to_string(),
            status,
        }),
        402 => Some(FailoverError::Billing {
            provider: provider.to_string(),
        }),
        408 => Some(FailoverError::Timeout {
            provider: provider.to_string(),
            model: model.to_string(),
        }),
        429 => Some(FailoverError::RateLimit {
            provider: provider.to_string(),
            model: model.to_string(),
            retry_after: None,
        }),
        400 => Some(FailoverError::Format {
            provider: provider.to_string(),
            message: format!("status {}", status),
        }),
        500 | 502 | 503 | 521 | 522 | 523 | 524 | 529 => Some(FailoverError::Overloaded {
            provider: provider.to_string(),
        }),
        _ => None,
    }
}

/// Classify by message pattern matching.
fn classify_by_message(msg: &str, provider: &str, model: &str) -> Option<FailoverError> {
    if matches_rate_limit(msg) {
        return Some(FailoverError::RateLimit {
            provider: provider.to_string(),
            model: model.to_string(),
            retry_after: None,
        });
    }
    if matches_overloaded(msg) {
        // Overloaded treated as rate_limit per OpenClaw convention.
        return Some(FailoverError::RateLimit {
            provider: provider.to_string(),
            model: model.to_string(),
            retry_after: None,
        });
    }
    if matches_billing(msg) {
        return Some(FailoverError::Billing {
            provider: provider.to_string(),
        });
    }
    if matches_timeout(msg) {
        return Some(FailoverError::Timeout {
            provider: provider.to_string(),
            model: model.to_string(),
        });
    }
    if matches_auth(msg) {
        return Some(FailoverError::Auth {
            provider: provider.to_string(),
            model: model.to_string(),
            status: 0,
        });
    }
    if matches_format(msg) {
        return Some(FailoverError::Format {
            provider: provider.to_string(),
            message: msg.to_string(),
        });
    }
    None
}

fn matches_rate_limit(msg: &str) -> bool {
    const PATTERNS: &[&str] = &[
        "rate limit",
        "rate_limit",
        "too many requests",
        "429",
        "exceeded your current quota",
        "resource has been exhausted",
        "resource_exhausted",
        "quota exceeded",
        "usage limit",
    ];
    const REGEX_PATTERNS: &[&str] = &[
        r"exceeded.*quota",
        r"resource.*exhausted",
    ];
    contains_any(msg, PATTERNS) || matches_any_regex(msg, REGEX_PATTERNS)
}

fn matches_overloaded(msg: &str) -> bool {
    const PATTERNS: &[&str] = &["overloaded"];
    const REGEX_PATTERNS: &[&str] =
        &[r#"overloaded_error"#, r#""type"\s*:\s*"overloaded_error""#];
    contains_any(msg, PATTERNS) || matches_any_regex(msg, REGEX_PATTERNS)
}

fn matches_timeout(msg: &str) -> bool {
    const PATTERNS: &[&str] = &[
        "timeout",
        "timed out",
        "deadline exceeded",
        "context deadline exceeded",
    ];
    contains_any(msg, PATTERNS)
}

fn matches_billing(msg: &str) -> bool {
    const PATTERNS: &[&str] = &[
        "payment required",
        "insufficient credits",
        "credit balance",
        "plans & billing",
        "insufficient balance",
    ];
    const REGEX_PATTERNS: &[&str] = &[r"\b402\b"];
    contains_any(msg, PATTERNS) || matches_any_regex(msg, REGEX_PATTERNS)
}

fn matches_auth(msg: &str) -> bool {
    const PATTERNS: &[&str] = &[
        "incorrect api key",
        "invalid token",
        "authentication",
        "re-authenticate",
        "oauth token refresh failed",
        "unauthorized",
        "forbidden",
        "access denied",
        "expired",
        "token has expired",
        "no credentials found",
        "no api key found",
    ];
    const REGEX_PATTERNS: &[&str] = &[
        r"invalid[_ ]?api[_ ]?key",
        r"\b401\b",
        r"\b403\b",
    ];
    contains_any(msg, PATTERNS) || matches_any_regex(msg, REGEX_PATTERNS)
}

fn matches_format(msg: &str) -> bool {
    const PATTERNS: &[&str] = &[
        "string should match pattern",
        "tool_use.id",
        "tool_use_id",
        "messages.1.content.1.tool_use.id",
        "invalid request format",
    ];
    contains_any(msg, PATTERNS)
}

/// Check if the message indicates an image dimension error.
pub fn is_image_dimension_error(msg: &str) -> bool {
    matches_any_regex(msg, &[r"image dimensions exceed max"])
}

/// Check if the message indicates an image file size error.
pub fn is_image_size_error(msg: &str) -> bool {
    matches_any_regex(msg, &[r"image exceeds.*mb"])
}

/// Extract HTTP status code from error message.
fn extract_http_status(msg: &str) -> Option<u16> {
    // Look for patterns like "status: 429", "status 429", "HTTP 429"
    let patterns = [
        regex::Regex::new(r"status[:\s]+(\d{3})").ok()?,
        regex::Regex::new(r"HTTP[/\s]+\d*\.?\d*\s+(\d{3})").ok()?,
    ];

    for p in &patterns {
        if let Some(caps) = p.captures(msg) {
            if let Some(m) = caps.get(1) {
                if let Ok(code) = m.as_str().parse::<u16>() {
                    return Some(code);
                }
            }
        }
    }
    None
}

fn contains_any(msg: &str, patterns: &[&str]) -> bool {
    patterns.iter().any(|p| msg.contains(p))
}

fn matches_any_regex(msg: &str, patterns: &[&str]) -> bool {
    patterns.iter().any(|p| {
        regex::Regex::new(&format!("(?i){}", p))
            .map(|re| re.is_match(msg))
            .unwrap_or(false)
    })
}

/// Get the `FailoverReason` for a classified error message.
/// Returns `None` if not classifiable.
pub fn classify_reason(error_msg: &str) -> Option<FailoverReason> {
    let msg = error_msg.to_lowercase();

    if matches_rate_limit(&msg) || matches_overloaded(&msg) {
        return Some(FailoverReason::RateLimit);
    }
    if matches_billing(&msg) {
        return Some(FailoverReason::Billing);
    }
    if matches_timeout(&msg) {
        return Some(FailoverReason::Timeout);
    }
    if matches_auth(&msg) {
        return Some(FailoverReason::Auth);
    }
    if matches_format(&msg) {
        return Some(FailoverReason::Format);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_rate_limit() {
        let err = classify_error("rate limit exceeded", "openai", "gpt-4");
        assert!(matches!(err, Some(FailoverError::RateLimit { .. })));
    }

    #[test]
    fn test_classify_too_many_requests() {
        let err = classify_error("Too Many Requests", "openai", "gpt-4");
        assert!(matches!(err, Some(FailoverError::RateLimit { .. })));
    }

    #[test]
    fn test_classify_overloaded() {
        let err = classify_error("overloaded_error", "anthropic", "claude-3");
        assert!(matches!(err, Some(FailoverError::RateLimit { .. })));
    }

    #[test]
    fn test_classify_timeout() {
        let err = classify_error("request timeout", "openai", "gpt-4");
        assert!(matches!(err, Some(FailoverError::Timeout { .. })));
    }

    #[test]
    fn test_classify_billing() {
        let err = classify_error("payment required", "openai", "gpt-4");
        assert!(matches!(err, Some(FailoverError::Billing { .. })));
    }

    #[test]
    fn test_classify_auth() {
        let err = classify_error("invalid api key", "openai", "gpt-4");
        assert!(matches!(err, Some(FailoverError::Auth { .. })));
    }

    #[test]
    fn test_classify_format() {
        let err = classify_error("invalid request format", "openai", "gpt-4");
        assert!(matches!(err, Some(FailoverError::Format { .. })));
    }

    #[test]
    fn test_classify_unknown_returns_none() {
        let err = classify_error("some random error", "openai", "gpt-4");
        assert!(err.is_none());
    }

    #[test]
    fn test_classify_image_dimension() {
        let err = classify_error("image dimensions exceed max 8000px", "openai", "gpt-4");
        assert!(matches!(err, Some(FailoverError::Format { .. })));
    }

    #[test]
    fn test_classify_image_size() {
        let err = classify_error("image exceeds 20mb", "openai", "gpt-4");
        assert!(matches!(err, Some(FailoverError::Format { .. })));
    }

    #[test]
    fn test_classify_reason() {
        assert_eq!(classify_reason("rate limit"), Some(FailoverReason::RateLimit));
        assert_eq!(classify_reason("timeout"), Some(FailoverReason::Timeout));
        assert_eq!(classify_reason("unknown thing"), None);
    }

    #[test]
    fn test_extract_http_status() {
        assert_eq!(extract_http_status("status: 429"), Some(429));
        assert_eq!(extract_http_status("HTTP/1.1 503 Service"), Some(503));
        assert_eq!(extract_http_status("no status here"), None);
    }

    #[test]
    fn test_expired_token() {
        let err = classify_error("token has expired", "anthropic", "claude-3");
        assert!(matches!(err, Some(FailoverError::Auth { .. })));
    }

    #[test]
    fn test_deadline_exceeded() {
        let err = classify_error("context deadline exceeded", "openai", "gpt-4");
        assert!(matches!(err, Some(FailoverError::Timeout { .. })));
    }

    // ============================================================
    // Additional tests for missing coverage
    // ============================================================

    #[test]
    fn test_classify_rate_limit_resource_exhausted() {
        let err = classify_error("resource has been exhausted", "google", "gemini-pro");
        assert!(matches!(err, Some(FailoverError::RateLimit { .. })));
    }

    #[test]
    fn test_classify_rate_limit_quota_exceeded() {
        let err = classify_error("quota exceeded for this month", "openai", "gpt-4");
        assert!(matches!(err, Some(FailoverError::RateLimit { .. })));
    }

    #[test]
    fn test_classify_rate_limit_usage_limit() {
        let err = classify_error("usage limit reached", "openai", "gpt-4");
        assert!(matches!(err, Some(FailoverError::RateLimit { .. })));
    }

    #[test]
    fn test_classify_rate_limit_429_pattern() {
        let err = classify_error("error 429 too many requests", "openai", "gpt-4");
        assert!(matches!(err, Some(FailoverError::RateLimit { .. })));
    }

    #[test]
    fn test_classify_billing_insufficient_credits() {
        let err = classify_error("insufficient credits", "openai", "gpt-4");
        assert!(matches!(err, Some(FailoverError::Billing { .. })));
    }

    #[test]
    fn test_classify_billing_credit_balance() {
        let err = classify_error("credit balance is low", "openai", "gpt-4");
        assert!(matches!(err, Some(FailoverError::Billing { .. })));
    }

    #[test]
    fn test_classify_billing_plans() {
        let err = classify_error("please check plans & billing", "openai", "gpt-4");
        assert!(matches!(err, Some(FailoverError::Billing { .. })));
    }

    #[test]
    fn test_classify_billing_insufficient_balance() {
        let err = classify_error("insufficient balance", "openai", "gpt-4");
        assert!(matches!(err, Some(FailoverError::Billing { .. })));
    }

    #[test]
    fn test_classify_billing_402() {
        let err = classify_error("HTTP/1.1 402 Payment Required", "openai", "gpt-4");
        assert!(matches!(err, Some(FailoverError::Billing { .. })));
    }

    #[test]
    fn test_classify_auth_incorrect_api_key() {
        let err = classify_error("incorrect api key provided", "openai", "gpt-4");
        assert!(matches!(err, Some(FailoverError::Auth { .. })));
    }

    #[test]
    fn test_classify_auth_invalid_token() {
        let err = classify_error("invalid token", "openai", "gpt-4");
        assert!(matches!(err, Some(FailoverError::Auth { .. })));
    }

    #[test]
    fn test_classify_auth_unauthorized() {
        let err = classify_error("unauthorized access", "openai", "gpt-4");
        assert!(matches!(err, Some(FailoverError::Auth { .. })));
    }

    #[test]
    fn test_classify_auth_forbidden() {
        let err = classify_error("forbidden: access denied", "openai", "gpt-4");
        assert!(matches!(err, Some(FailoverError::Auth { .. })));
    }

    #[test]
    fn test_classify_auth_401() {
        let err = classify_error("status: 401", "openai", "gpt-4");
        assert!(matches!(err, Some(FailoverError::Auth { .. })));
    }

    #[test]
    fn test_classify_auth_403() {
        let err = classify_error("status: 403", "openai", "gpt-4");
        assert!(matches!(err, Some(FailoverError::Auth { .. })));
    }

    #[test]
    fn test_classify_auth_no_credentials() {
        let err = classify_error("no credentials found", "openai", "gpt-4");
        assert!(matches!(err, Some(FailoverError::Auth { .. })));
    }

    #[test]
    fn test_classify_auth_no_api_key() {
        let err = classify_error("no api key found", "openai", "gpt-4");
        assert!(matches!(err, Some(FailoverError::Auth { .. })));
    }

    #[test]
    fn test_classify_timeout_timed_out() {
        let err = classify_error("request timed out after 30s", "openai", "gpt-4");
        assert!(matches!(err, Some(FailoverError::Timeout { .. })));
    }

    #[test]
    fn test_classify_overloaded_500() {
        let err = classify_error("status: 500", "openai", "gpt-4");
        assert!(matches!(err, Some(FailoverError::Overloaded { .. })));
    }

    #[test]
    fn test_classify_overloaded_502() {
        let err = classify_error("status: 502", "openai", "gpt-4");
        assert!(matches!(err, Some(FailoverError::Overloaded { .. })));
    }

    #[test]
    fn test_classify_overloaded_503() {
        let err = classify_error("status: 503", "openai", "gpt-4");
        assert!(matches!(err, Some(FailoverError::Overloaded { .. })));
    }

    #[test]
    fn test_classify_format_tool_use_id() {
        let err = classify_error("tool_use.id is invalid", "anthropic", "claude-3");
        assert!(matches!(err, Some(FailoverError::Format { .. })));
    }

    #[test]
    fn test_classify_format_string_pattern() {
        let err = classify_error("string should match pattern ^[a-z]+$", "openai", "gpt-4");
        assert!(matches!(err, Some(FailoverError::Format { .. })));
    }

    #[test]
    fn test_classify_reason_rate_limit() {
        assert_eq!(classify_reason("rate limit exceeded"), Some(FailoverReason::RateLimit));
    }

    #[test]
    fn test_classify_reason_billing() {
        assert_eq!(classify_reason("insufficient credits"), Some(FailoverReason::Billing));
    }

    #[test]
    fn test_classify_reason_auth() {
        assert_eq!(classify_reason("invalid token"), Some(FailoverReason::Auth));
    }

    #[test]
    fn test_classify_reason_format() {
        assert_eq!(classify_reason("invalid request format"), Some(FailoverReason::Format));
    }

    #[test]
    fn test_classify_reason_timeout() {
        assert_eq!(classify_reason("timeout"), Some(FailoverReason::Timeout));
    }

    #[test]
    fn test_classify_reason_overloaded_treated_as_rate_limit() {
        assert_eq!(classify_reason("overloaded"), Some(FailoverReason::RateLimit));
    }

    #[test]
    fn test_extract_http_status_various() {
        assert_eq!(extract_http_status("status: 401"), Some(401));
        assert_eq!(extract_http_status("status: 403"), Some(403));
        assert_eq!(extract_http_status("status: 429"), Some(429));
        assert_eq!(extract_http_status("HTTP/1.1 200 OK"), Some(200));
        assert_eq!(extract_http_status("HTTP/2 502"), Some(502));
        assert_eq!(extract_http_status("no error here"), None);
    }

    #[test]
    fn test_is_image_dimension_error() {
        assert!(is_image_dimension_error("image dimensions exceed max 8000px"));
        assert!(!is_image_dimension_error("file not found"));
    }

    #[test]
    fn test_is_image_size_error() {
        assert!(is_image_size_error("image exceeds 20mb"));
        assert!(!is_image_size_error("file too small"));
    }

    // ============================================================
    // Additional edge-case tests for Go parity
    // ============================================================

    #[test]
    fn test_classify_rate_limit_resource_exhausted_regex() {
        let err = classify_error("resource exhausted for project", "google", "gemini");
        assert!(matches!(err, Some(FailoverError::RateLimit { .. })));
    }

    #[test]
    fn test_classify_overloaded_json_type() {
        // Go tests verify overloaded_error with JSON type field
        let err = classify_error(r#"overloaded_error: "type":"overloaded_error""#, "anthropic", "claude");
        assert!(matches!(err, Some(FailoverError::RateLimit { .. })));
    }

    #[test]
    fn test_classify_auth_expired_token() {
        let err = classify_error("The token has expired and needs to be refreshed", "openai", "gpt-4");
        assert!(matches!(err, Some(FailoverError::Auth { .. })));
    }

    #[test]
    fn test_classify_auth_re_authenticate() {
        let err = classify_error("Please re-authenticate to continue", "openai", "gpt-4");
        assert!(matches!(err, Some(FailoverError::Auth { .. })));
    }

    #[test]
    fn test_classify_auth_oauth_refresh_failed() {
        let err = classify_error("oauth token refresh failed", "openai", "gpt-4");
        assert!(matches!(err, Some(FailoverError::Auth { .. })));
    }

    #[test]
    fn test_classify_auth_access_denied() {
        let err = classify_error("access denied for this resource", "openai", "gpt-4");
        assert!(matches!(err, Some(FailoverError::Auth { .. })));
    }

    #[test]
    fn test_classify_billing_402_via_http_status() {
        let err = classify_error("status: 402 payment required", "openai", "gpt-4");
        // HTTP 402 via status code should classify as Billing
        assert!(matches!(err, Some(FailoverError::Billing { .. })));
    }

    #[test]
    fn test_classify_timeout_deadline_exceeded() {
        let err = classify_error("context deadline exceeded after 60s", "openai", "gpt-4");
        assert!(matches!(err, Some(FailoverError::Timeout { .. })));
    }

    #[test]
    fn test_classify_format_tool_use_id_path() {
        let err = classify_error("messages.1.content.1.tool_use.id is invalid", "anthropic", "claude-3");
        assert!(matches!(err, Some(FailoverError::Format { .. })));
    }

    #[test]
    fn test_classify_format_tool_use_id_variant() {
        let err = classify_error("tool_use_id must be a valid string", "anthropic", "claude-3");
        assert!(matches!(err, Some(FailoverError::Format { .. })));
    }

    #[test]
    fn test_classify_overloaded_521() {
        let err = classify_error("status: 521", "openai", "gpt-4");
        assert!(matches!(err, Some(FailoverError::Overloaded { .. })));
    }

    #[test]
    fn test_classify_overloaded_522() {
        let err = classify_error("status: 522", "openai", "gpt-4");
        assert!(matches!(err, Some(FailoverError::Overloaded { .. })));
    }

    #[test]
    fn test_classify_overloaded_524() {
        let err = classify_error("status: 524", "openai", "gpt-4");
        assert!(matches!(err, Some(FailoverError::Overloaded { .. })));
    }

    #[test]
    fn test_classify_overloaded_529() {
        let err = classify_error("status: 529", "openai", "gpt-4");
        assert!(matches!(err, Some(FailoverError::Overloaded { .. })));
    }

    #[test]
    fn test_classify_http_400_is_format() {
        let err = classify_error("status: 400 bad request", "openai", "gpt-4");
        assert!(matches!(err, Some(FailoverError::Format { .. })));
    }

    #[test]
    fn test_classify_http_200_not_classified() {
        let err = classify_error("status: 200", "openai", "gpt-4");
        // 200 is success, should not be classified as an error
        assert!(err.is_none());
    }

    #[test]
    fn test_classify_case_insensitive() {
        // Verify case-insensitive matching
        let err = classify_error("RATE LIMIT EXCEEDED", "openai", "gpt-4");
        assert!(matches!(err, Some(FailoverError::RateLimit { .. })));

        let err = classify_error("TIMEOUT AFTER 30S", "openai", "gpt-4");
        assert!(matches!(err, Some(FailoverError::Timeout { .. })));
    }

    #[test]
    fn test_extract_http_status_http2() {
        assert_eq!(extract_http_status("HTTP/2 502 bad gateway"), Some(502));
    }

    #[test]
    fn test_extract_http_status_http11() {
        assert_eq!(extract_http_status("HTTP/1.1 429 Too Many Requests"), Some(429));
    }

    #[test]
    fn test_classify_reason_all_unknown() {
        assert_eq!(classify_reason("connection reset by peer"), None);
        assert_eq!(classify_reason("unexpected EOF"), None);
    }

    #[test]
    fn test_classify_reason_combined_rate_limit() {
        // Overloaded should also return RateLimit reason
        assert_eq!(classify_reason("server is overloaded"), Some(FailoverReason::RateLimit));
    }
}
