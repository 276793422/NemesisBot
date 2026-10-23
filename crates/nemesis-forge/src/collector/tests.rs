// Collector tests (production surface: record/record_with_args/dedup_hash/
// sanitize_args/persistence/load_from_file/clear).
//
// 聚合面（pattern_count/process_record/flush/aggregates.jsonl）、ForgePlugin、
// SessionPositions、summarize_value/truncate_str、compute_pattern_hash
// 2026-10-22 随生产代码整体移除（审计 P2-a），对应测试同步删除。

use super::*;

fn make_experience(tool: &str, input: &str, success: bool) -> Experience {
    Experience {
        id: uuid::Uuid::new_v4().to_string(),
        tool_name: tool.into(),
        input_summary: input.into(),
        output_summary: if success { "ok".into() } else { "err".into() },
        success,
        duration_ms: 100,
        timestamp: "2026-04-29T00:00:00Z".into(),
        session_key: "sess-test".into(),
    }
}

fn distinct_hashes(collector: &Collector) -> HashSet<String> {
    collector
        .experiences()
        .iter()
        .map(|c| c.dedup_hash.clone())
        .collect()
}

#[tokio::test]
async fn test_record_always_stores() {
    let collector = Collector::new(CollectorConfig::default());

    let exp1 = make_experience("file_read", "read a.txt", true);
    let exp2 = make_experience("file_read", "read b.txt", true);
    let exp3 = make_experience("file_read", "read a.txt", false);

    // record() synthesizes args {input_summary: ...}; dedup_hash uses key
    // names only, so all three share one hash — but all are stored (Go-style
    // always-store behavior; the aggregate layer was removed 2026-10-22).
    assert!(collector.record(exp1.clone()).await);
    assert!(collector.record(exp2).await);
    assert!(collector.record(exp3).await);

    assert_eq!(collector.len(), 3);
    assert_eq!(
        distinct_hashes(&collector).len(),
        1,
        "same key-shape records share one dedup hash"
    );
}

#[tokio::test]
async fn test_record_with_args_different_patterns() {
    let collector = Collector::new(CollectorConfig::default());

    // Different arg keys produce different dedup hashes
    let exp1 = make_experience("file_read", "read a.txt", true);
    let exp2 = make_experience("file_read", "read b.txt", true);

    let args1 = serde_json::json!({"path": "/tmp/a.txt"});
    let args2 = serde_json::json!({"path": "/tmp/b.txt", "mode": "read"});

    assert!(collector.record_with_args(exp1, &args1).await);
    assert!(collector.record_with_args(exp2, &args2).await);

    assert_eq!(collector.len(), 2);
    assert_eq!(
        distinct_hashes(&collector).len(),
        2,
        "different arg keys => different dedup hashes"
    );
}

#[tokio::test]
async fn test_record_with_args_same_keys_different_values() {
    let collector = Collector::new(CollectorConfig::default());

    // Same keys, different values => same dedup hash (Go's ComputePatternHash behavior)
    let exp1 = make_experience("file_read", "read a.txt", true);
    let exp2 = make_experience("file_read", "read b.txt", false);

    let args1 = serde_json::json!({"path": "/tmp/a.txt"});
    let args2 = serde_json::json!({"path": "/tmp/b.txt"});

    assert!(collector.record_with_args(exp1, &args1).await);
    assert!(collector.record_with_args(exp2, &args2).await);

    assert_eq!(collector.len(), 2);
    assert_eq!(
        distinct_hashes(&collector).len(),
        1,
        "same key names => same dedup hash"
    );
}

#[tokio::test]
async fn test_max_size_eviction() {
    let mut config = CollectorConfig::default();
    config.max_size = 2;
    let collector = Collector::new(config);

    let exp1 = make_experience("tool_a", "input1", true);
    let exp2 = make_experience("tool_b", "input2", true);
    let exp3 = make_experience("tool_c", "input3", true);

    collector.record(exp1).await;
    collector.record(exp2).await;
    collector.record(exp3).await;

    assert_eq!(collector.len(), 2);
    let exps = collector.experiences();
    // Oldest (tool_a) should have been evicted.
    assert!(exps.iter().all(|e| e.experience.tool_name != "tool_a"));
}

