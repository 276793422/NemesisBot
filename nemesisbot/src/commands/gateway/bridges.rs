// ---------------------------------------------------------------------------
// Gateway 桥接/适配器件（审批闸、guardian 二审、弹窗/集群/workflow 桥）
// ---------------------------------------------------------------------------

// Arc 消费者：GatewayMemoryGate / GatewayLlmJudge / ApprovalPopupAdapter
// （security 家族门）、ClusterResultPersisterAdapter / BusToClusterAdapter
// （cluster 门）、GatewayAgentRunner（workflow 门）——随门收放。
#[cfg(any(feature = "cluster", feature = "security", feature = "workflow"))]
use std::sync::Arc;
// 裸 info!/warn! 仅 ApprovalPopupAdapter（desktop+security 门）消费。
#[cfg(all(feature = "desktop", feature = "security"))]
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// Plugin window management via ProcessManager
// ---------------------------------------------------------------------------

/// Check if plugin-ui library exists in the `plugins/` directory next to the executable.
///
/// M7（devtool-upgrade 阶段 5）：审批弹窗被 WebApprovalManager（Dashboard 审批卡）
/// 同构替换，本函数仅剩 ApprovalPopupAdapter（同样停用）消费——非测试构建下
/// dead。恢复弹窗：gateway 装配块里把 `web_mgr` 换回
/// `Arc::new(ApprovalPopupAdapter::new(process_manager.clone()))` 即可。
#[cfg(all(feature = "desktop", feature = "security"))]
#[allow(dead_code)]
pub(crate) fn plugin_ui_library_exists() -> bool {
    nemesis_utils::find_plugin_library("plugin_ui").is_some()
}

/// Adapter bridging the agent's memory write/forget gate (P2) to the interactive
/// approval popup. When attached, agent `memory_store`/`memory_forget` calls pop
/// up an approval dialog; approval is never bypassed by YOLO/auto. Denies on
/// timeout/error so a memory write never silently succeeds unapproved.
#[cfg(all(feature = "desktop", feature = "memory", feature = "security"))]
pub(crate) struct GatewayMemoryGate {
    approval: Arc<dyn nemesis_security::auditor::ApprovalManager>,
}

#[cfg(all(feature = "desktop", feature = "memory", feature = "security"))]
impl GatewayMemoryGate {
    // pub(crate)（ASM-02）：cluster loop 装配点（agent_factory）复用同一闸
    // ——审批闸此前只接主 loop，集群 loop 的 memory_store/forget 缺省放行。
    pub(crate) fn new(approval: Arc<dyn nemesis_security::auditor::ApprovalManager>) -> Self {
        Self { approval }
    }

    /// Request interactive approval. Returns true only if explicitly approved.
    /// Runs the blocking `request_approval_sync` on a spawn_blocking thread (it
    /// spawns a popup child process and waits for the user).
    async fn request(&self, operation: &str, preview: &str) -> bool {
        let am = self.approval.clone();
        let operation = operation.to_string();
        let preview = preview.to_string();
        let req_id = uuid::Uuid::new_v4().to_string();
        match tokio::task::spawn_blocking(move || {
            am.request_approval_sync(&req_id, &operation, "memory", "MEDIUM", &preview, 30)
        })
        .await
        {
            Ok(Ok(v)) => v.approved,
            _ => false, // denied / expired / errored → treat as not approved
        }
    }
}

#[cfg(all(feature = "desktop", feature = "memory", feature = "security"))]
#[async_trait::async_trait]
impl nemesis_memory::memory_tools::MemoryApprovalGate for GatewayMemoryGate {
    async fn approve_store(&self, preview: &str) -> bool {
        self.request("memory_store", preview).await
    }
    async fn approve_forget(&self, preview: &str) -> bool {
        self.request("memory_forget", preview).await
    }
}

