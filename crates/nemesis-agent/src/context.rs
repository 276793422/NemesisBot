//! Request context: carries per-request metadata through the agent loop.
//!
//! `RequestContext` holds channel, chat, user, and session information for
//! each inbound request. It also provides helper methods for RPC-specific
//! formatting, such as adding the `[rpc:correlation_id]` prefix to responses.
//!
//! `ContextBuilder` builds system prompts from workspace files (identity,
//! bootstrap, skills, memory context).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tracing::{debug, info};

/// Callback type for async tool notifications.
///
/// When a tool completes asynchronously (e.g., `cluster_rpc`), it can invoke
/// this callback to notify the agent loop or other subsystems. Mirrors Go's
/// `asyncCallback` parameter passed to `ExecuteWithContext`.
pub type AsyncCallback = Arc<dyn Fn(String) + Send + Sync>;

/// Carries per-request metadata for the agent loop.
#[derive(Clone, Serialize, Deserialize)]
pub struct RequestContext {
    /// Channel name (e.g. "web", "rpc", "discord").
    pub channel: String,
    /// Chat or conversation identifier.
    pub chat_id: String,
    /// User who sent the message.
    pub user: String,
    /// Session key for this conversation.
    pub session_key: String,
    /// Correlation ID for RPC request-response matching.
    pub correlation_id: Option<String>,
    /// Optional async callback for tools that complete asynchronously
    /// (e.g., cluster_rpc). Mirrors Go's `asyncCallback` parameter.
    /// Skipped during serialization since closures are not serializable.
    #[serde(skip)]
    pub async_callback: Option<AsyncCallback>,
}

impl std::fmt::Debug for RequestContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RequestContext")
            .field("channel", &self.channel)
            .field("chat_id", &self.chat_id)
            .field("user", &self.user)
            .field("session_key", &self.session_key)
            .field("correlation_id", &self.correlation_id)
            .field("async_callback", &self.async_callback.as_ref().map(|_| "..."))
            .finish()
    }
}

impl RequestContext {
    /// Create a new request context with the given fields and no correlation ID.
    pub fn new(channel: &str, chat_id: &str, user: &str, session_key: &str) -> Self {
        Self {
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
            user: user.to_string(),
            session_key: session_key.to_string(),
            correlation_id: None,
            async_callback: None,
        }
    }

    /// Create a new context with a correlation ID.
    pub fn with_correlation_id(
        channel: &str,
        chat_id: &str,
        user: &str,
        session_key: &str,
        correlation_id: &str,
    ) -> Self {
        Self {
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
            user: user.to_string(),
            session_key: session_key.to_string(),
            correlation_id: Some(correlation_id.to_string()),
            async_callback: None,
        }
    }

    /// Create an RPC-specific request context.
    ///
    /// This sets the channel to "rpc" and attaches the correlation ID
    /// for response matching.
    pub fn for_rpc(chat_id: &str, user: &str, session_key: &str, correlation_id: &str) -> Self {
        Self {
            channel: "rpc".to_string(),
            chat_id: chat_id.to_string(),
            user: user.to_string(),
            session_key: session_key.to_string(),
            correlation_id: Some(correlation_id.to_string()),
            async_callback: None,
        }
    }

    /// Format a message with the RPC correlation ID prefix if this is an RPC context.
    ///
    /// For RPC contexts, the output format is `[rpc:correlation_id] message`.
    /// For non-RPC contexts, the message is returned unchanged.
    pub fn format_rpc_message(&self, message: &str) -> String {
        if self.channel == "rpc" {
            if let Some(ref cid) = self.correlation_id {
                if !cid.is_empty() {
                    return format!("[rpc:{}] {}", cid, message);
                }
            }
        }
        message.to_string()
    }

    /// Returns true if this context represents an RPC request.
    pub fn is_rpc(&self) -> bool {
        self.channel == "rpc"
    }

    /// Set the async callback for this context.
    ///
    /// Mirrors Go's `asyncCallback` parameter passed to `ExecuteWithContext`.
    /// Tools like `cluster_rpc` can invoke this when they complete asynchronously.
    pub fn set_async_callback(&mut self, callback: AsyncCallback) {
        self.async_callback = Some(callback);
    }

    /// Invoke the async callback if one is set.
    ///
    /// Returns `true` if a callback was invoked, `false` if none was set.
    pub fn invoke_async_callback(&self, message: &str) -> bool {
        if let Some(ref cb) = self.async_callback {
            cb(message.to_string());
            true
        } else {
            false
        }
    }
}

