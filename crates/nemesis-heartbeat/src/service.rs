//! Heartbeat service: periodic heartbeat checks with prompt templates,
//! message bus integration, and channel routing.
//!
//! Matches Go's HeartbeatService with:
//! - `build_prompt()` - reads HEARTBEAT.md template
//! - `send_response()` - sends heartbeat result via message bus
//! - `create_default_heartbeat_template()` - creates default HEARTBEAT.md
//! - `parse_last_channel()` - parses last active channel
//! - `is_heartbeat_file_empty()` - checks if template is comments-only
//! - File-based logging to `logs/heartbeat.log`

use chrono::Local;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tokio::task::JoinHandle;

/// Heartbeat result, mirroring Go's `tools.ToolResult` branching.
#[derive(Debug, Clone)]
pub struct HeartbeatResult {
    /// Whether the result represents an error.
    pub is_error: bool,
    /// Whether the task was started asynchronously.
    pub is_async: bool,
    /// Whether the result should be silently consumed (not sent to user).
    pub silent: bool,
    /// Message for the user (displayed in chat).
    pub for_user: String,
    /// Message for the LLM context.
    pub for_llm: String,
}

/// Heartbeat callback type: called on each tick with prompt, channel, and chatID.
/// Returns `None` to skip (matching Go's nil result), or a `HeartbeatResult`.
pub type HeartbeatHandler =
    Box<dyn Fn(String, String, String) -> Option<HeartbeatResult> + Send + Sync>;

/// Internal shared form of [`HeartbeatHandler`]. BUG #17: `start()` clones
/// this into the spawned task instead of `take()`ing it, so a stop()+start()
/// cycle keeps the handler (previously the restart silently dropped it and
/// every tick after logged "handler not configured").
type SharedHeartbeatHandler =
    Arc<dyn Fn(String, String, String) -> Option<HeartbeatResult> + Send + Sync>;

/// Message bus trait for sending outbound heartbeat responses.
pub trait MessageBus: Send + Sync {
    /// Publish an outbound message to a channel.
    fn publish_outbound(&self, channel: String, chat_id: String, content: String);
}

/// State manager trait for reading last active channel.
pub trait StateManager: Send + Sync {
    /// Get the last active channel string (format: "platform:user_id").
    fn get_last_channel(&self) -> String;
}

/// Heartbeat service configuration.
#[derive(Debug, Clone)]
pub struct HeartbeatConfig {
    /// Interval between heartbeat ticks.
    pub interval: Duration,
    /// Whether the service is enabled.
    pub enabled: bool,
    /// Workspace directory path.
    pub workspace: Option<String>,
    /// Minimum interval in minutes (default: 5).
    pub min_interval_minutes: u64,
    /// Default interval in minutes when 0 is specified (default: 30).
    pub default_interval_minutes: u64,
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(30),
            enabled: true,
            workspace: None,
            min_interval_minutes: 5,
            default_interval_minutes: 30,
        }
    }
}

impl HeartbeatConfig {
    /// Create config with the given interval in minutes and workspace.
    pub fn new(interval_minutes: u64, enabled: bool, workspace: String) -> Self {
        let resolved = if interval_minutes == 0 {
            30
        } else if interval_minutes < 5 {
            5
        } else {
            interval_minutes
        };

        Self {
            interval: Duration::from_secs(resolved * 60),
            enabled,
            workspace: Some(workspace),
            min_interval_minutes: 5,
            default_interval_minutes: 30,
        }
    }
}

/// 心跳结果**不投递**的通道集合（BUG-20261025-46 更名自 INTERNAL_CHANNELS
/// 以绝误统一）：与 `nemesis_types::INTERNAL_CHANNELS`（cli/system/subagent
/// = 非用户面基础设施通道）是**刻意不同**的语义——rpc/cluster 是合法出站
/// 目标（不能进全局内部集合），cli 是用户面（心跳结果可以回 CLI），故本
/// 集合只排除心跳这类定时产物不该去的基础设施通道。
const HEARTBEAT_RESULT_EXCLUDED_CHANNELS: &[&str] = &["system", "rpc", "cluster", "internal"];

