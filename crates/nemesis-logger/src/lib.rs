//! Logging configuration and utilities.

pub mod logger;

pub use logger::{LoggerConfig, NemesisLogger, LogLevel, LogEntry, init_logger, init_default, global};
