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

/// 看板讨论唤醒事件（Swarm M3：wake.post 下行 / board.sync 补拉的反序列化
/// 产物，投进 cluster agent loop 的 select 队列——worker 端 handler 越薄
/// 越好，这里只带数据不带行为）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DiscussionEvent {
    /// 线程种类（issue / channel）。
    pub thread_kind: String,
    /// 线程 id（issue 编号 / 频道 id）。
    pub thread_id: i64,
    /// 线程标题（issue 有，频道为空）。
    #[serde(default)]
    pub thread_title: String,
    /// 线程上下文（wake 包随带最近 N 条；board.sync 补拉为空——历史在
    /// master 台账，agent 拿到的就是补投到醒为止的最小信息）。
    #[serde(default)]
    pub messages: Vec<DiscussionCtxMessage>,
    /// 唤醒原因（mention / assignee_comment / moderator_call / sync_backfill）。
    #[serde(default)]
    pub event: String,
    /// master 节点 id（回复路由目标；来自 RPC 帧 `_rpc.from`，伪造不了）。
    #[serde(default)]
    pub from_node: String,
    /// 触发消息的发送者。
    #[serde(default)]
    pub new_sender: String,
    /// 触发消息内容。
    #[serde(default)]
    pub new_content: String,
    /// 触发消息时间（unix secs）。
    #[serde(default)]
    pub new_at: i64,
    /// 建议回复目标（master 台账 message id）。
    #[serde(default)]
    pub reply_to: Option<i64>,
    /// 本线程剩余 agent 发言额度（master 记账随包下发；建议值，真正扣账
    /// 在 master 上行 handler）。
    #[serde(default)]
    pub max_turns_left: u32,
    /// master 单调序号（下行幂等游标：worker 记录每线程已处理最大 seq，
    /// `seq ≤` 已处理的直接丢弃）。
    pub seq: i64,
}

/// 线程上下文里的一条消息（wake 包 messages 数组元素）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DiscussionCtxMessage {
    pub sender: String,
    pub content: String,
    /// unix secs。
    pub at: i64,
}
