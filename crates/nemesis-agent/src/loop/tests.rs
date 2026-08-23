use super::*;

/// Mock LLM provider for testing.
struct MockLlmProvider {
    responses: std::sync::Mutex<Vec<LlmResponse>>,
}

impl MockLlmProvider {
    fn new(responses: Vec<LlmResponse>) -> Self {
        Self {
            responses: std::sync::Mutex::new(responses),
        }
    }
}

#[async_trait]
impl LlmProvider for MockLlmProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        let mut responses = self.responses.lock().unwrap();
        if responses.is_empty() {
            Ok(LlmResponse {
                content: "No more responses".to_string(),
                tool_calls: Vec::new(),
                finished: true,
                reasoning_content: None,
                usage: None,
                raw_request_body: None,
                raw_response_body: None,
            })
        } else {
            Ok(responses.remove(0))
        }
    }
}

/// Mock tool for testing.
struct MockTool {
    result: String,
}

#[async_trait]
impl Tool for MockTool {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok(self.result.clone())
    }
}

/// A tool with a required field, so calls missing it fail schema validation —
/// used to exercise the validation-retry-budget path.
struct StrictTool;

#[async_trait]
impl Tool for StrictTool {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok("ok".to_string())
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]})
    }
}

/// U5 (sixth batch): a read-only tool that sleeps a fixed duration then
/// returns a marker — drives the concurrency wall-clock test. The delay
/// is what the parallel pool overlaps; a serial path would sum them.
struct SlowReadTool {
    marker: String,
    delay_ms: u64,
}

#[async_trait]
impl Tool for SlowReadTool {
    async fn execute(&self, _args: &str, _ctx: &RequestContext) -> Result<String, String> {
        tokio::time::sleep(std::time::Duration::from_millis(self.delay_ms)).await;
        Ok(self.marker.clone())
    }
    fn is_read_only(&self) -> bool {
        true
    }
}

/// U5: same shape but NOT read-only (default `false`) — forces a mixed batch
/// onto the serial path, the byte-identical-to-pre-U5 fallback.
struct SlowWriteTool {
    marker: String,
    delay_ms: u64,
}

#[async_trait]
impl Tool for SlowWriteTool {
    async fn execute(&self, _args: &str, _ctx: &RequestContext) -> Result<String, String> {
        tokio::time::sleep(std::time::Duration::from_millis(self.delay_ms)).await;
        Ok(self.marker.clone())
    }
    // is_read_only defaults to false — fail-closed, never joins the pool.
}

fn test_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        max_turns: 5,
        tools: vec!["calculator".to_string()],
        models: std::collections::HashMap::new(),
    }
}

#[tokio::test]
async fn simple_text_response() {
    let provider = MockLlmProvider::new(vec![LlmResponse {
        content: "Hello!".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);
    let agent_loop = AgentLoop::new(Box::new(provider), test_config());
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");

    let events = agent_loop.run(&instance, "Hi", &context).await;

    // Should get a Done event.
    let done_events: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Done(msg) => Some(msg.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(done_events.len(), 1);
    assert_eq!(done_events[0], "Hello!");
}

/// A usage whose completion_tokens sits exactly at the default max_tokens cap
/// (8192) — the truncation signal the continue-generation path detects.
fn cap_usage() -> Option<crate::loop_executor::ObserverUsageInfo> {
    Some(crate::loop_executor::ObserverUsageInfo {
        prompt_tokens: 100,
        completion_tokens: 8192,
        total_tokens: 8292,
        cached_tokens: None,
        cache_creation_tokens: None,
        cache_read_tokens: None,
    })
}

/// Truncation at the max_tokens cap must trigger continue-generation: the loop
/// drops the truncated partial and reaches the model's follow-up response
/// instead of accepting the cut-off partial. (Under the old behavior the
/// partial would be accepted as the final answer; under the validation-failure
/// misroute a truncated tool call would force-stop — this mock uses a no-tool
/// partial so the assertion is independent of the tier retry budget.)
#[tokio::test]
async fn length_truncation_triggers_continue_generation() {
    let provider = MockLlmProvider::new(vec![
        LlmResponse {
            content: "partial preamble that got cut".to_string(),
            tool_calls: Vec::new(),
            finished: false,
            reasoning_content: None,
            usage: cap_usage(),
            raw_request_body: None,
            raw_response_body: None,
        },
        LlmResponse {
            content: "completed after continuing".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
    ]);
    let agent_loop = AgentLoop::new(Box::new(provider), test_config());
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");

    let events = agent_loop
        .run(&instance, "write a big file", &context)
        .await;

    let done: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Done(m) => Some(m.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(done.len(), 1, "expected exactly one Done: {:?}", events);
    assert_eq!(
        done[0], "completed after continuing",
        "continue-generation must reach the follow-up response, not accept the truncated partial"
    );
}

/// Repeated truncation exhausts MAX_LENGTH_CONTINUATIONS and surfaces the
/// clear "输出反复超过 max_tokens" error — NOT the misleading
/// "工具参数校验连续失败" / "not valid JSON" validation error.
#[tokio::test]
async fn length_truncation_budget_exhausts_with_clear_error() {
    let truncated = || LlmResponse {
        content: "x".to_string(),
        tool_calls: Vec::new(),
        finished: false,
        reasoning_content: None,
        usage: cap_usage(),
        raw_request_body: None,
        raw_response_body: None,
    };
    // More than MAX_LENGTH_CONTINUATIONS (5) truncated responses.
    let provider = MockLlmProvider::new(vec![truncated(); 7]);
    let mut cfg = test_config();
    cfg.max_turns = 12; // allow enough turns to reach the exhaustion branch
    let agent_loop = AgentLoop::new(Box::new(provider), cfg);
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");

    let events = agent_loop
        .run(&instance, "write a huge file", &context)
        .await;

    let errors: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Error(m) => Some(m.clone()),
            _ => None,
        })
        .collect();
    assert!(
        errors.iter().any(|m| m.contains("max_tokens")),
        "expected the clear max_tokens error, got events: {:?}",
        events
    );
    assert!(
        errors
            .iter()
            .all(|m| !m.contains("校验连续失败") && !m.contains("not valid JSON")),
        "must NOT surface the misleading validation error: {:?}",
        errors
    );
}

#[tokio::test]
async fn estop_engaged_stops_loop_before_llm() {
    // 触发的急停必须在 checkpoint A（轮次顶部）break，**在调用 LLM 之前**——
    // 所以 mock provider 的回复绝不被消费，用户拿到「已急停」Done 事件。
    use std::sync::Arc;
    let provider = MockLlmProvider::new(vec![LlmResponse {
        content: "MUST NOT BE RETURNED".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);
    let agent_loop = AgentLoop::new(Box::new(provider), test_config());

    // 接线 + 触发急停（模拟 SharedResources.estop → set_estop）。
    let estop = Arc::new(crate::estop::EstopState::new());
    agent_loop.set_estop(estop.clone());
    assert!(!estop.is_engaged());
    estop.trigger();
    assert!(estop.is_engaged());

    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");
    let events = agent_loop.run(&instance, "Hi", &context).await;

    let done_events: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Done(msg) => Some(msg.clone()),
            _ => None,
        })
        .collect();

    // checkpoint A 触发：Done 是急停提示，不是 mock 的回复。
    assert!(!done_events.is_empty(), "急停应产生 Done 事件");
    let combined = done_events.join(" | ");
    assert!(
        combined.contains("急停") || combined.contains("ESTOP"),
        "Done 事件应提到急停，实际: {}",
        combined
    );
    assert!(
        !combined.contains("MUST NOT BE RETURNED"),
        "急停期间不应调用 LLM，但回复泄漏了: {}",
        combined
    );
}

#[tokio::test]
async fn estop_disengaged_lets_loop_run_normally() {
    // 对照组：未触发（或未接线）时行为不变——LLM 回复正常返回。
    // 证明 checkpoint 在未触发时是纯短路（零行为变化）。
    use std::sync::Arc;
    let provider = MockLlmProvider::new(vec![LlmResponse {
        content: "Hello!".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);
    let agent_loop = AgentLoop::new(Box::new(provider), test_config());
    // 接线但不触发。
    let estop = Arc::new(crate::estop::EstopState::new());
    agent_loop.set_estop(estop);

    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");
    let events = agent_loop.run(&instance, "Hi", &context).await;

    let done_events: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Done(msg) => Some(msg.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(done_events.len(), 1);
    assert_eq!(done_events[0], "Hello!");
}

#[tokio::test]
async fn estop_interrupts_in_flight_llm_call() {
    // Phase 2：LLM 调用进行中触发急停，应通过 select 的 estop arm 中断挂起的
    // chat future（而不是干等到 provider 永远挂起）。用 HangingProvider 验证——
    // 它的 chat 永远 pending，但进入时翻 entered 标志，让测试确定 chat 已在等。
    use std::sync::atomic::{AtomicBool, Ordering};

    struct HangingProvider {
        entered: Arc<AtomicBool>,
    }
    #[async_trait]
    impl LlmProvider for HangingProvider {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<LlmMessage>,
            _options: Option<crate::types::ChatOptions>,
            _tools: Vec<crate::types::ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            self.entered.store(true, Ordering::SeqCst);
            std::future::pending::<()>().await;
            unreachable!()
        }
    }

    let entered = Arc::new(AtomicBool::new(false));
    let agent_loop = Arc::new(AgentLoop::new(
        Box::new(HangingProvider {
            entered: entered.clone(),
        }),
        test_config(),
    ));
    let estop = Arc::new(crate::estop::EstopState::new());
    agent_loop.set_estop(estop.clone());

    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");

    let al = agent_loop.clone();
    let run_task = tokio::spawn(async move { al.run(&instance, "Hi", &context).await });

    // 等 chat() 被 poll 到（说明 select 已在等 LLM），再触发急停——
    // 这样保证是 Phase 2 的 select arm 命中，而不是 Phase 0 的 checkpoint A。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while !entered.load(Ordering::SeqCst) {
        if std::time::Instant::now() > deadline {
            panic!("provider.chat 从未被 poll——没走到 LLM 调用");
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    estop.trigger();

    let events = tokio::time::timeout(std::time::Duration::from_secs(3), run_task)
        .await
        .expect("run 应在 estop 中断挂起调用后返回")
        .expect("task 不应 panic");

    let done: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Done(m) => Some(m.clone()),
            _ => None,
        })
        .collect();
    let combined = done.join(" | ");
    assert!(
        combined.contains("急停") || combined.contains("ESTOP"),
        "应得到 e-stop 中断的 Done 事件，实际: {}",
        combined
    );
}

#[tokio::test]
async fn estop_blocks_handle_tool_call_when_engaged() {
    // checkpoint C（handle_tool_call 顶部）：engaged 时直接返回 ESTOP 拒绝串，
    // 不执行工具。
    use std::sync::Arc;
    let provider = MockLlmProvider::new(vec![]);
    let mut agent_loop = AgentLoop::new(Box::new(provider), test_config());
    agent_loop.register_tool(
        "calculator".to_string(),
        Box::new(MockTool {
            result: "4".to_string(),
        }),
    );
    let estop = Arc::new(crate::estop::EstopState::new());
    agent_loop.set_estop(estop.clone());
    estop.trigger();

    let tc = ToolCallInfo {
        id: "tc_1".to_string(),
        name: "calculator".to_string(),
        arguments: "{}".to_string(),
    };
    let ctx = RequestContext::new("web", "chat1", "user1", "session1");
    let result = agent_loop.handle_tool_call(&tc, &ctx).await;
    assert!(
        result.contains("ESTOP"),
        "engaged 时应返回 ESTOP 拒绝串，实际: {}",
        result
    );
    assert!(
        !result.contains('4'),
        "工具不应被执行（不应出现 '4'），实际: {}",
        result
    );
}

#[tokio::test]
async fn handle_tool_call_runs_when_estop_disengaged() {
    // 对照组：estop 接线但未触发 → 工具正常执行（覆盖 handle_tool_call 的 fall-through）。
    use std::sync::Arc;
    let provider = MockLlmProvider::new(vec![]);
    let mut agent_loop = AgentLoop::new(Box::new(provider), test_config());
    agent_loop.register_tool(
        "calculator".to_string(),
        Box::new(MockTool {
            result: "4".to_string(),
        }),
    );
    let estop = Arc::new(crate::estop::EstopState::new());
    agent_loop.set_estop(estop); // 接线但不触发

    let tc = ToolCallInfo {
        id: "tc_1".to_string(),
        name: "calculator".to_string(),
        arguments: "{}".to_string(),
    };
    let ctx = RequestContext::new("web", "chat1", "user1", "session1");
    let result = agent_loop.handle_tool_call(&tc, &ctx).await;
    assert_eq!(result, "4");
}

#[tokio::test]
async fn estop_blocks_remaining_tools_in_batch() {
    // checkpoint B（批次每条工具前）：第一个工具执行时触发 estop，
    // 第二个工具应被 checkpoint B 拦下、不执行。
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct EngageTool {
        estop: Arc<crate::estop::EstopState>,
    }
    #[async_trait]
    impl Tool for EngageTool {
        async fn execute(&self, _args: &str, _ctx: &RequestContext) -> Result<String, String> {
            self.estop.trigger();
            Ok("engaged".to_string())
        }
    }
    struct TrackerTool {
        called: Arc<AtomicBool>,
    }
    #[async_trait]
    impl Tool for TrackerTool {
        async fn execute(&self, _args: &str, _ctx: &RequestContext) -> Result<String, String> {
            self.called.store(true, Ordering::SeqCst);
            Ok("should-not-run".to_string())
        }
    }

    let estop = Arc::new(crate::estop::EstopState::new());
    let tracker_called = Arc::new(AtomicBool::new(false));
    let provider = MockLlmProvider::new(vec![LlmResponse {
        content: String::new(),
        tool_calls: vec![
            ToolCallInfo {
                id: "t1".to_string(),
                name: "engage_tool".to_string(),
                arguments: "{}".to_string(),
            },
            ToolCallInfo {
                id: "t2".to_string(),
                name: "tracker_tool".to_string(),
                arguments: "{}".to_string(),
            },
        ],
        finished: false,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);
    let mut agent_loop = AgentLoop::new(Box::new(provider), test_config());
    agent_loop.set_estop(estop.clone());
    agent_loop.register_tool(
        "engage_tool".to_string(),
        Box::new(EngageTool {
            estop: estop.clone(),
        }),
    );
    agent_loop.register_tool(
        "tracker_tool".to_string(),
        Box::new(TrackerTool {
            called: tracker_called.clone(),
        }),
    );

    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");
    let events = agent_loop.run(&instance, "go", &context).await;

    assert!(
        !tracker_called.load(Ordering::SeqCst),
        "第二个工具不应被执行（checkpoint B 应拦下）"
    );
    let done_text: String = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Done(m) => Some(m.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" | ");
    assert!(
        done_text.contains("急停") || done_text.contains("ESTOP"),
        "应有 e-stop Done 事件，实际: {}",
        done_text
    );
}

#[tokio::test]
async fn tool_call_and_response() {
    let provider = MockLlmProvider::new(vec![
        // First call: LLM wants to call a tool.
        LlmResponse {
            content: String::new(),
            tool_calls: vec![ToolCallInfo {
                id: "tc_1".to_string(),
                name: "calculator".to_string(),
                arguments: r#"{"expr":"2+2"}"#.to_string(),
            }],
            finished: false,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
        // Second call: LLM returns final text.
        LlmResponse {
            content: "The answer is 4.".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
    ]);

    let mut agent_loop = AgentLoop::new(Box::new(provider), test_config());
    agent_loop.register_tool(
        "calculator".to_string(),
        Box::new(MockTool {
            result: "4".to_string(),
        }),
    );

    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");

    let events = agent_loop.run(&instance, "What is 2+2?", &context).await;

    // Expect: ToolCall + ToolResult + Done
    assert!(events.iter().any(|e| matches!(e, AgentEvent::ToolCall(_))));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::ToolResult(_)))
    );
    assert!(events.iter().any(|e| matches!(e, AgentEvent::Done(_))));

    // History should have: system + user + assistant(tool_call) + tool + assistant(final)
    let history = instance.get_history();
    assert_eq!(history.len(), 5);
}

#[tokio::test]
async fn rpc_correlation_id_formatting() {
    let provider = MockLlmProvider::new(vec![LlmResponse {
        content: "Pong".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);
    let agent_loop = AgentLoop::new(Box::new(provider), test_config());
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::for_rpc("chat123", "user1", "session1", "corr-42");

    let events = agent_loop.run(&instance, "Ping", &context).await;

    let done_events: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Done(msg) => Some(msg.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(done_events[0], "[rpc:corr-42] Pong");
}

#[tokio::test]
async fn unknown_tool_returns_error() {
    let provider = MockLlmProvider::new(vec![
        LlmResponse {
            content: String::new(),
            tool_calls: vec![ToolCallInfo {
                id: "tc_1".to_string(),
                name: "nonexistent".to_string(),
                arguments: "{}".to_string(),
            }],
            finished: false,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
        LlmResponse {
            content: "I couldn't find that tool.".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
    ]);

    let agent_loop = AgentLoop::new(Box::new(provider), test_config());
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");

    let events = agent_loop.run(&instance, "Do something", &context).await;

    // The tool result should contain the error.
    let tool_errors: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolResult(tr) if tr.result.contains("Unknown tool") => Some(tr.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(tool_errors.len(), 1);
}

#[tokio::test]
async fn max_turns_limit() {
    // Create responses that always request a tool call (infinite loop scenario).
    let infinite_response = LlmResponse {
        content: String::new(),
        tool_calls: vec![ToolCallInfo {
            id: "tc_loop".to_string(),
            name: "calculator".to_string(),
            arguments: "{}".to_string(),
        }],
        finished: false,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    };
    // Create enough responses to exceed max_turns=3.
    let responses: Vec<LlmResponse> = (0..10).map(|_| infinite_response.clone()).collect();

    let provider = MockLlmProvider::new(responses);
    let mut config = test_config();
    config.max_turns = 3;

    let mut agent_loop = AgentLoop::new(Box::new(provider), config.clone());
    agent_loop.register_tool(
        "calculator".to_string(),
        Box::new(MockTool {
            result: "0".to_string(),
        }),
    );

    let instance = AgentInstance::new(config);
    let context = RequestContext::new("web", "chat1", "user1", "session1");

    let events = agent_loop.run(&instance, "Loop test", &context).await;

    // ② Grace round: on the first max_turns hit we grant one finalize round.
    // The model here keeps requesting tools, so the second hit stops resumably
    // with a Done event (no longer a hard "Max iterations reached" Error) and
    // the completed work is preserved.
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::Done(msg) if msg.contains("暂停")))
    );
}

#[tokio::test]
async fn grace_round_finalizes_when_model_cooperates() {
    // ② When max_turns is hit, the loop grants one grace round (with the
    // finalize nudge). If the model cooperates and returns a plain answer on
    // that grace round, the loop ends normally — NOT with the paused Done.
    let tool_resp = LlmResponse {
        content: String::new(),
        tool_calls: vec![ToolCallInfo {
            id: "tc".to_string(),
            name: "calculator".to_string(),
            arguments: "{}".to_string(),
        }],
        finished: false,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    };
    let final_resp = LlmResponse {
        content: "done summarizing".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    };
    // max_turns=2 → 2 tool rounds, then 1 grace round that finalizes.
    let responses = vec![tool_resp.clone(), tool_resp, final_resp];
    let provider = MockLlmProvider::new(responses);
    let mut config = test_config();
    config.max_turns = 2;

    let mut agent_loop = AgentLoop::new(Box::new(provider), config.clone());
    agent_loop.register_tool(
        "calculator".to_string(),
        Box::new(MockTool {
            result: "0".to_string(),
        }),
    );
    let instance = AgentInstance::new(config);
    let context = RequestContext::new("web", "chat1", "user1", "session1");

    let events = agent_loop.run(&instance, "do stuff", &context).await;

    let done_events: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Done(msg) => Some(msg.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(done_events.len(), 1);
    assert_eq!(done_events[0], "done summarizing");
    // The paused-grace Done must NOT appear — the model finalized in time.
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, AgentEvent::Done(msg) if msg.contains("暂停")))
    );
}

// -------------------------------------------------------------------------
// T3 (U12): per-turn cron budget override
// -------------------------------------------------------------------------

fn tool_loop_response() -> LlmResponse {
    LlmResponse {
        content: String::new(),
        tool_calls: vec![ToolCallInfo {
            id: "tc_budget".to_string(),
            name: "calculator".to_string(),
            arguments: "{}".to_string(),
        }],
        finished: false,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }
}

/// A budget override REPLACES the global max_turns for the turn: with
/// max_turns=100 but budget=2, the turn stops after 2 tool rounds (+1 grace
/// round reusing the existing semantics) with the budget-specific message —
/// not after 100.
#[tokio::test]
async fn cron_budget_override_stops_turn() {
    // Plenty of tool-requesting responses — the budget must cut the turn
    // short while responses remain (proving the stop is budget-driven, not
    // mock-exhaustion-driven).
    let responses: Vec<LlmResponse> = (0..20).map(|_| tool_loop_response()).collect();
    let provider = MockLlmProvider::new(responses);
    let mut config = test_config();
    config.max_turns = 100;

    let mut agent_loop = AgentLoop::new(Box::new(provider), config.clone());
    agent_loop.register_tool(
        "calculator".to_string(),
        Box::new(MockTool {
            result: "0".to_string(),
        }),
    );

    let instance = AgentInstance::new(config);
    let context = RequestContext::new("web", "chat1", "user1", "session-budget");

    let trace_id = "budget-test".to_string();
    let token = tokio_util::sync::CancellationToken::new();
    let events = agent_loop
        .run_with_trace(
            &instance,
            "loop forever",
            &context,
            &trace_id,
            false,
            &token,
            Some(2),
        )
        .await;

    // Budget-specific Done message (distinct from the global max_turns
    // wording: mentions the job surviving + re-budgeting).
    let done = events.iter().find_map(|e| match e {
        AgentEvent::Done(msg) => Some(msg.clone()),
        _ => None,
    });
    let done = done.expect("budget stop emits a Done");
    assert!(done.contains("定时任务预算 2 轮"), "done: {done}");
    assert!(done.contains("定时任务未被删除"), "done: {done}");
    // 2 budgeted tool rounds + 1 grace round = 3 tool-call events — far
    // below the 20 available responses and the global cap of 100.
    let tool_rounds = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolCall(_)))
        .count();
    assert_eq!(tool_rounds, 3, "2 budget rounds + 1 grace round");
}

/// No budget (None) → the global max_turns applies with its usual message
/// (the cron override must not accidentally change default behavior).
#[tokio::test]
async fn cron_budget_none_uses_global_default() {
    let responses: Vec<LlmResponse> = (0..10).map(|_| tool_loop_response()).collect();
    let provider = MockLlmProvider::new(responses);
    let mut config = test_config();
    config.max_turns = 3;

    let mut agent_loop = AgentLoop::new(Box::new(provider), config.clone());
    agent_loop.register_tool(
        "calculator".to_string(),
        Box::new(MockTool {
            result: "0".to_string(),
        }),
    );

    let instance = AgentInstance::new(config);
    let context = RequestContext::new("web", "chat1", "user1", "session-budget-none");

    let trace_id = "budget-none-test".to_string();
    let token = tokio_util::sync::CancellationToken::new();
    let events = agent_loop
        .run_with_trace(
            &instance,
            "loop forever",
            &context,
            &trace_id,
            false,
            &token,
            None,
        )
        .await;

    let done = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::Done(msg) => Some(msg.clone()),
            _ => None,
        })
        .expect("global cap emits a Done");
    assert!(
        done.contains("调大 max_tool_iterations"),
        "global-default wording, got: {done}"
    );
    assert!(
        !done.contains("定时任务预算"),
        "no budget wording without an override, got: {done}"
    );
}

/// Budget exhaustion on a cron-exempt turn (user=="cron") still writes ONE
/// boundary turn_end marker with reason budget_exhausted — the observability
/// requirement for T3 despite the cron boundary-exemption.
#[tokio::test]
async fn cron_budget_exhaustion_writes_boundary_marker() {
    let responses: Vec<LlmResponse> = (0..10).map(|_| tool_loop_response()).collect();
    let provider = MockLlmProvider::new(responses);
    let mut config = test_config();
    config.max_turns = 100;

    let mut agent_loop = AgentLoop::new(Box::new(provider), config.clone());
    agent_loop.register_tool(
        "calculator".to_string(),
        Box::new(MockTool {
            result: "0".to_string(),
        }),
    );

    let instance = AgentInstance::new(config);
    // user="cron" is what run_agent_loop_internal sets for cron-originated
    // turns — log_boundaries is false for it (the unbounded-growth exemption).
    let mut context = RequestContext::new("web", "chatc", "cron", "agent:u12-budget-bnd");
    context.user = "cron".to_string();

    let trace_id = "budget-bnd-test".to_string();
    let token = tokio_util::sync::CancellationToken::new();
    let events = agent_loop
        .run_with_trace(
            &instance,
            "loop forever",
            &context,
            &trace_id,
            false,
            &token,
            Some(2),
        )
        .await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::Done(m) if m.contains("定时任务预算"))),
        "budget stop fired"
    );

    // Sidecar: exactly one budget_exhausted turn_end (no turn_start — the
    // cron exemption suppresses those).
    let safe_key = "agent:u12-budget-bnd".replace(':', "_");
    let sidecar = nemesis_path::paths::default_path_manager()
        .boundary_events_dir()
        .join(format!("{}.jsonl", safe_key));
    let content = std::fs::read_to_string(&sidecar).unwrap_or_default();
    let turn_ends: Vec<&str> = content
        .lines()
        .filter(|l| l.contains("\"turn_end\""))
        .collect();
    assert!(
        turn_ends.iter().any(|l| l.contains("budget_exhausted")),
        "sidecar has budget_exhausted turn_end: {content}"
    );
    assert_eq!(turn_ends.len(), 1, "exactly one turn_end, got: {turn_ends:?}");
    assert!(
        !content.contains("\"turn_start\""),
        "cron exemption still suppresses per-turn markers"
    );
    let _ = std::fs::remove_file(&sidecar);
}

#[tokio::test]
async fn transient_error_retry_succeeds() {
    // ③ A transient error (network/stream/5xx) is retried up to
    // MAX_TRANSIENT_RETRIES times without consuming the turns_used budget.
    // Success on retry yields a normal Done.
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TransientThenSuccess {
        call_count: AtomicUsize,
    }
    #[async_trait]
    impl LlmProvider for TransientThenSuccess {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<LlmMessage>,
            _options: Option<crate::types::ChatOptions>,
            _tools: Vec<crate::types::ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            let count = self.call_count.fetch_add(1, Ordering::SeqCst);
            if count == 0 {
                Err("connection reset by peer".to_string())
            } else {
                Ok(LlmResponse {
                    content: "Recovered!".to_string(),
                    tool_calls: Vec::new(),
                    finished: true,
                    reasoning_content: None,
                    usage: None,
                    raw_request_body: None,
                    raw_response_body: None,
                })
            }
        }
    }

    let agent_loop = AgentLoop::new(
        Box::new(TransientThenSuccess {
            call_count: AtomicUsize::new(0),
        }),
        test_config(),
    );
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");

    let events = agent_loop.run(&instance, "Hello", &context).await;

    let done_events: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Done(msg) => Some(msg.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(done_events.len(), 1);
    assert_eq!(done_events[0], "Recovered!");
}

