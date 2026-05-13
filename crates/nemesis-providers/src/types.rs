//! Protocol types for LLM communication.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A tool call from the LLM response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub call_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<FunctionCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<HashMap<String, serde_json::Value>>,
}

/// A function call within a tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

/// LLM response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LLMResponse {
    pub content: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageInfo>,
}

/// Token usage info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageInfo {
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub total_tokens: i64,
}

/// A message in the conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tool_calls: Vec<ToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<chrono::DateTime<chrono::Utc>>,
}

/// Tool definition for LLM API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    #[serde(rename = "type", default = "default_tool_type")]
    pub tool_type: String,
    pub function: ToolFunctionDefinition,
}

fn default_tool_type() -> String {
    "function".to_string()
}

/// Tool function definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolFunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// Chat completion request options.
#[derive(Debug, Clone, Default)]
pub struct ChatOptions {
    pub temperature: Option<f64>,
    pub max_tokens: Option<i64>,
    pub top_p: Option<f64>,
    pub stop: Option<Vec<String>>,
    pub extra: HashMap<String, serde_json::Value>,
}

/// Model configuration with primary model and fallback list.
///
/// Used by the failover system to determine which models to try
/// when the primary model is unavailable.
///
/// Mirrors the Go `ModelConfig` struct from `module/providers/types.go`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderModelConfig {
    /// The primary model to use.
    #[serde(default)]
    pub primary: String,
    /// Ordered list of fallback models to try if the primary fails.
    #[serde(default)]
    pub fallbacks: Vec<String>,
}

impl ProviderModelConfig {
    /// Create a new ModelConfig with just a primary model and no fallbacks.
    pub fn new(primary: &str) -> Self {
        Self {
            primary: primary.to_string(),
            fallbacks: Vec::new(),
        }
    }

    /// Create a ModelConfig with a primary model and fallback list.
    pub fn with_fallbacks(primary: &str, fallbacks: &[&str]) -> Self {
        Self {
            primary: primary.to_string(),
            fallbacks: fallbacks.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// Get all model names in priority order (primary first, then fallbacks).
    pub fn all_models(&self) -> Vec<&str> {
        let mut models = vec![self.primary.as_str()];
        for fb in &self.fallbacks {
            models.push(fb.as_str());
        }
        models
    }

    /// Returns true if there are any fallback models configured.
    pub fn has_fallbacks(&self) -> bool {
        !self.fallbacks.is_empty()
    }
}

/// Token source type for providers that support OAuth or token refresh.
///
/// This is a simplified version of the Go `createCodexTokenSource` /
/// `createClaudeTokenSource` functions. The actual credential loading
/// is handled by `codex_credentials` module and `auth` module in Go.
/// In Rust, we provide this enum to represent the token source type,
/// and the actual loading is done at construction time or via callbacks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TokenSourceType {
    /// Static API key, no refresh needed.
    Static,
    /// OAuth-based token with refresh capability.
    OAuth,
    /// CLI-based credentials (e.g., from ~/.codex/auth.json).
    CliCredentials,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_serialization() {
        let msg = Message {
            role: "user".to_string(),
            content: "Hello".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"role\":\"user\""));
    }

    #[test]
    fn test_tool_call_parsing() {
        let json = r#"{"id":"call_123","function":{"name":"read_file","arguments":"{\"path\":\"/tmp/test\"}"}}"#;
        let tc: ToolCall = serde_json::from_str(json).unwrap();
        assert_eq!(tc.id, "call_123");
        assert_eq!(tc.function.as_ref().unwrap().name, "read_file");
    }

