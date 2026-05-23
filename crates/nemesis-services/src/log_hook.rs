//! LogHook trait and registration for bridging logger output to external consumers.
//!
//! Provides a `LogHook` trait that can be registered with `BotService`. The
//! gateway layer can then bridge the log hook to the web server's SSE event
//! stream, matching Go's `logger.SetLogHook` pattern.

use std::sync::Arc;

/// A log event captured by the log hook.
#[derive(Debug, Clone)]
pub struct LogEvent {
    /// Log level: "trace", "debug", "info", "warn", "error"
    pub level: String,
    /// Timestamp in RFC 3339 format.
    pub timestamp: String,
    /// Log message.
    pub message: String,
    /// Optional target/module path.
    pub target: Option<String>,
    /// Optional key-value fields as JSON.
    pub fields: Option<serde_json::Value>,
}

/// Trait for receiving log events.
///
/// Implementations can bridge log events to external systems such as
/// WebSocket SSE event streams, file logging, or monitoring dashboards.
///
/// # Example
///
/// ```rust,ignore
/// struct SseLogHook {
///     event_tx: tokio::sync::broadcast::Sender<LogEvent>,
/// }
///
/// impl LogHook for SseLogHook {
///     fn on_log(&self, event: LogEvent) {
///         let _ = self.event_tx.send(event);
///     }
/// }
/// ```
pub trait LogHook: Send + Sync {
    /// Called when a log event is emitted.
    ///
    /// Implementations should be fast and non-blocking. Heavy processing
    /// should be offloaded to a channel or background task.
    fn on_log(&self, event: LogEvent);
}

/// Type-erased log hook behind Arc.
pub type LogHookHandle = Arc<dyn LogHook>;

/// A composite log hook that fans out to multiple registered hooks.
///
/// Thread-safe: hooks can be registered and unregistered at any time.
pub struct LogHookChain {
    hooks: parking_lot::RwLock<Vec<LogHookHandle>>,
}

impl LogHookChain {
    /// Create an empty log hook chain.
    pub fn new() -> Self {
        Self {
            hooks: parking_lot::RwLock::new(Vec::new()),
        }
    }

    /// Register a new log hook.
    pub fn register(&self, hook: LogHookHandle) {
        self.hooks.write().push(hook);
    }

    /// Remove all hooks.
    pub fn clear(&self) {
        self.hooks.write().clear();
    }

    /// Return the number of registered hooks.
    pub fn len(&self) -> usize {
        self.hooks.read().len()
    }

    /// Return whether any hooks are registered.
    pub fn is_empty(&self) -> bool {
        self.hooks.read().is_empty()
    }

    /// Dispatch a log event to all registered hooks.
    pub fn dispatch(&self, event: LogEvent) {
        let hooks = self.hooks.read();
        for hook in hooks.iter() {
            hook.on_log(event.clone());
        }
    }
}

impl Default for LogHookChain {
    fn default() -> Self {
        Self::new()
    }
}

impl LogHook for LogHookChain {
    fn on_log(&self, event: LogEvent) {
        self.dispatch(event);
    }
}

/// A no-op log hook that discards all events.
pub struct NoopLogHook;

impl LogHook for NoopLogHook {
    fn on_log(&self, _event: LogEvent) {
        // Discard
    }
}

#[cfg(test)]
mod tests;
