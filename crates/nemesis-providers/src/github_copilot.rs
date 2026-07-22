//! GitHub Copilot provider.

use crate::failover::FailoverError;
use crate::router::LLMProvider;
use crate::types::*;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

const DEFAULT_COPILOT_MODEL: &str = "gpt-4.1";

/// GitHub Copilot provider configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubCopilotConfig {
    #[serde(default)]
    pub uri: String,
    #[serde(default)]
    pub connect_mode: String,
    #[serde(default)]
    pub default_model: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_timeout() -> u64 {
    120
}

impl Default for GitHubCopilotConfig {
    fn default() -> Self {
        Self {
            uri: String::new(),
            connect_mode: "grpc".to_string(),
            default_model: DEFAULT_COPILOT_MODEL.to_string(),
            timeout_secs: 120,
        }
    }
}

/// GitHub Copilot provider.
///
/// Note: The real Go implementation uses the `copilot-sdk-go` library for gRPC
/// communication. This Rust implementation provides an HTTP-based bridge that
/// communicates with the Copilot CLI server via HTTP. The gRPC path would require
/// additional proto definitions and the `tonic` crate.
pub struct GitHubCopilotProvider {
    config: GitHubCopilotConfig,
    client: reqwest::Client,
}

impl GitHubCopilotProvider {
    pub fn new(config: GitHubCopilotConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout_secs))
            .build()
            .expect("failed to build HTTP client");
        Self { config, client }
    }

    /// Serialize messages to a prompt string for Copilot.
    fn messages_to_prompt(&self, messages: &[Message]) -> String {
        let prompt_messages: Vec<serde_json::Value> = messages
            .iter()
            .map(|msg| {
                serde_json::json!({
                    "role": msg.role,
                    "content": msg.content
                })
            })
            .collect();

        serde_json::to_string(&prompt_messages).unwrap_or_default()
    }
}

#[async_trait]
impl LLMProvider for GitHubCopilotProvider {
    async fn chat(
        &self,
        messages: &[Message],
        _tools: &[ToolDefinition],
        _model: &str,
        _options: &ChatOptions,
    ) -> Result<LLMResponse, FailoverError> {
        let prompt = self.messages_to_prompt(messages);

        let url = if self.config.uri.is_empty() {
            "http://localhost:8080/copilot/chat".to_string()
        } else {
            format!("{}/copilot/chat", self.config.uri.trim_end_matches('/'))
        };

        let body = serde_json::json!({
            "prompt": prompt,
        });

        let resp = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|_| FailoverError::Timeout {
                provider: "github-copilot".to_string(),
                model: self.config.default_model.clone(),
            })?;

        let status = resp.status().as_u16();
        if status >= 400 {
            let text = resp.text().await.unwrap_or_default();
            return Err(FailoverError::from_status(
                "github-copilot",
                &self.config.default_model,
                status,
                &text,
            ));
        }

        let content: String = resp.text().await.unwrap_or_default();

        Ok(LLMResponse {
            content,
            tool_calls: vec![],
            finish_reason: "stop".to_string(),
            usage: None,
            reasoning_content: None,
            extra: std::collections::HashMap::new(),
            raw_request_body: None,
            raw_response_body: None,
        })
    }

    fn default_model(&self) -> &str {
        &self.config.default_model
    }

    fn name(&self) -> &str {
        "github-copilot"
    }
}

#[cfg(test)]
mod tests;
