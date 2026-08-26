use super::*;
use crate::types::TriggerSource;
use async_trait::async_trait;
use std::sync::Mutex;
use std::time::Duration;

/// Observer that records every event it receives into a Vec for later
/// inspection. Uses Mutex (not async-aware) because tests need synchronous
/// access to the recorded events after awaiting emit().
struct RecordingObserver {
    name: String,
    events: Mutex<Vec<WorkflowEvent>>,
}

impl RecordingObserver {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            events: Mutex::new(Vec::new()),
        }
    }

    fn snapshot(&self) -> Vec<WorkflowEvent> {
        self.events.lock().unwrap().clone()
    }
}

#[async_trait]
impl WorkflowObserver for RecordingObserver {
    fn name(&self) -> &str {
        &self.name
    }

    async fn on_event(&self, event: WorkflowEvent) {
        let mut events = self.events.lock().unwrap();
        events.push(event);
    }
}

#[tokio::test]
async fn test_register_and_emit() {
    let manager = WorkflowEventManager::new();
    let observer = Arc::new(RecordingObserver::new("recorder"));
    manager
        .register(Arc::clone(&observer) as Arc<dyn WorkflowObserver>)
        .await;

    assert!(manager.has_observers().await);

    let event = WorkflowEvent::Started {
        execution_id: "exec-1".to_string(),
        workflow_name: "wf".to_string(),
        trigger_source: None,
        timestamp: chrono::Local::now(),
    };
    manager.emit(event).await;

    // emit() spawns a tokio task per observer; yield to let it land.
    tokio::time::sleep(Duration::from_millis(20)).await;

    let snapshot = observer.snapshot();
    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0].execution_id(), "exec-1");
}

#[tokio::test]
async fn test_unregister_stops_delivery() {
    let manager = WorkflowEventManager::new();
    let observer = Arc::new(RecordingObserver::new("recorder"));
    manager
        .register(Arc::clone(&observer) as Arc<dyn WorkflowObserver>)
        .await;

    manager.unregister("recorder").await;
    assert!(!manager.has_observers().await);

    let event = WorkflowEvent::Completed {
        execution_id: "exec-2".to_string(),
        workflow_name: "wf".to_string(),
        timestamp: chrono::Local::now(),
    };
    manager.emit(event).await;
    tokio::time::sleep(Duration::from_millis(20)).await;

    assert_eq!(observer.snapshot().len(), 0);
}

#[tokio::test]
async fn test_unregister_all_clears_everything() {
    let manager = WorkflowEventManager::new();
    manager
        .register(Arc::new(RecordingObserver::new("a")) as Arc<dyn WorkflowObserver>)
        .await;
    manager
        .register(Arc::new(RecordingObserver::new("b")) as Arc<dyn WorkflowObserver>)
        .await;
    assert_eq!(manager.has_observers().await, true);

    manager.unregister_all().await;
    assert_eq!(manager.has_observers().await, false);
}

#[tokio::test]
async fn test_multiple_observers_each_receive_event() {
    let manager = WorkflowEventManager::new();
    let o1 = Arc::new(RecordingObserver::new("o1"));
    let o2 = Arc::new(RecordingObserver::new("o2"));
    manager
        .register(Arc::clone(&o1) as Arc<dyn WorkflowObserver>)
        .await;
    manager
        .register(Arc::clone(&o2) as Arc<dyn WorkflowObserver>)
        .await;

    let event = WorkflowEvent::Failed {
        execution_id: "exec-3".to_string(),
        workflow_name: "wf".to_string(),
        error: "boom".to_string(),
        timestamp: chrono::Local::now(),
    };
    manager.emit(event).await;
    tokio::time::sleep(Duration::from_millis(20)).await;

    assert_eq!(o1.snapshot().len(), 1);
    assert_eq!(o2.snapshot().len(), 1);
}

#[tokio::test]
async fn test_event_serialization_roundtrip() {
    // Events are serializable so they can be shipped over WS/SSE in 1c.
    let event = WorkflowEvent::Started {
        execution_id: "exec-4".to_string(),
        workflow_name: "wf".to_string(),
        trigger_source: Some(TriggerSource::Cli),
        timestamp: chrono::Local::now(),
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("\"kind\":\"started\""));
    assert!(json.contains("\"execution_id\":\"exec-4\""));

    let back: WorkflowEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(back.execution_id(), "exec-4");
    assert_eq!(back.workflow_name(), Some("wf"));
}

