use super::*;

#[test]
fn test_delay_progression() {
    let config = RetryConfig::default();
    let d0 = config.delay_for_attempt(0);
    let d1 = config.delay_for_attempt(1);
    let d2 = config.delay_for_attempt(2);
    assert!(d1 > d0);
    assert!(d2 > d1);
}

#[test]
fn test_should_retry() {
    assert!(should_retry(429));
    assert!(should_retry(500));
    assert!(should_retry(502));
    assert!(should_retry(503));
    assert!(!should_retry(200));
    assert!(!should_retry(201));
    assert!(!should_retry(400));
    assert!(!should_retry(401));
    assert!(!should_retry(403));
    assert!(!should_retry(404));
}

#[tokio::test]
async fn test_retry_succeeds_first_try() {
    let call_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let count_clone = call_count.clone();

    let result = do_request_with_retry_simple(|| {
        let c = count_clone.clone();
        async move {
            c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok((200, "ok".to_string()))
        }
    })
    .await;

    assert!(result.is_ok());
    let (status, body) = result.unwrap();
    assert_eq!(status, 200);
    assert_eq!(body, "ok");
    assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_retry_succeeds_after_500() {
    let call_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let count_clone = call_count.clone();

    let result = do_request_with_retry_simple(|| {
        let c = count_clone.clone();
        async move {
            let n = c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n == 0 {
                Ok((500, "internal error".to_string()))
            } else {
                Ok((200, "ok".to_string()))
            }
        }
    })
    .await;

    assert!(result.is_ok());
    let (status, _) = result.unwrap();
    assert_eq!(status, 200);
    assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[tokio::test]
async fn test_retry_exhausted() {
    let call_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let count_clone = call_count.clone();

    let result = do_request_with_retry_simple(|| {
        let c = count_clone.clone();
        async move {
            c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok((503, "service unavailable".to_string()))
        }
    })
    .await;

    assert!(result.is_ok());
    let (status, _) = result.unwrap();
    assert_eq!(status, 503);
    assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 3);
}

#[tokio::test]
async fn test_retry_no_retry_on_404() {
    let call_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let count_clone = call_count.clone();

    let result = do_request_with_retry_simple(|| {
        let c = count_clone.clone();
        async move {
            c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok((404, "not found".to_string()))
        }
    })
    .await;

    assert!(result.is_ok());
    let (status, _) = result.unwrap();
    assert_eq!(status, 404);
    // 404 is not retryable, so only 1 call
    assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 1);
}

// ============================================================
// Additional tests for missing coverage
// ============================================================

#[test]
fn test_retry_config_default() {
    let config = RetryConfig::default();
    assert_eq!(config.max_retries, 3);
    assert_eq!(config.initial_delay, Duration::from_secs(1));
    assert_eq!(config.max_delay, Duration::from_secs(30));
    assert!((config.multiplier - 2.0).abs() < 0.001);
}

#[test]
fn test_delay_for_attempt_progression() {
    let config = RetryConfig::default();
    let d0 = config.delay_for_attempt(0);
    let d1 = config.delay_for_attempt(1);
    let d2 = config.delay_for_attempt(2);

    assert_eq!(d0, Duration::from_secs(1));
    assert_eq!(d1, Duration::from_secs(2));
    assert_eq!(d2, Duration::from_secs(4));
}

#[test]
fn test_delay_capped_at_max() {
    let config = RetryConfig::default();
    let d10 = config.delay_for_attempt(10);
    assert!(d10 <= config.max_delay);
}

#[test]
fn test_should_retry_all_codes() {
    // Retryable
    assert!(should_retry(429));
    assert!(should_retry(500));
    assert!(should_retry(502));
    assert!(should_retry(503));
    assert!(should_retry(504));
    assert!(should_retry(599));

    // Not retryable
    assert!(!should_retry(200));
    assert!(!should_retry(201));
    assert!(!should_retry(204));
    assert!(!should_retry(301));
    assert!(!should_retry(400));
    assert!(!should_retry(401));
    assert!(!should_retry(403));
    assert!(!should_retry(404));
    assert!(!should_retry(408));
}

