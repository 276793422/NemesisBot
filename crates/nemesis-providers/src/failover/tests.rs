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

// -- L02（2026-11-01）StreamInterrupted --------------------------------------

#[test]
fn stream_interrupted_is_retriable_and_maps_timeout_reason() {
    let e = FailoverError::StreamInterrupted {
        provider: "openai".to_string(),
        model: "gpt-4".to_string(),
        reason: "stream closed without finish_reason".to_string(),
        partial_content: "partial".to_string(),
        has_partial_tool_calls: false,
    };
    assert!(
        e.is_retriable(),
        "流中断与 Timeout 同门（瞬态可重试）——fallback 链必须照常轮转"
    );
    assert_eq!(e.reason(), FailoverReason::Timeout);
    // retry_after_hint 非 RateLimit 恒 None（既有约定不破）。
    assert_eq!(e.retry_after_hint(), None);
}

#[test]
fn stream_interrupted_display_headline_prefix_and_tail_full_partial() {
    // Display 是跨层契约：头行（首个 \n 之前）以 "stream interrupted from
    // provider " 开头 = recovery / llm_error_class 的前缀锚判据；尾段携带
    // partial 全文；chars 计数按字符数非字节数（CJK 诚实）。
    let partial = "第一段\n第二段";
    let e = FailoverError::StreamInterrupted {
        provider: "anthropic".to_string(),
        model: "glm-5".to_string(),
        reason: "connection reset by peer".to_string(),
        partial_content: partial.to_string(),
        has_partial_tool_calls: true,
    };
    let display = e.to_string();
    let headline = display.lines().next().unwrap_or("");
    assert!(
        headline.starts_with("stream interrupted from provider anthropic/glm-5"),
        "头行前缀锚: {headline:?}"
    );
    assert!(
        !headline.contains("第一段"),
        "头行不携带 partial（前缀锚不可被内容伪造的前提）: {headline:?}"
    );
    assert!(
        display.ends_with(partial),
        "尾段携带 partial 全文（多行半截文本完整保留，对抗 A4）: {display:?}"
    );
    assert!(
        headline.contains("已接收 7 字符"),
        "chars 计数按字符数（7 字符非 22 字节）: {headline:?}"
    );
    assert!(
        display.contains("connection reset by peer"),
        "根因链不丢（诚实化纪律）: {display:?}"
    );
    // L29 契约：分隔符与 tc 标记常量出现在 Display（agent 侧解析器用同
    // 常量解析——字面量与常量漂移在此钉死）。
    assert!(
        display.contains(super::STREAM_PARTIAL_SEPARATOR),
        "partial 分隔符常量必须在 Display（跨 crate 解析契约）: {display:?}"
    );
    assert!(
        headline.contains(super::STREAM_TOOL_CALLS_TRUNCATED_MARK),
        "has_partial_tool_calls=true 头行必须带截断标记（L29 拼接续写判定面）: {headline:?}"
    );
}

#[test]
fn stream_interrupted_display_no_tool_calls_mark_when_false() {
    // L29 契约另一半：has_partial_tool_calls=false 头行不带标记（可拼接续写）。
    let e = FailoverError::StreamInterrupted {
        provider: "openai".to_string(),
        model: "gpt-5".to_string(),
        reason: "channel closed".to_string(),
        partial_content: "half text".to_string(),
        has_partial_tool_calls: false,
    };
    let display = e.to_string();
    let headline = display.lines().next().unwrap_or("");
    assert!(
        !headline.contains(super::STREAM_TOOL_CALLS_TRUNCATED_MARK),
        "false 不得带标记: {headline:?}"
    );
    assert!(
        display.contains(super::STREAM_PARTIAL_SEPARATOR),
        "分隔符恒在: {display:?}"
    );
}

// -- S16（2026-11-06 三方对比整改）截断路径 usage 记账 ------------------------

#[test]
fn stream_interrupted_estimated_output_tokens_contract() {
    // 非空 partial：下界估计 = ceil(chars/4)，与 Display 头行 chars 计数
    // 同源（同一 partial 字段，两投影不得漂移）。
    let partial = "abcd"; // 4 chars → 1 token
    let e = FailoverError::StreamInterrupted {
        provider: "p".to_string(),
        model: "m".to_string(),
        reason: "conn reset".to_string(),
        partial_content: partial.to_string(),
        has_partial_tool_calls: false,
    };
    assert_eq!(e.estimated_output_tokens(), Some(1), "ceil(4/4)=1");
    // Display 头行的「已接收 N 字符」与估计同字段派生（一致性锚）。
    let display = e.to_string();
    assert!(
        display.contains("已接收 4 字符"),
        "Display chars 计数在场: {display:?}"
    );

    // 5 chars → ceil(5/4)=2（向上取整语义：半 token 也按 1 计，不向下漏）。
    let e = FailoverError::StreamInterrupted {
        provider: "p".to_string(),
        model: "m".to_string(),
        reason: "conn reset".to_string(),
        partial_content: "abcde".to_string(),
        has_partial_tool_calls: false,
    };
    assert_eq!(e.estimated_output_tokens(), Some(2), "ceil(5/4)=2");

    // CJK 按 chars 非字节（7 字符 → 2，与 Display 同口径诚实）。
    let e = FailoverError::StreamInterrupted {
        provider: "p".to_string(),
        model: "m".to_string(),
        reason: "conn reset".to_string(),
        partial_content: "第一段\n第二段".to_string(),
        has_partial_tool_calls: false,
    };
    assert_eq!(e.estimated_output_tokens(), Some(2), "7 chars → ceil=2");
}

