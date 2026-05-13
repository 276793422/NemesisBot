//! Request logger: logs LLM requests and responses to markdown files.
//!
//! `RequestLogger` writes structured markdown log files for each conversation,
//! including the user request, LLM requests/responses, local operations,
//! and the final response. Each conversation gets its own timestamped directory.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::warn;

/// Detail level for log output.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum DetailLevel {
    Full,
    Truncated,
}

impl Default for DetailLevel {
    fn default() -> Self {
        Self::Full
    }
}

/// Configuration for the request logger.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    /// Whether logging is enabled.
    pub enabled: bool,
    /// Detail level: "full" or "truncated".
    pub detail_level: DetailLevel,
    /// Log directory (relative to workspace or absolute).
    pub log_dir: String,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            detail_level: DetailLevel::Full,
            log_dir: "logs/llm".to_string(),
        }
    }
}

/// Truncation limit for response content in truncated mode.
const TRUNCATE_RESPONSE_LIMIT: usize = 500;

/// Truncation limits for truncated mode (message content and tool arguments).
#[allow(dead_code)]
const TRUNCATE_MESSAGE_LIMIT: usize = 200;
#[allow(dead_code)]
const TRUNCATE_ARGS_LIMIT: usize = 200;

/// Information about the user's request.
#[derive(Debug, Clone)]
pub struct UserRequestInfo {
    pub timestamp: DateTime<Utc>,
    pub channel: String,
    pub sender_id: String,
    pub chat_id: String,
    pub content: String,
}

/// Provider metadata for logging.
#[derive(Debug, Clone)]
pub struct ProviderMetadata {
    pub name: String,
    pub api_key: String,
    pub api_base: String,
}

/// Information about a single tool call in an LLM response.
#[derive(Debug, Clone)]
pub struct ToolCallDetail {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// Information about an LLM request.
#[derive(Debug, Clone)]
pub struct LLMRequestInfo {
    pub round: usize,
    pub timestamp: DateTime<Utc>,
    pub model: String,
    pub provider_name: String,
    pub api_key: String,
    pub api_base: String,
    pub messages_count: usize,
    pub tools_count: usize,
    /// Full message list for detailed logging (mirrors Go's Messages field).
    pub messages: Vec<crate::r#loop::LlmMessage>,
    /// HTTP headers for detailed logging (mirrors Go's HTTPHeaders field).
    pub http_headers: Vec<(String, String)>,
    /// Full config map for detailed logging (mirrors Go's FullConfig field).
    pub config: std::collections::HashMap<String, String>,
    /// Fallback attempt details (mirrors Go's FallbackAttempts field).
    pub fallback_attempts: Vec<FallbackAttemptInfo>,
}

impl Default for LLMRequestInfo {
    fn default() -> Self {
        Self {
            round: 0,
            timestamp: Utc::now(),
            model: String::new(),
            provider_name: String::new(),
            api_key: String::new(),
            api_base: String::new(),
            messages_count: 0,
            tools_count: 0,
            messages: Vec::new(),
            http_headers: Vec::new(),
            config: std::collections::HashMap::new(),
            fallback_attempts: Vec::new(),
        }
    }
}

/// Information about a fallback attempt.
#[derive(Debug, Clone)]
pub struct FallbackAttemptInfo {
    pub provider: String,
    pub model: String,
    pub api_key: String,
    pub api_base: String,
    pub error: String,
    pub duration_ms: u64,
}

/// Token usage information from an LLM response.
#[derive(Debug, Clone, Default)]
pub struct UsageInfo {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// Information about an LLM response.
#[derive(Debug, Clone)]
pub struct LLMResponseInfo {
    pub round: usize,
    pub timestamp: DateTime<Utc>,
    pub duration_ms: u64,
    pub content: String,
    pub tool_calls_count: usize,
    pub finish_reason: String,
    /// Detailed tool calls from the response (mirrors Go's ToolCalls field).
    pub tool_calls: Vec<ToolCallDetail>,
    /// Token usage information (mirrors Go's Usage field).
    pub usage: UsageInfo,
}

/// Information about a local operation.
#[derive(Debug, Clone)]
pub struct OperationInfo {
    pub op_type: String,
    pub name: String,
    pub status: String,
    pub error: String,
    pub duration_ms: u64,
    /// Tool arguments (JSON, mirrors Go's Arguments field).
    pub arguments: String,
    /// Tool result (JSON, mirrors Go's Result field).
    pub result: String,
}

impl Default for OperationInfo {
    fn default() -> Self {
        Self {
            op_type: String::new(),
            name: String::new(),
            status: String::new(),
            error: String::new(),
            duration_ms: 0,
            arguments: String::new(),
            result: String::new(),
        }
    }
}

/// Information about local operations for a round.
#[derive(Debug, Clone)]
pub struct LocalOperationInfo {
    pub round: usize,
    pub timestamp: DateTime<Utc>,
    pub operations: Vec<OperationInfo>,
}

/// Information about the final response.
#[derive(Debug, Clone)]
pub struct FinalResponseInfo {
    pub timestamp: DateTime<Utc>,
    pub total_duration_ms: u64,
    pub llm_rounds: usize,
    pub content: String,
    pub channel: String,
    pub chat_id: String,
}

/// Handles logging of LLM requests and responses to markdown files.
pub struct RequestLogger {
    config: LoggingConfig,
    base_dir: PathBuf,
    session_dir: Mutex<Option<PathBuf>>,
    file_index: AtomicI32,
    enabled: bool,
    #[allow(dead_code)]
    start_time: DateTime<Utc>,
}

impl RequestLogger {
    /// Create a new request logger with the given configuration and workspace path.
    pub fn new(config: LoggingConfig, workspace: &Path) -> Self {
        if !config.enabled {
            return Self {
                config,
                base_dir: PathBuf::new(),
                session_dir: Mutex::new(None),
                file_index: AtomicI32::new(0),
                enabled: false,
                start_time: Utc::now(),
            };
        }

        let base_dir = resolve_log_path(&config.log_dir, workspace);

        Self {
            config,
            base_dir,
            session_dir: Mutex::new(None),
            file_index: AtomicI32::new(0),
            enabled: true,
            start_time: Utc::now(),
        }
    }

