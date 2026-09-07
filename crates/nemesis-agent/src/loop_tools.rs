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
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use nemesis_path::paths::canonicalize_for_compare;

use async_trait::async_trait;
use tokio::sync::Mutex;
use tokio::time::sleep;
use tracing::info;

use crate::background_registry::{
    BackgroundKillTool, BackgroundOutputTool, BackgroundProcessRegistry, BackgroundStartTool,
};
use crate::context::RequestContext;
use crate::r#loop::{FileChange, FileChangeKind, Tool};
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
        self.sent_in_round
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Reset the sent-in-round flag (called at the start of each LLM iteration).
    pub fn reset_sent_in_round(&self) {
        self.sent_in_round
            .store(false, std::sync::atomic::Ordering::Relaxed);
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
        "Send a message to user on a chat channel. Use this when you want to communicate something."
            .to_string()
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

        // If a send callback is registered, publish the outbound message.
        let guard = self.send_callback.lock().await;
        if let Some(ref callback) = *guard {
            // Format with RPC prefix if applicable.
            let formatted = context.format_rpc_message(&content);
            callback(&channel, &chat_id, &formatted);
            self.sent_in_round
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }

        Ok(content)
    }
}

/// A tool that reads the contents of a file from disk.
pub struct ReadFileTool;

#[async_trait]
impl Tool for ReadFileTool {
    fn description(&self) -> String {
        "Read the contents of a file. For large files (e.g. spill locator files), pass offset/limit to read a character-based segment instead of the whole file".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to read"
                },
                "offset": {
                    "type": "integer",
                    "description": "Optional start offset in characters (0-based). Use with limit to read large files in segments"
                },
                "limit": {
                    "type": "integer",
                    "description": "Optional max characters to return starting at offset (positive)"
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

        let content = match tokio::fs::read_to_string(path).await {
            Ok(c) => c,
            // A8（2026-09-04 devtool-upgrade）：非 UTF-8 → magic byte 二进制
            // 分支（图片提示走 vision 附加正道 / PDF 诚实说明 / 其他报字节
            // 数），不再一律报错让模型反复重试。
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                return Ok(binary_file_summary(path));
            }
            Err(e) => return Err(format!("Failed to read file: {}", e)),
        };

        // Optional char-based segmentation (offset/limit). This backs the
        // spill marker's promise ("可用 read_file 工具按 offset/limit 分段
        // 读取"): without it, re-reading a >64KB spill file would itself
        // spill again (a locator pointing at another locator). Char-based
        // slicing is multibyte-safe. When neither param is present the raw
        // full content is returned byte-identically (legacy path).
        let (offset, limit) = extract_offset_limit(args)?;
        if offset.is_none() && limit.is_none() {
            return Ok(content);
        }
        let offset = offset.unwrap_or(0);
        let total = content.chars().count();
        // Effective limit defaults to "to end of file" when only offset is given.
        let eff_limit = limit.unwrap_or(total.saturating_sub(offset));
        let slice: String = content.chars().skip(offset).take(eff_limit).collect();
        let returned = slice.chars().count();
        Ok(format!(
            "[read_file 分段] path={} total_chars={} offset={} limit={} chars_returned={}\n{}",
            path.display(),
            total,
            offset,
            eff_limit,
            returned,
            slice
        ))
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

/// A8（2026-09-04 devtool-upgrade）：read_file 对二进制文件的诚实摘要。
///
/// 非 UTF-8 文件不再一律报错：按 magic byte 分类——图片 → 提示走 vision
/// 附加正道（多模态管线已有 base64 上行，不塞 base64 进文本）；PDF →
/// 诚实说明文本提取是远期项；其他二进制 → 报字节数。返回 Ok 说明文本
/// （工具执行成功但内容不可文本化），模型读后自行决策，不再盲目重试。
fn binary_file_summary(path: &Path) -> String {
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let head = std::fs::File::open(path)
        .and_then(|mut f| {
            use std::io::Read;
            let mut buf = [0u8; 12];
            let mut read = 0;
            while read < buf.len() {
                let n = f.read(&mut buf[read..])?;
                if n == 0 {
                    break;
                }
                read += n;
            }
            Ok(buf[..read].to_vec())
        })
        .unwrap_or_default();
    // PDF：头 `%PDF`（规范允许头 1024 字节内，实际恒在文件最前）。
    if head.starts_with(b"%PDF") {
        return format!(
            "binary PDF file ({} bytes); text extraction is not supported yet - ask the user for the content or process it with a dedicated tool",
            size
        );
    }
    // 图片格式（与多模态附加管线同一张 magic 签名表）。
    if let Some(ext) = crate::image_path_detector::ext_from_magic(&head) {
        return format!(
            "binary {} image file ({} bytes); use vision by attaching the image instead of reading it as text",
            ext, size
        );
    }
    format!("{} bytes binary (type unknown)", size)
}

/// write_file / edit_file / append_file 的工作区边界。
///
/// 安全 8 层管线之外的第二道闸（纵深防御，不单靠管线）：管线按操作语义
/// 拦截，本边界保证写路径落点不出工作区。`restrict=false` 时形同未设界
/// （用户在 config 显式关掉 `agents.defaults.restrict_to_workspace`）。
#[derive(Debug, Clone)]
pub struct WorkspaceBoundary {
    /// 工作区根（gateway 侧 = `<home>/workspace`；executor 子进程侧 =
    /// `NEMESISBOT_EXECUTOR_WORKSPACE`）。
    pub root: PathBuf,
    /// 是否启用限制。
    pub restrict: bool,
}

/// 校验 `path` 是否落在边界内，返回归一后的路径供后续 IO 使用。
///
/// 相对路径先 join 到边界根；双侧 `canonicalize_for_compare` 归一（防
/// symlink escape + Windows 8.3 短名/大小写失配误拒——RUNNER~1 家族），
/// 目标不存在时按最长存在祖先解析（write 新文件路径合法）。
pub fn validate_workspace_path(
    path: &str,
    boundary: &WorkspaceBoundary,
) -> Result<PathBuf, String> {
    let target = Path::new(path);
    let canonical = if target.is_absolute() {
        target.to_path_buf()
    } else {
        boundary.root.join(target)
    };
    if !boundary.restrict {
        return Ok(canonical);
    }
    let resolved = canonicalize_for_compare(&canonical);
    let ws = canonicalize_for_compare(&boundary.root);
    if resolved.starts_with(&ws) {
        Ok(canonical)
    } else {
        Err(format!(
            "access denied: path '{}' is outside the workspace",
            path
        ))
    }
}

/// A tool that writes content to a file on disk.
///
/// A5（2026-09-04）：可携带工作区边界（`with_boundary`，生产形态）；
/// `default()` 无界（基线注册 / 测试形态，行为同边界引入前）。
#[derive(Default)]
pub struct WriteFileTool {
    boundary: Option<Arc<WorkspaceBoundary>>,
}

impl WriteFileTool {
    /// 带工作区边界的生产形态。
    pub fn with_boundary(boundary: Arc<WorkspaceBoundary>) -> Self {
        Self {
            boundary: Some(boundary),
        }
    }
}

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

        // A5（2026-09-04）：工作区边界（纵深防御，不单靠安全 8 层管线）。
        let path = match &self.boundary {
            Some(b) => validate_workspace_path(&path, b)?,
            None => PathBuf::from(&path),
        };

        // A5：覆盖保护——已存在文件先读旧内容（读取失败按新建处理，不挡
        // 写入），覆盖成功后附 unified diff（A2）。
        let old_content = tokio::fs::read_to_string(&path).await.ok();

        // Create parent directories if needed.
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("Failed to create directories: {}", e))?;
        }

        tokio::fs::write(&path, &content)
            .await
            .map_err(|e| format!("Failed to write file: {}", e))?;

        match old_content {
            Some(old) => {
                let diff = edit_hint::unified_diff(&path.to_string_lossy(), &old, &content);
                let mut out = format!(
                    "Successfully wrote {} bytes to {} (overwrote existing)",
                    content.len(),
                    path.display()
                );
                if !diff.is_empty() {
                    out.push_str(&format!("\n```diff\n{diff}\n```"));
                }
                Ok(out)
            }
            None => Ok(format!(
                "Successfully wrote {} bytes to {}",
                content.len(),
                path.display()
            )),
        }
    }

    fn preview(&self, args: &str) -> Option<FileChange> {
        let (path, _content) = extract_path_and_content(args).ok()?;
        let kind = if Path::new(&path).exists() {
            FileChangeKind::Modify
        } else {
            FileChangeKind::Create
        };
        Some(FileChange { path, kind })
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
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| format!("Entry error: {}", e))?
        {
            let name = entry.file_name().to_string_lossy().to_string();
            let metadata = entry
                .metadata()
                .await
                .map_err(|e| format!("Metadata error: {}", e))?;
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

    fn is_read_only(&self) -> bool {
        true
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

/// Extract optional `offset`/`limit` (non-negative integers) from read_file
/// arguments. Returns `Ok((None, None))` when neither is present (legacy
/// full-read path) and when args are not JSON at all (raw-path fallback).
/// Validation errors are explicit so a malformed value is never silently
/// ignored (args_validator would catch type errors first, but this tool is
/// also callable via paths that bypass it).
fn extract_offset_limit(args: &str) -> Result<(Option<usize>, Option<usize>), String> {
    let val: serde_json::Value = match serde_json::from_str(args) {
        Ok(v) => v,
        Err(_) => return Ok((None, None)),
    };
    let parse = |name: &str| -> Result<Option<usize>, String> {
        match val.get(name) {
            None | Some(serde_json::Value::Null) => Ok(None),
            Some(v) => {
                let n = v
                    .as_u64()
                    .ok_or_else(|| format!("'{name}' must be a non-negative integer, got: {v}"))?;
                Ok(Some(n as usize))
            }
        }
    };
    let offset = parse("offset")?;
    let limit = parse("limit")?;
    if limit == Some(0) {
        return Err("'limit' must be a positive integer (got 0)".to_string());
    }
    Ok((offset, limit))
}

/// Extract path and content from tool arguments (JSON).
///
/// Expects a JSON object with "path" and "content" fields.
fn extract_path_and_content(args: &str) -> Result<(String, String), String> {
    let val: serde_json::Value =
        serde_json::from_str(args).map_err(|e| format!("Invalid JSON arguments: {}", e))?;

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
/// `replace_all` is optional and defaults to false.
fn extract_edit_args(args: &str) -> Result<(String, String, String, bool), String> {
    let val: serde_json::Value =
        serde_json::from_str(args).map_err(|e| format!("Invalid JSON arguments: {}", e))?;

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

    let replace_all = val
        .get("replace_all")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    Ok((path, old_text, new_text, replace_all))
}

/// A7（2026-09-06）：单条编辑替换决策的**单一真相源**（纯函数）——
/// `edit_file`（A1-A4）与 `multiedit`（A7）共用同一套匹配语义：A1 多命中
/// 歧义提示 → A3 全量替换 → A4 五级模糊级联。错误文案与历史 edit_file
/// 行为逐字一致（A1-A4 测试锁死）。
pub(crate) struct EditOutcome {
    pub new_content: String,
    /// old_text 在原文中的出现次数（`replace_all` 计数文案用；级联命中
    /// 路径恒 0——级联的命中语义由 level 名承载）。
    pub occurrences: usize,
    /// A4 级联命中级名（exact 命中为 None）。
    pub cascade_level: Option<&'static str>,
}

/// 对 `content` 应用一条替换决策。`path_disp` 仅用于错误文案。
pub(crate) fn apply_edit_to_content(
    content: &str,
    old_text: &str,
    new_text: &str,
    replace_all: bool,
    path_disp: &str,
) -> Result<EditOutcome, String> {
    let count = content.matches(old_text).count();
    if count > 1 && !replace_all {
        let lines = edit_hint::match_line_numbers(content, old_text);
        return Err(format!(
            "old_text appears {} times in {} (at lines {:?}). Provide more \
             surrounding context to make it unique, or use replace_all=true to \
             replace every occurrence",
            count, path_disp, lines
        ));
    }

    if count == 0 {
        if replace_all {
            return Err(format!(
                "old_text not found in {}. {}",
                path_disp,
                edit_hint::build_not_found_hint(content, old_text)
            ));
        }
        // A4：exact 未命中且非 replace_all → 五级模糊替换级联。replace_all 不进
        // 级联：模糊命中位置内容各不相同，"全部替换"语义无法推广。
        return match edit_replacers::cascade_replace(content, old_text, new_text) {
            Ok(m) => Ok(EditOutcome {
                new_content: m.content,
                occurrences: 0,
                cascade_level: Some(m.level),
            }),
            Err(edit_replacers::CascadeError::Ambiguous(msg)) => Err(msg),
            Err(edit_replacers::CascadeError::NoMatch { span_note }) => {
                let mut msg = format!("old_text not found in {}. ", path_disp);
                if let Some(note) = span_note {
                    msg.push_str(&note);
                }
                msg.push_str(&edit_hint::build_not_found_hint(content, old_text));
                Err(msg)
            }
        };
    }

    // A3：replace_all=true → 全部替换。注意 Rust 语义：`replacen(…, 0)` 是
    // 「最多换 0 处」（什么都不换）而非全量——全量是 `str::replace`
    // （实施计划该处笔误，已按 §1.1 核实现修订）。
    let new_content = if replace_all {
        content.replace(old_text, new_text)
    } else {
        content.replacen(old_text, new_text, 1)
    };
    Ok(EditOutcome {
        new_content,
        occurrences: count,
        cascade_level: None,
    })
}

/// A tool that edits a file by replacing old_text with new_text.
///
/// By default old_text must occur exactly once; `replace_all=true` replaces
/// every occurrence.
///
/// A5（2026-09-04）：可携带工作区边界（`with_boundary`，生产形态）；
/// `default()` 无界（基线注册 / 测试形态，行为同边界引入前）。
#[derive(Default)]
pub struct EditFileTool {
    boundary: Option<Arc<WorkspaceBoundary>>,
}

impl EditFileTool {
    /// 带工作区边界的生产形态。
    pub fn with_boundary(boundary: Arc<WorkspaceBoundary>) -> Self {
        Self {
            boundary: Some(boundary),
        }
    }
}

#[async_trait]
impl Tool for EditFileTool {
    fn description(&self) -> String {
        "Edit a file by replacing old_text with new_text. By default old_text must \
         occur exactly once; set replace_all=true to replace every occurrence."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Path to the file to edit"},
                "old_text": {"type": "string", "description": "Exact text to find and replace"},
                "new_text": {"type": "string", "description": "Replacement text"},
                "replace_all": {"type": "boolean", "description": "Replace every occurrence of old_text instead of requiring a unique match (default: false)"}
            },
            "required": ["path", "old_text", "new_text"]
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let (path, old_text, new_text, replace_all) = extract_edit_args(args)?;

        // A5（2026-09-04）：工作区边界（纵深防御，不单靠安全 8 层管线）。
        let path = match &self.boundary {
            Some(b) => validate_workspace_path(&path, b)?,
            None => PathBuf::from(&path),
        };
        let path = Path::new(&path);

        if !path.exists() {
            return Err(format!("File not found: {}", path.display()));
        }

        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| format!("Failed to read file: {}", e))?;

        // A1/A3/A4 匹配语义已抽出为 [`apply_edit_to_content`]（A7 起与
        // multiedit 共用单一真相源）；此处只做 IO 与回执组装。
        let outcome = apply_edit_to_content(
            &content,
            &old_text,
            &new_text,
            replace_all,
            &path.to_string_lossy(),
        )?;

        tokio::fs::write(path, &outcome.new_content)
            .await
            .map_err(|e| format!("Failed to write file: {}", e))?;

        // A2（2026-09-04）：成功回 unified diff（模型自检 + 前端渲染 +
        // 审计可读；超长回退统计摘要，全量语义由 loop 层 spill 兜底）。
        let diff = edit_hint::unified_diff(&path.to_string_lossy(), &content, &outcome.new_content);
        // A3：replace_all 在头部报替换计数（count>=1 恒成立——0 已被 not-found
        // 拦截；无差异时也报计数，诚实反映发生了什么）。
        let header = if let Some(level) = outcome.cascade_level {
            format!("File edited: {} (matched via {})", path.display(), level)
        } else if replace_all {
            format!(
                "File edited: {} ({} occurrences replaced)",
                path.display(),
                outcome.occurrences
            )
        } else {
            format!("File edited: {}", path.display())
        };
        if diff.is_empty() {
            return Ok(header);
        }
        Ok(format!("{}\n```diff\n{}```", header, diff))
    }

    fn preview(&self, args: &str) -> Option<FileChange> {
        let (path, _old_text, _new_text, _replace_all) = extract_edit_args(args).ok()?;
        // edit_file requires the file to exist (execute errors otherwise).
        Some(FileChange {
            path,
            kind: FileChangeKind::Modify,
        })
    }
}

/// A7（2026-09-06）：multiedit 批量编辑——`edits: [{path, old_text,
/// new_text, replace_all?}, ...]`，**原子性**：全部编辑先在内存完成（同
/// 文件多条编辑按序累积生效），任一失败则整批不落盘并回灌首个失败详情 +
/// 逐条状态清单；全部成功才统一写盘（只写内容实际变化的文件），逐文件
/// 回 unified diff。匹配语义与 `edit_file` 共用 [`apply_edit_to_content`]
/// 单一真相源。
///
/// A5（2026-09-04）同款边界形态：`with_boundary` 生产 / `default()` 基线。
#[derive(Default)]
pub struct MultiEditTool {
    boundary: Option<Arc<WorkspaceBoundary>>,
}

/// 一条批量编辑条目（schema 校验后的形态）。
struct MultiEditEntry {
    path: String,
    old_text: String,
    new_text: String,
    replace_all: bool,
}

/// 解析 multiedit args：`edits` 必须是非空数组，每条必须带字符串
/// `path`/`old_text`/`new_text`（`replace_all` 可选布尔）。
fn extract_multiedit_args(args: &str) -> Result<Vec<MultiEditEntry>, String> {
    let val: serde_json::Value =
        serde_json::from_str(args).map_err(|e| format!("Invalid JSON args: {}", e))?;
    let arr = val
        .get("edits")
        .and_then(|v| v.as_array())
        .ok_or("Missing 'edits' field (must be a non-empty array)")?;
    if arr.is_empty() {
        return Err("'edits' must be a non-empty array".to_string());
    }
    arr.iter()
        .enumerate()
        .map(|(i, e)| {
            let need = |k: &str| {
                e.get(k)
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .ok_or_else(|| format!("edits[{}]: missing string field '{}'", i, k))
            };
            Ok(MultiEditEntry {
                path: need("path")?,
                old_text: need("old_text")?,
                new_text: need("new_text")?,
                replace_all: e
                    .get("replace_all")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
            })
        })
        .collect()
}

impl MultiEditTool {
    /// 带工作区边界的生产形态。
    pub fn with_boundary(boundary: Arc<WorkspaceBoundary>) -> Self {
        Self {
            boundary: Some(boundary),
        }
    }
}

#[async_trait]
impl Tool for MultiEditTool {
    fn description(&self) -> String {
        "Apply multiple text replacements across one or more files in a single \
         atomic operation. All edits are applied in memory first; if any edit \
         fails, nothing is written to disk. Multiple edits to the same file are \
         applied sequentially (later edits see earlier results)."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "edits": {
                    "type": "array",
                    "description": "Edits to apply atomically",
                    "items": {
                        "type": "object",
                        "properties": {
                            "path": {"type": "string", "description": "Path to the file to edit"},
                            "old_text": {"type": "string", "description": "Exact text to find and replace"},
                            "new_text": {"type": "string", "description": "Replacement text"},
                            "replace_all": {"type": "boolean", "description": "Replace every occurrence of old_text (default: false)"}
                        },
                        "required": ["path", "old_text", "new_text"]
                    }
                }
            },
            "required": ["edits"]
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let edits = extract_multiedit_args(args)?;
        let total = edits.len();

        // 内存工作区：path → (原文, 当前内容, 已应用条数, 级联名)。
        // 同一文件的多条编辑按序累积（后者看到前者的结果）。
        struct MemFile {
            path: PathBuf,
            original: String,
            current: String,
            edits_applied: usize,
            levels: Vec<&'static str>,
        }
        let mut files: Vec<MemFile> = Vec::new();
        let mut statuses: Vec<String> = Vec::with_capacity(total);

        for (i, e) in edits.iter().enumerate() {
            let label = format!("edit {}/{}", i + 1, total);
            // A5：工作区边界（纵深防御，与 edit_file 同源校验）。
            let vpath = match &self.boundary {
                Some(b) => match validate_workspace_path(&e.path, b) {
                    Ok(p) => p,
                    Err(m) => {
                        statuses.push(format!("  {}. {} — FAILED: {}", i + 1, e.path, m));
                        return Err(format!(
                            "multiedit aborted at {}: {}\n\nNo files were \
                             modified. Edit status:\n{}",
                            label,
                            m,
                            statuses.join("\n")
                        ));
                    }
                },
                None => PathBuf::from(&e.path),
            };
            let idx = match files.iter().position(|f| f.path == vpath) {
                Some(i) => i,
                None => {
                    if !vpath.exists() {
                        let m = format!("File not found: {}", vpath.display());
                        statuses.push(format!("  {}. {} — FAILED: {}", i + 1, e.path, m));
                        return Err(format!(
                            "multiedit aborted at {}: {}\n\nNo files were \
                             modified. Edit status:\n{}",
                            label,
                            m,
                            statuses.join("\n")
                        ));
                    }
                    let content = match tokio::fs::read_to_string(&vpath).await {
                        Ok(c) => c,
                        Err(err) => {
                            let m = format!("Failed to read {}: {}", vpath.display(), err);
                            statuses.push(format!("  {}. {} — FAILED: {}", i + 1, e.path, m));
                            return Err(format!(
                                "multiedit aborted at {}: {}\n\nNo files were \
                                 modified. Edit status:\n{}",
                                label,
                                m,
                                statuses.join("\n")
                            ));
                        }
                    };
                    files.push(MemFile {
                        path: vpath,
                        original: content.clone(),
                        current: content,
                        edits_applied: 0,
                        levels: Vec::new(),
                    });
                    files.len() - 1
                }
            };
            // 匹配语义单一真相源（与 edit_file 同函数）；失败整批中止。
            match apply_edit_to_content(
                &files[idx].current,
                &e.old_text,
                &e.new_text,
                e.replace_all,
                &files[idx].path.to_string_lossy(),
            ) {
                Ok(outcome) => {
                    if let Some(level) = outcome.cascade_level {
                        files[idx].levels.push(level);
                    }
                    files[idx].current = outcome.new_content;
                    files[idx].edits_applied += 1;
                    statuses.push(format!("  {}. {} — ok", i + 1, e.path));
                }
                Err(m) => {
                    statuses.push(format!("  {}. {} — FAILED: {}", i + 1, e.path, m));
                    return Err(format!(
                        "multiedit aborted at {}: {}\n\nNo files were modified. \
                         Edit status:\n{}",
                        label,
                        m,
                        statuses.join("\n")
                    ));
                }
            }
        }

        // 全部成功 → 统一写盘（只写内容实际变化的文件，避免无谓 mtime）。
        let mut written: Vec<String> = Vec::new();
        for f in &files {
            if f.current == f.original {
                continue;
            }
            if let Err(err) = tokio::fs::write(&f.path, &f.current).await {
                // 磁盘写阶段失败无跨文件事务——诚实报告已写清单。
                return Err(format!(
                    "multiedit write failed on {}: {}. Files already written: {}",
                    f.path.display(),
                    err,
                    if written.is_empty() {
                        "(none)".to_string()
                    } else {
                        written.join(", ")
                    }
                ));
            }
            written.push(f.path.to_string_lossy().to_string());
        }

        // 回执：总数 + 逐文件 diff 汇总。
        let mut out = format!(
            "Multiedit complete: {} edit(s) across {} file(s).",
            total,
            files.len()
        );
        for f in &files {
            out.push_str(&format!(
                "\n\n── {} ({} edit(s)",
                f.path.display(),
                f.edits_applied
            ));
            if !f.levels.is_empty() {
                out.push_str(&format!(", matched via {}", f.levels.join(", ")));
            }
            out.push(')');
            let diff = edit_hint::unified_diff(&f.path.to_string_lossy(), &f.original, &f.current);
            if diff.is_empty() {
                out.push_str("\n(no change)");
            } else {
                out.push_str(&format!("\n```diff\n{}```", diff));
            }
        }
        Ok(out)
    }

    fn preview(&self, _args: &str) -> Option<FileChange> {
        // multiedit 的 checkpoint 预检走 [`Tool::preview_all`]（多文件）；
        // 单点 `preview` 无从选代表文件，诚实返回 None。
        None
    }

    fn preview_all(&self, args: &str) -> Vec<FileChange> {
        let Ok(edits) = extract_multiedit_args(args) else {
            return Vec::new();
        };
        // 去重保序（同文件多条编辑只快照一次——checkpoint seen 表本就幂等）。
        let mut seen = std::collections::HashSet::new();
        edits
            .into_iter()
            .filter(|e| seen.insert(e.path.clone()))
            .map(|e| FileChange {
                path: e.path,
                kind: FileChangeKind::Modify,
            })
            .collect()
    }
}

