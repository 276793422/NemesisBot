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

        let output = {
            #[cfg(target_os = "windows")]
            let mut cmd = {
                #[allow(unused_imports)]
                use std::os::windows::process::CommandExt;
                let mut c = tokio::process::Command::new("cmd");
                // On Windows, .arg() auto-quotes arguments containing spaces,
                // which garbles cmd.exe's own quote handling (e.g. "if exist"
                // paths with inner quotes). Use raw_arg to pass the command
                // verbatim so cmd.exe parses it correctly.
                c.raw_arg(format!("/C {}", command));
                c
            };
            #[cfg(not(target_os = "windows"))]
            let mut cmd = {
                let mut c = tokio::process::Command::new("sh");
                c.arg("-c").arg(command);
                c
            };
            tokio::time::timeout(
                std::time::Duration::from_secs(timeout_secs),
                cmd.current_dir(cwd).output(),
            ).await
        };

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

        let mut child = {
            #[cfg(target_os = "windows")]
            let mut c = {
                #[allow(unused_imports)]
                use std::os::windows::process::CommandExt;
                let mut c = tokio::process::Command::new("cmd");
                c.raw_arg(format!("/C {}", command));
                c
            };
            #[cfg(not(target_os = "windows"))]
            let mut c = {
                let mut c = tokio::process::Command::new("sh");
                c.arg("-c").arg(command);
                c
            };
            c.current_dir(cwd)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .map_err(|e| format!("Failed to start command: {}", e))?
        };

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

    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String> {
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

                // Use context first, fallback to stored values (mirrors MessageTool pattern).
                let channel = if context.channel.is_empty() {
                    self.channel.lock().unwrap_or_else(|e| e.into_inner()).clone()
                } else {
                    context.channel.clone()
                };
                let chat_id = if context.chat_id.is_empty() {
                    self.chat_id.lock().unwrap_or_else(|e| e.into_inner()).clone()
                } else {
                    context.chat_id.clone()
                };

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
            val.get("seconds")
                .or_else(|| val.get("duration"))
                .and_then(|v| v.as_u64())
                .ok_or("Missing or invalid 'seconds' field (must be a positive integer)")?
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

/// Extract the "name" argument from tool arguments.
fn extract_name_arg(args: &str) -> Result<String, String> {
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(args) {
        if let Some(name) = val.get("name").and_then(|v| v.as_str()) {
            return Ok(name.to_string());
        }
    }
    // Fallback: treat the entire argument as a name.
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
        "[ClusterRPC] Cluster RPC channel configured (24h B-side safety net)"
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
                "[ClusterRPC] Received peer_chat_callback"
            );
            Ok(serde_json::json!({
                "status": "received",
                "task_id": task_id,
            }))
        }),
    );

    tracing::info!("[ClusterRPC] Registered peer_chat + peer_chat_callback handlers");
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
    /// Whether the cluster module is enabled and running.
    /// When false, execute() returns immediately with "cluster not enabled" error
    /// instead of attempting network calls that would fail unpredictably.
    /// This guard preserves LLM prompt cache hit rates (tool definition stays in prompt).
    enabled: Arc<std::sync::atomic::AtomicBool>,
    /// Optional RPC call function: (target_node, action, payload) -> Result<serde_json::Value, String>
    rpc_call_fn: Option<Arc<dyn Fn(&str, &str, serde_json::Value) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<serde_json::Value, String>> + Send>> + Send + Sync>>,
    /// Returns online peer nodes with their capabilities for dynamic tool description.
    /// Each tuple: (node_id, node_name, capabilities).
    peers_fn: Option<Arc<dyn Fn() -> Vec<(String, String, Vec<String>)> + Send + Sync>>,
}

impl ClusterRpcTool {
    /// Create a new cluster RPC tool.
    pub fn new(config: ClusterRpcConfig) -> Self {
        Self {
            config,
            stored_channel: Arc::new(std::sync::Mutex::new(String::new())),
            stored_chat_id: Arc::new(std::sync::Mutex::new(String::new())),
            enabled: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            rpc_call_fn: None,
            peers_fn: None,
        }
    }