    /// Returns whether the logger is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Create a new logging session directory.
    pub fn create_session(&self) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }

        // Create base directory
        if let Err(e) = fs::create_dir_all(&self.base_dir) {
            warn!("Failed to create log directory: {}", e);
            return Ok(()); // Silent failure
        }

        // Create timestamped session directory
        let timestamp = Utc::now().format("%Y-%m-%d_%H-%M-%S").to_string();
        let suffix = format!("_{:03x}", rand_u16());
        let session_dir = self.base_dir.join(format!("{}{}", timestamp, suffix));

        if let Err(e) = fs::create_dir_all(&session_dir) {
            warn!("Failed to create session directory: {}", e);
            return Ok(()); // Silent failure
        }

        *self.session_dir.lock().unwrap() = Some(session_dir);
        Ok(())
    }

    /// Get the next file index and return it as a zero-padded string.
    fn next_index(&self) -> String {
        let idx = self.file_index.fetch_add(1, Ordering::Relaxed);
        format!("{:02}", idx)
    }

    /// Log the initial user request.
    pub fn log_user_request(&self, info: &UserRequestInfo) {
        if !self.enabled {
            return;
        }

        let index = self.next_index();
        let filename = format!("{}.request.md", index);
        let content = format!(
            "# User Request\n\n\
             **Timestamp**: {}\n\
             **Channel**: {}\n\
             **Sender ID**: {}\n\
             **Chat ID**: {}\n\n\
             ## Message\n\n{}\n",
            info.timestamp.to_rfc3339(),
            info.channel,
            info.sender_id,
            info.chat_id,
            info.content,
        );

        let _ = self.write_file(&filename, &content);
    }