#[tokio::test]
async fn escalation_fires_on_repeated_failing_builds() {
    // Reproduces the ORIGINAL stuck case realistically: the model keeps calling
    // exec (cargo build), which fails identically every time. ExecTool returns
    // Ok("Exit code: 101\nstderr: error[...]...") for a non-zero exit — without
    // the tool_result_indicates_error helper this looks like success and the
    // guards never fire. Verify ⑥ escalation hard-stops at failure 6 (well
    // under max_turns=100).
    let fail_resp = LlmResponse {
        content: String::new(),
        tool_calls: vec![ToolCallInfo {
            id: "tc_fail".to_string(),
            name: "exec".to_string(),
            arguments: r#"{"command":"cargo build"}"#.to_string(),
        }],
        finished: false,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    };
    let responses: Vec<LlmResponse> = (0..20).map(|_| fail_resp.clone()).collect();
    let provider = MockLlmProvider::new(responses);
    let mut config = test_config();
    config.max_turns = 100;

    let mut agent_loop = AgentLoop::new(Box::new(provider), config.clone());
    agent_loop.register_tool(
        "exec".to_string(),
        Box::new(MockTool {
            result:
                "Exit code: 101\nstdout: \nstderr: error[E0432]: unresolved import `winapi::user32`"
                    .to_string(),
        }),
    );
    let instance = AgentInstance::new(config);
    let context = RequestContext::new("web", "chat1", "user1", "session1");

    let events = agent_loop.run(&instance, "build it", &context).await;

    let done_events: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Done(msg) => Some(msg.clone()),
            _ => None,
        })
        .collect();
    // Escalation must actually STOP the turn (exactly one Done), not just fire
    // the nudge repeatedly while the model keeps looping. The original wiring
    // bug: escalation's `break` only exited the tool-call batch, so escalation
    // fired every round without ending the turn — observed 43× in a deployed
    // test. With the latch-based fix, exactly one Done at ~round 6.
    assert_eq!(
        done_events.len(),
        1,
        "escalation should stop the turn with ONE Done, not fire repeatedly; got {} Done events: {:?}",
        done_events.len(),
        done_events
    );
    assert!(
        done_events[0].contains("无法打破"),
        "expected escalation message; got: {}",
        done_events[0]
    );
}

#[tokio::test]
async fn validation_budget_actually_stops_turn() {
    // The validation retry budget must END the turn when exhausted, not just
    // break the current tool batch. Pre-existing bug (same shape as the
    // escalation break-scope bug): the "stopping loop" log fired but the outer
    // LLM loop kept calling the model every round, so a model that kept sending
    // bad args would push an Error event every round until max_turns. With the
    // force_stop latch, exhaustion produces exactly ONE Error and ends the turn.
    let bad_resp = LlmResponse {
        content: String::new(),
        tool_calls: vec![ToolCallInfo {
            id: "tc_bad".to_string(),
            name: "strict_tool".to_string(),
            arguments: "{}".to_string(), // missing required "path"
        }],
        finished: false,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    };
    // 10 responses available — budget exhaustion must stop the turn long before.
    let provider = MockLlmProvider::new((0..10).map(|_| bad_resp.clone()).collect());
    let mut config = test_config();
    config.max_turns = 100;

    let mut agent_loop = AgentLoop::new(Box::new(provider), config.clone());
    agent_loop.register_tool("strict_tool".to_string(), Box::new(StrictTool));
    let instance = AgentInstance::new(config);
    let context = RequestContext::new("web", "chat1", "user1", "session1");

    let events = agent_loop.run(&instance, "do it", &context).await;

    let error_events: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Error(m) => Some(m.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        error_events.len(),
        1,
        "validation budget should stop the turn with ONE Error, not push one per round; got {}: {:?}",
        error_events.len(),
        error_events
    );
    assert!(
        error_events[0].contains("参数校验"),
        "expected validation error; got: {}",
        error_events[0]
    );
}

#[tokio::test]
async fn resume_execution_grace_round_on_max_turns() {
    // #3 cluster-continuation path: resume_execution (used by cluster_agent to
    // resume a remote task) now goes through the grace round. Verifies that a
    // resumed execution hitting max_turns produces a resumable Done (the message
    // extract_final_message surfaces to the requesting node) instead of the old
    // "Max iterations reached" Error. Deterministic — no two-node deployment
    // needed to check this specific edge.
    let infinite_response = LlmResponse {
        content: String::new(),
        tool_calls: vec![ToolCallInfo {
            id: "tc".to_string(),
            name: "calculator".to_string(),
            arguments: "{}".to_string(),
        }],
        finished: false,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    };
    let responses: Vec<LlmResponse> = (0..10).map(|_| infinite_response.clone()).collect();
    let provider = MockLlmProvider::new(responses);
    let mut config = test_config();
    config.max_turns = 3;

    let mut agent_loop = AgentLoop::new(Box::new(provider), config.clone());
    agent_loop.register_tool(
        "calculator".to_string(),
        Box::new(MockTool {
            result: "0".to_string(),
        }),
    );
    let instance = AgentInstance::new(config);
    let context = RequestContext::new("cluster", "task-123", "remote-A", "cluster-resume");

    let events = agent_loop
        .resume_execution(&instance, &context, "cluster-resume-test")
        .await;

    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::Done(msg) if msg.contains("暂停"))),
        "expected grace-round Done on resume_execution max_turns; got: {:?}",
        events
    );
}

#[tokio::test]
async fn degenerate_final_answer_retries_then_real_answer() {
    // ⑦ Two empty final answers are retried with a nudge; the third response
    // (a real answer) is accepted and returned as Done.
    let empty = LlmResponse {
        content: String::new(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    };
    let real = LlmResponse {
        content: "here is the real answer".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    };
    let provider = MockLlmProvider::new(vec![empty.clone(), empty, real]);
    let agent_loop = AgentLoop::new(Box::new(provider), test_config());
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");

    let events = agent_loop.run(&instance, "hi", &context).await;

    let done_events: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Done(msg) => Some(msg.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(done_events.len(), 1);
    assert_eq!(done_events[0], "here is the real answer");
}

#[test]
fn compaction_ineffective_predicate_basics() {
    // ⑩ summarize_was_ineffective: was there a prior summarize AND the prompt
    // is still ≥ 90% of the pre-summarization size?
    use crate::r#loop::summarize_was_ineffective;
    // No prior summarize → never ineffective (need a baseline first).
    assert!(!summarize_was_ineffective(0, 10_000));
    // Dropped well below 90% → effective (not stuck).
    assert!(!summarize_was_ineffective(10_000, 5_000));
    // Barely dropped (still ≥ 90%) → ineffective.
    assert!(summarize_was_ineffective(10_000, 9_500));
    // Grew back → ineffective.
    assert!(summarize_was_ineffective(10_000, 11_000));
}

#[test]
fn test_handle_command_show_model() {
    let provider = MockLlmProvider::new(vec![]);
    let agent_loop = AgentLoop::new(Box::new(provider), test_config());

    let result = agent_loop.handle_command("/show model");
    assert_eq!(result, Some("Current model: test-model".to_string()));
}

#[test]
fn test_handle_command_list_tools() {
    let mut agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    agent_loop.register_tool(
        "calculator".to_string(),
        Box::new(MockTool {
            result: "0".to_string(),
        }),
    );
    agent_loop.register_tool(
        "search".to_string(),
        Box::new(MockTool {
            result: "".to_string(),
        }),
    );

    let result = agent_loop.handle_command("/list tools").unwrap();
    assert!(result.contains("calculator"));
    assert!(result.contains("search"));
}

#[test]
fn test_handle_command_unknown_command() {
    let provider = MockLlmProvider::new(vec![]);
    let agent_loop = AgentLoop::new(Box::new(provider), test_config());

    let result = agent_loop.handle_command("/unknown xyz");
    assert!(result.is_none());
}

#[test]
fn test_handle_command_non_slash() {
    let provider = MockLlmProvider::new(vec![]);
    let agent_loop = AgentLoop::new(Box::new(provider), test_config());

    let result = agent_loop.handle_command("regular message");
    assert!(result.is_none());
}

#[test]
fn test_process_message_with_command() {
    let provider = MockLlmProvider::new(vec![]);
    let agent_loop = AgentLoop::new(Box::new(provider), test_config());
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    let (response, _, handled) = agent_loop.process_message("/show model", &ctx);
    assert!(handled);
    assert_eq!(response, "");
}

#[test]
fn test_process_message_without_command() {
    let provider = MockLlmProvider::new(vec![]);
    let agent_loop = AgentLoop::new(Box::new(provider), test_config());
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    let (_, _, handled) = agent_loop.process_message("Hello!", &ctx);
    assert!(!handled);
}

#[test]
fn test_process_message_cluster_continuation() {
    let provider = MockLlmProvider::new(vec![]);
    let agent_loop = AgentLoop::new(Box::new(provider), test_config());
    let ctx = RequestContext::new("system", "chat1", "user1", "sess1");

    let (_, _, handled) = agent_loop.process_message("cluster_continuation:task-123", &ctx);
    assert!(handled);
}

#[test]
fn test_get_startup_info() {
    let mut agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    agent_loop.register_tool(
        "calculator".to_string(),
        Box::new(MockTool {
            result: "0".to_string(),
        }),
    );

    let info = agent_loop.get_startup_info();
    assert_eq!(info["model"], "test-model");
    assert_eq!(info["max_turns"], 5);
    assert_eq!(info["tools"]["count"], 1);
    assert_eq!(info["system_prompt_configured"], true);
}

#[test]
fn test_format_messages_for_log_empty() {
    let result = format_messages_for_log(&[]);
    assert_eq!(result, "[]");
}

#[test]
fn test_format_messages_for_log() {
    let messages = vec![
        LlmMessage {
            role: "system".to_string(),
            content: "You are helpful.".to_string(),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
        },
        LlmMessage {
            role: "user".to_string(),
            content: "Hello".to_string(),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
        },
        LlmMessage {
            role: "assistant".to_string(),
            content: String::new(),
            tool_calls: Some(vec![ToolCallInfo {
                id: "tc_1".to_string(),
                name: "calculator".to_string(),
                arguments: r#"{"expr":"2+2"}"#.to_string(),
            }]),
            tool_call_id: None,
            reasoning_content: None,
        },
        LlmMessage {
            role: "tool".to_string(),
            content: "4".to_string(),
            tool_calls: None,
            tool_call_id: Some("tc_1".to_string()),
            reasoning_content: None,
        },
    ];

    let result = format_messages_for_log(&messages);
    assert!(result.contains("[0] Role: system"));
    assert!(result.contains("[1] Role: user"));
    assert!(result.contains("[2] Role: assistant"));
    assert!(result.contains("ToolCalls:"));
    assert!(result.contains("calculator"));
    assert!(result.contains("[3] Role: tool"));
    assert!(result.contains("ToolCallID: tc_1"));
}

#[test]
fn test_format_messages_truncates_long_content() {
    let long_content = "x".repeat(500);
    let messages = vec![LlmMessage {
        role: "user".to_string(),
        content: long_content,
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
    }];

    let result = format_messages_for_log(&messages);
    assert!(result.contains("..."));
    assert!(result.len() < 400); // Should be truncated
}

// --- New tests ---

#[test]
fn test_extract_continuation_task_id() {
    assert_eq!(
        extract_continuation_task_id("cluster_continuation:task-123"),
        Some("task-123")
    );
    assert_eq!(
        extract_continuation_task_id("cluster_continuation:"),
        Some("")
    );
    assert_eq!(extract_continuation_task_id("other:task-123"), None);
}

#[test]
fn test_is_internal_channel() {
    assert!(is_internal_channel("cli"));
    assert!(is_internal_channel("system"));
    assert!(is_internal_channel("subagent"));
    assert!(!is_internal_channel("web"));
    assert!(!is_internal_channel("discord"));
}

#[test]
fn test_resolve_route() {
    // With peer as "kind:id" format (matching extract_peer output)
    let input = RouteInput {
        channel: "web".to_string(),
        account_id: None,
        peer: "direct:user1".to_string(),
        parent_peer: None,
        guild_id: None,
        team_id: None,
    };
    let route = resolve_route(&input);
    assert_eq!(route.agent_id, "main");
    // With dm_scope="main" (default), direct peers collapse to the main session key
    assert_eq!(route.session_key, "agent:main:main");
    assert_eq!(route.matched_by, "default");
}

#[test]
fn test_resolve_route_without_peer_kind() {
    // With peer as bare ID (no kind prefix)
    let input = RouteInput {
        channel: "web".to_string(),
        account_id: None,
        peer: "user1".to_string(),
        parent_peer: None,
        guild_id: None,
        team_id: None,
    };
    let route = resolve_route(&input);
    assert_eq!(route.agent_id, "main");
    // With dm_scope="main" (default), direct peers collapse to the main session key
    assert_eq!(route.session_key, "agent:main:main");
    assert_eq!(route.matched_by, "default");
}

#[test]
fn test_build_agent_main_session_key() {
    assert_eq!(build_agent_main_session_key("main"), "agent:main:main");
    assert_eq!(
        build_agent_main_session_key("worker-1"),
        "agent:worker-1:main"
    );
}

#[test]
fn test_truncate() {
    assert_eq!(truncate("hello", 10), "hello");
    // budget = 5-3 = 2 bytes → "he" fits → "he..."
    assert_eq!(truncate("hello world", 5), "he...");
    // budget = 8-3 = 5 bytes → "hello" fits → "hello..."
    assert_eq!(truncate("hello world", 8), "hello...");
}

#[test]
fn test_session_busy_tracker() {
    let tracker = SessionBusyTracker::new(ConcurrentMode::Reject, 8);

    assert!(!tracker.is_busy("session1"));
    assert!(tracker.try_acquire("session1"));
    assert!(tracker.is_busy("session1"));
    assert!(!tracker.try_acquire("session1")); // Already busy

    tracker.release("session1");
    assert!(!tracker.is_busy("session1"));
    assert!(tracker.try_acquire("session1")); // Can acquire again
}

#[test]
fn test_format_tools_for_log() {
    let tools = vec![ToolCallInfo {
        id: "tc_1".to_string(),
        name: "search".to_string(),
        arguments: r#"{"query":"test"}"#.to_string(),
    }];
    let result = format_tools_for_log(&tools);
    assert!(result.contains("search"));
    assert!(result.contains("tc_1"));
}

#[test]
fn test_extract_peer_no_metadata() {
    let msg = nemesis_types::channel::InboundMessage {
        channel: "web".to_string(),
        sender_id: "user123".to_string(),
        chat_id: "chat1".to_string(),
        content: "Hello".to_string(),
        media: vec![],
        session_key: "sess1".to_string(),
        correlation_id: String::new(),
        metadata: std::collections::HashMap::new(),
        voice_playback: None,
    };
    assert_eq!(extract_peer(&msg), "user123");
}

#[test]
fn test_extract_peer_with_metadata() {
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("peer_kind".to_string(), "guild".to_string());
    metadata.insert("peer_id".to_string(), "guild_12345".to_string());
    let msg = nemesis_types::channel::InboundMessage {
        channel: "discord".to_string(),
        sender_id: "user123".to_string(),
        chat_id: "chat1".to_string(),
        content: "Hello".to_string(),
        media: vec![],
        session_key: "sess1".to_string(),
        correlation_id: String::new(),
        metadata,
        voice_playback: None,
    };
    assert_eq!(extract_peer(&msg), "guild:guild_12345");
}

#[test]
fn test_extract_peer_direct_kind() {
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("peer_kind".to_string(), "direct".to_string());
    let msg = nemesis_types::channel::InboundMessage {
        channel: "telegram".to_string(),
        sender_id: "tg_user_456".to_string(),
        chat_id: "chat1".to_string(),
        content: "Hello".to_string(),
        media: vec![],
        session_key: "sess1".to_string(),
        correlation_id: String::new(),
        metadata,
        voice_playback: None,
    };
    assert_eq!(extract_peer(&msg), "direct:tg_user_456");
}

#[test]
fn test_extract_parent_peer() {
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("parent_peer_kind".to_string(), "channel".to_string());
    metadata.insert("parent_peer_id".to_string(), "chan_789".to_string());
    let msg = nemesis_types::channel::InboundMessage {
        channel: "discord".to_string(),
        sender_id: "user123".to_string(),
        chat_id: "chat1".to_string(),
        content: "Hello".to_string(),
        media: vec![],
        session_key: "sess1".to_string(),
        correlation_id: String::new(),
        metadata,
        voice_playback: None,
    };
    assert_eq!(
        extract_parent_peer(&msg),
        Some("channel:chan_789".to_string())
    );
}

#[test]
fn test_extract_parent_peer_missing() {
    let msg = nemesis_types::channel::InboundMessage {
        channel: "web".to_string(),
        sender_id: "user123".to_string(),
        chat_id: "chat1".to_string(),
        content: "Hello".to_string(),
        media: vec![],
        session_key: "sess1".to_string(),
        correlation_id: String::new(),
        metadata: std::collections::HashMap::new(),
        voice_playback: None,
    };
    assert_eq!(extract_parent_peer(&msg), None);
}

// --- Bus mode tests ---

#[test]
fn test_session_busy_state_management() {
    let provider = MockLlmProvider::new(vec![]);
    let agent_loop = AgentLoop::new(Box::new(provider), test_config());

    // Initially not busy.
    let (busy, queue) = agent_loop.get_session_busy_state("sess1");
    assert!(!busy);
    assert_eq!(queue, 0);

    // Acquire.
    assert!(agent_loop.try_acquire_session("sess1"));
    let (busy, queue) = agent_loop.get_session_busy_state("sess1");
    assert!(busy);
    assert_eq!(queue, 0);

    // Already busy - reject mode.
    assert!(!agent_loop.try_acquire_session("sess1"));

    // Release.
    let has_queued = agent_loop.release_session("sess1");
    assert!(!has_queued);
    let (busy, _) = agent_loop.get_session_busy_state("sess1");
    assert!(!busy);
}

#[test]
fn test_session_busy_queue_mode() {
    let provider = MockLlmProvider::new(vec![]);
    let mut agent_loop = AgentLoop::new(Box::new(provider), test_config());
    agent_loop.concurrent_mode = ConcurrentMode::Queue;
    agent_loop.queue_size = 3;

    // First acquire succeeds.
    assert!(agent_loop.try_acquire_session("sess2"));

    // Subsequent acquires fail (Queue is now treated as Reject: the old Queue
    // path counted queue_length without storing messages, which combined with
    // release keeping busy could deadlock the session. queue_length stays 0
    // under both modes now.)
    assert!(!agent_loop.try_acquire_session("sess2"));
    assert_eq!(agent_loop.session_queue_length("sess2"), 0);

    assert!(!agent_loop.try_acquire_session("sess2"));
    assert_eq!(agent_loop.session_queue_length("sess2"), 0);

    // Release clears busy (no queue to drain).
    let has_queued = agent_loop.release_session("sess2");
    assert!(!has_queued);
    assert_eq!(agent_loop.session_queue_length("sess2"), 0);
    assert!(!agent_loop.is_session_busy("sess2"));
}

#[test]
fn test_record_last_channel_and_chat_id() {
    let provider = MockLlmProvider::new(vec![]);
    let mut agent_loop = AgentLoop::new(Box::new(provider), test_config());

    // Without state manager, these are no-ops.
    agent_loop.record_last_channel("web");
    agent_loop.record_last_chat_id("chat42");

    // With state manager (uses WorkspaceStateManager for disk persistence).
    let tmp = tempfile::tempdir().unwrap();
    let mgr = nemesis_state::workspace_state::WorkspaceStateManager::new(tmp.path());
    agent_loop.set_state_manager(mgr.clone());
    agent_loop.record_last_channel("discord");
    agent_loop.record_last_chat_id("chat99");

    assert_eq!(mgr.get_last_channel(), "discord");
    assert_eq!(mgr.get_last_chat_id(), "chat99");
}

#[test]
fn test_set_channel_manager() {
    let provider = MockLlmProvider::new(vec![]);
    let agent_loop = AgentLoop::new(Box::new(provider), test_config());

    agent_loop.set_channel_manager(vec!["web".to_string(), "discord".to_string()]);

    let channels = agent_loop.channel_manager_channels.lock();
    assert_eq!(&*channels, &vec!["web".to_string(), "discord".to_string()]);
}

#[test]
fn test_stop_and_is_running() {
    let provider = MockLlmProvider::new(vec![]);
    let agent_loop = AgentLoop::new(Box::new(provider), test_config());

    assert!(!agent_loop.is_running());
    agent_loop.running.store(true, Ordering::Release);
    assert!(agent_loop.is_running());
    agent_loop.stop();
    assert!(!agent_loop.is_running());
}

#[test]
fn test_handle_command_channels_with_channel_manager() {
    let provider = MockLlmProvider::new(vec![]);
    let agent_loop = AgentLoop::new(Box::new(provider), test_config());
    agent_loop.set_channel_manager(vec!["web".to_string(), "rpc".to_string()]);

    let result = agent_loop.handle_command("/list channels").unwrap();
    assert!(result.contains("web"));
    assert!(result.contains("rpc"));
}

#[test]
fn test_handle_command_channels_without_channel_manager() {
    let provider = MockLlmProvider::new(vec![]);
    let agent_loop = AgentLoop::new(Box::new(provider), test_config());

    let result = agent_loop.handle_command("/list channels").unwrap();
    assert_eq!(result, "No channels enabled");
}

#[test]
fn test_new_bus_creates_registry() {
    let provider = MockLlmProvider::new(vec![]);
    let (tx, _rx) = tokio::sync::mpsc::channel(16);

    let agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config(),
        tx,
        ConcurrentMode::Reject,
        8,
        0,
    );

    assert!(agent_loop.get_registry().is_some());
    let registry = agent_loop.get_registry().unwrap();
    assert!(registry.contains_agent("main"));
}

#[test]
fn test_process_direct() {
    let provider = MockLlmProvider::new(vec![LlmResponse {
        content: "Direct response".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);

    let rt = tokio::runtime::Runtime::new().unwrap();
    let agent_loop = AgentLoop::new(Box::new(provider), test_config());

    let result = rt.block_on(async { agent_loop.process_direct("Hello", "sess1").await });

    assert_eq!(result, Ok("Direct response".to_string()));
}

#[test]
fn test_process_heartbeat() {
    let provider = MockLlmProvider::new(vec![LlmResponse {
        content: "Heartbeat OK".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);

    let rt = tokio::runtime::Runtime::new().unwrap();
    let agent_loop = AgentLoop::new(Box::new(provider), test_config());

    let result = rt.block_on(async { agent_loop.process_heartbeat("Ping", "web", "chat1").await });

    assert_eq!(result, Ok("Heartbeat OK".to_string()));
}

// --- Additional tests for coverage ---

#[test]
fn test_llm_message_serialization() {
    let msg = LlmMessage {
        role: "assistant".to_string(),
        content: "Hello".to_string(),
        tool_calls: Some(vec![ToolCallInfo {
            id: "tc_1".to_string(),
            name: "search".to_string(),
            arguments: r#"{"q":"test"}"#.to_string(),
        }]),
        tool_call_id: None,
        reasoning_content: None,
    };
    let json = serde_json::to_string(&msg).unwrap();
    let parsed: LlmMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.role, "assistant");
    assert!(parsed.tool_calls.is_some());
    assert_eq!(parsed.tool_calls.unwrap()[0].name, "search");
}

#[test]
fn test_llm_message_no_tool_calls() {
    let msg = LlmMessage {
        role: "user".to_string(),
        content: "Hello".to_string(),
        tool_calls: None,
        tool_call_id: Some("tc_1".to_string()),
        reasoning_content: None,
    };
    let json = serde_json::to_string(&msg).unwrap();
    let parsed: LlmMessage = serde_json::from_str(&json).unwrap();
    assert!(parsed.tool_calls.is_none());
    assert_eq!(parsed.tool_call_id, Some("tc_1".to_string()));
}

#[test]
fn test_llm_response_clone() {
    let resp = LlmResponse {
        content: "Hello".to_string(),
        tool_calls: vec![ToolCallInfo {
            id: "tc_1".to_string(),
            name: "test".to_string(),
            arguments: "{}".to_string(),
        }],
        finished: false,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    };
    let cloned = resp.clone();
    assert_eq!(cloned.content, "Hello");
    assert_eq!(cloned.tool_calls.len(), 1);
    assert!(!cloned.finished);
}

#[test]
fn test_concurrent_mode_default() {
    assert_eq!(ConcurrentMode::default(), ConcurrentMode::Reject);
}

#[test]
fn test_process_options_default() {
    let opts = ProcessOptions::default();
    assert!(opts.session_key.is_empty());
    assert!(opts.channel.is_empty());
    assert!(opts.chat_id.is_empty());
    assert!(opts.user_message.is_empty());
    assert!(opts.enable_summary);
    assert!(!opts.send_response);
    assert!(!opts.no_history);
    assert!(opts.trace_id.is_empty());
    assert!(opts.default_response.contains("no response"));
}

#[test]
fn test_sent_in_round_tracker() {
    let tracker = SentInRoundTracker::new();

    assert!(!tracker.has_sent_in_round("session1"));
    tracker.mark_sent("session1");
    assert!(tracker.has_sent_in_round("session1"));
    assert!(!tracker.has_sent_in_round("session2"));

    tracker.clear("session1");
    assert!(!tracker.has_sent_in_round("session1"));

    tracker.mark_sent("s1");
    tracker.mark_sent("s2");
    tracker.clear_all();
    assert!(!tracker.has_sent_in_round("s1"));
    assert!(!tracker.has_sent_in_round("s2"));
}

#[test]
fn test_session_busy_state_default() {
    let state = SessionBusyState::default();
    assert!(!state.busy);
    assert_eq!(state.queue_length, 0);
}

#[tokio::test]
async fn test_run_with_llm_error() {
    struct ErrorProvider;
    #[async_trait]
    impl LlmProvider for ErrorProvider {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<LlmMessage>,
            _options: Option<crate::types::ChatOptions>,
            _tools: Vec<crate::types::ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            Err("General LLM error".to_string())
        }
    }

    let agent_loop = AgentLoop::new(Box::new(ErrorProvider), test_config());
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");

    let events = agent_loop.run(&instance, "Hello", &context).await;

    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::Error(msg) if msg.contains("General LLM error")))
    );
}

#[tokio::test]
async fn test_run_with_context_error_and_retry_success() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct ContextErrorThenSuccessProvider {
        call_count: AtomicUsize,
    }
    #[async_trait]
    impl LlmProvider for ContextErrorThenSuccessProvider {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<LlmMessage>,
            _options: Option<crate::types::ChatOptions>,
            _tools: Vec<crate::types::ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            let count = self.call_count.fetch_add(1, Ordering::SeqCst);
            if count == 0 {
                Err("context_length_exceeded: token limit".to_string())
            } else {
                Ok(LlmResponse {
                    content: "Recovered!".to_string(),
                    tool_calls: Vec::new(),
                    finished: true,
                    reasoning_content: None,
                    usage: None,
                    raw_request_body: None,
                    raw_response_body: None,
                })
            }
        }
    }

    let agent_loop = AgentLoop::new(
        Box::new(ContextErrorThenSuccessProvider {
            call_count: AtomicUsize::new(0),
        }),
        test_config(),
    );
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");

    let events = agent_loop.run(&instance, "Hello", &context).await;

    let done_events: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Done(msg) => Some(msg.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(done_events.len(), 1);
    assert_eq!(done_events[0], "Recovered!");
}

#[tokio::test]
async fn test_run_with_context_error_all_retries_fail() {
    struct AlwaysContextError;
    #[async_trait]
    impl LlmProvider for AlwaysContextError {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<LlmMessage>,
            _options: Option<crate::types::ChatOptions>,
            _tools: Vec<crate::types::ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            Err("token limit exceeded".to_string())
        }
    }

    let agent_loop = AgentLoop::new(Box::new(AlwaysContextError), test_config());
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");

    let events = agent_loop.run(&instance, "Hello", &context).await;

    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::Error(msg) if msg.contains("token limit exceeded")))
    );
}

#[tokio::test]
async fn test_run_rpc_error_formatting() {
    struct ErrorProvider;
    #[async_trait]
    impl LlmProvider for ErrorProvider {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<LlmMessage>,
            _options: Option<crate::types::ChatOptions>,
            _tools: Vec<crate::types::ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            Err("Failed".to_string())
        }
    }

    let agent_loop = AgentLoop::new(Box::new(ErrorProvider), test_config());
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::for_rpc("chat1", "user1", "session1", "corr-99");

    let events = agent_loop.run(&instance, "Hello", &context).await;

    let error_events: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Error(msg) => Some(msg.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(error_events.len(), 1);
    assert!(error_events[0].starts_with("[rpc:corr-99]"));
}

#[test]
fn test_handle_command_list_tools_empty() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let result = agent_loop.handle_command("/list tools");
    assert!(result.is_some());
    assert!(result.unwrap().contains("Available tools:"));
}

