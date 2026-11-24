//! （扩展计划）：LLM 错误分类器单测。
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
    // 诚实化配套：transport_error 盖章的 "connect error" 前缀
    // 必须先于 auth/format 判定——错误链里的 URL/系统文案可能含孤立三位
    // 数字（端口号:401/、"os error 10061"），不能被 \b401\b 误归 Auth。
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
fn status_504_maps_to_timeout() {
    // 504（Gateway Timeout）此前缺席——同族 408 已归
    // Timeout，loop transient 词表也有 "504"。不含 "timeout" 字样的裸
    // 状态码形态是红证锚（修复前 extract → 504 → None → 文案分支也无
    // 命中 → None 不可重试）。
    assert_eq!(
        classify_llm_error("status 504"),
        Some(LlmErrorClass::Timeout)
    );
}

#[test]
fn http_status_line_form_is_classified() {
    // 红证锚：`extract_http_status` 的 "HTTP[/\s]+..."
    // 模式写的是大写字面 "HTTP"，而 classify_llm_error / providers
    // classify_error 传给它的都是 to_lowercase() 后的文本——纯状态行形态
    // （"HTTP/1.1 503 Service Unavailable"，reqwest 常见 Display 之一）在
    // classify 语境下 extract 恒 miss，词表又无裸 "503"/"502"，分类落 None
    // → fallback 不轮转；同一输入直调 extract_http_status 却能拿 Some(503)
    // （providers tests 钉住的形态）——同一 pub 函数两语境分叉。
    assert_eq!(
        classify_llm_error("HTTP/1.1 503 Service Unavailable"),
        Some(LlmErrorClass::Overloaded)
    );
    // lowercase 后仍带 "http/1.1" 前缀的形态同样必须命中（修复前同死）。
    assert_eq!(
        classify_llm_error("http/2 502 bad gateway"),
        Some(LlmErrorClass::Overloaded)
    );
    // 状态行里的 429 也应经 extract → RateLimit（修复前落文案词表恰好
    // 含裸 "429" 误打误撞命中——钉住走的是 extract 通道而非词表巧合）。
    assert_eq!(
        classify_llm_error("HTTP/1.1 429 Too Many Requests"),
        Some(LlmErrorClass::RateLimit)
    );
    // 首字母大写 "Status:" 形态（原始大小写文本直调 pub 函数）——修复前
    // pattern 1 只认小写 "status"，同样 miss。
    assert_eq!(extract_http_status("Status: 429"), Some(429));
}

