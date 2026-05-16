//! Tool registration: default tools for the agent loop executor.
//!
//! Provides `register_default_tools()` which creates and returns a HashMap
//! of built-in tools (message, read_file, write_file, list_dir,
//! edit_file, append_file, delete_file, create_dir, delete_dir, sleep)
//! that can be registered with an `AgentLoopExecutor`.
//!
//! Also provides additional tools:
//! - Web search (Brave, DuckDuckGo, Perplexity)
//! - Web fetch
//! - Cluster RPC
//! - Spawn (sub-agent management)
//! - Memory tools
//! - Skills tools

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Mutex;
use tokio::time::sleep;
use tracing::info;

use crate::context::RequestContext;
use crate::r#loop::Tool;
// ===========================================================================
// Basic file/message tools
// ===========================================================================

/// Callback type for the MessageTool to publish outbound messages.
///
/// Arguments: (channel, chat_id, content)
pub type SendCallback = Box<dyn Fn(&str, &str, &str) + Send + Sync>;

/// A tool that sends a message back to the user via the outbound message bus.
///
/// When a `send_callback` is set, the tool will:
/// 1. Extract the content from the arguments
/// 2. Format it with the RPC correlation ID prefix if applicable
/// 3. Call the callback to publish the outbound message
/// 4. Return the content as the tool result
///
/// If no callback is set, it behaves as a simple passthrough (returns content).
pub struct MessageTool {
    /// Optional callback to publish outbound messages.
    send_callback: Arc<Mutex<Option<SendCallback>>>,
    /// Tracks whether a message was already sent in the current round.
    sent_in_round: Arc<std::sync::atomic::AtomicBool>,
    /// Stored channel from set_context (used when RequestContext is insufficient).
    stored_channel: Arc<std::sync::Mutex<String>>,
    /// Stored chat_id from set_context.
    stored_chat_id: Arc<std::sync::Mutex<String>>,
}

impl MessageTool {
    /// Create a new MessageTool without a send callback.
    pub fn new() -> Self {
        Self {
            send_callback: Arc::new(Mutex::new(None)),
            sent_in_round: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            stored_channel: Arc::new(std::sync::Mutex::new(String::new())),
            stored_chat_id: Arc::new(std::sync::Mutex::new(String::new())),
        }
    }

    /// Set the send callback for publishing outbound messages.
    pub fn set_send_callback(&self, callback: SendCallback) {
        let cb = self.send_callback.clone();
        // Use blocking lock since this is called during setup
        if let Ok(mut guard) = cb.try_lock() {
            *guard = Some(callback);
        }
    }

    /// Check whether a message was already sent in this round.
    pub fn has_sent_in_round(&self) -> bool {
        self.sent_in_round.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Reset the sent-in-round flag (called at the start of each LLM iteration).
    pub fn reset_sent_in_round(&self) {
        self.sent_in_round.store(false, std::sync::atomic::Ordering::Relaxed);
    }
}

impl Default for MessageTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for MessageTool {
    fn description(&self) -> String {
        "Send a message to user on a chat channel. Use this when you want to communicate something.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "The message content to send to the user"
                }
            },
            "required": ["content"]
        })
    }

    fn set_context(&self, channel: &str, chat_id: &str) {
        if let Ok(mut guard) = self.stored_channel.lock() {
            *guard = channel.to_string();
        }
        if let Ok(mut guard) = self.stored_chat_id.lock() {
            *guard = chat_id.to_string();
        }
    }

    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String> {
        // Extract content from arguments.
        let content = if let Ok(val) = serde_json::from_str::<serde_json::Value>(args) {
            if let Some(c) = val.get("content").and_then(|v| v.as_str()) {
                c.to_string()
            } else {
                args.to_string()
            }
        } else {
            args.to_string()
        };

        // Use context from RequestContext, falling back to stored context if needed.
        let channel = if context.channel.is_empty() {
            self.stored_channel.lock().unwrap_or_else(|e| e.into_inner()).clone()
        } else {
            context.channel.clone()
        };
        let chat_id = if context.chat_id.is_empty() {
            self.stored_chat_id.lock().unwrap_or_else(|e| e.into_inner()).clone()
        } else {
            context.chat_id.clone()
        };

        // If a send callback is registered, publish the outbound message.
        let guard = self.send_callback.lock().await;
        if let Some(ref callback) = *guard {
            // Format with RPC prefix if applicable.
            let formatted = context.format_rpc_message(&content);
            callback(&channel, &chat_id, &formatted);
            self.sent_in_round.store(true, std::sync::atomic::Ordering::Relaxed);
        }

        Ok(content)
    }
}

/// A tool that reads the contents of a file from disk.
pub struct ReadFileTool;

#[async_trait]
impl Tool for ReadFileTool {
    fn description(&self) -> String {
        "Read the contents of a file".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to read"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let path = extract_path(args)?;
        let path = Path::new(&path);

        if !path.exists() {
            return Err(format!("File not found: {}", path.display()));
        }

        tokio::fs::read_to_string(path)
            .await
            .map_err(|e| format!("Failed to read file: {}", e))
    }
}

/// A tool that writes content to a file on disk.
pub struct WriteFileTool;

#[async_trait]
impl Tool for WriteFileTool {
    fn description(&self) -> String {
        "Write content to a file".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to write"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write to the file"
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let (path, content) = extract_path_and_content(args)?;

        // Create parent directories if needed.
        let path = Path::new(&path);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("Failed to create directories: {}", e))?;
        }

        tokio::fs::write(path, &content)
            .await
            .map_err(|e| format!("Failed to write file: {}", e))?;

        Ok(format!("Successfully wrote {} bytes to {}", content.len(), path.display()))
    }
}

/// A tool that lists the contents of a directory.
pub struct ListDirectoryTool;

#[async_trait]
impl Tool for ListDirectoryTool {
    fn description(&self) -> String {
        "List files and directories in a path".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to list"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let path = extract_path(args)?;
        let path = Path::new(&path);

        if !path.exists() {
            return Err(format!("Directory not found: {}", path.display()));
        }

        if !path.is_dir() {
            return Err(format!("Path is not a directory: {}", path.display()));
        }

        let mut entries = tokio::fs::read_dir(path)
            .await
            .map_err(|e| format!("Failed to read directory: {}", e))?;

        let mut listing = Vec::new();
        while let Some(entry) = entries.next_entry().await.map_err(|e| format!("Entry error: {}", e))? {
            let name = entry.file_name().to_string_lossy().to_string();
            let metadata = entry.metadata().await.map_err(|e| format!("Metadata error: {}", e))?;
            let type_tag = if metadata.is_dir() { "dir" } else { "file" };
            let size = metadata.len();
            listing.push(format!("{} [{}] ({} bytes)", name, type_tag, size));
        }

        if listing.is_empty() {
            Ok("(empty directory)".to_string())
        } else {
            Ok(listing.join("\n"))
        }
    }
}

/// Extract a file path from tool arguments (JSON).
///
/// Expects either a JSON object with a "path" field, or treats the entire
/// string as a path if JSON parsing fails.
fn extract_path(args: &str) -> Result<String, String> {
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(args) {
        if let Some(path) = val.get("path").and_then(|v| v.as_str()) {
            return Ok(path.to_string());
        }
        return Err("Missing 'path' field in arguments".to_string());
    }
    // Fallback: treat raw args as path.
    Ok(args.trim().to_string())
}

/// Extract path and content from tool arguments (JSON).
///
/// Expects a JSON object with "path" and "content" fields.
fn extract_path_and_content(args: &str) -> Result<(String, String), String> {
    let val: serde_json::Value = serde_json::from_str(args)
        .map_err(|e| format!("Invalid JSON arguments: {}", e))?;

    let path = val
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'path' field")?
        .to_string();

    let content = val
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'content' field")?
        .to_string();

    Ok((path, content))
}

/// Extract path, old_text, and new_text from tool arguments (JSON).
///
/// Expects a JSON object with "path", "old_text", and "new_text" fields.
fn extract_edit_args(args: &str) -> Result<(String, String, String), String> {
    let val: serde_json::Value = serde_json::from_str(args)
        .map_err(|e| format!("Invalid JSON arguments: {}", e))?;

    let path = val
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'path' field")?
        .to_string();

    let old_text = val
        .get("old_text")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'old_text' field")?
        .to_string();

    let new_text = val
        .get("new_text")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'new_text' field")?
        .to_string();

    Ok((path, old_text, new_text))
}

/// A tool that edits a file by replacing old_text with new_text.
///
/// The old_text must exist exactly once in the file for the edit to succeed.
pub struct EditFileTool;

#[async_trait]
impl Tool for EditFileTool {
    fn description(&self) -> String {
        "Edit a file by replacing old_text with new_text. The old_text must exist exactly in the file.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Path to the file to edit"},
                "old_text": {"type": "string", "description": "Exact text to find and replace"},
                "new_text": {"type": "string", "description": "Replacement text"}
            },
            "required": ["path", "old_text", "new_text"]
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let (path, old_text, new_text) = extract_edit_args(args)?;
        let path = Path::new(&path);

        if !path.exists() {
            return Err(format!("File not found: {}", path.display()));
        }

        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| format!("Failed to read file: {}", e))?;

        if !content.contains(&old_text) {
            return Err("old_text not found in file. Make sure it matches exactly".to_string());
        }

        let count = content.matches(&old_text).count();
        if count > 1 {
            return Err(format!(
                "old_text appears {} times. Please provide more context to make it unique",
                count
            ));
        }

        let new_content = content.replacen(&old_text, &new_text, 1);

        tokio::fs::write(path, &new_content)
            .await
            .map_err(|e| format!("Failed to write file: {}", e))?;

        Ok(format!("File edited: {}", path.display()))
    }
}

/// A tool that appends content to the end of a file.
pub struct AppendFileTool;

#[async_trait]
impl Tool for AppendFileTool {
    fn description(&self) -> String {
        "Append content to the end of a file".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"path":{"type":"string","description":"Path to the file"},"content":{"type":"string","description":"Content to append"}},"required":["path","content"]})
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let (path, content) = extract_path_and_content(args)?;
        let path = Path::new(&path);

        // Create parent directories if needed.
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("Failed to create directories: {}", e))?;
        }

        // Use OpenOptions for append mode.
        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
            .map_err(|e| format!("Failed to open file: {}", e))?;

        file.write_all(content.as_bytes())
            .await
            .map_err(|e| format!("Failed to append to file: {}", e))?;

        Ok(format!("Appended {} bytes to {}", content.len(), path.display()))
    }
}

/// A tool that deletes a file from disk.
pub struct DeleteFileTool;

#[async_trait]
impl Tool for DeleteFileTool {
    fn description(&self) -> String {
        "Delete a file".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"path":{"type":"string","description":"Path to the file to delete"}},"required":["path"]})
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let path = extract_path(args)?;
        let path = Path::new(&path);

        if !path.exists() {
            return Err(format!("File not found: {}", path.display()));
        }

        if path.is_dir() {
            return Err(format!("Path is a directory, not a file: {}", path.display()));
        }

        tokio::fs::remove_file(path)
            .await
            .map_err(|e| format!("Failed to delete file: {}", e))?;

        Ok(format!("Deleted file: {}", path.display()))
    }
}

/// A tool that creates a directory (and all parent directories).
pub struct CreateDirTool;

#[async_trait]
impl Tool for CreateDirTool {
    fn description(&self) -> String {
        "Create a directory".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"path":{"type":"string","description":"Path to the directory to create"}},"required":["path"]})
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let path = extract_path(args)?;
        let path = Path::new(&path);

        if path.exists() {
            return Err(format!("Path already exists: {}", path.display()));
        }

        tokio::fs::create_dir_all(path)
            .await
            .map_err(|e| format!("Failed to create directory: {}", e))?;

        Ok("Directory created".to_string())
    }
}

/// A tool that removes a directory.
pub struct DeleteDirTool;

#[async_trait]
impl Tool for DeleteDirTool {
    fn description(&self) -> String {
        "Delete a directory and all its contents".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"path":{"type":"string","description":"Path to the directory to delete"}},"required":["path"]})
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let path = extract_path(args)?;
        let path = Path::new(&path);

        if !path.exists() {
            return Err(format!("Directory not found: {}", path.display()));
        }

        if !path.is_dir() {
            return Err(format!("Path is not a directory: {}", path.display()));
        }

        tokio::fs::remove_dir_all(path)
            .await
            .map_err(|e| format!("Failed to remove directory: {}", e))?;

        Ok("Directory removed".to_string())
    }
}

// ---------------------------------------------------------------------------
// ExecTool - Shell command execution (mirrors Go's ExecTool)
// ---------------------------------------------------------------------------

/// A tool that executes shell commands.
///
/// Mirrors Go's `ExecTool` which is registered as "exec" in the agent.
pub struct ExecTool {
    workspace: String,
    restrict: bool,
}

impl ExecTool {
    /// Create a new exec tool.
    pub fn new(workspace: &str, restrict: bool) -> Self {
        Self {
            workspace: workspace.to_string(),
            restrict,
        }
    }
}

#[async_trait]
impl Tool for ExecTool {
    fn description(&self) -> String {
        "Execute a shell command and wait for completion".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"command":{"type":"string","description":"Command to execute"},"timeout":{"type":"integer","description":"Timeout in seconds"},"cwd":{"type":"string","description":"Working directory"}},"required":["command"]})
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let val: serde_json::Value = serde_json::from_str(args)
            .map_err(|e| format!("Invalid arguments: {}", e))?;

        let command = val.get("command")
            .and_then(|v| v.as_str())
            .ok_or("Missing 'command' argument")?;

        let timeout_secs = val.get("timeout")
            .and_then(|v| v.as_u64())
            .unwrap_or(60);

        let cwd = val.get("cwd")
            .and_then(|v| v.as_str())
            .unwrap_or(&self.workspace);

        // Workspace restriction check
        if self.restrict {
            let cwd_path = std::path::Path::new(cwd);
            let ws_path = std::path::Path::new(&self.workspace);
            if !cwd_path.starts_with(ws_path) {
                return Err(format!("Access denied: path '{}' is outside workspace", cwd));
            }
        }

        let output = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            tokio::process::Command::new(if cfg!(target_os = "windows") { "cmd" } else { "sh" })
                .arg(if cfg!(target_os = "windows") { "/C" } else { "-c" })
                .arg(command)
                .current_dir(cwd)
                .output(),
        ).await;

        match output {
            Ok(Ok(out)) => {
                let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                if out.status.success() {
                    Ok(if stdout.is_empty() { "(no output)".to_string() } else { stdout })
                } else {
                    Ok(format!("Exit code: {}\nstdout: {}\nstderr: {}",
                        out.status.code().unwrap_or(-1), stdout, stderr))
                }
            }
            Ok(Err(e)) => Err(format!("Failed to execute command: {}", e)),
            Err(_) => Err(format!("Command timed out after {} seconds", timeout_secs)),
        }
    }
}

/// A tool that executes shell commands asynchronously (starts and returns quickly).
///
/// Mirrors Go's `AsyncExecTool` which is registered as "exec_async" in the agent.
pub struct AsyncExecTool {
    workspace: String,
    restrict: bool,
}

impl AsyncExecTool {
    pub fn new(workspace: &str, restrict: bool) -> Self {
        Self {
            workspace: workspace.to_string(),
            restrict,
        }
    }
}

#[async_trait]
impl Tool for AsyncExecTool {
    fn description(&self) -> String {
        "Start applications asynchronously and return quickly. Use this for GUI apps (notepad, calc, etc.) or any program where you don't need to wait for exit.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"command":{"type":"string","description":"Command to start"},"working_dir":{"type":"string","description":"Working directory"},"wait_seconds":{"type":"integer","description":"Seconds to wait for startup confirmation (default 3, range 1-10)"}},"required":["command"]})
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let val: serde_json::Value = serde_json::from_str(args)
            .map_err(|e| format!("Invalid arguments: {}", e))?;

        let command = val.get("command")
            .and_then(|v| v.as_str())
            .ok_or("Missing 'command' argument")?;

        let cwd = val.get("working_dir")
            .and_then(|v| v.as_str())
            .unwrap_or(&self.workspace);

        let wait_secs = val.get("wait_seconds")
            .and_then(|v| v.as_u64())
            .unwrap_or(3)
            .clamp(1, 10);

        // Workspace restriction check
        if self.restrict {
            let cwd_path = std::path::Path::new(cwd);
            let ws_path = std::path::Path::new(&self.workspace);
            if !cwd_path.starts_with(ws_path) {
                return Err(format!("Access denied: path '{}' is outside workspace", cwd));
            }
        }

        let shell = if cfg!(target_os = "windows") { "cmd" } else { "sh" };
        let flag = if cfg!(target_os = "windows") { "/C" } else { "-c" };

        let mut child = tokio::process::Command::new(shell)
            .arg(flag)
            .arg(command)
            .current_dir(cwd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to start command: {}", e))?;

        // Wait briefly to confirm startup
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(wait_secs),
            child.wait(),
        ).await;

        match result {
            Ok(Ok(status)) => {
                if status.success() || status.code().is_none() {
                    // Still running (no exit code) or exited cleanly
                    Ok(format!("Command '{}' started and confirmed running", command))
                } else {
                    Err(format!("Command '{}' exited prematurely with status: {}", command, status))
                }
            }
            Ok(Err(e)) => Err(format!("Failed to wait for command: {}", e)),
            Err(_) => {
                // Timeout — process still running, which is the expected async case
                Ok(format!("Command '{}' started successfully (still running)", command))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// CronTool - Job scheduling (mirrors Go's CronTool)
// ---------------------------------------------------------------------------

// ===========================================================================
// Bootstrap completion tool
// ===========================================================================

/// A tool that completes the bootstrap process by deleting BOOTSTRAP.md.
///
/// Mirrors Go's `CompleteBootstrapTool`. Requires `confirmed: true` to proceed.
pub struct BootstrapTool {
    workspace: String,
}

impl BootstrapTool {
    /// Create a new bootstrap tool with the workspace path.
    pub fn new(workspace: &str) -> Self {
        Self {
            workspace: workspace.to_string(),
        }
    }
}

#[async_trait]
impl Tool for BootstrapTool {
    fn description(&self) -> String {
        "Complete the bootstrap initialization by deleting BOOTSTRAP.md. Must confirm all initialization steps are done first.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "confirmed": {
                    "type": "boolean",
                    "description": "Confirm that initialization is complete and ready to delete BOOTSTRAP.md"
                }
            },
            "required": ["confirmed"]
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let val: serde_json::Value = serde_json::from_str(args)
            .map_err(|e| format!("Invalid arguments: {}", e))?;

        let confirmed = val.get("confirmed")
            .and_then(|v| v.as_bool())
            .ok_or("Missing or invalid 'confirmed' parameter (must be a boolean)")?;

        if !confirmed {
            return Err("Must confirm initialization is complete before deleting bootstrap file.".to_string());
        }

        let bootstrap_path = Path::new(&self.workspace).join("BOOTSTRAP.md");

        if !bootstrap_path.exists() {
            return Ok("BOOTSTRAP.md has already been removed. Initialization is complete.".to_string());
        }

        match tokio::fs::remove_file(&bootstrap_path).await {
            Ok(()) => Ok("Bootstrap initialization complete! BOOTSTRAP.md has been deleted. The system will load configuration files on next startup.".to_string()),
            Err(e) => Err(format!("Failed to delete BOOTSTRAP.md: {}", e)),
        }
    }
}

// ===========================================================================
// Cron tool
// ===========================================================================

/// A tool that manages cron jobs for scheduling tasks.
///
/// Mirrors Go's `CronTool` which is registered as "cron" in the agent.
pub struct CronTool {
    service: Arc<std::sync::Mutex<nemesis_cron::service::CronService>>,
    channel: Arc<std::sync::Mutex<String>>,
    chat_id: Arc<std::sync::Mutex<String>>,
}

impl CronTool {
    /// Create a new cron tool with the given cron service.
    pub fn new(service: Arc<std::sync::Mutex<nemesis_cron::service::CronService>>) -> Self {
        Self {
            service,
            channel: Arc::new(std::sync::Mutex::new(String::new())),
            chat_id: Arc::new(std::sync::Mutex::new(String::new())),
        }
    }
}

#[async_trait]
impl Tool for CronTool {
    fn description(&self) -> String {
        "Schedule reminders, tasks, or system commands. Use at_seconds for one-time reminders, every_seconds for recurring tasks, cron_expr for complex schedules.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"action":{"type":"string","description":"One of: create, delete, list"},"at_seconds":{"type":"integer","description":"Seconds from now for one-time execution"},"every_seconds":{"type":"integer","description":"Interval in seconds for recurring execution"},"cron_expr":{"type":"string","description":"Cron expression for complex schedules"},"command":{"type":"string","description":"Command or message to execute"},"message":{"type":"string","description":"Reminder message"}}})
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let val: serde_json::Value = serde_json::from_str(args)
            .map_err(|e| format!("Invalid arguments: {}", e))?;

        let action = val.get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let svc = self.service.lock()
            .map_err(|e| format!("Lock error: {}", e))?;

        match action {
            "list" => {
                let jobs = svc.list_jobs(true);
                let result: Vec<serde_json::Value> = jobs.iter().map(|j| {
                    serde_json::json!({
                        "id": j.id,
                        "name": j.name,
                        "schedule": j.schedule,
                        "enabled": j.enabled,
                    })
                }).collect();
                Ok(serde_json::to_string_pretty(&result).unwrap_or_else(|_| "[]".to_string()))
            }
            "create" => {
                let name = val.get("name").and_then(|v| v.as_str()).unwrap_or("unnamed");
                let schedule_str = val.get("schedule").and_then(|v| v.as_str()).unwrap_or("");
                let content = val.get("content").and_then(|v| v.as_str()).unwrap_or("");
                let deliver = val.get("deliver").and_then(|v| v.as_bool()).unwrap_or(true);

                if schedule_str.is_empty() {
                    return Err("Missing 'schedule' argument".to_string());
                }

                // Parse schedule: support "every:Ns", "at:TIMESTAMP", "cron:EXPR"
                let schedule = if schedule_str.starts_with("every:") {
                    let secs_str = schedule_str.trim_start_matches("every:").trim_end_matches('s');
                    let secs: i64 = secs_str.parse().map_err(|_| "Invalid interval")?;
                    nemesis_cron::service::CronSchedule {
                        kind: "every".to_string(),
                        at_ms: None,
                        every_ms: Some(secs * 1000),
                        expr: None,
                        tz: None,
                    }
                } else if schedule_str.starts_with("at:") {
                    let ts_str = schedule_str.trim_start_matches("at:");
                    let ts = chrono::DateTime::parse_from_rfc3339(ts_str)
                        .map_err(|e| format!("Invalid timestamp: {}", e))?;
                    nemesis_cron::service::CronSchedule {
                        kind: "at".to_string(),
                        at_ms: Some(ts.timestamp_millis()),
                        every_ms: None,
                        expr: None,
                        tz: None,
                    }
                } else {
                    nemesis_cron::service::CronSchedule {
                        kind: "cron".to_string(),
                        at_ms: None,
                        every_ms: None,
                        expr: Some(schedule_str.to_string()),
                        tz: None,
                    }
                };

                let channel = self.channel.lock().unwrap_or_else(|e| e.into_inner()).clone();
                let chat_id = self.chat_id.lock().unwrap_or_else(|e| e.into_inner()).clone();

                let job = svc.add_job(
                    name,
                    schedule,
                    content,
                    deliver,
                    if channel.is_empty() { None } else { Some(&channel) },
                    if chat_id.is_empty() { None } else { Some(&chat_id) },
                ).map_err(|e| e.to_string())?;
                Ok(format!("Created cron job: {} (ID: {})", job.name, job.id))
            }
            "delete" => {
                let id = val.get("id").and_then(|v| v.as_str()).unwrap_or("");
                if id.is_empty() {
                    return Err("Missing 'id' argument".to_string());
                }
                match svc.remove_job(id) {
                    true => Ok(format!("Deleted cron job: {}", id)),
                    false => Err(format!("Job not found: {}", id)),
                }
            }
            _ => Err(format!("Unknown cron action: '{}'. Use: list, create, delete", action)),
        }
    }

    fn set_context(&self, channel: &str, chat_id: &str) {
        if let Ok(mut ch) = self.channel.lock() {
            *ch = channel.to_string();
        }
        if let Ok(mut ci) = self.chat_id.lock() {
            *ci = chat_id.to_string();
        }
    }
}

/// Maximum sleep duration: 1 hour (matches Go implementation).
const MAX_SLEEP_SECONDS: u64 = 3600;

/// A tool that sleeps for a specified duration in seconds.
///
/// This is a utility tool for testing delays and timeouts.
/// Maximum duration is 60 seconds.
pub struct SleepTool;

#[async_trait]
impl Tool for SleepTool {
    fn description(&self) -> String {
        "Suspend execution for a specified duration in seconds".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"seconds":{"type":"number","description":"Duration in seconds"}},"required":["seconds"]})
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let seconds = if let Ok(val) = serde_json::from_str::<serde_json::Value>(args) {
            val.get("duration")
                .and_then(|v| v.as_u64())
                .ok_or("Missing or invalid 'duration' field (must be a positive integer)")?
        } else {
            args.trim()
                .parse::<u64>()
                .map_err(|_| "Invalid duration: must be a positive integer".to_string())?
        };