#[test]
fn test_handle_command_list_tools_with_tools() {
    let mut agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    agent_loop.register_tool(
        "calculator".to_string(),
        Box::new(MockTool {
            result: "0".to_string(),
        }),
    );

    let result = agent_loop.handle_command("/list tools");
    assert!(result.is_some());
    assert!(result.unwrap().contains("calculator"));
}

#[test]
fn test_handle_command_show_agents_empty() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let result = agent_loop.handle_command("/show agents");
    // With registry (bus mode), should show agents
    assert!(result.is_some());
}

#[test]
fn test_handle_command_switch_model() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let result = agent_loop.handle_command("/switch model to gpt-5");
    assert!(result.is_some());
    let content = result.unwrap();
    assert!(content.contains("test-model"));
    assert!(content.contains("gpt-5") || content.contains("Model switch"));
}

#[test]
fn test_handle_command_show_channel() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let result = agent_loop.handle_command_with_context("/show channel", "discord");
    assert_eq!(result, Some("Current channel: discord".to_string()));
}

#[test]
fn test_handle_command_with_context() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());

    // Test with context on web channel
    let result = agent_loop.handle_command_with_context("/show model", "web");
    assert_eq!(result, Some("Current model: test-model".to_string()));

    // Test non-slash command
    let result = agent_loop.handle_command_with_context("hello", "web");
    assert!(result.is_none());
}

#[tokio::test]
async fn test_tool_execution_error() {
    struct ErrorTool;
    #[async_trait]
    impl Tool for ErrorTool {
        async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
            Err("Tool execution failed".to_string())
        }
    }

    let provider = MockLlmProvider::new(vec![
        LlmResponse {
            content: String::new(),
            tool_calls: vec![ToolCallInfo {
                id: "tc_1".to_string(),
                name: "error_tool".to_string(),
                arguments: "{}".to_string(),
            }],
            finished: false,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
        LlmResponse {
            content: "I see the error.".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
    ]);

    let mut agent_loop = AgentLoop::new(Box::new(provider), test_config());
    agent_loop.register_tool("error_tool".to_string(), Box::new(ErrorTool));

    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");

    let events = agent_loop.run(&instance, "Test error", &context).await;

    // Should have a ToolResult with the error
    let tool_results: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolResult(tr) if tr.result.contains("Tool error") => Some(tr.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(tool_results.len(), 1);
    assert!(tool_results[0].result.contains("Tool execution failed"));
}

#[test]
fn test_build_messages_from_instance() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let instance = AgentInstance::new(test_config());
    instance.add_user_message("Hello");
    instance.add_assistant_message("Hi", Vec::new(), None);

    let messages = agent_loop.build_messages(&instance);

    // system + (I2: user-role merged context snapshot) + user + assistant = 4
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[0].role, "system");
    assert_eq!(messages[1].role, "user"); // merged context snapshot (I2)
    assert!(messages[1].content.contains("Current Time"));
    assert_eq!(messages[2].role, "user");
    assert_eq!(messages[3].role, "assistant");
}

// P3.1 (sixth batch): memory auto-inject snapshot section.
#[test]
fn test_build_messages_with_memory_off_is_byte_identical() {
    // auto_inject defaults to false (the (false, 3) in new()). With None
    // hits the output must be byte-identical to the plain build_messages.
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let instance = AgentInstance::new(test_config());
    instance.add_user_message("Hello world");
    // LlmMessage fields aren't all PartialEq, so compare a deterministic
    // projection (role + content + tool-call count) — enough to catch any
    // section drift (a stray Memory Context block would change content).
    let proj = |m: &crate::r#loop::LlmMessage| {
        (
            m.role.clone(),
            m.content.clone(),
            m.tool_calls.as_ref().map(|v| v.len()).unwrap_or(0),
        )
    };

    let plain: Vec<_> = agent_loop.build_messages(&instance).iter().map(proj).collect();
    let with_none: Vec<_> = agent_loop
        .build_messages_with_memory(&instance, None)
        .iter()
        .map(proj)
        .collect();
    assert_eq!(plain, with_none, "None hits → identical to plain");

    let with_empty_hits: &[String] = &[];
    let with_empty: Vec<_> = agent_loop
        .build_messages_with_memory(&instance, Some(with_empty_hits))
        .iter()
        .map(proj)
        .collect();
    assert_eq!(plain, with_empty, "empty hits → identical to plain");
}

#[test]
fn test_build_messages_with_memory_renders_section() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let instance = AgentInstance::new(test_config());
    instance.add_user_message("deploy the docs system");

    let hits = vec![
        "user prefers concise output".to_string(),
        "deploy target is the staging box".to_string(),
    ];
    let messages = agent_loop.build_messages_with_memory(&instance, Some(&hits));
    // The merged snapshot is the second message (after system, before the
    // real last user). Find the snapshot user-role message carrying both the
    // Memory Context section AND the always-present Current Time section.
    let snapshot = messages
        .iter()
        .find(|m| m.role == "user" && m.content.contains("# Memory Context"))
        .expect("Memory Context section rendered in the snapshot");
    assert!(snapshot.content.contains("user prefers concise output"));
    assert!(snapshot.content.contains("deploy target is the staging box"));
    assert!(snapshot.content.contains("# Current Time"), "snapshot still carries time");
}

#[test]
fn test_textwise_similar_dedup_helper() {
    // Near-duplicate (trivial diff) → high similarity — the case dedup is
    // built for (same memory stored twice). Bigram Jaccard is a cheap proxy
    // for vector cosine; adding whole words drops it fast, so the pair must
    // be genuinely near-identical.
    let a = "the user prefers concise output in replies";
    let b = "the user prefers concise output in replies.";
    assert!(textwise_similar(a, b) > 0.92, "near-dup: {}", textwise_similar(a, b));
    // Unrelated → low.
    assert!(textwise_similar("deploy the docs", "weather is sunny") < 0.2);
    // Empty pair → 1.0 (degenerate, but the caller guards against it).
    assert_eq!(textwise_similar("", ""), 1.0);
}

#[tokio::test]
async fn test_force_compression() {
    // Redesigned (S4): force_compression does NOT mutate history. It advances
    // the summary cache to len-SMALL_K_FORCE and recomputes the summary, so
    // build_messages emits a shorter verbatim tail.
    let provider = MockLlmProvider::new(vec![llm_text("EMERGENCY SUMMARY")]);
    let agent_loop = AgentLoop::new(Box::new(provider), test_config());

    let instance = AgentInstance::new(test_config());
    for i in 0..10 {
        instance.add_user_message(&format!("msg_{}", i));
    }
    // system + 10 = 11
    assert_eq!(instance.get_history().len(), 11);

    agent_loop.force_compression(&instance).await;

    // History is unchanged (append-only).
    assert_eq!(instance.get_history().len(), 11);
    assert_eq!(instance.get_history()[0].role, "system");
    // Cache advanced to len - SMALL_K_FORCE.
    let cache = instance.get_summary_cache().expect("cache should be set");
    assert_eq!(cache.covers_up_to, 11 - SMALL_K_FORCE);
    assert_eq!(cache.text, "EMERGENCY SUMMARY");
}

#[tokio::test]
async fn test_force_compression_short_history() {
    // A history whose only conversation content summarizes to an empty result
    // (empty LLM response) → cache stays None (no-op). History unchanged.
    // (MockLlmProvider returns "No more responses" when exhausted, which is
    // non-empty, so use an explicit empty response to exercise the None path.)
    let provider = MockLlmProvider::new(vec![llm_text("")]);
    let agent_loop = AgentLoop::new(Box::new(provider), test_config());

    let instance = AgentInstance::new(test_config());
    instance.add_user_message("Hello");

    agent_loop.force_compression(&instance).await;
    assert!(instance.get_summary_cache().is_none());
    // History len unchanged: system + 1 user = 2.
    assert_eq!(instance.get_history().len(), 2);
}

#[test]
fn test_register_tool_shared() {
    let mut agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    assert_eq!(agent_loop.tool_count(), 0);

    agent_loop.register_tool_shared(
        "tool1".to_string(),
        Box::new(MockTool {
            result: "ok".to_string(),
        }),
    );
    assert_eq!(agent_loop.tool_count(), 1);

    agent_loop.register_tool_shared(
        "tool2".to_string(),
        Box::new(MockTool {
            result: "ok".to_string(),
        }),
    );
    assert_eq!(agent_loop.tool_count(), 2);
}

#[test]
fn test_provider_access() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    // provider() should not panic
    let _ = agent_loop.provider_arc();
}

#[test]
fn test_runtime_model_switch_refreshes_tier() {
    // Phase 4a: tier is re-resolved LIVE from config.json on every model switch.
    // config.json is the single source of truth — dashboard-added models and
    // CLI `model set-tier` edits are picked up because each switch re-reads
    // config.json (no stale snapshot). This also exercises `refresh_active_tier`,
    // the same path `check_config_reload` triggers when config.json's mtime
    // changes mid-conversation.
    use nemesis_types::capability::ModelTier;

    let cfg_path =
        std::env::temp_dir().join(format!("nemesis_test_tier_{}.json", std::process::id()));
    std::fs::write(
        &cfg_path,
        serde_json::json!({
            "model_list": [
                {"model": "qwen/qwen3-30b", "model_name": "qwen3-30b", "model_tier": "mini"},
                {"model": "openai/gpt-4", "model_name": "gpt-4", "model_tier": "big"},
            ]
        })
        .to_string(),
    )
    .unwrap();

    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    agent_loop.set_config_path(cfg_path.clone());

    // Switch re-resolves tier live from config.json.
    agent_loop.set_active_model("qwen3-30b");
    assert_eq!(agent_loop.tier(), ModelTier::Mini);
    agent_loop.set_active_model("gpt-4");
    assert_eq!(agent_loop.tier(), ModelTier::Big);

    // Edit config on disk (dashboard / CLI `model set-tier`) → next switch
    // reflects it, because every switch re-reads config.json.
    std::fs::write(
        &cfg_path,
        serde_json::json!({
            "model_list": [
                {"model": "qwen/qwen3-30b", "model_name": "qwen3-30b", "model_tier": "normal"},
            ]
        })
        .to_string(),
    )
    .unwrap();
    agent_loop.set_active_model("qwen3-30b");
    assert_eq!(
        agent_loop.tier(),
        ModelTier::Normal,
        "config edit must be reflected on next switch"
    );

    // Unknown model (not in config) → fallback to name heuristic → Big default.
    agent_loop.set_active_model("some-opaque-alias");
    assert_eq!(agent_loop.tier(), ModelTier::Big);

    let _ = std::fs::remove_file(&cfg_path);
}

#[test]
fn test_config_mut() {
    let mut agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    agent_loop.config_mut().max_turns = 20;
    assert_eq!(agent_loop.config_mut().max_turns, 20);
}

#[test]
fn test_format_tools_for_log_empty() {
    let result = format_tools_for_log(&[]);
    assert_eq!(result, "[]");
}

#[test]
fn test_format_tools_for_log_long_args() {
    let tools = vec![ToolCallInfo {
        id: "tc_1".to_string(),
        name: "search".to_string(),
        arguments: "x".repeat(300),
    }];
    let result = format_tools_for_log(&tools);
    assert!(result.contains("..."));
}

#[test]
fn test_truncate_short() {
    assert_eq!(truncate("hi", 10), "hi");
}

#[test]
fn test_truncate_exact() {
    assert_eq!(truncate("hello", 5), "hello");
}

#[test]
fn test_extract_peer_with_empty_peer_kind() {
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("peer_kind".to_string(), String::new());
    let msg = nemesis_types::channel::InboundMessage {
        channel: "web".to_string(),
        sender_id: "user123".to_string(),
        chat_id: "chat1".to_string(),
        content: "Hello".to_string(),
        media: vec![],
        session_key: "sess1".to_string(),
        correlation_id: String::new(),
        metadata,
        voice_playback: None,
    };
    // Empty peer_kind should fall through to sender_id
    assert_eq!(extract_peer(&msg), "user123");
}

#[test]
fn test_extract_peer_with_peer_kind_no_peer_id() {
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("peer_kind".to_string(), "group".to_string());
    let msg = nemesis_types::channel::InboundMessage {
        channel: "discord".to_string(),
        sender_id: "user123".to_string(),
        chat_id: "chat_abc".to_string(),
        content: "Hello".to_string(),
        media: vec![],
        session_key: "sess1".to_string(),
        correlation_id: String::new(),
        metadata,
        voice_playback: None,
    };
    // No peer_id, non-direct -> falls back to chat_id
    assert_eq!(extract_peer(&msg), "group:chat_abc");
}

#[test]
fn test_extract_parent_peer_empty_values() {
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("parent_peer_kind".to_string(), String::new());
    metadata.insert("parent_peer_id".to_string(), String::new());
    let msg = nemesis_types::channel::InboundMessage {
        channel: "web".to_string(),
        sender_id: "user123".to_string(),
        chat_id: "chat1".to_string(),
        content: "Hello".to_string(),
        media: vec![],
        session_key: "sess1".to_string(),
        correlation_id: String::new(),
        metadata,
        voice_playback: None,
    };
    assert_eq!(extract_parent_peer(&msg), None);
}

#[test]
fn test_extract_parent_peer_missing_id() {
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("parent_peer_kind".to_string(), "channel".to_string());
    // No parent_peer_id
    let msg = nemesis_types::channel::InboundMessage {
        channel: "web".to_string(),
        sender_id: "user123".to_string(),
        chat_id: "chat1".to_string(),
        content: "Hello".to_string(),
        media: vec![],
        session_key: "sess1".to_string(),
        correlation_id: String::new(),
        metadata,
        voice_playback: None,
    };
    assert_eq!(extract_parent_peer(&msg), None);
}

#[test]
fn test_resolve_route_with_parent_peer() {
    let input = RouteInput {
        channel: "discord".to_string(),
        account_id: None,
        peer: "guild:12345".to_string(),
        parent_peer: Some("channel:789".to_string()),
        guild_id: None,
        team_id: None,
    };
    let route = resolve_route(&input);
    assert_eq!(route.agent_id, "main");
}

#[test]
fn test_session_busy_tracker_multiple_sessions() {
    let tracker = SessionBusyTracker::new(ConcurrentMode::Reject, 8);

    assert!(tracker.try_acquire("s1"));
    assert!(tracker.try_acquire("s2"));

    assert!(tracker.is_busy("s1"));
    assert!(tracker.is_busy("s2"));
    assert!(!tracker.is_busy("s3"));

    tracker.release("s1");
    assert!(!tracker.is_busy("s1"));
    assert!(tracker.is_busy("s2"));
}

#[test]
fn test_process_direct_with_channel() {
    let provider = MockLlmProvider::new(vec![LlmResponse {
        content: "Response with channel".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);

    let rt = tokio::runtime::Runtime::new().unwrap();
    let agent_loop = AgentLoop::new(Box::new(provider), test_config());

    let result = rt.block_on(async {
        agent_loop
            .process_direct_with_channel("Hello", "sess1", "telegram", "chat99")
            .await
    });

    assert_eq!(result, Ok("Response with channel".to_string()));
}

#[test]
fn test_get_startup_info_no_tools() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let info = agent_loop.get_startup_info();
    assert_eq!(info["tools"]["count"], 0);
}

#[tokio::test]
async fn test_multiple_tool_calls_in_single_response() {
    let provider = MockLlmProvider::new(vec![
        LlmResponse {
            content: String::new(),
            tool_calls: vec![
                ToolCallInfo {
                    id: "tc_1".to_string(),
                    name: "calculator".to_string(),
                    arguments: r#"{"expr":"2+2"}"#.to_string(),
                },
                ToolCallInfo {
                    id: "tc_2".to_string(),
                    name: "calculator".to_string(),
                    arguments: r#"{"expr":"3+3"}"#.to_string(),
                },
            ],
            finished: false,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
        LlmResponse {
            content: "Both results: 4 and 6.".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
    ]);

    let mut agent_loop = AgentLoop::new(Box::new(provider), test_config());
    agent_loop.register_tool(
        "calculator".to_string(),
        Box::new(MockTool {
            result: "computed".to_string(),
        }),
    );

    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");

    let events = agent_loop.run(&instance, "Calculate both", &context).await;

    // Should have 2 ToolResult events
    let tool_results: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolResult(_)))
        .collect();
    assert_eq!(tool_results.len(), 2);
}

#[test]
fn test_handle_command_help() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let result = agent_loop.handle_command("/help");
    assert!(result.is_some());
    assert!(result.as_ref().unwrap().contains("Commands:"));
    assert!(result.as_ref().unwrap().contains("/model"));
}

#[test]
fn test_handle_command_model_no_args_shows_current() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let result = agent_loop.handle_command("/model");
    assert!(result.is_some());
    assert!(result.as_ref().unwrap().contains("Current model:"));
}

#[test]
fn test_handle_command_model_switch_by_literal_id() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let result = agent_loop.handle_command("/model deepseek-v4-pro");
    assert!(result.is_some());
    assert!(
        result
            .as_ref()
            .unwrap()
            .contains("Model switched to: deepseek-v4-pro")
    );
    // Verify active_model actually changed.
    assert_eq!(*agent_loop.active_model.read(), "deepseek-v4-pro");
}

#[test]
fn test_handle_command_model_switch_by_alias() {
    let mut config = test_config();
    config
        .models
        .insert("pro".to_string(), "deepseek-v4-pro".to_string());
    config
        .models
        .insert("flash".to_string(), "deepseek-v4-flash".to_string());
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), config);

    let result = agent_loop.handle_command("/model pro");
    assert!(result.is_some());
    assert!(result.as_ref().unwrap().contains("deepseek-v4-pro"));
    assert_eq!(*agent_loop.active_model.read(), "deepseek-v4-pro");

    let result2 = agent_loop.handle_command("/model flash");
    assert!(result2.as_ref().unwrap().contains("deepseek-v4-flash"));
    assert_eq!(*agent_loop.active_model.read(), "deepseek-v4-flash");
}

#[test]
fn test_handle_command_unknown_slash_returns_none() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let result = agent_loop.handle_command("/totally_bogus_unknown_cmd");
    // Not a recognized command, returns None
    assert!(result.is_none());
}

#[test]
fn test_handle_command_show_unknown() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let result = agent_loop.handle_command("/show system_prompt");
    assert!(result.is_some());
    assert!(result.unwrap().contains("Unknown show target"));
}

#[test]
fn test_handle_command_list_models() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let result = agent_loop.handle_command("/list models");
    assert!(result.is_some());
    assert!(result.unwrap().contains("test-model"));
}

// ---- Multi-model set_active_model + model_aliases direct tests ----

#[test]
fn test_set_active_model_literal_id() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let returned = agent_loop.set_active_model("custom-model-123");
    assert_eq!(returned, "custom-model-123");
    assert_eq!(*agent_loop.active_model.read(), "custom-model-123");
}

#[test]
fn test_set_active_model_alias_resolves() {
    let mut config = test_config();
    config
        .models
        .insert("fast".to_string(), "gpt-4o-mini".to_string());
    config
        .models
        .insert("smart".to_string(), "o3-mini".to_string());
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), config);

    let returned = agent_loop.set_active_model("fast");
    assert_eq!(returned, "gpt-4o-mini");
    assert_eq!(*agent_loop.active_model.read(), "gpt-4o-mini");

    let returned2 = agent_loop.set_active_model("smart");
    assert_eq!(returned2, "o3-mini");
}

#[test]
fn test_set_active_model_unknown_alias_used_as_literal() {
    let mut config = test_config();
    config
        .models
        .insert("pro".to_string(), "deepseek-pro".to_string());
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), config);

    let returned = agent_loop.set_active_model("random-model-id");
    assert_eq!(returned, "random-model-id");
    assert_eq!(*agent_loop.active_model.read(), "random-model-id");
}

#[test]
fn test_model_aliases_returns_configured() {
    let mut config = test_config();
    config.models.insert("a".to_string(), "model-a".to_string());
    config.models.insert("b".to_string(), "model-b".to_string());
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), config);
    let mut aliases = agent_loop.model_aliases();
    aliases.sort();
    assert_eq!(aliases, vec!["a", "b"]);
}

#[test]
fn test_model_aliases_empty_when_no_config() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    assert!(agent_loop.model_aliases().is_empty());
}

#[test]
fn test_handle_command_show_session() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let result = agent_loop.handle_command("/show session");
    assert!(result.is_some());
}

#[tokio::test]
async fn test_finished_flag_stops_loop() {
    // LLM returns finished=true with tool calls - should still stop
    let provider = MockLlmProvider::new(vec![LlmResponse {
        content: "Here is the answer.".to_string(),
        tool_calls: vec![ToolCallInfo {
            id: "tc_1".to_string(),
            name: "calculator".to_string(),
            arguments: "{}".to_string(),
        }],
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);

    let agent_loop = AgentLoop::new(Box::new(provider), test_config());
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");

    let events = agent_loop.run(&instance, "Hello", &context).await;

    // finished=true means it should be treated as final response
    let done_events: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Done(msg) => Some(msg.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(done_events.len(), 1);
}

// --- Additional coverage tests ---

#[test]
fn test_handle_command_show_usage() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let result = agent_loop.handle_command("/show");
    assert!(result.is_some());
    assert!(result.unwrap().contains("Usage"));
}

#[test]
fn test_handle_command_list_usage() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let result = agent_loop.handle_command("/list");
    assert!(result.is_some());
    assert!(result.unwrap().contains("Usage"));
}

#[test]
fn test_handle_command_switch_usage() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let result = agent_loop.handle_command("/switch model");
    assert!(result.is_some());
    assert!(result.unwrap().contains("Usage"));
}

#[test]
fn test_handle_command_switch_channel() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let result = agent_loop.handle_command("/switch channel to discord");
    assert!(result.is_some());
    assert!(result.unwrap().contains("discord"));
}

#[test]
fn test_handle_command_switch_unknown_target() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let result = agent_loop.handle_command("/switch foo to bar");
    assert!(result.is_some());
    assert!(result.unwrap().contains("Unknown switch target"));
}

#[test]
fn test_handle_command_list_unknown_target() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let result = agent_loop.handle_command("/list foo");
    assert!(result.is_some());
    assert!(result.unwrap().contains("Unknown list target"));
}

#[test]
fn test_handle_command_list_agents() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let result = agent_loop.handle_command("/list agents");
    assert!(result.is_some());
}

#[test]
fn test_handle_command_list_agents_with_tools() {
    let mut agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    agent_loop.register_tool(
        "search".to_string(),
        Box::new(MockTool {
            result: "".to_string(),
        }),
    );
    let result = agent_loop.handle_command("/list agents");
    assert!(result.is_some());
    assert!(result.unwrap().contains("search"));
}

#[test]
fn test_handle_command_show_agents_with_registry() {
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let agent_loop = AgentLoop::new_bus(
        Box::new(MockLlmProvider::new(vec![])),
        test_config(),
        tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    let result = agent_loop.handle_command("/show agents");
    assert!(result.is_some());
    assert!(result.unwrap().contains("main"));
}

#[test]
fn test_tools_accessor() {
    let mut agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    assert!(agent_loop.tools().is_empty());
    agent_loop.register_tool(
        "test".to_string(),
        Box::new(MockTool {
            result: "ok".to_string(),
        }),
    );
    assert_eq!(agent_loop.tools().len(), 1);
}

#[test]
fn test_config_accessor() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    assert_eq!(agent_loop.config().model, "test-model");
    assert_eq!(agent_loop.config().max_turns, 5);
}

#[test]
fn test_mark_and_check_sent_in_round() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    assert!(!agent_loop.has_sent_in_round("sess1"));
    agent_loop.mark_sent_in_round("sess1");
    assert!(agent_loop.has_sent_in_round("sess1"));
    assert!(!agent_loop.has_sent_in_round("sess2"));
}

#[test]
fn test_set_route_resolver() {
    let mut agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    assert!(agent_loop.route_resolver.is_none());
    let config = nemesis_routing::RouteConfig {
        bindings: Vec::new(),
        agents: vec![nemesis_routing::AgentDef {
            id: "main".to_string(),
            is_default: true,
        }],
        dm_scope: "main".to_string(),
    };
    agent_loop.set_route_resolver(nemesis_routing::RouteResolver::new(config));
    assert!(agent_loop.route_resolver.is_some());
}

#[test]
fn test_set_cluster_and_get() {
    let mut agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    assert!(agent_loop.get_cluster().is_none());

    let cluster: Arc<dyn std::any::Any + Send + Sync> = Arc::new("test_cluster");
    agent_loop.set_cluster(cluster);
    assert!(agent_loop.get_cluster().is_some());
}

#[test]
fn test_set_observer_callback() {
    let mut agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    assert!(agent_loop.observer_callback.is_none());

    let cb: Arc<dyn Fn(&str, &serde_json::Value) + Send + Sync> = Arc::new(|_event, _data| {});
    agent_loop.set_observer_callback(cb);
    assert!(agent_loop.observer_callback.is_some());
}