    /// Set whether the cluster module is enabled.
    /// When disabled, execute() returns immediately without attempting RPC calls.
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, std::sync::atomic::Ordering::Relaxed);
    }

    /// Check if the cluster module is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Get a clone of the enabled flag Arc for external control.
    /// The ClusterServiceAdapter uses this to toggle the tool's enabled state
    /// without removing the tool from the prompt (preserving LLM cache).
    pub fn enabled_arc(&self) -> Arc<std::sync::atomic::AtomicBool> {
        self.enabled.clone()
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

    /// Set the peers function for dynamic tool description.
    ///
    /// The function returns online peer nodes: `Vec<(node_id, node_name, capabilities)>`.
    /// Called each time the LLM requests tool definitions so the peer list stays current.
    pub fn set_peers_fn(
        &mut self,
        f: Arc<dyn Fn() -> Vec<(String, String, Vec<String>)> + Send + Sync>,
    ) {
        self.peers_fn = Some(f);
    }
}

#[async_trait]
impl Tool for ClusterRpcTool {
    fn description(&self) -> String {
        "Send a message to another bot in the cluster".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        // Dynamically inject online peer list with capabilities into the target description.
        let target_desc = if let Some(ref peers_fn) = self.peers_fn {
            let peers = peers_fn();
            if peers.is_empty() {
                "Target bot ID (no peers currently online)".to_string()
            } else {
                let mut desc = "Target bot ID. Available online peers:\n".to_string();
                for (id, name, caps) in &peers {
                    let caps_str = if caps.is_empty() {
                        "unknown capabilities".to_string()
                    } else {
                        caps.join(", ")
                    };
                    desc.push_str(&format!("- {} ({}): {}\n", id, name, caps_str));
                }
                desc
            }
        } else {
            "Target bot ID".to_string()
        };

        serde_json::json!({
            "type": "object",
            "properties": {
                "target": {"type": "string", "description": target_desc},
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
        // Guard: check if cluster is enabled before any processing.
        // This prevents unpredictable network errors when the cluster module is stopped.
        // The tool definition remains in the prompt for LLM cache hit rate.
        if !self.enabled.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("集群功能未启用，无法调用远程节点。请勿重试。".to_string());
        }

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
                "[ClusterRPC] Peer chat ACK received, returning async marker"
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
        let skill_name = extract_name_arg(args)?;

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

        let slug = match val["name"].as_str().or_else(|| val["slug"].as_str()) {
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
            "[AgentTools] RPC channel for peer chat configured with continuation manager (timeout={:?}, cleanup={:?})",
            config.request_timeout, config.cleanup_interval
        );
        // The continuation manager is ready to save snapshots when async
        // cluster_rpc tools are invoked. It will be used by the executor
        // to save continuation snapshots and handle async callbacks.
        let _ = cm; // Available for caller to wire up
    } else {
        info!(
            "[AgentTools] RPC channel for peer chat configured without continuation manager (timeout={:?}, cleanup={:?})",
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

// ===========================================================================
// MCP Discovery Tools
// ===========================================================================

/// Tool for discovering what tools, resources, and prompts an MCP server provides.
///
/// Connects to an MCP server via stdio or HTTP, performs the handshake, collects
/// metadata, formats it as markdown, and closes the connection.
pub struct McpDiscoverTool;

impl McpDiscoverTool {
    pub fn new() -> Self { Self }
}

#[async_trait]
impl Tool for McpDiscoverTool {
    fn description(&self) -> String {
        "Discover what tools, resources, and prompts an MCP server provides. \
         For stdio-based servers provide the 'command' (executable path); \
         for HTTP-based servers provide the 'url' (e.g. 'http://localhost:8080/mcp'). \
         This tool will connect, query capabilities, and return a formatted summary.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Command to start the MCP server for stdio-based servers (e.g. '/path/to/server.exe', 'npx', 'python')"
                },
                "args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Arguments to pass to the command (stdio only, optional)"
                },
                "url": {
                    "type": "string",
                    "description": "URL of an HTTP-based MCP server (e.g. 'http://localhost:8080/mcp')"
                },
                "timeout": {
                    "type": "number",
                    "description": "Timeout in seconds (default: 15)"
                }
            }
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let parsed = serde_json::from_str::<serde_json::Value>(args)
            .unwrap_or(serde_json::Value::Null);

        let url = parsed["url"].as_str();
        let command = parsed["command"].as_str();
        let timeout_secs = parsed["timeout"].as_u64().unwrap_or(15);

        let result = match (url, command) {
            (Some(url), _) => {
                nemesis_mcp::manager::discover_server_metadata_http(url, timeout_secs).await
            }
            (None, Some(command)) => {
                let tool_args: Vec<String> = parsed["args"].as_array()
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                    .unwrap_or_default();
                nemesis_mcp::manager::discover_server_metadata(
                    command, tool_args, vec![], timeout_secs,
                ).await
            }
            (None, None) => {
                return Err("missing required 'command' or 'url' parameter".to_string());
            }
        };

        match result {
            Ok(info) => Ok(format_discovery_result(&info)),
            Err(e) => Err(e),
        }
    }
}

// ---------------------------------------------------------------------------
// CliReferenceTool — CLI 命令按需查询
// ---------------------------------------------------------------------------

/// Tool for looking up NemesisBot CLI commands.
///
/// Without parameters returns a compact overview of all commands.
/// With a `command` parameter returns detailed help for that command area.
/// Keep data in sync with `nemesisbot/src/commands/*.rs`.
pub struct CliReferenceTool;

impl CliReferenceTool {
    pub fn new() -> Self { Self }
}

#[async_trait]
impl Tool for CliReferenceTool {
    fn description(&self) -> String {
        "Look up NemesisBot CLI commands. Without parameters returns an overview of all commands. \
         Pass a command name for detailed help (e.g. 'model', 'mcp', 'cluster', 'scanner').".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Command name for detailed help. Omit for overview of all commands."
                }
            }
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let parsed = serde_json::from_str::<serde_json::Value>(args)
            .unwrap_or(serde_json::Value::Null);
        let command = parsed["command"].as_str().unwrap_or("").trim();

        if command.is_empty() {
            Ok(cli_overview())
        } else {
            cli_detail(command)
        }
    }
}

