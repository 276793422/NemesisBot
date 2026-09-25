// audit_log.rs 覆盖率补充测试（new(enabled) 初始化臂 39 / log_event 的
// denied-warn 与 allowed-info 双臂 114/127 / export_log 嵌套父目录 175 /
// flush 的 Some(file) 臂 196）。

use super::*;
use std::path::PathBuf;

fn temp_dir(tag: &str) -> PathBuf {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("nmb-al-cov-{}-{tag}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn enabled_config(dir: &std::path::Path) -> AuditLogConfig {
    AuditLogConfig {
        audit_log_dir: dir.join("logs"),
        enabled: true,
    }
}

/// new(enabled=true) → 初始化日志文件（39）；log_event 两条决策臂
/// （114 denied-warn / 127 allowed-info）落盘可见；export 到嵌套路径
/// （175）；flush（196）。
#[test]
fn audit_logger_full_flow() {
    let dir = temp_dir("flow");
    let mut logger = AuditLogger::new(enabled_config(&dir)).unwrap();

    logger.log_event(
        "ev-1",
        "denied",
        "process_exec",
        "u",
        "cli",
        "rm -rf /",
        "CRITICAL",
        "blocked",
        "cmd-guard",
    );
    logger.log_event(
        "ev-2",
        "allowed",
        "file_read",
        "u",
        "cli",
        "a.txt",
        "LOW",
        "ok",
        "abac",
    );
    logger.flush().unwrap();

    // 导出到嵌套不存在目录（175 create_dir_all 臂）并读回校验。
    let out = dir.join("export").join("nested").join("audit-export.log");
    logger.export_log(&out).unwrap();
    let content = std::fs::read_to_string(&out).unwrap();
    assert!(content.contains("ev-1"), "{content}");
    assert!(content.contains("ev-2"), "{content}");
    assert!(content.contains("rm -rf /"), "target 必须落盘: {content}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// 对照：disabled() 不落盘，export 走空文件头臂。
#[test]
fn disabled_logger_exports_header_only() {
    let dir = temp_dir("off");
    let logger = AuditLogger::disabled();
    let out = dir.join("out.log");
    logger.export_log(&out).unwrap();
    let content = std::fs::read_to_string(&out).unwrap();
    assert!(content.contains("(empty)"), "{content}");

    let _ = std::fs::remove_dir_all(&dir);
}