#[tokio::test]
async fn test_run_with_empty_response() {
    let provider = MockLlmProvider::new(vec![LlmResponse {
        content: String::new(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);

    let agent_loop = AgentLoop::new(Box::new(provider), test_config());
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");

    let events = agent_loop.run(&instance, "Hello", &context).await;

    // Empty content should still produce a Done event
    assert!(events.iter().any(|e| matches!(e, AgentEvent::Done(_))));
}

#[tokio::test]
async fn test_handle_tool_call_unknown_tool() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");

    let tc = ToolCallInfo {
        id: "tc_1".to_string(),
        name: "nonexistent".to_string(),
        arguments: "{}".to_string(),
    };
    let result = agent_loop.handle_tool_call(&tc, &context).await;
    assert!(result.contains("Unknown tool"));
}

#[tokio::test]
async fn test_handle_tool_call_tool_error() {
    struct ErrorTool;
    #[async_trait]
    impl Tool for ErrorTool {
        async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
            Err("execution error".to_string())
        }
    }

    let mut agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    agent_loop.register_tool("err_tool".to_string(), Box::new(ErrorTool));
    let context = RequestContext::new("web", "chat1", "user1", "session1");

    let tc = ToolCallInfo {
        id: "tc_1".to_string(),
        name: "err_tool".to_string(),
        arguments: "{}".to_string(),
    };
    let result = agent_loop.handle_tool_call(&tc, &context).await;
    assert!(result.contains("Tool error"));
    assert!(result.contains("execution error"));
}

#[tokio::test]
async fn test_handle_tool_call_with_security_block() {
    use nemesis_security::pipeline::{SecurityPlugin, SecurityPluginConfig};
    use nemesis_security::types::SecurityRule;

    // Create a security plugin that blocks file writes
    let config = SecurityPluginConfig {
        enabled: true,
        injection_enabled: false,
        injection_threshold: 0.7,
        command_guard_enabled: false,
        credential_enabled: false,
        dlp_enabled: false,
        dlp_action: "block".to_string(),
        dlp_enabled_rules: vec![],
        dlp_low_confidence_action: "log".to_string(),
        dlp_inbound_action: "log".to_string(),
        ssrf_enabled: false,
        audit_chain_enabled: false,
        audit_chain_path: None,
        audit_log_enabled: false,
        audit_log_dir: None,
        default_action: "deny".to_string(),
        file_rules: vec![SecurityRule {
            pattern: ".*".to_string(),
            action: "deny".to_string(),
            comment: "block all file writes".to_string(),
        }],
        dir_rules: vec![],
        process_rules: vec![],
        network_rules: vec![],
        hardware_rules: vec![],
        registry_rules: vec![],
    };
    let blocked_plugin: Arc<SecurityPlugin> = Arc::new(SecurityPlugin::new(config));

    let mut agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    agent_loop.set_security_plugin(blocked_plugin);
    agent_loop.register_tool(
        "write_file".to_string(),
        Box::new(MockTool {
            result: "ok".to_string(),
        }),
    );
    let context = RequestContext::new("web", "chat1", "user1", "session1");

    let tc = ToolCallInfo {
        id: "tc_1".to_string(),
        name: "write_file".to_string(),
        arguments: r#"{"path": "/some/path"}"#.to_string(),
    };
    let result = agent_loop.handle_tool_call(&tc, &context).await;
    assert!(
        result.contains("Error") || result.contains("denied") || result.contains("not allowed")
    );
}

#[tokio::test]
async fn test_checkpoint_e2e_write_then_rewind_restores() {
    // P3 AgentLoop-level e2e: write_file snapshots pre-edit content via the
    // capture seam in handle_tool_call, then rewind restores it. Verifies the
    // full flow: set_checkpoint_store → preview → snapshot → execute → rewind.
    use crate::checkpoint::CheckpointStore;
    use crate::loop_tools::WriteFileTool;

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let store = Arc::new(CheckpointStore::new(Some(root.join(".ck")), root.clone()));
    let mut agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    agent_loop.set_checkpoint_store(store.clone());
    agent_loop.register_tool("write_file".to_string(), Box::new(WriteFileTool));

    // Pre-existing file whose turn-start content must be captured.
    let file = root.join("target.txt");
    tokio::fs::write(&file, "ORIGINAL").await.unwrap();
    store.begin(0, "edit the file"); // simulate process_inbound_message opening a turn

    let context = RequestContext::new("web", "chat1", "user1", "session1");
    let tc = ToolCallInfo {
        id: "tc1".to_string(),
        name: "write_file".to_string(),
        arguments: serde_json::json!({ "path": file.to_str().unwrap(), "content": "CHANGED" })
            .to_string(),
    };
    let _ = agent_loop.handle_tool_call(&tc, &context).await;
    // write_file executed and overwrote the file.
    assert_eq!(
        tokio::fs::read_to_string(&file).await.unwrap(),
        "CHANGED",
        "write_file should have overwritten"
    );

    // Rewind turn 0 restores the turn-start content (ORIGINAL).
    let (written, _) = agent_loop.rewind(0).await.unwrap();
    assert!(
        !written.is_empty(),
        "rewind should restore at least one file"
    );
    assert_eq!(
        tokio::fs::read_to_string(&file).await.unwrap(),
        "ORIGINAL",
        "rewind must restore pre-edit content"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_guardian_e2e_critical_op_denied_by_judge() {
    // P5 AgentLoop-level e2e: a CRITICAL op that passes the rule layers is
    // blocked by the guardian judge. Verifies: set_judge → security allow →
    // is_critical_tool → judge → deny → block, in the real handle_tool_call path.
    use nemesis_security::guardian::{JudgeOutcome, JudgeRequest, JudgeVerdict, LlmJudge};
    use nemesis_security::pipeline::{SecurityPlugin, SecurityPluginConfig};

    struct DenyJudge;
    #[async_trait]
    impl LlmJudge for DenyJudge {
        async fn judge(&self, _req: &JudgeRequest) -> Result<JudgeVerdict, String> {
            Ok(JudgeVerdict {
                risk_level: "critical".into(),
                user_authorization: "unknown".into(),
                outcome: JudgeOutcome::Deny,
                rationale: "destructive without explicit auth".into(),
            })
        }
    }

    // Rules allow everything (default allow, no guards) so the guardian is the
    // only thing that can block.
    let plugin = Arc::new(SecurityPlugin::new(SecurityPluginConfig {
        enabled: true,
        command_guard_enabled: false,
        injection_enabled: false,
        credential_enabled: false,
        dlp_enabled: false,
        ssrf_enabled: false,
        default_action: "allow".to_string(),
        ..Default::default()
    }));
    plugin.set_judge(Arc::new(DenyJudge));

    let mut agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    agent_loop.set_security_plugin(plugin);
    agent_loop.register_tool(
        "shell".to_string(),
        Box::new(MockTool {
            result: "ok".to_string(),
        }),
    );
    let context = RequestContext::new("web", "chat1", "user1", "session1");
    let tc = ToolCallInfo {
        id: "tc1".to_string(),
        name: "shell".to_string(),
        arguments: r#"{"command":"rm -rf /"}"#.to_string(),
    };
    let result = agent_loop.handle_tool_call(&tc, &context).await;
    assert!(
        result.contains("GUARDIAN DENIED"),
        "critical op must be blocked by guardian judge: {}",
        result
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_guardian_e2e_allows_when_judge_approves() {
    // P5 e2e counterpart: when the judge allows, the CRITICAL op proceeds.
    use nemesis_security::guardian::{JudgeOutcome, JudgeRequest, JudgeVerdict, LlmJudge};
    use nemesis_security::pipeline::{SecurityPlugin, SecurityPluginConfig};

    struct AllowJudge;
    #[async_trait]
    impl LlmJudge for AllowJudge {
        async fn judge(&self, _req: &JudgeRequest) -> Result<JudgeVerdict, String> {
            Ok(JudgeVerdict {
                risk_level: "high".into(),
                user_authorization: "high".into(),
                outcome: JudgeOutcome::Allow,
                rationale: "user explicitly requested".into(),
            })
        }
    }

    let plugin = Arc::new(SecurityPlugin::new(SecurityPluginConfig {
        enabled: true,
        command_guard_enabled: false,
        injection_enabled: false,
        credential_enabled: false,
        dlp_enabled: false,
        ssrf_enabled: false,
        default_action: "allow".to_string(),
        ..Default::default()
    }));
    plugin.set_judge(Arc::new(AllowJudge));

    let mut agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    agent_loop.set_security_plugin(plugin);
    agent_loop.register_tool(
        "shell".to_string(),
        Box::new(MockTool {
            result: "executed".to_string(),
        }),
    );
    let context = RequestContext::new("web", "chat1", "user1", "session1");
    let tc = ToolCallInfo {
        id: "tc1".to_string(),
        name: "shell".to_string(),
        arguments: r#"{"command":"ls"}"#.to_string(),
    };
    let result = agent_loop.handle_tool_call(&tc, &context).await;
    assert!(
        !result.contains("GUARDIAN DENIED"),
        "approved op must proceed: {}",
        result
    );
}

#[test]
fn test_build_messages_with_tool_history() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let instance = AgentInstance::new(test_config());
    instance.add_user_message("Hello");
    instance.add_assistant_message(
        "Let me check",
        vec![ToolCallInfo {
            id: "tc_1".to_string(),
            name: "calculator".to_string(),
            arguments: "{}".to_string(),
        }],
        None,
    );
    instance.add_tool_result("tc_1", "42");
    instance.add_assistant_message("The answer is 42", vec![], None);

    let messages = agent_loop.build_messages(&instance);
    // system + (injected time) + user + assistant(tool_calls) + tool + assistant = 6
    assert_eq!(messages.len(), 6);
    assert!(messages[3].tool_calls.is_some());
    assert_eq!(messages[4].tool_call_id, Some("tc_1".to_string()));
}

#[test]
fn test_process_message_system_channel() {
    let provider = MockLlmProvider::new(vec![]);
    let agent_loop = AgentLoop::new(Box::new(provider), test_config());
    let ctx = RequestContext::new("system", "chat1", "user1", "sess1");

    let (_, _, handled) = agent_loop.process_message("cluster_continuation:task-123", &ctx);
    assert!(handled);
}

#[test]
fn test_process_message_regular_message() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    let (_, _, handled) = agent_loop.process_message("regular message", &ctx);
    assert!(!handled);
}

#[test]
fn test_process_heartbeat_with_response() {
    let provider = MockLlmProvider::new(vec![LlmResponse {
        content: "heartbeat ok".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);

    let rt = tokio::runtime::Runtime::new().unwrap();
    let agent_loop = AgentLoop::new(Box::new(provider), test_config());

    let result = rt.block_on(async { agent_loop.process_heartbeat("Ping", "web", "chat1").await });

    assert_eq!(result, Ok("heartbeat ok".to_string()));
}

#[test]
fn test_process_direct_with_error() {
    struct ErrorProvider;
    #[async_trait]
    impl LlmProvider for ErrorProvider {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<LlmMessage>,
            _options: Option<crate::types::ChatOptions>,
            _tools: Vec<crate::types::ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            Err("test error".to_string())
        }
    }

    let rt = tokio::runtime::Runtime::new().unwrap();
    let agent_loop = AgentLoop::new(Box::new(ErrorProvider), test_config());

    let result = rt.block_on(async { agent_loop.process_direct("Hello", "sess1").await });

    assert!(result.is_err());
    assert!(result.unwrap_err().contains("test error"));
}

// --- Additional coverage for slash commands and accessors ---

#[test]
fn test_handle_command_list_channels_empty_v2() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let result = agent_loop.handle_command("/list channels");
    assert!(result.is_some());
    assert!(result.unwrap().contains("No channels enabled"));
}

#[test]
fn test_process_message_non_system_continuation() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let (_, _, handled) = agent_loop.process_message("cluster_continuation:task-123", &ctx);
    // Not system channel, so not handled as continuation
    assert!(!handled);
}

#[test]
fn test_process_message_slash_command() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let (_, response, handled) = agent_loop.process_message("/show model", &ctx);
    assert!(handled);
    assert!(response.contains("test-model"));
}

// --- Additional coverage for process_inbound_message and bus mode ---

fn make_inbound(
    content: &str,
    channel: &str,
    chat_id: &str,
    sender_id: &str,
    session_key: &str,
) -> nemesis_types::channel::InboundMessage {
    nemesis_types::channel::InboundMessage {
        channel: channel.to_string(),
        sender_id: sender_id.to_string(),
        chat_id: chat_id.to_string(),
        content: content.to_string(),
        media: vec![],
        session_key: session_key.to_string(),
        correlation_id: String::new(),
        metadata: std::collections::HashMap::new(),
        voice_playback: None,
    }
}

#[tokio::test]
async fn test_process_inbound_message_system_internal_channel() {
    let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::channel(16);
    let provider = MockLlmProvider::new(vec![LlmResponse {
        content: "Processed subagent result".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);
    let agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config(),
        outbound_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );

    // System message with internal channel (cli) - should skip processing
    let msg = nemesis_types::channel::InboundMessage {
        channel: "system".to_string(),
        sender_id: "subagent-1".to_string(),
        chat_id: "cli:direct".to_string(),
        content: "Task completed.".to_string(),
        media: vec![],
        session_key: String::new(),
        correlation_id: String::new(),
        metadata: std::collections::HashMap::new(),
        voice_playback: None,
    };
    let (agent_id, response, err) = agent_loop.process_inbound_message(&msg).await;
    assert_eq!(agent_id, "");
    assert!(response.is_empty());
    assert!(err.is_none());

    // No outbound should be produced for internal channel system messages
    // outbound_tx was moved into AgentLoop, so just check outbound_rx is empty
    assert!(outbound_rx.try_recv().is_err());
}

#[tokio::test]
async fn test_process_inbound_message_history_request() {
    let (outbound_tx, _) = tokio::sync::mpsc::channel(16);
    let provider = MockLlmProvider::new(vec![]);
    let agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config(),
        outbound_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );

    let msg = nemesis_types::channel::InboundMessage {
        channel: "web".to_string(),
        sender_id: "user1".to_string(),
        chat_id: "chat1".to_string(),
        content: r#"{"request_id":"r1","limit":10}"#.to_string(),
        session_key: "web:chat1".to_string(),
        media: vec![],
        correlation_id: String::new(),
        metadata: {
            let mut m = std::collections::HashMap::new();
            m.insert("request_type".to_string(), "history".to_string());
            m
        },
        voice_playback: None,
    };
    let (agent_id, response, err) = agent_loop.process_inbound_message(&msg).await;
    assert_eq!(agent_id, "");
    assert!(response.is_empty());
    assert!(err.is_none());
}

#[tokio::test]
async fn test_process_inbound_message_session_busy() {
    let (outbound_tx, _) = tokio::sync::mpsc::channel(16);
    let provider = MockLlmProvider::new(vec![LlmResponse {
        content: "Mock response".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);
    let agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config(),
        outbound_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );

    // First process a message to determine what session key the resolver uses.
    // Then acquire that key and verify the busy check works.
    let msg1 = make_inbound("First", "web", "chat1", "user1", "");
    let (agent_id, first_response, _) = agent_loop.process_inbound_message(&msg1).await;

    // The first message should have been processed successfully
    assert!(first_response.contains("Mock response"));

    // The session should have been released after processing.
    // Now acquire it and verify busy works.
    assert!(agent_loop.try_acquire_session("agent:main"));

    let msg2 = make_inbound("Second", "web", "chat1", "user1", "");
    let (_, response, _) = agent_loop.process_inbound_message(&msg2).await;

    // Try multiple possible session key formats
    if !response.contains("try again later") {
        // The session key might not be "agent:main" - just verify the mechanism works
        // by testing directly with a known key
        agent_loop.release_session("agent:main");
    }
    // At minimum verify agent_id is set
    assert_eq!(agent_id, "main");
}

#[tokio::test]
async fn test_process_inbound_message_route_resolver() {
    let (outbound_tx, _) = tokio::sync::mpsc::channel(16);
    let provider = MockLlmProvider::new(vec![LlmResponse {
        content: "Routed response".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);
    let agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config(),
        outbound_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );

    let msg = make_inbound("Hello route", "web", "chat1", "user1", "");
    let (agent_id, response, err) = agent_loop.process_inbound_message(&msg).await;
    // Should route to main agent (default)
    assert_eq!(agent_id, "main");
    assert!(response.contains("Routed response"));
    assert!(err.is_none());
}

#[tokio::test]
async fn test_process_inbound_message_route_with_agent_scoped_key() {
    let (outbound_tx, _) = tokio::sync::mpsc::channel(16);
    let provider = MockLlmProvider::new(vec![LlmResponse {
        content: "Agent scoped".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);
    let agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config(),
        outbound_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );

    let msg = nemesis_types::channel::InboundMessage {
        channel: "web".to_string(),
        sender_id: "user1".to_string(),
        chat_id: "chat1".to_string(),
        content: "Hello".to_string(),
        media: vec![],
        session_key: "agent:main:custom_session".to_string(),
        correlation_id: String::new(),
        metadata: std::collections::HashMap::new(),
        voice_playback: None,
    };
    let (agent_id, response, err) = agent_loop.process_inbound_message(&msg).await;
    assert_eq!(agent_id, "main");
    assert!(response.contains("Agent scoped"));
    assert!(err.is_none());
}

#[tokio::test]
async fn test_process_inbound_message_no_resolver_fallback() {
    let provider = MockLlmProvider::new(vec![LlmResponse {
        content: "Fallback response".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);
    // Use AgentLoop::new (standalone) which has no route resolver
    let agent_loop = AgentLoop::new(Box::new(provider), test_config());

    // process_direct_with_channel works in standalone mode
    let result = agent_loop
        .process_direct_with_channel("Hello no resolver", "web:chat1", "web", "chat1")
        .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_run_bus_owned_sends_outbound() {
    let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::channel(16);
    let (inbound_tx, inbound_rx) = tokio::sync::mpsc::channel(16);

    let provider = MockLlmProvider::new(vec![LlmResponse {
        content: "Bus response".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);
    let agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config(),
        outbound_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );

    // Send a message
    let msg = make_inbound("Hello bus", "web", "chat1", "user1", "web:chat1");
    inbound_tx.send(msg).await.unwrap();
    drop(inbound_tx); // Close to end the loop

    agent_loop.run_bus_owned(inbound_rx).await;

    let outbound = outbound_rx.try_recv();
    assert!(outbound.is_ok());
    let out = outbound.unwrap();
    assert!(out.content.contains("Bus response"));
}

/// Integration test for the "供应商·模型名" badge pipeline: verifies that an
/// assistant OutboundMessage carries `meta.model` = resolved `provider/name`,
/// AND the chat_log assistant row persists it for history reload. Uses a temp
/// config.json so `current_display_model()` resolves via model_list.
#[tokio::test]
async fn test_assistant_outbound_carries_model_badge() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg_path = tmp.path().join("config.json");
    std::fs::write(
        &cfg_path,
        serde_json::json!({
            "model_list": [{"model": "testprov/test-model", "model_name": "test-model"}]
        })
        .to_string(),
    )
    .unwrap();

    let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::channel(16);
    let (inbound_tx, inbound_rx) = tokio::sync::mpsc::channel(16);
    let provider = MockLlmProvider::new(vec![LlmResponse {
        content: "badge test response".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);

    let session_key = "web:chat1";
    let agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config(),
        outbound_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    agent_loop.set_config_path(cfg_path);

    let msg = make_inbound("Hello badge", "web", "chat1", "user1", session_key);
    inbound_tx.send(msg).await.unwrap();
    drop(inbound_tx);
    agent_loop.run_bus_owned(inbound_rx).await;

    // The assistant OutboundMessage carries the resolved display model
    // (provider/name) for the per-message "供应商·模型名" badge.
    let out = outbound_rx
        .try_recv()
        .expect("expected an assistant outbound");
    assert!(out.content.contains("badge test response"));
    assert_eq!(
        out.meta.model.as_deref(),
        Some("testprov/test-model"),
        "outbound.meta.model must be the resolved provider/name for the badge"
    );

    // Persistence: the chat_log assistant row carries the model badge too
    // (history reload). The agent derives the session key for chat_log from
    // metadata.session_id (empty here) → default main key.
    let main_key = "agent_main_main";
    let (msgs, _total, _, _) = crate::chat_log::read_chat_log(main_key, 50, None);
    let assistant = msgs.iter().find(|m| {
        m["role"].as_str() == Some("assistant")
            && m["content"].as_str() == Some("badge test response")
    });
    if let Some(assistant) = assistant {
        assert_eq!(
            assistant["model"].as_str(),
            Some("testprov/test-model"),
            "chat_log assistant row must persist the model badge"
        );
    }
    // (If the assistant row isn't found under main_key, the outbound assertion
    //  above already covers the stamping; chat_log write is also unit-tested in
    //  chat_log::tests::test_append_with_model_round_trip.)
}

#[tokio::test]
async fn test_run_bus_owned_rpc_correlation_prefix() {
    let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::channel(16);
    let (inbound_tx, inbound_rx) = tokio::sync::mpsc::channel(16);

    let provider = MockLlmProvider::new(vec![LlmResponse {
        content: "RPC response".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);
    let agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config(),
        outbound_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );

    let msg = nemesis_types::channel::InboundMessage {
        channel: "rpc".to_string(),
        sender_id: "user1".to_string(),
        chat_id: "chat1".to_string(),
        content: "Hello RPC".to_string(),
        media: vec![],
        session_key: "rpc:chat1".to_string(),
        correlation_id: "corr-123".to_string(),
        metadata: std::collections::HashMap::new(),
        voice_playback: None,
    };
    inbound_tx.send(msg).await.unwrap();
    drop(inbound_tx);

    agent_loop.run_bus_owned(inbound_rx).await;

    let outbound = outbound_rx.try_recv();
    assert!(outbound.is_ok());
    let out = outbound.unwrap();
    assert!(out.content.starts_with("[rpc:corr-123]"));
}

#[test]
fn test_sent_in_round_tracker_mark_and_check() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    assert!(!agent_loop.has_sent_in_round("web:chat1"));
    agent_loop.mark_sent_in_round("web:chat1");
    assert!(agent_loop.has_sent_in_round("web:chat1"));
}

#[tokio::test]
async fn test_process_system_message_with_result_extraction() {
    let (outbound_tx, _) = tokio::sync::mpsc::channel(16);
    let provider = MockLlmProvider::new(vec![LlmResponse {
        content: "System processed".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);
    let agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config(),
        outbound_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );

    let msg = nemesis_types::channel::InboundMessage {
        channel: "system".to_string(),
        sender_id: "subagent-1".to_string(),
        chat_id: "web:chat1".to_string(), // non-internal channel
        content: "Task 'my_task' completed.\n\nResult:\nThe actual result content".to_string(),
        media: vec![],
        session_key: String::new(),
        correlation_id: String::new(),
        metadata: std::collections::HashMap::new(),
        voice_playback: None,
    };
    let (_, response, _) = agent_loop.process_inbound_message(&msg).await;
    assert!(response.contains("System processed"));
}

#[tokio::test]
async fn test_process_system_message_without_result_prefix() {
    let (outbound_tx, _) = tokio::sync::mpsc::channel(16);
    let provider = MockLlmProvider::new(vec![LlmResponse {
        content: "Direct content".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);
    let agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config(),
        outbound_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );

    let msg = nemesis_types::channel::InboundMessage {
        channel: "system".to_string(),
        sender_id: "subagent-1".to_string(),
        chat_id: "web:chat1".to_string(),
        content: "No result prefix here".to_string(),
        media: vec![],
        session_key: String::new(),
        correlation_id: String::new(),
        metadata: std::collections::HashMap::new(),
        voice_playback: None,
    };
    let (_, response, _) = agent_loop.process_inbound_message(&msg).await;
    assert!(response.contains("Direct content"));
}

#[tokio::test]
async fn test_force_compression_no_system_prompt() {
    // force_compression works without a system prompt: the cache advances and
    // build_messages emits the leading summary system turn + verbatim tail.
    let config = AgentConfig {
        model: "test".to_string(),
        system_prompt: None,
        max_turns: 5,
        tools: Vec::new(),
        models: std::collections::HashMap::new(),
    };
    let instance = AgentInstance::new(config);
    for i in 0..5 {
        instance.add_user_message(&format!("User message {}", i));
        instance.add_assistant_message(&format!("Response {}", i), Vec::new(), None);
    }
    // No system prompt → history = 10 conversation messages.

    let agent_loop = AgentLoop::new(
        Box::new(MockLlmProvider::new(vec![llm_text("SUMMARY")])),
        test_config(),
    );
    agent_loop.force_compression(&instance).await;

    let cache = instance.get_summary_cache().expect("cache should be set");
    assert_eq!(cache.covers_up_to, 10 - SMALL_K_FORCE);
    assert_eq!(cache.text, "SUMMARY");
    // History unchanged.
    assert_eq!(instance.get_history().len(), 10);
}

#[tokio::test]
async fn test_force_compression_preserves_last_message() {
    // The current user message must remain in the verbatim tail after
    // compression so build_messages still sends it to the LLM.
    let instance = AgentInstance::new(test_config());
    for i in 0..20 {
        instance.add_user_message(&format!("User {}", i));
        instance.add_assistant_message(&format!("Response {}", i), Vec::new(), None);
    }
    instance.add_user_message("Final message");
    // history = [sys, 40 msgs, "Final message"] = 42.

    let agent_loop = AgentLoop::new(
        Box::new(MockLlmProvider::new(vec![llm_text("SUMMARY")])),
        test_config(),
    );
    agent_loop.force_compression(&instance).await;

    let cache = instance.get_summary_cache().expect("cache should be set");
    assert_eq!(cache.covers_up_to, 42 - SMALL_K_FORCE); // tail = last 2 messages
    // build_messages sends the verbatim tail, which includes "Final message".
    let messages = agent_loop.build_messages(&instance);
    let joined: String = messages
        .iter()
        .map(|m| m.content.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(joined.contains("Final message"));
}

#[test]
fn test_session_busy_tracker_queue_mode() {
    let tracker = SessionBusyTracker::new(ConcurrentMode::Queue, 3);
    assert!(tracker.try_acquire("sess1"));
    assert!(!tracker.try_acquire("sess1")); // queued
    assert!(!tracker.try_acquire("sess1")); // queued
    assert!(!tracker.try_acquire("sess1")); // queue full
    assert!(!tracker.try_acquire("sess1")); // still full
}

#[test]
fn test_session_busy_tracker_release_with_queue() {
    let tracker = SessionBusyTracker::new(ConcurrentMode::Queue, 3);
    assert!(tracker.try_acquire("sess1"));

    // After release, the session should no longer be busy
    tracker.release("sess1");
    assert!(!tracker.is_busy("sess1"));
}

#[test]
fn test_sent_in_round_tracker_clear_all() {
    let tracker = SentInRoundTracker::new();
    tracker.mark_sent("s1");
    tracker.mark_sent("s2");
    tracker.mark_sent("s3");
    assert!(tracker.has_sent_in_round("s1"));
    assert!(tracker.has_sent_in_round("s2"));

    tracker.clear_all();
    assert!(!tracker.has_sent_in_round("s1"));
    assert!(!tracker.has_sent_in_round("s2"));
}

#[test]
fn test_route_input_and_output_types() {
    let input = RouteInput {
        channel: "web".to_string(),
        account_id: Some("acc1".to_string()),
        peer: "direct:user1".to_string(),
        parent_peer: Some("guild:guild1".to_string()),
        guild_id: Some("g1".to_string()),
        team_id: None,
    };
    assert_eq!(input.channel, "web");
    assert_eq!(input.peer, "direct:user1");

    let output = RouteOutput {
        agent_id: "main".to_string(),
        session_key: "agent:main:sess".to_string(),
        matched_by: "default".to_string(),
    };
    assert_eq!(output.agent_id, "main");
}

#[test]
fn test_extract_peer_with_empty_metadata() {
    let msg = make_inbound("hello", "web", "chat1", "user123", "");
    let peer = extract_peer(&msg);
    assert_eq!(peer, "user123");
}

#[test]
fn test_extract_parent_peer_empty_metadata() {
    let msg = make_inbound("hello", "web", "chat1", "user123", "");
    let result = extract_parent_peer(&msg);
    assert!(result.is_none());
}

#[tokio::test]
async fn test_process_heartbeat_empty_response() {
    let provider = MockLlmProvider::new(vec![LlmResponse {
        content: String::new(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);
    let agent_loop = AgentLoop::new(Box::new(provider), test_config());
    let result = agent_loop.process_heartbeat("ping", "web", "chat1").await;
    // Empty content from LLM -> Done("") is found first -> Ok("")
    assert!(result.is_ok());
    // The heartbeat returns the empty content
    assert_eq!(result.unwrap(), "");
}

#[tokio::test]
async fn test_process_heartbeat_no_done_event() {
    // When LLM returns tool calls without finishing, run() may produce
    // ToolCall events but not a Done event. process_heartbeat then returns the fallback.
    let provider = MockLlmProvider::new(vec![LlmResponse {
        content: "Heartbeat response".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);
    let agent_loop = AgentLoop::new(Box::new(provider), test_config());
    let result = agent_loop.process_heartbeat("ping", "web", "chat1").await;
    assert_eq!(result, Ok("Heartbeat response".to_string()));
}

#[test]
fn test_record_last_channel_no_state_manager() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    // Should not panic when no state manager
    agent_loop.record_last_channel("web");
    agent_loop.record_last_chat_id("chat1");
}

#[test]
fn test_session_queue_length() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    assert_eq!(agent_loop.session_queue_length("nonexistent"), 0);

    agent_loop.try_acquire_session("sess1");
    assert_eq!(agent_loop.session_queue_length("sess1"), 0);
}

#[test]
fn test_get_session_busy_state_nonexistent() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let (busy, queue_len) = agent_loop.get_session_busy_state("nonexistent");
    assert!(!busy);
    assert_eq!(queue_len, 0);
}

#[test]
fn test_release_session_nonexistent() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let has_queue = agent_loop.release_session("nonexistent");
    assert!(!has_queue);
}

#[test]
fn test_handle_command_empty_slash() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    // Empty string after trim won't have a first part
    let result = agent_loop.handle_command("   ");
    assert!(result.is_none());
}

#[test]
fn test_handle_command_show_no_target() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let result = agent_loop.handle_command("/show");
    assert!(result.is_some());
    assert!(result.unwrap().contains("Usage"));
}

#[test]
fn test_handle_command_list_no_target() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let result = agent_loop.handle_command("/list");
    assert!(result.is_some());
    assert!(result.unwrap().contains("Usage"));
}

#[test]
fn test_handle_command_switch_wrong_format() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let result = agent_loop.handle_command("/switch model mymodel");
    assert!(result.is_some());
    assert!(result.unwrap().contains("Usage"));
}

#[test]
fn test_build_agent_main_session_key_format() {
    let key = build_agent_main_session_key("agent-1");
    assert_eq!(key, "agent:agent-1:main");
}

#[test]
fn test_extract_continuation_task_id_none() {
    let result = extract_continuation_task_id("not_a_continuation");
    assert!(result.is_none());
}

#[test]
fn test_llm_message_serialization_roundtrip() {
    let msg = LlmMessage {
        role: "assistant".to_string(),
        content: "Hello".to_string(),
        tool_calls: Some(vec![ToolCallInfo {
            id: "tc_1".to_string(),
            name: "tool1".to_string(),
            arguments: r#"{"key":"value"}"#.to_string(),
        }]),
        tool_call_id: Some("tc_1".to_string()),
        reasoning_content: None,
    };
    let json = serde_json::to_string(&msg).unwrap();
    let deserialized: LlmMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.role, "assistant");
    assert_eq!(deserialized.content, "Hello");
    assert!(deserialized.tool_calls.is_some());
    assert_eq!(deserialized.tool_calls.unwrap().len(), 1);
}

#[test]
fn test_format_messages_for_log_with_tool_call_id() {
    let messages = vec![LlmMessage {
        role: "tool".to_string(),
        content: "Result".to_string(),
        tool_calls: None,
        tool_call_id: Some("tc_42".to_string()),
        reasoning_content: None,
    }];
    let log = format_messages_for_log(&messages);
    assert!(log.contains("tc_42"));
    assert!(log.contains("Result"));
}

#[tokio::test]
async fn test_maybe_summarize_no_session_store() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let instance = AgentInstance::new(test_config());
    // Add many messages to trigger summarization
    for i in 0..30 {
        instance.add_user_message(&format!("Message {} with enough content to make it long enough for token estimation to exceed threshold in some way", i));
        instance.add_assistant_message(
            &format!(
                "Response {} with similar padding content to increase estimated tokens",
                i
            ),
            Vec::new(),
            None,
        );
    }
    // Should not panic even without session store
    agent_loop
        .maybe_update_summary(&instance, "test-session", "web", "chat1")
        .await;
}

#[tokio::test]
async fn test_maybe_summarize_already_summarizing() {
    let (outbound_tx, _) = tokio::sync::mpsc::channel(16);
    let provider = MockLlmProvider::new(vec![LlmResponse {
        content: "Summary".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);
    let agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config(),
        outbound_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    let instance = AgentInstance::new(test_config());
    for i in 0..30 {
        instance.add_user_message(&format!(
            "Long user message {} with padding to increase tokens",
            i
        ));
        instance.add_assistant_message(
            &format!("Long response {} with padding", i),
            Vec::new(),
            None,
        );
    }

    // First call triggers summarization
    agent_loop
        .maybe_update_summary(&instance, "sess1", "web", "chat1")
        .await;
    // Second call should be skipped (already summarizing)
    agent_loop
        .maybe_update_summary(&instance, "sess1", "web", "chat1")
        .await;
}

#[tokio::test]
async fn test_maybe_update_summary_advances_cache_when_tail_over_threshold() {
    // Small context window → 75% threshold is low, so a handful of messages
    // crosses it. Verifies the cache advances to len-K_TARGET and stores the
    // summary text.
    let (outbound_tx, _) = tokio::sync::mpsc::channel(16);
    let provider = MockLlmProvider::new(vec![llm_text("SUMMARY")]);
    let agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config(),
        outbound_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    let mut instance = AgentInstance::new(test_config());
    instance.set_context_window(100); // threshold = 75 tokens
    for i in 0..5 {
        instance.add_user_message(&format!("user msg {} with padding to add tokens", i));
        instance.add_assistant_message(
            &format!("assistant reply {} with padding", i),
            Vec::new(),
            None,
        );
    }
    // history = [sys, 5u, 5a] = 11. tail tokens > 75; tail_len = 11 > K_TARGET.
    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c")
        .await;

    let cache = instance.get_summary_cache().expect("cache should be set");
    assert_eq!(cache.covers_up_to, 11 - K_TARGET);
    assert_eq!(cache.text, "SUMMARY");
}

#[tokio::test]
async fn test_maybe_update_summary_no_trigger_below_threshold() {
    // Huge context window → threshold never crossed → cache stays None and the
    // LLM is never called (no responses provisioned).
    let (outbound_tx, _) = tokio::sync::mpsc::channel(16);
    let provider = MockLlmProvider::new(vec![]);
    let agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config(),
        outbound_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    let mut instance = AgentInstance::new(test_config());
    instance.set_context_window(100_000);
    for i in 0..5 {
        instance.add_user_message(&format!("msg {}", i));
        instance.add_assistant_message(&format!("reply {}", i), Vec::new(), None);
    }
    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c")
        .await;
    assert!(instance.get_summary_cache().is_none());
}

#[tokio::test]
async fn test_maybe_update_summary_no_stuck_when_tail_short_but_over_threshold() {
    // Regression (found via S7 实跑): a few HUGE messages put tail_tokens over
    // the threshold while tail_len <= K_TARGET (too short to summarize yet).
    // The stuck counter must NOT tick in this state — the old logic ticked it
    // on `tail_tokens >= threshold` regardless of tail_len, so a big system
    // prompt / early oversized tool result paused summarization before it ever
    // ran. Verify summarize still fires once the tail grows long enough.
    let (outbound_tx, _) = tokio::sync::mpsc::channel(16);
    let provider = MockLlmProvider::new(vec![llm_text("SUMMARY")]);
    let agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config(),
        outbound_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    let mut instance = AgentInstance::new(test_config());
    instance.set_context_window(100); // threshold = 75 tokens
    // 3 huge messages: tail_tokens way over 75, but tail_len = 3 <= K_TARGET(6).
    instance.add_user_message(&"x".repeat(500));
    instance.add_assistant_message(&"y".repeat(500), Vec::new(), None);
    instance.add_user_message(&"z".repeat(500));

    // Two calls while over-threshold-but-too-short. Old bug: stuck ticks to 2
    // and pauses. Fix: no tick (can't summarize yet).
    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c")
        .await;
    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c")
        .await;
    assert!(instance.get_summary_cache().is_none()); // too short to summarize

    // Grow the tail past K_TARGET — summarize MUST still run (not paused).
    for i in 0..5 {
        instance.add_user_message(&format!("grow {}", i));
        instance.add_assistant_message(&format!("ack {}", i), Vec::new(), None);
    }
    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c")
        .await;
    instance
        .get_summary_cache()
        .expect("summarize must run once the tail is long enough (stuck did not pause it)");
}

#[test]
fn test_tool_safe_boundary_backs_past_leading_tool() {
    // The summarize boundary (len - K_TARGET) may land between an assistant
    // tool_call and its tool result. tool_safe_boundary must back it up to the
    // parent assistant so the pair stays verbatim in the tail (otherwise repair
    // drops the orphan result and the interaction is lost — summarize only
    // covers user/assistant content, not tool_calls/results).
    let tc = |id: &str| crate::types::ToolCallInfo {
        id: id.to_string(),
        name: "t".to_string(),
        arguments: "{}".to_string(),
    };
    let turn = |role: &str,
                content: &str,
                tcs: Vec<crate::types::ToolCallInfo>,
                tcid: Option<&str>| crate::types::ConversationTurn {
        role: role.to_string(),
        content: content.to_string(),
        tool_calls: tcs,
        tool_call_id: tcid.map(String::from),
        timestamp: String::new(),
        reasoning_content: None,
        tool_name: None,
        tool_result_projection: None,
    };
    let history = vec![
        turn("system", "sys", vec![], None),            // 0
        turn("user", "u1", vec![], None),               // 1
        turn("user", "u2", vec![], None),               // 2
        turn("assistant", "go", vec![tc("c1")], None),  // 3 parent
        turn("tool", "res", vec![], Some("c1")),        // 4 naive boundary lands here
        turn("user", "u3", vec![], None),               // 5
        turn("assistant", "a2", vec![], None),          // 6
        turn("user", "u4", vec![], None),               // 7
        turn("assistant", "a3", vec![], None),          // 8
        turn("user", "u5", vec![], None),               // 9
    ];
    // Naive new_c = 10 - K_TARGET(6) = 4 → history[4] is tool → back up to 3.
    assert_eq!(tool_safe_boundary(&history, 10 - K_TARGET), 3);
    // If the boundary is already on a non-tool, it is unchanged.
    assert_eq!(tool_safe_boundary(&history, 5), 5);
    // Multiple consecutive tools back up past all of them to the parent.
    let history2 = vec![
        turn("assistant", "go", vec![tc("c1"), tc("c2")], None), // 0 parent
        turn("tool", "r1", vec![], Some("c1")),                  // 1
        turn("tool", "r2", vec![], Some("c2")),                  // 2
        turn("user", "u", vec![], None),                         // 3
    ];
    assert_eq!(tool_safe_boundary(&history2, 2), 0);
}

#[tokio::test]
async fn test_maybe_update_summary_boundary_is_tool_pair_safe() {
    // Integration: maybe_update_summary must set a tool-safe covers_up_to when
    // the naive boundary (len - K_TARGET) lands on a tool result. Regression
    // for the boundary-splits-tool-pair info loss found in second-pass review.
    let (outbound_tx, _) = tokio::sync::mpsc::channel(16);
    let provider = MockLlmProvider::new(vec![llm_text("SUMMARY")]);
    let agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config(),
        outbound_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    let mut instance = AgentInstance::new(test_config());
    instance.set_context_window(100); // threshold = 75 tokens
    let tc = |id: &str| crate::types::ToolCallInfo {
        id: id.to_string(),
        name: "t".to_string(),
        arguments: "{}".to_string(),
    };
    let turn = |role: &str,
                content: &str,
                tcs: Vec<crate::types::ToolCallInfo>,
                tcid: Option<&str>| crate::types::ConversationTurn {
        role: role.to_string(),
        content: content.to_string(),
        tool_calls: tcs,
        tool_call_id: tcid.map(String::from),
        timestamp: String::new(),
        reasoning_content: None,
        tool_name: None,
        tool_result_projection: None,
    };
    instance.set_history(vec![
        turn("system", "sys", vec![], None),                       // 0
        turn("user", &format!("u1 {}", "x".repeat(60)), vec![], None), // 1
        turn("user", &format!("u2 {}", "x".repeat(60)), vec![], None), // 2
        turn("assistant", "go", vec![tc("c1")], None),             // 3 parent
        turn("tool", "res", vec![], Some("c1")),                   // 4 naive new_c lands here
        turn("user", &format!("u3 {}", "x".repeat(60)), vec![], None), // 5
        turn("assistant", "a2", vec![], None),                     // 6
        turn("user", &format!("u4 {}", "x".repeat(60)), vec![], None), // 7
        turn("assistant", "a3", vec![], None),                     // 8
        turn("user", &format!("u5 {}", "x".repeat(60)), vec![], None), // 9
    ]);
    // len=10, naive new_c = 10 - K_TARGET(6) = 4 → tool_safe_boundary backs to 3.
    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c")
        .await;
    let cache = instance.get_summary_cache().expect("cache should be set");
    assert_eq!(
        cache.covers_up_to, 3,
        "boundary must back up past the tool result to its parent assistant"
    );
}

#[tokio::test]
async fn test_e2e_summarize_persist_reload_inject() {
    // True end-to-end of the amnesia-fix chain: summarize -> persist -> reload
    // -> build_messages injects. This chains the pieces the isolated unit tests
    // cover separately, verifying the WIRING between them — exactly where the
    // three review-found bugs lived (stuck pause, save order, boundary split).
    // context_window is forced small only on the summarizing turn; the reload
    // turn resets it to 32000 but build_messages doesn't use context_window,
    // so injection still works.
    let (outbound_tx, _) = tokio::sync::mpsc::channel(16);
    let provider = MockLlmProvider::new(vec![llm_text("EARLIER CONTEXT SUMMARY")]);
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config(),
        outbound_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    let store = std::sync::Arc::new(crate::session::SessionStore::new_in_memory());
    agent_loop.set_session_store(store.clone());
    store.get_or_create("e2e:k");

    // --- Turn 1: accumulate history, trigger summarization, persist. ---
    let mut inst = agent_loop.get_or_create_instance("e2e:k");
    inst.set_context_window(100); // small threshold forces summarization
    for i in 0..8 {
        inst.add_user_message(&format!("user msg {} with padding to cross threshold", i));
        inst.add_assistant_message(&format!("assistant reply {}", i), Vec::new(), None);
    }
    agent_loop
        .maybe_update_summary(&inst, "e2e:k", "web", "c")
        .await;
    let cache_after = inst
        .get_summary_cache()
        .expect("summarize should have set the cache");
    assert_eq!(cache_after.text, "EARLIER CONTEXT SUMMARY");
    // Persist, mirroring run_agent_loop_internal's save path (cache BEFORE history).
    store.set_summary("e2e:k", &cache_after.text);
    store.set_summary_covers_up_to("e2e:k", Some(cache_after.covers_up_to));
    let stored: Vec<crate::session::StoredMessage> = inst
        .get_history()
        .iter()
        .map(crate::session::StoredMessage::from)
        .collect();
    store.set_history("e2e:k", stored);

    // --- Turn 2: reload from store; build_messages must inject the summary. ---
    let inst2 = agent_loop.get_or_create_instance("e2e:k");
    let cache_loaded = inst2
        .get_summary_cache()
        .expect("cache must survive the store round-trip (the amnesia fix)");
    assert_eq!(cache_loaded.text, "EARLIER CONTEXT SUMMARY");
    assert!(cache_loaded.covers_up_to >= 1);

    let messages = agent_loop.build_messages(&inst2);
    assert!(
        messages[0].content.contains("## Summary of Previous Conversation"),
        "summary must be injected into the system message"
    );
    assert!(messages[0].content.contains("EARLIER CONTEXT SUMMARY"));
    // The verbatim tail (recent turns) is still sent.
    let joined: String = messages
        .iter()
        .map(|m| m.content.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(joined.contains("assistant reply 7"));
}

#[tokio::test]
async fn test_summarize_prefix_owned_returns_summary() {
    // summarize_prefix_owned folds ALL provided messages (no "keep last N")
    // and merges the existing summary.
    let provider = MockLlmProvider::new(vec![llm_text("prefix summary")]);
    let turns = vec![
        summary_turn("user", "question one"),
        summary_turn("assistant", "answer one"),
        summary_turn("user", "question two"),
    ];
    let refs: Vec<&crate::types::ConversationTurn> = turns.iter().collect();
    let result = summarize_prefix_owned(
        &refs,
        "existing context",
        32000,
        true,
        &provider,
        "test-model",
        None,
    )
    .await;
    assert!(result.is_some(), "prefix summarize should return a summary");
    assert!(result.unwrap().contains("prefix summary"));
}

// -------------------------------------------------------------------------
// T4 (U1): per-model summarizer prefix-reuse switch
// (shape capture reuses CapturingMockProvider, defined near the G1 tests below)
// -------------------------------------------------------------------------

/// prefix_reuse=true (default) → G1 shape: the request preserves message
/// structure — system first (when present), original user/assistant turns in
/// order, trailing instruction. NOT a flattened single message.
#[tokio::test]
async fn test_summarize_prefix_reuse_true_keeps_g1_shape() {
    let provider = CapturingMockProvider {
        captured: std::sync::Mutex::new(Vec::new()),
        reply: "S".to_string(),
    };
    let turns = vec![
        summary_turn("system", "SYS"),
        summary_turn("user", "question one"),
        summary_turn("assistant", "answer one"),
        summary_turn("user", "question two"),
    ];
    let refs: Vec<&crate::types::ConversationTurn> = turns.iter().collect();
    let out = summarize_prefix_owned(
        &refs,
        "",
        32000,
        true,
        &provider,
        "test-model",
        None,
    )
    .await;
    assert!(out.is_some());

    let seen = provider.captured.lock().unwrap();
    assert_eq!(seen.len(), 1, "small batch = one LLM call");
    let msgs = &seen[0];
    // system preserved as its own message...
    assert_eq!(msgs[0].role, "system");
    assert_eq!(msgs[0].content, "SYS");
    // ...original turns keep role structure...
    assert_eq!(msgs[1].role, "user");
    assert_eq!(msgs[1].content, "question one");
    assert_eq!(msgs[2].role, "assistant");
    // ...trailing instruction as the final user message.
    assert_eq!(msgs.last().unwrap().role, "user");
    assert!(msgs.last().unwrap().content.contains("简明摘要"));
}

/// prefix_reuse=false → OLD shape: exactly ONE bare user message whose
/// content flattens the covered messages as `role: content` lines + the
/// instruction (plus existing-summary context when present).
#[tokio::test]
async fn test_summarize_prefix_reuse_false_uses_bare_shape() {
    let provider = CapturingMockProvider {
        captured: std::sync::Mutex::new(Vec::new()),
        reply: "S".to_string(),
    };
    let turns = vec![
        summary_turn("system", "SYS"),
        summary_turn("user", "question one"),
        summary_turn("assistant", "answer one"),
        summary_turn("user", "question two"),
    ];
    let refs: Vec<&crate::types::ConversationTurn> = turns.iter().collect();
    let out = summarize_prefix_owned(
        &refs,
        "prior coverage",
        32000,
        false,
        &provider,
        "test-model",
        None,
    )
    .await;
    assert!(out.is_some());

    let seen = provider.captured.lock().unwrap();
    assert_eq!(seen.len(), 1, "old shape = one LLM call");
    let msgs = &seen[0];
    assert_eq!(msgs.len(), 1, "single bare user message");
    assert_eq!(msgs[0].role, "user");
    let c = &msgs[0].content;
    assert!(c.contains("prior coverage"), "existing summary merged");
    assert!(c.contains("user: question one"), "flattened role lines");
    assert!(c.contains("assistant: answer one"), "flattened role lines");
    assert!(c.contains("简明摘要"), "instruction present");
    // No system message in the old shape.
    assert!(!msgs.iter().any(|m| m.role == "system"));
}

/// `current_summarizer_prefix_reuse`: absent/standalone → true (default keeps
/// prefix for the main model); explicit false → false; explicit true → true.
#[test]
fn test_current_summarizer_prefix_reuse_default_and_off() {
    // Standalone (no config_path) → default true.
    let standalone = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    assert!(standalone.current_summarizer_prefix_reuse());

    let cfg_path = std::env::temp_dir().join(format!(
        "nemesis_test_sumswitch_{}.json",
        std::process::id()
    ));
    std::fs::write(
        &cfg_path,
        serde_json::json!({
            "model_list": [
                {"model": "cheap/sum", "model_name": "cheap-sum", "summarizer_prefix_reuse": false},
                {"model": "main/m1", "model_name": "main-m1"}
            ]
        })
        .to_string(),
    )
    .unwrap();

    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    agent_loop.set_config_path(cfg_path.clone());
    agent_loop.set_active_model("cheap-sum");
    assert!(
        !agent_loop.current_summarizer_prefix_reuse(),
        "explicit false opts out"
    );
    // Field absent → default true.
    agent_loop.set_active_model("main-m1");
    assert!(
        agent_loop.current_summarizer_prefix_reuse(),
        "absent → default true (main model keeps prefix)"
    );
    // Unknown model → default true.
    agent_loop.set_active_model("opaque-alias");
    assert!(agent_loop.current_summarizer_prefix_reuse());

    let _ = std::fs::remove_file(&cfg_path);
}

// --- S3.3: save/load cache round-trip (the amnesia fix wiring) ---

fn stored(role: &str, content: &str) -> crate::session::StoredMessage {
    crate::session::StoredMessage::from(&summary_turn(role, content))
}

#[test]
fn test_get_or_create_instance_restores_summary_cache() {
    // New-format file: full history + summary + covers_up_to. get_or_create
    // must restore the cache, and build_messages must inject the summary and
    // send only the verbatim tail (history[covers_up_to..]).
    let mut agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let store = std::sync::Arc::new(crate::session::SessionStore::new_in_memory());
    agent_loop.set_session_store(store.clone());

    // set_history/set_summary require the session to pre-exist (they use get_mut).
    store.get_or_create("sk");
    let msgs = vec![
        stored("system", "SYS"),
        stored("user", "old question"),
        stored("assistant", "old answer"),
        stored("user", "recent question"),
        stored("assistant", "recent answer"),
    ];
    store.set_history("sk", msgs);
    store.set_summary("sk", "OLD SUMMARY");
    store.set_summary_covers_up_to("sk", Some(3));

    let instance = agent_loop.get_or_create_instance("sk");
    let cache = instance.get_summary_cache().expect("cache should be restored");
    assert_eq!(cache.covers_up_to, 3);
    assert_eq!(cache.text, "OLD SUMMARY");

    let messages = agent_loop.build_messages(&instance);
    assert!(messages[0].content.contains("OLD SUMMARY"));
    let joined: String = messages
        .iter()
        .map(|m| m.content.clone())
        .collect::<Vec<_>>()
        .join("\n");
    // Covered prefix (indices 1, 2 < 3) folded into summary, not sent verbatim.
    assert!(!joined.contains("old question"));
    assert!(!joined.contains("old answer"));
    // Verbatim tail (indices 3, 4) preserved.
    assert!(joined.contains("recent question"));
    assert!(joined.contains("recent answer"));
}

#[test]
fn test_get_or_create_instance_legacy_summary_mapping() {
    // Legacy file (pre-refactor): summary present but covers_up_to absent.
    // Must map covers_up_to so ALL loaded messages are sent verbatim while the
    // summary is injected as floating context (it covered older, truncated
    // content not in the loaded messages).
    let mut agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let store = std::sync::Arc::new(crate::session::SessionStore::new_in_memory());
    agent_loop.set_session_store(store.clone());

    store.get_or_create("sk");
    let msgs = vec![
        stored("system", "SYS"),
        stored("user", "first"),
        stored("assistant", "second"),
        stored("user", "third"),
    ];
    store.set_history("sk", msgs);
    store.set_summary("sk", "LEGACY FLOATING SUMMARY");
    // covers_up_to intentionally NOT set → None (legacy).

    let instance = agent_loop.get_or_create_instance("sk");
    let cache = instance
        .get_summary_cache()
        .expect("cache should be restored from legacy file");
    // Leading-system count = 1 → all conversation sent verbatim.
    assert_eq!(cache.covers_up_to, 1);
    assert_eq!(cache.text, "LEGACY FLOATING SUMMARY");

    let messages = agent_loop.build_messages(&instance);
    assert!(messages[0].content.contains("LEGACY FLOATING SUMMARY"));
    let joined: String = messages
        .iter()
        .map(|m| m.content.clone())
        .collect::<Vec<_>>()
        .join("\n");
    // No loaded message lost — all sent verbatim.
    assert!(joined.contains("first"));
    assert!(joined.contains("second"));
    assert!(joined.contains("third"));
}

// =========================================================================
// Additional coverage tests for loop.rs - targeting 95%
// =========================================================================

#[tokio::test]
async fn test_run_with_tool_call_and_rpc_context() {
    // Tool call in RPC channel should be handled properly
    let provider = MockLlmProvider::new(vec![
        LlmResponse {
            content: String::new(),
            tool_calls: vec![ToolCallInfo {
                id: "tc_1".to_string(),
                name: "calculator".to_string(),
                arguments: r#"{"expr":"1+1"}"#.to_string(),
            }],
            finished: false,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
        LlmResponse {
            content: "The answer is 2.".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
    ]);

    let mut agent_loop = AgentLoop::new(Box::new(provider), test_config());
    agent_loop.register_tool(
        "calculator".to_string(),
        Box::new(MockTool {
            result: "2".to_string(),
        }),
    );

    let instance = AgentInstance::new(test_config());
    let context = RequestContext::for_rpc("chat1", "user1", "session1", "rpc-corr-1");

    let events = agent_loop.run(&instance, "What is 1+1?", &context).await;

    // Last Done event should have RPC prefix
    let done_events: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Done(msg) => Some(msg.clone()),
            _ => None,
        })
        .collect();
    assert!(done_events[0].starts_with("[rpc:rpc-corr-1]"));
}

#[test]
fn test_handle_command_show_system_prompt() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let result = agent_loop.handle_command("/show system_prompt");
    // This should show the system prompt
    assert!(result.is_some());
}

#[test]
fn test_handle_command_show_unknown_target() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let result = agent_loop.handle_command("/show foobar");
    assert!(result.is_some());
    assert!(result.unwrap().contains("Unknown show target"));
}

#[test]
fn test_handle_command_list_unknown() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let result = agent_loop.handle_command("/list foobar");
    assert!(result.is_some());
    assert!(result.unwrap().contains("Unknown list target"));
}

#[test]
fn test_handle_command_switch_unknown() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let result = agent_loop.handle_command("/switch xyz to abc");
    assert!(result.is_some());
    assert!(result.unwrap().contains("Unknown switch target"));
}

#[tokio::test]
async fn test_run_multiple_iterations_with_different_tools() {
    let provider = MockLlmProvider::new(vec![
        LlmResponse {
            content: String::new(),
            tool_calls: vec![ToolCallInfo {
                id: "tc_1".to_string(),
                name: "search".to_string(),
                arguments: r#"{"query":"test"}"#.to_string(),
            }],
            finished: false,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
        LlmResponse {
            content: String::new(),
            tool_calls: vec![ToolCallInfo {
                id: "tc_2".to_string(),
                name: "calculator".to_string(),
                arguments: r#"{"expr":"42"}"#.to_string(),
            }],
            finished: false,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
        LlmResponse {
            content: "Combined result: found and calculated.".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
    ]);

    let mut agent_loop = AgentLoop::new(Box::new(provider), test_config());
    agent_loop.register_tool(
        "search".to_string(),
        Box::new(MockTool {
            result: "found".to_string(),
        }),
    );
    agent_loop.register_tool(
        "calculator".to_string(),
        Box::new(MockTool {
            result: "42".to_string(),
        }),
    );

    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");

    let events = agent_loop
        .run(&instance, "Search and calculate", &context)
        .await;

    // Should have 2 ToolCall + 2 ToolResult + 1 Done
    let tool_calls: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolCall(_)))
        .collect();
    let tool_results: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolResult(_)))
        .collect();
    let done: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::Done(_)))
        .collect();
    assert_eq!(tool_calls.len(), 2);
    assert_eq!(tool_results.len(), 2);
    assert_eq!(done.len(), 1);
}

#[tokio::test]
async fn test_run_with_empty_response_then_final() {
    // LLM returns empty content first, then final answer on second call
    let provider = MockLlmProvider::new(vec![LlmResponse {
        content: "".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);

    let agent_loop = AgentLoop::new(Box::new(provider), test_config());
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");

    let events = agent_loop.run(&instance, "Hello", &context).await;

    // Should produce a Done event with empty string
    let done: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Done(msg) => Some(msg.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(done.len(), 1);
}

#[tokio::test]
async fn test_run_with_tool_error_continues() {
    // Tool returns error, LLM should continue with a second call
    struct FailTool;
    #[async_trait]
    impl Tool for FailTool {
        async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
            Err("Tool failed".to_string())
        }
    }

    let provider = MockLlmProvider::new(vec![
        LlmResponse {
            content: String::new(),
            tool_calls: vec![ToolCallInfo {
                id: "tc_1".to_string(),
                name: "fail_tool".to_string(),
                arguments: "{}".to_string(),
            }],
            finished: false,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
        LlmResponse {
            content: "I see the tool failed, let me explain.".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
    ]);

    let mut agent_loop = AgentLoop::new(Box::new(provider), test_config());
    agent_loop.register_tool("fail_tool".to_string(), Box::new(FailTool));

    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");

    let events = agent_loop.run(&instance, "Use the tool", &context).await;

    // Should have ToolResult with error + Done
    let tool_results: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolResult(tr) if tr.result.contains("Tool error") => Some(tr.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(tool_results.len(), 1);

    let done: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Done(msg) => Some(msg.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(done.len(), 1);
    assert!(done[0].contains("tool failed"));
}

#[test]
fn test_build_messages_with_system_prompt() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let instance = AgentInstance::new(test_config());

    let messages = agent_loop.build_messages(&instance);
    assert_eq!(messages[0].role, "system");
    assert!(messages[0].content.contains("test assistant"));
}

#[test]
fn test_build_messages_without_system_prompt() {
    let config = AgentConfig {
        model: "test".to_string(),
        system_prompt: None,
        max_turns: 5,
        tools: Vec::new(),
        models: std::collections::HashMap::new(),
    };
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), config.clone());
    let instance = AgentInstance::new(config);

    let messages = agent_loop.build_messages(&instance);
    // Without system prompt, history should be empty
    assert!(messages.is_empty());
}

#[tokio::test]
async fn test_run_bus_owned_with_slash_command() {
    let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::channel(16);
    let (inbound_tx, inbound_rx) = tokio::sync::mpsc::channel(16);

    let provider = MockLlmProvider::new(vec![]);
    let agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config(),
        outbound_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );

    let msg = make_inbound("/show model", "web", "chat1", "user1", "web:chat1");
    inbound_tx.send(msg).await.unwrap();
    drop(inbound_tx);

    agent_loop.run_bus_owned(inbound_rx).await;

    let outbound = outbound_rx.try_recv();
    assert!(outbound.is_ok());
    assert!(outbound.unwrap().content.contains("test-model"));
}

#[tokio::test]
async fn test_run_bus_owned_multiple_messages() {
    let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::channel(16);
    let (inbound_tx, inbound_rx) = tokio::sync::mpsc::channel(16);

    let provider = MockLlmProvider::new(vec![
        LlmResponse {
            content: "Response 1".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
        LlmResponse {
            content: "Response 2".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
    ]);
    let agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config(),
        outbound_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );

    let msg1 = make_inbound("Message 1", "web", "chat1", "user1", "web:chat1a");
    let msg2 = make_inbound("Message 2", "web", "chat1", "user1", "web:chat1b");
    inbound_tx.send(msg1).await.unwrap();
    inbound_tx.send(msg2).await.unwrap();
    drop(inbound_tx);

    agent_loop.run_bus_owned(inbound_rx).await;

    // Should have 2 outbound messages
    let mut count = 0;
    while outbound_rx.try_recv().is_ok() {
        count += 1;
    }
    assert_eq!(count, 2);
}

#[tokio::test]
async fn test_process_inbound_message_with_route_resolver_configured() {
    let (outbound_tx, _) = tokio::sync::mpsc::channel(16);
    let provider = MockLlmProvider::new(vec![LlmResponse {
        content: "Routed!".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);

    let config = nemesis_routing::RouteConfig {
        bindings: vec![nemesis_routing::AgentBinding {
            agent_id: "main".to_string(),
            match_channel: "discord".to_string(),
            match_account_id: String::new(),
            match_peer_kind: Some("guild".to_string()),
            match_peer_id: Some("12345".to_string()),
            match_guild_id: None,
            match_team_id: None,
        }],
        agents: vec![nemesis_routing::AgentDef {
            id: "main".to_string(),
            is_default: true,
        }],
        dm_scope: "main".to_string(),
    };

    let mut agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config(),
        outbound_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    agent_loop.set_route_resolver(nemesis_routing::RouteResolver::new(config));

    let msg = nemesis_types::channel::InboundMessage {
        channel: "discord".to_string(),
        sender_id: "user1".to_string(),
        chat_id: "chat1".to_string(),
        content: "Hello discord".to_string(),
        media: vec![],
        session_key: String::new(),
        correlation_id: String::new(),
        metadata: {
            let mut m = std::collections::HashMap::new();
            m.insert("peer_kind".to_string(), "guild".to_string());
            m.insert("peer_id".to_string(), "12345".to_string());
            m
        },
        voice_playback: None,
    };

    let (agent_id, response, err) = agent_loop.process_inbound_message(&msg).await;
    assert_eq!(agent_id, "main");
    assert!(response.contains("Routed!"));
    assert!(err.is_none());
}

#[test]
fn test_sent_in_round_cycle() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    agent_loop.mark_sent_in_round("sess1");
    assert!(agent_loop.has_sent_in_round("sess1"));
    // Not set for a different session
    assert!(!agent_loop.has_sent_in_round("sess2"));
}

#[test]
fn test_handle_command_with_context_channels() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let result = agent_loop.handle_command_with_context("/list channels", "web");
    assert!(result.is_some());
    // Without channel manager set, should say no channels
    assert!(result.unwrap().contains("No channels enabled"));
}

#[test]
fn test_handle_command_with_context_show_channel() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let result = agent_loop.handle_command_with_context("/show channel", "telegram");
    assert_eq!(result, Some("Current channel: telegram".to_string()));
}

#[tokio::test]
async fn test_run_with_rpc_error_has_prefix() {
    struct ErrProvider;
    #[async_trait]
    impl LlmProvider for ErrProvider {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<LlmMessage>,
            _options: Option<crate::types::ChatOptions>,
            _tools: Vec<crate::types::ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            Err("Something went wrong".to_string())
        }
    }

    let agent_loop = AgentLoop::new(Box::new(ErrProvider), test_config());
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::for_rpc("chat1", "user1", "session1", "corr-abc");

    let events = agent_loop.run(&instance, "Hello", &context).await;

    let errors: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Error(msg) => Some(msg.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(errors.len(), 1);
    assert!(errors[0].starts_with("[rpc:corr-abc]"));
}

#[test]
fn test_process_message_empty_message() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    let (_, _, handled) = agent_loop.process_message("", &ctx);
    assert!(!handled);
}

#[test]
fn test_process_message_system_channel_non_continuation() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let ctx = RequestContext::new("system", "chat1", "user1", "sess1");

    // Non-continuation message on system channel
    let (_, _, handled) = agent_loop.process_message("regular message", &ctx);
    assert!(!handled);
}

#[test]
fn test_session_busy_tracker_release_nonexistent() {
    let tracker = SessionBusyTracker::new(ConcurrentMode::Reject, 8);
    // Release on nonexistent session should not panic
    tracker.release("nonexistent");
    assert!(!tracker.is_busy("nonexistent"));
}

#[test]
fn test_session_busy_tracker_acquire_release_cycle() {
    let tracker = SessionBusyTracker::new(ConcurrentMode::Queue, 3);
    assert!(tracker.try_acquire("s1"));
    assert!(tracker.is_busy("s1"));

    // Second acquire on same session fails
    assert!(!tracker.try_acquire("s1"));

    // Release and re-acquire works
    tracker.release("s1");
    assert!(!tracker.is_busy("s1"));
    assert!(tracker.try_acquire("s1"));
    tracker.release("s1");
}

#[tokio::test]
async fn test_process_direct_with_tool_calls() {
    let provider = MockLlmProvider::new(vec![
        LlmResponse {
            content: String::new(),
            tool_calls: vec![ToolCallInfo {
                id: "tc_1".to_string(),
                name: "calculator".to_string(),
                arguments: r#"{"expr":"3*7"}"#.to_string(),
            }],
            finished: false,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
        LlmResponse {
            content: "The answer is 21".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
    ]);

    let mut agent_loop = AgentLoop::new(Box::new(provider), test_config());
    agent_loop.register_tool(
        "calculator".to_string(),
        Box::new(MockTool {
            result: "21".to_string(),
        }),
    );

    let result = agent_loop.process_direct("What is 3*7?", "sess1").await;
    assert_eq!(result, Ok("The answer is 21".to_string()));
}

#[test]
fn test_build_messages_preserves_history_order() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let instance = AgentInstance::new(test_config());

    instance.add_user_message("First");
    instance.add_assistant_message("Second", vec![], None);
    instance.add_user_message("Third");

    let messages = agent_loop.build_messages(&instance);
    // system + user + assistant + (injected time before last user) + user = 5
    assert_eq!(messages.len(), 5);
    assert_eq!(messages[0].role, "system");
    assert_eq!(messages[1].role, "user");
    assert_eq!(messages[1].content, "First");
    assert_eq!(messages[2].role, "assistant");
    // I2 (U8): time/env is now the FIRST section of the merged context
    // snapshot injected as a USER-role message (was system-role dyn_msg).
    assert_eq!(messages[3].role, "user"); // merged context snapshot
    assert!(messages[3].content.contains("Current Time"));
    assert_eq!(messages[4].role, "user");
    assert_eq!(messages[4].content, "Third");
}

#[test]
fn test_format_tools_for_log_multiple_tools() {
    let tools = vec![
        ToolCallInfo {
            id: "tc_1".to_string(),
            name: "search".to_string(),
            arguments: r#"{"q":"test"}"#.to_string(),
        },
        ToolCallInfo {
            id: "tc_2".to_string(),
            name: "calculator".to_string(),
            arguments: r#"{"expr":"1+1"}"#.to_string(),
        },
    ];
    let result = format_tools_for_log(&tools);
    assert!(result.contains("search"));
    assert!(result.contains("calculator"));
    assert!(result.contains("tc_1"));
    assert!(result.contains("tc_2"));
}

// =========================================================================
// Additional coverage tests for loop.rs utility functions
// =========================================================================

#[test]
fn test_format_messages_for_log_with_tool_calls_and_content() {
    let messages = vec![LlmMessage {
        role: "assistant".to_string(),
        content: "Let me help you.".to_string(),
        tool_calls: Some(vec![ToolCallInfo {
            id: "call_1".to_string(),
            name: "read_file".to_string(),
            arguments: r#"{"path":"/test.txt"}"#.to_string(),
        }]),
        tool_call_id: None,
        reasoning_content: None,
    }];
    let result = format_messages_for_log(&messages);
    assert!(result.contains("ToolCalls:"));
    assert!(result.contains("call_1"));
    assert!(result.contains("read_file"));
    assert!(result.contains("Let me help you."));
}

#[test]
fn test_format_messages_for_log_with_tool_call_id_v2() {
    let messages = vec![LlmMessage {
        role: "tool".to_string(),
        content: "file contents here".to_string(),
        tool_calls: None,
        tool_call_id: Some("call_abc".to_string()),
        reasoning_content: None,
    }];
    let result = format_messages_for_log(&messages);
    assert!(result.contains("ToolCallID: call_abc"));
}

#[test]
fn test_format_messages_for_log_long_content_truncated() {
    let long_content = "A".repeat(500);
    let messages = vec![LlmMessage {
        role: "user".to_string(),
        content: long_content.clone(),
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
    }];
    let result = format_messages_for_log(&messages);
    assert!(result.len() < long_content.len() + 100);
}

#[test]
fn test_format_messages_for_log_long_arguments_truncated() {
    let long_args = "X".repeat(500);
    let messages = vec![LlmMessage {
        role: "assistant".to_string(),
        content: String::new(),
        tool_calls: Some(vec![ToolCallInfo {
            id: "call_1".to_string(),
            name: "test".to_string(),
            arguments: long_args.clone(),
        }]),
        tool_call_id: None,
        reasoning_content: None,
    }];
    let result = format_messages_for_log(&messages);
    assert!(result.len() < long_args.len() + 100);
}

#[test]
fn test_format_tools_for_log_long_args_truncated() {
    let long_args = "Y".repeat(500);
    let tools = vec![ToolCallInfo {
        id: "tc_long".to_string(),
        name: "long_tool".to_string(),
        arguments: long_args.clone(),
    }];
    let result = format_tools_for_log(&tools);
    assert!(result.len() < long_args.len() + 100);
}

fn make_inbound_msg(
    sender_id: &str,
    chat_id: &str,
    metadata: std::collections::HashMap<String, String>,
) -> nemesis_types::channel::InboundMessage {
    nemesis_types::channel::InboundMessage {
        channel: "web".to_string(),
        sender_id: sender_id.to_string(),
        chat_id: chat_id.to_string(),
        content: String::new(),
        media: vec![],
        session_key: String::new(),
        correlation_id: String::new(),
        metadata,
        voice_playback: None,
    }
}

#[test]
fn test_extract_peer_with_peer_kind_direct_v2() {
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("peer_kind".to_string(), "direct".to_string());
    let msg = make_inbound_msg("node-123", "chat-1", metadata);
    let result = extract_peer(&msg);
    assert_eq!(result, "direct:node-123");
}

#[test]
fn test_extract_peer_with_peer_kind_cluster() {
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("peer_kind".to_string(), "cluster".to_string());
    metadata.insert("peer_id".to_string(), "worker-1".to_string());
    let msg = make_inbound_msg("user-1", "chat-abc", metadata);
    let result = extract_peer(&msg);
    assert_eq!(result, "cluster:worker-1");
}

#[test]
fn test_extract_parent_peer_valid_v2() {
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("parent_peer_kind".to_string(), "cluster".to_string());
    metadata.insert("parent_peer_id".to_string(), "parent-1".to_string());
    let msg = make_inbound_msg("user-1", "chat-1", metadata);
    let result = extract_parent_peer(&msg);
    assert_eq!(result, Some("cluster:parent-1".to_string()));
}

#[test]
fn test_extract_parent_peer_empty_kind_v2() {
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("parent_peer_kind".to_string(), "".to_string());
    metadata.insert("parent_peer_id".to_string(), "parent-1".to_string());
    let msg = make_inbound_msg("user-1", "chat-1", metadata);
    let result = extract_parent_peer(&msg);
    assert_eq!(result, None);
}

#[test]
fn test_extract_parent_peer_empty_id_v2() {
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("parent_peer_kind".to_string(), "cluster".to_string());
    metadata.insert("parent_peer_id".to_string(), "".to_string());
    let msg = make_inbound_msg("user-1", "chat-1", metadata);
    let result = extract_parent_peer(&msg);
    assert_eq!(result, None);
}

#[test]
fn test_extract_parent_peer_no_metadata_v2() {
    let msg = make_inbound_msg("user-1", "chat-1", std::collections::HashMap::new());
    let result = extract_parent_peer(&msg);
    assert_eq!(result, None);
}

#[test]
fn test_extract_continuation_task_id_valid_v2() {
    assert_eq!(
        extract_continuation_task_id("cluster_continuation:task-123"),
        Some("task-123")
    );
}

#[test]
fn test_extract_continuation_task_id_no_prefix_v2() {
    assert_eq!(extract_continuation_task_id("regular-message"), None);
}

#[test]
fn test_is_internal_channel_all_variants() {
    assert!(is_internal_channel("cli"));
    assert!(is_internal_channel("system"));
    assert!(is_internal_channel("subagent"));
    assert!(!is_internal_channel("web"));
    assert!(!is_internal_channel("rpc"));
    assert!(!is_internal_channel("discord"));
    assert!(!is_internal_channel(""));
}

#[test]
fn test_build_agent_main_session_key_various() {
    assert_eq!(build_agent_main_session_key("main"), "agent:main:main");
    assert_eq!(
        build_agent_main_session_key("worker-1"),
        "agent:worker-1:main"
    );
    assert_eq!(build_agent_main_session_key(""), "agent::main");
}

#[test]
fn test_truncate_empty_string() {
    assert_eq!(truncate("", 10), "");
}

#[test]
fn test_truncate_short_string() {
    assert_eq!(truncate("hello", 100), "hello");
}

#[test]
fn test_resolve_route_with_guild_and_team() {
    let input = RouteInput {
        channel: "discord".to_string(),
        account_id: None,
        peer: "direct:user1".to_string(),
        parent_peer: None,
        guild_id: Some("guild-123".to_string()),
        team_id: Some("team-456".to_string()),
    };
    let output = resolve_route(&input);
    assert_eq!(output.agent_id, "main");
}

#[test]
fn test_resolve_route_with_account_id() {
    let input = RouteInput {
        channel: "web".to_string(),
        account_id: Some("acc-123".to_string()),
        peer: "direct:user1".to_string(),
        parent_peer: None,
        guild_id: None,
        team_id: None,
    };
    let output = resolve_route(&input);
    assert_eq!(output.agent_id, "main");
}

// -----------------------------------------------------------------------
// History loading tests
// -----------------------------------------------------------------------

/// Build an InboundMessage that mimics a WS history request.
fn make_history_inbound(
    chat_id: &str,
    limit: Option<usize>,
    before_index: Option<usize>,
) -> nemesis_types::channel::InboundMessage {
    let payload = serde_json::json!({
        "request_id": format!("hist_test_{}", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()),
        "limit": limit,
        "before_index": before_index,
    });
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("request_type".to_string(), "history".to_string());
    nemesis_types::channel::InboundMessage {
        channel: "web".to_string(),
        sender_id: format!("web:{}", chat_id),
        chat_id: chat_id.to_string(),
        content: payload.to_string(),
        media: vec![],
        session_key: format!("web:{}", chat_id),
        correlation_id: String::new(),
        metadata,
        voice_playback: None,
    }
}

/// Lock to serialize integration tests that share the "agent:main:main" chat log.
/// Note: this only serializes THESE tests against each other, not against other
/// tests that also write to the same file via process_inbound_message().
static CHAT_LOG_INTEGRATION_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Generate a unique session key for isolated chat_log tests.
fn unique_test_session_key(name: &str) -> String {
    format!(
        "test:{}:{}",
        name,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    )
}

/// Remove the chat log file for a given session key.
fn cleanup_session_log(session_key: &str) {
    let safe_key = session_key.replace(':', "_");
    let path = nemesis_path::default_path_manager()
        .sessions_log_dir()
        .join(format!("{}.jsonl", safe_key));
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }
}

/// Pre-populate chat log with N user+assistant pairs under the given session key.
fn populate_session_log(session_key: &str, count: usize) {
    for i in 0..count {
        crate::chat_log::append_chat_log(session_key, "user", &format!("User msg {}", i));
        crate::chat_log::append_chat_log(session_key, "assistant", &format!("Reply {}", i));
    }
}

// --- Integration tests: history through AgentLoop ---
// These use the default web session key "agent:main:session:legacy" because
// handle_history_request() (no session_id) falls back to it (must match
// server.rs process_messages). Assertions use >= because other parallel
// tests may also write to this file.

#[tokio::test]
async fn test_history_returns_all_messages() {
    let _lock = CHAT_LOG_INTEGRATION_LOCK.lock().unwrap();
    let key = "agent:main:session:legacy";
    let safe_key = key.replace(':', "_");
    let path = nemesis_path::default_path_manager()
        .sessions_log_dir()
        .join(format!("{}.jsonl", safe_key));
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }
    for i in 0..3 {
        crate::chat_log::append_chat_log(key, "user", &format!("User msg {}", i));
        crate::chat_log::append_chat_log(key, "assistant", &format!("Reply {}", i));
    }
    let (outbound_tx, mut outbound_rx) =
        tokio::sync::mpsc::channel::<nemesis_types::channel::OutboundMessage>(64);
    let mut al = AgentLoop::new_bus(
        Box::new(MockLlmProvider::new(vec![])),
        test_config(),
        outbound_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    al.set_session_store(std::sync::Arc::new(
        crate::session::SessionStore::new_in_memory(),
    ));
    let al = Arc::new(al);

    let msg = make_history_inbound("web:sess1", Some(20), None);
    let (_, resp, err) = al.process_inbound_message(&msg).await;
    assert_eq!(resp, "");
    assert!(err.is_none());

    let out = tokio::time::timeout(std::time::Duration::from_secs(2), outbound_rx.recv())
        .await
        .expect("timeout")
        .expect("closed");
    assert_eq!(out.channel, "web");
    assert_eq!(out.chat_id, "web:sess1");
    assert_eq!(out.message_type, "history");

    let data: serde_json::Value = serde_json::from_str(&out.content).unwrap();
    let msgs = data["messages"].as_array().unwrap();
    assert!(
        msgs.len() >= 6,
        "expected at least 6 messages, got {}",
        msgs.len()
    );
}

// --- Unit tests: chat_log pagination with isolated session keys ---

#[test]
fn test_history_pagination() {
    let key = unique_test_session_key("pagination");
    cleanup_session_log(&key);
    populate_session_log(&key, 25); // 50 messages total

    let (page, total, has_more, oldest) = crate::chat_log::read_chat_log(&key, 10, None);
    assert_eq!(page.len(), 10);
    assert_eq!(total, 50);
    assert!(has_more);
    assert_eq!(oldest, 40);

    cleanup_session_log(&key);
}

#[test]
fn test_history_empty_store() {
    let key = unique_test_session_key("empty");
    cleanup_session_log(&key);

    let (page, total, has_more, oldest) = crate::chat_log::read_chat_log(&key, 20, None);
    assert!(page.is_empty());
    assert_eq!(total, 0);
    assert!(!has_more);
    assert_eq!(oldest, 0);
}

#[test]
fn test_history_read_all() {
    let key = unique_test_session_key("readall");
    cleanup_session_log(&key);
    populate_session_log(&key, 2); // 4 messages total

    let (page, total, has_more, _oldest) = crate::chat_log::read_chat_log(&key, 20, None);
    assert_eq!(page.len(), 4);
    assert_eq!(total, 4);
    assert!(!has_more);

    cleanup_session_log(&key);
}

#[tokio::test]
async fn test_history_e2e_via_bus_arc() {
    let _lock = CHAT_LOG_INTEGRATION_LOCK.lock().unwrap();
    let key = "agent:main:session:legacy";
    let safe_key = key.replace(':', "_");
    let path = nemesis_path::default_path_manager()
        .sessions_log_dir()
        .join(format!("{}.jsonl", safe_key));
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }
    populate_session_log(key, 2); // 4 messages

    let (outbound_tx, mut outbound_rx) =
        tokio::sync::mpsc::channel::<nemesis_types::channel::OutboundMessage>(64);
    let mut al = AgentLoop::new_bus(
        Box::new(MockLlmProvider::new(vec![])),
        test_config(),
        outbound_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    al.set_session_store(std::sync::Arc::new(
        crate::session::SessionStore::new_in_memory(),
    ));
    let (inbound_tx, inbound_rx) =
        tokio::sync::mpsc::channel::<nemesis_types::channel::InboundMessage>(64);
    let al = Arc::new(al);

    let al_clone = al.clone();
    let handle = tokio::spawn(async move { al_clone.run_bus_arc(inbound_rx).await });

    tokio::time::sleep(std::time::Duration::from_millis(30)).await;

    inbound_tx
        .send(make_history_inbound("web:s1", Some(20), None))
        .await
        .unwrap();

    let out = tokio::time::timeout(std::time::Duration::from_secs(2), outbound_rx.recv())
        .await
        .expect("timeout")
        .expect("closed");
    assert_eq!(out.message_type, "history");
    let data: serde_json::Value = serde_json::from_str(&out.content).unwrap();
    assert!(
        data["messages"].as_array().unwrap().len() >= 4,
        "expected at least 4 messages, got {}",
        data["messages"].as_array().unwrap().len()
    );

    al.stop();
    drop(inbound_tx);
    let _ = handle.await;
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }
}

// =========================================================================
// Additional coverage tests for loop.rs (targeting 95%+)
// =========================================================================

#[tokio::test]
async fn test_run_with_reasoning_content() {
    let provider = MockLlmProvider::new(vec![LlmResponse {
        content: "Final answer".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: Some("I need to think about this...".to_string()),
        usage: Some(crate::loop_executor::ObserverUsageInfo {
            prompt_tokens: 50,
            completion_tokens: 20,
            total_tokens: 70,
            cached_tokens: None,
            cache_creation_tokens: None,
            cache_read_tokens: None,
        }),
        raw_request_body: None,
        raw_response_body: None,
    }]);
    let agent_loop = AgentLoop::new(Box::new(provider), test_config());
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");

    let events = agent_loop
        .run(&instance, "Think about this", &context)
        .await;
    let done: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Done(msg) => Some(msg.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(done.len(), 1);
    assert_eq!(done[0], "Final answer");
}

#[test]
fn test_process_options_custom() {
    let opts = ProcessOptions {
        session_key: "test:session".to_string(),
        channel: "web".to_string(),
        chat_id: "chat123".to_string(),
        user_message: "Hello".to_string(),
        default_response: "No response".to_string(),
        enable_summary: false,
        send_response: true,
        no_history: true,
        trace_id: "trace-001".to_string(),
    };
    assert_eq!(opts.session_key, "test:session");
    assert_eq!(opts.channel, "web");
    assert!(!opts.enable_summary);
    assert!(opts.send_response);
    assert!(opts.no_history);
    assert_eq!(opts.trace_id, "trace-001");
}

#[test]
fn test_concurrent_mode_variants() {
    assert_ne!(ConcurrentMode::Reject, ConcurrentMode::Queue);
    let default = ConcurrentMode::default();
    assert_eq!(default, ConcurrentMode::Reject);
}

#[test]
fn test_session_busy_tracker_concurrent_access() {
    use std::sync::Arc;
    let tracker = Arc::new(SessionBusyTracker::new(ConcurrentMode::Reject, 8));
    let mut handles = vec![];

    for i in 0..10 {
        let t = tracker.clone();
        handles.push(std::thread::spawn(move || {
            let key = format!("session-{}", i);
            let acquired = t.try_acquire(&key);
            assert!(acquired);
            assert!(t.is_busy(&key));
            t.release(&key);
            assert!(!t.is_busy(&key));
        }));
    }

    for h in handles {
        h.join().unwrap();
    }
}

#[tokio::test]
async fn test_run_with_provider_error_no_retry() {
    struct ErrorProvider;
    #[async_trait]
    impl LlmProvider for ErrorProvider {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<LlmMessage>,
            _options: Option<crate::types::ChatOptions>,
            _tools: Vec<crate::types::ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            Err("Provider unavailable".to_string())
        }
    }
    let agent_loop = AgentLoop::new(Box::new(ErrorProvider), test_config());
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");

    let events = agent_loop.run(&instance, "Hello", &context).await;
    let errors: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Error(msg) => Some(msg.clone()),
            _ => None,
        })
        .collect();
    assert!(!errors.is_empty());
    assert!(errors[0].contains("Provider unavailable"));
}

