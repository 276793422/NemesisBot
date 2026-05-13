//! Codex CLI wrapper provider.

use crate::failover::FailoverError;
use crate::router::LLMProvider;
use crate::tool_call_extract::{extract_tool_calls_from_text, strip_tool_calls_from_text};
use crate::types::*;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Codex CLI provider configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexCliConfig {
    #[serde(default = "default_command")]
    pub command: String,
    #[serde(default)]
    pub workspace: String,
    #[serde(default)]
    pub default_model: String,
}

fn default_command() -> String {
    "codex".to_string()
}

impl Default for CodexCliConfig {
    fn default() -> Self {
        Self {
            command: "codex".to_string(),
            workspace: String::new(),
            default_model: "codex-cli".to_string(),
        }
    }
}

/// A single JSONL event from `codex exec --json`.
#[derive(Debug, Deserialize)]
struct CodexEvent {
    #[serde(rename = "type", default)]
    event_type: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    item: Option<CodexEventItem>,
    #[serde(default)]
    usage: Option<CodexUsage>,
    #[serde(default)]
    error: Option<CodexEventErr>,
}

#[derive(Debug, Deserialize)]
struct CodexEventItem {
    #[serde(rename = "type", default)]
    item_type: String,
    #[serde(default)]
    text: String,
}

#[derive(Debug, Deserialize)]
struct CodexUsage {
    #[serde(default)]
    input_tokens: i64,
    #[serde(default)]
    cached_input_tokens: i64,
    #[serde(default)]
    output_tokens: i64,
}

#[derive(Debug, Deserialize)]
struct CodexEventErr {
    #[serde(default)]
    message: String,
}

/// Provider that wraps the `codex` CLI as a subprocess.
pub struct CodexCliProvider {
    config: CodexCliConfig,
}

impl CodexCliProvider {
    pub fn new(config: CodexCliConfig) -> Self {
        Self { config }
    }

