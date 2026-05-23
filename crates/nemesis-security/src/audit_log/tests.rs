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
