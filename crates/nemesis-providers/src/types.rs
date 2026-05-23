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
    /// Reasoning content from thinking-mode models (e.g., DeepSeek R1, GLM).
    /// Must be passed back to the API in subsequent turns.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reasoning_content: Option<String>,
    /// Passthrough for any unknown fields from the API response.
    /// Captured via serde flatten so future API fields are never silently dropped.
    #[serde(flatten, default)]
    pub extra: HashMap<String, serde_json::Value>,
}

/// Token usage info.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageInfo {
    #[serde(default)]
    pub prompt_tokens: i64,
    #[serde(default)]
    pub completion_tokens: i64,
    #[serde(default)]
    pub total_tokens: i64,
    /// Cached prompt tokens (DeepSeek: prompt_cache_hit_tokens, OpenAI: cached_tokens in prompt_tokens_details).
    #[serde(default, skip_serializing_if = "Option::is_none", alias = "prompt_cache_hit_tokens")]
    pub cached_tokens: Option<i64>,
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
    /// Reasoning content from thinking-mode models (e.g., DeepSeek R1, GLM).
    /// Must be passed back to the API in subsequent assistant turns.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reasoning_content: Option<String>,
    /// Passthrough for any unknown fields from the API.
    /// Captured via serde flatten so future API fields are never silently dropped.
    #[serde(flatten, default)]
    pub extra: HashMap<String, serde_json::Value>,
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
mod tests;