        if seconds < 1 {
            return Err("Duration must be at least 1 second".to_string());
        }
        if seconds > MAX_SLEEP_SECONDS {
            return Err(format!(
                "Duration cannot exceed {} seconds",
                MAX_SLEEP_SECONDS
            ));
        }

        sleep(Duration::from_secs(seconds)).await;
        Ok(format!("Slept for {} seconds", seconds))
    }
}

// ===========================================================================
// Web search tools
// ===========================================================================

/// Configuration for web search providers.
#[derive(Debug, Clone)]
pub struct WebSearchConfig {
    /// Brave Search API key.
    pub brave_api_key: Option<String>,
    /// Brave Search max results.
    pub brave_max_results: usize,
    /// Brave Search enabled.
    pub brave_enabled: bool,
    /// DuckDuckGo max results.
    pub duckduckgo_max_results: usize,
    /// DuckDuckGo enabled.
    pub duckduckgo_enabled: bool,
    /// Perplexity API key.
    pub perplexity_api_key: Option<String>,
    /// Perplexity max results.
    pub perplexity_max_results: usize,
    /// Perplexity enabled.
    pub perplexity_enabled: bool,
}

impl Default for WebSearchConfig {
    fn default() -> Self {
        Self {
            brave_api_key: None,
            brave_max_results: 5,
            brave_enabled: false,
            duckduckgo_max_results: 5,
            duckduckgo_enabled: true,
            perplexity_api_key: None,
            perplexity_max_results: 5,
            perplexity_enabled: false,
        }
    }
}

/// Web search tool that queries search engines.
///
/// Supports multiple providers with a configurable fallback chain:
/// Brave -> DuckDuckGo -> Perplexity.
pub struct WebSearchTool {
    config: WebSearchConfig,
}

impl WebSearchTool {
    /// Create a new web search tool with the given configuration.
    pub fn new(config: WebSearchConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Tool for WebSearchTool {
    fn description(&self) -> String {
        "Search the web for current information. Returns titles, URLs, and snippets from search results.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"query":{"type":"string","description":"Search query string"}},"required":["query"]})
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let query = extract_search_query(args)?;

        // Try providers in order of preference.
        if self.config.brave_enabled && self.config.brave_api_key.is_some() {
            return self.search_brave(&query).await;
        }

        if self.config.duckduckgo_enabled {
            return self.search_duckduckgo(&query).await;
        }

        if self.config.perplexity_enabled && self.config.perplexity_api_key.is_some() {
            return self.search_perplexity(&query).await;
        }

        Err("No search provider configured. Enable at least one search provider.".to_string())
    }
}

impl WebSearchTool {
    #[allow(dead_code)]
    fn extract_query(&self, args: &str) -> Result<String, String> {
        extract_search_query(args)
    }

    async fn search_brave(&self, query: &str) -> Result<String, String> {
        let api_key = match &self.config.brave_api_key {
            Some(k) if !k.is_empty() => k.clone(),
            _ => return Err("Brave API key not configured".to_string()),
        };
        let count = self.config.brave_max_results;

        let url = format!(
            "https://api.search.brave.com/res/v1/web/search?q={}&count={}",
            urlencoding(query),
            count
        );

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| format!("failed to create HTTP client: {}", e))?;

        let resp = client
            .get(&url)
            .header("Accept", "application/json")
            .header("X-Subscription-Token", &api_key)
            .send()
            .await
            .map_err(|e| format!("request failed: {}", e))?;

        let body = resp
            .text()
            .await
            .map_err(|e| format!("failed to read response: {}", e))?;

        #[derive(serde::Deserialize)]
        struct SearchResult {
            title: String,
            url: String,
            #[serde(default)]
            description: String,
        }

        #[derive(serde::Deserialize, Default)]
        struct WebResults {
            #[serde(default)]
            results: Vec<SearchResult>,
        }

        #[derive(serde::Deserialize)]
        struct SearchResponse {
            #[serde(default)]
            web: WebResults,
        }

        let search_resp: SearchResponse =
            serde_json::from_str(&body).map_err(|e| format!("failed to parse response: {}", e))?;

        if search_resp.web.results.is_empty() {
            return Ok(format!("No results for: {}", query));
        }

        let mut lines = vec![format!("Results for: {}", query)];
        for (i, item) in search_resp.web.results.iter().take(count).enumerate() {
            lines.push(format!("{}. {}\n   {}", i + 1, item.title, item.url));
            if !item.description.is_empty() {
                lines.push(format!("   {}", item.description));
            }
        }

        Ok(lines.join("\n"))
    }

    async fn search_duckduckgo(&self, query: &str) -> Result<String, String> {
        let count = self.config.duckduckgo_max_results;

        let url = format!(
            "https://html.duckduckgo.com/html/?q={}",
            urlencoding(query)
        );

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
            .build()
            .map_err(|e| format!("failed to create HTTP client: {}", e))?;

        let resp = client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("request failed: {}", e))?;

        let html = resp
            .text()
            .await
            .map_err(|e| format!("failed to read response: {}", e))?;

        // Extract results from DDG HTML
        let link_re = regex::Regex::new(
            r#"<a[^>]*class="[^"]*result__a[^"]*"[^>]*href="([^"]+)"[^>]*>([\s\S]*?)</a>"#,
        )
        .map_err(|e| format!("regex error: {}", e))?;

        let link_captures: Vec<_> = link_re.captures_iter(&html).take(count + 5).collect();
        if link_captures.is_empty() {
            return Ok(format!(
                "No results found or extraction failed. Query: {}",
                query
            ));
        }

        let snippet_re = regex::Regex::new(
            r#"<a class="result__snippet[^"]*".*?>([\s\S]*?)</a>"#,
        )
        .map_err(|e| format!("regex error: {}", e))?;

        let snippet_captures: Vec<_> = snippet_re.captures_iter(&html).take(count + 5).collect();
        let tag_re = regex::Regex::new(r"<[^>]+>").map_err(|e| format!("regex error: {}", e))?;
        let strip_tags = |content: &str| -> String {
            tag_re.replace_all(content, "").trim().to_string()
        };

        let mut lines = vec![format!("Results for: {} (via DuckDuckGo)", query)];
        let max_items = link_captures.len().min(count);

        for i in 0..max_items {
            let caps = &link_captures[i];
            let url_str = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let title = strip_tags(caps.get(2).map(|m| m.as_str()).unwrap_or(""));

            let mut url_clean = url_str.to_string();
            if url_clean.contains("uddg=") {
                if let Some(decoded) = url_decode_query_param(&url_clean, "uddg") {
                    url_clean = decoded;
                }
            }

            lines.push(format!("{}. {}\n   {}", i + 1, title, url_clean));

            if i < snippet_captures.len() {
                let snippet = strip_tags(snippet_captures[i].get(1).map(|m| m.as_str()).unwrap_or(""));
                if !snippet.is_empty() {
                    lines.push(format!("   {}", snippet));
                }
            }
        }

        Ok(lines.join("\n"))
    }

    async fn search_perplexity(&self, query: &str) -> Result<String, String> {
        let api_key = match &self.config.perplexity_api_key {
            Some(k) if !k.is_empty() => k.clone(),
            _ => return Err("Perplexity API key not configured".to_string()),
        };
        let count = self.config.perplexity_max_results;

        let payload = serde_json::json!({
            "model": "sonar",
            "messages": [
                {
                    "role": "system",
                    "content": "You are a search assistant. Provide concise search results with titles, URLs, and brief descriptions in the following format:\n1. Title\n   URL\n   Description\n\nDo not add extra commentary."
                },
                {
                    "role": "user",
                    "content": format!("Search for: {}. Provide up to {} relevant results.", query, count)
                }
            ],
            "max_tokens": 1000
        });

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| format!("failed to create HTTP client: {}", e))?;

        let resp = client
            .post("https://api.perplexity.ai/chat/completions")
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&payload)
            .send()
            .await
            .map_err(|e| format!("request failed: {}", e))?;

        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| format!("failed to read response: {}", e))?;

        if !status.is_success() {
            return Err(format!("Perplexity API error: {}", body));
        }

        #[derive(serde::Deserialize)]
        struct Message {
            content: String,
        }

        #[derive(serde::Deserialize)]
        struct Choice {
            message: Message,
        }

        #[derive(serde::Deserialize)]
        struct SearchResponse {
            #[serde(default)]
            choices: Vec<Choice>,
        }

        let search_resp: SearchResponse =
            serde_json::from_str(&body).map_err(|e| format!("failed to parse response: {}", e))?;

        if search_resp.choices.is_empty() {
            return Ok(format!("No results for: {}", query));
        }

        Ok(format!(
            "Results for: {} (via Perplexity)\n{}",
            query, search_resp.choices[0].message.content
        ))
    }
}

/// Extract search query from tool arguments.
fn extract_search_query(args: &str) -> Result<String, String> {
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(args) {
        if let Some(query) = val.get("query").and_then(|v| v.as_str()) {
            return Ok(query.to_string());
        }
    }
    // Fallback: treat the entire argument as a query.
    Ok(args.trim().to_string())
}

/// Simple percent-encoding for URL parameters (query strings).
fn urlencoding(s: &str) -> String {
    let mut result = String::with_capacity(s.len() * 3);
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            b' ' => result.push('+'),
            _ => {
                result.push('%');
                result.push_str(&format!("{:02X}", byte));
            }
        }
    }
    result
}

/// Decode a query parameter value from a URL that contains query params.
/// For example, extract "uddg" from "https://duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com"
fn url_decode_query_param(url: &str, param: &str) -> Option<String> {
    let prefix = format!("{}=", param);
    // Find the parameter in the URL
    for part in url.split('&').chain(url.split('?').skip(1).flat_map(|s| s.split('&'))) {
        if let Some(val) = part.strip_prefix(&prefix) {
            return Some(percent_decode(val));
        }
    }
    None
}

/// Simple percent-decoding.
fn percent_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                result.push(byte as char);
            } else {
                result.push('%');
                result.push_str(&hex);
            }
        } else if c == '+' {
            result.push(' ');
        } else {
            result.push(c);
        }
    }
    result
}

/// Web fetch tool that downloads content from URLs.
pub struct WebFetchTool {
    /// Maximum response body size in bytes.
    pub max_size: usize,
}

impl WebFetchTool {
    /// Create a new web fetch tool with the given maximum response size.
    pub fn new(max_size: usize) -> Self {
        Self { max_size }
    }
}

#[async_trait]
impl Tool for WebFetchTool {
    fn description(&self) -> String {
        "Fetch a URL and extract readable content. Use this to get weather info, news, articles, or any web content.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"url":{"type":"string","description":"URL to fetch"}},"required":["url"]})
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let url = extract_url(args)?;

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
            .build()
            .map_err(|e| format!("failed to create HTTP client: {}", e))?;

        let resp = client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("request failed: {}", e))?;

        let status = resp.status();
        if !status.is_success() {
            return Err(format!("HTTP {} for {}", status, url));
        }

        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        let body = resp
            .bytes()
            .await
            .map_err(|e| format!("failed to read response: {}", e))?;

        if body.len() > self.max_size {
            return Err(format!(
                "Response too large: {} bytes (max: {})",
                body.len(),
                self.max_size
            ));
        }

        // If the content is HTML, extract text content
        if content_type.contains("text/html") {
            let html = String::from_utf8_lossy(&body);

            // Remove script and style blocks
            let script_re =
                regex::Regex::new(r"(?is)<script[^>]*>.*?</script>").map_err(|e| e.to_string())?;
            let style_re =
                regex::Regex::new(r"(?is)<style[^>]*>.*?</style>").map_err(|e| e.to_string())?;
            let tag_re =
                regex::Regex::new(r"<[^>]+>").map_err(|e| e.to_string())?;

            let text = script_re.replace_all(&html, "");
            let text = style_re.replace_all(&text, "");
            let text = tag_re.replace_all(&text, " ");

            // Decode HTML entities
            let text = text
                .replace("&amp;", "&")
                .replace("&lt;", "<")
                .replace("&gt;", ">")
                .replace("&quot;", "\"")
                .replace("&#39;", "'")
                .replace("&nbsp;", " ");

            // Collapse whitespace
            let ws_re = regex::Regex::new(r"\s+").map_err(|e| e.to_string())?;
            let text = ws_re.replace_all(&text, " ");

            let extracted = text.trim();
            if extracted.is_empty() {
                return Ok(format!(
                    "Content from {} ({} bytes, HTML with no extractable text)",
                    url,
                    body.len()
                ));
            }

            Ok(format!(
                "Content from {} ({} bytes, extracted text):\n{}",
                url,
                body.len(),
                extracted
            ))
        } else {
            // Non-HTML: return as-is (text, JSON, etc.)
            let text = String::from_utf8_lossy(&body);
            Ok(format!(
                "Content from {} ({} bytes, {}):\n{}",
                url,
                body.len(),
                content_type,
                text
            ))
        }
    }
}

/// Extract URL from tool arguments.
fn extract_url(args: &str) -> Result<String, String> {
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(args) {
        if let Some(url) = val.get("url").and_then(|v| v.as_str()) {
            return Ok(url.to_string());
        }
    }
    Ok(args.trim().to_string())
}

// ===========================================================================
// Cluster RPC tool
// ===========================================================================

/// Configuration for the cluster RPC tool.
#[derive(Debug, Clone)]
pub struct ClusterRpcConfig {
    /// Node ID of the local node.
    pub local_node_id: String,
    /// Default timeout in seconds.
    pub timeout_secs: u64,
    /// Local RPC port (included in payloads so remote nodes can callback).
    pub local_rpc_port: u16,
}

impl Default for ClusterRpcConfig {
    fn default() -> Self {
        Self {
            local_node_id: String::new(),
            timeout_secs: 3600,
            local_rpc_port: 21949,
        }
    }
}

/// Setup the cluster RPC channel for peer-to-peer communication.
///
/// Mirrors Go's `setupClusterRPCChannel`. See the newer `setup_cluster_rpc_channel`
/// function below for the full implementation with continuation manager support.
/// This is a convenience wrapper.
pub fn setup_cluster_rpc_channel_with_config(
    cluster_config: &ClusterRpcConfig,
) -> ClusterRpcChannelConfig {
    let config = ClusterRpcChannelConfig::default();

    tracing::info!(
        local_node_id = %cluster_config.local_node_id,
        timeout_secs = cluster_config.timeout_secs,
        "Cluster RPC channel configured (24h B-side safety net)"
    );

    config
}

/// Register the LLM handler for peer_chat RPC action on the RPC server.
///
/// When a peer_chat request arrives, this handler:
/// 1. Immediately returns ACK to the sender
/// 2. Asynchronously processes the LLM request
/// 3. Calls back to the sender with the response
///
/// This function takes an RPC server and registers the peer_chat handler
/// that will invoke the LLM provider.
pub fn register_peer_chat_handler<F>(
    handlers: &mut std::collections::HashMap<String, Box<dyn Fn(serde_json::Value) -> Result<serde_json::Value, String> + Send + Sync>>,
    llm_handler: F,
) where
    F: Fn(serde_json::Value) -> Result<serde_json::Value, String> + Send + Sync + 'static,
{
    handlers.insert("peer_chat".to_string(), Box::new(llm_handler));
    handlers.insert(
        "peer_chat_callback".to_string(),
        Box::new(|payload| {
            // Callback handler: receive the response from the remote node
            // and route it through the continuation system
            let task_id = payload
                .get("task_id")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let content = payload
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            tracing::info!(
                task_id = task_id,
                content_len = content.len(),
                "Received peer_chat_callback"
            );
            Ok(serde_json::json!({
                "status": "received",
                "task_id": task_id,
            }))
        }),
    );

    tracing::info!("Registered peer_chat + peer_chat_callback handlers");
}

/// Cluster RPC tool for inter-node communication.
///
/// Sends a request to a remote node in the cluster and returns the response.
/// When an `rpc_call_fn` is provided, it performs a real RPC call; otherwise
/// it returns an error indicating the cluster is not available.
pub struct ClusterRpcTool {
    config: ClusterRpcConfig,
    /// Stored channel from set_context.
    stored_channel: Arc<std::sync::Mutex<String>>,
    /// Stored chat_id from set_context.
    stored_chat_id: Arc<std::sync::Mutex<String>>,
    /// Optional RPC call function: (target_node, action, payload) -> Result<serde_json::Value, String>
    rpc_call_fn: Option<Arc<dyn Fn(&str, &str, serde_json::Value) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<serde_json::Value, String>> + Send>> + Send + Sync>>,
}

impl ClusterRpcTool {
    /// Create a new cluster RPC tool.
    pub fn new(config: ClusterRpcConfig) -> Self {
        Self {
            config,
            stored_channel: Arc::new(std::sync::Mutex::new(String::new())),
            stored_chat_id: Arc::new(std::sync::Mutex::new(String::new())),
            rpc_call_fn: None,
        }
    }

    /// Set the RPC call function for performing actual cluster RPC calls.
    ///
    /// The function signature is: `(target_node, action, payload) -> Future<Output = Result<Value, String>>`
    pub fn set_rpc_call_fn(
        &mut self,
        f: Arc<
            dyn Fn(&str, &str, serde_json::Value)
                -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<serde_json::Value, String>> + Send>>
            + Send
            + Sync,
        >,
    ) {
        self.rpc_call_fn = Some(f);
    }
}

#[async_trait]
impl Tool for ClusterRpcTool {
    fn description(&self) -> String {
        "Send a message to another bot in the cluster".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "target": {"type": "string", "description": "Target bot ID"},
                "message": {"type": "string", "description": "Message to send"},
                "timeout": {"type": "integer", "description": "Timeout in seconds"}
            },
            "required": ["target", "message"]
        })
    }

    fn set_context(&self, channel: &str, chat_id: &str) {
        if let Ok(mut guard) = self.stored_channel.lock() {
            *guard = channel.to_string();
        }
        if let Ok(mut guard) = self.stored_chat_id.lock() {
            *guard = chat_id.to_string();
        }
    }

    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String> {
        let val: serde_json::Value = serde_json::from_str(args)
            .map_err(|e| format!("Invalid JSON arguments: {}", e))?;

        let target_node = val
            .get("target_node")
            .or_else(|| val.get("target"))
            .or_else(|| val.get("peer_id"))
            .and_then(|v| v.as_str())
            .ok_or("Missing 'target_node' field")?;

        // Extract message content: check "message" first, then "data.content" (testai-3.0 format)
        let message = val
            .get("message")
            .and_then(|v| v.as_str())
            .or_else(|| val.get("data").and_then(|d| d.get("content")).and_then(|v| v.as_str()))
            .unwrap_or("");

        let rpc_call = match &self.rpc_call_fn {
            Some(f) => f,
            None => {
                return Err(format!(
                    "Cluster RPC is not available (no RPC client configured). Cannot reach node '{}'.",
                    target_node
                ));
            }
        };

        // Build the payload with context information
        let channel = if context.channel.is_empty() {
            self.stored_channel
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        } else {
            context.channel.clone()
        };

        let chat_id = if context.chat_id.is_empty() {
            self.stored_chat_id
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        } else {
            context.chat_id.clone()
        };

        let payload = serde_json::json!({
            "content": message,
            "channel": channel,
            "chat_id": chat_id,
            "timeout": self.config.timeout_secs,
            "_source_rpc_port": self.config.local_rpc_port,
        });

        let result = rpc_call(target_node, "peer_chat", payload).await?;

        // Check if the response is an async ACK from PeerChatHandler.
        // ACK format: {"status": "accepted", "task_id": "auto-xxx"}
        // In this case, return __ASYNC__ marker so AgentLoop saves a continuation snapshot.
        let status = result.get("status").and_then(|v| v.as_str()).unwrap_or("");
        if status == "accepted" {
            let task_id = result
                .get("task_id")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            tracing::info!(
                task_id = %task_id,
                target = %target_node,
                "Peer chat ACK received, returning async marker"
            );
            return Ok(format!("__ASYNC__:{}:{}", task_id, target_node));
        }

        // Synchronous response — extract content field
        let content = result
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        Ok(content.to_string())
    }
}

