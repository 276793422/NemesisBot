//! Claude CLI wrapper provider.

use crate::failover::FailoverError;
use crate::router::LLMProvider;
use crate::tool_call_extract::{extract_tool_calls_from_text, strip_tool_calls_from_text};
use crate::types::*;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Claude CLI provider configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeCliConfig {
    #[serde(default = "default_command")]
    pub command: String,
    #[serde(default)]
    pub workspace: String,
    #[serde(default)]
    pub default_model: String,
}

fn default_command() -> String {
    "claude".to_string()
}

impl Default for ClaudeCliConfig {
    fn default() -> Self {
        Self {
            command: "claude".to_string(),
            workspace: String::new(),
            default_model: "claude-code".to_string(),
        }
    }
}

/// Claude CLI JSON response format (matches claude CLI v2.x output).
#[derive(Debug, Deserialize)]
struct ClaudeCliResponse {
    #[allow(dead_code)]
    #[serde(rename = "type", default)]
    resp_type: String,
    #[serde(default)]
    is_error: bool,
    #[serde(default)]
    result: String,
    #[serde(default)]
    usage: ClaudeCliUsage,
}

#[derive(Debug, Default, Deserialize)]
struct ClaudeCliUsage {
    #[serde(default)]
    input_tokens: i64,
    #[serde(default)]
    output_tokens: i64,
    #[serde(default)]
    cache_creation_input_tokens: i64,
    #[serde(default)]
    cache_read_input_tokens: i64,
}

/// Provider that wraps the `claude` CLI as a subprocess.
pub struct ClaudeCliProvider {
    config: ClaudeCliConfig,
}

impl ClaudeCliProvider {
    pub fn new(config: ClaudeCliConfig) -> Self {
        Self { config }
    }

    /// Build the system prompt from system messages and tool definitions.
    fn build_system_prompt(&self, messages: &[Message], tools: &[ToolDefinition]) -> String {
        let mut parts = Vec::new();

        for msg in messages {
            if msg.role == "system" {
                parts.push(msg.content.clone());
            }
        }

        if !tools.is_empty() {
            parts.push(self.build_tools_prompt(tools));
        }

        parts.join("\n\n")
    }

