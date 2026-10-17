// B2（profession-capability-evolution 2026-10-14）：职能工具围栏双闸测试。
//
// 覆盖面：
// ① 派发拒绝文案单一真相源 `profession_fence_refusal_for`——白名单外拒绝
//    （含 who 子句两形态）、白名单内放行、question 豁免；
// ② instance 级闸 `profession_fence_refusal`——空表 = 未围（fail-open）；
// ③ 供给闸 `effective_tool_defs`——围栏与注册表取交集，question 豁免，
//    未围实例全量直通；
// ④ 并行安全批随行闸 `precompute_parallel_batch`——白名单外诚实拒绝、
//    白名单内照常执行。
//
// 自带迷你 Mock（本文件独立，不与 tests.rs 共享私有类型）。

use super::*;

// ---------------------------------------------------------------------------
// 迷你 fixture
// ---------------------------------------------------------------------------

struct FenceMockProvider;

#[async_trait]
impl LlmProvider for FenceMockProvider {
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

struct EchoTool {
    result: String,
}

#[async_trait]
impl Tool for EchoTool {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok(self.result.clone())
    }
}

fn fence_config(tools: Vec<String>) -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: None,
        max_turns: 5,
        tools,
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
    }
}

fn fence_profession_config(tools: Vec<String>, profession: &str) -> AgentConfig {
    AgentConfig {
        tool_fence_profession: Some(profession.to_string()),
        ..fence_config(tools)
    }
}

// ---------------------------------------------------------------------------
// ① 派发拒绝文案单一真相源
// ---------------------------------------------------------------------------

#[test]
fn fence_refusal_outside_tool_gets_message_with_who_clause() {
    let allowed = vec!["read_file".to_string(), "grep".to_string()];
    let msg = super::tool_dispatch::profession_fence_refusal_for(Some("product"), &allowed, "exec")
        .expect("白名单外必须拒绝");
    assert!(
        msg.starts_with("Error: tool 'exec' is outside the task's tool fence"),
        "{msg}"
    );
    assert!(msg.contains("(profession: product)"), "{msg}");
    assert!(msg.contains("allowed: read_file, grep"), "{msg}");
    assert!(msg.contains("Do NOT retry this tool"), "{msg}");
}

#[test]
fn fence_refusal_no_profession_omits_who_clause() {
    let allowed = vec!["read_file".to_string()];
    let msg = super::tool_dispatch::profession_fence_refusal_for(None, &allowed, "exec")
        .expect("白名单外必须拒绝");
    assert!(
        !msg.contains("profession:"),
        "无职能来源时 who 子句缺席：{msg}"
    );
    assert!(msg.contains("allowed: read_file"), "{msg}");
}

#[test]
fn fence_refusal_inside_tool_and_question_are_exempt() {
    let allowed = vec!["read_file".to_string(), "grep".to_string()];
    assert_eq!(
        super::tool_dispatch::profession_fence_refusal_for(Some("product"), &allowed, "read_file"),
        None,
        "白名单内放行"
    );
    // question 是围栏内任务向人求助的唯一通道，恒豁免（FENCE_EXEMPT_TOOLS）。
    assert_eq!(
        super::tool_dispatch::profession_fence_refusal_for(Some("product"), &allowed, "question"),
        None,
        "question 必须豁免"
    );
    assert!(super::tool_dispatch::fence_exempt_tool("question"));
}

// ---------------------------------------------------------------------------
// ② instance 级闸：空表 = 未围
// ---------------------------------------------------------------------------

#[test]
fn fence_refusal_instance_empty_tools_not_fenced() {
    let instance = AgentInstance::new(fence_config(vec![]));
    assert_eq!(
        super::tool_dispatch::profession_fence_refusal(&instance, "exec"),
        None,
        "空表 = 没围，任何工具都不拦（loop 级 config.tools 恒空的生产主路径）"
    );
}

#[test]
fn fence_refusal_instance_fenced_tool_refused() {
    let instance = AgentInstance::new(fence_profession_config(
        vec!["read_file".to_string()],
        "product",
    ));
    assert!(super::tool_dispatch::profession_fence_refusal(&instance, "exec").is_some());
    assert_eq!(
        super::tool_dispatch::profession_fence_refusal(&instance, "read_file"),
        None
    );
}

// ---------------------------------------------------------------------------
// ③ 供给闸：effective_tool_defs 交集 + question 豁免
// ---------------------------------------------------------------------------

