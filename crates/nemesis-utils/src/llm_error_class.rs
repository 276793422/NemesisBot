//! LLM provider 错误的**纯文本分类**——模式表与判定逻辑的统一定义。
//!
//! （扩展计划）：
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
    /// 认证失败（401/凭据文案）——重试无意义。
    Auth,
    /// 拒绝访问（403/内容风控文案，BUG-20261123-012 批）——**已认证但被
    /// 拒**：内容风控（"content was flagged"）、权限/区域限制、策略拦截。
    /// 与 Auth 分家：处置同门（重试无意义）但归因不同——403 撞认证词表
    /// 曾驱动凭据卡循环重放同一被拒请求（key 无关，重放必然再拒）。
    Forbidden,
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
    /// 上下文溢出（prompt 超出模型窗口，B29/D1 2026-11-06）——重试无意义，
    /// loop 层消费方据此路由进响应式压缩（复用既有 force 压缩入口）。
    ContextOverflow,
}

/// 把 provider 错误文本分类为 [`LlmErrorClass`]；`None` = 分类不出（调用方
/// 不应据此触发任何自动重试/切换）。
pub fn classify_llm_error(error_msg: &str) -> Option<LlmErrorClass> {
    let msg = error_msg.to_lowercase();

    // 流中断短路：**必须最先**——StreamInterrupted 的
    // Display 尾段携带模型生成的任意文本（partial 全文），状态码提取
    // （partial 里恰有 "status: 429" 形态）与后续全部词表匹配（"token"/
    // "expired"/"unauthorized" 等）都会被内容污染误分类。归 Timeout 类 =
    // 瞬态可重试（is_retriable 同口径）；头行根因在 reason 字段不丢。
    // 用 starts_with 前缀锚不用 contains——contains 对携带任意文本的
    // 错误值同样可被伪造/污染。
    if msg.starts_with("stream interrupted from provider") {
        return Some(LlmErrorClass::Timeout);
    }

    // 流内失败事件黑名单前缀短路（2026-11-24 C1）：classify_stream_error_payload
    // ③ 未收录文案 / ④ 连 message 都没有 → 包 "stream failure (model X):"
    // 前缀落 Unknown 可重试（failover 侧 is_retriable 前缀门同门）。
    // 用 contains 不用 starts_with——这类错误经 FailoverError::Unknown
    // Display 展平后前段是 "unknown error from provider X: "，starts_with
    // 锚不住；"(model " 锚形只由本仓 ③None/④ 构造路径生产，外部文本
    // 无法预知该形态；StreamInterrupted Display 尾段携带的模型任意文本
    // （partial 全文）已被上一条 starts_with 短路更早拦截，污染不到这里。
    // 短路先于一切后续词表——前缀内的上游原文/excerpt 不参与分类
    // （黑名单语义：未识别默认可重试，"quota: 429" 数字excerpt 不得
    // 误入限流桶）。
    if msg.contains("stream failure (model ") {
        return Some(LlmErrorClass::Timeout);
    }

    // B29/D1（2026-11-06）上下文溢出文案族：高特异性短语，先于 image 维度
    // 与状态码判定——400+overflow body 由 from_status 包成
    // Unknown{message: "status 400: <body 前 200 字>"}，body 在 message 里，
    // 文案族对 message 匹配照常命中，状态码与文案两条路同判。归
    // ContextOverflow = 重试无意义 + loop 层路由进响应式压缩（器官 4a）。
    if is_context_overflow(&msg) {
        return Some(LlmErrorClass::ContextOverflow);
    }

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
/// 委托本表（此前 from_status 自带只认 502/503 的私有表，
/// 408/500/521-524/529 落非 retriable Unknown，与文本分类路径、loop 词表
/// 三面口径分裂，fallback 轮转在主级间歇 500 类故障时失效）。
pub fn class_by_status(status: u16) -> Option<LlmErrorClass> {
    match status {
        // 403 与 401 分家（BUG-20261123-012）：403 = 已认证但被拒（内容
        // 风控/权限/区域）——归 Auth 曾驱动凭据卡循环重放被拒请求。
        401 => Some(LlmErrorClass::Auth),
        403 => Some(LlmErrorClass::Forbidden),
        402 => Some(LlmErrorClass::Billing),
        // 504（Gateway Timeout）与 408 同族；loop transient 词表亦收录 "504"。
        408 | 504 => Some(LlmErrorClass::Timeout),
        429 => Some(LlmErrorClass::RateLimit),
        400 => Some(LlmErrorClass::Format),
        500 | 502 | 503 | 521 | 522 | 523 | 524 | 529 => Some(LlmErrorClass::Overloaded),
        _ => None,
    }
}

/// 错误码/错误 type → 类别映射的**单一真相源**（字符串语义码，与
/// [`class_by_status`] 并列）。
///
/// 三方权威调查合并表（2026-11-24 错误码补全批）：codex
/// `parse_terminal_provider_error` 终态黑名单 + openclaw
/// `FAILOVER_CODE_REASONS`/`FAILOVER_ERROR_TYPE_REASONS` + openai 官方
/// type 值 + anthropic 官方六枚举。归一（trim + 小写 + `'-'`→`'_'`）后
/// **精确匹配**——码是枚举值不是自由文本，contains 会把
/// `"not_rate_limit_related"` 之类自由文本误吸；仅两条受控后缀族用
/// `ends_with`（codex 同构码变体收敛成一条）。
///
/// 消费方（providers failover）语义约定：
/// - 流内错误事件：数字码探测之后、message 文案**之前**——码的特异性
///   高于自由文案，`insufficient_quota` 的 openai 文案
///   "You exceeded your current quota" 不得被限流词表错吸进瞬态桶；
/// - HTTP 错误体：**码给出的终态恒覆盖状态码粗粒度默认**（openclaw
///   `classifyFailoverClassificationFromHttpStatus` 同纪律——429 +
///   insufficient_quota = 配额耗尽终态，不是限流可重试）；码给出的
///   瞬态不推翻状态码的终态默认（401 + rate_limit 荒谬形态保 Auth，
///   保守单向闸由消费方实现，本表只供给类别）。
pub fn class_by_code(code: &str) -> Option<LlmErrorClass> {
    let normalized = code.trim().to_lowercase().replace('-', "_");
    const TABLE: &[(&str, LlmErrorClass)] = &[
        // —— 终态：计费/配额（三方交叉实证，黑名单制）——
        // openai 官方 429 type（官方语义=配额耗尽重试无意义）+ codex
        // 终态黑名单 + openclaw billing 正则 `/insufficient[_ ]quota/i`
        // 三方一致：
        ("insufficient_quota", LlmErrorClass::Billing),
        ("credit_balance_exhausted", LlmErrorClass::Billing),
        ("usage_not_included", LlmErrorClass::Billing),
        ("budget_reached", LlmErrorClass::Billing),
        // DeepSeek 403 实测 + openclaw billing 表 `/insufficient[_ ]balance/i`：
        ("insufficient_balance", LlmErrorClass::Billing),
        // codex api_key/project/organization_spend_limit_exceeded 三变体
        // 一条受控后缀：
        // —— 终态：认证 ——
        ("invalid_api_key", LlmErrorClass::Auth),
        ("authentication_error", LlmErrorClass::Auth),
        // —— 终态：拒绝访问（anthropic ② 并入 + Google/通用）——
        ("permission_error", LlmErrorClass::Forbidden),
        ("permission_denied", LlmErrorClass::Forbidden),
        // codex：workspace 推理钩子拦截 + 裸 policy 码（cyber/bio 不带
        // _violation 后缀，须精确条目）：
        ("inference_hook_denied", LlmErrorClass::Forbidden),
        ("cyber_policy", LlmErrorClass::Forbidden),
        ("bio_policy", LlmErrorClass::Forbidden),
        // —— 终态：请求格式/协议不兼容 ——
        // openai 官方 type + openclaw FAILOVER_ERROR_TYPE_REASONS：
        ("invalid_request_error", LlmErrorClass::Format),
        ("invalid_prompt", LlmErrorClass::Format),
        // openclaw UNKNOWN_PARAMETER：
        ("unknown_parameter", LlmErrorClass::Format),
        // codex + openclaw：终态处置同门（不轮转同模型）——
        // 我们无独立 model_not_found 类，归 Format（终态 Unknown）。
        ("model_not_found", LlmErrorClass::Format),
        // codex access program 族 + audio 输入校验族（精确匹配零误报，
        // 出现即终态正确）：
        ("access_program_not_enabled", LlmErrorClass::Format),
        ("invalid_access_program", LlmErrorClass::Format),
        ("unsupported_access_program", LlmErrorClass::Format),
        ("audio_too_large", LlmErrorClass::Format),
        ("audio_too_short", LlmErrorClass::Format),
        ("invalid_audio", LlmErrorClass::Format),
        ("invalid_audio_data", LlmErrorClass::Format),
        ("invalid_audio_format", LlmErrorClass::Format),
        ("missing_audio_sample_rate", LlmErrorClass::Format),
        // —— 终态：上下文溢出（codex 码形态；loop 侧据此路由响应式
        // 压缩环——比三方更聪明的归宿，openclaw/codex 只当 format）——
        ("context_length_exceeded", LlmErrorClass::ContextOverflow),
        // —— 瞬态：限流 ——
        // openclaw FAILOVER_CODE_REASONS（RATE_LIMIT/RATE_LIMITED/
        // RATE_LIMIT_EXCEEDED/TOO_MANY_REQUESTS/THROTTLING*）+ 文案词表
        // "quota exceeded" 同判（码形态跟随词表，不分裂）：
        ("rate_limit", LlmErrorClass::RateLimit),
        ("rate_limited", LlmErrorClass::RateLimit),
        ("rate_limit_exceeded", LlmErrorClass::RateLimit),
        // anthropic 官方枚举（流内 ② 并入面）：
        ("rate_limit_error", LlmErrorClass::RateLimit),
        ("billing_error", LlmErrorClass::Billing),
        ("quota_exceeded", LlmErrorClass::RateLimit),
        ("too_many_requests", LlmErrorClass::RateLimit),
        ("throttled", LlmErrorClass::RateLimit),
        ("throttling", LlmErrorClass::RateLimit),
        ("throttling_exception", LlmErrorClass::RateLimit),
        ("throttlingexception", LlmErrorClass::RateLimit),
        // Google/vertex（openclaw RESOURCE_EXHAUSTED 同款）：
        ("resource_exhausted", LlmErrorClass::RateLimit),
        // openclaw：
        ("model_cooldown", LlmErrorClass::RateLimit),
        // codex 流内（slow_down + flex 模式不可用，429 守卫暗示限流语义）：
        ("slow_down", LlmErrorClass::RateLimit),
        ("flex_unavailable", LlmErrorClass::RateLimit),
        // —— 瞬态：过载 ——
        // anthropic ② 并入（overloaded_error/api_error）+ openclaw
        // OVERLOADED：
        ("overloaded", LlmErrorClass::Overloaded),
        ("overloaded_error", LlmErrorClass::Overloaded),
        ("api_error", LlmErrorClass::Overloaded),
        // openclaw FAILOVER_ERROR_TYPE_REASONS：
        ("server_error", LlmErrorClass::Overloaded),
        ("upstream_error", LlmErrorClass::Overloaded),
        // codex 流内：
        ("policy_capacity_unavailable", LlmErrorClass::Overloaded),
        // —— 瞬态：超时 ——
        ("timeout_error", LlmErrorClass::Timeout),
        // openclaw FAILOVER_CODE_REASONS 传输 errno 族：
        ("ehostdown", LlmErrorClass::Timeout),
        ("enetreset", LlmErrorClass::Timeout),
        ("err_stream_premature_close", LlmErrorClass::Timeout),
    ];
    for (key, class) in TABLE {
        if normalized == *key {
            return Some(*class);
        }
    }
    // 受控后缀族（归一后判定）：codex 同构码变体收敛——
    // api_key/project/organization_spend_limit_exceeded 三变体一条；
    // content/misalignment_policy_violation 等 `_policy_violation` 结尾族
    // （cyber_policy/bio_policy 裸码不带后缀，已在上方精确表）。
    if normalized.ends_with("spend_limit_exceeded") {
        return Some(LlmErrorClass::Billing);
    }
    if normalized.ends_with("_policy_violation") {
        return Some(LlmErrorClass::Forbidden);
    }
    // 会话级封禁码族（2026-11-24 用户事故批；[`is_session_scoped_block`]
    // 同一词表的码形态臂）：`session_blocked_by_cyber_policy` 不落在上方
    // 精确表（`cyber_policy` 是裸码条目，带前缀的完整码精确匹配不命中），
    // 受控前缀族收敛同构变体——封会话恒终态 Forbidden。
    if normalized.starts_with("session_blocked") {
        return Some(LlmErrorClass::Forbidden);
    }
    None
}

/// Classify by message pattern matching.
fn class_by_message(msg: &str) -> Option<LlmErrorClass> {
    // 传输层连接失败（诚实化配套）：`failover::transport_error`
    // 把 DNS/连接拒绝/SYN 黑洞/中途断流统一盖章 "connect error (model X):
    // <根因链>" 前缀。必须**先于** auth/format 判定——错误链里携带的
    // URL/系统文案可能含孤立三位数字（如端口号:401），放行到 auth 分支
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
    if matches_forbidden(msg) {
        return Some(LlmErrorClass::Forbidden);
    }
    if matches_auth(msg) {
        return Some(LlmErrorClass::Auth);
    }
    if matches_format(msg) {
        return Some(LlmErrorClass::Format);
    }
    // —— 瞬态流断/传输断连措辞族（2026-11-24 黑名单架构 C2）**置于全部
    // 终态词之后**晚判：terminal specificity wins（三方调查同纪律）——
    // 混合文案（如 "stream interrupted: invalid request format"）终态词
    // 优先，避免把终态错误错当瞬态反复重试烧预算；纯传输措辞（前序全
    // 不命中）落到此处归 Timeout。本族只服务「归对桶」（瞬态快速重试
    // vs 限流退避梯子的分流），不再决定「能否自愈」（那归 C1 黑名单
    // 前缀短路，入口层先于一切）。
    if matches_transient_stream_wording(msg) {
        return Some(LlmErrorClass::Timeout);
    }
    None
}

/// 瞬态流断/传输断连措辞族（openclaw TRANSIENT_RETRY_EVIDENCE_RE /
/// message-patterns + gemini-cli RETRYABLE_NETWORK_CODES + 本仓现网事故串）。
/// 全部为高特异性短语，contains 匹配安全。**刻意不加**（防误报，openclaw
/// 锚定纪律）：裸 `interrupted`（取消语义 "request interrupted by user"
/// 误伤）；裸 `terminated`（openclaw 只收 ^terminated$ 锚定形态，
/// "process terminated with exit 1" 是进程终局非传输瞬态）；裸 5xx 数字
/// （词边界锚既有纪律）。
fn matches_transient_stream_wording(msg: &str) -> bool {
    const PATTERNS: &[&str] = &[
        // 真实事故串（codex 中转礼貌报错的应用层事件形态）——与下一条
        // 是不同子串（was 之差），两条都要：
        "stream was interrupted",
        // 本仓 StreamInterrupted 同族措辞（loop 瞬态词表对称补齐）：
        "stream interrupted",
        // openclaw message-patterns 断流措辞：
        "stream terminated",
        "stream ended without",
        "other side closed",
        "socket hang up",
        // openclaw 传输断连措辞（loop 瞬态词表既有词，本表对称补齐）：
        "connection lost",
        "connection reset",
        "reset by peer",
        "broken pipe",
        // 服务端明示可重试（openclaw 瞬态证据措辞）：
        "you can retry your request",
        // 真机实证（GLM anthropic lane 间歇 500 原文「网络错误，请稍后
        // 重试」的文案兜底臂——状态码缺失形态）：
        "请稍后重试",
        // gemini-cli RETRYABLE_NETWORK_CODES（node 系代理透传的 errno 码，
        // classify_llm_error 入口已 to_lowercase）：
        "econnreset",
        "etimedout",
        "econnrefused",
        "econnaborted",
        "epipe",
    ];
    contains_any(msg, PATTERNS)
}

fn matches_rate_limit(msg: &str) -> bool {
    const PATTERNS: &[&str] = &[
        "rate limit",
        "rate_limit",
        "too many requests",
        // "429" 已迁词边界正则（BUG-20261107-019）：裸 contains 会把长数字里
        // 嵌的 429（中转端口号 ":8429/"、毫秒时长 "2429ms"、计数 "4290"）误判
        // 限流——消费方据此记 provider cooldown + 限流退避梯子，真形态（timeout
        // /connect error）本应走瞬态快速重试。与同文件 \b402\b/\b401\b/\b403\b
        // 词边界纪律对齐。
        "exceeded your current quota",
        "resource has been exhausted",
        "resource_exhausted",
        "quota exceeded",
        "usage limit",
        // cc-switch / one-api 系流内限流实测文案（"Concurrency limit
        // exceeded for account"）——错误码在流内 error 事件采集点丢失时
        // 的文案兜底（2026-11-17 治本批；loop 层 RATE_LIMIT_ERROR_KEYWORDS
        // 双写同步）。
        "concurrency limit",
    ];
    const REGEX_PATTERNS: &[&str] = &[r"exceeded.*quota", r"resource.*exhausted", r"\b429\b"];
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
        "expired",
        "token has expired",
        "no credentials found",
        "no api key found",
    ];
    const REGEX_PATTERNS: &[&str] = &[r"invalid[_ ]?api[_ ]?key", r"\b401\b"];
    contains_any(msg, PATTERNS) || matches_any_regex(msg, REGEX_PATTERNS)
}