fn cli_overview() -> String {
    r#"## NemesisBot CLI 命令概览

| 命令 | 说明 | 关键子命令 |
|------|------|-----------|
| model | 管理 LLM 模型 | add, list, remove, default |
| mcp | 管理 MCP 服务器 | list, add, remove, test, tools, resources, prompts, discover |
| channel | 管理通信通道 | list, enable, disable, status, web, websocket, external |
| cluster | 管理集群 | status, config, info, peers, token, init, enable, disable, reset |
| skills | 管理技能 | list, search, install, remove, source, add-source, install-builtin |
| forge | 管理自学习模块 | status, enable, disable, reflect, list, evaluate, export, learning |
| cron | 管理定时任务 | list, add, remove, enable, disable |
| security | 管理安全设置 | status, enable, disable, config, audit, rules, test, approve, deny, pending |
| scanner | 管理病毒扫描引擎 | list, add, remove, check, install, clamav |
| log | 管理日志配置 | llm (enable/disable/status/config/type), general (enable/disable/level/file/console) |
| auth | 管理认证 | login, logout, status |
| memory | 管理增强内存 | enable, disable, status |
| workflow | 管理工作流 | list, run, status, template, validate |
| cors | 管理 CORS 配置 | list, add, remove, dev-mode, show, validate |
| status | 显示系统状态 | — |
| version | 显示版本信息 | — |

使用方式: `nemesisbot <命令> <子命令> [参数]`
示例: `nemesisbot model add --model zhipu/glm-4.7 --key YOUR_KEY --default`

查询具体命令的详细用法请传入 command 参数。"#.to_string()
}

