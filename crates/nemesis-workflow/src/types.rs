//! Workflow type definitions.
//!
//! Core types for defining, executing, and tracking workflows.

use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Parse a Go-style duration string or plain number into a Duration.
///
/// Accepts:
/// - `"30s"` -> 30 seconds
/// - `"5m"` -> 5 minutes
/// - `"1h"` -> 1 hour
/// - `"90"` (plain number) -> 90 seconds
/// Returns `None` if the string cannot be parsed.
pub fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    // Try plain number (treated as seconds)
    if let Ok(secs) = s.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }

    // Try Go-style duration string: strip the unit suffix and parse the number
    if let Some(num_str) = s.strip_suffix('s') {
        if let Ok(secs) = num_str.parse::<u64>() {
            return Some(Duration::from_secs(secs));
        }
    } else if let Some(num_str) = s.strip_suffix('m') {
        if let Ok(mins) = num_str.parse::<u64>() {
            return Some(Duration::from_secs(mins * 60));
        }
    } else if let Some(num_str) = s.strip_suffix('h') {
        if let Ok(hours) = num_str.parse::<u64>() {
            return Some(Duration::from_secs(hours * 3600));
        }
    }

    None
}

/// State of a workflow execution or node result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionState {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
    Waiting,
}

impl fmt::Display for ExecutionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExecutionState::Pending => write!(f, "pending"),
            ExecutionState::Running => write!(f, "running"),
            ExecutionState::Completed => write!(f, "completed"),
            ExecutionState::Failed => write!(f, "failed"),
            ExecutionState::Cancelled => write!(f, "cancelled"),
            ExecutionState::Waiting => write!(f, "waiting"),
        }
    }
}

/// Definition of a single node within a workflow DAG.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeDef {
    pub id: String,
    pub node_type: String,
    #[serde(default)]
    pub config: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub retry_count: usize,
    /// Timeout as a Go-style duration string (e.g. "30s", "5m", "1h") or plain number in seconds.
    /// Matches Go's `Timeout string` field so workflow definition files can be shared.
    #[serde(default)]
    pub timeout: Option<String>,
}

impl NodeDef {
    /// Parse the timeout string into a Duration, if set.
    /// Returns None if timeout is not set or cannot be parsed.
    pub fn timeout_duration(&self) -> Option<Duration> {
        self.timeout.as_ref().and_then(|s| parse_duration(s))
    }
}

/// Directed edge between two nodes with an optional condition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub from_node: String,
    pub to_node: String,
    pub condition: Option<String>,
}

/// Trigger configuration for starting a workflow automatically.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerConfig {
    pub trigger_type: String,
    #[serde(default)]
    pub config: HashMap<String, serde_json::Value>,
}

/// A complete workflow definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workflow {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub triggers: Vec<TriggerConfig>,
    pub nodes: Vec<NodeDef>,
    #[serde(default)]
    pub edges: Vec<Edge>,
    /// Workflow variables stored as flat strings, matching Go's `map[string]string`.
    #[serde(default)]
    pub variables: HashMap<String, String>,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

fn default_version() -> String {
    "1.0.0".to_string()
}

/// Result produced by a single node execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeResult {
    pub node_id: String,
    pub output: serde_json::Value,
    pub error: Option<String>,
    pub state: ExecutionState,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// A workflow execution instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Execution {
    pub id: String,
    pub workflow_name: String,
    pub state: ExecutionState,
    #[serde(default)]
    pub input: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub node_results: HashMap<String, NodeResult>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub variables: HashMap<String, String>,
}

impl Execution {
    /// Create a new execution for the given workflow.
    pub fn new(workflow_name: String, input: HashMap<String, serde_json::Value>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            workflow_name,
            state: ExecutionState::Pending,
            input,
            node_results: HashMap::new(),
            started_at: Utc::now(),
            ended_at: None,
            error: None,
            variables: HashMap::new(),
        }
    }
}

#[cfg(test)]
mod tests;
