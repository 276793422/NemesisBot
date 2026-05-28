use super::*;
use crate::r#loop::LlmResponse;
use async_trait::async_trait;

/// Mock LLM provider that returns pre-configured responses in sequence.
struct MockProvider {
    responses: std::sync::Mutex<Vec<LlmResponse>>,
}

impl MockProvider {
    fn new(responses: Vec<LlmResponse>) -> Self {
        Self {
            responses: std::sync::Mutex::new(responses),
        }
    }
}

#[async_trait]
impl LlmProvider for MockProvider {
    async fn chat(&self, _model: &str, _messages: Vec<LlmMessage>, _options: Option<crate::types::ChatOptions>, _tools: Vec<crate::types::ToolDefinition>) -> Result<LlmResponse, String> {
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
    async fn execute(
        &self,
        _args: &str,
        _context: &RequestContext,
    ) -> Result<String, String> {
        Ok(self.result.clone())
    }
}

/// Mock observer for testing.
struct MockObserver {
    events: std::sync::Mutex<Vec<String>>,
}

impl MockObserver {
    fn new() -> Self {
        Self {
            events: std::sync::Mutex::new(Vec::new()),
        }
    }
}

impl Observer for MockObserver {
    fn on_event(&self, event: ObserverEvent) {
        let label = match &event {
            ObserverEvent::ConversationStart { .. } => "conversation_start",
            ObserverEvent::ConversationEnd { .. } => "conversation_end",
            ObserverEvent::LlmRequest { .. } => "llm_request",
            ObserverEvent::LlmResponse { .. } => "llm_response",
            ObserverEvent::ToolCall { .. } => "tool_call",
        };
        self.events.lock().unwrap().push(label.to_string());
    }
}

fn make_inbound(
    content: &str,
    channel: &str,
    correlation_id: &str,
) -> nemesis_types::channel::InboundMessage {
    nemesis_types::channel::InboundMessage {
        channel: channel.to_string(),
        sender_id: "user1".to_string(),
        chat_id: "chat1".to_string(),
        content: content.to_string(),
        media: vec![],
        session_key: "test:chat1".to_string(),
        correlation_id: correlation_id.to_string(),
        metadata: std::collections::HashMap::new(),
        voice_playback: None,
    }
}

fn test_executor_config() -> ExecutorConfig {
    ExecutorConfig {
        model: "test-model".to_string(),
        max_turns: 5,
        system_prompt: Some("You are a test assistant.".to_string()),
        event_buffer_size: 16,
    }
}

#[tokio::test]
async fn test_simple_text_response() {
    let provider = Arc::new(MockProvider::new(vec![LlmResponse {
        content: "Hello!".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]));
    let (inbound_tx, inbound_rx) = mpsc::channel(16);
    let (outbound_tx, mut outbound_rx) = mpsc::channel(16);

    let mut executor =
        AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());

    // Send a message.
    inbound_tx
        .send(make_inbound("Hi", "web", ""))
        .await
        .unwrap();
    drop(inbound_tx); // Close to terminate executor.

    executor.run().await;

    // Should have received one outbound message.
    let msg = outbound_rx.recv().await.unwrap();
    assert_eq!(msg.channel, "web");
    assert_eq!(msg.chat_id, "chat1");
    assert_eq!(msg.content, "Hello!");
}

#[tokio::test]
async fn test_tool_call_and_final_response() {
    let provider = Arc::new(MockProvider::new(vec![
        // First call: tool call.
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
        // Second call: final text.
        LlmResponse {
            content: "Found results.".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
    ]));

    let (inbound_tx, inbound_rx) = mpsc::channel(16);
    let (outbound_tx, mut outbound_rx) = mpsc::channel(16);

    let mut executor =
        AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());
    executor.register_tool(
        "search",
        Arc::new(MockTool {
            result: "search results".to_string(),
        }),
    );

    inbound_tx
        .send(make_inbound("Search for test", "web", ""))
        .await
        .unwrap();
    drop(inbound_tx);

    executor.run().await;

    let msg = outbound_rx.recv().await.unwrap();
    assert_eq!(msg.content, "Found results.");
}

#[tokio::test]
async fn test_rpc_correlation_id_formatting() {
    let provider = Arc::new(MockProvider::new(vec![LlmResponse {
        content: "Pong".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]));

    let (_inbound_tx, inbound_rx) = mpsc::channel(16);
    let (outbound_tx, mut outbound_rx) = mpsc::channel(16);

    let executor =
        AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());

    // Process an RPC message directly.
    executor
        .process_message(make_inbound("Ping", "rpc", "corr-42"))
        .await;

    let msg = outbound_rx.recv().await.unwrap();
    assert_eq!(msg.content, "[rpc:corr-42] Pong");
    assert_eq!(msg.channel, "rpc");
}

#[tokio::test]
async fn test_unknown_tool_returns_error_in_result() {
    let provider = Arc::new(MockProvider::new(vec![
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
            content: "Tool not found.".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
    ]));

    let (_inbound_tx, inbound_rx) = mpsc::channel(16);
    let (outbound_tx, mut outbound_rx) = mpsc::channel(16);

    let executor =
        AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());

    executor
        .process_message(make_inbound("Do something", "web", ""))
        .await;

    let msg = outbound_rx.recv().await.unwrap();
    assert_eq!(msg.content, "Tool not found.");
}

#[tokio::test]
async fn test_max_turns_limit() {
    // Responses that always request a tool call.
    let responses: Vec<LlmResponse> = (0..20)
        .map(|_| LlmResponse {
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
        })
        .collect();

    let provider = Arc::new(MockProvider::new(responses));
    let (inbound_tx, inbound_rx) = mpsc::channel(16);
    let (outbound_tx, mut outbound_rx) = mpsc::channel(16);

    let mut config = test_executor_config();
    config.max_turns = 3;

    let mut executor =
        AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, config);
    executor.register_tool(
        "calculator",
        Arc::new(MockTool {
            result: "0".to_string(),
        }),
    );

    inbound_tx
        .send(make_inbound("Loop test", "web", ""))
        .await
        .unwrap();
    drop(inbound_tx);

    executor.run().await;

    let msg = outbound_rx.recv().await.unwrap();
    assert!(
        msg.content.contains("Max iterations") || msg.content.is_empty(),
        "Expected max iterations error, got: {}",
        msg.content
    );
}

