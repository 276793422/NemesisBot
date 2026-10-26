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
///
/// 状态码→类别映射的**单一真相源**：providers 侧 `FailoverError::from_status`
/// 委托本表（BUG-20261025-83——此前 from_status 自带只认 502/503 的私有表，
/// 408/500/521-524/529 落非 retriable Unknown，与文本分类路径、loop 词表
/// 三面口径分裂，fallback 轮转在主级间歇 500 类故障时失效）。
pub fn class_by_status(status: u16) -> Option<LlmErrorClass> {
    match status {
        401 | 403 => Some(LlmErrorClass::Auth),
        402 => Some(LlmErrorClass::Billing),
        // 504（Gateway Timeout）与 408 同族；loop transient 词表亦收录 "504"。
        408 | 504 => Some(LlmErrorClass::Timeout),
        429 => Some(LlmErrorClass::RateLimit),
        400 => Some(LlmErrorClass::Format),
        500 | 502 | 503 | 521 | 522 | 523 | 524 | 529 => Some(LlmErrorClass::Overloaded),
        _ => None,
    }
}

/// Classify by message pattern matching.
fn class_by_message(msg: &str) -> Option<LlmErrorClass> {
    // 传输层连接失败（2026-10-13 诚实化配套）：`failover::transport_error`
    // 把 DNS/连接拒绝/SYN 黑洞/中途断流统一盖章 "connect error (model X):
    // <根因链>" 前缀。必须**先于** auth/format 判定——错误链里携带的
    // URL/系统文案可能含孤立三位数字（如端口号 :401/），放行到 auth 分支
    // 会被 `\b401\b` 误分类成 Auth 而跳过一切重试。归 Timeout 类 = 同属
    // 「瞬态、可重试/可故障转移」处置；真实根因在消息链里不丢，只是重试
    // 归宿与超时同门。
    if msg.contains("connect error") {
        return Some(LlmErrorClass::Timeout);
    }
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

/// 错误 source 链全文拼接（"top: source1: source2: …"）。给 failover 层
/// 包装错误时用——reqwest 顶层 Display 只有 "error sending request for url
/// (…)"，连接拒绝/不可达的判定词（"connection refused" / "os error 10061"
/// 等，见 [`is_connect_error`] 词表）在 source 链深处，必须整链保留封存
/// 判定才可见。
pub fn error_chain_msg(e: &(dyn std::error::Error + 'static)) -> String {
    let mut msg = e.to_string();
    let mut src = std::error::Error::source(e);
    while let Some(s) = src {
        msg.push_str(": ");
        msg.push_str(&s.to_string());
        src = s.source();
    }
    msg
}

/// 连接建立类失败判定（U-B 2026-10-12 端点不可用封存专用；**不进**
/// [`LlmErrorClass`]——failover 语义里连接失败≈Timeout 可重试，但封存判定
/// 必须区分「连接建立失败」与「慢模型跑满超时」：前者=端点死了值得封存，
/// 后者=端点活着只是慢，计入会误封存）。
///
/// 纯文本启发式：reqwest 错误 Display 自带一层 source（"error sending
/// request for url (...): tcp connect error"），连接建立失败的词表在此层
/// 可见。裸 "error sending request" **不收录**——超时错误同形，会误判。
/// tests 用真实 reqwest 连接失败钉住 Display 形态。
pub fn is_connect_error(msg: &str) -> bool {
    let msg = msg.to_lowercase();
    const PATTERNS: &[&str] = &[
        // reqwest/hyper 连接建立层
        "error trying to connect",
        "tcp connect error",
        "connect error",
        // OS 连接拒绝/不可达（Windows WSA + Linux errno，Display 常态保留）
        "connection refused",
        "os error 10061", // WSAECONNREFUSED
        "os error 111",   // ECONNREFUSED
        "os error 10060", // WSAETIMEDOUT（连接尝试失败——主机不可达形态，非慢响应）
        "os error 113",   // EHOSTUNREACH
        "host unreachable",
        "network unreachable",
        "no route to host",
        // DNS 解析失败
        "dns error",
        "failed to lookup",
        "name or service not known",
        "temporary failure in name resolution",
        "no address associated with hostname",
        "failed to resolve",
    ];
    PATTERNS.iter().any(|p| msg.contains(p))
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
