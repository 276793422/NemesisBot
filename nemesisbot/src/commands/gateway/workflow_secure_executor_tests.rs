// workflow 安全执行通道 gateway 实现回归锁（A 批，PLAN 回归锁 e2e 面）。
//
// `WorkflowSecureExecutor` 是 workflow tool 节点 → AgentLoop dispatch 瀑布的
// 宿主接线（闸序单一真相源零重实现）。本文件钉住 gateway 侧三个接缝：
//   ① e2e：identity 三键（chat_id/sender_id/session_key）→ RequestContext
//      （channel="workflow"）→ 工具体可见——审批卡路由回触发会话与
//      审计归因的身份源头在这条构造里；args JSON 逐字节透传；
//   ② 失败映射：dispatch 返回错误文本（工具体 Err）→ ToolResult::error
//      （NodeResult 拿到失败态，workflow 可按失败分支）；
//   ③ upgrade 失败：loop 停机窗口 → 诚实 error（绝不回退裸注册表——
//      工具不幂等，静默双执行）。
//
// 五闸/审批/归因行为面在 nemesis-agent 侧回归锁（loop/workflow_gate_tests.rs
// + workflow_attrib_gate_tests.rs）；本文件只锁 gateway 构造与映射。
//
// fixture 自带。零 LLM 链（provider 恒不投递消息，dispatch-only）。

use std::sync::Arc;

use nemesis_agent::context::RequestContext;
use nemesis_agent::r#loop::{AgentLoop, LlmMessage, LlmProvider, LlmResponse, Tool};
use nemesis_agent::types::AgentConfig;
use nemesis_workflow::WorkflowIdentity;
use nemesis_workflow::nodes::SecureExecutor;

// ---------------------------------------------------------------------------
// fixture
// ---------------------------------------------------------------------------

/// 恒不投递消息的假 provider（dispatch-only：本文件不跑 LLM 轮）。
struct WfSecNoopProvider;