#[tokio::test]
async fn test_session_busy_reject_mode() {
    let provider = Arc::new(MockProvider::new(vec![
        // First response: has a tool call (will take time).
        LlmResponse {
            content: String::new(),
            tool_calls: vec![ToolCallInfo {
                id: "tc_1".to_string(),
                name: "slow_tool".to_string(),
                arguments: "{}".to_string(),
            }],
            finished: false,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
        // Second response: final.
        LlmResponse {
            content: "Done.".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
    ]));

    let (_inbound_tx, inbound_rx) = mpsc::channel(16);
    let (outbound_tx, mut outbound_rx) = mpsc::channel(16);

    let executor =
        AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());

    // Manually mark the session as busy.
    executor.busy_sessions.insert("test:chat1".to_string());

    // Processing should return busy message.
    executor
        .process_message(make_inbound("Hello", "web", ""))
        .await;

    let msg = outbound_rx.recv().await.unwrap();
    assert!(
        msg.content.contains("processing a previous request"),
        "Expected busy message, got: {}",
        msg.content
    );

    // Clean up.
    executor.busy_sessions.remove("test:chat1");
}

#[tokio::test]
async fn test_observer_events_emitted() {
    let observer = Arc::new(MockObserver::new());

    let provider = Arc::new(MockProvider::new(vec![LlmResponse {
        content: "Hello!".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]));

    let (inbound_tx, inbound_rx) = mpsc::channel(16);
    let (outbound_tx, mut outbound_rx) = mpsc::channel(16);

    let mut executor =
        AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());
    executor.set_observer(observer.clone());

    inbound_tx
        .send(make_inbound("Hi", "web", ""))
        .await
        .unwrap();
    drop(inbound_tx);

    executor.run().await;

    let _msg = outbound_rx.recv().await.unwrap();

    // Check observer events.
    let events = observer.events.lock().unwrap();
    assert!(events.contains(&"conversation_start".to_string()));
    assert!(events.contains(&"conversation_end".to_string()));
    assert!(events.contains(&"llm_request".to_string()));
    assert!(events.contains(&"llm_response".to_string()));
}

#[tokio::test]
async fn test_process_and_publish() {
    let provider = Arc::new(MockProvider::new(vec![LlmResponse {
        content: "Published response".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]));

    let (_inbound_tx, inbound_rx) = mpsc::channel(16);
    let (outbound_tx, mut outbound_rx) = mpsc::channel(16);

    let executor =
        AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());

    let context = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = executor
        .process_and_publish("sess1", "Hello", &context)
        .await
        .unwrap();

    assert_eq!(result, "Published response");

    let msg = outbound_rx.recv().await.unwrap();
    assert_eq!(msg.content, "Published response");
    assert_eq!(msg.channel, "web");
}

#[test]
fn test_tool_result_simple() {
    let result = ToolResult::simple("hello".to_string());
    assert_eq!(result.for_llm, "hello");
    assert!(result.for_user.is_empty());
    assert!(result.silent);
    assert!(!result.is_async);
    assert!(result.err.is_none());
}

#[test]
fn test_tool_result_for_llm_only() {
    let result = ToolResult::for_llm_only("internal data".to_string());
    assert_eq!(result.for_llm, "internal data");
    assert!(result.for_user.is_empty());
    assert!(result.silent);
}

#[test]
fn test_tool_result_async() {
    let result = ToolResult::async_result(
        "task-123".to_string(),
        "Processing your request...".to_string(),
    );
    assert!(result.is_async);
    assert_eq!(result.task_id, "task-123");
    assert_eq!(result.for_user, "Processing your request...");
    assert!(!result.silent);
}

#[test]
fn test_tool_result_error() {
    let result = ToolResult::error("Something went wrong".to_string());
    assert!(result.err.is_some());
    assert!(result.for_llm.contains("Something went wrong"));
}

#[test]
fn test_fallback_executor_single_candidate_success() {
    let executor = FallbackExecutor::new();
    let candidates = vec![FallbackCandidate {
        provider: "test".to_string(),
        model: "model-1".to_string(),
    }];

    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(executor.execute(&candidates, |_p, _m| async {
        Ok(LlmResponse {
            content: "success".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }));

    assert!(result.is_ok());
    let fb = result.unwrap();
    assert_eq!(fb.model, "model-1");
    assert_eq!(fb.response.content, "success");
    assert_eq!(fb.attempts, 1);
}

#[test]
fn test_fallback_executor_all_fail() {
    let executor = FallbackExecutor::new();
    let candidates = vec![
        FallbackCandidate {
            provider: "test".to_string(),
            model: "model-1".to_string(),
        },
        FallbackCandidate {
            provider: "test".to_string(),
            model: "model-2".to_string(),
        },
    ];

    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(executor.execute(&candidates, |_p, _m| async {
        Err("provider error".to_string())
    }));

    assert!(result.is_err());
    assert_eq!(result.unwrap_err(), "provider error");
}

#[test]
fn test_session_persistence_in_memory() {
    let persistence = SessionPersistence::new_in_memory();
    // Save should succeed silently without a store.
    assert!(persistence.save_session("test").is_ok());
    // maybe_summarize should return false without a summarizer.
    assert!(!persistence.maybe_summarize("test", "web", "chat1", &[], 128000));
}

#[test]
fn test_is_internal_channel() {
    assert!(nemesis_types::constants::is_internal_channel("cli"));
    assert!(nemesis_types::constants::is_internal_channel("system"));
    assert!(nemesis_types::constants::is_internal_channel("subagent"));
    assert!(!nemesis_types::constants::is_internal_channel("web"));
    assert!(!nemesis_types::constants::is_internal_channel("rpc"));
    assert!(!nemesis_types::constants::is_internal_channel("discord"));
}

// --- Additional executor tests ---

#[test]
fn test_executor_config_default() {
    let config = ExecutorConfig::default();
    assert_eq!(config.model, "gpt-4");
    assert_eq!(config.max_turns, 10);
    assert!(config.system_prompt.is_none());
    assert_eq!(config.event_buffer_size, 64);
}

#[test]
fn test_concurrent_mode_default() {
    assert_eq!(ConcurrentMode::default(), ConcurrentMode::Reject);
}

#[test]
fn test_tool_result_default() {
    let result = ToolResult::default();
    assert!(result.for_llm.is_empty());
    assert!(result.for_user.is_empty());
    assert!(result.silent);
    assert!(!result.is_async);
    assert!(result.task_id.is_empty());
    assert!(result.err.is_none());
}

#[test]
fn test_fallback_candidate_debug() {
    let candidate = FallbackCandidate {
        provider: "openai".to_string(),
        model: "gpt-4".to_string(),
    };
    let debug_str = format!("{:?}", candidate);
    assert!(debug_str.contains("openai"));
    assert!(debug_str.contains("gpt-4"));
}

#[test]
fn test_fallback_executor_no_candidates() {
    let executor = FallbackExecutor::new();
    let candidates: Vec<FallbackCandidate> = vec![];

    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(executor.execute(&candidates, |_p, _m| async {
        Ok(LlmResponse {
            content: "should not reach".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }));

    assert!(result.is_err());
    assert_eq!(result.unwrap_err(), "No candidates available");
}

#[test]
fn test_fallback_executor_first_fails_second_succeeds() {
    let executor = FallbackExecutor::new();
    let candidates = vec![
        FallbackCandidate {
            provider: "test".to_string(),
            model: "model-1".to_string(),
        },
        FallbackCandidate {
            provider: "test".to_string(),
            model: "model-2".to_string(),
        },
    ];

    let rt = tokio::runtime::Runtime::new().unwrap();
    let call_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let call_count_clone = call_count.clone();

    let result = rt.block_on(executor.execute(&candidates, move |_p, m| {
        let count = call_count_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let m_owned = m.to_string();
        async move {
            if count == 0 {
                Err("first failed".to_string())
            } else {
                Ok(LlmResponse {
                    content: format!("success from {}", m_owned),
                    tool_calls: Vec::new(),
                    finished: true,
                    reasoning_content: None,
                    usage: None,
                    raw_request_body: None,
                    raw_response_body: None,
                })
            }
        }
    }));

    assert!(result.is_ok());
    let fb = result.unwrap();
    assert_eq!(fb.model, "model-2");
    assert_eq!(fb.attempts, 2);
    assert_eq!(fb.response.content, "success from model-2");
}

#[test]
fn test_fallback_executor_default() {
    let executor = FallbackExecutor::default();
    let candidates = vec![FallbackCandidate {
        provider: "test".to_string(),
        model: "model-1".to_string(),
    }];

    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(executor.execute(&candidates, |_p, _m| async {
        Ok(LlmResponse {
            content: "ok".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }));

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_executor_register_tool() {
    let provider = Arc::new(MockProvider::new(vec![]));
    let (_inbound_tx, inbound_rx) = mpsc::channel(16);
    let (outbound_tx, _outbound_rx) = mpsc::channel(16);

    let mut executor =
        AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());
    assert!(executor.tools.is_empty());

    executor.register_tool("test_tool", Arc::new(MockTool { result: "ok".to_string() }));
    assert_eq!(executor.tools.len(), 1);
    assert!(executor.tools.contains_key("test_tool"));
    assert!(executor.agent_config.tools.contains(&"test_tool".to_string()));
}

#[tokio::test]
async fn test_executor_set_fallback_candidates() {
    let provider = Arc::new(MockProvider::new(vec![]));
    let (_inbound_tx, inbound_rx) = mpsc::channel(16);
    let (outbound_tx, _outbound_rx) = mpsc::channel(16);

    let mut executor =
        AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());
    assert!(executor.fallback_candidates.is_empty());

    executor.set_fallback_candidates(vec![
        FallbackCandidate { provider: "p1".to_string(), model: "m1".to_string() },
        FallbackCandidate { provider: "p2".to_string(), model: "m2".to_string() },
    ]);
    assert_eq!(executor.fallback_candidates.len(), 2);
}

#[tokio::test]
async fn test_executor_set_concurrent_mode() {
    let provider = Arc::new(MockProvider::new(vec![]));
    let (_inbound_tx, inbound_rx) = mpsc::channel(16);
    let (outbound_tx, _outbound_rx) = mpsc::channel(16);

    let mut executor =
        AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());
    executor.set_concurrent_mode(ConcurrentMode::Queue, 16);
    assert_eq!(executor.concurrent_mode, ConcurrentMode::Queue);
    assert_eq!(executor.queue_size, 16);
}

#[tokio::test]
async fn test_executor_set_observer() {
    let provider = Arc::new(MockProvider::new(vec![]));
    let (_inbound_tx, inbound_rx) = mpsc::channel(16);
    let (outbound_tx, _outbound_rx) = mpsc::channel(16);

    let mut executor =
        AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());
    assert!(executor.observer.is_none());

    executor.set_observer(Arc::new(MockObserver::new()));
    assert!(executor.observer.is_some());
}

#[tokio::test]
async fn test_executor_set_context_window() {
    let provider = Arc::new(MockProvider::new(vec![]));
    let (_inbound_tx, inbound_rx) = mpsc::channel(16);
    let (outbound_tx, _outbound_rx) = mpsc::channel(16);

    let mut executor =
        AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());
    assert_eq!(executor.context_window, 128_000);

    executor.set_context_window(64000);
    assert_eq!(executor.context_window, 64000);
}

#[tokio::test]
async fn test_executor_set_continuation_manager() {
    let provider = Arc::new(MockProvider::new(vec![]));
    let (_inbound_tx, inbound_rx) = mpsc::channel(16);
    let (outbound_tx, _outbound_rx) = mpsc::channel(16);

    let mut executor =
        AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());
    assert!(executor.continuation_manager.is_none());

    executor.set_continuation_manager(Arc::new(
        crate::loop_continuation::ContinuationManager::new()
    ));
    assert!(executor.continuation_manager.is_some());
}

#[test]
fn test_observer_event_conversation_start() {
    let event = ObserverEvent::ConversationStart {
        trace_id: "t1".to_string(),
        session_key: "s1".to_string(),
        channel: "web".to_string(),
        chat_id: "chat1".to_string(),
        sender_id: "user1".to_string(),
        content: "hello".to_string(),
    };
    let conv_event = event.to_conversation_event();
    assert_eq!(conv_event.event_type, nemesis_observer::EventType::ConversationStart);
}

#[test]
fn test_observer_event_conversation_end() {
    let event = ObserverEvent::ConversationEnd {
        trace_id: "t1".to_string(),
        session_key: "s1".to_string(),
        total_rounds: 3,
        duration_ms: 1500,
        content: "response".to_string(),
        channel: "web".to_string(),
        chat_id: "chat1".to_string(),
    };
    let conv_event = event.to_conversation_event();
    assert_eq!(conv_event.event_type, nemesis_observer::EventType::ConversationEnd);
}

#[test]
fn test_observer_event_llm_request() {
    let event = ObserverEvent::LlmRequest {
        trace_id: "t1".to_string(),
        round: 1,
        model: "gpt-4".to_string(),
        messages: vec![],
        tools: vec![],
        messages_count: 0,
        tools_count: 0,
        provider_name: String::new(),
        api_key: String::new(),
        api_base: String::new(),
    };
    let conv_event = event.to_conversation_event();
    assert_eq!(conv_event.event_type, nemesis_observer::EventType::LlmRequest);
}

#[test]
fn test_observer_event_llm_response() {
    let event = ObserverEvent::LlmResponse {
        trace_id: "t1".to_string(),
        round: 1,
        duration_ms: 200,
        has_tool_calls: true,
        content: "response text".to_string(),
        tool_calls: vec![],
        tool_calls_count: 0,
        finish_reason: Some("stop".to_string()),
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    };
    let conv_event = event.to_conversation_event();
    assert_eq!(conv_event.event_type, nemesis_observer::EventType::LlmResponse);
}

#[test]
fn test_observer_event_tool_call() {
    let event = ObserverEvent::ToolCall {
        trace_id: "t1".to_string(),
        tool_name: "search".to_string(),
        success: true,
        duration_ms: 50,
        round: 1,
        arguments: "{}".to_string(),
        result: "ok".to_string(),
    };
    let conv_event = event.to_conversation_event();
    assert_eq!(conv_event.event_type, nemesis_observer::EventType::ToolCall);
}

#[tokio::test]
async fn test_process_message_empty_response() {
    let provider = Arc::new(MockProvider::new(vec![LlmResponse {
        content: String::new(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]));

    let (_inbound_tx, inbound_rx) = mpsc::channel(16);
    let (outbound_tx, _outbound_rx) = mpsc::channel(16);

    let executor =
        AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());

    // Empty response should not panic
    executor
        .process_message(make_inbound("Hello", "web", ""))
        .await;
}

#[tokio::test]
async fn test_session_persistence_with_store() {
    use crate::session::Summarizer;

    let provider = Arc::new(MockProvider::new(vec![]));
    let (_inbound_tx, inbound_rx) = mpsc::channel(16);
    let (outbound_tx, _outbound_rx) = mpsc::channel(16);

    let session_store = Arc::new(crate::session::SessionStore::new_in_memory());
    let summarizer = Summarizer::new_silent(
        provider.clone(),
        "test-model".to_string(),
        128000,
        session_store.clone(),
    );

    let mut executor =
        AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());
    executor.set_session_persistence(SessionPersistence::with_storage(session_store, summarizer));
}

#[test]
fn test_generate_trace_id() {
    let id = AgentLoopExecutor::generate_trace_id("test-session");
    assert!(id.starts_with("test-session-"));
    assert!(id.len() > "test-session-".len());
}

// --- Additional executor coverage tests ---

#[test]
fn test_tool_result_from_async_extra() {
    let result = ToolResult::async_result("task-42".to_string(), "waiting...".to_string());
    assert!(result.is_async);
    assert_eq!(result.task_id, "task-42");
    assert_eq!(result.for_user, "waiting...");
}

#[test]
fn test_fallback_result_debug() {
    let fr = FallbackResult {
        provider: "test".to_string(),
        model: "model-1".to_string(),
        response: LlmResponse {
            content: "hello".to_string(),
            tool_calls: vec![],
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
        attempts: 1,
    };
    let debug_str = format!("{:?}", fr);
    assert!(debug_str.contains("model-1"));
}

#[tokio::test]
async fn test_executor_process_message_llm_error() {
    struct ErrorProvider;
    #[async_trait]
    impl LlmProvider for ErrorProvider {
        async fn chat(&self, _model: &str, _messages: Vec<LlmMessage>, _options: Option<crate::types::ChatOptions>, _tools: Vec<crate::types::ToolDefinition>) -> Result<LlmResponse, String> {
            Err("LLM failed".to_string())
        }
    }

    let provider = Arc::new(ErrorProvider);
    let (_inbound_tx, inbound_rx) = mpsc::channel(16);
    let (outbound_tx, mut outbound_rx) = mpsc::channel(16);

    let executor =
        AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());

    executor
        .process_message(make_inbound("Hello", "web", ""))
        .await;

    let msg = outbound_rx.recv().await.unwrap();
    assert!(msg.content.contains("Error") || msg.content.contains("LLM failed"));
}

#[tokio::test]
async fn test_executor_set_session_persistence() {
    let provider = Arc::new(MockProvider::new(vec![]));
    let (_inbound_tx, inbound_rx) = mpsc::channel(16);
    let (outbound_tx, _outbound_rx) = mpsc::channel(16);

    let mut executor =
        AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());
    // Default session persistence is in-memory (not None)
    executor.set_session_persistence(SessionPersistence::with_storage(
        Arc::new(crate::session::SessionStore::new_in_memory()),
        crate::session::Summarizer::new_silent(
            Arc::new(MockProvider::new(vec![])),
            "test-model".to_string(),
            128000,
            Arc::new(crate::session::SessionStore::new_in_memory()),
        ),
    ));
}

#[tokio::test]
async fn test_executor_set_observer_manager() {
    let provider = Arc::new(MockProvider::new(vec![]));
    let (_inbound_tx, inbound_rx) = mpsc::channel(16);
    let (outbound_tx, _outbound_rx) = mpsc::channel(16);

    let mut executor =
        AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());

    executor.set_observer_manager(Arc::new(nemesis_observer::Manager::new()));
    assert!(executor.get_observer_manager().is_some());
}

