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

use chrono::Utc;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
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
pub type HeartbeatHandler = Box<dyn Fn(String, String, String) -> Option<HeartbeatResult> + Send + Sync>;

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

/// Internal channel types that should not receive heartbeat messages.
const INTERNAL_CHANNELS: &[&str] = &["system", "rpc", "cluster", "internal"];

/// Heartbeat service.
pub struct HeartbeatService {
    config: HeartbeatConfig,
    running: Arc<AtomicBool>,
    last_beat: Arc<Mutex<chrono::DateTime<Utc>>>,
    beat_count: Arc<AtomicU64>,
    handle: Mutex<Option<JoinHandle<()>>>,
    handler: Mutex<Option<HeartbeatHandler>>,
    skip_file: Arc<Mutex<Option<String>>>,
    message_bus: Mutex<Option<Arc<dyn MessageBus>>>,
    state_manager: Mutex<Option<Arc<dyn StateManager>>>,
}

impl HeartbeatService {
    /// Create a new heartbeat service.
    pub fn new(config: HeartbeatConfig) -> Self {
        // Ensure logs directory exists if workspace is set
        if let Some(ref workspace) = config.workspace {
            let log_dir = std::path::Path::new(workspace).join("logs");
            let _ = std::fs::create_dir_all(&log_dir);
        }

        Self {
            config,
            running: Arc::new(AtomicBool::new(false)),
            last_beat: Arc::new(Mutex::new(Utc::now())),
            beat_count: Arc::new(AtomicU64::new(0)),
            handle: Mutex::new(None),
            handler: Mutex::new(None),
            skip_file: Arc::new(Mutex::new(None)),
            message_bus: Mutex::new(None),
            state_manager: Mutex::new(None),
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
        *self.handler.lock() = Some(handler);
    }

    /// Set the skip file path (BOOTSTRAP.md). If the file exists, heartbeat is skipped.
    pub fn set_skip_file(&self, path: String) {
        *self.skip_file.lock() = Some(path);
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
        let handler = self.handler.lock().take();

        let handle = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            loop {
                ticker.tick().await;
                if !running.load(Ordering::SeqCst) {
                    break;
                }

                // Check skip file
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

                let count = beat_count.fetch_add(1, Ordering::SeqCst) + 1;
                *last_beat.lock() = Utc::now();

                tracing::debug!("Heartbeat tick #{}", count);

                if let Some(ref handler) = handler {
                    // Build prompt, get channel info
                    // Note: in this simplified loop, we call with empty args
                    // since the full execute_heartbeat logic requires mutable
                    // access to self which can't be done from a closure.
                    handler(count.to_string(), String::new(), String::new());
                }
            }
        });