// ---------------------------------------------------------------------------
// ContextBuilder
// ---------------------------------------------------------------------------

/// Bootstrap files to load from the workspace directory.
const BOOTSTRAP_FILES: &[&str] = &["AGENT.md", "IDENTITY.md", "SOUL.md", "USER.md", "MCP.md"];

/// Builds system prompts from workspace files.
///
/// The `ContextBuilder` reads workspace configuration files (identity, soul,
/// user preferences, MCP config), optional bootstrap files, and assembles
/// them into a complete system prompt for the LLM.
pub struct ContextBuilder {
    /// Workspace directory path.
    workspace: PathBuf,
    /// Tool summaries for inclusion in the system prompt.
    tool_summaries: Vec<String>,
    /// Skills information for inclusion in the system prompt.
    skills_info: Vec<SkillInfo>,
    /// Memory context (long-term + daily notes) for inclusion.
    memory_context: Option<String>,
    /// Tool definitions from a tools registry (for dynamic tool summary generation).
    /// Mirrors Go's `ContextBuilder.tools *tools.ToolRegistry`.
    tool_definitions: Vec<serde_json::Value>,
}

/// Information about a loaded skill.
#[derive(Debug, Clone)]
pub struct SkillInfo {
    /// Skill name.
    pub name: String,
    /// Skill description or first line of SKILL.md.
    pub description: String,
    /// Whether the skill is active.
    pub active: bool,
}

impl ContextBuilder {
    /// Create a new context builder for the given workspace directory.
    pub fn new(workspace: &Path) -> Self {
        Self {
            workspace: workspace.to_path_buf(),
            tool_summaries: Vec::new(),
            skills_info: Vec::new(),
            memory_context: None,
            tool_definitions: Vec::new(),
        }
    }

    /// Set tool summaries for inclusion in the system prompt.
    pub fn set_tool_summaries(&mut self, summaries: Vec<String>) {
        self.tool_summaries = summaries;
    }

    /// Set the tools registry definitions for dynamic tool summary generation.
    ///
    /// Mirrors Go's `ContextBuilder.SetToolsRegistry()`. Accepts a list of
    /// OpenAI-format tool definitions (as JSON values) and generates tool
    /// summaries from them. These summaries are included in the system prompt
    /// to inform the LLM about available tools.
    ///
    /// If `tool_summaries` has already been set manually, this method will
    /// append the generated summaries rather than replacing them.
    pub fn set_tools_registry(&mut self, definitions: Vec<serde_json::Value>) {
        self.tool_definitions = definitions.clone();

        // Generate summaries from the tool definitions
        let generated: Vec<String> = definitions
            .iter()
            .filter_map(|def| {
                let func = def.get("function")?;
                let name = func.get("name")?.as_str()?;
                let desc = func.get("description").and_then(|d| d.as_str()).unwrap_or("");
                Some(format!("- **{}**: {}", name, desc))
            })
            .collect();

        if !generated.is_empty() && self.tool_summaries.is_empty() {
            self.tool_summaries = generated;
        } else if !generated.is_empty() {
            self.tool_summaries.extend(generated);
        }
    }

    /// Get the tool definitions currently stored (if any).
    pub fn tool_definitions(&self) -> &[serde_json::Value] {
        &self.tool_definitions
    }

    /// Set skills information for inclusion in the system prompt.
    pub fn set_skills_info(&mut self, skills: Vec<SkillInfo>) {
        self.skills_info = skills;
    }

