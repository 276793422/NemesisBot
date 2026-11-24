use super::*;

fn make_event(event_type: &str) -> TraceEvent {
    TraceEvent {
        id: uuid::Uuid::new_v4().to_string(),
        event_type: event_type.into(),
        session_key: "test-session".into(),
        timestamp: chrono::Local::now().to_rfc3339(),
        data: serde_json::json!({"test": true}),
    }
}

fn make_event_at(event_type: &str, time: chrono::DateTime<chrono::Local>) -> TraceEvent {
    TraceEvent {
        id: uuid::Uuid::new_v4().to_string(),
        event_type: event_type.into(),
        session_key: "test-session".into(),
        timestamp: time.to_rfc3339(),
        data: serde_json::json!({"test": true}),
    }
}

#[tokio::test]
async fn test_append_and_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("traces.jsonl");
    let store = TraceStore::new(&path);

    store.append(&make_event("tool_call")).await.unwrap();
    store.append(&make_event("llm_response")).await.unwrap();

    let events = store.read_all().await.unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].event_type, "tool_call");
    assert_eq!(events[1].event_type, "llm_response");
}

#[tokio::test]
async fn test_count() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("traces.jsonl");
    let store = TraceStore::new(&path);

    assert_eq!(store.count().await.unwrap(), 0);
    store.append(&make_event("test")).await.unwrap();
    assert_eq!(store.count().await.unwrap(), 1);
}

#[tokio::test]
async fn test_clear() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("traces.jsonl");
    let store = TraceStore::new(&path);

    store.append(&make_event("test")).await.unwrap();
    assert!(path.exists());

    store.clear().await.unwrap();
    assert!(!path.exists());
}

#[tokio::test]
async fn test_empty_store() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nonexistent.jsonl");
    let store = TraceStore::new(&path);

    let events = store.read_all().await.unwrap();
    assert!(events.is_empty());
}

#[tokio::test]
async fn test_read_traces_since() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("traces.jsonl");
    let store = TraceStore::new(&path);

    let now = chrono::Local::now();
    let two_hours_ago = now - chrono::Duration::hours(2);
    let one_hour_ago = now - chrono::Duration::hours(1);

    store
        .append(&make_event_at("old_event", two_hours_ago))
        .await
        .unwrap();
    store
        .append(&make_event_at("recent_event", one_hour_ago))
        .await
        .unwrap();
    store
        .append(&make_event_at("new_event", now))
        .await
        .unwrap();

    // Read only events from the last 90 minutes
    let cutoff = now - chrono::Duration::minutes(90);
    let recent = store.read_traces_since(cutoff).await.unwrap();

    assert_eq!(recent.len(), 2);
    assert!(recent.iter().any(|e| e.event_type == "recent_event"));
    assert!(recent.iter().any(|e| e.event_type == "new_event"));
    assert!(!recent.iter().any(|e| e.event_type == "old_event"));
}

#[tokio::test]
async fn test_read_traces_since_all_match() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("traces.jsonl");
    let store = TraceStore::new(&path);

    store.append(&make_event("a")).await.unwrap();
    store.append(&make_event("b")).await.unwrap();

    let long_ago = chrono::Local::now() - chrono::Duration::days(365);
    let all = store.read_traces_since(long_ago).await.unwrap();
    assert_eq!(all.len(), 2);
}

#[tokio::test]
async fn test_read_traces_since_none_match() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("traces.jsonl");
    let store = TraceStore::new(&path);

    let past = chrono::Local::now() - chrono::Duration::hours(2);
    store.append(&make_event_at("old", past)).await.unwrap();

    let future = chrono::Local::now() + chrono::Duration::hours(1);
    let none = store.read_traces_since(future).await.unwrap();
    assert!(none.is_empty());
}

#[tokio::test]
async fn test_cleanup_removes_old() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("traces.jsonl");
    let store = TraceStore::new(&path);

    let now = chrono::Local::now();
    let two_days_ago = now - chrono::Duration::days(2);
    let one_hour_ago = now - chrono::Duration::hours(1);

    store
        .append(&make_event_at("old_event", two_days_ago))
        .await
        .unwrap();
    store
        .append(&make_event_at("recent_event", one_hour_ago))
        .await
        .unwrap();
    store
        .append(&make_event_at("new_event", now))
        .await
        .unwrap();

    let removed = store.cleanup(1).await.unwrap();
    assert_eq!(removed, 1);

    // Verify remaining events
    let remaining = store.read_all().await.unwrap();
    assert_eq!(remaining.len(), 2);
    assert!(!remaining.iter().any(|e| e.event_type == "old_event"));
    assert!(remaining.iter().any(|e| e.event_type == "recent_event"));
    assert!(remaining.iter().any(|e| e.event_type == "new_event"));
}