#[tokio::test]
async fn test_jsonl_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("experiences.jsonl");
    let mut config = CollectorConfig::default();
    config.persistence_path = path.to_string_lossy().to_string();
    let collector = Collector::new(config);

    let exp = make_experience("file_write", "write data", true);
    collector.record(exp).await;

    // File should exist and contain one line.
    let content = tokio::fs::read_to_string(&path).await.unwrap();
    assert_eq!(content.lines().count(), 1);
    assert!(content.contains("file_write"));
}

#[tokio::test]
async fn test_load_from_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("load.jsonl");

    // Write some JSONL manually.
    let exp = make_experience("tool_x", "load_test", true);
    let ce = CollectedExperience {
        experience: exp,
        dedup_hash: Collector::dedup_hash(
            "tool_x",
            &serde_json::json!({"input_summary": "load_test"}),
        ),
    };
    let line = serde_json::to_string(&ce).unwrap();
    tokio::fs::write(&path, format!("{}\n", line))
        .await
        .unwrap();

    let collector = Collector::new(CollectorConfig::default());
    collector.load_from_file(&path).await.unwrap();

    assert_eq!(collector.len(), 1);
    assert_eq!(collector.experiences()[0].experience.tool_name, "tool_x");
}

#[test]
fn test_dedup_hash_matches_go_compute_pattern_hash() {
    // Go: ComputePatternHash("read_file", map[string]interface{}{"path": "/tmp/test", "mode": "read"})
    // produces "sha256:" + hex(sha256("read_file:mode,path")[:8])
    let args = serde_json::json!({"path": "/tmp/test", "mode": "read"});
    let h1 = Collector::dedup_hash("read_file", &args);

    // Same keys, different order => same hash
    let args2 = serde_json::json!({"mode": "read", "path": "/tmp/test"});
    let h2 = Collector::dedup_hash("read_file", &args2);
    assert_eq!(h1, h2);

    // Different values but same keys => same hash (Go only hashes key names)
    let args3 = serde_json::json!({"mode": "write", "path": "/other/path"});
    let h3 = Collector::dedup_hash("read_file", &args3);
    assert_eq!(h1, h3);

    // Different tool name => different hash
    let h4 = Collector::dedup_hash("write_file", &args);
    assert_ne!(h1, h4);

    // Has sha256: prefix
    assert!(h1.starts_with("sha256:"));

    // Empty args
    let h_empty = Collector::dedup_hash("tool", &serde_json::json!({}));
    assert!(h_empty.starts_with("sha256:"));
    assert_ne!(h_empty, h1);
}

#[test]
fn test_dedup_hash_non_object_fallback() {
    // Non-object args should still produce a valid hash
    let h = Collector::dedup_hash("tool", &serde_json::json!("string"));
    assert!(h.starts_with("sha256:"));
}

#[test]
fn test_sanitize_args() {
    let args = serde_json::json!({
        "path": "/tmp/file.txt",
        "api_key": "sk-secret-key",
        "content": "hello world"
    });

    let sanitized = Collector::sanitize_args(&args, &["api_key"]);
    assert_eq!(sanitized["path"], "/tmp/file.txt");
    assert_eq!(sanitized["api_key"], "[REDACTED]");
    assert_eq!(sanitized["content"], "hello world");
}

#[test]
fn test_sanitize_args_no_fields() {
    let args = serde_json::json!({"key": "value"});
    let sanitized = Collector::sanitize_args(&args, &[]);
    assert_eq!(sanitized["key"], "value");
}

// --- Additional collector tests ---

#[tokio::test]
async fn test_record_returns_true() {
    let collector = Collector::new(CollectorConfig::default());
    let exp = make_experience("tool", "input", true);
    assert!(collector.record(exp).await);
}

#[tokio::test]
async fn test_record_many_experiences() {
    let collector = Collector::new(CollectorConfig::default());
    for i in 0..100 {
        let exp = make_experience("tool", &format!("input-{}", i), true);
        assert!(collector.record(exp).await);
    }
    assert_eq!(collector.len(), 100);
}

