//! 集群任务认领（2026-10-22 计划 §3.6）：`board_claim` 工具——cluster
//! agent 主动浏览待领单 / 认领（board.claimable.list / board.claim 上行
//! 信封）。
//!
//! 讨论组「认领制」的 worker 侧：组长在频道发任务后，组员 agent 用
//! browse 看待领单（含硬闸满足标记）自评「我合适」，claim 先到先得——
//! master 原子裁决，认领成功即经既有派发链落进本节点工作队列。
//! 注册面：**仅 cluster agent**（`agent_factory.rs`，与 board_discuss
//! 同点）。tier 白名单不收录——与 board_discuss/cluster_rpc 同族
//! （Big/Unresolved 全量可见，mini/normal 不给：跨节点认领与跨节点发言
//! 同信任级，Q5 拍板）。
//!
//! 信封构造在本文件（两个 op 单消费者，不进 cluster_agent 共用件）；
//! RPC 往返与 coordinator 现查找复用 cluster_agent.rs 共享件。

use std::sync::Arc;

use nemesis_agent::context::RequestContext;
use nemesis_cluster::cluster::Cluster;
use nemesis_cluster::envelope::{Envelope, EnvelopeResponse};

use crate::cluster_agent::{find_online_coordinator, send_nb_bus};

/// 工具注册名。
pub const TOOL_NAME: &str = "board_claim";

/// board_claim 工具：浏览 coordinator 的待领单列表，或认领其中一单。
pub struct BoardClaimTool {
    cluster: Arc<Cluster>,
}

impl BoardClaimTool {
    pub fn new(cluster: Arc<Cluster>) -> Self {
        Self { cluster }
    }
}

/// 解析后的动作（execute 内部第一步；独立成纯函数便于单测）。
#[derive(Debug)]
pub(crate) enum ClaimAction {
    /// 浏览待领单。
    Browse,
    /// 认领一单（编号 `NB-15` 或数字 id 字符串均收）。
    Claim { issue: String },
}

/// args JSON → 结构化动作（缺字段 / 词表外 action 一律诚实报错——
/// args_validator 兜的是 schema 层，语义层这里自己守）。
pub(crate) fn parse_claim_args(args: &str) -> Result<ClaimAction, String> {
    let v: serde_json::Value =
        serde_json::from_str(args).map_err(|e| format!("invalid JSON args: {e}"))?;
    let action = v
        .get("action")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_lowercase();
    match action.as_str() {
        "browse" => Ok(ClaimAction::Browse),
        "claim" => {
            let issue = v
                .get("issue")
                .and_then(|x| x.as_str())
                .map(str::trim)
                .unwrap_or("")
                .to_string();
            if issue.is_empty() {
                return Err(
                    "claim requires \"issue\" (issue number like \"NB-15\", or numeric id)"
                        .to_string(),
                );
            }
            Ok(ClaimAction::Claim { issue })
        }
        other => Err(format!(
            "action must be \"browse\" or \"claim\", got \"{other}\""
        )),
    }
}

/// `board.claimable.list` 上行信封（本文件单消费者，不进共用件）。
fn build_claimable_list_envelope(limit: i64) -> serde_json::Value {
    EnvelopeResponse::success(
        &Envelope {
            ns: "board".to_string(),
            op: "claimable.list".to_string(),
            corr_id: uuid::Uuid::new_v4().to_string(),
            ..Envelope::default()
        },
        serde_json::json!({ "limit": limit }),
    )
    .to_json()
}

/// `board.claim` 上行信封（body 与 master 侧 handle_claim 消费字段同形）。
fn build_claim_envelope(issue: &str) -> serde_json::Value {
    EnvelopeResponse::success(
        &Envelope {
            ns: "board".to_string(),
            op: "claim".to_string(),
            corr_id: uuid::Uuid::new_v4().to_string(),
            ..Envelope::default()
        },
        serde_json::json!({ "issue": issue }),
    )
    .to_json()
}

