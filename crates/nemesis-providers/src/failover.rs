//! Failover system for LLM providers.

use thiserror::Error;

use nemesis_utils::llm_error_class::LlmErrorClass;

/// StreamInterrupted Display 的 partial 分隔符
/// ——Display 字面量引用本常量（thiserror format 内联展开）。⚠ 分层约束：
/// `nemesis-providers` 只是 agent 的 dev-dependency，agent 侧解析器持**同文
/// 字面量副本**互锚——providers 侧测试钉「Display contains 本常量」，agent
/// 侧测试钉副本值，任一端改文本两端测试必红。
pub const STREAM_PARTIAL_SEPARATOR: &str = "\n\n--- 以下为中断前已生成的部分内容 ---\n";

/// `has_partial_tool_calls=true` 时头行追加的标记（只出现在头行——
/// agent 侧拼接续写解析只对头行做 contains 判定，模型任意输出污染不了
/// 判定）。与 agent 侧字面量副本互锚（同上）。
pub const STREAM_TOOL_CALLS_TRUNCATED_MARK: &str = "，工具调用已截断（不可拼接续写）";

/// Reason for provider failover.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FailoverReason {
    Auth,
    /// 拒绝访问（403/内容风控，BUG-20261123-012 批）——与 Auth 分家的
    /// 归因桶；轮转语义同门（不可重试不轮转），cooldown 记账独立。
    Forbidden,
    RateLimit,
    Billing,
    Timeout,
    Format,
    Overloaded,
    Unknown,
}

use serde::{Deserialize, Serialize};

/// Failover error with context.
#[derive(Debug, Clone, Error)]
pub enum FailoverError {
    #[error("auth failure for provider {provider}/{model}: status {status}")]
    Auth {
        provider: String,
        model: String,
        status: u16,
    },
    /// 拒绝访问（403/内容风控，BUG-20261123-012 批）：已认证但被拒——
    /// 重放同一请求必然再拒，`is_retriable=false`（fallback 不轮转）。
    /// Display 同时携带 `forbidden` 词锚与 `status {status}` 码锚——展平
    /// 文本回灌 `classify_llm_error` 时两条路同判 Forbidden（词表/状态表
    /// 双保险，与 StreamInterrupted 头行前缀锚同门的分类诚实性纪律）。
    /// `message` 携带上游 body 摘要（600 字符）——用户终局文案靠它看到
    /// 真实拒绝原因（此前 403 body 截 200 进 Unknown，Daybreak flag 原文
    /// 被截断且被误归 Auth）。
    #[error("forbidden by provider {provider}/{model}: status {status}: {message}")]
    Forbidden {
        provider: String,
        model: String,
        status: u16,
        message: String,
    },
    #[error("rate limited by provider {provider}/{model}")]
    RateLimit {
        provider: String,
        model: String,
        retry_after: Option<u64>,
    },
    #[error("billing issue with provider {provider}")]
    Billing { provider: String },
    #[error("timeout calling provider {provider}/{model}")]
    Timeout { provider: String, model: String },
    // 流式中断部分输出保留：流中途 Err / channel 关闭无终态
    // 且已聚合文本非空时，partial 不再丢弃——一等公民随错误值携带。
    // Display 结构是**契约**：头行（首个 \n 之前）以 "stream interrupted from
    // provider " 开头 = 下游（llm_error_class 入口短路 / recovery 词表短路）
    // 的前缀锚判据，尾段携带 partial 全文（agent loop 侧 LlmProvider trait
    // 不见 providers 类型，Display 文本是唯一跨层通道——设计文档 关键
    // 约束）。头行只含 reason 不含 partial——分类器对整段文本 contains 匹配
    // 会被模型生成的任意内容污染（对抗用例），前缀锚 + 头行根因隔离
    // 是分类诚实性的前提。
    // 头行追加 `has_partial_tool_calls` 标记
    //（「工具调用已截断」字样只出现在头行）——半截 tool_calls 不可当纯文本
    // 续用的判定面经 Display 跨层（仅携带字段未消费， 拼接续写消费）。
    #[error("stream interrupted from provider {provider}/{model}: {reason}（已接收 {} 字符{}，中断前部分文本随错误通知保留）\n\n{STREAM_PARTIAL_SEPARATOR}{partial_content}", .partial_content.chars().count(), if *.has_partial_tool_calls { STREAM_TOOL_CALLS_TRUNCATED_MARK } else { "" })]
    StreamInterrupted {
        provider: String,
        model: String,
        /// 原始根因（Err 臂 = lane 原错误 to_string()；None 臂 = 固定说明）。
        /// 根因链不丢（10-13 传输错误诚实化同门纪律）。
        reason: String,
        /// 已聚合的文本增量（模型已产出的工作成果）。
        partial_content: String,
        /// tool_calls 出现过 = 半截不可当纯文本续用（本期仅携带供消费方判定）。
        has_partial_tool_calls: bool,
    },
    #[error("format error from provider {provider}: {message}")]
    Format { provider: String, message: String },
    #[error("provider {provider} is overloaded (status: {status})")]
    Overloaded { provider: String, status: u16 },
    #[error("unknown error from provider {provider}: {message}")]
    Unknown { provider: String, message: String },
}

