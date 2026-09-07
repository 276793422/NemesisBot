use super::*;

#[test]
fn test_retriable_errors() {
    let err = FailoverError::RateLimit {
        provider: "openai".to_string(),
        model: "gpt-4".to_string(),
        retry_after: None,
    };
    assert!(err.is_retriable());
    assert_eq!(err.reason(), FailoverReason::RateLimit);

    let err = FailoverError::Auth {
        provider: "openai".to_string(),
        model: "gpt-4".to_string(),
        status: 401,
    };
    assert!(!err.is_retriable());
}

#[test]
fn test_from_status() {
    let err = FailoverError::from_status("openai", "gpt-4", 401, "unauthorized", None);
    assert!(matches!(err, FailoverError::Auth { .. }));

    let err = FailoverError::from_status("openai", "gpt-4", 429, "slow down", None);
    assert!(matches!(err, FailoverError::RateLimit { .. }));

    let err = FailoverError::from_status("openai", "gpt-4", 503, "overloaded", None);
    assert!(matches!(err, FailoverError::Overloaded { .. }));

    let err = FailoverError::from_status("openai", "gpt-4", 500, "internal error", None);
    assert!(matches!(err, FailoverError::Unknown { .. }));
}

// ============================================================
// Additional tests for missing coverage
// ============================================================

#[test]
fn test_retriable_timeout() {
    let err = FailoverError::Timeout {
        provider: "openai".to_string(),
        model: "gpt-4".to_string(),
    };
    assert!(err.is_retriable());
    assert_eq!(err.reason(), FailoverReason::Timeout);
}

#[test]
fn test_retriable_overloaded() {
    let err = FailoverError::Overloaded {
        provider: "openai".to_string(),
    };
    assert!(err.is_retriable());
    assert_eq!(err.reason(), FailoverReason::Overloaded);
}

#[test]
fn test_not_retriable_billing() {
    let err = FailoverError::Billing {
        provider: "openai".to_string(),
    };
    assert!(!err.is_retriable());
    assert_eq!(err.reason(), FailoverReason::Billing);
}

#[test]
fn test_not_retriable_format() {
    let err = FailoverError::Format {
        provider: "openai".to_string(),
        message: "bad format".to_string(),
    };
    assert!(!err.is_retriable());
    assert_eq!(err.reason(), FailoverReason::Format);
}

#[test]
fn test_not_retriable_unknown() {
    let err = FailoverError::Unknown {
        provider: "openai".to_string(),
        message: "something weird".to_string(),
    };
    assert!(!err.is_retriable());
    assert_eq!(err.reason(), FailoverReason::Unknown);
}

#[test]
fn test_from_status_401() {
    let err = FailoverError::from_status("anthropic", "claude-3", 401, "bad key", None);
    assert!(matches!(err, FailoverError::Auth { status: 401, .. }));
}

#[test]
fn test_from_status_403() {
    let err = FailoverError::from_status("anthropic", "claude-3", 403, "forbidden", None);
    assert!(matches!(err, FailoverError::Auth { status: 403, .. }));
}

#[test]
fn test_from_status_402() {
    let err = FailoverError::from_status("openai", "gpt-4", 402, "payment required", None);
    assert!(matches!(err, FailoverError::Billing { .. }));
}

#[test]
fn test_from_status_502() {
    let err = FailoverError::from_status("openai", "gpt-4", 502, "bad gateway", None);
    assert!(matches!(err, FailoverError::Overloaded { .. }));
}

#[test]
fn test_from_status_400() {
    let err = FailoverError::from_status("openai", "gpt-4", 400, "bad request", None);
    assert!(matches!(err, FailoverError::Unknown { .. }));
}

#[test]
fn test_from_status_404() {
    let err = FailoverError::from_status("openai", "gpt-4", 404, "not found", None);
    assert!(matches!(err, FailoverError::Unknown { .. }));
}

#[test]
fn test_error_display() {
    let err = FailoverError::Auth {
        provider: "openai".to_string(),
        model: "gpt-4".to_string(),
        status: 401,
    };
    let msg = format!("{}", err);
    assert!(msg.contains("openai"));
    assert!(msg.contains("gpt-4"));
    assert!(msg.contains("401"));
}

#[test]
fn test_rate_limit_display() {
    let err = FailoverError::RateLimit {
        provider: "anthropic".to_string(),
        model: "claude-3".to_string(),
        retry_after: Some(60),
    };
    let msg = format!("{}", err);
    assert!(msg.contains("anthropic"));
    assert!(msg.contains("claude-3"));
}

