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

use crate::cluster_agent::{build_comment_list_envelope, build_comment_post_envelope, send_nb_bus};

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

/// 解析后的动作（D2 读取臂：args 无 `content` = 读线程）。
#[derive(Debug)]
pub(crate) enum DiscussAction {
    Post(DiscussArgs),
    Read { thread_kind: String, thread_id: i64 },
}

/// args JSON → 结构化动作（缺字段 / 词表外 thread_kind / 空 content 一律
/// 诚实报错——args_validator 兜的是 schema 层，语义层这里自己守）。
/// D2：`content` 键缺席或 null = 读取臂（轮询讨论线程）；键在场但空 =
/// 报错（写了 content: "" 是笔误不是读意图，诚实拒绝）。
pub(crate) fn parse_discuss_args(args: &str) -> Result<DiscussAction, String> {
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
    let content = match v.get("content") {
        None | Some(serde_json::Value::Null) => {
            return Ok(DiscussAction::Read {
                thread_kind,
                thread_id,
            });
        }
        Some(x) => x.as_str().unwrap_or("").trim().to_string(),
    };
    if content.is_empty() {
        return Err("content is empty".to_string());
    }
    let reply_to = v.get("reply_to").and_then(|x| x.as_i64());
    Ok(DiscussAction::Post(DiscussArgs {
        thread_kind,
        thread_id,
        content,
        reply_to,
    }))
}

impl BoardDiscussTool {
    /// 找在线 coordinator（≠本节点）。实现抽到 cluster_agent.rs 共享
    /// （[`crate::cluster_agent::find_online_coordinator`]）——board_claim
    /// 工具同源，master 地址运行时现查（节点表鲜活，重选主 / 离线切换
    /// 不需要重启本工具）。
    fn find_coordinator(&self, self_node_id: &str) -> Result<String, String> {
        crate::cluster_agent::find_online_coordinator(&self.cluster, self_node_id)
    }
}

#[async_trait::async_trait]
impl nemesis_agent::r#loop::Tool for BoardDiscussTool {
    fn description(&self) -> String {
        "Post a message to a board discussion thread on the coordinator node, or READ \
         a thread's messages (omit the content field to read). Use posting to ask \
         questions, report progress, or contribute findings to an issue discussion or \
         channel conversation — other nodes and the moderator will see it. Use reading \
         to poll a thread for replies (e.g. during a review wait). The coordinator \
         enforces a per-thread reply budget on posting; exceeding it returns an error."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "thread_kind": {
                    "type": "string",
                    "enum": ["issue", "channel"],
                    "description": "Which kind of thread to post into or read from."
                },
                "thread_id": {
                    "type": "integer",
                    "description": "The issue number or channel id."
                },
                "content": {
                    "type": "string",
                    "description": "Message text to post. Omit this field entirely to \
                                    READ the thread instead of posting (D2 review wait: \
                                    poll for replies). Address a specific node with \
                                    @node-id, a role with @role:qa."
                },
                "reply_to": {
                    "type": "integer",
                    "description": "Optional seq of the message being replied to \
                                    (threads the reply; posting only)."
                }
            },
            "required": ["thread_kind", "thread_id"]
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let self_node_id = self.cluster.node_id().to_string();
        let target = self.find_coordinator(&self_node_id)?;
        let rpc = self
            .cluster
            .rpc_client_arc()
            .ok_or_else(|| "rpc client unavailable (cluster not started)".to_string())?;

        match parse_discuss_args(args)? {
            DiscussAction::Read {
                thread_kind,
                thread_id,
            } => {
                let payload = build_comment_list_envelope(&thread_kind, thread_id, 200);
                let body = send_nb_bus(&rpc, &self_node_id, &target, payload).await?;
                let ok = body.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
                if ok {
                    Ok(format_comment_list(&thread_kind, thread_id, &body))
                } else {
                    let code = body
                        .pointer("/error/code")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    let msg = body
                        .pointer("/error/message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    Err(format!("coordinator rejected the read: {code} ({msg})"))
                }
            }
            DiscussAction::Post(parsed) => {
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
                        Some(s) => format!(
                            "Posted to {}:{} (seq {s}); coordinator acknowledged.",
                            parsed.thread_kind, parsed.thread_id
                        ),
                        None => format!(
                            "Posted to {}:{}; coordinator acknowledged.",
                            parsed.thread_kind, parsed.thread_id
                        ),
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
    }
}

/// 读取臂响应 → LLM 可读文本（纯函数便于单测）。空线程 = 诚实声明（
/// 不是错误——定稿等待「尚无回复」是正常中间态）。
///
/// 入参 `body` 是 nb_bus RPC 的完整信封响应（`EnvelopeResponse` 序列化形
/// `{"ok":..,"error":..,"body":{...}}`）——messages 在 `/body/messages`，
/// 与 POST 臂读 `/body/seq`、master 侧 sync 测试读 `body.body.messages`
/// 同一口径。实录 2026-10-14（run4）：读顶层 `messages` 恒得空 → D2 对话
/// 机 sleep↔read 永动 → U9 墙钟红；单测按顶层形状手搓镜像了 bug 所以自测
/// 绿——测试体必须复刻线上信封形状而非想象形状。
pub(crate) fn format_comment_list(
    thread_kind: &str,
    thread_id: i64,
    body: &serde_json::Value,
) -> String {
    let messages = body
        .pointer("/body/messages")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    if messages.is_empty() {
        return format!(
            "Thread {thread_kind}:{thread_id} has no messages yet (no replies so far — \
             keep waiting or finalize with what you have)."
        );
    }
    let mut out = format!(
        "Thread {thread_kind}:{thread_id} ({} messages):\n",
        messages.len()
    );
    for m in &messages {
        let id = m.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
        // 身份透传（2026-10-23 同族横扫）：master 返回 sender {type,id}，
        // 渲染带身份标注（admin=human user / agent=agent node）——读线程的
        // agent 与被动唤醒轮同样需要知道说话的是人还是节点。
        let sender = crate::board_bus::actor_label(
            m.pointer("/sender/type")
                .and_then(|v| v.as_str())
                .unwrap_or(""),
            m.pointer("/sender/id")
                .and_then(|v| v.as_str())
                .unwrap_or("?"),
        );
        let content = m.get("content").and_then(|v| v.as_str()).unwrap_or("");
        out.push_str(&format!("- [#{id}] {sender}: {content}\n"));
    }
    out
}

#[cfg(test)]
mod tests;
