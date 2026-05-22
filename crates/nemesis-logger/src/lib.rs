//! Logging configuration and utilities.

pub mod format;
pub mod logger;

pub use format::{DualMakeWriter, GoStyleFormatter};
pub use logger::{global, init_default, init_logger, LogEntry, LoggerConfig, LogLevel, NemesisLogger};
