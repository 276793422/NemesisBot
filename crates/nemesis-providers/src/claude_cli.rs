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
            })
        } else {
            None
        };

        Ok(LLMResponse {
            content: content.trim().to_string(),
            tool_calls,
            finish_reason: finish_reason.to_string(),
            usage,
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
mod tests {
    use super::*;

    #[test]
    fn test_build_system_prompt() {
        let provider = ClaudeCliProvider::new(ClaudeCliConfig::default());
        let messages = vec![Message {
            role: "system".to_string(),
            content: "You are helpful".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
        }];
        let prompt = provider.build_system_prompt(&messages, &[]);
        assert_eq!(prompt, "You are helpful");
    }

    #[test]
    fn test_build_tools_prompt() {
        let provider = ClaudeCliProvider::new(ClaudeCliConfig::default());
        let tools = vec![ToolDefinition {
            tool_type: "function".to_string(),
            function: ToolFunctionDefinition {
                name: "read_file".to_string(),
                description: "Read a file".to_string(),
                parameters: serde_json::json!({"type": "object"}),
            },
        }];
        let prompt = provider.build_tools_prompt(&tools);
        assert!(prompt.contains("Available Tools"));
        assert!(prompt.contains("read_file"));
    }

    #[test]
    fn test_messages_to_prompt_single_user() {
        let provider = ClaudeCliProvider::new(ClaudeCliConfig::default());
        let messages = vec![Message {
            role: "user".to_string(),
            content: "Hello world".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
        }];
        let prompt = provider.messages_to_prompt(&messages);
        assert_eq!(prompt, "Hello world");
    }

    #[test]
    fn test_messages_to_prompt_multi() {
        let provider = ClaudeCliProvider::new(ClaudeCliConfig::default());
        let messages = vec![
            Message {
                role: "user".to_string(),
                content: "Hello".to_string(),
                tool_calls: vec![],
                tool_call_id: None,
                timestamp: None,
            },
            Message {
                role: "assistant".to_string(),
                content: "Hi there".to_string(),
                tool_calls: vec![],
                tool_call_id: None,
                timestamp: None,
            },
        ];
        let prompt = provider.messages_to_prompt(&messages);
        assert!(prompt.contains("User: Hello"));
        assert!(prompt.contains("Assistant: Hi there"));
    }

    #[test]
    fn test_parse_response_text() {
        let provider = ClaudeCliProvider::new(ClaudeCliConfig::default());
        let output = r#"{"type":"result","is_error":false,"result":"Hello!","usage":{"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}"#;
        let resp = provider.parse_response(output).unwrap();
        assert_eq!(resp.content, "Hello!");
        assert_eq!(resp.finish_reason, "stop");
        assert!(resp.tool_calls.is_empty());
        assert_eq!(resp.usage.unwrap().total_tokens, 15);
    }

    #[test]
    fn test_parse_response_error() {
        let provider = ClaudeCliProvider::new(ClaudeCliConfig::default());
        let output = r#"{"type":"result","is_error":true,"result":"Something went wrong","usage":{}}"#;
        let result = provider.parse_response(output);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_response_with_tool_calls() {
        let provider = ClaudeCliProvider::new(ClaudeCliConfig::default());
        let output = r#"{"type":"result","is_error":false,"result":"Using tool {\"tool_calls\":[{\"id\":\"c1\",\"type\":\"function\",\"function\":{\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\":\\\"/tmp\\\"}\"}}]}","usage":{"input_tokens":20,"output_tokens":10}}"#;
        // Note: this test uses the extract_tool_calls_from_text internally
        // The result field has escaped JSON which should be parsed by the extract function
        let _resp = provider.parse_response(output).unwrap();
    }

    #[test]
    fn test_config_default() {
        let config = ClaudeCliConfig::default();
        assert_eq!(config.command, "claude");
        assert_eq!(config.default_model, "claude-code");
    }

    // -- Additional tests --

    #[test]
    fn test_claude_cli_config_serialization_roundtrip() {
        let config = ClaudeCliConfig {
            command: "custom-claude".into(),
            workspace: "/tmp/project".into(),
            default_model: "claude-3-opus".into(),
        };
        let json = serde_json::to_string(&config).unwrap();
        let back: ClaudeCliConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.command, "custom-claude");
        assert_eq!(back.workspace, "/tmp/project");
        assert_eq!(back.default_model, "claude-3-opus");
    }

    #[test]
    fn test_claude_cli_config_deserialization_defaults() {
        let json = r#"{}"#;
        let config: ClaudeCliConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.command, "claude");
        assert!(config.workspace.is_empty());
        assert!(config.default_model.is_empty());
    }

    #[test]
    fn test_build_system_prompt_with_tools() {
        let provider = ClaudeCliProvider::new(ClaudeCliConfig::default());
        let messages = vec![
            Message {
                role: "system".to_string(),
                content: "Be helpful".to_string(),
                tool_calls: vec![],
                tool_call_id: None,
                timestamp: None,
            },
        ];
        let tools = vec![ToolDefinition {
            tool_type: "function".to_string(),
            function: ToolFunctionDefinition {
                name: "read_file".to_string(),
                description: "Read a file".to_string(),
                parameters: serde_json::json!({"type": "object"}),
            },
        }];
        let prompt = provider.build_system_prompt(&messages, &tools);
        assert!(prompt.contains("Be helpful"));
        assert!(prompt.contains("Available Tools"));
        assert!(prompt.contains("read_file"));
    }

    #[test]
    fn test_build_system_prompt_no_system_messages() {
        let provider = ClaudeCliProvider::new(ClaudeCliConfig::default());
        let messages = vec![Message {
            role: "user".to_string(),
            content: "Hello".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
        }];
        let prompt = provider.build_system_prompt(&messages, &[]);
        assert!(prompt.is_empty());
    }

    #[test]
    fn test_messages_to_prompt_with_tool_result() {
        let provider = ClaudeCliProvider::new(ClaudeCliConfig::default());
        let messages = vec![
            Message {
                role: "user".to_string(),
                content: "Check".to_string(),
                tool_calls: vec![],
                tool_call_id: None,
                timestamp: None,
            },
            Message {
                role: "tool".to_string(),
                content: "result data".to_string(),
                tool_calls: vec![],
                tool_call_id: Some("call_1".into()),
                timestamp: None,
            },
        ];
        let prompt = provider.messages_to_prompt(&messages);
        assert!(prompt.contains("[Tool Result for call_1]: result data"));
    }

    #[test]
    fn test_messages_to_prompt_system_ignored() {
        let provider = ClaudeCliProvider::new(ClaudeCliConfig::default());
        let messages = vec![
            Message {
                role: "system".to_string(),
                content: "System prompt".to_string(),
                tool_calls: vec![],
                tool_call_id: None,
                timestamp: None,
            },
            Message {
                role: "user".to_string(),
                content: "Hello".to_string(),
                tool_calls: vec![],
                tool_call_id: None,
                timestamp: None,
            },
        ];
        let prompt = provider.messages_to_prompt(&messages);
        // System messages should not appear in the prompt (handled by --system-prompt flag)
        assert!(!prompt.contains("System prompt"));
        assert_eq!(prompt, "Hello"); // single user message simplified
    }

    #[test]
    fn test_messages_to_prompt_tool_without_call_id() {
        let provider = ClaudeCliProvider::new(ClaudeCliConfig::default());
        let messages = vec![
            Message {
                role: "tool".to_string(),
                content: "orphan result".to_string(),
                tool_calls: vec![],
                tool_call_id: None,
                timestamp: None,
            },
        ];
        let prompt = provider.messages_to_prompt(&messages);
        // Tool message without tool_call_id should be skipped
        assert!(prompt.is_empty());
    }

    #[test]
    fn test_parse_response_with_usage() {
        let provider = ClaudeCliProvider::new(ClaudeCliConfig::default());
        let output = r#"{"type":"result","is_error":false,"result":"Done!","usage":{"input_tokens":100,"output_tokens":50,"cache_creation_input_tokens":10,"cache_read_input_tokens":20}}"#;
        let resp = provider.parse_response(output).unwrap();
        assert_eq!(resp.content, "Done!");
        let usage = resp.usage.unwrap();
        assert_eq!(usage.prompt_tokens, 130); // 100 + 10 + 20
        assert_eq!(usage.completion_tokens, 50);
        assert_eq!(usage.total_tokens, 180);
    }

    #[test]
    fn test_parse_response_no_usage() {
        let provider = ClaudeCliProvider::new(ClaudeCliConfig::default());
        let output = r#"{"type":"result","is_error":false,"result":"No usage info","usage":{}}"#;
        let resp = provider.parse_response(output).unwrap();
        assert_eq!(resp.content, "No usage info");
        assert!(resp.usage.is_none()); // Both input and output are 0
    }

    #[test]
    fn test_parse_response_invalid_json() {
        let provider = ClaudeCliProvider::new(ClaudeCliConfig::default());
        let result = provider.parse_response("not json at all");
        assert!(result.is_err());
    }

    #[test]
    fn test_provider_name_and_default_model() {
        let provider = ClaudeCliProvider::new(ClaudeCliConfig::default());
        assert_eq!(provider.name(), "claude-cli");
        assert_eq!(provider.default_model(), "claude-code");
    }
}