        *self.handle.lock() = Some(handle);
        Ok(())
    }

    /// Stop the heartbeat service.
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(h) = self.handle.lock().take() {
            h.abort();
        }
    }

    /// Get last heartbeat time.
    pub fn last_beat(&self) -> chrono::DateTime<Utc> {
        *self.last_beat.lock()
    }

    /// Get total beat count.
    pub fn beat_count(&self) -> u64 {
        self.beat_count.load(Ordering::SeqCst)
    }

    /// Get status info.
    pub fn status(&self) -> HashMap<String, serde_json::Value> {
        let mut map = HashMap::new();
        map.insert("running".to_string(), serde_json::json!(self.running.load(Ordering::SeqCst)));
        map.insert("enabled".to_string(), serde_json::json!(self.config.enabled));
        map.insert("beat_count".to_string(), serde_json::json!(self.beat_count.load(Ordering::SeqCst)));
        map.insert("last_beat".to_string(), serde_json::json!(self.last_beat.lock().to_rfc3339()));
        map.insert("interval_secs".to_string(), serde_json::json!(self.config.interval.as_secs()));
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
                    tracing::info!("HEARTBEAT.md is empty (only comments/blank lines) - skipping LLM");
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
        if INTERNAL_CHANNELS.contains(&platform.as_str()) {
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

        tracing::debug!("Executing heartbeat");

        let prompt = self.build_prompt();
        if prompt.is_empty() {
            tracing::info!("No heartbeat prompt (HEARTBEAT.md empty or missing)");
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
            tracing::info!(message = result.for_llm.as_str(), "Async heartbeat task started");
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

        let log_file = std::path::Path::new(workspace)
            .join("logs")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_heartbeat_disabled() {
        let svc = HeartbeatService::new(HeartbeatConfig { enabled: false, ..Default::default() });
        assert!(svc.start().await.is_ok());
        assert!(!svc.is_running());
    }

    #[test]
    fn test_should_skip_no_file() {
        let svc = HeartbeatService::new(HeartbeatConfig::default());
        assert!(!svc.should_skip());
    }

    #[test]
    fn test_should_skip_with_nonexistent_file() {
        let svc = HeartbeatService::new(HeartbeatConfig::default());
        svc.set_skip_file("/nonexistent/path/BOOTSTRAP.md".to_string());
        assert!(!svc.should_skip());
    }

    #[test]
    fn test_status() {
        let svc = HeartbeatService::new(HeartbeatConfig::default());
        let status = svc.status();
        assert_eq!(status["beat_count"], serde_json::json!(0));
    }

    #[tokio::test]
    async fn test_start_stop() {
        let svc = HeartbeatService::new(HeartbeatConfig {
            interval: Duration::from_millis(100),
            enabled: true,
            workspace: None,
            min_interval_minutes: 5,
            default_interval_minutes: 30,
        });
        svc.start().await.unwrap();
        assert!(svc.is_running());
        tokio::time::sleep(Duration::from_millis(250)).await;
        svc.stop();
        assert!(!svc.is_running());
        assert!(svc.beat_count() >= 1);
    }

    #[test]
    fn test_handler() {
        let svc = HeartbeatService::new(HeartbeatConfig::default());
        let called = Arc::new(AtomicU64::new(0));
        let called_clone = called.clone();
        svc.set_handler(Box::new(move |_prompt, _channel, _chat_id| {
            called_clone.fetch_add(1, Ordering::SeqCst);
            None
        }));
        // Handler is set, will be called on tick
        assert_eq!(called.load(Ordering::SeqCst), 0);
    }

    // --- Tests for new methods ---

    #[test]
    fn test_is_heartbeat_file_empty_all_comments() {
        let svc = HeartbeatService::new(HeartbeatConfig::default());
        let data = b"# Title\n## Subtitle\n\n# Another comment\n";
        assert!(svc.is_heartbeat_file_empty(data));
    }

    #[test]
    fn test_is_heartbeat_file_empty_with_content() {
        let svc = HeartbeatService::new(HeartbeatConfig::default());
        let data = b"# Title\nSome actual content here\n";
        assert!(!svc.is_heartbeat_file_empty(data));
    }

    #[test]
    fn test_is_heartbeat_file_empty_blank_only() {
        let svc = HeartbeatService::new(HeartbeatConfig::default());
        let data = b"\n\n  \n\t\n";
        assert!(svc.is_heartbeat_file_empty(data));
    }

    #[test]
    fn test_is_heartbeat_file_empty_truly_empty() {
        let svc = HeartbeatService::new(HeartbeatConfig::default());
        let data = b"";
        assert!(svc.is_heartbeat_file_empty(data));
    }

    #[test]
    fn test_parse_last_channel_valid() {
        let svc = HeartbeatService::new(HeartbeatConfig::default());
        let (platform, user_id) = svc.parse_last_channel("telegram:123456");
        assert_eq!(platform, "telegram");
        assert_eq!(user_id, "123456");
    }

    #[test]
    fn test_parse_last_channel_empty() {
        let svc = HeartbeatService::new(HeartbeatConfig::default());
        let (p, u) = svc.parse_last_channel("");
        assert!(p.is_empty());
        assert!(u.is_empty());
    }

    #[test]
    fn test_parse_last_channel_no_colon() {
        let svc = HeartbeatService::new(HeartbeatConfig::default());
        let (p, u) = svc.parse_last_channel("invalidformat");
        assert!(p.is_empty());
        assert!(u.is_empty());
    }

    #[test]
    fn test_parse_last_channel_internal() {
        let svc = HeartbeatService::new(HeartbeatConfig::default());
        let (p, u) = svc.parse_last_channel("system:123");
        assert!(p.is_empty());
        assert!(u.is_empty());
    }

    #[test]
    fn test_parse_last_channel_rpc() {
        let svc = HeartbeatService::new(HeartbeatConfig::default());
        let (p, u) = svc.parse_last_channel("rpc:abc");
        assert!(p.is_empty());
        assert!(u.is_empty());
    }

    #[test]
    fn test_create_default_heartbeat_template() {
        let dir = tempfile::tempdir().unwrap();
        let svc = HeartbeatService::new(HeartbeatConfig {
            workspace: Some(dir.path().to_string_lossy().to_string()),
            ..Default::default()
        });

        svc.create_default_heartbeat_template();

        let path = dir.path().join("HEARTBEAT.md");
        assert!(path.exists());

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("Heartbeat Check List"));
        assert!(content.contains("heartbeat tasks below this line"));
    }

    #[test]
    fn test_build_prompt_no_workspace() {
        let svc = HeartbeatService::new(HeartbeatConfig {
            workspace: None,
            ..Default::default()
        });
        let prompt = svc.build_prompt();
        assert!(prompt.is_empty());
    }

    #[test]
    fn test_build_prompt_missing_file_creates_template() {
        let dir = tempfile::tempdir().unwrap();
        let svc = HeartbeatService::new(HeartbeatConfig {
            workspace: Some(dir.path().to_string_lossy().to_string()),
            ..Default::default()
        });

        let prompt = svc.build_prompt();
        assert!(prompt.is_empty()); // Returns empty because file didn't exist

        // But default template should have been created
        let path = dir.path().join("HEARTBEAT.md");
        assert!(path.exists());
    }

    #[test]
    fn test_build_prompt_with_content() {
        let dir = tempfile::tempdir().unwrap();
        let heartbeat_path = dir.path().join("HEARTBEAT.md");
        std::fs::write(&heartbeat_path, "- Check email\n- Review calendar\n").unwrap();

        let svc = HeartbeatService::new(HeartbeatConfig {
            workspace: Some(dir.path().to_string_lossy().to_string()),
            ..Default::default()
        });

        let prompt = svc.build_prompt();
        assert!(prompt.contains("Heartbeat Check"));
        assert!(prompt.contains("Check email"));
        assert!(prompt.contains("Current time:"));
    }

    #[test]
    fn test_build_prompt_comments_only() {
        let dir = tempfile::tempdir().unwrap();
        let heartbeat_path = dir.path().join("HEARTBEAT.md");
        std::fs::write(&heartbeat_path, "# Just a comment\n## Another comment\n").unwrap();

        let svc = HeartbeatService::new(HeartbeatConfig {
            workspace: Some(dir.path().to_string_lossy().to_string()),
            ..Default::default()
        });

        let prompt = svc.build_prompt();
        assert!(prompt.is_empty()); // Only comments = empty prompt
    }

    #[test]
    fn test_config_minimum_interval() {
        let config = HeartbeatConfig::new(2, true, "/tmp/test".to_string());
        assert_eq!(config.interval.as_secs(), 5 * 60); // Clamped to 5 minutes
    }

    #[test]
    fn test_config_zero_uses_default() {
        let config = HeartbeatConfig::new(0, true, "/tmp/test".to_string());
        assert_eq!(config.interval.as_secs(), 30 * 60);
    }

    #[test]
    fn test_config_normal_value() {
        let config = HeartbeatConfig::new(15, true, "/tmp/test".to_string());
        assert_eq!(config.interval.as_secs(), 15 * 60);
    }

    struct MockBus {
        sent: Arc<Mutex<Vec<(String, String, String)>>>,
    }
    impl MockBus {
        fn new() -> (Self, Arc<Mutex<Vec<(String, String, String)>>>) {
            let sent = Arc::new(Mutex::new(Vec::new()));
            let sent_clone = sent.clone();
            (Self { sent }, sent_clone)
        }
    }
    impl MessageBus for MockBus {
        fn publish_outbound(&self, channel: String, chat_id: String, content: String) {
            self.sent.lock().push((channel, chat_id, content));
        }
    }

    struct MockState {
        last_channel: String,
    }
    impl StateManager for MockState {
        fn get_last_channel(&self) -> String {
            self.last_channel.clone()
        }
    }

    #[test]
    fn test_send_response_with_bus_and_channel() {
        let svc = HeartbeatService::new(HeartbeatConfig {
            workspace: Some("/tmp".to_string()),
            ..Default::default()
        });

        let (mock_bus, sent) = MockBus::new();
        svc.set_bus(Arc::new(mock_bus));
        svc.set_state_manager(Arc::new(MockState {
            last_channel: "telegram:123456".to_string(),
        }));

        svc.send_response("Hello from heartbeat!");

        let sent_lock = sent.lock();
        assert_eq!(sent_lock.len(), 1);
        assert_eq!(sent_lock[0].0, "telegram");
        assert_eq!(sent_lock[0].1, "123456");
        assert_eq!(sent_lock[0].2, "Hello from heartbeat!");
    }

    #[test]
    fn test_send_response_no_bus() {
        let svc = HeartbeatService::new(HeartbeatConfig {
            workspace: Some("/tmp".to_string()),
            ..Default::default()
        });
        // Should not panic
        svc.send_response("test");
    }

    #[test]
    fn test_send_response_internal_channel() {
        let svc = HeartbeatService::new(HeartbeatConfig {
            workspace: Some("/tmp".to_string()),
            ..Default::default()
        });

        let (mock_bus, sent) = MockBus::new();
        svc.set_bus(Arc::new(mock_bus));
        svc.set_state_manager(Arc::new(MockState {
            last_channel: "system:123".to_string(),
        }));

        svc.send_response("Hello");

        assert!(sent.lock().is_empty()); // Internal channel should be skipped
    }

    // ============================================================
    // Additional heartbeat tests for missing coverage
    // ============================================================

    #[test]
    fn test_config_default_values() {
        let config = HeartbeatConfig::default();
        assert!(config.enabled);
        assert_eq!(config.interval, Duration::from_secs(30));
        assert!(config.workspace.is_none());
        assert_eq!(config.min_interval_minutes, 5);
        assert_eq!(config.default_interval_minutes, 30);
    }

    #[test]
    fn test_config_new_disabled() {
        let config = HeartbeatConfig::new(10, false, "/tmp/ws".to_string());
        assert!(!config.enabled);
        assert_eq!(config.workspace, Some("/tmp/ws".to_string()));
    }

    #[test]
    fn test_heartbeat_result_fields() {
        let result = HeartbeatResult {
            is_error: false,
            is_async: false,
            silent: true,
            for_user: String::new(),
            for_llm: "OK".to_string(),
        };
        assert!(!result.is_error);
        assert!(!result.is_async);
        assert!(result.silent);
        assert!(result.for_user.is_empty());
        assert_eq!(result.for_llm, "OK");
    }

    #[test]
    fn test_heartbeat_result_debug() {
        let result = HeartbeatResult {
            is_error: true,
            is_async: false,
            silent: false,
            for_user: "err".to_string(),
            for_llm: "error msg".to_string(),
        };
        let debug = format!("{:?}", result);
        assert!(debug.contains("is_error"));
        assert!(debug.contains("error msg"));
    }

    #[test]
    fn test_should_skip_with_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let skip_path = dir.path().join("BOOTSTRAP.md");
        std::fs::write(&skip_path, "bootstrap active").unwrap();

        let svc = HeartbeatService::new(HeartbeatConfig::default());
        svc.set_skip_file(skip_path.to_string_lossy().to_string());
        assert!(svc.should_skip());
    }

    #[test]
    fn test_execute_heartbeat_disabled() {
        let svc = HeartbeatService::new(HeartbeatConfig {
            enabled: false,
            ..Default::default()
        });
        // Should not panic and should return immediately
        svc.execute_heartbeat();
    }

    #[test]
    fn test_execute_heartbeat_no_workspace() {
        let svc = HeartbeatService::new(HeartbeatConfig {
            enabled: true,
            workspace: None,
            ..Default::default()
        });
        svc.execute_heartbeat();
        // Should return early (no prompt built)
    }

    #[test]
    fn test_execute_heartbeat_no_handler() {
        let dir = tempfile::tempdir().unwrap();
        let heartbeat_path = dir.path().join("HEARTBEAT.md");
        std::fs::write(&heartbeat_path, "- Check email\n").unwrap();

        let svc = HeartbeatService::new(HeartbeatConfig {
            enabled: true,
            workspace: Some(dir.path().to_string_lossy().to_string()),
            ..Default::default()
        });
        // No handler set - should log error and return
        svc.execute_heartbeat();
    }

    #[test]
    fn test_execute_heartbeat_handler_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let heartbeat_path = dir.path().join("HEARTBEAT.md");
        std::fs::write(&heartbeat_path, "- Task\n").unwrap();

        let svc = HeartbeatService::new(HeartbeatConfig {
            enabled: true,
            workspace: Some(dir.path().to_string_lossy().to_string()),
            ..Default::default()
        });
        svc.set_handler(Box::new(|_prompt, _channel, _chat_id| None));
        svc.execute_heartbeat();
    }

    #[test]
    fn test_execute_heartbeat_handler_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let heartbeat_path = dir.path().join("HEARTBEAT.md");
        std::fs::write(&heartbeat_path, "- Task\n").unwrap();

        let svc = HeartbeatService::new(HeartbeatConfig {
            enabled: true,
            workspace: Some(dir.path().to_string_lossy().to_string()),
            ..Default::default()
        });
        svc.set_handler(Box::new(|_p, _c, _ch| {
            Some(HeartbeatResult {
                is_error: true,
                is_async: false,
                silent: false,
                for_user: String::new(),
                for_llm: "something failed".to_string(),
            })
        }));
        svc.execute_heartbeat();
    }

    #[test]
    fn test_execute_heartbeat_handler_returns_silent() {
        let dir = tempfile::tempdir().unwrap();
        let heartbeat_path = dir.path().join("HEARTBEAT.md");
        std::fs::write(&heartbeat_path, "- Task\n").unwrap();

        let svc = HeartbeatService::new(HeartbeatConfig {
            enabled: true,
            workspace: Some(dir.path().to_string_lossy().to_string()),
            ..Default::default()
        });
        svc.set_handler(Box::new(|_p, _c, _ch| {
            Some(HeartbeatResult {
                is_error: false,
                is_async: false,
                silent: true,
                for_user: String::new(),
                for_llm: "HEARTBEAT_OK".to_string(),
            })
        }));
        svc.execute_heartbeat();
    }

    #[test]
    fn test_execute_heartbeat_handler_returns_async() {
        let dir = tempfile::tempdir().unwrap();
        let heartbeat_path = dir.path().join("HEARTBEAT.md");
        std::fs::write(&heartbeat_path, "- Task\n").unwrap();

        let svc = HeartbeatService::new(HeartbeatConfig {
            enabled: true,
            workspace: Some(dir.path().to_string_lossy().to_string()),
            ..Default::default()
        });
        svc.set_handler(Box::new(|_p, _c, _ch| {
            Some(HeartbeatResult {
                is_error: false,
                is_async: true,
                silent: false,
                for_user: String::new(),
                for_llm: "spawned task-1".to_string(),
            })
        }));
        svc.execute_heartbeat();
    }

    #[test]
    fn test_execute_heartbeat_sends_for_user() {
        let dir = tempfile::tempdir().unwrap();
        let heartbeat_path = dir.path().join("HEARTBEAT.md");
        std::fs::write(&heartbeat_path, "- Task\n").unwrap();

        let svc = HeartbeatService::new(HeartbeatConfig {
            enabled: true,
            workspace: Some(dir.path().to_string_lossy().to_string()),
            ..Default::default()
        });

        let (mock_bus, sent) = MockBus::new();
        svc.set_bus(Arc::new(mock_bus));
        svc.set_state_manager(Arc::new(MockState {
            last_channel: "web:user123".to_string(),
        }));
        svc.set_handler(Box::new(|_p, _c, _ch| {
            Some(HeartbeatResult {
                is_error: false,
                is_async: false,
                silent: false,
                for_user: "Hello user!".to_string(),
                for_llm: "processed".to_string(),
            })
        }));

        svc.execute_heartbeat();
        assert_eq!(sent.lock().len(), 1);
        assert_eq!(sent.lock()[0].2, "Hello user!");
    }

    #[test]
    fn test_execute_heartbeat_sends_for_llm_when_no_for_user() {
        let dir = tempfile::tempdir().unwrap();
        let heartbeat_path = dir.path().join("HEARTBEAT.md");
        std::fs::write(&heartbeat_path, "- Task\n").unwrap();

        let svc = HeartbeatService::new(HeartbeatConfig {
            enabled: true,
            workspace: Some(dir.path().to_string_lossy().to_string()),
            ..Default::default()
        });

        let (mock_bus, sent) = MockBus::new();
        svc.set_bus(Arc::new(mock_bus));
        svc.set_state_manager(Arc::new(MockState {
            last_channel: "discord:789".to_string(),
        }));
        svc.set_handler(Box::new(|_p, _c, _ch| {
            Some(HeartbeatResult {
                is_error: false,
                is_async: false,
                silent: false,
                for_user: String::new(),
                for_llm: "LLM response content".to_string(),
            })
        }));

        svc.execute_heartbeat();
        assert_eq!(sent.lock().len(), 1);
        assert_eq!(sent.lock()[0].2, "LLM response content");
    }

    #[test]
    fn test_parse_last_channel_cluster() {
        let svc = HeartbeatService::new(HeartbeatConfig::default());
        let (p, u) = svc.parse_last_channel("cluster:node-1");
        assert!(p.is_empty());
        assert!(u.is_empty());
    }

    #[test]
    fn test_parse_last_channel_internal_keyword() {
        let svc = HeartbeatService::new(HeartbeatConfig::default());
        let (p, u) = svc.parse_last_channel("internal:test");
        assert!(p.is_empty());
        assert!(u.is_empty());
    }

    #[test]
    fn test_parse_last_channel_empty_parts() {
        let svc = HeartbeatService::new(HeartbeatConfig::default());
        let (p, u) = svc.parse_last_channel(":");
        assert!(p.is_empty());
        assert!(u.is_empty());
    }

    #[test]
    fn test_parse_last_channel_missing_user() {
        let svc = HeartbeatService::new(HeartbeatConfig::default());
        let (p, u) = svc.parse_last_channel("telegram:");
        assert!(p.is_empty());
        assert!(u.is_empty());
    }

    #[test]
    fn test_beat_count_starts_at_zero() {
        let svc = HeartbeatService::new(HeartbeatConfig::default());
        assert_eq!(svc.beat_count(), 0);
    }

    #[test]
    fn test_last_beat_is_recent() {
        let svc = HeartbeatService::new(HeartbeatConfig::default());
        let now = Utc::now();
        let diff = (now - svc.last_beat()).num_seconds().abs();
        assert!(diff < 5, "last_beat should be close to now");
    }

    #[test]
    fn test_is_running_initially_false() {
        let svc = HeartbeatService::new(HeartbeatConfig::default());
        assert!(!svc.is_running());
    }

    #[test]
    fn test_status_contains_expected_keys() {
        let svc = HeartbeatService::new(HeartbeatConfig::default());
        let status = svc.status();
        assert!(status.contains_key("running"));
        assert!(status.contains_key("enabled"));
        assert!(status.contains_key("beat_count"));
        assert!(status.contains_key("last_beat"));
        assert!(status.contains_key("interval_secs"));
    }

    #[tokio::test]
    async fn test_start_twice_no_error() {
        let svc = HeartbeatService::new(HeartbeatConfig {
            interval: Duration::from_secs(60),
            enabled: true,
            workspace: None,
            min_interval_minutes: 5,
            default_interval_minutes: 30,
        });
        svc.start().await.unwrap();
        let result = svc.start().await;
        assert!(result.is_ok()); // Second start should be no-op
        svc.stop();
    }

    #[test]
    fn test_create_default_template_no_workspace() {
        let svc = HeartbeatService::new(HeartbeatConfig {
            workspace: None,
            ..Default::default()
        });
        // Should not panic
        svc.create_default_heartbeat_template();
    }

    #[test]
    fn test_is_heartbeat_file_empty_mixed_content() {
        let svc = HeartbeatService::new(HeartbeatConfig::default());
        let data = b"# Header\n\nSome real content\n# Footer\n";
        assert!(!svc.is_heartbeat_file_empty(data));
    }

    #[test]
    fn test_is_heartbeat_file_empty_whitespace_lines() {
        let svc = HeartbeatService::new(HeartbeatConfig::default());
        let data = b"  \n  \n# Only comments and whitespace\n  ";
        assert!(svc.is_heartbeat_file_empty(data));
    }

    #[test]
    fn test_send_response_no_state_manager() {
        let svc = HeartbeatService::new(HeartbeatConfig {
            workspace: Some("/tmp".to_string()),
            ..Default::default()
        });
        let (mock_bus, _sent) = MockBus::new();
        svc.set_bus(Arc::new(mock_bus));
        // No state manager - should not panic
        svc.send_response("test");
    }

    #[test]
    fn test_send_response_empty_channel() {
        let svc = HeartbeatService::new(HeartbeatConfig {
            workspace: Some("/tmp".to_string()),
            ..Default::default()
        });
        let (mock_bus, sent) = MockBus::new();
        svc.set_bus(Arc::new(mock_bus));
        svc.set_state_manager(Arc::new(MockState {
            last_channel: String::new(),
        }));
        svc.send_response("test");
        assert!(sent.lock().is_empty());
    }

    #[test]
    fn test_heartbeat_logging_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        let svc = HeartbeatService::new(HeartbeatConfig {
            workspace: Some(dir.path().to_string_lossy().to_string()),
            ..Default::default()
        });

        // Call log_info which should create the log file
        svc.set_handler(Box::new(|_p, _c, _ch| None));
        svc.execute_heartbeat();

        // Check that logs directory exists
        let logs_dir = dir.path().join("logs");
        assert!(logs_dir.exists());
    }
}