impl FailoverError {
    /// Check if this error is retriable with a different provider.
    pub fn is_retriable(&self) -> bool {
        match self {
            FailoverError::RateLimit { .. }
            | FailoverError::Timeout { .. }
            // 流中断与 Timeout 同门（瞬态可重试）——重试=全新请求，
            // partial 不进重试请求由结构性保证。
            | FailoverError::StreamInterrupted { .. }
            | FailoverError::Overloaded { .. } => true,
            // 诚实化配套：`transport_error` 的传输层连接失败
            // （DNS/连接拒绝/SYN 黑洞/中途断流）落 Unknown 但带 "connect
            // error" 前缀——与 Timeout 同属瞬态故障，fallback 链与路由
            // 必须照常轮转（此前伪装 Timeout 时 is_retriable 恒 true，
            // 诚实化若不补这条，主端点连不上就不再故障转移 = 回归）。
            // 同门：成功体超帽（BUG-20261115-024，`body_cap`）落 Unknown
            // 但带 "response body exceeds limit" 前缀——端点失控/恶意是
            // 端点问题，换下一家。
            // 黑名单架构（2026-11-24 C1）：流内失败事件未收录文案 /
            // 连 message 都没有 → 前缀化 Unknown（`classify_message_error`
            // None 臂 / `classify_stream_error_payload` ④）——未识别默认
            // 可重试（codex 官方 parse_failed_response 同语义）。白名单
            // 架构（未识别=不可重试）永远有词表漏网：现网事故
            // "Upstream response stream was interrupted" 与词表
            // "stream interrupted" 一词之差（was），礼貌报错的应用层事件
            // 反而比粗暴断连处置更差（终局中断 vs 瞬态自愈）。
            // 其余 Unknown（400/404 等应用层错误、Format/ContextOverflow
            // 终态）维持不可重试——500 过载族自 起由 `from_status` 直接归
            // Overloaded，不再落 Unknown。
            FailoverError::Unknown { message, .. } => {
                message.starts_with("connect error")
                    || message.starts_with("response body exceeds limit")
                    || message.starts_with("stream failure")
            }
            _ => false,
        }
    }

    /// Get the failover reason.
    pub fn reason(&self) -> FailoverReason {
        match self {
            FailoverError::Auth { .. } => FailoverReason::Auth,
            FailoverError::Forbidden { .. } => FailoverReason::Forbidden,
            FailoverError::RateLimit { .. } => FailoverReason::RateLimit,
            FailoverError::Billing { .. } => FailoverReason::Billing,
            FailoverError::Timeout { .. }
            // 轮转语义同门（瞬态可 fallback）——不新增 FailoverReason
            // 枚举值，波及面最小（设计文档）。
            | FailoverError::StreamInterrupted { .. }
            => FailoverReason::Timeout,
            FailoverError::Format { .. } => FailoverReason::Format,
            FailoverError::Overloaded { .. } => FailoverReason::Overloaded,
            FailoverError::Unknown { .. } => FailoverReason::Unknown,
        }
    }

