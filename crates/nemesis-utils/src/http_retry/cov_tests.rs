// http_retry.rs 覆盖率补充测试（泛型版最后一轮可重试 Ok 的终态返回臂）。
// BUG-20261113-159 修复注记：此前最后一轮拿到可重试状态码时「re-run one
// final time」补跑一次上游（副作用双执行 + 返回值不验态直接 Ok 出门），
// 旧测试把 N+1 次调用钉成预期。修复后 = 恰好 max 次请求，最后一轮结果
// 原样返回（与 simple/reqwest 两版语义对齐）。

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

/// max 次全部命中可重试 Ok → 最后一轮结果原样返回，**无补跑**：调用数
/// 恰好 = max（N+1 补跑已废）。
#[tokio::test]
async fn retryable_ok_exhaustion_returns_last_result_as_is() {
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
    let resp = result.expect("最后一轮的 Ok 必须原样返回");
    assert_eq!(resp.status_code(), 500);
    // 恰好 max 次，无终局补跑（BUG-20261113-159 回归锁）。
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
}