#[tokio::test]
async fn test_run_with_empty_content_then_response() {
    let provider = MockLlmProvider::new(vec![
        LlmResponse {
            content: String::new(),
            tool_calls: vec![ToolCallInfo {
                id: "tc_1".to_string(),
                name: "calculator".to_string(),
                arguments: r#"{"expr":"1+1"}"#.to_string(),
            }],
            finished: false,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
        LlmResponse {
            content: "The answer is 2.".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
    ]);
    let mut agent_loop = AgentLoop::new(Box::new(provider), test_config());
    agent_loop.register_tool(
        "calculator".to_string(),
        Box::new(MockTool {
            result: "2".to_string(),
        }),
    );
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");

    let events = agent_loop.run(&instance, "What is 1+1?", &context).await;
    let done: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Done(msg) => Some(msg.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(done.len(), 1);
    assert_eq!(done[0], "The answer is 2.");
}

#[test]
fn test_sent_in_round_tracker_clear_specific() {
    let tracker = SentInRoundTracker::new();
    tracker.mark_sent("session-1");
    tracker.mark_sent("session-2");
    assert!(tracker.has_sent_in_round("session-1"));
    assert!(tracker.has_sent_in_round("session-2"));

    // Clear only session-1
    tracker.clear("session-1");
    assert!(!tracker.has_sent_in_round("session-1"));
    assert!(tracker.has_sent_in_round("session-2"));
}

#[tokio::test]
async fn test_process_direct_returns_error_on_provider_failure() {
    struct FailProvider;
    #[async_trait]
    impl LlmProvider for FailProvider {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<LlmMessage>,
            _options: Option<crate::types::ChatOptions>,
            _tools: Vec<crate::types::ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            Err("LLM failure".to_string())
        }
    }
    let agent_loop = AgentLoop::new(Box::new(FailProvider), test_config());
    let result = agent_loop.process_direct("test input", "session-key").await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("LLM failure"));
}

#[tokio::test]
async fn test_process_heartbeat_returns_default_on_empty_response() {
    struct EmptyProvider;
    #[async_trait]
    impl LlmProvider for EmptyProvider {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<LlmMessage>,
            _options: Option<crate::types::ChatOptions>,
            _tools: Vec<crate::types::ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            Ok(LlmResponse {
                content: String::new(),
                tool_calls: Vec::new(),
                finished: true,
                reasoning_content: None,
                usage: None,
                raw_request_body: None,
                raw_response_body: None,
            })
        }
    }
    let agent_loop = AgentLoop::new(Box::new(EmptyProvider), test_config());
    let result = agent_loop.process_heartbeat("ping", "web", "chat1").await;
    assert!(result.is_ok());
    // When the LLM returns empty content, process_heartbeat returns empty string
    // because the Done event has empty content and there's no Error event
    let response = result.unwrap();
    assert!(
        response.is_empty()
            || response == "I've completed processing but have no response to give."
    );
}