#[test]
fn rate_limit_429_matching_requires_word_boundary() {
    // BUG-20261107-019 红证锚：裸 contains("429") 把长数字里嵌的 429
    // （毫秒时长/端口号/计数）误判 RateLimit——比 Timeout 误判更重：
    // 消费方把 RateLimit 记 provider cooldown + 限流退避梯子（5→210s），
    // 而真形态应归 Timeout/瞬态快速重试。词边界纪律与 \b402\b/\b401\b 对齐。
    // 毫秒时长形态：timeout 文案 + 数字嵌 429 → 必须归 Timeout（修复前进
    // 限流词表 = RateLimit）。
    assert_eq!(
        classify_llm_error("request timed out after 2429ms"),
        Some(LlmErrorClass::Timeout)
    );
    // 端口形态（无 "connect error" 盖章的裸 reqwest Display——流内错误事件
    // 原样透传的形态）：数字嵌 429 不得命中限流词表。
    assert_eq!(
        classify_llm_error("error sending request for url (http://127.0.0.1:8429/v1)"),
        None
    );
    // 真 429（独立成词）照常命中——既有形态零回归。
    assert_eq!(
        classify_llm_error("error: 429 slow down"),
        Some(LlmErrorClass::RateLimit)
    );
    assert_eq!(
        classify_llm_error("status 429"),
        Some(LlmErrorClass::RateLimit)
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

#[test]
fn extract_status_requires_trailing_digit_boundary() {
    // BUG-20261121-001 红证锚：extract 层 `(\d{3})` 无尾随数字界，
    // "status 4290" 按前 3 位截取 → 429 → RateLimit（cooldown+限流退避
    // 梯子误路由）——词表层 `\b429\b`（BUG-20261107-019 纪律）已堵，
    // extract 层同族漏网。修复 = 3 位后必须紧跟非数字/串尾（`(?:[^\d]|$)`；
    // regex crate 不支持 lookahead，不能写 `(?!\d)`）：4+ 位数字整段落
    // None 走词表分支，词表 `\b429\b` 同样不命中长数字 → 诚实不分类
    // （两层一致）。修复前前两个断言分别返回 Some(429)、classify 返回
    // RateLimit。
    assert_eq!(extract_http_status("status 4290"), None);
    assert_eq!(extract_http_status("HTTP/1.1 4290"), None);
    assert_eq!(classify_llm_error("status 4290"), None);
    // 4+ 位但前 3 位非限流码同样不截取（截了也是错码——"20000" 截 200
    // 虽无害，但截取行为本身就是错的，一并钉死）。
    assert_eq!(extract_http_status("status: 20000"), None);
    // 合法 3 位形态零回归（后随串尾/标点/空白/字母）。
    assert_eq!(extract_http_status("status 429"), Some(429));
    assert_eq!(extract_http_status("status: 503."), Some(503));
    assert_eq!(extract_http_status("HTTP/1.1 429 Too Many"), Some(429));
}

// ---- U-B（端点不可用封存）：连接类失败判定 ----

#[test]
fn connect_error_wording_forms() {
    // reqwest Display 单层 source 形态（error sending request 之下的连接层）
    assert!(is_connect_error(
        "error sending request for url (http://127.0.0.1:15721/v1): error trying to connect"
    ));
    assert!(is_connect_error(
        "error sending request for url (http://127.0.0.1:15721/v1): tcp connect error"
    ));
    // OS 文案形态（Windows WSA / Linux errno，本地化文案前的 errno 段稳定）
    assert!(is_connect_error(
        "由于目标计算机积极拒绝，无法连接。 (os error 10061)"
    ));
    assert!(is_connect_error("Connection refused (os error 111)"));
    assert!(is_connect_error(
        "dns error: failed to lookup address information"
    ));
    assert!(is_connect_error("no address associated with hostname"));
    assert!(is_connect_error("connect error: no route to host"));
}

#[tokio::test]
async fn connect_error_real_reqwest_display_is_classified() {
    // 真实 reqwest 连接失败形态钉住（分类器与传输层的契约测试）：
    // 端口 1（tcpmux）常规无监听。**裸 Display 不含连接词**（只有
    // "error sending request for url (…)"——这正是 E2E-9 验收抓出的
    // 判定失明根因），必须走 [`error_chain_msg`] 拼出 source 链全文
    // （… client error (Connect): tcp connect error: … (os error 10061)）
    // 词表才命中。
    let err = reqwest::Client::new()
        .get("http://127.0.0.1:1/")
        .send()
        .await;
    match err {
        Err(e) => {
            // 裸 Display 确实失明（回归锁：词表永远不应依赖裸 Display）。
            let bare = e.to_string();
            assert!(
                !is_connect_error(&bare),
                "裸 Display 不应命中连接词表（失明是已知形态，链才是判定输入）：{bare}"
            );
            let chain = error_chain_msg(&e);
            assert!(
                is_connect_error(&chain),
                "error_chain_msg 全链未命中连接词表：{chain}"
            );
        }
        Ok(_) => {
            // 本机 1 端口真有监听（异常环境）：分类器词表测试已覆盖，诚实跳过。
            eprintln!("SKIP: 127.0.0.1:1 有监听，无法产生真实连接失败样本");
        }
    }
}

#[test]
fn timeout_and_http_errors_are_not_connect_class() {
    // 超时不计入（慢模型跑满 provider 超时 ≠ 端点死）——U-B 设计红线。
    assert!(!is_connect_error("operation timed out"));
    assert!(!is_connect_error(
        "error sending request for url (http://x): operation timed out"
    ));
    assert!(!is_connect_error("request timeout: deadline exceeded"));
    // HTTP 状态错（429/5xx）= 端点活着，不计入。
    assert!(!is_connect_error("HTTP status 429 too many requests"));
    assert!(!is_connect_error("status: 503 service unavailable"));
    // 解析失败 = 端点活着，不计入。
    assert!(!is_connect_error(
        "评审输出连续 3 轮无法解析：missing verdict"
    ));
    assert!(!is_connect_error(""));
}

#[test]
fn error_chain_msg_preserves_source_chain_for_connect_detection() {
    // 回归锁（E2E-9 验收）：failover 层包装错误时必须保留 source 链——
    // 否则 is_connect_error 对「connection refused / os error 10061」失明，
    // 端点封存（board.unavailable）永远不触发。
    use std::error::Error;
    use std::fmt;

    #[derive(Debug)]
    struct Leaf(&'static str);
    impl fmt::Display for Leaf {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{}", self.0)
        }
    }
    impl Error for Leaf {}

    #[derive(Debug)]
    struct Wrap {
        leaf: Leaf,
    }
    impl fmt::Display for Wrap {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "timeout calling provider anthropic/glm")
        }
    }
    impl Error for Wrap {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            Some(&self.leaf)
        }
    }

    let w = Wrap {
        leaf: Leaf("由于目标计算机积极拒绝，无法连接。 (os error 10061)"),
    };
    let chain = error_chain_msg(&w);
    assert!(chain.starts_with("timeout calling provider"));
    assert!(is_connect_error(&chain), "链拼接后应命中连接词表：{chain}");
    // 裸顶层（无链）不应命中——词表纪律保持。
    assert!(!is_connect_error(&w.to_string()));
}

