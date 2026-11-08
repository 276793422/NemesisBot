//! workflow `tool` 节点安全执行通道（A 批五闸补齐，gateway 侧实现）。
//!
//! workflow 引擎的 tool 节点默认直连裸 `ToolRegistry`（只有 AgentToolAdapter
//! 的安全 8 层管线+审批），dispatch 层五闸（estop/hidden/tier/plan 写闸/
//! args_validator）与 guardian 不在路径上。本模块把工具调用转交 AgentLoop
//! dispatch 瀑布（[`nemesis_agent::r#loop::AgentLoop::handle_tool_call_with_attrib`]），
//! 闸序单一真相源零重实现；workflow 实例/节点身份经 attrib 键 append 进
//! `ToolInvocation.metadata`（审计链与审批卡可辨触发源头）。

use std::sync::Arc;

use nemesis_agent::context::RequestContext;
use nemesis_workflow::nodes::SecureExecutor;
use tracing::warn;

/// [`SecureExecutor`] 的 gateway 实现：持 gateway 级动态解析槽（与
/// [`crate::plugin_bridge::GatewayHostInvoker`] 同域的「agent 重启后解析到
/// 最新 loop」纪律；BUG-20261108-012：此前 `Weak<AgentLoop>` 被
/// [`crate::commands::gateway::bridges::GatewayAgentRunner`] 等强引用保活，
/// Weak 恒 upgrade 成功——agent stop→start 后仍打死旧 loop，旧 persona
/// system prompt 缓存照常服务）。槽空 = 停机窗口，诚实报错绝不回退裸
/// 注册表——工具不幂等，静默双执行。
pub(crate) struct WorkflowSecureExecutor {
    loop_ref: Arc<parking_lot::RwLock<Option<Arc<nemesis_agent::r#loop::AgentLoop>>>>,
}

impl WorkflowSecureExecutor {
    pub(crate) fn new(
        loop_ref: Arc<parking_lot::RwLock<Option<Arc<nemesis_agent::r#loop::AgentLoop>>>>,
    ) -> Self {
        Self { loop_ref }
    }
}

#[async_trait::async_trait]
impl SecureExecutor for WorkflowSecureExecutor {
    async fn execute(
        &self,
        node_id: &str,
        tool_name: &str,
        args: &serde_json::Value,
        identity: &nemesis_workflow::WorkflowIdentity,
    ) -> nemesis_tools::types::ToolResult {
        let Some(loop_arc) = self.loop_ref.read().clone() else {
            warn!(
                "[workflow-secure] tool node '{}' -> '{}' dropped: agent loop \
                 unavailable (shutting down?)",
                node_id, tool_name
            );
            return nemesis_tools::types::ToolResult::error(
                "agent loop unavailable (shutting down?)",
            );
        };
        let arguments = match serde_json::to_string(args) {
            Ok(s) => s,
            Err(e) => {
                return nemesis_tools::types::ToolResult::error(&format!(
                    "failed to serialize tool args: {}",
                    e
                ));
            }
        };
        let call = nemesis_agent::types::ToolCallInfo {
            id: uuid::Uuid::new_v4().to_string(),
            name: tool_name.to_string(),
            arguments,
        };
        // 审批卡路由回触发会话（chat_id/sender_id/session_key 三键随行），
        // 会话级全部放行豁免按 session_key 生效；channel="workflow" 使
        // 审计 source 可辨（来源是 workflow 引擎而非聊天通道）。
        let ctx = RequestContext::new(
            "workflow",
            &identity.chat_id,
            &identity.sender_id,
            &identity.session_key,
        );
        // A4 归因穿透：workflow 实例/节点身份 append 进 metadata。
        let attrib = vec![
            ("wf_instance".to_string(), identity.execution_id.clone()),
            ("wf_name".to_string(), identity.workflow_name.clone()),
            ("wf_node_id".to_string(), node_id.to_string()),
            ("wf_trigger".to_string(), identity.trigger_label.clone()),
            ("wf_depth".to_string(), identity.recursion_depth.to_string()),
        ];
        let result = loop_arc
            .handle_tool_call_with_attrib(&call, &ctx, 0, &attrib)
            .await;
        if nemesis_agent::turn_guard::tool_result_indicates_error(&result) {
            nemesis_tools::types::ToolResult::error(&result)
        } else {
            nemesis_tools::types::ToolResult::success(&result)
        }
    }
}

/// workflow 引擎 estop 联动 watcher（A 批批 4）：急停触发 → 引擎全部
/// Running/Waiting 执行取消（cancel token fire → 调度器取消路径收口，
/// 落 Cancelled 终态）。**释放不动作**——被取消的执行不自动复活（诚实
/// 终态：显式 resume 才续，与 PTY kill / 插件冻结同款纪律）。watcher
/// 断 = gateway 停机路径（estop watch sender 全 drop）。
pub(crate) fn spawn_estop_watcher(
    engine: &std::sync::Arc<nemesis_workflow::engine::WorkflowEngine>,
    estop: &std::sync::Arc<nemesis_agent::estop::EstopState>,
) {
    let engine = engine.clone();
    let estop = estop.clone();
    tokio::spawn(async move {
        let mut rx = estop.subscribe();
        while rx.changed().await.is_ok() {
            if *rx.borrow() {
                let cancelled = engine.cancel_all_running().await;
                if !cancelled.is_empty() {
                    tracing::info!(
                        "[Gateway] estop: cancelled {} workflow execution(s): {:?}",
                        cancelled.len(),
                        cancelled
                    );
                }
            }
            // 释放臂：无动作（取消是诚实终态，不自动恢复）。
        }
    });
}