/// LLM safety judge (guardian) backed by a gateway-owned LLM provider. One
/// stateless `chat()` call per audit — 独立提示词点，绝不进 agent 流程（无
/// session、无历史、无工具）。请求构造 = 固定审计宪法 system prompt（编译期
/// 常量，条条相同 → prompt cache 全命中）+ 仅命令本体的 user message（
/// `<command>` 分隔符包裹，零任务信息零历史——无上下文是宪法，见
/// nemesis-security::guardian 模块文档）。
/// 覆盖由 `guardian_mode` 闸控制（默认 off 不装配）；模型走
/// `agents.small_model` 杂务通道（未配置回落主模型 + warn）。
#[cfg(feature = "security")]
pub(crate) struct GatewayLlmJudge {
    pub(crate) provider: Arc<dyn nemesis_providers::router::LLMProvider>,
    pub(crate) model: String,
}

#[cfg(feature = "security")]
#[async_trait::async_trait]
impl nemesis_security::guardian::LlmJudge for GatewayLlmJudge {
    async fn judge(
        &self,
        req: &nemesis_security::guardian::JudgeRequest,
    ) -> Result<nemesis_security::guardian::JudgeVerdict, String> {
        // 工具名 + 管线危级是命令的元数据（非任务上下文），帮助 judge 理解
        // 它在审什么形态（delete_file 的 args 是路径，exec 的 args 是命令）。
        let user = format!(
            "Tool: {}\nPipeline danger class: {}\n\n<command>\n{}\n</command>\n\nAudit the command. Output the JSON verdict now.",
            req.action, req.risk_level, req.command
        );
        let messages = vec![
            nemesis_providers::types::Message {
                role: "system".into(),
                content: nemesis_security::guardian::GUARDIAN_PROMPT
                    .to_string()
                    .into(),
                tool_calls: vec![],
                tool_call_id: None,
                timestamp: None,
                reasoning_content: None,
                extra: std::collections::HashMap::new(),
            },
            nemesis_providers::types::Message {
                role: "user".into(),
                content: user.into(),
                tool_calls: vec![],
                tool_call_id: None,
                timestamp: None,
                reasoning_content: None,
                extra: std::collections::HashMap::new(),
            },
        ];
        let opts = nemesis_providers::types::ChatOptions {
            temperature: Some(0.0),
            max_tokens: Some(256),
            top_p: None,
            stop: None,
            reasoning_effort: None,
            extra: std::collections::HashMap::new(),
        };
        let resp = self
            .provider
            .chat(&messages, &[], &self.model, &opts)
            .await
            .map_err(|e| format!("guardian LLM call failed: {}", e))?;
        nemesis_security::guardian::parse_verdict(&resp.content)
    }
}

/// Adapter connecting ProcessManager to the security auditor's ApprovalManager trait.
///
/// When a tool call triggers an "ask" security rule, the auditor calls
/// `request_approval_sync()` which spawns an approval popup child process
/// via ProcessManager and blocks until the user responds.
///
/// **M7（devtool-upgrade 阶段 5）已停用**：审批交互同构替换为
/// `crate::web_approval::WebApprovalManager`（Dashboard 审批卡，SSE +
/// WSAPI approval.respond），全平台可用。恢复方法：gateway 装配块（"Wire up
/// ApprovalManager" 注释处）把 web_mgr 换回
/// `Arc::new(ApprovalPopupAdapter::new(process_manager.clone()))`；本结构体
/// 与 `plugin_ui_library_exists` 一并恢复使用。测试（gateway/tests.rs、
/// r9_live_tests.rs）仍直接构造它，保留编译。
#[cfg(all(feature = "desktop", feature = "security"))]
#[allow(dead_code)]
pub(crate) struct ApprovalPopupAdapter {
    process_manager: Arc<nemesis_desktop::process::ProcessManager>,
}

#[cfg(all(feature = "desktop", feature = "security"))]
#[allow(dead_code)]
impl ApprovalPopupAdapter {
    pub(crate) fn new(pm: Arc<nemesis_desktop::process::ProcessManager>) -> Self {
        Self {
            process_manager: pm,
        }
    }
}

#[cfg(all(feature = "desktop", feature = "security"))]
impl nemesis_security::auditor::ApprovalManager for ApprovalPopupAdapter {
    fn is_running(&self) -> bool {
        true
    }

