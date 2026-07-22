//! Logging configuration and utilities.

pub mod format;
pub mod jsonl_format;
pub mod logger;
pub mod sse_layer;

pub use format::{DualMakeWriter, GoStyleFormatter};
pub use jsonl_format::JsonLinesFormatter;
pub use logger::{
    LogEntry, LogLevel, LoggerConfig, NemesisLogger, global, init_default, init_logger,
};
pub use sse_layer::{
    GlobalSseLogLayer, LogCallback, LogCallbackSlot, SseLogEvent, SseLogLayer, build_sse_log_event,
    clear_global_log_callback, global_log_callback_slot, next_seq, set_global_log_callback,
};

// Re-export tracing-appender so downstream crates (e.g. nemesisbot) can construct
// RollingFileAppender without adding tracing-appender as a direct dependency.
pub use tracing_appender::rolling::{RollingFileAppender, Rotation};