// -- 流中断入口短路（对抗用例）----------------------

/// 构造 StreamInterrupted 的真实 Display 形态（头行 + 注记框架 + partial）。
fn stream_interrupted_display(partial: &str) -> String {
    format!(
        "stream interrupted from provider openai/gpt-4: connection reset（已接收 {} 字符，中断前部分文本随错误通知保留）\n\n--- 以下为中断前已生成的部分内容 ---\n{}",
        partial.chars().count(),
        partial
    )
}

#[test]
fn l02_prefix_short_circuits_to_timeout_despite_polluted_partial() {
    // partial 全文含 429 / token / expired / unauthorized 污染词——
    // 入口前缀短路必须恒 Timeout 类，状态码提取与全部词表不许碰到 partial。
    let display = stream_interrupted_display(
        "上游返回 status: 429 且 token 计数异常，授权已 expired (unauthorized)，rate limit exceeded",
    );
    assert_eq!(classify_llm_error(&display), Some(LlmErrorClass::Timeout));
}

#[test]
fn l02_multiline_partial_with_fake_error_line_short_circuits() {
    // 多行 partial 内嵌完整 "Error: fake..." 伪行——前缀锚只认头行，
    // 尾段内容不参与分类。
    let display = stream_interrupted_display(
        "第一行\nError: fake context length overflow 999999 tokens\ninvalid request",
    );
    assert_eq!(classify_llm_error(&display), Some(LlmErrorClass::Timeout));
}

#[test]
fn l02_non_prefixed_text_with_stream_word_not_short_circuited() {
    // 短路只认头行前缀锚（starts_with）——普通错误文本里恰好含
    // "stream interrupted" 字样不享受短路（词表照常工作）。
    let msg = "upstream error: stream interrupted budget; invalid request format";
    // "invalid request" → Format（词表既有行为，不被 短路劫持）。
    assert_eq!(classify_llm_error(msg), Some(LlmErrorClass::Format));
}

// ---- B29/D1（2026-11-06）上下文溢出文案族 ----

#[test]
fn context_overflow_family_forms_are_classified() {
    // 高特异性短语族（各厂商真实文案形态）。
    let positives = [
        // OpenAI 兼容 lane 经典形态。
        "This model's maximum context length is 8192 tokens",
        "context_length_exceeded",
        // Anthropic 形态。
        "prompt is too long: 250000 tokens > 200000 maximum",
        "your prompt too long for this model",
        // 通用变体。
        "maximum context length exceeded",
        "max context window reached",
        "too many input tokens",
        "input tokens exceed the model limit",
        // s-less 超集（exceeds/exceeded/exceeding 全命中）。
        "token count exceeds allowed maximum",
        // 泛词双词同现族（exceeds the maximum + token/context/prompt）。
        "number of tokens exceeds the maximum allowed",
        "the context exceeds the maximum supported size",
        "reduce the length of your messages",
        // input length + exceed/limit。
        "input length exceeds limit",
        "input length limit reached",
        // 中文厂商文案族。
        "输入长度超出模型上限",
        "请求超出模型上下文窗口",
        "上下文长度超限",
    ];
    for msg in positives {
        assert_eq!(
            classify_llm_error(msg),
            Some(LlmErrorClass::ContextOverflow),
            "溢出文案族应归 ContextOverflow：{msg}"
        );
    }
    // 大小写混合（真相源内部自 lowercase，调用方传原始大小写文本同判）。
    assert!(is_context_overflow("Your Prompt Is Too Long"));
}