    fn request_approval_sync(
        &self,
        request_id: &str,
        operation: &str,
        target: &str,
        risk_level: &str,
        reason: &str,
        timeout_secs: u64,
    ) -> Result<nemesis_security::auditor::ApprovalVerdict, String> {
        use nemesis_security::auditor::ApprovalVerdict;
        // Check if plugin-ui library exists. If not, reject immediately —
        // we cannot show an approval popup without the UI plugin, and
        // allowing the operation without user confirmation is unsafe.
        if !plugin_ui_library_exists() {
            let label = nemesis_utils::plugin_library_label();
            warn!(
                "[Gateway] Approval rejected: plugin-ui {} not found (operation={}, target={}, risk={}). \
                 Cannot show approval popup — denying by default.",
                label, operation, target, risk_level
            );
            return Ok(ApprovalVerdict::denied());
        }

        let data = serde_json::json!({
            "request_id": request_id,
            "operation": operation,
            "operation_name": operation,
            "target": target,
            "risk_level": risk_level,
            "reason": reason,
            "timeout_seconds": timeout_secs.max(30),
            "context": {},
            "timestamp": chrono::Local::now().timestamp(),
        });

        info!(
            "[Gateway] Requesting approval popup: operation={}, target={}, risk={}",
            operation, target, risk_level
        );

        let (_child_id, result_rx) = self
            .process_manager
            .spawn_child("approval", &data)
            .map_err(|e| format!("spawn_child failed: {}", e))?;

        let result_rx = result_rx.ok_or("no result channel")?;

        // The oneshot receiver is async but we're in a sync context.
        // Use a dedicated thread with its own tokio runtime to wait for the result.
        let (tx, rx) = std::sync::mpsc::channel::<Result<serde_json::Value, String>>();
        let wait_secs = timeout_secs + 10;
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            let result = rt.block_on(async {
                tokio::time::timeout(std::time::Duration::from_secs(wait_secs), result_rx).await
            });
            match result {
                Ok(Ok(value)) => {
                    let _ = tx.send(Ok(value));
                }
                Ok(Err(_)) => {
                    let _ = tx.send(Err("channel closed".to_string()));
                }
                Err(_) => {
                    let _ = tx.send(Err("timeout".to_string()));
                }
            }
        });

        // Block until the user responds or timeout
        match rx.recv_timeout(std::time::Duration::from_secs(timeout_secs + 15)) {
            Ok(Ok(value)) => {
                let action = value
                    .get("action")
                    .and_then(|v| v.as_str())
                    .unwrap_or("rejected");
                info!(
                    "[Gateway] Approval result: action={} for request_id={}",
                    action, request_id
                );
                if action == "approved" {
                    Ok(ApprovalVerdict::approved())
                } else {
                    Ok(ApprovalVerdict::denied())
                }
            }
            Ok(Err(e)) => {
                warn!("[Gateway] Approval channel error: {}", e);
                Ok(ApprovalVerdict::denied())
            }
            Err(_) => {
                warn!("[Gateway] Approval timeout after {}s", timeout_secs);
                Ok(ApprovalVerdict::denied()) // timeout = rejected
            }
        }
    }
}

/// Bridge adapter connecting Cluster to Forge's ClusterForgeBridge trait.
///
/// Enables Forge to share reflections with and receive reflections from
/// cluster peers. Mirrors Go's `forge.NewClusterForgeBridge(cluster)`.
#[cfg(all(feature = "cluster", feature = "forge"))]
pub(crate) struct ClusterForgeBridgeAdapter {
    node_id: String,
}

#[cfg(all(feature = "cluster", feature = "forge"))]
impl ClusterForgeBridgeAdapter {
    pub(crate) fn new(node_id: String) -> Self {
        Self { node_id }
    }
}

#[cfg(all(feature = "cluster", feature = "forge"))]
#[async_trait::async_trait]
impl nemesis_forge::bridge::ClusterForgeBridge for ClusterForgeBridgeAdapter {
    async fn share_reflection(&self, report_json: serde_json::Value) -> Result<usize, String> {
        // TODO: When cluster has a share_reflection method, call it here.
        // For now, store locally only (matches Go's early implementation).
        let _ = report_json;
        Ok(0)
    }

