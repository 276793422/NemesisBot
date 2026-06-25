//! Workflow type definitions.
//!
//! Core types for defining, executing, and tracking workflows.

use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

use chrono::{DateTime, Local};
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

/// Origin of a workflow execution. First-class concept borrowed from n8n's
/// Trigger Node model (decision 2 in the integration plan).
///
/// `AgentTool` carries a `recursion_depth` so deeply nested workflow_run calls
/// can be rejected once they exceed `MAX_RECURSION_DEPTH` (decision 6 from the
/// Spike phase, see `WorkflowCallStack`).

/// Maximum nesting depth for workflow_run tool calls (decision 6 from the
/// Spike phase). Depth counts the number of `AgentTool` frames on the call
/// stack:
///   - 0 = top-level workflow (no AgentTool ancestor)
///   - 1 = workflow_run invoked from an agent at top level
///   - 2 = workflow_run → workflow with agent → workflow_run
///   - 3 = one more level (the limit)
///
/// Once a new call would push depth past this limit, the call is rejected.
/// The limit exists because each workflow_run call blocks an LLM-driven tool
/// iteration, and deep nesting can stack up tokio tasks + memory quickly.
pub const MAX_RECURSION_DEPTH: u32 = 3;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TriggerSource {
    /// Triggered from a CLI command (`nemesisbot workflow run`).
    Cli,
    /// Triggered by a cron schedule registered with `nemesis-cron`.
    Cron,
    /// Triggered by an HTTP webhook hit on `/api/workflow/webhook/:name`.
    Webhook {
        #[serde(default)]
        payload: serde_json::Value,
    },
    /// Triggered by an agent invoking the `workflow_run` tool.
    AgentTool {
        tool_call_id: String,
        /// Increments on each nested workflow_run call. Stage 1c
        /// `WorkflowCallStack` rejects calls past `MAX_RECURSION_DEPTH`.
        #[serde(default)]
        recursion_depth: u32,
    },
    /// Triggered by an inbound chat message routed to a workflow.
    Chat {
        chat_id: String,
        session_key: String,
        sender_id: String,
        message: String,
    },
    /// Triggered by an inbound bus message matching a `message` trigger config.
    /// Distinct from `Chat` (which is used when an agent tool returns a chat
    /// message back to the caller): `Message` is the *producer* side — the
    /// gateway subscribes to the inbound bus, matches channel/content/sender
    /// against trigger configs, and starts the workflow with this source.
    Message {
        channel: String,
        chat_id: String,
        sender_id: String,
        content: String,
    },
    /// Triggered from the dashboard / WebSocket UI (milestone 1c-E7).
    /// Carries the WebSocket session_id so downstream handlers can attribute
    /// the run back to the originating user.
    WebUI {
        #[serde(default)]
        session_id: String,
    },
    /// Triggered by a generic event bus subscription.
    Event {
        event_type: String,
        #[serde(default)]
        data: serde_json::Value,
    },
    /// Triggered from the dedicated workflow-chat page (`/#/workflow/chat/<index>`).
    ///
    /// The chat page is the URL contract — being on the page runs that workflow
    /// unconditionally, no trigger config needed. This variant carries both
    /// the transport identifiers (needed to route the reply OutboundMessage
    /// back to the originating WebSocket) and the logical identifiers (used
    /// for session_key namespacing so memory/agent nodes fetch the right
    /// per-workflow conversation history).
    WorkflowChat {
        /// Transport chat_id, format `"web:<session_id>"`. The reply observer
        /// publishes OutboundMessage with this chat_id so the existing
        /// `crates/nemesis-web/src/server.rs:958` dispatcher routes it back
        /// to the right WebSocket session.
        chat_id: String,
        /// Bare session_id (without the `"web:"` prefix). Used for direct
        /// `send_to_session` calls if needed.
        session_id: String,
        /// The workflow that's running. Used by the reply observer to look
        /// up the workflow def (terminal agent nodes etc.) without re-resolving
        /// from index.
        workflow_name: String,
        /// Logical index, first 8 hex chars of `sha256(name)`. Same value
        /// as the URL path segment. Mostly for logging/diagnostics.
        index: String,
        /// Logical session_key, format `"wf_chat:<workflow_name>"`. Memory
        /// and agent nodes inside the workflow use this to fetch prior turns
        /// of the same workflow-chat conversation.
        session_key: String,
    },
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
    /// Marks this node as a workflow output source. When set, the node's
    /// `output` is merged into the workflow's final result returned to
    /// callers (decision 5 in the integration plan, used by `workflow_run`
    /// agent tool). If no node is marked terminal, leaf nodes (no
    /// downstream edges) are used as fallback.
    #[serde(default)]
    pub is_terminal: bool,
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

impl Workflow {
    /// Compute the workflow's output by merging `NodeResult::output` from
    /// terminal nodes.
    ///
    /// - If one or more nodes have `is_terminal: true`, only those nodes'
    ///   outputs are merged (object fields combined; non-object values are
    ///   keyed by node id). Later nodes overwrite earlier ones on key
    ///   collision.
    /// - If no node is marked terminal, falls back to leaf nodes (nodes with
    ///   no downstream edges in `self.edges`).
    /// - Returns `Value::Null` when no terminal/leaf node produced output.
    ///
    /// Decision 5 in the integration plan: surfaces workflow results back to
    /// the LLM via the `workflow_run` agent tool.
    pub fn compute_output(
        &self,
        node_results: &HashMap<String, NodeResult>,
    ) -> serde_json::Value {
        let terminal_ids: Vec<&str> = self
            .nodes
            .iter()
            .filter(|n| n.is_terminal)
            .map(|n| n.id.as_str())
            .collect();

        let output_ids: Vec<&str> = if !terminal_ids.is_empty() {
            terminal_ids
        } else {
            let downstream: Vec<&str> = self.edges.iter().map(|e| e.from_node.as_str()).collect();
            self.nodes
                .iter()
                .filter(|n| !downstream.contains(&n.id.as_str()))
                .map(|n| n.id.as_str())
                .collect()
        };

        let mut merged = serde_json::Map::new();
        for id in output_ids {
            if let Some(nr) = node_results.get(id) {
                if let Some(obj) = nr.output.as_object() {
                    for (k, v) in obj {
                        merged.insert(k.clone(), v.clone());
                    }
                } else if !nr.output.is_null() {
                    merged.insert(id.to_string(), nr.output.clone());
                }
            }
        }

        if merged.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::Value::Object(merged)
        }
    }

    /// Compute a stable SHA-256 hash of the workflow's structural content.
    ///
    /// Used by the Checkpointer (1b-A1) to detect config drift between
    /// checkpoint save and resume — if the workflow definition changed
    /// (nodes added/removed, edges rewired, node configs altered) the hash
    /// changes and the resume path can refuse or warn.
    ///
    /// Only structural fields are hashed: name, nodes, edges. `description`,
    /// `version`, `metadata`, `triggers`, `variables` are excluded so cosmetic
    /// edits and trigger-only changes don't invalidate in-flight checkpoints.
    /// The hash is the hex digest of canonical JSON
    /// (`{"name":...,"nodes":[...],"edges":[...]}`).
    pub fn hash(&self) -> String {
        use sha2::{Digest, Sha256};
        // Build a stable canonical form: keys in deterministic order, no
        // pretty-printing, sorted-by-id node list.
        let mut nodes_sorted = self.nodes.clone();
        nodes_sorted.sort_by(|a, b| a.id.cmp(&b.id));
        let mut edges_sorted = self.edges.clone();
        edges_sorted.sort_by(|a, b| {
            (a.from_node.as_str(), a.to_node.as_str()).cmp(&(b.from_node.as_str(), b.to_node.as_str()))
        });

        // Serialise each field independently to control key order and skip
        // non-structural fields. Wrap in a single JSON object for hashing.
        let canonical = serde_json::json!({
            "name": self.name,
            "nodes": nodes_sorted,
            "edges": edges_sorted,
        });
        let bytes = serde_json::to_vec(&canonical).unwrap_or_default();
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        format!("{:x}", hasher.finalize())
    }
}