    /// Get the `retry_after` hint carried by this error (RateLimit only).
    ///
    /// `RateLimit` 变体可携带服务端 `Retry-After` 秒数；转成 `Duration`
    /// 供 `CooldownTracker::mark_failure` 作退避 hint（其余变体恒 None）。
    pub fn retry_after_hint(&self) -> Option<std::time::Duration> {
        match self {
            FailoverError::RateLimit {
                retry_after: Some(secs),
                ..
            } => Some(std::time::Duration::from_secs(*secs)),
            _ => None,
        }
    }

    /// S16（2026-11-06 三方对比整改）：流断路径的诚实 usage 下界估计。
    ///
    /// 流中断时上游 usage 终帧未到达（usage 只随 finish_reason 终帧透传），
    /// 该轮真实消耗对账本不可见。本方法从已接收的 partial 给出**下界估计**
    /// ：`tokens ≈ chars/4` 向上取整（英文近似下界；CJK≈1 char/token 会低
    /// 估——估计语义是「至少消耗了这么多」，不是精确值，消费端落账必须以
    /// estimated 注记表达，不得当实测值）；空 partial → `None`（服务端缓冲
    /// 可能已有未 flush 产出，报 0 不诚实）；其余变体恒 `None`（无 partial
    /// 依据）。输入侧（prompt 处理先于流式开始，输入 token 必已消耗）本层
    /// 不可知——消费端以 usage_missing 注记表达，不造数。
    ///
    /// 账本消费点：organ-5 observer（nemesis-agent 边界，接线挂 B29/B30
    /// 设计文档同窗）；proxy lane 错误行已自带 500+error_message（Display
    /// 携带 partial），不依赖本方法。
    pub fn estimated_output_tokens(&self) -> Option<u64> {
        match self {
            FailoverError::StreamInterrupted {
                partial_content, ..
            } if !partial_content.is_empty() => {
                Some((partial_content.chars().count() as u64).div_ceil(4))
            }
            _ => None,
        }
    }

    /// Create from HTTP status code.
    ///
    /// `retry_after`：调用方从响应头解析的 `Retry-After` 秒数（见
    /// [`retry_after_from_headers`]）；None = 响应未携带（退避走公式）。
    ///
    /// 状态码→类别映射**委托 `llm_error_class::class_by_status` 单一真相源**
    /// （此前自带只认 502/503 的私有表，408/500/521-524/529
    /// 落非 retriable Unknown——`is_retriable` 是 fallback 链与 router 的
    /// 轮转判据，主级间歇 500 类故障时永不轮转；文本分类路径与 loop 词表
    /// 同判可重试，三面口径分裂）。504 亦由真相源补齐归 Timeout。
    pub fn from_status(
        provider: &str,
        model: &str,
        status: u16,
        body: &str,
        retry_after: Option<u64>,
    ) -> Self {
        use nemesis_utils::llm_error_class::{LlmErrorClass, class_by_status};
        let Some(class) = class_by_status(status) else {
            // 未收录状态码：保持 Unknown 原文案形态（不可重试），
            // "status N: {body}" 是 loop 侧 is_context_error 词表的消费面。
            return FailoverError::Unknown {
                provider: provider.to_string(),
                message: format!(
                    "status {}: {}",
                    status,
                    body.chars().take(200).collect::<String>()
                ),
            };
        };
        // message 按类预构造（既有约定：Forbidden 600——用户终局文案里唯一
        // 的真实拒绝原因，Daybreak flag 原文实测超 200；Format/
        // ContextOverflow 带 "status N: " 前缀 + 200 摘要——前缀是 loop 侧
        // is_context_error 词表的消费面形态，不能丢）。
        let message = match class {
            LlmErrorClass::Forbidden => body.chars().take(600).collect::<String>(),
            LlmErrorClass::Format | LlmErrorClass::ContextOverflow => format!(
                "status {}: {}",
                status,
                body.chars().take(200).collect::<String>()
            ),
            _ => String::new(),
        };
        Self::from_class(provider, model, class, status, &message, retry_after)
    }