#[test]
fn context_overflow_negatives_do_not_misclassify() {
    // G2 失败模式回归锁：泛词不得裸误配——图像/凭据/限流等既有分类不被劫持。
    // "image exceeds the maximum size" 缺 token/context/prompt 同现 → 不入族
    // （否则图像错误被误吸进压缩环，白受必败重呼）。
    assert!(!is_context_overflow(
        "image exceeds the maximum size of 25 mb"
    ));
    assert_eq!(
        classify_llm_error("image exceeds the maximum size of 25 mb"),
        Some(LlmErrorClass::Format)
    );
    // 裸 "exceeds the maximum" 无主题词 → 不入族。
    assert!(!is_context_overflow("value exceeds the maximum"));
    // 裸 "input length" 无 exceed/limit → 不入族。
    assert!(!is_context_overflow("please check your input length"));
    // 空串/无关文本。
    assert!(!is_context_overflow(""));
    assert!(!is_context_overflow("totally unknown thing"));
    // 既有分类不被劫持（auth 负例是词表内部 proven：matches_auth 有
    // "invalid token"/"expired"，溢出族不含这些词）。
    assert_eq!(
        classify_llm_error("invalid token provided"),
        Some(LlmErrorClass::Auth)
    );
    assert_eq!(
        classify_llm_error("invalid api key"),
        Some(LlmErrorClass::Auth)
    );
    assert_eq!(
        classify_llm_error("invalid request format: unknown field `foo`"),
        Some(LlmErrorClass::Format)
    );
    assert_eq!(
        classify_llm_error("request timeout"),
        Some(LlmErrorClass::Timeout)
    );
    assert_eq!(
        classify_llm_error("too many requests"),
        Some(LlmErrorClass::RateLimit)
    );
}

#[test]
fn l02_short_circuit_still_precedes_overflow_family() {
    // L02 头行前缀锚必须仍最先行——partial 尾段携带的溢出文案不得把流中断
    // 误吸进压缩环（流中断是瞬态可重试，溢出是确定性失败，处置相反）。
    let display = stream_interrupted_display(
        "上游说 prompt is too long: 999999 tokens > 8192 maximum context",
    );
    assert_eq!(classify_llm_error(&display), Some(LlmErrorClass::Timeout));
}

#[test]
fn concurrency_limit_wording_maps_rate_limit() {
    // 2026-11-17 治本批：cc-switch / one-api 系流内限流实测文案——流内
    // error 事件码采集缺失时的文案兜底（loop 层 RATE_LIMIT_ERROR_KEYWORDS
    // 双写同步）。
    assert_eq!(
        classify_llm_error("Concurrency limit exceeded for account"),
        Some(LlmErrorClass::RateLimit)
    );
}

// ---- 403 与 Auth 分家（BUG-20261123-012）Forbidden ----

#[test]
fn forbidden_403_and_policy_phrases_map_forbidden() {
    // 状态码形态（Daybreak 实测链路：HTTP 403 + flag body）。
    assert_eq!(
        classify_llm_error("status 403: request rejected"),
        Some(LlmErrorClass::Forbidden)
    );
    // 裸 "forbidden" 词形态（无状态码）。
    assert_eq!(
        classify_llm_error("forbidden"),
        Some(LlmErrorClass::Forbidden)
    );
    // Daybreak 无状态码纯文案形态（只能靠词表兜底的形态——修复前落
    // matches_auth 的 "forbidden" 词归 Auth 驱动凭据卡循环重放）。
    assert_eq!(
        classify_llm_error(
            "This content was flagged per our content policy and cannot be processed"
        ),
        Some(LlmErrorClass::Forbidden)
    );
    // 内容风控/策略短语族各形态。
    let positives = [
        "content policy violation",
        "your request was blocked by content filtering",
        "flagged by our content management policy",
        "output rejected by moderation system",
        "violates the usage policy",
        "your access has been denied",
        "access denied by administrator",
    ];
    for msg in positives {
        assert_eq!(
            classify_llm_error(msg),
            Some(LlmErrorClass::Forbidden),
            "拒绝访问文案应归 Forbidden：{msg}"
        );
    }
}