    #[test]
    fn test_llm_response_parsing() {
        let json = r#"{
            "content": "Hello!",
            "tool_calls": [],
            "finish_reason": "stop",
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
        }"#;
        let resp: LLMResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.content, "Hello!");
        assert_eq!(resp.usage.unwrap().total_tokens, 15);
    }

    #[test]
    fn test_tool_definition() {
        let td = ToolDefinition {
            tool_type: "function".to_string(),
            function: ToolFunctionDefinition {
                name: "read_file".to_string(),
                description: "Read a file".to_string(),
                parameters: serde_json::json!({"type": "object", "properties": {"path": {"type": "string"}}}),
            },
        };
        let json = serde_json::to_string(&td).unwrap();
        assert!(json.contains("\"read_file\""));
    }

    #[test]
    fn test_provider_model_config_new() {
        let cfg = ProviderModelConfig::new("gpt-4o");
        assert_eq!(cfg.primary, "gpt-4o");
        assert!(cfg.fallbacks.is_empty());
        assert!(!cfg.has_fallbacks());
    }

    #[test]
    fn test_provider_model_config_with_fallbacks() {
        let cfg = ProviderModelConfig::with_fallbacks("gpt-4o", &["gpt-4o-mini", "gpt-3.5-turbo"]);
        assert_eq!(cfg.primary, "gpt-4o");
        assert_eq!(cfg.fallbacks, vec!["gpt-4o-mini", "gpt-3.5-turbo"]);
        assert!(cfg.has_fallbacks());
    }

    #[test]
    fn test_provider_model_config_all_models() {
        let cfg = ProviderModelConfig::with_fallbacks("gpt-4o", &["gpt-4o-mini"]);
        let all = cfg.all_models();
        assert_eq!(all, vec!["gpt-4o", "gpt-4o-mini"]);
    }

    #[test]
    fn test_provider_model_config_serialization() {
        let cfg = ProviderModelConfig::with_fallbacks("gpt-4o", &["gpt-4o-mini"]);
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(json.contains("\"primary\":\"gpt-4o\""));
        assert!(json.contains("\"fallbacks\""));
    }

    #[test]
    fn test_provider_model_config_deserialization() {
        let json = r#"{"primary":"claude-3","fallbacks":["claude-3-haiku"]}"#;
        let cfg: ProviderModelConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.primary, "claude-3");
        assert_eq!(cfg.fallbacks, vec!["claude-3-haiku"]);
    }

    // ============================================================
    // Additional tests for missing coverage
    // ============================================================

    #[test]
    fn test_message_with_tool_calls() {
        let msg = Message {
            role: "assistant".to_string(),
            content: "".to_string(),
            tool_calls: vec![ToolCall {
                id: "call_1".to_string(),
                call_type: Some("function".to_string()),
                function: Some(FunctionCall {
                    name: "read_file".to_string(),
                    arguments: r#"{"path":"/tmp"}"#.to_string(),
                }),
                name: None,
                arguments: None,
            }],
            tool_call_id: None,
            timestamp: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.tool_calls.len(), 1);
    }

    #[test]
    fn test_message_with_tool_call_id() {
        let msg = Message {
            role: "tool".to_string(),
            content: "file contents".to_string(),
            tool_calls: vec![],
            tool_call_id: Some("call_123".to_string()),
            timestamp: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("call_123"));
        let deserialized: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.tool_call_id, Some("call_123".to_string()));
    }

    #[test]
    fn test_message_with_timestamp() {
        let now = chrono::Utc::now();
        let msg = Message {
            role: "user".to_string(),
            content: "hello".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: Some(now),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: Message = serde_json::from_str(&json).unwrap();
        assert!(deserialized.timestamp.is_some());
    }

    #[test]
    fn test_message_skip_empty_tool_calls() {
        let msg = Message {
            role: "user".to_string(),
            content: "hello".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        // Empty tool_calls should be skipped
        assert!(!json.contains("tool_calls"));
    }

    #[test]
    fn test_message_skip_none_fields() {
        let msg = Message {
            role: "user".to_string(),
            content: "hello".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(!json.contains("tool_call_id"));
        assert!(!json.contains("timestamp"));
    }

    #[test]
    fn test_tool_call_serialization_roundtrip() {
        let tc = ToolCall {
            id: "call_abc".to_string(),
            call_type: Some("function".to_string()),
            function: Some(FunctionCall {
                name: "read_file".to_string(),
                arguments: r#"{"path":"/test"}"#.to_string(),
            }),
            name: Some("read_file".to_string()),
            arguments: Some({
                let mut m = HashMap::new();
                m.insert("path".to_string(), serde_json::json!("/test"));
                m
            }),
        };
        let json = serde_json::to_string(&tc).unwrap();
        let deserialized: ToolCall = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, "call_abc");
        assert_eq!(deserialized.call_type, Some("function".to_string()));
        assert_eq!(deserialized.name, Some("read_file".to_string()));
    }

    #[test]
    fn test_tool_call_skip_none_fields() {
        let tc = ToolCall {
            id: "call_1".to_string(),
            call_type: None,
            function: None,
            name: None,
            arguments: None,
        };
        let json = serde_json::to_string(&tc).unwrap();
        assert!(!json.contains("type"));
        assert!(!json.contains("function"));
        assert!(!json.contains("name"));
        assert!(!json.contains("arguments"));
    }

    #[test]
    fn test_function_call_serialization() {
        let fc = FunctionCall {
            name: "test".to_string(),
            arguments: "{}".to_string(),
        };
        let json = serde_json::to_string(&fc).unwrap();
        let deserialized: FunctionCall = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "test");
        assert_eq!(deserialized.arguments, "{}");
    }

    #[test]
    fn test_llm_response_no_usage_serialization() {
        let resp = LLMResponse {
            content: "Hello".to_string(),
            tool_calls: vec![],
            finish_reason: "stop".to_string(),
            usage: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(!json.contains("usage"));
    }

    #[test]
    fn test_llm_response_with_tool_calls_serialization() {
        let resp = LLMResponse {
            content: "".to_string(),
            tool_calls: vec![ToolCall {
                id: "c1".to_string(),
                call_type: Some("function".to_string()),
                function: Some(FunctionCall {
                    name: "tool1".to_string(),
                    arguments: "{}".to_string(),
                }),
                name: None,
                arguments: None,
            }],
            finish_reason: "tool_calls".to_string(),
            usage: Some(UsageInfo {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            }),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let deserialized: LLMResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.tool_calls.len(), 1);
        assert_eq!(deserialized.usage.unwrap().total_tokens, 15);
    }

    #[test]
    fn test_usage_info_serialization() {
        let usage = UsageInfo {
            prompt_tokens: 100,
            completion_tokens: 50,
            total_tokens: 150,
        };
        let json = serde_json::to_string(&usage).unwrap();
        let deserialized: UsageInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.prompt_tokens, 100);
        assert_eq!(deserialized.completion_tokens, 50);
        assert_eq!(deserialized.total_tokens, 150);
    }

    #[test]
    fn test_tool_definition_default_type() {
        let json = r#"{"function":{"name":"test","description":"a test","parameters":{"type":"object"}}}"#;
        let td: ToolDefinition = serde_json::from_str(json).unwrap();
        assert_eq!(td.tool_type, "function");
    }

    #[test]
    fn test_tool_definition_explicit_type() {
        let json = r#"{"type":"custom","function":{"name":"test","description":"a test","parameters":{}}}"#;
        let td: ToolDefinition = serde_json::from_str(json).unwrap();
        assert_eq!(td.tool_type, "custom");
    }

    #[test]
    fn test_tool_function_definition() {
        let tfd = ToolFunctionDefinition {
            name: "my_tool".to_string(),
            description: "A test tool".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path" }
                },
                "required": ["path"]
            }),
        };
        let json = serde_json::to_string(&tfd).unwrap();
        let deserialized: ToolFunctionDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "my_tool");
    }

    #[test]
    fn test_chat_options_default() {
        let opts = ChatOptions::default();
        assert!(opts.temperature.is_none());
        assert!(opts.max_tokens.is_none());
        assert!(opts.top_p.is_none());
        assert!(opts.stop.is_none());
        assert!(opts.extra.is_empty());
    }

    #[test]
    fn test_chat_options_with_extra() {
        let mut extra = HashMap::new();
        extra.insert("custom_field".to_string(), serde_json::json!("custom_value"));
        let opts = ChatOptions {
            temperature: Some(0.7),
            max_tokens: Some(4096),
            top_p: Some(0.9),
            stop: Some(vec!["END".to_string()]),
            extra,
        };
        assert_eq!(opts.temperature, Some(0.7));
        assert_eq!(opts.extra.get("custom_field").unwrap(), "custom_value");
    }

    #[test]
    fn test_provider_model_config_deserialization_defaults() {
        let json = r#"{}"#;
        let cfg: ProviderModelConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.primary, "");
        assert!(cfg.fallbacks.is_empty());
        assert!(!cfg.has_fallbacks());
    }

    #[test]
    fn test_provider_model_config_all_models_empty_fallbacks() {
        let cfg = ProviderModelConfig::new("gpt-4");
        let all = cfg.all_models();
        assert_eq!(all, vec!["gpt-4"]);
    }

    #[test]
    fn test_provider_model_config_all_models_order() {
        let cfg = ProviderModelConfig::with_fallbacks("primary", &["fb1", "fb2", "fb3"]);
        let all = cfg.all_models();
        assert_eq!(all, vec!["primary", "fb1", "fb2", "fb3"]);
    }

    #[test]
    fn test_token_source_type_serialization() {
        let ts = TokenSourceType::Static;
        let json = serde_json::to_string(&ts).unwrap();
        assert!(json.contains("Static"));

        let ts = TokenSourceType::OAuth;
        let json = serde_json::to_string(&ts).unwrap();
        assert!(json.contains("OAuth"));

        let ts = TokenSourceType::CliCredentials;
        let json = serde_json::to_string(&ts).unwrap();
        assert!(json.contains("CliCredentials"));
    }

    #[test]
    fn test_token_source_type_deserialization() {
        let ts: TokenSourceType = serde_json::from_str("\"Static\"").unwrap();
        assert!(matches!(ts, TokenSourceType::Static));

        let ts: TokenSourceType = serde_json::from_str("\"OAuth\"").unwrap();
        assert!(matches!(ts, TokenSourceType::OAuth));

        let ts: TokenSourceType = serde_json::from_str("\"CliCredentials\"").unwrap();
        assert!(matches!(ts, TokenSourceType::CliCredentials));
    }

    // ---- Additional coverage tests for 95%+ ----

    #[test]
    fn test_tool_call_serialization() {
        let tc = ToolCall {
            id: "call_123".to_string(),
            call_type: Some("function".to_string()),
            function: Some(FunctionCall {
                name: "file_read".to_string(),
                arguments: "{\"path\": \"/tmp/test\"}".to_string(),
            }),
            name: None,
            arguments: None,
        };
        let json = serde_json::to_string(&tc).unwrap();
        let parsed: ToolCall = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, "call_123");
        assert!(parsed.function.is_some());
        assert_eq!(parsed.function.unwrap().name, "file_read");
    }

    #[test]
    fn test_tool_call_minimal() {
        let tc = ToolCall {
            id: "call_456".to_string(),
            call_type: None,
            function: None,
            name: Some("direct_tool".to_string()),
            arguments: Some({
                let mut m = HashMap::new();
                m.insert("key".to_string(), serde_json::json!("value"));
                m
            }),
        };
        let json = serde_json::to_string(&tc).unwrap();
        let parsed: ToolCall = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, Some("direct_tool".to_string()));
        assert!(parsed.arguments.is_some());
    }

    #[test]
    fn test_llm_response_serialization() {
        let resp = LLMResponse {
            content: "Hello!".to_string(),
            tool_calls: vec![],
            finish_reason: "stop".to_string(),
            usage: Some(UsageInfo {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            }),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: LLMResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.content, "Hello!");
        assert_eq!(parsed.finish_reason, "stop");
        assert!(resp.usage.is_some());
    }

    #[test]
    fn test_provider_model_config_has_fallbacks_v2() {
        let cfg = ProviderModelConfig::with_fallbacks("primary", &["fb1"]);
        assert!(cfg.has_fallbacks());
    }

    #[test]
    fn test_provider_model_config_no_fallbacks_v2() {
        let cfg = ProviderModelConfig::new("only_model");
        assert!(!cfg.has_fallbacks());
    }

    #[test]
    fn test_tool_definition_serialization() {
        let td = ToolDefinition {
            tool_type: "function".to_string(),
            function: ToolFunctionDefinition {
                name: "test_tool".to_string(),
                description: "test description".to_string(),
                parameters: serde_json::json!({"type": "object"}),
            },
        };
        let json = serde_json::to_string(&td).unwrap();
        assert!(json.contains("test_tool"));
        assert!(json.contains("function"));
    }
}