#[test]
fn stream_interrupted_estimated_none_on_empty_or_other_variants() {
    // 空 partial → None（服务端缓冲可能有未 flush 产出，报 0 不诚实）。
    let e = FailoverError::StreamInterrupted {
        provider: "p".to_string(),
        model: "m".to_string(),
        reason: "closed before first delta".to_string(),
        partial_content: String::new(),
        has_partial_tool_calls: false,
    };
    assert_eq!(e.estimated_output_tokens(), None, "空 partial 无估计依据");

    // 其余变体恒 None（无 partial 依据；非流断路径没有截断语义）。
    let timeout = FailoverError::Timeout {
        provider: "p".to_string(),
        model: "m".to_string(),
    };
    assert_eq!(timeout.estimated_output_tokens(), None);
    let unknown = FailoverError::Unknown {
        provider: "p".to_string(),
        message: "boom".to_string(),
    };
    assert_eq!(unknown.estimated_output_tokens(), None);
    let auth = FailoverError::Auth {
        provider: "p".to_string(),
        model: "m".to_string(),
        status: 401,
    };
    assert_eq!(auth.estimated_output_tokens(), None);
}

// BUG-20261103-351 回归锁：classify_message_error 与真相源同判——自由文案
// （流内错误事件 / CLI stderr）此前恒 Unknown 不可重试，上游显式瞬态文案
// 放弃 fallback 轮转。
#[test]
fn test_classify_message_error_transient_maps_retriable() {
    // 限流文案 → RateLimit（可轮转）。
    let e = super::classify_message_error("codex", "gpt-4o", " quota exceeded ");
    assert!(
        matches!(e, FailoverError::RateLimit { ref model, retry_after: None, .. }
        if model == "gpt-4o"),
        "quota exceeded 应归 RateLimit: {e:?}"
    );
    assert!(e.is_retriable(), "限流必须可轮转");

    // overloaded 文案按既有约定同归 RateLimit（class_by_message 同判）。
    let e =
        super::classify_message_error("codex", "m", "The model is overloaded, please try again");
    assert!(matches!(e, FailoverError::RateLimit { .. }), "{e:?}");
    assert!(e.is_retriable());

    // 认证文案 → Auth（诚实归因；不可轮转语义正确）。
    let e = super::classify_message_error("codex", "m", "invalid api key");
    assert!(matches!(e, FailoverError::Auth { .. }), "{e:?}");
    assert!(!e.is_retriable());
}

#[test]
fn test_classify_message_error_unclassifiable_falls_back_unknown() {
    // 分类不出维持 Unknown（不可重试兜底语义不变；原文案随 message 保留）。
    let msg = "totally novel failure mode";
    let e = super::classify_message_error("codex-cli", "m", msg);
    match e {
        FailoverError::Unknown { ref message, .. } => assert_eq!(message, msg),
        other => panic!("未收录文案应维持 Unknown，实际 {other:?}"),
    }
    assert!(!e.is_retriable());

    // Format 类文案同兜底（与流内 lane 行为一致）。
    let e = super::classify_message_error("claude-cli", "m", "invalid request format: x");
    assert!(matches!(e, FailoverError::Unknown { .. }), "{e:?}");
}

// B29/D1（2026-11-06）回归锁：上下文溢出归 Unknown 携带原文（不新增
// FailoverError 变体——换 provider 大概率同溢，不轮转语义正确；loop 层
// 靠分类结果/文案族路由进响应式压缩，不靠 Failover 变体）。
#[test]
fn test_classify_message_error_context_overflow_maps_unknown_with_original() {
    // 文案路径：anthropic lane 经典形态。
    let msg = "prompt is too long: 250000 tokens > 200000 maximum";
    let e = super::classify_message_error("anthropic", "claude-x", msg);
    match &e {
        FailoverError::Unknown { provider, message } => {
            assert_eq!(provider, "anthropic");
            assert_eq!(message, msg, "原文案必须随 message 保留");
        }
        other => panic!("溢出文案应归 Unknown 携带原文，实际 {other:?}"),
    }
    assert!(!e.is_retriable(), "溢出不可重试（换 provider 大概率同溢）");

    // 状态码路径：400 + overflow body——from_status 包成 "status 400: <body>"
    // 形态 Unknown，body 在 message 里，B29 文案族对 message 匹配照常命中。
    let e = super::FailoverError::from_status(
        "openai-compat",
        "glm-x",
        400,
        "this model's maximum context length is 8192 tokens, however you requested 12000",
        None,
    );
    match &e {
        FailoverError::Unknown { message, .. } => {
            assert!(message.starts_with("status 400: "), "{message:?}");
            assert!(
                super::super::error_classifier::classify_reason(message)
                    == Some(crate::failover::FailoverReason::Format),
                "溢出 body 走状态码包装后仍应可归因（Format 同门）: {message:?}"
            );
        }
        other => panic!("400+overflow body 应维持 status 400 形态 Unknown，实际 {other:?}"),
    }
    assert!(!e.is_retriable());
}