    /// Build prompt from messages, combining system messages and tool definitions.
    fn build_prompt(&self, messages: &[Message], tools: &[ToolDefinition]) -> String {
        let mut system_parts = Vec::new();
        let mut conversation_parts = Vec::new();

        for msg in messages {
            match msg.role.as_str() {
                "system" => system_parts.push(msg.content.clone()),
                "user" => conversation_parts.push(msg.content.clone()),
                "assistant" => conversation_parts.push(format!("Assistant: {}", msg.content)),
                "tool" => {
                    if let Some(ref tc_id) = msg.tool_call_id {
                        conversation_parts.push(format!(
                            "[Tool Result for {}]: {}",
                            tc_id, msg.content
                        ));
                    }
                }
                _ => {}
            }
        }

        let mut sb = String::new();

        if !system_parts.is_empty() {
            sb.push_str("## System Instructions\n\n");
            sb.push_str(&system_parts.join("\n\n"));
            sb.push_str("\n\n## Task\n\n");
        }

        if !tools.is_empty() {
            sb.push_str(&self.build_tools_prompt(tools));
            sb.push_str("\n\n");
        }

        // Simplify single user message (no prefix)
        if conversation_parts.len() == 1 && system_parts.is_empty() && tools.is_empty() {
            return conversation_parts[0].clone();
        }

        sb.push_str(&conversation_parts.join("\n"));
        sb
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

    /// Parse JSONL events from codex exec --json output.
    fn parse_jsonl_events(&self, output: &str) -> Result<LLMResponse, FailoverError> {
        let mut content_parts = Vec::new();
        let mut usage = None;
        let mut last_error = String::new();

        for line in output.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let event: CodexEvent = match serde_json::from_str(line) {
                Ok(e) => e,
                Err(_) => continue, // skip malformed lines
            };

            match event.event_type.as_str() {
                "item.completed" => {
                    if let Some(item) = event.item {
                        if item.item_type == "agent_message" && !item.text.is_empty() {
                            content_parts.push(item.text);
                        }
                    }
                }
                "turn.completed" => {
                    if let Some(u) = event.usage {
                        let prompt = u.input_tokens + u.cached_input_tokens;
                        usage = Some(UsageInfo {
                            prompt_tokens: prompt,
                            completion_tokens: u.output_tokens,
                            total_tokens: prompt + u.output_tokens,
                        });
                    }
                }
                "error" => {
                    last_error = event.message;
                }
                "turn.failed" => {
                    if let Some(err) = event.error {
                        last_error = err.message;
                    }
                }
                _ => {}
            }
        }

        if !last_error.is_empty() && content_parts.is_empty() {
            return Err(FailoverError::Unknown {
                provider: "codex-cli".to_string(),
                message: format!("codex cli: {}", last_error),
            });
        }

        let content = content_parts.join("\n");
        let tool_calls = extract_tool_calls_from_text(&content);

        let finish_reason = if !tool_calls.is_empty() {
            "tool_calls"
        } else {
            "stop"
        };
        let content = if !tool_calls.is_empty() {
            strip_tool_calls_from_text(&content)
        } else {
            content
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
impl LLMProvider for CodexCliProvider {
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        model: &str,
        _options: &ChatOptions,
    ) -> Result<LLMResponse, FailoverError> {
        if self.config.command.is_empty() {
            return Err(FailoverError::Unknown {
                provider: "codex-cli".to_string(),
                message: "codex command not configured".to_string(),
            });
        }

        let _prompt = self.build_prompt(messages, tools);

        let mut args = vec![
            "exec".to_string(),
            "--json".to_string(),
            "--dangerously-bypass-approvals-and-sandbox".to_string(),
            "--skip-git-repo-check".to_string(),
            "--color".to_string(),
            "never".to_string(),
        ];

        let effective_model = if model.is_empty() || model == "codex-cli" {
            ""
        } else {
            model
        };
        if !effective_model.is_empty() {
            args.push("-m".to_string());
            args.push(effective_model.to_string());
        }
        if !self.config.workspace.is_empty() {
            args.push("-C".to_string());
            args.push(self.config.workspace.clone());
        }
        args.push("-".to_string()); // read prompt from stdin

        let output = tokio::process::Command::new(&self.config.command)
            .args(&args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .await
            .map_err(|e| FailoverError::Unknown {
                provider: "codex-cli".to_string(),
                message: format!("failed to execute codex cli: {}", e),
            })?;

        let stdout = String::from_utf8_lossy(&output.stdout);

        // Parse JSONL from stdout even if exit code is non-zero
        if !stdout.is_empty() {
            if let Ok(resp) = self.parse_jsonl_events(&stdout) {
                if !resp.content.is_empty() || !resp.tool_calls.is_empty() {
                    return Ok(resp);
                }
            }
        }

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.is_empty() {
                return Err(FailoverError::Unknown {
                    provider: "codex-cli".to_string(),
                    message: format!("codex cli error: {}", stderr),
                });
            }
            return Err(FailoverError::Unknown {
                provider: "codex-cli".to_string(),
                message: format!("codex cli exited with status: {}", output.status),
            });
        }

        self.parse_jsonl_events(&stdout)
    }

    fn default_model(&self) -> &str {
        &self.config.default_model
    }

    fn name(&self) -> &str {
        "codex-cli"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_prompt_single_user() {
        let provider = CodexCliProvider::new(CodexCliConfig::default());
        let messages = vec![Message {
            role: "user".to_string(),
            content: "Hello".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
        }];
        let prompt = provider.build_prompt(&messages, &[]);
        assert_eq!(prompt, "Hello");
    }

    #[test]
    fn test_build_prompt_with_system() {
        let provider = CodexCliProvider::new(CodexCliConfig::default());
        let messages = vec![
            Message {
                role: "system".to_string(),
                content: "Be helpful".to_string(),
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
        let prompt = provider.build_prompt(&messages, &[]);
        assert!(prompt.contains("System Instructions"));
        assert!(prompt.contains("Be helpful"));
    }

    #[test]
    fn test_build_tools_prompt() {
        let provider = CodexCliProvider::new(CodexCliConfig::default());
        let tools = vec![ToolDefinition {
            tool_type: "function".to_string(),
            function: ToolFunctionDefinition {
                name: "read_file".to_string(),
                description: "Read".to_string(),
                parameters: serde_json::json!({"type": "object"}),
            },
        }];
        let prompt = provider.build_tools_prompt(&tools);
        assert!(prompt.contains("Available Tools"));
        assert!(prompt.contains("read_file"));
    }

    #[test]
    fn test_parse_jsonl_agent_message() {
        let provider = CodexCliProvider::new(CodexCliConfig::default());
        let output = r#"{"type":"item.completed","item":{"id":"i1","type":"agent_message","text":"Hello!"}}
{"type":"turn.completed","usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":5}}"#;
        let resp = provider.parse_jsonl_events(output).unwrap();
        assert_eq!(resp.content, "Hello!");
        assert_eq!(resp.usage.unwrap().total_tokens, 17);
    }

    #[test]
    fn test_parse_jsonl_error_only() {
        let provider = CodexCliProvider::new(CodexCliConfig::default());
        let output = r#"{"type":"error","message":"something went wrong"}"#;
        let result = provider.parse_jsonl_events(output);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_jsonl_error_with_content() {
        let provider = CodexCliProvider::new(CodexCliConfig::default());
        let output = r#"{"type":"item.completed","item":{"id":"i1","type":"agent_message","text":"Partial result"}}
{"type":"turn.failed","error":{"message":"api error"}}"#;
        let resp = provider.parse_jsonl_events(output).unwrap();
        assert_eq!(resp.content, "Partial result"); // content wins over error
    }

    #[test]
    fn test_parse_jsonl_empty() {
        let provider = CodexCliProvider::new(CodexCliConfig::default());
        let resp = provider.parse_jsonl_events("").unwrap();
        assert_eq!(resp.content, "");
        assert!(resp.tool_calls.is_empty());
    }

    #[test]
    fn test_parse_jsonl_malformed_lines_skipped() {
        let provider = CodexCliProvider::new(CodexCliConfig::default());
        let output = "not json\n{\"type\":\"item.completed\",\"item\":{\"id\":\"i1\",\"type\":\"agent_message\",\"text\":\"OK\"}}\nalso not json";
        let resp = provider.parse_jsonl_events(output).unwrap();
        assert_eq!(resp.content, "OK");
    }

    #[test]
    fn test_config_default() {
        let config = CodexCliConfig::default();
        assert_eq!(config.command, "codex");
        assert_eq!(config.default_model, "codex-cli");
    }

    // -- Additional tests --

    #[test]
    fn test_codex_cli_config_serialization_roundtrip() {
        let config = CodexCliConfig {
            command: "custom-codex".into(),
            workspace: "/tmp/workspace".into(),
            default_model: "o3-mini".into(),
        };
        let json = serde_json::to_string(&config).unwrap();
        let back: CodexCliConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.command, "custom-codex");
        assert_eq!(back.workspace, "/tmp/workspace");
        assert_eq!(back.default_model, "o3-mini");
    }

    #[test]
    fn test_codex_cli_config_deserialization_defaults() {
        let json = r#"{}"#;
        let config: CodexCliConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.command, "codex"); // default via serde
        assert!(config.workspace.is_empty());
        assert!(config.default_model.is_empty());
    }

    #[test]
    fn test_build_prompt_with_tools_and_system() {
        let provider = CodexCliProvider::new(CodexCliConfig::default());
        let messages = vec![
            Message {
                role: "system".to_string(),
                content: "Be helpful".to_string(),
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
        let tools = vec![ToolDefinition {
            tool_type: "function".to_string(),
            function: ToolFunctionDefinition {
                name: "calc".to_string(),
                description: "Calculate".to_string(),
                parameters: serde_json::json!({"type": "object"}),
            },
        }];
        let prompt = provider.build_prompt(&messages, &tools);
        assert!(prompt.contains("System Instructions"));
        assert!(prompt.contains("Available Tools"));
        assert!(prompt.contains("calc"));
        assert!(prompt.contains("Hello"));
    }

    #[test]
    fn test_build_prompt_with_assistant_message() {
        let provider = CodexCliProvider::new(CodexCliConfig::default());
        let messages = vec![
            Message {
                role: "user".to_string(),
                content: "Hi".to_string(),
                tool_calls: vec![],
                tool_call_id: None,
                timestamp: None,
            },
            Message {
                role: "assistant".to_string(),
                content: "Hello!".to_string(),
                tool_calls: vec![],
                tool_call_id: None,
                timestamp: None,
            },
        ];
        let prompt = provider.build_prompt(&messages, &[]);
        assert!(prompt.contains("Assistant: Hello!"));
        assert!(prompt.contains("Hi"));
    }

    #[test]
    fn test_build_prompt_with_tool_result() {
        let provider = CodexCliProvider::new(CodexCliConfig::default());
        let messages = vec![
            Message {
                role: "user".to_string(),
                content: "Read file".to_string(),
                tool_calls: vec![],
                tool_call_id: None,
                timestamp: None,
            },
            Message {
                role: "tool".to_string(),
                content: "file content here".to_string(),
                tool_calls: vec![],
                tool_call_id: Some("call_123".into()),
                timestamp: None,
            },
        ];
        let prompt = provider.build_prompt(&messages, &[]);
        assert!(prompt.contains("[Tool Result for call_123]: file content here"));
    }

    #[test]
    fn test_build_tools_prompt_non_function_type() {
        let provider = CodexCliProvider::new(CodexCliConfig::default());
        let tools = vec![ToolDefinition {
            tool_type: "non_function".to_string(),
            function: ToolFunctionDefinition {
                name: "ignored".to_string(),
                description: "Should be ignored".to_string(),
                parameters: serde_json::json!({"type": "object"}),
            },
        }];
        let prompt = provider.build_tools_prompt(&tools);
        // Non-function tools should be skipped
        assert!(prompt.contains("Available Tools"));
        assert!(!prompt.contains("ignored"));
    }

    #[test]
    fn test_build_tools_prompt_no_params() {
        let provider = CodexCliProvider::new(CodexCliConfig::default());
        let tools = vec![ToolDefinition {
            tool_type: "function".to_string(),
            function: ToolFunctionDefinition {
                name: "simple".to_string(),
                description: "Simple tool".to_string(),
                parameters: serde_json::Value::Null,
            },
        }];
        let prompt = provider.build_tools_prompt(&tools);
        assert!(prompt.contains("simple"));
        assert!(prompt.contains("Simple tool"));
        // Should not contain Parameters section when params is null
        assert!(!prompt.contains("Parameters:\n"));
    }

    #[test]
    fn test_parse_jsonl_turn_completed_no_usage() {
        let provider = CodexCliProvider::new(CodexCliConfig::default());
        let output = r#"{"type":"item.completed","item":{"id":"i1","type":"agent_message","text":"Hello!"}}
{"type":"turn.completed"}"#;
        let resp = provider.parse_jsonl_events(output).unwrap();
        assert_eq!(resp.content, "Hello!");
        assert!(resp.usage.is_none());
    }

    #[test]
    fn test_parse_jsonl_turn_failed_no_error() {
        let provider = CodexCliProvider::new(CodexCliConfig::default());
        let output = r#"{"type":"turn.failed"}"#;
        // turn.failed without error field, no content - returns empty response (not error)
        let resp = provider.parse_jsonl_events(output).unwrap();
        assert_eq!(resp.content, "");
    }

    #[test]
    fn test_parse_jsonl_non_agent_message_item() {
        let provider = CodexCliProvider::new(CodexCliConfig::default());
        let output = r#"{"type":"item.completed","item":{"id":"i1","type":"tool_call","text":"ignored"}}
{"type":"turn.completed","usage":{"input_tokens":5,"cached_input_tokens":0,"output_tokens":2}}"#;
        let resp = provider.parse_jsonl_events(output).unwrap();
        // Non agent_message items are skipped
        assert_eq!(resp.content, "");
    }

    #[test]
    fn test_parse_jsonl_item_completed_empty_text() {
        let provider = CodexCliProvider::new(CodexCliConfig::default());
        let output = r#"{"type":"item.completed","item":{"id":"i1","type":"agent_message","text":""}}"#;
        let resp = provider.parse_jsonl_events(output).unwrap();
        // Empty text is skipped
        assert_eq!(resp.content, "");
    }

    #[test]
    fn test_parse_jsonl_cached_tokens_in_usage() {
        let provider = CodexCliProvider::new(CodexCliConfig::default());
        let output = r#"{"type":"turn.completed","usage":{"input_tokens":10,"cached_input_tokens":5,"output_tokens":3}}"#;
        let resp = provider.parse_jsonl_events(output).unwrap();
        let usage = resp.usage.unwrap();
        assert_eq!(usage.prompt_tokens, 15); // 10 + 5 cached
        assert_eq!(usage.completion_tokens, 3);
        assert_eq!(usage.total_tokens, 18);
    }

    #[test]
    fn test_provider_name_and_default_model() {
        let provider = CodexCliProvider::new(CodexCliConfig::default());
        assert_eq!(provider.name(), "codex-cli");
        assert_eq!(provider.default_model(), "codex-cli");
    }
}
