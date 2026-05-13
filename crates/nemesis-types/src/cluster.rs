//! Cluster-related types.

use serde::{Deserialize, Serialize};

/// Task status in the cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

/// Cluster task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub status: TaskStatus,
    pub action: String,
    pub peer_id: String,
    pub payload: serde_json::Value,
    pub result: Option<serde_json::Value>,
    pub original_channel: String,
    pub original_chat_id: String,
    pub created_at: String,
    pub completed_at: Option<String>,
}

/// Node information in the cluster.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    pub id: String,
    pub name: String,
    pub role: NodeRole,
    pub address: String,
    pub category: String,
    pub last_seen: String,
}

/// Node role in the cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeRole {
    Master,
    Worker,
}

/// RPC message envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcMessage {
    pub id: String,
    pub action: String,
    pub payload: serde_json::Value,
    pub source: String,
    pub target: Option<String>,
    pub timestamp: String,
}