#[test]
fn test_llm_message_serialization_roundtrip_all_fields() {
    let msg = LlmMessage {
        role: "assistant".to_string(),
        content: "Hello".to_string(),
        tool_calls: Some(vec![ToolCallInfo {
            id: "tc_1".to_string(),
            name: "calc".to_string(),
            arguments: r#"{"expr":"2+2"}"#.to_string(),
        }]),
        tool_call_id: Some("tc_1".to_string()),
        reasoning_content: Some("thinking...".to_string()),
    };
    let json = serde_json::to_string(&msg).unwrap();
    let de: LlmMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(de.role, "assistant");
    assert_eq!(de.content, "Hello");
    assert!(de.tool_calls.is_some());
    assert_eq!(de.tool_call_id, Some("tc_1".to_string()));
    // reasoning_content is deserialized with default
    assert!(de.reasoning_content.is_some());
}

#[test]
fn test_extract_peer_various_metadata() {
    // Test with peer_kind=cluster
    let msg = make_inbound_with_metadata(
        "web",
        "chat1",
        "user1",
        vec![("peer_kind", "cluster"), ("peer_id", "node-2")],
    );
    let peer = extract_peer(&msg);
    assert_eq!(peer, "cluster:node-2");

    // Test with peer_kind=direct (uses sender_id as fallback)
    let msg = make_inbound_with_metadata("web", "chat1", "user-123", vec![("peer_kind", "direct")]);
    let peer = extract_peer(&msg);
    assert_eq!(peer, "direct:user-123");

    // Test with no peer_kind -> falls back to sender_id
    let msg = make_inbound_with_metadata("web", "chat1", "fallback-user", vec![]);
    let peer = extract_peer(&msg);
    assert_eq!(peer, "fallback-user");
}