#[tokio::test]
async fn test_experiences_returns_snapshot() {
    let collector = Collector::new(CollectorConfig::default());
    let exp = make_experience("tool", "input", true);
    collector.record(exp).await;
    let snapshot = collector.experiences();
    assert_eq!(snapshot.len(), 1);
    // Snapshot should be a copy, further records don't change it
    let exp2 = make_experience("tool2", "input2", false);
    collector.record(exp2).await;
    assert_eq!(snapshot.len(), 1); // snapshot unchanged
}

#[tokio::test]
async fn test_load_from_nonexistent_file() {
    // 指到 tempdir 内的不存在文件——裸 `/nonexistent/...` 在 Windows 上会
    // 解析到当前盘根（C:\nonexistent\...），测试不该依赖盘根状态。
    let dir = tempfile::tempdir().unwrap();
    let collector = Collector::new(CollectorConfig::default());
    let result = collector
        .load_from_file(dir.path().join("nonexistent.jsonl"))
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_load_from_empty_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty.jsonl");
    tokio::fs::write(&path, "").await.unwrap();

    let collector = Collector::new(CollectorConfig::default());
    collector.load_from_file(&path).await.unwrap();
    assert_eq!(collector.len(), 0);
}

#[tokio::test]
async fn test_load_from_file_with_invalid_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mixed.jsonl");
    let valid_exp = make_experience("tool", "input", true);
    let ce = CollectedExperience {
        experience: valid_exp,
        dedup_hash: Collector::dedup_hash("tool", &serde_json::json!({"input_summary": "input"})),
    };
    let valid_line = serde_json::to_string(&ce).unwrap();
    let content = format!("invalid json\n{}\n", valid_line);
    tokio::fs::write(&path, content).await.unwrap();

    let collector = Collector::new(CollectorConfig::default());
    collector.load_from_file(&path).await.unwrap();
    assert_eq!(collector.len(), 1); // Only valid line loaded
}

#[tokio::test]
async fn test_record_with_args_stores_experience() {
    let collector = Collector::new(CollectorConfig::default());
    let exp = make_experience("tool", "input", true);
    let args = serde_json::json!({"key": "value"});
    assert!(collector.record_with_args(exp, &args).await);
    assert_eq!(collector.len(), 1);
}

#[test]
fn test_dedup_hash_with_null() {
    let h = Collector::dedup_hash("tool", &serde_json::Value::Null);
    assert!(h.starts_with("sha256:"));
}

#[test]
fn test_dedup_hash_with_array() {
    let h = Collector::dedup_hash("tool", &serde_json::json!([1, 2, 3]));
    assert!(h.starts_with("sha256:"));
}

#[test]
fn test_dedup_hash_with_number() {
    let h = Collector::dedup_hash("tool", &serde_json::json!(42));
    assert!(h.starts_with("sha256:"));
}

#[test]
fn test_sanitize_args_multiple_sensitive_fields() {
    let args = serde_json::json!({
        "path": "/tmp/file",
        "api_key": "secret1",
        "token": "secret2",
        "password": "secret3",
        "data": "normal"
    });
    let sanitized = Collector::sanitize_args(&args, &["api_key", "token", "password"]);
    assert_eq!(sanitized["api_key"], "[REDACTED]");
    assert_eq!(sanitized["token"], "[REDACTED]");
    assert_eq!(sanitized["password"], "[REDACTED]");
    assert_eq!(sanitized["path"], "/tmp/file");
    assert_eq!(sanitized["data"], "normal");
}

#[test]
fn test_sanitize_args_nested_object() {
    let args = serde_json::json!({
        "config": {"api_key": "secret"},
        "name": "test"
    });
    let sanitized = Collector::sanitize_args(&args, &["api_key"]);
    // Only top-level keys are sanitized
    assert_eq!(sanitized["name"], "test");
    // 钉住边界：嵌套对象里的同名键不脱敏（文档化限制——若未来加深层递归，
    // 这里按新语义改断言，而不是静默漂移）。
    assert_eq!(
        sanitized["config"]["api_key"], "secret",
        "nested api_key must survive unchanged (top-level-only sanitize)"
    );
}