/// 拒绝访问（403 族）文案判定（BUG-20261123-012 批）。
///
/// 三个来源，均在 [`class_by_message`] 中先于 [`matches_auth`] 消费：
/// - 403 族词（"forbidden"/"access denied"/`\b403\b`——从 matches_auth
///   **语义重定位**而来：403 = 已认证但被拒，不是凭据问题）；
/// - 内容风控/策略短语族（各厂商流内 flag 文案形态——Daybreak 实测
///   "This content was flagged per our content policy" 无状态码形态，
///   只能靠文案兜底；`moderation` 单词即高特异性）；
/// - "access has been denied" 长形态（与短形态 "access denied" 互不包含）。
fn matches_forbidden(msg: &str) -> bool {
    const PATTERNS: &[&str] = &[
        "forbidden",
        "access denied",
        "access has been denied",
        // 内容风控/策略短语族（流内 error 事件无状态码形态的兜底；
        // loop 层不再需要双写——Forbidden 归终局不进任何重试环）。
        "content policy",
        "content was flagged",
        "content filtering",
        "content management policy",
        "moderation",
        "usage policy",
    ];
    const REGEX_PATTERNS: &[&str] = &[r"\b403\b"];
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
    // Look for patterns like "status: 429", "status 429", "HTTP 429".
    // 两个模式均 (?i)：classify_llm_error 与 providers
    // classify_error 传进来的都是 to_lowercase() 后的文本，大写 "HTTP" 字面
    // 在该语境恒 miss——"HTTP/1.1 503..." 纯状态行形态分类落 None，fallback
    // 不轮转；(?i) 同时让原始大小写文本直调时的 "Status:"/"HTTP" 首字母
    // 大写形态也命中。
    //
    // 尾随数字界（BUG-20261121-001）：捕获 3 位后必须紧跟**非数字或串尾**
    // （`(?:[^\d]|$)`），否则整段不是状态码（"status 4290" 按前 3 位截取
    // 会把嵌码误路由 429→RateLimit 进 cooldown+退避梯子）——与
    // [`class_by_message`] 词表层 `\b429\b`（BUG-20261107-019 纪律）同族
    // 对齐。regex crate（RE2 家族）不支持 lookahead，`(?!\d)` 会编译失败
    // 被 `.ok()?` 静默吞成全 None——边界只能表达成消费式字符类；合法
    // 3 位形态（后随非数字/串尾）行为零变化，截取失败则整体落 None 走
    // 词表分支，词表 `\b429\b` 同样不命中长数字 →两层语义一致地诚实不分类。
    let patterns = [
        regex::Regex::new(r"(?i)status[:\s]+(\d{3})(?:[^\d]|$)").ok()?,
        regex::Regex::new(r"(?i)HTTP[/\s]+\d*\.?\d*\s+(\d{3})(?:[^\d]|$)").ok()?,
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

/// 连接建立类失败判定（U-B 端点不可用封存专用；**不进**
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

/// 会话级封禁（session-scoped block）判定（2026-11-24 用户事故批）。
///
/// 语义：上游封的是**会话身份**不是单条请求——删除本轮/重发/改内容都无法
/// 解除，唯一出路是换会话身份（新建会话/清空当前会话）。与普通 403（换
/// 内容可解）在提示面上必须区分，否则用户在死会话里反复删除重试（实录：
/// 403 会话封禁后用户反复删轮/重发均撞同一封禁）。
///
/// 单一真相源：loop 层 Forbidden 臂的用户面提示选择（recovery）与
/// [`class_by_code`] 的码族映射共用本词表。词表是**黑名单制非穷举**（码形
/// 态 + 实测短语，遇到新厂商形态再扩）；大小写不敏感 contains，内部自行
/// lowercase（与 [`is_context_overflow`] 同款纪律——调用方可能传原始大小
/// 写文本）。
pub fn is_session_scoped_block(msg: &str) -> bool {
    let m = msg.to_lowercase();
    const MARKERS: &[&str] = &[
        // 错误码族锚（CC Switch 实录 `session_blocked_by_cyber_policy`；
        // `session_blocked` 前缀族覆盖同构码变体）：
        "session_blocked",
        // 实录英文原文短语（无码形态的兜底，高特异性）：
        "blocked by cyber-security policy",
        // 实录中文原文短语（同上）：
        "请开启新会话",
    ];
    MARKERS.iter().any(|p| m.contains(p))
}

/// 上下文溢出（prompt 超出模型窗口）文案族判定（B29/D1 2026-11-06）。
///
/// 单一真相源：loop 层词表臂（recovery，D2 接线挂协调窗）与 providers 层
/// 分派表（`classify_error` / `classify_message_error`）共用本函数。大小写
/// 不敏感 contains，内部自行 lowercase（与 [`is_connect_error`] 同款纪律
/// ——调用方可能传原始大小写文本）。
///
/// 家族表语义分级：高特异性短语独立成立；`input length` / `exceeds the
/// maximum` 两族是泛词，**双词同现**防裸误配（`input length` 须与
/// exceed/limit 同现；`exceeds the maximum` 须与 token/context/prompt
/// 同现——否则 "image exceeds the maximum size" 这类图像错误会被误吸进
/// 压缩环，白受必败重呼，正是 B29 要消灭的 G2 误路由形态）。
pub fn is_context_overflow(msg: &str) -> bool {
    let m = msg.to_lowercase();
    const FAMILIES: &[&str] = &[
        "context length",
        "context_length",
        "maximum context",
        "max context",
        "prompt is too long",
        "prompt too long",
        "too many input tokens",
        "input tokens exceed",
        "token count exceed",
        "reduce the length",
        // 中文族（无词界问题，contains 安全）。
        "超出模型上下文",
        "输入长度超出",
        "上下文长度",
    ];
    if FAMILIES.iter().any(|p| m.contains(p)) {
        return true;
    }
    // 泛词双词同现（防裸误配）。
    if m.contains("input length") && (m.contains("exceed") || m.contains("limit")) {
        return true;
    }
    if m.contains("exceeds the maximum")
        && (m.contains("token") || m.contains("context") || m.contains("prompt"))
    {
        return true;
    }
    false
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
