//! Workflow-related types.

use serde::{Deserialize, Serialize};

/// Node types in the workflow DAG.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum NodeType {
    Llm,
    Tool,
    Condition,
    Parallel,
    Loop,
    SubWorkflow,
    Transform,
    Http,
    Script,
    Delay,
    HumanReview,
}

/// Workflow node definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowNode {
    pub id: String,
    pub name: String,
    pub node_type: NodeType,
    pub config: serde_json::Value,
    pub next: Vec<String>,
    pub error_handler: Option<String>,
}

/// Workflow trigger definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkflowTrigger {
    Cron { expression: String },
    Event { event_type: String },
    Webhook { path: String },
    Manual,
}

/// Condition for branching in workflows.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Condition {
    pub field: String,
    pub operator: ConditionOperator,
    pub value: serde_json::Value,
}

/// Condition operators.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ConditionOperator {
    Eq,
    Ne,
    Gt,
    Lt,
    Contains,
    Matches,
    And(Vec<Condition>),
    Or(Vec<Condition>),
    Not(Box<Condition>),
}