#[test]
fn test_reason_all_variants() {
    assert_eq!(
        FailoverError::Auth {
            provider: "p".to_string(),
            model: "m".to_string(),
            status: 0
        }
        .reason(),
        FailoverReason::Auth
    );
    assert_eq!(
        FailoverError::RateLimit {
            provider: "p".to_string(),
            model: "m".to_string(),
            retry_after: None
        }
        .reason(),
        FailoverReason::RateLimit
    );
    assert_eq!(
        FailoverError::Billing {
            provider: "p".to_string()
        }
        .reason(),
        FailoverReason::Billing
    );
    assert_eq!(
        FailoverError::Timeout {
            provider: "p".to_string(),
            model: "m".to_string()
        }
        .reason(),
        FailoverReason::Timeout
    );
    assert_eq!(
        FailoverError::Format {
            provider: "p".to_string(),
            message: "m".to_string()
        }
        .reason(),
        FailoverReason::Format
    );
    assert_eq!(
        FailoverError::Overloaded {
            provider: "p".to_string()
        }
        .reason(),
        FailoverReason::Overloaded
    );
    assert_eq!(
        FailoverError::Unknown {
            provider: "p".to_string(),
            message: "m".to_string()
        }
        .reason(),
        FailoverReason::Unknown
    );
}

// ---------------------------------------------------------------------------
// J1: Retry-After 填充
// ---------------------------------------------------------------------------

#[test]
fn test_parse_retry_after_seconds() {
    assert_eq!(parse_retry_after(Some("30")), Some(30));
    assert_eq!(parse_retry_after(Some(" 120 ")), Some(120));
    assert_eq!(parse_retry_after(Some("0")), Some(0));
}

#[test]
fn test_parse_retry_after_http_date_future() {
    let target = chrono::Utc::now() + chrono::Duration::hours(1);
    let header = target.format("%a, %d %b %Y %H:%M:%S GMT").to_string();
    let got = parse_retry_after(Some(&header)).expect("should parse HTTP-date");
    // ~1h 未来（±5s 换算余量）。
    assert!(got > 3500 && got <= 3600, "got {}s", got);
}

#[test]
fn test_parse_retry_after_http_date_past_is_zero() {
    let target = chrono::Utc::now() - chrono::Duration::hours(1);
    let header = target.format("%a, %d %b %Y %H:%M:%S GMT").to_string();
    assert_eq!(parse_retry_after(Some(&header)), Some(0));
}

#[test]
fn test_parse_retry_after_invalid() {
    assert_eq!(parse_retry_after(None), None);
    assert_eq!(parse_retry_after(Some("")), None);
    assert_eq!(parse_retry_after(Some("   ")), None);
    assert_eq!(parse_retry_after(Some("soon")), None);
    assert_eq!(
        parse_retry_after(Some("Wed, 99 Xxx 2026 07:28:00 GMT")),
        None
    );
}

#[test]
fn test_from_status_429_carries_retry_after() {
    let err = FailoverError::from_status("openai", "gpt-4", 429, "slow down", Some(30));
    match err {
        FailoverError::RateLimit { retry_after, .. } => assert_eq!(retry_after, Some(30)),
        other => panic!("expected RateLimit, got {:?}", other),
    }
    // None 形态不受影响。
    let err = FailoverError::from_status("openai", "gpt-4", 429, "slow down", None);
    match err {
        FailoverError::RateLimit { retry_after, .. } => assert_eq!(retry_after, None),
        other => panic!("expected RateLimit, got {:?}", other),
    }
}

#[test]
fn test_retry_after_hint_accessor() {
    let with = FailoverError::RateLimit {
        provider: "p".to_string(),
        model: "m".to_string(),
        retry_after: Some(45),
    };
    assert_eq!(
        with.retry_after_hint(),
        Some(std::time::Duration::from_secs(45))
    );

    let without = FailoverError::RateLimit {
        provider: "p".to_string(),
        model: "m".to_string(),
        retry_after: None,
    };
    assert_eq!(without.retry_after_hint(), None);

    let timeout = FailoverError::Timeout {
        provider: "p".to_string(),
        model: "m".to_string(),
    };
    assert_eq!(timeout.retry_after_hint(), None);
}

#[test]
fn test_retry_after_from_headers_parses() {
    let mut map = reqwest::header::HeaderMap::new();
    assert_eq!(retry_after_from_headers(&map), None);
    map.insert("retry-after", "30".parse().unwrap());
    assert_eq!(retry_after_from_headers(&map), Some(30));
    map.insert("retry-after", "bogus".parse().unwrap());
    assert_eq!(retry_after_from_headers(&map), None);
}