    /// [`LlmErrorClass`] → 变体构造**单一转换点**（错误码补全批 2026-11-24）：
    /// `from_status` / `classify_http_error_body` / 流内
    /// `classify_stream_error_payload` ② 三路共用。
    ///
    /// `message`：Forbidden / Unknown 形态（Format·ContextOverflow）携带的
    /// 上游原文——**截断由调用方按类负责**（Forbidden 600 / Unknown 200 是
    /// `from_status` 既有约定；流内 ② 传原文案不截）。
    /// Format / ContextOverflow → `Unknown`（不新增变体，B29 同约定：
    /// 换 provider 大概率同问题不轮转；ContextOverflow 由 loop 侧按文案族
    /// 路由响应式压缩）。Overloaded 带真实上游状态码（502/503 归因盲点
    /// 修复的既有语义；Display "(status: NNN)" 形态可被
    /// `llm_error_class::extract_http_status` 还原）。
    fn from_class(
        provider: &str,
        model: &str,
        class: LlmErrorClass,
        status: u16,
        message: &str,
        retry_after: Option<u64>,
    ) -> Self {
        match class {
            LlmErrorClass::Auth => FailoverError::Auth {
                provider: provider.to_string(),
                model: model.to_string(),
                status,
            },
            LlmErrorClass::Forbidden => FailoverError::Forbidden {
                provider: provider.to_string(),
                model: model.to_string(),
                status,
                message: message.to_string(),
            },
            LlmErrorClass::RateLimit => FailoverError::RateLimit {
                provider: provider.to_string(),
                model: model.to_string(),
                retry_after,
            },
            LlmErrorClass::Billing => FailoverError::Billing {
                provider: provider.to_string(),
            },
            LlmErrorClass::Timeout => FailoverError::Timeout {
                provider: provider.to_string(),
                model: model.to_string(),
            },
            LlmErrorClass::Overloaded => FailoverError::Overloaded {
                provider: provider.to_string(),
                status,
            },
            LlmErrorClass::Format | LlmErrorClass::ContextOverflow => FailoverError::Unknown {
                provider: provider.to_string(),
                message: message.to_string(),
            },
        }
    }

