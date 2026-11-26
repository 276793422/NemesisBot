//! Request logger: logs LLM requests and responses to markdown files.
//!
//! `RequestLogger` writes structured markdown log files for each conversation,
//! including the user request, LLM requests/responses, local operations,
//! and the final response. Each conversation gets its own timestamped directory.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use chrono::{DateTime, Local};
use nemesis_types::utils;
use serde::{Deserialize, Serialize};
use tracing::warn;

/// Detail level for log output.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub enum DetailLevel {
    #[default]
    Full,
    Truncated,
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
#[derive(Debug, Clone, Default)]
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

/// BUG-8 写失败记账（Arc 包装——spawn_blocking fire-and-forget 闭包要求
/// 'static，记账状态必须能 clone 进闭包）。
#[derive(Default)]
struct WriteStats {
    /// 连续失败计数（成功清零）。
    consecutive_failures: AtomicU32,
    /// 累计失败总数（诊断用）。
    failures_total: AtomicU64,
    /// 上次写失败 WARN 的 unix 毫秒（限频 [`WRITE_WARN_THROTTLE_MS`]）。
    last_warn_ms: AtomicU64,
    /// 失败发生过（恢复标记：失败后首次成功 → WARN「写恢复」）。
    was_failing: AtomicBool,
    /// 在飞写盘数（含挂死的）——饱和闸用。spawn_blocking fire-and-forget
    /// 的挂死写会占住 tokio blocking 线程直到线程池耗尽（默认 512），饿死
    /// 应用其他 spawn_blocking 任务；本计数把**本 logger**的在飞写盘钳在
    /// [`MAX_IN_FLIGHT_WRITES`] 内，超限丢弃 + 记账（故障隔离：单个挂死
    /// 磁盘不得拖垮共享线程池）。
    in_flight: AtomicUsize,
}

/// 在飞写盘饱和闸（BUG-8 复核补强）：挂死写占线程不释放，本 logger 自身
/// 最多同时占 64 个 blocking 线程——远低于 tokio 默认 blocking 池上限
/// （512），饿死面收敛到「后续日志条丢弃」（记账限频 WARN 可见），不外溢
/// 到无关 spawn_blocking 任务。
const MAX_IN_FLIGHT_WRITES: usize = 64;

/// 在飞计数释放 guard（构造性保证）：Drop 时减计数——写闭包内任何一段
/// 代码 panic（unwind）计数也会被释放，杜绝「一次闭包 panic → 计数永久
/// 泄漏 → 64 条后全部丢弃」这个与 BUG-8 同族的静默死亡模式。饱和闸的
/// 提前丢弃臂不经 guard（直接 fetch_sub 回退）。
struct InFlightGuard(Arc<WriteStats>);

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.0.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
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
    /// BUG-8（2026-11-25 远程 home 取证）：写失败记账——一次写盘故障（D 盘
    /// 渐进故障现场：写半截 hang）不得静默吞掉，也不得把全局锁带死。
    write_stats: std::sync::Arc<WriteStats>,
    #[allow(dead_code)]
    start_time: DateTime<Local>,
}

/// 写失败 WARN 限频间隔（毫秒）。
const WRITE_WARN_THROTTLE_MS: u64 = 300_000;

/// 纯写盘（开文件 + 全量写）。移出锁与调用方的独立函数——同步/异步两路共用。
fn write_to_disk(path: &Path, content: &str) -> Result<(), String> {
    let mut file = fs::File::create(path).map_err(|e| format!("Failed to create file: {}", e))?;
    file.write_all(content.as_bytes())
        .map_err(|e| format!("Failed to write file: {}", e))
}

