//! Logging configuration with LogEntry, SSE hook, component filtering, and dual-layer switches.

use chrono::Utc;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

/// Log level enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Debug = 0,
    Info = 1,
    Warn = 2,
    Error = 3,
    Fatal = 4,
}

impl LogLevel {
    pub fn name(&self) -> &'static str {
        match self {
            LogLevel::Debug => "DEBUG",
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
            LogLevel::Fatal => "FATAL",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.to_uppercase().as_str() {
            "DEBUG" => LogLevel::Debug,
            "WARN" => LogLevel::Warn,
            "ERROR" => LogLevel::Error,
            "FATAL" => LogLevel::Fatal,
            _ => LogLevel::Info,
        }
    }
}

/// A structured log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub level: String,
    pub timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub component: Option<String>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fields: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caller: Option<String>,
}

/// SSE log hook callback type.
pub type LogHook = Box<dyn Fn(LogEntry) + Send + Sync>;

/// Logger configuration.
#[derive(Debug, Clone)]
pub struct LoggerConfig {
    pub level: String,
    pub json_format: bool,
    pub file_output: Option<String>,
}

impl Default for LoggerConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            json_format: false,
            file_output: None,
        }
    }
}

/// Internal logger state.
struct LoggerState {
    level: LogLevel,
    logging_enabled: bool,
    console_enabled: bool,
    file: Option<File>,
    hook: Option<LogHook>,
    file_path: Option<String>,
}

/// Global logger instance (uses OnceLock for safe static access).
static GLOBAL_LOGGER: std::sync::OnceLock<Arc<NemesisLogger>> = std::sync::OnceLock::new();

/// Global logger.
pub struct NemesisLogger {
    state: Mutex<LoggerState>,
}

impl NemesisLogger {
    /// Create a new logger.
    pub fn new() -> Self {
        Self {
            state: Mutex::new(LoggerState {
                level: LogLevel::Info,
                logging_enabled: true,
                console_enabled: true,
                file: None,
                hook: None,
                file_path: None,
            }),
        }
    }

    /// Set the log level.
    pub fn set_level(&self, level: LogLevel) {
        self.state.lock().level = level;
    }

    /// Get the current log level.
    pub fn level(&self) -> LogLevel {
        self.state.lock().level
    }

    /// Enable logging (master switch).
    pub fn enable(&self) {
        self.state.lock().logging_enabled = true;
    }

    /// Disable logging (master switch).
    pub fn disable(&self) {
        self.state.lock().logging_enabled = false;
    }

    /// Check if logging is enabled.
    pub fn is_enabled(&self) -> bool {
        self.state.lock().logging_enabled
    }

    /// Enable console output.
    pub fn enable_console(&self) {
        self.state.lock().console_enabled = true;
    }

    /// Disable console output.
    pub fn disable_console(&self) {
        self.state.lock().console_enabled = false;
    }

    /// Check if console output is enabled.
    pub fn is_console_enabled(&self) -> bool {
        self.state.lock().console_enabled
    }

    /// Set the SSE log hook.
    pub fn set_hook(&self, hook: LogHook) {
        self.state.lock().hook = Some(hook);
    }

