// workflow 工具节点执行器安全闸回归锁（A 批，PLAN 回归锁 1 + 归因面）。
//
// `RealToolNodeExecutor` 是 tool 节点的两条执行路径单一真相源：
//   ① 锁 1（未接线）：`new()`（secure=None）→ 裸注册表直连路径行为不变
//      （旧路径兼容），一次性 loud warn 提醒宿主接线（warn 是 tracing
//      侧效应，本 crate 无日志捕获基建，行为面锁定；文案由 tool.rs 源码
//      与 host_dll_freshness 同级纪律守护）；
//   ② 锁 1 接线面：`new_with_secure_executor` → 全部调用路由 SecureExecutor
//      （裸注册表不再被触碰——空注册表 + secure mock 也放行，证明确实
//      走的是 secure 通道）；
//   ③ ToolResult→NodeResult 映射：success→Completed（output 五键）、
//      error→Failed（error 带 `tool '<name>' error:` 前缀）；
//   ④ `{{var}}` 模板解析后 args 才进 secure 通道；
//   ⑤ TriggerSource::label()/identity_triple() 全变体表测（A4 归因的
//      身份来源单一真相源）；
//   ⑥ engine e2e：`set_tool_executor` + `run(..., Some(trigger))` →
//      WorkflowIdentity 七字段全量随行（execution_id/workflow_name/
//      trigger_label/会话三元组/recursion_depth）。
//
// fixture 自带（兄弟测试模块不能互相 import）。

use super::*;
use crate::context::WorkflowIdentity;
use crate::engine::WorkflowEngine;
use crate::types::{TriggerSource, Workflow};

// ---------------------------------------------------------------------------
// fixture：nemesis-tools Tool 假件 + SecureExecutor 捕获假件
// ---------------------------------------------------------------------------

/// 裸注册表假工具（锁 1 未接线路径）。
struct WfBareTool;

#[async_trait]
impl nemesis_tools::registry::Tool for WfBareTool {
    fn name(&self) -> &str {
        "wf_bare_tool"
    }
    fn description(&self) -> &str {
        "bare registry stub"
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({})
    }
    async fn execute(&self, args: &serde_json::Value) -> nemesis_tools::types::ToolResult {
        nemesis_tools::types::ToolResult::success(&format!("bare_ok:{args}"))
    }
}

/// SecureExecutor 假件：捕获四参（node_id/tool_name/args/identity），按构造
/// 返回 success/error。捕获句柄由 new() 返回给测试断言。
struct WfCapturingSecure {
    fail_with: Option<String>,
    captured: std::sync::Arc<std::sync::Mutex<Option<CapturedSecureCall>>>,
}

#[derive(Debug, Clone)]
struct CapturedSecureCall {
    node_id: String,
    tool_name: String,
    args: serde_json::Value,
    identity: WorkflowIdentity,
}

impl WfCapturingSecure {
    fn new(
        fail_with: Option<String>,
    ) -> (
        Self,
        std::sync::Arc<std::sync::Mutex<Option<CapturedSecureCall>>>,
    ) {
        let captured = std::sync::Arc::new(std::sync::Mutex::new(None));
        (
            Self {
                fail_with,
                captured: captured.clone(),
            },
            captured,
        )
    }
}

#[async_trait]
impl SecureExecutor for WfCapturingSecure {
    async fn execute(
        &self,
        node_id: &str,
        tool_name: &str,
        args: &serde_json::Value,
        identity: &crate::context::WorkflowIdentity,
    ) -> nemesis_tools::types::ToolResult {
        *self.captured.lock().unwrap() = Some(CapturedSecureCall {
            node_id: node_id.to_string(),
            tool_name: tool_name.to_string(),
            args: args.clone(),
            identity: identity.clone(),
        });
        match &self.fail_with {
            Some(msg) => nemesis_tools::types::ToolResult::error(msg),
            None => nemesis_tools::types::ToolResult::success("secure_ok"),
        }
    }
}

fn wf_gate_node(id: &str, config: HashMap<String, serde_json::Value>) -> NodeDef {
    NodeDef {
        id: id.to_string(),
        node_type: "tool".to_string(),
        config,
        depends_on: vec![],
        retry_count: 0,
        timeout: None,
        is_terminal: false,
    }
}

