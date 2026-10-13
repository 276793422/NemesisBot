//! S10b (quality-hardening goal 冲刺, web 批次 2): ProviderAdapter::chat
//! conversion arms the existing `llm_bridge_extra_tests` plain-message path
//! skips — agent-side `tool_calls` messages, the `Some(options)` pass-through
//! (H4 reasoning_effort), the tool-definition mapping, and the provider-Err
//! arm — observed through recording/failing mocks on the provider side.

use crate::llm_bridge::ProviderAdapter;
use nemesis_agent::r#loop::LlmMessage;
use nemesis_agent::r#loop::LlmProvider as AgentLlmProvider;
use nemesis_agent::types::{
    ChatOptions as AgentChatOptions, ToolCallInfo, ToolDefinition, ToolFunctionDef,
};
use nemesis_providers::failover::FailoverError;
use nemesis_providers::router::LLMProvider;
use nemesis_providers::types::{
    ChatOptions as ProviderChatOptions, LLMResponse, Message as ProviderMessage,
    ToolDefinition as ProviderToolDef,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

struct RecordingMock {
    default_model: String,
    seen_messages: Mutex<Vec<ProviderMessage>>,
    seen_tools: Mutex<Vec<ProviderToolDef>>,
    seen_options: Mutex<Option<ProviderChatOptions>>,
}

impl RecordingMock {
    fn new() -> Self {
        Self {
            default_model: "mock/default".to_string(),
            seen_messages: Mutex::new(Vec::new()),
            seen_tools: Mutex::new(Vec::new()),
            seen_options: Mutex::new(None),
        }
    }
}

#[async_trait::async_trait]
impl LLMProvider for RecordingMock {
    async fn chat(
        &self,
        messages: &[ProviderMessage],
        tools: &[ProviderToolDef],
        _model: &str,
        options: &ProviderChatOptions,
    ) -> Result<LLMResponse, FailoverError> {
        *self.seen_messages.lock().unwrap() = messages.to_vec();
        *self.seen_tools.lock().unwrap() = tools.to_vec();
        *self.seen_options.lock().unwrap() = Some(options.clone());
        Ok(LLMResponse {
            content: "好的".to_string(),
            tool_calls: vec![],
            finish_reason: "stop".to_string(),
            usage: None,
            reasoning_content: None,
            extra: HashMap::new(),
            raw_request_body: None,
            raw_response_body: None,
        })
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    fn name(&self) -> &str {
        "recording-mock"
    }
}

struct FailingMock;

#[async_trait::async_trait]
impl LLMProvider for FailingMock {
    async fn chat(
        &self,
        _messages: &[ProviderMessage],
        _tools: &[ProviderToolDef],
        _model: &str,
        _options: &ProviderChatOptions,
    ) -> Result<LLMResponse, FailoverError> {
        Err(FailoverError::Timeout {
            provider: "failing-mock".to_string(),
            model: "m".to_string(),
        })
    }

    fn default_model(&self) -> &str {
        "m"
    }

    fn name(&self) -> &str {
        "failing-mock"
    }
}

fn agent_messages_with_tool_calls() -> Vec<LlmMessage> {
    vec![
        LlmMessage {
            role: "user".to_string(),
            content: "北京天气".to_string(),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
            images: Vec::new(),
        },
        LlmMessage {
            role: "assistant".to_string(),
            content: String::new(),
            tool_calls: Some(vec![ToolCallInfo {
                id: "call-1".to_string(),
                name: "weather".to_string(),
                arguments: r#"{"city":"北京"}"#.to_string(),
            }]),
            tool_call_id: None,
            reasoning_content: None,
            images: Vec::new(),
        },
        LlmMessage {
            role: "tool".to_string(),
            content: "晴".to_string(),
            tool_calls: None,
            tool_call_id: Some("call-1".to_string()),
            reasoning_content: None,
            images: Vec::new(),
        },
    ]
}

#[tokio::test]
async fn adapter_maps_tool_call_messages_options_and_tools() {
    let mock = Arc::new(RecordingMock::new());
    let adapter = ProviderAdapter::new(mock.clone(), "mock/default".to_string());

    let agent_tools = vec![ToolDefinition {
        tool_type: "function".to_string(),
        function: ToolFunctionDef {
            name: "weather".to_string(),
            description: "查询天气".to_string(),
            parameters: serde_json::json!({ "type": "object" }),
        },
    }];
    let options = AgentChatOptions {
        max_tokens: Some(128),
        temperature: Some(0.5),
        top_p: None,
        stop: None,
        reasoning_effort: Some("high".to_string()),
    };

    // Empty model → adapter falls back to the default model.
    let resp = adapter
        .chat(
            "",
            agent_messages_with_tool_calls(),
            Some(options),
            agent_tools,
        )
        .await
        .expect("mock chat succeeds");
    assert_eq!(resp.content, "好的");
    assert!(resp.finished);

    let msgs = mock.seen_messages.lock().unwrap();
    assert_eq!(msgs.len(), 3);
    // Assistant tool_calls mapped to provider ToolCall (call_type=function).
    let tc = msgs[1].tool_calls.first().expect("tool_call mapped");
    assert_eq!(tc.id, "call-1");
    assert_eq!(tc.call_type.as_deref(), Some("function"));
    let func = tc.function.as_ref().expect("function payload mapped");
    assert_eq!(func.name, "weather");
    assert_eq!(func.arguments, r#"{"city":"北京"}"#);
    // Tool result message keeps its tool_call_id.
    assert_eq!(msgs[2].tool_call_id.as_deref(), Some("call-1"));

    let tools = mock.seen_tools.lock().unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].function.name, "weather");
    assert_eq!(tools[0].function.description, "查询天气");

    let opts = mock.seen_options.lock().unwrap().clone().unwrap();
    assert_eq!(opts.temperature, Some(0.5));
    assert_eq!(opts.max_tokens, Some(128));
    assert_eq!(
        opts.reasoning_effort.as_deref(),
        Some("high"),
        "H4 effort passthrough"
    );
}

#[tokio::test]
async fn adapter_none_options_use_provider_defaults_and_err_maps_to_string() {
    // None options → the hardcoded default ChatOptions arm.
    let mock = Arc::new(RecordingMock::new());
    let adapter = ProviderAdapter::new(mock.clone(), "m".to_string());
    let resp = adapter
        .chat(
            "m",
            vec![LlmMessage {
                role: "user".to_string(),
                content: "hi".to_string(),
                tool_calls: None,
                tool_call_id: None,
                reasoning_content: None,
                images: Vec::new(),
            }],
            None,
            vec![],
        )
        .await
        .expect("ok arm");
    assert!(resp.finished);
    let opts = mock.seen_options.lock().unwrap().clone().unwrap();
    assert_eq!(opts.temperature, Some(0.7));
    assert_eq!(opts.max_tokens, Some(8192));
    assert_eq!(opts.reasoning_effort, None);

    // Provider error → adapter maps to Err(String) carrying the message.
    let failing = ProviderAdapter::new(Arc::new(FailingMock), "m".to_string());
    let err = failing
        .chat("m", vec![], None, vec![])
        .await
        .expect_err("provider error propagates");
    assert!(
        err.contains("timeout"),
        "error text carried through: {}",
        err
    );
}
