// error_classifier.rs 覆盖率补充测试（classify_error 的 HTTP 状态码抽取
// 分派臂 43-46：消息文本带 "status[: ]NNN" / "HTTP/2 NNN" 即进
// classify_by_status 分派）。

use super::*;
use crate::failover::FailoverError;

/// 消息内嵌状态码 → classify_by_status 四个代表分支（408/402/429/400）。
#[test]
fn status_codes_in_message_dispatch_by_status() {
    // 408 → Timeout。
    let e = classify_error("request failed: status: 408 after 30s", "prov-a", "model-a").unwrap();
    match e {
        FailoverError::Timeout { provider, model } => {
            assert_eq!(provider, "prov-a");
            assert_eq!(model, "model-a");
        }
        other => panic!("预期 Timeout，得到 {other:?}"),
    }

    // 402 → Billing。
    let e = classify_error("payment required (status 402)", "prov-a", "model-a").unwrap();
    assert!(matches!(e, FailoverError::Billing { .. }), "{e:?}");

    // HTTP/2 429 形态（第二正则）→ RateLimit。
    let e = classify_error("HTTP/2 429 too many requests", "p", "m").unwrap();
    assert!(matches!(e, FailoverError::RateLimit { .. }), "{e:?}");

    // 400 → Format（message = "status 400"）。
    let e = classify_error("Status: 400 bad request", "p", "m").unwrap();
    match e {
        FailoverError::Format { message, .. } => assert!(message.contains("400"), "{message}"),
        other => panic!("预期 Format，得到 {other:?}"),
    }
}
