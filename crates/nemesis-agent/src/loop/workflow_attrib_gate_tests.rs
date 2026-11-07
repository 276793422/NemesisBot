// workflow 工具节点 dispatch 入口安全面回归锁（A 批，PLAN 回归锁 7/8/9）。
//
// `handle_tool_call_with_attrib` 携 workflow 触发身份（channel="workflow" +
// 触发会话三键）进 dispatch 瀑布；本文件钉住审批/审计归因的**可观测面**：
//   ① 锁 9（保持弹卡拍板）：RequireApproval → 审批卡请求产生且带 workflow
//      触发身份（管线层 3 路径）；
//   ② 锁 7（归因穿透）：guardian 语义二审的升级审批同样路由回触发会话
//      （guardian_gate 臂）——两条审批升级通路（管线 + guardian）都经
//      `ApprovalManager::request_approval_sync_ctx` 携 `ApprovalContext`；
//   ③ 锁 8：guardian verdict=deny → 节点失败（结构化拒绝文本，工具不执行）。
//
// 诚实边界：`wf_*` attrib 键只进 `ToolInvocation.metadata`，`ApprovalContext`
// 不携带（其字段止步 approval_* 四键 + tool_args），`AuditEvent` 无 metadata
// 字段——wf_* 键的直接断言面在审批卡渲染侧（gateway 消费），此处能钉的是
// channel="workflow" + 触发会话三键这两条审计归因通路。
//
// fixture 自带（兄弟测试模块不能互相 import）。安全层涉及
// `tokio::task::block_in_place`，一律 `multi_thread` flavor。

#![cfg(feature = "security")]

use super::*;
use crate::test_support::capture_logs;

// ---------------------------------------------------------------------------
// 迷你 fixture
// ---------------------------------------------------------------------------

struct WfAttribProvider;

