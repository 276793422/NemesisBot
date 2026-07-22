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
    const REGEX_PATTERNS: &[&str] = &[r"exceeded.*quota", r"resource.*exhausted"];
    contains_any(msg, PATTERNS) || matches_any_regex(msg, REGEX_PATTERNS)
}

fn matches_overloaded(msg: &str) -> bool {
    const PATTERNS: &[&str] = &["overloaded"];
    const REGEX_PATTERNS: &[&str] = &[r#"overloaded_error"#, r#""type"\s*:\s*"overloaded_error""#];
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
    const REGEX_PATTERNS: &[&str] = &[r"invalid[_ ]?api[_ ]?key", r"\b401\b", r"\b403\b"];
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
mod tests;