/// Heartbeat service.
pub struct HeartbeatService {
    config: HeartbeatConfig,
    running: Arc<AtomicBool>,
    last_beat: Arc<Mutex<chrono::DateTime<Local>>>,
    beat_count: Arc<AtomicU64>,
    handle: Mutex<Option<JoinHandle<()>>>,
    /// 停机令牌（2026-10-24 tokio 停机 panic 根修）：`stop()` 时 cancel，
    /// tick 循环与首拍 sleep 经 `select!` 立即退出——不依赖 abort 的取消
    /// 时序。仅靠 abort 的实测问题：gateway 停机流程完成后 runtime
    /// shutdown 会唤醒存活任务做最后 poll，poll 里注册新 timer 撞
    /// "A Tokio 1.x context was found, but it is being shutdown" panic
    /// （r9 gateway boot 测试实录）。Mutex 包装：start/stop 周期需要在
    /// stop 时重置为新 token（cancelled 的 token 无法复用）。
    stop_token: Mutex<tokio_util::sync::CancellationToken>,
    handler: Mutex<Option<SharedHeartbeatHandler>>,
    skip_file: Arc<Mutex<Option<String>>>,
    message_bus: Mutex<Option<Arc<dyn MessageBus>>>,
    state_manager: Mutex<Option<Arc<dyn StateManager>>>,
    /// 每次有效 tick（通过 skip 检查、beat 计数自增处）触发的回调。
    /// BUG-20261025-65：供 gateway 接线 `HealthServer::record_beat`——
    /// 此前该回调面不存在，`/ready` 的 beat_count 恒 0（生产零调用）。
    on_beat: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl HeartbeatService {
    /// Create a new heartbeat service.
    pub fn new(config: HeartbeatConfig) -> Self {
        // Ensure logs directory exists if workspace is set
        if let Some(ref workspace) = config.workspace {
            let log_dir = nemesis_path::logs_dir_in_workspace(std::path::Path::new(workspace));
            let _ = std::fs::create_dir_all(&log_dir);
        }

        Self {
            config,
            running: Arc::new(AtomicBool::new(false)),
            last_beat: Arc::new(Mutex::new(Local::now())),
            beat_count: Arc::new(AtomicU64::new(0)),
            handle: Mutex::new(None),
            stop_token: Mutex::new(tokio_util::sync::CancellationToken::new()),
            handler: Mutex::new(None),
            skip_file: Arc::new(Mutex::new(None)),
            message_bus: Mutex::new(None),
            state_manager: Mutex::new(None),
            on_beat: Mutex::new(None),
        }
    }

    /// Set the message bus for delivering heartbeat results.
    pub fn set_bus(&self, bus: Arc<dyn MessageBus>) {
        *self.message_bus.lock() = Some(bus);
    }

    /// Set the state manager for reading last active channel.
    pub fn set_state_manager(&self, state: Arc<dyn StateManager>) {
        *self.state_manager.lock() = Some(state);
    }

    /// Set a custom heartbeat handler called on each tick.
    pub fn set_handler(&self, handler: HeartbeatHandler) {
        *self.handler.lock() = Some(Arc::from(handler));
    }

    /// Set the skip file path (BOOTSTRAP.md). If the file exists, heartbeat is skipped.
    pub fn set_skip_file(&self, path: String) {
        *self.skip_file.lock() = Some(path);
    }

    /// Set a callback fired at the same point the internal beat counter is
    /// incremented (after skip checks and prompt build, before the handler
    /// runs). Ticks that are skipped (BOOTSTRAP.md) or lack a prompt do NOT
    /// fire it——与 [`Self::beat_count`] 的计数口径完全一致。
    pub fn set_on_beat(&self, on_beat: Arc<dyn Fn() + Send + Sync>) {
        *self.on_beat.lock() = Some(on_beat);
    }

    /// Check if heartbeat should be skipped (bootstrap mode).
    pub fn should_skip(&self) -> bool {
        if let Some(ref path) = *self.skip_file.lock() {
            std::path::Path::new(path).exists()
        } else {
            false
        }
    }

    /// Start the heartbeat service.
    ///
    /// Mirrors Go's `runLoop()`:
    /// - First heartbeat fires after 1 second (`time.AfterFunc(time.Second, ...)`)
    /// - Subsequent heartbeats fire on the configured interval
    /// - Each tick calls the full `executeHeartbeat()` flow:
    ///   build prompt → get last channel → call handler → dispatch result
    pub async fn start(&self) -> Result<(), String> {
        if self.running.load(Ordering::SeqCst) || !self.config.enabled {
            return Ok(());
        }

        self.running.store(true, Ordering::SeqCst);
        let interval = self.config.interval;
        let running = self.running.clone();
        let last_beat = self.last_beat.clone();
        let beat_count = self.beat_count.clone();
        let skip_file = self.skip_file.clone();

        // Clone the shared handler into the task (BUG #17: was a `take()`,
        // which dropped the handler on any stop+start restart).
        let handler = self.handler.lock().clone();

        // Clone Arc references needed for execute_heartbeat logic.
        // These are needed because the spawned task cannot hold &self.
        let workspace = self.config.workspace.clone();
        let message_bus = self.message_bus.lock().clone();
        let state_manager = self.state_manager.lock().clone();
        let on_beat = self.on_beat.lock().clone();

        let handle = {
            // 本轮启停周期的停机令牌：stop() cancel → 任务在 select 点立即
            // 退出（abort 保留兜底但不再是唯一停机通路）。
            let token = tokio_util::sync::CancellationToken::new();
            *self.stop_token.lock() = token.clone();

            tokio::spawn(async move {
                // First heartbeat after 1 second delay.
                // Mirrors Go's `time.AfterFunc(time.Second, func() { hs.executeHeartbeat() })`.
                // 臂序铁律同下：cancelled 第一。
                tokio::select! {
                    _ = token.cancelled() => return,
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                }

                if running.load(Ordering::SeqCst) {
                    execute_heartbeat_tick(
                        &handler,
                        &workspace,
                        &message_bus,
                        &state_manager,
                        &skip_file,
                        &last_beat,
                        &beat_count,
                        &on_beat,
                    );
                }

                // Regular interval loop.
                // interval_at 而非 interval：tokio interval 的首个 tick
                // 立即完成，不消费的话「1s 首拍」后会紧跟一记 0 间隔的第二
                // 拍——首拍缺文件时惰性创建模板（内容含非注释行=非空），
                // 第二拍立刻拿到真 prompt 触发 handler → 嵌套
                // block_in_place+block_on 的 LLM 调用（全程 debug 级日志，
                // INFO 面零痕迹）；测试/快速重启场景下这记在飞调用撞进
                // runtime 停机窗口，阻塞池线程上的嵌套 poll 逃不过
                // cancel/abort，TimerEntry 注册 timer = 停机 panic（实录
                // 2026-10-24 r9 家族 4 场景各 1 panic）。Go 原版
                // AfterFunc(1s)+ticker 即「首拍 1s，此后每 interval 一拍」，
                // interval_at 恢复同一节奏。
                let mut ticker =
                    tokio::time::interval_at(tokio::time::Instant::now() + interval, interval);
                loop {
                    // 臂序铁律：cancelled 第一。select! 按书写顺序 poll——
                    // tick 在前的话，停机后被唤醒的任务会先 poll tick（注册
                    // timer 撞 shutdown panic），根本轮不到 cancelled 臂。
                    tokio::select! {
                        _ = token.cancelled() => break,
                        _ = ticker.tick() => {}
                    }
                    if !running.load(Ordering::SeqCst) {
                        break;
                    }

                    // Check skip file (BOOTSTRAP.md exists → skip).
                    let skip = {
                        let sf = skip_file.lock();
                        if let Some(ref path) = *sf {
                            std::path::Path::new(path).exists()
                        } else {
                            false
                        }
                    };
                    if skip {
                        continue;
                    }

                    execute_heartbeat_tick(
                        &handler,
                        &workspace,
                        &message_bus,
                        &state_manager,
                        &skip_file,
                        &last_beat,
                        &beat_count,
                        &on_beat,
                    );
                }
            })
        };

        *self.handle.lock() = Some(handle);
        Ok(())
    }

    /// Stop the heartbeat service.
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
        // 停机令牌优先（2026-10-24 根修）：先 cancel 让任务在 select 点
        // 自然退出，再 abort 兜底——纯 abort 与 runtime shutdown 竞态时
        // 任务仍可能被 shutdown 唤醒 poll 一次，注册 timer 即 panic。
        self.stop_token.lock().cancel();
        if let Some(h) = self.handle.lock().take() {
            h.abort();
        }
    }

