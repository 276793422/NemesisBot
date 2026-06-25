//! Workflow trigger-event dispatcher — a lightweight pub/sub bus for
//! system-internal events that should fire workflows with
//! `trigger_type: "event"`.
//!
//! ## What it solves
//!
//! Before this module existed, [`crate::triggers::TriggerManager::match_event`]
//! was dead code: nothing in the codebase called it, so `event` triggers never
//! fired. This module provides the missing producer side:
//!
//! 1. Business code calls [`EventDispatcher::publish`] when something
//!    interesting happens (workflow completes, forge produces a pattern, etc.).
//! 2. The gateway spawns a subscriber that turns each event into a
//!    [`crate::triggers::TriggerManager::match_event`] lookup, then calls
//!    [`crate::engine::WorkflowEngine::start_async`] for each hit.
//!
//! ## Why a separate bus instead of reusing the inbound message bus
//!
//! `InboundMessage` is shaped for chat (sender_id, chat_id, content, media)
//! and would force every event to fake those fields. A dedicated
//! [`TriggerEvent`] is cheaper and clearer.
//!
//! ## Naming
//!
//! Note: this is *not* [`crate::events::WorkflowEvent`] (the engine's
//! lifecycle observer enum). The two types serve different purposes:
//! - lifecycle `WorkflowEvent` = "engine state changed" → observers
//!   (logs, metrics)
//! - `TriggerEvent` = "something happened in the system" → trigger matching
//!   → workflow execution
//!
//! ## Channel choice
//!
//! `tokio::sync::broadcast` because (a) we already use it for the inbound bus,
//! (b) it's fan-out so multiple subscribers can observe events (the workflow
//! trigger subscriber + future observers like audit log), and (c) lagged
//! receivers get a clear error they can recover from.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast;

/// A single event fired into the dispatcher. The `event_type` string is the
/// primary match key (e.g. `"workflow.completed"`, `"forge.pattern_created"`).
/// `data` carries optional fields the trigger config can match against.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerEvent {
    /// Dotted event type — convention is `namespace.action`, e.g.
    /// `workflow.completed`, `user.login`, `forge.pattern_created`.
    pub event_type: String,
    /// Arbitrary key/value payload. Trigger configs match these with glob
    /// patterns (see [`crate::triggers::TriggerManager::match_event`]).
    #[serde(default)]
    pub data: HashMap<String, serde_json::Value>,
    /// When the event was produced (server clock). Useful for ordering and
    /// for trigger configs that filter by recency.
    pub timestamp: DateTime<Utc>,
    /// Optional originating workflow execution. Set when one workflow's
    /// completion fires an event that another workflow listens to — useful
    /// for audit chains.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_execution_id: Option<String>,
}

impl TriggerEvent {
    /// Build a new event with the given type and the current time.
    /// `data` is for additional matchable fields (can be empty).
    pub fn new(event_type: impl Into<String>, data: HashMap<String, serde_json::Value>) -> Self {
        Self {
            event_type: event_type.into(),
            data,
            timestamp: Utc::now(),
            source_execution_id: None,
        }
    }

    /// Builder-style setter for `source_execution_id`.
    pub fn with_source_execution_id(mut self, id: impl Into<String>) -> Self {
        self.source_execution_id = Some(id.into());
        self
    }
}

/// Fan-out event bus for [`TriggerEvent`]. Cheap to clone (internally an
/// `Arc` around a broadcast channel).
#[derive(Clone)]
pub struct EventDispatcher {
    tx: Arc<broadcast::Sender<TriggerEvent>>,
}

impl EventDispatcher {
    /// Create a new dispatcher with the given subscriber buffer capacity.
    /// 256 is plenty for trigger events (low volume compared to chat).
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx: Arc::new(tx) }
    }

    /// Default capacity (256) — enough headroom for bursts without holding
    /// memory long-term.
    pub fn default_capacity() -> usize {
        256
    }

    /// Publish an event. Subscribers receive a clone. Logs a debug line if
    /// there are no subscribers (the event is lost) — same pattern as the
    /// inbound bus.
    pub fn publish(&self, event: TriggerEvent) {
        if self.tx.receiver_count() == 0 {
            tracing::debug!(
                event_type = %event.event_type,
                "[WorkflowEventDispatcher] no subscribers, event dropped"
            );
            return;
        }
        let _ = self.tx.send(event);
    }

    /// Subscribe to the dispatcher. Each subscriber gets its own copy of
    /// every event (fan-out).
    pub fn subscribe(&self) -> broadcast::Receiver<TriggerEvent> {
        self.tx.subscribe()
    }

    /// Number of active subscribers. Mostly useful for diagnostics.
    pub fn subscriber_count(&self) -> usize {
        self.tx.receiver_count()
    }
}

impl Default for EventDispatcher {
    fn default() -> Self {
        Self::new(Self::default_capacity())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
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
}