#[tokio::test]
async fn test_process_and_publish_with_tool_call() {
    let provider = Arc::new(MockProvider::new(vec![
        LlmResponse {
            content: String::new(),
            tool_calls: vec![ToolCallInfo {
                id: "tc_1".to_string(),
                name: "test_tool".to_string(),
                arguments: "{}".to_string(),
            }],
            finished: false,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
        LlmResponse {
            content: "Tool done.".to_string(),
            tool_calls: vec![],
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
    ]));

    let (_inbound_tx, inbound_rx) = mpsc::channel(16);
    let (outbound_tx, mut outbound_rx) = mpsc::channel(16);

    let mut executor =
        AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());
    executor.register_tool("test_tool", Arc::new(MockTool { result: "ok".to_string() }));

    let context = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = executor
        .process_and_publish("sess1", "Do something", &context)
        .await
        .unwrap();

    assert_eq!(result, "Tool done.");
    let msg = outbound_rx.recv().await.unwrap();
    assert_eq!(msg.content, "Tool done.");
}

#[tokio::test]
async fn test_executor_runs_with_channel_close() {
    let provider = Arc::new(MockProvider::new(vec![LlmResponse {
        content: "Done.".to_string(),
        tool_calls: vec![],
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]));

    let (inbound_tx, inbound_rx) = mpsc::channel(16);
    let (outbound_tx, mut outbound_rx) = mpsc::channel(16);

    let mut executor =
        AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());

    inbound_tx.send(make_inbound("Hi", "web", "")).await.unwrap();
    drop(inbound_tx);

    executor.run().await;

    let msg = outbound_rx.recv().await.unwrap();
    assert_eq!(msg.content, "Done.");
}

#[tokio::test]
async fn test_executor_context_window_error_retry() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct ContextErrorProvider {
        call_count: AtomicUsize,
    }
    #[async_trait]
    impl LlmProvider for ContextErrorProvider {
        async fn chat(&self, _model: &str, _messages: Vec<LlmMessage>, _options: Option<crate::types::ChatOptions>, _tools: Vec<crate::types::ToolDefinition>) -> Result<LlmResponse, String> {
            let count = self.call_count.fetch_add(1, Ordering::SeqCst);
            if count == 0 {
                Err("context_length_exceeded".to_string())
            } else {
                Ok(LlmResponse {
                    content: "Recovered.".to_string(),
                    tool_calls: vec![],
                    finished: true,
                    reasoning_content: None,
                    usage: None,
                    raw_request_body: None,
                    raw_response_body: None,
                })
            }
        }
    }

    let provider = Arc::new(ContextErrorProvider { call_count: AtomicUsize::new(0) });
    let (_inbound_tx, inbound_rx) = mpsc::channel(16);
    let (outbound_tx, mut outbound_rx) = mpsc::channel(16);

    let executor =
        AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());

    executor.process_message(make_inbound("Big query", "web", "")).await;

    // First message should be about compression
    let _msg1 = outbound_rx.recv().await.unwrap();
    // Second message should be the actual response
    let msg2 = outbound_rx.recv().await.unwrap();
    assert_eq!(msg2.content, "Recovered.");
}