// ---------------------------------------------------------------------------
// ① 锁 1 未接线：裸注册表直连路径行为不变
// ---------------------------------------------------------------------------

/// secure=None 时走旧直连路径：注册表里的工具照常执行、结果映射不变。
/// （Once loud warn 是 tracing 侧效应，无捕获基建不断言；接线缺口由
/// gateway 装配代码保证消除，本锁钉「行为不变」这一半。）
#[tokio::test]
async fn tool_node_without_secure_falls_back_to_bare_registry_unchanged() {
    let registry = std::sync::Arc::new(nemesis_tools::registry::ToolRegistry::new());
    registry.register(std::sync::Arc::new(WfBareTool));
    let exec = RealToolNodeExecutor::new(registry);

    let mut config = HashMap::new();
    config.insert("name".to_string(), serde_json::json!("wf_bare_tool"));
    config.insert("args".to_string(), serde_json::json!({ "k": "v" }));
    let node = wf_gate_node("bare-1", config);

    let result = exec
        .execute(
            &node,
            &HashMap::new(),
            &WorkflowContext::new(HashMap::new()),
        )
        .await
        .unwrap();
    assert_eq!(
        result.state,
        ExecutionState::Completed,
        "未接线路径必须保持旧行为（裸注册表执行成功）: {:?}",
        result.error
    );
    assert_eq!(result.output["tool"], "wf_bare_tool");
    let expected = format!("bare_ok:{}", serde_json::json!({ "k": "v" }));
    assert_eq!(
        result.output["result"], expected,
        "裸路径结果必须原样透传（行为不变锁）"
    );
}

// ---------------------------------------------------------------------------
// ② 锁 1 接线面：全量路由 SecureExecutor + identity 全字段透传
// ---------------------------------------------------------------------------

/// 接线后：调用路由 secure 通道（空注册表也放行 = 证明不再触碰裸注册表），
/// node_id/tool_name/模板解析后 args/identity 七字段全部如实透传。
#[tokio::test]
async fn tool_node_with_secure_routes_everything_and_skips_bare_registry() {
    let (mock, captured) = WfCapturingSecure::new(None);
    // 注册表刻意为空：若误走裸路径会得到 tool not found 错误。
    let exec = RealToolNodeExecutor::new_with_secure_executor(
        std::sync::Arc::new(nemesis_tools::registry::ToolRegistry::new()),
        std::sync::Arc::new(mock),
    );

    let identity = WorkflowIdentity {
        execution_id: "exec-9".to_string(),
        workflow_name: "demo-flow".to_string(),
        trigger_label: "agent-tool".to_string(),
        session_key: "agent:main:session:s-1".to_string(),
        chat_id: "c-1".to_string(),
        sender_id: "u-1".to_string(),
        recursion_depth: 2,
    };
    let wf_ctx = WorkflowContext::new_with_identity(identity, HashMap::new());

    let mut config = HashMap::new();
    config.insert("name".to_string(), serde_json::json!("wf_sec_tool"));
    config.insert(
        "args".to_string(),
        serde_json::json!({ "path": "{{infile}}" }),
    );
    let node = wf_gate_node("sec-node-7", config);

    let mut ctx = HashMap::new();
    ctx.insert("infile".to_string(), serde_json::json!("a.txt"));

    let result = exec.execute(&node, &ctx, &wf_ctx).await.unwrap();
    assert_eq!(result.state, ExecutionState::Completed);
    assert_eq!(result.output["result"], "secure_ok");

    let call = captured
        .lock()
        .unwrap()
        .clone()
        .expect("接线后调用必须进 secure 通道");
    assert_eq!(
        call.node_id, "sec-node-7",
        "节点 id 必须随行（归因键 wf_node_id）"
    );
    assert_eq!(call.tool_name, "wf_sec_tool");
    assert_eq!(
        call.args,
        serde_json::json!({ "path": "a.txt" }),
        "args 必须先经模板解析再进 secure 通道"
    );
    assert_eq!(call.identity.execution_id, "exec-9");
    assert_eq!(call.identity.workflow_name, "demo-flow");
    assert_eq!(call.identity.trigger_label, "agent-tool");
    assert_eq!(call.identity.session_key, "agent:main:session:s-1");
    assert_eq!(call.identity.chat_id, "c-1");
    assert_eq!(call.identity.sender_id, "u-1");
    assert_eq!(call.identity.recursion_depth, 2);
}

