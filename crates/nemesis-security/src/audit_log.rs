//! File-persisted audit logging for the security module.
//!
//! Writes audit events to daily log files in a structured pipe-delimited format.

use chrono::Local;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use tracing;

use nemesis_types::utils;

/// Configuration for the audit log.
#[derive(Debug, Clone)]
pub struct AuditLogConfig {
    /// Directory where audit log files are stored.
    pub audit_log_dir: PathBuf,
    /// Whether file-based audit logging is enabled.
    pub enabled: bool,
}

/// File-persisted audit logger.
pub struct AuditLogger {
    config: AuditLogConfig,
    log_file: Option<File>,
    log_file_path: Option<PathBuf>,
}

impl AuditLogger {
    /// Create a new audit logger.
    pub fn new(config: AuditLogConfig) -> Result<Self, String> {
        let mut logger = Self {
            config,
            log_file: None,
            log_file_path: None,
        };
        if logger.config.enabled {
            logger.init_log_file()?;
        }
        Ok(logger)
    }

    /// Create a disabled audit logger (no-op).
    pub fn disabled() -> Self {
        Self {
            config: AuditLogConfig {
                audit_log_dir: PathBuf::new(),
                enabled: false,
            },
            log_file: None,
            log_file_path: None,
        }
    }

    fn init_log_file(&mut self) -> Result<(), String> {
        if self.config.audit_log_dir.as_os_str().is_empty() {
            return Err("audit log directory not configured".to_string());
        }

        std::fs::create_dir_all(&self.config.audit_log_dir)
            .map_err(|e| format!("failed to create audit log directory: {}", e))?;

        let date_str = Local::now().format("%Y-%m-%d").to_string();
        let log_file_name = format!("security_audit_{}.log", date_str);
        let log_path = self.config.audit_log_dir.join(&log_file_name);

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map_err(|e| format!("failed to open audit log file: {}", e))?;

        // Write header if file is empty
        let metadata = file.metadata().map_err(|e| format!("failed to stat file: {}", e))?;
        if metadata.len() == 0 {
            let header = "# NemesisBot Security Audit Log\n\
                 # Format: TIMESTAMP | EVENT_ID | DECISION | OPERATION | USER | SOURCE | TARGET | DANGER | REASON | POLICY\n\
                 # ==============================================================================================================\n";
            let mut f = file;
            f.write_all(header.as_bytes())
                .map_err(|e| format!("failed to write audit log header: {}", e))?;
            self.log_file = Some(f);
        } else {
            self.log_file = Some(file);
        }

        self.log_file_path = Some(log_path);
        Ok(())
    }

    /// Log an audit event to both the structured logger and the file.
    pub fn log_event(
        &mut self,
        event_id: &str,
        decision: &str,
        operation: &str,
        user: &str,
        source: &str,
        target: &str,
        danger_level: &str,
        reason: &str,
        policy_rule: &str,
    ) {
        // Log to tracing
        if decision == "denied" {
            tracing::warn!(
                event_id = event_id,
                decision = decision,
                operation = operation,
                user = user,
                source = source,
                target = sanitize_target(target),
                danger = danger_level,
                reason = reason,
                policy = policy_rule,
                "Security audit event"
            );
        } else {
            tracing::info!(
                event_id = event_id,
                decision = decision,
                operation = operation,
                user = user,
                source = source,
                target = sanitize_target(target),
                danger = danger_level,
                reason = reason,
                policy = policy_rule,
                "Security audit event"
            );
        }

        // Write to file if enabled
        if self.config.enabled {
            if let Some(ref mut file) = self.log_file {
                let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string();
                let log_line = format!(
                    "{} | {} | {} | {} | {} | {} | {} | {} | {} | {}\n",
                    timestamp,
                    event_id,
                    decision,
                    operation,
                    user,
                    source,
                    sanitize_target(target),
                    danger_level,
                    sanitize_reason(reason),
                    policy_rule,
                );
                let _ = file.write_all(log_line.as_bytes());
            }
        }
    }

    /// Get the current log file path.
    pub fn log_file_path(&self) -> Option<&Path> {
        self.log_file_path.as_deref()
    }

    /// Check if file logging is enabled.
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Export the audit log to another file path.
    ///
    /// Copies the current audit log file contents to the specified destination.
    /// If no log file is available, creates an empty file with a header.
    pub fn export_log(&self, destination: &Path) -> Result<(), String> {
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create export directory: {}", e))?;
        }

        if let Some(ref src_path) = self.log_file_path {
            let content = std::fs::read(src_path)
                .map_err(|e| format!("failed to read audit log file: {}", e))?;
            std::fs::write(destination, content)
                .map_err(|e| format!("failed to write exported audit log: {}", e))?;
        } else {
            // No log file available, create empty export with header
            std::fs::write(
                destination,
                "# NemesisBot Security Audit Log (empty)\n",
            )
            .map_err(|e| format!("failed to write empty export: {}", e))?;
        }

