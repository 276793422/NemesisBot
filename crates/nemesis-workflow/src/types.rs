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
mod tests {
    use super::*;

    #[test]
    fn execution_state_display() {
        assert_eq!(ExecutionState::Pending.to_string(), "pending");
        assert_eq!(ExecutionState::Running.to_string(), "running");
        assert_eq!(ExecutionState::Completed.to_string(), "completed");
        assert_eq!(ExecutionState::Failed.to_string(), "failed");
        assert_eq!(ExecutionState::Cancelled.to_string(), "cancelled");
        assert_eq!(ExecutionState::Waiting.to_string(), "waiting");
    }

    #[test]
    fn execution_state_serde_roundtrip() {
        let state = ExecutionState::Running;
        let json = serde_json::to_string(&state).unwrap();
        assert_eq!(json, "\"running\"");
        let back: ExecutionState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, back);
    }

    #[test]
    fn execution_new_generates_unique_ids() {
        let input = HashMap::new();
        let e1 = Execution::new("test_wf".to_string(), input.clone());
        let e2 = Execution::new("test_wf".to_string(), input);
        assert_ne!(e1.id, e2.id);
        assert_eq!(e1.workflow_name, "test_wf");
        assert_eq!(e1.state, ExecutionState::Pending);
        assert!(e1.ended_at.is_none());
    }

    #[test]
    fn workflow_serialization_roundtrip() {
        let wf = Workflow {
            name: "my_flow".to_string(),
            description: "A test workflow".to_string(),
            version: "2.0.0".to_string(),
            triggers: vec![TriggerConfig {
                trigger_type: "webhook".to_string(),
                config: HashMap::new(),
            }],
            nodes: vec![NodeDef {
                id: "n1".to_string(),
                node_type: "llm".to_string(),
                config: HashMap::new(),
                depends_on: vec![],
                retry_count: 0,
                timeout: None,
            }],
            edges: vec![Edge {
                from_node: "n1".to_string(),
                to_node: "n2".to_string(),
                condition: Some("success".to_string()),
            }],
            variables: HashMap::new(),
            metadata: HashMap::new(),
        };

        let json = serde_json::to_string(&wf).unwrap();
        let back: Workflow = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "my_flow");
        assert_eq!(back.nodes.len(), 1);
        assert_eq!(back.edges[0].condition.as_deref(), Some("success"));
    }

    #[test]
    fn execution_state_all_variants() {
        let states = vec![
            (ExecutionState::Pending, "pending"),
            (ExecutionState::Running, "running"),
            (ExecutionState::Completed, "completed"),
            (ExecutionState::Failed, "failed"),
            (ExecutionState::Cancelled, "cancelled"),
            (ExecutionState::Waiting, "waiting"),
        ];
        for (state, expected) in states {
            assert_eq!(state.to_string(), expected);
        }
    }

    #[test]
    fn node_def_with_config() {
        let node = NodeDef {
            id: "n1".to_string(),
            node_type: "llm".to_string(),
            config: {
                let mut m = HashMap::new();
                m.insert("model".to_string(), serde_json::json!("gpt-4"));
                m.insert("temperature".to_string(), serde_json::json!(0.7));
                m
            },
            depends_on: vec!["n0".to_string()],
            retry_count: 3,
            timeout: Some("60".to_string()),
        };
        let json = serde_json::to_string(&node).unwrap();
        let back: NodeDef = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "n1");
        assert_eq!(back.retry_count, 3);
        assert_eq!(back.timeout, Some("60".to_string()));
        assert_eq!(back.timeout_duration(), Some(Duration::from_secs(60)));
        assert_eq!(back.depends_on, vec!["n0"]);
    }

    #[test]
    fn edge_without_condition() {
        let edge = Edge {
            from_node: "a".to_string(),
            to_node: "b".to_string(),
            condition: None,
        };
        let json = serde_json::to_string(&edge).unwrap();
        let back: Edge = serde_json::from_str(&json).unwrap();
        assert!(back.condition.is_none());
    }

    #[test]
    fn execution_with_input() {
        let mut input = HashMap::new();
        input.insert("query".to_string(), serde_json::json!("test query"));

        let exec = Execution::new("test_wf".to_string(), input.clone());
        assert_eq!(exec.workflow_name, "test_wf");
        assert_eq!(exec.state, ExecutionState::Pending);
        assert_eq!(exec.input.get("query").unwrap(), "test query");
        assert!(exec.node_results.is_empty());
        assert!(exec.error.is_none());
        assert!(exec.variables.is_empty());
    }

    #[test]
    fn workflow_default_version() {
        let wf = Workflow {
            name: "test".to_string(),
            description: String::new(),
            version: default_version(),
            triggers: vec![],
            nodes: vec![],
            edges: vec![],
            variables: HashMap::new(),
            metadata: HashMap::new(),
        };
        assert_eq!(wf.version, "1.0.0");
    }

    #[test]
    fn trigger_config_serialization() {
        let tc = TriggerConfig {
            trigger_type: "cron".to_string(),
            config: {
                let mut m = HashMap::new();
                m.insert("schedule".to_string(), serde_json::json!("0 * * * *"));
                m
            },
        };
        let json = serde_json::to_string(&tc).unwrap();
        let back: TriggerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.trigger_type, "cron");
        assert_eq!(back.config.get("schedule").unwrap(), "0 * * * *");
    }

    #[test]
    fn node_result_serialization() {
        let nr = NodeResult {
            node_id: "n1".to_string(),
            output: serde_json::json!({"result": "ok"}),
            error: None,
            state: ExecutionState::Completed,
            started_at: chrono::Utc::now(),
            ended_at: chrono::Utc::now(),
            metadata: HashMap::new(),
        };
        let json = serde_json::to_string(&nr).unwrap();
        let back: NodeResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.node_id, "n1");
        assert_eq!(back.state, ExecutionState::Completed);
        assert!(back.error.is_none());
    }

    #[test]
    fn node_result_with_error() {
        let nr = NodeResult {
            node_id: "n2".to_string(),
            output: serde_json::Value::Null,
            error: Some("timeout".to_string()),
            state: ExecutionState::Failed,
            started_at: chrono::Utc::now(),
            ended_at: chrono::Utc::now(),
            metadata: HashMap::new(),
        };
        assert_eq!(nr.state, ExecutionState::Failed);
        assert_eq!(nr.error.as_deref(), Some("timeout"));
    }

    #[test]
    fn parse_duration_go_style() {
        assert_eq!(parse_duration("30s"), Some(Duration::from_secs(30)));
        assert_eq!(parse_duration("5m"), Some(Duration::from_secs(300)));
        assert_eq!(parse_duration("1h"), Some(Duration::from_secs(3600)));
        assert_eq!(parse_duration("90"), Some(Duration::from_secs(90)));
        assert_eq!(parse_duration("0"), Some(Duration::from_secs(0)));
        assert_eq!(parse_duration(""), None);
        assert_eq!(parse_duration("abc"), None);
        assert_eq!(parse_duration("  30s  "), Some(Duration::from_secs(30)));
    }

    #[test]
    fn node_def_timeout_duration() {
        let node = NodeDef {
            id: "n1".to_string(),
            node_type: "llm".to_string(),
            config: HashMap::new(),
            depends_on: vec![],
            retry_count: 0,
            timeout: Some("5m".to_string()),
        };
        assert_eq!(node.timeout_duration(), Some(Duration::from_secs(300)));

        let node_no_timeout = NodeDef {
            id: "n2".to_string(),
            node_type: "llm".to_string(),
            config: HashMap::new(),
            depends_on: vec![],
            retry_count: 0,
            timeout: None,
        };
        assert_eq!(node_no_timeout.timeout_duration(), None);
    }

    #[test]
    fn workflow_variables_are_strings() {
        let wf = Workflow {
            name: "test".to_string(),
            description: String::new(),
            version: "1.0.0".to_string(),
            triggers: vec![],
            nodes: vec![],
            edges: vec![],
            variables: {
                let mut m = HashMap::new();
                m.insert("key".to_string(), "value".to_string());
                m.insert("count".to_string(), "42".to_string());
                m
            },
            metadata: HashMap::new(),
        };
        assert_eq!(wf.variables.get("key").unwrap(), "value");
        assert_eq!(wf.variables.get("count").unwrap(), "42");
    }

    #[test]
    fn parse_duration_whitespace_handling() {
        assert_eq!(parse_duration("  60  "), Some(Duration::from_secs(60)));
        assert_eq!(parse_duration("\t30s\t"), Some(Duration::from_secs(30)));
    }

    #[test]
    fn parse_duration_zero_values() {
        assert_eq!(parse_duration("0s"), Some(Duration::from_secs(0)));
        assert_eq!(parse_duration("0m"), Some(Duration::from_secs(0)));
        assert_eq!(parse_duration("0h"), Some(Duration::from_secs(0)));
    }

    #[test]
    fn parse_duration_large_values() {
        assert_eq!(parse_duration("24h"), Some(Duration::from_secs(86400)));
        assert_eq!(parse_duration("60m"), Some(Duration::from_secs(3600)));
    }

    #[test]
    fn parse_duration_invalid_suffix() {
        assert_eq!(parse_duration("30d"), None);
        assert_eq!(parse_duration("1w"), None);
    }

    #[test]
    fn parse_duration_negative_number() {
        // Negative numbers can't parse as u64, so should return None
        assert_eq!(parse_duration("-5"), None);
    }

    #[test]
    fn execution_state_equality() {
        assert_eq!(ExecutionState::Pending, ExecutionState::Pending);
        assert_ne!(ExecutionState::Running, ExecutionState::Completed);
    }

    #[test]
    fn execution_state_copy() {
        let state = ExecutionState::Running;
        let copied = state;
        assert_eq!(state, copied);
    }

    #[test]
    fn node_def_with_all_defaults() {
        let node = NodeDef {
            id: "n1".to_string(),
            node_type: "llm".to_string(),
            config: HashMap::new(),
            depends_on: vec![],
            retry_count: 0,
            timeout: None,
        };
        assert!(node.config.is_empty());
        assert!(node.depends_on.is_empty());
        assert_eq!(node.retry_count, 0);
        assert!(node.timeout.is_none());
        assert!(node.timeout_duration().is_none());
    }

    #[test]
    fn node_def_with_multiple_dependencies() {
        let node = NodeDef {
            id: "n3".to_string(),
            node_type: "tool".to_string(),
            config: HashMap::new(),
            depends_on: vec!["n1".to_string(), "n2".to_string()],
            retry_count: 5,
            timeout: Some("10s".to_string()),
        };
        assert_eq!(node.depends_on.len(), 2);
        assert_eq!(node.retry_count, 5);
        assert_eq!(node.timeout_duration(), Some(Duration::from_secs(10)));
    }

    #[test]
    fn node_def_invalid_timeout() {
        let node = NodeDef {
            id: "n1".to_string(),
            node_type: "llm".to_string(),
            config: HashMap::new(),
            depends_on: vec![],
            retry_count: 0,
            timeout: Some("invalid".to_string()),
        };
        assert!(node.timeout_duration().is_none());
    }

    #[test]
    fn execution_with_node_results() {
        let mut exec = Execution::new("test_wf".to_string(), HashMap::new());
        exec.state = ExecutionState::Running;
        exec.node_results.insert(
            "n1".to_string(),
            NodeResult {
                node_id: "n1".to_string(),
                output: serde_json::json!({"result": "ok"}),
                error: None,
                state: ExecutionState::Completed,
                started_at: chrono::Utc::now(),
                ended_at: chrono::Utc::now(),
                metadata: HashMap::new(),
            },
        );
        assert_eq!(exec.node_results.len(), 1);
        assert_eq!(exec.node_results["n1"].state, ExecutionState::Completed);
    }

    #[test]
    fn execution_with_error() {
        let mut exec = Execution::new("test_wf".to_string(), HashMap::new());
        exec.state = ExecutionState::Failed;
        exec.error = Some("something went wrong".to_string());
        exec.ended_at = Some(chrono::Utc::now());
        assert_eq!(exec.state, ExecutionState::Failed);
        assert_eq!(exec.error.unwrap(), "something went wrong");
        assert!(exec.ended_at.is_some());
    }

    #[test]
    fn execution_with_variables() {
        let mut exec = Execution::new("test_wf".to_string(), HashMap::new());
        exec.variables.insert("key".to_string(), "value".to_string());
        assert_eq!(exec.variables.get("key").unwrap(), "value");
    }

    #[test]
    fn workflow_serialization_with_all_fields() {
        let wf = Workflow {
            name: "full_wf".to_string(),
            description: "A full workflow".to_string(),
            version: "2.0.0".to_string(),
            triggers: vec![TriggerConfig {
                trigger_type: "cron".to_string(),
                config: {
                    let mut m = HashMap::new();
                    m.insert("schedule".to_string(), serde_json::json!("0 * * * *"));
                    m
                },
            }],
            nodes: vec![
                NodeDef {
                    id: "n1".to_string(),
                    node_type: "llm".to_string(),
                    config: {
                        let mut m = HashMap::new();
                        m.insert("model".to_string(), serde_json::json!("gpt-4"));
                        m
                    },
                    depends_on: vec![],
                    retry_count: 3,
                    timeout: Some("30s".to_string()),
                },
                NodeDef {
                    id: "n2".to_string(),
                    node_type: "tool".to_string(),
                    config: HashMap::new(),
                    depends_on: vec!["n1".to_string()],
                    retry_count: 0,
                    timeout: None,
                },
            ],
            edges: vec![Edge {
                from_node: "n1".to_string(),
                to_node: "n2".to_string(),
                condition: Some("success".to_string()),
            }],
            variables: {
                let mut m = HashMap::new();
                m.insert("env".to_string(), "production".to_string());
                m
            },
            metadata: {
                let mut m = HashMap::new();
                m.insert("author".to_string(), "test".to_string());
                m
            },
        };
        let json = serde_json::to_string_pretty(&wf).unwrap();
        let parsed: Workflow = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "full_wf");
        assert_eq!(parsed.nodes.len(), 2);
        assert_eq!(parsed.edges.len(), 1);
        assert_eq!(parsed.triggers.len(), 1);
        assert_eq!(parsed.variables.get("env").unwrap(), "production");
        assert_eq!(parsed.metadata.get("author").unwrap(), "test");
    }

    #[test]
    fn edge_with_condition_serialization() {
        let edge = Edge {
            from_node: "a".to_string(),
            to_node: "b".to_string(),
            condition: Some("x > 5".to_string()),
        };
        let json = serde_json::to_string(&edge).unwrap();
        let parsed: Edge = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.condition, Some("x > 5".to_string()));
    }

    #[test]
    fn node_result_with_metadata() {
        let mut metadata = HashMap::new();
        metadata.insert("duration_ms".to_string(), serde_json::json!(1500));
        let nr = NodeResult {
            node_id: "n1".to_string(),
            output: serde_json::json!({"status": "ok"}),
            error: None,
            state: ExecutionState::Completed,
            started_at: chrono::Utc::now(),
            ended_at: chrono::Utc::now(),
            metadata,
        };
        assert_eq!(nr.metadata.get("duration_ms").unwrap(), 1500);
    }

    #[test]
    fn execution_id_is_uuid_format() {
        let exec = Execution::new("test".to_string(), HashMap::new());
        // UUID v4 format: 8-4-4-4-12
        let parts: Vec<&str> = exec.id.split('-').collect();
        assert_eq!(parts.len(), 5);
        assert_eq!(parts[0].len(), 8);
        assert_eq!(parts[1].len(), 4);
        assert_eq!(parts[2].len(), 4);
        assert_eq!(parts[3].len(), 4);
        assert_eq!(parts[4].len(), 12);
    }
}