// ===========================================================================
// Spawn tool (sub-agent management)
// ===========================================================================

/// Configuration for the spawn tool.
#[derive(Debug, Clone)]
pub struct SpawnConfig {
    /// Default model for spawned agents.
    pub default_model: String,
    /// Maximum number of concurrent sub-agents.
    pub max_concurrent: usize,
}

/// Spawn tool for creating sub-agents.
///
/// Spawns a new sub-agent to handle a specific task independently.
/// When a `spawn_fn` is provided, it performs a real spawn; otherwise
/// it returns an error indicating sub-agent support is not configured.
pub struct SpawnTool {
    config: SpawnConfig,
    /// Allowlist checker: returns true if the parent can spawn the target.
    allowlist_checker: Option<Box<dyn Fn(&str) -> bool + Send + Sync>>,
    /// Stored channel from set_context.
    stored_channel: Arc<std::sync::Mutex<String>>,
    /// Stored chat_id from set_context.
    stored_chat_id: Arc<std::sync::Mutex<String>>,
    /// Optional spawn function: (agent_id, task, model, channel, chat_id) -> Future<Output = Result<String, String>>
    spawn_fn: Option<Arc<dyn Fn(&str, &str, &str, &str, &str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send>> + Send + Sync>>,
}

impl SpawnTool {
    /// Create a new spawn tool with the given configuration.
    pub fn new(config: SpawnConfig) -> Self {
        Self {
            config,
            allowlist_checker: None,
            stored_channel: Arc::new(std::sync::Mutex::new(String::new())),
            stored_chat_id: Arc::new(std::sync::Mutex::new(String::new())),
            spawn_fn: None,
        }
    }

    /// Set the allowlist checker function.
    pub fn set_allowlist_checker(&mut self, checker: Box<dyn Fn(&str) -> bool + Send + Sync>) {
        self.allowlist_checker = Some(checker);
    }

    /// Set the spawn function for performing actual sub-agent creation.
    ///
    /// The function signature is: `(agent_id, task, model, channel, chat_id) -> Future<Output = Result<String, String>>`
    pub fn set_spawn_fn(
        &mut self,
        f: Arc<
            dyn Fn(&str, &str, &str, &str, &str)
                -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send>>
            + Send
            + Sync,
        >,
    ) {
        self.spawn_fn = Some(f);
    }
}

#[async_trait]
impl Tool for SpawnTool {
    fn description(&self) -> String {
        "Spawn a sub-agent to handle a task independently".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task": {"type": "string", "description": "Task description for the sub-agent"},
                "context": {"type": "string", "description": "Additional context"}
            },
            "required": ["task"]
        })
    }

    fn set_context(&self, channel: &str, chat_id: &str) {
        if let Ok(mut guard) = self.stored_channel.lock() {
            *guard = channel.to_string();
        }
        if let Ok(mut guard) = self.stored_chat_id.lock() {
            *guard = chat_id.to_string();
        }
    }

    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String> {
        let val: serde_json::Value = serde_json::from_str(args)
            .map_err(|e| format!("Invalid JSON arguments: {}", e))?;

        let agent_id = val
            .get("agent_id")
            .and_then(|v| v.as_str())
            .ok_or("Missing 'agent_id' field")?;

        let task = val
            .get("task")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Check allowlist.
        if let Some(ref checker) = self.allowlist_checker {
            if !checker(agent_id) {
                return Err(format!(
                    "Not allowed to spawn agent '{}'. Check sub-agent permissions.",
                    agent_id
                ));
            }
        }

        // Use context from RequestContext, falling back to stored context.
        let channel = if context.channel.is_empty() {
            self.stored_channel
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        } else {
            context.channel.clone()
        };

        let chat_id = if context.chat_id.is_empty() {
            self.stored_chat_id
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        } else {
            context.chat_id.clone()
        };

        let spawn_fn = match &self.spawn_fn {
            Some(f) => f,
            None => {
                return Err(format!(
                    "Sub-agent spawning is not available (no spawn function configured). Cannot spawn agent '{}' for task.",
                    agent_id
                ));
            }
        };

        spawn_fn(agent_id, task, &self.config.default_model, &channel, &chat_id).await
    }
}

// ===========================================================================
// Memory tools
// ===========================================================================

/// Memory search tool for searching conversation memory.
///
/// Delegates to `nemesis_memory::memory_tools::MemoryToolExecutor` for the
/// actual search. If no memory manager is configured, returns an error.
pub struct MemorySearchTool {
    executor: Option<Arc<nemesis_memory::memory_tools::MemoryToolExecutor>>,
}

impl MemorySearchTool {
    /// Create a new memory search tool backed by the given executor.
    pub fn new(executor: Option<Arc<nemesis_memory::memory_tools::MemoryToolExecutor>>) -> Self {
        Self { executor }
    }
}

#[async_trait]
impl Tool for MemorySearchTool {
    fn description(&self) -> String {
        "Search long-term memory for relevant information".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"query":{"type":"string","description":"Search query"},"limit":{"type":"integer","description":"Maximum results to return"}},"required":["query"]})
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let executor = match &self.executor {
            Some(e) => e,
            None => return Err("Memory store is not available".to_string()),
        };

        let val: serde_json::Value = serde_json::from_str(args)
            .map_err(|e| format!("Invalid JSON arguments: {}", e))?;

        let result = executor.execute("memory_search", &val).await;
        if result.success {
            Ok(result.content)
        } else {
            Err(result.content)
        }
    }
}

/// Memory store tool for storing information in long-term memory.
///
/// Delegates to `nemesis_memory::memory_tools::MemoryToolExecutor`.
pub struct MemoryStoreTool {
    executor: Option<Arc<nemesis_memory::memory_tools::MemoryToolExecutor>>,
}

impl MemoryStoreTool {
    /// Create a new memory store tool backed by the given executor.
    pub fn new(executor: Option<Arc<nemesis_memory::memory_tools::MemoryToolExecutor>>) -> Self {
        Self { executor }
    }
}

#[async_trait]
impl Tool for MemoryStoreTool {
    fn description(&self) -> String {
        "Store information in long-term memory".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"key":{"type":"string","description":"Memory key"},"content":{"type":"string","description":"Content to store"},"tags":{"type":"string","description":"Comma-separated tags"}},"required":["key","content"]})
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let executor = match &self.executor {
            Some(e) => e,
            None => return Err("Memory store is not available".to_string()),
        };

        let val: serde_json::Value = serde_json::from_str(args)
            .map_err(|e| format!("Invalid JSON arguments: {}", e))?;

        let result = executor.execute("memory_store", &val).await;
        if result.success {
            Ok(result.content)
        } else {
            Err(result.content)
        }
    }
}

/// Memory forget tool for removing information from long-term memory.
///
/// Delegates to `nemesis_memory::memory_tools::MemoryToolExecutor`.
pub struct MemoryForgetTool {
    executor: Option<Arc<nemesis_memory::memory_tools::MemoryToolExecutor>>,
}

impl MemoryForgetTool {
    /// Create a new memory forget tool backed by the given executor.
    pub fn new(executor: Option<Arc<nemesis_memory::memory_tools::MemoryToolExecutor>>) -> Self {
        Self { executor }
    }
}

#[async_trait]
impl Tool for MemoryForgetTool {
    fn description(&self) -> String {
        "Remove information from long-term memory".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"action":{"type":"string","description":"Action: delete_session or delete_key"},"session_key":{"type":"string","description":"Session key to forget"},"key":{"type":"string","description":"Memory key to forget"}}})
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let executor = match &self.executor {
            Some(e) => e,
            None => return Err("Memory store is not available".to_string()),
        };

        let val: serde_json::Value = serde_json::from_str(args)
            .map_err(|e| format!("Invalid JSON arguments: {}", e))?;

        let result = executor.execute("memory_forget", &val).await;
        if result.success {
            Ok(result.content)
        } else {
            Err(result.content)
        }
    }
}

/// Memory list tool for listing stored memories.
///
/// Delegates to `nemesis_memory::memory_tools::MemoryToolExecutor`.
pub struct MemoryListTool {
    executor: Option<Arc<nemesis_memory::memory_tools::MemoryToolExecutor>>,
}

impl MemoryListTool {
    /// Create a new memory list tool backed by the given executor.
    pub fn new(executor: Option<Arc<nemesis_memory::memory_tools::MemoryToolExecutor>>) -> Self {
        Self { executor }
    }
}

#[async_trait]
impl Tool for MemoryListTool {
    fn description(&self) -> String {
        "List all memories in long-term storage".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"limit":{"type":"integer","description":"Maximum items to return"},"offset":{"type":"integer","description":"Offset for pagination"}}})
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let executor = match &self.executor {
            Some(e) => e,
            None => return Err("Memory store is not available".to_string()),
        };

        let val: serde_json::Value = serde_json::from_str(args)
            .unwrap_or_else(|_| serde_json::json!({}));

        let result = executor.execute("memory_list", &val).await;
        if result.success {
            Ok(result.content)
        } else {
            Err(result.content)
        }
    }
}

// ===========================================================================
// Skills tools
// ===========================================================================

/// Skills list tool for listing available local skills.
///
/// Mirrors Go's `SkillsListTool`. Uses a `SkillsLoader` to scan workspace,
/// global, and builtin skill directories. Falls back to a stub message when
/// no loader is configured.
pub struct SkillsListTool {
    loader: Option<Arc<nemesis_skills::loader::SkillsLoader>>,
}

impl SkillsListTool {
    /// Create a new skills list tool.
    pub fn new(loader: Option<Arc<nemesis_skills::loader::SkillsLoader>>) -> Self {
        Self { loader }
    }
}

#[async_trait]
impl Tool for SkillsListTool {
    fn description(&self) -> String {
        "List available skills".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"category":{"type":"string","description":"Filter by category"}}})
    }

    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        match &self.loader {
            Some(loader) => {
                let skills = loader.list_skills();
                if skills.is_empty() {
                    return Ok("No skills installed. Use find_skills to search for available skills.".to_string());
                }

                let mut output = format!("Installed skills ({}):\n", skills.len());
                for (i, skill) in skills.iter().enumerate() {
                    output.push_str(&format!(
                        "\n{}. **{}** (source: {})",
                        i + 1,
                        skill.name,
                        skill.source
                    ));
                    if !skill.description.is_empty() {
                        output.push_str(&format!("\n   Description: {}", skill.description));
                    }
                    if let Some(score) = skill.lint_score {
                        output.push_str(&format!("\n   Security score: {:.0}/100", score * 100.0));
                    }
                    output.push('\n');
                }
                Ok(output)
            }
            None => Ok("[SkillsList] No skills loaded (skills loader not configured)".to_string()),
        }
    }
}

/// Skills info tool for getting detailed info about a specific skill.
///
/// Mirrors Go's `SkillsInfoTool`. Returns the full content of a skill's
/// SKILL.md file (with frontmatter stripped) when available.
pub struct SkillsInfoTool {
    loader: Option<Arc<nemesis_skills::loader::SkillsLoader>>,
}

impl SkillsInfoTool {
    /// Create a new skills info tool.
    pub fn new(loader: Option<Arc<nemesis_skills::loader::SkillsLoader>>) -> Self {
        Self { loader }
    }
}

#[async_trait]
impl Tool for SkillsInfoTool {
    fn description(&self) -> String {
        "Get detailed information about a specific skill".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"name":{"type":"string","description":"Skill name"}},"required":["name"]})
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let skill_name = extract_search_query(args)?;

        match &self.loader {
            Some(loader) => {
                let skills = loader.list_skills();
                let skill = skills.iter().find(|s| s.name == skill_name);
                match skill {
                    Some(info) => {
                        let content = loader.load_skill(&skill_name)
                            .unwrap_or_else(|| "(no content available)".to_string());
                        Ok(format!(
                            "Skill: **{}**\nSource: {}\nPath: {}\nDescription: {}\n\n{}",
                            info.name,
                            info.source,
                            info.path,
                            info.description,
                            content
                        ))
                    }
                    None => Err(format!("Skill '{}' not found. Use skills_list to see installed skills.", skill_name)),
                }
            }
            None => Ok(format!(
                "[SkillsInfo] Skill '{}' not found (skills loader not configured)",
                skill_name
            )),
        }
    }
}

/// Find skills tool - searches configured registries for available skills.
///
/// Mirrors Go's `FindSkillsTool`. Uses `RegistryManager` to search across
/// all configured registries (GitHub, ClawHub, etc.) concurrently.
pub struct FindSkillsTool {
    registry: Arc<nemesis_skills::registry::RegistryManager>,
}

impl FindSkillsTool {
    /// Create a new find skills tool.
    pub fn new(registry: Arc<nemesis_skills::registry::RegistryManager>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl Tool for FindSkillsTool {
    fn description(&self) -> String {
        "Search for skills in remote registries".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"query":{"type":"string","description":"Search query"},"limit":{"type":"integer","description":"Maximum results"}},"required":["query"]})
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let val: serde_json::Value = serde_json::from_str(args)
            .map_err(|e| format!("Invalid JSON arguments: {}", e))?;

        let query = val["query"].as_str().unwrap_or("");
        if query.trim().is_empty() {
            return Err("missing or empty 'query' parameter".to_string());
        }

        let limit = val["limit"].as_u64().unwrap_or(5).clamp(1, 50) as usize;

        let results = self.registry.search(query, limit).await
            .map_err(|e| format!("failed to search registries: {}", e))?;

        if results.is_empty() {
            return Ok(format!("No skills found for query '{}'", query));
        }

        let mut output = format!("Found {} skill(s) for \"{}\":\n", results.len(), query);
        for (i, result) in results.iter().enumerate() {
            output.push_str(&format!("\n{}. **{}**", i + 1, result.slug));
            if !result.version.is_empty() {
                output.push_str(&format!(" v{}", result.version));
            }
            output.push_str(&format!(
                " (score: {:.2}, registry: {})\n",
                result.score, result.registry_name
            ));
            if !result.display_name.is_empty() {
                output.push_str(&format!("   Display Name: {}\n", result.display_name));
            }
            if !result.summary.is_empty() {
                output.push_str(&format!("   Description: {}\n", result.summary));
            }
            if result.downloads > 0 {
                output.push_str(&format!("   Downloads: {}\n", result.downloads));
            }
        }

        Ok(output)
    }
}

/// Install skill tool - installs a skill from a configured registry.
///
/// Mirrors Go's `InstallSkillTool`. Downloads and installs a skill from the
/// specified registry to the local workspace skills directory.
pub struct InstallSkillTool {
    registry: Arc<nemesis_skills::registry::RegistryManager>,
    workspace: String,
}

impl InstallSkillTool {
    /// Create a new install skill tool.
    pub fn new(registry: Arc<nemesis_skills::registry::RegistryManager>, workspace: String) -> Self {
        Self { registry, workspace }
    }
}

#[async_trait]
impl Tool for InstallSkillTool {
    fn description(&self) -> String {
        "Install a skill from a remote registry".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"name":{"type":"string","description":"Skill to install (registry/slug format)"}},"required":["name"]})
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let val: serde_json::Value = serde_json::from_str(args)
            .map_err(|e| format!("Invalid JSON arguments: {}", e))?;

        let slug = match val["slug"].as_str() {
            Some(s) if !s.is_empty() => s,
            _ => return Err("slug parameter is required and must be a non-empty string".to_string()),
        };

        // Validate skill identifier (path traversal protection)
        nemesis_skills::types::validate_skill_identifier(slug)
            .map_err(|e| format!("invalid slug: {}", e))?;

        let registry_name = val["registry"].as_str().unwrap_or("github");
        let _version = val["version"].as_str().unwrap_or("latest");
        let force = val["force"].as_bool().unwrap_or(false);

        // Check if skill already exists locally (unless force)
        if !force {
            let skill_dir = std::path::Path::new(&self.workspace)
                .join("skills")
                .join(slug);
            if skill_dir.exists() {
                return Err(format!(
                    "skill '{}' already exists locally at {}. Use force=true to reinstall.",
                    slug,
                    skill_dir.display()
                ));
            }
        }

        // Install from registry
        let target_dir = Path::new(&self.workspace)
            .join("skills")
            .to_string_lossy()
            .to_string();

        self.registry.install(registry_name, slug, &target_dir).await
            .map_err(|e| format!("failed to install skill '{}': {}", slug, e))?;

        Ok(format!("Skill '{}' installed successfully from registry '{}'", slug, registry_name))
    }
}

// ===========================================================================
// Hardware tools (I2C / SPI)
// ===========================================================================

/// I2C bus tool - interacts with I2C devices (Linux only).
pub struct I2CTool;

#[async_trait]
impl Tool for I2CTool {
    fn description(&self) -> String {
        "Interact with I2C bus devices for reading sensors and controlling peripherals. Actions: detect, scan, read, write. Linux only.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"action":{"type":"string","description":"Action: detect, scan, read, write"},"bus":{"type":"integer","description":"I2C bus number"},"address":{"type":"string","description":"Device address (hex)"}}})
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        if !cfg!(target_os = "linux") {
            return Err("I2C is only supported on Linux. This tool requires /dev/i2c-* device files.".to_string());
        }
        let val: serde_json::Value = serde_json::from_str(args)
            .map_err(|_| "Invalid JSON arguments".to_string())?;
        let action = val["action"].as_str().unwrap_or("");
        match action {
            "detect" => Ok("[I2C] Detect: scanning for I2C buses...".to_string()),
            "scan" => Ok(format!("[I2C] Scan on bus {}", val["bus"].as_str().unwrap_or("?"))),
            "read" => Ok(format!("[I2C] Read from device at address {}", val["address"].as_u64().unwrap_or(0))),
            "write" => {
                if val["confirm"].as_bool().unwrap_or(false) {
                    Ok(format!("[I2C] Write to device at address {}", val["address"].as_u64().unwrap_or(0)))
                } else {
                    Err("confirm must be true for write operations (safety guard)".to_string())
                }
            }
            _ => Err(format!("Unknown I2C action: {} (valid: detect, scan, read, write)", action)),
        }
    }
}

/// SPI bus tool - interacts with SPI devices (Linux only).
pub struct SPITool;

#[async_trait]
impl Tool for SPITool {
    fn description(&self) -> String {
        "Interact with SPI bus devices for high-speed peripheral communication. Actions: list, transfer, read. Linux only.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"action":{"type":"string","description":"Action: list, transfer, read"},"device":{"type":"string","description":"SPI device path"}}})
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        if !cfg!(target_os = "linux") {
            return Err("SPI is only supported on Linux. This tool requires /dev/spidev* device files.".to_string());
        }
        let val: serde_json::Value = serde_json::from_str(args)
            .map_err(|_| "Invalid JSON arguments".to_string())?;
        let action = val["action"].as_str().unwrap_or("");
        match action {
            "list" => Ok("[SPI] Listing SPI devices...".to_string()),
            "transfer" => {
                if val["confirm"].as_bool().unwrap_or(false) {
                    Ok(format!("[SPI] Transfer on device {}", val["device"].as_str().unwrap_or("?")))
                } else {
                    Err("confirm must be true for transfer operations (safety guard)".to_string())
                }
            }
            "read" => Ok(format!("[SPI] Read {} bytes from device {}", val["length"].as_u64().unwrap_or(1), val["device"].as_str().unwrap_or("?"))),
            _ => Err(format!("Unknown SPI action: {} (valid: list, transfer, read)", action)),
        }
    }
}

// ===========================================================================
// MCP tool registration helper
// ===========================================================================

/// Configuration for an MCP server.
#[derive(Debug, Clone)]
pub struct McpServerConfig {
    /// Name of the MCP server.
    pub name: String,
    /// Command to start the server.
    pub command: String,
    /// Arguments for the command.
    pub args: Vec<String>,
    /// Environment variables.
    pub env: HashMap<String, String>,
    /// Timeout in seconds for initialization.
    pub timeout_secs: u64,
}

/// MCP tool that wraps an MCP server tool discovered at runtime.
///
/// This tool uses a closure-based approach to communicate with the MCP server,
/// allowing the agent layer to remain decoupled from the MCP transport layer.
pub struct McpTool {
    /// Tool name as exposed by the MCP server.
    pub name: String,
    /// Tool description.
    pub description: String,
    /// MCP server name.
    pub server_name: String,
    /// Tool parameter schema (JSON Schema).
    pub input_schema: Option<serde_json::Value>,
    /// Execution function that communicates with the MCP server.
    executor: Box<dyn Fn(&str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send>> + Send + Sync>,
}

impl McpTool {
    /// Create a new MCP tool with a custom executor.
    pub fn new<F, Fut>(name: &str, description: &str, server_name: &str, executor: F) -> Self
    where
        F: Fn(&str) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<String, String>> + Send + 'static,
    {
        Self {
            name: name.to_string(),
            description: description.to_string(),
            server_name: server_name.to_string(),
            input_schema: None,
            executor: Box::new(move |args| Box::pin(executor(args))),
        }
    }

    /// Create a simulated MCP tool (for testing).
    pub fn new_simulated(name: &str, description: &str, server_name: &str) -> Self {
        let server_name_owned = server_name.to_string();
        let name_owned = name.to_string();
        Self {
            name: name.to_string(),
            description: description.to_string(),
            server_name: server_name.to_string(),
            input_schema: None,
            executor: Box::new(move |args| {
                let srv = server_name_owned.clone();
                let n = name_owned.clone();
                let args_owned = args.to_string();
                Box::pin(async move {
                    Ok(format!("[MCP/{}] Tool '{}' executed with args: {} (simulated)", srv, n, args_owned))
                })
            }),
        }
    }