    /// Get last heartbeat time.
    pub fn last_beat(&self) -> chrono::DateTime<Local> {
        *self.last_beat.lock()
    }

    /// Get total beat count.
    pub fn beat_count(&self) -> u64 {
        self.beat_count.load(Ordering::SeqCst)
    }

    /// Get status info.
    pub fn status(&self) -> HashMap<String, serde_json::Value> {
        let mut map = HashMap::new();
        map.insert(
            "running".to_string(),
            serde_json::json!(self.running.load(Ordering::SeqCst)),
        );
        map.insert(
            "enabled".to_string(),
            serde_json::json!(self.config.enabled),
        );
        map.insert(
            "beat_count".to_string(),
            serde_json::json!(self.beat_count.load(Ordering::SeqCst)),
        );
        map.insert(
            "last_beat".to_string(),
            serde_json::json!(self.last_beat.lock().to_rfc3339()),
        );
        map.insert(
            "interval_secs".to_string(),
            serde_json::json!(self.config.interval.as_secs()),
        );
        map
    }

    /// Is the service currently running?
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    // ----- Methods matching Go's HeartbeatService -----

    /// Build the heartbeat prompt from HEARTBEAT.md.
    ///
    /// Returns the fully formatted prompt string, or empty string if:
    /// - The file does not exist (a default template is created)
    /// - The file is empty/only comments
    pub fn build_prompt(&self) -> String {
        let workspace = match self.config.workspace {
            Some(ref w) => w,
            None => return String::new(),
        };

        let heartbeat_path = std::path::Path::new(workspace).join("HEARTBEAT.md");

        match std::fs::read(&heartbeat_path) {
            Ok(data) => {
                // Check if file is empty (only comments or blank lines)
                if self.is_heartbeat_file_empty(&data) {
                    tracing::info!(
                        "[Heartbeat] HEARTBEAT.md is empty (only comments/blank lines) - skipping LLM"
                    );
                    return String::new();
                }

                let content = String::from_utf8_lossy(&data);
                if content.is_empty() {
                    return String::new();
                }

                let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
                format!(
                    "# Heartbeat Check\n\n\
                     Current time: {}\n\n\
                     You are a proactive AI assistant. This is a scheduled heartbeat check.\n\
                     Review the following tasks and execute any necessary actions using available skills.\n\
                     If there is nothing that requires attention, respond ONLY with: HEARTBEAT_OK\n\n\
                     {}",
                    now, content
                )
            }
            Err(_) => {
                // File does not exist - create default template
                self.create_default_heartbeat_template();
                String::new()
            }
        }
    }

