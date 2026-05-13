//! NemesisBot - Observer Framework
//!
//! Event-driven observation system for tracking agent conversation lifecycle.
//! Supports both async (Emit) and sync (EmitSync) event delivery.

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::warn;

/// Event type identifiers for conversation lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventType {
    ConversationStart,
    ConversationEnd,
    LlmRequest,
    LlmResponse,
    ToolCall,
}

impl std::fmt::Display for EventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConversationStart => write!(f, "conversation_start"),
            Self::ConversationEnd => write!(f, "conversation_end"),
            Self::LlmRequest => write!(f, "llm_request"),
            Self::LlmResponse => write!(f, "llm_response"),
            Self::ToolCall => write!(f, "tool_call"),
        }
    }
}

/// A conversation event with typed data.
#[derive(Debug, Clone)]
pub struct ConversationEvent {
    pub event_type: EventType,
    pub trace_id: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub data: EventData,
}

/// Typed event data payloads.
#[derive(Debug, Clone)]
pub enum EventData {
    ConversationStart(ConversationStartData),
    ConversationEnd(ConversationEndData),
    LlmRequest(LlmRequestData),
    LlmResponse(LlmResponseData),
    ToolCall(ToolCallData),
}

/// Data for conversation start events.
#[derive(Debug, Clone)]
pub struct ConversationStartData {
    pub session_key: String,
    pub channel: String,
    pub chat_id: String,
    pub sender_id: String,
    pub content: String,
}

/// Data for conversation end events.
#[derive(Debug, Clone)]
pub struct ConversationEndData {
    pub session_key: String,
    pub channel: String,
    pub chat_id: String,
    pub total_rounds: u32,
    pub total_duration: Duration,
    pub content: String,
    pub error: Option<String>,
}

/// Data for LLM request events.
///
/// Mirrors Go `LLMRequestData` from module/observer/observer.go.
/// The `messages` and `tools` fields contain the full conversation context
/// sent to the LLM, while `messages_count` and `tools_count` are convenience
/// fields for quick access.
#[derive(Debug, Clone)]
pub struct LlmRequestData {
    pub round: u32,
    pub model: String,
    pub provider_name: String,
    pub api_key: String,
    pub api_base: String,
    pub http_headers: HashMap<String, String>,
    /// Full provider configuration as a JSON value.
    pub full_config: Option<serde_json::Value>,
    /// Full message list sent to the LLM (serialized as JSON values).
    pub messages: Vec<serde_json::Value>,
    /// Full tool definitions sent to the LLM (serialized as JSON values).
    pub tools: Vec<serde_json::Value>,
    /// Convenience: number of messages (mirrors Go's len(Messages)).
    pub messages_count: usize,
    /// Convenience: number of tools (mirrors Go's len(Tools)).
    pub tools_count: usize,
}

/// Data for LLM response events.
///
/// Mirrors Go `LLMResponseData` from module/observer/observer.go.
/// Contains the full tool calls list and usage info in addition to
/// the convenience `tool_calls_count` field.
#[derive(Debug, Clone)]
pub struct LlmResponseData {
    pub round: u32,
    pub duration: Duration,
    pub content: String,
    /// Full tool calls from the LLM response (serialized as JSON values).
    pub tool_calls: Vec<serde_json::Value>,
    /// Convenience: number of tool calls.
    pub tool_calls_count: usize,
    /// Token usage information.
    pub usage: Option<UsageInfo>,
    pub finish_reason: Option<String>,
}

/// Token usage information, mirroring Go's `providers.UsageInfo`.
#[derive(Debug, Clone)]
pub struct UsageInfo {
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub total_tokens: i64,
}

/// Data for tool call events.
///
/// Mirrors Go `ToolCallData` from module/observer/observer.go.
#[derive(Debug, Clone)]
pub struct ToolCallData {
    pub tool_name: String,
    /// Full tool call arguments as a JSON object (mirrors Go's `Arguments map[string]interface{}`).
    pub arguments: HashMap<String, serde_json::Value>,
    pub success: bool,
    pub duration: Duration,
    pub error: Option<String>,
    pub llm_round: u32,
    pub chain_pos: u32,
}

