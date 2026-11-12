use super::*;
use crate::limits::PluginLimits;
use std::sync::atomic::AtomicU32;

fn caps_with(log_cap: usize, line_max: usize) -> CallCaps {
    CallCaps {
        slug: "t".into(),
        kind: PluginKind::Tool,
        observer_frame: false,
        frame: FrameBudget {
            remaining: AtomicU32::new(4),
        },
        logs: Arc::new(PluginLogBuffer::new(log_cap, line_max)),
        limits: PluginLimits {
            log_line_max_bytes: line_max,
            ..PluginLimits::default()
        },
        config: Arc::new(InstanceConfig::default()),
        secrets: Arc::new(NoSecrets),
        workspace_root: std::env::temp_dir(),
        egress: Arc::new(crate::egress::EgressPolicy::default()),
        invoker: None,
        llm: None,
        secret_write: None,
        config_write: None,
        tokio_handle: None,
        audit: None,
        session_key: String::new(),
    }
}

struct NoSecrets;
impl SecretResolver for NoSecrets {
    fn resolve(&self, _alias: &str) -> Option<String> {
        None
    }
}

// -- 日志环形缓冲 ---------------------------------------------------------

#[test]
fn log_buffer_snapshot_empty() {
    let buf = PluginLogBuffer::new(8, 256);
    let (lines, dropped) = buf.snapshot();
    assert!(lines.is_empty());
    assert_eq!(dropped, 0);
}

#[test]
fn log_buffer_evicts_oldest_and_counts() {
    let caps = caps_with(3, 4096);
    for i in 0..5 {
        caps.log_line("info", &format!("line-{i}"));
    }
    let (lines, dropped) = caps.logs.snapshot();
    assert_eq!(dropped, 2);
    let msgs: Vec<&str> = lines.iter().map(|l| l.message.as_str()).collect();
    assert_eq!(msgs, vec!["line-2", "line-3", "line-4"]);
    assert!(lines[0].ts_ms > 0);
    assert_eq!(lines[0].level, "info");
}

// -- log_line：脱敏 → 截断 → 环形 ------------------------------------------

#[test]
fn log_line_redacts_credentials_before_truncation() {
    let caps = caps_with(8, 4096);
    // 假 AWS Key 形态（Scanner 词表命中即脱敏——形态由其词表定）。夹具
    // 必须含数字：16 字符值段全大写会命中占位符豁免（SCREAMING 判据——
    // `YOURACCESSKEY` 类教程形态合法降权不脱敏），裸大写夹具测不出
    // 词表命中臂。
    caps.log_line("info", "AKIA1A2B3C4D5E6F7G8H");
    let (lines, _) = caps.logs.snapshot();
    assert_eq!(lines.len(), 1);
    assert!(
        !lines[0].message.contains("AKIA1A2B3C4D5E6F7G8H"),
        "凭据原文必须脱敏: {}",
        lines[0].message
    );
}

#[test]
fn log_line_truncates_overlong_multibyte_safely() {
    let caps = caps_with(8, 64);
    // 全中文（每字符 3 字节）：64 字节判据命中，切点必须落在字符边界。
    let long = "页".repeat(64);
    caps.log_line("warn", &long);
    let (lines, _) = caps.logs.snapshot();
    assert_eq!(lines.len(), 1);
    let m = &lines[0].message;
    assert!(m.contains("…[截断]"), "超长截断注记: {m}");
    // 截断点前的内容必须是合法 UTF-8（String 化已保证）且无替换符。
    assert!(!m.contains('\u{FFFD}'), "不得切进多字节序列中间: {m}");
}

#[test]
fn log_line_short_message_untouched() {
    let caps = caps_with(8, 4096);
    caps.log_line("error", "普通错误文本");
    let (lines, _) = caps.logs.snapshot();
    assert_eq!(lines[0].message, "普通错误文本");
    assert_eq!(lines[0].level, "error");
}

// -- 帧预算 ----------------------------------------------------------------

#[test]
fn frame_budget_consumes_until_exhausted() {
    let budget = FrameBudget::new(2);
    assert!(budget.consume().is_ok());
    assert!(budget.consume().is_ok());
    let err = budget
        .consume()
        .err()
        .unwrap_or_else(|| panic!("exhausted"));
    assert_eq!(err, "host call budget exhausted");
    // 0 值反复消耗也只报错（饱和语义，不下探）。
    assert!(budget.consume().is_err());
}

// -- 实例配置 --------------------------------------------------------------

