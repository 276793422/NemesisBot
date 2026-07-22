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
    assert_eq!(
        classify_reason("rate limit"),
        Some(FailoverReason::RateLimit)
    );
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
    assert_eq!(
        classify_reason("rate limit exceeded"),
        Some(FailoverReason::RateLimit)
    );
}

#[test]
fn test_classify_reason_billing() {
    assert_eq!(
        classify_reason("insufficient credits"),
        Some(FailoverReason::Billing)
    );
}

#[test]
fn test_classify_reason_auth() {
    assert_eq!(classify_reason("invalid token"), Some(FailoverReason::Auth));
}

#[test]
fn test_classify_reason_format() {
    assert_eq!(
        classify_reason("invalid request format"),
        Some(FailoverReason::Format)
    );
}

#[test]
fn test_classify_reason_timeout() {
    assert_eq!(classify_reason("timeout"), Some(FailoverReason::Timeout));
}

#[test]
fn test_classify_reason_overloaded_treated_as_rate_limit() {
    assert_eq!(
        classify_reason("overloaded"),
        Some(FailoverReason::RateLimit)
    );
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
    assert!(is_image_dimension_error(
        "image dimensions exceed max 8000px"
    ));
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
    let err = classify_error(
        r#"overloaded_error: "type":"overloaded_error""#,
        "anthropic",
        "claude",
    );
    assert!(matches!(err, Some(FailoverError::RateLimit { .. })));
}

#[test]
fn test_classify_auth_expired_token() {
    let err = classify_error(
        "The token has expired and needs to be refreshed",
        "openai",
        "gpt-4",
    );
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
    let err = classify_error(
        "messages.1.content.1.tool_use.id is invalid",
        "anthropic",
        "claude-3",
    );
    assert!(matches!(err, Some(FailoverError::Format { .. })));
}

#[test]
fn test_classify_format_tool_use_id_variant() {
    let err = classify_error(
        "tool_use_id must be a valid string",
        "anthropic",
        "claude-3",
    );
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
    assert_eq!(
        extract_http_status("HTTP/1.1 429 Too Many Requests"),
        Some(429)
    );
}

#[test]
fn test_classify_reason_all_unknown() {
    assert_eq!(classify_reason("connection reset by peer"), None);
    assert_eq!(classify_reason("unexpected EOF"), None);
}

#[test]
fn test_classify_reason_combined_rate_limit() {
    // Overloaded should also return RateLimit reason
    assert_eq!(
        classify_reason("server is overloaded"),
        Some(FailoverReason::RateLimit)
    );
}