    /// Log an LLM request.
    pub fn log_llm_request(&self, info: &LLMRequestInfo) {
        if !self.enabled {
            return;
        }

        let index = self.next_index();
        let filename = format!("{}.AI.Request.md", index);

        let mut content = format!(
            "# LLM Request\n\n\
             **Timestamp**: {}\n\
             **Round**: {}\n\n\
             ## Provider\n\n",
            info.timestamp.to_rfc3339(),
            info.round,
        );

        if !info.provider_name.is_empty() {
            content.push_str(&format!("- **Provider**: {}\n", info.provider_name));
        }
        content.push_str(&format!("- **Model**: {}\n", info.model));
        if !info.api_base.is_empty() {
            content.push_str(&format!("- **API Base**: {}\n", info.api_base));
        }
        if !info.api_key.is_empty() {
            content.push_str(&format!("- **API Key**: {}\n", mask_api_key(&info.api_key)));
        }

        // HTTP Headers section (mirrors Go's formatHeaders).
        if !info.http_headers.is_empty() {
            content.push_str("\n## HTTP Headers\n\n```\n");
            for (key, value) in &info.http_headers {
                let masked_val = if key.to_lowercase().contains("auth") || key.to_lowercase().contains("key") {
                    mask_api_key(value)
                } else {
                    value.clone()
                };
                content.push_str(&format!("{}: {}\n", key, masked_val));
            }
            content.push_str("```\n");
        }

        // Full Config section (mirrors Go's FullConfig).
        if !info.config.is_empty() {
            content.push_str("\n## Full Config\n\n");
            for (key, value) in &info.config {
                content.push_str(&format!("- **{}**: {}\n", key, value));
            }
        }

        // Fallback Attempts section (mirrors Go's FallbackAttempts).
        if !info.fallback_attempts.is_empty() {
            content.push_str(&format!("\n## Fallback Attempts ({} total)\n\n", info.fallback_attempts.len()));
            for (i, attempt) in info.fallback_attempts.iter().enumerate() {
                content.push_str(&format!(
                    "### Attempt {}\n\n\
                     - **Provider**: {}\n\
                     - **Model**: {}\n\
                     - **API Base**: {}\n\
                     - **API Key**: {}\n\
                     - **Error**: {}\n\
                     - **Duration**: {:.1}s\n\n",
                    i + 1,
                    attempt.provider,
                    attempt.model,
                    attempt.api_base,
                    mask_api_key(&attempt.api_key),
                    attempt.error,
                    attempt.duration_ms as f64 / 1000.0,
                ));
            }
        }

        // Messages section with full content (mirrors Go's formatMessagesForLog).
        content.push_str(&format!("\n## Messages\n\n{} messages included\n", info.messages_count));
        if !info.messages.is_empty() {
            content.push_str("\n### Message Details\n\n");
            for (i, msg) in info.messages.iter().enumerate() {
                let msg_preview = if self.config.detail_level == DetailLevel::Truncated && msg.content.len() > TRUNCATE_MESSAGE_LIMIT {
                    format!("{}...", &msg.content[..TRUNCATE_MESSAGE_LIMIT])
                } else {
                    msg.content.clone()
                };
                content.push_str(&format!("**[{}] {}**: {}\n", i, msg.role, msg_preview));
                if let Some(ref tool_calls) = msg.tool_calls {
                    for tc in tool_calls {
                        let args_preview = if tc.arguments.len() > TRUNCATE_ARGS_LIMIT {
                            format!("{}...", &tc.arguments[..TRUNCATE_ARGS_LIMIT])
                        } else {
                            tc.arguments.clone()
                        };
                        content.push_str(&format!("  - ToolCall: {} ({}) args: {}\n", tc.name, tc.id, args_preview));
                    }
                }
            }
        }

        content.push_str(&format!("\n## Tools\n\n{} tools available\n", info.tools_count));

        let _ = self.write_file(&filename, &content);
    }

