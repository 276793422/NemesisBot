//! SSE Event Hub for server-sent events.

use std::sync::Arc;
use tokio::sync::broadcast;

/// Event type constants.
pub const EVENT_LOG: &str = "log";
pub const EVENT_STATUS: &str = "status";
pub const EVENT_SECURITY_ALERT: &str = "security-alert";
pub const EVENT_SCANNER_PROGRESS: &str = "scanner-progress";
pub const EVENT_CLUSTER_EVENT: &str = "cluster-event";
pub const EVENT_HEARTBEAT: &str = "heartbeat";
/// Chat streaming delta — published for each streamed LLM token chunk.
pub const EVENT_CHAT_STREAM: &str = "chat-stream";

/// A server-sent event.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Event {
    pub event_type: String,
    pub data: serde_json::Value,
}

/// Event hub that manages SSE subscribers and broadcasts events.
pub struct EventHub {
    sender: broadcast::Sender<Event>,
    subscriber_count: Arc<std::sync::atomic::AtomicUsize>,
}

impl EventHub {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(256);
        Self {
            sender,
            subscriber_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    /// Subscribe to events. Returns a receiver.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.subscriber_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.sender.subscribe()
    }

    /// Unsubscribe (decrement counter).
    pub fn unsubscribe(&self) {
        self.subscriber_count.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }

    /// Publish an event to all subscribers.
    pub fn publish(&self, event_type: &str, data: serde_json::Value) {
        let event = Event {
            event_type: event_type.to_string(),
            data,
        };
        // broadcast::send ignores errors when no receivers
        let _ = self.sender.send(event);
    }