/// A tool that appends content to the end of a file.
/// A tool that appends content to the end of a file.
///
/// A5（2026-09-04）：可携带工作区边界（`with_boundary`，生产形态）；
/// `default()` 无界（基线注册 / 测试形态，行为同边界引入前）。
#[derive(Default)]
pub struct AppendFileTool {
    boundary: Option<Arc<WorkspaceBoundary>>,
}

impl AppendFileTool {
    /// 带工作区边界的生产形态。
    pub fn with_boundary(boundary: Arc<WorkspaceBoundary>) -> Self {
        Self {
            boundary: Some(boundary),
        }
    }
}

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

        // A5（2026-09-04）：工作区边界（纵深防御，不单靠安全 8 层管线）。
        let path = match &self.boundary {
            Some(b) => validate_workspace_path(&path, b)?,
            None => PathBuf::from(&path),
        };
        let path = path.as_path();

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
        // tokio fs::File 的 write_all 只提交到后台任务，不 flush 则数据可能
        // 尚未写入文件（drop 不等待未完成 IO）。
        file.flush()
            .await
            .map_err(|e| format!("Failed to flush file: {}", e))?;

        Ok(format!(
            "Appended {} bytes to {}",
            content.len(),
            path.display()
        ))
    }

    fn preview(&self, args: &str) -> Option<FileChange> {
        let (path, _content) = extract_path_and_content(args).ok()?;
        let kind = if Path::new(&path).exists() {
            FileChangeKind::Modify
        } else {
            FileChangeKind::Create
        };
        Some(FileChange { path, kind })
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
            return Err(format!(
                "Path is a directory, not a file: {}",
                path.display()
            ));
        }

        tokio::fs::remove_file(path)
            .await
            .map_err(|e| format!("Failed to delete file: {}", e))?;

        Ok(format!("Deleted file: {}", path.display()))
    }

    fn preview(&self, args: &str) -> Option<FileChange> {
        let path = extract_path(args).ok()?;
        Some(FileChange {
            path,
            kind: FileChangeKind::Delete,
        })
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
///
/// B1（2026-09-04）输出留存契约：本工具**不做工具内截断**——全量输出返回给
/// agent loop，由 loop 层两档闸统一治理（`spill.rs` ≥65536 字符全量落盘
/// `<workspace>/logs/spill/` + locator 回灌；`prune.rs` 8193..65535 head+tail
/// 内联收窄）。工具层再截一刀会让全量字节在生产路径上不可恢复，且与
/// loop 闸双重截断；如需调整阈值改 spill/prune 常量，勿在本工具加截断。
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
        serde_json::json!({"type":"object","properties":{"command":{"type":"string","description":"Command to execute"},"timeout":{"type":"integer","description":"Timeout in seconds (default 30, max 600)"},"cwd":{"type":"string","description":"Working directory"}},"required":["command"]})
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let val: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid JSON arguments: {}", e))?;

        let command = val
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or("Missing 'command' argument")?;

        let timeout_secs = exec_timeout_secs(val.get("timeout").and_then(|v| v.as_u64()));

        let cwd = val
            .get("cwd")
            .and_then(|v| v.as_str())
            .unwrap_or(&self.workspace);

        // Workspace restriction check
        if self.restrict {
            // 与 ExecTool（shell.rs）对齐：相对 cwd join workspace；双侧统一
            // 归一化（nemesis-path 单一真相源）——组件级比较防前缀误判，
            // workspace 侧归一化防 8.3 短名/大小写失配（2026-09-01）。
            let target = if Path::new(cwd).is_absolute() {
                PathBuf::from(cwd)
            } else {
                Path::new(&self.workspace).join(cwd)
            };
            let resolved = canonicalize_for_compare(&target);
            let ws = canonicalize_for_compare(Path::new(&self.workspace));
            if !resolved.starts_with(&ws) {
                return Err(format!(
                    "Access denied: path '{}' is outside workspace",
                    cwd
                ));
            }
        }

        // B2（2026-09-04）：stdout/stderr 改为显式 piped——超时收尸后要从管道
        // 缓冲区读回进程被杀前已产生的残余输出（旧 `cmd.output()` 的管道随
        // future 取消一起丢弃，超时即全丢）。stdin 保持 null（见下）。
        // C8：平台 shell 形态抽到 `make_piped_shell_command`（与 run_checks
        // 共用单一真相源）。
        let mut cmd = make_piped_shell_command(command);

        let mut child = cmd
            .current_dir(cwd)
            .spawn()
            .map_err(|e| format!("Failed to execute command: {}", e))?;

        // B2：管道读取必须**独立任务并发**进行——
        // ① 不能只 `child.wait()`：子进程写满 ~64KB 管道缓冲会阻塞在 write
        //    上永不退出，正常长输出路径会整体退化成超时；
        // ② 不能把管道移进被 timeout 包住的 future：超时取消时管道随 future
        //    一起 drop，残余输出照样丢。任务持管道所有权，与 wait 竞争无关；
        //    进程死后管道 EOF，任务自然收尾。
        let out_task = tokio::spawn(drain_pipe(child.stdout.take()));
        let err_task = tokio::spawn(drain_pipe(child.stderr.take()));

        let output =
            tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), child.wait()).await;

        match output {
            Ok(Ok(status)) => {
                let stdout = String::from_utf8_lossy(&join_pipe_task(out_task).await).to_string();
                let stderr = String::from_utf8_lossy(&join_pipe_task(err_task).await).to_string();
                if status.success() {
                    Ok(if stdout.is_empty() {
                        "(no output)".to_string()
                    } else {
                        stdout
                    })
                } else {
                    Ok(format!(
                        "Exit code: {}\nstdout: {}\nstderr: {}",
                        status.code().unwrap_or(-1),
                        stdout,
                        stderr
                    ))
                }
            }
            Ok(Err(e)) => Err(format!("Failed to execute command: {}", e)),
            Err(_) => {
                // B2：超时改 Ok 回灌（模型可自纠），先收尸再读残余输出。
                // 管道任务持有所有权不受本次取消影响；kill + reap 后管道
                // EOF，任务收尾。任务 await 兜 5s 上限：若命令 fork 了继承
                // 管道的后台孙进程，EOF 要等孙进程死——此时如实返回已读不到
                // 的空残余（旧实现同场景连收尸都没有，此处严格更优）。
                let _ = child.start_kill();
                let _ = child.wait().await;
                let stdout = String::from_utf8_lossy(&join_pipe_task(out_task).await).to_string();
                let stderr = String::from_utf8_lossy(&join_pipe_task(err_task).await).to_string();

                let mut partial = String::new();
                if !stdout.trim().is_empty() {
                    partial.push_str("stdout (tail):\n");
                    partial.push_str(&tail_chars(&stdout, 2048));
                }
                if !stderr.trim().is_empty() {
                    if !partial.is_empty() {
                        partial.push('\n');
                    }
                    partial.push_str("stderr (tail):\n");
                    partial.push_str(&tail_chars(&stderr, 2048));
                }
                if partial.is_empty() {
                    partial.push_str("(no output before timeout)");
                }
                Ok(format!(
                    "Command timed out after {} seconds. Partial output:\n{}\nTip: the \
                     command may be waiting for interactive input — on Windows \
                     `date`/`time` are interactive (use `date /t`/`time /t` or \
                     PowerShell Get-Date). Raise the timeout for genuinely long \
                     commands or narrow the command. Command was: {}",
                    timeout_secs, partial, command
                ))
            }
        }
    }
}

/// B2（2026-09-04）：exec 超时解析——默认 30s、上限 600s（对齐
/// nemesis-tools shell.rs 的 cap）。模型传 999999 之类不再被照单全收。
fn exec_timeout_secs(v: Option<u64>) -> u64 {
    v.unwrap_or(30).min(600)
}

/// C8（2026-09-06）：平台 shell 形态的单一真相源——ExecTool 与 RunChecksTool
/// 共用。逐字保留 B2 语义：Windows `cmd /C` + raw_arg（.arg() 的自动加引号
/// 会搅乱 cmd.exe 自身的引号处理）；stdin null（交互式命令立即 EOF 不挂满
/// 超时）；stdout/stderr piped；kill_on_drop（挂死命令不留孤儿）。
fn make_piped_shell_command(command: &str) -> tokio::process::Command {
    #[cfg(target_os = "windows")]
    {
        #[allow(unused_imports)]
        use std::os::windows::process::CommandExt;
        let mut c = tokio::process::Command::new("cmd");
        c.raw_arg(format!("/C {}", command));
        c.stdin(std::process::Stdio::null());
        c.stdout(std::process::Stdio::piped());
        c.stderr(std::process::Stdio::piped());
        c.kill_on_drop(true);
        c
    }
    #[cfg(not(target_os = "windows"))]
    {
        let mut c = tokio::process::Command::new("sh");
        c.arg("-c").arg(command);
        c.stdin(std::process::Stdio::null());
        c.stdout(std::process::Stdio::piped());
        c.stderr(std::process::Stdio::piped());
        c.kill_on_drop(true);
        c
    }
}

/// B2：排干管道到字节缓冲（pipe 缺失时返空）。
async fn drain_pipe(pipe: Option<impl tokio::io::AsyncRead + Unpin>) -> Vec<u8> {
    match pipe {
        Some(mut p) => {
            let mut buf = Vec::new();
            let _ = tokio::io::AsyncReadExt::read_to_end(&mut p, &mut buf).await;
            buf
        }
        None => Vec::new(),
    }
}

/// B2：收管道读取任务的结果；兜 5s 上限（孙进程继承管道 EOF 不达时不再挂）。
async fn join_pipe_task(handle: tokio::task::JoinHandle<Vec<u8>>) -> Vec<u8> {
    tokio::time::timeout(std::time::Duration::from_secs(5), handle)
        .await
        .ok()
        .and_then(|r| r.ok())
        .unwrap_or_default()
}

/// B2：超时残余输出的字符安全尾部截取（多字节安全，不按字节切）。
fn tail_chars(s: &str, max_chars: usize) -> String {
    let total = s.chars().count();
    if total <= max_chars {
        s.to_string()
    } else {
        s.chars().skip(total - max_chars).collect()
    }
}

// ---------------------------------------------------------------------------
// C8（2026-09-06 devtool-upgrade 阶段 4）：run_checks 构建/测试运行器
// ---------------------------------------------------------------------------

/// 项目类型探测结果（探测顺序即数组顺序，首个命中者胜）。
const CHECK_ECOSYSTEMS: &[(&str, &str)] = &[
    ("rust", "Cargo.toml"),
    ("node", "package.json"),
    ("go", "go.mod"),
    ("python", "pyproject.toml"),
    ("maven", "pom.xml"),
];

/// 各生态 × scope 的命令映射（`filter` 只对 test 有意义的生态追加）。
/// 返回 (阶段标签, 命令) 有序对——`all` 按 build→test→lint 顺序排列。
/// 故意**不做**映射的格子返回 None——回灌诚实报「未映射」，不硬猜命令。
fn checks_commands(
    eco: &str,
    scope: &str,
    filter: Option<&str>,
) -> Option<Vec<(&'static str, String)>> {
    let filter = filter.map(str::trim).filter(|s| !s.is_empty());
    match (eco, scope) {
        ("rust", "build") => Some(vec![("build", "cargo build --message-format short".into())]),
        ("rust", "test") => Some(vec![(
            "test",
            match filter {
                Some(f) => format!("cargo test {f}"),
                None => "cargo test".into(),
            },
        )]),
        ("rust", "lint") => Some(vec![("lint", "cargo clippy --message-format short".into())]),
        ("rust", "all") => Some(vec![
            ("build", "cargo build --message-format short".into()),
            ("test", "cargo test".into()),
            ("lint", "cargo clippy --message-format short".into()),
        ]),
        ("node", "build") => Some(vec![("build", "npm run build".into())]),
        ("node", "test") => Some(vec![("test", "npm test".into())]),
        ("node", "lint") => Some(vec![("lint", "npm run lint".into())]),
        ("node", "all") => Some(vec![
            ("build", "npm run build".into()),
            ("test", "npm test".into()),
            ("lint", "npm run lint".into()),
        ]),
        ("go", "build") => Some(vec![("build", "go build ./...".into())]),
        ("go", "test") => Some(vec![(
            "test",
            match filter {
                Some(f) => format!("go test -run {f} ./..."),
                None => "go test ./...".into(),
            },
        )]),
        ("go", "lint") => Some(vec![("lint", "go vet ./...".into())]),
        ("go", "all") => Some(vec![
            ("build", "go build ./...".into()),
            ("test", "go test ./...".into()),
            ("lint", "go vet ./...".into()),
        ]),
        ("python", "test") => Some(vec![(
            "test",
            match filter {
                Some(f) => format!("pytest -q -k {f}"),
                None => "pytest -q".into(),
            },
        )]),
        ("python", "lint") => Some(vec![("lint", "ruff check .".into())]),
        // python 无标准 build 命令——不猜。
        ("maven", "build") => Some(vec![("build", "mvn -q compile".into())]),
        ("maven", "test") => Some(vec![(
            "test",
            match filter {
                Some(f) => format!("mvn -q test -Dtest={f}"),
                None => "mvn -q test".into(),
            },
        )]),
        // maven 无独立 lint——checkstyle 插件不假设。
        _ => None,
    }
}

/// 在目录下探测项目类型（首个命中的标记文件决定生态）。
fn detect_project_eco(root: &Path) -> Option<&'static str> {
    for (eco, marker) in CHECK_ECOSYSTEMS {
        if root.join(marker).is_file() {
            return Some(eco);
        }
    }
    None
}

/// 相邻 token 对（数字 + 计数词）扫描：返回 (passed, failed, ignored, any)。
/// 容忍段内装饰 token（cargo 的 `ok.` 前缀、pytest 的 `====` 围栏）——数字
/// 对出现在哪一对都收。pytest 的 `N error` 计入 failed。
fn count_labeled_numbers(toks: &[&str]) -> (u64, u64, u64, bool) {
    let mut p = 0u64;
    let mut f = 0u64;
    let mut i = 0u64;
    let mut any = false;
    for w in toks.windows(2) {
        let Ok(num) = w[0].parse::<u64>() else {
            continue;
        };
        match w[1] {
            l if l.starts_with("passed") => {
                p += num;
                any = true;
            }
            l if l.starts_with("failed") || l.starts_with("error") => {
                f += num;
                any = true;
            }
            l if l.starts_with("ignored") || l.starts_with("skipped") => {
                i += num;
                any = true;
            }
            _ => {}
        }
    }
    (p, f, i, any)
}

/// 从输出里聚合结构化测试统计（passed, failed, ignored）。只解析**有结构**
/// 的汇总行，解析不出来返 None（不编造数字）：
/// - cargo：多段 `test result: ok. 3 passed; 0 failed; ...` 全部累加（lib +
///   多 bin 各一段）。逐段容错——`ok.` 前缀等非数字 token 跳过，不毒化整
///   个函数（曾有 `?` 连坐 bug：一段垃圾 token 让全部统计变 None）。
/// - pytest：`===== N passed, M failed, 1 skipped in 0.5s =====` 汇总行（取
///   最后一行；`====` 装饰 token 不在数字位则天然跳过）。
fn parse_test_stats(output: &str) -> Option<(u64, u64, u64)> {
    let mut passed = 0u64;
    let mut failed = 0u64;
    let mut ignored = 0u64;
    let mut saw_cargo = false;
    for line in output.lines() {
        if let Some(rest) = line.trim().strip_prefix("test result:") {
            saw_cargo = true;
            for part in rest.split(';') {
                let toks: Vec<&str> = part.split_whitespace().collect();
                let (p, f, i, _) = count_labeled_numbers(&toks);
                passed += p;
                failed += f;
                ignored += i;
            }
        }
    }
    if saw_cargo {
        return Some((passed, failed, ignored));
    }
    // pytest 汇总：取匹配的最后一行。
    let mut pytest: Option<(u64, u64, u64)> = None;
    for line in output.lines() {
        if !(line.contains("passed") || line.contains("failed")) {
            continue;
        }
        let toks: Vec<&str> = line.split_whitespace().collect();
        let (p, f, i, any) = count_labeled_numbers(&toks);
        if any {
            pytest = Some((p, f, i));
        }
    }
    pytest
}

/// 失败签名行提取（去重 + 计数，保序，cap 条）。聚焦回灌的核心：模型先看
/// 到错误行，全量在 spill 文件里。签名清单保守——只收确定是失败信号的行。
fn extract_failure_lines(output: &str, cap: usize) -> Vec<String> {
    const SIGNATURES: &[&str] = &[
        "error[",              // rustc E---- 结构化错误
        "error:",              // rustc / clippy / go / npm 通用
        "test result: FAILED", // cargo test 失败汇总行
        "---- ",               // cargo test 失败用例头（`---- name stdout ----`）
        "FAILED",              // pytest FAILURES / npm
        "AssertionError",
        "SyntaxError",
        "ELIFECYCLE",
        "panicked",
        "✗",
    ];
    let mut out: Vec<String> = Vec::new();
    for line in output.lines() {
        let t = line.trim_end();
        if t.is_empty() {
            continue;
        }
        if SIGNATURES.iter().any(|s| t.contains(s)) {
            // 同一行重复（如多 target 重编）→ 附加计数提示（best-effort，
            // 只做降噪不追求精确次数）。
            if let Some(pos) = out.iter().position(|e| e == t) {
                out[pos] = format!("{t}  (×2)");
                continue;
            }
            if out.len() >= cap {
                out.push(format!(
                    "…（更多失败行省略，共 {} 行签名，全文见下方存档）",
                    cap
                ));
                break;
            }
            out.push(t.to_string());
        }
    }
    out
}