/// Observer trait for receiving conversation events.
#[async_trait]
pub trait Observer: Send + Sync {
    /// Name of the observer for identification.
    fn name(&self) -> &str;

    /// Handle a conversation event.
    async fn on_event(&self, event: ConversationEvent);
}

/// Manager for multiple observers with async and sync delivery.
pub struct Manager {
    observers: Arc<RwLock<Vec<Arc<dyn Observer>>>>,
}

impl Manager {
    /// Create a new observer manager.
    pub fn new() -> Self {
        Self {
            observers: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Register an observer.
    pub async fn register(&self, observer: Arc<dyn Observer>) {
        let mut obs = self.observers.write().await;
        obs.push(observer);
    }

    /// Unregister an observer by name.
    pub async fn unregister(&self, name: &str) {
        let mut obs = self.observers.write().await;
        obs.retain(|o| o.name() != name);
    }

    /// Emit an event to all observers asynchronously.
    /// Each observer runs in its own tokio task.
    /// Matches Go's `Emit()` which spawns a goroutine with `defer recover()`.
    /// Tokio's task runtime already catches panics in spawned tasks.
    pub async fn emit(&self, event: ConversationEvent) {
        let observers = self.observers.read().await;
        for obs in observers.iter() {
            let o = Arc::clone(obs);
            let e = event.clone();
            tokio::spawn(async move {
                o.on_event(e).await;
            });
        }
    }

    /// Emit an event to all observers synchronously.
    /// Use for events where all observers must complete before proceeding.
    /// Matches Go's `EmitSync()` with `defer recover()` wrapping each call.
    /// Each observer is spawned in its own task; if one panics, the rest still run.
    pub async fn emit_sync(&self, event: ConversationEvent) {
        let observers = self.observers.read().await;
        for obs in observers.iter() {
            let o = Arc::clone(obs);
            let e = event.clone();
            let name = o.name().to_string();
            // Spawn and await sequentially, with panic recovery via JoinHandle
            let handle = tokio::spawn(async move {
                o.on_event(e).await;
            });
            if let Err(err) = handle.await {
                if err.is_panic() {
                    warn!("Observer {} panicked during emit_sync", name);
                }
            }
        }
    }

    /// Unregister all observers.
    pub async fn unregister_all(&self) {
        let mut obs = self.observers.write().await;
        obs.clear();
    }

    /// Check if any observers are registered.
    pub async fn has_observers(&self) -> bool {
        let obs = self.observers.read().await;
        !obs.is_empty()
    }
}

impl Default for Manager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TestObserver {
        name: String,
        count: Arc<AtomicUsize>,
    }

    impl TestObserver {
        fn new(name: &str) -> (Arc<Self>, Arc<AtomicUsize>) {
            let count = Arc::new(AtomicUsize::new(0));
            let obs = Arc::new(Self {
                name: name.to_string(),
                count: Arc::clone(&count),
            });
            (obs, count)
        }
    }

    #[async_trait]
    impl Observer for TestObserver {
        fn name(&self) -> &str {
            &self.name
        }

        async fn on_event(&self, _event: ConversationEvent) {
            self.count.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn make_event(event_type: EventType) -> ConversationEvent {
        ConversationEvent {
            event_type,
            trace_id: "test-trace".to_string(),
            timestamp: chrono::Utc::now(),
            data: EventData::ConversationStart(ConversationStartData {
                session_key: "test".to_string(),
                channel: "test".to_string(),
                chat_id: "chat1".to_string(),
                sender_id: "user1".to_string(),
                content: "hello".to_string(),
            }),
        }
    }

    #[tokio::test]
    async fn test_register_unregister() {
        let manager = Manager::new();
        assert!(!manager.has_observers().await);

        let (obs, _) = TestObserver::new("test");
        manager.register(obs).await;
        assert!(manager.has_observers().await);

        manager.unregister("test").await;
        assert!(!manager.has_observers().await);
    }

    #[tokio::test]
    async fn test_emit_sync() {
        let manager = Manager::new();
        let (obs, count) = TestObserver::new("test");
        manager.register(obs).await;

        let event = make_event(EventType::ConversationStart);
        manager.emit_sync(event).await;

        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_emit_async() {
        let manager = Manager::new();
        let (obs, count) = TestObserver::new("test");
        manager.register(obs).await;

        let event = make_event(EventType::ConversationStart);
        manager.emit(event).await;

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_multiple_observers() {
        let manager = Manager::new();
        let (obs1, c1) = TestObserver::new("obs1");
        let (obs2, c2) = TestObserver::new("obs2");

        manager.register(obs1).await;
        manager.register(obs2).await;

        let event = ConversationEvent {
            event_type: EventType::ToolCall,
            trace_id: "test-trace".to_string(),
            timestamp: chrono::Utc::now(),
            data: EventData::ToolCall(ToolCallData {
                tool_name: "test_tool".to_string(),
                arguments: HashMap::new(),
                success: true,
                duration: Duration::from_millis(100),
                error: None,
                llm_round: 1,
                chain_pos: 0,
            }),
        };
        manager.emit_sync(event).await;

        assert_eq!(c1.load(Ordering::SeqCst), 1);
        assert_eq!(c2.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_event_types_display() {
        assert_eq!(EventType::ConversationStart.to_string(), "conversation_start");
        assert_eq!(EventType::ConversationEnd.to_string(), "conversation_end");
        assert_eq!(EventType::LlmRequest.to_string(), "llm_request");
        assert_eq!(EventType::LlmResponse.to_string(), "llm_response");
        assert_eq!(EventType::ToolCall.to_string(), "tool_call");
    }

    #[tokio::test]
    async fn test_unregister_all() {
        let manager = Manager::new();
        let (obs1, c1) = TestObserver::new("obs1");
        let (obs2, c2) = TestObserver::new("obs2");

        manager.register(obs1).await;
        manager.register(obs2).await;
        assert!(manager.has_observers().await);

        manager.unregister_all().await;
        assert!(!manager.has_observers().await);

        // Emit should not increment counters after unregister_all
        let event = make_event(EventType::ConversationStart);
        manager.emit_sync(event).await;
        assert_eq!(c1.load(Ordering::SeqCst), 0);
        assert_eq!(c2.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_unregister_all_on_empty_manager() {
        let manager = Manager::new();
        // Should not panic on empty manager
        manager.unregister_all().await;
        assert!(!manager.has_observers().await);
    }

    #[tokio::test]
    async fn test_manager_default() {
        let manager = Manager::default();
        assert!(!manager.has_observers().await);
    }

    #[tokio::test]
    async fn test_emit_sync_with_panicking_observer() {
        use std::sync::Arc;

        struct PanicObserver {
            name: String,
        }

        #[async_trait]
        impl Observer for PanicObserver {
            fn name(&self) -> &str {
                &self.name
            }

            async fn on_event(&self, _event: ConversationEvent) {
                panic!("intentional panic for testing");
            }
        }

        let manager = Manager::new();
        let (good_obs, good_count) = TestObserver::new("good");
        let panic_obs = Arc::new(PanicObserver {
            name: "panicker".to_string(),
        });

        // Register panicking observer first, then a good one
        manager.register(panic_obs).await;
        manager.register(good_obs).await;

        let event = make_event(EventType::ConversationStart);
        // emit_sync should not panic even though one observer panics;
        // the other observer should still receive the event
        manager.emit_sync(event).await;

        // The good observer should have been called (emit_sync is sequential)
        assert_eq!(good_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_emit_sync_panic_then_good_observer() {
        use std::sync::Arc;

        struct PanicObserver {
            name: String,
        }

        #[async_trait]
        impl Observer for PanicObserver {
            fn name(&self) -> &str {
                &self.name
            }

            async fn on_event(&self, _event: ConversationEvent) {
                panic!("intentional panic");
            }
        }

        let manager = Manager::new();
        let panic_obs = Arc::new(PanicObserver {
            name: "panic_first".to_string(),
        });
        let (after_obs, after_count) = TestObserver::new("after");

        manager.register(panic_obs).await;
        manager.register(after_obs).await;

        let event = make_event(EventType::ConversationStart);
        // emit_sync processes sequentially; after panicking observer,
        // the second one should still be invoked
        manager.emit_sync(event).await;

        assert_eq!(after_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_conversation_end_data_construction() {
        let data = ConversationEndData {
            session_key: "test:chat1".to_string(),
            channel: "web".to_string(),
            chat_id: "chat1".to_string(),
            total_rounds: 3,
            total_duration: Duration::from_secs(120),
            content: "final response".to_string(),
            error: Some("something went wrong".to_string()),
        };

        assert_eq!(data.session_key, "test:chat1");
        assert_eq!(data.channel, "web");
        assert_eq!(data.chat_id, "chat1");
        assert_eq!(data.total_rounds, 3);
        assert_eq!(data.total_duration, Duration::from_secs(120));
        assert_eq!(data.content, "final response");
        assert_eq!(data.error, Some("something went wrong".to_string()));
    }

    #[test]
    fn test_conversation_end_data_no_error() {
        let data = ConversationEndData {
            session_key: "sk".to_string(),
            channel: "rpc".to_string(),
            chat_id: "c1".to_string(),
            total_rounds: 1,
            total_duration: Duration::from_millis(500),
            content: "ok".to_string(),
            error: None,
        };
        assert!(data.error.is_none());
    }

    #[test]
    fn test_llm_request_data_construction() {
        let mut headers = HashMap::new();
        headers.insert("Authorization".to_string(), "Bearer token".to_string());
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        let messages = vec![
            serde_json::json!({"role": "user", "content": "hello"}),
        ];
        let tools = vec![
            serde_json::json!({"type": "function", "function": {"name": "test"}}),
        ];

        let data = LlmRequestData {
            round: 1,
            model: "gpt-4".to_string(),
            provider_name: "openai".to_string(),
            api_key: "sk-xxx".to_string(),
            api_base: "https://api.openai.com/v1".to_string(),
            http_headers: headers.clone(),
            full_config: Some(serde_json::json!({"temperature": 0.7})),
            messages: messages.clone(),
            tools: tools.clone(),
            messages_count: messages.len(),
            tools_count: tools.len(),
        };

        assert_eq!(data.round, 1);
        assert_eq!(data.model, "gpt-4");
        assert_eq!(data.provider_name, "openai");
        assert_eq!(data.api_key, "sk-xxx");
        assert_eq!(data.api_base, "https://api.openai.com/v1");
        assert_eq!(data.http_headers.len(), 2);
        assert!(data.http_headers.contains_key("Authorization"));
        assert!(data.full_config.is_some());
        assert_eq!(data.messages_count, 1);
        assert_eq!(data.tools_count, 1);
    }

    #[test]
    fn test_llm_request_data_minimal() {
        let data = LlmRequestData {
            round: 0,
            model: String::new(),
            provider_name: String::new(),
            api_key: String::new(),
            api_base: String::new(),
            http_headers: HashMap::new(),
            full_config: None,
            messages: vec![],
            tools: vec![],
            messages_count: 0,
            tools_count: 0,
        };

        assert_eq!(data.round, 0);
        assert!(data.model.is_empty());
        assert!(data.full_config.is_none());
        assert!(data.messages.is_empty());
        assert!(data.tools.is_empty());
    }

    #[test]
    fn test_llm_response_data_construction() {
        let usage = UsageInfo {
            prompt_tokens: 100,
            completion_tokens: 50,
            total_tokens: 150,
        };

        let tool_calls = vec![
            serde_json::json!({"id": "call_1", "function": {"name": "test"}}),
        ];

        let data = LlmResponseData {
            round: 2,
            duration: Duration::from_millis(1500),
            content: "Here is the result".to_string(),
            tool_calls: tool_calls.clone(),
            tool_calls_count: tool_calls.len(),
            usage: Some(usage.clone()),
            finish_reason: Some("stop".to_string()),
        };

        assert_eq!(data.round, 2);
        assert_eq!(data.duration, Duration::from_millis(1500));
        assert_eq!(data.content, "Here is the result");
        assert_eq!(data.tool_calls_count, 1);
        assert!(data.usage.is_some());
        let u = data.usage.unwrap();
        assert_eq!(u.prompt_tokens, 100);
        assert_eq!(u.completion_tokens, 50);
        assert_eq!(u.total_tokens, 150);
        assert_eq!(data.finish_reason, Some("stop".to_string()));
    }

    #[test]
    fn test_llm_response_data_no_usage() {
        let data = LlmResponseData {
            round: 0,
            duration: Duration::from_millis(100),
            content: String::new(),
            tool_calls: vec![],
            tool_calls_count: 0,
            usage: None,
            finish_reason: None,
        };

        assert!(data.usage.is_none());
        assert!(data.finish_reason.is_none());
        assert!(data.content.is_empty());
        assert_eq!(data.tool_calls_count, 0);
    }

    #[test]
    fn test_usage_info_construction() {
        let usage = UsageInfo {
            prompt_tokens: 500,
            completion_tokens: 200,
            total_tokens: 700,
        };
        assert_eq!(usage.prompt_tokens, 500);
        assert_eq!(usage.completion_tokens, 200);
        assert_eq!(usage.total_tokens, 700);
    }

    #[tokio::test]
    async fn test_event_data_conversation_end_variant() {
        let manager = Manager::new();
        let (obs, count) = TestObserver::new("test");
        manager.register(obs).await;

        let event = ConversationEvent {
            event_type: EventType::ConversationEnd,
            trace_id: "trace-123".to_string(),
            timestamp: chrono::Utc::now(),
            data: EventData::ConversationEnd(ConversationEndData {
                session_key: "web:chat1".to_string(),
                channel: "web".to_string(),
                chat_id: "chat1".to_string(),
                total_rounds: 5,
                total_duration: Duration::from_secs(60),
                content: "done".to_string(),
                error: None,
            }),
        };

        manager.emit_sync(event).await;
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_event_data_llm_request_variant() {
        let manager = Manager::new();
        let (obs, count) = TestObserver::new("test");
        manager.register(obs).await;

        let event = ConversationEvent {
            event_type: EventType::LlmRequest,
            trace_id: "trace-456".to_string(),
            timestamp: chrono::Utc::now(),
            data: EventData::LlmRequest(LlmRequestData {
                round: 1,
                model: "gpt-4".to_string(),
                provider_name: "openai".to_string(),
                api_key: "key".to_string(),
                api_base: "https://api.example.com".to_string(),
                http_headers: HashMap::new(),
                full_config: None,
                messages: vec![serde_json::json!({"role": "user", "content": "hi"})],
                tools: vec![],
                messages_count: 1,
                tools_count: 0,
            }),
        };

        manager.emit_sync(event).await;
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_event_data_llm_response_variant() {
        let manager = Manager::new();
        let (obs, count) = TestObserver::new("test");
        manager.register(obs).await;

        let event = ConversationEvent {
            event_type: EventType::LlmResponse,
            trace_id: "trace-789".to_string(),
            timestamp: chrono::Utc::now(),
            data: EventData::LlmResponse(LlmResponseData {
                round: 1,
                duration: Duration::from_millis(250),
                content: "response text".to_string(),
                tool_calls: vec![],
                tool_calls_count: 0,
                usage: Some(UsageInfo {
                    prompt_tokens: 10,
                    completion_tokens: 5,
                    total_tokens: 15,
                }),
                finish_reason: Some("stop".to_string()),
            }),
        };

        manager.emit_sync(event).await;
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }
}