/// BUG-8 写盘结果记账：失败 → 连续计数 +1、限频 WARN；成功 → 失败后首次
/// 成功发恢复 WARN + 清零。WARN 走 tracing（BUG-2 修复后的 ResilientRollingWriter
/// 管线自身有韧性，WARN 可落盘）。
fn record_write_result(stats: &WriteStats, err: Option<&String>, path: &Path) {
    match err {
        Some(e) => {
            let n = stats.consecutive_failures.fetch_add(1, Ordering::Relaxed) + 1;
            stats.failures_total.fetch_add(1, Ordering::Relaxed);
            stats.was_failing.store(true, Ordering::Relaxed);
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            let last = stats.last_warn_ms.load(Ordering::Relaxed);
            if now_ms.saturating_sub(last) >= WRITE_WARN_THROTTLE_MS {
                stats.last_warn_ms.store(now_ms, Ordering::Relaxed);
                warn!(
                    "[RequestLogger] 日志文件写入失败（连续 {} 次，累计 {} 次）：{}（file={}）",
                    n,
                    stats.failures_total.load(Ordering::Relaxed),
                    e,
                    path.display()
                );
            }
        }
        None => {
            stats.consecutive_failures.store(0, Ordering::Relaxed);
            if stats.was_failing.swap(false, Ordering::Relaxed) {
                warn!(
                    "[RequestLogger] 日志文件写入恢复（此前累计失败 {} 次，file={}）",
                    stats.failures_total.load(Ordering::Relaxed),
                    path.display()
                );
            }
        }
    }
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
                write_stats: std::sync::Arc::new(WriteStats::default()),
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
            write_stats: std::sync::Arc::new(WriteStats::default()),
            start_time: Local::now(),
        }
    }

    /// Create a logger whose session directory carries a fixed tag suffix
    /// （压缩杂务隔离， BUG-3：sender_id="summarizer" 的事件不再伪装成
    /// 普通对话）。base_dir 派生同 [`Self::new`]（resolve_log_path）；
    /// 目录名 = `{秒.微秒}_{rand}_{tag}`——与常规目录同形（时间戳段可被
    /// dashboard 解析进时间线），`_aux` 尾缀供前端识别「杂务」分组；
    /// rand 防同秒碰撞（multipart 连发三次摘要调用是常态）。
    pub fn new_with_session_suffix(config: LoggingConfig, workspace: &Path, tag: &str) -> Self {
        if !config.enabled {
            return Self::new(config, workspace);
        }
        let base_dir = resolve_log_path(&config.log_dir, workspace);
        let name = format!(
            "{}_{:03x}_{}",
            Local::now().format("%Y-%m-%d_%H-%M-%S%.6f"),
            rand_u16(),
            tag
        );
        Self {
            config,
            base_dir,
            session_name_override: Some(name),
            session_dir: Mutex::new(None),
            file_index: AtomicI32::new(0),
            enabled: true,
            write_stats: std::sync::Arc::new(WriteStats::default()),
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
                write_stats: std::sync::Arc::new(WriteStats::default()),
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
            write_stats: std::sync::Arc::new(WriteStats::default()),
            start_time: Local::now(),
        }
    }

    /// Returns whether the logger is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Sanitize a string for use as a filename component.
    ///
    /// SAN-04/SAN-05：薄包装委托仓内单一真相源白名单消毒。原黑名单实现
    /// （`/ \: * ? " < > | \0` → `_`）对 `..` 不设防——远端 device_id 为
    /// `..` 时 `base_dir.join("..")` 逃一级目录。白名单对运行时 id 族
    /// （node id / task id，alnum+-_）映射一致，另加 80 字符上限。
    /// Used by cluster logger to handle potentially untrusted task_id values
    /// from remote peers (A side can pass arbitrary strings).
    pub fn sanitize_filename(s: &str) -> String {
        nemesis_utils::sanitize::sanitize_path_segment(s)
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
                // 微秒进目录名：同秒多次对话不再只靠 3 hex rand 区分，
                // 且与 aux 目录（new_with_session_suffix）时间线粒度一致。
                let timestamp = Local::now().format("%Y-%m-%d_%H-%M-%S%.6f").to_string();
                let suffix = format!("_{:03x}", rand_u16());
                format!("{}{}", timestamp, suffix)
            }
        };
        let session_dir = self.base_dir.join(session_dir_name);

        if let Err(e) = fs::create_dir_all(&session_dir) {
            warn!("[RequestLogger] Failed to create session directory: {}", e);
            return Ok(()); // Silent failure
        }

        *self.session_dir.lock().unwrap_or_else(|e| e.into_inner()) = Some(session_dir);
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
        // 正文包自适应反引号围栏（见 md_fence_len）：消息本身可能含 markdown
        // 标题（# 行）与任意反引号行，裸插或定长围栏都会被正文打碎
        // （dashboard read_md_section 在标题行断节 →「用户消息全文」被截）。
        // 定长 4 反引号对正文里的 ``` 永不失步，但正文贴 4+ 反引号行
        // （复制粘贴本系统日志片段回聊即触发）仍会闭栏失步——围栏长度
        // 取正文最长纯反引号行 +1，渲染器语义下永不失步。与 log_llm_request
        // 的 Messages 段同一处理逻辑（那边读端不走此路径）。
        let fence = "`".repeat(md_fence_len(&info.content));
        let content = format!(
            "# User Request\n\n\
             **Timestamp**: {}\n\
             **Channel**: {}\n\
             **Sender ID**: {}\n\
             **Chat ID**: {}\n\n\
             ## Message\n\n{fence}text\n{}\n{fence}\n",
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
            let mut body = String::new();
            for (key, value) in &info.http_headers {
                let masked_val =
                    if key.to_lowercase().contains("auth") || key.to_lowercase().contains("key") {
                        mask_api_key(value)
                    } else {
                        value.clone()
                    };
                body.push_str(&format!("{}: {}\n", key, masked_val));
            }
            // 自适应围栏（同 md_fence_len）：header 值含反引号行也不失步。
            let fence = "`".repeat(md_fence_len(&body));
            content.push_str(&format!("\n## HTTP Headers\n\n{fence}\n{body}{fence}\n"));
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
            content.push_str(&format!(
                "\n## Fallback Attempts ({} total)\n\n",
                info.fallback_attempts.len()
            ));
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
        content.push_str(&format!(
            "\n## Tools\n\n{} tools available\n",
            info.tools_count
        ));

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
                // 自适应围栏（同 md_fence_len）：会话史（系统提示/工具输出）
                // 含任意反引号行是常态，定长三反引号被正文打碎——同族横扫
                // 一并加固。
                let fence = "`".repeat(md_fence_len(&msg_preview));
                content.push_str(&format!(
                    "\n## [{}] {}\n\n{fence}text\n{}\n{fence}\n",
                    i, msg.role, msg_preview
                ));
                if let Some(ref tool_calls) = msg.tool_calls {
                    for tc in tool_calls {
                        // 与上方消息正文同一闸门：只在 truncated 档截断。
                        // full 档（默认）此前漏了这层守卫，tool_call args
                        // 被无条件砍到 200 字符——本地记录不能少。
                        let args_preview = if self.config.detail_level == DetailLevel::Truncated
                            && tc.arguments.len() > TRUNCATE_ARGS_LIMIT
                        {
                            utils::truncate(&tc.arguments, TRUNCATE_ARGS_LIMIT)
                        } else {
                            tc.arguments.clone()
                        };
                        // 自适应围栏 + 逐行加引前缀（同族横扫 2026-11-26
                        // BUG-20261126-122）：args 是任意外部内容（可含 ```
                        // 行或 `# 标题` 行），定长围栏被正文打碎、裸行还会
                        // 逃出 blockquote 伪造结构。围栏长度取正文最长纯反
                        // 引号行 +1（md_fence_len），内容行统一加 "> " 前缀
                        // 留在引用块内。
                        let fence = "`".repeat(md_fence_len(&args_preview));
                        let mut quoted = String::new();
                        for line in args_preview.lines() {
                            quoted.push_str("> ");
                            quoted.push_str(line);
                            quoted.push('\n');
                        }
                        content.push_str(&format!(
                            "\n> ToolCall: `{}` ({})\n> {fence}json\n{quoted}> {fence}\n",
                            tc.name, tc.id,
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
            format!("{}\n\n... [truncated]", &info.content[..end])
        } else {
            info.content.clone()
        };

        // 自适应围栏（同族横扫 BUG-20261126-122）：LLM 输出可含任意
        // markdown（含 `# ` 标题行与反引号行），裸插会打碎文件自身结构
        // （读端 read_md_section 在标题行断节）。与 Messages 段同一处理。
        let fence = "`".repeat(md_fence_len(&response_content));
        let mut content = format!(
            "# LLM Response\n\n\
             **Timestamp**: {}\n\
             **Round**: {}\n\
             **Duration**: {:.1}s\n\n\
             ## Response Content\n\n{fence}text\n{}\n{fence}\n\n",
            info.timestamp.to_rfc3339(),
            info.round,
            info.duration_ms as f64 / 1000.0,
            response_content,
        );

        // Tool Calls section with full details (mirrors Go's formatArguments).
        content.push_str(&format!(
            "## Tool Calls\n\n{} tool call(s)\n",
            info.tool_calls_count
        ));
        if !info.tool_calls.is_empty() {
            content.push_str("\n### Tool Call Details\n\n");
            for tc in &info.tool_calls {
                let args_preview = if self.config.detail_level == DetailLevel::Truncated
                    && tc.arguments.len() > TRUNCATE_ARGS_LIMIT
                {
                    utils::truncate(&tc.arguments, TRUNCATE_ARGS_LIMIT)
                } else {
                    tc.arguments.clone()
                };
                // 自适应围栏（同族横扫）：args 此前裸插在 **Arguments**:
                // 行后，多行 args 的任意行可伪造 **ID**/**Name** 结构行。
                let fence = "`".repeat(md_fence_len(&args_preview));
                content.push_str(&format!(
                    "- **ID**: {}\n  **Name**: {}\n  **Arguments**:\n\n{fence}json\n{}\n{fence}\n\n",
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
                info.usage.prompt_tokens, info.usage.completion_tokens, info.usage.total_tokens,
            ));
            if info.usage.cached_tokens > 0 {
                // 命中率分母 = 纯输入 + 缓存读（prompt 已按账本不变量归一为
                // 非缓存纯输入，直接除会得出 >100%）。u64 饱和/checked 算术：
                // usage 数字是服务器可控的（敌意/故障端点可报 u32 满量级），
                // u32 裸加 + 乘 100 在 43M cached tokens 即溢出——debug panic
                // 落在每次 LLM 请求都会跑的日志路径上（BUG-20261119-175；
                // 旧注「现实 token 数不可能溢出」只对善意端点成立）。
                let cache_pct = (info.usage.cached_tokens as u64)
                    .saturating_mul(100)
                    .checked_div(
                        (info.usage.prompt_tokens as u64)
                            .saturating_add(info.usage.cached_tokens as u64),
                    )
                    .unwrap_or(0);
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
    pub fn log_raw_request(
        &self,
        body: &serde_json::Value,
        timestamp: chrono::DateTime<chrono::Local>,
        round: usize,
    ) {
        if !self.enabled {
            return;
        }
        let index = self.next_index();
        let filename = format!("{}.AI.Request.raw.json", index);
        let envelope = serde_json::json!({
            "timestamp": timestamp.to_rfc3339(),
            "round": round,
            "body": body,
        });
        let content =
            serde_json::to_string_pretty(&envelope).unwrap_or_else(|_| envelope.to_string());
        let _ = self.write_file(&filename, &content);
    }

    /// Log raw LLM request using a pre-built envelope (written immediately at request time).
    pub fn log_raw_request_envelope(&self, envelope: &serde_json::Value) {
        if !self.enabled {
            return;
        }
        let index = self.next_index();
        let filename = format!("{}.AI.Request.raw.json", index);
        let content =
            serde_json::to_string_pretty(envelope).unwrap_or_else(|_| envelope.to_string());
        let _ = self.write_file(&filename, &content);
    }

    /// Log raw LLM response in JSON envelope format.
    pub fn log_raw_response(
        &self,
        body: &str,
        timestamp: chrono::DateTime<chrono::Local>,
        round: usize,
        duration_ms: u64,
    ) {
        if !self.enabled {
            return;
        }
        let index = self.next_index();
        let filename = format!("{}.AI.Response.raw.json", index);
        let body_value: serde_json::Value =
            serde_json::from_str(body).unwrap_or(serde_json::Value::String(body.to_string()));
        let envelope = serde_json::json!({
            "timestamp": timestamp.to_rfc3339(),
            "round": round,
            "duration_ms": duration_ms,
            "body": body_value,
        });
        let content =
            serde_json::to_string_pretty(&envelope).unwrap_or_else(|_| envelope.to_string());
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
                let args_preview = if self.config.detail_level == DetailLevel::Truncated
                    && op.arguments.len() > TRUNCATE_ARGS_LIMIT
                {
                    utils::truncate(&op.arguments, TRUNCATE_ARGS_LIMIT)
                } else {
                    op.arguments.clone()
                };
                // 自适应围栏（同族横扫 BUG-20261126-122）：定长 ``` 围栏
                // 被工具入参里的 ``` 行打碎，正文逃逸伪造结构。
                let fence = "`".repeat(md_fence_len(&args_preview));
                content.push_str(&format!(
                    "### Arguments\n{fence}json\n{}\n{fence}\n\n",
                    args_preview
                ));
            }

            // Result section (mirrors Go's Result field).
            if !op.result.is_empty() {
                let result_preview = if self.config.detail_level == DetailLevel::Truncated
                    && op.result.len() > TRUNCATE_RESPONSE_LIMIT
                {
                    utils::truncate(&op.result, TRUNCATE_RESPONSE_LIMIT)
                } else {
                    op.result.clone()
                };
                // 自适应围栏（同族横扫 BUG-20261126-122）：工具结果含 ```
                // 行是常态（读 markdown/代码输出），定长围栏被打碎后正文
                // 逃逸——消费方曾据全文子串 `### Error` 误判任务失败。
                let fence = "`".repeat(md_fence_len(&result_preview));
                content.push_str(&format!(
                    "### Result\n{fence}text\n{}\n{fence}\n\n",
                    result_preview
                ));
            }

            if !op.error.is_empty() {
                // 自适应围栏（同族横扫 BUG-20261126-122）：错误详情（常含
                // 工具结果摘录）此前裸插，可伪造后续结构行；围栏化后消费方
                // （logs.rs local_md_has_error_section）按结构性标题行判定。
                let fence = "`".repeat(md_fence_len(&op.error));
                content.push_str(&format!(
                    "### Error\n{fence}text\n{}\n{fence}\n\n",
                    op.error
                ));
            }

            if op.duration_ms > 0 {
                content.push_str(&format!(
                    "### Duration\n{:.3}s\n\n",
                    op.duration_ms as f64 / 1000.0
                ));
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

        // 自适应围栏（同族横扫 BUG-20261126-122）：最终回复是 LLM 输出
        // （任意 markdown），裸插打碎文件结构；头部元数据（Total Duration/
        // LLM Rounds）在正文之前，读端 extract_md_header 先命中真头不受影响。
        let fence = "`".repeat(md_fence_len(&info.content));
        let content = format!(
            "# Agent Response\n\n\
             **Timestamp**: {}\n\
             **Total Duration**: {:.1}s\n\
             **LLM Rounds**: {}\n\n\
             ## Response Content\n\n{fence}text\n{}\n{fence}\n\n\
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
    ///
    /// BUG-8（2026-11-25 远程 home 取证）：写盘**移出锁**——原实现锁内做
    /// `File::create` + `write_all`，一次写盘 hang（D 盘渐进故障现场）会
    /// 把 `session_dir` 互斥锁永久带死，此后**所有**日志调用（含只读的
    /// `session_dir()`）全部阻塞，agent loop 整体停摆。现锁内只 clone
    /// 目录路径；写盘在有 tokio runtime 的上下文经 `spawn_blocking`
    /// fire-and-forget（hang 的写只占一个 blocking 线程，不再阻塞调用方
    /// 与其他日志调用；文件名含原子递增 index，天然无并发竞争），无
    /// runtime（sync 测试 / 纯 CLI）保持同步写。失败/恢复记账统一走
    /// [`Self::record_write_result`]（Arc<[`WriteStats>]>，连续计数 +
    /// 限频 WARN + 恢复标记）；在飞写盘钳在 [`MAX_IN_FLIGHT_WRITES`] 内
    /// （饱和 = 存储层挂死堆积 → 丢弃 + 记账，不拖垮共享 blocking 池）。
    ///
    /// 关停边界（诚实声明，复核 2026-11-26）：fire-and-forget 在飞写**不
    /// 等 shutdown**——进程退出（gateway teardown `process::exit`）时在飞
    /// 写随线程死亡丢弃。可接受：健康盘上单条写 ms 级、teardown 秒级，
    /// 覆盖在飞绰绰有余；挂死盘上写本来就没有产物（丢弃是既有语义）。
    /// 审计完整性以 session_logs / 审计链等带落盘闸的通道为准，本文件是
    /// 观测投影，不承诺 crash/exit 前最后一条在场。
    fn write_file(&self, filename: &str, content: &str) -> Result<(), String> {
        let session_dir = {
            let guard = self.session_dir.lock().unwrap_or_else(|e| e.into_inner());
            guard.clone()
        };
        let Some(dir) = session_dir else {
            return Ok(());
        };
        let path = dir.join(filename);
        let content = content.to_string();
        let stats = Arc::clone(&self.write_stats);
        // 饱和闸（BUG-8 复核补强）：在飞写 ≥64 = 存储层挂死堆积，本条
        // 丢弃 + 记账（限频 WARN），不再往 blocking 池压任务——挂死写占
        // 线程不释放，无闸会拖到池耗尽饿死无关任务。
        if stats.in_flight.fetch_add(1, Ordering::SeqCst) >= MAX_IN_FLIGHT_WRITES {
            stats.in_flight.fetch_sub(1, Ordering::SeqCst);
            let err =
                format!("写盘队列饱和（在飞 ≥{MAX_IN_FLIGHT_WRITES}，疑似存储层挂死）——本条丢弃");
            record_write_result(&stats, Some(&err), &path);
            return Ok(());
        }
        match tokio::runtime::Handle::try_current() {
            Ok(h) => {
                h.spawn_blocking(move || {
                    // guard 先建：闭包内 panic（unwind）也经 Drop 释放在飞计数。
                    let _in_flight = InFlightGuard(Arc::clone(&stats));
                    let r = write_to_disk(&path, &content);
                    record_write_result(&stats, r.as_ref().err(), &path);
                });
                Ok(())
            }
            Err(_) => {
                let _in_flight = InFlightGuard(Arc::clone(&stats));
                let r = write_to_disk(&path, &content);
                record_write_result(&stats, r.as_ref().err(), &path);
                r
            }
        }
    }

    /// Returns the session directory path, if a session has been created.
    ///
    /// BUG-8：锁 poisoned 不永久死（`into_inner` 恢复）——只读口被一次
    /// panic 带死后，其余调用全挂的形态必须消除。
    pub fn session_dir(&self) -> Option<PathBuf> {
        self.session_dir
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
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

/// 包正文围栏的反引号长度（自适应，渲染器语义下永不失步）。
///
/// CommonMark：围栏被「≥开栏长度的同字符 + 行内再无其他字符（首尾空白
/// 可容忍）」的行闭合。正文里任何纯反引号行都是潜在闭栏候选，围栏必须
/// 比最长候选长一；无候选 = 4（对正文里的 ``` 永不失步——dcb0db68 基线；
/// 正文贴 4+ 反引号行——复制粘贴本系统日志片段回聊即触发——自适应加长）。
/// ````abc 形态行（反引号后带非空白）不是合法闭栏，不计入候选。
fn md_fence_len(content: &str) -> usize {
    let mut max_run = 3usize; // 无候选 → 返回 4
    for line in content.lines() {
        let t = line.trim();
        let run = t.chars().take_while(|&c| c == '`').count();
        if run == 0 {
            continue;
        }
        let rest = &t[run..];
        if rest.chars().all(|c| c == '`' || c.is_whitespace()) {
            max_run = max_run.max(run);
        }
    }
    max_run + 1
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

// ---------------------------------------------------------------------------
// request/chat-log consistency anchor (lightweight)
// ---------------------------------------------------------------------------

/// Message-role sequence extracted from one side of the comparison.
#[derive(Debug, PartialEq)]
pub struct RoleSequence {
    pub roles: Vec<String>,
    /// Boundary events in the chat log (kind, in order).
    pub boundaries: Vec<String>,
}

/// Compare a recorded LLM request's message role sequence against the
/// roles replayed from the session's chat log entries. Lightweight anchor
/// (counts + role order only — NOT byte equality; the full-fidelity check
/// comes with complete session event sourcing). Returns a warning string
/// on mismatch (caller logs it; this NEVER fails the request path).
///
/// STATUS (full review): audit tooling — consumed by tests today; the
/// production wiring point is the future replay/audit console ('s
/// follow-up). Exported deliberately for that consumer.
pub fn check_request_log_consistency(
    request_roles: &[&str],
    chat_log_roles: &[&str],
) -> Result<(), String> {
    // The request may append injected non-persisted context (snapshots,
    // nudges) as extra user/system messages, and repair may drop/insert
    // tool pairs. The INVARIANT worth catching today: every PERSISTED chat
    // log role sequence must appear as a subsequence of the request roles
    // (persistence cannot invent messages the provider never saw).
    let mut ri = 0usize;
    for cr in chat_log_roles {
        loop {
            if ri >= request_roles.len() {
                let failed_at = chat_log_roles.iter().take_while(|r| **r != *cr).count();
                return Err(format!(
                    "consistency: chat-log role '{}' at index {} not present in the recorded request roles (request has {} entries)",
                    cr,
                    failed_at,
                    request_roles.len()
                ));
            }
            if &request_roles[ri] == cr {
                ri += 1;
                break;
            }
            ri += 1;
        }
    }
    Ok(())
}

// 独立测试文件挂载（声明式，无内联测试）。
#[cfg(test)]
mod s9_tests;
// BUG-8（2026-11-25 远程 home 取证）回归：写盘移出锁 + 失败记账 + 恢复
// 自愈（同步/异步两路 + 只读口不被拖死）。
#[cfg(test)]
mod write_fail_tests;