// ---------------------------------------------------------------------------
// ③ ToolResult 映射：error → Failed
// ---------------------------------------------------------------------------

/// secure 返回 error ToolResult → 节点 Failed（不是 Err），错误文本带
/// `tool '<name>' error:` 前缀（workflow 可按失败态分支）。
#[tokio::test]
async fn tool_node_error_tool_result_maps_to_failed_node_result() {
    let (mock, captured) = WfCapturingSecure::new(Some("boom".to_string()));
    let exec = RealToolNodeExecutor::new_with_secure_executor(
        std::sync::Arc::new(nemesis_tools::registry::ToolRegistry::new()),
        std::sync::Arc::new(mock),
    );
    let mut config = HashMap::new();
    config.insert("name".to_string(), serde_json::json!("wf_sec_tool"));
    let node = wf_gate_node("err-node", config);

    let result = exec
        .execute(
            &node,
            &HashMap::new(),
            &WorkflowContext::new(HashMap::new()),
        )
        .await
        .unwrap();
    assert_eq!(
        result.state,
        ExecutionState::Failed,
        "error ToolResult 必须映射 Failed 态（不是 Err 返回）"
    );
    let err = result.error.as_deref().unwrap_or_default();
    assert!(
        err.contains("tool 'wf_sec_tool' error: boom"),
        "错误文本必须带工具名与原因: {err}"
    );
    assert!(
        captured.lock().unwrap().is_some(),
        "失败形态也必须先经过 secure 通道（闸在执行前）"
    );
}

// ---------------------------------------------------------------------------
// ⑤ TriggerSource label/identity_triple 全变体表测
// ---------------------------------------------------------------------------

#[test]
fn trigger_source_labels_cover_all_variants() {
    let cases: Vec<(TriggerSource, &str)> = vec![
        (TriggerSource::Cli, "cli"),
        (TriggerSource::Cron, "cron"),
        (
            TriggerSource::Webhook {
                payload: serde_json::json!({}),
            },
            "webhook",
        ),
        (
            TriggerSource::AgentTool {
                tool_call_id: "t".to_string(),
                recursion_depth: 0,
                session_key: String::new(),
                chat_id: String::new(),
                sender_id: String::new(),
            },
            "agent-tool",
        ),
        (
            TriggerSource::Chat {
                chat_id: "c".to_string(),
                session_key: "s".to_string(),
                sender_id: "u".to_string(),
                message: "m".to_string(),
            },
            "chat",
        ),
        (
            TriggerSource::Message {
                channel: "web".to_string(),
                chat_id: "c".to_string(),
                sender_id: "u".to_string(),
                content: "x".to_string(),
            },
            "message",
        ),
        (
            TriggerSource::WebUI {
                session_id: "w".to_string(),
            },
            "web-ui",
        ),
        (
            TriggerSource::Event {
                event_type: "e".to_string(),
                data: serde_json::json!({}),
            },
            "event",
        ),
        (
            TriggerSource::WorkflowChat {
                chat_id: "c".to_string(),
                session_id: "sid".to_string(),
                workflow_name: "demo".to_string(),
                index: "abcd1234".to_string(),
                session_key: "s".to_string(),
            },
            "workflow-chat",
        ),
    ];
    for (source, expected) in cases {
        assert_eq!(
            source.label(),
            expected,
            "label() 表必须覆盖全部变体（审计 wf_trigger 来源）"
        );
    }
}