    /// Log an LLM response.
    pub fn log_llm_response(&self, info: &LLMResponseInfo) {
        if !self.enabled {
            return;
        }

        let index = self.next_index();
        let filename = format!("{}.AI.Response.md", index);

        let response_content = if self.config.detail_level == DetailLevel::Truncated
            && info.content.len() > TRUNCATE_RESPONSE_LIMIT
        {
            format!(
                "{}\n\n... [truncated]",
                &info.content[..TRUNCATE_RESPONSE_LIMIT]
            )
        } else {
            info.content.clone()
        };

        let mut content = format!(
            "# LLM Response\n\n\
             **Timestamp**: {}\n\
             **Round**: {}\n\
             **Duration**: {:.1}s\n\n\
             ## Response Content\n\n{}\n\n",
            info.timestamp.to_rfc3339(),
            info.round,
            info.duration_ms as f64 / 1000.0,
            response_content,
        );

        // Tool Calls section with full details (mirrors Go's formatArguments).
        content.push_str(&format!("## Tool Calls\n\n{} tool call(s)\n", info.tool_calls_count));
        if !info.tool_calls.is_empty() {
            content.push_str("\n### Tool Call Details\n\n");
            for tc in &info.tool_calls {
                let args_preview = if self.config.detail_level == DetailLevel::Truncated && tc.arguments.len() > TRUNCATE_ARGS_LIMIT {
                    format!("{}...", &tc.arguments[..TRUNCATE_ARGS_LIMIT])
                } else {
                    tc.arguments.clone()
                };
                content.push_str(&format!(
                    "- **ID**: {}\n  **Name**: {}\n  **Arguments**: {}\n\n",
                    tc.id, tc.name, args_preview
                ));
            }
        }

        // Usage section (mirrors Go's Usage field).
        if info.usage.total_tokens > 0 {
            content.push_str(&format!(
                "\n## Token Usage\n\n\
                 - **Prompt Tokens**: {}\n\
                 - **Completion Tokens**: {}\n\
                 - **Total Tokens**: {}\n",
                info.usage.prompt_tokens,
                info.usage.completion_tokens,
                info.usage.total_tokens,
            ));
        }

        content.push_str(&format!("\n## Finish Reason\n\n{}\n", info.finish_reason));

        let _ = self.write_file(&filename, &content);
    }

    /// Log local operations for a round.
    pub fn log_local_operations(&self, info: &LocalOperationInfo) {
        if !self.enabled || info.operations.is_empty() {
            return;
        }

        let index = self.next_index();
        let filename = format!("{}.Local.md", index);

        let mut content = format!(
            "# Local Operations\n\n\
             **Timestamp**: {}\n\
             **Round**: {}\n\
             **Operations Count**: {}\n\n",
            info.timestamp.to_rfc3339(),
            info.round,
            info.operations.len(),
        );

        for (i, op) in info.operations.iter().enumerate() {
            content.push_str(&format!(
                "## Operation {}: {}\n\n\
                 **Name**: {}\n\
                 **Status**: {}\n\n",
                i + 1,
                format_operation_type(&op.op_type),
                op.name,
                op.status,
            ));

            // Arguments section (mirrors Go's formatArguments).
            if !op.arguments.is_empty() {
                let args_preview = if self.config.detail_level == DetailLevel::Truncated && op.arguments.len() > TRUNCATE_ARGS_LIMIT {
                    format!("{}...", &op.arguments[..TRUNCATE_ARGS_LIMIT])
                } else {
                    op.arguments.clone()
                };
                content.push_str(&format!("### Arguments\n```json\n{}\n```\n\n", args_preview));
            }

            // Result section (mirrors Go's Result field).
            if !op.result.is_empty() {
                let result_preview = if self.config.detail_level == DetailLevel::Truncated && op.result.len() > TRUNCATE_RESPONSE_LIMIT {
                    format!("{}...", &op.result[..TRUNCATE_RESPONSE_LIMIT])
                } else {
                    op.result.clone()
                };
                content.push_str(&format!("### Result\n```\n{}\n```\n\n", result_preview));
            }

            if !op.error.is_empty() {
                content.push_str(&format!("### Error\n{}\n\n", op.error));
            }

            if op.duration_ms > 0 {
                content.push_str(&format!("### Duration\n{:.3}s\n\n", op.duration_ms as f64 / 1000.0));
            }

            content.push_str("---\n\n");
        }

        let _ = self.write_file(&filename, &content);
    }

    /// Log the final response to the user.
    pub fn log_final_response(&self, info: &FinalResponseInfo) {
        if !self.enabled {
            return;
        }

        let index = self.next_index();
        let filename = format!("{}.response.md", index);

        let content = format!(
            "# Agent Response\n\n\
             **Timestamp**: {}\n\
             **Total Duration**: {:.1}s\n\
             **LLM Rounds**: {}\n\n\
             ## Response Content\n\n{}\n\n\
             ---\n\n\
             **Channel**: {}\n\
             **Chat ID**: {}\n\
             **Sent At**: {}\n",
            info.timestamp.to_rfc3339(),
            info.total_duration_ms as f64 / 1000.0,
            info.llm_rounds,
            info.content,
            info.channel,
            info.chat_id,
            info.timestamp.to_rfc3339(),
        );

        let _ = self.write_file(&filename, &content);
    }

