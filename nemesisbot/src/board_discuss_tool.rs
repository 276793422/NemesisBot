//! Swarm M3（G4 主动发言通道）：`board_discuss` 工具——cluster agent
//! 主动向 master 看板线程发言（board.comment.post 上行信封）。
//!
//! 与被动响应互补：被动是「被 @ / 指派评论才说话」（cluster_agent.rs 第三
//! select 臂），这个工具让 worker 干活途中主动提问 / 汇报进展 / 补充结论。
//! 注册面：**仅 cluster agent**（`agent_factory.rs::build_cluster_agent_loop`
//! ——master 本尊直写 store 不需要；主 agent 同理不装）。tier 白名单不收录
//! ——与 cluster_rpc 同族（Big/Unresolved 全量可见，mini/normal 不给：
//! 跨节点发言是高信任交互，小模型容易滥用刷屏）。
//!
//! 信封构造 / RPC 往返复用 cluster_agent.rs 的共用件（单一真相源）；
//! coordinator 地址运行时现查（节点表是鲜活的，不用装配期快照）。

use std::sync::Arc;

use nemesis_agent::context::RequestContext;
use nemesis_cluster::cluster::Cluster;
use nemesis_types::cluster::DiscussionEvent;

use crate::cluster_agent::{build_comment_post_envelope, send_nb_bus};

/// 工具注册名。
pub const TOOL_NAME: &str = "board_discuss";

/// board_discuss 工具：向 coordinator 的看板讨论线程发一条消息。
pub struct BoardDiscussTool {
    cluster: Arc<Cluster>,
}

impl BoardDiscussTool {
    pub fn new(cluster: Arc<Cluster>) -> Self {
        Self { cluster }
    }
}

/// 解析后的工具参数（execute 内部第一步；独立成纯函数便于单测）。
#[derive(Debug)]
pub(crate) struct DiscussArgs {
    pub thread_kind: String,
    pub thread_id: i64,
    pub content: String,
    pub reply_to: Option<i64>,
}

/// args JSON → 结构化参数（缺字段 / 词表外 thread_kind / 空 content 一律
/// 诚实报错——args_validator 兜的是 schema 层，语义层这里自己守）。
pub(crate) fn parse_discuss_args(args: &str) -> Result<DiscussArgs, String> {
    let v: serde_json::Value =
        serde_json::from_str(args).map_err(|e| format!("invalid JSON args: {e}"))?;
    let thread_kind = v
        .get("thread_kind")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_lowercase();
    if thread_kind != "issue" && thread_kind != "channel" {
        return Err("thread_kind must be \"issue\" or \"channel\"".to_string());
    }
    let thread_id = v
        .get("thread_id")
        .and_then(|x| x.as_i64())
        .ok_or("missing or non-integer thread_id")?;
    let content = v
        .get("content")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if content.is_empty() {
        return Err("content is empty".to_string());
    }
    let reply_to = v.get("reply_to").and_then(|x| x.as_i64());
    Ok(DiscussArgs {
        thread_kind,
        thread_id,
        content,
        reply_to,
    })
}

impl BoardDiscussTool {
    /// 找在线 coordinator（≠本节点）。master 地址每次现查——节点表鲜活，
    /// 重选主 / 离线切换不需要重启本工具。
    fn find_coordinator(&self, self_node_id: &str) -> Result<String, String> {
        self.cluster
            .list_nodes()
            .into_iter()
            .find(|n| {
                n.base.role == nemesis_types::cluster::NodeRole::Coordinator
                    && n.base.id != self_node_id
                    && n.is_online()
            })
            .map(|n| n.base.id)
            .ok_or_else(|| {
                "no online coordinator known (cluster not started, or master offline \
                 — the board.sync backfill channel will retry later)"
                    .to_string()
            })
    }
}

#[async_trait::async_trait]
impl nemesis_agent::r#loop::Tool for BoardDiscussTool {
    fn description(&self) -> String {
        "Post a message to a board discussion thread on the coordinator node. \
         Use this to ask questions, report progress, or contribute findings to \
         an issue discussion or channel conversation — other nodes and the \
         moderator will see it. The coordinator enforces a per-thread reply \
         budget; exceeding it returns an error."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "thread_kind": {
                    "type": "string",
                    "enum": ["issue", "channel"],
                    "description": "Which kind of thread to post into."
                },
                "thread_id": {
                    "type": "integer",
                    "description": "The issue number or channel id to post into."
                },
                "content": {
                    "type": "string",
                    "description": "Message text. Address a specific node with @node-id, \
                                    a role with @role:qa."
                },
                "reply_to": {
                    "type": "integer",
                    "description": "Optional seq of the message being replied to \
                                    (threads the reply)."
                }
            },
            "required": ["thread_kind", "thread_id", "content"]
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let parsed = parse_discuss_args(args)?;
        let self_node_id = self.cluster.node_id().to_string();
        let target = self.find_coordinator(&self_node_id)?;
        let rpc = self
            .cluster
            .rpc_client_arc()
            .ok_or_else(|| "rpc client unavailable (cluster not started)".to_string())?;

        // 复用被动回帖的信封构造（kind_tag 映射 / 幂等键同源）——最小事件
        // 只填信封消费的字段（thread_kind / thread_id / reply_to）。
        let event = DiscussionEvent {
            thread_kind: parsed.thread_kind.clone(),
            thread_id: parsed.thread_id,
            reply_to: parsed.reply_to,
            ..DiscussionEvent::default()
        };
        let payload = build_comment_post_envelope(&self_node_id, &event, &parsed.content);
        let body = send_nb_bus(&rpc, &self_node_id, &target, payload).await?;

        let ok = body.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
        if ok {
            let seq = body.pointer("/body/seq").and_then(|v| v.as_i64());
            Ok(match seq {
                Some(s) => format!("Posted to {}:{} (seq {s}); coordinator acknowledged.", parsed.thread_kind, parsed.thread_id),
                None => format!("Posted to {}:{}; coordinator acknowledged.", parsed.thread_kind, parsed.thread_id),
            })
        } else {
            let code = body
                .pointer("/error/code")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let msg = body
                .pointer("/error/message")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            Err(format!("coordinator rejected the post: {code} ({msg})"))
        }
    }
}

#[cfg(test)]
mod tests;
