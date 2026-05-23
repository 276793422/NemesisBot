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
    #[allow(dead_code)]
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

#[cfg(test)]
mod tests;
