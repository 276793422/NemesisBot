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
mod tests;
