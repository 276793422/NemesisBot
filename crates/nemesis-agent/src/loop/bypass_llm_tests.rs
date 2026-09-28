//! bypass_llm.rs 覆盖率补充（aux 杂务三层治理 2026-09-28）：重试原语
//! `with_one_retry` + 重试版护栏 `guarded_llm_call_retrying`（空输出触发
//! 重试 / 两次全败诚实上抛 / 超时包住全部尝试不翻倍）+ `aux_chat_options`
//! 显式禁思考标记（`Some("off")`，主循环 effort 值空间不受影响）。

use super::*;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[tokio::test]
async fn retry_succeeds_on_second_attempt() {
    let calls = Arc::new(AtomicUsize::new(0));
    let c2 = calls.clone();
    let out = with_one_retry("t", move || {
        let c = c2.clone();
        async move {
            if c.fetch_add(1, Ordering::SeqCst) == 0 {
                Err("first fails".to_string())
            } else {
                Ok("second wins".to_string())
            }
        }
    })
    .await;
    assert_eq!(out.unwrap(), "second wins");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn retry_first_ok_never_retries() {
    let calls = Arc::new(AtomicUsize::new(0));
    let c2 = calls.clone();
    let out = with_one_retry("t", move || {
        let c = c2.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
            Ok::<_, String>("first ok".to_string())
        }
    })
    .await;
    assert_eq!(out.unwrap(), "first ok");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn retry_both_fail_returns_second_err() {
    let calls = Arc::new(AtomicUsize::new(0));
    let c2 = calls.clone();
    let out: Result<String, String> = with_one_retry("t", move || {
        let c = c2.clone();
        async move {
            let n = c.fetch_add(1, Ordering::SeqCst);
            Err(format!("fail #{n}"))
        }
    })
    .await;
    assert_eq!(out.unwrap_err(), "fail #1");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn retrying_guard_retries_on_empty_output() {
    // 空白输出 = 护栏判失败 → 触发重试 → 第二次真内容生效。
    let calls = Arc::new(AtomicUsize::new(0));
    let c2 = calls.clone();
    let out = guarded_llm_call_retrying("g", std::time::Duration::from_secs(5), move || {
        let c = c2.clone();
        async move {
            if c.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok("   \n".to_string())
            } else {
                Ok("real content".to_string())
            }
        }
    })
    .await;
    assert_eq!(out.unwrap(), "real content");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn retrying_guard_honest_err_after_both_empty() {
    let calls = Arc::new(AtomicUsize::new(0));
    let c2 = calls.clone();
    let out = guarded_llm_call_retrying("g2", std::time::Duration::from_secs(5), move || {
        let c = c2.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
            Ok::<_, String>(String::new())
        }
    })
    .await;
    let err = out.unwrap_err();
    assert!(err.contains("空输出"), "got: {err}");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn retrying_guard_timeout_bounds_total_wallclock() {
    // 超时包在重试外层：单次尝试即超预算 → 整体超时，不放大墙钟。
    let out = guarded_llm_call_retrying("slow", std::time::Duration::from_millis(80), || async {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        Ok("late".to_string())
    })
    .await;
    let err = out.unwrap_err();
    assert!(err.contains("超时"), "got: {err}");
}

#[test]
fn aux_options_mark_thinking_disabled() {
    let o = aux_chat_options(AUX_TITLE_MAX_TOKENS);
    assert_eq!(o.max_tokens, Some(AUX_TITLE_MAX_TOKENS));
    assert_eq!(o.reasoning_effort.as_deref(), Some("off"));
}