#[test]
fn trigger_source_identity_triple_per_variant() {
    // 携带会话身份的变体：三元组如实提取。
    let chat = TriggerSource::Chat {
        chat_id: "chat-1".to_string(),
        session_key: "sess-1".to_string(),
        sender_id: "user-1".to_string(),
        message: String::new(),
    };
    assert_eq!(
        chat.identity_triple(),
        ("sess-1".into(), "chat-1".into(), "user-1".into())
    );

    let agent_tool = TriggerSource::AgentTool {
        tool_call_id: "t".to_string(),
        recursion_depth: 1,
        session_key: "sess-2".to_string(),
        chat_id: "chat-2".to_string(),
        sender_id: "user-2".to_string(),
    };
    assert_eq!(
        agent_tool.identity_triple(),
        ("sess-2".into(), "chat-2".into(), "user-2".into())
    );

    // Message 是生产者侧：无 session_key（诚实空串），chat/sender 随行。
    let message = TriggerSource::Message {
        channel: "rpc".to_string(),
        chat_id: "chat-3".to_string(),
        sender_id: "user-3".to_string(),
        content: String::new(),
    };
    assert_eq!(
        message.identity_triple(),
        (String::new(), "chat-3".into(), "user-3".into())
    );

    // WorkflowChat：sender 恒空（页面身份），session/chat 随行。
    let wfchat = TriggerSource::WorkflowChat {
        chat_id: "chat-4".to_string(),
        session_id: "sid-4".to_string(),
        workflow_name: "demo".to_string(),
        index: "abcd1234".to_string(),
        session_key: "sess-4".to_string(),
    };
    assert_eq!(
        wfchat.identity_triple(),
        ("sess-4".into(), "chat-4".into(), String::new())
    );

    // 无会话身份的变体：全空串（诚实缺省，不猜身份）。
    assert_eq!(
        TriggerSource::Cli.identity_triple(),
        (String::new(), String::new(), String::new())
    );
    assert_eq!(
        TriggerSource::Cron.identity_triple(),
        (String::new(), String::new(), String::new())
    );
    assert_eq!(
        TriggerSource::WebUI {
            session_id: "w".to_string()
        }
        .identity_triple(),
        (String::new(), String::new(), String::new())
    );
}

// ---------------------------------------------------------------------------
// ⑥ engine e2e：set_tool_executor + run(trigger) → identity 七字段随行
// ---------------------------------------------------------------------------

/// 宿主接线形态（gateway 同构）：engine.set_tool_executor 覆盖占位执行器 →
/// AgentTool 触发整跑 → mock 捕获的 identity 与 execution/trigger 一致
/// （execution_id=执行记录 id、trigger_label、会话三元组、递归深度）。
#[tokio::test]
async fn engine_run_agent_tool_trigger_fills_identity_end_to_end() {
    let (mock, captured) = WfCapturingSecure::new(None);
    let engine = WorkflowEngine::new();
    engine.set_tool_executor(std::sync::Arc::new(
        RealToolNodeExecutor::new_with_secure_executor(
            std::sync::Arc::new(nemesis_tools::registry::ToolRegistry::new()),
            std::sync::Arc::new(mock),
        ),
    ));

    let mut config = HashMap::new();
    config.insert("name".to_string(), serde_json::json!("wf_sec_tool"));
    let wf = Workflow {
        name: "identity_wf".to_string(),
        description: String::new(),
        version: "1.0.0".to_string(),
        triggers: vec![],
        nodes: vec![wf_gate_node("n1", config)],
        edges: vec![],
        variables: HashMap::new(),
        metadata: HashMap::new(),
    };
    engine.register_workflow(wf).unwrap();

    let execution = engine
        .run(
            "identity_wf",
            HashMap::new(),
            Some(TriggerSource::AgentTool {
                tool_call_id: "tc-1".to_string(),
                recursion_depth: 1,
                session_key: "agent:main:session:s-9".to_string(),
                chat_id: "c-9".to_string(),
                sender_id: "u-9".to_string(),
            }),
        )
        .await
        .unwrap();
    assert_eq!(execution.state, ExecutionState::Completed);

    let call = captured
        .lock()
        .unwrap()
        .clone()
        .expect("tool 节点必须经 secure 通道执行");
    assert_eq!(
        call.identity.execution_id, execution.id,
        "identity 的 execution_id 必须等于执行记录 id"
    );
    assert_eq!(call.identity.workflow_name, "identity_wf");
    assert_eq!(call.identity.trigger_label, "agent-tool");
    assert_eq!(call.identity.session_key, "agent:main:session:s-9");
    assert_eq!(call.identity.chat_id, "c-9");
    assert_eq!(call.identity.sender_id, "u-9");
    assert_eq!(
        call.identity.recursion_depth, 1,
        "递归深度必须来自触发源（AgentTool recursion_depth）"
    );
}