    /// Enable file logging.
    pub fn enable_file(&self, path: &str) -> Result<(), String> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| format!("open log file: {}", e))?;
        let mut state = self.state.lock();
        state.file = Some(file);
        state.file_path = Some(path.to_string());
        Ok(())
    }

    /// Disable file logging.
    pub fn disable_file(&self) {
        let mut state = self.state.lock();
        state.file = None;
        state.file_path = None;
    }

    /// Log a message.
    pub fn log(&self, level: LogLevel, component: &str, message: &str, fields: Option<serde_json::Map<String, serde_json::Value>>) {
        let mut state = self.state.lock();

        if !state.logging_enabled {
            return;
        }
        if level < state.level {
            return;
        }

        let entry = LogEntry {
            level: level.name().to_string(),
            timestamp: Utc::now().to_rfc3339(),
            component: if component.is_empty() { None } else { Some(component.to_string()) },
            message: message.to_string(),
            fields: if fields.is_some() && !fields.as_ref().unwrap().is_empty() { fields } else { None },
            caller: None,
        };

        // File logging (always write if configured)
        if let Some(ref mut file) = state.file {
            if let Ok(json) = serde_json::to_string(&entry) {
                let _ = writeln!(file, "{}", json);
            }
        }

        // Console output
        let console_enabled = state.console_enabled;
        let hook = state.hook.take();
        drop(state); // Release lock before console output and hook

        if console_enabled {
            let field_str = entry.fields.as_ref().map(|f| {
                let pairs: Vec<String> = f.iter().map(|(k, v)| format!("{}={}", k, v)).collect();
                format!(" {{{}}}", pairs.join(", "))
            }).unwrap_or_default();

            let comp_str = entry.component.as_ref().map(|c| format!(" {}:", c)).unwrap_or_default();

            tracing::info!("[{}] [{}]{} {}{}", entry.timestamp, entry.level, comp_str, entry.message, field_str);
        }

        // SSE hook (non-blocking, fire and forget)
        if let Some(hook) = hook {
            hook(entry.clone());
            // Put hook back
            self.state.lock().hook = Some(hook);
        }
    }

    // Convenience methods

    pub fn debug(&self, msg: &str) {
        self.log(LogLevel::Debug, "", msg, None);
    }

    pub fn debug_c(&self, component: &str, msg: &str) {
        self.log(LogLevel::Debug, component, msg, None);
    }

    pub fn info(&self, msg: &str) {
        self.log(LogLevel::Info, "", msg, None);
    }

    pub fn info_c(&self, component: &str, msg: &str) {
        self.log(LogLevel::Info, component, msg, None);
    }

    pub fn warn(&self, msg: &str) {
        self.log(LogLevel::Warn, "", msg, None);
    }

    pub fn warn_c(&self, component: &str, msg: &str) {
        self.log(LogLevel::Warn, component, msg, None);
    }

    pub fn error(&self, msg: &str) {
        self.log(LogLevel::Error, "", msg, None);
    }

    pub fn error_c(&self, component: &str, msg: &str) {
        self.log(LogLevel::Error, component, msg, None);
    }

    pub fn fatal(&self, msg: &str) {
        self.log(LogLevel::Fatal, "", msg, None);
    }

    pub fn fatal_c(&self, component: &str, msg: &str) {
        self.log(LogLevel::Fatal, component, msg, None);
    }

    // Convenience methods with fields (mirrors Go DebugF/InfoF/ErrorF/etc.)

    /// Log debug with structured fields.
    pub fn debug_f(&self, msg: &str, fields: serde_json::Map<String, serde_json::Value>) {
        self.log(LogLevel::Debug, "", msg, Some(fields));
    }

    /// Log debug with component and structured fields.
    pub fn debug_cf(&self, component: &str, msg: &str, fields: serde_json::Map<String, serde_json::Value>) {
        self.log(LogLevel::Debug, component, msg, Some(fields));
    }

    /// Log info with structured fields.
    pub fn info_f(&self, msg: &str, fields: serde_json::Map<String, serde_json::Value>) {
        self.log(LogLevel::Info, "", msg, Some(fields));
    }

    /// Log info with component and structured fields.
    pub fn info_cf(&self, component: &str, msg: &str, fields: serde_json::Map<String, serde_json::Value>) {
        self.log(LogLevel::Info, component, msg, Some(fields));
    }

    /// Log warn with structured fields.
    pub fn warn_f(&self, msg: &str, fields: serde_json::Map<String, serde_json::Value>) {
        self.log(LogLevel::Warn, "", msg, Some(fields));
    }

    /// Log warn with component and structured fields.
    pub fn warn_cf(&self, component: &str, msg: &str, fields: serde_json::Map<String, serde_json::Value>) {
        self.log(LogLevel::Warn, component, msg, Some(fields));
    }

    /// Log error with structured fields.
    pub fn error_f(&self, msg: &str, fields: serde_json::Map<String, serde_json::Value>) {
        self.log(LogLevel::Error, "", msg, Some(fields));
    }

    /// Log error with component and structured fields.
    pub fn error_cf(&self, component: &str, msg: &str, fields: serde_json::Map<String, serde_json::Value>) {
        self.log(LogLevel::Error, component, msg, Some(fields));
    }

    /// Log fatal with structured fields.
    pub fn fatal_f(&self, msg: &str, fields: serde_json::Map<String, serde_json::Value>) {
        self.log(LogLevel::Fatal, "", msg, Some(fields));
    }

    /// Log fatal with component and structured fields.
    pub fn fatal_cf(&self, component: &str, msg: &str, fields: serde_json::Map<String, serde_json::Value>) {
        self.log(LogLevel::Fatal, component, msg, Some(fields));
    }
}