#[test]
fn forbidden_split_does_not_regress_auth_and_neighbors() {
    // 401 必须仍归 Auth（分家回归锁）——状态码与文案双形态。
    assert_eq!(
        classify_llm_error("status 401: unauthorized"),
        Some(LlmErrorClass::Auth)
    );
    assert_eq!(
        classify_llm_error("invalid api key provided"),
        Some(LlmErrorClass::Auth)
    );
    assert_eq!(
        classify_llm_error("token has expired"),
        Some(LlmErrorClass::Auth)
    );
    // 相邻类别不被劫持。
    assert_eq!(
        classify_llm_error("status 402: payment required"),
        Some(LlmErrorClass::Billing)
    );
    assert_eq!(
        classify_llm_error("too many requests"),
        Some(LlmErrorClass::RateLimit)
    );
}

#[test]
fn forbidden_403_matching_requires_word_boundary() {
    // 与 \b401\b/\b429\b 同款词界纪律：长数字里嵌的 403（端口号 :8403/、
    // 计数 4030）不得命中 Forbidden。
    assert_eq!(
        classify_llm_error("error sending request for url (http://127.0.0.1:8403/v1)"),
        None
    );
    // 真 403（独立成词）照常命中。
    assert_eq!(
        classify_llm_error("error: 403 from upstream"),
        Some(LlmErrorClass::Forbidden)
    );
}

// ==========================================================================
// 黑名单架构语料库（2026-11-24 C1/C2/C3）
// ------------------------------------------------------------------
// 现网事故串 "Upstream response stream was interrupted"（codex 中转礼貌
// 报错的应用层事件）与既有词表 "stream interrupted" 一词之差漏网——
// 本组钉死 C1 前缀短路 + C2 瞬态措辞族 + 误报防护。
// ==========================================================================

#[test]
fn stream_failure_prefix_short_circuit_returns_timeout() {
    // C1：classify_stream_error_payload ③None/④ 前缀化 Unknown 的
    // Display 展平形态（前段 "unknown error from provider X: "，
    // contains 锚自产前缀）。短路先于一切词表——前缀内携带的上游
    // 原文/excerpt（auth 词、429 数字、401 码）不得改判。
    let cases = [
        "unknown error from provider codex: stream failure (model m): Upstream response stream was interrupted",
        "unknown error from provider codex: stream failure (model m): totally novel hiccup",
        "unknown error from provider codex: stream failure (model m): {\"reason\":\"expired token\"}",
        "unknown error from provider codex: stream failure (model m): {\"remaining\":429}",
    ];
    for msg in cases {
        assert_eq!(
            classify_llm_error(msg),
            Some(LlmErrorClass::Timeout),
            "流内失败前缀必须短路归 Timeout：{msg}"
        );
    }
}

#[test]
fn transient_stream_wording_corpus_maps_timeout() {
    // C2 瞬态措辞族语料库（openclaw TRANSIENT_RETRY_EVIDENCE_RE /
    // message-patterns + gemini-cli RETRYABLE_NETWORK_CODES + 真机事故串）。
    let positives = [
        // 真实事故串（was 之差——与既有 "stream interrupted" 是不同子串）：
        "Upstream response stream was interrupted",
        // 既有同族（loop 瞬态词表对称补齐）：
        "request failed: stream interrupted mid-response",
        // openclaw message-patterns 断流措辞：
        "stream terminated before completion",
        "stream ended without a final response",
        "other side closed connection",
        "socket hang up (ECONNRESET)",
        // openclaw 传输断连措辞：
        "connection lost to upstream",
        "connection reset by peer",
        // 服务端明示可重试：
        "capacity reached, you can retry your request",
        // 真机实证（GLM 文案兜底臂）：
        "网络错误，请稍后重试",
        // gemini-cli RETRYABLE_NETWORK_CODES（errno 码形态）：
        "fetch failed: ECONNRESET",
        "request failed: ETIMEDOUT after 30s",
        "upstream ECONNREFUSED",
        "tunnel aborted ECONNABORTED",
        "write EPIPE",
    ];
    for msg in positives {
        assert_eq!(
            classify_llm_error(msg),
            Some(LlmErrorClass::Timeout),
            "瞬态措辞族应归 Timeout：{msg}"
        );
    }
}