#[test]
fn test_session_persistence_save_no_store() {
    let persistence = SessionPersistence::new_in_memory();
    assert!(persistence.save_session("test").is_ok());
}

#[test]
fn test_session_persistence_no_summarizer() {
    let persistence = SessionPersistence::new_in_memory();
    let history: Vec<crate::types::ConversationTurn> = vec![];
    assert!(!persistence.maybe_summarize("test", "web", "chat1", &history, 128000));
}

// --- Additional coverage for loop_executor ---

#[tokio::test]
async fn test_run_agent_loop_simple() {
    let (inbound_tx, inbound_rx) = mpsc::channel(16);
    let (outbound_tx, mut outbound_rx) = mpsc::channel(16);
    drop(inbound_tx);

    let provider = Arc::new(MockProvider::new(vec![
        crate::r#loop::LlmResponse {
            content: "Agent loop result".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
    ]));
    let mut executor = AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, ExecutorConfig::default());

    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = executor.run_agent_loop("sess1", "Hello", &ctx).await;
    assert!(result.is_ok());
    assert!(result.unwrap().contains("Agent loop result"));
}

#[tokio::test]
async fn test_run_agent_loop_with_tool_call() {
    let (inbound_tx, inbound_rx) = mpsc::channel(16);
    let (outbound_tx, _outbound_rx) = mpsc::channel(16);
    drop(inbound_tx);

    let provider = Arc::new(MockProvider::new(vec![
        crate::r#loop::LlmResponse {
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
        crate::r#loop::LlmResponse {
            content: "The answer is 2".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
    ]));
    let mut executor = AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, ExecutorConfig::default());
    executor.register_tool("calculator", Arc::new(MockTool { result: "2".to_string() }));

    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let result = executor.run_agent_loop("sess-tools", "What is 1+1?", &ctx).await;
    assert!(result.is_ok());
    assert!(result.unwrap().contains("The answer is 2"));
}

