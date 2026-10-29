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
        let metadata = file
            .metadata()
            .map_err(|e| format!("failed to stat file: {}", e))?;
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
                "[Security] Security audit event"
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
                "[Security] Security audit event"
            );
        }

        // Write to file if enabled
        if self.config.enabled
            && let Some(ref mut file) = self.log_file
        {
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
            // 明文审计不允许静默丢行：写失败必须可见（BUG 2026-10-29 横扫
            // 角度 A）。句柄已打开时写失败多为磁盘满/IO 瞬时错误，后续行
            // 继续 best-effort，但本次丢失必须留痕。
            if let Err(e) = file.write_all(log_line.as_bytes()) {
                tracing::warn!(
                    target: "nemesis_security::audit_log",
                    event_id = %event_id,
                    error = %e,
                    "[AuditLog] 审计明文日志写入失败，本条决策记录丢失: {e}"
                );
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
            std::fs::write(destination, "# NemesisBot Security Audit Log (empty)\n")
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
    let s = target.replace(['\n', '\r', '\t'], " ");
    utils::truncate(&s, 200)
}

/// Sanitize a reason string for log output.
fn sanitize_reason(reason: &str) -> String {
    let s = reason.replace(['\n', '\r', '\t'], " ");
    utils::truncate(&s, 100)
}

/// Sanitize a string for CSV output（单一真相源：原地委托
/// `nemesis_utils::sanitize::sanitize_csv`，语义含 `\r` 触发）。
pub fn sanitize_csv(s: &str) -> String {
    nemesis_utils::sanitize::sanitize_csv(s)
}

#[cfg(test)]
mod cov_tests;
#[cfg(test)]
mod tests;