    /// Check if the heartbeat file contains only comments or blank lines.
    ///
    /// Returns true if all lines are empty (after trimming) or start with '#'.
    pub fn is_heartbeat_file_empty(&self, data: &[u8]) -> bool {
        let content = String::from_utf8_lossy(data);
        for line in content.lines() {
            let trimmed = line.trim();
            // If line is not empty and not a comment, file has actual content
            if !trimmed.is_empty() && !trimmed.starts_with('#') {
                return false;
            }
        }
        true // All lines are empty or comments
    }

    /// Create the default HEARTBEAT.md template file.
    pub fn create_default_heartbeat_template(&self) {
        let workspace = match self.config.workspace {
            Some(ref w) => w,
            None => return,
        };

        let heartbeat_path = std::path::Path::new(workspace).join("HEARTBEAT.md");

        let default_content = r#"# Heartbeat Check List

This file contains tasks for the heartbeat service to check periodically.

## Examples

- Check for unread messages
- Review upcoming calendar events
- Check device status (e.g., MaixCam)

## Instructions

- Execute ALL tasks listed below. Do NOT skip any task.
- For simple tasks (e.g., report current time), respond directly.
- For complex tasks that may take time, use the spawn tool to create a subagent.
- The spawn tool is async - subagent results will be sent to the user automatically.
- After spawning a subagent, CONTINUE to process remaining tasks.
- Only respond with HEARTBEAT_OK when ALL tasks are done AND nothing needs attention.

---

Add your heartbeat tasks below this line:
"#;

        if let Err(e) = std::fs::write(&heartbeat_path, default_content) {
            self.log_error(&format!("Failed to create default HEARTBEAT.md: {}", e));
        } else {
            self.log_info("Created default HEARTBEAT.md template");
        }
    }