#[tokio::test]
async fn test_run_agent_loop_max_iterations() {
    let (inbound_tx, inbound_rx) = mpsc::channel(16);
    let (outbound_tx, _outbound_rx) = mpsc::channel(16);
    drop(inbound_tx);

    // Every LLM response returns tool calls - the loop will exhaust max_turns
    let infinite_response = crate::r#loop::LlmResponse {
        content: String::new(),
        tool_calls: vec![ToolCallInfo {
            id: "tc_loop".to_string(),
            name: "loop_tool".to_string(),
            arguments: "{}".to_string(),
        }],
        finished: false,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    };
    let responses: Vec<_> = (0..15).map(|_| infinite_response.clone()).collect();
    let provider = Arc::new(MockProvider::new(responses));

    let mut config = ExecutorConfig::default();
    config.max_turns = 3;
    let mut executor = AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, config);
    executor.register_tool("loop_tool", Arc::new(MockTool { result: "0".to_string() }));

    let ctx = RequestContext::new("web", "chat1", "user1", "sess-loop");
    let result = executor.run_agent_loop("sess-loop", "Loop test", &ctx).await;
    assert!(result.is_ok());
    // run_agent_loop does not call check_iteration_limit, so it returns
    // empty content when max turns is reached with only tool calls
    let content = result.unwrap();
    assert!(content.is_empty() || content.contains("No more responses"),
        "Expected empty or exhaustion, got: {}", content);
}

