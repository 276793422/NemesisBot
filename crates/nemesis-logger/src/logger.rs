//! Logging configuration with LogEntry, SSE hook, component filtering, and dual-layer switches.

use chrono::Local;
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
            timestamp: Local::now().to_rfc3339(),
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
mod tests;
