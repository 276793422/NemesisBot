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
    /// 看板权威节点（board 读写全开；旧词 master/manager 兼容解析）。
    #[serde(alias = "Master")]
    Coordinator,
    Worker,
}

impl NodeRole {
    /// 解析角色字符串（单一真相源：peers.toml `[node].role`、UDP 广播、
    /// 身份更新都走这里）。接受现行词表 `coordinator` 与旧值
    /// `master`/`manager`（向后兼容），其余一律回落 Worker。
    pub fn from_role_str(s: &str) -> Self {
        match s {
            "master" | "manager" | "coordinator" => NodeRole::Coordinator,
            _ => NodeRole::Worker,
        }
    }

    /// 规范配置词表（写 peers.toml / 广播身份用）。
    pub fn as_role_str(&self) -> &'static str {
        match self {
            NodeRole::Coordinator => "coordinator",
            NodeRole::Worker => "worker",
        }
    }
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
