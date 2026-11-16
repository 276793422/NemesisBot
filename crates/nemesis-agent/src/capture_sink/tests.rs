use super::*;

#[test]
fn sanitize_replaces_unsafe_chars() {
    assert_eq!(sanitize("web:b6e6d66b"), "web_b6e6d66b");
    assert_eq!(sanitize("ok-name.1"), "ok-name.1");
}

#[test]
fn uninit_global_is_noop() {
    // Without init, global() is None — call sites must tolerate this.
    let _ = CaptureSink::global(); // must not panic
}

#[test]
fn flush_writes_all_evidence_files() {
    let dir = std::env::temp_dir().join(format!(
        "nemesis_cap_flush_{}_{}",
        std::process::id(),
        line!()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let sink = CaptureSink::for_test(dir.clone());
    let sk = "web:abc123";

    // Simulate a bloated tool output (the suspected context-blowout trigger).
    sink.record_tool(
        sk,
        ToolCapture {
            tool_name: "read_file".to_string(),
            arguments: "{}".to_string(),
            result: "X".repeat(5000),
            success: true,
            duration_ms: 12,
            error: String::new(),
            llm_round: 1,
            ts: String::new(),
        },
    );
    // Simulate the suspected old-snapshot overwrite (before<after? no: 4<52).
    sink.record_session_write(
        sk,
        SessionWriteCapture {
            writer: "set_history".to_string(),
            op: "set_history".to_string(),
            before_len: Some(52),
            after_len: Some(4),
            first_role: None,
            last_role: None,
            messages_hash: "deadbeef".to_string(),
            overwrite_detected: false, // avoid auto-flush; tested separately
            ts: String::new(),
        },
    );
    sink.flush(
        sk,
        "llm_retry_exhausted",
        Some("trace-1"),
        Some("context_length_exceeded: this model maximum is 65536 tokens"),
    );

    let base = dir.join("logs").join("capture").join("web_abc123");
    let entries: Vec<_> = std::fs::read_dir(&base).unwrap().collect();
    assert_eq!(entries.len(), 1, "exactly one capture event dir expected");
    let edir = entries[0].as_ref().unwrap().path();
    let edir_s = edir.to_string_lossy().to_string();
    assert!(edir_s.contains("llm_retry_exhausted"), "dir: {}", edir_s);
    assert!(edir.join("00.summary.json").exists());
    assert!(edir.join("01.tools.json").exists(), "tool capture missing");
    assert!(
        edir.join("02.session_writes.jsonl").exists(),
        "session writes missing"
    );
    assert!(edir.join("05.error.txt").exists(), "error text missing");
    let err = std::fs::read_to_string(edir.join("05.error.txt")).unwrap();
    assert!(
        err.contains("context_length_exceeded"),
        "full error text must be preserved untruncated"
    );
    let tools = std::fs::read_to_string(edir.join("01.tools.json")).unwrap();
    assert!(tools.contains("read_file"), "tool name missing");
    assert!(
        tools.contains(&"X".repeat(100)),
        "full result must be captured"
    );
    let summary = std::fs::read_to_string(edir.join("00.summary.json")).unwrap();
    assert!(summary.contains("trace-1"));
    assert!(summary.contains("\"tool_calls\": 1"));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn overwrite_auto_flushes() {
    let dir =
        std::env::temp_dir().join(format!("nemesis_cap_ow_{}_{}", std::process::id(), line!()));
    let _ = std::fs::remove_dir_all(&dir);
    let sink = CaptureSink::for_test(dir.clone());
    let sk = "web:ow";

    sink.record_session_write(
        sk,
        SessionWriteCapture {
            writer: "set_history".to_string(),
            op: "set_history".to_string(),
            before_len: Some(52),
            after_len: Some(4),
            first_role: None,
            last_role: None,
            messages_hash: "h".to_string(),
            overwrite_detected: true,
            ts: String::new(),
        },
    );

    let base = dir.join("logs").join("capture").join("web_ow");
    let entries: Vec<_> = std::fs::read_dir(&base).unwrap().collect();
    assert_eq!(entries.len(), 1, "overwrite should auto-flush immediately");
    let edir = entries[0].as_ref().unwrap().path();
    assert!(
        edir.to_string_lossy().contains("session_overwrite"),
        "dir: {}",
        edir.display()
    );
    assert!(edir.join("02.session_writes.jsonl").exists());

    let _ = std::fs::remove_dir_all(&dir);
}

// ---: ringbuffer bounds, disabled arms, flush IO-error arm ---

fn disabled_sink() -> CaptureSink {
    CaptureSink {
        workspace: std::path::PathBuf::new(),
        enabled: false,
        max_buffer_bytes: MAX_TOTAL_BUFFER_BYTES,
        buffers: std::sync::Mutex::new(Default::default()),
    }
}

fn tool_cap(name: &str) -> ToolCapture {
    ToolCapture {
        tool_name: name.to_string(),
        arguments: "{}".to_string(),
        result: "r".to_string(),
        success: true,
        duration_ms: 1,
        error: String::new(),
        llm_round: 1,
        ts: String::new(),
    }
}

fn write_cap(overwrite: bool) -> SessionWriteCapture {
    SessionWriteCapture {
        writer: "add_message".to_string(),
        op: "add_message".to_string(),
        before_len: Some(1),
        after_len: Some(2),
        first_role: Some("user".to_string()),
        last_role: Some("assistant".to_string()),
        messages_hash: "h".to_string(),
        overwrite_detected: overwrite,
        ts: String::new(),
    }
}

/// record_tool on a disabled sink is a no-op (early return).
#[test]
fn record_tool_disabled_is_noop() {
    let sink = disabled_sink();
    sink.record_tool("sk", tool_cap("t"));
    assert!(sink.buffers.lock().unwrap().map.is_empty());
}

/// record_session_write on a disabled sink is a no-op (early return).
#[test]
fn record_session_write_disabled_is_noop() {
    let sink = disabled_sink();
    sink.record_session_write("sk", write_cap(false));
    assert!(sink.buffers.lock().unwrap().map.is_empty());
}

/// flush on a disabled sink returns before touching the filesystem.
#[test]
fn flush_disabled_is_noop() {
    let sink = disabled_sink();
    sink.flush("sk", "context_error", None, Some("boom"));
    assert!(!sink.workspace.join("logs").exists());
}

/// Tool ringbuffer is bounded at MAX_TOOLS=50: oldest entries dropped.
#[test]
fn tool_ringbuffer_drains_oldest_beyond_50() {
    let dir =
        std::env::temp_dir().join(format!("nemesis_cap_rb_{}_{}", std::process::id(), line!()));
    let _ = std::fs::remove_dir_all(&dir);
    let sink = CaptureSink::for_test(dir.clone());
    for i in 0..52 {
        sink.record_tool("rb", tool_cap(&format!("t{i}")));
    }
    {
        let bufs = sink.buffers.lock().unwrap();
        let buf = bufs.map.get("rb").unwrap();
        assert_eq!(buf.tools.len(), 50, "bounded at MAX_TOOLS");
        assert_eq!(buf.tools[0].tool_name, "t2", "oldest two dropped");
        assert_eq!(buf.tools[49].tool_name, "t51");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Session-write ringbuffer is bounded at MAX_SESSION_WRITES=200 (and
/// overwrite=false records never auto-flush).
#[test]
fn session_write_ringbuffer_drains_oldest_beyond_200() {
    let dir =
        std::env::temp_dir().join(format!("nemesis_cap_wr_{}_{}", std::process::id(), line!()));
    let _ = std::fs::remove_dir_all(&dir);
    let sink = CaptureSink::for_test(dir.clone());
    for _ in 0..202 {
        sink.record_session_write("wr", write_cap(false));
    }
    {
        let bufs = sink.buffers.lock().unwrap();
        let buf = bufs.map.get("wr").unwrap();
        assert_eq!(
            buf.session_writes.len(),
            200,
            "bounded at MAX_SESSION_WRITES"
        );
        // ts was auto-filled (non-empty) on record.
        assert!(!buf.session_writes[0].ts.is_empty());
    }
    assert!(
        !dir.join("logs").exists(),
        "no auto-flush without overwrite signal"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// flush with a workspace whose path is a FILE: create_dir_all fails ->
/// warn + early return (best-effort, no panic).
#[test]
fn flush_with_file_workspace_warns_and_returns() {
    let tmp = tempfile::tempdir().unwrap();
    let blocker = tmp.path().join("blocker");
    std::fs::write(&blocker, b"x").unwrap();
    let sink = CaptureSink::for_test(blocker);
    sink.record_tool("sk2", tool_cap("t"));
    sink.flush("sk2", "llm_retry_exhausted", None, None); // must not panic
}

// ---: global byte budget / LRU eviction (BUG-20261117-045) ---

/// A tool record of approximately `n` payload bytes.
fn fat_tool_cap(name: &str, n: usize) -> ToolCapture {
    ToolCapture {
        tool_name: name.to_string(),
        arguments: "{}".to_string(),
        result: "x".repeat(n),
        success: true,
        duration_ms: 1,
        error: String::new(),
        llm_round: 1,
        ts: String::new(),
    }
}

/// Budget exceeded -> idle sessions evicted oldest-recorded-first; the most
/// recently recorded session survives. Recording to A *after* B makes A the
/// most recent despite B being inserted later — LRU, not insertion order.
#[test]
fn budget_exceeded_evicts_idle_sessions_lru() {
    let dir = std::env::temp_dir().join(format!(
        "nemesis_cap_lru_{}_{}",
        std::process::id(),
        line!()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    // Budget fits ~1.5 records of 1000 bytes.
    let sink = CaptureSink::for_test_with_budget(dir.clone(), 1500);
    sink.record_tool("B", fat_tool_cap("b1", 1000));
    sink.record_tool("A", fat_tool_cap("a1", 1000)); // total > 1500
    let rec_bytes = tool_rec_bytes(&fat_tool_cap("a1", 1000));
    {
        let bufs = sink.buffers.lock().unwrap();
        // B (idle, oldest) evicted; A survives as most-recent.
        assert!(bufs.map.contains_key("A"), "most-recent session survives");
        assert!(!bufs.map.contains_key("B"), "idle oldest session evicted");
        assert_eq!(
            bufs.total_bytes, rec_bytes,
            "accounting reflects survivor only"
        );
        assert_eq!(bufs.order.len(), 1);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Refresh-on-record: a session re-recorded after others stays alive; an
/// older session that was NOT re-touched is evicted first even though its
/// neighbour is newer by insertion.
#[test]
fn budget_lru_refresh_keeps_active_session() {
    let dir = std::env::temp_dir().join(format!(
        "nemesis_cap_lr2_{}_{}",
        std::process::id(),
        line!()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let sink = CaptureSink::for_test_with_budget(dir.clone(), 1500);
    sink.record_tool("old", fat_tool_cap("o1", 1000));
    sink.record_tool("new", fat_tool_cap("n1", 1000)); // evicts "old" at this point
    sink.record_tool("new", fat_tool_cap("n2", 1000)); // over budget again
    let rec_bytes = tool_rec_bytes(&fat_tool_cap("n2", 1000));
    {
        let bufs = sink.buffers.lock().unwrap();
        assert!(bufs.map.contains_key("new"), "re-touched session survives");
        assert!(!bufs.map.contains_key("old"));
        // Both of "new"'s records are retained: a single session may exceed
        // the budget on its own (documented residual — never evict the live
        // session), so total = 2 records.
        assert_eq!(bufs.total_bytes, 2 * rec_bytes);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// flush releases the session's byte accounting: total drops to 0 and the
/// map/order no longer hold the key (no stale LRU entries).
#[test]
fn flush_releases_budget_accounting() {
    let dir = std::env::temp_dir().join(format!(
        "nemesis_cap_acct_{}_{}",
        std::process::id(),
        line!()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let sink = CaptureSink::for_test_with_budget(dir.clone(), 1 << 20);
    sink.record_tool("acct", fat_tool_cap("t1", 5000));
    sink.record_session_write("acct", write_cap(false));
    assert!(sink.buffers.lock().unwrap().total_bytes > 5000);
    sink.flush("acct", "agent_error", None, None);
    {
        let bufs = sink.buffers.lock().unwrap();
        assert!(bufs.map.is_empty());
        assert!(bufs.order.is_empty());
        assert_eq!(bufs.total_bytes, 0);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A single session over budget on its own is retained (documented
/// residual: the live session's full evidence is the point of the sink) —
/// and the saturating accounting never panics on underflow.
#[test]
fn single_session_over_budget_is_retained() {
    let dir = std::env::temp_dir().join(format!(
        "nemesis_cap_res_{}_{}",
        std::process::id(),
        line!()
    ));
    let _ = fs::remove_dir_all(&dir);
    let sink = CaptureSink::for_test_with_budget(dir.clone(), 100);
    sink.record_tool("solo", fat_tool_cap("big", 5000));
    {
        let bufs = sink.buffers.lock().unwrap();
        assert_eq!(bufs.map.len(), 1, "live session never evicted");
        assert!(bufs.order.len() == 1);
    }
    let _ = fs::remove_dir_all(&dir);
}

/// Ring drain (count bound) releases byte accounting too: pushing 52 fat
/// records keeps total at 50 records' worth, not 52.
#[test]
fn ring_drain_releases_bytes() {
    let dir =
        std::env::temp_dir().join(format!("nemesis_cap_rd_{}_{}", std::process::id(), line!()));
    let _ = fs::remove_dir_all(&dir);
    let sink = CaptureSink::for_test_with_budget(dir.clone(), 1 << 30);
    for _ in 0..52 {
        sink.record_tool("rd", fat_tool_cap("t", 1000));
    }
    let bufs = sink.buffers.lock().unwrap();
    assert_eq!(bufs.map.get("rd").unwrap().tools.len(), 50);
    let rec_bytes = tool_rec_bytes(&fat_tool_cap("t", 1000));
    assert_eq!(
        bufs.total_bytes,
        50 * rec_bytes,
        "drained records release bytes"
    );
    drop(bufs);
    let _ = fs::remove_dir_all(&dir);
}
