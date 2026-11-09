//! B3（2026-11-09）思考增量消费面：`ProviderAdapter::chat_streamed_with_
//! reasoning` 的 fragment chunk 契约——片段（delta 空 + finish None +
//! reasoning Some）逐个进 `on_reasoning_delta` 回调；终态 chunk 只进聚合
//! （全文覆盖兜底），不再重复进回调（回调面与终态面不重复）。无思考回调
//! 的消费方走 `chat_streamed` 委托（no-op 思考回调）同样安全。

use crate::llm_bridge::ProviderAdapter;
use nemesis_agent::r#loop::LlmMessage;
use nemesis_agent::r#loop::LlmProvider as AgentLlmProvider;
use nemesis_providers::failover::FailoverError;
use nemesis_providers::http_provider::StreamChunk;
use nemesis_providers::router::LLMProvider;
use nemesis_providers::types::{
    ChatOptions as ProviderChatOptions, LLMResponse, Message as ProviderMessage,
    ToolDefinition as ProviderToolDef,
};
use std::sync::Arc;
use std::sync::Mutex;

/// 思考流 mock：两片段（delta 空 + finish None + reasoning Some）→ 正文
/// 增量 → 终态 chunk（finish stop + reasoning 全文覆盖）。
struct ReasoningStreamMock;

#[async_trait::async_trait]
impl LLMProvider for ReasoningStreamMock {
    async fn chat(
        &self,
        _messages: &[ProviderMessage],
        _tools: &[ProviderToolDef],
        _model: &str,
        _options: &ProviderChatOptions,
    ) -> Result<LLMResponse, FailoverError> {
        unreachable!("stream_preferred=true 时不应走非流式")
    }

    fn chat_stream(
        &self,
        _messages: &[ProviderMessage],
        _tools: &[ProviderToolDef],
        _model: &str,
        _options: &ProviderChatOptions,
    ) -> tokio::sync::mpsc::Receiver<Result<StreamChunk, FailoverError>> {
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        let _ = tx.try_send(Ok(StreamChunk {
            delta: String::new(),
            tool_calls: vec![],
            finish_reason: None,
            usage: None,
            reasoning_content: Some("片段一".to_string()),
        }));
        let _ = tx.try_send(Ok(StreamChunk {
            delta: String::new(),
            tool_calls: vec![],
            finish_reason: None,
            usage: None,
            reasoning_content: Some("片段二".to_string()),
        }));
        let _ = tx.try_send(Ok(StreamChunk {
            delta: "正文".to_string(),
            tool_calls: vec![],
            finish_reason: None,
            usage: None,
            reasoning_content: None,
        }));
        let _ = tx.try_send(Ok(StreamChunk {
            delta: String::new(),
            tool_calls: vec![],
            finish_reason: Some("stop".to_string()),
            usage: None,
            // 终态全文覆盖兜底。
            reasoning_content: Some("片段一片段二".to_string()),
        }));
        rx
    }

    fn stream_preferred(&self, _model: &str) -> bool {
        true
    }

    fn default_model(&self) -> &str {
        "mock/model"
    }

    fn name(&self) -> &str {
        "reasoning-mock"
    }
}

fn empty_agent_args() -> (
    Vec<LlmMessage>,
    Option<nemesis_agent::types::ChatOptions>,
    Vec<nemesis_agent::types::ToolDefinition>,
) {
    (Vec::new(), None, Vec::new())
}

#[tokio::test]
async fn fragments_fire_reasoning_callback_finish_does_not() {
    let adapter = ProviderAdapter::new(Arc::new(ReasoningStreamMock), "fallback".to_string());
    let deltas: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let contents: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let (messages, options, tools) = empty_agent_args();
    let resp = adapter
        .chat_streamed_with_reasoning(
            "mock/model",
            messages,
            options,
            tools,
            &|d: String| contents.lock().unwrap().push(d),
            &|d: String| deltas.lock().unwrap().push(d),
        )
        .await
        .unwrap();

    // 片段逐个进回调。
    assert_eq!(*deltas.lock().unwrap(), vec!["片段一", "片段二"]);
    // 终态全文不再进回调（回调面与终态面不重复）。
    // 正文照常。
    assert_eq!(*contents.lock().unwrap(), vec!["正文"]);
    // 聚合结果：终态全文为权威值。
    assert_eq!(resp.reasoning_content.as_deref(), Some("片段一片段二"));
    assert_eq!(resp.content, "正文");
    assert_eq!(resp.finish_reason.as_deref(), Some("stop"));
}

#[tokio::test]
async fn chat_streamed_noop_reasoning_consumer_safe() {
    // 无思考回调消费方（provider_stream 缺省路径）委托后行为不变：正文
    // 照常回调，思考片段被 no-op 吞掉不炸。
    let adapter = ProviderAdapter::new(Arc::new(ReasoningStreamMock), "fallback".to_string());
    let contents: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let (messages, options, tools) = empty_agent_args();
    let resp = adapter
        .chat_streamed("mock/model", messages, options, tools, &|d: String| {
            contents.lock().unwrap().push(d)
        })
        .await
        .unwrap();
    assert_eq!(*contents.lock().unwrap(), vec!["正文"]);
    assert_eq!(resp.reasoning_content.as_deref(), Some("片段一片段二"));
}