    /// Get the number of active subscribers.
    pub fn subscriber_count(&self) -> usize {
        self.subscriber_count.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl Default for EventHub {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_publish_subscribe() {
        let hub = EventHub::new();
        let mut rx = hub.subscribe();
        assert_eq!(hub.subscriber_count(), 1);

        hub.publish("test", serde_json::json!({"key": "value"}));
        let event = rx.try_recv().unwrap();
        assert_eq!(event.event_type, "test");
    }

    #[test]
    fn test_subscriber_count() {
        let hub = EventHub::new();
        assert_eq!(hub.subscriber_count(), 0);
        let _rx1 = hub.subscribe();
        assert_eq!(hub.subscriber_count(), 1);
        let _rx2 = hub.subscribe();
        assert_eq!(hub.subscriber_count(), 2);
        hub.unsubscribe();
        assert_eq!(hub.subscriber_count(), 1);
    }

    #[test]
    fn test_default_creates_hub() {
        let hub = EventHub::default();
        assert_eq!(hub.subscriber_count(), 0);
    }

    #[test]
    fn test_multiple_events_in_order() {
        let hub = EventHub::new();
        let mut rx = hub.subscribe();

        hub.publish("log", serde_json::json!({"msg": "first"}));
        hub.publish("log", serde_json::json!({"msg": "second"}));
        hub.publish("status", serde_json::json!({"state": "running"}));

        let e1 = rx.try_recv().unwrap();
        assert_eq!(e1.event_type, "log");
        assert_eq!(e1.data["msg"], "first");

        let e2 = rx.try_recv().unwrap();
        assert_eq!(e2.event_type, "log");
        assert_eq!(e2.data["msg"], "second");

        let e3 = rx.try_recv().unwrap();
        assert_eq!(e3.event_type, "status");
        assert_eq!(e3.data["state"], "running");
    }

    #[test]
    fn test_no_subscriber_publish_no_panic() {
        let hub = EventHub::new();
        // Should not panic when publishing with no subscribers
        hub.publish("test", serde_json::json!({"key": "value"}));
    }

    #[test]
    fn test_event_serialization() {
        let hub = EventHub::new();
        let mut rx = hub.subscribe();

        hub.publish("test", serde_json::json!({"nested": {"key": 42}}));

        let event = rx.try_recv().unwrap();
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["event_type"], "test");
        assert_eq!(json["data"]["nested"]["key"], 42);
    }

    #[test]
    fn test_unsubscribe_without_subscribe_no_panic() {
        let hub = EventHub::new();
        // Calling unsubscribe without subscribe - should not panic
        // (AtomicUsize wraps on underflow, which is expected behavior)
        hub.unsubscribe();
        hub.unsubscribe();
        // Don't assert count == 0 since AtomicUsize wraps on underflow
    }

    #[test]
    fn test_event_type_constants() {
        assert_eq!(EVENT_LOG, "log");
        assert_eq!(EVENT_STATUS, "status");
        assert_eq!(EVENT_SECURITY_ALERT, "security-alert");
        assert_eq!(EVENT_SCANNER_PROGRESS, "scanner-progress");
        assert_eq!(EVENT_CLUSTER_EVENT, "cluster-event");
        assert_eq!(EVENT_HEARTBEAT, "heartbeat");
        assert_eq!(EVENT_CHAT_STREAM, "chat-stream");
    }

    #[test]
    fn test_subscribe_and_unsubscribe_flow() {
        let hub = EventHub::new();
        let rx1 = hub.subscribe();
        let mut rx1 = rx1;
        let _rx2 = hub.subscribe();

        hub.publish("test", serde_json::json!({"val": 1}));
        let e = rx1.try_recv().unwrap();
        assert_eq!(e.data["val"], 1);

        hub.unsubscribe();
        assert_eq!(hub.subscriber_count(), 1);
    }

    #[test]
    fn test_large_event_data() {
        let hub = EventHub::new();
        let mut rx = hub.subscribe();

        let large_data: Vec<i32> = (0..1000).collect();
        hub.publish("bulk", serde_json::json!({"data": large_data}));

        let event = rx.try_recv().unwrap();
        assert_eq!(event.data["data"].as_array().unwrap().len(), 1000);
    }

    #[test]
    fn test_event_debug_format() {
        let event = Event {
            event_type: "test".to_string(),
            data: serde_json::json!({"key": "value"}),
        };
        let debug_str = format!("{:?}", event);
        assert!(debug_str.contains("test"));
    }

    #[test]
    fn test_multiple_subscribers_receive_same_event() {
        let hub = EventHub::new();
        let mut rx1 = hub.subscribe();
        let mut rx2 = hub.subscribe();

        hub.publish("broadcast", serde_json::json!({"msg": "hello"}));

        let e1 = rx1.try_recv().unwrap();
        let e2 = rx2.try_recv().unwrap();
        assert_eq!(e1.event_type, e2.event_type);
        assert_eq!(e1.data, e2.data);
    }

    #[test]
    fn test_subscriber_count_after_multiple_unsubscribes() {
        let hub = EventHub::new();
        hub.subscribe();
        hub.subscribe();
        hub.subscribe();
        assert_eq!(hub.subscriber_count(), 3);
        hub.unsubscribe();
        hub.unsubscribe();
        assert_eq!(hub.subscriber_count(), 1);
        hub.unsubscribe();
        assert_eq!(hub.subscriber_count(), 0);
    }

    #[test]
    fn test_event_with_null_data() {
        let hub = EventHub::new();
        let mut rx = hub.subscribe();
        hub.publish("test", serde_json::Value::Null);
        let event = rx.try_recv().unwrap();
        assert_eq!(event.data, serde_json::Value::Null);
    }

    #[test]
    fn test_event_with_string_data() {
        let hub = EventHub::new();
        let mut rx = hub.subscribe();
        hub.publish("test", serde_json::json!("plain string"));
        let event = rx.try_recv().unwrap();
        assert_eq!(event.data, "plain string");
    }

    #[test]
    fn test_event_with_number_data() {
        let hub = EventHub::new();
        let mut rx = hub.subscribe();
        hub.publish("test", serde_json::json!(42));
        let event = rx.try_recv().unwrap();
        assert_eq!(event.data, 42);
    }

    #[test]
    fn test_event_with_boolean_data() {
        let hub = EventHub::new();
        let mut rx = hub.subscribe();
        hub.publish("test", serde_json::json!(true));
        let event = rx.try_recv().unwrap();
        assert_eq!(event.data, true);
    }

    #[test]
    fn test_publish_many_events() {
        let hub = EventHub::new();
        let mut rx = hub.subscribe();

        for i in 0..50 {
            hub.publish("seq", serde_json::json!({"i": i}));
        }

        for i in 0..50 {
            let event = rx.try_recv().unwrap();
            assert_eq!(event.data["i"], i);
        }
    }

    #[test]
    fn test_event_serialization_contains_event_type() {
        let event = Event {
            event_type: "custom-type".to_string(),
            data: serde_json::json!({"payload": 123}),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["event_type"], "custom-type");
        assert_eq!(json["data"]["payload"], 123);
    }

    #[test]
    fn test_broadcast_channel_capacity() {
        let hub = EventHub::new();
        let mut rx = hub.subscribe();

        for i in 0..10 {
            hub.publish("flood", serde_json::json!(i));
        }

        // Should be able to get events
        let mut count = 0;
        while let Ok(_) = rx.try_recv() {
            count += 1;
        }
        assert!(count > 0);
    }

    #[test]
    fn test_subscribe_and_drop_receiver() {
        let hub = EventHub::new();
        {
            let _rx = hub.subscribe();
            assert_eq!(hub.subscriber_count(), 1);
        }
        // Receiver dropped but subscriber count not decremented
        // (unsubscribe must be called explicitly)
        assert_eq!(hub.subscriber_count(), 1);
        hub.unsubscribe();
        assert_eq!(hub.subscriber_count(), 0);
    }

    #[test]
    fn test_event_with_nested_object() {
        let hub = EventHub::new();
        let mut rx = hub.subscribe();
        hub.publish("nested", serde_json::json!({
            "level1": {
                "level2": {
                    "value": 42
                }
            }
        }));
        let event = rx.try_recv().unwrap();
        assert_eq!(event.data["level1"]["level2"]["value"], 42);
    }

    // --- Additional events tests ---

    #[test]
    fn test_event_clone() {
        let event = Event {
            event_type: "test".into(),
            data: serde_json::json!({"key": "value"}),
        };
        let cloned = event.clone();
        assert_eq!(cloned.event_type, "test");
        assert_eq!(cloned.data["key"], "value");
    }

    #[test]
    fn test_event_default_hub() {
        let hub = EventHub::default();
        assert_eq!(hub.subscriber_count(), 0);
    }

    #[test]
    fn test_publish_after_unsubscribe() {
        let hub = EventHub::new();
        let rx = hub.subscribe();
        hub.unsubscribe();
        // Publishing after unsubscribe should not panic
        hub.publish("test", serde_json::json!({}));
        // The receiver should still be usable but may not get the event
        drop(rx);
    }

    #[test]
    fn test_multiple_publishes_ordering() {
        let hub = EventHub::new();
        let mut rx = hub.subscribe();
        for i in 0..5 {
            hub.publish("ordered", serde_json::json!({"index": i}));
        }
        for i in 0..5 {
            let event = rx.try_recv().unwrap();
            assert_eq!(event.data["index"], i);
        }
    }

    #[test]
    fn test_event_with_empty_object() {
        let hub = EventHub::new();
        let mut rx = hub.subscribe();
        hub.publish("empty", serde_json::json!({}));
        let event = rx.try_recv().unwrap();
        assert!(event.data.as_object().unwrap().is_empty());
    }

    #[test]
    fn test_event_with_array_data() {
        let hub = EventHub::new();
        let mut rx = hub.subscribe();
        hub.publish("array", serde_json::json!([1, 2, 3]));
        let event = rx.try_recv().unwrap();
        assert_eq!(event.data.as_array().unwrap().len(), 3);
    }

    #[test]
    fn test_subscriber_count_multiple() {
        let hub = EventHub::new();
        let _rx1 = hub.subscribe();
        let _rx2 = hub.subscribe();
        let _rx3 = hub.subscribe();
        assert_eq!(hub.subscriber_count(), 3);
    }

    #[test]
    fn test_event_serialization_roundtrip() {
        let event = Event {
            event_type: "test".into(),
            data: serde_json::json!({"msg": "hello"}),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("test"));
        assert!(json.contains("hello"));
    }

    #[test]
    fn test_event_without_data() {
        let event = Event {
            event_type: "nots".into(),
            data: serde_json::json!(null),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("nots"));
    }

    #[test]
    fn test_hub_publish_no_subscriber_no_panic() {
        let hub = EventHub::new();
        // Should not panic
        hub.publish("lonely", serde_json::json!({"alone": true}));
        assert_eq!(hub.subscriber_count(), 0);
    }

    #[test]
    fn test_event_debug_output() {
        let event = Event {
            event_type: "debug-test".into(),
            data: serde_json::json!({"key": "val"}),
        };
        let debug_str = format!("{:?}", event);
        assert!(debug_str.contains("debug-test"));
    }
}