#[tokio::test]
async fn test_check_iteration_limit_hit() {
    let (inbound_tx, inbound_rx) = mpsc::channel(16);
    let (outbound_tx, _outbound_rx) = mpsc::channel(16);
    drop(inbound_tx);

    let provider = Arc::new(MockProvider::new(vec![]));
    let mut config = ExecutorConfig::default();
    config.max_turns = 5;
    let executor = AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, config);

    let result = executor.check_iteration_limit("", 5);
    assert!(result.contains("Max iterations"));
}

#[tokio::test]
async fn test_check_iteration_limit_not_hit() {
    let (inbound_tx, inbound_rx) = mpsc::channel(16);
    let (outbound_tx, _outbound_rx) = mpsc::channel(16);
    drop(inbound_tx);

    let provider = Arc::new(MockProvider::new(vec![]));
    let executor = AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, ExecutorConfig::default());

    let result = executor.check_iteration_limit("Normal response", 3);
    assert_eq!(result, "Normal response");
}

#[tokio::test]
async fn test_handle_tool_calls_batch() {
    let (inbound_tx, inbound_rx) = mpsc::channel(16);
    let (outbound_tx, _outbound_rx) = mpsc::channel(16);
    drop(inbound_tx);

    let provider = Arc::new(MockProvider::new(vec![]));
    let mut executor = AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, ExecutorConfig::default());
    executor.register_tool("tool_a", Arc::new(MockTool { result: "A result".to_string() }));
    executor.register_tool("tool_b", Arc::new(MockTool { result: "B result".to_string() }));

    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let tool_calls = vec![
        ToolCallInfo {
            id: "tc_1".to_string(),
            name: "tool_a".to_string(),
            arguments: "{}".to_string(),
        },
        ToolCallInfo {
            id: "tc_2".to_string(),
            name: "tool_b".to_string(),
            arguments: "{}".to_string(),
        },
    ];
    let results = executor.handle_tool_calls(&tool_calls, &ctx, "trace-1", 1).await;
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].result, "A result");
    assert_eq!(results[1].result, "B result");
}

#[tokio::test]
async fn test_handle_tool_calls_unknown_tool() {
    let (inbound_tx, inbound_rx) = mpsc::channel(16);
    let (outbound_tx, _outbound_rx) = mpsc::channel(16);
    drop(inbound_tx);

    let provider = Arc::new(MockProvider::new(vec![]));
    let executor = AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, ExecutorConfig::default());

    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let tool_calls = vec![ToolCallInfo {
        id: "tc_1".to_string(),
        name: "nonexistent".to_string(),
        arguments: "{}".to_string(),
    }];
    let results = executor.handle_tool_calls(&tool_calls, &ctx, "trace-1", 1).await;
    assert_eq!(results.len(), 1);
    assert!(results[0].is_error);
    assert!(results[0].result.contains("Unknown tool"));
}

#[tokio::test]
async fn test_update_tool_contexts() {
    let (inbound_tx, inbound_rx) = mpsc::channel(16);
    let (outbound_tx, _outbound_rx) = mpsc::channel(16);
    drop(inbound_tx);

    let provider = Arc::new(MockProvider::new(vec![]));
    let executor = AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, ExecutorConfig::default());

    // Should not panic even without context-aware tools
    executor.update_tool_contexts("web", "chat1");
}

#[test]
fn test_tool_result_for_llm_only_values() {
    let result = ToolResult::for_llm_only("LLM content".to_string());
    assert_eq!(result.for_llm, "LLM content");
    assert!(result.silent);
    assert!(!result.is_async);
}

#[test]
fn test_tool_result_async_result_values() {
    let result = ToolResult::async_result("task-123".to_string(), "Working on it...".to_string());
    assert!(result.is_async);
    assert_eq!(result.task_id, "task-123");
    assert!(!result.silent);
    assert_eq!(result.for_user, "Working on it...");
}

#[test]
fn test_fallback_result_fields() {
    let result = FallbackResult {
        response: crate::r#loop::LlmResponse {
            content: "Hello".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
        provider: "provider1".to_string(),
        model: "model1".to_string(),
        attempts: 2,
    };
    assert_eq!(result.provider, "provider1");
    assert_eq!(result.attempts, 2);
}

#[tokio::test]
async fn test_call_llm_with_fallback_no_candidates() {
    let (inbound_tx, inbound_rx) = mpsc::channel(16);
    let (outbound_tx, _outbound_rx) = mpsc::channel(16);
    drop(inbound_tx);

    let provider = Arc::new(MockProvider::new(vec![
        crate::r#loop::LlmResponse {
            content: "Direct response".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
    ]));
    let executor = AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, ExecutorConfig::default());

    let messages = vec![crate::r#loop::LlmMessage {
        role: "user".to_string(),
        content: "Hello".to_string(),
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
    }];
    let result = executor.call_llm_with_fallback(&messages, None, vec![]).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap().content, "Direct response");
}

