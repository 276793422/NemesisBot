use super::*;

fn make_event(event_type: &str, session_key: &str) -> TraceEvent {
    TraceEvent {
        id: uuid::Uuid::new_v4().to_string(),
        event_type: event_type.into(),
        session_key: session_key.into(),
        timestamp: chrono::Local::now().to_rfc3339(),
        data: serde_json::json!({}),
    }
}

#[test]
fn test_record_and_get_events() {
    let collector = TraceCollector::new();
    collector.record_event(make_event("tool_call", "sess-1"));
    collector.record_event(make_event("llm_response", "sess-1"));

    assert_eq!(collector.len(), 2);
    let events = collector.events();
    assert_eq!(events.len(), 2);
}

#[test]
fn test_record_signals() {
    let collector = TraceCollector::new();
    collector.record_signal(SessionSignal {
        signal_type: "retry".into(),
        tool_name: "file_read".into(),
        timestamp: chrono::Local::now().to_rfc3339(),
        session_key: "sess-1".into(),
    });

    let signals = collector.signals();
    assert_eq!(signals.len(), 1);
    assert_eq!(signals[0].signal_type, "retry");
}

#[test]
fn test_compute_stats() {
    let collector = TraceCollector::new();
    collector.record_event(make_event("conversation_start", "sess-1"));
    collector.record_event(make_event("llm_response", "sess-1"));
    collector.record_event(make_event("tool_call", "sess-1"));
    collector.record_event(make_event("llm_response", "sess-1"));
    collector.record_signal(SessionSignal {
        signal_type: "retry".into(),
        tool_name: "tool_a".into(),
        timestamp: chrono::Local::now().to_rfc3339(),
        session_key: "sess-1".into(),
    });

    let stats = collector.compute_stats();
    assert_eq!(stats.total_traces, 4);
    assert!(stats.avg_rounds > 0.0);
    assert_eq!(stats.signal_summary.get("retry"), Some(&1));
}

#[test]
fn test_clear() {
    let collector = TraceCollector::new();
    collector.record_event(make_event("test", "s"));
    assert!(!collector.is_empty());
    collector.clear();
    assert!(collector.is_empty());
}