/// 构建产物/测试运行的输出聚焦回灌工具。
///
/// 与 `exec` 的差异：exec 是自由命令，输出全量回灌（≥64KB 才 spill）；
/// run_checks 是**固定命令表**（按项目类型 × scope 映射），回灌只有统计 +
/// 失败签名行摘录，全量输出无条件存档到 spill（`save_tool_output`），模型
/// 要细节时再 read_file/grep 检索——编译/测试输出动辄几千行，全量回灌烧
/// context 且噪声淹没真正的 error 行。
///
/// 注册在 `register_shared_tools`（有 workspace 时）；进 MOVE_TOOLS（构建
/// 会写 target/，executor 分离/沙盒应罩住它；plan 模式下供给与 dispatch
/// 双闸都拦）；tier Normal/Big（Mini 不给——exec 已够）。
pub struct RunChecksTool {
    workspace: String,
}

impl RunChecksTool {
    pub fn new(workspace: &str) -> Self {
        Self {
            workspace: workspace.to_string(),
        }
    }
}

#[async_trait]
impl Tool for RunChecksTool {
    fn description(&self) -> String {
        "Run project build/test/lint checks with output focusing: replies contain pass/fail \
         stats plus deduplicated error-signature lines only; the full output is archived to \
         a file (path included in the reply) for read_file/grep retrieval. Use this instead \
         of `exec` for builds and test suites."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type":"object",
            "properties":{
                "scope":{"type":"string","enum":["build","test","lint","all"],"description":"Which checks to run (default \"all\": build → test → lint, stops at first failing stage)"},
                "filter":{"type":"string","description":"Optional single-test filter passed to the test command (e.g. cargo test <name>, pytest -k <name>)"},
                "timeout":{"type":"integer","description":"Timeout in seconds (default 600, max 600)"}
            }
        })
    }

    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String> {
        let val: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid JSON arguments: {e}"))?;
        let scope = val
            .get("scope")
            .and_then(|v| v.as_str())
            .unwrap_or("all")
            .to_lowercase();
        let filter = val
            .get("filter")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let timeout_secs = val
            .get("timeout")
            .and_then(|v| v.as_u64())
            .unwrap_or(600)
            .min(600);

        // 项目类型探测：无标记 → 诚实报错（列出查过的标记文件）。
        let root = Path::new(&self.workspace);
        let Some(eco) = detect_project_eco(root) else {
            let markers: Vec<&str> = CHECK_ECOSYSTEMS.iter().map(|(_, m)| *m).collect();
            return Err(format!(
                "No recognizable project at workspace root (checked: {}). run_checks only \
                 maps commands for detected project types — use `exec` for anything else.",
                markers.join(", ")
            ));
        };
        let Some(stages) = checks_commands(eco, &scope, filter.as_deref()) else {
            return Err(format!(
                "Scope \"{scope}\" is not mapped for {eco} projects (mapped scopes differ \
                 per ecosystem). Use `exec` with the project's own command instead."
            ));
        };

        // 逐阶段执行；失败早停（build 失败后跑 test 只会复读同一批错误）。
        let mut full = String::new();
        let mut summary = Vec::new();
        let mut failed_stage = false;
        for (label, command) in stages {
            let started = std::time::Instant::now();
            let (exit_code, stdout, stderr, timed_out) =
                run_one_stage(&command, Some(root), timeout_secs).await;
            let elapsed = started.elapsed().as_secs_f32();
            let combined =
                format!("$ {command}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}\n");
            full.push_str(&combined);
            let stats = parse_test_stats(&combined);
            if timed_out {
                summary.push(format!(
                    "⏱ {label}: TIMED OUT after {timeout_secs}s ({elapsed:.1}s elapsed)"
                ));
                failed_stage = true;
                break;
            }
            let stats_note = match stats {
                Some((p, f, i)) => format!(" — passed {p} / failed {f} / skipped {i}"),
                None => String::new(),
            };
            match exit_code {
                Some(0) => summary.push(format!("▶ {label}: exit 0 ({elapsed:.1}s){stats_note}")),
                Some(code) => {
                    summary.push(format!(
                        "✗ {label}: exit {code} ({elapsed:.1}s){stats_note}"
                    ));
                    failed_stage = true;
                }
                None => {
                    summary.push(format!("✗ {label}: spawn failed ({elapsed:.1}s)"));
                    failed_stage = true;
                }
            }
            if failed_stage {
                break;
            }
        }

        // 全量存档（无条件；失败诚实注明，聚焦部分照常回灌）。
        let session_key = &context.session_key;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis().to_string())
            .unwrap_or_default();
        let spill_root = nemesis_path::resolve_spill_dir_in_workspace(root);
        let archive = crate::spill::save_tool_output(
            &full,
            &spill_root,
            session_key,
            &stamp,
            &format!("run_checks_{scope}"),
        );
        let archive_note = match archive {
            Ok(path) => format!(
                "\n[完整输出（{} 字符）已保存：{} —— read_file/grep 可检索]",
                full.chars().count(),
                path.display()
            ),
            Err(e) => format!("\n[完整输出存档失败：{e}——上方摘录即全部可查内容]"),
        };

        // 聚焦回灌：统计行 + 失败签名摘录（去重计数）。
        let focus = extract_failure_lines(&full, 40);
        let mut reply = format!(
            "[run_checks] scope={scope} · {eco}\n{}\n",
            summary.join("\n")
        );
        if focus.is_empty() {
            if failed_stage {
                reply.push_str("（exit 非零但未匹配到失败签名行——细节见完整输出）\n");
            } else {
                reply.push_str("全部通过，无失败签名行。\n");
            }
        } else {
            reply.push_str(&format!(
                "失败签名行（去重后 {} 行）：\n  {}\n",
                focus.len(),
                focus.join("\n  ")
            ));
        }
        reply.push_str(&archive_note);
        // 失败提示：单测重跑（计划验收要求的指引）。
        if failed_stage {
            reply.push_str(
                "\n提示：重跑单个检查用 run_checks（scope=test + filter），或用 exec 跑带 \
                 参数的完整命令。",
            );
        }
        Ok(reply)
    }
}

/// C8：跑单个检查阶段，返回 (exit_code, stdout, stderr, timed_out)。
/// exit_code None = 进程没能启动（程序不存在等）。spawn 形态与超时收尸
/// 语义复用 B2 exec 内核（`make_piped_shell_command` + 并发管道排水 +
/// kill 后读残余——此处超时走独立返回值，不混进输出文本）。
/// K3：改为 `pub(crate)` + `cwd: Option`——`` !`cmd` `` 注入（loop.rs
/// rewrite_custom_command）共用同一执行内核；cwd None = 不设（继承进程）。
pub(crate) async fn run_one_stage(
    command: &str,
    cwd: Option<&Path>,
    timeout_secs: u64,
) -> (Option<i32>, String, String, bool) {
    let mut cmd = make_piped_shell_command(command);
    if let Some(d) = cwd {
        cmd.current_dir(d);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return (None, String::new(), format!("failed to spawn: {e}"), false);
        }
    };
    let out_task = tokio::spawn(drain_pipe(child.stdout.take()));
    let err_task = tokio::spawn(drain_pipe(child.stderr.take()));
    let waited =
        tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), child.wait()).await;
    match waited {
        Ok(Ok(status)) => {
            let stdout = String::from_utf8_lossy(&join_pipe_task(out_task).await).to_string();
            let stderr = String::from_utf8_lossy(&join_pipe_task(err_task).await).to_string();
            (status.code(), stdout, stderr, false)
        }
        Ok(Err(e)) => (None, String::new(), format!("wait failed: {e}"), false),
        Err(_) => {
            // 超时：先收尸再读残余输出（B2 同款——管道任务持所有权不受
            // 取消影响，kill 后 EOF 收尾）。
            let _ = child.start_kill();
            let _ = child.wait().await;
            let stdout = String::from_utf8_lossy(&join_pipe_task(out_task).await).to_string();
            let stderr = String::from_utf8_lossy(&join_pipe_task(err_task).await).to_string();
            (None, stdout, stderr, true)
        }
    }
}

/// Execute a script with a caller-chosen interpreter (`interpreter [flag] script`).
///
/// Registered as `"run_script"` and listed in [`MOVE_TOOLS`], so when executor
/// separation is enabled it runs in the executor subprocess (Layer 1) or the
/// Sandboxie box (Layer 2) — identical containment to `exec`. The workflow
/// `script` node delegates to this tool to gain sandbox awareness: the spawn is
/// byte-identical to `ScriptNodeExecutor`'s direct spawn (`Command::new(interp).
/// arg(flag).arg(script)`), so enabling the sandbox changes only *where* the
/// script runs, not *how*.
///
/// Unlike `exec` (which flattens output to a string for the LLM), this returns
/// STRUCTURED `{stdout, stderr, exit_code}` as a JSON string, so the workflow
/// `script` node preserves its `{stdout,stderr,exit_code,language}` output
/// contract. The interpreter + flag are passed in by the caller (the workflow
/// node resolves `language → (interpreter, flag)` via its own table), keeping
/// this tool generic and free of workflow-specific mapping.
pub struct RunScriptTool {
    workspace: String,
    restrict: bool,
}

impl RunScriptTool {
    /// Create a new run_script tool. `workspace` is the default cwd; `restrict`
    /// confines cwd to the workspace (mirrors [`ExecTool::new`]).
    pub fn new(workspace: &str, restrict: bool) -> Self {
        Self {
            workspace: workspace.to_string(),
            restrict,
        }
    }
}

#[async_trait]
impl Tool for RunScriptTool {
    fn description(&self) -> String {
        "Run a script with a given interpreter (e.g. `bash -c <script>`, \
         `python3 -c <script>`, `cmd /C <script>`). Returns structured \
         {stdout, stderr, exit_code}."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "interpreter": {
                    "type": "string",
                    "description": "Interpreter executable (bash, sh, python3, node, cmd, powershell, ...)"
                },
                "flag": {
                    "type": "string",
                    "description": "Interpreter flag passing the script (e.g. -c, /C, -Command). Empty string for none."
                },
                "script": {
                    "type": "string",
                    "description": "The script source to execute"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in seconds (default 60)"
                },
                "cwd": {
                    "type": "string",
                    "description": "Working directory (default: workspace)"
                }
            },
            "required": ["interpreter", "script"]
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let val: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid arguments: {}", e))?;

        let interpreter = val
            .get("interpreter")
            .and_then(|v| v.as_str())
            .ok_or("Missing 'interpreter' argument")?;
        let flag = val.get("flag").and_then(|v| v.as_str()).unwrap_or("");
        let script = val
            .get("script")
            .and_then(|v| v.as_str())
            .ok_or("Missing 'script' argument")?;
        let timeout_secs = val.get("timeout").and_then(|v| v.as_u64()).unwrap_or(60);
        let cwd = val
            .get("cwd")
            .and_then(|v| v.as_str())
            .unwrap_or(&self.workspace);

        // Workspace restriction (mirrors ExecTool).
        if self.restrict {
            // 与 ExecTool（shell.rs）对齐：相对 cwd join workspace；双侧统一
            // 归一化（nemesis-path 单一真相源）——组件级比较防前缀误判，
            // workspace 侧归一化防 8.3 短名/大小写失配（2026-09-01）。
            let target = if Path::new(cwd).is_absolute() {
                PathBuf::from(cwd)
            } else {
                Path::new(&self.workspace).join(cwd)
            };
            let resolved = canonicalize_for_compare(&target);
            let ws = canonicalize_for_compare(Path::new(&self.workspace));
            if !resolved.starts_with(&ws) {
                return Err(format!(
                    "Access denied: path '{}' is outside workspace",
                    cwd
                ));
            }
        }

