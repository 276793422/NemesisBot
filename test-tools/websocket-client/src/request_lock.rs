// Request Lock - Global mutex to ensure only one request at a time
use std::sync::Arc;
use tokio::sync::Mutex;

/// Global request lock to ensure only one request is being processed at a time
#[derive(Debug, Clone)]
pub struct RequestLock {
    inner: Arc<Mutex<RequestState>>,
}

#[derive(Debug)]
struct RequestState {
    busy: bool,
    pending_input: Option<String>,
}

impl RequestLock {
    /// Create a new request lock
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(RequestState {
                busy: false,
                pending_input: None,
            })),
        }
    }

    /// Try to acquire the lock for sending a request
    /// Returns Ok if acquired, Err(message) if busy
    pub async fn try_acquire(&self, input: String) -> Result<(), String> {
        let mut state = self.inner.lock().await;

        if state.busy {
            // Store pending input (will be discarded, user needs to retry)
            state.pending_input = Some(input);
            Err("繁忙中，别那么着急".to_string())
        } else {
            state.busy = true;
            state.pending_input = None;
            Ok(())
        }
    }

    /// Release the lock after receiving response
    pub async fn release(&self) {
        let mut state = self.inner.lock().await;
        state.busy = false;
        state.pending_input = None;
    }

    /// Check if currently busy
    pub async fn is_busy(&self) -> bool {
        let state = self.inner.lock().await;
        state.busy
    }
}

impl Default for RequestLock {
    fn default() -> Self {
        Self::new()
    }
}

// Unit tests for request lock
#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{sleep, Duration};

    #[tokio::test]
    async fn test_request_lock_basic() {
        let lock = RequestLock::new();

        // First acquire should succeed
        assert!(lock.try_acquire("test1".to_string()).await.is_ok());

        // Second acquire should fail while first is held
        assert!(lock.try_acquire("test2".to_string()).await.is_err());

        // Release and try again
        lock.release().await;
        assert!(lock.try_acquire("test3".to_string()).await.is_ok());

        lock.release().await;
    }

    #[tokio::test]
    async fn test_request_lock_is_busy() {
        let lock = RequestLock::new();

        // Not busy initially
        assert!(!lock.is_busy().await);

        // Acquire and check
        lock.try_acquire("test".to_string()).await.unwrap();
        assert!(lock.is_busy().await);

        // Release and check
        lock.release().await;
        assert!(!lock.is_busy().await);
    }

    #[tokio::test]
    async fn test_request_lock_concurrent() {
        let lock = Arc::new(RequestLock::new());
        let lock1 = lock.clone();
        let lock2 = lock.clone();

        let task1 = tokio::spawn(async move {
            for i in 1..=3 {
                while lock1.try_acquire(format!("task1-{}", i).to_string()).await.is_err() {
                    sleep(Duration::from_millis(10)).await;
                }
                sleep(Duration::from_millis(50)).await;
                lock1.release().await;
            }
        });

        let task2 = tokio::spawn(async move {
            for i in 1..=3 {
                while lock2.try_acquire(format!("task2-{}", i).to_string()).await.is_err() {
                    sleep(Duration::from_millis(10)).await;
                }
                sleep(Duration::from_millis(30)).await;
                lock2.release().await;
            }
        });

        let _ = tokio::time::timeout(Duration::from_secs(5), async {
            let _ = task1.await;
            let _ = task2.await;
        });
    }
}
