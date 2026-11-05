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