        // Spawn `interpreter [flag] script` — identical to ScriptNodeExecutor's
        // direct spawn. stdin=null + kill_on_drop so interactive prompts / hung
        // children don't survive the timeout.
        // Windows：裸 "bash"/"sh" 经 PATH 恒命中 System32 的 WSL launcher
        // （feature 启用但无发行版 = exit 1 空壳，2026-09-02 CI 实证）→
        // 解析到 Git/MSYS2 的真 bash。单一真相源 nemesis-tools::shell。
        let interpreter = nemesis_tools::shell::resolve_posix_shell_path(interpreter);
        let mut cmd = tokio::process::Command::new(&interpreter);
        if !flag.is_empty() {
            cmd.arg(flag);
        }
        cmd.arg(script)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);

        let output = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            cmd.current_dir(cwd).output(),
        )
        .await;

        match output {
            Ok(Ok(out)) => {
                let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                let exit_code = out.status.code().unwrap_or(-1);
                // Always Ok: encode success/failure in exit_code so the caller
                // (workflow script node) keeps its structured contract. Only
                // spawn/timeout failures are Err.
                Ok(serde_json::json!({
                    "stdout": stdout,
                    "stderr": stderr,
                    "exit_code": exit_code,
                })
                .to_string())
            }
            Ok(Err(e)) => Err(format!("Failed to execute script: {}", e)),
            Err(_) => Err(format!(
                "Script timed out after {} seconds (interpreter: {}). It may be \
                 waiting for input (e.g. an interactive prompt).",
                timeout_secs, interpreter
            )),
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
        let val: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid arguments: {}", e))?;

        let command = val
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or("Missing 'command' argument")?;

        let cwd = val
            .get("working_dir")
            .and_then(|v| v.as_str())
            .unwrap_or(&self.workspace);

        let wait_secs = val
            .get("wait_seconds")
            .and_then(|v| v.as_u64())
            .unwrap_or(3)
            .clamp(1, 10);

        // Workspace restriction check
        if self.restrict {
            // 与 ExecTool（shell.rs）对齐：相对 cwd join workspace；双侧统一
            // 归一化（nemesis-path 单一真相源）——组件级比较防前缀误判，
            // workspace 侧归一化防 8.3 短名/大小写失配（2026-09-01）。
            let target = if Path::new(cwd).is_absolute() {
                PathBuf::from(cwd)
            } else {
                Path::new(&self.workspace).join(cwd)
            };
            let resolved = canonicalize_for_compare(&target);
            let ws = canonicalize_for_compare(Path::new(&self.workspace));
            if !resolved.starts_with(&ws) {
                return Err(format!(
                    "Access denied: path '{}' is outside workspace",
                    cwd
                ));
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
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .map_err(|e| format!("Failed to start command: {}", e))?
        };

        // Wait briefly to confirm startup
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(wait_secs), child.wait()).await;

        match result {
            Ok(Ok(status)) => {
                if status.success() || status.code().is_none() {
                    // Still running (no exit code) or exited cleanly
                    Ok(format!(
                        "Command '{}' started and confirmed running",
                        command
                    ))
                } else {
                    Err(format!(
                        "Command '{}' exited prematurely with status: {}",
                        command, status
                    ))
                }
            }
            Ok(Err(e)) => Err(format!("Failed to wait for command: {}", e)),
            Err(_) => {
                // Timeout — process still running, which is the expected async case
                Ok(format!(
                    "Command '{}' started successfully (still running)",
                    command
                ))
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
        let val: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid arguments: {}", e))?;

        let confirmed = val
            .get("confirmed")
            .and_then(|v| v.as_bool())
            .ok_or("Missing or invalid 'confirmed' parameter (must be a boolean)")?;

        if !confirmed {
            return Err(
                "Must confirm initialization is complete before deleting bootstrap file."
                    .to_string(),
            );
        }

        let bootstrap_path = Path::new(&self.workspace).join("BOOTSTRAP.md");

        if !bootstrap_path.exists() {
            return Ok(
                "BOOTSTRAP.md has already been removed. Initialization is complete.".to_string(),
            );
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
        serde_json::json!({"type":"object","properties":{"action":{"type":"string","description":"One of: create, delete, list"},"at_seconds":{"type":"integer","description":"Seconds from now for one-time execution"},"every_seconds":{"type":"integer","description":"Interval in seconds for recurring execution"},"cron_expr":{"type":"string","description":"Cron expression for complex schedules"},"command":{"type":"string","description":"Command or message to execute"},"message":{"type":"string","description":"Reminder message"},"continue_session":{"type":"boolean","description":"true = when the job fires, continue THIS conversation's session (context/persona preserved, reply lands in this chat history). false (default) = fresh session at fire time. Only meaningful with deliver=false."},"max_rounds":{"type":"integer","enum":[5,10,20],"default":10,"description":"Per-fire tool-round budget when continue_session=true: the fired turn stops gracefully after this many tool rounds (work is saved; the job is NOT deleted — next fire re-budgets). Fixed tiers only. Ignored when continue_session=false."}}})
    }

    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String> {
        let val: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid arguments: {}", e))?;

        let action = val.get("action").and_then(|v| v.as_str()).unwrap_or("");

        let svc = self
            .service
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;

        match action {
            "list" => {
                let jobs = svc.list_jobs(true);
                let result: Vec<serde_json::Value> = jobs
                    .iter()
                    .map(|j| {
                        serde_json::json!({
                            "id": j.id,
                            "name": j.name,
                            "schedule": j.schedule,
                            "enabled": j.enabled,
                        })
                    })
                    .collect();
                Ok(serde_json::to_string_pretty(&result).unwrap_or_else(|_| "[]".to_string()))
            }
            "create" => {
                let name = val
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unnamed");
                let schedule_str = val.get("schedule").and_then(|v| v.as_str()).unwrap_or("");
                let content = val.get("content").and_then(|v| v.as_str()).unwrap_or("");
                let deliver = val.get("deliver").and_then(|v| v.as_bool()).unwrap_or(true);

                if schedule_str.is_empty() {
                    return Err("Missing 'schedule' argument".to_string());
                }

                // Parse schedule: support "every:Ns", "at:TIMESTAMP", "cron:EXPR"
                let schedule = if schedule_str.starts_with("every:") {
                    let secs_str = schedule_str
                        .trim_start_matches("every:")
                        .trim_end_matches('s');
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
                    self.channel
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .clone()
                } else {
                    context.channel.clone()
                };
                let chat_id = if context.chat_id.is_empty() {
                    self.chat_id
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .clone()
                } else {
                    context.chat_id.clone()
                };

                // H1 (U12): continue_session=true routes the fired job back
                // into THIS conversation's session (context/persona kept,
                // reply persisted to this chat's history) via add_job_ext's
                // session_key — the gateway's on_job handler consumes it.
                // The session key comes from the CALLING context (the
                // conversation creating the job), never from model input.
                let continue_session = val
                    .get("continue_session")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                // T3 (U12): per-fire tool-round budget, only meaningful with
                // continue_session=true. Fixed enum tiers (5/10/20) — the
                // args_validator rejects off-enum values before dispatch, so a
                // model cannot free-form this into a 1000-round budget. Default
                // 10 per the goal's tier table. None when not continuing (the
                // fired turn then runs under the global max_turns).
                let max_rounds: Option<u32> = if continue_session {
                    Some(val.get("max_rounds").and_then(|v| v.as_u64()).unwrap_or(10) as u32)
                } else {
                    None
                };

                let job = if continue_session {
                    svc.add_job_ext(
                        name,
                        schedule,
                        content,
                        deliver,
                        if channel.is_empty() {
                            None
                        } else {
                            Some(&channel)
                        },
                        if chat_id.is_empty() {
                            None
                        } else {
                            Some(&chat_id)
                        },
                        Some(&context.session_key),
                        max_rounds,
                        true,
                    )
                    .map_err(|e| e.to_string())?
                } else {
                    svc.add_job(
                        name,
                        schedule,
                        content,
                        deliver,
                        if channel.is_empty() {
                            None
                        } else {
                            Some(&channel)
                        },
                        if chat_id.is_empty() {
                            None
                        } else {
                            Some(&chat_id)
                        },
                    )
                    .map_err(|e| e.to_string())?
                };
                let mode_note = if continue_session {
                    format!(
                        " [continues this session; per-fire budget {} rounds]",
                        max_rounds.unwrap_or(10)
                    )
                } else {
                    String::new()
                };
                Ok(format!(
                    "Created cron job: {} (ID: {}){}",
                    job.name, job.id, mode_note
                ))
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
            _ => Err(format!(
                "Unknown cron action: '{}'. Use: list, create, delete",
                action
            )),
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
// Todo list tool (H1, devtool-upgrade 阶段 2)
// ===========================================================================

/// `todowrite`：全量提交式 todo 清单工具。
///
/// 语义：每次调用**替换整个清单**（模型无需 diff 思维），落盘
/// `{workspace}/sessions/todo_{safe_session_key}.json`（原子写：tmp + rename），
/// 成功后广播 [`nemesis_types::agent::AgentEvent::TodoUpdated`]（web pump 转
/// WS push + SSE，前端实时渲染）。**不回灌 prompt**——靠 H3 的 system
/// prompt few-shot 示范驱动模型维持清单。
pub struct TodoWriteTool {
    /// 存储根 workspace（`sessions/` 目录挂其下）。
    workspace: std::path::PathBuf,
    /// TodoUpdated 广播通道（None = 只落盘，不广播）。
    event_tx: Option<tokio::sync::broadcast::Sender<nemesis_types::agent::AgentEvent>>,
}

impl TodoWriteTool {
    pub fn new(
        workspace: std::path::PathBuf,
        event_tx: Option<tokio::sync::broadcast::Sender<nemesis_types::agent::AgentEvent>>,
    ) -> Self {
        Self {
            workspace,
            event_tx,
        }
    }
}

#[async_trait]
impl Tool for TodoWriteTool {
    fn description(&self) -> String {
        "Write the todo list for the current task. FULL REPLACEMENT: each call replaces the entire list. Use for multi-step tasks: create the list up front, keep exactly one item in_progress while working, mark items completed as you go."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "description": "The complete todo list (replaces the previous list entirely)",
                    "items": {
                        "type": "object",
                        "properties": {
                            "content": {"type": "string", "description": "The task description"},
                            "status": {"type": "string", "enum": ["pending", "in_progress", "completed"], "description": "Item status"}
                        },
                        "required": ["content", "status"]
                    }
                }
            },
            "required": ["todos"]
        })
    }

    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String> {
        let parsed: TodosArgs =
            serde_json::from_str(args).map_err(|e| format!("invalid todowrite args: {e}"))?;
        let todos = parsed.todos;

        // 存储路径：sessions/todo_{safe_session_key}.json。安全化规则与
        // nemesis-session::sanitize_filename 同款（':' → '_'）——nemesis-agent
        // 不依赖 nemesis-session（各自持有 SessionStore），跨 crate 引一条
        // 一行规则不值新增依赖边；两处注释互指，漂移时一起改。
        let safe_key = context.session_key.replace(':', "_");
        let dir = nemesis_path::resolve_sessions_dir_in_workspace(&self.workspace);
        let path = dir.join(format!("todo_{safe_key}.json"));

        // 原子写：tmp + rename（rename 两平台都替换已存在目标）。
        let body = serde_json::to_string_pretty(&todos)
            .map_err(|e| format!("serialize todos failed: {e}"))?;
        std::fs::create_dir_all(&dir).map_err(|e| format!("create sessions dir failed: {e}"))?;
        let tmp = dir.join(format!("todo_{safe_key}.json.tmp"));
        std::fs::write(&tmp, body).map_err(|e| format!("write todo tmp failed: {e}"))?;
        std::fs::rename(&tmp, &path).map_err(|e| format!("rename todo file failed: {e}"))?;

        // 回执统计（事件广播前先取 counts，todos 随后 move 进事件）。
        let total = todos.len();
        let done = todos
            .iter()
            .filter(|t| t.status == nemesis_types::agent::TodoStatus::Completed)
            .count();
        let in_progress = todos
            .iter()
            .filter(|t| t.status == nemesis_types::agent::TodoStatus::InProgress)
            .count();

        // 广播 TodoUpdated（无订阅者 = Err，静默——观察者通道空转是常态）。
        if let Some(tx) = &self.event_tx {
            let _ = tx.send(nemesis_types::agent::AgentEvent::TodoUpdated {
                session_key: context.session_key.clone(),
                chat_id: context.chat_id.clone(),
                todos,
            });
        }

        Ok(format!(
            "Todo list updated: {total} items ({done} completed, {in_progress} in_progress)"
        ))
    }
}

/// todowrite 的 args 形态（serde 解析 + enum 校验一步完成：status 非法值
/// 直接 Err，由 args_validator 回灌层决定重试预算）。
#[derive(serde::Deserialize)]
struct TodosArgs {
    todos: Vec<nemesis_types::agent::TodoItem>,
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

    fn is_read_only(&self) -> bool {
        true
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

        let url = format!("https://html.duckduckgo.com/html/?q={}", urlencoding(query));

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
            .build()
            .map_err(|e| format!("failed to create HTTP client: {}", e))?;

        let resp = client
            .get(&url)
            .send()
            .await
            .map_err(|e| expand_error("request failed", &e))?;

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

        let snippet_re = regex::Regex::new(r#"<a class="result__snippet[^"]*".*?>([\s\S]*?)</a>"#)
            .map_err(|e| format!("regex error: {}", e))?;

        let snippet_captures: Vec<_> = snippet_re.captures_iter(&html).take(count + 5).collect();
        let tag_re = regex::Regex::new(r"<[^>]+>").map_err(|e| format!("regex error: {}", e))?;
        let strip_tags =
            |content: &str| -> String { tag_re.replace_all(content, "").trim().to_string() };

        let mut lines = vec![format!("Results for: {} (via DuckDuckGo)", query)];
        let max_items = link_captures.len().min(count);

        for i in 0..max_items {
            let caps = &link_captures[i];
            let url_str = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let title = strip_tags(caps.get(2).map(|m| m.as_str()).unwrap_or(""));

            let mut url_clean = url_str.to_string();
            if url_clean.contains("uddg=")
                && let Some(decoded) = url_decode_query_param(&url_clean, "uddg")
            {
                url_clean = decoded;
            }

            lines.push(format!("{}. {}\n   {}", i + 1, title, url_clean));

            if i < snippet_captures.len() {
                let snippet =
                    strip_tags(snippet_captures[i].get(1).map(|m| m.as_str()).unwrap_or(""));
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
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(args)
        && let Some(query) = val.get("query").and_then(|v| v.as_str())
    {
        return Ok(query.to_string());
    }
    // Fallback: treat the entire argument as a query.
    Ok(args.trim().to_string())
}

/// Extract the "name" argument from tool arguments.
fn extract_name_arg(args: &str) -> Result<String, String> {
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(args)
        && let Some(name) = val.get("name").and_then(|v| v.as_str())
    {
        return Ok(name.to_string());
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
    for part in url
        .split('&')
        .chain(url.split('?').skip(1).flat_map(|s| s.split('&')))
    {
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

/// web_fetch 手动重定向循环的跳数上限（J2a 2026-09-04）。严于 reqwest
/// 默认的 10 跳：每跳都要过 SSRF 闸 + 重新建连，5 跳足够覆盖正常业务
/// 跳转，更长的链几乎总是开放重定向滥用或循环。
const WEB_FETCH_MAX_REDIRECTS: usize = 5;

/// web_fetch 请求 UA（伪装浏览器以最大化站点兼容，沿用旧实现原值）。
const WEB_FETCH_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

/// Web fetch tool that downloads content from URLs.
pub struct WebFetchTool {
    /// Maximum response body size in bytes.
    pub max_size: usize,
    /// J2b（2026-09-06）：workspace 根（spill 存档用）。空 = 无 spill 落点
    /// （超限时退回旧「截断+注记」行为，诚实不静默）。生产注册处必填；
    /// 旧测试构造路径不带（断言截断注记的用例不受扰）。
    workspace: String,
    /// J2a（2026-09-04）：SSRF 闸宿主（SecurityPlugin）。重定向改为手动
    /// 循环后**每一跳**都重新过闸（旧实现自动跟随 ≤10 跳、闸只查首跳，
    /// `302 → 内网` 直接绕闸）。None（feature 裁剪 / 插件未注入）时跳数
    /// 上限仍生效，仅不做内网校验。
    #[cfg(feature = "security")]
    pub(crate) ssrf: Option<Arc<nemesis_security::pipeline::SecurityPlugin>>,
    #[cfg(not(feature = "security"))]
    #[allow(dead_code)]
    pub(crate) ssrf: Option<()>,
}

impl WebFetchTool {
    /// Create a new web fetch tool with the given maximum response size.
    pub fn new(max_size: usize) -> Self {
        Self {
            max_size,
            workspace: String::new(),
            ssrf: None,
        }
    }

    /// J2b：设置 workspace 根（超限全文存档到 `{workspace}/logs/spill/`）。
    pub fn with_workspace(mut self, workspace: &str) -> Self {
        self.workspace = workspace.to_string();
        self
    }

    /// J2a：为本跳 URL 构建请求 client（SSRF 闸逐跳复查）。
    ///
    /// 三态（与图片 URL 预取 T9/S2 同一真相源 `ssrf::Guard`）：
    /// - `Err` = 闸拦截（回环/内网/链路本地/元数据/用户 CIDR/非 http(s)）
    ///   → 整个请求报错；
    /// - `Ok(ips)` 非空 = 钉死 DNS 的专用 client（防 rebinding TOCTOU）；
    /// - `Ok(ips)` 空 = 闸关闭 / host 白名单直通 → 用 plain client。
    ///
    /// 无闸（feature 裁剪 / 插件未注入）直通 plain client——跳数上限仍生效。
    fn hop_client(&self, url: &str, plain: &reqwest::Client) -> Result<reqwest::Client, String> {
        #[cfg(feature = "security")]
        if let Some(guard) = self.ssrf.as_deref().and_then(|p| p.ssrf_guard()) {
            return match guard.resolve_and_validate_collect(url) {
                Ok(ips) if !ips.is_empty() => {
                    Ok(crate::image_attach::build_pinned_no_redirect_client(
                        url,
                        &ips,
                        WEB_FETCH_UA,
                        std::time::Duration::from_secs(10),
                        std::time::Duration::from_secs(60),
                    )
                    .unwrap_or_else(|| plain.clone()))
                }
                Ok(_) => Ok(plain.clone()),
                Err(e) => Err(format!("SSRF blocked: {}", e)),
            };
        }
        let _ = url;
        Ok(plain.clone())
    }

    /// 响应体下载 + HTML 提取（原 execute 内联管线；J2a 拆出以便重定向
    /// 循环收敛到最终 URL 后调用。`url` 报告的是**最终** URL，消息文本
    /// 格式与旧实现字节一致）。
    async fn read_body(
        &self,
        resp: reqwest::Response,
        url: &str,
        session_key: &str,
    ) -> Result<String, String> {
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

        // Download guard against truly huge responses only (OOM / bandwidth
        // abuse). The user-facing bound is on the *extracted* text below, so
        // large real pages (e.g. baidu.com ~680KB HTML) succeed instead of
        // failing at the download stage. Previously this checked `max_size`
        // (50KB) on the raw body, which rejected almost every real webpage.
        const DOWNLOAD_LIMIT: usize = 10 * 1024 * 1024; // 10 MB
        if body.len() > DOWNLOAD_LIMIT {
            return Err(format!(
                "Response too large: {} bytes (download limit: {} bytes)",
                body.len(),
                DOWNLOAD_LIMIT
            ));
        }

        let html = looks_like_html(&content_type, &body);
        let text = if html {
            let extracted = extract_html_text(&String::from_utf8_lossy(&body));
            if extracted.is_empty() {
                return Ok(format!(
                    "Content from {} ({} bytes, HTML with no extractable text)",
                    url,
                    body.len()
                ));
            }
            extracted
        } else {
            String::from_utf8_lossy(&body).trim().to_string()
        };

        self.bound_reply(text, url, &content_type, body.len(), html, session_key)
    }

    /// J2b：超 `max_size` 的提取文本不再「截断+注记」，而是全文存档到
    /// workspace spill 目录 + 回灌 2KB preview + locator（B1 同款语义——
    /// read_file/grep 可检索全量）。存档失败（无 workspace/磁盘错误）退回
    /// 旧「截断+注记」行为，诚实注明。低于限值时回灌格式与旧实现字节一致。
    fn bound_reply(
        &self,
        text: String,
        url: &str,
        content_type: &str,
        body_len: usize,
        was_html: bool,
        session_key: &str,
    ) -> Result<String, String> {
        let label = if was_html {
            "extracted text"
        } else {
            content_type
        };
        if text.len() <= self.max_size {
            return Ok(format!(
                "Content from {} ({} bytes, {}):\n{}",
                url, body_len, label, text
            ));
        }

        let (preview, total) = preview_chars(&text, WEB_FETCH_PREVIEW_CHARS);
        if self.workspace.is_empty() {
            let (cut, truncated) = truncate_str(&text, self.max_size);
            let note = if truncated {
                format!(" truncated to {} bytes", self.max_size)
            } else {
                String::new()
            };
            return Ok(format!(
                "Content from {} ({} bytes, {}{}):\n{}",
                url, body_len, label, note, cut
            ));
        }

        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis().to_string())
            .unwrap_or_default();
        let host = reqwest::Url::parse(url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .unwrap_or_else(|| "url".to_string());
        let spill_root = nemesis_path::resolve_spill_dir_in_workspace(Path::new(&self.workspace));
        match crate::spill::save_tool_output(
            &text,
            &spill_root,
            session_key,
            &stamp,
            &format!("web_fetch_{host}"),
        ) {
            Ok(path) => Ok(format!(
                "Content from {} ({} bytes, {} {} chars, showing first {}):\n{}\n[内容过大已完整保存到：{}。可用 read_file 工具按 offset/limit 分段读取该文件，或用 grep 工具在其中检索关键词。]",
                url,
                body_len,
                label,
                total,
                preview.chars().count(),
                preview,
                path.display()
            )),
            Err(e) => {
                tracing::warn!("[WebFetchTool] spill save failed: {e}");
                let (cut, truncated) = truncate_str(&text, self.max_size);
                let note = if truncated {
                    format!(" truncated to {} bytes", self.max_size)
                } else {
                    String::new()
                };
                Ok(format!(
                    "Content from {} ({} bytes, {}{}):\n{}",
                    url, body_len, label, note, cut
                ))
            }
        }
    }
}

/// J2b：web_fetch 超限回灌的 preview 字符预算（与 spill 层
/// `SPILL_PREVIEW_CHARS` 同量级）。
const WEB_FETCH_PREVIEW_CHARS: usize = 2000;

/// J2b：取前 `max_chars` 个字符（char-boundary 安全），返回
/// `(preview, 总字符数)`。
fn preview_chars(s: &str, max_chars: usize) -> (String, usize) {
    let total = s.chars().count();
    (s.chars().take(max_chars).collect(), total)
}

/// J2b：HTML 判定——Content-Type 含 `text/html`，或 Content-Type 未标明时
/// 按内容兜底（trim 后小写前缀 `<!doctype html` / `<html`；只看前 1KB，
/// 避免为判定扫描整个 body）。
fn looks_like_html(content_type: &str, body: &[u8]) -> bool {
    if content_type.to_ascii_lowercase().contains("text/html") {
        return true;
    }
    let head = String::from_utf8_lossy(&body[..body.len().min(1024)]);
    let head = head.trim_start().to_ascii_lowercase();
    head.starts_with("<!doctype html") || head.starts_with("<html")
}

/// J2b：HTML → 可读文本（html2text 渲染：script/style 剥除、链接/列表/
/// 标题结构保留为 markdown 形态；替代旧 regex 剥标签管线——旧管线把
/// 全页压成单行、链接 URL 丢失、表格/列表结构全毁）。80 列渲染宽度。
/// 渲染错误（html5ever 极端容错，实践中几乎不可达）诚实降级为空 →
/// 调用方回「HTML with no extractable text」。
fn extract_html_text(html: &str) -> String {
    match html2text::from_read(html.as_bytes(), 80) {
        Ok(rendered) => rendered
            .lines()
            .map(str::trim_end)
            .collect::<Vec<&str>>()
            .join("\n")
            .trim()
            .to_string(),
        Err(e) => {
            tracing::warn!("[WebFetchTool] html2text render failed: {e}");
            String::new()
        }
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

    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String> {
        let start_url = extract_url(args)?;

        // J2a（2026-09-04）：手动重定向循环替代 reqwest 默认自动跟随——
        // 旧实现用默认 client（≤10 跳自动跟随），SSRF 闸只查首跳 URL，
        // `302 → 内网` 直接绕闸。现在每一跳都重新过闸 + 可钉死 DNS；
        // 跳数上限 5；3xx 无 Location / 相对 Location 都诚实处理。
        let plain_client = crate::image_attach::build_no_redirect_client(
            WEB_FETCH_UA,
            std::time::Duration::from_secs(10),
            std::time::Duration::from_secs(60),
        );

        let mut current = start_url;
        let mut hops: usize = 0;
        loop {
            let client = self.hop_client(&current, &plain_client)?;
            let resp = client
                .get(&current)
                .send()
                .await
                .map_err(|e| expand_error("request failed", &e))?;

            let status = resp.status();
            if status.is_redirection() {
                hops += 1;
                if hops > WEB_FETCH_MAX_REDIRECTS {
                    return Err(format!(
                        "too many redirects (limit {}), stopped at {}",
                        WEB_FETCH_MAX_REDIRECTS, current
                    ));
                }
                let location = resp
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| {
                        format!(
                            "HTTP {} redirect without Location header at {}",
                            status, current
                        )
                    })?
                    .to_string();
                let base = reqwest::Url::parse(&current)
                    .map_err(|e| format!("invalid URL '{}': {}", current, e))?;
                current = base
                    .join(&location)
                    .map_err(|e| format!("invalid redirect target '{}': {}", location, e))?
                    .to_string();
                continue;
            }

            if !status.is_success() {
                return Err(format!("HTTP {} for {}", status, current));
            }

            return self.read_body(resp, &current, &context.session_key).await;
        }
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

/// Truncate a string to at most `max_bytes`, cutting on a UTF-8 char boundary
/// (never mid-character). Returns `(truncated_string, was_truncated)`.
fn truncate_str(s: &str, max_bytes: usize) -> (String, bool) {
    if s.len() <= max_bytes {
        return (s.to_string(), false);
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    (s[..end].to_string(), true)
}

/// Format an error with its full source chain. reqwest collapses the real
/// cause (TLS / DNS / connect / certificate) into an opaque "error sending
/// request"; this surfaces the underlying reason for diagnosis.
fn expand_error(prefix: &str, e: &dyn std::error::Error) -> String {
    let mut msg = format!("{}: {}", prefix, e);
    let mut cur = e.source();
    while let Some(s) = cur {
        msg.push_str(&format!(" | {}", s));
        cur = s.source();
    }
    msg
}

/// Extract URL from tool arguments.
fn extract_url(args: &str) -> Result<String, String> {
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(args)
        && let Some(url) = val.get("url").and_then(|v| v.as_str())
    {
        return Ok(url.to_string());
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
    handlers: &mut std::collections::HashMap<
        String,
        Box<dyn Fn(serde_json::Value) -> Result<serde_json::Value, String> + Send + Sync>,
    >,
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
    rpc_call_fn: Option<
        Arc<
            dyn Fn(
                    &str,
                    &str,
                    serde_json::Value,
                ) -> std::pin::Pin<
                    Box<dyn std::future::Future<Output = Result<serde_json::Value, String>> + Send>,
                > + Send
                + Sync,
        >,
    >,
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
        self.enabled
            .store(enabled, std::sync::atomic::Ordering::Relaxed);
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
            dyn Fn(
                    &str,
                    &str,
                    serde_json::Value,
                ) -> std::pin::Pin<
                    Box<dyn std::future::Future<Output = Result<serde_json::Value, String>> + Send>,
                > + Send
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
        "Send a message to ANOTHER bot in the cluster (never yourself). \
         Returns the remote node's final response text. If the remote node executes an LLM task, \
         the call may take tens of seconds to several minutes. Before calling, use the `message` \
         tool to tell the user \"已联系对方，稍等\" so they know to wait. \
         The remote node only sees your final message text — it does NOT see your prior tool \
         calls, file writes, or command outputs. If you need the remote node to act on something \
         you've produced locally (e.g. code you wrote), include the relevant content directly in \
         the `message` field rather than just referencing it."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        // Dynamically inject online peer list with capabilities into the target description.
        // The peers_fn list already excludes the local node (filtered at the
        // cluster level via get_online_peers_excluding_self), so the LLM should
        // never see itself as a candidate. We also explicitly annotate the
        // local node_id in the description as belt-and-suspenders.
        let self_id_note = if self.config.local_node_id.is_empty() {
            String::new()
        } else {
            format!(
                "\nNote: your own node_id is '{}'. Do NOT select it — this tool is for calling OTHER nodes only.\n",
                self.config.local_node_id
            )
        };

        let target_desc = if let Some(ref peers_fn) = self.peers_fn {
            let peers = peers_fn();
            if peers.is_empty() {
                format!(
                    "Target bot ID (no other peers currently online).{}",
                    self_id_note
                )
            } else {
                let mut desc = format!(
                    "Target bot ID. Available online peers (excluding yourself):\n{}",
                    self_id_note
                );
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
            format!("Target bot ID{}", self_id_note)
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

        let val: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid JSON arguments: {}", e))?;

        let target_node = val
            .get("target_node")
            .or_else(|| val.get("target"))
            .or_else(|| val.get("peer_id"))
            .and_then(|v| v.as_str())
            .ok_or("Missing 'target_node' field")?;

        // Guard: reject self-invocation. The LLM should never target the local
        // node via cluster_rpc — doing so creates a nested child task that loops
        // back to this same node, which serves no purpose and corrupts the
        // continuation history. The peers list passed to the LLM already
        // excludes self, but this is the hard backstop in case of cache lag,
        // stale tool definitions, or unexpected LLM behavior.
        if !self.config.local_node_id.is_empty() && target_node == self.config.local_node_id {
            return Err(format!(
                "不能通过 cluster_rpc 调用本节点（{}）。这个工具用于和其他节点通信；\
                 如果需要在本地执行操作，请直接使用本地工具（exec/filesystem/etc），\
                 不要重试 cluster_rpc。",
                target_node
            ));
        }

        // Extract message content: check "message" first, then "data.content" (testai-3.0 format)
        let message = val
            .get("message")
            .and_then(|v| v.as_str())
            .or_else(|| {
                val.get("data")
                    .and_then(|d| d.get("content"))
                    .and_then(|v| v.as_str())
            })
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

        // 方案 D：检测并改写 chat_id 加 origin 前缀，防止多跳链路撞车。
        //
        // 传播规则：
        //   - chat_id 已经以 "cluster:" 开头 → 已经被上游节点标记过，原样传播
        //   - chat_id 为空 → 不改写（避免出现 "cluster:node-X:" 这种尾部空值）
        //   - 其他 → 改写为 "cluster:{local_node_id}:{chat_id}"，把本节点 ID 嵌进去
        //
        // 下游节点 peer_chat_handler 用 `cluster_rpc:{source_node_id}/{chat_id}` 组 session_key，
        // 多跳链路（A→B→C→D）中 C 端 session_key 会含原始 A 的 node_id，跟 Q→B→C→D 链的
        // C 端 session_key 自然区分开。
        let propagated_chat_id = if chat_id.starts_with("cluster:") || chat_id.is_empty() {
            chat_id
        } else {
            format!("cluster:{}:{}", self.config.local_node_id, chat_id)
        };

        let payload = serde_json::json!({
            "content": message,
            "channel": channel,
            "chat_id": propagated_chat_id,
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

            // Plan C (template-based UX): resolve the peer's display name
            // (e.g. "Alex") here and pass it through the `__ASYNC__` marker
            // so loop.rs can render a human-friendly "waiting" message
            // without a cluster registry lookup (nemesis-agent deliberately
            // does not depend on nemesis-cluster).
            //
            // Falls back to the bare node ID if the peer is not in the
            // online list (just went offline, or peers_fn unset in tests).
            // Colons in the name are stripped because the marker is
            // `:`-delimited and splitn would otherwise mis-parse.
            let target_name = self
                .peers_fn
                .as_ref()
                .and_then(|f| f().into_iter().find(|(id, _, _)| id == target_node))
                .map(|(_, name, _)| name)
                .filter(|n| !n.is_empty())
                .map(|n| n.replace(':', ""))
                .unwrap_or_else(|| target_node.to_string());

            return Ok(format!(
                "__ASYNC__:{}:{}:{}",
                task_id, target_node, target_name
            ));
        }

        // Synchronous response — extract content field
        let content = result.get("content").and_then(|v| v.as_str()).unwrap_or("");

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
    /// G2 (devtool-upgrade 阶段 3)：子代理最大嵌套深度。父深度 N（0 = 顶层
    /// agent）发起 spawn 产生深度 N+1 的子代理，仅当 N+1 ≤ max_depth 放行；
    /// 超限回灌 `Sub-agent depth limit (N) reached` 供模型自纠。生产接线 =
    /// config `agents.subagent.max_depth`（默认 1 = 子代理不能再 spawn）。
    pub max_depth: usize,
}

/// G1：readonly 子代理工具白名单（spawn `tools` 档位的受限集，与 F2 explore
/// 档共用同一清单）。语义 = 与注册表求交（`effective_tool_defs` retain）——
/// 未注册/未启用的项（如 lsp_tool 关闭时）自然缺席，不报错。
pub const DETACHED_READONLY_TOOLS: &[&str] = &[
    "read_file",
    "list_dir",
    "grep",
    "git",
    "web_fetch",
    "lsp",
    "cli_reference",
];

/// G1：spawn `tools` 档位 → `DetachedOpts.allowed_tools`。
/// `"readonly"` → 白名单（`DETACHED_READONLY_TOOLS`）；`"full"` → `None`
/// （不设限，仍经父 tier 过滤）。未知档位 = `Err`（调用方诚实回灌模型自纠，
/// 不静默降级）。
pub fn detached_tools_for_profile(
    profile: &str,
) -> Result<Option<&'static [&'static str]>, String> {
    match profile {
        "readonly" => Ok(Some(DETACHED_READONLY_TOOLS)),
        "full" => Ok(None),
        other => Err(format!(
            "Unknown tools profile '{other}'. Valid values: \"readonly\" (default) or \"full\"."
        )),
    }
}

/// Spawn 闭包类型：`(agent_id, task, model, channel, chat_id, tools, depth) -> Future<Result<String, String>>`。
/// G0 (devtool-upgrade 阶段 3)：生产接线 = agent_factory 组装 AgentLoop 后向
/// spawn_slot 注入的闭包（持 `Weak<AgentLoop>` 调 `run_detached`）。
/// G1：第 6 参 `tools` = 工具档位（"readonly"|"full"，SpawnTool 侧已校验，
/// 闭包内经 `detached_tools_for_profile` 映射为白名单）。
/// G2：第 7 参 `depth` = 本 spawn 应产生的子代理深度（父深度 + 1，已过
/// max_depth 检查），闭包经 `DetachedOpts.depth` 写到子 instance。
/// G4：第 8 参 `background` = 后台化（true 时闭包侧 `tokio::spawn` 包住
/// run_detached，立即返回 `__BG_SPAWN__:{task_id}` marker；任务完成经
/// `subagent_continuation:{task_id}` bus 消息回灌续行）。深度限制沿用第
/// 7 参（后台任务不另计深度——派生前已过 max_depth 检查）。
pub type SpawnFn = Arc<
    dyn Fn(
            &str,
            &str,
            &str,
            &str,
            &str,
            &str,
            usize,
            bool,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send>>
        + Send
        + Sync,
>;

/// Spawn tool for creating sub-agents.
///
/// Spawns a new sub-agent to handle a specific task independently.
/// When a spawn function is present in `spawn_slot`, it performs a real
/// spawn; otherwise it returns an error indicating sub-agent support is not
/// configured.
///
/// G0 (devtool-upgrade 阶段 3)：spawn 闭包经 `Arc<OnceLock>` **槽**注入而
/// 非直接 setter——`Tool` trait 对象无法 downcast，注册进 tools map 后拿
/// 不到自身；槽让 agent_factory 在 AgentLoop 组装完成后（闭包需要 loop 的
/// Arc）延迟注入，工具即刻生效。`max_concurrent` 由真信号量兜住（超出
/// 排队等待，不再是无装饰的配置数字）。
pub struct SpawnTool {
    config: SpawnConfig,
    /// Allowlist checker: returns true if the parent can spawn the target.
    allowlist_checker: Option<Box<dyn Fn(&str) -> bool + Send + Sync>>,
    /// Stored channel from set_context.
    stored_channel: Arc<std::sync::Mutex<String>>,
    /// Stored chat_id from set_context.
    stored_chat_id: Arc<std::sync::Mutex<String>>,
    /// 并发上限信号量（permits = `config.max_concurrent`；execute 时
    /// acquire，超并发排队等待而非放行）。
    semaphore: Arc<tokio::sync::Semaphore>,
    /// G2：本次调用的父深度（`set_invocation_depth` 注入，0 = 顶层 agent）。
    /// execute 读它执行 `parent + 1 > config.max_depth` 拒绝，并把子深度
    /// （parent + 1）经第 7 参传给 spawn 闭包。共享实例上 set-then-read 的
    /// 竞态只会在并发派发间边际误归属深度（与既有 set_context 竞态同类，
    /// 已接受）。
    invocation_depth: std::sync::atomic::AtomicUsize,
    /// Spawn 闭包槽（agent_factory 组装后 `set`；`None` 时 execute 诚实
    /// 报 not available——旧装配路径行为不回归）。
    spawn_slot: Arc<std::sync::OnceLock<SpawnFn>>,
}

impl SpawnTool {
    /// Create a new spawn tool with the given configuration.
    pub fn new(config: SpawnConfig) -> Self {
        let permits = config.max_concurrent.max(1);
        Self::with_spawn_slot(
            config,
            Arc::new(std::sync::OnceLock::new()),
            Arc::new(tokio::sync::Semaphore::new(permits)),
        )
    }

    /// G0：共享槽构造器——生产接线用。调用方持有同一 slot Arc，在
    /// AgentLoop 组装完成后向其注入闭包。
    pub fn with_spawn_slot(
        config: SpawnConfig,
        spawn_slot: Arc<std::sync::OnceLock<SpawnFn>>,
        semaphore: Arc<tokio::sync::Semaphore>,
    ) -> Self {
        Self {
            config,
            allowlist_checker: None,
            stored_channel: Arc::new(std::sync::Mutex::new(String::new())),
            stored_chat_id: Arc::new(std::sync::Mutex::new(String::new())),
            semaphore,
            invocation_depth: std::sync::atomic::AtomicUsize::new(0),
            spawn_slot,
        }
    }

    /// Set the allowlist checker function.
    pub fn set_allowlist_checker(&mut self, checker: Box<dyn Fn(&str) -> bool + Send + Sync>) {
        self.allowlist_checker = Some(checker);
    }

    /// Set the spawn function for performing actual sub-agent creation.
    ///
    /// The function signature is: `(agent_id, task, model, channel, chat_id) -> Future<Output = Result<String, String>>`
    ///
    /// G0：内部写入共享槽（`OnceLock::set` 首次生效，重复 set 静默忽略——
    /// 与旧「直接替换」语义的差异仅在同一工具实例重复 set 的场景，生产/
    /// 测试均不存在该用法）。
    pub fn set_spawn_fn(&mut self, f: SpawnFn) {
        let _ = self.spawn_slot.set(f);
    }
}

#[async_trait]
impl Tool for SpawnTool {
    fn description(&self) -> String {
        "Spawn a sub-agent to handle a task independently. The sub-agent runs with the same tools and governance as the main agent and returns its final answer.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task": {"type": "string", "description": "Task description for the sub-agent"},
                "context": {"type": "string", "description": "Additional context"},
                "agent_id": {"type": "string", "description": "Optional sub-agent preset name (v1 informational only; defaults to \"default\")"},
                "tools": {
                    "type": "string",
                    "enum": ["readonly", "full"],
                    "description": "Tool profile for the sub-agent. \"readonly\" (default) limits it to read-only tools (read_file/list_dir/grep/git/web_fetch/lsp/cli_reference); \"full\" inherits the parent's tier-filtered tool set. Use readonly for research/exploration tasks, full only when the sub-agent must write."
                },
                "background": {
                    "type": "boolean",
                    "description": "Run the sub-agent in the background (default false). When true, this tool returns immediately with a task marker and the main conversation continues; the sub-agent's result is automatically delivered back to this session when it completes. Use for long-running tasks (builds, large refactors, research) where waiting would stall the conversation."
                }
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

    /// G2：分发层注入的父深度（`handle_tool_call_at_depth` → trait 默认
    /// no-op，这里覆写为存储）。execute 据此执行 max_depth 限制。
    fn set_invocation_depth(&self, depth: usize) {
        self.invocation_depth
            .store(depth, std::sync::atomic::Ordering::Relaxed);
    }

    /// G3：spawn 加入 U5 并行池——并发安全由构造保证：池 4 许可 + G0
    /// 信号量双层限流、深度经 at_depth 注入（上面覆写消费）、每笔分发
    /// 仍过完整瀑布、兄弟子代理实例/会话全隔离。spawn 有副作用（真的
    /// 起子代理）故 `is_read_only` 保持默认 false。
    fn is_parallel_safe(&self) -> bool {
        true
    }

    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String> {
        let val: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid JSON arguments: {}", e))?;

        // G0：agent_id 改 optional（schema/实现一致化——schema 一直只
        // required task）；语义 = 子代理 preset 名，v1 信息性，缺省 default。
        let agent_id = val
            .get("agent_id")
            .and_then(|v| v.as_str())
            .unwrap_or("default");

        let task = val.get("task").and_then(|v| v.as_str()).unwrap_or("");

        // G1：工具档位（缺省 readonly=最小权限）。未知档位在 spawn 前诚实
        // 拒绝（不占信号量、不起子代理），错误文案列合法值供模型自纠。
        let tools_profile = val
            .get("tools")
            .and_then(|v| v.as_str())
            .unwrap_or("readonly");
        detached_tools_for_profile(tools_profile)?;

        // G4：后台化开关（缺省 false=同步等待子代理完成）。true 时闭包
        // 侧转后台，本调用立即返回 `__BG_SPAWN__` marker（loop 侧存续行
        // 快照 + 中间消息收尾回合；完成结果经 subagent_continuation 回灌）。
        let background = val
            .get("background")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Check allowlist.
        if let Some(ref checker) = self.allowlist_checker
            && !checker(agent_id)
        {
            return Err(format!(
                "Not allowed to spawn agent '{}'. Check sub-agent permissions.",
                agent_id
            ));
        }

        // G2：深度限制——父深度 + 1（子代理深度）不得超过 `max_depth`。
        // 超限在占信号量/起子代理**之前**诚实拒绝，文案供模型自纠
        // （告诉它当前嵌套已到顶，应自己完成任务而不是再派生）。
        let parent_depth = self
            .invocation_depth
            .load(std::sync::atomic::Ordering::Relaxed);
        let child_depth = parent_depth + 1;
        if child_depth > self.config.max_depth {
            return Err(format!(
                "Sub-agent depth limit ({}) reached. You are already {} level(s) deep in the sub-agent hierarchy — complete the task yourself instead of spawning another sub-agent.",
                self.config.max_depth, parent_depth
            ));
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

        let spawn_fn = match self.spawn_slot.get() {
            Some(f) => f,
            None => {
                return Err(format!(
                    "Sub-agent spawning is not available (no spawn function configured). Cannot spawn agent '{}' for task.",
                    agent_id
                ));
            }
        };

        // G0：并发上限真生效——acquire 许可，超出 max_concurrent 的调用在
        // 此排队等待（信号量关闭时 acquire 返回 Err，诚实上报）。
        let _permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| "spawn semaphore closed".to_string())?;

        spawn_fn(
            agent_id,
            task,
            &self.config.default_model,
            &channel,
            &chat_id,
            tools_profile,
            child_depth,
            background,
        )
        .await
    }
}

// ===========================================================================
// Memory tools
// ===========================================================================

/// Fetch a memory tool's canonical definition from nemesis-memory.
///
/// Single source of truth for the agent-facing memory tool schemas: the
/// wrappers' `parameters()`/`description()` below delegate to
/// [`memory_tool_definitions`] so they can never drift from what
/// `MemoryToolExecutor` actually consumes. (T7 e2e found exactly that
/// drift: the hand-copied `memory_store` schema demanded `key` +
/// tags-as-string while the executor reads `memory_type`/`content` +
/// tags-as-array — args_validator then rejected executor-valid calls.)
///
/// [`memory_tool_definitions`]: nemesis_memory::memory_tools::memory_tool_definitions
#[cfg(feature = "memory")]
fn memory_tool_def(name: &str) -> nemesis_memory::memory_tools::MemoryTool {
    nemesis_memory::memory_tools::memory_tool_definitions()
        .into_iter()
        .find(|d| d.name == name)
        .unwrap_or_else(|| panic!("memory_tool_definitions must cover `{name}`"))
}

/// Memory search tool for searching conversation memory.
///
/// Delegates to `nemesis_memory::memory_tools::MemoryToolExecutor` for the
/// actual search. If no memory manager is configured, returns an error.
#[cfg(feature = "memory")]
pub struct MemorySearchTool {
    executor: Option<Arc<nemesis_memory::memory_tools::MemoryToolExecutor>>,
}

#[cfg(feature = "memory")]
impl MemorySearchTool {
    /// Create a new memory search tool backed by the given executor.
    pub fn new(executor: Option<Arc<nemesis_memory::memory_tools::MemoryToolExecutor>>) -> Self {
        Self { executor }
    }
}

#[cfg(feature = "memory")]
#[async_trait]
impl Tool for MemorySearchTool {
    fn description(&self) -> String {
        memory_tool_def("memory_search").description
    }

    fn parameters(&self) -> serde_json::Value {
        memory_tool_def("memory_search").parameters
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let executor = match &self.executor {
            Some(e) => e,
            None => return Err("Memory store is not available".to_string()),
        };

        let val: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid JSON arguments: {}", e))?;

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
#[cfg(feature = "memory")]
pub struct MemoryStoreTool {
    executor: Option<Arc<nemesis_memory::memory_tools::MemoryToolExecutor>>,
}

#[cfg(feature = "memory")]
impl MemoryStoreTool {
    /// Create a new memory store tool backed by the given executor.
    pub fn new(executor: Option<Arc<nemesis_memory::memory_tools::MemoryToolExecutor>>) -> Self {
        Self { executor }
    }
}

#[cfg(feature = "memory")]
#[async_trait]
impl Tool for MemoryStoreTool {
    fn description(&self) -> String {
        memory_tool_def("memory_store").description
    }

    fn parameters(&self) -> serde_json::Value {
        memory_tool_def("memory_store").parameters
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let executor = match &self.executor {
            Some(e) => e,
            None => return Err("Memory store is not available".to_string()),
        };

        let val: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid JSON arguments: {}", e))?;

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
#[cfg(feature = "memory")]
pub struct MemoryForgetTool {
    executor: Option<Arc<nemesis_memory::memory_tools::MemoryToolExecutor>>,
}

#[cfg(feature = "memory")]
impl MemoryForgetTool {
    /// Create a new memory forget tool backed by the given executor.
    pub fn new(executor: Option<Arc<nemesis_memory::memory_tools::MemoryToolExecutor>>) -> Self {
        Self { executor }
    }
}

#[cfg(feature = "memory")]
#[async_trait]
impl Tool for MemoryForgetTool {
    fn description(&self) -> String {
        memory_tool_def("memory_forget").description
    }

    fn parameters(&self) -> serde_json::Value {
        memory_tool_def("memory_forget").parameters
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let executor = match &self.executor {
            Some(e) => e,
            None => return Err("Memory store is not available".to_string()),
        };

        let val: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid JSON arguments: {}", e))?;

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
#[cfg(feature = "memory")]
pub struct MemoryListTool {
    executor: Option<Arc<nemesis_memory::memory_tools::MemoryToolExecutor>>,
}

#[cfg(feature = "memory")]
impl MemoryListTool {
    /// Create a new memory list tool backed by the given executor.
    pub fn new(executor: Option<Arc<nemesis_memory::memory_tools::MemoryToolExecutor>>) -> Self {
        Self { executor }
    }
}

#[cfg(feature = "memory")]
#[async_trait]
impl Tool for MemoryListTool {
    fn description(&self) -> String {
        memory_tool_def("memory_list").description
    }

    fn parameters(&self) -> serde_json::Value {
        memory_tool_def("memory_list").parameters
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let executor = match &self.executor {
            Some(e) => e,
            None => return Err("Memory store is not available".to_string()),
        };

        let val: serde_json::Value =
            serde_json::from_str(args).unwrap_or_else(|_| serde_json::json!({}));

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
                    return Ok(
                        "No skills installed. Use find_skills to search for available skills."
                            .to_string(),
                    );
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
                        let content = loader
                            .load_skill(&skill_name)
                            .unwrap_or_else(|| "(no content available)".to_string());
                        Ok(format!(
                            "Skill: **{}**\nSource: {}\nPath: {}\nDescription: {}\n\n{}",
                            info.name, info.source, info.path, info.description, content
                        ))
                    }
                    None => Err(format!(
                        "Skill '{}' not found. Use skills_list to see installed skills.",
                        skill_name
                    )),
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
        let val: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid JSON arguments: {}", e))?;

        let query = val["query"].as_str().unwrap_or("");
        if query.trim().is_empty() {
            return Err("missing or empty 'query' parameter".to_string());
        }

        let limit = val["limit"].as_u64().unwrap_or(5).clamp(1, 50) as usize;

        let results = self
            .registry
            .search(query, limit)
            .await
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
    pub fn new(
        registry: Arc<nemesis_skills::registry::RegistryManager>,
        workspace: String,
    ) -> Self {
        Self {
            registry,
            workspace,
        }
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
        let val: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid JSON arguments: {}", e))?;

        let slug = match val["name"].as_str().or_else(|| val["slug"].as_str()) {
            Some(s) if !s.is_empty() => s,
            _ => {
                return Err("slug parameter is required and must be a non-empty string".to_string());
            }
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

        self.registry
            .install(registry_name, slug, &target_dir)
            .await
            .map_err(|e| format!("failed to install skill '{}': {}", slug, e))?;

        Ok(format!(
            "Skill '{}' installed successfully from registry '{}'",
            slug, registry_name
        ))
    }
}

// ===========================================================================
// Skill manage tool (agent-authored skills / procedural memory)
// ===========================================================================

/// Skill manage tool — the agent's procedural memory.
///
/// Lets the agent author reusable SKILL.md skills under `workspace/skills/<name>/`,
/// so a workflow it just learned (or distilled from reference material via the
/// `learn` skill) becomes a reusable slash command. Mirrors Hermes Agent's
/// `skill_manage`. Actions: `create` (full SKILL.md incl. frontmatter), `patch`/
/// `edit` (replace `old`->`new` text), `write_file`/`remove_file` (supporting
/// files inside the skill dir), `delete` (remove whole skill). All writes are
/// security-checked before landing.
/// Shareable slot holding an optional approval manager, filled in later by the
/// gateway (after the agent loop is built). Lets `skill_manage` request
/// interactive approval for writes when `skills.manage_approval` is enabled.
#[cfg(feature = "security")]
pub type ApprovalManagerSlot =
    Arc<parking_lot::RwLock<Option<Arc<dyn nemesis_security::auditor::ApprovalManager>>>>;
#[cfg(not(feature = "security"))]
/// Placeholder when the security feature is off — `skill_manage` auto-allows
/// writes (no approval middleware). All `Option<ApprovalManagerSlot>` fields
/// stay `None`-compatible via this `()` alias.
pub type ApprovalManagerSlot = ();

#[cfg_attr(not(feature = "security"), allow(dead_code))]
pub struct SkillManageTool {
    workspace: String,
    /// Optional approval manager slot (filled later by the gateway). When
    /// `require_approval` is true and this slot is populated, writes prompt.
    approval_manager: Option<ApprovalManagerSlot>,
    /// Whether writes require interactive approval (config: skills.manage_approval).
    require_approval: bool,
}

impl SkillManageTool {
    /// Create a new skill manage tool writing under `<workspace>/skills/`.
    pub fn new(
        workspace: String,
        approval_manager: Option<ApprovalManagerSlot>,
        require_approval: bool,
    ) -> Self {
        Self {
            workspace,
            approval_manager,
            require_approval,
        }
    }

    fn skill_dir(&self, name: &str) -> PathBuf {
        Path::new(&self.workspace).join("skills").join(name)
    }

    /// Request interactive approval for a write action. No-op when approval is
    /// disabled. When enabled but no manager is running, refuse the write (safe
    /// default). Runs the blocking popup on a `spawn_blocking` thread.
    async fn check_approval(
        &self,
        operation: &str,
        target: &str,
        reason: &str,
    ) -> Result<(), String> {
        #[cfg(not(feature = "security"))]
        {
            // Security feature trimmed — no approval middleware; auto-allow skill writes.
            let _ = (operation, target, reason);
            // 尾表达式形式（非 `return Ok(())`）：not(security) 单臂编译时
            // 本块即函数尾，clippy::needless_return 会判 return 多余
            // （minimal-feature 形态 2026-09-05 远端 clippy 实录）。
            Ok(())
        }
        #[cfg(feature = "security")]
        {
            if !self.require_approval {
                return Ok(());
            }
            let slot = match &self.approval_manager {
                Some(s) => s.clone(),
                None => {
                    return Err(
                        "skill write requires approval but no approval manager is configured"
                            .to_string(),
                    );
                }
            };
            let am = {
                let guard = slot.read();
                match guard.as_ref() {
                    Some(m) if m.is_running() => m.clone(),
                    _ => {
                        return Err(
                            "skill write requires approval but no approval manager is running"
                                .to_string(),
                        );
                    }
                }
            };
            let req_id = format!("skill_manage:{}:{}", operation, target);
            let op = format!("skill_manage.{}", operation);
            let target = target.to_string();
            let reason = reason.to_string();
            match tokio::task::spawn_blocking(move || {
                am.request_approval_sync(&req_id, &op, &target, "MEDIUM", &reason, 30)
            })
            .await
            {
                Ok(Ok(v)) if v.approved => Ok(()),
                Ok(Ok(_)) => Err(format!("skill write '{}' denied by user", operation)),
                Ok(Err(e)) => Err(format!("approval request failed: {}", e)),
                Err(e) => Err(format!("approval task failed: {}", e)),
            }
        }
    }

    fn do_create(
        &self,
        skill_dir: &Path,
        name: &str,
        v: &serde_json::Value,
    ) -> Result<String, String> {
        let content = v["content"]
            .as_str()
            .ok_or_else(|| "'content' is required for create".to_string())?;
        let overwrite = v["overwrite"].as_bool().unwrap_or(false);
        let skill_md = skill_dir.join("SKILL.md");
        if skill_md.exists() && !overwrite {
            return Err(format!(
                "skill '{}' already exists at {}. Set overwrite=true to replace.",
                name,
                skill_md.display()
            ));
        }
        let check = nemesis_skills::security_check::check_skill_security(content, name, "");
        if check.blocked {
            return Err(format!(
                "skill content blocked by security check: {}",
                check.block_reason
            ));
        }
        std::fs::create_dir_all(skill_dir)
            .map_err(|e| format!("failed to create skill dir: {}", e))?;
        if let Err(e) = std::fs::write(&skill_md, content) {
            let _ = std::fs::remove_dir_all(skill_dir);
            return Err(format!("failed to write SKILL.md: {}", e));
        }
        Ok(format!(
            "Skill '{}' created at {} (lint {:.0}/100{}).",
            name,
            skill_md.display(),
            check.lint_result.score * 100.0,
            check
                .quality_score
                .as_ref()
                .map(|q| format!(", quality {:.0}/100", q.overall))
                .unwrap_or_default()
        ))
    }

    fn do_patch(
        &self,
        skill_dir: &Path,
        name: &str,
        v: &serde_json::Value,
    ) -> Result<String, String> {
        let old = v["old"].as_str().unwrap_or("");
        let new = v["new"]
            .as_str()
            .ok_or_else(|| "'new' is required for patch/edit".to_string())?;
        let skill_md = skill_dir.join("SKILL.md");
        let mut content = std::fs::read_to_string(&skill_md)
            .map_err(|e| format!("failed to read SKILL.md for '{}': {}", name, e))?;
        if old.is_empty() {
            content.push_str(new);
        } else if let Some(idx) = content.find(old) {
            content.replace_range(idx..idx + old.len(), new);
        } else {
            return Err(format!(
                "'old' text not found in SKILL.md for skill '{}'",
                name
            ));
        }
        let check = nemesis_skills::security_check::check_skill_security(&content, name, "");
        if check.blocked {
            return Err(format!(
                "edited content blocked by security check: {}",
                check.block_reason
            ));
        }
        std::fs::write(&skill_md, &content)
            .map_err(|e| format!("failed to write SKILL.md: {}", e))?;
        Ok(format!(
            "Skill '{}' updated (lint {:.0}/100).",
            name,
            check.lint_result.score * 100.0
        ))
    }

    fn do_write_file(
        &self,
        skill_dir: &Path,
        name: &str,
        v: &serde_json::Value,
    ) -> Result<String, String> {
        if !skill_dir.exists() {
            return Err(format!(
                "skill '{}' has no directory yet; create it first",
                name
            ));
        }
        let path = v["path"]
            .as_str()
            .ok_or_else(|| "'path' is required for write_file".to_string())?;
        let content = v["content"]
            .as_str()
            .ok_or_else(|| "'content' is required for write_file".to_string())?;
        let overwrite = v["overwrite"].as_bool().unwrap_or(false);
        let target = resolve_within(skill_dir, path)?;
        if target.exists() && !overwrite {
            return Err(format!(
                "file already exists at {}. Set overwrite=true.",
                target.display()
            ));
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("failed to create dir: {}", e))?;
        }
        std::fs::write(&target, content).map_err(|e| format!("failed to write file: {}", e))?;
        Ok(format!(
            "Wrote {} ({} bytes) in skill '{}'.",
            target.display(),
            content.len(),
            name
        ))
    }

    fn do_remove_file(
        &self,
        skill_dir: &Path,
        name: &str,
        v: &serde_json::Value,
    ) -> Result<String, String> {
        if !skill_dir.exists() {
            return Err(format!("skill '{}' has no directory yet", name));
        }
        let path = v["path"]
            .as_str()
            .ok_or_else(|| "'path' is required for remove_file".to_string())?;
        let target = resolve_within(skill_dir, path)?;
        if !target.exists() {
            return Err(format!("file not found: {}", target.display()));
        }
        std::fs::remove_file(&target).map_err(|e| format!("failed to remove file: {}", e))?;
        Ok(format!(
            "Removed {} from skill '{}'.",
            target.display(),
            name
        ))
    }

    fn do_delete(&self, skill_dir: &Path, name: &str) -> Result<String, String> {
        if !skill_dir.exists() {
            return Err(format!(
                "skill '{}' not found at {}",
                name,
                skill_dir.display()
            ));
        }
        std::fs::remove_dir_all(skill_dir).map_err(|e| format!("failed to delete skill: {}", e))?;
        Ok(format!("Skill '{}' deleted.", name))
    }
}

#[async_trait]
impl Tool for SkillManageTool {
    fn description(&self) -> String {
        "Create, update, or delete reusable skills (the agent's procedural memory). \
         Persists a SKILL.md to workspace/skills/<name>/ so a workflow can be reused later \
         as a slash command. Actions: create (full SKILL.md incl. --- frontmatter ---), \
         patch/edit (replace old->new), write_file/remove_file (supporting files under the \
         skill dir), delete (remove whole skill). Security-checked before writing."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["create", "patch", "edit", "write_file", "remove_file", "delete"],
                    "description": "Operation to perform"
                },
                "name": {"type": "string", "description": "Skill slug (lowercase, digits, hyphens, <64 chars)"},
                "content": {"type": "string", "description": "Full SKILL.md (create) or file content (write_file)"},
                "path": {"type": "string", "description": "Sub-path within skill dir, e.g. references/api.md (write_file/remove_file)"},
                "old": {"type": "string", "description": "Exact text to replace (patch/edit)"},
                "new": {"type": "string", "description": "Replacement text (patch/edit)"},
                "overwrite": {"type": "boolean", "default": false, "description": "Allow overwriting an existing file"}
            },
            "required": ["action", "name"]
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let v: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid JSON arguments: {}", e))?;

        let action = v["action"]
            .as_str()
            .ok_or_else(|| "missing required 'action' field".to_string())?;
        let name = v["name"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "missing required 'name' field".to_string())?;

        // Path-traversal protection on the skill name.
        nemesis_skills::types::validate_skill_identifier(name)
            .map_err(|e| format!("invalid skill name '{}': {}", name, e))?;

        let skill_dir = self.skill_dir(name);

        // Approval gate for any write action (no-op unless manage_approval is on).
        if matches!(
            action,
            "create" | "patch" | "edit" | "write_file" | "remove_file" | "delete"
        ) {
            self.check_approval(
                action,
                &skill_dir.display().to_string(),
                &format!("skill '{}' — {}", name, action),
            )
            .await?;
        }

        match action {
            "create" => self.do_create(&skill_dir, name, &v),
            "patch" | "edit" => self.do_patch(&skill_dir, name, &v),
            "write_file" => self.do_write_file(&skill_dir, name, &v),
            "remove_file" => self.do_remove_file(&skill_dir, name, &v),
            "delete" => self.do_delete(&skill_dir, name),
            other => Err(format!(
                "unknown action '{}' (valid: create, patch, edit, write_file, remove_file, delete)",
                other
            )),
        }
    }
}

/// Resolve a relative path inside `base`, rejecting absolute paths and `..`
/// traversal so write_file/remove_file cannot escape the skill directory.
fn resolve_within(base: &Path, rel: &str) -> Result<PathBuf, String> {
    let rel_path = Path::new(rel);
    if rel_path.is_absolute() || rel.contains("..") {
        return Err(format!(
            "path must be a relative path within the skill dir, with no '..': {}",
            rel
        ));
    }
    let canon_base = base
        .canonicalize()
        .map_err(|e| format!("cannot canonicalize skill dir: {}", e))?;
    let canon_target = canon_base.join(rel_path);
    if !canon_target.starts_with(&canon_base) {
        return Err(format!("path escapes skill directory: {}", rel));
    }
    Ok(canon_target)
}

// ===========================================================================
// F7（devtool-upgrade 阶段 5）：question 工具 —— 结构化提问阻塞等答
// ===========================================================================

/// Shareable slot holding an optional question broker, filled in later by the
/// gateway (after the agent loop is built). Lets the `question` tool surface a
/// structured ask-the-user card on the Dashboard and block on the answer.
/// `None`（headless / exec_worker / 基线注册）= 工具不注册（模型根本看不到，
/// 而不是看到一个只会失败的调用）。
pub type QuestionBrokerSlot =
    Arc<parking_lot::RwLock<Option<Arc<dyn nemesis_types::agent::QuestionAsker>>>>;

/// question 工具默认等待窗口（秒）。超时回灌「按最佳判断继续」。
pub const QUESTION_DEFAULT_TIMEOUT_SECS: u64 = 120;

/// F7：结构化提问工具。向用户发一张选项卡（Dashboard QuestionCard）并阻塞
/// 等作答：单选 radio / `multi=true` 多选 checkbox。回答以选中项原文回灌，
/// 模型据此继续；超时回灌 "user did not answer, proceed with your best
/// judgment"（不是错误——轮次照常推进）。broker 经槽注入（gateway 装配
/// `WebQuestionBroker`）；进程内 id 自增避免并发提问撞号。
pub struct QuestionTool {
    broker: QuestionBrokerSlot,
}

impl QuestionTool {
    pub fn new(broker: QuestionBrokerSlot) -> Self {
        Self { broker }
    }
}

/// F7/J5 共用：提问卡 ID 发号器（question 工具与 doom-loop 审批卡同源，
/// 保证 Dashboard 上 ID 全局唯一）。
pub(crate) fn next_question_id() -> String {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    format!("q-{}", N.fetch_add(1, std::sync::atomic::Ordering::SeqCst))
}

#[async_trait]
impl Tool for QuestionTool {
    fn description(&self) -> String {
        "Ask the user a structured question with fixed options and block until they answer \
         (or it times out). Use when the choice materially changes what you do next — e.g. \
         which of two fix strategies to apply. Single-choice by default; multi=true lets the \
         user pick several. Returns the selected option text(s); on timeout returns a note \
         to proceed with your best judgment. Do NOT use for trivial choices or free-form \
         conversation."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "question": {
                    "type": "string",
                    "description": "The question to ask (clear and self-contained)"
                },
                "options": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "2-6 candidate answers the user picks from"
                },
                "multi": {
                    "type": "boolean",
                    "default": false,
                    "description": "true = user may select multiple options"
                }
            },
            "required": ["question", "options"]
        })
    }

    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String> {
        let v: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid JSON arguments: {}", e))?;

        let question = v["question"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "missing required 'question' field".to_string())?;
        let options: Vec<String> = v["options"]
            .as_array()
            .ok_or_else(|| "missing required 'options' field (array of strings)".to_string())?
            .iter()
            .map(|o| o.as_str().map(|s| s.trim().to_string()))
            .collect::<Option<Vec<String>>>()
            .ok_or_else(|| "'options' must be an array of strings".to_string())?;
        if options.len() < 2 || options.len() > 6 {
            return Err(format!(
                "'options' must have 2-6 entries, got {}",
                options.len()
            ));
        }
        if options.iter().any(String::is_empty) {
            return Err("'options' entries must be non-empty".to_string());
        }
        let multi = v["multi"].as_bool().unwrap_or(false);

        let broker = {
            let guard = self.broker.read();
            match guard.as_ref() {
                Some(b) => b.clone(),
                None => {
                    return Err(
                        "question tool unavailable: no interactive session wired".to_string()
                    );
                }
            }
        };

        let req = nemesis_types::agent::QuestionRequest {
            question_id: next_question_id(),
            question: question.to_string(),
            options,
            multi,
            chat_id: context.chat_id.clone(),
            session_key: context.session_key.clone(),
            timeout_secs: QUESTION_DEFAULT_TIMEOUT_SECS,
        };

        // ask 是同步阻塞（最长 120s）——spawn_blocking 出 worker 线程，
        // broker 内部再按上下文决定 block_in_place/直等（同审批先例）。
        let outcome = tokio::task::spawn_blocking(move || broker.ask(req))
            .await
            .map_err(|e| format!("question task failed: {}", e))??;

        match outcome {
            nemesis_types::agent::QuestionOutcome::Answered(selected) => {
                if selected.len() == 1 {
                    Ok(format!("User selected: {}", selected[0]))
                } else {
                    Ok(format!(
                        "User selected {} option(s): {}",
                        selected.len(),
                        selected.join("; ")
                    ))
                }
            }
            nemesis_types::agent::QuestionOutcome::Timeout => {
                Ok("No answer: user did not answer, proceed with your best judgment.".to_string())
            }
        }
    }
}

// ===========================================================================
// Coding tools (grep / git)
// ===========================================================================

/// Grep tool — regex search across workspace files. Returns file:line: match.
pub struct GrepTool {
    workspace: String,
}

impl GrepTool {
    pub fn new(workspace: String) -> Self {
        Self { workspace }
    }
}

#[async_trait]
impl Tool for GrepTool {
    fn description(&self) -> String {
        "Search file contents with a regex pattern across the workspace. Returns matching \
         lines as file:line: content. Use to find code, symbols, or text. Faster and more \
         structured than exec grep."
            .to_string()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string", "description": "Regex pattern to search for"},
                "path": {"type": "string", "description": "Directory to search (default: workspace root)"},
                "glob": {"type": "string", "description": "File-name filter, e.g. *.rs (simple suffix match)"},
                "max_results": {"type": "integer", "description": "Max matches (default 50)"}
            },
            "required": ["pattern"]
        })
    }
    async fn execute(&self, args: &str, _ctx: &RequestContext) -> Result<String, String> {
        let v: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid JSON: {}", e))?;
        let pattern = v["pattern"].as_str().ok_or("missing 'pattern'")?;
        let re = regex::Regex::new(pattern).map_err(|e| format!("invalid regex: {}", e))?;
        let root = v["path"]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| self.workspace.clone());
        let glob = v["glob"].as_str();
        let max = v["max_results"].as_u64().unwrap_or(50) as usize;
        let mut out: Vec<String> = Vec::new();
        grep_recursive(Path::new(&root), &re, glob, max, &mut out);
        if out.is_empty() {
            Ok(format!("No matches for pattern /{}/", pattern))
        } else {
            Ok(format!(
                "Found {} match(es):\n{}",
                out.len(),
                out.join("\n")
            ))
        }
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

/// Git tool — read queries (status/diff/log/show/branch) plus the daily-safe
/// write actions (add/commit/branch_create/checkout/restore/stash) in the
/// workspace. D1 (2026-09-04): the write surface is enum-whitelisted — push,
/// reset --hard, clean, rebase … are intentionally NOT reachable here (the
/// model must use exec for those, which goes through the 8-layer security
/// pipeline and approval). Write actions take dedicated params (paths/message/
/// name/ref), not the free-form `args` string, so flags can't be smuggled in.
pub struct GitTool {
    workspace: String,
}

impl GitTool {
    pub fn new(workspace: String) -> Self {
        Self { workspace }
    }

    /// Run git with `args` in the workspace and format the output using the
    /// three-state convention shared by all read actions:
    /// fail with empty stdout → Err(stderr) / empty output → no-changes note /
    /// otherwise → stdout. (D1: write actions reuse the same three states.)
    fn run_three_state(&self, args: &[&str], action: &str) -> Result<String, String> {
        let out = std::process::Command::new("git")
            .current_dir(&self.workspace)
            .args(args)
            .output()
            .map_err(|e| format!("failed to run git: {}", e))?;
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        if !out.status.success() && stdout.trim().is_empty() {
            Err(format!("git {} failed: {}", action, stderr.trim()))
        } else if stdout.trim().is_empty() {
            Ok(format!("(no changes / empty)\n{}", stderr.trim()))
        } else {
            Ok(stdout)
        }
    }

    /// Write actions take dedicated keys, not the free-form `args` string
    /// (D1: anti flag-smuggling). `args` on a write action is an honest error
    /// that tells the model the right shape (same philosophy as A1 edit hints).
    fn reject_freeform_args(
        &self,
        v: &serde_json::Value,
        action: &str,
        takes_args: bool,
    ) -> Result<(), String> {
        if !takes_args && !v["args"].as_str().unwrap_or("").trim().is_empty() {
            return Err(format!(
                "action '{}' takes dedicated params, not free-form 'args' (see the tool schema)",
                action
            ));
        }
        Ok(())
    }

    fn required_str<'a>(
        &self,
        v: &'a serde_json::Value,
        key: &str,
        action: &str,
    ) -> Result<&'a str, String> {
        v[key]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("action '{}' requires a non-empty '{}' param", action, key))
    }
}

#[async_trait]
impl Tool for GitTool {
    fn description(&self) -> String {
        "Run git commands in the workspace. Read: status, diff, log, show, branch. Safe writes \
         (D1): add, commit, branch_create, checkout, restore, stash (list/push/pop). Dangerous \
         git ops (push, reset --hard, clean, rebase, ...) are intentionally NOT exposed — use \
         the exec tool for those (it goes through the security pipeline and approval)."
            .to_string()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["status", "diff", "log", "show", "branch", "add", "commit", "branch_create", "checkout", "restore", "stash"], "description": "Git subcommand"},
                "args": {"type": "string", "description": "Extra args for read actions (e.g. a file path for diff/show, or '-10' for log count); for 'stash' it selects the op: list (default), push, or pop"},
                "paths": {"type": "string", "description": "Whitespace-separated paths for add/restore (e.g. 'src/main.rs' or '-A' to stage everything)"},
                "message": {"type": "string", "description": "Commit message for 'commit' (required); optional message for 'stash' push"},
                "name": {"type": "string", "description": "Branch name for 'branch_create'"},
                "ref": {"type": "string", "description": "Branch or commit to switch to for 'checkout'"}
            },
            "required": ["action"]
        })
    }
    async fn execute(&self, args: &str, _ctx: &RequestContext) -> Result<String, String> {
        let v: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid JSON: {}", e))?;
        let action = v["action"].as_str().ok_or("missing 'action'")?;
        let extra = v["args"].as_str().unwrap_or("");

        // --- Read actions: free-form extra args, as before (D1 unchanged) ---
        let base: Vec<&str> = match action {
            "status" => vec!["status", "--short", "--branch"],
            "diff" => vec!["diff"],
            "log" => vec!["log", "--oneline", "-20"],
            "show" => vec!["show"],
            "branch" => vec!["branch", "-vv"],
            // --- Write actions (D1): dedicated params, enum-whitelisted ---
            "add" => {
                self.reject_freeform_args(&v, action, false)?;
                let paths = self.required_str(&v, "paths", action)?;
                let mut base = vec!["add"];
                base.extend(paths.split_whitespace());
                return self.run_three_state(&base, action);
            }
            "commit" => {
                self.reject_freeform_args(&v, action, false)?;
                let message = self.required_str(&v, "message", action)?;
                // Not run_three_state: `git commit` prints its failure text
                // ("nothing to commit", hook rejections) on *stdout* with exit
                // code 1, which the shared three-state would surface as Ok —
                // a failed commit must be an honest Err. Success text goes to
                // stdout as usual.
                let out = std::process::Command::new("git")
                    .current_dir(&self.workspace)
                    .args(["commit", "-m", message])
                    .output()
                    .map_err(|e| format!("failed to run git: {}", e))?;
                let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                if !out.status.success() {
                    let detail = if stderr.trim().is_empty() {
                        stdout.trim()
                    } else {
                        stderr.trim()
                    };
                    return Err(format!("git commit failed: {}", detail));
                }
                // Best-effort: show the new commit (hash + subject) so the
                // model immediately sees the result of the write.
                let mut result = stdout;
                if let Ok(o) = std::process::Command::new("git")
                    .current_dir(&self.workspace)
                    .args(["log", "--oneline", "-1"])
                    .output()
                {
                    let line = String::from_utf8_lossy(&o.stdout);
                    if !line.trim().is_empty() {
                        result.push_str(&format!("\nLast commit: {}", line.trim()));
                    }
                }
                return Ok(result);
            }
            "branch_create" => {
                self.reject_freeform_args(&v, action, false)?;
                let name = self.required_str(&v, "name", action)?;
                return self.run_three_state(&["branch", name], action);
            }
            "checkout" => {
                self.reject_freeform_args(&v, action, false)?;
                let target = self.required_str(&v, "ref", action)?;
                return self.run_three_state(&["checkout", target], action);
            }
            "restore" => {
                self.reject_freeform_args(&v, action, false)?;
                let paths = self.required_str(&v, "paths", action)?;
                let mut base = vec!["restore"];
                base.extend(paths.split_whitespace());
                return self.run_three_state(&base, action);
            }
            "stash" => {
                // Op whitelist: only list/push/pop (empty = list). Drop/clear/
                // apply and any flags are rejected — stash stays recoverable.
                let op = extra.trim();
                let base: Vec<&str> = match op {
                    "" | "list" => vec!["stash", "list"],
                    "push" => {
                        let mut base = vec!["stash", "push"];
                        if let Some(m) = v["message"].as_str().filter(|m| !m.trim().is_empty()) {
                            base.push("-m");
                            base.push(m.trim());
                        }
                        base
                    }
                    "pop" => vec!["stash", "pop"],
                    other => {
                        return Err(format!(
                            "unsupported stash op '{}' (allowed: list, push, pop)",
                            other
                        ));
                    }
                };
                return self.run_three_state(&base, action);
            }
            other => {
                return Err(format!(
                    "unknown git action '{}' (write actions: add/commit/branch_create/checkout/restore/stash; \
                     push/reset --hard and other dangerous ops are not exposed — use exec, \
                     which goes through the security pipeline)",
                    other
                ));
            }
        };
        let mut cmd_args = base;
        if !extra.is_empty() {
            cmd_args.extend(extra.split_whitespace());
        }
        self.run_three_state(&cmd_args, action)
    }
}

/// Recursively search `dir` for regex matches, collecting file:line: content lines.
/// Skips hidden/build/dep dirs and large or non-UTF-8 files.
fn grep_recursive(
    dir: &Path,
    re: &regex::Regex,
    glob: Option<&str>,
    max: usize,
    out: &mut Vec<String>,
) {
    if out.len() >= max {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        if out.len() >= max {
            return;
        }
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with('.')
                || matches!(name, "target" | "node_modules" | "dist" | "build" | ".git")
            {
                continue;
            }
            grep_recursive(&path, re, glob, max, out);
        } else if path.is_file() {
            if let Some(g) = glob
                && let Some(fname) = path.file_name().and_then(|n| n.to_str())
            {
                let suffix = g.trim_start_matches('*');
                if !fname.ends_with(suffix) {
                    continue;
                }
            }
            if let Ok(meta) = std::fs::metadata(&path)
                && meta.len() > 1_000_000
            {
                continue;
            }
            if let Ok(content) = std::fs::read_to_string(&path) {
                for (i, line) in content.lines().enumerate() {
                    if out.len() >= max {
                        return;
                    }
                    if re.is_match(line) {
                        let display = line.trim();
                        if display.len() > 300 {
                            // Truncate at the nearest char boundary ≤ 300 bytes.
                            // Slicing at a fixed byte index can land inside a
                            // multibyte UTF-8 char (e.g. Chinese) and panic.
                            let mut end = 300;
                            while !display.is_char_boundary(end) {
                                end -= 1;
                            }
                            out.push(format!(
                                "{}:{}: {}…",
                                path.display(),
                                i + 1,
                                &display[..end]
                            ));
                        } else {
                            out.push(format!("{}:{}: {}", path.display(), i + 1, display));
                        }
                    }
                }
            }
        }
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
            return Err(
                "I2C is only supported on Linux. This tool requires /dev/i2c-* device files."
                    .to_string(),
            );
        }
        let val: serde_json::Value =
            serde_json::from_str(args).map_err(|_| "Invalid JSON arguments".to_string())?;
        let action = val["action"].as_str().unwrap_or("");
        match action {
            "detect" => Ok("[I2C] Detect: scanning for I2C buses...".to_string()),
            "scan" => Ok(format!(
                "[I2C] Scan on bus {}",
                val["bus"].as_str().unwrap_or("?")
            )),
            "read" => Ok(format!(
                "[I2C] Read from device at address {}",
                val["address"].as_u64().unwrap_or(0)
            )),
            "write" => {
                if val["confirm"].as_bool().unwrap_or(false) {
                    Ok(format!(
                        "[I2C] Write to device at address {}",
                        val["address"].as_u64().unwrap_or(0)
                    ))
                } else {
                    Err("confirm must be true for write operations (safety guard)".to_string())
                }
            }
            _ => Err(format!(
                "Unknown I2C action: {} (valid: detect, scan, read, write)",
                action
            )),
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
            return Err(
                "SPI is only supported on Linux. This tool requires /dev/spidev* device files."
                    .to_string(),
            );
        }
        let val: serde_json::Value =
            serde_json::from_str(args).map_err(|_| "Invalid JSON arguments".to_string())?;
        let action = val["action"].as_str().unwrap_or("");
        match action {
            "list" => Ok("[SPI] Listing SPI devices...".to_string()),
            "transfer" => {
                if val["confirm"].as_bool().unwrap_or(false) {
                    Ok(format!(
                        "[SPI] Transfer on device {}",
                        val["device"].as_str().unwrap_or("?")
                    ))
                } else {
                    Err("confirm must be true for transfer operations (safety guard)".to_string())
                }
            }
            "read" => Ok(format!(
                "[SPI] Read {} bytes from device {}",
                val["length"].as_u64().unwrap_or(1),
                val["device"].as_str().unwrap_or("?")
            )),
            _ => Err(format!(
                "Unknown SPI action: {} (valid: list, transfer, read)",
                action
            )),
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
    tools.insert("write_file".to_string(), Box::new(WriteFileTool::default()));
    tools.insert("list_dir".to_string(), Box::new(ListDirectoryTool));
    tools.insert("edit_file".to_string(), Box::new(EditFileTool::default()));
    tools.insert("multiedit".to_string(), Box::new(MultiEditTool::default()));
    tools.insert(
        "append_file".to_string(),
        Box::new(AppendFileTool::default()),
    );
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
            request_timeout: Duration::from_secs(
                nemesis_types::constants::RPC_CHANNEL_TIMEOUT_SECS,
            ),
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
#[cfg(feature = "forge")]
struct ForgeBridgeTool {
    name: String,
    description: String,
    parameters: serde_json::Value,
    executor: Arc<nemesis_forge::forge_tools::ForgeToolExecutor>,
}

#[cfg(feature = "forge")]
impl ForgeBridgeTool {
    fn new(
        name: String,
        description: String,
        parameters: serde_json::Value,
        executor: Arc<nemesis_forge::forge_tools::ForgeToolExecutor>,
    ) -> Self {
        Self {
            name,
            description,
            parameters,
            executor,
        }
    }
}

#[cfg(feature = "forge")]
#[async_trait]
impl Tool for ForgeBridgeTool {
    fn description(&self) -> String {
        self.description.clone()
    }

    fn parameters(&self) -> serde_json::Value {
        self.parameters.clone()
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let args_value =
            serde_json::from_str::<serde_json::Value>(args).unwrap_or(serde_json::Value::Null);
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

impl Default for McpDiscoverTool {
    fn default() -> Self {
        Self::new()
    }
}

impl McpDiscoverTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for McpDiscoverTool {
    fn description(&self) -> String {
        "Discover what tools, resources, and prompts an MCP server provides. \
         For stdio-based servers provide the 'command' (executable path); \
         for HTTP-based servers provide the 'url' (e.g. 'http://localhost:8080/mcp'). \
         This tool will connect, query capabilities, and return a formatted summary."
            .to_string()
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
        let parsed =
            serde_json::from_str::<serde_json::Value>(args).unwrap_or(serde_json::Value::Null);

        let url = parsed["url"].as_str();
        let command = parsed["command"].as_str();
        let timeout_secs = parsed["timeout"].as_u64().unwrap_or(15);

        let result = match (url, command) {
            (Some(url), _) => {
                nemesis_mcp::manager::discover_server_metadata_http(url, timeout_secs).await
            }
            (None, Some(command)) => {
                let tool_args: Vec<String> = parsed["args"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                nemesis_mcp::manager::discover_server_metadata(
                    command,
                    tool_args,
                    vec![],
                    timeout_secs,
                )
                .await
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

impl Default for CliReferenceTool {
    fn default() -> Self {
        Self::new()
    }
}

impl CliReferenceTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for CliReferenceTool {
    fn description(&self) -> String {
        "Look up NemesisBot CLI commands. Without parameters returns an overview of all commands. \
         Pass a command name for detailed help (e.g. 'model', 'mcp', 'cluster', 'scanner')."
            .to_string()
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
        let parsed =
            serde_json::from_str::<serde_json::Value>(args).unwrap_or(serde_json::Value::Null);
        let command = parsed["command"].as_str().unwrap_or("").trim();

        if command.is_empty() {
            Ok(cli_overview())
        } else {
            cli_detail(command)
        }
    }
}

/// U20 (sixth batch): cross-session full-text search over chat history
/// (session_logs). Lazy-indexes on first use (FTS5), falls back to a linear
/// scan if the index DB is unavailable. Read-only.
pub struct HistorySearchTool;

impl Default for HistorySearchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl HistorySearchTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for HistorySearchTool {
    fn description(&self) -> String {
        "Search past conversation history across ALL sessions (FTS full-text). \
         Returns session key + snippet + timestamp for each hit — use it to find \
         which conversation discussed a topic or contains specific wording. \
         Supports Chinese and English."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search text (words or a phrase)."
                },
                "limit": {
                    "type": "integer",
                    "description": "Max hits (default 10, max 100)."
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let v: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid arguments: {}", e))?;
        let query = v
            .get("query")
            .and_then(|q| q.as_str())
            .ok_or("Missing 'query' argument")?;
        if query.trim().is_empty() {
            return Err("'query' must not be empty".to_string());
        }
        let limit = v.get("limit").and_then(|l| l.as_u64()).unwrap_or(10) as usize;
        // Lazy full index on first use (mtime-incremental after that).
        crate::history_search::reindex_session_logs();
        let hits = crate::history_search::search(query, limit);
        Ok(crate::history_search::render_hits(&hits))
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
  nemesisbot model remove gpt-4o --force"#
            .to_string()),

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
  nemesisbot mcp discover --command C:\AI\MCP\server.exe"#
            .to_string()),

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
  nemesisbot channel web port 49000"#
            .to_string()),

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
  nemesisbot cluster token generate --save"#
            .to_string()),

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
  nemesisbot skills list"#
            .to_string()),

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
  nemesisbot forge list"#
            .to_string()),

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
  nemesisbot cron remove abc123"#
            .to_string()),

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
  nemesisbot security approve 123"#
            .to_string()),

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
  nemesisbot scanner clamav test /path/to/file"#
            .to_string()),

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
  nemesisbot log general level debug"#
            .to_string()),

        "auth" => Ok(r#"## auth — 管理认证

用法: `nemesisbot auth <子命令>`

子命令:
  login --provider <名称>       登录（OAuth 或粘贴令牌）
            --device-code       使用设备码流程
  logout --provider <名称>      登出（省略名称则登出全部）
  status                        查看认证状态

示例:
  nemesisbot auth login --provider openai
  nemesisbot auth status"#
            .to_string()),

        "memory" => Ok(r#"## memory — 管理增强内存

用法: `nemesisbot memory <子命令>`

子命令:
  enable    启用增强内存（需要 plugin_onnx.dll 在 plugins/ 目录）
  disable   禁用增强内存
  status    查看内存系统状态

示例:
  nemesisbot memory status
  nemesisbot memory enable"#
            .to_string()),

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
  nemesisbot workflow template list"#
            .to_string()),

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
  nemesisbot cors dev-mode enable"#
            .to_string()),

        "status" => Ok(r#"## status — 显示系统状态

用法: `nemesisbot status`

显示当前系统配置和运行状态。"#
            .to_string()),

        "version" => Ok(r#"## version — 显示版本信息

用法: `nemesisbot version`

显示 NemesisBot 版本号和构建信息。"#
            .to_string()),

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
            if let Some(props) = tool
                .input_schema
                .get("properties")
                .and_then(|p| p.as_object())
            {
                let required: Vec<&str> = tool
                    .input_schema
                    .get("required")
                    .and_then(|r| r.as_array())
                    .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                    .unwrap_or_default();

                let param_summary: Vec<String> = props
                    .iter()
                    .map(|(name, schema)| {
                        let type_str = schema.get("type").and_then(|t| t.as_str()).unwrap_or("any");
                        if required.contains(&name.as_str()) {
                            format!("{}* ({})", name, type_str)
                        } else {
                            format!("{} ({})", name, type_str)
                        }
                    })
                    .collect();

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
                let args: Vec<String> = prompt
                    .arguments
                    .iter()
                    .map(|a| {
                        let req = if a.required.unwrap_or(false) { "*" } else { "" };
                        let desc = a.description.as_deref().unwrap_or("");
                        if desc.is_empty() {
                            format!("{}{}", a.name, req)
                        } else {
                            format!("{}{} ({})", a.name, req, desc)
                        }
                    })
                    .collect();
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
         Use this to see what MCP tools are available in the current session."
            .to_string()
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
#[derive(Clone, Default)]
pub struct SharedToolConfig {
    /// Web search configuration.
    pub web_search: Option<WebSearchConfig>,
    /// Cluster RPC configuration.
    pub cluster_rpc: Option<ClusterRpcConfig>,
    /// Spawn/subagent configuration.
    pub spawn: Option<SpawnConfig>,
    /// G0 (devtool-upgrade 阶段 3)：spawn 闭包共享槽——agent_factory 构造
    /// slot 并在 AgentLoop 组装完成后注入闭包（持 `Weak<AgentLoop>`），
    /// SpawnTool 与其共享同一 Arc。`None` = SpawnTool 自建空槽（行为与
    /// 旧「无 spawn_fn」完全一致：execute 诚实报 not available）。
    pub spawn_slot: Option<Arc<std::sync::OnceLock<SpawnFn>>>,
    /// Skills registry manager for find/install tools.
    pub skills_registry: Option<Arc<nemesis_skills::registry::RegistryManager>>,
    /// Skills loader for listing local skills.
    pub skills_loader: Option<Arc<nemesis_skills::loader::SkillsLoader>>,
    /// Workspace path for skill installation.
    pub workspace: Option<String>,
    /// Cron service for scheduling jobs.
    pub cron_service: Option<Arc<std::sync::Mutex<nemesis_cron::service::CronService>>>,
    /// H7 (U13 half): enable the claude_code delegation tool. Default false
    /// (opt-in) — a user with the CLI installed is never surprised by it.
    pub claude_code_tool_enabled: bool,
    /// H7: wall-clock timeout per delegation (None = 300s default).
    pub claude_code_tool_timeout_secs: Option<u64>,
    /// T5 (U13): fixed claude permission tier (empty = default accept_edits;
    /// enum default/accept_edits/plan/bypass_permissions). NOT in the tool
    /// schema — deployment config governs, the model cannot choose it.
    pub claude_code_tool_permission_mode: String,
    /// I4 (U13 half): enable the codex_delegate tool. Default false.
    pub codex_tool_enabled: bool,
    /// I4: wall-clock timeout per codex delegation (None = 300s default).
    pub codex_tool_timeout_secs: Option<u64>,
    /// T5 (U13): fixed codex sandbox tier (empty = default read_only; enum
    /// read_only/workspace_write/danger_full_access, snake_case config →
    /// kebab-case CLI value). NOT in the tool schema.
    pub codex_tool_sandbox: String,
    /// L1 (U19): enable the read-only `lsp` semantic-code tool. Default
    /// false — and even when true, registration additionally requires at
    /// least one language server on PATH (probe at registration).
    pub lsp_tool_enabled: bool,
    /// L1: per-request LSP timeout (None = 120s default).
    pub lsp_tool_timeout_secs: Option<u64>,
    /// L1: idle session reap threshold (None = 600s default).
    pub lsp_tool_idle_secs: Option<u64>,
    /// C5（2026-09-04）：外部 LSP manager 单例（agent_factory 在
    /// SharedResources 建一次；gateway shutdown_all 可达）。None = 注册路径
    /// 自建 manager（`LspTool::new` 兜底，旧行为——测试/独立 runner 用）。
    pub lsp_manager: Option<Arc<nemesis_lsp::LspManager>>,
    /// Forge tool executor for self-learning tools (forge_reflect, forge_create, etc).
    #[cfg(feature = "forge")]
    pub forge_executor: Option<Arc<nemesis_forge::forge_tools::ForgeToolExecutor>>,
    #[cfg(not(feature = "forge"))]
    pub forge_executor: Option<()>,
    /// Forge instance for experience collection in AgentLoop.
    #[cfg(feature = "forge")]
    pub forge: Option<Arc<nemesis_forge::forge::Forge>>,
    #[cfg(not(feature = "forge"))]
    pub forge: Option<()>,
    /// Memory tool executor for memory_search, memory_store, etc.
    #[cfg(feature = "memory")]
    pub memory_executor: Option<Arc<nemesis_memory::memory_tools::MemoryToolExecutor>>,
    #[cfg(not(feature = "memory"))]
    #[allow(dead_code)]
    pub memory_executor: Option<()>,
    /// Snapshot of registered MCP tool names and descriptions for McpListTool.
    pub mcp_tool_snapshot: Option<Arc<parking_lot::RwLock<Vec<(String, String)>>>>,
    /// Workflow engine reference for the `workflow_run` agent tool.
    /// `None` means workflows aren't wired in (tool stays unregistered).
    #[cfg(feature = "workflow")]
    pub workflow_engine: Option<Arc<nemesis_workflow::engine::WorkflowEngine>>,
    #[cfg(not(feature = "workflow"))]
    #[allow(dead_code)]
    pub workflow_engine: Option<()>,
    /// Optional approval manager slot for skill_manage write approval.
    pub approval_manager: Option<ApprovalManagerSlot>,
    /// Whether skill_manage writes require interactive approval.
    pub skills_manage_approval: bool,
    /// J2a（2026-09-04）：SSRF 闸宿主（SecurityPlugin），web_fetch 重定向
    /// 循环逐跳复查用。None = 闸不生效（跳数上限仍生效）。执行体子进程按
    /// 设计不注入（哑执行——判断在 gateway，且 web_fetch 不在 MOVE_TOOLS，
    /// 子进程里的 web_fetch 本就不会被调用）。
    #[cfg(feature = "security")]
    pub security: Option<Arc<nemesis_security::pipeline::SecurityPlugin>>,
    #[cfg(not(feature = "security"))]
    #[allow(dead_code)]
    pub security: Option<()>,
    /// A5（2026-09-04）：文件工具工作区边界（write_file / edit_file /
    /// append_file 纵深防御，不单靠安全 8 层管线）。None = 不设界（基线
    /// 注册 / 测试形态）。executor 子进程按同根注入（NEMESISBOT_EXECUTOR_
    /// WORKSPACE），保证 Layer-1 分离后写路径仍受同一边界约束。
    pub workspace_boundary: Option<Arc<WorkspaceBoundary>>,
    /// H1（2026-09-05）：todowrite 工具配置（workspace 存储 + TodoUpdated
    /// 广播通道）。None = 不注册该工具（register_default_tools 基线形态 /
    /// 无 workspace 的测试）。事件通道与 M1a ToolEventHook 共用同一
    /// broadcast（gateway 建、web pump 消费）；None tx = 仍落盘、不广播。
    pub todo: Option<TodoToolConfig>,
    /// B4（2026-09-05）：后台进程注册表（gateway 级单例，跨 agent 重启
    /// 存活；Drop 时给残余任务发 kill 旗标）。Some 时注册
    /// background_start / background_output / background_kill 三件套。
    /// None = 不注册（exec_worker 子进程——per-call 生命周期里起后台任务
    /// 必然孤儿，见 remote_executor_tool.rs 的 exec_async 注记；以及
    /// register_default_tools 基线/测试形态）。
    pub background_registry: Option<Arc<BackgroundProcessRegistry>>,
    /// F7（2026-09-06）：question 工具的 broker 槽（gateway 装配后填
    /// `WebQuestionBroker`）。None = 不注册该工具（headless / exec_worker /
    /// register_default_tools 基线形态——模型看不到一个只会失败的调用）。
    pub question_broker: Option<QuestionBrokerSlot>,
}

/// H1（2026-09-05）：`todowrite` 工具的接线配置。
#[derive(Clone, Debug)]
pub struct TodoToolConfig {
    /// 存储根（`{workspace}/sessions/todo_{safe_session_key}.json`）。
    pub workspace: std::path::PathBuf,
    /// TodoUpdated 广播通道（与 M1a ToolEventHook 同一 sender；None =
    /// 只落盘不广播——CLI / exec_worker 形态）。
    pub event_tx: Option<tokio::sync::broadcast::Sender<nemesis_types::agent::AgentEvent>>,
}

impl std::fmt::Debug for SharedToolConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedToolConfig")
            .field("web_search", &self.web_search)
            .field("cluster_rpc", &self.cluster_rpc)
            .field("spawn", &self.spawn)
            .field(
                "skills_registry",
                &self.skills_registry.as_ref().map(|_| "RegistryManager"),
            )
            .field(
                "skills_loader",
                &self.skills_loader.as_ref().map(|_| "SkillsLoader"),
            )
            .field("workspace", &self.workspace)
            .field(
                "memory_executor",
                &self.memory_executor.as_ref().map(|_| "MemoryToolExecutor"),
            )
            .field(
                "mcp_tool_snapshot",
                &self.mcp_tool_snapshot.as_ref().map(|_| "McpToolSnapshot"),
            )
            .field(
                "workflow_engine",
                &self.workflow_engine.as_ref().map(|_| "WorkflowEngine"),
            )
            .field(
                "approval_manager",
                &self
                    .approval_manager
                    .as_ref()
                    .map(|_| "ApprovalManagerSlot"),
            )
            .field("skills_manage_approval", &self.skills_manage_approval)
            .field("todo", &self.todo)
            .field(
                "question_broker",
                &self.question_broker.as_ref().map(|_| "QuestionBrokerSlot"),
            )
            .field(
                "security",
                &self.security.as_ref().map(|_| "SecurityPlugin"),
            )
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

    // A5（2026-09-04）：配置了工作区边界时，write/edit/append 换成带界的
    // 生产形态（纵深防御，不单靠安全 8 层管线）。无界（测试 / 基线注册）
    // 保持 register_default_tools 的默认形态。
    if let Some(ref boundary) = config.workspace_boundary {
        tools.insert(
            "write_file".to_string(),
            Box::new(WriteFileTool::with_boundary(boundary.clone())),
        );
        tools.insert(
            "edit_file".to_string(),
            Box::new(EditFileTool::with_boundary(boundary.clone())),
        );
        tools.insert(
            "multiedit".to_string(),
            Box::new(MultiEditTool::with_boundary(boundary.clone())),
        );
        tools.insert(
            "append_file".to_string(),
            Box::new(AppendFileTool::with_boundary(boundary.clone())),
        );
    }

    // H1（2026-09-05）：todowrite —— 全量提交式 todo 清单，写
    // sessions/todo_{safe_key}.json + 广播 TodoUpdated（web pump → WS push
    // + SSE）。None = 不注册（register_default_tools 基线形态零开销）。
    if let Some(ref todo) = config.todo {
        tools.insert(
            "todowrite".to_string(),
            Box::new(TodoWriteTool::new(
                todo.workspace.clone(),
                todo.event_tx.clone(),
            )),
        );
    }

    // Web search tool.
    if let Some(ref web_config) = config.web_search {
        tools.insert(
            "web_search".to_string(),
            Box::new(WebSearchTool::new(web_config.clone())),
        );
    }

    // Web fetch tool (always available). J2a（2026-09-04）：注入 SSRF 闸
    // 宿主——重定向手动循环逐跳复查（无闸时仍限跳数 ≤5）。mut 只在
    // security 形态用到（ssrf 注入），minimal 形态 cfg_attr 压 unused_mut
    // （2026-09-05 远端 minimal clippy 实录；不能删 mut——security 形态需要）。
    // J2b（2026-09-06）：注入 workspace 根（超限全文存档 spill 用）。web_fetch
    // 无条件注册（与 workspace 有无无关）——workspace 缺席时 with_workspace
    // 收空串，超限退回旧「截断+注记」行为（诚实不静默）。
    #[cfg_attr(not(feature = "security"), allow(unused_mut))]
    let mut web_fetch =
        WebFetchTool::new(50000).with_workspace(config.workspace.as_deref().unwrap_or(""));
    #[cfg(feature = "security")]
    {
        web_fetch.ssrf = config.security.clone();
    }
    tools.insert("web_fetch".to_string(), Box::new(web_fetch));

    // Cluster RPC tool (bot-to-bot communication).
    if let Some(ref cluster_config) = config.cluster_rpc {
        tools.insert(
            "cluster_rpc".to_string(),
            Box::new(ClusterRpcTool::new(cluster_config.clone())),
        );
    }

    // Spawn/subagent tool.
    // G0：spawn_slot Some 时共享槽（factory 延迟注入闭包）；None 自建空槽
    // （not available 行为与旧装配路径完全一致）。
    if let Some(ref spawn_config) = config.spawn {
        let tool = match config.spawn_slot.clone() {
            Some(slot) => SpawnTool::with_spawn_slot(
                spawn_config.clone(),
                slot,
                Arc::new(tokio::sync::Semaphore::new(
                    spawn_config.max_concurrent.max(1),
                )),
            ),
            None => SpawnTool::new(spawn_config.clone()),
        };
        tools.insert("spawn".to_string(), Box::new(tool));
    }

    // Memory tools.
    #[cfg(feature = "memory")]
    {
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
    }

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

    // Skill manage tool — agent-authored skills (procedural memory).
    if let Some(ref workspace) = config.workspace {
        // approval_manager 按 cfg 取值：security 形态是 Arc 槽需 clone，
        // minimal 形态别名退化为 ()（Copy）——clone 会被判 clone_on_copy
        // （2026-09-05 远端 minimal clippy 实录）。
        #[cfg(feature = "security")]
        let approval_manager = config.approval_manager.clone();
        #[cfg(not(feature = "security"))]
        let approval_manager = config.approval_manager;
        tools.insert(
            "skill_manage".to_string(),
            Box::new(SkillManageTool::new(
                workspace.clone(),
                approval_manager,
                config.skills_manage_approval,
            )),
        );
    }

    // F7（2026-09-06）: question tool — structured ask-the-user card on the
    // Dashboard, blocking on the answer. Only registered when a broker is
    // wired (gateway); headless / baseline forms never see it.
    if let Some(ref slot) = config.question_broker {
        tools.insert(
            "question".to_string(),
            Box::new(QuestionTool::new(slot.clone())),
        );
    }

    // Coding tools (grep / git) — read-only code search & git queries.
    if let Some(ref workspace) = config.workspace {
        tools.insert(
            "grep".to_string(),
            Box::new(GrepTool::new(workspace.clone())),
        );
        tools.insert("git".to_string(), Box::new(GitTool::new(workspace.clone())));
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

        // run_script: interpreter-driven script execution for the workflow
        // `script` node. Listed in MOVE_TOOLS so executor separation / sandbox
        // contains workflow scripts identically to `exec`. Returns structured
        // {stdout,stderr,exit_code} (not flattened), preserving the script
        // node's output contract.
        tools.insert(
            "run_script".to_string(),
            Box::new(RunScriptTool::new(workspace, restrict)),
        );

        // Bootstrap completion tool — deletes BOOTSTRAP.md after initialization.
        tools.insert(
            "complete_bootstrap".to_string(),
            Box::new(BootstrapTool::new(workspace)),
        );

        // C8 (2026-09-06): build/test runner with focused output. In
        // MOVE_TOOLS (builds write target/; executor separation & sandbox
        // should contain it; plan mode blocks it via the same derivation).
        tools.insert(
            "run_checks".to_string(),
            Box::new(RunChecksTool::new(workspace)),
        );
    }

    // B4 (2026-09-05): background process trio — needs BOTH a workspace (cwd
    // resolution / boundary) and a gateway-level registry. NOT in MOVE_TOOLS:
    // job handles live in this registry (gateway process lifetime), so a
    // per-call executor child could not own them — see background_registry.rs
    // top docs. exec_worker passes background_registry: None ⇒ not registered
    // there.
    if let (Some(workspace), Some(registry)) = (
        config.workspace.as_deref(),
        config.background_registry.as_ref(),
    ) {
        let restrict = true; // cwd confined to workspace, same as exec
        tools.insert(
            "background_start".to_string(),
            Box::new(BackgroundStartTool::new(
                workspace,
                restrict,
                Arc::clone(registry),
            )),
        );
        tools.insert(
            "background_output".to_string(),
            Box::new(BackgroundOutputTool::new(Arc::clone(registry))),
        );
        tools.insert(
            "background_kill".to_string(),
            Box::new(BackgroundKillTool::new(Arc::clone(registry))),
        );
    }

    // Cron tool (mirrors Go's CronTool).
    if let Some(ref cron_svc) = config.cron_service {
        tools.insert(
            "cron".to_string(),
            Box::new(CronTool::new(Arc::clone(cron_svc))),
        );
    }

    // H7 (U13 half): claude CLI delegation — opt-in (config flag, default
    // false) AND the CLI must be locatable. Absent CLI ⇒ not registered
    // (graceful degradation, one info line). The probe is cached for the
    // process lifetime (static OnceLock).
    if config.claude_code_tool_enabled {
        static CLI_PROBE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
        let cli = CLI_PROBE.get_or_init(claude_code_tool::find_claude_cli);
        match cli {
            Some(path) => {
                info!(
                    "[Tools] claude_code delegation tool registered (cli: {})",
                    path
                );
                tools.insert(
                    "claude_code".to_string(),
                    Box::new(claude_code_tool::ClaudeCodeTool::new(
                        path.clone(),
                        config.claude_code_tool_timeout_secs,
                        Some(&config.claude_code_tool_permission_mode),
                    )),
                );
            }
            None => {
                info!(
                    "[Tools] claude_code_tool enabled in config but claude CLI not found on PATH; tool not registered"
                );
            }
        }
    }

    // I4 (U13 other half): Codex CLI delegation — same opt-in + probe shape.
    if config.codex_tool_enabled {
        static CODEX_PROBE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
        let cli = CODEX_PROBE.get_or_init(codex_tool::find_codex_cli);
        match cli {
            Some(path) => {
                info!("[Tools] codex_delegate tool registered (cli: {})", path);
                tools.insert(
                    "codex_delegate".to_string(),
                    Box::new(codex_tool::CodexTool::new(
                        path.clone(),
                        config.codex_tool_timeout_secs,
                        Some(&config.codex_tool_sandbox),
                    )),
                );
            }
            None => {
                info!(
                    "[Tools] codex_tool enabled in config but codex CLI not found on PATH; tool not registered"
                );
            }
        }
    }

    // L1 (U19): read-only LSP semantic-code tool — same opt-in + probe
    // shape: config opts in AND at least one language server on PATH.
    // Absent/failed probe ⇒ tool not registered at all (no dead schema).
    if config.lsp_tool_enabled {
        static LSP_PROBE: std::sync::OnceLock<Vec<&'static str>> = std::sync::OnceLock::new();
        let available = LSP_PROBE.get_or_init(|| {
            nemesis_lsp::registry::probe_available()
                .iter()
                .map(|l| l.label())
                .collect()
        });
        if available.is_empty() {
            info!(
                "[Tools] lsp_tool enabled in config but no language server \
                 (rust-analyzer/gopls/typescript-language-server/pyright/clangd) \
                 found on PATH; tool not registered"
            );
        } else {
            info!(
                "[Tools] lsp tool registered (language servers on PATH: {})",
                available.join(", ")
            );
            // C5: external singleton manager when provided (gateway-owned,
            // shutdown_all-reachable); self-built fallback otherwise.
            let mut lsp_impl = match config.lsp_manager.clone() {
                Some(mgr) => lsp_tool::LspTool::with_manager(mgr),
                None => {
                    lsp_tool::LspTool::new(config.lsp_tool_timeout_secs, config.lsp_tool_idle_secs)
                }
            };
            // C7：rename 落盘闸宿主——与 web_fetch 的 ssrf 注入同构
            // （SharedToolConfig.security 是 J2a 起的统一注入点）。
            // clone 必需：security feature 下字段是 Option<Arc<SecurityPlugin>>；
            // 仅 no-security 编译（字段降级 Option<()>）时 clippy 会报
            // clone_on_copy——feature 组合差异，workspace 门禁（security 开）不触发。
            lsp_impl.security = config.security.clone();
            tools.insert("lsp".to_string(), Box::new(lsp_impl));
        }
    }

    // Forge tools (mirrors Go's forgeTools registration in bot_service.go).
    // Registered when forge executor is provided (i.e. forge.enabled = true).
    #[cfg(feature = "forge")]
    {
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
    }

    // MCP discovery and listing tools.
    tools.insert("mcp_discover".to_string(), Box::new(McpDiscoverTool::new()));
    tools.insert(
        "cli_reference".to_string(),
        Box::new(CliReferenceTool::new()),
    );
    // U20 (sixth batch): cross-session history full-text search.
    tools.insert(
        "history_search".to_string(),
        Box::new(HistorySearchTool::new()),
    );
    {
        let snapshot = config
            .mcp_tool_snapshot
            .clone()
            .unwrap_or_else(|| Arc::new(parking_lot::RwLock::new(Vec::new())));
        tools.insert("mcp_list".to_string(), Box::new(McpListTool::new(snapshot)));
    }

    // Workflow tool — lets the agent trigger registered workflows.
    #[cfg(feature = "workflow")]
    {
        if let Some(ref engine) = config.workflow_engine {
            tools.insert(
                "workflow_run".to_string(),
                Box::new(WorkflowRunTool::new(engine.clone())),
            );
            info!("[AgentTools] Registered workflow_run tool");
        }
    }

    info!(
        "[AgentTools] Registered {} shared tools (web={}, cluster={}, spawn={}, workflow={})",
        tools.len(),
        config.web_search.is_some(),
        config.cluster_rpc.is_some(),
        config.spawn.is_some(),
        config.workflow_engine.is_some(),
    );

    tools
}

// ===========================================================================
// WorkflowRunTool — lets the agent invoke a registered workflow by name.
// ===========================================================================

/// Agent tool that invokes a registered workflow synchronously.
///
/// Mirrors Go's `workflow_run` tool. The agent supplies a workflow name and
/// an optional input object; the tool calls `WorkflowEngine::run` and
/// returns the execution id / state / aggregated node output as JSON.
///
/// **Recursion depth**: each call increments the depth carried in
/// `TriggerSource::AgentTool`. When the new depth would exceed
/// `MAX_RECURSION_DEPTH`, the call is rejected without dispatching to the
/// engine. This prevents runaway `workflow_run → agent → workflow_run`
/// cycles from stack-allocating unbounded tokio tasks.
#[cfg(feature = "workflow")]
pub struct WorkflowRunTool {
    engine: Arc<nemesis_workflow::engine::WorkflowEngine>,
    /// Depth attributed to the call this tool is about to make. Top-level
    /// agent calls pass 0; nested calls (when the tool is itself invoked
    /// from inside a sub_workflow that was triggered by an AgentTool) read
    /// this from the enclosing execution's trigger source. The tool always
    /// increments by 1 before dispatching.
    starting_depth: u32,
}

#[cfg(feature = "workflow")]
impl WorkflowRunTool {
    pub fn new(engine: Arc<nemesis_workflow::engine::WorkflowEngine>) -> Self {
        Self {
            engine,
            starting_depth: 0,
        }
    }

    /// Construct with a non-zero starting depth. Used when the tool is
    /// invoked from within a workflow that was itself triggered by an
    /// AgentTool (so the depth chains correctly across nestings).
    pub fn with_starting_depth(
        engine: Arc<nemesis_workflow::engine::WorkflowEngine>,
        starting_depth: u32,
    ) -> Self {
        Self {
            engine,
            starting_depth,
        }
    }
}

#[cfg(feature = "workflow")]
#[async_trait]
impl Tool for WorkflowRunTool {
    fn description(&self) -> String {
        "Run a registered workflow by name. Returns the workflow's execution id, final state, and aggregated node output as JSON. Use this when the user wants to execute a multi-step predefined process (a workflow) rather than ad-hoc tool calls.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "workflow": {
                    "type": "string",
                    "description": "Name of the registered workflow to run."
                },
                "input": {
                    "type": "object",
                    "description": "Optional input variables for the workflow. Keys become workflow variables accessible to nodes.",
                    "additionalProperties": true
                }
            },
            "required": ["workflow"]
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let args_value: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("invalid JSON args: {}", e))?;

        let workflow_name = args_value
            .get("workflow")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "parameter 'workflow' (non-empty string) is required".to_string())?
            .to_string();

        let input: std::collections::HashMap<String, serde_json::Value> =
            match args_value.get("input") {
                Some(serde_json::Value::Object(map)) => {
                    map.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
                }
                Some(serde_json::Value::Null) | None => std::collections::HashMap::new(),
                Some(other) => {
                    return Err(format!(
                        "parameter 'input' must be an object, got {}",
                        other
                    ));
                }
            };

        let new_depth = self.starting_depth.saturating_add(1);
        if new_depth > nemesis_workflow::MAX_RECURSION_DEPTH {
            return Err(format!(
                "workflow_run recursion limit reached: new depth {} would exceed MAX_RECURSION_DEPTH={}",
                new_depth,
                nemesis_workflow::MAX_RECURSION_DEPTH
            ));
        }

        let trigger = nemesis_workflow::types::TriggerSource::AgentTool {
            tool_call_id: uuid::Uuid::new_v4().to_string(),
            recursion_depth: new_depth,
        };

        let execution = self
            .engine
            .run(&workflow_name, input, Some(trigger))
            .await
            .map_err(|e| format!("workflow '{}' failed to execute: {}", workflow_name, e))?;

        let mut output_map = serde_json::Map::new();
        for (node_id, nr) in &execution.node_results {
            output_map.insert(node_id.clone(), nr.output.clone());
        }
        let payload = serde_json::json!({
            "execution_id": execution.id,
            "workflow": execution.workflow_name,
            "state": format!("{:?}", execution.state),
            "started_at": execution.started_at,
            "ended_at": execution.ended_at,
            "node_results": serde_json::Value::Object(output_map),
            "error": execution.error,
        });
        Ok(serde_json::to_string(&payload)
            .map_err(|e| format!("failed to serialize workflow output: {}", e))?)
    }
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
        spawn_slot: None,
        skills_registry: None,
        skills_loader: None,
        workspace: None,
        cron_service: None,
        claude_code_tool_enabled: false,
        claude_code_tool_timeout_secs: None,
        claude_code_tool_permission_mode: String::new(),
        codex_tool_enabled: false,
        codex_tool_timeout_secs: None,
        codex_tool_sandbox: String::new(),
        lsp_tool_enabled: false,
        lsp_tool_timeout_secs: None,
        lsp_tool_idle_secs: None,
        lsp_manager: None,
        forge_executor: None,
        forge: None,
        memory_executor: None,
        mcp_tool_snapshot: None,
        workflow_engine: None,
        approval_manager: None,
        skills_manage_approval: false,
        security: None,
        workspace_boundary: None,
        todo: None,
        background_registry: None,
        question_broker: None,
    };
    register_shared_tools(&shared_config)
}

/// A1/A2 (2026-09-04): edit_file 失败修复指令 + unified diff 辅助。
pub(crate) mod edit_hint;

/// A4 (2026-09-04): edit_file 五级模糊替换级联。
pub(crate) mod edit_replacers;

pub mod claude_code_tool;
pub mod cli_delegation;
pub mod codex_tool;
pub mod lsp_tool;

#[cfg(test)]
mod coverage_boost_tests;
// D1 (2026-09-04): GitTool 写操作（add/commit/branch_create/checkout/restore/stash）
// 枚举白名单 + push/reset 不可达 + 参数走私拒绝测试。
#[cfg(test)]
mod git_tool_tests;
// J2a (2026-09-04): web_fetch 重定向循环 + SSRF 逐跳复查测试。
#[cfg(test)]
mod exec_timeout_tests;
#[cfg(test)]
mod loop_tools_extra_tests;
#[cfg(test)]
mod skill_manage_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod web_fetch_tests;

// S9 (quality-hardening goal 冲刺 S9): 独立测试文件挂载（声明式，无内联测试）。
#[cfg(test)]
mod s9_tests;
#[cfg(test)]
mod s9c_tests;
// S9 (quality-hardening goal 冲刺 S9): 独立测试文件挂载（声明式，无内联测试）。
#[cfg(test)]
mod s9d_tests;
// A5 (2026-09-04): WorkspaceBoundary 校验矩阵 + write/edit/append 带界形态
// + register_shared_tools 注入接线测试。
#[cfg(test)]
mod workspace_boundary_tests;
// H1 (2026-09-05): todowrite 全量提交语义 + 原子落盘 + TodoUpdated 广播 +
// 安全管线放行 + tier 供给测试。
#[cfg(test)]
mod todowrite_tests;

// C8 (2026-09-06): run_checks 生态探测/命令映射/统计解析/失败聚焦纯函数层 +
// 真实 cargo fixture 端到端（成功 build / 故意编译失败回灌 error 聚焦 + 存档）。
#[cfg(test)]
mod run_checks_tests;

// F7 (2026-09-06): question 工具——注册门控（有 broker 才注册）/ args 校验
// / 回灌格式（选中项 / 超时按最佳判断继续）/ 槽未装配诚实报错 / id 自增。
#[cfg(test)]
mod question_tool_tests;

// A7 (2026-09-06): multiedit 批量编辑——args 解析 / 原子性（一败全不落盘）
// / 同文件累积 / 级联与 replace_all / preview_all 多文件预检 / 边界拦截。
#[cfg(test)]
mod multiedit_tests;

// A8 (2026-09-07): read_file 二进制/PDF 分支——非 UTF-8 按 magic byte 分类
// 诚实摘要（png vision 提示 / pdf 诚实说明 / exe·无 magic 报字节数），
// 文本路径字节不变。
#[cfg(test)]
mod read_binary_tests;