    /// Write content to a file in the session directory.
    fn write_file(&self, filename: &str, content: &str) -> Result<(), String> {
        let session_dir = self.session_dir.lock().unwrap();
        if let Some(ref dir) = *session_dir {
            let path = dir.join(filename);
            let mut file =
                fs::File::create(&path).map_err(|e| format!("Failed to create file: {}", e))?;
            file.write_all(content.as_bytes())
                .map_err(|e| format!("Failed to write file: {}", e))?;
        }
        Ok(())
    }

    /// Returns the session directory path, if a session has been created.
    pub fn session_dir(&self) -> Option<PathBuf> {
        self.session_dir.lock().unwrap().clone()
    }
}

/// Resolve the log directory path.
///
/// - If log_dir is absolute, use it as-is.
/// - If log_dir is relative, join it with workspace.
fn resolve_log_path(log_dir: &str, workspace: &Path) -> PathBuf {
    let path = Path::new(log_dir);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace.join(log_dir)
    }
}

/// Mask an API key for logging (shows first 3 and last 3 characters).
fn mask_api_key(key: &str) -> String {
    let key = key.trim();
    if key.is_empty() {
        return "<empty>".to_string();
    }
    if key.len() <= 6 {
        return "***".to_string();
    }
    format!("{}***{}", &key[..3], &key[key.len() - 3..])
}

/// Format an operation type for display.
fn format_operation_type(op_type: &str) -> String {
    match op_type {
        "tool_call" => "Tool Execution".to_string(),
        "file_write" => "File Write".to_string(),
        "file_read" => "File Read".to_string(),
        "command_exec" => "Command Execution".to_string(),
        _ => op_type.replace('_', " "),
    }
}