        Ok(())
    }

    /// Flush pending writes to disk.
    pub fn flush(&mut self) -> Result<(), String> {
        if let Some(ref mut file) = self.log_file {
            file.flush()
                .map_err(|e| format!("failed to flush audit log: {}", e))?;
        }
        Ok(())
    }
}

/// Sanitize a target string for log output.
fn sanitize_target(target: &str) -> String {
    let s = target
        .replace('\n', " ")
        .replace('\r', " ")
        .replace('\t', " ");
    utils::truncate(&s, 200)
}

/// Sanitize a reason string for log output.
fn sanitize_reason(reason: &str) -> String {
    let s = reason
        .replace('\n', " ")
        .replace('\r', " ")
        .replace('\t', " ");
    utils::truncate(&s, 100)
}

/// Sanitize a string for CSV output.
///
/// Escapes double quotes by doubling them, and wraps the string in quotes
/// if it contains commas, quotes, or newlines.
pub fn sanitize_csv(s: &str) -> String {
    if s.contains('"') || s.contains(',') || s.contains('\n') {
        let escaped = s.replace('"', "\"\"");
        format!("\"{}\"", escaped)
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_target_normal() {
        assert_eq!(sanitize_target("/tmp/test.txt"), "/tmp/test.txt");
    }

    #[test]
    fn test_sanitize_target_newlines() {
        assert_eq!(sanitize_target("hello\nworld\r\n"), "hello world  ");
    }

    #[test]
    fn test_sanitize_target_long() {
        let long = "a".repeat(300);
        let result = sanitize_target(&long);
        assert_eq!(result.len(), 200); // 197 + "..." = 200
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_sanitize_reason_normal() {
        assert_eq!(sanitize_reason("policy match"), "policy match");
    }

    #[test]
    fn test_sanitize_reason_long() {
        let long = "x".repeat(150);
        let result = sanitize_reason(&long);
        assert_eq!(result.len(), 100); // 97 + "..." = 100
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_audit_logger_disabled() {
        let logger = AuditLogger::disabled();
        assert!(!logger.is_enabled());
    }

    #[test]
    fn test_audit_logger_file() {
        let dir = tempfile::tempdir().unwrap();
        let config = AuditLogConfig {
            audit_log_dir: dir.path().to_path_buf(),
            enabled: true,
        };
        let mut logger = AuditLogger::new(config).unwrap();
        assert!(logger.is_enabled());

        logger.log_event("evt-1", "allowed", "file_read", "user1", "cli", "/tmp/test.txt", "LOW", "ok", "default");
        logger.log_event("evt-2", "denied", "process_exec", "user1", "cli", "rm -rf /", "CRITICAL", "blocked", "deny_all");

        let path = logger.log_file_path().unwrap();
        let content = std::fs::read_to_string(path).unwrap();
        assert!(content.contains("evt-1"));
        assert!(content.contains("evt-2"));
        assert!(content.contains("denied"));
    }

    #[test]
    fn test_sanitize_csv_normal() {
        assert_eq!(sanitize_csv("hello"), "hello");
    }

    #[test]
    fn test_sanitize_csv_with_comma() {
        assert_eq!(sanitize_csv("hello, world"), "\"hello, world\"");
    }

    #[test]
    fn test_sanitize_csv_with_quotes() {
        assert_eq!(sanitize_csv("say \"hello\""), "\"say \"\"hello\"\"\"");
    }

    #[test]
    fn test_sanitize_csv_with_newline() {
        assert_eq!(sanitize_csv("line1\nline2"), "\"line1\nline2\"");
    }

    #[test]
    fn test_sanitize_csv_combined() {
        assert_eq!(sanitize_csv("a,b\nc"), "\"a,b\nc\"");
    }

    #[test]
    fn test_export_log() {
        let dir = tempfile::tempdir().unwrap();
        let export_dir = tempfile::tempdir().unwrap();

        let config = AuditLogConfig {
            audit_log_dir: dir.path().to_path_buf(),
            enabled: true,
        };
        let mut logger = AuditLogger::new(config).unwrap();

        logger.log_event("evt-export", "allowed", "file_read", "user1", "cli", "/tmp/test", "LOW", "ok", "default");

        let export_path = export_dir.path().join("export.log");
        logger.export_log(&export_path).unwrap();

        let content = std::fs::read_to_string(&export_path).unwrap();
        assert!(content.contains("evt-export"));
    }

    #[test]
    fn test_export_log_no_source() {
        let export_dir = tempfile::tempdir().unwrap();
        let logger = AuditLogger::disabled();

        let export_path = export_dir.path().join("empty.log");
        logger.export_log(&export_path).unwrap();

        let content = std::fs::read_to_string(&export_path).unwrap();
        assert!(content.contains("empty"));
    }

    #[test]
    fn test_flush() {
        let dir = tempfile::tempdir().unwrap();
        let config = AuditLogConfig {
            audit_log_dir: dir.path().to_path_buf(),
            enabled: true,
        };
        let mut logger = AuditLogger::new(config).unwrap();
        logger.log_event("evt-flush", "allowed", "file_read", "u", "s", "t", "LOW", "ok", "p");
        assert!(logger.flush().is_ok());
    }
}
