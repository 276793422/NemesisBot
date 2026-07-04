//! Checkpoint data structures — milestone 1b-A1 step 1.
//!
//! These are the durable, serialisable forms of the engine's in-memory state.
//! Designed to be stable across versions:
//! - `#[serde(default)]` on every optional / new field
//! - state stored as a snake_case string so adding new variants doesn't break
//!   old snapshots (the engine clamps unknown strings to a sane default)
//! - DateTime stored as UTC; local-time rendering is the UI's job

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Durable snapshot of a workflow execution at a single point in time.
///
/// One checkpoint is written per node completion (1b-A1 step 6) so the engine
/// can resume from the most recent consistent state after a crash.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Checkpoint {
    /// Unique checkpoint ID (UUID v4).
    pub id: String,
    /// Execution this checkpoint belongs to.
    pub execution_id: String,
    /// When the checkpoint was written (UTC).
    pub saved_at: DateTime<Utc>,
    /// Node IDs that had already completed (in any prior checkpoint + this one).
    /// Used by `schedule_resume` to skip already-run nodes.
    #[serde(default)]
    pub completed_nodes: HashSet<String>,
    /// Set when the execution is paused at a `human_review` node. `None` for
    /// an in-flight checkpoint. (Spike 2 decision.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_node: Option<String>,
    /// Parent execution ID for nested / sub-workflow executions. `None` for
    /// top-level executions. (Spike 3 decision 3.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_execution_id: Option<String>,
    /// What triggered this execution. Restored from the execution's
    /// `trigger_source` field so post-restore observers can still tell
    /// webhook / cli / agent invocations apart. (Gap 1 fix.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_source: Option<crate::types::TriggerSource>,
    /// `true` when this checkpoint captures a terminal state
    /// (Completed / Failed / Cancelled). `restore_incomplete_executions`
    /// skips terminal checkpoints so finished workflows don't get
    /// resurrected by the next process restart. (Gap 2 fix.)
    #[serde(default)]
    pub terminal: bool,
    /// Snapshot of the workflow context (variables, node results, input).
    pub context_snapshot: SerializableContext,
    /// Hash of the workflow definition at save time. Used to detect config
    /// drift between checkpoint and resume.
    ///
    /// Format: see [`crate::types::Workflow::hash`] — currently a SHA-256 of
    /// the canonical JSON serialisation.
    pub workflow_hash: String,
}

/// Compact, listing-friendly view of a checkpoint. Returned by `list()` so
/// callers don't need to load the full context snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CheckpointMeta {
    pub id: String,
    pub execution_id: String,
    pub saved_at: DateTime<Utc>,
    pub completed_node_count: usize,
    /// `true` if this checkpoint captures a paused (Waiting) state.
    pub has_waiting: bool,
    /// `true` if this checkpoint captures a terminal state. Mirrors
    /// [`Checkpoint::terminal`] so listing endpoints can hide finished
    /// executions without loading the full snapshot.
    #[serde(default)]
    pub terminal: bool,
}

impl From<&Checkpoint> for CheckpointMeta {
    fn from(cp: &Checkpoint) -> Self {
        Self {
            id: cp.id.clone(),
            execution_id: cp.execution_id.clone(),
            saved_at: cp.saved_at,
            completed_node_count: cp.completed_nodes.len(),
            has_waiting: cp.waiting_node.is_some(),
            terminal: cp.terminal,
        }
    }
}

/// Serialisable form of [`crate::context::WorkflowContext`].
///
/// We use a separate struct instead of serialising `WorkflowContext` directly
/// because `WorkflowContext` wraps its state in `RwLock`, which doesn't impl
/// `Serialize`. (Spike 1 verification point 1.)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SerializableContext {
    /// JSON-typed variables (since 1b-B3).
    #[serde(default)]
    pub variables: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub node_results: HashMap<String, SerializableNodeResult>,
    #[serde(default)]
    pub input: HashMap<String, serde_json::Value>,
}

/// Serialisable form of [`crate::types::NodeResult`].
///
/// `state` is a snake_case string (not the enum) so adding new
/// `ExecutionState` variants doesn't break old snapshots — unknown strings
/// fall back to `Pending` on load. `started_at` / `ended_at` use UTC.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SerializableNodeResult {
    pub node_id: String,
    pub output: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// `ExecutionState` rendered as snake_case. Use [`parse_state`] to
    /// convert back.
    pub state: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Convert a serialised state string back to the enum.
///
/// Unknown strings become [`crate::types::ExecutionState::Pending`] (safe
/// default that lets the resume path inspect the loaded state).
pub fn parse_state(s: &str) -> crate::types::ExecutionState {
    use crate::types::ExecutionState::*;
    match s {
        "pending" => Pending,
        "running" => Running,
        "completed" => Completed,
        "failed" => Failed,
        "cancelled" => Cancelled,
        "waiting" => Waiting,
        _ => Pending,
    }
}

#[cfg(test)]
mod tests;