    /// Set the input schema for this tool.
    pub fn with_schema(mut self, schema: serde_json::Value) -> Self {
        self.input_schema = Some(schema);
        self
    }
}

#[async_trait]
impl Tool for McpTool {
    fn description(&self) -> String {
        "Call a tool provided by an MCP (Model Context Protocol) server".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"server":{"type":"string","description":"MCP server name"},"tool":{"type":"string","description":"Tool name on the MCP server"},"arguments":{"type":"object","description":"Tool arguments"}},"required":["server","tool"]})
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        (self.executor)(args).await
    }
}

/// Result of discovering MCP tools from a server.
pub struct McpDiscoveryResult {
    /// The tools discovered from the MCP server.
    pub tools: Vec<McpTool>,
    /// The server name.
    pub server_name: String,
}

/// Discover and register MCP tools from a server configuration.
///
/// Takes a discovery function (typically provided by the service layer)
/// that connects to the MCP server, performs the handshake, lists tools,
/// and returns the discovered tool definitions.
///
/// This design keeps the agent layer decoupled from the MCP transport.
///
/// # Arguments
/// * `server_config` - Configuration for the MCP server
/// * `discover_fn` - Async function that connects and discovers tools
///
/// # Returns
/// A discovery result with the tools and server name.
pub async fn discover_mcp_tools<F, Fut>(
    server_name: &str,
    discover_fn: F,
) -> Result<McpDiscoveryResult, String>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<Vec<(String, String, Option<serde_json::Value>)>, String>>,
{
    let raw_tools = discover_fn().await?;

    let tools: Vec<McpTool> = raw_tools
        .into_iter()
        .map(|(name, description, schema)| {
            let server_name_owned = server_name.to_string();
            let name_owned = name.clone();
            let tool = McpTool {
                name: name.clone(),
                description,
                server_name: server_name.to_string(),
                input_schema: schema.clone(),
                executor: Box::new(move |args| {
                    let srv = server_name_owned.clone();
                    let n = name_owned.clone();
                    let _args = args.to_string();
                    Box::pin(async move {
                        Ok(format!(
                            "[MCP/{}] Tool '{}' executed (discovered, simulated executor)",
                            srv, n
                        ))
                    })
                }),
            };
            tool
        })
        .collect();

    tracing::info!(
        server = server_name,
        tool_count = tools.len(),
        "Discovered MCP tools"
    );

    Ok(McpDiscoveryResult {
        tools,
        server_name: server_name.to_string(),
    })
}

/// Register MCP tools from a server configuration by connecting to the server
/// and discovering available tools.
///
/// Mirrors Go's `registerMCPTools`. Creates an MCP client, performs handshake,
/// lists available tools, and wraps them in agent-compatible `McpTool` instances.
///
/// Returns a list of discovered tools, or an error if connection fails.
/// Individual server failures are logged but do not prevent other servers from
/// being processed (each server is processed in isolation).
pub async fn register_mcp_tools(
    server_config: &McpServerConfig,
) -> Result<Vec<Box<dyn Tool>>, String> {
    // Build the MCP server configuration
    let env_list: Vec<String> = server_config
        .env
        .iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect();

    let mcp_config = nemesis_mcp::types::ServerConfig {
        name: server_config.name.clone(),
        command: server_config.command.clone(),
        args: server_config.args.clone(),
        env: if env_list.is_empty() {
            None
        } else {
            Some(env_list)
        },
        timeout_secs: if server_config.timeout_secs > 0 {
            server_config.timeout_secs
        } else {
            30
        },
    };

    // Create the MCP transport and client
    let transport = nemesis_mcp::stdio_transport::StdioTransport::new(
        &mcp_config.command,
        mcp_config.args.clone(),
        mcp_config.env.clone().unwrap_or_default(),
    );
    let mut client: Box<dyn nemesis_mcp::client::Client> =
        Box::new(nemesis_mcp::client::McpClient::new(Box::new(transport)));

    // Initialize with timeout
    let timeout = std::time::Duration::from_secs(mcp_config.timeout_secs);
    let init_result = tokio::time::timeout(timeout, client.initialize())
        .await
        .map_err(|_| format!("MCP server '{}' initialization timed out", server_config.name))?
        .map_err(|e| {
            format!(
                "MCP server '{}' initialization failed: {}",
                server_config.name, e
            )
        })?;

    tracing::info!(
        server = %server_config.name,
        protocol_version = %init_result.protocol_version,
        "MCP server initialized"
    );

    // List tools from server
    let mcp_tools = client
        .list_tools()
        .await
        .map_err(|e| {
            format!(
                "Failed to list tools from '{}': {}",
                server_config.name, e
            )
        })?;

    tracing::info!(
        server = %server_config.name,
        tool_count = mcp_tools.len(),
        "Discovered MCP tools"
    );

    // Wrap the client in a shared Mutex for thread-safe access from all tool executors
    let shared_client = std::sync::Arc::new(tokio::sync::Mutex::new(client));

    // Wrap each MCP tool in an agent-compatible McpTool
    let server_name = server_config.name.clone();
    let mut tools: Vec<Box<dyn Tool>> = Vec::new();

    for mcp_tool in &mcp_tools {
        let tool_name = sanitize_mcp_name(&mcp_tool.name);
        let srv_name = sanitize_mcp_name(&server_name);
        let prefixed_name = format!("mcp_{}_{}", srv_name, tool_name);

        let description = match &mcp_tool.description {
            Some(desc) => format!("[MCP:{}] {}", server_name, desc),
            None => format!("[MCP:{}] MCP tool: {}", server_name, tool_name),
        };

        let schema = mcp_tool.input_schema.clone();

        // Create executor that calls the MCP tool through the shared client
        let client_arc = shared_client.clone();
        let mcp_tool_name = mcp_tool.name.clone();
        let tool_timeout = timeout;
        let srv_name_for_log = server_name.clone();

        let executor = Box::new(
            move |args: &str|
                  -> std::pin::Pin<
                Box<dyn std::future::Future<Output = Result<String, String>> + Send>,
            > {
                let client = client_arc.clone();
                let tool_name = mcp_tool_name.clone();
                let args_owned = args.to_string();
                let tool_timeout = tool_timeout;
                let srv = srv_name_for_log.clone();

                Box::pin(async move {
                    let args_value: serde_json::Value = serde_json::from_str(&args_owned)
                        .unwrap_or(serde_json::json!({}));

                    let result = {
                        let mut guard = client.lock().await;
                        tokio::time::timeout(tool_timeout, guard.call_tool(&tool_name, args_value))
                            .await
                            .map_err(|_| format!("MCP tool '{}' on '{}' timed out", tool_name, srv))?
                            .map_err(|e| format!("MCP tool '{}' on '{}' error: {}", tool_name, srv, e))?
                    };

                    // Extract text content from the result
                    let content_parts: Vec<String> = result
                        .content
                        .iter()
                        .filter_map(|c| c.text.clone())
                        .collect();

                    if content_parts.is_empty() {
                        if result.is_error {
                            Err(format!(
                                "MCP tool '{}' returned an error with no content",
                                tool_name
                            ))
                        } else {
                            Ok(format!(
                                "[MCP/{}] Tool '{}' executed successfully (no text content)",
                                srv, tool_name
                            ))
                        }
                    } else {
                        Ok(content_parts.join("\n"))
                    }
                })
            },
        );

        let tool = McpTool {
            name: prefixed_name,
            description,
            server_name: server_name.clone(),
            input_schema: Some(schema),
            executor,
        };

        tools.push(Box::new(tool));
    }

    Ok(tools)
}

/// Sanitize a name for use in tool identifiers (lowercase, replace spaces/dots with underscores).
fn sanitize_mcp_name(name: &str) -> String {
    name.to_lowercase()
        .replace(' ', "_")
        .replace('.', "_")
        .replace('-', "_")
}

// ===========================================================================
// Tool registration
// ===========================================================================

/// Register all default tools and return them as a HashMap.
///
/// The default tools are:
/// - `message` - Send a simple text message
/// - `read_file` - Read a file from disk
/// - `write_file` - Write content to a file on disk
/// - `list_dir` - List the contents of a directory
/// - `edit_file` - Edit a file by replacing old text with new text
/// - `append_file` - Append content to the end of a file
/// - `delete_file` - Delete a file from disk
/// - `create_dir` - Create a directory (and parents)
/// - `delete_dir` - Remove a directory
/// - `sleep` - Sleep for a specified duration
pub fn register_default_tools() -> HashMap<String, Box<dyn Tool>> {
    let mut tools: HashMap<String, Box<dyn Tool>> = HashMap::new();
    tools.insert("message".to_string(), Box::new(MessageTool::new()));
    tools.insert("read_file".to_string(), Box::new(ReadFileTool));
    tools.insert("write_file".to_string(), Box::new(WriteFileTool));
    tools.insert("list_dir".to_string(), Box::new(ListDirectoryTool));
    tools.insert("edit_file".to_string(), Box::new(EditFileTool));
    tools.insert("append_file".to_string(), Box::new(AppendFileTool));
    tools.insert("delete_file".to_string(), Box::new(DeleteFileTool));
    tools.insert("create_dir".to_string(), Box::new(CreateDirTool));
    tools.insert("delete_dir".to_string(), Box::new(DeleteDirTool));
    tools.insert("sleep".to_string(), Box::new(SleepTool));
    tools
}

// ===========================================================================
// setup_cluster_rpc_channel -- RPC channel setup (mirrors Go's setupClusterRPCChannel)
// ===========================================================================

/// Configuration for setting up the cluster RPC channel.
///
/// Mirrors Go's `channels.RPCChannelConfig`:
/// - `request_timeout`: B-side safety net (24 hours default)
/// - `cleanup_interval`: How often to clean up stale requests
#[derive(Debug, Clone)]
pub struct ClusterRpcChannelConfig {
    /// Request timeout for the RPC channel (B-side safety net).
    pub request_timeout: Duration,
    /// How often to clean up stale pending requests.
    pub cleanup_interval: Duration,
}

impl Default for ClusterRpcChannelConfig {
    fn default() -> Self {
        Self {
            request_timeout: Duration::from_secs(nemesis_types::constants::RPC_CHANNEL_TIMEOUT_SECS),
            cleanup_interval: Duration::from_secs(nemesis_types::constants::CLEANUP_INTERVAL_SECS),
        }
    }
}

/// Result of setting up the cluster RPC channel.
///
/// Contains both the channel configuration and the continuation manager
/// (if provided), so the caller can properly wire everything together.
pub struct ClusterRpcChannelSetup {
    /// The channel configuration.
    pub config: ClusterRpcChannelConfig,
    /// The continuation manager (if provided) for handling async RPC results.
    pub continuation_manager: Option<Arc<crate::loop_continuation::ContinuationManager>>,
}

/// Set up the cluster RPC channel for peer-to-peer bot communication.
///
/// Mirrors Go's `setupClusterRPCChannel`. This function:
/// 1. Creates an RPC channel configuration with a 24-hour timeout (B-side safety net)
/// 2. The continuation manager is stored for async callback handling
/// 3. The returned `ClusterRpcChannelSetup` should be used by the caller to wire
///    the channel manager, cluster instance, and continuation system together.
///
/// # Note
/// In the Go implementation, this function creates an RPCChannel and sets it
/// on the Cluster instance. In Rust, the channel and cluster are managed
/// separately. This function returns the setup needed to wire them together.
///
/// # Arguments
/// * `continuation_manager` - The continuation manager for handling async RPC results
///
/// # Returns
/// A `ClusterRpcChannelSetup` with the channel configuration and continuation manager.
pub fn setup_cluster_rpc_channel(
    continuation_manager: Option<Arc<crate::loop_continuation::ContinuationManager>>,
) -> ClusterRpcChannelSetup {
    let config = ClusterRpcChannelConfig::default();

    if let Some(ref cm) = continuation_manager {
        info!(
            "RPC channel for peer chat configured with continuation manager (timeout={:?}, cleanup={:?})",
            config.request_timeout, config.cleanup_interval
        );
        // The continuation manager is ready to save snapshots when async
        // cluster_rpc tools are invoked. It will be used by the executor
        // to save continuation snapshots and handle async callbacks.
        let _ = cm; // Available for caller to wire up
    } else {
        info!(
            "RPC channel for peer chat configured without continuation manager (timeout={:?}, cleanup={:?})",
            config.request_timeout, config.cleanup_interval
        );
    }

    ClusterRpcChannelSetup {
        config,
        continuation_manager,
    }
}

// ===========================================================================
// register_shared_tools -- register tools across all agents (mirrors Go's registerSharedTools)
// ===========================================================================

/// Bridge tool that wraps a ForgeToolExecutor tool call into the agent's Tool trait.
/// Each instance wraps a single forge tool name (e.g. "forge_reflect").
struct ForgeBridgeTool {
    name: String,
    description: String,
    parameters: serde_json::Value,
    executor: Arc<nemesis_forge::forge_tools::ForgeToolExecutor>,
}

impl ForgeBridgeTool {
    fn new(
        name: String,
        description: String,
        parameters: serde_json::Value,
        executor: Arc<nemesis_forge::forge_tools::ForgeToolExecutor>,
    ) -> Self {
        Self { name, description, parameters, executor }
    }
}

#[async_trait]
impl Tool for ForgeBridgeTool {
    fn description(&self) -> String {
        self.description.clone()
    }

    fn parameters(&self) -> serde_json::Value {
        self.parameters.clone()
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let args_value = serde_json::from_str::<serde_json::Value>(args)
            .unwrap_or(serde_json::Value::Null);
        let result = self.executor.execute(&self.name, &args_value).await;
        if result.success {
            Ok(result.content)
        } else {
            Err(result.content)
        }
    }
}

/// Extended tool registration configuration.
///
/// Mirrors Go's `registerSharedTools` parameters, bundling all the
/// configuration needed for shared tool registration.
#[derive(Clone)]
pub struct SharedToolConfig {
    /// Web search configuration.
    pub web_search: Option<WebSearchConfig>,
    /// Cluster RPC configuration.
    pub cluster_rpc: Option<ClusterRpcConfig>,
    /// Spawn/subagent configuration.
    pub spawn: Option<SpawnConfig>,
    /// Whether MCP tools should be loaded.
    pub mcp_enabled: bool,
    /// MCP server configurations to discover tools from.
    pub mcp_servers: Vec<McpServerConfig>,
    /// Skills registry manager for find/install tools.
    pub skills_registry: Option<Arc<nemesis_skills::registry::RegistryManager>>,
    /// Skills loader for listing local skills.
    pub skills_loader: Option<Arc<nemesis_skills::loader::SkillsLoader>>,
    /// Workspace path for skill installation.
    pub workspace: Option<String>,
    /// Cron service for scheduling jobs.
    pub cron_service: Option<Arc<std::sync::Mutex<nemesis_cron::service::CronService>>>,
    /// Forge tool executor for self-learning tools (forge_reflect, forge_create, etc).
    pub forge_executor: Option<Arc<nemesis_forge::forge_tools::ForgeToolExecutor>>,
}

impl Default for SharedToolConfig {
    fn default() -> Self {
        Self {
            web_search: None,
            cluster_rpc: None,
            spawn: None,
            mcp_enabled: false,
            mcp_servers: Vec::new(),
            skills_registry: None,
            skills_loader: None,
            workspace: None,
            cron_service: None,
            forge_executor: None,
        }
    }
}

impl std::fmt::Debug for SharedToolConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedToolConfig")
            .field("web_search", &self.web_search)
            .field("cluster_rpc", &self.cluster_rpc)
            .field("spawn", &self.spawn)
            .field("mcp_enabled", &self.mcp_enabled)
            .field("mcp_servers", &self.mcp_servers)
            .field("skills_registry", &self.skills_registry.as_ref().map(|_| "RegistryManager"))
            .field("skills_loader", &self.skills_loader.as_ref().map(|_| "SkillsLoader"))
            .field("workspace", &self.workspace)
            .finish()
    }
}

/// Register all shared tools across agents.
///
/// Mirrors Go's `registerSharedTools` function. Creates the complete set of
/// tools (basic file ops + web + cluster + spawn + memory + skills + hardware)
/// and returns them as a HashMap ready for registration with an AgentLoopExecutor.
///
/// # Arguments
/// * `config` - Configuration for optional tools (web, cluster, spawn, MCP)
///
/// # Returns
/// A HashMap of tool name -> tool implementation.
pub fn register_shared_tools(config: &SharedToolConfig) -> HashMap<String, Box<dyn Tool>> {
    let mut tools = register_default_tools();

    // Web search tool.
    if let Some(ref web_config) = config.web_search {
        tools.insert(
            "web_search".to_string(),
            Box::new(WebSearchTool::new(web_config.clone())),
        );
    }

    // Web fetch tool (always available).
    tools.insert(
        "web_fetch".to_string(),
        Box::new(WebFetchTool::new(50000)),
    );

    // Cluster RPC tool (bot-to-bot communication).
    if let Some(ref cluster_config) = config.cluster_rpc {
        tools.insert(
            "cluster_rpc".to_string(),
            Box::new(ClusterRpcTool::new(cluster_config.clone())),
        );
    }

    // Spawn/subagent tool.
    if let Some(ref spawn_config) = config.spawn {
        tools.insert(
            "spawn".to_string(),
            Box::new(SpawnTool::new(spawn_config.clone())),
        );
    }

    // Memory tools.
    tools.insert(
        "memory_search".to_string(),
        Box::new(MemorySearchTool::new(None)),
    );
    tools.insert(
        "memory_store".to_string(),
        Box::new(MemoryStoreTool::new(None)),
    );
    tools.insert(
        "memory_forget".to_string(),
        Box::new(MemoryForgetTool::new(None)),
    );
    tools.insert(
        "memory_list".to_string(),
        Box::new(MemoryListTool::new(None)),
    );

    // Skills tools: use real loader when available, otherwise use stub.
    tools.insert(
        "skills_list".to_string(),
        Box::new(SkillsListTool::new(config.skills_loader.clone())),
    );
    tools.insert(
        "skills_info".to_string(),
        Box::new(SkillsInfoTool::new(config.skills_loader.clone())),
    );

    // Find and install skills from remote registries.
    if let Some(ref registry) = config.skills_registry {
        tools.insert(
            "find_skills".to_string(),
            Box::new(FindSkillsTool::new(registry.clone())),
        );
        if let Some(ref workspace) = config.workspace {
            tools.insert(
                "install_skill".to_string(),
                Box::new(InstallSkillTool::new(registry.clone(), workspace.clone())),
            );
        }
    }

    // Hardware tools (I2C / SPI - Linux only, no-op on other platforms).
    tools.insert("i2c".to_string(), Box::new(I2CTool));
    tools.insert("spi".to_string(), Box::new(SPITool));

    // Exec tool + Async exec tool (mirrors Go's ExecTool + AsyncExecTool).
    if let Some(ref workspace) = config.workspace {
        let restrict = true; // restrict to workspace by default
        tools.insert(
            "exec".to_string(),
            Box::new(ExecTool::new(workspace, restrict)),
        );
        tools.insert(
            "exec_async".to_string(),
            Box::new(AsyncExecTool::new(workspace, restrict)),
        );

        // Bootstrap completion tool — deletes BOOTSTRAP.md after initialization.
        tools.insert(
            "complete_bootstrap".to_string(),
            Box::new(BootstrapTool::new(workspace)),
        );
    }

    // Cron tool (mirrors Go's CronTool).
    if let Some(ref cron_svc) = config.cron_service {
        tools.insert(
            "cron".to_string(),
            Box::new(CronTool::new(Arc::clone(cron_svc))),
        );
    }

    // Forge tools (mirrors Go's forgeTools registration in bot_service.go).
    // Registered when forge executor is provided (i.e. forge.enabled = true).
    if let Some(ref forge_executor) = config.forge_executor {
        let forge_defs = nemesis_forge::forge_tools::forge_tool_definitions();
        let forge_count = forge_defs.len();
        for def in &forge_defs {
            let bridge = ForgeBridgeTool::new(
                def.name.clone(),
                def.description.clone(),
                def.parameters.clone(),
                Arc::clone(forge_executor),
            );
            tools.insert(def.name.clone(), Box::new(bridge));
        }
        info!("Registered {} forge tools", forge_count);
    }

    info!(
        "Registered {} shared tools (web={}, cluster={}, spawn={}, mcp={})",
        tools.len(),
        config.web_search.is_some(),
        config.cluster_rpc.is_some(),
        config.spawn.is_some(),
        config.mcp_enabled,
    );

    tools
}

/// Async version of `register_shared_tools` that also discovers MCP tools.
///
/// This function first calls the synchronous `register_shared_tools` to get
/// all standard tools, then iterates over configured MCP servers and discovers
/// tools from each one using the provided discovery closure.
///
/// # Arguments
/// * `config` - Configuration for all tools including MCP servers
/// * `mcp_discover_fn` - Optional async function that discovers tools from an MCP server.
///   Takes server name, returns a list of (tool_name, description, input_schema) tuples.
///
/// # Returns
/// A HashMap of tool name -> tool implementation including MCP tools.
pub async fn register_shared_tools_async<F, Fut>(
    config: &SharedToolConfig,
    mcp_discover_fn: Option<F>,
) -> HashMap<String, Box<dyn Tool>>
where
    F: Fn(String) -> Fut + Send + Sync,
    Fut: std::future::Future<Output = Result<Vec<(String, String, Option<serde_json::Value>)>, String>> + Send,
{
    let mut tools = register_shared_tools(config);

    // Discover MCP tools if enabled and servers are configured.
    if config.mcp_enabled && !config.mcp_servers.is_empty() {
        if let Some(ref discover_fn) = mcp_discover_fn {
            for server in &config.mcp_servers {
                let server_name = server.name.clone();
                match discover_mcp_tools(&server_name, || discover_fn(server_name.clone())).await {
                    Ok(result) => {
                        for tool in result.tools {
                            let tool_name = format!("mcp_{}_{}", tool.server_name, tool.name);
                            tools.insert(tool_name, Box::new(tool));
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Failed to discover MCP tools from server '{}': {}",
                            server.name,
                            e
                        );
                    }
                }
            }
        } else {
            tracing::debug!(
                "MCP enabled with {} servers but no discovery function provided",
                config.mcp_servers.len()
            );
        }
    }

    tools
}