    /// HTTP 非 stream 错误体 → `FailoverError`（**码感知**版，错误码补全批
    /// 2026-11-24）。全 lane 的 `status >= 400` 路径统一入口。
    ///
    /// 探测序：
    /// ① body JSON 解析失败 / 无字符串码或 type → [`FailoverError::from_status`]
    ///    现状（状态码定类，零回退）。
    /// ② error 子对象（`/error` 对象，否则整体）的 `code` / `type` /
    ///    `status_code` 字符串值 → [`nemesis_utils::llm_error_class::class_by_code`]
    ///    命中 → **保守单向闸**后采纳：
    ///    - 码给出的**终态**（Auth/Forbidden/Billing/Format/ContextOverflow）
    ///      恒覆盖状态码粗粒度默认——openclaw
    ///      `classifyFailoverClassificationFromHttpStatus` 同纪律。核心
    ///      事故场景：OpenAI quota 错误 = HTTP 429 +
    ///      `error.type="insufficient_quota"`（官方语义=配额耗尽，重试
    ///      无意义）——旧路径 429 臂归 RateLimit 可重试，终态错误在状态码
    ///      掩护下无限重试烧预算。
    ///    - 码给出的**瞬态**（RateLimit/Overloaded/Timeout）不推翻状态码
    ///      的异类终态默认（401 + `"rate_limit_exceeded"` 荒谬形态保
    ///      Auth——终态判定不被可疑载荷翻转）；仅当状态码默认同类或未收录
    ///      时采用。
    pub fn classify_http_error_body(
        provider: &str,
        model: &str,
        status: u16,
        body: &str,
        retry_after: Option<u64>,
    ) -> Self {
        use nemesis_utils::llm_error_class::{LlmErrorClass, class_by_code, class_by_status};
        let Ok(data) = serde_json::from_str::<serde_json::Value>(body.trim()) else {
            return Self::from_status(provider, model, status, body, retry_after);
        };
        // error 子对象定位：openai 家族 /error；网关直透形态整体即错误对象。
        let err_obj = data.get("error").filter(|v| v.is_object()).unwrap_or(&data);
        let Some(code_class) = ["code", "type", "status_code"].iter().find_map(|key| {
            err_obj
                .get(*key)
                .and_then(|v| v.as_str())
                .and_then(class_by_code)
        }) else {
            return Self::from_status(provider, model, status, body, retry_after);
        };
        let status_class = class_by_status(status);
        let code_terminal = matches!(
            code_class,
            LlmErrorClass::Auth
                | LlmErrorClass::Forbidden
                | LlmErrorClass::Billing
                | LlmErrorClass::Format
                | LlmErrorClass::ContextOverflow
        );
        let adopt = code_terminal || !matches!(status_class, Some(sc) if sc != code_class);
        if !adopt {
            return Self::from_status(provider, model, status, body, retry_after);
        }
        // message：与 from_status 同形态约定——Forbidden 600；Format/
        // ContextOverflow 带 "status N: " 前缀（loop 词表消费面形态）+
        // 200 摘要；err_obj.message 优先（openai body 的 message 是人读
        // 文案），缺失回落整体 body。
        let raw = err_obj
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or(body);
        let message = match code_class {
            LlmErrorClass::Forbidden => raw.chars().take(600).collect::<String>(),
            LlmErrorClass::Format | LlmErrorClass::ContextOverflow => format!(
                "status {}: {}",
                status,
                raw.chars().take(200).collect::<String>()
            ),
            _ => String::new(),
        };
        Self::from_class(provider, model, code_class, status, &message, retry_after)
    }
}

/// Parse a raw `Retry-After` header value into seconds (RFC 7231).
///
/// 两种合法形态：延迟秒数（`"30"`）或 HTTP-date（`"Wed, 21 Oct 2026 07:28:00 GMT"`，
/// RFC 2822 格式，按当前墙钟换算成剩余秒数，已过去的日期 → `Some(0)` 立即可重试）。
/// 解析失败 / 值缺失 → `None`（退避走公式，不猜）。
pub fn parse_retry_after(value: Option<&str>) -> Option<u64> {
    let raw = value?.trim();
    if raw.is_empty() {
        return None;
    }
    // 形态 1：延迟秒数。
    if let Ok(secs) = raw.parse::<u64>() {
        return Some(secs);
    }
    // 形态 2：HTTP-date（IMF-fixdate，RFC 2822 同构）。
    let target = chrono::DateTime::parse_from_rfc2822(raw).ok()?;
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Some(target.timestamp().max(0) as u64).map(|t| t.saturating_sub(now_secs))
}

/// Extract + parse the `Retry-After` header from a response header map.
pub fn retry_after_from_headers(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    parse_retry_after(headers.get("retry-after").and_then(|v| v.to_str().ok()))
}

