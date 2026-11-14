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

    logger.log_event(
        "evt-1",
        "allowed",
        "file_read",
        "user1",
        "cli",
        "/tmp/test.txt",
        "LOW",
        "ok",
        "default",
    );
    logger.log_event(
        "evt-2",
        "denied",
        "process_exec",
        "user1",
        "cli",
        "rm -rf /",
        "CRITICAL",
        "blocked",
        "deny_all",
    );

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

    logger.log_event(
        "evt-export",
        "allowed",
        "file_read",
        "user1",
        "cli",
        "/tmp/test",
        "LOW",
        "ok",
        "default",
    );

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
    logger.log_event(
        "evt-flush",
        "allowed",
        "file_read",
        "u",
        "s",
        "t",
        "LOW",
        "ok",
        "p",
    );
    assert!(logger.flush().is_ok());
}

#[test]
fn audit_logger_new_enabled_ok_and_reuse_same_dir() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = AuditLogConfig {
        audit_log_dir: dir.path().to_path_buf(),
        enabled: true,
    };
    // First logger: file does not exist → header written.
    let mut logger = AuditLogger::new(cfg.clone()).unwrap();
    assert!(logger.flush().is_ok());
    // Second logger on the same dir: file already exists → no-header else arm.
    let mut logger2 = AuditLogger::new(cfg).unwrap();
    logger2.log_event(
        "e2",
        "allowed",
        "file_read",
        "user",
        "cli",
        "a.txt",
        "low",
        "ok",
        "rule",
    );
    assert!(logger2.flush().is_ok());
}

#[test]
fn audit_logger_new_missing_dir_errors() {
    let cfg = AuditLogConfig {
        audit_log_dir: std::path::PathBuf::new(),
        enabled: true,
    };
    let e = match AuditLogger::new(cfg) {
        Ok(_) => panic!("expected error for empty audit_log_dir"),
        Err(e) => e,
    };
    assert!(e.contains("audit log directory not configured"), "{e}");
}

#[test]
fn audit_logger_denied_event_warns_and_export_creates_parent() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = AuditLogConfig {
        audit_log_dir: dir.path().to_path_buf(),
        enabled: true,
    };
    let mut logger = AuditLogger::new(cfg).unwrap();
    logger.log_event(
        "e3",
        "denied",
        "process_exec",
        "user",
        "cli",
        "cmd.exe",
        "critical",
        "blocked",
        "policy",
    );
    assert!(logger.flush().is_ok());
    // Export to a destination whose parent directory does not exist yet.
    let dest = dir.path().join("export/nested/out.log");
    logger.export_log(&dest).unwrap();
    assert!(dest.exists());
}

/// 跨日滚动回归（BUG-20261114-020）：模块契约是「daily log files」，旧实现
/// 构造期开一次句柄长驻不滚——网关跨午夜后审计行全部落启动日文件（按日
/// 取证的文件名归档错位）。变异锁：删掉 log_event 里的滚动比对块，本测试
/// 第二段断言必红（e2 落旧日文件而非新日文件）。
#[test]
fn audit_log_rolls_over_when_handle_date_is_stale() {
    let dir = tempfile::tempdir().unwrap();
    let mut logger = AuditLogger::new(AuditLogConfig {
        audit_log_dir: dir.path().to_path_buf(),
        enabled: true,
    })
    .unwrap();
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let today_file = dir.path().join(format!("security_audit_{today}.log"));

    // 正常路径：首条事件落今日文件。
    logger.log_event(
        "e1",
        "denied",
        "process_exec",
        "user",
        "cli",
        "cmd.exe",
        "high",
        "blocked",
        "policy",
    );
    assert!(today_file.exists(), "first event must create today's file");
    assert!(
        std::fs::read_to_string(&today_file).unwrap().contains("e1"),
        "first event must land in today's file"
    );

    // 伪造跨午夜态：把句柄重定向到旧日文件（同模块测试可直接触私有字段）。
    let old_file = dir.path().join("security_audit_2000-01-01.log");
    std::fs::write(&old_file, "# header\n").unwrap();
    logger.log_file = Some(
        std::fs::OpenOptions::new()
            .append(true)
            .open(&old_file)
            .unwrap(),
    );
    logger.log_file_path = Some(old_file.clone());

    // 跨午夜后的第二条事件必须滚动进新日文件，旧日文件不被污染。
    logger.log_event(
        "e2",
        "denied",
        "process_exec",
        "user",
        "cli",
        "reg.exe",
        "critical",
        "blocked",
        "policy",
    );
    let today_content = std::fs::read_to_string(&today_file).unwrap();
    assert!(
        today_content.contains("e1") && today_content.contains("e2"),
        "stale-handle event must roll into the new day file (got: {today_content})"
    );
    let old_content = std::fs::read_to_string(&old_file).unwrap();
    assert!(
        !old_content.contains("e2"),
        "stale day file must not receive post-midnight events (got: {old_content})"
    );
    assert!(
        logger
            .log_file_path
            .as_ref()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains(&today),
        "handle must now point at the new day file"
    );
}