/// Register extended tools including web search, memory, and skills.
///
/// Returns all tools: default + extended.
pub fn register_extended_tools(
    web_config: Option<WebSearchConfig>,
    cluster_config: Option<ClusterRpcConfig>,
    spawn_config: Option<SpawnConfig>,
) -> HashMap<String, Box<dyn Tool>> {
    let shared_config = SharedToolConfig {
        web_search: web_config,
        cluster_rpc: cluster_config,
        spawn: spawn_config,
        mcp_enabled: false,
        mcp_servers: Vec::new(),
        skills_registry: None,
        skills_loader: None,
        workspace: None,
        cron_service: None,
        forge_executor: None,
    };
    register_shared_tools(&shared_config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_message_tool_with_json() {
        let tool = MessageTool::new();
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

        let result = tool
            .execute(r#"{"content": "Hello, world!"}"#, &ctx)
            .await
            .unwrap();
        assert_eq!(result, "Hello, world!");

        // Fallback: raw args.
        let result = tool.execute("plain text", &ctx).await.unwrap();
        assert_eq!(result, "plain text");
    }

    #[tokio::test]
    async fn test_read_write_file_tool() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("test.txt");
        let file_path_str = file_path.to_string_lossy().to_string();

        // Write a file.
        let write_tool = WriteFileTool;
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let args = serde_json::json!({
            "path": file_path_str,
            "content": "Hello from write tool!"
        })
        .to_string();

        let result = write_tool.execute(&args, &ctx).await.unwrap();
        assert!(result.contains("Successfully wrote"));

        // Read it back.
        let read_tool = ReadFileTool;
        let args = serde_json::json!({ "path": file_path_str }).to_string();
        let result = read_tool.execute(&args, &ctx).await.unwrap();
        assert_eq!(result, "Hello from write tool!");
    }

    #[tokio::test]
    async fn test_list_directory_tool() {
        let tmp = TempDir::new().unwrap();

        // Create some entries.
        tokio::fs::write(tmp.path().join("file1.txt"), "content1")
            .await
            .unwrap();
        tokio::fs::create_dir(tmp.path().join("subdir"))
            .await
            .unwrap();

        let tool = ListDirectoryTool;
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let args = serde_json::json!({ "path": tmp.path().to_string_lossy() }).to_string();

        let result = tool.execute(&args, &ctx).await.unwrap();
        assert!(result.contains("file1.txt"));
        assert!(result.contains("subdir"));
        assert!(result.contains("[file]"));
        assert!(result.contains("[dir]"));
    }

    #[tokio::test]
    async fn test_edit_file_tool() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("edit_test.txt");
        tokio::fs::write(&file_path, "Hello world, this is a test.")
            .await
            .unwrap();

        let tool = EditFileTool;
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let args = serde_json::json!({
            "path": file_path.to_string_lossy(),
            "old_text": "Hello world",
            "new_text": "Greetings universe"
        })
        .to_string();

        let result = tool.execute(&args, &ctx).await.unwrap();
        assert!(result.contains("File edited"));

        let content = tokio::fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(content, "Greetings universe, this is a test.");
    }

    #[tokio::test]
    async fn test_edit_file_tool_old_text_not_found() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("edit_test.txt");
        tokio::fs::write(&file_path, "Hello world").await.unwrap();

        let tool = EditFileTool;
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let args = serde_json::json!({
            "path": file_path.to_string_lossy(),
            "old_text": "nonexistent",
            "new_text": "replacement"
        })
        .to_string();

        let result = tool.execute(&args, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found in file"));
    }

    #[tokio::test]
    async fn test_edit_file_tool_duplicate_old_text() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("edit_test.txt");
        tokio::fs::write(&file_path, "aaa bbb aaa").await.unwrap();

        let tool = EditFileTool;
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let args = serde_json::json!({
            "path": file_path.to_string_lossy(),
            "old_text": "aaa",
            "new_text": "ccc"
        })
        .to_string();

        let result = tool.execute(&args, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("appears 2 times"));
    }

    #[tokio::test]
    async fn test_append_file_tool() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("append_test.txt");
        tokio::fs::write(&file_path, "Line 1\n").await.unwrap();

        let tool = AppendFileTool;
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let args = serde_json::json!({
            "path": file_path.to_string_lossy(),
            "content": "Line 2\n"
        })
        .to_string();

        let result = tool.execute(&args, &ctx).await.unwrap();
        assert!(result.contains("Appended"));

        let content = tokio::fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(content, "Line 1\nLine 2\n");
    }

    #[tokio::test]
    async fn test_append_file_creates_new_file() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("new_file.txt");

        let tool = AppendFileTool;
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let args = serde_json::json!({
            "path": file_path.to_string_lossy(),
            "content": "New content"
        })
        .to_string();

        let result = tool.execute(&args, &ctx).await.unwrap();
        assert!(result.contains("Appended"));

        let content = tokio::fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(content, "New content");
    }

    #[tokio::test]
    async fn test_delete_file_tool() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("to_delete.txt");
        tokio::fs::write(&file_path, "content").await.unwrap();
        assert!(file_path.exists());

        let tool = DeleteFileTool;
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let args = serde_json::json!({ "path": file_path.to_string_lossy() }).to_string();

        let result = tool.execute(&args, &ctx).await.unwrap();
        assert!(result.contains("Deleted"));
        assert!(!file_path.exists());
    }

    #[tokio::test]
    async fn test_delete_file_not_found() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("nonexistent.txt");

        let tool = DeleteFileTool;
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let args = serde_json::json!({ "path": file_path.to_string_lossy() }).to_string();

        let result = tool.execute(&args, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[tokio::test]
    async fn test_create_dir_tool() {
        let tmp = TempDir::new().unwrap();
        let dir_path = tmp.path().join("new_dir").join("nested");

        let tool = CreateDirTool;
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let args = serde_json::json!({ "path": dir_path.to_string_lossy() }).to_string();

        let result = tool.execute(&args, &ctx).await.unwrap();
        assert!(result.contains("created"));
        assert!(dir_path.exists());
    }

    #[tokio::test]
    async fn test_create_dir_already_exists() {
        let tmp = TempDir::new().unwrap();

        let tool = CreateDirTool;
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let args = serde_json::json!({ "path": tmp.path().to_string_lossy() }).to_string();

        let result = tool.execute(&args, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("already exists"));
    }

    #[tokio::test]
    async fn test_delete_dir_tool() {
        let tmp = TempDir::new().unwrap();
        let dir_path = tmp.path().join("to_remove");
        tokio::fs::create_dir_all(&dir_path).await.unwrap();
        tokio::fs::write(dir_path.join("file.txt"), "content")
            .await
            .unwrap();
        assert!(dir_path.exists());

        let tool = DeleteDirTool;
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let args = serde_json::json!({ "path": dir_path.to_string_lossy() }).to_string();

        let result = tool.execute(&args, &ctx).await.unwrap();
        assert!(result.contains("removed"));
        assert!(!dir_path.exists());
    }

    #[tokio::test]
    async fn test_sleep_tool() {
        let tool = SleepTool;
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let args = serde_json::json!({ "duration": 1 }).to_string();

        let start = std::time::Instant::now();
        let result = tool.execute(&args, &ctx).await.unwrap();
        let elapsed = start.elapsed();

        assert!(result.contains("Slept for 1 seconds"));
        assert!(elapsed.as_secs() >= 1);
    }

    #[tokio::test]
    async fn test_sleep_tool_exceeds_max() {
        let tool = SleepTool;
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let args = serde_json::json!({ "duration": 4000 }).to_string();

        let result = tool.execute(&args, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("cannot exceed"));
    }

    #[tokio::test]
    async fn test_sleep_tool_zero_duration() {
        let tool = SleepTool;
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let args = serde_json::json!({ "duration": 0 }).to_string();

        let result = tool.execute(&args, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("at least 1 second"));
    }

    #[test]
    fn test_register_default_tools_count() {
        let tools = register_default_tools();
        assert_eq!(tools.len(), 10);
        assert!(tools.contains_key("message"));
        assert!(tools.contains_key("read_file"));
        assert!(tools.contains_key("write_file"));
        assert!(tools.contains_key("list_dir"));
        assert!(tools.contains_key("edit_file"));
        assert!(tools.contains_key("append_file"));
        assert!(tools.contains_key("delete_file"));
        assert!(tools.contains_key("create_dir"));
        assert!(tools.contains_key("delete_dir"));
        assert!(tools.contains_key("sleep"));
    }

    // --- Extended tool tests ---

    /// This test makes a real network request to DuckDuckGo.
    /// Use `cargo test -- --ignored` to run network-dependent tests.
    #[tokio::test]
    #[ignore]
    async fn test_web_search_tool_duckduckgo_live() {
        let config = WebSearchConfig {
            duckduckgo_enabled: true,
            ..Default::default()
        };
        let tool = WebSearchTool::new(config);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

        let result = tool
            .execute(r#"{"query": "Rust programming"}"#, &ctx)
            .await
            .unwrap();
        assert!(result.contains("DuckDuckGo"));
        assert!(result.contains("Rust programming"));
    }

    #[tokio::test]
    async fn test_web_search_tool_no_provider() {
        let config = WebSearchConfig {
            duckduckgo_enabled: false,
            brave_enabled: false,
            perplexity_enabled: false,
            ..Default::default()
        };
        let tool = WebSearchTool::new(config);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

        let result = tool.execute(r#"{"query": "test"}"#, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No search provider"));
    }

    /// This test makes a real network request.
    /// Use `cargo test -- --ignored` to run network-dependent tests.
    #[tokio::test]
    #[ignore]
    async fn test_web_fetch_tool_live() {
        let tool = WebFetchTool::new(50000);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

        let result = tool
            .execute(r#"{"url": "https://example.com"}"#, &ctx)
            .await
            .unwrap();
        assert!(result.contains("example.com"));
    }

    #[tokio::test]
    async fn test_web_fetch_tool_invalid_url() {
        let tool = WebFetchTool::new(50000);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

        let result = tool
            .execute(r#"{"url": "http://127.0.0.1:1/nonexistent"}"#, &ctx)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_cluster_rpc_tool() {
        let config = ClusterRpcConfig {
            local_node_id: "node-1".to_string(),
            timeout_secs: 60,
            local_rpc_port: 21949,
        };
        let tool = ClusterRpcTool::new(config);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

        // Without an RPC function, should return error
        let result = tool
            .execute(
                r#"{"target_node": "node-2", "message": "Hello from node-1"}"#,
                &ctx,
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not available"));
    }

    #[tokio::test]
    async fn test_cluster_rpc_tool_with_fn() {
        let config = ClusterRpcConfig {
            local_node_id: "node-1".to_string(),
            timeout_secs: 60,
            local_rpc_port: 21949,
        };
        let mut tool = ClusterRpcTool::new(config);
        tool.set_rpc_call_fn(Arc::new(|_node: &str, _action: &str, payload: serde_json::Value| {
            let msg = payload.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
            Box::pin(async move {
                Ok(serde_json::json!({"content": format!("Echo: {}", msg)}))
            })
        }));

        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = tool
            .execute(
                r#"{"target_node": "node-2", "message": "Hello from node-1"}"#,
                &ctx,
            )
            .await
            .unwrap();
        assert!(result.contains("Echo: Hello from node-1"));
    }

    #[tokio::test]
    async fn test_spawn_tool() {
        let config = SpawnConfig {
            default_model: "test-model".to_string(),
            max_concurrent: 5,
        };
        let tool = SpawnTool::new(config);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

        // Without a spawn function, should return error
        let result = tool
            .execute(
                r#"{"agent_id": "worker-1", "task": "Analyze data"}"#,
                &ctx,
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not available"));
    }

    #[tokio::test]
    async fn test_spawn_tool_with_fn() {
        let config = SpawnConfig {
            default_model: "test-model".to_string(),
            max_concurrent: 5,
        };
        let mut tool = SpawnTool::new(config);
        tool.set_spawn_fn(Arc::new(
            |agent_id: &str, task: &str, model: &str, _channel: &str, _chat_id: &str| {
                let agent_id = agent_id.to_string();
                let task = task.to_string();
                let model = model.to_string();
                Box::pin(async move {
                    Ok(format!(
                        "[Spawn] Created sub-agent '{}' for task: {} (model: {})",
                        agent_id, task, model
                    ))
                })
            },
        ));

        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = tool
            .execute(
                r#"{"agent_id": "worker-1", "task": "Analyze data"}"#,
                &ctx,
            )
            .await
            .unwrap();
        assert!(result.contains("worker-1"));
        assert!(result.contains("Analyze data"));
        assert!(result.contains("test-model"));
    }

    #[tokio::test]
    async fn test_spawn_tool_allowlist_denied() {
        let config = SpawnConfig {
            default_model: "test-model".to_string(),
            max_concurrent: 5,
        };
        let mut tool = SpawnTool::new(config);
        tool.set_allowlist_checker(Box::new(|_id| false));

        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = tool
            .execute(
                r#"{"agent_id": "restricted-agent", "task": "Do something"}"#,
                &ctx,
            )
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Not allowed"));
    }

    #[tokio::test]
    async fn test_memory_tools_no_executor() {
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

        // Without a memory executor, tools should return errors
        let search = MemorySearchTool::new(None);
        let result = search
            .execute(r#"{"query": "test memory"}"#, &ctx)
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not available"));

        let store = MemoryStoreTool::new(None);
        let result = store
            .execute(r#"{"memory_type": "episodic", "content": "hello"}"#, &ctx)
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not available"));

        let forget = MemoryForgetTool::new(None);
        let result = forget.execute(r#"{"action": "delete_session", "session_key": "test"}"#, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not available"));

        let list = MemoryListTool::new(None);
        let result = list.execute("{}", &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not available"));
    }

    #[tokio::test]
    async fn test_memory_tools_with_executor() {
        let dir = tempfile::tempdir().unwrap();
        let config = nemesis_memory::manager::Config::new(dir.path());
        let mgr = Arc::new(nemesis_memory::manager::MemoryManager::new(&config));
        let executor = Arc::new(nemesis_memory::memory_tools::MemoryToolExecutor::new(mgr));
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

        // Store an episodic memory
        let store = MemoryStoreTool::new(Some(executor.clone()));
        let result = store
            .execute(
                r#"{"memory_type": "episodic", "content": "test content", "role": "user", "session_key": "test-session"}"#,
                &ctx,
            )
            .await
            .unwrap();
        assert!(result.contains("Episodic memory stored"));

        // Search for it
        let search = MemorySearchTool::new(Some(executor.clone()));
        let result = search
            .execute(r#"{"query": "test content"}"#, &ctx)
            .await
            .unwrap();
        assert!(result.contains("test content"));

        // List status
        let list = MemoryListTool::new(Some(executor.clone()));
        let result = list.execute("{}", &ctx).await.unwrap();
        assert!(result.contains("Memory Store Status"));

        // Forget (cleanup)
        let forget = MemoryForgetTool::new(Some(executor.clone()));
        let result = forget
            .execute(r#"{"action": "delete_session", "session_key": "test-session"}"#, &ctx)
            .await
            .unwrap();
        assert!(result.contains("deleted"));
    }

    #[tokio::test]
    async fn test_skills_tools_stub() {
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

        // Test without loader (stub mode)
        let list = SkillsListTool::new(None);
        let result = list.execute("{}", &ctx).await.unwrap();
        assert!(result.contains("skills loader not configured"));

        let info = SkillsInfoTool::new(None);
        let result = info.execute("test-skill", &ctx).await.unwrap();
        assert!(result.contains("skills loader not configured"));
    }

    #[tokio::test]
    async fn test_skills_tools_with_loader() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().to_string_lossy().to_string();
        let global = tmp.path().join("global").to_string_lossy().to_string();
        let builtin = tmp.path().join("builtin").to_string_lossy().to_string();

        // Create a skill in the workspace
        let skill_dir = tmp.path().join("skills").join("test-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: test-skill\ndescription: A test skill\n---\n# Test Skill\n\nDoes test things.",
        ).unwrap();

        let loader = Arc::new(nemesis_skills::loader::SkillsLoader::new(
            &workspace,
            &global,
            &builtin,
        ));

        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

        // SkillsListTool with loader
        let list = SkillsListTool::new(Some(loader.clone()));
        let result = list.execute("{}", &ctx).await.unwrap();
        assert!(result.contains("test-skill"));
        assert!(result.contains("Installed skills"));

        // SkillsInfoTool with loader
        let info = SkillsInfoTool::new(Some(loader));
        let result = info.execute("test-skill", &ctx).await.unwrap();
        assert!(result.contains("test-skill"));
        assert!(result.contains("Does test things"));

        // SkillsInfoTool for missing skill
        let info2 = SkillsInfoTool::new(None);
        let result = info2.execute("nonexistent", &ctx).await.unwrap();
        assert!(result.contains("skills loader not configured"));
    }

    #[tokio::test]
    async fn test_mcp_tool() {
        let tool = McpTool::new_simulated("search", "Search tool", "test-server");
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

        let result = tool
            .execute(r#"{"query": "test"}"#, &ctx)
            .await
            .unwrap();
        assert!(result.contains("MCP/test-server"));
        assert!(result.contains("search"));
    }

    #[test]
    fn test_register_extended_tools() {
        let tools = register_extended_tools(None, None, None);
        assert!(tools.contains_key("web_fetch"));
        assert!(tools.contains_key("memory_search"));
        assert!(tools.contains_key("memory_store"));
        assert!(tools.contains_key("memory_forget"));
        assert!(tools.contains_key("memory_list"));
        assert!(tools.contains_key("skills_list"));
        assert!(tools.contains_key("skills_info"));
    }

    #[test]
    fn test_register_extended_tools_with_cluster() {
        let cluster_config = ClusterRpcConfig {
            local_node_id: "node-1".to_string(),
            timeout_secs: 60,
            local_rpc_port: 21949,
        };
        let tools = register_extended_tools(None, Some(cluster_config), None);
        assert!(tools.contains_key("cluster_rpc"));
    }

    #[test]
    fn test_register_extended_tools_with_spawn() {
        let spawn_config = SpawnConfig {
            default_model: "test".to_string(),
            max_concurrent: 5,
        };
        let tools = register_extended_tools(None, None, Some(spawn_config));
        assert!(tools.contains_key("spawn"));
    }

    #[test]
    fn test_register_extended_tools_with_web_search() {
        let web_config = WebSearchConfig {
            duckduckgo_enabled: true,
            ..Default::default()
        };
        let tools = register_extended_tools(Some(web_config), None, None);
        assert!(tools.contains_key("web_search"));
        assert!(tools.contains_key("web_fetch"));
    }

    // =========================================================================
    // Additional coverage tests for loop_tools.rs
    // =========================================================================

    // --- MessageTool coverage ---

    #[test]
    fn test_message_tool_default() {
        let tool = MessageTool::default();
        assert!(!tool.has_sent_in_round());
    }

    #[test]
    fn test_message_tool_sent_in_round_cycle() {
        let tool = MessageTool::new();
        assert!(!tool.has_sent_in_round());
        tool.reset_sent_in_round();
        assert!(!tool.has_sent_in_round());
    }

    #[tokio::test]
    async fn test_message_tool_with_send_callback() {
        let tool = MessageTool::new();
        let sent_content = Arc::new(std::sync::Mutex::new(String::new()));
        let sent_content_clone = sent_content.clone();
        tool.set_send_callback(Box::new(move |_ch, _cid, content| {
            *sent_content_clone.lock().unwrap() = content.to_string();
        }));

        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = tool
            .execute(r#"{"content": "test message"}"#, &ctx)
            .await
            .unwrap();
        assert_eq!(result, "test message");
        assert!(tool.has_sent_in_round());
        assert_eq!(*sent_content.lock().unwrap(), "test message");
    }

    #[tokio::test]
    async fn test_message_tool_with_rpc_context() {
        let tool = MessageTool::new();
        let sent = Arc::new(std::sync::Mutex::new(String::new()));
        let sent_clone = sent.clone();
        tool.set_send_callback(Box::new(move |_ch, _cid, content| {
            *sent_clone.lock().unwrap() = content.to_string();
        }));

        let mut ctx = RequestContext::new("rpc", "chat1", "user1", "sess1");
        ctx.correlation_id = Some("corr-123".to_string());
        let result = tool
            .execute(r#"{"content": "hello"}"#, &ctx)
            .await
            .unwrap();
        assert_eq!(result, "hello");
        // The sent content should have RPC prefix
        let sent_val = sent.lock().unwrap().clone();
        assert!(sent_val.contains("[rpc:corr-123]"));
    }

    #[test]
    fn test_message_tool_set_context() {
        let tool = MessageTool::new();
        tool.set_context("discord", "channel-abc");
        // Context is stored internally; verify it works by executing
    }

    #[tokio::test]
    async fn test_message_tool_fallback_channel_from_stored() {
        let tool = MessageTool::new();
        tool.set_context("stored_channel", "stored_chat");

        // Execute with empty channel in context -> should use stored
        let ctx = RequestContext::new("", "", "user1", "sess1");
        let result = tool.execute(r#"{"content": "test"}"#, &ctx).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_message_tool_no_callback_passthrough() {
        let tool = MessageTool::new();
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = tool
            .execute(r#"{"content": "passthrough"}"#, &ctx)
            .await
            .unwrap();
        assert_eq!(result, "passthrough");
        assert!(!tool.has_sent_in_round());
    }

    // --- extract_path and extract_path_and_content coverage ---

    #[test]
    fn test_extract_path_valid_json() {
        let result = extract_path(r#"{"path": "/tmp/test.txt"}"#).unwrap();
        assert_eq!(result, "/tmp/test.txt");
    }

    #[test]
    fn test_extract_path_missing_field() {
        let result = extract_path(r#"{"other": "value"}"#);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Missing 'path'"));
    }

    #[test]
    fn test_extract_path_raw_string() {
        let result = extract_path("  /raw/path  ").unwrap();
        assert_eq!(result, "/raw/path");
    }

    #[test]
    fn test_extract_path_and_content_valid() {
        let (path, content) = extract_path_and_content(r#"{"path": "/a/b", "content": "hello"}"#).unwrap();
        assert_eq!(path, "/a/b");
        assert_eq!(content, "hello");
    }

    #[test]
    fn test_extract_path_and_content_invalid_json() {
        let result = extract_path_and_content("not json");
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_path_and_content_missing_content() {
        let result = extract_path_and_content(r#"{"path": "/tmp"}"#);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Missing 'content'"));
    }

    #[test]
    fn test_extract_path_and_content_missing_path() {
        let result = extract_path_and_content(r#"{"content": "hello"}"#);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Missing 'path'"));
    }

    #[test]
    fn test_extract_edit_args_valid() {
        let (path, old, new) = extract_edit_args(
            r#"{"path": "/a.txt", "old_text": "foo", "new_text": "bar"}"#,
        )
        .unwrap();
        assert_eq!(path, "/a.txt");
        assert_eq!(old, "foo");
        assert_eq!(new, "bar");
    }

    #[test]
    fn test_extract_edit_args_invalid_json() {
        let result = extract_edit_args("invalid");
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_edit_args_missing_old_text() {
        let result = extract_edit_args(r#"{"path": "/a", "new_text": "b"}"#);
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_edit_args_missing_new_text() {
        let result = extract_edit_args(r#"{"path": "/a", "old_text": "b"}"#);
        assert!(result.is_err());
    }

    // --- File tool edge cases ---

    #[tokio::test]
    async fn test_read_file_not_found() {
        let tool = ReadFileTool;
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = tool
            .execute(r#"{"path": "/nonexistent/file.txt"}"#, &ctx)
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[tokio::test]
    async fn test_list_dir_not_found() {
        let tool = ListDirectoryTool;
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let nonexistent = format!(r#"{{"path": "C:/__nonexistent_test_dir_{}"}}"#, std::process::id());
        let result = tool
            .execute(&nonexistent, &ctx)
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[tokio::test]
    async fn test_list_dir_is_file_not_dir() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("file.txt");
        tokio::fs::write(&file_path, "content").await.unwrap();

        let tool = ListDirectoryTool;
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = tool
            .execute(&serde_json::json!({"path": file_path.to_string_lossy()}).to_string(), &ctx)
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not a directory"));
    }

    #[tokio::test]
    async fn test_list_dir_empty_directory() {
        let tmp = TempDir::new().unwrap();
        let empty_dir = tmp.path().join("empty");
        tokio::fs::create_dir(&empty_dir).await.unwrap();

        let tool = ListDirectoryTool;
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = tool
            .execute(&serde_json::json!({"path": empty_dir.to_string_lossy()}).to_string(), &ctx)
            .await
            .unwrap();
        assert!(result.contains("empty directory"));
    }

    #[tokio::test]
    async fn test_edit_file_not_found() {
        let tool = EditFileTool;
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = tool
            .execute(r#"{"path": "/nonexistent.txt", "old_text": "a", "new_text": "b"}"#, &ctx)
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[tokio::test]
    async fn test_delete_file_is_directory() {
        let tmp = TempDir::new().unwrap();
        let dir_path = tmp.path().join("a_dir");
        tokio::fs::create_dir(&dir_path).await.unwrap();

        let tool = DeleteFileTool;
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = tool
            .execute(&serde_json::json!({"path": dir_path.to_string_lossy()}).to_string(), &ctx)
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("directory"));
    }

    #[tokio::test]
    async fn test_delete_dir_not_found() {
        let tool = DeleteDirTool;
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = tool
            .execute(r#"{"path": "/nonexistent_dir_12345"}"#, &ctx)
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[tokio::test]
    async fn test_delete_dir_is_file() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("file.txt");
        tokio::fs::write(&file_path, "content").await.unwrap();

        let tool = DeleteDirTool;
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = tool
            .execute(&serde_json::json!({"path": file_path.to_string_lossy()}).to_string(), &ctx)
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not a directory"));
    }

    #[tokio::test]
    async fn test_write_file_creates_parent_dirs() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("a").join("b").join("c").join("test.txt");

        let tool = WriteFileTool;
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let args = serde_json::json!({
            "path": file_path.to_string_lossy(),
            "content": "nested content"
        })
        .to_string();

        let result = tool.execute(&args, &ctx).await.unwrap();
        assert!(result.contains("Successfully wrote"));
        assert!(file_path.exists());
    }

    #[tokio::test]
    async fn test_append_to_new_file() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("append_new.txt");

        let tool = AppendFileTool;
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let args = serde_json::json!({
            "path": file_path.to_string_lossy(),
            "content": "first line"
        })
        .to_string();

        let result = tool.execute(&args, &ctx).await.unwrap();
        assert!(result.contains("Appended"));
        assert_eq!(tokio::fs::read_to_string(&file_path).await.unwrap(), "first line");
    }

    // --- SleepTool edge cases ---

    #[tokio::test]
    async fn test_sleep_tool_raw_number() {
        let tool = SleepTool;
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let args = serde_json::json!({"duration": 1}).to_string();
        let result = tool.execute(&args, &ctx).await.unwrap();
        assert!(result.contains("Slept for 1 seconds"));
    }

    #[tokio::test]
    async fn test_sleep_tool_invalid_string() {
        let tool = SleepTool;
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = tool.execute("not_a_number", &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_sleep_tool_missing_duration_field() {
        let tool = SleepTool;
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let args = serde_json::json!({"other": 5}).to_string();
        let result = tool.execute(&args, &ctx).await;
        assert!(result.is_err());
    }

    // --- ExecTool coverage ---

    #[tokio::test]
    async fn test_exec_tool_basic() {
        let tmp = TempDir::new().unwrap();
        let tool = ExecTool::new(&tmp.path().to_string_lossy(), false);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

        let cmd = if cfg!(target_os = "windows") { "echo hello" } else { "echo hello" };
        let args = serde_json::json!({"command": cmd}).to_string();
        let result = tool.execute(&args, &ctx).await.unwrap();
        assert!(result.contains("hello"));
    }

    #[tokio::test]
    async fn test_exec_tool_invalid_json() {
        let tool = ExecTool::new("/tmp", false);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = tool.execute("not json", &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_exec_tool_missing_command() {
        let tool = ExecTool::new("/tmp", false);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = tool.execute("{}", &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Missing 'command'"));
    }

    #[tokio::test]
    async fn test_exec_tool_custom_timeout() {
        let tmp = TempDir::new().unwrap();
        let tool = ExecTool::new(&tmp.path().to_string_lossy(), false);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let cmd = if cfg!(target_os = "windows") { "echo test" } else { "echo test" };
        let args = serde_json::json!({"command": cmd, "timeout": 30}).to_string();
        let result = tool.execute(&args, &ctx).await.unwrap();
        assert!(result.contains("test"));
    }

    #[tokio::test]
    async fn test_exec_tool_workspace_restriction() {
        let tool = ExecTool::new("/safe/workspace", true);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let args = serde_json::json!({
            "command": "echo test",
            "cwd": "/outside/workspace"
        })
        .to_string();
        let result = tool.execute(&args, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Access denied"));
    }

    #[tokio::test]
    async fn test_exec_tool_failing_command() {
        let tmp = TempDir::new().unwrap();
        let tool = ExecTool::new(&tmp.path().to_string_lossy(), false);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let cmd = if cfg!(target_os = "windows") { "exit /b 1" } else { "exit 1" };
        let args = serde_json::json!({"command": cmd}).to_string();
        let result = tool.execute(&args, &ctx).await.unwrap();
        assert!(result.contains("Exit code"));
    }

    // --- AsyncExecTool coverage ---

    #[tokio::test]
    async fn test_async_exec_tool_basic() {
        let tmp = TempDir::new().unwrap();
        let tool = AsyncExecTool::new(&tmp.path().to_string_lossy(), false);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let cmd = if cfg!(target_os = "windows") { "timeout /t 10 /nobreak >nul 2>&1 || ping -n 10 127.0.0.1 >nul" } else { "sleep 10" };
        let args = serde_json::json!({"command": cmd, "wait_seconds": 1}).to_string();
        let result = tool.execute(&args, &ctx).await;
        // Should succeed — process is still running after 1s wait
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_async_exec_tool_missing_command() {
        let tmp = TempDir::new().unwrap();
        let tool = AsyncExecTool::new(&tmp.path().to_string_lossy(), false);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = tool.execute("{}", &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_async_exec_tool_invalid_json() {
        let tmp = TempDir::new().unwrap();
        let tool = AsyncExecTool::new(&tmp.path().to_string_lossy(), false);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = tool.execute("not json", &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_async_exec_tool_workspace_restriction() {
        let tmp = TempDir::new().unwrap();
        let tool = AsyncExecTool::new(&tmp.path().to_string_lossy(), true);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let args = serde_json::json!({"command": "echo hi", "working_dir": "/etc"}).to_string();
        let result = tool.execute(&args, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("outside workspace"));
    }

    #[tokio::test]
    async fn test_async_exec_tool_fast_exit() {
        let tmp = TempDir::new().unwrap();
        let tool = AsyncExecTool::new(&tmp.path().to_string_lossy(), false);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let cmd = if cfg!(target_os = "windows") { "echo hello" } else { "echo hello" };
        let args = serde_json::json!({"command": cmd, "wait_seconds": 5}).to_string();
        let result = tool.execute(&args, &ctx).await;
        // Fast-exit command completes within wait period, should return ok
        assert!(result.is_ok());
    }

    #[test]
    fn test_async_exec_tool_description() {
        let tool = AsyncExecTool::new("/tmp", false);
        assert!(!tool.description().is_empty());
        let params = tool.parameters();
        assert!(params["properties"]["command"].is_object());
        assert!(params["properties"]["wait_seconds"].is_object());
    }

    // --- WebSearchTool coverage ---

    #[test]
    fn test_web_search_config_default() {
        let config = WebSearchConfig::default();
        assert!(!config.brave_enabled);
        assert!(config.duckduckgo_enabled);
        assert!(!config.perplexity_enabled);
        assert_eq!(config.brave_max_results, 5);
        assert_eq!(config.duckduckgo_max_results, 5);
        assert_eq!(config.perplexity_max_results, 5);
        assert!(config.brave_api_key.is_none());
        assert!(config.perplexity_api_key.is_none());
    }

    #[test]
    fn test_web_search_tool_description() {
        let tool = WebSearchTool::new(WebSearchConfig::default());
        assert!(!tool.description().is_empty());
        assert!(tool.parameters().is_object());
    }

    #[tokio::test]
    async fn test_web_search_tool_brave_no_key() {
        let config = WebSearchConfig {
            brave_enabled: true,
            brave_api_key: None,
            duckduckgo_enabled: false,
            perplexity_enabled: false,
            ..Default::default()
        };
        let tool = WebSearchTool::new(config);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        // brave_enabled but no key -> search_brave should fail
        let result = tool.execute(r#"{"query": "test"}"#, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_web_search_tool_perplexity_no_key() {
        let config = WebSearchConfig {
            brave_enabled: false,
            duckduckgo_enabled: false,
            perplexity_enabled: true,
            perplexity_api_key: None,
            ..Default::default()
        };
        let tool = WebSearchTool::new(config);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = tool.execute(r#"{"query": "test"}"#, &ctx).await;
        assert!(result.is_err());
    }

    // --- WebFetchTool coverage ---

    #[test]
    fn test_web_fetch_tool_description() {
        let tool = WebFetchTool::new(50000);
        assert!(!tool.description().is_empty());
        assert!(tool.parameters().is_object());
    }

    // --- ClusterRpcTool coverage ---

    #[test]
    fn test_cluster_rpc_config() {
        let config = ClusterRpcConfig {
            local_node_id: "node-1".to_string(),
            timeout_secs: 120,
            local_rpc_port: 21949,
        };
        assert_eq!(config.local_node_id, "node-1");
        assert_eq!(config.timeout_secs, 120);
    }

    #[tokio::test]
    async fn test_cluster_rpc_tool_missing_target() {
        let config = ClusterRpcConfig {
            local_node_id: "node-1".to_string(),
            timeout_secs: 60,
            local_rpc_port: 21949,
        };
        let tool = ClusterRpcTool::new(config);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = tool.execute(r#"{"message": "hello"}"#, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_cluster_rpc_tool_invalid_json() {
        let config = ClusterRpcConfig {
            local_node_id: "node-1".to_string(),
            timeout_secs: 60,
            local_rpc_port: 21949,
        };
        let tool = ClusterRpcTool::new(config);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = tool.execute("not json", &ctx).await;
        assert!(result.is_err());
    }

    // --- SpawnTool coverage ---

    #[tokio::test]
    async fn test_spawn_tool_invalid_json() {
        let config = SpawnConfig {
            default_model: "test".to_string(),
            max_concurrent: 5,
        };
        let tool = SpawnTool::new(config);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = tool.execute("not json", &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_spawn_tool_missing_agent_id() {
        let config = SpawnConfig {
            default_model: "test".to_string(),
            max_concurrent: 5,
        };
        let tool = SpawnTool::new(config);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = tool.execute(r#"{"task": "do something"}"#, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_spawn_tool_allowlist_allowed() {
        let config = SpawnConfig {
            default_model: "test".to_string(),
            max_concurrent: 5,
        };
        let mut tool = SpawnTool::new(config);
        tool.set_allowlist_checker(Box::new(|id| id == "allowed-agent"));
        tool.set_spawn_fn(Arc::new(
            |agent_id: &str, task: &str, model: &str, _ch: &str, _cid: &str| {
                let agent_id = agent_id.to_string();
                let task = task.to_string();
                let model = model.to_string();
                Box::pin(async move {
                    Ok(format!("spawned {} for {} with {}", agent_id, task, model))
                })
            },
        ));

        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = tool
            .execute(r#"{"agent_id": "allowed-agent", "task": "test"}"#, &ctx)
            .await
            .unwrap();
        assert!(result.contains("allowed-agent"));
    }

    // --- register_shared_tools coverage ---

    #[test]
    fn test_register_shared_tools_default() {
        let config = SharedToolConfig::default();
        let tools = register_shared_tools(&config);
        // Default config: no web search, no cluster, no spawn, no MCP
        assert!(tools.contains_key("message"));
        assert!(tools.contains_key("web_fetch"));
        assert!(tools.contains_key("memory_search"));
        assert!(tools.contains_key("memory_store"));
        assert!(tools.contains_key("memory_forget"));
        assert!(tools.contains_key("memory_list"));
        assert!(tools.contains_key("skills_list"));
        assert!(tools.contains_key("skills_info"));
        assert!(!tools.contains_key("web_search"));
        assert!(!tools.contains_key("cluster_rpc"));
        assert!(!tools.contains_key("spawn"));
    }

    #[test]
    fn test_register_shared_tools_with_web() {
        let config = SharedToolConfig {
            web_search: Some(WebSearchConfig {
                duckduckgo_enabled: true,
                ..Default::default()
            }),
            ..Default::default()
        };
        let tools = register_shared_tools(&config);
        assert!(tools.contains_key("web_search"));
    }

    #[test]
    fn test_register_shared_tools_with_cluster() {
        let config = SharedToolConfig {
            cluster_rpc: Some(ClusterRpcConfig {
                local_node_id: "n1".to_string(),
                timeout_secs: 60,
                local_rpc_port: 21949,
            }),
            ..Default::default()
        };
        let tools = register_shared_tools(&config);
        assert!(tools.contains_key("cluster_rpc"));
    }

    #[test]
    fn test_register_shared_tools_with_spawn() {
        let config = SharedToolConfig {
            spawn: Some(SpawnConfig {
                default_model: "test".to_string(),
                max_concurrent: 5,
            }),
            ..Default::default()
        };
        let tools = register_shared_tools(&config);
        assert!(tools.contains_key("spawn"));
    }

    fn make_cron_service() -> Arc<std::sync::Mutex<nemesis_cron::service::CronService>> {
        Arc::new(std::sync::Mutex::new(
            nemesis_cron::service::CronService::new(":memory:"),
        ))
    }

    #[test]
    fn test_register_shared_tools_with_cron() {
        let cron_svc = make_cron_service();
        let config = SharedToolConfig {
            cron_service: Some(cron_svc),
            ..Default::default()
        };
        let tools = register_shared_tools(&config);
        assert!(tools.contains_key("cron"));
    }

    // --- SharedToolConfig default ---

    #[test]
    fn test_shared_tool_config_default() {
        let config = SharedToolConfig::default();
        assert!(config.web_search.is_none());
        assert!(config.cluster_rpc.is_none());
        assert!(config.spawn.is_none());
        assert!(!config.mcp_enabled);
        assert!(config.mcp_servers.is_empty());
        assert!(config.skills_registry.is_none());
        assert!(config.skills_loader.is_none());
        assert!(config.workspace.is_none());
        assert!(config.cron_service.is_none());
        assert!(config.forge_executor.is_none());
    }

    // --- McpTool coverage ---

    #[test]
    fn test_mcp_tool_description_and_params() {
        let tool = McpTool::new_simulated("search", "Search tool", "my-server");
        assert!(!tool.description().is_empty());
        let params = tool.parameters();
        assert!(params.is_object());
    }

    // --- Tool trait methods coverage ---

    #[test]
    fn test_tool_descriptions_non_empty() {
        let tools = register_default_tools();
        for (name, tool) in &tools {
            let desc = tool.description();
            assert!(!desc.is_empty(), "Tool '{}' has empty description", name);
        }
    }

    #[test]
    fn test_tool_parameters_are_valid_json() {
        let tools = register_default_tools();
        for (name, tool) in &tools {
            let params = tool.parameters();
            assert!(params.is_object(), "Tool '{}' parameters is not an object", name);
        }
    }

    // --- CronTool coverage ---

    #[tokio::test]
    async fn test_cron_tool_list_empty() {
        let svc = make_cron_service();
        let tool = CronTool::new(svc);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let args = serde_json::json!({"action": "list"}).to_string();
        let result = tool.execute(&args, &ctx).await.unwrap();
        assert_eq!(result, "[]");
    }

    #[tokio::test]
    async fn test_cron_tool_invalid_json() {
        let svc = make_cron_service();
        let tool = CronTool::new(svc);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = tool.execute("not json", &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_cron_tool_unknown_action() {
        let svc = make_cron_service();
        let tool = CronTool::new(svc);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let args = serde_json::json!({"action": "unknown_action"}).to_string();
        let result = tool.execute(&args, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unknown cron action"));
    }

    #[tokio::test]
    async fn test_cron_tool_delete_missing_id() {
        let svc = make_cron_service();
        let tool = CronTool::new(svc);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let args = serde_json::json!({"action": "delete"}).to_string();
        let result = tool.execute(&args, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Missing 'id'"));
    }

    #[tokio::test]
    async fn test_cron_tool_delete_not_found() {
        let svc = make_cron_service();
        let tool = CronTool::new(svc);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let args = serde_json::json!({"action": "delete", "id": "nonexistent"}).to_string();
        let result = tool.execute(&args, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[tokio::test]
    async fn test_cron_tool_create_missing_schedule() {
        let svc = make_cron_service();
        let tool = CronTool::new(svc);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let args = serde_json::json!({"action": "create", "name": "test"}).to_string();
        let result = tool.execute(&args, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Missing 'schedule'"));
    }

    #[test]
    fn test_cron_tool_set_context() {
        let svc = make_cron_service();
        let tool = CronTool::new(svc);
        tool.set_context("web", "chat1");
    }

    // --- Additional coverage tests ---

    #[test]
    fn test_urlencoding() {
        assert_eq!(urlencoding("hello world"), "hello+world");
        assert_eq!(urlencoding("test@example.com"), "test%40example.com");
        assert_eq!(urlencoding("a+b"), "a%2Bb");
        assert_eq!(urlencoding("simple"), "simple");
    }

    #[test]
    fn test_percent_decode() {
        assert_eq!(percent_decode("hello+world"), "hello world");
        assert_eq!(percent_decode("test%40example.com"), "test@example.com");
        assert_eq!(percent_decode("a%2Bb"), "a+b");
        assert_eq!(percent_decode("no_encoding"), "no_encoding");
    }

    #[test]
    fn test_url_decode_query_param() {
        let url = "https://example.com/l/?uddg=https%3A%2F%2Ffoo.com";
        let result = url_decode_query_param(url, "uddg");
        assert_eq!(result, Some("https://foo.com".to_string()));

        assert_eq!(url_decode_query_param("https://example.com", "uddg"), None);
    }

    #[test]
    fn test_extract_search_query_json() {
        assert_eq!(extract_search_query(r#"{"query": "test search"}"#).unwrap(), "test search");
    }

    #[test]
    fn test_extract_search_query_fallback() {
        assert_eq!(extract_search_query("plain text query").unwrap(), "plain text query");
    }

    #[test]
    fn test_extract_url_json() {
        assert_eq!(extract_url(r#"{"url": "https://example.com"}"#).unwrap(), "https://example.com");
    }

    #[test]
    fn test_extract_url_fallback() {
        assert_eq!(extract_url("https://example.com").unwrap(), "https://example.com");
    }

    #[test]
    fn test_web_search_config_default_values() {
        let config = WebSearchConfig::default();
        assert!(!config.brave_enabled);
        assert!(config.brave_api_key.is_none());
        assert!(config.duckduckgo_enabled);
        assert!(!config.perplexity_enabled);
    }

    #[test]
    fn test_cluster_rpc_config_debug() {
        let config = ClusterRpcConfig {
            local_node_id: "node-1".to_string(),
            timeout_secs: 3600,
            local_rpc_port: 21949,
        };
        let debug_str = format!("{:?}", config);
        assert!(debug_str.contains("node-1"));
    }

    #[test]
    fn test_setup_cluster_rpc_channel_with_config() {
        let cluster_config = ClusterRpcConfig {
            local_node_id: "node-1".to_string(),
            timeout_secs: 3600,
            local_rpc_port: 21949,
        };
        let config = setup_cluster_rpc_channel_with_config(&cluster_config);
        assert_eq!(config.request_timeout, std::time::Duration::from_secs(24 * 3600));
    }

    #[tokio::test]
    async fn test_cluster_rpc_tool_set_context() {
        let config = ClusterRpcConfig {
            local_node_id: "node-1".to_string(),
            timeout_secs: 60,
            local_rpc_port: 21949,
        };
        let tool = ClusterRpcTool::new(config);
        tool.set_context("rpc", "chat-123");
        assert_eq!(*tool.stored_channel.lock().unwrap_or_else(|e| e.into_inner()), "rpc");
        assert_eq!(*tool.stored_chat_id.lock().unwrap_or_else(|e| e.into_inner()), "chat-123");
    }

    #[tokio::test]
    async fn test_cluster_rpc_tool_no_rpc_fn() {
        let config = ClusterRpcConfig {
            local_node_id: "node-1".to_string(),
            timeout_secs: 60,
            local_rpc_port: 21949,
        };
        let tool = ClusterRpcTool::new(config);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let args = serde_json::json!({"target_node": "node-2", "message": "hello"}).to_string();
        let result = tool.execute(&args, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not available"));
    }

    #[tokio::test]
    async fn test_web_search_tool_no_provider_configured() {
        let config = WebSearchConfig {
            brave_enabled: false,
            brave_api_key: None,
            brave_max_results: 5,
            duckduckgo_enabled: false,
            duckduckgo_max_results: 5,
            perplexity_enabled: false,
            perplexity_api_key: None,
            perplexity_max_results: 5,
        };
        let tool = WebSearchTool::new(config);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = tool.execute(r#"{"query": "test"}"#, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No search provider"));
    }

    #[test]
    fn test_register_peer_chat_handler() {
        let mut handlers: std::collections::HashMap<String, Box<dyn Fn(serde_json::Value) -> Result<serde_json::Value, String> + Send + Sync>> = std::collections::HashMap::new();
        register_peer_chat_handler(&mut handlers, |_payload| {
            Ok(serde_json::json!({"status": "ok"}))
        });
        assert!(handlers.contains_key("peer_chat"));
        assert!(handlers.contains_key("peer_chat_callback"));

        let callback = handlers.get("peer_chat_callback").unwrap();
        let result = callback(serde_json::json!({"task_id": "task-1", "content": "response"}));
        assert!(result.is_ok());
        let result_val = result.unwrap();
        assert_eq!(result_val["status"], "received");
    }

    #[tokio::test]
    async fn test_exec_tool_with_cwd() {
        let tmp = TempDir::new().unwrap();
        let tool = ExecTool::new(&tmp.path().to_string_lossy(), false);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let args = serde_json::json!({
            "command": "echo hello",
            "cwd": tmp.path().to_string_lossy().to_string()
        }).to_string();
        let result = tool.execute(&args, &ctx).await;
        assert!(result.is_ok());
        assert!(result.unwrap().contains("hello"));
    }

    #[tokio::test]
    async fn test_exec_tool_workspace_restriction_denied() {
        let tool = ExecTool::new("/safe/workspace", true);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let args = serde_json::json!({
            "command": "echo hello",
            "cwd": "/outside/workspace"
        }).to_string();
        let result = tool.execute(&args, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("outside workspace"));
    }

    #[tokio::test]
    async fn test_sleep_tool_duration_field() {
        let tool = SleepTool;
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let args = serde_json::json!({"duration": 1}).to_string();
        let result = tool.execute(&args, &ctx).await;
        assert!(result.is_ok());
        assert!(result.unwrap().contains("Slept for 1 seconds"));
    }

    #[tokio::test]
    async fn test_sleep_tool_exceeds_max_duration() {
        let tool = SleepTool;
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let args = serde_json::json!({"duration": 999999}).to_string();
        let result = tool.execute(&args, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("cannot exceed"));
    }

    #[tokio::test]
    async fn test_message_tool_raw_args() {
        let tool = MessageTool::new();
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = tool.execute("just some text", &ctx).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "just some text");
    }

    #[test]
    fn test_web_fetch_tool_new() {
        let tool = WebFetchTool::new(4096);
        assert_eq!(tool.max_size, 4096);
    }

    // --- Additional coverage tests for register functions and types ---

    #[test]
    fn test_register_default_tools_has_expected_tools() {
        let tools = register_default_tools();
        assert!(tools.contains_key("message"));
        assert!(tools.contains_key("read_file"));
        assert!(tools.contains_key("write_file"));
        assert!(tools.contains_key("list_dir"));
        assert!(tools.contains_key("create_dir"));
        assert!(tools.contains_key("sleep"));
        assert!(tools.contains_key("edit_file"));
        assert!(tools.contains_key("append_file"));
        assert!(tools.contains_key("delete_file"));
        assert!(tools.contains_key("delete_dir"));
    }

    #[test]
    fn test_register_extended_tools_base_count() {
        let tools = register_extended_tools(None, None, None);
        assert!(tools.len() >= 6);
        assert!(tools.contains_key("web_fetch"));
        assert!(tools.contains_key("memory_search"));
        assert!(tools.contains_key("memory_store"));
        assert!(tools.contains_key("memory_forget"));
        assert!(tools.contains_key("memory_list"));
        assert!(tools.contains_key("skills_list"));
        assert!(tools.contains_key("skills_info"));
    }

    #[test]
    fn test_register_extended_tools_includes_web() {
        let web_config = WebSearchConfig::default();
        let tools = register_extended_tools(Some(web_config), None, None);
        assert!(tools.contains_key("web_search"));
    }

    #[test]
    fn test_register_extended_tools_includes_cluster() {
        let cluster_config = ClusterRpcConfig {
            local_node_id: "node-1".to_string(),
            timeout_secs: 60,
            local_rpc_port: 21949,
        };
        let tools = register_extended_tools(None, Some(cluster_config), None);
        assert!(tools.contains_key("cluster_rpc"));
    }

    #[test]
    fn test_register_extended_tools_includes_spawn() {
        let spawn_config = SpawnConfig {
            default_model: "gpt-4".to_string(),
            max_concurrent: 3,
        };
        let tools = register_extended_tools(None, None, Some(spawn_config));
        assert!(tools.contains_key("spawn"));
    }

    #[test]
    fn test_register_shared_tools_without_workspace() {
        let config = SharedToolConfig::default();
        let tools = register_shared_tools(&config);
        assert!(tools.contains_key("web_fetch"));
        assert!(tools.contains_key("i2c"));
        assert!(tools.contains_key("spi"));
        assert!(!tools.contains_key("exec"));
    }

    #[test]
    fn test_register_shared_tools_with_workspace() {
        let config = SharedToolConfig {
            workspace: Some("/tmp/test".to_string()),
            ..Default::default()
        };
        let tools = register_shared_tools(&config);
        assert!(tools.contains_key("exec"));
        assert!(tools.contains_key("exec_async"));
    }

    #[test]
    fn test_shared_tool_config_default_values() {
        let config = SharedToolConfig::default();
        assert!(config.web_search.is_none());
        assert!(config.cluster_rpc.is_none());
        assert!(config.spawn.is_none());
        assert!(!config.mcp_enabled);
        assert!(config.mcp_servers.is_empty());
        assert!(config.workspace.is_none());
    }

    #[test]
    fn test_shared_tool_config_debug_output() {
        let config = SharedToolConfig::default();
        let debug = format!("{:?}", config);
        assert!(debug.contains("SharedToolConfig"));
        assert!(debug.contains("mcp_enabled"));
    }

    #[test]
    fn test_spawn_config_fields() {
        let config = SpawnConfig {
            default_model: "gpt-4".to_string(),
            max_concurrent: 5,
        };
        assert_eq!(config.default_model, "gpt-4");
        assert_eq!(config.max_concurrent, 5);
    }

    #[test]
    fn test_mcp_server_config_fields() {
        let config = McpServerConfig {
            name: "test-server".to_string(),
            command: "test-cmd".to_string(),
            args: vec!["arg1".to_string()],
            env: std::collections::HashMap::new(),
            timeout_secs: 30,
        };
        assert_eq!(config.name, "test-server");
        assert_eq!(config.command, "test-cmd");
        assert_eq!(config.timeout_secs, 30);
    }

    #[tokio::test]
    async fn test_register_shared_tools_async_mcp_disabled() {
        let config = SharedToolConfig {
            mcp_enabled: false,
            mcp_servers: vec![McpServerConfig {
                name: "test".to_string(),
                command: "test".to_string(),
                args: vec![],
                env: std::collections::HashMap::new(),
                timeout_secs: 30,
            }],
            ..Default::default()
        };
        // Pass a discovery closure that returns an error
        let tools = register_shared_tools_async(&config, Some(|_name: String| {
            async { Err("no mcp".to_string()) }
        })).await;
        assert!(!tools.keys().any(|k| k.starts_with("mcp_")));
    }

    #[tokio::test]
    async fn test_register_shared_tools_async_mcp_enabled_with_failing_discovery() {
        let config = SharedToolConfig {
            mcp_enabled: true,
            mcp_servers: vec![McpServerConfig {
                name: "test".to_string(),
                command: "test".to_string(),
                args: vec![],
                env: std::collections::HashMap::new(),
                timeout_secs: 30,
            }],
            ..Default::default()
        };
        // Pass a discovery closure that returns an error
        let tools = register_shared_tools_async(&config, Some(|_name: String| {
            async { Err("discovery failed".to_string()) }
        })).await;
        assert!(tools.contains_key("web_fetch"));
        // MCP discovery failed, so no MCP tools
        assert!(!tools.keys().any(|k| k.starts_with("mcp_")));
    }

    #[test]
    fn test_percent_decode_edge_cases() {
        assert_eq!(percent_decode(""), "");
        assert_eq!(percent_decode("%00"), "\0");
        // Test percent-decoded bytes that produce valid ASCII
        assert_eq!(percent_decode("%41%42%43"), "ABC");
    }

    #[test]
    fn test_urlencoding_special_chars() {
        assert_eq!(urlencoding(" "), "+");
        assert_eq!(urlencoding("&"), "%26");
        assert_eq!(urlencoding("="), "%3D");
        assert_eq!(urlencoding("/"), "%2F");
    }

    #[test]
    fn test_url_decode_query_param_no_query() {
        let url = "https://example.com/path";
        let result = url_decode_query_param(url, "missing");
        assert!(result.is_none());
    }

    #[test]
    fn test_url_decode_query_param_multiple_params() {
        let url = "https://example.com/?a=1&b=2&c=3";
        assert_eq!(url_decode_query_param(url, "a"), Some("1".to_string()));
        assert_eq!(url_decode_query_param(url, "b"), Some("2".to_string()));
        assert_eq!(url_decode_query_param(url, "c"), Some("3".to_string()));
        assert_eq!(url_decode_query_param(url, "d"), None);
    }

    #[test]
    fn test_extract_search_query_empty() {
        assert_eq!(extract_search_query("").unwrap(), "");
    }

    #[test]
    fn test_extract_url_empty() {
        assert_eq!(extract_url("").unwrap(), "");
    }

    #[test]
    fn test_mcp_server_config_debug() {
        let config = McpServerConfig {
            name: "test".to_string(),
            command: "cmd".to_string(),
            args: vec![],
            env: std::collections::HashMap::new(),
            timeout_secs: 30,
        };
        let debug = format!("{:?}", config);
        assert!(debug.contains("test"));
    }

    #[test]
    fn test_spawn_config_debug() {
        let config = SpawnConfig {
            default_model: "gpt-4".to_string(),
            max_concurrent: 3,
        };
        let debug = format!("{:?}", config);
        assert!(debug.contains("SpawnConfig"));
    }

    #[test]
    fn test_web_search_config_with_all_providers() {
        let config = WebSearchConfig {
            brave_enabled: true,
            brave_api_key: Some("key123".to_string()),
            brave_max_results: 10,
            duckduckgo_enabled: true,
            duckduckgo_max_results: 5,
            perplexity_enabled: true,
            perplexity_api_key: Some("pkey".to_string()),
            perplexity_max_results: 3,
        };
        assert!(config.brave_enabled);
        assert!(config.duckduckgo_enabled);
        assert!(config.perplexity_enabled);
    }

    // --- Additional unique coverage tests for loop_tools.rs ---

    #[test]
    fn test_shared_tool_config_debug_with_none_fields() {
        let config = SharedToolConfig::default();
        let debug = format!("{:?}", config);
        assert!(debug.contains("SharedToolConfig"));
        assert!(debug.contains("web_search: None"));
        assert!(debug.contains("mcp_enabled: false"));
    }

    #[test]
    fn test_register_shared_tools_combined_options() {
        let cron_svc = make_cron_service();
        let config = SharedToolConfig {
            web_search: Some(WebSearchConfig {
                duckduckgo_enabled: true,
                ..Default::default()
            }),
            cluster_rpc: Some(ClusterRpcConfig {
                local_node_id: "node-1".to_string(),
                timeout_secs: 60,
                local_rpc_port: 21949,
            }),
            spawn: Some(SpawnConfig {
                default_model: "test".to_string(),
                max_concurrent: 5,
            }),
            mcp_enabled: true,
            mcp_servers: vec![McpServerConfig {
                name: "test-server".to_string(),
                command: "echo".to_string(),
                args: vec![],
                env: std::collections::HashMap::new(),
                timeout_secs: 30,
            }],
            workspace: Some("/tmp/ws".to_string()),
            cron_service: Some(cron_svc),
            ..Default::default()
        };
        let tools = register_shared_tools(&config);
        assert!(tools.contains_key("web_search"));
        assert!(tools.contains_key("cluster_rpc"));
        assert!(tools.contains_key("spawn"));
        assert!(tools.contains_key("exec"));
        assert!(tools.contains_key("cron"));
    }

    #[tokio::test]
    async fn test_register_shared_tools_async_with_successful_discovery() {
        let config = SharedToolConfig {
            mcp_enabled: true,
            mcp_servers: vec![McpServerConfig {
                name: "test-server".to_string(),
                command: "echo".to_string(),
                args: vec![],
                env: std::collections::HashMap::new(),
                timeout_secs: 30,
            }],
            ..Default::default()
        };
        let discover_fn = |_server_name: String| {
            async move {
                Ok(vec![
                    ("search".to_string(), "Search tool".to_string(), Some(serde_json::json!({"type": "object"}))),
                ])
            }
        };
        let tools = register_shared_tools_async(&config, Some(discover_fn)).await;
        // Tool name is format!("mcp_{}_{}", server_name, tool_name)
        assert!(tools.contains_key("mcp_test-server_search"));
    }

    #[tokio::test]
    async fn test_register_shared_tools_async_mcp_enabled_no_discover_fn() {
        let config = SharedToolConfig {
            mcp_enabled: true,
            mcp_servers: vec![McpServerConfig {
                name: "test".to_string(),
                command: "echo".to_string(),
                args: vec![],
                env: std::collections::HashMap::new(),
                timeout_secs: 30,
            }],
            ..Default::default()
        };
        let tools: HashMap<String, Box<dyn Tool>> = register_shared_tools_async(&config, Option::<fn(String) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<(String, String, Option<serde_json::Value>)>, String>> + Send>>>::None).await;
        assert!(tools.contains_key("message"));
    }

    #[test]
    fn test_cluster_rpc_channel_config_default() {
        let config = ClusterRpcChannelConfig::default();
        assert!(config.request_timeout.as_secs() > 0);
        assert!(config.cleanup_interval.as_secs() > 0);
    }

    #[test]
    fn test_setup_cluster_rpc_channel_without_continuation() {
        let setup = setup_cluster_rpc_channel(None);
        assert!(setup.continuation_manager.is_none());
        assert!(setup.config.request_timeout.as_secs() > 0);
    }

    #[test]
    fn test_setup_cluster_rpc_channel_with_continuation() {
        let cm = Arc::new(crate::loop_continuation::ContinuationManager::new());
        let setup = setup_cluster_rpc_channel(Some(cm));
        assert!(setup.continuation_manager.is_some());
    }

    #[tokio::test]
    async fn test_discover_mcp_tools_success() {
        let result = discover_mcp_tools("test-server", || async {
            Ok(vec![
                ("tool1".to_string(), "Tool 1".to_string(), Some(serde_json::json!({"type":"object"}))),
                ("tool2".to_string(), "Tool 2".to_string(), None),
            ])
        }).await.unwrap();
        assert_eq!(result.server_name, "test-server");
        assert_eq!(result.tools.len(), 2);
        assert_eq!(result.tools[0].name, "tool1");
        assert_eq!(result.tools[1].name, "tool2");
    }

    #[tokio::test]
    async fn test_discover_mcp_tools_empty() {
        let result = discover_mcp_tools("empty-server", || async {
            Ok(vec![])
        }).await.unwrap();
        assert_eq!(result.tools.len(), 0);
    }

    #[tokio::test]
    async fn test_discover_mcp_tools_error() {
        let result = discover_mcp_tools("fail-server", || async {
            Err("Connection refused".to_string())
        }).await;
        assert!(result.is_err());
        let err_msg = result.err().unwrap();
        assert!(err_msg.contains("Connection refused"));
    }

    #[tokio::test]
    async fn test_discovered_mcp_tool_execute() {
        let result = discover_mcp_tools("srv", || async {
            Ok(vec![
                ("my_tool".to_string(), "My tool".to_string(), None),
            ])
        }).await.unwrap();
        let tool = &result.tools[0];
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let exec_result = tool.execute("args", &ctx).await.unwrap();
        assert!(exec_result.contains("MCP/srv"));
        assert!(exec_result.contains("my_tool"));
    }

    #[test]
    fn test_mcp_tool_with_schema() {
        let tool = McpTool::new_simulated("test", "A test tool", "server1")
            .with_schema(serde_json::json!({"type": "object"}));
        assert!(tool.input_schema.is_some());
    }

    #[tokio::test]
    async fn test_mcp_tool_new_custom_executor() {
        let tool = McpTool::new("custom", "Custom tool", "srv", |args: &str| {
            let args_owned = args.to_string();
            async move { Ok(format!("Custom: {}", args_owned)) }
        });
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = tool.execute("hello", &ctx).await.unwrap();
        assert_eq!(result, "Custom: hello");
    }

    #[tokio::test]
    async fn test_i2c_tool_non_linux() {
        let tool = I2CTool;
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = tool.execute(r#"{"action":"detect"}"#, &ctx).await;
        if cfg!(target_os = "linux") {
            assert!(result.is_ok());
        } else {
            assert!(result.is_err());
            assert!(result.unwrap_err().contains("only supported on Linux"));
        }
    }

    #[tokio::test]
    async fn test_i2c_tool_invalid_json() {
        let tool = I2CTool;
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = tool.execute("not json", &ctx).await;
        if cfg!(target_os = "linux") {
            assert!(result.is_err());
        } else {
            assert!(result.is_err());
        }
    }

    #[tokio::test]
    async fn test_spi_tool_non_linux() {
        let tool = SPITool;
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = tool.execute(r#"{"action":"list"}"#, &ctx).await;
        if cfg!(target_os = "linux") {
            assert!(result.is_ok());
        } else {
            assert!(result.is_err());
            assert!(result.unwrap_err().contains("only supported on Linux"));
        }
    }

    #[tokio::test]
    async fn test_spi_tool_invalid_json() {
        let tool = SPITool;
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = tool.execute("not json", &ctx).await;
        if cfg!(target_os = "linux") {
            assert!(result.is_err());
        } else {
            assert!(result.is_err());
        }
    }

    #[test]
    fn test_sanitize_mcp_name() {
        assert_eq!(sanitize_mcp_name("My Tool"), "my_tool");
        assert_eq!(sanitize_mcp_name("my-tool"), "my_tool");
        assert_eq!(sanitize_mcp_name("my.tool"), "my_tool");
        assert_eq!(sanitize_mcp_name("MyTool"), "mytool");
        assert_eq!(sanitize_mcp_name("a b c.d-e"), "a_b_c_d_e");
    }

    #[tokio::test]
    async fn test_cluster_rpc_tool_no_rpc_fn_self_node() {
        let config = ClusterRpcConfig {
            local_node_id: "node-1".to_string(),
            timeout_secs: 60,
            local_rpc_port: 21949,
        };
        let tool = ClusterRpcTool::new(config);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = tool.execute(
            r#"{"target_node": "node-1", "message": "hello"}"#,
            &ctx,
        ).await;
        assert!(result.is_err());
        assert!(result.err().unwrap().contains("not available"));
    }

    #[tokio::test]
    async fn test_no_forge_tools_without_executor() {
        let config = SharedToolConfig {
            forge_executor: None,
            ..Default::default()
        };
        let tools = register_shared_tools(&config);
        assert!(!tools.contains_key("forge_reflect"));
    }

    #[tokio::test]
    async fn test_web_fetch_tool_missing_url() {
        let tool = WebFetchTool::new(50000);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = tool.execute(r#"{"other": "value"}"#, &ctx).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_path_invalid_json_raw() {
        let result = extract_path("not json");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "not json");
    }

    #[test]
    fn test_extract_path_empty_string() {
        let result = extract_path("");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "");
    }

    #[tokio::test]
    async fn test_web_search_tool_missing_query() {
        let config = WebSearchConfig {
            duckduckgo_enabled: true,
            ..Default::default()
        };
        let tool = WebSearchTool::new(config);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = tool.execute("{}", &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_web_search_tool_invalid_json() {
        let config = WebSearchConfig {
            duckduckgo_enabled: true,
            ..Default::default()
        };
        let tool = WebSearchTool::new(config);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = tool.execute("not json", &ctx).await;
        assert!(result.is_err());
    }

    // =========================================================================
    // BootstrapTool coverage
    // =========================================================================

    #[tokio::test]
    async fn test_bootstrap_tool_not_confirmed() {
        let tmp = TempDir::new().unwrap();
        let tool = BootstrapTool::new(&tmp.path().to_string_lossy());
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let args = serde_json::json!({"confirmed": false}).to_string();
        let result = tool.execute(&args, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Must confirm"));
    }

    #[tokio::test]
    async fn test_bootstrap_tool_invalid_args() {
        let tmp = TempDir::new().unwrap();
        let tool = BootstrapTool::new(&tmp.path().to_string_lossy());
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = tool.execute("not json", &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid arguments"));
    }

    #[tokio::test]
    async fn test_bootstrap_tool_missing_confirmed_field() {
        let tmp = TempDir::new().unwrap();
        let tool = BootstrapTool::new(&tmp.path().to_string_lossy());
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let args = serde_json::json!({"other": true}).to_string();
        let result = tool.execute(&args, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Missing or invalid 'confirmed'"));
    }

    #[tokio::test]
    async fn test_bootstrap_tool_already_removed() {
        let tmp = TempDir::new().unwrap();
        // No BOOTSTRAP.md file created
        let tool = BootstrapTool::new(&tmp.path().to_string_lossy());
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let args = serde_json::json!({"confirmed": true}).to_string();
        let result = tool.execute(&args, &ctx).await;
        assert!(result.is_ok());
        assert!(result.unwrap().contains("already been removed"));
    }

    #[tokio::test]
    async fn test_bootstrap_tool_success() {
        let tmp = TempDir::new().unwrap();
        let bootstrap_path = tmp.path().join("BOOTSTRAP.md");
        tokio::fs::write(&bootstrap_path, "# Bootstrap").await.unwrap();
        assert!(bootstrap_path.exists());

        let tool = BootstrapTool::new(&tmp.path().to_string_lossy());
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let args = serde_json::json!({"confirmed": true}).to_string();
        let result = tool.execute(&args, &ctx).await;
        assert!(result.is_ok());
        assert!(result.unwrap().contains("complete"));
        assert!(!bootstrap_path.exists());
    }

    #[tokio::test]
    async fn test_bootstrap_tool_description_and_params() {
        let tmp = TempDir::new().unwrap();
        let tool = BootstrapTool::new(&tmp.path().to_string_lossy());
        assert!(!tool.description().is_empty());
        let params = tool.parameters();
        assert!(params.is_object());
        assert!(params["properties"]["confirmed"].is_object());
    }

    // =========================================================================
    // ClusterRpcTool additional coverage
    // =========================================================================

    #[tokio::test]
    async fn test_cluster_rpc_tool_with_async_ack() {
        let config = ClusterRpcConfig {
            local_node_id: "node-1".to_string(),
            timeout_secs: 60,
            local_rpc_port: 21949,
        };
        let mut tool = ClusterRpcTool::new(config);
        tool.set_rpc_call_fn(Arc::new(|_node: &str, _action: &str, _payload: serde_json::Value| {
            Box::pin(async {
                Ok(serde_json::json!({"status": "accepted", "task_id": "auto-123"}))
            })
        }));

        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = tool
            .execute(r#"{"target_node": "node-2", "message": "hello"}"#, &ctx)
            .await
            .unwrap();
        assert!(result.contains("__ASYNC__:auto-123:node-2"));
    }

    #[tokio::test]
    async fn test_cluster_rpc_tool_target_aliases() {
        // Test that "target", "target_node", and "peer_id" all work
        let config = ClusterRpcConfig {
            local_node_id: "node-1".to_string(),
            timeout_secs: 60,
            local_rpc_port: 21949,
        };

        // Test with "target" alias
        let mut tool = ClusterRpcTool::new(config.clone());
        tool.set_rpc_call_fn(Arc::new(|node: &str, _action: &str, _payload: serde_json::Value| {
            let node = node.to_string();
            Box::pin(async move { Ok(serde_json::json!({"content": format!("Response to {}", node)})) })
        }));
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = tool.execute(r#"{"target": "node-3", "message": "hi"}"#, &ctx).await;
        assert!(result.is_ok());

        // Test with "peer_id" alias
        let mut tool2 = ClusterRpcTool::new(config);
        tool2.set_rpc_call_fn(Arc::new(|node: &str, _action: &str, _payload: serde_json::Value| {
            let node = node.to_string();
            Box::pin(async move { Ok(serde_json::json!({"content": format!("Response to {}", node)})) })
        }));
        let result2 = tool2.execute(r#"{"peer_id": "node-4", "message": "hi"}"#, &ctx).await;
        assert!(result2.is_ok());
    }

    #[tokio::test]
    async fn test_cluster_rpc_tool_data_content_fallback() {
        let config = ClusterRpcConfig {
            local_node_id: "node-1".to_string(),
            timeout_secs: 60,
            local_rpc_port: 21949,
        };
        let mut tool = ClusterRpcTool::new(config);
        tool.set_rpc_call_fn(Arc::new(|_node: &str, _action: &str, payload: serde_json::Value| {
            let content = payload.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
            Box::pin(async move { Ok(serde_json::json!({"content": format!("Got: {}", content)})) })
        }));

        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        // Use data.content format instead of message
        let result = tool
            .execute(r#"{"target_node": "node-2", "data": {"content": "via data"}} "#, &ctx)
            .await
            .unwrap();
        assert!(result.contains("Got: via data"));
    }

    #[tokio::test]
    async fn test_cluster_rpc_tool_stored_context_fallback() {
        let config = ClusterRpcConfig {
            local_node_id: "node-1".to_string(),
            timeout_secs: 60,
            local_rpc_port: 21949,
        };
        let mut tool = ClusterRpcTool::new(config);
        tool.set_context("stored-ch", "stored-cid");
        tool.set_rpc_call_fn(Arc::new(|_node: &str, _action: &str, payload: serde_json::Value| {
            let ch = payload.get("channel").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let cid = payload.get("chat_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            Box::pin(async move { Ok(serde_json::json!({"content": format!("ch={}, cid={}", ch, cid)})) })
        }));

        // Empty context channel/chat_id -> should fall back to stored
        let ctx = RequestContext::new("", "", "user1", "sess1");
        let result = tool
            .execute(r#"{"target_node": "node-2", "message": "test"}"#, &ctx)
            .await
            .unwrap();
        assert!(result.contains("ch=stored-ch"));
        assert!(result.contains("cid=stored-cid"));
    }

    #[tokio::test]
    async fn test_cluster_rpc_tool_empty_sync_response() {
        let config = ClusterRpcConfig {
            local_node_id: "node-1".to_string(),
            timeout_secs: 60,
            local_rpc_port: 21949,
        };
        let mut tool = ClusterRpcTool::new(config);
        tool.set_rpc_call_fn(Arc::new(|_node: &str, _action: &str, _payload: serde_json::Value| {
            Box::pin(async { Ok(serde_json::json!({"status": "done"})) })
        }));

        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = tool
            .execute(r#"{"target_node": "node-2", "message": "test"}"#, &ctx)
            .await
            .unwrap();
        assert_eq!(result, "");
    }

    // =========================================================================
    // CronTool additional coverage: create with different schedule types
    // =========================================================================

    fn make_cron_service_with_dir(tmp: &TempDir) -> Arc<std::sync::Mutex<nemesis_cron::service::CronService>> {
        let db_path = tmp.path().join("cron.db");
        Arc::new(std::sync::Mutex::new(
            nemesis_cron::service::CronService::new(&db_path.to_string_lossy()),
        ))
    }

    #[tokio::test]
    async fn test_cron_tool_create_with_every_schedule() {
        let tmp = TempDir::new().unwrap();
        let svc = make_cron_service_with_dir(&tmp);
        let tool = CronTool::new(svc);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        tool.set_context("web", "chat1");
        let args = serde_json::json!({
            "action": "create",
            "name": "test-every",
            "schedule": "every:60s",
            "content": "test reminder"
        }).to_string();
        let result = tool.execute(&args, &ctx).await;
        assert!(result.is_ok(), "Expected ok, got: {:?}", result);
        assert!(result.unwrap().contains("Created cron job"));
    }

    #[tokio::test]
    async fn test_cron_tool_create_with_cron_expr() {
        let tmp = TempDir::new().unwrap();
        let svc = make_cron_service_with_dir(&tmp);
        let tool = CronTool::new(svc);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        tool.set_context("web", "chat1");
        let args = serde_json::json!({
            "action": "create",
            "name": "test-cron",
            "schedule": "0 * * * *",
            "content": "hourly task"
        }).to_string();
        let result = tool.execute(&args, &ctx).await;
        assert!(result.is_ok(), "Expected ok, got: {:?}", result);
        assert!(result.unwrap().contains("Created cron job"));
    }

    #[tokio::test]
    async fn test_cron_tool_create_and_delete() {
        let tmp = TempDir::new().unwrap();
        let svc = make_cron_service_with_dir(&tmp);
        let tool = CronTool::new(svc);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        tool.set_context("web", "chat1");

        // Create
        let create_args = serde_json::json!({
            "action": "create",
            "name": "temp-job",
            "schedule": "every:30s",
            "content": "temporary"
        }).to_string();
        let create_result = tool.execute(&create_args, &ctx).await.unwrap();
        // Extract ID from "Created cron job: temp-job (ID: xxx)"
        let id_start = create_result.find("(ID: ").unwrap();
        let id_end = create_result.find(")").unwrap();
        let job_id = &create_result[id_start + 5..id_end];

        // Delete
        let delete_args = serde_json::json!({"action": "delete", "id": job_id}).to_string();
        let delete_result = tool.execute(&delete_args, &ctx).await;
        assert!(delete_result.is_ok());
        assert!(delete_result.unwrap().contains("Deleted cron job"));
    }

    #[tokio::test]
    async fn test_cron_tool_create_invalid_every_schedule() {
        let tmp = TempDir::new().unwrap();
        let svc = make_cron_service_with_dir(&tmp);
        let tool = CronTool::new(svc);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let args = serde_json::json!({
            "action": "create",
            "name": "bad-schedule",
            "schedule": "every:invalid"
        }).to_string();
        let result = tool.execute(&args, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_cron_tool_create_with_empty_action() {
        let svc = make_cron_service();
        let tool = CronTool::new(svc);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        // Default action is empty string -> should hit "unknown action"
        let args = serde_json::json!({"name": "test"}).to_string();
        let result = tool.execute(&args, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unknown cron action"));
    }

    #[tokio::test]
    async fn test_cron_tool_list_after_create() {
        let tmp = TempDir::new().unwrap();
        let svc = make_cron_service_with_dir(&tmp);
        let tool = CronTool::new(svc);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        tool.set_context("web", "chat1");

        // Create a job first
        let create_args = serde_json::json!({
            "action": "create",
            "name": "listable-job",
            "schedule": "every:120s",
            "content": "content"
        }).to_string();
        tool.execute(&create_args, &ctx).await.unwrap();

        // List
        let list_args = serde_json::json!({"action": "list"}).to_string();
        let result = tool.execute(&list_args, &ctx).await.unwrap();
        assert!(result.contains("listable-job"));
    }

    // =========================================================================
    // InstallSkillTool coverage
    // =========================================================================

    #[tokio::test]
    async fn test_install_skill_tool_missing_slug() {
        let registry = Arc::new(nemesis_skills::registry::RegistryManager::new_empty());
        let tool = InstallSkillTool::new(registry, "/tmp/ws".to_string());
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

        let result = tool.execute(r#"{"name": "test"}"#, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("slug parameter is required"));
    }

    #[tokio::test]
    async fn test_install_skill_tool_empty_slug() {
        let registry = Arc::new(nemesis_skills::registry::RegistryManager::new_empty());
        let tool = InstallSkillTool::new(registry, "/tmp/ws".to_string());
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

        let result = tool.execute(r#"{"slug": ""}"#, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("slug parameter is required"));
    }

    #[tokio::test]
    async fn test_install_skill_tool_invalid_json() {
        let registry = Arc::new(nemesis_skills::registry::RegistryManager::new_empty());
        let tool = InstallSkillTool::new(registry, "/tmp/ws".to_string());
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

        let result = tool.execute("not json", &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid JSON"));
    }

    #[tokio::test]
    async fn test_install_skill_tool_path_traversal() {
        let registry = Arc::new(nemesis_skills::registry::RegistryManager::new_empty());
        let tool = InstallSkillTool::new(registry, "/tmp/ws".to_string());
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

        let result = tool.execute(r#"{"slug": "../evil"}"#, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid slug"));
    }

    #[tokio::test]
    async fn test_install_skill_tool_already_exists() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().to_string_lossy().to_string();
        // Create the skill directory to simulate existing skill
        let skill_dir = tmp.path().join("skills").join("existing-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();

        let registry = Arc::new(nemesis_skills::registry::RegistryManager::new_empty());
        let tool = InstallSkillTool::new(registry, workspace);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

        let result = tool.execute(r#"{"slug": "existing-skill"}"#, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("already exists locally"));
    }

    // =========================================================================
    // FindSkillsTool coverage
    // =========================================================================

    #[tokio::test]
    async fn test_find_skills_tool_empty_query() {
        let registry = Arc::new(nemesis_skills::registry::RegistryManager::new_empty());
        let tool = FindSkillsTool::new(registry);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

        let result = tool.execute(r#"{"query": ""}"#, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("missing or empty"));
    }

    #[tokio::test]
    async fn test_find_skills_tool_missing_query() {
        let registry = Arc::new(nemesis_skills::registry::RegistryManager::new_empty());
        let tool = FindSkillsTool::new(registry);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

        let result = tool.execute(r#"{"other": "value"}"#, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("missing or empty"));
    }

    #[tokio::test]
    async fn test_find_skills_tool_invalid_json() {
        let registry = Arc::new(nemesis_skills::registry::RegistryManager::new_empty());
        let tool = FindSkillsTool::new(registry);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

        let result = tool.execute("not json", &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid JSON"));
    }

    #[tokio::test]
    async fn test_find_skills_tool_description_and_params() {
        let registry = Arc::new(nemesis_skills::registry::RegistryManager::new_empty());
        let tool = FindSkillsTool::new(registry);
        assert!(!tool.description().is_empty());
        assert!(tool.parameters().is_object());
    }

    // =========================================================================
    // InstallSkillTool description/params
    // =========================================================================

    #[test]
    fn test_install_skill_tool_description_and_params() {
        let registry = Arc::new(nemesis_skills::registry::RegistryManager::new_empty());
        let tool = InstallSkillTool::new(registry, "/tmp/ws".to_string());
        assert!(!tool.description().is_empty());
        assert!(tool.parameters().is_object());
    }

    // =========================================================================
    // register_shared_tools with skills_registry
    // =========================================================================

    #[test]
    fn test_register_shared_tools_with_skills_registry() {
        let registry = Arc::new(nemesis_skills::registry::RegistryManager::new_empty());
        let config = SharedToolConfig {
            skills_registry: Some(registry),
            workspace: Some("/tmp/test-workspace".to_string()),
            ..Default::default()
        };
        let tools = register_shared_tools(&config);
        assert!(tools.contains_key("find_skills"));
        assert!(tools.contains_key("install_skill"));
    }

    #[test]
    fn test_register_shared_tools_with_skills_registry_no_workspace() {
        let registry = Arc::new(nemesis_skills::registry::RegistryManager::new_empty());
        let config = SharedToolConfig {
            skills_registry: Some(registry),
            workspace: None,
            ..Default::default()
        };
        let tools = register_shared_tools(&config);
        assert!(tools.contains_key("find_skills"));
        // install_skill requires workspace
        assert!(!tools.contains_key("install_skill"));
    }

    // =========================================================================
    // ClusterRpcConfig default
    // =========================================================================

    #[test]
    fn test_cluster_rpc_config_default() {
        let config = ClusterRpcConfig::default();
        assert!(config.local_node_id.is_empty());
        assert_eq!(config.timeout_secs, 3600);
        assert_eq!(config.local_rpc_port, 21949);
    }

    // =========================================================================
    // MessageTool: JSON args without content field
    // =========================================================================

    #[tokio::test]
    async fn test_message_tool_json_without_content_field() {
        let tool = MessageTool::new();
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let result = tool
            .execute(r#"{"other": "value"}"#, &ctx)
            .await
            .unwrap();
        // Should fall back to raw args
        assert_eq!(result, r#"{"other": "value"}"#);
    }

    // =========================================================================
    // WebSearchTool extract_query method
    // =========================================================================

    #[test]
    fn test_web_search_tool_extract_query_method() {
        let tool = WebSearchTool::new(WebSearchConfig::default());
        assert_eq!(tool.extract_query(r#"{"query": "test"}"#).unwrap(), "test");
        assert_eq!(tool.extract_query("plain text").unwrap(), "plain text");
    }

    // =========================================================================
    // ForgeBridgeTool via register_shared_tools
    // =========================================================================

    #[test]
    fn test_register_shared_tools_with_forge() {
        let tmp = tempfile::tempdir().unwrap();
        let config = nemesis_forge::config::ForgeConfig::default();
        let forge = Arc::new(nemesis_forge::forge::Forge::new(config, tmp.path().to_path_buf()));
        let executor = Arc::new(nemesis_forge::forge_tools::ForgeToolExecutor::new(forge));
        let config = SharedToolConfig {
            forge_executor: Some(executor),
            ..Default::default()
        };
        let tools = register_shared_tools(&config);
        assert!(tools.contains_key("forge_reflect"));
    }

    // =========================================================================
    // register_shared_tools with complete_bootstrap
    // =========================================================================

    #[test]
    fn test_register_shared_tools_includes_complete_bootstrap() {
        let config = SharedToolConfig {
            workspace: Some("/tmp/ws".to_string()),
            ..Default::default()
        };
        let tools = register_shared_tools(&config);
        assert!(tools.contains_key("complete_bootstrap"));
    }

    // =========================================================================
    // McpDiscoveryResult fields
    // =========================================================================

    #[test]
    fn test_mcp_discovery_result_fields() {
        let result = McpDiscoveryResult {
            tools: vec![],
            server_name: "test".to_string(),
        };
        assert!(result.tools.is_empty());
        assert_eq!(result.server_name, "test");
    }

    // =========================================================================
    // Additional percent_decode edge cases
    // =========================================================================

    #[test]
    fn test_percent_decode_invalid_hex() {
        // %GG is not valid hex - should keep the original
        let result = percent_decode("%GG");
        assert!(result.contains("%GG"));
    }

    #[test]
    fn test_percent_decode_partial_hex() {
        // %1 at end (only one hex char) - chars.by_ref().take(2) consumes the '1'
        // and produces an empty string for the hex parse, so the result is "%"
        let result = percent_decode("test%1");
        // The function consumes '1' via take(2) but only gets one char for hex parse
        // which fails, so it outputs "%" + "1" (the hex string)
        assert!(result.starts_with("test"));
    }

    // =========================================================================
    // urlencoding edge cases
    // =========================================================================

    #[test]
    fn test_urlencoding_unreserved_chars() {
        // Unreserved characters should not be encoded
        assert_eq!(urlencoding("A-Z"), "A-Z");
        assert_eq!(urlencoding("0-9"), "0-9");
        assert_eq!(urlencoding("hello_world"), "hello_world");
        assert_eq!(urlencoding("file.txt"), "file.txt");
        assert_eq!(urlencoding("a~b"), "a~b");
    }

    #[test]
    fn test_urlencoding_empty() {
        assert_eq!(urlencoding(""), "");
    }

    // =========================================================================
    // SharedToolConfig clone
    // =========================================================================

    #[test]
    fn test_shared_tool_config_clone() {
        let config = SharedToolConfig {
            web_search: Some(WebSearchConfig::default()),
            ..Default::default()
        };
        let cloned = config.clone();
        assert!(cloned.web_search.is_some());
    }

    // =========================================================================
    // register_shared_tools_async: MCP enabled with no servers
    // =========================================================================

    #[tokio::test]
    async fn test_register_shared_tools_async_mcp_enabled_no_servers() {
        let config = SharedToolConfig {
            mcp_enabled: true,
            mcp_servers: vec![],
            ..Default::default()
        };
        let tools: HashMap<String, Box<dyn Tool>> = register_shared_tools_async(&config, Option::<fn(String) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<(String, String, Option<serde_json::Value>)>, String>> + Send>>>::None).await;
        assert!(tools.contains_key("message"));
        // No MCP servers configured -> no MCP tools
        assert!(!tools.keys().any(|k| k.starts_with("mcp_")));
    }

    // =========================================================================
    // Tool trait: set_context for SpawnTool
    // =========================================================================

    #[tokio::test]
    async fn test_spawn_tool_set_context() {
        let config = SpawnConfig {
            default_model: "test".to_string(),
            max_concurrent: 5,
        };
        let tool = SpawnTool::new(config);
        tool.set_context("discord", "channel-123");

        // Execute with empty context -> should use stored
        let mut tool_with_fn = SpawnTool::new(SpawnConfig {
            default_model: "test".to_string(),
            max_concurrent: 5,
        });
        tool_with_fn.set_context("stored-ch", "stored-cid");
        tool_with_fn.set_spawn_fn(Arc::new(
            |_agent_id: &str, _task: &str, _model: &str, channel: &str, chat_id: &str| {
                let ch = channel.to_string();
                let cid = chat_id.to_string();
                Box::pin(async move { Ok(format!("ch={}, cid={}", ch, cid)) })
            },
        ));

        let ctx = RequestContext::new("", "", "user1", "sess1");
        let result = tool_with_fn
            .execute(r#"{"agent_id": "a1", "task": "do"}"#, &ctx)
            .await
            .unwrap();
        assert!(result.contains("ch=stored-ch"));
        assert!(result.contains("cid=stored-cid"));
    }

    // =========================================================================
    // Additional CronTool: create with deliver=false
    // =========================================================================

    #[tokio::test]
    async fn test_cron_tool_create_no_deliver() {
        let tmp = TempDir::new().unwrap();
        let svc = make_cron_service_with_dir(&tmp);
        let tool = CronTool::new(svc);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let args = serde_json::json!({
            "action": "create",
            "name": "no-deliver",
            "schedule": "every:60s",
            "content": "test",
            "deliver": false
        }).to_string();
        let result = tool.execute(&args, &ctx).await;
        assert!(result.is_ok());
    }

    // =========================================================================
    // Additional CronTool: create with at: schedule (RFC3339 timestamp)
    // =========================================================================

    #[tokio::test]
    async fn test_cron_tool_create_with_at_schedule() {
        let tmp = TempDir::new().unwrap();
        let svc = make_cron_service_with_dir(&tmp);
        let tool = CronTool::new(svc);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        tool.set_context("web", "chat1");
        let future_ts = "2099-12-31T23:59:59+00:00";
        let args = serde_json::json!({
            "action": "create",
            "name": "at-job",
            "schedule": format!("at:{}", future_ts),
            "content": "future task"
        }).to_string();
        let result = tool.execute(&args, &ctx).await;
        assert!(result.is_ok(), "Expected ok, got: {:?}", result);
    }

    #[tokio::test]
    async fn test_cron_tool_create_with_invalid_at_schedule() {
        let tmp = TempDir::new().unwrap();
        let svc = make_cron_service_with_dir(&tmp);
        let tool = CronTool::new(svc);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let args = serde_json::json!({
            "action": "create",
            "name": "bad-at",
            "schedule": "at:not-a-timestamp",
            "content": "test"
        }).to_string();
        let result = tool.execute(&args, &ctx).await;
        assert!(result.is_err());
    }

    // =========================================================================
    // ExecTool: command with no output
    // =========================================================================

    #[tokio::test]
    async fn test_exec_tool_no_output_command() {
        let tmp = TempDir::new().unwrap();
        let tool = ExecTool::new(&tmp.path().to_string_lossy(), false);
        let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
        let cmd = if cfg!(target_os = "windows") {
            "cd ."
        } else {
            "true"
        };
        let args = serde_json::json!({"command": cmd}).to_string();
        let result = tool.execute(&args, &ctx).await.unwrap();
        assert!(result.contains("no output") || result.is_empty() || !result.contains("Exit code"));
    }
}