#[async_trait::async_trait]
impl nemesis_agent::r#loop::Tool for BoardClaimTool {
    fn description(&self) -> String {
        "Browse claimable board issues on the coordinator node and claim one for \
         yourself (first-come-first-served). Use action=browse to list unassigned, \
         dependency-clear issues that anyone on the team may pick up (each entry is \
         marked eligible/blocked for THIS node against the issue's role/profession/tier \
         requirements). Use action=claim with an issue number to claim it: the \
         coordinator atomically assigns it to this node and dispatches it into this \
         node's work queue. A rejected claim means someone else got it or this node \
         does not meet the requirements — the error says which."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["browse", "claim"],
                    "description": "browse = list claimable issues (with eligibility \
                                    marks); claim = take one for this node."
                },
                "issue": {
                    "type": "string",
                    "description": "Issue to claim (claim only): issue number like \
                                    \"NB-15\", or numeric issue id."
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let self_node_id = self.cluster.node_id().to_string();
        let target = find_online_coordinator(&self.cluster, &self_node_id)?;
        let rpc = self
            .cluster
            .rpc_client_arc()
            .ok_or_else(|| "rpc client unavailable (cluster not started)".to_string())?;

        match parse_claim_args(args)? {
            ClaimAction::Browse => {
                let payload = build_claimable_list_envelope(50);
                let body = send_nb_bus(&rpc, &self_node_id, &target, payload).await?;
                let ok = body.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
                if ok {
                    Ok(format_claimable_list(&body))
                } else {
                    let code = body
                        .pointer("/error/code")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    let msg = body
                        .pointer("/error/message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    Err(format!("coordinator rejected the browse: {code} ({msg})"))
                }
            }
            ClaimAction::Claim { issue } => {
                let payload = build_claim_envelope(&issue);
                let body = send_nb_bus(&rpc, &self_node_id, &target, payload).await?;
                let ok = body.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
                if ok {
                    // 成功文案必须挡住 LLM 的两个典型误解：①任务不会立刻
                    // 出现在本工具的返回里——它经工作队列异步到达；②无需
                    // 重复动作（再 claim / 再发消息催 / 另起炉灶重做）。
                    let number = body
                        .pointer("/body/number")
                        .and_then(|v| v.as_str())
                        .unwrap_or(&issue);
                    let title = body
                        .pointer("/body/title")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    Ok(format!(
                        "Claim accepted: {number} \"{title}\" is now assigned to this \
                         node and has been dispatched into this node's work queue. The \
                         task will arrive via the work queue shortly — no further \
                         action needed; do NOT repeat the claim or start the work \
                         manually."
                    ))
                } else {
                    let code = body
                        .pointer("/error/code")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    let msg = body
                        .pointer("/error/message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    Err(format!("claim rejected: {code} ({msg})"))
                }
            }
        }
    }
}

/// browse 臂响应 → LLM 可读文本（纯函数便于单测）。空列表 = 诚实声明
/// （不是错误——「当前无待领单」是正常态）。
///
/// 入参 `body` 是 nb_bus RPC 的完整信封响应（`EnvelopeResponse` 序列化形
/// `{"ok":..,"error":..,"body":{...}}`）——items 在 `/body/items`，与
/// board_discuss 读 `/body/messages` 同口径（顶层读恒空，run4 实录）。
pub(crate) fn format_claimable_list(body: &serde_json::Value) -> String {
    let items = body
        .pointer("/body/items")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    if items.is_empty() {
        return "No claimable issues right now (nothing unassigned in backlog/todo \
                that is dependency-clear and dispatch-idle)."
            .to_string();
    }
    let mut out = format!("Claimable issues ({}):\n", items.len());
    for it in &items {
        let number = it.get("number").and_then(|v| v.as_str()).unwrap_or("?");
        let title = it.get("title").and_then(|v| v.as_str()).unwrap_or("");
        let priority = it.get("priority").and_then(|v| v.as_i64()).unwrap_or(0);
        let status = it.get("status").and_then(|v| v.as_str()).unwrap_or("?");
        let prof = it
            .get("required_profession")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("-");
        let role = it
            .get("required_role")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("-");
        let eligible = it
            .get("eligible")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let marker = if eligible { "ELIGIBLE" } else { "BLOCKED" };
        out.push_str(&format!(
            "- {number} [{marker}] (p{priority}, {status}, role: {role}, profession: \
             {prof}) {title}\n"
        ));
        if !eligible
            && let Some(reason) = it.get("blocked_reason").and_then(|v| v.as_str())
            && !reason.is_empty()
        {
            out.push_str(&format!("    blocked: {reason}\n"));
        }
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests;
