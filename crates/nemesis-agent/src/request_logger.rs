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

use chrono::{DateTime, Local};
use nemesis_types::utils;
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
    /// Save raw HTTP request/response JSON instead of markdown summaries.
    pub save_raw: bool,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            detail_level: DetailLevel::Full,
            log_dir: "logs/llm".to_string(),
            save_raw: false,
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
    pub timestamp: DateTime<Local>,
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
    pub timestamp: DateTime<Local>,
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
            timestamp: Local::now(),
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
    /// Cached prompt tokens (DeepSeek: prompt_cache_hit_tokens, OpenAI: cached_tokens).
    pub cached_tokens: u32,
}

/// Information about an LLM response.
#[derive(Debug, Clone)]
pub struct LLMResponseInfo {
    pub round: usize,
    pub timestamp: DateTime<Local>,
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
    pub timestamp: DateTime<Local>,
    pub operations: Vec<OperationInfo>,
}

/// Information about the final response.
#[derive(Debug, Clone)]
pub struct FinalResponseInfo {
    pub timestamp: DateTime<Local>,
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
    /// Optional override for the session directory name. When None, the
    /// default `{timestamp}_{rand_hex}` scheme is used. When Some(name),
    /// `create_session()` uses `name` directly (after sanitization).
    session_name_override: Option<String>,
    session_dir: Mutex<Option<PathBuf>>,
    file_index: AtomicI32,
    enabled: bool,
    #[allow(dead_code)]
    start_time: DateTime<Local>,
}

impl RequestLogger {
    /// Create a new request logger with the given configuration and workspace path.
    pub fn new(config: LoggingConfig, workspace: &Path) -> Self {
        if !config.enabled {
            return Self {
                config,
                base_dir: PathBuf::new(),
                session_name_override: None,
                session_dir: Mutex::new(None),
                file_index: AtomicI32::new(0),
                enabled: false,
                start_time: Local::now(),
            };
        }

        let base_dir = resolve_log_path(&config.log_dir, workspace);

        Self {
            config,
            base_dir,
            session_name_override: None,
            session_dir: Mutex::new(None),
            file_index: AtomicI32::new(0),
            enabled: true,
            start_time: Local::now(),
        }
    }

    /// Create a new request logger with explicit base_dir and optional session name.
    ///
    /// Used by `ClusterRequestLoggerObserver` to write logs under
    /// `cluster_logs/{device_id}/{ts_ms}_{task_id}/` instead of the default
    /// `request_logs/{ts}_{rand}/` location.
    ///
    /// - `base_dir`: absolute or workspace-relative path to the parent directory.
    /// - `session_name`: when `Some(name)`, the session subdirectory uses this
    ///   name (after sanitization). When `None`, falls back to the default
    ///   `{timestamp}_{rand_hex}` scheme.
    pub fn new_with_paths(
        config: LoggingConfig,
        base_dir: PathBuf,
        session_name: Option<String>,
    ) -> Self {
        if !config.enabled {
            return Self {
                config,
                base_dir: PathBuf::new(),
                session_name_override: None,
                session_dir: Mutex::new(None),
                file_index: AtomicI32::new(0),
                enabled: false,
                start_time: Local::now(),
            };
        }

        Self {
            config,
            base_dir,
            session_name_override: session_name,
            session_dir: Mutex::new(None),
            file_index: AtomicI32::new(0),
            enabled: true,
            start_time: Local::now(),
        }
    }

    /// Returns whether the logger is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Sanitize a string for safe use as a filename component.
    ///
    /// Replaces path separators and shell metacharacters with `_`. Used by
    /// cluster logger to handle potentially untrusted task_id values from
    /// remote peers (A side can pass arbitrary strings).
    pub fn sanitize_filename(s: &str) -> String {
        s.chars()
            .map(|c| match c {
                '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '_',
                _ => c,
            })
            .collect()
    }