    /// Build tools prompt section for the CLI.
    fn build_tools_prompt(&self, tools: &[ToolDefinition]) -> String {
        let mut sb = String::new();
        sb.push_str("## Available Tools\n\n");
        sb.push_str("When you need to use a tool, respond with ONLY a JSON object:\n\n");
        sb.push_str("```json\n");
        sb.push_str(r#"{"tool_calls":[{"id":"call_xxx","type":"function","function":{"name":"tool_name","arguments":"{...}"}}]}"#);
        sb.push_str("\n```\n\n");
        sb.push_str("CRITICAL: The 'arguments' field MUST be a JSON-encoded STRING.\n\n");
        sb.push_str("### Tool Definitions:\n\n");

        for tool in tools {
            if tool.tool_type != "function" {
                continue;
            }
            sb.push_str(&format!("#### {}\n", tool.function.name));
            if !tool.function.description.is_empty() {
                sb.push_str(&format!("Description: {}\n", tool.function.description));
            }
            if !tool.function.parameters.is_null() {
                let params_json =
                    serde_json::to_string_pretty(&tool.function.parameters).unwrap_or_default();
                sb.push_str(&format!("Parameters:\n```json\n{}\n```\n", params_json));
            }
            sb.push('\n');
        }

        sb
    }

    /// Convert messages to CLI-compatible prompt string.
    fn messages_to_prompt(&self, messages: &[Message]) -> String {
        let mut parts = Vec::new();

        for msg in messages {
            match msg.role.as_str() {
                "system" => { /* handled via --system-prompt flag */ }
                "user" => parts.push(format!("User: {}", msg.content)),
                "assistant" => parts.push(format!("Assistant: {}", msg.content)),
                "tool" => {
                    if let Some(ref tc_id) = msg.tool_call_id {
                        parts.push(format!("[Tool Result for {}]: {}", tc_id, msg.content));
                    }
                }
                _ => {}
            }
        }

        // Simplify single user message
        if parts.len() == 1 && parts[0].starts_with("User: ") {
            return parts[0].strip_prefix("User: ").unwrap().to_string();
        }

        parts.join("\n")
    }

    /// Parse the JSON output from the claude CLI.
    fn parse_response(&self, output: &str) -> Result<LLMResponse, FailoverError> {
        let resp: ClaudeCliResponse = serde_json::from_str(output).map_err(|e| {
            FailoverError::Format {
                provider: "claude-cli".to_string(),
                message: format!("failed to parse claude cli response: {}", e),
            }
        })?;

        if resp.is_error {
            return Err(FailoverError::Unknown {
                provider: "claude-cli".to_string(),
                message: resp.result,
            });
        }

        let tool_calls = extract_tool_calls_from_text(&resp.result);
        let finish_reason = if !tool_calls.is_empty() {
            "tool_calls"
        } else {
            "stop"
        };
        let content = if !tool_calls.is_empty() {
            strip_tool_calls_from_text(&resp.result)
        } else {
            resp.result.clone()
        };

        let usage = if resp.usage.input_tokens > 0 || resp.usage.output_tokens > 0 {
            Some(UsageInfo {
                prompt_tokens: resp.usage.input_tokens
                    + resp.usage.cache_creation_input_tokens
                    + resp.usage.cache_read_input_tokens,
                completion_tokens: resp.usage.output_tokens,
                total_tokens: resp.usage.input_tokens
                    + resp.usage.cache_creation_input_tokens
                    + resp.usage.cache_read_input_tokens
                    + resp.usage.output_tokens,
                cached_tokens: None,
            })
        } else {
            None
        };

        Ok(LLMResponse {
            content: content.trim().to_string(),
            tool_calls,
            finish_reason: finish_reason.to_string(),
            usage,
            reasoning_content: None,
    extra: std::collections::HashMap::new(),
        })
    }
}

#[async_trait]
impl LLMProvider for ClaudeCliProvider {
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        model: &str,
        _options: &ChatOptions,
    ) -> Result<LLMResponse, FailoverError> {
        let system_prompt = self.build_system_prompt(messages, tools);
        let _prompt = self.messages_to_prompt(messages);

        let mut args = vec![
            "-p".to_string(),
            "--output-format".to_string(),
            "json".to_string(),
            "--dangerously-skip-permissions".to_string(),
            "--no-chrome".to_string(),
        ];

        if !system_prompt.is_empty() {
            args.push("--system-prompt".to_string());
            args.push(system_prompt);
        }

        let effective_model = if model.is_empty() || model == "claude-code" {
            ""
        } else {
            model
        };
        if !effective_model.is_empty() {
            args.push("--model".to_string());
            args.push(effective_model.to_string());
        }

        args.push("-".to_string()); // read from stdin

        let output = tokio::process::Command::new(&self.config.command)
            .args(&args)
            .current_dir(if self.config.workspace.is_empty() {
                "."
            } else {
                &self.config.workspace
            })
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .await
            .map_err(|e| FailoverError::Unknown {
                provider: "claude-cli".to_string(),
                message: format!("failed to execute claude cli: {}", e),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.is_empty() {
                return Err(FailoverError::Unknown {
                    provider: "claude-cli".to_string(),
                    message: format!("claude cli error: {}", stderr),
                });
            }
            return Err(FailoverError::Unknown {
                provider: "claude-cli".to_string(),
                message: format!("claude cli exited with status: {}", output.status),
            });
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        self.parse_response(&stdout)
    }

    fn default_model(&self) -> &str {
        &self.config.default_model
    }

    fn name(&self) -> &str {
        "claude-cli"
    }
}

#[cfg(test)]
mod tests;
