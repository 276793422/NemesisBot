//! HTTP retry utilities.
//!
//! Provides resilient HTTP request execution with automatic retry on transient
//! failures (429 Too Many Requests, 5xx server errors) and exponential backoff.

use std::future::Future;
use std::time::Duration;

use tokio::time::sleep;

/// Maximum number of retry attempts.
const MAX_RETRIES: u32 = 3;

/// Base delay unit for exponential backoff.
const RETRY_DELAY_UNIT: Duration = Duration::from_secs(1);

/// Retry configuration.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    pub max_retries: u32,
    pub initial_delay: Duration,
    pub max_delay: Duration,
    pub multiplier: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
            multiplier: 2.0,
        }
    }
}

impl RetryConfig {
    /// Get delay for a given attempt number (0-indexed).
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        let delay_secs = self.initial_delay.as_secs_f64()
            * self.multiplier.powi(attempt as i32);
        let delay = Duration::from_secs_f64(delay_secs);
        delay.min(self.max_delay)
    }
}

/// Determine whether an HTTP status code warrants a retry.
///
/// Returns `true` for:
/// - 429 Too Many Requests
/// - Any 5xx server error
pub fn should_retry(status_code: u16) -> bool {
    status_code == 429 || status_code >= 500
}

/// Cancellable sleep that aborts when the provided future resolves.
///
/// Mirrors the Go `sleepWithCtx(ctx, duration)` function. Returns `Ok(())` if
/// the sleep completed, or `Err(cancelled_value)` if `cancel_rx` resolved first.
///
/// # Arguments
/// * `duration` - How long to sleep
/// * `cancel_rx` - A future that, when resolved, cancels the sleep
pub async fn sleep_with_cancel<F, T>(duration: Duration, cancel_rx: F) -> Result<(), T>
where
    F: std::future::Future<Output = T>,
{
    tokio::select! {
        _ = sleep(duration) => Ok(()),
        result = cancel_rx => Err(result),
    }
}

/// Execute an HTTP request with retry using a `reqwest::Client` and `reqwest::Request`.
///
/// This is the closest Rust analogue of the Go `DoRequestWithRetry(client, req)` function.
/// It retries up to 3 times with exponential backoff (1s, 2s, 3s) on 429/5xx responses
/// or connection errors. The request must be cloneable (i.e., have a non-streaming body).
///
/// # Arguments
/// * `client` - The reqwest client to use
/// * `req` - The request to execute (will be cloned on retry)
///
/// # Returns
/// `Some(Ok(response))` on success, `Some(Err(error))` on failure, or `None` if
/// the request body could not be cloned for retry.
pub async fn do_request_with_retry_reqwest(
    client: &reqwest::Client,
    req: &reqwest::Request,
) -> Option<Result<reqwest::Response, reqwest::Error>> {
    let mut last_resp: Option<Result<reqwest::Response, reqwest::Error>> = None;

    for attempt in 0..MAX_RETRIES {
        let req_clone = match req.try_clone() {
            Some(r) => r,
            None => {
                // Cannot clone (e.g., streaming body), return whatever we have
                return last_resp;
            }
        };

        match client.execute(req_clone).await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                if status == 200 || !should_retry(status) {
                    return Some(Ok(resp));
                }
                // Status is retryable; store and retry
                last_resp = Some(Ok(resp));
            }
            Err(e) => {
                if attempt + 1 >= MAX_RETRIES {
                    return Some(Err(e));
                }
                last_resp = Some(Err(e));
            }
        }

        if attempt + 1 < MAX_RETRIES {
            let delay = RETRY_DELAY_UNIT * (attempt + 1);
            sleep(delay).await;
        }
    }

    last_resp
}

/// Execute an HTTP request with automatic retry logic.
///
/// Retries on transient failures (429 Too Many Requests, 5xx server errors)
/// with linear backoff (1s, 2s, 3s). The request is cloned for each attempt,
/// so the caller must ensure the request is clonable.
///
/// This is an async wrapper that mirrors the Go `DoRequestWithRetry` function.
/// Rather than taking a raw `http::Request`, it accepts a closure that builds
/// and sends the request, returning a `Result<(u16, String)>` where the tuple
/// is `(status_code, response_body)`.
///
/// # Arguments
/// * `max_retries` - Maximum number of attempts (including the first)
/// * `request_fn` - Async function that executes the HTTP request
///
/// # Returns
/// The result from the last attempt.
pub async fn do_request_with_retry<F, Fut, T, E>(
    max_retries: u32,
    request_fn: F,
) -> Result<T, E>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    T: HasStatusCode,
    E: std::fmt::Debug,
{
    let mut last_err: Option<E> = None;
    let max = if max_retries == 0 { MAX_RETRIES } else { max_retries };

    for attempt in 0..max {
        match request_fn().await {
            Ok(result) => {
                let status = result.status_code();
                if status == 200 || !should_retry(status) {
                    return Ok(result);
                }
                // Retryable status; if this is the last attempt, return the result anyway
                if attempt + 1 >= max {
                    // Can't easily reconstruct T, so re-run one final time
                    // and return whatever we get
                    return request_fn().await;
                }
            }
            Err(e) => {
                if attempt + 1 >= max {
                    return Err(e);
                }
                last_err = Some(e);
            }
        }

        // Linear backoff: 1s, 2s, 3s, ...
        let delay = RETRY_DELAY_UNIT * (attempt + 1);
        sleep(delay).await;
    }

    // Unreachable in normal flow, but satisfy the type system
    match last_err {
        Some(e) => Err(e),
        None => request_fn().await,
    }
}

/// Execute an HTTP request with retry using a simpler status/body interface.
///
/// This mirrors the Go `DoRequestWithRetry` more directly. The closure should
/// return `Ok((status_code, body_string))` on success or `Err(message)` on failure.
///
/// # Arguments
/// * `request_fn` - Async function that performs the HTTP request
///
/// # Returns
/// `Ok((status_code, body))` from the last successful attempt.
pub async fn do_request_with_retry_simple<F, Fut>(
    request_fn: F,
) -> Result<(u16, String), String>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<(u16, String), String>>,
{
    let mut last_result: Option<Result<(u16, String), String>> = None;

    for attempt in 0..MAX_RETRIES {
        match request_fn().await {
            Ok((status, body)) => {
                if status == 200 || !should_retry(status) {
                    return Ok((status, body));
                }
                // Retryable status
                last_result = Some(Ok((status, body)));
            }
            Err(e) => {
                last_result = Some(Err(e));
            }
        }

        if attempt + 1 < MAX_RETRIES {
            let delay = RETRY_DELAY_UNIT * (attempt + 1);
            sleep(delay).await;
        }
    }

    last_result.unwrap_or_else(|| Err("no attempts made".to_string()))
}

/// Trait for types that carry an HTTP status code.
///
/// Implement this for your response types to use [`do_request_with_retry`].
pub trait HasStatusCode {
    /// Return the HTTP status code of the response.
    fn status_code(&self) -> u16;
}

/// Simple response type that implements [`HasStatusCode`].
#[derive(Debug, Clone)]
pub struct RetryableResponse {
    /// HTTP status code.
    pub status: u16,
    /// Response body text.
    pub body: String,
}

impl HasStatusCode for RetryableResponse {
    fn status_code(&self) -> u16 {
        self.status
    }
}

#[cfg(test)]
mod tests {
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
}
