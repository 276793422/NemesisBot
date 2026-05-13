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
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A test hook that counts events.
    struct CountingHook {
        count: AtomicUsize,
    }

    impl CountingHook {
        fn new() -> Self {
            Self {
                count: AtomicUsize::new(0),
            }
        }

        fn count(&self) -> usize {
            self.count.load(Ordering::SeqCst)
        }
    }

    impl LogHook for CountingHook {
        fn on_log(&self, _event: LogEvent) {
            self.count.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn test_log_event_clone() {
        let event = LogEvent {
            level: "info".into(),
            timestamp: "2026-05-04T12:00:00Z".into(),
            message: "test message".into(),
            target: Some("test::module".into()),
            fields: Some(serde_json::json!({"key": "value"})),
        };
        let cloned = event.clone();
        assert_eq!(cloned.level, "info");
        assert_eq!(cloned.message, "test message");
    }

    #[test]
    fn test_noop_log_hook() {
        let hook = NoopLogHook;
        hook.on_log(LogEvent {
            level: "info".into(),
            timestamp: String::new(),
            message: "test".into(),
            target: None,
            fields: None,
        });
        // Should not panic
    }

    #[test]
    fn test_log_hook_chain_dispatches_to_all() {
        let chain = LogHookChain::new();
        let counter1 = Arc::new(CountingHook::new());
        let counter2 = Arc::new(CountingHook::new());

        chain.register(counter1.clone());
        chain.register(counter2.clone());

        chain.dispatch(LogEvent {
            level: "info".into(),
            timestamp: String::new(),
            message: "test".into(),
            target: None,
            fields: None,
        });

        assert_eq!(counter1.count(), 1);
        assert_eq!(counter2.count(), 1);
    }

    #[test]
    fn test_log_hook_chain_multiple_events() {
        let chain = LogHookChain::new();
        let counter = Arc::new(CountingHook::new());
        chain.register(counter.clone());

        for i in 0..10 {
            chain.dispatch(LogEvent {
                level: "debug".into(),
                timestamp: String::new(),
                message: format!("event {}", i),
                target: None,
                fields: None,
            });
        }

        assert_eq!(counter.count(), 10);
    }

    #[test]
    fn test_log_hook_chain_clear() {
        let chain = LogHookChain::new();
        let counter = Arc::new(CountingHook::new());
        chain.register(counter.clone());

        chain.dispatch(LogEvent {
            level: "info".into(),
            timestamp: String::new(),
            message: "before clear".into(),
            target: None,
            fields: None,
        });
        assert_eq!(counter.count(), 1);

        chain.clear();
        assert!(chain.is_empty());

        chain.dispatch(LogEvent {
            level: "info".into(),
            timestamp: String::new(),
            message: "after clear".into(),
            target: None,
            fields: None,
        });
        assert_eq!(counter.count(), 1); // No new events received
    }

    #[test]
    fn test_log_hook_chain_len() {
        let chain = LogHookChain::new();
        assert!(chain.is_empty());
        assert_eq!(chain.len(), 0);

        chain.register(Arc::new(NoopLogHook));
        assert!(!chain.is_empty());
        assert_eq!(chain.len(), 1);

        chain.register(Arc::new(NoopLogHook));
        assert_eq!(chain.len(), 2);
    }

    #[test]
    fn test_log_hook_chain_as_log_hook() {
        let chain = Arc::new(LogHookChain::new());
        let counter = Arc::new(CountingHook::new());
        chain.register(counter.clone());

        // Use via trait object
        let hook: &dyn LogHook = chain.as_ref();
        hook.on_log(LogEvent {
            level: "info".into(),
            timestamp: String::new(),
            message: "via trait".into(),
            target: None,
            fields: None,
        });

        assert_eq!(counter.count(), 1);
    }

    #[test]
    fn test_log_hook_chain_default() {
        let chain = LogHookChain::default();
        assert!(chain.is_empty());
    }

    // ---- New tests ----

    #[test]
    fn test_log_event_debug() {
        let event = LogEvent {
            level: "error".into(),
            timestamp: "2026-01-01T00:00:00Z".into(),
            message: "something broke".into(),
            target: None,
            fields: None,
        };
        let debug = format!("{:?}", event);
        assert!(debug.contains("error"));
        assert!(debug.contains("something broke"));
    }

    #[test]
    fn test_log_event_with_fields() {
        let event = LogEvent {
            level: "info".into(),
            timestamp: "2026-01-01T00:00:00Z".into(),
            message: "user login".into(),
            target: Some("auth::service".into()),
            fields: Some(serde_json::json!({"user_id": 42, "ip": "10.0.0.1"})),
        };
        assert_eq!(event.target.as_deref(), Some("auth::service"));
        assert!(event.fields.is_some());
        let fields = event.fields.unwrap();
        assert_eq!(fields["user_id"], 42);
    }

    #[test]
    fn test_log_event_empty_fields() {
        let event = LogEvent {
            level: "trace".into(),
            timestamp: String::new(),
            message: String::new(),
            target: None,
            fields: None,
        };
        assert!(event.target.is_none());
        assert!(event.fields.is_none());
        assert!(event.message.is_empty());
    }

    #[test]
    fn test_dispatch_empty_chain() {
        let chain = LogHookChain::new();
        // Should not panic
        chain.dispatch(LogEvent {
            level: "info".into(),
            timestamp: String::new(),
            message: "no hooks".into(),
            target: None,
            fields: None,
        });
    }

    #[test]
    fn test_dispatch_many_hooks() {
        let chain = LogHookChain::new();
        let counters: Vec<Arc<CountingHook>> = (0..10)
            .map(|_| Arc::new(CountingHook::new()))
            .collect();

        for c in &counters {
            chain.register(c.clone());
        }

        chain.dispatch(LogEvent {
            level: "info".into(),
            timestamp: String::new(),
            message: "broadcast".into(),
            target: None,
            fields: None,
        });

        for c in &counters {
            assert_eq!(c.count(), 1);
        }
    }

    #[test]
    fn test_noop_log_hook_multiple_calls() {
        let hook = NoopLogHook;
        for _ in 0..100 {
            hook.on_log(LogEvent {
                level: "info".into(),
                timestamp: String::new(),
                message: "noop".into(),
                target: None,
                fields: None,
            });
        }
        // Should not panic or accumulate
    }

    #[test]
    fn test_register_after_clear() {
        let chain = LogHookChain::new();
        let counter1 = Arc::new(CountingHook::new());
        chain.register(counter1.clone());
        chain.clear();

        let counter2 = Arc::new(CountingHook::new());
        chain.register(counter2.clone());

        chain.dispatch(LogEvent {
            level: "info".into(),
            timestamp: String::new(),
            message: "after reregister".into(),
            target: None,
            fields: None,
        });

        assert_eq!(counter1.count(), 0);
        assert_eq!(counter2.count(), 1);
    }

    #[test]
    fn test_log_event_all_levels() {
        for level in &["trace", "debug", "info", "warn", "error"] {
            let event = LogEvent {
                level: level.to_string(),
                timestamp: "2026-01-01T00:00:00Z".into(),
                message: format!("{} message", level),
                target: None,
                fields: None,
            };
            assert_eq!(event.level, *level);
        }
    }
}