    /// Create a new logging session directory.
    pub fn create_session(&self) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }

        // Create base directory
        if let Err(e) = fs::create_dir_all(&self.base_dir) {
            warn!("[RequestLogger] Failed to create log directory: {}", e);
            return Ok(()); // Silent failure
        }

        // Determine session directory name. When session_name_override is set
        // (cluster logger case), use it directly after sanitization. Otherwise
        // fall back to the default timestamped scheme.
        let session_dir_name = match self.session_name_override.as_ref() {
            Some(name) => Self::sanitize_filename(name),
            None => {
                let timestamp = Local::now().format("%Y-%m-%d_%H-%M-%S").to_string();
                let suffix = format!("_{:03x}", rand_u16());
                format!("{}{}", timestamp, suffix)
            }
        };
        let session_dir = self.base_dir.join(session_dir_name);

        if let Err(e) = fs::create_dir_all(&session_dir) {
            warn!("[RequestLogger] Failed to create session directory: {}", e);
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

        // Tools count — part of the request metadata.
        content.push_str(&format!("\n## Tools\n\n{} tools available\n", info.tools_count));

        // Messages section — placed as a separate h1 section at the end of the file.
        // This prevents markdown headers inside system prompts from polluting
        // the file's own structure.
        if !info.messages.is_empty() {
            content.push_str(&format!(
                "\n# Messages\n\n{} messages included\n",
                info.messages_count,
            ));
            for (i, msg) in info.messages.iter().enumerate() {
                let msg_preview = if self.config.detail_level == DetailLevel::Truncated
                    && msg.content.len() > TRUNCATE_MESSAGE_LIMIT
                {
                    utils::truncate(&msg.content, TRUNCATE_MESSAGE_LIMIT)
                } else {
                    msg.content.clone()
                };
                content.push_str(&format!("\n## [{}] {}\n\n```text\n{}\n```\n", i, msg.role, msg_preview));
                if let Some(ref tool_calls) = msg.tool_calls {
                    for tc in tool_calls {
                        let args_preview = if tc.arguments.len() > TRUNCATE_ARGS_LIMIT {
                            utils::truncate(&tc.arguments, TRUNCATE_ARGS_LIMIT)
                        } else {
                            tc.arguments.clone()
                        };
                        content.push_str(&format!(
                            "\n> ToolCall: `{}` ({})\n> ```json\n> {}\n> ```\n",
                            tc.name, tc.id, args_preview,
                        ));
                    }
                }
            }
        }

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
            let end = utils::floor_char_boundary(&info.content, TRUNCATE_RESPONSE_LIMIT);
            format!(
                "{}\n\n... [truncated]",
                &info.content[..end]
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
                    utils::truncate(&tc.arguments, TRUNCATE_ARGS_LIMIT)
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
            if info.usage.cached_tokens > 0 {
                let cache_pct = if info.usage.prompt_tokens > 0 {
                    info.usage.cached_tokens * 100 / info.usage.prompt_tokens
                } else {
                    0
                };
                content.push_str(&format!(
                    " - **Cached Tokens**: {} ({ }% cache hit)\n",
                    info.usage.cached_tokens, cache_pct,
                ));
            }
        }

        content.push_str(&format!("\n## Finish Reason\n\n{}\n", info.finish_reason));

        let _ = self.write_file(&filename, &content);
    }

    /// Log raw LLM request in JSON envelope format.
    pub fn log_raw_request(&self, body: &serde_json::Value, timestamp: chrono::DateTime<chrono::Local>, round: usize) {
        if !self.enabled { return; }
        let index = self.next_index();
        let filename = format!("{}.AI.Request.raw.json", index);
        let envelope = serde_json::json!({
            "timestamp": timestamp.to_rfc3339(),
            "round": round,
            "body": body,
        });
        let content = serde_json::to_string_pretty(&envelope)
            .unwrap_or_else(|_| envelope.to_string());
        let _ = self.write_file(&filename, &content);
    }

    /// Log raw LLM request using a pre-built envelope (written immediately at request time).
    pub fn log_raw_request_envelope(&self, envelope: &serde_json::Value) {
        if !self.enabled { return; }
        let index = self.next_index();
        let filename = format!("{}.AI.Request.raw.json", index);
        let content = serde_json::to_string_pretty(envelope)
            .unwrap_or_else(|_| envelope.to_string());
        let _ = self.write_file(&filename, &content);
    }

    /// Log raw LLM response in JSON envelope format.
    pub fn log_raw_response(&self, body: &str, timestamp: chrono::DateTime<chrono::Local>, round: usize, duration_ms: u64) {
        if !self.enabled { return; }
        let index = self.next_index();
        let filename = format!("{}.AI.Response.raw.json", index);
        let body_value: serde_json::Value = serde_json::from_str(body)
            .unwrap_or(serde_json::Value::String(body.to_string()));
        let envelope = serde_json::json!({
            "timestamp": timestamp.to_rfc3339(),
            "round": round,
            "duration_ms": duration_ms,
            "body": body_value,
        });
        let content = serde_json::to_string_pretty(&envelope)
            .unwrap_or_else(|_| envelope.to_string());
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
                    utils::truncate(&op.arguments, TRUNCATE_ARGS_LIMIT)
                } else {
                    op.arguments.clone()
                };
                content.push_str(&format!("### Arguments\n```json\n{}\n```\n\n", args_preview));
            }

            // Result section (mirrors Go's Result field).
            if !op.result.is_empty() {
                let result_preview = if self.config.detail_level == DetailLevel::Truncated && op.result.len() > TRUNCATE_RESPONSE_LIMIT {
                    utils::truncate(&op.result, TRUNCATE_RESPONSE_LIMIT)
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
    let end = utils::floor_char_boundary(key, 3);
    let start = utils::ceil_char_boundary(key, key.len() - 3);
    format!("{}***{}", &key[..end], &key[start..])
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
mod tests;