/// Result produced by a single node execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeResult {
    pub node_id: String,
    pub output: serde_json::Value,
    pub error: Option<String>,
    pub state: ExecutionState,
    pub started_at: DateTime<Local>,
    pub ended_at: DateTime<Local>,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// A workflow execution instance.
///
/// Extended in 1a-B1 with `trigger_source`, `chat_id`, `session_key`, `owner`,
/// `tags`, `workflow_hash` to support agent tool calls, cron ownership, UI
/// filtering, and Checkpointer config-drift detection. All new fields use
/// `#[serde(default)]` so old JSONL files load with sensible defaults.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Execution {
    pub id: String,
    pub workflow_name: String,
    pub state: ExecutionState,
    #[serde(default)]
    pub input: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub node_results: HashMap<String, NodeResult>,
    pub started_at: DateTime<Local>,
    pub ended_at: Option<DateTime<Local>>,
    #[serde(default)]
    pub error: Option<String>,
    /// Per-execution variables. JSON-typed since 1b-B3 so workflows can carry
    /// arbitrary structures. Old JSONL files stored `HashMap<String, String>`
    /// — `serde_json::Value` deserialises those as `Value::String` automatically,
    /// so no explicit migration is required.
    #[serde(default)]
    pub variables: HashMap<String, serde_json::Value>,

    // --- 1a-B1 additions ---
    /// What triggered this execution. `None` for legacy executions created
    /// before the field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_source: Option<TriggerSource>,
    /// Chat/conversation ID when triggered from chat or agent tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<String>,
    /// Session key for memory persistence scoping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_key: Option<String>,
    /// Owner (user/device) for cron/webhook executions that have no chat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// Free-form tags for filtering / grouping in the UI.
    #[serde(default)]
    pub tags: HashMap<String, String>,
    /// Hash of the workflow definition at execution start, used by the
    /// Checkpointer (1b) to warn on resume-time config drift.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_hash: Option<String>,
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
            started_at: Local::now(),
            ended_at: None,
            error: None,
            variables: HashMap::new(),
            trigger_source: None,
            chat_id: None,
            session_key: None,
            owner: None,
            tags: HashMap::new(),
            workflow_hash: None,
        }
    }
}

#[cfg(test)]
mod tests;