#[test]
fn instance_config_with_schema_json() {
    let cfg = InstanceConfig::default()
        .with_schema_json(Some(r#"{"properties":{"a":{"type":"string"},"b":{}}}"#));
    assert_eq!(cfg.declared_keys, vec!["a".to_string(), "b".to_string()]);
    // 非 JSON / 缺 properties → 保持空。
    let cfg = InstanceConfig::default().with_schema_json(Some("not-json"));
    assert!(cfg.declared_keys.is_empty());
    let cfg = InstanceConfig::default().with_schema_json(Some(r#"{"other":1}"#));
    assert!(cfg.declared_keys.is_empty());
    let cfg = InstanceConfig::default().with_schema_json(None);
    assert!(cfg.declared_keys.is_empty());
}

// -- 审计 ------------------------------------------------------------------

#[test]
fn audit_event_without_logger_is_silent() {
    let caps = caps_with(4, 4096);
    // audit None = 静默跳过，不 panic。
    caps.audit_event("plugin_secret_get", "denied", "plugin/t/x", "MEDIUM", "no");
}

#[test]
fn audit_event_with_logger_writes_record() {
    let dir = tempfile::tempdir().expect("tmp");
    let logger = nemesis_security::audit_log::AuditLogger::new(
        nemesis_security::audit_log::AuditLogConfig {
            audit_log_dir: dir.path().to_path_buf(),
            enabled: true,
        },
    )
    .expect("logger");
    let mut caps = caps_with(4, 4096);
    caps.audit = Some(Arc::new(std::sync::Mutex::new(logger)));
    caps.session_key = "sess-x".into();
    caps.audit_event("plugin_http_send", "denied", "http://x", "MEDIUM", "egress");
    // 审计文件落地（有记录 = 链路通）。
    let entries: Vec<_> = std::fs::read_dir(dir.path()).expect("dir").collect();
    assert!(!entries.is_empty(), "审计文件必须落盘");
}

// -- workspace 只读闸 --------------------------------------------------------

#[test]
fn read_workspace_rel_rejects_malformed_paths() {
    let dir = tempfile::tempdir().expect("tmp");
    // 根 canonicalize（与闸内 canonical 比较同形态，Windows \?\ 前缀对齐）。
    let root = dir.path().canonicalize().expect("canonical root");
    let reject = |rel: &str, needle: &str| {
        let err = read_workspace_rel(&root, rel, 1024).expect_err(rel);
        assert!(err.contains(needle), "{rel}: {err}");
    };
    reject("", "empty path");
    // 盘符形态的平台语义分叉：Windows 下 `C:\x` 是绝对路径（落 absolute
    // 臂）、`/x` 是盘符相对（落 cannot resolve 臂）；POSIX 下 `C:\x` 只是
    // 含反斜杠的相对文件名（走 resolve → cannot resolve），`/x` 才是绝对。
    reject(
        "C:\\abs\\path.txt",
        if cfg!(windows) {
            "absolute"
        } else {
            "cannot resolve"
        },
    );
    reject(
        "/abs/path.txt",
        if cfg!(windows) {
            "cannot resolve"
        } else {
            "absolute"
        },
    );
    reject("../escape.txt", "escapes workspace");
    reject("a/../b.txt", "escapes workspace");
    reject("a~b.txt", "malformed");
    reject("ABCDEF~1", "malformed");
    reject("ghost.txt", "cannot resolve");
    // 目录拒绝。
    std::fs::create_dir_all(root.join("sub")).expect("mkdir");
    reject("sub", "directory");
    // 超限拒绝。
    std::fs::write(root.join("big.txt"), vec![0u8; 2048]).expect("write");
    let err = read_workspace_rel(&root, "big.txt", 1024)
        .err()
        .unwrap_or_else(|| panic!("over limit"));
    assert!(err.contains("exceeds workspace-read limit"));
}

#[test]
fn read_workspace_rel_reads_file_within_root() {
    let dir = tempfile::tempdir().expect("tmp");
    // 根先行 canonicalize（与闸内 canonical 比较同形态，Windows \?\ 前缀
    // 才能对齐）。
    let root = dir.path().canonicalize().expect("canonical root");
    std::fs::create_dir_all(dir.path().join("nested")).expect("mkdir");
    std::fs::write(root.join("nested").join("ok.txt"), b"payload").expect("write");
    let bytes = read_workspace_rel(&root, "nested/ok.txt", 1024).expect("read");
    assert_eq!(bytes, b"payload");
}

// -- 错误文本 ---------------------------------------------------------------

#[test]
fn truncate_err_boundaries() {
    assert_eq!(truncate_err("short", 10), "short");
    let long = "a".repeat(20);
    assert_eq!(truncate_err(&long, 10), format!("{}…", "a".repeat(10)));
    // 多字节切点回退到字符边界。
    let multibyte = "页".repeat(10);
    let out = truncate_err(&multibyte, 10);
    assert!(out.ends_with('…'));
    assert!(!out.contains('\u{FFFD}'));
    assert_eq!(out.chars().count(), 4); // 3 字符 + 省略号
}

#[test]
fn plugin_error_text_all_buckets() {
    assert_eq!(
        plugin_error_text(&PluginError::Timeout { ms: 1234 }),
        "Tool error: plugin execution timeout after 1234ms"
    );
    assert_eq!(
        plugin_error_text(&PluginError::Busy(3)),
        "Tool error: plugin instance limit reached (3)"
    );
    let trap = plugin_error_text(&PluginError::Trap("boom".into()));
    assert!(trap.contains("plugin trapped: boom"));
    // Trap 文本超 300 截断。
    let long_trap = plugin_error_text(&PluginError::Trap("x".repeat(500)));
    assert!(long_trap.chars().count() < 500);
    let manifest = plugin_error_text(&PluginError::Manifest("bad".into()));
    assert!(manifest.contains("manifest invalid: bad"));
}

#[test]
fn unix_millis_is_positive() {
    assert!(super::unix_millis() > 0);
}
