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

// --- W3a: ringbuffer bounds, disabled arms, flush IO-error arm ---

fn disabled_sink() -> CaptureSink {
    CaptureSink {
        workspace: std::path::PathBuf::new(),
        enabled: false,
        buffers: std::sync::Mutex::new(std::collections::HashMap::new()),
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
    assert!(sink.buffers.lock().unwrap().is_empty());
}

/// record_session_write on a disabled sink is a no-op (early return).
#[test]
fn record_session_write_disabled_is_noop() {
    let sink = disabled_sink();
    sink.record_session_write("sk", write_cap(false));
    assert!(sink.buffers.lock().unwrap().is_empty());
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
    let dir = std::env::temp_dir().join(format!("nemesis_cap_rb_{}_{}", std::process::id(), line!()));
    let _ = std::fs::remove_dir_all(&dir);
    let sink = CaptureSink::for_test(dir.clone());
    for i in 0..52 {
        sink.record_tool("rb", tool_cap(&format!("t{i}")));
    }
    {
        let bufs = sink.buffers.lock().unwrap();
        let buf = bufs.get("rb").unwrap();
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
    let dir = std::env::temp_dir().join(format!("nemesis_cap_wr_{}_{}", std::process::id(), line!()));
    let _ = std::fs::remove_dir_all(&dir);
    let sink = CaptureSink::for_test(dir.clone());
    for _ in 0..202 {
        sink.record_session_write("wr", write_cap(false));
    }
    {
        let bufs = sink.buffers.lock().unwrap();
        let buf = bufs.get("wr").unwrap();
        assert_eq!(buf.session_writes.len(), 200, "bounded at MAX_SESSION_WRITES");
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