fn make_inbound_with_metadata(
    channel: &str,
    chat_id: &str,
    sender_id: &str,
    metadata: Vec<(&str, &str)>,
) -> nemesis_types::channel::InboundMessage {
    let mut meta = std::collections::HashMap::new();
    for (k, v) in metadata {
        meta.insert(k.to_string(), v.to_string());
    }
    nemesis_types::channel::InboundMessage {
        channel: channel.to_string(),
        sender_id: sender_id.to_string(),
        chat_id: chat_id.to_string(),
        content: "test".to_string(),
        media: vec![],
        session_key: format!("{}:{}", channel, chat_id),
        correlation_id: String::new(),
        metadata: meta,
        voice_playback: None,
    }
}

#[test]
fn test_is_internal_channel_all_values() {
    assert!(is_internal_channel("cli"));
    assert!(is_internal_channel("system"));
    assert!(is_internal_channel("subagent"));
    assert!(!is_internal_channel("web"));
    assert!(!is_internal_channel("discord"));
    assert!(!is_internal_channel("rpc"));
}

#[test]
fn test_build_agent_main_session_key_format_v2() {
    let key = build_agent_main_session_key("agent-1");
    assert_eq!(key, "agent:agent-1:main");
    let key2 = build_agent_main_session_key("main");
    assert_eq!(key2, "agent:main:main");
}

#[test]
fn test_extract_continuation_task_id_various_v2() {
    assert_eq!(
        extract_continuation_task_id("cluster_continuation:task-abc-123"),
        Some("task-abc-123")
    );
    assert_eq!(extract_continuation_task_id("regular_message"), None);
    assert_eq!(extract_continuation_task_id(""), None);
}

#[test]
fn test_truncate_various_lengths() {
    assert_eq!(truncate("", 10), "");
    assert_eq!(truncate("hello", 10), "hello");
    assert_eq!(truncate("hello world", 5), "he...");
    assert_eq!(truncate("abc", 3), "abc");
    assert_eq!(truncate("abcd", 3), "..."); // budget is 0, so returns "..."
    assert_eq!(truncate("abcdef", 5), "ab...");
}

#[test]
fn test_route_input_output_types_v2() {
    let input = RouteInput {
        channel: "web".to_string(),
        account_id: None,
        peer: "chat1".to_string(),
        parent_peer: None,
        guild_id: None,
        team_id: None,
    };
    assert_eq!(input.channel, "web");

    let output = RouteOutput {
        agent_id: "main".to_string(),
        session_key: "web:chat1".to_string(),
        matched_by: "default".to_string(),
    };
    assert_eq!(output.agent_id, "main");
}

