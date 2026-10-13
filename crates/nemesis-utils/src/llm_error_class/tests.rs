//! T2a（扩展计划 D4）：LLM 错误分类器单测。
//! （自生产文件内联块迁出——2026-07-17 起测试代码放独立文件的纪律。）

use super::*;

#[test]
fn six_classes_representative_samples() {
    assert_eq!(
        classify_llm_error("invalid api key"),
        Some(LlmErrorClass::Auth)
    );
    assert_eq!(
        classify_llm_error("status 401: nope"),
        Some(LlmErrorClass::Auth)
    );
    assert_eq!(
        classify_llm_error("status 402: payment required"),
        Some(LlmErrorClass::Billing)
    );
    assert_eq!(
        classify_llm_error("too many requests"),
        Some(LlmErrorClass::RateLimit)
    );
    assert_eq!(
        classify_llm_error("status 429"),
        Some(LlmErrorClass::RateLimit)
    );
    assert_eq!(
        classify_llm_error("request timeout"),
        Some(LlmErrorClass::Timeout)
    );
    assert_eq!(
        classify_llm_error("upstream deadline exceeded"),
        Some(LlmErrorClass::Timeout)
    );
    assert_eq!(
        classify_llm_error("status 500: oops"),
        Some(LlmErrorClass::Overloaded)
    );
    assert_eq!(
        classify_llm_error("invalid request format"),
        Some(LlmErrorClass::Format)
    );
    assert_eq!(classify_llm_error("totally unknown thing"), None);
}

#[test]
fn transport_connect_error_prefix_wins_over_later_patterns() {
    // 2026-10-13 诚实化配套：transport_error 盖章的 "connect error" 前缀
    // 必须先于 auth/format 判定——错误链里的 URL/系统文案可能含孤立三位
    // 数字（端口号 :401/、"os error 10061"），不能被 \b401\b 误归 Auth。
    let chained = "unknown error from provider codex/gpt-5.6-sol: connect error: \
                   error sending request for url (http://127.0.0.1:401/v1/responses): \
                   error trying to connect: tcp connect error: os error 10061";
    assert_eq!(classify_llm_error(chained), Some(LlmErrorClass::Timeout));
    // 无数字干扰的朴素形态同样归 Timeout（瞬态可重试）。
    assert_eq!(
        classify_llm_error("connect error: connection refused"),
        Some(LlmErrorClass::Timeout)
    );
}

#[test]
fn overloaded_wording_follows_rate_limit_convention() {
    // 文案形态按限流对待（既有约定）；状态码形态归 Overloaded。
    assert_eq!(
        classify_llm_error("provider is overloaded"),
        Some(LlmErrorClass::RateLimit)
    );
    assert_eq!(
        classify_llm_error("status 503"),
        Some(LlmErrorClass::Overloaded)
    );
}

#[test]
fn image_errors_are_format_class() {
    assert_eq!(
        classify_llm_error("image dimensions exceed max 8000px"),
        Some(LlmErrorClass::Format)
    );
    assert_eq!(
        classify_llm_error("image exceeds 25 mb"),
        Some(LlmErrorClass::Format)
    );
}

#[test]
fn extract_status_forms() {
    assert_eq!(extract_http_status("status: 429"), Some(429));
    assert_eq!(extract_http_status("HTTP/1.1 503 Service"), Some(503));
    assert_eq!(extract_http_status("no status here"), None);
}