#[tokio::test]
async fn test_cleanup_removes_all() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("traces.jsonl");
    let store = TraceStore::new(&path);

    let past = chrono::Local::now() - chrono::Duration::days(10);
    store.append(&make_event_at("old1", past)).await.unwrap();
    store.append(&make_event_at("old2", past)).await.unwrap();

    let removed = store.cleanup(5).await.unwrap();
    assert_eq!(removed, 2);
    assert!(!path.exists()); // File deleted when empty
}

#[tokio::test]
async fn test_cleanup_nothing_to_remove() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("traces.jsonl");
    let store = TraceStore::new(&path);

    store.append(&make_event("fresh")).await.unwrap();

    let removed = store.cleanup(30).await.unwrap();
    assert_eq!(removed, 0);

    let remaining = store.read_all().await.unwrap();
    assert_eq!(remaining.len(), 1);
}

#[tokio::test]
async fn test_cleanup_nonexistent_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nonexistent.jsonl");
    let store = TraceStore::new(&path);

    let removed = store.cleanup(30).await.unwrap();
    assert_eq!(removed, 0);
}

// --- Additional trace_store tests ---

#[tokio::test]
async fn test_append_preserves_data() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("traces.jsonl");
    let store = TraceStore::new(&path);
    let event = TraceEvent {
        id: "evt-123".into(),
        event_type: "custom_type".into(),
        session_key: "session-abc".into(),
        timestamp: "2026-05-01T12:00:00Z".into(),
        data: serde_json::json!({"key": "value", "nested": {"a": 1}}),
    };
    store.append(&event).await.unwrap();
    let events = store.read_all().await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].id, "evt-123");
    assert_eq!(events[0].event_type, "custom_type");
    assert_eq!(events[0].session_key, "session-abc");
}

#[tokio::test]
async fn test_read_all_ignores_malformed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("traces.jsonl");
    let store = TraceStore::new(&path);
    store.append(&make_event("valid")).await.unwrap();
    // Append malformed data
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    writeln!(f, "not json").unwrap();
    writeln!(f).unwrap();
    let events = store.read_all().await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, "valid");
}

#[tokio::test]
async fn test_count_after_multiple_appends() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("traces.jsonl");
    let store = TraceStore::new(&path);
    for i in 0..5 {
        store
            .append(&make_event(&format!("evt-{}", i)))
            .await
            .unwrap();
    }
    assert_eq!(store.count().await.unwrap(), 5);
}

#[tokio::test]
async fn test_clear_nonexistent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nope.jsonl");
    let store = TraceStore::new(&path);
    store.clear().await.unwrap(); // Should not panic
}

#[tokio::test]
async fn test_read_traces_since_empty_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty.jsonl");
    let store = TraceStore::new(&path);
    let result = store.read_traces_since(chrono::Local::now()).await.unwrap();
    assert!(result.is_empty());
}

#[tokio::test]
async fn test_append_many_events() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("many.jsonl");
    let store = TraceStore::new(&path);
    for i in 0..20 {
        store
            .append(&make_event(&format!("type-{}", i)))
            .await
            .unwrap();
    }
    assert_eq!(store.count().await.unwrap(), 20);
}

#[tokio::test]
async fn test_cleanup_partial() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("partial.jsonl");
    let store = TraceStore::new(&path);
    let now = chrono::Local::now();
    let three_days_ago = now - chrono::Duration::days(3);
    store
        .append(&make_event_at("old", three_days_ago))
        .await
        .unwrap();
    store.append(&make_event("new")).await.unwrap();
    let removed = store.cleanup(2).await.unwrap();
    assert_eq!(removed, 1);
    let remaining = store.read_all().await.unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].event_type, "new");
}

#[tokio::test]
async fn test_append_creates_nested_parent_dirs() {
    let dir = tempfile::tempdir().unwrap();
    // No part of tmp/sub/dir exists beforehand — append must create parents.
    let path = dir.path().join("sub").join("dir").join("traces.jsonl");
    let store = TraceStore::new(&path);

    store.append(&make_event("nested")).await.unwrap();
    assert!(path.exists());

    let events = store.read_all().await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, "nested");
}

// --- coverage additions (quality-hardening goal 冲刺) ---

#[tokio::test]
async fn test_s8_append_empty_path_takes_no_parent_branch() {
    // An empty path has no parent component: append() must take the
    // `if let Some(parent)` fall-through region, then fail to open "" as a
    // file. Deterministic Err on Windows and Unix, no filesystem side effect.
    let store = TraceStore::new("");
    let result = store.append(&make_event("no-parent")).await;
    assert!(result.is_err());
}