#[test]
fn test_fallback_candidate_clone() {
    let candidate = FallbackCandidate {
        provider: "prov1".to_string(),
        model: "mod1".to_string(),
    };
    let cloned = candidate.clone();
    assert_eq!(cloned.provider, "prov1");
    assert_eq!(cloned.model, "mod1");
}

#[test]
fn test_executor_config_clone() {
    let config = ExecutorConfig {
        model: "gpt-4".to_string(),
        max_turns: 5,
        system_prompt: Some("You are helpful".to_string()),
        event_buffer_size: 32,
    };
    let cloned = config.clone();
    assert_eq!(cloned.model, "gpt-4");
    assert_eq!(cloned.max_turns, 5);
}

#[test]
fn test_session_persistence_with_storage_creation() {
    use async_trait::async_trait;

    struct SilentProvider;
    #[async_trait]
    impl crate::r#loop::LlmProvider for SilentProvider {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<crate::r#loop::LlmMessage>,
            _options: Option<crate::types::ChatOptions>,
            _tools: Vec<crate::types::ToolDefinition>,
        ) -> Result<crate::r#loop::LlmResponse, String> {
            Ok(crate::r#loop::LlmResponse {
                content: "summary".to_string(),
                tool_calls: Vec::new(),
                finished: true,
                reasoning_content: None,
                usage: None,
                raw_request_body: None,
                raw_response_body: None,
            })
        }
    }

    let store = Arc::new(crate::session::SessionStore::new_in_memory());
    let summarizer = crate::session::Summarizer::new_silent(
        Arc::new(SilentProvider),
        "test-model".to_string(),
        128000,
        store.clone(),
    );
    let persistence = SessionPersistence::with_storage(store, summarizer);
    // save_session should work
    assert!(persistence.save_session("test-key").is_ok());
}

// --- Additional coverage tests ---

#[test]
fn test_concurrent_mode_variants() {
    assert_eq!(ConcurrentMode::Reject, ConcurrentMode::Reject);
    assert_eq!(ConcurrentMode::Queue, ConcurrentMode::Queue);
    assert_ne!(ConcurrentMode::Reject, ConcurrentMode::Queue);
}

#[test]
fn test_tool_result_debug() {
    let result = ToolResult::simple("test content".to_string());
    let debug = format!("{:?}", result);
    assert!(debug.contains("test content"));
}

#[test]
fn test_tool_result_error_debug() {
    let result = ToolResult::error("something failed".to_string());
    let debug = format!("{:?}", result);
    assert!(debug.contains("something failed"));
}

#[test]
fn test_tool_result_async_debug() {
    let result = ToolResult::async_result("task-99".to_string(), "processing".to_string());
    let debug = format!("{:?}", result);
    assert!(debug.contains("task-99"));
}

#[test]
fn test_observer_event_all_variants_debug() {
    let start = ObserverEvent::ConversationStart {
        trace_id: "t1".to_string(),
        session_key: "s1".to_string(),
        channel: "web".to_string(),
        chat_id: "c1".to_string(),
        sender_id: "user1".to_string(),
        content: "hello".to_string(),
    };
    assert!(format!("{:?}", start).contains("t1"));

    let end = ObserverEvent::ConversationEnd {
        trace_id: "t2".to_string(),
        session_key: "s2".to_string(),
        total_rounds: 5,
        duration_ms: 1000,
        content: "response".to_string(),
        channel: "web".to_string(),
        chat_id: "c1".to_string(),
    };
    assert!(format!("{:?}", end).contains("t2"));

    let req = ObserverEvent::LlmRequest {
        trace_id: "t3".to_string(),
        round: 1,
        model: "gpt-4".to_string(),
        messages: vec![],
        tools: vec![],
        messages_count: 0,
        tools_count: 0,
        provider_name: String::new(),
        api_key: String::new(),
        api_base: String::new(),
    };
    assert!(format!("{:?}", req).contains("gpt-4"));

    let resp = ObserverEvent::LlmResponse {
        trace_id: "t4".to_string(),
        round: 2,
        duration_ms: 500,
        has_tool_calls: false,
        content: "text".to_string(),
        tool_calls: vec![],
        tool_calls_count: 0,
        finish_reason: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    };
    assert!(format!("{:?}", resp).contains("t4"));

    let tool = ObserverEvent::ToolCall {
        trace_id: "t5".to_string(),
        tool_name: "read_file".to_string(),
        success: true,
        duration_ms: 10,
        round: 1,
        arguments: "{}".to_string(),
        result: "ok".to_string(),
    };
    assert!(format!("{:?}", tool).contains("read_file"));
}

#[test]
fn test_fallback_executor_cooldown_skips() {
    let executor = FallbackExecutor::new();
    let candidates = vec![
        FallbackCandidate {
            provider: "test".to_string(),
            model: "model-1".to_string(),
        },
        FallbackCandidate {
            provider: "test".to_string(),
            model: "model-2".to_string(),
        },
    ];

    let rt = tokio::runtime::Runtime::new().unwrap();
    // First call: model-1 fails, model-2 succeeds
    let result = rt.block_on(executor.execute(&candidates, |_p, m| {
        let m_owned = m.to_string();
        async move {
            if m_owned == "model-1" {
                Err("fail".to_string())
            } else {
                Ok(LlmResponse {
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
    }));
    assert!(result.is_ok());
    assert_eq!(result.unwrap().model, "model-2");
}

#[tokio::test]
async fn test_executor_context_window_default() {
    let provider = Arc::new(MockProvider::new(vec![]));
    let (_inbound_tx, inbound_rx) = mpsc::channel(16);
    let (outbound_tx, _outbound_rx) = mpsc::channel(16);

    let executor =
        AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, ExecutorConfig::default());
    assert_eq!(executor.context_window, 128_000);
}

#[test]
fn test_session_persistence_with_storage_save() {
    use async_trait::async_trait;

    struct SilentProvider;
    #[async_trait]
    impl crate::r#loop::LlmProvider for SilentProvider {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<crate::r#loop::LlmMessage>,
            _options: Option<crate::types::ChatOptions>,
            _tools: Vec<crate::types::ToolDefinition>,
        ) -> Result<crate::r#loop::LlmResponse, String> {
            Ok(crate::r#loop::LlmResponse {
                content: "summary".to_string(),
                tool_calls: Vec::new(),
                finished: true,
                reasoning_content: None,
                usage: None,
                raw_request_body: None,
                raw_response_body: None,
            })
        }
    }

    let store = Arc::new(crate::session::SessionStore::new_in_memory());
    let summarizer = crate::session::Summarizer::new_silent(
        Arc::new(SilentProvider),
        "test-model".to_string(),
        128000,
        store.clone(),
    );
    let persistence = SessionPersistence::with_storage(store, summarizer);

    // Save a session
    assert!(persistence.save_session("test-session").is_ok());
}

