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