    async fn get_remote_reflections(&self) -> Result<Vec<serde_json::Value>, String> {
        // TODO: When cluster has get_reflection_reports, call it here.
        Ok(Vec::new())
    }

    async fn get_online_peers(&self) -> Result<Vec<String>, String> {
        // TODO: When cluster has get_online_peers with node IDs, call it here.
        Ok(Vec::new())
    }

    fn local_node_id(&self) -> &str {
        &self.node_id
    }

    fn is_cluster_enabled(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// Cluster adapter types
// ---------------------------------------------------------------------------

/// Adapter: Cluster result store → TaskResultPersister trait.
///
// Bridges the cluster's TaskResultStore to PeerChatHandler's
/// TaskResultPersister interface.
#[cfg(feature = "cluster")]
pub(crate) struct ClusterResultPersisterAdapter {
    pub(crate) result_store: Arc<nemesis_cluster::task_result_store::TaskResultStore>,
    pub(crate) node_id: String,
    /// P3/D2（看板项目档案 goal）：执行档案发件箱。board feature 形态才
    /// 装配；None = 终态钩子 no-op（on_task_terminal 默认实现兜底）。
    pub(crate) outbox: Option<Arc<nemesis_cluster::outbox::TransferOutbox>>,
    /// P4/E3 worker 侧档案工作副本根（`<workspace>`；变更集组装定位 exec
    /// 目录用）。outbox 未装配时永不消费。
    pub(crate) workspace: Option<std::path::PathBuf>,
}

#[cfg(feature = "cluster")]
impl nemesis_cluster::rpc::peer_chat_handler::TaskResultPersister
    for ClusterResultPersisterAdapter
{
    fn set_running(&self, task_id: &str, _source_node: &str) {
        // Mark as running with a placeholder result
        self.result_store.store_success(
            task_id,
            "peer_chat",
            serde_json::json!({
                "status": "running",
                "from": self.node_id,
            }),
        );
    }

    fn set_result(
        &self,
        task_id: &str,
        status: &str,
        response: &str,
        error: &str,
        _source_node: &str,
    ) -> Result<(), String> {
        if status == "error" {
            self.result_store.store_failure(task_id, "peer_chat", error);
        } else {
            // G5 键归一（2026-09-01）：恢复轮询的 query_task_result handler
            // 读 "response"（与 peer_chat_callback 信封同键）；旧值 "content"
            // 曾导致查询恢复拿到空回复。
            self.result_store.store_success(
                task_id,
                "peer_chat",
                serde_json::json!({
                    "response": response,
                    "from": self.node_id,
                }),
            );
        }
        Ok(())
    }

    fn delete(&self, task_id: &str) -> Result<(), String> {
        // 2026-09-08 修复：旧实现是 no-op（注释谎称 "TaskResultStore doesn't
        // have a delete method"——remove() 明明存在）。后果：回调成功后
        // set_running 占位文件永留 rpc_cache/results/，7 天 TTL 才清扫。
        self.result_store.remove(task_id);
        Ok(())
    }

    /// P3/D2 任务终态钩子：执行档案入发件箱（回调成功与否无关——档案
    /// 推送不依赖 A 端是否收到本轮结果；source 为空时 enqueue 诚实拒绝）。
    /// P4/E3：档案工作副本在场 = 先全树 diff 组装变更集随行入队（保序：
    /// 合并不可能抢在交付前）；入队失败保留现场（启动清扫重试）。
    fn on_task_terminal(&self, task_id: &str, source_node_id: &str) {
        let Some(ob) = &self.outbox else { return };
        if let Some(ws) = self.workspace.as_deref() {
            match nemesis_cluster::exec_workspace::finish_task_exec(ws, ob, task_id, source_node_id)
            {
                Ok(true) => return, // 档案管线已处理（变更集随行或零差异）
                Ok(false) => {}     // 无工作副本/sidecar → 非档案管线
                Err(e) => {
                    tracing::warn!(
                        task_id = %task_id,
                        "[Transfer] 变更集组装/入队失败（现场保留，启动清扫重试）: {e}"
                    );
                    return;
                }
            }
        }
        if let Err(e) = ob.enqueue(task_id, source_node_id) {
            tracing::warn!(task_id = %task_id, "[Transfer] 执行档案入队失败: {e}");
        }
    }
}

/// Adapter: nemesis_bus::MessageBus → Cluster's MessageBus trait.
///
/// Translates Cluster's BusInboundMessage to nemesis_types::InboundMessage
/// and publishes on the real message bus.
#[cfg(feature = "cluster")]
pub(crate) struct BusToClusterAdapter {
    pub(crate) bus: Arc<nemesis_bus::MessageBus>,
}

#[cfg(feature = "cluster")]
impl nemesis_cluster::cluster::MessageBus for BusToClusterAdapter {
    fn publish_inbound(&self, msg: nemesis_cluster::cluster::BusInboundMessage) {
        let inbound = nemesis_types::channel::InboundMessage {
            channel: msg.channel,
            sender_id: msg.sender_id,
            chat_id: msg.chat_id,
            content: msg.content,
            media: vec![],
            session_key: String::new(),
            correlation_id: String::new(),
            // G5: 透传结构化元数据（cluster_continuation 的 status /
            // source_node / error）——AgentLoop 续行拦截依赖这些字段。
            metadata: msg.metadata,
            voice_playback: None,
        };
        self.bus.publish_inbound(inbound);
    }
}

// ---------------------------------------------------------------------------
// GatewayAgentRunner — bridges workflow `agent` nodes to the live AgentLoop
// ---------------------------------------------------------------------------

/// Adapter that lets workflow `agent` nodes drive the gateway's AgentLoop
/// without `nemesis-workflow` needing to depend on `nemesis-agent` (which
/// would pull in a huge transitive dependency closure).
///
/// Session keys are namespaced as `workflow:{agent_id}` so that workflow
/// sessions stay isolated from human user sessions in the SessionStore.
///
/// `tools_used` is currently always empty: the public AgentLoop entry point
/// only returns the final response string. If/when we surface tool-call
/// events, this struct can be extended to capture them without breaking the
/// trait contract.
#[cfg(feature = "workflow")]
pub(crate) struct GatewayAgentRunner {
    agent_loop: Arc<nemesis_agent::r#loop::AgentLoop>,
}

#[cfg(feature = "workflow")]
impl GatewayAgentRunner {
    pub(crate) fn new(agent_loop: Arc<nemesis_agent::r#loop::AgentLoop>) -> Self {
        Self { agent_loop }
    }
}

#[cfg(feature = "workflow")]
#[async_trait::async_trait]
impl nemesis_workflow::nodes::AgentRunner for GatewayAgentRunner {
    async fn run_direct(
        &self,
        prompt: &str,
        agent_id: &str,
        max_turns: u32,
        model: Option<&str>,
    ) -> Result<nemesis_workflow::nodes::AgentRunResult, String> {
        // Note: max_turns is currently applied via AgentLoop's own config at
        // construction time. Once we add a per-call override on AgentLoop
        // (e.g. `process_direct_with_options`), this runner should respect
        // it explicitly to prevent runaway workflow agent loops.
        let _ = max_turns;
        // Same applies to model: AgentLoop currently uses its configured
        // default. Log when a override is requested but cannot be honored so
        // the silent-drop pattern from BUG #8 (form collecting model that
        // backend ignored) stays visible.
        if let Some(m) = model {
            tracing::warn!(
                model = %m,
                agent_id = %agent_id,
                "workflow `agent` node requested model override but AgentLoop does not yet support per-call model switching; using default"
            );
        }

        let session_key = format!("workflow:{}", agent_id);
        let response = self.agent_loop.process_direct(prompt, &session_key).await?;

        Ok(nemesis_workflow::nodes::AgentRunResult {
            response,
            tools_used: Vec::new(),
        })
    }
}
