use super::*;

#[tokio::test]
async fn publish_delivers_to_all_subscribers() {
    let d = EventDispatcher::new(8);
    let mut a = d.subscribe();
    let mut b = d.subscribe();

    let mut data = HashMap::new();
    data.insert("status".to_string(), serde_json::json!("ok"));
    d.publish(TriggerEvent::new("workflow.completed", data));

    let ea = a.recv().await.expect("subscriber a missed event");
    let eb = b.recv().await.expect("subscriber b missed event");
    assert_eq!(ea.event_type, "workflow.completed");
    assert_eq!(eb.event_type, "workflow.completed");
    assert_eq!(ea.data.get("status").and_then(|v| v.as_str()), Some("ok"));
}

#[tokio::test]
async fn publish_with_no_subscribers_is_silently_dropped() {
    // No subscribers — publish should not panic.
    let d = EventDispatcher::new(4);
    d.publish(TriggerEvent::new("noop", HashMap::new()));
    // Just reaching here is success.
}

#[tokio::test]
async fn lagged_subscriber_recovers_on_next_publish() {
    // capacity=1 + force lag by publishing 2 events before the receiver
    // catches up.
    let d = EventDispatcher::new(1);
    let mut rx = d.subscribe();
    d.publish(TriggerEvent::new("e1", HashMap::new()));
    d.publish(TriggerEvent::new("e2", HashMap::new()));

    // First recv likely returns Lagged(1); subsequent should return e2.
    let mut saw_e2 = false;
    for _ in 0..3 {
        match rx.try_recv() {
            Ok(ev) => {
                if ev.event_type == "e2" {
                    saw_e2 = true;
                    break;
                }
            }
            Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
            Err(_) => break,
        }
    }
    assert!(saw_e2, "subscriber should recover from lag and see e2");
}

#[test]
fn with_source_execution_id_sets_field() {
    let ev = TriggerEvent::new("workflow.completed", HashMap::new())
        .with_source_execution_id("exec_123");
    assert_eq!(ev.source_execution_id.as_deref(), Some("exec_123"));
}

#[test]
fn subscriber_count_reflects_active_subscriptions() {
    let d = EventDispatcher::new(4);
    assert_eq!(d.subscriber_count(), 0);
    let _r1 = d.subscribe();
    assert_eq!(d.subscriber_count(), 1);
    let _r2 = d.subscribe();
    assert_eq!(d.subscriber_count(), 2);
    drop(_r1);
    // broadcast only reaps when the next send/subscribe happens.
    let _ = d.tx.send(TriggerEvent::new("noop", HashMap::new()));
    assert_eq!(d.subscriber_count(), 1);
}