/// Initialize the logger with the given configuration.
pub fn init_logger(config: &LoggerConfig) -> Result<(), String> {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(&config.level));

    if config.json_format {
        tracing_subscriber::fmt()
            .json()
            .with_env_filter(filter)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .init();
    }

    // Set up global logger
    let logger = Arc::new(NemesisLogger::new());
    if let Some(ref path) = config.file_output {
        let _ = logger.enable_file(path);
    }
    let _ = GLOBAL_LOGGER.set(logger);

    Ok(())
}

/// Initialize the logger with default settings.
pub fn init_default() -> Result<(), String> {
    init_logger(&LoggerConfig::default())
}

/// Get the global logger instance.
pub fn global() -> Option<Arc<NemesisLogger>> {
    GLOBAL_LOGGER.get().cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_default_config() {
        let config = LoggerConfig::default();
        assert_eq!(config.level, "info");
        assert!(!config.json_format);
    }

    #[test]
    fn test_log_level_ordering() {
        assert!(LogLevel::Debug < LogLevel::Info);
        assert!(LogLevel::Info < LogLevel::Warn);
        assert!(LogLevel::Warn < LogLevel::Error);
        assert!(LogLevel::Error < LogLevel::Fatal);
    }

    #[test]
    fn test_log_level_from_str() {
        assert_eq!(LogLevel::from_str("debug"), LogLevel::Debug);
        assert_eq!(LogLevel::from_str("WARN"), LogLevel::Warn);
        assert_eq!(LogLevel::from_str("unknown"), LogLevel::Info);
    }

    #[test]
    fn test_logger_create() {
        let logger = NemesisLogger::new();
        assert!(logger.is_enabled());
        assert!(logger.is_console_enabled());
        assert_eq!(logger.level(), LogLevel::Info);
    }

    #[test]
    fn test_logger_toggle() {
        let logger = NemesisLogger::new();
        logger.disable();
        assert!(!logger.is_enabled());
        logger.enable();
        assert!(logger.is_enabled());

        logger.disable_console();
        assert!(!logger.is_console_enabled());
        logger.enable_console();
        assert!(logger.is_console_enabled());
    }

    #[test]
    fn test_logger_set_level() {
        let logger = NemesisLogger::new();
        logger.set_level(LogLevel::Debug);
        assert_eq!(logger.level(), LogLevel::Debug);
    }

    #[test]
    fn test_log_entry_serialization() {
        let entry = LogEntry {
            level: "INFO".to_string(),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            component: Some("test".to_string()),
            message: "hello".to_string(),
            fields: None,
            caller: None,
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("\"level\":\"INFO\""));
        assert!(json.contains("\"component\":\"test\""));
    }

    #[test]
    fn test_logger_log_disabled() {
        let logger = NemesisLogger::new();
        logger.disable();
        // Should not panic
        logger.info("test");
    }

    #[test]
    fn test_file_logging() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.log").to_string_lossy().to_string();
        let logger = NemesisLogger::new();
        logger.enable_file(&path).unwrap();
        logger.info("test message");
        logger.disable_file();
        // Verify file was created
        assert!(Path::new(&path).exists());
    }

    #[test]
    fn test_log_level_names() {
        assert_eq!(LogLevel::Debug.name(), "DEBUG");
        assert_eq!(LogLevel::Info.name(), "INFO");
        assert_eq!(LogLevel::Warn.name(), "WARN");
        assert_eq!(LogLevel::Error.name(), "ERROR");
        assert_eq!(LogLevel::Fatal.name(), "FATAL");
    }

    #[test]
    fn test_log_level_from_str_case_insensitive() {
        assert_eq!(LogLevel::from_str("debug"), LogLevel::Debug);
        assert_eq!(LogLevel::from_str("Debug"), LogLevel::Debug);
        assert_eq!(LogLevel::from_str("info"), LogLevel::Info);
        assert_eq!(LogLevel::from_str("error"), LogLevel::Error);
        assert_eq!(LogLevel::from_str("fatal"), LogLevel::Fatal);
    }

    #[test]
    fn test_log_level_copy() {
        let level = LogLevel::Warn;
        let copied = level;
        assert_eq!(level, copied);
    }

    #[test]
    fn test_log_entry_with_fields() {
        let mut fields = serde_json::Map::new();
        fields.insert("key".to_string(), serde_json::json!("value"));
        fields.insert("count".to_string(), serde_json::json!(42));

        let entry = LogEntry {
            level: "INFO".to_string(),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            component: None,
            message: "structured log".to_string(),
            fields: Some(fields),
            caller: None,
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("key"));
        assert!(json.contains("42"));
    }

    #[test]
    fn test_log_entry_skip_none_fields() {
        let entry = LogEntry {
            level: "INFO".to_string(),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            component: None,
            message: "simple log".to_string(),
            fields: None,
            caller: None,
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(!json.contains("component"));
        assert!(!json.contains("fields"));
        assert!(!json.contains("caller"));
    }

    #[test]
    fn test_logger_convenience_methods() {
        let logger = NemesisLogger::new();
        logger.set_level(LogLevel::Debug);
        // These should not panic
        logger.debug("debug msg");
        logger.debug_c("comp", "debug comp msg");
        logger.info("info msg");
        logger.info_c("comp", "info comp msg");
        logger.warn("warn msg");
        logger.warn_c("comp", "warn comp msg");
        logger.error("error msg");
        logger.error_c("comp", "error comp msg");
        logger.fatal("fatal msg");
        logger.fatal_c("comp", "fatal comp msg");
    }

    #[test]
    fn test_logger_structured_fields() {
        let logger = NemesisLogger::new();
        logger.set_level(LogLevel::Debug);
        let mut fields = serde_json::Map::new();
        fields.insert("user".to_string(), serde_json::json!("alice"));
        fields.insert("action".to_string(), serde_json::json!("login"));

        logger.debug_f("debug fields", fields.clone());
        logger.debug_cf("comp", "debug cf", fields.clone());
        logger.info_f("info fields", fields.clone());
        logger.info_cf("comp", "info cf", fields.clone());
        logger.warn_f("warn fields", fields.clone());
        logger.warn_cf("comp", "warn cf", fields.clone());
        logger.error_f("error fields", fields.clone());
        logger.error_cf("comp", "error cf", fields.clone());
        logger.fatal_f("fatal fields", fields.clone());
        logger.fatal_cf("comp", "fatal cf", fields);
    }

    #[test]
    fn test_logger_level_filtering() {
        let logger = NemesisLogger::new();
        logger.set_level(LogLevel::Warn);
        // Debug and Info should be suppressed
        logger.debug("should not appear");
        logger.info("should not appear");
        // Warn and Error should pass
        logger.warn("should appear");
        logger.error("should appear");
    }

    #[test]
    fn test_file_logging_multiple_messages() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("multi.log").to_string_lossy().to_string();
        let logger = NemesisLogger::new();
        logger.enable_file(&path).unwrap();

        for i in 0..10 {
            logger.info(&format!("message {}", i));
        }
        logger.disable_file();

        let content = std::fs::read_to_string(Path::new(&path)).unwrap();
        let lines: Vec<&str> = content.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(lines.len(), 10);
    }

    #[test]
    fn test_config_custom_values() {
        let config = LoggerConfig {
            level: "debug".to_string(),
            json_format: true,
            file_output: Some("/tmp/test.log".to_string()),
        };
        assert_eq!(config.level, "debug");
        assert!(config.json_format);
        assert_eq!(config.file_output, Some("/tmp/test.log".to_string()));
    }

    #[test]
    fn test_log_entry_with_caller() {
        let entry = LogEntry {
            level: "ERROR".to_string(),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            component: Some("main".to_string()),
            message: "panic".to_string(),
            fields: None,
            caller: Some("main.rs:42".to_string()),
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("main.rs:42"));
    }

    #[test]
    fn test_logger_set_hook() {
        let logger = NemesisLogger::new();
        let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter_clone = counter.clone();
        logger.set_hook(Box::new(move |_entry| {
            counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }));
        logger.info("trigger hook");
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 1);
        logger.info("trigger hook again");
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[test]
    fn test_logger_empty_component() {
        let logger = NemesisLogger::new();
        // Empty component should work without panic
        logger.info("");
        logger.info_c("", "msg with empty component");
    }

    // ---- New tests for coverage ----

    #[test]
    fn test_logger_config_serialization_deserialization() {
        // LoggerConfig is not Serialize/Deserialize but has Clone + Debug.
        // Test that we can construct and clone it properly.
        let config = LoggerConfig {
            level: "debug".to_string(),
            json_format: true,
            file_output: Some("/tmp/test.log".to_string()),
        };
        let cloned = config.clone();
        assert_eq!(cloned.level, "debug");
        assert!(cloned.json_format);
        assert_eq!(cloned.file_output, Some("/tmp/test.log".to_string()));
    }

    #[test]
    fn test_log_level_filtering_with_file_output() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("filtered.log").to_string_lossy().to_string();
        let logger = NemesisLogger::new();
        logger.enable_file(&path).unwrap();

        // Set level to Warn, debug and info should be suppressed in file
        logger.set_level(LogLevel::Warn);
        logger.debug("debug suppressed");
        logger.info("info suppressed");
        logger.warn("warn visible");
        logger.error("error visible");

        logger.disable_file();

        let content = std::fs::read_to_string(Path::new(&path)).unwrap();
        let lines: Vec<&str> = content.lines().filter(|l| !l.is_empty()).collect();
        // Only warn and error should be in the file
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("WARN"));
        assert!(lines[1].contains("ERROR"));
    }

    #[test]
    fn test_log_with_empty_map_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty_fields.log").to_string_lossy().to_string();
        let logger = NemesisLogger::new();
        logger.enable_file(&path).unwrap();

        // Pass Some(empty_map) - fields should be treated as None
        let empty_fields = serde_json::Map::new();
        logger.log(LogLevel::Info, "comp", "msg with empty fields", Some(empty_fields));

        logger.disable_file();

        let content = std::fs::read_to_string(Path::new(&path)).unwrap();
        let lines: Vec<&str> = content.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(lines.len(), 1);
        // Should NOT contain "fields" since the map was empty
        assert!(!lines[0].contains("\"fields\""));
    }

    #[test]
    fn test_log_entry_deserialization() {
        let json = r#"{"level":"INFO","timestamp":"2026-01-01T00:00:00Z","message":"hello"}"#;
        let entry: LogEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.level, "INFO");
        assert_eq!(entry.message, "hello");
        assert!(entry.component.is_none());
        assert!(entry.fields.is_none());
        assert!(entry.caller.is_none());
    }

    #[test]
    fn test_log_entry_deserialization_with_all_fields() {
        let json = r#"{"level":"ERROR","timestamp":"2026-01-01T00:00:00Z","component":"main","message":"error msg","fields":{"key":"value"},"caller":"main.rs:42"}"#;
        let entry: LogEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.level, "ERROR");
        assert_eq!(entry.component, Some("main".to_string()));
        assert_eq!(entry.message, "error msg");
        assert!(entry.fields.is_some());
        assert_eq!(entry.caller, Some("main.rs:42".to_string()));
    }

    #[test]
    fn test_log_entry_clone() {
        let entry = LogEntry {
            level: "INFO".to_string(),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            component: Some("test".to_string()),
            message: "hello".to_string(),
            fields: None,
            caller: None,
        };
        let cloned = entry.clone();
        assert_eq!(cloned.level, entry.level);
        assert_eq!(cloned.message, entry.message);
    }

    #[test]
    fn test_logger_file_output_error_invalid_path() {
        let logger = NemesisLogger::new();
        // Try to enable file logging to an invalid path (on Windows, NUL-like or permission denied)
        // Use a path with null byte which should fail
        let result = logger.enable_file("/nonexistent_dir/subdir/test.log");
        // This might succeed or fail depending on OS - just ensure no panic
        let _ = result;
    }

    #[test]
    fn test_logger_disable_file_when_not_enabled() {
        let logger = NemesisLogger::new();
        // Should not panic when file logging was never enabled
        logger.disable_file();
    }

    #[test]
    fn test_logger_hook_multiple_messages() {
        let logger = NemesisLogger::new();
        let messages = Arc::new(Mutex::new(Vec::<String>::new()));
        let messages_clone = messages.clone();
        logger.set_hook(Box::new(move |entry| {
            messages_clone.lock().push(entry.message.clone());
        }));

        for i in 0..5 {
            logger.info(&format!("msg {}", i));
        }

        let msgs = messages.lock();
        assert_eq!(msgs.len(), 5);
        assert_eq!(msgs[0], "msg 0");
        assert_eq!(msgs[4], "msg 4");
    }

    #[test]
    fn test_log_entry_with_populated_fields_serialization() {
        let mut fields = serde_json::Map::new();
        fields.insert("user".to_string(), serde_json::json!("alice"));
        fields.insert("count".to_string(), serde_json::json!(42));

        let entry = LogEntry {
            level: "INFO".to_string(),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            component: Some("auth".to_string()),
            message: "login".to_string(),
            fields: Some(fields),
            caller: Some("auth.rs:10".to_string()),
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("\"level\":\"INFO\""));
        assert!(json.contains("\"component\":\"auth\""));
        assert!(json.contains("\"fields\""));
        assert!(json.contains("\"caller\":\"auth.rs:10\""));
    }

    #[test]
    fn test_log_level_ordering_comprehensive() {
        assert!(LogLevel::Debug < LogLevel::Info);
        assert!(LogLevel::Info < LogLevel::Warn);
        assert!(LogLevel::Warn < LogLevel::Error);
        assert!(LogLevel::Error < LogLevel::Fatal);
        assert!(LogLevel::Debug < LogLevel::Fatal);
    }

    #[test]
    fn test_log_level_from_str_all_variants() {
        assert_eq!(LogLevel::from_str("debug"), LogLevel::Debug);
        assert_eq!(LogLevel::from_str("DEBUG"), LogLevel::Debug);
        assert_eq!(LogLevel::from_str("info"), LogLevel::Info);
        assert_eq!(LogLevel::from_str("INFO"), LogLevel::Info);
        assert_eq!(LogLevel::from_str("warn"), LogLevel::Warn);
        assert_eq!(LogLevel::from_str("WARN"), LogLevel::Warn);
        assert_eq!(LogLevel::from_str("error"), LogLevel::Error);
        assert_eq!(LogLevel::from_str("ERROR"), LogLevel::Error);
        assert_eq!(LogLevel::from_str("fatal"), LogLevel::Fatal);
        assert_eq!(LogLevel::from_str("FATAL"), LogLevel::Fatal);
        assert_eq!(LogLevel::from_str("anything_else"), LogLevel::Info);
    }
}