/// Map a reqwest transport error to a `FailoverError`, honestly.
///
/// 根修（gpt-5.6-sol 风暴排查副产品）：此前各 lane 把**一切**
/// `send()`/流读错误一律 `.map_err(|_| Timeout)`——DNS 失败、连接拒绝、
/// SYN 黑洞（Windows TCP 重传 ~21s）全部伪装成「超时」，误导排查方向
/// （真超时=上游慢；连接失败=地址/网络错，处置与文案完全不同）。现在：
/// - `reqwest::Error:is_timeout()`（客户端级真超时，含 connect_timeout）
///   → `Timeout`（语义不变，瞬态可重试）；
/// - 其余传输错误 → `Unknown`，message 以 **"connect error: "** 前缀 +
///   完整 source 链（DNS/连接/OS 根因原文）。该前缀是 agent loop transient
///   词表成员（"connect error"），也是 `llm_error_class` 的早判分支
///   （先于 auth/format，防 URL 孤立数字误分类）——可重试归宿有双保险。
pub fn transport_error(provider: &str, model: &str, e: &reqwest::Error) -> FailoverError {
    if e.is_timeout() {
        return FailoverError::Timeout {
            provider: provider.to_string(),
            model: model.to_string(),
        };
    }
    // Unknown 变体只带 provider + message（Timeout 才带 model 槽位）——把
    // model 编进 message 保住可观测性与「空模型名回落 default_model」的
    // 可断言性（"connect error" 前缀保持在最前，词表/分类器锚定它）。
    FailoverError::Unknown {
        provider: provider.to_string(),
        message: format!("connect error (model {}): {}", model, error_chain(e)),
    }
}

/// Walk `std::error:Error::source()` chain and join with ": "——reqwest 的
/// `Display` 只有顶层（"error sending request for url (...)"），真实根因
/// （hyper/tcp/OS 文案）藏在 source 链里。
fn error_chain(e: &dyn std::error::Error) -> String {
    let mut parts = vec![e.to_string()];
    let mut src = e.source();
    while let Some(s) = src {
        parts.push(s.to_string());
        src = s.source();
    }
    parts.join(": ")
}

/// 上游自由文案 → `FailoverError`，与真相源（`llm_error_class`）同判。
///
/// 流内错误事件（anthropic `error` / codex `response.failed`）与 CLI 型 lane
/// 的 stderr/事件文案此前恒落 `Unknown`（不带 "connect error" 前缀 = 不可
/// 重试）——上游显式瞬态文案（"quota exceeded"/"overloaded"）反而放弃
/// fallback 轮转，比断流（Timeout 可重试）处置更差。
/// 分类命中 → 对应变体（轮转语义与同类 HTTP 状态码同判）；Format →
/// `Unknown` 终态（原文案随 message 保留）；**未收录文案 → "stream
/// failure (model X):" 前缀 Unknown 可重试**（黑名单架构 2026-11-24 C1：
/// 未识别默认可重试，codex 官方 parse_failed_response 同语义——白名单
/// 永远有词表漏网，一词之差即终局中断会话）。
pub fn classify_message_error(provider: &str, model: &str, msg: &str) -> FailoverError {
    use nemesis_utils::llm_error_class::{LlmErrorClass, classify_llm_error};
    match classify_llm_error(msg) {
        // 文本层「overloaded 文案按限流对待」是既有约定（class_by_message
        // 同判）——两者同臂归 RateLimit（可轮转 + 退避），与 HTTP 429 处置
        // 一致。
        Some(LlmErrorClass::RateLimit | LlmErrorClass::Overloaded) => FailoverError::RateLimit {
            provider: provider.to_string(),
            model: model.to_string(),
            retry_after: None,
        },
        Some(LlmErrorClass::Timeout) => FailoverError::Timeout {
            provider: provider.to_string(),
            model: model.to_string(),
        },
        Some(LlmErrorClass::Auth) => FailoverError::Auth {
            provider: provider.to_string(),
            model: model.to_string(),
            status: 0,
        },
        // 403 文案形态（BUG-20261123-012）：status 0（纯文案无状态码，
        // Auth 同约定）；message 携带原文案。
        Some(LlmErrorClass::Forbidden) => FailoverError::Forbidden {
            provider: provider.to_string(),
            model: model.to_string(),
            status: 0,
            message: msg.to_string(),
        },
        Some(LlmErrorClass::Billing) => FailoverError::Billing {
            provider: provider.to_string(),
        },
        // B29/D1：上下文溢出归 Unknown 携带原文（不新增 FailoverError
        // 变体——换 provider 大概率同溢，不轮转语义正确；loop 层靠分类
        // 结果/文案族路由进响应式压缩，不靠 Failover 变体）。
        Some(LlmErrorClass::ContextOverflow) => FailoverError::Unknown {
            provider: provider.to_string(),
            message: msg.to_string(),
        },
        // Format = 终态（协议/请求格式不兼容，重试无意义）——原文案随
        // message 保留，不带前缀（黑名单语义：已识别终态类不翻转）。
        Some(LlmErrorClass::Format) => FailoverError::Unknown {
            provider: provider.to_string(),
            message: msg.to_string(),
        },
        // 黑名单架构（2026-11-24 C1）：未收录文案 = 流内失败默认可重试
        // （codex 官方 parse_failed_response 同语义——未识别 code 默认
        // ApiError::Retryable）。包 "stream failure (model X):" 前缀落
        // Unknown：is_retriable 前缀门放行 fallback 轮转 +
        // classify_llm_error 入口短路归 Timeout 类 + loop 瞬态词表
        // 三面同判。词表此后只服务「更快归对桶」（退避策略精细度），
        // 不再决定「能否自愈」。现网事故串
        // "Upstream response stream was interrupted"（与词表
        // "stream interrupted" 一词之差）由此整体翻转可自愈。
        None => FailoverError::Unknown {
            provider: provider.to_string(),
            message: format!("stream failure (model {model}): {msg}"),
        },
    }
}

