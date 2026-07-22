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
    let counters: Vec<Arc<CountingHook>> = (0..10).map(|_| Arc::new(CountingHook::new())).collect();

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
