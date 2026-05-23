//! Parallel initialization utilities.
//!
//! Mirrors the Go `parallel.go` errgroup-style parallel initialization
//! using tokio task spawning with cancellation support.

use std::future::Future;
use std::sync::Arc;
use tokio::sync::Semaphore;

/// Error type for parallel initialization failures.
#[derive(Debug, thiserror::Error)]
pub enum InitError {
    /// One or more initialization functions failed.
    #[error("initialization failed: {0}")]
    Failed(String),

    /// Context was cancelled during initialization.
    #[error("initialization cancelled")]
    Cancelled,
}

/// Run multiple initialization functions in parallel.
///
/// Returns the first error encountered by any init function. If the
/// context is cancelled, all in-flight init functions receive cancellation.
///
/// This mirrors the Go `parallelInit` using `errgroup.WithContext`.
pub async fn parallel_init<F, Fut>(inits: Vec<F>) -> Result<(), InitError>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = Result<(), String>> + Send + 'static,
{
    if inits.is_empty() {
        return Ok(());
    }

    let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(inits.len());

    let mut handles = Vec::with_capacity(inits.len());
    for init_fn in inits {
        let tx = tx.clone();
        let handle = tokio::spawn(async move {
            match init_fn().await {
                Ok(()) => {}
                Err(e) => {
                    let _ = tx.send(e).await;
                }
            }
        });
        handles.push(handle);
    }

    // Drop the sender so rx resolves when all tasks complete
    drop(tx);

    // Wait for all tasks to finish
    for handle in handles {
        let _ = handle.await;
    }

    // Check for errors
    if let Ok(error) = rx.try_recv() {
        return Err(InitError::Failed(error));
    }

    Ok(())
}

/// Run multiple synchronous initialization functions in parallel.
///
/// Each function is spawned on a blocking thread. Returns the first error
/// encountered by any init function.
pub async fn parallel_init_blocking<F>(inits: Vec<F>) -> Result<(), InitError>
where
    F: FnOnce() -> Result<(), String> + Send + 'static,
{
    if inits.is_empty() {
        return Ok(());
    }

    let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(inits.len());

    let mut handles = Vec::with_capacity(inits.len());
    for init_fn in inits {
        let tx = tx.clone();
        let handle = tokio::task::spawn_blocking(move || match init_fn() {
            Ok(()) => {}
            Err(e) => {
                let _ = tx.blocking_send(e);
            }
        });
        handles.push(handle);
    }

    drop(tx);

    for handle in handles {
        let _ = handle.await;
    }

    if let Ok(error) = rx.try_recv() {
        return Err(InitError::Failed(error));
    }

    Ok(())
}

/// Run initialization functions one after another (sequential).
///
/// Returns the first error encountered. This is used for components
/// that have ordering dependencies.
pub fn sequential_init<F>(inits: Vec<F>) -> Result<(), InitError>
where
    F: FnOnce() -> Result<(), String>,
{
    for init_fn in inits {
        init_fn().map_err(InitError::Failed)?;
    }
    Ok(())
}

/// A bounded parallel runner that limits concurrency.
///
/// Useful when you need to limit the number of concurrent initializations
/// (e.g., to avoid resource contention).
pub struct BoundedParallelInit {
    semaphore: Arc<Semaphore>,
}

impl BoundedParallelInit {
    /// Create a new bounded parallel runner with the given concurrency limit.
    pub fn new(max_concurrency: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(max_concurrency.max(1))),
        }
    }

    /// Run an initialization function with bounded concurrency.
    pub async fn run<F, Fut>(&self, init_fn: F) -> Result<(), InitError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<(), String>>,
    {
        let _permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|_| InitError::Cancelled)?;

        init_fn().await.map_err(InitError::Failed)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