    /// Send the heartbeat response to the last active channel via the message bus.
    pub fn send_response(&self, response: &str) {
        let bus = self.message_bus.lock();
        let bus = match bus.as_ref() {
            Some(b) => b,
            None => {
                self.log_info("No message bus configured, heartbeat result not sent");
                return;
            }
        };

        let state = self.state_manager.lock();
        let last_channel = match state.as_ref() {
            Some(s) => s.get_last_channel(),
            None => {
                self.log_info("No state manager configured, heartbeat result not sent");
                return;
            }
        };

        if last_channel.is_empty() {
            self.log_info("No last channel recorded, heartbeat result not sent");
            return;
        }

        let (platform, user_id) = self.parse_last_channel(&last_channel);

        if platform.is_empty() || user_id.is_empty() {
            return;
        }

        bus.publish_outbound(platform.clone(), user_id, response.to_string());
        self.log_info(&format!("Heartbeat result sent to {}", platform));
    }

    /// Parse the last channel string into (platform, user_id).
    ///
    /// Returns empty strings for invalid or internal channels.
    /// Expected format: "platform:user_id" (e.g., "telegram:123456").
    pub fn parse_last_channel(&self, last_channel: &str) -> (String, String) {
        if last_channel.is_empty() {
            return (String::new(), String::new());
        }

        let parts: Vec<&str> = last_channel.splitn(2, ':').collect();
        if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
            self.log_error(&format!("Invalid last channel format: {}", last_channel));
            return (String::new(), String::new());
        }

        let platform = parts[0].to_string();
        let user_id = parts[1].to_string();

        // Skip internal channels
        if HEARTBEAT_RESULT_EXCLUDED_CHANNELS.contains(&platform.as_str()) {
            self.log_info(&format!("Skipping internal channel: {}", platform));
            return (String::new(), String::new());
        }

        (platform, user_id)
    }

    /// Execute a single heartbeat check.
    ///
    /// This is the full heartbeat execution flow matching Go's `executeHeartbeat()`:
    /// 1. Build prompt from HEARTBEAT.md
    /// 2. Parse last active channel
    /// 3. Call handler with (prompt, channel, chatID)
    /// 4. Send response via message bus
    pub fn execute_heartbeat(&self) {
        if !self.config.enabled {
            return;
        }

        tracing::debug!("[Heartbeat] Executing heartbeat");

        let prompt = self.build_prompt();
        if prompt.is_empty() {
            tracing::info!("[Heartbeat] No heartbeat prompt (HEARTBEAT.md empty or missing)");
            return;
        }

        // Get last channel info for context
        let last_channel = {
            let state = self.state_manager.lock();
            match state.as_ref() {
                Some(s) => s.get_last_channel(),
                None => String::new(),
            }
        };
        let (channel, chat_id) = self.parse_last_channel(&last_channel);

        self.log_info(&format!(
            "Resolved channel: {}, chatID: {} (from lastChannel: {})",
            channel, chat_id, last_channel
        ));

        // Call handler
        let result = {
            let handler = self.handler.lock();
            if let Some(ref handler) = *handler {
                handler(prompt.clone(), channel.clone(), chat_id.clone())
            } else {
                self.log_error("Heartbeat handler not configured");
                return;
            }
        };

        // Handle result, matching Go's 5-branch ToolResult logic
        let result = match result {
            Some(r) => r,
            None => {
                self.log_info("Heartbeat handler returned nil result");
                return;
            }
        };

        if result.is_error {
            self.log_error(&format!("Heartbeat error: {}", result.for_llm));
            return;
        }

        if result.is_async {
            self.log_info(&format!("Async task started: {}", result.for_llm));
            tracing::info!(
                message = result.for_llm.as_str(),
                "[Heartbeat] Async heartbeat task started"
            );
            return;
        }

        if result.silent {
            self.log_info("Heartbeat OK - silent");
            return;
        }

        // Send result to user
        if !result.for_user.is_empty() {
            self.send_response(&result.for_user);
        } else if !result.for_llm.is_empty() {
            self.send_response(&result.for_llm);
        }

        self.log_info(&format!("Heartbeat completed: {}", result.for_llm));
    }

    // ----- Logging -----

    /// Log an informational message to the heartbeat log file.
    fn log_info(&self, msg: &str) {
        self.log("INFO", msg);
    }

    /// Log an error message to the heartbeat log file.
    fn log_error(&self, msg: &str) {
        self.log("ERROR", msg);
    }

    /// Write a log message to the heartbeat log file.
    fn log(&self, level: &str, msg: &str) {
        let workspace = match self.config.workspace {
            Some(ref w) => w,
            None => return,
        };

        let log_file = nemesis_path::logs_dir_in_workspace(std::path::Path::new(workspace))
            .join("heartbeat.log");

        if let Ok(f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_file)
        {
            use std::io::Write;
            let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
            let mut writer = std::io::BufWriter::new(f);
            let _ = writeln!(writer, "[{}] [{}] {}", timestamp, level, msg);
        }
    }
}

