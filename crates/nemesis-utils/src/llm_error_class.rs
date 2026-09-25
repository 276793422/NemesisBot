//! LLM provider 错误的**纯文本分类**——模式表与判定逻辑的单一真相源。
//!
//! T2a（扩展计划 D4-2a）：
//! 原实现在 nemesis-providers `error_classifier`，但 agent loop 生产代码看不见
//! providers（nemesis-agent 对 providers 只有 dev-dependency，loop 经本地
//! `LlmProvider` trait 解耦）——分类逻辑若留在 providers，loop 只能继续用
//! 自己的词表，两套口径漂移（本 goal 要修的正是这个）。下沉到 nemesis-utils
//! （双方共同依赖的叶 crate）后：providers 侧 `classify_error` 委托此处做
//! FailoverError 富化，agent loop 同源消费 [`classify_llm_error`]。
//!
//! 语义与原 providers 实现**逐条对齐**（含"overloaded 文案按限流对待"的
//! 既有约定；状态码 5xx 形态仍归 [`LlmErrorClass::Overloaded`]）。

/// 错误类别（不含 provider/model 等载荷——载荷由消费方自行富化）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmErrorClass {
    /// 认证失败（401/403/凭据文案）——重试无意义。
    Auth,
    /// 计费失败（402/余额文案）——重试无意义。
    Billing,
    /// 限流（429/配额文案；含"overloaded 文案"既有约定）——可重试。
    RateLimit,
    /// 超时（timeout/deadline 文案）——可重试。
    Timeout,
    /// 协议/请求格式不兼容——重试无意义。
    Format,
    /// 上游过载（5xx 状态码形态）——可重试。
    Overloaded,
}

/// 把 provider 错误文本分类为 [`LlmErrorClass`]；`None` = 分类不出（调用方
/// 不应据此触发任何自动重试/切换）。
pub fn classify_llm_error(error_msg: &str) -> Option<LlmErrorClass> {
    let msg = error_msg.to_lowercase();

    // Image dimension/size errors: non-retriable format error.
    if is_image_dimension_error(&msg) || is_image_size_error(&msg) {
        return Some(LlmErrorClass::Format);
    }

    // Try HTTP status code extraction first.
    if let Some(status) = extract_http_status(&msg)
        && let Some(class) = class_by_status(status)
    {
        return Some(class);
    }

    // Message pattern matching (priority order).
    class_by_message(&msg)
}

/// Classify by HTTP status code.
fn class_by_status(status: u16) -> Option<LlmErrorClass> {
    match status {
        401 | 403 => Some(LlmErrorClass::Auth),
        402 => Some(LlmErrorClass::Billing),
        408 => Some(LlmErrorClass::Timeout),
        429 => Some(LlmErrorClass::RateLimit),
        400 => Some(LlmErrorClass::Format),
        500 | 502 | 503 | 521 | 522 | 523 | 524 | 529 => Some(LlmErrorClass::Overloaded),
        _ => None,
    }
}

/// Classify by message pattern matching.
fn class_by_message(msg: &str) -> Option<LlmErrorClass> {
    if matches_rate_limit(msg) {
        return Some(LlmErrorClass::RateLimit);
    }
    if matches_overloaded(msg) {
        // Overloaded treated as rate_limit (existing convention).
        return Some(LlmErrorClass::RateLimit);
    }
    if matches_billing(msg) {
        return Some(LlmErrorClass::Billing);
    }
    if matches_timeout(msg) {
        return Some(LlmErrorClass::Timeout);
    }
    if matches_auth(msg) {
        return Some(LlmErrorClass::Auth);
    }
    if matches_format(msg) {
        return Some(LlmErrorClass::Format);
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
pub fn extract_http_status(msg: &str) -> Option<u16> {
    // Look for patterns like "status: 429", "status 429", "HTTP 429"
    let patterns = [
        regex::Regex::new(r"status[:\s]+(\d{3})").ok()?,
        regex::Regex::new(r"HTTP[/\s]+\d*\.?\d*\s+(\d{3})").ok()?,
    ];

    for p in &patterns {
        if let Some(caps) = p.captures(msg)
            && let Some(m) = caps.get(1)
            && let Ok(code) = m.as_str().parse::<u16>()
        {
            return Some(code);
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

#[cfg(test)]
mod tests;