/// 流内错误事件载荷 → `FailoverError` **统一分类入口**（2026-11-17 治本批）。
///
/// 三 lane 此前各自手搓解析：openai 兼容 lane 只抽 message 丢 code（cc-switch
/// 429 风暴根因——流内 `{"error":{"code":429,...}}` 的状态码在采集点被丢弃，
/// 文案不在两级词表 → `Unknown` 不可重试，429 信号永不进限流退避环）；
/// anthropic lane 私有 type 映射表；codex lane `SseOutcome::Failed` 只携文案。
/// 本函数单点收敛，探测序：
///
/// ① **数字码探测**（治本核心）：error 子对象（openai 家族 `/error`、codex
///    `response.failed` 的 `/response/error`；两者皆无则整个 payload——
///    codex 裸 `error` 事件形态）上的 `code` / `status` / `status_code`
///    三键，数字或纯数字字符串；0 / 非法 / 真相源不收录的码跳过。
///    命中 → [`FailoverError::from_status`]（`class_by_status` 单一真相源）。
/// ② **字符串语义码 / type 探测**（错误码补全批 2026-11-24，原 anthropic
///    lane 私有六枚举表并入 [`nemesis_utils::llm_error_class::class_by_code`]
///    单一真相源）：同三键的字符串值（openai `"insufficient_quota"` /
///    `"invalid_request_error"`、codex 终态黑名单族、openclaw 跨 provider
///    码表、anthropic 枚举）→ [`Self::from_class`]。**先于 ③ 文案**——码的
///    特异性高于自由文案，`insufficient_quota` 的官方文案
///    "You exceeded your current quota" 不得被限流词表错吸进瞬态桶；
///    message 缺失只剩码的形态不再落 ④ 可重试。
/// ③ **message 文案**：[`classify_message_error`]（`llm_error_class` 同判）；
///    `error` 本身是字符串的形态（部分网关）同样收录。
/// ④ 全无 → `Unknown{"stream failure (model X): " + fallback_msg}`（黑名单
/// 架构 C1：未识别默认可重试，调用方现场描述随前缀保留）。
pub fn classify_stream_error_payload(
    event: &serde_json::Value,
    provider: &str,
    model: &str,
    fallback_msg: String,
) -> FailoverError {
    // error 子对象定位：openai 家族在 /error；codex response.failed 在
    // /response/error；两者皆无 → 整个 payload 即错误对象。
    let err_obj = event
        .get("error")
        .filter(|v| v.is_object())
        .or_else(|| event.pointer("/response/error").filter(|v| v.is_object()))
        .unwrap_or(event);

    // ① 码探测：code / status / status_code，数字或纯数字字符串。
    for key in ["code", "status", "status_code"] {
        let Some(v) = err_obj.get(key) else {
            continue;
        };
        let raw = match v {
            serde_json::Value::Number(n) => n.as_u64(),
            serde_json::Value::String(s) => s.trim().parse::<u64>().ok(),
            _ => None,
        };
        let Some(code) = raw.filter(|&c| c != 0).and_then(|c| u16::try_from(c).ok()) else {
            continue;
        };
        // 只在真相源收录该码时采纳——未收录码（418 等）不产
        // `Unknown{"status N: "}`（会掩埋 message 真语义），把探测机会
        // 让给 ②③。
        if nemesis_utils::llm_error_class::class_by_status(code).is_some() {
            return FailoverError::from_status(provider, model, code, "", None);
        }
    }

    // ② 字符串语义码 / type 探测（错误码补全批 2026-11-24，原 anthropic
    // 私有六枚举表并入 `class_by_code` 单一真相源——overloaded_error/
    // api_error/rate_limit_error/authentication_error/permission_error/
    // billing_error/timeout_error 全部在码表，零回退；新增 openai 家族
    // type（invalid_request_error/server_error/insufficient_quota）与
    // codex/openclaw 语义码全表）。此前字符串码显式跳过（假设「语义在
    // message 里」被三方调查推翻）——message 缺失/文案词表 miss 时
    // quota 类字符串码落 ④ 前缀化可重试，终态错误进瞬态桶烧预算；
    // openai `insufficient_quota` 的官方文案 "You exceeded your current
    // quota" 更会被限流词表错吸——码先于文案定桶。
    // 流内 SSE error 事件无 HTTP 状态码 → status 0（纯文案形态，Auth
    // 同约定）；message 携带 err_obj 原文案（④ 兜底收不到——② 命中即
    // return）。
    if let Some((code_class, code_raw)) = ["code", "type", "status_code"].iter().find_map(|key| {
        let raw = err_obj.get(*key).and_then(|v| v.as_str())?;
        nemesis_utils::llm_error_class::class_by_code(raw).map(|c| (c, raw))
    }) {
        // message 缺失/空白 → 码原文兜底：码字面自身常是文案族成员
        // （"context_length_exceeded" 命中 B29 文案族 → loop 压缩环路由
        // 不丢；"invalid_request_error" 等终态码无词表消费需求，携带码
        // 原文即可观测）。
        let msg_text = err_obj
            .get("message")
            .and_then(|m| m.as_str())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(code_raw);
        return FailoverError::from_class(provider, model, code_class, 0, msg_text, None);
    }

    // ③ message 文案兜底：error 子对象 message 优先；`error` 本身是字符串
    // 的形态（部分网关）同样收录。
    let msg = err_obj
        .get("message")
        .and_then(|m| m.as_str())
        .or_else(|| event.get("error").and_then(|e| e.as_str()));
    if let Some(msg) = msg {
        return classify_message_error(provider, model, msg);
    }

    // ④ 全无 → 前缀化 Unknown（黑名单架构 2026-11-24 C1）：连 message
    // 都没有的流内失败事件默认可重试（codex 官方同语义），调用方现场
    // 描述随前缀保留（excerpt 里的 JSON 字段/数字不参与分类——入口
    // 短路先于一切词表，污染免疫）。
    FailoverError::Unknown {
        provider: provider.to_string(),
        message: format!("stream failure (model {model}): {fallback_msg}"),
    }
}

#[cfg(test)]
mod tests;