#[async_trait::async_trait]
impl LlmProvider for WfSecNoopProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<nemesis_agent::types::ChatOptions>,
        _tools: Vec<nemesis_agent::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Ok(LlmResponse {
            finish_reason: None,
            escalated_to: None,
            content: "unused".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

/// 工具体看到的调用现场（e2e 断言面）。
#[derive(Debug, Clone, Default)]
struct SeenCall {
    args: String,
    channel: String,
    chat_id: String,
    user: String,
    session_key: String,
}

/// 捕获调用现场、按构造 Ok/Err 的工具假件。
struct WfSecCaptureTool {
    fail: Option<&'static str>,
    seen: Arc<std::sync::Mutex<SeenCall>>,
}

#[async_trait::async_trait]
impl Tool for WfSecCaptureTool {
    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String> {
        *self.seen.lock().unwrap() = SeenCall {
            args: args.to_string(),
            channel: context.channel.clone(),
            chat_id: context.chat_id.clone(),
            user: context.user.clone(),
            session_key: context.session_key.clone(),
        };
        match self.fail {
            Some(msg) => Err(msg.to_string()),
            None => Ok("wf_echo_ok".to_string()),
        }
    }
}

fn wf_sec_identity() -> WorkflowIdentity {
    WorkflowIdentity {
        execution_id: "exec-e2e-1".to_string(),
        workflow_name: "demo-flow".to_string(),
        trigger_label: "agent-tool".to_string(),
        session_key: "agent:main:session:wf-e2e".to_string(),
        chat_id: "chat-e2e".to_string(),
        sender_id: "user-e2e".to_string(),
        recursion_depth: 0,
    }
}

fn wf_sec_loop(fail: Option<&'static str>) -> (Arc<AgentLoop>, Arc<std::sync::Mutex<SeenCall>>) {
    let seen = Arc::new(std::sync::Mutex::new(SeenCall::default()));
    let mut agent_loop = AgentLoop::new(
        Box::new(WfSecNoopProvider),
        AgentConfig {
            model: "test-model".to_string(),
            system_prompt: None,
            max_turns: 1,
            tools: vec![],
            ..Default::default()
        },
    );
    agent_loop.register_tool(
        "wf_e2e_tool".to_string(),
        Box::new(WfSecCaptureTool {
            fail,
            seen: seen.clone(),
        }),
    );
    (Arc::new(agent_loop), seen)
}

// ---------------------------------------------------------------------------
// ① e2e：identity 三键 → RequestContext（channel="workflow"）→ 工具体
// ---------------------------------------------------------------------------

/// 宿主接线形态整跑：executor.execute → dispatch 瀑布 → 注册表工具体。
/// 断言工具体看到的 channel/chat_id/user/session_key 全部来自 identity
/// （审批卡路由与审计归因的身份源头），args JSON 无损透传。
#[tokio::test]
async fn executor_routes_identity_into_request_context_end_to_end() {
    let (loop_arc, seen) = wf_sec_loop(None);
    let executor = super::workflow_secure_executor::WorkflowSecureExecutor::new(&loop_arc);

    let result = executor
        .execute(
            "node-e2e",
            "wf_e2e_tool",
            &serde_json::json!({ "p": "v" }),
            &wf_sec_identity(),
        )
        .await;
    assert!(!result.is_error, "正常路径必须 success: {}", result.for_llm);
    assert_eq!(result.for_llm, "wf_echo_ok", "工具体返回值必须透传");

    let seen = seen.lock().unwrap().clone();
    assert_eq!(
        seen.channel, "workflow",
        "RequestContext.channel 必须是 workflow（审计 source 可辨触发源头）"
    );
    assert_eq!(
        seen.chat_id, "chat-e2e",
        "审批卡路由目标（chat_id）必须来自 identity"
    );
    assert_eq!(seen.user, "user-e2e", "发起人身份必须随行");
    assert_eq!(
        seen.session_key, "agent:main:session:wf-e2e",
        "会话键必须随行（会话级豁免按它裁决）"
    );
    let seen_args: serde_json::Value = serde_json::from_str(&seen.args)
        .unwrap_or_else(|e| panic!("工具体收到的 args 必须是合法 JSON（{e}）: {}", seen.args));
    assert_eq!(
        seen_args,
        serde_json::json!({ "p": "v" }),
        "args 必须从 Value 无损透传（无模板残留、无包装）"
    );
}

// ---------------------------------------------------------------------------
// ② 失败映射：工具体 Err → ToolResult::error
// ---------------------------------------------------------------------------

/// 工具体返回 Err → dispatch 回灌错误文本 → tool_result_indicates_error →
/// ToolResult::error（workflow 引擎侧映射 NodeResult 失败态）。
#[tokio::test]
async fn executor_maps_tool_failure_to_error_tool_result() {
    let (loop_arc, _seen) = wf_sec_loop(Some("kaboom"));
    let executor = super::workflow_secure_executor::WorkflowSecureExecutor::new(&loop_arc);

    let result = executor
        .execute(
            "node-e2e",
            "wf_e2e_tool",
            &serde_json::json!({}),
            &wf_sec_identity(),
        )
        .await;
    assert!(
        result.is_error,
        "工具体 Err 必须映射 error ToolResult: {}",
        result.for_llm
    );
    assert!(
        result.for_llm.contains("kaboom"),
        "错误原因必须透传进 for_llm: {}",
        result.for_llm
    );
}

// ---------------------------------------------------------------------------
// ③ upgrade 失败：loop 停机窗口 → 诚实 error（绝不回退裸注册表）
// ---------------------------------------------------------------------------

/// loop Arc drop 后 upgrade 失败 → 诚实 error 文本；不回退裸注册表
/// （工具不幂等，静默双执行红线）。
#[tokio::test]
async fn executor_upgrade_failure_is_honest_error() {
    let (loop_arc, _seen) = wf_sec_loop(None);
    let executor = super::workflow_secure_executor::WorkflowSecureExecutor::new(&loop_arc);
    drop(loop_arc); // 模拟 agent loop 停机窗口（重建后旧 Weak 失效）。

    let result = executor
        .execute(
            "node-e2e",
            "wf_e2e_tool",
            &serde_json::json!({}),
            &wf_sec_identity(),
        )
        .await;
    assert!(result.is_error, "停机窗口必须诚实报错: {}", result.for_llm);
    assert!(
        result.for_llm.contains("agent loop unavailable"),
        "错误文案必须指明 loop 不可用（不静默、不回退）: {}",
        result.for_llm
    );
}

// ---------------------------------------------------------------------------
// ④ estop 联动（PLAN 回归锁 #10）：急停触发 → 运行中 execution 全取消
// ---------------------------------------------------------------------------

/// estop watcher e2e：注册 delay 30s 工作流后台跑 → spawn_estop_watcher →
/// estop.trigger() → 执行经 cancel token 取消路径落 **Cancelled** 终态
/// （estop 取消 = 用户取消语义；墙钟看门狗超时才落 Failed + 注记，两者
/// 收口面不同——见 engine.rs 墙钟注释）。释放不动作（不复活）由引擎
/// cancel 终态锁钉住，此处锁 gateway watcher → 全取消接线。
#[tokio::test]
async fn estop_watcher_cancels_running_workflow_executions() {
    let engine = nemesis_workflow::engine::WorkflowEngine::new_arc();

    let mut config = std::collections::HashMap::new();
    config.insert("seconds".to_string(), serde_json::json!(30.0));
    let node = nemesis_workflow::types::NodeDef {
        id: "slow".to_string(),
        node_type: "delay".to_string(),
        config,
        depends_on: vec![],
        retry_count: 0,
        timeout: None,
        is_terminal: false,
    };
    let wf = nemesis_workflow::types::Workflow {
        name: "estop-watch-e2e".to_string(),
        description: String::new(),
        version: "1.0.0".to_string(),
        triggers: vec![],
        nodes: vec![node],
        edges: vec![],
        variables: std::collections::HashMap::new(),
        metadata: std::collections::HashMap::new(),
    };
    engine
        .register_workflow(wf)
        .expect("register estop-watch-e2e");

    let estop = Arc::new(nemesis_agent::estop::EstopState::new());
    super::workflow_secure_executor::spawn_estop_watcher(&engine, &estop);

    // 后台跑（run 完成前 estop 就该取消它）。
    let engine_bg = engine.clone();
    tokio::spawn(async move {
        let _ = engine_bg
            .run(
                "estop-watch-e2e",
                std::collections::HashMap::new(),
                Some(nemesis_workflow::types::TriggerSource::Cli),
            )
            .await;
    });

    // 等执行进入 Running（delay 节点睡着即 Running）。
    let exec_id = {
        let mut found = None;
        for _ in 0..100 {
            let execs = engine.list_executions(Some("estop-watch-e2e")).await;
            if let Some(e) = execs
                .iter()
                .find(|e| e.state == nemesis_workflow::types::ExecutionState::Running)
            {
                found = Some(e.id.clone());
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        found.expect("execution 应在 2s 内进入 Running")
    };

    estop.trigger();

    // 轮询终态：watcher 收到 watch 通知 → cancel_all_running → Cancelled。
    let mut final_state = None;
    for _ in 0..200 {
        let e = engine.get_execution(&exec_id).await.expect("执行必须存在");
        let s = e.state;
        match s {
            nemesis_workflow::types::ExecutionState::Cancelled => {
                final_state = Some(s);
                break;
            }
            nemesis_workflow::types::ExecutionState::Running
            | nemesis_workflow::types::ExecutionState::Pending => {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            other => panic!(
                "estop 触发后执行不得落到其它状态（{:?}）——取消接线只应产生 Cancelled",
                other
            ),
        }
    }
    assert_eq!(
        final_state,
        Some(nemesis_workflow::types::ExecutionState::Cancelled),
        "estop 触发必须把运行中执行取消（Cancelled 终态）"
    );

    // 释放不动作：release 后执行保持 Cancelled（诚实终态，不自动复活）。
    estop.release();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let e = engine.get_execution(&exec_id).await.expect("执行必须存在");
    assert_eq!(
        e.state,
        nemesis_workflow::types::ExecutionState::Cancelled,
        "estop release 不复活被取消的执行（诚实终态）"
    );
}