// ---------------------------------------------------------------------------
// Free function: execute a single heartbeat tick from the spawned task.
// Mirrors Go's `HeartbeatService.executeHeartbeat()` exactly.
// ---------------------------------------------------------------------------

/// Execute a single heartbeat tick inside the spawned task.
///
/// This is the full heartbeat flow matching Go's `executeHeartbeat()`:
/// 1. Check skip file (BOOTSTRAP.md)
/// 2. Build prompt from HEARTBEAT.md
/// 3. Get last active channel from state manager
/// 4. Call handler with (prompt, channel, chatID)
/// 5. Handle result (5 branches: error, async, silent, for_user, for_llm)
/// 6. Send response via message bus
fn execute_heartbeat_tick(
    handler: &Option<SharedHeartbeatHandler>,
    workspace: &Option<String>,
    message_bus: &Option<Arc<dyn MessageBus>>,
    state_manager: &Option<Arc<dyn StateManager>>,
    skip_file: &Arc<Mutex<Option<String>>>,
    last_beat: &Arc<Mutex<chrono::DateTime<Local>>>,
    beat_count: &Arc<AtomicU64>,
    on_beat: &Option<Arc<dyn Fn() + Send + Sync>>,
) {
    // Step 1: Check skip file.
    let skip = {
        let sf = skip_file.lock();
        if let Some(ref path) = *sf {
            std::path::Path::new(path).exists()
        } else {
            false
        }
    };
    if skip {
        tracing::debug!("[Heartbeat] Heartbeat skipped (BOOTSTRAP.md exists)");
        return;
    }

    tracing::debug!("[Heartbeat] Executing heartbeat");

    // Step 2: Build prompt from HEARTBEAT.md.
    let prompt = build_prompt_from_workspace(workspace);
    if prompt.is_empty() {
        tracing::info!("[Heartbeat] No heartbeat prompt (HEARTBEAT.md empty or missing)");
        return;
    }

    // Step 3: Get last channel from state manager.
    let (channel, chat_id) = {
        let state = state_manager.as_ref();
        match state {
            Some(s) => {
                let last = s.get_last_channel();
                parse_last_channel_static(&last)
            }
            None => (String::new(), String::new()),
        }
    };

    tracing::debug!(
        "[Heartbeat] Resolved channel: {}, chatID: {}",
        channel,
        chat_id
    );

    // Update beat tracking.
    let count = beat_count.fetch_add(1, Ordering::SeqCst) + 1;
    *last_beat.lock() = Local::now();
    tracing::debug!("[Heartbeat] Heartbeat tick #{}", count);
    // BUG-20261025-65：与内部计数同点触发外部回调（gateway 接线
    // HealthServer::record_beat）；无回调时零开销 no-op。
    if let Some(on_beat) = on_beat {
        on_beat();
    }

    // Step 4: Call handler.
    let result = match handler {
        Some(h) => h(prompt, channel, chat_id),
        None => {
            tracing::error!("[Heartbeat] Heartbeat handler not configured");
            return;
        }
    };

    // Step 5: Handle result (matching Go's 5-branch ToolResult logic).
    let result = match result {
        Some(r) => r,
        None => {
            tracing::debug!("[Heartbeat] Heartbeat handler returned nil result");
            return;
        }
    };

    if result.is_error {
        tracing::error!("[Heartbeat] Heartbeat error: {}", result.for_llm);
        return;
    }

    if result.is_async {
        tracing::info!(
            message = result.for_llm.as_str(),
            "[Heartbeat] Async heartbeat task started"
        );
        return;
    }

    if result.silent {
        tracing::debug!("[Heartbeat] Heartbeat OK - silent");
        return;
    }

    // Step 6: Send result to user via message bus.
    let response = if !result.for_user.is_empty() {
        &result.for_user
    } else {
        &result.for_llm
    };

    if response.is_empty() {
        return;
    }

    send_response_static(message_bus, state_manager, response);
}