#[test]
fn transient_stream_wording_false_positive_guards() {
    // 误报防护（openclaw 锚定纪律同门）：刻意不加的裸词/裸数字不得因
    // 新词族被误判瞬态。
    // 裸 "interrupted" 不收录——取消语义不是传输瞬态（重试无意义）。
    assert_eq!(classify_llm_error("request interrupted by user"), None);
    // 裸 "terminated" 不收录——进程终局非传输断连。
    assert_eq!(
        classify_llm_error("process terminated with exit code 1"),
        None
    );
    // 裸 5xx 数字误报防护（openclaw "exited with 503 lines" 教训，
    // 词边界数字锚既有纪律不因新词族松动）。
    assert_eq!(classify_llm_error("grep exited with 503 lines"), None);
    // 终态类不被新词族劫持：含 "ended"/"lost" 字样的认证/计费文案。
    assert_eq!(
        classify_llm_error("session ended: invalid api key"),
        Some(LlmErrorClass::Auth)
    );
    assert_eq!(
        classify_llm_error("subscription lost, credit balance exhausted"),
        Some(LlmErrorClass::Billing)
    );
    // 分类不出仍 None（兜底语义上移到 providers 侧黑名单前缀，
    // 纯文本分类器本体不产「默认可重试」结论）。
    assert_eq!(classify_llm_error("totally novel failure mode"), None);
}

// ==========================================================================
// class_by_code 错误码单一真相源（2026-11-24 错误码补全批）
// ------------------------------------------------------------------
// 三方调查合并表：codex parse_terminal_provider_error 终态黑名单 +
// openclaw FAILOVER_CODE_REASONS/FAILOVER_ERROR_TYPE_REASONS + openai
// 官方 type + anthropic 官方六枚举。归一后精确匹配。
// ==========================================================================

#[test]
fn class_by_code_terminal_families() {
    use LlmErrorClass as C;
    // —— Billing：三方一致的核心码（openai 官方 429 type 语义=配额耗尽）——
    assert_eq!(class_by_code("insufficient_quota"), Some(C::Billing));
    assert_eq!(class_by_code("credit_balance_exhausted"), Some(C::Billing));
    assert_eq!(class_by_code("usage_not_included"), Some(C::Billing));
    assert_eq!(class_by_code("budget_reached"), Some(C::Billing));
    assert_eq!(class_by_code("insufficient_balance"), Some(C::Billing));
    // codex spend limit 三变体（后缀族一条）：
    assert_eq!(
        class_by_code("api_key_spend_limit_exceeded"),
        Some(C::Billing)
    );
    assert_eq!(
        class_by_code("project_spend_limit_exceeded"),
        Some(C::Billing)
    );
    assert_eq!(
        class_by_code("organization_spend_limit_exceeded"),
        Some(C::Billing)
    );
    // —— Auth ——
    assert_eq!(class_by_code("invalid_api_key"), Some(C::Auth));
    assert_eq!(class_by_code("authentication_error"), Some(C::Auth));
    // —— Forbidden ——
    assert_eq!(class_by_code("permission_error"), Some(C::Forbidden));
    assert_eq!(class_by_code("permission_denied"), Some(C::Forbidden));
    assert_eq!(class_by_code("inference_hook_denied"), Some(C::Forbidden));
    assert_eq!(class_by_code("cyber_policy"), Some(C::Forbidden));
    assert_eq!(class_by_code("bio_policy"), Some(C::Forbidden));
    // codex policy violation 后缀族（content/misalignment 两代表）：
    assert_eq!(
        class_by_code("content_policy_violation"),
        Some(C::Forbidden)
    );
    assert_eq!(
        class_by_code("misalignment_policy_violation"),
        Some(C::Forbidden)
    );
    // —— Format（终态）——
    assert_eq!(class_by_code("invalid_request_error"), Some(C::Format));
    assert_eq!(class_by_code("invalid_prompt"), Some(C::Format));
    assert_eq!(class_by_code("unknown_parameter"), Some(C::Format));
    assert_eq!(class_by_code("model_not_found"), Some(C::Format));
    assert_eq!(class_by_code("access_program_not_enabled"), Some(C::Format));
    assert_eq!(class_by_code("invalid_access_program"), Some(C::Format));
    assert_eq!(class_by_code("unsupported_access_program"), Some(C::Format));
    assert_eq!(class_by_code("invalid_audio_data"), Some(C::Format));
    assert_eq!(class_by_code("missing_audio_sample_rate"), Some(C::Format));
    // —— ContextOverflow（loop 压缩环路由归宿，比三方更聪明的桶）——
    assert_eq!(
        class_by_code("context_length_exceeded"),
        Some(C::ContextOverflow)
    );
}