#[test]
fn test_retryable_response_status_code() {
    let resp = RetryableResponse {
        status: 200,
        body: "ok".to_string(),
    };
    assert_eq!(resp.status_code(), 200);

    let resp = RetryableResponse {
        status: 503,
        body: "unavailable".to_string(),
    };
    assert_eq!(resp.status_code(), 503);
}

#[tokio::test]
async fn test_retry_succeeds_after_429() {
    let call_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let count_clone = call_count.clone();

    let result = do_request_with_retry_simple(|| {
        let c = count_clone.clone();
        async move {
            let n = c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n == 0 {
                Ok((429, "slow down".to_string()))
            } else {
                Ok((200, "ok".to_string()))
            }
        }
    })
    .await;

    assert!(result.is_ok());
    let (status, _) = result.unwrap();
    assert_eq!(status, 200);
    assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[tokio::test]
async fn test_retry_with_error_then_success() {
    let call_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let count_clone = call_count.clone();

    let result = do_request_with_retry_simple(|| {
        let c = count_clone.clone();
        async move {
            let n = c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n == 0 {
                Err("connection refused".to_string())
            } else {
                Ok((200, "ok".to_string()))
            }
        }
    })
    .await;

    assert!(result.is_ok());
    let (status, _) = result.unwrap();
    assert_eq!(status, 200);
    assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[tokio::test]
async fn test_retry_all_errors() {
    let call_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let count_clone = call_count.clone();

    let result = do_request_with_retry_simple(|| {
        let c = count_clone.clone();
        async move {
            c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err("persistent error".to_string())
        }
    })
    .await;

    assert!(result.is_err());
    assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 3);
}

#[tokio::test]
async fn test_do_request_with_retry_generic_ok() {
    let call_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let count_clone = call_count.clone();

    let result: Result<RetryableResponse, String> = do_request_with_retry(3, || {
        let c = count_clone.clone();
        async move {
            c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(RetryableResponse {
                status: 200,
                body: "ok".to_string(),
            })
        }
    })
    .await;

    assert!(result.is_ok());
    assert_eq!(result.unwrap().status, 200);
    assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
fn test_custom_retry_config() {
    let config = RetryConfig {
        max_retries: 5,
        initial_delay: Duration::from_millis(500),
        max_delay: Duration::from_secs(10),
        multiplier: 3.0,
    };
    let d0 = config.delay_for_attempt(0);
    let d1 = config.delay_for_attempt(1);
    let d2 = config.delay_for_attempt(2);

    assert_eq!(d0, Duration::from_millis(500));
    assert_eq!(d1, Duration::from_millis(1500));
    assert_eq!(d2, Duration::from_millis(4500));
}

#[tokio::test]
async fn test_sleep_with_cancel_sleep_completes() {
    // Sleep completes before cancel fires
    let cancel_future = std::future::pending::<()>();
    let result = sleep_with_cancel(Duration::from_millis(1), cancel_future).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_sleep_with_cancel_cancel_fires() {
    // Cancel fires before sleep completes
    let cancel_future = async { "cancelled" };
    let result = sleep_with_cancel(Duration::from_secs(10), cancel_future).await;
    assert!(result.is_err());
    assert_eq!(result.unwrap_err(), "cancelled");
}

#[tokio::test]
async fn test_do_request_with_retry_retryable_status() {
    let call_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let count_clone = call_count.clone();

    let result: Result<RetryableResponse, String> = do_request_with_retry(3, || {
        let c = count_clone.clone();
        async move {
            let n = c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n < 2 {
                Ok(RetryableResponse {
                    status: 500,
                    body: "error".to_string(),
                })
            } else {
                Ok(RetryableResponse {
                    status: 200,
                    body: "ok".to_string(),
                })
            }
        }
    })
    .await;

    assert!(result.is_ok());
    assert_eq!(result.unwrap().status, 200);
    assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 3);
}

#[tokio::test]
async fn test_do_request_with_retry_max_1() {
    let call_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let count_clone = call_count.clone();

    let result: Result<RetryableResponse, String> = do_request_with_retry(1, || {
        let c = count_clone.clone();
        async move {
            c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(RetryableResponse {
                status: 500,
                body: "error".to_string(),
            })
        }
    })
    .await;

    // With max_retries=1, it should only call once, then re-run once more
    assert!(result.is_ok());
    assert_eq!(result.unwrap().status, 500);
}

#[tokio::test]
async fn test_do_request_with_retry_all_errors() {
    let call_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let count_clone = call_count.clone();

    let result: Result<RetryableResponse, String> = do_request_with_retry(3, || {
        let c = count_clone.clone();
        async move {
            c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err("persistent error".to_string())
        }
    })
    .await;

    assert!(result.is_err());
    assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 3);
}

#[tokio::test]
async fn test_do_request_with_retry_zero_max_uses_default() {
    let call_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let count_clone = call_count.clone();

    let result: Result<RetryableResponse, String> = do_request_with_retry(0, || {
        let c = count_clone.clone();
        async move {
            c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(RetryableResponse {
                status: 200,
                body: "ok".to_string(),
            })
        }
    })
    .await;

    assert!(result.is_ok());
    assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_retry_succeeds_after_error() {
    let call_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let count_clone = call_count.clone();

    let result = do_request_with_retry_simple(|| {
        let c = count_clone.clone();
        async move {
            let n = c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n == 0 {
                Err("connection refused".to_string())
            } else {
                Ok((200, "ok".to_string()))
            }
        }
    })
    .await;

    assert!(result.is_ok());
    let (status, _) = result.unwrap();
    assert_eq!(status, 200);
    assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[test]
fn test_retry_config_debug() {
    let config = RetryConfig::default();
    let debug = format!("{:?}", config);
    assert!(debug.contains("max_retries: 3"));
}

#[test]
fn test_retryable_response_debug() {
    let resp = RetryableResponse {
        status: 200,
        body: "test".to_string(),
    };
    let debug = format!("{:?}", resp);
    assert!(debug.contains("200"));
}

#[test]
fn test_delay_for_attempt_exact_values_default() {
    let config = RetryConfig::default();
    // initial_delay=1s, multiplier=2.0
    // attempt 0: 1 * 2^0 = 1
    assert_eq!(config.delay_for_attempt(0), Duration::from_secs(1));
    // attempt 1: 1 * 2^1 = 2
    assert_eq!(config.delay_for_attempt(1), Duration::from_secs(2));
    // attempt 2: 1 * 2^2 = 4
    assert_eq!(config.delay_for_attempt(2), Duration::from_secs(4));
    // attempt 3: 1 * 2^3 = 8
    assert_eq!(config.delay_for_attempt(3), Duration::from_secs(8));
    // attempt 4: 1 * 2^4 = 16
    assert_eq!(config.delay_for_attempt(4), Duration::from_secs(16));
    // attempt 5: 1 * 2^5 = 32, but capped at max_delay=30
    assert_eq!(config.delay_for_attempt(5), Duration::from_secs(30));
}

#[test]
fn test_retry_config_clone() {
    let config = RetryConfig::default();
    let cloned = config.clone();
    assert_eq!(config.max_retries, cloned.max_retries);
    assert_eq!(config.initial_delay, cloned.initial_delay);
    assert_eq!(config.max_delay, cloned.max_delay);
    assert!((config.multiplier - cloned.multiplier).abs() < f64::EPSILON);
}

#[test]
fn test_retry_config_small_multiplier() {
    let config = RetryConfig {
        max_retries: 5,
        initial_delay: Duration::from_secs(2),
        max_delay: Duration::from_secs(60),
        multiplier: 1.5,
    };
    // attempt 0: 2 * 1.5^0 = 2.0
    assert_eq!(config.delay_for_attempt(0), Duration::from_secs_f64(2.0));
    // attempt 1: 2 * 1.5^1 = 3.0
    assert_eq!(config.delay_for_attempt(1), Duration::from_secs_f64(3.0));
    // attempt 2: 2 * 1.5^2 = 4.5
    assert_eq!(config.delay_for_attempt(2), Duration::from_secs_f64(4.5));
}

#[tokio::test]
async fn test_do_request_with_retry_generic_500_then_200() {
    let call_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let count_clone = call_count.clone();

    let result: Result<RetryableResponse, String> = do_request_with_retry(3, || {
        let c = count_clone.clone();
        async move {
            let n = c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n == 0 {
                Ok(RetryableResponse { status: 503, body: "unavailable".to_string() })
            } else {
                Ok(RetryableResponse { status: 200, body: "ok".to_string() })
            }
        }
    }).await;

    assert!(result.is_ok());
    assert_eq!(result.unwrap().status, 200);
    assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[tokio::test]
async fn test_do_request_with_retry_error_then_success() {
    let call_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let count_clone = call_count.clone();

    let result: Result<RetryableResponse, String> = do_request_with_retry(3, || {
        let c = count_clone.clone();
        async move {
            let n = c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n == 0 {
                Err("timeout".to_string())
            } else {
                Ok(RetryableResponse { status: 200, body: "ok".to_string() })
            }
        }
    }).await;

    assert!(result.is_ok());
    assert_eq!(result.unwrap().status, 200);
    assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[tokio::test]
async fn test_do_request_with_retry_429_then_200() {
    let call_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let count_clone = call_count.clone();

    let result: Result<RetryableResponse, String> = do_request_with_retry(3, || {
        let c = count_clone.clone();
        async move {
            let n = c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n == 0 {
                Ok(RetryableResponse { status: 429, body: "slow down".to_string() })
            } else {
                Ok(RetryableResponse { status: 200, body: "ok".to_string() })
            }
        }
    }).await;

    assert!(result.is_ok());
    assert_eq!(result.unwrap().status, 200);
    assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[tokio::test]
async fn test_do_request_with_retry_non_retryable_status_no_retry() {
    let call_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let count_clone = call_count.clone();

    let result: Result<RetryableResponse, String> = do_request_with_retry(3, || {
        let c = count_clone.clone();
        async move {
            c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(RetryableResponse { status: 404, body: "not found".to_string() })
        }
    }).await;

    assert!(result.is_ok());
    assert_eq!(result.unwrap().status, 404);
    // 404 is not retryable -> only 1 call
    assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_do_request_with_retry_simple_non_retryable_403() {
    let call_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let count_clone = call_count.clone();

    let result = do_request_with_retry_simple(|| {
        let c = count_clone.clone();
        async move {
            c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok((403, "forbidden".to_string()))
        }
    }).await;

    assert!(result.is_ok());
    assert_eq!(result.unwrap().0, 403);
    assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_do_request_with_retry_simple_success_first_try() {
    let result = do_request_with_retry_simple(|| {
        async { Ok((200, "ok".to_string())) }
    }).await;
    assert_eq!(result.unwrap(), (200, "ok".to_string()));
}

#[tokio::test]
async fn test_sleep_with_cancel_both_resolutions() {
    // Test with a non-Unit cancel type
    let cancel_future = async { 42i32 };
    let result = sleep_with_cancel(Duration::from_secs(10), cancel_future).await;
    assert_eq!(result.unwrap_err(), 42);
}

#[test]
fn test_should_retry_boundary_codes() {
    // Exactly 499 should not retry
    assert!(!should_retry(499));
    // Exactly 500 should retry
    assert!(should_retry(500));
    // 599 should retry
    assert!(should_retry(599));
    // 400 should not retry
    assert!(!should_retry(400));
    // 429 should retry
    assert!(should_retry(429));
}

#[test]
fn test_has_status_code_trait() {
    let resp = RetryableResponse { status: 201, body: "created".to_string() };
    assert_eq!(resp.status_code(), 201);
}

#[test]
fn test_retryable_response_clone() {
    let resp = RetryableResponse { status: 200, body: "ok".to_string() };
    let cloned = resp.clone();
    assert_eq!(cloned.status, 200);
    assert_eq!(cloned.body, "ok");
}

// --- Additional coverage tests ---

#[test]
fn test_should_retry_all_5xx_codes() {
    assert!(should_retry(500));
    assert!(should_retry(501));
    assert!(should_retry(502));
    assert!(should_retry(503));
    assert!(should_retry(504));
    assert!(should_retry(505));
    assert!(should_retry(599));
}

#[test]
fn test_should_not_retry_2xx_3xx_4xx() {
    assert!(!should_retry(200));
    assert!(!should_retry(201));
    assert!(!should_retry(204));
    assert!(!should_retry(301));
    assert!(!should_retry(302));
    assert!(!should_retry(304));
    assert!(!should_retry(400));
    assert!(!should_retry(401));
    assert!(!should_retry(403));
    assert!(!should_retry(404));
    assert!(!should_retry(408));
}

#[test]
fn test_delay_for_attempt_zero_multiplier() {
    let config = RetryConfig {
        max_retries: 3,
        initial_delay: Duration::from_secs(1),
        max_delay: Duration::from_secs(30),
        multiplier: 0.0,
    };
    // 0.0 multiplier: initial * 0^attempt = initial * 1 = 1s for attempt 0
    let d = config.delay_for_attempt(0);
    assert_eq!(d, Duration::from_secs(1));
}

#[test]
fn test_delay_for_attempt_high_capped() {
    let config = RetryConfig::default();
    let d = config.delay_for_attempt(10);
    // Should be capped at max_delay
    assert!(d <= config.max_delay);
}

#[tokio::test]
async fn test_do_request_with_retry_simple_all_500s() {
    let call_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let count_clone = call_count.clone();

    let result = do_request_with_retry_simple(|| {
        let c = count_clone.clone();
        async move {
            c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok((500, "server error".to_string()))
        }
    })
    .await;

    // After exhausting retries, should return last result
    assert!(result.is_ok());
    assert_eq!(result.unwrap().0, 500);
    assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 3);
}

#[tokio::test]
async fn test_do_request_with_retry_error_then_500_then_success() {
    let call_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let count_clone = call_count.clone();

    let result: Result<RetryableResponse, String> = do_request_with_retry(3, || {
        let c = count_clone.clone();
        async move {
            let n = c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            match n {
                0 => Err("connection failed".to_string()),
                1 => Ok(RetryableResponse { status: 503, body: "unavailable".to_string() }),
                _ => Ok(RetryableResponse { status: 200, body: "ok".to_string() }),
            }
        }
    })
    .await;

    assert!(result.is_ok());
    assert_eq!(result.unwrap().status, 200);
}

#[test]
fn test_retryable_response_debug_output() {
    let resp = RetryableResponse { status: 200, body: "ok".to_string() };
    let debug = format!("{:?}", resp);
    assert!(debug.contains("200"));
    assert!(debug.contains("ok"));
}

#[tokio::test]
async fn test_sleep_with_cancel_immediate_cancel() {
    // Cancel future resolves immediately
    let result = sleep_with_cancel(Duration::from_secs(60), async { "done" }).await;
    assert_eq!(result.unwrap_err(), "done");
}

#[tokio::test]
async fn test_sleep_with_cancel_sleep_completes_fast() {
    // Very short sleep that should complete before any cancel
    let cancel_future = std::future::pending::<()>();
    let result = sleep_with_cancel(Duration::from_millis(1), cancel_future).await;
    assert!(result.is_ok());
}