/// Build the heartbeat prompt from HEARTBEAT.md (standalone version for spawned task).
fn build_prompt_from_workspace(workspace: &Option<String>) -> String {
    let workspace = match workspace {
        Some(w) => w,
        None => return String::new(),
    };

    let heartbeat_path = std::path::Path::new(workspace).join("HEARTBEAT.md");

    match std::fs::read(&heartbeat_path) {
        Ok(data) => {
            if is_heartbeat_file_empty_static(&data) {
                tracing::info!(
                    "[Heartbeat] HEARTBEAT.md is empty (only comments/blank lines) - skipping LLM"
                );
                return String::new();
            }

            let content = String::from_utf8_lossy(&data);
            if content.is_empty() {
                return String::new();
            }

            let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
            format!(
                "# Heartbeat Check\n\n\
                 Current time: {}\n\n\
                 You are a proactive AI assistant. This is a scheduled heartbeat check.\n\
                 Review the following tasks and execute any necessary actions using available skills.\n\
                 If there is nothing that requires attention, respond ONLY with: HEARTBEAT_OK\n\n\
                 {}",
                now, content
            )
        }
        Err(_) => {
            // File does not exist — create default template (matching Go).
            create_default_heartbeat_template_static(workspace);
            String::new()
        }
    }
}

/// Check if the heartbeat file is empty (static version).
fn is_heartbeat_file_empty_static(data: &[u8]) -> bool {
    let content = String::from_utf8_lossy(data);
    for line in content.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            return false;
        }
    }
    true
}

/// Create default HEARTBEAT.md template (static version).
fn create_default_heartbeat_template_static(workspace: &str) {
    let heartbeat_path = std::path::Path::new(workspace).join("HEARTBEAT.md");
    if heartbeat_path.exists() {
        return;
    }

    let default_content = r#"# Heartbeat Check List

This file contains tasks for the heartbeat service to check periodically.

## Examples

- Check for unread messages
- Review upcoming calendar events
- Check device status (e.g., MaixCam)

## Instructions

- Execute ALL tasks listed below. Do NOT skip any task.
- For simple tasks (e.g., report current time), respond directly.
- For complex tasks that may take time, use the spawn tool to create a subagent.
- The spawn tool is async - subagent results will be sent to the user automatically.
- After spawning a subagent, CONTINUE to process remaining tasks.
- Only respond with HEARTBEAT_OK when ALL tasks are done AND nothing needs attention.

---

Add your heartbeat tasks below this line:
"#;

    if let Err(e) = std::fs::write(&heartbeat_path, default_content) {
        tracing::warn!("[Heartbeat] Failed to create default HEARTBEAT.md: {}", e);
    } else {
        tracing::info!("[Heartbeat] Created default HEARTBEAT.md template");
    }
}

/// Parse last channel string into (platform, user_id) — static version.
fn parse_last_channel_static(last_channel: &str) -> (String, String) {
    if last_channel.is_empty() {
        return (String::new(), String::new());
    }

    let parts: Vec<&str> = last_channel.splitn(2, ':').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        return (String::new(), String::new());
    }

    let platform = parts[0].to_string();
    let user_id = parts[1].to_string();

    // Skip internal channels.
    if HEARTBEAT_RESULT_EXCLUDED_CHANNELS.contains(&platform.as_str()) {
        return (String::new(), String::new());
    }

    (platform, user_id)
}

/// Send heartbeat response via message bus — static version.
fn send_response_static(
    message_bus: &Option<Arc<dyn MessageBus>>,
    state_manager: &Option<Arc<dyn StateManager>>,
    response: &str,
) {
    let bus = match message_bus {
        Some(b) => b,
        None => {
            tracing::debug!("[Heartbeat] No message bus configured, heartbeat result not sent");
            return;
        }
    };

    let last_channel = match state_manager {
        Some(s) => s.get_last_channel(),
        None => {
            tracing::debug!("[Heartbeat] No state manager configured, heartbeat result not sent");
            return;
        }
    };

    if last_channel.is_empty() {
        tracing::debug!("[Heartbeat] No last channel recorded, heartbeat result not sent");
        return;
    }

    let (platform, user_id) = parse_last_channel_static(&last_channel);
    if platform.is_empty() || user_id.is_empty() {
        return;
    }

    bus.publish_outbound(platform.clone(), user_id, response.to_string());
    tracing::info!("[Heartbeat] Heartbeat result sent to {}", platform);
}

#[cfg(test)]
mod s1_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod w1b_tests;
