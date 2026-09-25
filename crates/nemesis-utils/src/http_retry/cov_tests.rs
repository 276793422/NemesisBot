// http_retry.rs 覆盖率补充测试（重试预算耗尽且无硬错误时的终局补跑臂
// 186-188：全程只回可重试状态码 → last_err 恒空 → 尾局再跑一次拿最终态）。

use super::*;

/// 恒 500（可重试）的桩响应。
struct AlwaysRetryable {
    status: u16,
}

impl HasStatusCode for AlwaysRetryable {
    fn status_code(&self) -> u16 {
        self.status
    }
}

/// max 次全部命中可重试 Ok → 循环自然耗尽 → 尾局 request_fn 补跑一次
/// （186-188 的 None 臂）。
#[tokio::test]
async fn retryable_ok_exhaustion_falls_to_final_rerun() {
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let calls2 = calls.clone();
    let result: Result<AlwaysRetryable, String> = do_request_with_retry(2, move || {
        let c = calls2.clone();
        async move {
            c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(AlwaysRetryable { status: 500 })
        }
    })
    .await;
    let resp = result.expect("终局补跑的 Ok 必须原样返回");
    assert_eq!(resp.status_code(), 500);
    // 2 次循环尝试 + 1 次终局补跑。
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 3);
}