#[tokio::test]
async fn test_call_llm_with_fallback_with_candidates() {
    let (inbound_tx, inbound_rx) = mpsc::channel(16);
    let (outbound_tx, _outbound_rx) = mpsc::channel(16);
    drop(inbound_tx);

    let provider = Arc::new(MockProvider::new(vec![
        crate::r#loop::LlmResponse {
            content: "Fallback response".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
    ]));
    let mut executor = AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, ExecutorConfig::default());
    executor.set_fallback_candidates(vec![
        FallbackCandidate { provider: "p1".to_string(), model: "m1".to_string() },
    ]);

    let messages = vec![crate::r#loop::LlmMessage {
        role: "user".to_string(),
        content: "Hello".to_string(),
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
    }];
    let result = executor.call_llm_with_fallback(&messages, None, vec![]).await;
    assert!(result.is_ok());
}

#[test]
fn test_observer_event_to_conversation_event_all() {
    // ConversationEnd
    let event = ObserverEvent::ConversationEnd {
        trace_id: "t1".to_string(),
        session_key: "s1".to_string(),
        total_rounds: 5,
        duration_ms: 2000,
        content: "done".to_string(),
        channel: "web".to_string(),
        chat_id: "c1".to_string(),
    };
    let ce = event.to_conversation_event();
    assert_eq!(ce.event_type, nemesis_observer::EventType::ConversationEnd);

    // LlmRequest
    let event = ObserverEvent::LlmRequest {
        trace_id: "t2".to_string(),
        round: 3,
        model: "gpt-4".to_string(),
        messages: vec![],
        tools: vec![],
        messages_count: 0,
        tools_count: 0,
        provider_name: String::new(),
        api_key: String::new(),
        api_base: String::new(),
    };
    let ce = event.to_conversation_event();
    assert_eq!(ce.event_type, nemesis_observer::EventType::LlmRequest);

    // LlmResponse with no tool calls
    let event = ObserverEvent::LlmResponse {
        trace_id: "t3".to_string(),
        round: 1,
        duration_ms: 100,
        has_tool_calls: false,
        content: "text".to_string(),
        tool_calls: vec![],
        tool_calls_count: 0,
        finish_reason: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    };
    let ce = event.to_conversation_event();
    assert_eq!(ce.event_type, nemesis_observer::EventType::LlmResponse);

    // ToolCall
    let event = ObserverEvent::ToolCall {
        trace_id: "t4".to_string(),
        tool_name: "write_file".to_string(),
        success: false,
        duration_ms: 50,
        round: 2,
        arguments: "{}".to_string(),
        result: "error".to_string(),
    };
    let ce = event.to_conversation_event();
    assert_eq!(ce.event_type, nemesis_observer::EventType::ToolCall);
}

#[tokio::test]
async fn test_executor_register_multiple_tools() {
    let provider = Arc::new(MockProvider::new(vec![]));
    let (_inbound_tx, inbound_rx) = mpsc::channel(16);
    let (outbound_tx, _outbound_rx) = mpsc::channel(16);

    let mut executor =
        AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, test_executor_config());

    executor.register_tool("tool1", Arc::new(MockTool { result: "r1".to_string() }));
    executor.register_tool("tool2", Arc::new(MockTool { result: "r2".to_string() }));
    executor.register_tool("tool3", Arc::new(MockTool { result: "r3".to_string() }));

    assert_eq!(executor.tools.len(), 3);
    assert!(executor.tools.contains_key("tool1"));
    assert!(executor.tools.contains_key("tool2"));
    assert!(executor.tools.contains_key("tool3"));
}

#[test]
fn test_generate_trace_id_uniqueness() {
    let id1 = AgentLoopExecutor::generate_trace_id("session-1");
    let id2 = AgentLoopExecutor::generate_trace_id("session-1");
    assert_ne!(id1, id2);
}

#[test]
fn test_executor_config_custom() {
    let config = ExecutorConfig {
        model: "custom-model".to_string(),
        max_turns: 20,
        system_prompt: Some("Be helpful".to_string()),
        event_buffer_size: 128,
    };
    assert_eq!(config.model, "custom-model");
    assert_eq!(config.max_turns, 20);
    assert_eq!(config.event_buffer_size, 128);
}

#[test]
fn test_fallback_executor_clears_cooldown_on_success() {
    let executor = FallbackExecutor::new();
    let candidates = vec![
        FallbackCandidate {
            provider: "test".to_string(),
            model: "model-1".to_string(),
        },
    ];

    let rt = tokio::runtime::Runtime::new().unwrap();

    // First call succeeds
    let result = rt.block_on(executor.execute(&candidates, |_p, _m| async {
        Ok(LlmResponse {
            content: "success".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }));
    assert!(result.is_ok());

    // Second call should also succeed (no cooldown from previous success)
    let result2 = rt.block_on(executor.execute(&candidates, |_p, _m| async {
        Ok(LlmResponse {
            content: "success2".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }));
    assert!(result2.is_ok());
}
