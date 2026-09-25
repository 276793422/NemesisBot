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
