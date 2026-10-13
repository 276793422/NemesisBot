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

// ---- U-B（2026-10-12 端点不可用封存）：连接类失败判定 ----

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