#[tokio::test]
async fn effective_tool_defs_fence_intersects_registry_and_exempts_question() {
    let mut agent_loop = AgentLoop::new(Box::new(FenceMockProvider), fence_config(vec![]));
    for name in ["echo_alpha", "echo_beta", "question"] {
        agent_loop.register_tool(
            name.to_string(),
            Box::new(EchoTool {
                result: name.to_string(),
            }),
        );
    }

    // 未围实例（tools 空）：全量直通。
    let plain = AgentInstance::new(fence_config(vec![]));
    let names: Vec<String> = agent_loop
        .effective_tool_defs(&plain)
        .iter()
        .map(|d| d.function.name.clone())
        .collect();
    for expected in ["echo_alpha", "echo_beta", "question"] {
        assert!(
            names.iter().any(|n| n == expected),
            "未围应直通 {expected}，实际 {names:?}"
        );
    }

    // 围栏实例（tools=["echo_alpha"]）：供给只剩 echo_alpha + question 豁免。
    let fenced = AgentInstance::new(fence_config(vec!["echo_alpha".to_string()]));
    let mut names: Vec<String> = agent_loop
        .effective_tool_defs(&fenced)
        .iter()
        .map(|d| d.function.name.clone())
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec!["echo_alpha".to_string(), "question".to_string()],
        "围栏供给 = 白名单 ∪ question 豁免，echo_beta 必须被滤掉"
    );
}

// ---------------------------------------------------------------------------
// ④ 并行安全批随行闸
// ---------------------------------------------------------------------------

#[tokio::test]
async fn precompute_parallel_batch_fence_refuses_outside_and_executes_inside() {
    let mut agent_loop = AgentLoop::new(Box::new(FenceMockProvider), fence_config(vec![]));
    agent_loop.register_tool(
        "echo_alpha".to_string(),
        Box::new(EchoTool {
            result: "ALPHA-OK".to_string(),
        }),
    );
    agent_loop.register_tool(
        "echo_beta".to_string(),
        Box::new(EchoTool {
            result: "BETA-OK".to_string(),
        }),
    );
    agent_loop.register_tool(
        "question".to_string(),
        Box::new(EchoTool {
            result: "question".to_string(),
        }),
    );
    let context = RequestContext::new("web", "chat1", "user1", "session1");
    let calls = vec![
        ToolCallInfo {
            id: "c1".into(),
            name: "echo_alpha".into(),
            arguments: "{}".into(),
        },
        ToolCallInfo {
            id: "c2".into(),
            name: "exec".into(),
            arguments: "{}".into(),
        },
        ToolCallInfo {
            id: "c3".into(),
            name: "question".into(),
            arguments: "{}".into(),
        },
    ];
    let results = agent_loop
        .precompute_parallel_batch(
            &calls,
            &context,
            0,
            false,
            None,
            Some("product".to_string()),
            vec!["echo_alpha".to_string()],
        )
        .await;
    assert_eq!(results.len(), 3);
    // 白名单内照常执行。
    assert_eq!(results[0].result, "ALPHA-OK", "白名单内工具必须真执行");
    assert!(!results[0].validation_failed);
    // 白名单外诚实拒绝（文案单一真相源，并行/串行一致）。
    assert!(
        results[1]
            .result
            .contains("is outside the task's tool fence (profession: product)"),
        "并行批白名单外必须拒绝：{}",
        results[1].result
    );
    // question 豁免（注册表里是我们注册的 EchoTool，能执行就证明没被拦）。
    assert_eq!(results[2].result, "question", "question 豁免后应照常执行");
}

#[tokio::test]
async fn precompute_parallel_batch_empty_fence_tools_passes_through() {
    let mut agent_loop = AgentLoop::new(Box::new(FenceMockProvider), fence_config(vec![]));
    agent_loop.register_tool(
        "echo_alpha".to_string(),
        Box::new(EchoTool {
            result: "ALPHA-OK".to_string(),
        }),
    );
    let context = RequestContext::new("web", "chat1", "user1", "session1");
    let calls = vec![ToolCallInfo {
        id: "c1".into(),
        name: "echo_alpha".into(),
        arguments: "{}".into(),
    }];
    // fence_tools 空 = 未围，恒放行（调用方从 instance config 取值的生产形态）。
    let results = agent_loop
        .precompute_parallel_batch(&calls, &context, 0, false, None, None, Vec::new())
        .await;
    assert_eq!(results[0].result, "ALPHA-OK");
}