#[async_trait]
impl LlmProvider for WfAttribProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Ok(LlmResponse {
            finish_reason: None,
            escalated_to: None,
            content: "ok".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

/// 固定 Ok 文本的工具。
struct WfExecTool(&'static str);

#[async_trait]
impl Tool for WfExecTool {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok(self.0.to_string())
    }
}

/// workflow 形态触发身份（与 gateway WorkflowSecureExecutor 构造同构）。
fn wf_attrib_ctx() -> RequestContext {
    RequestContext::new(
        "workflow",
        "chat-wf",
        "user-wf",
        "agent:main:session:wf-attrib",
    )
}

/// workflow 归因 attrib 键（gateway 侧五键的最小复刻）。
fn wf_attrib() -> Vec<(String, String)> {
    vec![
        ("wf_instance".to_string(), "exec-1".to_string()),
        ("wf_name".to_string(), "demo-flow".to_string()),
        ("wf_node_id".to_string(), "node-3".to_string()),
        ("wf_trigger".to_string(), "agent_tool".to_string()),
        ("wf_depth".to_string(), "0".to_string()),
    ]
}

fn wf_attrib_loop() -> AgentLoop {
    AgentLoop::new(
        Box::new(WfAttribProvider),
        AgentConfig {
            model: "test-model".to_string(),
            system_prompt: None,
            max_turns: 5,
            tools: vec![],
            models: std::collections::HashMap::new(),
            tool_fence_profession: None,
        },
    )
}

/// 全关规则层 + 指定 default_action 的插件（审批是唯一活动闸）。
fn wf_plugin(default_action: &str) -> Arc<nemesis_security::pipeline::SecurityPlugin> {
    Arc::new(nemesis_security::pipeline::SecurityPlugin::new(
        nemesis_security::pipeline::SecurityPluginConfig {
            enabled: true,
            command_guard_enabled: false,
            injection_enabled: false,
            credential_enabled: false,
            dlp_enabled: false,
            ssrf_enabled: false,
            default_action: default_action.to_string(),
            ..Default::default()
        },
    ))
}

// ---------------------------------------------------------------------------
// 审批管理器假件：捕获 ApprovalContext + 按构造裁决
// ---------------------------------------------------------------------------

#[cfg(feature = "security")]
enum WfApproveBehavior {
    Approve,
    Reject,
}

/// override `request_approval_sync_ctx` 的审批管理器假件——捕获管线上行的
/// `ApprovalContext`（锁 7/9 的断言面），裁决按构造而定。
#[cfg(feature = "security")]
struct WfCapturingApprovalManager {
    behavior: WfApproveBehavior,
    captured: std::sync::Arc<std::sync::Mutex<Option<nemesis_security::auditor::ApprovalContext>>>,
}

#[cfg(feature = "security")]
impl WfCapturingApprovalManager {
    fn new(
        behavior: WfApproveBehavior,
    ) -> (
        Self,
        std::sync::Arc<std::sync::Mutex<Option<nemesis_security::auditor::ApprovalContext>>>,
    ) {
        let captured = std::sync::Arc::new(std::sync::Mutex::new(None));
        (
            Self {
                behavior,
                captured: captured.clone(),
            },
            captured,
        )
    }
}

#[cfg(feature = "security")]
impl nemesis_security::auditor::ApprovalManager for WfCapturingApprovalManager {
    fn is_running(&self) -> bool {
        true
    }
    fn request_approval_sync(
        &self,
        _request_id: &str,
        _operation: &str,
        _target: &str,
        _risk_level: &str,
        _reason: &str,
        _timeout_secs: u64,
    ) -> Result<nemesis_security::auditor::ApprovalVerdict, String> {
        // 旧方法兜底（ctx 缺席的旧链路）：本文件所有路径都带 ctx，
        // 命中这里即断言目标落空。
        match self.behavior {
            WfApproveBehavior::Approve => {
                Ok(nemesis_security::auditor::ApprovalVerdict::approved())
            }
            WfApproveBehavior::Reject => Ok(nemesis_security::auditor::ApprovalVerdict {
                approved: false,
                note: Some("wf-fallback-note".to_string()),
            }),
        }
    }
    fn request_approval_sync_ctx(
        &self,
        _request_id: &str,
        _operation: &str,
        _target: &str,
        _risk_level: &str,
        _reason: &str,
        _timeout_secs: u64,
        ctx: &nemesis_security::auditor::ApprovalContext,
    ) -> Result<nemesis_security::auditor::ApprovalVerdict, String> {
        *self.captured.lock().unwrap() = Some(ctx.clone());
        match self.behavior {
            WfApproveBehavior::Approve => {
                Ok(nemesis_security::auditor::ApprovalVerdict::approved())
            }
            WfApproveBehavior::Reject => Ok(nemesis_security::auditor::ApprovalVerdict {
                approved: false,
                note: Some("wf-note".to_string()),
            }),
        }
    }
}

/// 固定 flag 裁决的 guardian judge 假件。
#[cfg(feature = "security")]
struct WfFlagJudge;

#[cfg(feature = "security")]
#[async_trait]
impl nemesis_security::guardian::LlmJudge for WfFlagJudge {
    async fn judge(
        &self,
        _req: &nemesis_security::guardian::JudgeRequest,
    ) -> Result<nemesis_security::guardian::JudgeVerdict, String> {
        Ok(nemesis_security::guardian::JudgeVerdict {
            intent: "wipes the disk".to_string(),
            matches_rules: true,
            risk_level: "critical".to_string(),
            recommendation: "deny".to_string(),
            rationale: "wf-attrib: destructive without authorization".to_string(),
        })
    }
}

// ---------------------------------------------------------------------------
// ① 锁 9 + 锁 7 审批面：RequireApproval → 卡请求带 workflow 触发身份
// ---------------------------------------------------------------------------

/// workflow 路径 RequireApproval（保持弹卡拍板）：审批卡请求**产生**且
/// `ApprovalContext` 携 workflow 触发身份（channel + 会话三键）；批准后
/// 工具放行执行。工具名未注册操作类型 → fail-closed CRITICAL 过管线
/// （`effective_tool_operation` 同口径），default_action="ask" 产
/// RequireApproval。
#[cfg(feature = "security")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workflow_approval_card_routes_to_trigger_session() {
    let _logs = capture_logs();
    let plugin = wf_plugin("ask");
    let (manager, captured) = WfCapturingApprovalManager::new(WfApproveBehavior::Approve);
    plugin.auditor().set_approval_manager(Arc::new(manager));

    let mut agent_loop = wf_attrib_loop();
    agent_loop.set_security_plugin(plugin);
    agent_loop.register_tool(
        "wf_report_tool".to_string(),
        Box::new(WfExecTool("executed_ok")),
    );

    let out = agent_loop
        .handle_tool_call_with_attrib(
            &ToolCallInfo {
                id: "wf-attrib-1".to_string(),
                name: "wf_report_tool".to_string(),
                arguments: r#"{"path":"report.md"}"#.to_string(),
            },
            &wf_attrib_ctx(),
            0,
            &wf_attrib(),
        )
        .await;
    assert_eq!(
        out, "executed_ok",
        "审批批准后必须放行执行（卡已产生已路由）: {out}"
    );

    let captured = captured.lock().unwrap().clone();
    let ctx = captured.expect("审批卡请求必须携带 ApprovalContext（锁 7 归因面）");
    assert_eq!(
        ctx.channel, "workflow",
        "审批来源通道必须是 workflow（审计 source 可辨触发源头）"
    );
    assert_eq!(ctx.chat_id, "chat-wf", "审批卡必须路由回触发 chat");
    assert_eq!(ctx.sender_id, "user-wf", "发起人身份必须随行");
    assert_eq!(
        ctx.session_key, "agent:main:session:wf-attrib",
        "会话键必须随行（重启补裁回灌靠它路由回原会话）"
    );
    assert!(
        ctx.batch_id.is_empty(),
        "workflow 节点无并行批，batch_id 必须为空"
    );
}

// ---------------------------------------------------------------------------
// ② 锁 7 guardian 臂 + 锁 8：verdict=deny → 节点失败（工具不执行）
// ---------------------------------------------------------------------------

/// guardian critical 模式下 workflow 路径 CRITICAL 工具进语义二审；升级审批
/// 同样携 workflow 触发身份（guardian_gate 臂的 ctx 透传）；裁决=deny →
/// 结构化拒绝文本（gateway 侧 tool_result_indicates_error 映射失败态进
/// NodeResult），工具体不执行。
#[cfg(feature = "security")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workflow_guardian_deny_blocks_node_and_escalation_carries_identity() {
    let _logs = capture_logs();
    let plugin = wf_plugin("allow");
    plugin.set_guardian_mode("critical");
    plugin.set_judge(Arc::new(WfFlagJudge));
    let (manager, captured) = WfCapturingApprovalManager::new(WfApproveBehavior::Reject);
    plugin.auditor().set_approval_manager(Arc::new(manager));

    let mut agent_loop = wf_attrib_loop();
    agent_loop.set_security_plugin(plugin);
    agent_loop.register_tool(
        "wf_deploy_tool".to_string(),
        Box::new(WfExecTool("should_not_run")),
    );

    let out = agent_loop
        .handle_tool_call_with_attrib(
            &ToolCallInfo {
                id: "wf-attrib-2".to_string(),
                name: "wf_deploy_tool".to_string(),
                arguments: r#"{"target":"prod"}"#.to_string(),
            },
            &wf_attrib_ctx(),
            0,
            &wf_attrib(),
        )
        .await;
    assert!(
        out.contains("GUARDIAN FLAGGED — USER REJECTED"),
        "guardian deny 后必须回灌结构化拒绝（节点失败态）: {out}"
    );
    assert!(out.contains(": wf-note"), "用户备注必须拼进文案: {out}");
    assert!(out.contains("Do NOT retry"), "必须明示不要原样重试: {out}");
    assert!(
        !out.contains("should_not_run"),
        "拒绝态下工具体绝不能执行: {out}"
    );

    // 锁 7 guardian 臂：升级审批同样路由回 workflow 触发会话。
    let captured = captured.lock().unwrap().clone();
    let ctx = captured.expect("guardian 升级审批必须携带 ApprovalContext");
    assert_eq!(
        ctx.channel, "workflow",
        "guardian 升级审批的通道必须是 workflow"
    );
    assert_eq!(
        ctx.chat_id, "chat-wf",
        "guardian 升级审批必须路由回触发 chat"
    );
    assert_eq!(
        ctx.session_key, "agent:main:session:wf-attrib",
        "guardian 升级审批必须携带触发会话键"
    );
}