/// 配套：cleanup 重写后文件内容必须恰好是保留事件
/// （JSONL 契约：每行合法 JSON 且以 \n 收尾，append 侧可继续追加），
/// 且不留原子写临时残渣。注：旧实现（write+truncate 原地重写）在成功
/// 路径上产出相同内容——本测试钉行为不回归；崩溃/并发丢失窗口的消除
/// 由修复构造性保证（唯一临时名 + rename，与 experience_store F- /
/// registry F- 同族），单测无法注入 crash 验证。
#[tokio::test]
async fn test_cleanup_rewrite_content_and_no_tmp_residue() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("traces.jsonl");
    let store = TraceStore::new(&path);

    let now = chrono::Local::now();
    store
        .append(&make_event_at(
            "old_event",
            now - chrono::Duration::days(10),
        ))
        .await
        .unwrap();
    store
        .append(&make_event_at("fresh_event", now))
        .await
        .unwrap();

    let removed = store.cleanup(7).await.unwrap();
    assert_eq!(removed, 1);

    let content = tokio::fs::read_to_string(&path).await.unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines.len(), 1, "only the fresh event line must remain");
    let parsed: TraceEvent = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(parsed.event_type, "fresh_event");
    assert!(content.ends_with('\n'), "JSONL lines must end with newline");

    // 原子写临时文件不残留（write_file_atomic 的.tmp-<pid>-<seq>- 形态）
    let leftovers: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.contains(".tmp-"))
        .collect();
    assert!(leftovers.is_empty(), "tmp residue: {leftovers:?}");
}

// R735 回归锁：u64::MAX 修复前 `as i64` 回绕成 -1 → cutoff 落未来 =
// 一次调用全删 trace；现钳到 time_bounds::MAX_AGE_DAYS，近期事件存活。
#[tokio::test]
async fn r735_cleanup_u64_max_does_not_panic_and_keeps_recent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("traces.jsonl");
    let store = TraceStore::new(&path);

    store
        .append(&make_event_at("recent", chrono::Local::now()))
        .await
        .unwrap();

    let removed = store.cleanup(u64::MAX).await.unwrap();
    assert_eq!(
        removed, 0,
        "钳到上界后近期 trace 必须存活，removed={removed}"
    );
    let remaining = store.read_all().await.unwrap();
    assert_eq!(remaining.len(), 1);
    assert!(remaining.iter().any(|e| e.event_type == "recent"));
}

/// BUG-20261125-004 同族回归锁（确定性锁纪律测试）：手动持 write_lock 后
/// spawn append 与 cleanup，持锁下给足 100ms 完成预算——若任一侧未被锁
/// 挡（锁纪律破坏），微秒级即完成 → timeout Ok = 红；正确阻塞 → Err = 绿。
/// 即 append（每次工具派发的频率）与 cleanup 的读→滤→整文件写回在同一
/// 把互斥上串行化，丢失窗在结构上不存在。
///
/// 为什么不用并发冒烟测：窗口是 µs 级同步段，多线程形态阳性对照实证
/// 撤锁 5/5 假绿；is_finished 探测同样不敏感（append 自身多段 async IO
/// 未跑完同样 false，撤锁后仍假绿）——「给足预算仍未完成」才是阻塞证据。
#[tokio::test]
async fn append_and_cleanup_serialize_on_write_lock() {
    let dir = tempfile::tempdir().unwrap();
    let store = std::sync::Arc::new(TraceStore::new(dir.path().join("traces.jsonl")));
    store.append(&make_event("seed")).await.unwrap();

    // 外部持锁（模拟清理侧已进入临界区）。
    let guard = store.write_lock.lock().await;

    let mut append_task = {
        let s = store.clone();
        tokio::spawn(async move { s.append(&make_event("x")).await })
    };
    let mut cleanup_task = {
        let s = store.clone();
        tokio::spawn(async move { s.cleanup(365).await })
    };
    let budget = std::time::Duration::from_millis(100);
    assert!(
        tokio::time::timeout(budget, &mut append_task)
            .await
            .is_err(),
        "append 必须在 write_lock 释放前阻塞"
    );
    assert!(
        tokio::time::timeout(budget, &mut cleanup_task)
            .await
            .is_err(),
        "cleanup 必须在 write_lock 释放前阻塞"
    );

    drop(guard);
    append_task.await.unwrap().unwrap();
    cleanup_task.await.unwrap().unwrap();
    let n = store.count().await.unwrap();
    assert_eq!(n, 2, "锁释放后两侧都必须完成且零丢失: {n}");
}