    /// Load skills from a Skills directory.
    ///
    /// Reads SKILL.md files from subdirectories of the given path.
    pub fn load_skills(&mut self, skills_dir: &Path) {
        if !skills_dir.exists() {
            return;
        }
        if let Ok(entries) = std::fs::read_dir(skills_dir) {
            for entry in entries.flatten() {
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    let skill_md = entry.path().join("SKILL.md");
                    if skill_md.exists() {
                        if let Ok(content) = std::fs::read_to_string(&skill_md) {
                            let name = entry.file_name().to_string_lossy().to_string();
                            let description = content
                                .lines()
                                .find(|l| !l.trim().is_empty())
                                .unwrap_or("")
                                .trim_start_matches('#')
                                .trim()
                                .to_string();
                            self.skills_info.push(SkillInfo {
                                name,
                                description,
                                active: true,
                            });
                        }
                    }
                }
            }
        }
    }

    /// Get skills information.
    pub fn get_skills_info(&self) -> &[SkillInfo] {
        &self.skills_info
    }

    /// Set memory context (long-term + daily notes) for inclusion in the system prompt.
    pub fn set_memory_context(&mut self, context: String) {
        self.memory_context = Some(context);
    }

    /// Build the complete system prompt.
    ///
    /// The prompt is assembled from:
    /// 1. Core identity section (time, environment, workspace)
    /// 2. Bootstrap files (IDENTITY.md, SOUL.md, USER.md, etc.)
    /// 3. Tools section
    pub fn build_system_prompt(&self, skip_bootstrap: bool) -> String {
        info!("[ContextBuilder] Building system prompt from workspace: {:?}", self.workspace);
        let mut parts = Vec::new();

        // Core identity section
        parts.push(self.build_identity());

        // Bootstrap content
        let bootstrap_content = self.load_bootstrap_files(skip_bootstrap);
        if !bootstrap_content.is_empty() {
            parts.push(bootstrap_content);
        }

        // Tools section
        let tools_section = self.build_tools_section();
        if !tools_section.is_empty() {
            parts.push(tools_section);
        }

        // Skills section
        let skills_section = self.build_skills_section();
        if !skills_section.is_empty() {
            parts.push(skills_section);
        }

        // Memory context section
        if let Some(ref memory) = self.memory_context {
            if !memory.is_empty() {
                parts.push(format!("## Memory Context\n\n{}", memory));
            }
        }

        // Join with "---" separator
        let result = parts.join("\n\n---\n\n");
        debug!("[ContextBuilder] System prompt built, total length={}", result.len());
        result
    }

    /// Build the core identity section with time, environment, and workspace info.
    fn build_identity(&self) -> String {
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M (%A)").to_string();
        let workspace_display = self.workspace.display();
        let memory_path = self.workspace.join("memory");
        let memory_display = if memory_path.exists() {
            memory_path.display().to_string()
        } else {
            "(not yet created)".to_string()
        };
        let skills_path = self.workspace.join("skills");
        let skills_display = if skills_path.exists() {
            skills_path.display().to_string()
        } else {
            "(not yet created)".to_string()
        };

        format!(
            "# Current Time\n\
             {}\n\n\
             ## Environment\n\
             - **Runtime**: NemesisBot (Rust)\n\
             - **Workspace**: {}\n\
             - **Memory Path**: {}\n\
             - **Skills Path**: {}\n\n\
             ## Workspace\n\
             Your workspace is located at: {}\n\n\
             ## Important Rules\n\n\
             1. **Always use tools** - When you need to perform an action, you must call the appropriate tool.\n\
             2. **Be helpful and accurate** - When using tools, briefly explain what you are doing.\n\
             3. **Memory** - When you need to remember something, write it to the memory file.",
            now, workspace_display, memory_display, skills_display, workspace_display
        )
    }

    /// Build the tools section of the system prompt.
    fn build_tools_section(&self) -> String {
        if self.tool_summaries.is_empty() {
            return String::new();
        }

        let mut sb = String::new();
        sb.push_str("## Available Tools\n\n");
        sb.push_str("**Important**: You must use tools to perform actions.\n\n");
        sb.push_str("You have access to the following tools:\n\n");

        for summary in &self.tool_summaries {
            sb.push_str(summary);
            sb.push('\n');
        }

        sb
    }

    /// Build the skills section of the system prompt.
    fn build_skills_section(&self) -> String {
        if self.skills_info.is_empty() {
            return String::new();
        }

        let mut sb = String::new();
        sb.push_str("## Loaded Skills\n\n");
        sb.push_str("The following skills are loaded and active:\n\n");

        for skill in &self.skills_info {
            let desc = if skill.description.is_empty() {
                "(no description)".to_string()
            } else {
                skill.description.clone()
            };
            sb.push_str(&format!("- **{}**: {}\n", skill.name, desc));
        }

        sb
    }

    /// Load bootstrap files from the workspace directory.
    ///
    /// If `skip_bootstrap` is true, only loads config files without triggering
    /// initialization logic (used for heartbeat requests).
    pub fn load_bootstrap_files(&self, skip_bootstrap: bool) -> String {
        let mut result = String::new();

        if skip_bootstrap {
            // Heartbeat mode: only load config files, do not trigger initialization
            for filename in BOOTSTRAP_FILES {
                if let Some(content) = self.read_workspace_file(filename) {
                    result.push_str(&format!("## {}\n\n{}\n\n", filename, content));
                }
            }
            return result;
        }

        // Normal mode: check for BOOTSTRAP.md first
        if let Some(content) = self.read_workspace_file("BOOTSTRAP.md") {
            return format!(
                "## Initialization Bootstrap Mode\n\n\
                 BOOTSTRAP.md file exists, indicating first startup or re-initialization.\n\n\
                 **Important instructions**:\n\
                 1. Initiate conversation following BOOTSTRAP.md content\n\
                 2. After initialization, call complete_bootstrap tool to remove BOOTSTRAP.md\n\
                 3. Do not delete the file by other means\n\n\
                 ## BOOTSTRAP.md\n\n{}",
                content
            );
        }

        // BOOTSTRAP.md does not exist: normal mode
        for filename in BOOTSTRAP_FILES {
            if let Some(content) = self.read_workspace_file(filename) {
                result.push_str(&format!("## {}\n\n{}\n\n", filename, content));
            }
        }

        result
    }

    /// Read a file from the workspace directory, returning None if it doesn't exist.
    fn read_workspace_file(&self, filename: &str) -> Option<String> {
        let path = self.workspace.join(filename);
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                debug!("[ContextBuilder] Loaded workspace file: {}", filename);
                Some(content)
            }
            Err(_) => None,
        }
    }

    /// Build a complete message list for the LLM.
    ///
    /// Constructs the system prompt, adds session info, appends history
    /// and the current user message.
    pub fn build_messages(
        &self,
        history: &[crate::types::ConversationTurn],
        summary: &str,
        current_message: &str,
        channel: &str,
        chat_id: &str,
        skip_bootstrap: bool,
    ) -> Vec<crate::r#loop::LlmMessage> {
        let mut messages = Vec::new();

        // Build system prompt
        let mut system_prompt = self.build_system_prompt(skip_bootstrap);

        // Add current session info if provided
        if !channel.is_empty() && !chat_id.is_empty() {
            system_prompt.push_str(&format!(
                "\n\n## Current Session\nChannel: {}\nChat ID: {}",
                channel, chat_id
            ));
        }

        // Add summary of previous conversation if present
        if !summary.is_empty() {
            system_prompt.push_str(&format!(
                "\n\n## Summary of Previous Conversation\n\n{}",
                summary
            ));
        }

        // Debug info
        debug!(
            "[ContextBuilder] System prompt built: {} chars, {} lines",
            system_prompt.len(),
            system_prompt.lines().count()
        );

        // System message
        messages.push(crate::r#loop::LlmMessage {
            role: "system".to_string(),
            content: system_prompt,
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
        });

        // History messages: skip ALL orphaned tool messages at the start of history.
        // Mirrors Go's loop: `for len(history) > 0 && history[0].Role == "tool" { history = history[1:] }`.
        let mut history_iter = history.iter().peekable();
        while history_iter.peek().map_or(false, |t| t.role == "tool") {
            debug!("[ContextBuilder] Skipping orphaned tool message from history");
            history_iter.next();
        }

        for turn in history_iter {
            messages.push(crate::r#loop::LlmMessage {
                role: turn.role.clone(),
                content: turn.content.clone(),
                tool_calls: if turn.tool_calls.is_empty() {
                    None
                } else {
                    Some(turn.tool_calls.clone())
                },
                tool_call_id: turn.tool_call_id.clone(),
                reasoning_content: turn.reasoning_content.clone(),
            });
        }

        // Current user message
        if !current_message.is_empty() {
            messages.push(crate::r#loop::LlmMessage {
                role: "user".to_string(),
                content: current_message.to_string(),
                tool_calls: None,
                tool_call_id: None,
                reasoning_content: None,
            });
        }

        messages
    }

    /// Returns the workspace path.
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// Append a tool result message to the message list.
    /// Mirrors Go's `ContextBuilder.AddToolResult()`.
    pub fn add_tool_result(
        messages: &mut Vec<crate::r#loop::LlmMessage>,
        tool_call_id: &str,
        _tool_name: &str,
        result: &str,
    ) {
        messages.push(crate::r#loop::LlmMessage {
            role: "tool".to_string(),
            content: result.to_string(),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.to_string()),
            reasoning_content: None,
        });
    }

    /// Append an assistant message (with optional tool calls) to the message list.
    /// Mirrors Go's `ContextBuilder.AddAssistantMessage()`.
    pub fn add_assistant_message(
        messages: &mut Vec<crate::r#loop::LlmMessage>,
        content: &str,
        tool_calls: Vec<crate::types::ToolCallInfo>,
    ) {
        messages.push(crate::r#loop::LlmMessage {
            role: "assistant".to_string(),
            content: content.to_string(),
            tool_calls: if tool_calls.is_empty() {
                None
            } else {
                Some(tool_calls)
            },
            tool_call_id: None,
            reasoning_content: None,
        });
    }
}

#[cfg(test)]
mod tests;