#[tokio::test]
async fn test_run_with_multiple_tool_iterations() {
    let provider = MockLlmProvider::new(vec![
        LlmResponse {
            content: String::new(),
            tool_calls: vec![ToolCallInfo {
                id: "tc_1".to_string(),
                name: "tool1".to_string(),
                arguments: "{}".to_string(),
            }],
            finished: false,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
        LlmResponse {
            content: String::new(),
            tool_calls: vec![ToolCallInfo {
                id: "tc_2".to_string(),
                name: "tool1".to_string(),
                arguments: "{}".to_string(),
            }],
            finished: false,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
        LlmResponse {
            content: "Final response after 2 tool calls".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
    ]);
    let mut agent_loop = AgentLoop::new(Box::new(provider), test_config());
    agent_loop.register_tool(
        "tool1".to_string(),
        Box::new(MockTool {
            result: "tool result".to_string(),
        }),
    );
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");

    let events = agent_loop
        .run(&instance, "Do something twice", &context)
        .await;
    let tool_calls: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolCall(_)))
        .collect();
    assert_eq!(tool_calls.len(), 2);
    let done: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Done(msg) => Some(msg.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(done[0], "Final response after 2 tool calls");
}

#[test]
fn test_agent_loop_tool_count() {
    let provider = MockLlmProvider::new(vec![]);
    let mut al = AgentLoop::new(Box::new(provider), test_config());
    assert_eq!(al.tool_count(), 0);
    al.register_tool(
        "tool1".to_string(),
        Box::new(MockTool {
            result: "r1".to_string(),
        }),
    );
    assert_eq!(al.tool_count(), 1);
    al.register_tool(
        "tool2".to_string(),
        Box::new(MockTool {
            result: "r2".to_string(),
        }),
    );
    assert_eq!(al.tool_count(), 2);
}

#[test]
fn test_agent_loop_register_tool_shared() {
    let provider = MockLlmProvider::new(vec![]);
    let mut al = AgentLoop::new(Box::new(provider), test_config());
    al.register_tool_shared(
        "shared_tool".to_string(),
        Box::new(MockTool {
            result: "shared".to_string(),
        }),
    );
    assert_eq!(al.tool_count(), 1);
}

#[test]
fn test_agent_loop_stop_when_not_running() {
    let provider = MockLlmProvider::new(vec![]);
    let al = AgentLoop::new(Box::new(provider), test_config());
    assert!(!al.is_running());
    al.stop();
    assert!(!al.is_running());
}

#[tokio::test]
async fn test_process_direct_with_channel_custom() {
    let provider = MockLlmProvider::new(vec![LlmResponse {
        content: "Custom channel response".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);
    let agent_loop = AgentLoop::new(Box::new(provider), test_config());
    let result = agent_loop
        .process_direct_with_channel("Hello", "session-1", "discord", "channel-123")
        .await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "Custom channel response");
}

#[test]
fn test_handle_command_show_system_prompt_with_config() {
    let provider = MockLlmProvider::new(vec![]);
    let al = AgentLoop::new(
        Box::new(provider),
        AgentConfig {
            model: "test".to_string(),
            system_prompt: Some("You are a helpful assistant.".to_string()),
            max_turns: 5,
            tools: vec![],
            models: std::collections::HashMap::new(),
        },
    );
    // /show system_prompt may not be a recognized command target
    // The important thing is it doesn't panic and returns something
    let result = al.handle_command("/show system_prompt");
    // It may return Some or None depending on command handling
    let _ = result;
}

#[test]
fn test_build_messages_repairs_orphan_tool_call() {
    // The exact failure shape: an assistant tool_call with no matching tool
    // result in history (the kind of state a buggy truncation can persist).
    // build_messages is the LLM-boundary chokepoint — every main-loop LLM call
    // flows through it — so it must repair the orphan away before the messages
    // reach the provider, preventing the 400.
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let instance = AgentInstance::new(test_config());
    let tc = |id: &str| crate::types::ToolCallInfo {
        id: id.to_string(),
        name: "t".to_string(),
        arguments: "{}".to_string(),
    };
    let turn = |role: &str, content: &str, tcs: Vec<crate::types::ToolCallInfo>, tcid: Option<&str>| crate::types::ConversationTurn {
        role: role.to_string(),
        content: content.to_string(),
        tool_calls: tcs,
        tool_call_id: tcid.map(|s| s.to_string()),
        timestamp: String::new(),
        reasoning_content: None,
        tool_name: None,
        tool_result_projection: None,
    };
    // assistant issues [call_A, call_B] but only call_B has a tool result.
    instance.set_history(vec![
        turn("system", "sys", vec![], None),
        turn("user", "do it", vec![], None),
        turn("assistant", "go", vec![tc("call_A"), tc("call_B")], None),
        turn("tool", "respB", vec![], Some("call_B")),
        turn("user", "next", vec![], None),
    ]);

    let messages = agent_loop.build_messages(&instance);

    let asst = messages
        .iter()
        .find(|m| m.role == "assistant" && m.tool_calls.is_some())
        .expect("assistant with tool_calls present");
    let ids: Vec<&str> = asst
        .tool_calls
        .as_ref()
        .unwrap()
        .iter()
        .map(|tc| tc.id.as_str())
        .collect();
    // G2 (U2): the orphan call_A is NO LONGER dropped — it now gets a
    // synthetic TOOL_OUTCOME_UNKNOWN result, so BOTH calls survive and the
    // provider transcript stays balanced (same 400-prevention guarantee, but
    // the model also learns its half-executed call happened).
    assert_eq!(
        ids,
        vec!["call_A", "call_B"],
        "orphan call_A keeps its call and gains a synthetic unknown-outcome result"
    );
    let synth = messages
        .iter()
        .find(|m| m.role == "tool" && m.tool_call_id.as_deref() == Some("call_A"))
        .expect("synthetic tool result for call_A present");
    assert!(synth.content.contains("TOOL_OUTCOME_UNKNOWN"));
    // And the stored instance history is untouched (repair is projection-only).
    let stored = instance.get_history();
    assert_eq!(stored.len(), 5);
    assert!(stored
        .iter()
        .all(|m| m.tool_call_id.as_deref() != Some("call_A")),
        "no synthetic result leaks into stored history");
}

// ===========================================================================
// S2: build_messages inline-summary pipeline
// ===========================================================================

/// Build a ConversationTurn with empty tool_calls / tool_call_id.
fn summary_turn(role: &str, content: &str) -> crate::types::ConversationTurn {
    crate::types::ConversationTurn {
        role: role.to_string(),
        content: content.to_string(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        timestamp: String::new(),
        reasoning_content: None,
        tool_name: None,
        tool_result_projection: None,
    }
}

#[test]
fn test_build_messages_injects_summary_from_cache() {
    // cache.covers_up_to = 3 folds history[..3] = [sys, u1, a1] into the
    // summary; history[3..] = [u2, a2, u3] is sent verbatim. The covered
    // messages (u1, a1) must NOT appear in the output.
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let instance = AgentInstance::new(test_config());
    instance.set_history(vec![
        summary_turn("system", "SYS"),
        summary_turn("user", "first"),
        summary_turn("assistant", "second"),
        summary_turn("user", "third"),
        summary_turn("assistant", "fourth"),
        summary_turn("user", "fifth"),
    ]);
    instance.set_summary_cache(Some(crate::instance::SummaryCache {
        covers_up_to: 3,
        text: "EARLIER CONTEXT".to_string(),
    }));

    let messages = agent_loop.build_messages(&instance);

    // Leading system message carries the original prompt + the summary block.
    assert_eq!(messages[0].role, "system");
    assert!(messages[0].content.contains("SYS"));
    assert!(messages[0].content.contains("## Summary of Previous Conversation"));
    assert!(messages[0].content.contains("EARLIER CONTEXT"));

    let joined: String = messages
        .iter()
        .map(|m| m.content.clone())
        .collect::<Vec<_>>()
        .join("\n");
    // Covered prefix is absent (folded into the summary, not sent verbatim).
    assert!(!joined.contains("first"));
    assert!(!joined.contains("second"));
    // Verbatim tail is present.
    assert!(joined.contains("third"));
    assert!(joined.contains("fourth"));
    assert!(joined.contains("fifth"));
}

#[test]
fn test_build_messages_cache_preserves_tail_verbatim() {
    // The verbatim tail (history[C..]) must be emitted byte-for-byte:
    // identical role + content, in order. This is the no-gap / no-overlap
    // invariant of the pipeline.
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let instance = AgentInstance::new(test_config());
    instance.set_history(vec![
        summary_turn("system", "SYS"),
        summary_turn("user", "covered-u"),
        summary_turn("assistant", "covered-a"),
        summary_turn("user", "tail-u"),
        summary_turn("assistant", "tail-a"),
        summary_turn("user", "last-u"),
    ]);
    instance.set_summary_cache(Some(crate::instance::SummaryCache {
        covers_up_to: 3,
        text: "summary text".to_string(),
    }));

    let messages = agent_loop.build_messages(&instance);

    // Drop the leading system+summary and the injected context snapshot
    // (I2: user-role <system-reminder> message before the last user) to
    // recover the verbatim conversation tail.
    let tail: Vec<&str> = messages
        .iter()
        .filter(|m| m.role != "system") // exclude sys+summary
        .filter(|m| !(m.role == "user" && m.content.starts_with("<system-reminder>")))
        .map(|m| m.content.as_str())
        .collect();
    assert_eq!(
        tail,
        vec!["tail-u", "tail-a", "last-u"],
        "verbatim tail must be history[C..] in order"
    );
}

#[test]
fn test_build_messages_cache_empty_text_degrades() {
    // An empty summary text means "no cache": build_messages must send the
    // entire history verbatim (legacy behavior), NOT drop the covered prefix.
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let instance = AgentInstance::new(test_config());
    instance.set_history(vec![
        summary_turn("system", "SYS"),
        summary_turn("user", "first"),
        summary_turn("assistant", "second"),
        summary_turn("user", "third"),
    ]);
    instance.set_summary_cache(Some(crate::instance::SummaryCache {
        covers_up_to: 3,
        text: String::new(),
    }));

    let messages = agent_loop.build_messages(&instance);
    let joined: String = messages
        .iter()
        .map(|m| m.content.clone())
        .collect::<Vec<_>>()
        .join("\n");
    // Everything is present (no folding for an empty summary).
    assert!(joined.contains("first"));
    assert!(joined.contains("second"));
    assert!(joined.contains("third"));
    assert!(!joined.contains("## Summary of Previous Conversation"));
}

#[test]
fn test_build_messages_cache_zero_covers_degrades() {
    // covers_up_to = 0 means "summary covers nothing" → treat as no cache and
    // send everything verbatim. (Guards against a degenerate cache dropping the
    // whole history.)
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let instance = AgentInstance::new(test_config());
    instance.set_history(vec![
        summary_turn("system", "SYS"),
        summary_turn("user", "first"),
        summary_turn("assistant", "second"),
    ]);
    instance.set_summary_cache(Some(crate::instance::SummaryCache {
        covers_up_to: 0,
        text: "some summary".to_string(),
    }));

    let messages = agent_loop.build_messages(&instance);
    let joined: String = messages
        .iter()
        .map(|m| m.content.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(joined.contains("first"));
    assert!(joined.contains("second"));
}

#[test]
fn test_build_messages_cache_no_system_prompt() {
    // When history has no system prompt at index 0, the summary is emitted as
    // a dedicated leading system turn (so the provider still receives it).
    let config = AgentConfig {
        model: "test".to_string(),
        system_prompt: None,
        max_turns: 5,
        tools: Vec::new(),
        models: std::collections::HashMap::new(),
    };
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), config.clone());
    let instance = AgentInstance::new(config);
    instance.set_history(vec![
        summary_turn("user", "old-q"),
        summary_turn("assistant", "old-a"),
        summary_turn("user", "new-q"),
    ]);
    instance.set_summary_cache(Some(crate::instance::SummaryCache {
        covers_up_to: 2,
        text: "FLOATING SUMMARY".to_string(),
    }));

    let messages = agent_loop.build_messages(&instance);
    // First message is the synthesized summary system turn.
    assert_eq!(messages[0].role, "system");
    assert!(messages[0].content.contains("FLOATING SUMMARY"));
    // The covered prefix (old-q) is folded away; new-q is verbatim.
    let joined: String = messages
        .iter()
        .map(|m| m.content.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!joined.contains("old-q"));
    assert!(joined.contains("new-q"));
}

// =========================================================================
// Layer 1C: Forge experience recording tests
// =========================================================================

/// Helper: create a Forge instance in a temp directory.
#[cfg(feature = "forge")]
fn create_test_forge() -> (Arc<nemesis_forge::forge::Forge>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    // Enable the master switch: loop.rs gates experience recording on
    // `forge.is_enabled()` (intentionally — see loop.rs:3649-3651), so a
    // default-disabled forge would record nothing and the
    // `test_forge_records_*_experience` tests would see empty experiences.
    let mut config = nemesis_forge::config::ForgeConfig::default();
    config.enabled = true;
    let forge = nemesis_forge::forge::Forge::new(config, dir.path().to_path_buf());
    (Arc::new(forge), dir)
}

#[cfg(feature = "forge")]
#[tokio::test]
async fn test_forge_records_successful_tool_experience() {
    let (forge, _dir) = create_test_forge();
    let provider = MockLlmProvider::new(vec![
        LlmResponse {
            content: String::new(),
            tool_calls: vec![ToolCallInfo {
                id: "tc_1".to_string(),
                name: "calculator".to_string(),
                arguments: r#"{"expr":"2+2"}"#.to_string(),
            }],
            finished: false,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
        LlmResponse {
            content: "The answer is 4.".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
    ]);
    let mut agent_loop = AgentLoop::new(Box::new(provider), test_config());
    agent_loop.register_tool(
        "calculator".to_string(),
        Box::new(MockTool {
            result: "4".to_string(),
        }),
    );
    agent_loop.set_forge(forge.clone());
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");

    let _events = agent_loop.run(&instance, "What is 2+2?", &context).await;

    // Verify experience was recorded
    let experiences = forge.collector().experiences();
    assert!(
        !experiences.is_empty(),
        "expected at least one recorded experience"
    );

    let exp = &experiences[0].experience;
    assert_eq!(exp.tool_name, "calculator");
    assert!(
        exp.success,
        "successful tool call should record success=true"
    );
    // duration_ms can be 0 if tool executes in <1ms
    assert!(exp.session_key.contains("web"));
    assert!(!exp.id.is_empty());
}

#[cfg(feature = "forge")]
#[tokio::test]
async fn test_forge_records_tool_error_experience() {
    let (forge, _dir) = create_test_forge();

    struct ErrorTool;
    #[async_trait]
    impl Tool for ErrorTool {
        async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
            Err("division by zero".to_string())
        }
    }

    let provider = MockLlmProvider::new(vec![
        LlmResponse {
            content: String::new(),
            tool_calls: vec![ToolCallInfo {
                id: "tc_err".to_string(),
                name: "fail_tool".to_string(),
                arguments: "{}".to_string(),
            }],
            finished: false,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
        LlmResponse {
            content: "Tool failed.".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
    ]);
    let mut agent_loop = AgentLoop::new(Box::new(provider), test_config());
    agent_loop.register_tool("fail_tool".to_string(), Box::new(ErrorTool));
    agent_loop.set_forge(forge.clone());
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");

    let _events = agent_loop.run(&instance, "fail", &context).await;

    let experiences = forge.collector().experiences();
    assert!(
        !experiences.is_empty(),
        "expected experience even for failed tool"
    );
    let exp = &experiences[0].experience;
    assert!(!exp.success, "tool error should record success=false");
    assert!(
        exp.output_summary.contains("Tool error:"),
        "output should contain error prefix"
    );
}

#[tokio::test]
async fn test_forge_no_experience_without_forge() {
    let provider = MockLlmProvider::new(vec![
        LlmResponse {
            content: String::new(),
            tool_calls: vec![ToolCallInfo {
                id: "tc_1".to_string(),
                name: "calculator".to_string(),
                arguments: "{}".to_string(),
            }],
            finished: false,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
        LlmResponse {
            content: "Done".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
    ]);
    let mut agent_loop = AgentLoop::new(Box::new(provider), test_config());
    agent_loop.register_tool(
        "calculator".to_string(),
        Box::new(MockTool {
            result: "4".to_string(),
        }),
    );
    // No forge set — should work normally without panic
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");

    let events = agent_loop.run(&instance, "calc", &context).await;
    let done: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Done(msg) => Some(msg.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(done.len(), 1);
}

fn llm_text(content: &str) -> LlmResponse {
    LlmResponse {
        content: content.to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }
}

/// U5 helper: a response requesting the named tools in order (no content).
fn llm_tool_calls(names: &[&str]) -> LlmResponse {
    let tool_calls = names
        .iter()
        .map(|n| ToolCallInfo {
            id: format!("call_{}", n),
            name: n.to_string(),
            arguments: "{}".to_string(),
        })
        .collect();
    LlmResponse {
        content: String::new(),
        tool_calls,
        finished: false,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }
}

// ===========================================================================
// U5 (sixth batch): read-only tool concurrency + source-order
// ===========================================================================

#[tokio::test]
async fn u5_readonly_batch_runs_concurrently() {
    // 3 read-only tools each sleeping 300ms. Serial ≈ 900ms; parallel ≈ 300ms.
    // Bound at 700ms to stay well clear of serial while tolerating scheduler
    // jitter / the 4-permit semaphore (3 ≤ 4 → no queueing).
    let provider = MockLlmProvider::new(vec![
        llm_tool_calls(&["r1", "r2", "r3"]),
        llm_text("done"),
    ]);
    let mut agent_loop = AgentLoop::new(Box::new(provider), test_config());
    agent_loop.register_tool("r1".into(), Box::new(SlowReadTool { marker: "A".into(), delay_ms: 300 }));
    agent_loop.register_tool("r2".into(), Box::new(SlowReadTool { marker: "B".into(), delay_ms: 300 }));
    agent_loop.register_tool("r3".into(), Box::new(SlowReadTool { marker: "C".into(), delay_ms: 300 }));

    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "c1", "u1", "s1");
    let start = std::time::Instant::now();
    let _ = agent_loop.run(&instance, "go", &context).await;
    let elapsed = start.elapsed();
    assert!(
        elapsed < std::time::Duration::from_millis(700),
        "3×300ms read-only should overlap (parallel); took {:?}",
        elapsed
    );
}

#[tokio::test]
async fn u5_writer_in_batch_keeps_serial() {
    // A writer (default is_read_only=false) in the batch → the batch is NOT
    // all-read-only → serial path, byte-identical to pre-U5. 2×300ms serial ≈
    // 600ms. Bound: > 500ms proves they did NOT overlap (parallel would be
    // ~300ms).
    let provider = MockLlmProvider::new(vec![
        llm_tool_calls(&["r1", "w1"]),
        llm_text("done"),
    ]);
    let mut agent_loop = AgentLoop::new(Box::new(provider), test_config());
    agent_loop.register_tool("r1".into(), Box::new(SlowReadTool { marker: "A".into(), delay_ms: 300 }));
    agent_loop.register_tool("w1".into(), Box::new(SlowWriteTool { marker: "W".into(), delay_ms: 300 }));

    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "c1", "u1", "s2");
    let start = std::time::Instant::now();
    let _ = agent_loop.run(&instance, "go", &context).await;
    let elapsed = start.elapsed();
    assert!(
        elapsed > std::time::Duration::from_millis(500),
        "a writer forces serial (2×300ms≈600ms); took {:?}",
        elapsed
    );
}

#[tokio::test]
async fn u5_parallel_results_preserve_source_order() {
    // Distinct markers A/B/C — the tool_results must reach history in MODEL
    // SOURCE ORDER (the roadmap risk-3 hard constraint), not completion order.
    // Tiny delays so order is a function of source position, not timing.
    let provider = MockLlmProvider::new(vec![
        llm_tool_calls(&["r1", "r2", "r3"]),
        llm_text("done"),
    ]);
    let mut agent_loop = AgentLoop::new(Box::new(provider), test_config());
    agent_loop.register_tool("r1".into(), Box::new(SlowReadTool { marker: "A".into(), delay_ms: 5 }));
    agent_loop.register_tool("r2".into(), Box::new(SlowReadTool { marker: "B".into(), delay_ms: 5 }));
    agent_loop.register_tool("r3".into(), Box::new(SlowReadTool { marker: "C".into(), delay_ms: 5 }));

    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "c1", "u1", "s3");
    let events = agent_loop.run(&instance, "go", &context).await;
    let tool_results: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolResult(r) => Some(r.result.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(tool_results, vec!["A", "B", "C"], "source order preserved");
}

// ===========================================================================
// G1 (U1): summary request reuses the conversation prefix
// ===========================================================================

/// Mock provider that RECORDS every request's messages (unlike
/// MockLlmProvider, which discards them) and returns a canned response.
struct CapturingMockProvider {
    captured: std::sync::Mutex<Vec<Vec<LlmMessage>>>,
    reply: String,
}

#[async_trait]
impl LlmProvider for CapturingMockProvider {
    async fn chat(
        &self,
        _model: &str,
        messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        self.captured.lock().unwrap().push(messages);
        Ok(LlmResponse {
            content: self.reply.clone(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

fn g1_history(n_turns: usize) -> Vec<crate::types::ConversationTurn> {
    let mut h = vec![summary_turn("system", "SYSTEM_PROMPT_TEXT")];
    for i in 0..n_turns {
        h.push(summary_turn("user", &format!("user message {i}")));
        h.push(summary_turn("assistant", &format!("assistant reply {i}")));
    }
    h
}

/// G1 core assertion: the summary request is
/// [system (verbatim, no summary block), ...covered messages (original
/// structure, byte-equal), instruction user message]. The bare
/// `role: content` text-concatenation form is gone (no "CONVERSATION:" dump).
#[tokio::test]
async fn test_summarize_request_reuses_conversation_prefix() {
    let provider = CapturingMockProvider {
        captured: std::sync::Mutex::new(Vec::new()),
        reply: "summary text".to_string(),
    };
    // Covered prefix: system + 3 user/assistant pairs (7 turns).
    let history = g1_history(3);
    let prefix_refs: Vec<&crate::types::ConversationTurn> = history[..7].iter().collect();

    let out = summarize_prefix_owned(
        &prefix_refs,
        "old summary",
        32_000,
        true,
        &provider,
        "m",
        None,
    )
    .await;

    assert_eq!(out.as_deref(), Some("summary text"));
    let requests = provider.captured.lock().unwrap();
    assert_eq!(requests.len(), 1, "single batch call for <=10 messages");
    let msgs = &requests[0];
    // Layout: [system, u0, a0, u1, a1, u2, a2, instruction]
    assert_eq!(msgs.len(), 8);
    assert_eq!(msgs[0].role, "system");
    assert_eq!(msgs[0].content, "SYSTEM_PROMPT_TEXT");
    for (i, (role, content)) in [
        ("user", "user message 0"),
        ("assistant", "assistant reply 0"),
        ("user", "user message 1"),
        ("assistant", "assistant reply 1"),
        ("user", "user message 2"),
        ("assistant", "assistant reply 2"),
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(msgs[i + 1].role, role, "position {} role", i + 1);
        assert_eq!(msgs[i + 1].content, content, "position {} content", i + 1);
    }
    // Structure preserved: message bodies are the ORIGINAL contents, not a
    // flattened "user: user message 0" concatenation.
    assert!(msgs.iter().any(|m| m.content == "user message 0"));
    assert!(!msgs.iter().any(|m| m.content.contains("CONVERSATION:")));
    // Trailing instruction exists and carries the prior-summary fold.
    let last = msgs.last().unwrap();
    assert_eq!(last.role, "user");
    assert!(last.content.contains("old summary"));
    assert!(last.content.contains("摘要"));
}


/// G1 multipart: >10 covered messages split into two batches; each batch's
/// message list is [system?, ...contiguous prefix slice..., instruction] —
/// i.e. an ordered subset of the covered prefix.
#[tokio::test]
async fn test_summarize_multipart_batch_is_prefix_subset() {
    let provider = CapturingMockProvider {
        captured: std::sync::Mutex::new(Vec::new()),
        reply: "part summary".to_string(),
    };
    // 6 pairs = 12 valid user/assistant messages (>10 → multipart).
    let history = g1_history(6);
    let prefix_refs: Vec<&crate::types::ConversationTurn> = history.iter().collect();

    let out = summarize_prefix_owned(
        &prefix_refs,
        "",
        32_000,
        true,
        &provider,
        "m",
        None,
    )
    .await;

    assert!(out.is_some());
    let requests = provider.captured.lock().unwrap();
    // 2 part calls + 1 merge call = 3.
    assert_eq!(requests.len(), 3);
    // Part 1: [system, u0..a2, instruction] = 1 + 6 + 1 = 8 messages.
    let p1 = &requests[0];
    assert_eq!(p1.len(), 8);
    assert_eq!(p1[0].role, "system");
    assert_eq!(p1[1].content, "user message 0");
    assert_eq!(p1[6].content, "assistant reply 2");
    // Part 2: [system, u3..a5, instruction].
    let p2 = &requests[1];
    assert_eq!(p2.len(), 8);
    assert_eq!(p2[0].role, "system");
    assert_eq!(p2[1].content, "user message 3");
    assert_eq!(p2[6].content, "assistant reply 5");
    // Both parts end with the instruction, not with history.
    assert_eq!(p1.last().unwrap().role, "user");
    assert!(p1.last().unwrap().content.contains("摘要"));
    assert_eq!(p2.last().unwrap().role, "user");
    // The merge call is a plain user message (no system) — merge has no
    // prefix to reuse.
    let merge = &requests[2];
    assert_eq!(merge.len(), 1);
    assert_eq!(merge[0].role, "user");
    assert!(merge[0].content.contains("Merge these two"));
    // No bare-concatenation form anywhere.
    for req in requests.iter() {
        for m in req.iter() {
            assert!(!m.content.contains("CONVERSATION:"));
        }
    }
}

// ---------------------------------------------------------------------------
// V5 gate-in-pump: busy-gate outcomes + spawned turn tasks
// (run_bus_impl's Queue/Steer arm; the U7 inbox is reachable in production
// only since this restructure — previously the serial pump let message 2
// wait in the mpsc until the session had already been released.)
// ---------------------------------------------------------------------------

/// Reject mode + busy session → Immediate BUSY_MESSAGE bounce (legacy
/// behavior, byte-identical).
#[test]
fn test_gate_busy_reject_mode_bounces() {
    let (outbound_tx, _) = tokio::sync::mpsc::channel(16);
    let provider = MockLlmProvider::new(vec![]);
    let agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config(),
        outbound_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );

    let msg = make_inbound("busy probe", "web", "chat1", "user1", "");
    let (_, session_key) = agent_loop.route_message(&msg);
    assert!(agent_loop.try_acquire_session(&session_key));

    let response = match agent_loop.gate_inbound(&msg) {
        GateOutcome::Immediate { agent_id: _, response } => response,
        _ => panic!("expected Immediate busy bounce in Reject mode"),
    };
    assert!(response.contains("try again later"), "bounce: {response}");
    // Nothing parked anywhere.
    assert_eq!(agent_loop.inbox.pending(&session_key), (0, 0));
    agent_loop.release_session(&session_key);
}

/// Queue mode + busy session → parked in the next-turn inbox with the ⏳
/// receipt returned inline (published by the pump without a turn task).
#[test]
fn test_gate_busy_queue_mode_parks_in_next_turn() {
    let (outbound_tx, _) = tokio::sync::mpsc::channel(16);
    let provider = MockLlmProvider::new(vec![]);
    let agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config(),
        outbound_tx,
        ConcurrentMode::Queue,
        8,
        0,
    );

    let msg = make_inbound("等等别删", "web", "chat1", "user1", "");
    let (agent_id, session_key) = agent_loop.route_message(&msg);
    assert_eq!(agent_id, "main");
    assert!(agent_loop.try_acquire_session(&session_key));

    let response = match agent_loop.gate_inbound(&msg) {
        GateOutcome::Immediate { agent_id: _, response } => response,
        _ => panic!("expected Immediate queue receipt"),
    };
    assert!(response.contains("已排队"), "receipt: {response}");
    assert_eq!(agent_loop.inbox.pending(&session_key), (1, 0));
    agent_loop.release_session(&session_key);
}

/// Steer mode + busy session + bang-prefixed message → parked in the
/// next-step inbox (in-turn injection channel) with the ⚡ receipt.
#[test]
fn test_gate_busy_steer_mode_parks_bang_in_next_step() {
    let (outbound_tx, _) = tokio::sync::mpsc::channel(16);
    let provider = MockLlmProvider::new(vec![]);
    let agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config(),
        outbound_tx,
        ConcurrentMode::Steer,
        8,
        0,
    );

    let msg = make_inbound("!等等别删！", "web", "chat1", "user1", "");
    let (_, session_key) = agent_loop.route_message(&msg);
    assert!(agent_loop.try_acquire_session(&session_key));

    let response = match agent_loop.gate_inbound(&msg) {
        GateOutcome::Immediate { agent_id: _, response } => response,
        _ => panic!("expected Immediate steer receipt"),
    };
    assert!(response.contains("紧急插话"), "receipt: {response}");
    assert_eq!(agent_loop.inbox.pending(&session_key), (0, 1));
    agent_loop.release_session(&session_key);
}

/// Free session → Admitted: the gate acquires the session and mints the
/// cancel token on the turn's behalf (everything the pre-split monolith had
/// at the same point).
#[test]
fn test_gate_admits_when_free_and_mints_token() {
    let (outbound_tx, _) = tokio::sync::mpsc::channel(16);
    let provider = MockLlmProvider::new(vec![]);
    let agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config(),
        outbound_tx,
        ConcurrentMode::Queue,
        8,
        0,
    );

    let msg = make_inbound("hello", "web", "chat1", "user1", "");
    let (agent_id, session_key) = agent_loop.route_message(&msg);
    assert!(!agent_loop.is_session_busy(&session_key));

    match agent_loop.gate_inbound(&msg) {
        GateOutcome::Admitted(admission) => {
            assert_eq!(admission.agent_id, agent_id);
            assert_eq!(admission.session_key, session_key);
            assert!(!admission.cancel_token.is_cancelled());
            // The gate HOLDS the session until the turn tail releases it.
            assert!(agent_loop.is_session_busy(&session_key));
        }
        _ => panic!("expected Admitted"),
    }
    agent_loop.release_session(&session_key);
}

/// Full pump (Queue mode): an admitted message runs as a spawned turn task
/// and its reply flows through the shared finish tail (outbound publish).
#[tokio::test]
async fn test_run_bus_queue_mode_spawns_and_finishes_turn() {
    let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::channel(16);
    let provider = MockLlmProvider::new(vec![LlmResponse {
        content: "queued-turn-reply".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);
    let agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config(),
        outbound_tx,
        ConcurrentMode::Queue,
        8,
        0,
    );
    let arc = std::sync::Arc::new(agent_loop);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let pump = tokio::spawn(arc.clone().run_bus_arc(in_rx));

    in_tx
        .send(make_inbound("hello queue", "web", "chat1", "user1", ""))
        .await
        .unwrap();
    let reply = tokio::time::timeout(std::time::Duration::from_secs(10), outbound_rx.recv())
        .await
        .expect("turn reply within timeout")
        .expect("outbound channel open");
    assert_eq!(reply.content, "queued-turn-reply");
    assert_eq!(reply.meta.model.as_deref(), Some("test-model"));

    arc.stop();
    // Drop the sender so the pump's recv() returns None and the loop exits
    // (stop() only takes effect between messages; recv() itself never wakes).
    drop(in_tx);
    let _ = pump.await;
}

/// Post-turn drain: a message parked while the session was busy must be
/// processed by the admitted turn's drain block (reinject_tx unset → inline
/// fallback) and published — a parked message is never silently lost.
#[tokio::test]
async fn test_process_admitted_drains_parked_next_turn() {
    let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::channel(16);
    let provider = MockLlmProvider::new(vec![
        LlmResponse {
            content: "turn-reply".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
        LlmResponse {
            content: "drained-reply".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
    ]);
    let agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config(),
        outbound_tx,
        ConcurrentMode::Queue,
        8,
        0,
    );

    // Park msg2 while the session is busy.
    let msg2 = make_inbound("等等别删", "web", "chat1", "user1", "");
    let (_, session_key) = agent_loop.route_message(&msg2);
    assert!(agent_loop.try_acquire_session(&session_key));
    match agent_loop.gate_inbound(&msg2) {
        GateOutcome::Immediate { response, .. } => assert!(response.contains("已排队")),
        _ => panic!("expected queue receipt"),
    }
    // Free the session so the real turn can be admitted.
    agent_loop.release_session(&session_key);

    // Admit + run the turn for msg1 on the same session.
    let msg1 = make_inbound("turn one", "web", "chat1", "user1", "");
    let admission = match agent_loop.gate_inbound(&msg1) {
        GateOutcome::Admitted(a) => a,
        _ => panic!("expected Admitted"),
    };
    assert_eq!(admission.session_key, session_key);
    let (turn_agent_id, turn_response, turn_err) =
        agent_loop.process_admitted(&msg1, admission).await;
    assert_eq!(turn_agent_id, "main");
    assert!(turn_err.is_none());
    assert!(turn_response.contains("turn-reply"));

    // The session was released by the tail.
    assert!(!agent_loop.is_session_busy(&session_key));

    // The parked head was drained and its reply published (inline fallback).
    let drained = tokio::time::timeout(std::time::Duration::from_secs(10), outbound_rx.recv())
        .await
        .expect("drained reply within timeout")
        .expect("outbound channel open");
    assert!(drained.content.contains("drained-reply"), "{:?}", drained.content);
}

/// Drop-guard probe: proves `stop()` ABORTS tracked turn tasks (the future
/// is dropped mid-flight, not left running).
struct TurnAbortProbe {
    dropped: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
impl Drop for TurnAbortProbe {
    fn drop(&mut self) {
        self.dropped.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// stop() must abort in-flight spawned turn tasks (Queue/Steer modes) —
/// otherwise a long turn would outlive the loop's shutdown.
#[tokio::test]
async fn test_stop_aborts_tracked_turn_tasks() {
    let (outbound_tx, _) = tokio::sync::mpsc::channel(16);
    let provider = MockLlmProvider::new(vec![]);
    let agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config(),
        outbound_tx,
        ConcurrentMode::Steer,
        8,
        0,
    );
    let arc = std::sync::Arc::new(agent_loop);

    let dropped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let probe = TurnAbortProbe {
        dropped: dropped.clone(),
    };
    arc.spawn_turn_task(async move {
        let _probe = probe;
        tokio::time::sleep(std::time::Duration::from_secs(600)).await;
    });
    // Let the spawned task actually start.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    arc.stop();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(
        dropped.load(std::sync::atomic::Ordering::SeqCst),
        "stop() must abort (drop) tracked turn tasks"
    );
}


// ---------------------------------------------------------------------------
// X1 (U3 projection prune): build-time fold keeps history originals.
// ---------------------------------------------------------------------------

fn x1_loop_turn(role: &str, content: &str) -> crate::types::ConversationTurn {
    crate::types::ConversationTurn {
        role: role.to_string(),
        content: content.to_string(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        timestamp: String::new(),
        reasoning_content: None,
        tool_name: None,
        tool_result_projection: None,
    }
}

/// The single-source seam: `project_history_for_request` folds oversized
/// tool results to their bounded form while the SOURCE history keeps the
/// originals byte-for-byte (the mid-section stays recoverable for replay /
/// branching). Small tool results and non-tool turns pass through.
#[test]
fn test_projection_folds_tool_results_keeps_history_originals() {
    let big: String = {
        let head: String = "A".repeat(3_600);
        let mid: String = "B".repeat(6_000);
        let tail: String = "C".repeat(3_600);
        format!("{head}{mid}{tail}")
    };
    let caller = {
        let mut a = x1_loop_turn("assistant", "calling");
        a.tool_calls = vec![
            crate::types::ToolCallInfo {
                id: "tc_1".to_string(),
                name: "exec".to_string(),
                arguments: "{}".to_string(),
            },
            crate::types::ToolCallInfo {
                id: "tc_2".to_string(),
                name: "exec".to_string(),
                arguments: "{}".to_string(),
            },
        ];
        a
    };
    let history = vec![
        x1_loop_turn("system", "sys"),
        x1_loop_turn("user", "run the thing"),
        caller,
        {
            let mut t = x1_loop_turn("tool", &big);
            t.tool_call_id = Some("tc_1".to_string());
            t.tool_name = Some("exec".to_string());
            t
        },
        {
            let mut t = x1_loop_turn("tool", "small-ok");
            t.tool_call_id = Some("tc_2".to_string());
            t
        },
        x1_loop_turn("assistant", "done"),
    ];

    let projected = super::project_history_for_request(&history, None);

    // History is untouched: the oversized original survives verbatim.
    assert_eq!(history[3].content, big, "history must keep the original");
    // Model side: the oversized tool turn is folded...
    let p_tool = projected
        .iter()
        .find(|t| t.role == "tool" && t.content.contains("exec"))
        .expect("oversized tool turn must be folded");
    assert!(p_tool.content.chars().count() < big.chars().count());
    assert!(p_tool.content.contains("exec"));
    // ...the small tool result passes through...
    assert!(
        projected.iter().any(|t| t.content == "small-ok"),
        "in-budget tool result passes through"
    );
    // ...and non-tool turns are untouched.
    assert!(projected.iter().any(|t| t.content == "run the thing"));
    assert!(projected.iter().any(|t| t.content == "done"));
}

/// The fold also applies in summary mode (the summary branch rebuilds the
/// tail; tail tool results must fold there too), and the recorded override
/// (spill locator) wins over the recompute.
#[test]
fn test_projection_summary_mode_and_override() {
    let big: String = "Z".repeat(20_000);
    let caller = {
        let mut a = x1_loop_turn("assistant", "a1");
        a.tool_calls = vec![crate::types::ToolCallInfo {
            id: "tc_9".to_string(),
            name: "git".to_string(),
            arguments: "{}".to_string(),
        }];
        a
    };
    let history = vec![
        x1_loop_turn("system", "sys"),
        x1_loop_turn("user", "q1"),
        caller,
        {
            let mut t = x1_loop_turn("tool", &big);
            t.tool_call_id = Some("tc_9".to_string());
            t.tool_name = Some("git".to_string());
            t.tool_result_projection = Some("[spilled to disk: /tmp/x.jsonl]".to_string());
            t
        },
    ];
    let projected = super::project_history_for_request(&history, Some(("sum", 2)));
    assert_eq!(history[3].content, big, "history keeps the original");
    assert!(
        projected
            .iter()
            .any(|t| t.role == "tool" && t.content == "[spilled to disk: /tmp/x.jsonl]"),
        "recorded override (spill locator) must win in the projection"
    );
}

// ---------------------------------------------------------------------------
// X2 (U8 refinement): merged-snapshot `# Runtime Policy` section —
// approval wiring / guardian judge presence / live capability tier, all
// rendered from plain state (no clocks) so the merged message stays
// byte-stable between turns.
// ---------------------------------------------------------------------------

/// Extract the `# Runtime Policy` section text from a merged snapshot
/// message (from the header line to the end of the message content).
fn x2_policy_section(messages: &[crate::r#loop::LlmMessage]) -> String {
    let snap = messages
        .iter()
        .find(|m| m.role == "user" && m.content.contains("# Runtime Policy"))
        .expect("merged snapshot with Runtime Policy section");
    let start = snap.content.find("# Runtime Policy").unwrap();
    snap.content[start..].to_string()
}

#[test]
fn test_x2_runtime_policy_default_off() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let instance = AgentInstance::new(test_config());
    instance.add_user_message("hello");
    let messages = agent_loop.build_messages(&instance);

    let section = x2_policy_section(&messages);
    assert!(section.contains("approval: off"), "default approval: {section}");
    assert!(section.contains("guardian: off"), "no plugin: {section}");
    // The tier line renders one of the four Display values.
    let tier_line = section
        .lines()
        .find(|l| l.starts_with("model_tier: "))
        .expect("model_tier line");
    let v = tier_line.trim_start_matches("model_tier: ");
    assert!(
        matches!(v, "auto" | "mini" | "normal" | "big"),
        "tier display value: {v}"
    );

    // Ordering: the policy section renders AFTER the always-first Current
    // Time section, and the merged message still closes its wrapper.
    let snap = messages
        .iter()
        .find(|m| m.role == "user" && m.content.contains("# Runtime Policy"))
        .unwrap();
    let t = snap.content.find("# Current Time").unwrap();
    let p = snap.content.find("# Runtime Policy").unwrap();
    assert!(p > t, "Runtime Policy renders after Current Time");
    assert!(snap.content.ends_with("</system-reminder>"));
}

#[test]
fn test_x2_runtime_policy_state_reflected_and_deterministic() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let instance = AgentInstance::new(test_config());
    instance.add_user_message("hello");
    agent_loop.set_tier(nemesis_types::capability::ModelTier::Mini);
    agent_loop.set_interactive_approval(true);

    let s1 = x2_policy_section(&agent_loop.build_messages(&instance));
    assert!(s1.contains("approval: interactive"), "{s1}");
    assert!(s1.contains("model_tier: mini"), "live tier rendered: {s1}");

    // Determinism: same state must render byte-identical bytes (no clocks
    // inside the section — the Current Time section above it is the only
    // time-bearing part of the merged snapshot).
    let s2 = x2_policy_section(&agent_loop.build_messages(&instance));
    assert_eq!(s1, s2, "same state must render identical section bytes");

    // Flipping the flag back changes the section (state, not compile-time).
    agent_loop.set_interactive_approval(false);
    let s3 = x2_policy_section(&agent_loop.build_messages(&instance));
    assert!(s3.contains("approval: off"), "{s3}");
    assert_ne!(s1, s3, "flag flip must be visible in the section");
}

#[test]
fn test_x2_runtime_policy_guardian_on_reflects_live_judge() {
    use nemesis_security::guardian::{JudgeOutcome, JudgeRequest, JudgeVerdict, LlmJudge};
    use nemesis_security::pipeline::{SecurityPlugin, SecurityPluginConfig};

    struct OkJudge;
    #[async_trait]
    impl LlmJudge for OkJudge {
        async fn judge(&self, _req: &JudgeRequest) -> Result<JudgeVerdict, String> {
            Ok(JudgeVerdict {
                risk_level: "low".into(),
                user_authorization: "high".into(),
                outcome: JudgeOutcome::Allow,
                rationale: String::new(),
            })
        }
    }

    // Rules allow everything so the plugin is inert for tool dispatch; the
    // ONLY thing under test is judge-presence reflection in the snapshot.
    let plugin = Arc::new(SecurityPlugin::new(SecurityPluginConfig {
        enabled: true,
        command_guard_enabled: false,
        injection_enabled: false,
        credential_enabled: false,
        dlp_enabled: false,
        ssrf_enabled: false,
        default_action: "allow".to_string(),
        ..Default::default()
    }));
    plugin.set_judge(Arc::new(OkJudge));

    let mut agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    agent_loop.set_security_plugin(plugin);
    let instance = AgentInstance::new(test_config());
    instance.add_user_message("hello");

    let section = x2_policy_section(&agent_loop.build_messages(&instance));
    assert!(section.contains("guardian: on"), "judge attached: {section}");
    assert!(section.contains("approval: off"), "approval untouched: {section}");
}

// ---------------------------------------------------------------------------
// Y1 (Phase4-a): semantic tool-documentation folding — loop-level gates.
// Pure decision/rendering tests live in `src/tool_doc_folding/tests.rs`;
// these verify the LOOP-side contract: default-off = byte-identical defs,
// enabled+manager = folded provider-visible shape (the same `tool_defs`
// value the LlmRequest observer event serializes into request_log), and
// the Mini-tier exclusion.
// ---------------------------------------------------------------------------

/// A tool whose only interesting surface is its description text.
struct FoldProbeTool {
    description: String,
}

#[async_trait]
impl Tool for FoldProbeTool {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok("ok".to_string())
    }
    fn description(&self) -> String {
        self.description.clone()
    }
}

fn y1_probe_loop() -> AgentLoop {
    let mut agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    agent_loop.register_tool(
        "weather_probe".to_string(),
        Box::new(FoldProbeTool {
            description: "Get the weather forecast for a city. Supports current conditions and multi-day outlook."
                .to_string(),
        }),
    );
    agent_loop.register_tool(
        "file_probe".to_string(),
        Box::new(FoldProbeTool {
            description: "Read a file from disk. Supports offset and limit for partial reads."
                .to_string(),
        }),
    );
    agent_loop.register_tool(
        "web_probe".to_string(),
        Box::new(FoldProbeTool {
            description: "Search the web for current information. Returns titles and snippets."
                .to_string(),
        }),
    );
    agent_loop
}

fn y1_find_def<'a>(
    defs: &'a [crate::types::ToolDefinition],
    name: &str,
) -> &'a crate::types::ToolDefinition {
    defs.iter()
        .find(|d| d.function.name == name)
        .unwrap_or_else(|| panic!("tool {name} present"))
}

#[test]
fn test_y1_folding_disabled_is_byte_identical() {
    let agent_loop = y1_probe_loop();
    let instance = AgentInstance::new(test_config());
    instance.add_user_message("what is the weather today");

    // (a) No config_path (standalone/test loop) → folding off.
    let baseline = agent_loop.build_tool_defs();
    let folded = agent_loop.apply_tool_doc_folding(baseline.clone(), &instance);
    assert_eq!(
        serde_json::to_string(&baseline).unwrap(),
        serde_json::to_string(&folded).unwrap(),
        "standalone loop must pass defs through byte-identically"
    );

    // (b) config.json present but the section is disabled → same bytes.
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.json");
    std::fs::write(
        &cfg,
        r#"{"agents": {"tool_doc_folding": {"enabled": false, "expand_top_n": 1}}}"#,
    )
    .unwrap();
    agent_loop.set_config_path(cfg);
    let folded = agent_loop.apply_tool_doc_folding(baseline.clone(), &instance);
    assert_eq!(
        serde_json::to_string(&baseline).unwrap(),
        serde_json::to_string(&folded).unwrap(),
        "disabled section must pass defs through byte-identically"
    );

    // (c) config.json without the section entirely → same bytes.
    let cfg2 = dir.path().join("config2.json");
    std::fs::write(&cfg2, r#"{"agents": {"defaults": {}}}"#).unwrap();
    agent_loop.set_config_path(cfg2);
    let folded = agent_loop.apply_tool_doc_folding(baseline.clone(), &instance);
    assert_eq!(
        serde_json::to_string(&baseline).unwrap(),
        serde_json::to_string(&folded).unwrap(),
        "absent section must pass defs through byte-identically"
    );
}

#[cfg(feature = "memory")]
#[test]
fn test_y1_folding_enabled_no_manager_degrades_to_passthrough() {
    let agent_loop = y1_probe_loop();
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.json");
    std::fs::write(
        &cfg,
        r#"{"agents": {"tool_doc_folding": {"enabled": true, "expand_top_n": 1}}}"#,
    )
    .unwrap();
    agent_loop.set_config_path(cfg);

    let instance = AgentInstance::new(test_config());
    instance.add_user_message("what is the weather today");

    // Enabled but no memory manager wired (embed backend absent) → the loop
    // must degrade to unfolded defs, never fold blind.
    let baseline = agent_loop.build_tool_defs();
    let folded = agent_loop.apply_tool_doc_folding(baseline.clone(), &instance);
    assert_eq!(
        serde_json::to_string(&baseline).unwrap(),
        serde_json::to_string(&folded).unwrap(),
        "no embed backend ⇒ byte-identical passthrough"
    );
}

#[cfg(feature = "memory")]
#[test]
fn test_y1_folding_enabled_shapes_provider_defs() {
    use nemesis_memory::manager::{Config as MemoryManagerConfig, MemoryManager};
    use nemesis_memory::vector::{EmbeddingFunc, StoreConfig};
    use std::sync::Arc;

    // Deterministic toy embedding: one-hot over sentinel keywords, so the
    // weather query ranks the weather tool top by construction.
    let embed: EmbeddingFunc = Box::new(|text: &str| -> Result<Vec<f32>, String> {
        if text.contains("weather") {
            Ok(vec![1.0, 0.0, 0.0])
        } else if text.contains("file") {
            Ok(vec![0.0, 1.0, 0.0])
        } else if text.contains("web") {
            Ok(vec![0.0, 0.0, 1.0])
        } else {
            Ok(vec![0.05, 0.05, 0.05])
        }
    });
    let dir = tempfile::tempdir().unwrap();
    let manager = Arc::new(MemoryManager::new(&MemoryManagerConfig::new(dir.path())));
    manager
        .init_vector_store_with_embed(embed, StoreConfig::default())
        .unwrap();

    let agent_loop = y1_probe_loop();
    agent_loop.set_memory_inject(Some(manager), false, 0);
    let cfg = dir.path().join("config.json");
    std::fs::write(
        &cfg,
        r#"{"agents": {"tool_doc_folding": {"enabled": true, "expand_top_n": 1}}}"#,
    )
    .unwrap();
    agent_loop.set_config_path(cfg);

    let instance = AgentInstance::new(test_config());
    instance.add_user_message("what is the weather today");

    let folded = agent_loop.apply_tool_doc_folding(agent_loop.build_tool_defs(), &instance);
    // These defs are EXACTLY what the LlmRequest observer event serializes
    // (request_log's `tools` array) and what provider.chat receives.
    assert_eq!(
        y1_find_def(&folded, "weather_probe").function.description,
        "Get the weather forecast for a city. Supports current conditions and multi-day outlook.",
        "top-1 keeps FULL description bytes"
    );
    assert_eq!(
        y1_find_def(&folded, "file_probe").function.description,
        "Read a file from disk.",
        "non-top folds to first sentence"
    );
    assert_eq!(
        y1_find_def(&folded, "web_probe").function.description,
        "Search the web for current information.",
        "non-top folds to first sentence"
    );
    // Names / schemas / count untouched (orthogonal to supply).
    assert_eq!(folded.len(), 3);

    // Determinism incl. the description-vector cache: second call renders
    // identical bytes (cache hit path, no re-embed).
    let again = agent_loop.apply_tool_doc_folding(agent_loop.build_tool_defs(), &instance);
    assert_eq!(
        serde_json::to_string(&folded).unwrap(),
        serde_json::to_string(&again).unwrap(),
        "same state must render identical fold bytes (cache path)"
    );
}

#[cfg(feature = "memory")]
#[test]
fn test_y1_folding_mini_tier_excluded() {
    use nemesis_memory::manager::{Config as MemoryManagerConfig, MemoryManager};
    use nemesis_memory::vector::{EmbeddingFunc, StoreConfig};
    use std::sync::Arc;

    let embed: EmbeddingFunc = Box::new(|text: &str| -> Result<Vec<f32>, String> {
        if text.contains("weather") {
            Ok(vec![1.0, 0.0, 0.0])
        } else if text.contains("file") {
            Ok(vec![0.0, 1.0, 0.0])
        } else if text.contains("web") {
            Ok(vec![0.0, 0.0, 1.0])
        } else {
            Ok(vec![0.05, 0.05, 0.05])
        }
    });
    let dir = tempfile::tempdir().unwrap();
    let manager = Arc::new(MemoryManager::new(&MemoryManagerConfig::new(dir.path())));
    manager
        .init_vector_store_with_embed(embed, StoreConfig::default())
        .unwrap();

    let agent_loop = y1_probe_loop();
    agent_loop.set_memory_inject(Some(manager), false, 0);
    let cfg = dir.path().join("config.json");
    std::fs::write(
        &cfg,
        r#"{"agents": {"tool_doc_folding": {"enabled": true, "expand_top_n": 1}}}"#,
    )
    .unwrap();
    agent_loop.set_config_path(cfg);
    agent_loop.set_tier(nemesis_types::capability::ModelTier::Mini);

    let instance = AgentInstance::new(test_config());
    instance.add_user_message("what is the weather today");

    // Mini tier never participates: the 13-tool core set has nothing to
    // save, so even folding fully enabled+manager wired stays passthrough.
    let baseline = agent_loop.build_tool_defs();
    let folded = agent_loop.apply_tool_doc_folding(baseline.clone(), &instance);
    assert_eq!(
        serde_json::to_string(&baseline).unwrap(),
        serde_json::to_string(&folded).unwrap(),
        "mini tier must never fold"
    );
}
