//! Workflow event system.
//!
//! Provides an observer-pattern hook for external systems (web dashboard,
//! log shippers, metrics collectors) to react to workflow lifecycle events.
//!
//! Engine-level events (Started/Completed/Failed/Cancelled) are emitted by
//! [`crate::engine::WorkflowEngine`] at the boundaries of `create_execution`
//! and `run_async`. Per-node events (NodeStarted/NodeCompleted/NodeFailed)
//! are reserved for a future scheduler-integrated emitter; the variants are
//! defined here so observers can pattern-match on them today.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::warn;

use crate::types::{ExecutionState, TriggerSource};

/// Workflow lifecycle event emitted by the engine.
///
/// Variants are grouped by scope: workflow-level (Started/Completed/Failed/
/// Cancelled) fire once per execution; node-level (NodeStarted/NodeCompleted/
/// NodeFailed) fire once per node execution. Node-level events are not yet
/// wired through the scheduler — observers should handle their absence
/// gracefully.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkflowEvent {
    /// A new execution was created (state = Running). Emitted from
    /// `create_execution` after the execution record is persisted.
    Started {
        execution_id: String,
        workflow_name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trigger_source: Option<TriggerSource>,
        timestamp: DateTime<Local>,
    },
    /// A node began executing. (Reserved — not currently emitted.)
    NodeStarted {
        execution_id: String,
        node_id: String,
        node_type: String,
        timestamp: DateTime<Local>,
    },
    /// A node reached a non-failed terminal state (Completed or Waiting).
    /// (Reserved — not currently emitted.)
    NodeCompleted {
        execution_id: String,
        node_id: String,
        state: ExecutionState,
        timestamp: DateTime<Local>,
    },
    /// A node failed during execution. (Reserved — not currently emitted.)
    NodeFailed {
        execution_id: String,
        node_id: String,
        error: String,
        timestamp: DateTime<Local>,
    },
    /// Execution reached Completed state. Emitted from `run_async` after
    /// the final state is persisted.
    Completed {
        execution_id: String,
        workflow_name: String,
        timestamp: DateTime<Local>,
    },
    /// Execution reached Failed state. Emitted from `run_async` after the
    /// final state (including the error message) is persisted.
    Failed {
        execution_id: String,
        workflow_name: String,
        error: String,
        timestamp: DateTime<Local>,
    },
    /// Execution reached Cancelled state (via `cancel_execution` or
    /// `CancellationToken`). Emitted from `run_async`.
    Cancelled {
        execution_id: String,
        workflow_name: String,
        timestamp: DateTime<Local>,
    },
}

impl WorkflowEvent {
    /// Execution ID the event pertains to. Convenience accessor for
    /// observers that route events by execution.
    pub fn execution_id(&self) -> &str {
        match self {
            Self::Started { execution_id, .. }
            | Self::NodeStarted { execution_id, .. }
            | Self::NodeCompleted { execution_id, .. }
            | Self::NodeFailed { execution_id, .. }
            | Self::Completed { execution_id, .. }
            | Self::Failed { execution_id, .. }
            | Self::Cancelled { execution_id, .. } => execution_id,
        }
    }

    /// Workflow name the event pertains to, where applicable. Node-level
    /// events do not carry the workflow name and return `None`.
    pub fn workflow_name(&self) -> Option<&str> {
        match self {
            Self::Started { workflow_name, .. }
            | Self::Completed { workflow_name, .. }
            | Self::Failed { workflow_name, .. }
            | Self::Cancelled { workflow_name, .. } => Some(workflow_name),
            _ => None,
        }
    }
}

/// Observer trait for receiving [`WorkflowEvent`]s.
///
/// Implementations must be `Send + Sync` because events are delivered via
/// spawned tokio tasks. Observer methods should not panic — the manager
/// spawns each handler independently so one panicking observer does not
/// affect others, but the panic is logged as a warning.
#[async_trait]
pub trait WorkflowObserver: Send + Sync {
    /// Stable identifier used for unregister/dedup. Two observers with the
    /// same name will both receive events; `unregister(name)` removes all
    /// matching.
    fn name(&self) -> &str;

    /// Handle a single event. Runs in its own tokio task per observer per
    /// emit; long-running work is fine but blocks only that one observer.
    async fn on_event(&self, event: WorkflowEvent);
}

/// Manager for a collection of [`WorkflowObserver`]s.
///
/// Mirrors the structure of `nemesis_observer::Manager` but for
/// workflow-scoped events. Held by [`crate::engine::WorkflowEngine`] and
/// accessible via `engine.event_manager()`.
pub struct WorkflowEventManager {
    observers: Arc<RwLock<Vec<Arc<dyn WorkflowObserver>>>>,
}

impl Default for WorkflowEventManager {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkflowEventManager {
    /// Create an empty manager.
    pub fn new() -> Self {
        Self {
            observers: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Register an observer. Subsequent emits will deliver to it.
    pub async fn register(&self, observer: Arc<dyn WorkflowObserver>) {
        let mut obs = self.observers.write().await;
        obs.push(observer);
    }

    /// Remove all observers with the given name. No-op if none match.
    pub async fn unregister(&self, name: &str) {
        let mut obs = self.observers.write().await;
        obs.retain(|o| o.name() != name);
    }

    /// Remove all observers.
    pub async fn unregister_all(&self) {
        let mut obs = self.observers.write().await;
        obs.clear();
    }

    /// Whether any observers are registered. Useful for short-circuiting
    /// event construction when there is nothing to deliver to.
    pub async fn has_observers(&self) -> bool {
        let obs = self.observers.read().await;
        !obs.is_empty()
    }

    /// Deliver an event to every registered observer. Each delivery runs in
    /// its own tokio task; a panic in one observer is logged but does not
    /// affect the others. Returns immediately after spawning (fire-and-forget).
    pub async fn emit(&self, event: WorkflowEvent) {
        let observers = self.observers.read().await;
        for obs in observers.iter() {
            let o = Arc::clone(obs);
            let e = event.clone();
            let name = o.name().to_string();
            tokio::spawn(async move {
                let handle = tokio::spawn(async move {
                    o.on_event(e).await;
                });
                if let Err(err) = handle.await {
                    if err.is_panic() {
                        warn!("WorkflowObserver {} panicked during emit", name);
                    }
                }
            });
        }
    }
}

#[cfg(test)]
mod tests;
