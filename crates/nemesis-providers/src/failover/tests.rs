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

    // BUG-20261025-83：500 必须归 Overloaded（可重试可轮转）——与
    // llm_error_class::class_by_status 同口径（单一真相源委托）。
    let err = FailoverError::from_status("openai", "gpt-4", 500, "internal error", None);
    assert!(matches!(err, FailoverError::Overloaded { .. }));
}

/// BUG-20261025-83 对齐锁：`from_status` 的状态码→类别映射必须与
/// `llm_error_class::class_by_status` 一致。此前本表只认 502/503，
/// 408/500/521-524/529 落 Unknown 非 retriable——fallback 链/router 的
/// 轮转判据是 `is_retriable`，主级间歇 500 类故障时永不轮转（loop 侧
/// 2026-09-24 补 "status 500" transient 词表时明确「同口径」意图，
/// 结构化路径漏对齐；文本分类路径同 crate 内测试早已钉 500 → Overloaded）。
#[test]
fn test_from_status_aligns_with_class_by_status() {
    // 可重试族：Timeout（408 / 504 Gateway Timeout）。
    for status in [408u16, 504] {
        let err = FailoverError::from_status("p", "m", status, "t", None);
        assert!(
            matches!(err, FailoverError::Timeout { .. }),
            "status {status} should map to Timeout, got {err:?}"
        );
        assert!(err.is_retriable(), "status {status} must be retriable");
    }
    // 可重试族：Overloaded（500/502/503/521-524/529），状态码随错误携带。
    for status in [500u16, 502, 503, 521, 522, 523, 524, 529] {
        let err = FailoverError::from_status("p", "m", status, "boom", None);
        assert!(
            matches!(err, FailoverError::Overloaded { status: s, .. } if s == status),
            "status {status} should map to Overloaded, got {err:?}"
        );
        assert!(err.is_retriable(), "status {status} must be retriable");
    }
    // 不可重试族保持原形态：400/404 → Unknown（400 在 class_by_status 归
    // Format，两类均不可重试；"status N: {body}" message 形态是 loop 侧
    // is_context_error 词表的消费面，不做无行为收益的变体迁移）。
    for status in [400u16, 404] {
        let err = FailoverError::from_status("p", "m", status, "nope", None);
        assert!(matches!(err, FailoverError::Unknown { .. }), "{err:?}");
        assert!(
            !err.is_retriable(),
            "status {status} must stay non-retriable"
        );
    }
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
        status: 502,
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
fn test_connect_error_unknown_is_retriable() {
    // 2026-10-13 诚实化配套：transport_error 的连接失败落 Unknown，但
    // fallback 链/路由的轮转判据是 is_retriable——前缀必须算 retriable
    // （此前伪装 Timeout 时恒 true，不补 = 主端点连不上不再故障转移）。
    let err = FailoverError::Unknown {
        provider: "codex".to_string(),
        message: "connect error (model gpt-x): error sending request".to_string(),
    };
    assert!(err.is_retriable(), "connect error must be retriable");
    // 其余 Unknown 维持不可重试。
    assert!(
        !FailoverError::Unknown {
            provider: "openai".to_string(),
            message: "status 500: boom".to_string(),
        }
        .is_retriable()
    );
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
            model: "m".to_string(),
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
            provider: "p".to_string(),
            status: 503,
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