#[test]
fn test_event_accessors() {
    let started = WorkflowEvent::Started {
        execution_id: "e1".to_string(),
        workflow_name: "wf".to_string(),
        trigger_source: None,
        timestamp: chrono::Local::now(),
    };
    assert_eq!(started.execution_id(), "e1");
    assert_eq!(started.workflow_name(), Some("wf"));

    let node_started = WorkflowEvent::NodeStarted {
        execution_id: "e1".to_string(),
        node_id: "n1".to_string(),
        node_type: "llm".to_string(),
        timestamp: chrono::Local::now(),
    };
    assert_eq!(node_started.execution_id(), "e1");
    assert_eq!(node_started.workflow_name(), None);
}

#[tokio::test]
async fn test_panic_in_observer_does_not_affect_others() {
    // A panicking observer must not prevent other observers from receiving
    // events. The manager isolates each emit in its own task with panic
    // recovery.
    struct PanickingObserver;
    #[async_trait]
    impl WorkflowObserver for PanickingObserver {
        fn name(&self) -> &str {
            "bomber"
        }
        async fn on_event(&self, _: WorkflowEvent) {
            panic!("intentional observer panic");
        }
    }

    let manager = WorkflowEventManager::new();
    manager
        .register(Arc::new(PanickingObserver) as Arc<dyn WorkflowObserver>)
        .await;
    let healthy = Arc::new(RecordingObserver::new("healthy"));
    manager
        .register(Arc::clone(&healthy) as Arc<dyn WorkflowObserver>)
        .await;

    let event = WorkflowEvent::Cancelled {
        execution_id: "e2".to_string(),
        workflow_name: "wf".to_string(),
        timestamp: chrono::Local::now(),
    };
    manager.emit(event).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Healthy observer still received the event despite the panic in the
    // other observer's task.
    assert_eq!(healthy.snapshot().len(), 1);
}

// ---------------------------------------------------------------------------
// W4a coverage gap closure (accessor arms for the remaining variants,
// WorkflowEventManager::default)
// ---------------------------------------------------------------------------

#[test]
fn w4a_event_accessors_all_variants() {
    let now = chrono::Local::now();
    let node_completed = WorkflowEvent::NodeCompleted {
        execution_id: "ec".to_string(),
        node_id: "n1".to_string(),
        state: crate::types::ExecutionState::Completed,
        timestamp: now,
    };
    assert_eq!(node_completed.execution_id(), "ec");
    assert_eq!(node_completed.workflow_name(), None);

    let node_failed = WorkflowEvent::NodeFailed {
        execution_id: "ef".to_string(),
        node_id: "n1".to_string(),
        error: "boom".to_string(),
        timestamp: now,
    };
    assert_eq!(node_failed.execution_id(), "ef");
    assert_eq!(node_failed.workflow_name(), None);

    let completed = WorkflowEvent::Completed {
        execution_id: "ed".to_string(),
        workflow_name: "wf_done".to_string(),
        timestamp: now,
    };
    assert_eq!(completed.execution_id(), "ed");
    assert_eq!(completed.workflow_name(), Some("wf_done"));

    let failed = WorkflowEvent::Failed {
        execution_id: "fl".to_string(),
        workflow_name: "wf_fail".to_string(),
        error: "kaput".to_string(),
        timestamp: now,
    };
    assert_eq!(failed.execution_id(), "fl");
    assert_eq!(failed.workflow_name(), Some("wf_fail"));

    let cancelled = WorkflowEvent::Cancelled {
        execution_id: "cx".to_string(),
        workflow_name: "wf_cancel".to_string(),
        timestamp: now,
    };
    assert_eq!(cancelled.execution_id(), "cx");
    assert_eq!(cancelled.workflow_name(), Some("wf_cancel"));
}

#[tokio::test]
async fn w4a_event_manager_default_is_empty() {
    let manager = WorkflowEventManager::default();
    assert!(!manager.has_observers().await);
    // register/unregister still work on the default instance
    let observer = Arc::new(RecordingObserver::new("dflt"));
    manager
        .register(Arc::clone(&observer) as Arc<dyn WorkflowObserver>)
        .await;
    assert!(manager.has_observers().await);
    manager.unregister("dflt").await;
    assert!(!manager.has_observers().await);
}