/// Simple pseudo-random u16 for session directory suffix.
fn rand_u16() -> u16 {
    use std::time::SystemTime;
    let duration = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    (duration.subsec_nanos() & 0xFFFF) as u16
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_config() -> LoggingConfig {
        LoggingConfig {
            enabled: true,
            detail_level: DetailLevel::Full,
            log_dir: "logs/llm".to_string(),
        }
    }

    #[test]
    fn disabled_logger_is_noop() {
        let config = LoggingConfig {
            enabled: false,
            detail_level: DetailLevel::Full,
            log_dir: String::new(),
        };
        let tmp = TempDir::new().unwrap();
        let logger = RequestLogger::new(config, tmp.path());

        assert!(!logger.is_enabled());
        logger.log_user_request(&UserRequestInfo {
            timestamp: Utc::now(),
            channel: "web".to_string(),
            sender_id: "user1".to_string(),
            chat_id: "chat1".to_string(),
            content: "Hello".to_string(),
        });
        // No crash, no files created.
    }

    #[test]
    fn create_session_and_log_user_request() {
        let tmp = TempDir::new().unwrap();
        let logger = RequestLogger::new(test_config(), tmp.path());
        assert!(logger.is_enabled());

        logger.create_session().unwrap();
        let session_dir = logger.session_dir().unwrap();
        assert!(session_dir.exists());

        logger.log_user_request(&UserRequestInfo {
            timestamp: Utc::now(),
            channel: "web".to_string(),
            sender_id: "user1".to_string(),
            chat_id: "chat1".to_string(),
            content: "Test message".to_string(),
        });

        // Find the request file
        let entries: Vec<_> = fs::read_dir(&session_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].file_name().to_string_lossy().ends_with(".request.md"));

        let content = fs::read_to_string(entries[0].path()).unwrap();
        assert!(content.contains("# User Request"));
        assert!(content.contains("Test message"));
        assert!(content.contains("web"));
    }

    #[test]
    fn log_llm_request_and_response() {
        let tmp = TempDir::new().unwrap();
        let logger = RequestLogger::new(test_config(), tmp.path());
        logger.create_session().unwrap();

        logger.log_llm_request(&LLMRequestInfo {
            round: 1,
            timestamp: Utc::now(),
            model: "gpt-4".to_string(),
            provider_name: "openai".to_string(),
            api_key: "sk-1234567890abcdef".to_string(),
            api_base: "https://api.openai.com".to_string(),
            messages_count: 5,
            tools_count: 3,
            messages: Vec::new(),
            http_headers: Vec::new(),
            config: std::collections::HashMap::new(),
            fallback_attempts: Vec::new(),
        });

        logger.log_llm_response(&LLMResponseInfo {
            round: 1,
            timestamp: Utc::now(),
            duration_ms: 1500,
            content: "The answer is 42.".to_string(),
            tool_calls_count: 0,
            finish_reason: "stop".to_string(),
            tool_calls: Vec::new(),
            usage: UsageInfo::default(),
        });

        let session_dir = logger.session_dir().unwrap();
        let entries: Vec<_> = fs::read_dir(&session_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(entries.len(), 2);

        // Check request file
        let req_content = fs::read_to_string(
            entries.iter().find(|e| e.file_name().to_string_lossy().contains("Request")).unwrap().path(),
        )
        .unwrap();
        assert!(req_content.contains("gpt-4"));
        assert!(req_content.contains("sk-***def")); // Masked API key

        // Check response file
        let resp_content = fs::read_to_string(
            entries.iter().find(|e| e.file_name().to_string_lossy().contains("Response")).unwrap().path(),
        )
        .unwrap();
        assert!(resp_content.contains("The answer is 42."));
        assert!(resp_content.contains("stop"));
    }

    #[test]
    fn log_local_operations() {
        let tmp = TempDir::new().unwrap();
        let logger = RequestLogger::new(test_config(), tmp.path());
        logger.create_session().unwrap();

        logger.log_local_operations(&LocalOperationInfo {
            round: 1,
            timestamp: Utc::now(),
            operations: vec![
                OperationInfo {
                    op_type: "tool_call".to_string(),
                    name: "calculator".to_string(),
                    status: "Success".to_string(),
                    error: String::new(),
                    duration_ms: 50,
                    arguments: r#"{"expr":"2+2"}"#.to_string(),
                    result: "4".to_string(),
                },
                OperationInfo {
                    op_type: "file_read".to_string(),
                    name: "read_config".to_string(),
                    status: "Failed".to_string(),
                    error: "file not found".to_string(),
                    duration_ms: 10,
                    arguments: String::new(),
                    result: String::new(),
                },
            ],
        });

        let session_dir = logger.session_dir().unwrap();
        let entries: Vec<_> = fs::read_dir(&session_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(entries.len(), 1);

        let content = fs::read_to_string(entries[0].path()).unwrap();
        assert!(content.contains("Tool Execution"));
        assert!(content.contains("calculator"));
        assert!(content.contains("Failed"));
        assert!(content.contains("file not found"));
    }

    #[test]
    fn log_final_response() {
        let tmp = TempDir::new().unwrap();
        let logger = RequestLogger::new(test_config(), tmp.path());
        logger.create_session().unwrap();

        logger.log_final_response(&FinalResponseInfo {
            timestamp: Utc::now(),
            total_duration_ms: 3500,
            llm_rounds: 3,
            content: "Final answer here.".to_string(),
            channel: "web".to_string(),
            chat_id: "chat1".to_string(),
        });

        let session_dir = logger.session_dir().unwrap();
        let entries: Vec<_> = fs::read_dir(&session_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(entries.len(), 1);

        let content = fs::read_to_string(entries[0].path()).unwrap();
        assert!(content.contains("Final answer here."));
        assert!(content.contains("3.5s"));
        assert!(content.contains("LLM Rounds**: 3"));
    }

    #[test]
    fn log_local_operations_skips_empty() {
        let tmp = TempDir::new().unwrap();
        let logger = RequestLogger::new(test_config(), tmp.path());
        logger.create_session().unwrap();

        logger.log_local_operations(&LocalOperationInfo {
            round: 1,
            timestamp: Utc::now(),
            operations: vec![],
        });

        let session_dir = logger.session_dir().unwrap();
        let entries: Vec<_> = fs::read_dir(&session_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(entries.len(), 0);
    }

    #[test]
    fn mask_api_key_tests() {
        assert_eq!(mask_api_key(""), "<empty>");
        assert_eq!(mask_api_key("short"), "***");
        assert_eq!(mask_api_key("sk-1234567890abcdef"), "sk-***def");
        assert_eq!(mask_api_key("  sk-1234567890abcdef  "), "sk-***def");
    }

    #[test]
    fn format_operation_type_tests() {
        assert_eq!(format_operation_type("tool_call"), "Tool Execution");
        assert_eq!(format_operation_type("file_write"), "File Write");
        assert_eq!(format_operation_type("file_read"), "File Read");
        assert_eq!(format_operation_type("command_exec"), "Command Execution");
        assert_eq!(format_operation_type("custom_op"), "custom op");
    }

    #[test]
    fn truncated_mode_truncates_long_content() {
        let config = LoggingConfig {
            enabled: true,
            detail_level: DetailLevel::Truncated,
            log_dir: "logs/llm".to_string(),
        };
        let tmp = TempDir::new().unwrap();
        let logger = RequestLogger::new(config, tmp.path());
        logger.create_session().unwrap();

        let long_content = "x".repeat(1000);
        logger.log_llm_response(&LLMResponseInfo {
            round: 1,
            timestamp: Utc::now(),
            duration_ms: 100,
            content: long_content,
            tool_calls_count: 0,
            finish_reason: "stop".to_string(),
            tool_calls: Vec::new(),
            usage: UsageInfo::default(),
        });

        let session_dir = logger.session_dir().unwrap();
        let entries: Vec<_> = fs::read_dir(&session_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(entries.len(), 1);

        let content = fs::read_to_string(entries[0].path()).unwrap();
        assert!(content.contains("truncated"));
    }

    #[test]
    fn resolve_log_path_absolute() {
        let workspace = Path::new("/workspace");
        let result = resolve_log_path("/var/log", workspace);
        assert_eq!(result, PathBuf::from("/var/log"));
    }

    #[test]
    fn resolve_log_path_relative() {
        let workspace = Path::new("/workspace");
        let result = resolve_log_path("logs/llm", workspace);
        assert_eq!(result, PathBuf::from("/workspace/logs/llm"));
    }

    #[test]
    fn log_llm_request_with_tool_calls() {
        let tmp = TempDir::new().unwrap();
        let logger = RequestLogger::new(test_config(), tmp.path());
        logger.create_session().unwrap();

        logger.log_llm_response(&LLMResponseInfo {
            round: 1,
            timestamp: Utc::now(),
            duration_ms: 500,
            content: "Using tools".to_string(),
            tool_calls_count: 2,
            finish_reason: "tool_calls".to_string(),
            tool_calls: vec![
                ToolCallDetail {
                    id: "tc-1".to_string(),
                    name: "file_read".to_string(),
                    arguments: r#"{"path": "/etc/config"}"#.to_string(),
                },
                ToolCallDetail {
                    id: "tc-2".to_string(),
                    name: "calculator".to_string(),
                    arguments: r#"{"expr": "2+2"}"#.to_string(),
                },
            ],
            usage: UsageInfo {
                prompt_tokens: 100,
                completion_tokens: 50,
                total_tokens: 150,
            },
        });

        let session_dir = logger.session_dir().unwrap();
        let entries: Vec<_> = fs::read_dir(&session_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(entries.len(), 1);

        let content = fs::read_to_string(entries[0].path()).unwrap();
        assert!(content.contains("file_read"));
        assert!(content.contains("calculator"));
        assert!(content.contains("tc-1"));
        assert!(content.contains("150"));
    }

    #[test]
    fn log_llm_request_with_fallback_attempts() {
        let tmp = TempDir::new().unwrap();
        let logger = RequestLogger::new(test_config(), tmp.path());
        logger.create_session().unwrap();

        logger.log_llm_request(&LLMRequestInfo {
            round: 1,
            timestamp: Utc::now(),
            model: "gpt-4".to_string(),
            provider_name: "openai".to_string(),
            api_key: "sk-test123456789".to_string(),
            api_base: "https://api.openai.com".to_string(),
            messages_count: 3,
            tools_count: 5,
            messages: Vec::new(),
            http_headers: vec![
                ("Content-Type".to_string(), "application/json".to_string()),
                ("Authorization".to_string(), "Bearer sk-test123456789".to_string()),
            ],
            config: {
                let mut m = std::collections::HashMap::new();
                m.insert("temperature".to_string(), "0.7".to_string());
                m
            },
            fallback_attempts: vec![
                FallbackAttemptInfo {
                    provider: "openai".to_string(),
                    model: "gpt-4".to_string(),
                    api_key: "sk-test123456789".to_string(),
                    api_base: "https://api.openai.com".to_string(),
                    error: "rate limited".to_string(),
                    duration_ms: 5000,
                },
            ],
        });

        let session_dir = logger.session_dir().unwrap();
        let entries: Vec<_> = fs::read_dir(&session_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(entries.len(), 1);

        let content = fs::read_to_string(entries[0].path()).unwrap();
        assert!(content.contains("Fallback Attempts"));
        assert!(content.contains("rate limited"));
        assert!(content.contains("application/json"));
        assert!(content.contains("temperature"));
    }

    #[test]
    fn disabled_logger_all_methods_noop() {
        let config = LoggingConfig {
            enabled: false,
            detail_level: DetailLevel::Full,
            log_dir: String::new(),
        };
        let tmp = TempDir::new().unwrap();
        let logger = RequestLogger::new(config, tmp.path());

        assert!(!logger.is_enabled());

        // None of these should panic
        logger.create_session().unwrap();
        logger.log_user_request(&UserRequestInfo {
            timestamp: Utc::now(),
            channel: "web".to_string(),
            sender_id: "u".to_string(),
            chat_id: "c".to_string(),
            content: "test".to_string(),
        });
        logger.log_llm_request(&LLMRequestInfo::default());
        logger.log_llm_response(&LLMResponseInfo {
            round: 1,
            timestamp: Utc::now(),
            duration_ms: 100,
            content: "test".to_string(),
            tool_calls_count: 0,
            finish_reason: "stop".to_string(),
            tool_calls: Vec::new(),
            usage: UsageInfo::default(),
        });
        logger.log_local_operations(&LocalOperationInfo {
            round: 1,
            timestamp: Utc::now(),
            operations: vec![OperationInfo::default()],
        });
        logger.log_final_response(&FinalResponseInfo {
            timestamp: Utc::now(),
            total_duration_ms: 100,
            llm_rounds: 1,
            content: "test".to_string(),
            channel: "web".to_string(),
            chat_id: "c".to_string(),
        });

        assert!(logger.session_dir().is_none());
    }

    #[test]
    fn multiple_sessions() {
        let tmp = TempDir::new().unwrap();
        let logger = RequestLogger::new(test_config(), tmp.path());

        // First session
        logger.create_session().unwrap();
        logger.log_user_request(&UserRequestInfo {
            timestamp: Utc::now(),
            channel: "web".to_string(),
            sender_id: "u".to_string(),
            chat_id: "c".to_string(),
            content: "first".to_string(),
        });

        // Second session (overwrites first)
        logger.create_session().unwrap();
        logger.log_user_request(&UserRequestInfo {
            timestamp: Utc::now(),
            channel: "web".to_string(),
            sender_id: "u".to_string(),
            chat_id: "c".to_string(),
            content: "second".to_string(),
        });

        let session_dir = logger.session_dir().unwrap();
        let entries: Vec<_> = fs::read_dir(&session_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(entries.len(), 1);
        let content = fs::read_to_string(entries[0].path()).unwrap();
        assert!(content.contains("second"));
    }

    #[test]
    fn log_with_special_characters() {
        let tmp = TempDir::new().unwrap();
        let logger = RequestLogger::new(test_config(), tmp.path());
        logger.create_session().unwrap();

        logger.log_user_request(&UserRequestInfo {
            timestamp: Utc::now(),
            channel: "web".to_string(),
            sender_id: "u".to_string(),
            chat_id: "c".to_string(),
            content: "Hello <script>alert('xss')</script> & 'quotes' \"double\"".to_string(),
        });

        let session_dir = logger.session_dir().unwrap();
        let entries: Vec<_> = fs::read_dir(&session_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(entries.len(), 1);
        let content = fs::read_to_string(entries[0].path()).unwrap();
        assert!(content.contains("script"));
    }
}