fn cli_detail(command: &str) -> Result<String, String> {
    match command.to_lowercase().as_str() {
        "model" => Ok(r#"## model — 管理 LLM 模型

用法: `nemesisbot model <子命令>`

子命令:
  add       添加模型配置
            --model <vendor/model>（必填）如 zhipu/glm-4.7
            --key <api-key>        API 密钥
            --base <url>           自定义 API 地址
            --proxy <url>          代理地址
            --auth <method>        认证方式
            --default              设为默认模型
  list      列出已配置的模型
            --verbose              显示详细信息
  remove    删除模型配置
            <name>                 模型名称
            --force                跳过确认
  default   显示当前默认模型

示例:
  nemesisbot model add --model openai/gpt-4o --key sk-xxx --default
  nemesisbot model list --verbose
  nemesisbot model remove gpt-4o --force"#.to_string()),

        "mcp" => Ok(r#"## mcp — 管理 MCP 服务器

用法: `nemesisbot mcp <子命令>`

子命令:
  list                          列出已配置的 MCP 服务器
  add -n <名称> -c <命令>       添加 MCP 服务器
            --args <参数>        启动参数
            --env <变量>         环境变量（KEY=VALUE）
            --timeout <秒>       超时时间（默认 30）
  remove <名称>                 删除 MCP 服务器
  test <名称>                   测试服务器连接
  inspect <名称>                查看服务器配置详情
  tools <名称>                  列出服务器提供的工具
  resources <名称>              列出服务器提供的资源
  prompts <名称>                列出服务器提供的提示词
  discover --command <路径>     发现 MCP 服务器能力（stdio 模式）
            --url <URL>          发现 MCP 服务器能力（HTTP 模式）
            --args <参数>        启动参数（stdio）
            --timeout <秒>       超时时间（默认 15）

示例:
  nemesisbot mcp add -n desktop -c C:\AI\MCP\desktop-mcp.exe
  nemesisbot mcp tools desktop
  nemesisbot mcp discover --command C:\AI\MCP\server.exe"#.to_string()),

        "channel" => Ok(r#"## channel — 管理通信通道

用法: `nemesisbot channel <子命令>`

子命令:
  list                          列出所有通道及状态
  enable <名称>                 启用通道
  disable <名称>                禁用通道
  status <名称>                 查看通道详情

  web <操作>                    Web 通道管理:
    auth                        交互式设置认证令牌
    auth-set <token>            直接设置令牌
    auth-get                    查看当前令牌（掩码）
    host <地址>                 设置服务器地址
    port <端口>                 设置端口
    status / config / clear     状态/配置/清除令牌

  websocket <操作>              WebSocket 通道管理:
    setup / config              设置/查看配置
    set <key> <value>           设置配置项
    get <key>                   获取配置项

  external <操作>               External 通道管理:
    setup / config / test       设置/配置/测试
    set <key> <value>           设置配置项
    get <key>                   获取配置项

示例:
  nemesisbot channel list
  nemesisbot channel enable discord
  nemesisbot channel web port 49000"#.to_string()),

        "cluster" => Ok(r#"## cluster — 管理集群

用法: `nemesisbot cluster <子命令>`

子命令:
  status                        显示集群状态
  config                        显示/修改集群配置
    --udp-port / --rpc-port / --broadcast-interval
  info                          显示/修改本节点信息
    --name / --role / --category / --tags / --address / --capabilities
  init                          初始化集群
    --name / --role / --category / --tags / --address / --capabilities
  enable / disable / start / stop   启用/禁用集群
  reset --hard                  重置集群配置

  peers <操作>                  管理对等节点:
    list / add / remove / enable / disable
    add --id <ID> --name <名称> --address <地址>

  token <操作>                  管理 RPC 认证令牌:
    generate --length 32 --save   生成令牌
    show --full                   显示令牌
    set <token> / verify <token>  设置/验证令牌
    revoke                        撤销令牌

示例:
  nemesisbot cluster init --name bot1 --role worker
  nemesisbot cluster peers list
  nemesisbot cluster token generate --save"#.to_string()),

        "skills" => Ok(r#"## skills — 管理技能

用法: `nemesisbot skills <子命令>`

子命令:
  list                          列出已安装的技能
  search [关键词] --limit <N>   搜索远程技能
  install <技能>                安装技能
  remove <名称>                 删除技能
  show <名称>                   查看技能详情
  validate <路径>               验证技能文件
  add-source <url>              添加技能源（GitHub）
  install-builtin [名称]        安装内置技能
  list-builtin                  列出可用的内置技能

  source <操作>                 管理技能源:
    list / add <url> / remove <名称>

  cache <操作>                  管理搜索缓存:
    stats / clear

示例:
  nemesisbot skills search weather
  nemesisbot skills install clawhub/author/weather
  nemesisbot skills list"#.to_string()),

        "forge" => Ok(r#"## forge — 管理自学习模块

用法: `nemesisbot forge <子命令>`

子命令:
  status                        显示 forge 状态
  enable / disable              启用/禁用
  reflect                       手动触发反思
  list --type <类型>            列出制品（默认 all）
  evaluate <id>                 评估制品
  export [id] --output <路径> --all  导出制品

  learning <操作>               学习管理:
    status / enable / disable
    history --limit <N>

示例:
  nemesisbot forge status
  nemesisbot forge reflect
  nemesisbot forge list"#.to_string()),

        "cron" => Ok(r#"## cron — 管理定时任务

用法: `nemesisbot cron <子命令>`

子命令:
  list                          列出所有任务
  add -n <名称> -m <消息>       添加任务
            --every <秒>         间隔执行
            --cron <表达式>      Cron 表达式执行
            --deliver            投递响应到通道
            --to <接收者>        指定接收者
            --channel <通道>     指定通道
  remove <id>                   删除任务
  enable <id>                   启用任务
  disable <id>                  禁用任务

示例:
  nemesisbot cron add -n "每日问候" -m "早上好" --cron "0 9 * * *"
  nemesisbot cron list
  nemesisbot cron remove abc123"#.to_string()),

        "security" => Ok(r#"## security — 管理安全设置

用法: `nemesisbot security <子命令>`

子命令:
  status                        显示安全状态
  enable / disable              启用/禁用安全模块
  edit                          编辑安全配置
  config-reset                  重置为默认配置

  config <操作>                 配置管理:
    show / edit / reset

  audit <操作>                  审计日志:
    show --limit <N>             查看日志
    export <文件>                导出日志
    denied                       查看被拒绝的操作

  rules <操作>                  安全规则:
    list [类型]                  列出规则
    add <类型> <操作> --pattern <模式> --action <deny/allow>
    remove <类型> <操作> <索引>
    test <类型> <操作> <目标>
  类型: file, directory, process, network, hardware, registry

  test --tool <工具> --args <JSON>  测试安全检查
  approve <id>                  批准待审批操作
  deny <id> [原因]              拒绝待审批操作
  pending                       列出待审批操作

示例:
  nemesisbot security status
  nemesisbot security rules list
  nemesisbot security approve 123"#.to_string()),

        "scanner" => Ok(r#"## scanner — 管理病毒扫描引擎

用法: `nemesisbot scanner <子命令>`

子命令:
  list                          列出所有引擎
  add <名称> --url <URL> --path <路径> --address <地址>  添加引擎
  remove <名称>                 删除引擎
  check                         检查所有引擎的安装状态
  install [--dir <目录>]        安装所有待安装引擎

  <引擎名> <操作>               引擎级操作（如 clamav）:
    install [--force] [--url <URL>] [--dir <目录>]  安装
    enable / disable                                启用/禁用
    update                                          更新病毒库
    test <文件路径>                                  测试扫描
    info                                            引擎详情

示例:
  nemesisbot scanner list
  nemesisbot scanner check
  nemesisbot scanner clamav install
  nemesisbot scanner clamav test /path/to/file"#.to_string()),

        "log" => Ok(r#"## log — 管理日志配置

用法: `nemesisbot log <子命令>`

LLM 日志:
  llm enable / disable          启用/禁用 LLM 日志
  llm status                    查看状态
  llm config --detail-level <级别> --log-dir <目录>  配置
  llm type <raw|default>        设置日志类型（原始 JSON / Markdown 摘要）

通用日志:
  general enable / disable       启用/禁用
  general status                 查看状态
  general level <级别>           设置日志级别（debug/info/warn/error）
  general file <路径>            设置日志文件路径
  general console                切换控制台输出

兼容性别名:
  log enable / disable / status / config / set-level
  log enable-file / disable-file / enable-console / disable-console

示例:
  nemesisbot log llm enable
  nemesisbot log llm type raw
  nemesisbot log general level debug"#.to_string()),

        "auth" => Ok(r#"## auth — 管理认证

用法: `nemesisbot auth <子命令>`

子命令:
  login --provider <名称>       登录（OAuth 或粘贴令牌）
            --device-code       使用设备码流程
  logout --provider <名称>      登出（省略名称则登出全部）
  status                        查看认证状态

示例:
  nemesisbot auth login --provider openai
  nemesisbot auth status"#.to_string()),

        "memory" => Ok(r#"## memory — 管理增强内存

用法: `nemesisbot memory <子命令>`

子命令:
  enable    启用增强内存（需要 plugin_onnx.dll 在 plugins/ 目录）
  disable   禁用增强内存
  status    查看内存系统状态

示例:
  nemesisbot memory status
  nemesisbot memory enable"#.to_string()),

        "workflow" => Ok(r#"## workflow — 管理工作流

用法: `nemesisbot workflow <子命令>`

子命令:
  list                          列出工作流
  run <名称> [key=value ...]    运行工作流
  status [执行ID]               查看执行状态
  validate <文件路径>           验证工作流定义

  template <操作>               模板管理:
    list                        列出可用模板
    show <名称>                 查看模板详情
    create <模板> --output <路径>  从模板创建

示例:
  nemesisbot workflow list
  nemesisbot workflow run my-flow input=hello
  nemesisbot workflow template list"#.to_string()),

        "cors" => Ok(r#"## cors — 管理 CORS 配置

用法: `nemesisbot cors <子命令>`

子命令:
  list                          列出所有允许的来源
  add <来源> --cdn              添加允许的来源（--cdn 添加为 CDN 域名）
  remove <来源> --cdn           删除来源
  show                          显示完整 CORS 配置
  validate <来源>               验证来源是否被允许

  dev-mode <操作>               开发模式管理:
    enable / disable / status   允许所有 localhost 来源

示例:
  nemesisbot cors add https://example.com
  nemesisbot cors dev-mode enable"#.to_string()),

        "status" => Ok(r#"## status — 显示系统状态

用法: `nemesisbot status`

显示当前系统配置和运行状态。"#.to_string()),

        "version" => Ok(r#"## version — 显示版本信息

用法: `nemesisbot version`

显示 NemesisBot 版本号和构建信息。"#.to_string()),

        _ => Err(format!(
            "Unknown command '{}'. Call cli_reference without parameters to see all commands.",
            command
        )),
    }
}

fn format_discovery_result(result: &nemesis_mcp::manager::DiscoveryResult) -> String {
    let mut lines = Vec::new();

    // Server info
    if let Some(ref info) = result.server_info {
        lines.push(format!("## MCP Server: {} v{}\n", info.name, info.version));
    } else {
        lines.push("## MCP Server (unknown)\n".to_string());
    }

    // Tools
    if result.tools.is_empty() {
        lines.push("### Tools\nNone.\n".to_string());
    } else {
        lines.push(format!("### Tools ({})\n", result.tools.len()));
        for tool in &result.tools {
            let desc = tool.description.as_deref().unwrap_or("no description");
            lines.push(format!("- **{}**: {}", tool.name, desc));

            // Parameter summary
            if let Some(props) = tool.input_schema.get("properties").and_then(|p| p.as_object()) {
                let required: Vec<&str> = tool.input_schema.get("required")
                    .and_then(|r| r.as_array())
                    .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                    .unwrap_or_default();

                let param_summary: Vec<String> = props.iter().map(|(name, schema)| {
                    let type_str = schema.get("type")
                        .and_then(|t| t.as_str())
                        .unwrap_or("any");
                    if required.contains(&name.as_str()) {
                        format!("{}* ({})", name, type_str)
                    } else {
                        format!("{} ({})", name, type_str)
                    }
                }).collect();

                if !param_summary.is_empty() {
                    lines.push(format!("  - Parameters: {}", param_summary.join(", ")));
                }
            }
        }
        lines.push(String::new());
    }

    // Resources
    if result.resources.is_empty() {
        lines.push("### Resources\nNone.\n".to_string());
    } else {
        lines.push(format!("### Resources ({})\n", result.resources.len()));
        for res in &result.resources {
            let desc = res.description.as_deref().unwrap_or("");
            if desc.is_empty() {
                lines.push(format!("- **{}** ({})", res.name, res.uri));
            } else {
                lines.push(format!("- **{}** ({}): {}", res.name, res.uri, desc));
            }
        }
        lines.push(String::new());
    }

    // Prompts
    if result.prompts.is_empty() {
        lines.push("### Prompts\nNone.".to_string());
    } else {
        lines.push(format!("### Prompts ({})\n", result.prompts.len()));
        for prompt in &result.prompts {
            let desc = prompt.description.as_deref().unwrap_or("no description");
            lines.push(format!("- **{}**: {}", prompt.name, desc));
            if !prompt.arguments.is_empty() {
                let args: Vec<String> = prompt.arguments.iter().map(|a| {
                    let req = if a.required.unwrap_or(false) { "*" } else { "" };
                    let desc = a.description.as_deref().unwrap_or("");
                    if desc.is_empty() {
                        format!("{}{}", a.name, req)
                    } else {
                        format!("{}{} ({})", a.name, req, desc)
                    }
                }).collect();
                lines.push(format!("  - Arguments: {} (* = required)", args.join(", ")));
            }
        }
    }

    lines.join("\n")
}

/// Tool for listing all currently registered MCP tools.
///
/// Reads from a shared snapshot updated by AgentLoop when MCP tools change.
pub struct McpListTool {
    mcp_tools: Arc<parking_lot::RwLock<Vec<(String, String)>>>,
}

impl McpListTool {
    pub fn new(mcp_tools: Arc<parking_lot::RwLock<Vec<(String, String)>>>) -> Self {
        Self { mcp_tools }
    }
}

#[async_trait]
impl Tool for McpListTool {
    fn description(&self) -> String {
        "List all currently registered MCP tools and their descriptions. \
         Use this to see what MCP tools are available in the current session.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {}})
    }

    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        let tools = self.mcp_tools.read();
        if tools.is_empty() {
            return Ok("No MCP tools are currently registered.".to_string());
        }
        let mut lines = vec![format!("## Registered MCP Tools ({})\n", tools.len())];
        for (name, desc) in tools.iter() {
            lines.push(format!("- **{}**: {}", name, desc));
        }
        Ok(lines.join("\n"))
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
    /// Forge instance for experience collection in AgentLoop.
    pub forge: Option<Arc<nemesis_forge::forge::Forge>>,
    /// Memory tool executor for memory_search, memory_store, etc.
    pub memory_executor: Option<Arc<nemesis_memory::memory_tools::MemoryToolExecutor>>,
    /// Snapshot of registered MCP tool names and descriptions for McpListTool.
    pub mcp_tool_snapshot: Option<Arc<parking_lot::RwLock<Vec<(String, String)>>>>,
}

impl Default for SharedToolConfig {
    fn default() -> Self {
        Self {
            web_search: None,
            cluster_rpc: None,
            spawn: None,
            skills_registry: None,
            skills_loader: None,
            workspace: None,
            cron_service: None,
            forge_executor: None,
            forge: None,
            memory_executor: None,
            mcp_tool_snapshot: None,
        }
    }
}

impl std::fmt::Debug for SharedToolConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedToolConfig")
            .field("web_search", &self.web_search)
            .field("cluster_rpc", &self.cluster_rpc)
            .field("spawn", &self.spawn)
            .field("skills_registry", &self.skills_registry.as_ref().map(|_| "RegistryManager"))
            .field("skills_loader", &self.skills_loader.as_ref().map(|_| "SkillsLoader"))
            .field("workspace", &self.workspace)
            .field("memory_executor", &self.memory_executor.as_ref().map(|_| "MemoryToolExecutor"))
            .field("mcp_tool_snapshot", &self.mcp_tool_snapshot.as_ref().map(|_| "McpToolSnapshot"))
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
        Box::new(MemorySearchTool::new(config.memory_executor.clone())),
    );
    tools.insert(
        "memory_store".to_string(),
        Box::new(MemoryStoreTool::new(config.memory_executor.clone())),
    );
    tools.insert(
        "memory_forget".to_string(),
        Box::new(MemoryForgetTool::new(config.memory_executor.clone())),
    );
    tools.insert(
        "memory_list".to_string(),
        Box::new(MemoryListTool::new(config.memory_executor.clone())),
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
        info!("[AgentTools] Registered {} forge tools", forge_count);
    }

    // MCP discovery and listing tools.
    tools.insert(
        "mcp_discover".to_string(),
        Box::new(McpDiscoverTool::new()),
    );
    tools.insert(
        "cli_reference".to_string(),
        Box::new(CliReferenceTool::new()),
    );
    {
        let snapshot = config.mcp_tool_snapshot.clone()
            .unwrap_or_else(|| Arc::new(parking_lot::RwLock::new(Vec::new())));
        tools.insert(
            "mcp_list".to_string(),
            Box::new(McpListTool::new(snapshot)),
        );
    }

    info!(
        "[AgentTools] Registered {} shared tools (web={}, cluster={}, spawn={})",
        tools.len(),
        config.web_search.is_some(),
        config.cluster_rpc.is_some(),
        config.spawn.is_some(),
    );

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
        skills_registry: None,
        skills_loader: None,
        workspace: None,
        cron_service: None,
        forge_executor: None,
        forge: None,
        memory_executor: None,
        mcp_tool_snapshot: None,
    };
    register_shared_tools(&shared_config)
}

#[cfg(test)]
mod tests;
mod coverage_boost_tests;
