//! Failover system for LLM providers.

use thiserror::Error;

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
            // 端点问题，换下一家。其余 Unknown（400/404 等应用层错误）
            // 维持不可重试——500 过载族自 起由 `from_status` 直接归
            // Overloaded，不再落 Unknown。
            FailoverError::Unknown { message, .. } => {
                message.starts_with("connect error")
                    || message.starts_with("response body exceeds limit")
            }
            _ => false,
        }
    }

    /// Get the failover reason.
    pub fn reason(&self) -> FailoverReason {
        match self {
            FailoverError::Auth { .. } => FailoverReason::Auth,
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
        match class_by_status(status) {
            Some(LlmErrorClass::Auth) => FailoverError::Auth {
                provider: provider.to_string(),
                model: model.to_string(),
                status,
            },
            Some(LlmErrorClass::RateLimit) => FailoverError::RateLimit {
                provider: provider.to_string(),
                model: model.to_string(),
                retry_after,
            },
            Some(LlmErrorClass::Billing) => FailoverError::Billing {
                provider: provider.to_string(),
            },
            Some(LlmErrorClass::Timeout) => FailoverError::Timeout {
                provider: provider.to_string(),
                model: model.to_string(),
            },
            Some(LlmErrorClass::Overloaded) => FailoverError::Overloaded {
                provider: provider.to_string(),
                // 真实上游状态码随错误携带：此前只带 provider，
                // 502（中转转发失败）与 503 分不清，归因盲点。Display 的
                // "(status: NNN)" 形态可被 llm_error_class::extract_http_status
                // 还原——展平文本回灌分类器时不丢状态码。
                status,
            },
            // 400（真相源归 Format）与未收录状态码：保持 Unknown 原文案
            // 形态——两类均不可重试（无行为差异），"status N: {body}" 是
            // loop 侧 is_context_error 词表的消费面，不做无行为收益的
            // 变体迁移。ContextOverflow 同臂（真相源状态表恒不产出，编译
            // 期穷尽要求在场；若未来状态表收录 overflow 码，body 仍随
            // message 保留，B29 文案族对 message 匹配照常命中，两条路同判）。
            Some(LlmErrorClass::Format | LlmErrorClass::ContextOverflow) | None => {
                FailoverError::Unknown {
                    provider: provider.to_string(),
                    message: format!(
                        "status {}: {}",
                        status,
                        body.chars().take(200).collect::<String>()
                    ),
                }
            }
        }
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
/// 分类命中 → 对应变体（轮转语义与同类 HTTP 状态码同判）；Format / 未收录
/// → `Unknown`（原文案随 message 保留，不可重试兜底语义不变）。
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
        Some(LlmErrorClass::Format) | None => FailoverError::Unknown {
            provider: provider.to_string(),
            message: msg.to_string(),
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
/// ① **码探测**（治本核心）：error 子对象（openai 家族 `/error`、codex
///    `response.failed` 的 `/response/error`；两者皆无则整个 payload——
///    codex 裸 `error` 事件形态）上的 `code` / `status` / `status_code`
///    三键，数字或纯数字字符串；0 / 非法 / 真相源不收录的码跳过（字符串
///    语义码如 `"insufficient_quota"` 不是码——语义在 message 里，落 ③）。
///    命中 → [`FailoverError::from_status`]（`class_by_status` 单一真相源）。
/// ② **anthropic 官方错误 type 枚举**：原 anthropic lane 私有表并入单点。
///    未收录 type **不判 Format**——openai 家族也带 type 字段（如
///    `"invalid_request_error"`），同一探测序两家族共用，语义交 ③ 定夺。
/// ③ **message 文案**：[`classify_message_error`]（`llm_error_class` 同判）；
///    `error` 本身是字符串的形态（部分网关）同样收录。
/// ④ 全无 → `Unknown{fallback_msg}`（调用方给的现场描述）。
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

    // ② anthropic 官方错误 type 枚举（原 lane 私有表并入单点）。
    if let Some(etype) = err_obj.get("type").and_then(|t| t.as_str()) {
        let mapped = match etype {
            "overloaded_error" | "api_error" => Some(FailoverError::Overloaded {
                provider: provider.to_string(),
                // SSE error 事件无 HTTP 状态码（流已在 200 里）→ 0（未
                // 携带），与 Auth 纯文案形态同约定。
                status: 0,
            }),
            "rate_limit_error" => Some(FailoverError::RateLimit {
                provider: provider.to_string(),
                model: model.to_string(),
                retry_after: None,
            }),
            "authentication_error" | "permission_error" => Some(FailoverError::Auth {
                provider: provider.to_string(),
                model: model.to_string(),
                status: 0,
            }),
            "billing_error" => Some(FailoverError::Billing {
                provider: provider.to_string(),
            }),
            "timeout_error" => Some(FailoverError::Timeout {
                provider: provider.to_string(),
                model: model.to_string(),
            }),
            _ => None,
        };
        if let Some(err) = mapped {
            return err;
        }
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

    // ④ 全无 → Unknown（调用方现场描述）。
    FailoverError::Unknown {
        provider: provider.to_string(),
        message: fallback_msg,
    }
}

#[cfg(test)]
mod tests;