#[test]
fn test_max_size_large_holds_all_records() {
    // max_size = 0 实际语义是「容量 0」——记录即刻被逐出，**不是**不限量；
    // 「不限量」要用足够大的 max_size 表达。旧名
    // test_max_size_zero_means_unlimited 与注释/断言正好相反，会误导读者。
    let mut config = CollectorConfig::default();
    config.max_size = 1000;
    let collector = Collector::new(config);
    for i in 0..50 {
        let exp = make_experience("tool", &format!("input-{}", i), true);
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(collector.record(exp));
    }
    assert_eq!(collector.len(), 50);
}

#[tokio::test]
async fn test_load_append_preserves_existing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("append.jsonl");

    // Write a CollectedExperience record manually
    let record = r#"{"experience":{"id":"exp-1","tool_name":"tool_a","input_summary":"input","output_summary":"ok","success":true,"duration_ms":100,"timestamp":"2026-01-01T00:00:00Z","session_key":"s1"},"dedup_hash":"h1"}"#;
    tokio::fs::write(&path, record).await.unwrap();

    // Load from file
    let config = CollectorConfig::default();
    let collector = Collector::new(config);
    collector.load_from_file(&path).await.unwrap();
    assert_eq!(collector.len(), 1);
}

#[tokio::test]
async fn test_collector_is_empty() {
    let collector = Collector::new(CollectorConfig::default());
    assert!(collector.is_empty());
    assert_eq!(collector.len(), 0);

    let exp = make_experience("tool", "input", true);
    collector.record(exp).await;
    assert!(!collector.is_empty());
    assert_eq!(collector.len(), 1);
}

#[tokio::test]
async fn test_load_from_file_skips_blank_lines() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("blanks.jsonl");

    let exp = make_experience("tool_blank", "input", true);
    let ce = CollectedExperience {
        experience: exp,
        dedup_hash: Collector::dedup_hash(
            "tool_blank",
            &serde_json::json!({"input_summary": "input"}),
        ),
    };
    let line = serde_json::to_string(&ce).unwrap();
    // Blank lines (including whitespace-only) must be skipped without error.
    let content = format!("\n   \n{}\n\n", line);
    tokio::fs::write(&path, content).await.unwrap();

    let collector = Collector::new(CollectorConfig::default());
    collector.load_from_file(&path).await.unwrap();
    assert_eq!(collector.len(), 1);
    assert_eq!(
        collector.experiences()[0].experience.tool_name,
        "tool_blank"
    );
}

#[tokio::test]
async fn test_clear_resets_all_state() {
    let collector = Collector::new(CollectorConfig::default());
    let exp = make_experience("tool_c", "input", true);
    collector.record(exp).await;
    assert_eq!(collector.len(), 1);

    collector.clear();
    assert_eq!(collector.len(), 0);
    assert!(collector.is_empty());
    assert!(collector.experiences().is_empty());
}

#[test]
fn test_sanitize_args_non_object_passthrough() {
    // Non-object args (string/number/array/null) are returned unchanged.
    let scalar = serde_json::json!("some-string");
    assert_eq!(Collector::sanitize_args(&scalar, &["api_key"]), scalar);

    let num = serde_json::json!(42);
    assert_eq!(Collector::sanitize_args(&num, &["secret"]), num);

    let arr = serde_json::json!([1, 2, 3]);
    assert_eq!(Collector::sanitize_args(&arr, &["password"]), arr);
}

// --- S8 coverage additions (quality-hardening goal 冲刺 S8) ---

#[test]
fn test_s8_collector_new_evaluates_tracing_field_expressions() {
    let _guard = crate::test_support::quiet_trace_guard();
    let dir = tempfile::tempdir().unwrap();
    let mut config = CollectorConfig::default();
    config.persistence_path = dir.path().join("x.jsonl").to_string_lossy().to_string();
    let collector = Collector::new(config);
    assert!(collector.experiences().is_empty());
}

#[tokio::test]
async fn test_s8_record_persistence_failure_still_succeeds_in_memory() {
    // persistence_path whose parent is a regular file → create_dir_all in
    // new() fails (ignored), append_jsonl cannot open the target → warn path.
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, "not a dir").unwrap();
    let mut config = CollectorConfig::default();
    config.persistence_path = blocker.join("x.jsonl").to_string_lossy().to_string();
    let collector = Collector::new(config);

    let inserted = collector
        .record(make_experience("tool_a", "input", true))
        .await;
    assert!(inserted);
    assert_eq!(collector.experiences().len(), 1);
}