#[test]
fn class_by_code_transient_families() {
    use LlmErrorClass as C;
    // —— RateLimit：openclaw 码表 + anthropic 枚举 + 文案词表同判 ——
    assert_eq!(class_by_code("rate_limit"), Some(C::RateLimit));
    assert_eq!(class_by_code("rate_limited"), Some(C::RateLimit));
    assert_eq!(class_by_code("rate_limit_exceeded"), Some(C::RateLimit));
    assert_eq!(class_by_code("rate_limit_error"), Some(C::RateLimit));
    assert_eq!(class_by_code("billing_error"), Some(C::Billing));
    assert_eq!(class_by_code("quota_exceeded"), Some(C::RateLimit));
    assert_eq!(class_by_code("too_many_requests"), Some(C::RateLimit));
    // AWS throttling 族（四形态）：
    assert_eq!(class_by_code("throttled"), Some(C::RateLimit));
    assert_eq!(class_by_code("throttling"), Some(C::RateLimit));
    assert_eq!(class_by_code("throttling_exception"), Some(C::RateLimit));
    assert_eq!(class_by_code("throttlingexception"), Some(C::RateLimit));
    assert_eq!(class_by_code("resource_exhausted"), Some(C::RateLimit));
    assert_eq!(class_by_code("model_cooldown"), Some(C::RateLimit));
    assert_eq!(class_by_code("slow_down"), Some(C::RateLimit));
    assert_eq!(class_by_code("flex_unavailable"), Some(C::RateLimit));
    // —— Overloaded ——
    assert_eq!(class_by_code("overloaded"), Some(C::Overloaded));
    assert_eq!(class_by_code("overloaded_error"), Some(C::Overloaded));
    assert_eq!(class_by_code("api_error"), Some(C::Overloaded));
    assert_eq!(class_by_code("server_error"), Some(C::Overloaded));
    assert_eq!(class_by_code("upstream_error"), Some(C::Overloaded));
    assert_eq!(
        class_by_code("policy_capacity_unavailable"),
        Some(C::Overloaded)
    );
    // —— Timeout ——
    assert_eq!(class_by_code("timeout_error"), Some(C::Timeout));
    assert_eq!(class_by_code("ehostdown"), Some(C::Timeout));
    assert_eq!(class_by_code("enetreset"), Some(C::Timeout));
    assert_eq!(
        class_by_code("err_stream_premature_close"),
        Some(C::Timeout)
    );
}

#[test]
fn class_by_code_normalization_tolerance() {
    use LlmErrorClass as C;
    // 大小写宽容（上游码大小写不统一是常态——AWS THROTTLING / Go 系大写）：
    assert_eq!(class_by_code("Insufficient_Quota"), Some(C::Billing));
    assert_eq!(class_by_code("RATE_LIMIT_EXCEEDED"), Some(C::RateLimit));
    assert_eq!(class_by_code("OVERLOADED_ERROR"), Some(C::Overloaded));
    // 连字符宽容（部分网关转 kebab-case）：
    assert_eq!(class_by_code("rate-limit-exceeded"), Some(C::RateLimit));
    assert_eq!(class_by_code("invalid-request-error"), Some(C::Format));
    // 前后空白宽容：
    assert_eq!(class_by_code("  insufficient_quota  "), Some(C::Billing));
}

#[test]
fn class_by_code_unlisted_and_contamination_guards() {
    // 未收录码 → None（诚实不上报类别，消费方走既有链）：
    assert_eq!(class_by_code("totally_unknown_code"), None);
    assert_eq!(class_by_code("internal_boom"), None);
    // 数字串不是语义码（HTTP 码归 class_by_status，码表不收——防双表
    // 漂移；"429" 不在码表）：
    assert_eq!(class_by_code("429"), None);
    assert_eq!(class_by_code("500"), None);
    // 精确匹配不 contains：自由文本里嵌码字样不得命中。
    assert_eq!(class_by_code("not_rate_limit_related"), None);
    assert_eq!(class_by_code("xxinvalid_api_keyxx"), None);
    assert_eq!(class_by_code("prefix_insufficient_quota_suffix"), None);
    // 后缀族不吞自由文本（spend_limit_exceeded 结尾校验）：
    assert_eq!(class_by_code("spend_limit_exceeded_was_waived"), None);
    // 空串 / 纯空白：
    assert_eq!(class_by_code(""), None);
    assert_eq!(class_by_code("   "), None);
}
