use super::*;
use crate::bridge::NoOpBridge;

#[test]
fn test_fm8_sanitize_report_never_leaks_secrets() {
    // F-M8: sanitized report must never leak secrets. The sanitizer matches
    // `token: <20+ chars>` inside a value and replaces with [REDACTED]. If
    // replacement corrupts JSON → fail-closed returns {} (not the raw). Either
    // way, the secret must not appear in the result.
    let bridge = Arc::new(NoOpBridge::new("test".into()));
    let syncer = Syncer::new(bridge);
    let report = serde_json::json!({"content": "token: abcdefghijklmnopqrst1234567890"});
    let result = syncer.sanitize_report_for_test(&report);
    let result_str = serde_json::to_string(&result).unwrap_or_default();
    assert!(
        !result_str.contains("abcdefghijklmnopqrst1234567890"),
        "must never leak the secret, even on parse failure (F-M8)"
    );
}

#[tokio::test]
async fn test_syncer_with_noop_bridge() {
    let bridge = Arc::new(NoOpBridge::new("test-node".into()));
    let syncer = Syncer::new(bridge);

    assert!(syncer.is_enabled());

    let count = syncer
        .share_reflection(serde_json::json!({"insights": ["test"]}))
        .await
        .unwrap();
    assert_eq!(count, 0); // NoOp bridge returns 0
}

#[tokio::test]
async fn test_syncer_disabled() {
    let bridge = Arc::new(NoOpBridge::new("test-node".into()));
    let mut syncer = Syncer::new(bridge);
    syncer.disable();

    assert!(!syncer.is_enabled());

    let result = syncer.share_reflection(serde_json::json!({})).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_enable_disable() {
    let bridge = Arc::new(NoOpBridge::new("test".into()));
    let mut syncer = Syncer::new(bridge);
    assert!(syncer.is_enabled());
    syncer.disable();
    assert!(!syncer.is_enabled());
    syncer.enable();
    assert!(syncer.is_enabled());
}

#[tokio::test]
async fn test_fetch_remote_reflections() {
    let bridge = Arc::new(NoOpBridge::new("test".into()));
    let syncer = Syncer::new(bridge);

    let reflections = syncer.fetch_remote_reflections().await.unwrap();
    assert!(reflections.is_empty());
}

#[test]
fn test_receive_reflection() {
    let dir = tempfile::tempdir().unwrap();
    let bridge = Arc::new(NoOpBridge::new("test".into()));
    let syncer = Syncer::with_forge_dir(bridge, dir.path().to_path_buf());

    let payload = serde_json::json!({
        "content": "# Test Reflection\nSome insights here",
        "filename": "test_report.md",
        "from": "node-abc",
        "timestamp": "2026-04-29T12:00:00Z"
    });

    syncer.receive_reflection(&payload).unwrap();

    // Check file was created
    let remote_dir = dir.path().join("reflections").join("remote");
    assert!(remote_dir.exists());
    let entries: Vec<_> = std::fs::read_dir(&remote_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(entries.len(), 1);
    let name = entries[0].file_name().to_string_lossy().to_string();
    // The filename should contain the sanitized node ID prefix
    assert!(name.ends_with(".md"), "Expected .md file, got: {}", name);
    assert!(
        name.contains("node"),
        "Expected 'node' in filename, got: {}",
        name
    );
}

#[test]
fn test_receive_reflection_path_traversal() {
    let dir = tempfile::tempdir().unwrap();
    let bridge = Arc::new(NoOpBridge::new("test".into()));
    let syncer = Syncer::with_forge_dir(bridge, dir.path().to_path_buf());

    // Try path traversal
    let payload = serde_json::json!({
        "content": "test",
        "filename": "../../../etc/passwd",
    });

    syncer.receive_reflection(&payload).unwrap();

    // Should still write to remote dir (path stripped)
    let remote_dir = dir.path().join("reflections").join("remote");
    assert!(remote_dir.exists());
}

#[test]
fn test_get_reflections_list_payload() {
    let dir = tempfile::tempdir().unwrap();
    let bridge = Arc::new(NoOpBridge::new("test".into()));
    let syncer = Syncer::with_forge_dir(bridge, dir.path().to_path_buf());

    // Create some reflection files
    let ref_dir = dir.path().join("reflections");
    std::fs::create_dir_all(&ref_dir).unwrap();
    std::fs::write(ref_dir.join("report1.md"), "test1").unwrap();
    std::fs::write(ref_dir.join("report2.md"), "test2").unwrap();

    let payload = syncer.get_reflections_list_payload();
    let reflections = payload["reflections"].as_array().unwrap();
    assert_eq!(reflections.len(), 2);
    assert_eq!(payload["count"].as_u64().unwrap(), 2);
}

#[test]
fn test_read_reflection_content() {
    let dir = tempfile::tempdir().unwrap();
    let bridge = Arc::new(NoOpBridge::new("test".into()));
    let syncer = Syncer::with_forge_dir(bridge, dir.path().to_path_buf());

    let ref_dir = dir.path().join("reflections");
    std::fs::create_dir_all(&ref_dir).unwrap();
    std::fs::write(ref_dir.join("test.md"), "# Test Content").unwrap();

    let content = syncer.read_reflection_content("test.md").unwrap();
    assert_eq!(content, "# Test Content");
}

#[test]
fn test_sanitize_content() {
    let bridge = Arc::new(NoOpBridge::new("test".into()));
    let syncer = Syncer::new(bridge);
    let result = syncer.sanitize_content("test content");
    assert!(!result.is_empty());
}

#[test]
fn test_sanitize_node_id() {
    assert_eq!(sanitize_node_id("node-abc_123"), "node-abc_123");
    assert_eq!(sanitize_node_id("node with spaces"), "node_with_spaces");
    assert_eq!(sanitize_node_id(""), "unknown");
    assert_eq!(sanitize_node_id("a/b\\c"), "a_b_c");
}

#[tokio::test]
async fn test_set_bridge() {
    let bridge1 = Arc::new(NoOpBridge::new("node-1".into()));
    let mut syncer = Syncer::new(bridge1);
    assert_eq!(syncer.bridge.local_node_id(), "node-1");

    // Swap to a different bridge
    let bridge2 = Arc::new(NoOpBridge::new("node-2".into()));
    syncer.set_bridge(bridge2);
    assert_eq!(syncer.bridge.local_node_id(), "node-2");

    // Verify the syncer still works with the new bridge
    let count = syncer
        .share_reflection(serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn test_receive_reflection_missing_content() {
    let dir = tempfile::tempdir().unwrap();
    let bridge = Arc::new(NoOpBridge::new("test".into()));
    let syncer = Syncer::with_forge_dir(bridge, dir.path().to_path_buf());

    let payload = serde_json::json!({
        "filename": "test.md"
    });
    let result = syncer.receive_reflection(&payload);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("content"));
}

#[test]
fn test_receive_reflection_auto_filename() {
    let dir = tempfile::tempdir().unwrap();
    let bridge = Arc::new(NoOpBridge::new("test".into()));
    let syncer = Syncer::with_forge_dir(bridge, dir.path().to_path_buf());

    let payload = serde_json::json!({
        "content": "# Auto-filename test",
        "from": "node-auto"
    });
    syncer.receive_reflection(&payload).unwrap();

    let remote_dir = dir.path().join("reflections").join("remote");
    assert!(remote_dir.exists());
    let entries: Vec<_> = std::fs::read_dir(&remote_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(entries.len(), 1);
}

#[test]
fn test_receive_reflection_with_metadata_header() {
    let dir = tempfile::tempdir().unwrap();
    let bridge = Arc::new(NoOpBridge::new("test".into()));
    let syncer = Syncer::with_forge_dir(bridge, dir.path().to_path_buf());

    let payload = serde_json::json!({
        "content": "# Test content",
        "filename": "header_test.md",
        "from": "node-meta",
        "timestamp": "2026-04-29T12:00:00Z"
    });
    syncer.receive_reflection(&payload).unwrap();

    let remote_dir = dir.path().join("reflections").join("remote");
    let entries: Vec<_> = std::fs::read_dir(&remote_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(entries.len(), 1);
    let content = std::fs::read_to_string(entries[0].path()).unwrap();
    assert!(content.contains("Remote reflection from"));
    assert!(content.contains("node-meta"));
    assert!(content.contains("# Test content"));
}

#[test]
fn test_get_local_reflections_empty() {
    let dir = tempfile::tempdir().unwrap();
    let bridge = Arc::new(NoOpBridge::new("test".into()));
    let syncer = Syncer::with_forge_dir(bridge, dir.path().to_path_buf());

    let paths = syncer.get_local_reflections().unwrap();
    assert!(paths.is_empty());
}

#[test]
fn test_get_remote_reflections_paths_empty() {
    let dir = tempfile::tempdir().unwrap();
    let bridge = Arc::new(NoOpBridge::new("test".into()));
    let syncer = Syncer::with_forge_dir(bridge, dir.path().to_path_buf());

    let paths = syncer.get_remote_reflections_paths().unwrap();
    assert!(paths.is_empty());
}

#[test]
fn test_get_local_reflections_with_files() {
    let dir = tempfile::tempdir().unwrap();
    let bridge = Arc::new(NoOpBridge::new("test".into()));
    let syncer = Syncer::with_forge_dir(bridge, dir.path().to_path_buf());

    let ref_dir = dir.path().join("reflections");
    std::fs::create_dir_all(&ref_dir).unwrap();
    std::fs::write(ref_dir.join("a.md"), "content a").unwrap();
    std::fs::write(ref_dir.join("b.md"), "content b").unwrap();
    std::fs::write(ref_dir.join("c.txt"), "not md").unwrap();

    let paths = syncer.get_local_reflections().unwrap();
    assert_eq!(paths.len(), 2);
}

#[test]
fn test_read_reflection_content_dot_dot() {
    let dir = tempfile::tempdir().unwrap();
    let bridge = Arc::new(NoOpBridge::new("test".into()));
    let syncer = Syncer::with_forge_dir(bridge, dir.path().to_path_buf());

    let result = syncer.read_reflection_content("..");
    assert!(result.is_err());
}

#[test]
fn test_read_reflection_content_dot() {
    let dir = tempfile::tempdir().unwrap();
    let bridge = Arc::new(NoOpBridge::new("test".into()));
    let syncer = Syncer::with_forge_dir(bridge, dir.path().to_path_buf());

    let result = syncer.read_reflection_content(".");
    assert!(result.is_err());
}

#[test]
fn test_sanitize_content_removes_sensitive_data() {
    let bridge = Arc::new(NoOpBridge::new("test".into()));
    let syncer = Syncer::new(bridge);
    let content = "api_key: sk-1234567890abcdefghijklmnop";
    let sanitized = syncer.sanitize_content(content);
    assert!(sanitized.contains("[REDACTED]"));
}

#[test]
fn test_sanitize_node_id_special_chars() {
    assert_eq!(sanitize_node_id("node@#$%123!"), "node____123_");
}

#[test]
fn test_sanitize_node_id_all_special() {
    let result = sanitize_node_id("@#$%");
    assert_eq!(result, "____");
}

#[test]
fn test_sanitize_node_id_unicode() {
    let result = sanitize_node_id("node-123");
    assert_eq!(result, "node-123");
}

#[tokio::test]
async fn test_fetch_remote_reflections_disabled() {
    let bridge = Arc::new(NoOpBridge::new("test".into()));
    let mut syncer = Syncer::new(bridge);
    syncer.disable();
    let result = syncer.fetch_remote_reflections().await;
    assert!(result.is_err());
}

#[test]
fn test_get_reflections_list_payload_empty() {
    let dir = tempfile::tempdir().unwrap();
    let bridge = Arc::new(NoOpBridge::new("test".into()));
    let syncer = Syncer::with_forge_dir(bridge, dir.path().to_path_buf());

    let payload = syncer.get_reflections_list_payload();
    assert_eq!(payload["count"].as_u64().unwrap(), 0);
    assert!(payload["reflections"].as_array().unwrap().is_empty());
}

#[test]
fn test_receive_reflection_from_empty() {
    let dir = tempfile::tempdir().unwrap();
    let bridge = Arc::new(NoOpBridge::new("test".into()));
    let syncer = Syncer::with_forge_dir(bridge, dir.path().to_path_buf());

    let payload = serde_json::json!({
        "content": "test content without from field",
        "filename": "no_from.md"
    });
    syncer.receive_reflection(&payload).unwrap();

    let remote_dir = dir.path().join("reflections").join("remote");
    let entries: Vec<_> = std::fs::read_dir(&remote_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(entries.len(), 1);
}

#[tokio::test]
async fn test_share_reflection_sanitizes() {
    let bridge = Arc::new(NoOpBridge::new("test".into()));
    let syncer = Syncer::new(bridge);

    let report = serde_json::json!({
        "data": "api_key=sk-1234567890abcdefghijklmnopqrstuv"
    });
    let count = syncer.share_reflection(report).await.unwrap();
    assert_eq!(count, 0);
}

#[test]
fn test_with_forge_dir_stores_dir() {
    let dir = tempfile::tempdir().unwrap();
    let bridge = Arc::new(NoOpBridge::new("test".into()));
    let syncer = Syncer::with_forge_dir(bridge, dir.path().to_path_buf());

    let payload = serde_json::json!({"content": "test"});
    // Should create reflections/remote directory
    syncer.receive_reflection(&payload).unwrap();
    assert!(dir.path().join("reflections").join("remote").exists());
}

/// Edge case: share_reflection with mock peers that return non-zero count
/// (matches Go's TestSyncer_ShareReflection_WithPeers)
#[tokio::test]
async fn test_share_reflection_with_mock_peers() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct MockPeerBridge {
        node_id: String,
        share_call_count: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl ClusterForgeBridge for MockPeerBridge {
        async fn share_reflection(&self, _report: serde_json::Value) -> Result<usize, String> {
            self.share_call_count.fetch_add(1, Ordering::SeqCst);
            Ok(5) // Simulate 5 peers received the report
        }
        async fn get_remote_reflections(&self) -> Result<Vec<serde_json::Value>, String> {
            Ok(Vec::new())
        }
        async fn get_online_peers(&self) -> Result<Vec<String>, String> {
            Ok(vec![
                "peer-a".into(),
                "peer-b".into(),
                "peer-c".into(),
                "peer-d".into(),
                "peer-e".into(),
            ])
        }
        fn local_node_id(&self) -> &str {
            &self.node_id
        }
        fn is_cluster_enabled(&self) -> bool {
            true
        }
    }

    let share_call_count = Arc::new(AtomicUsize::new(0));
    let bridge = Arc::new(MockPeerBridge {
        node_id: "node-test".into(),
        share_call_count: share_call_count.clone(),
    });
    let syncer = Syncer::new(bridge);

    let report = serde_json::json!({
        "insights": ["tool X has 80% success rate"],
        "recommendations": ["Consider creating a skill for tool X"]
    });

    let count = syncer.share_reflection(report).await.unwrap();
    assert_eq!(count, 5, "Should report sharing with 5 peers");
    assert_eq!(share_call_count.load(Ordering::SeqCst), 1);
}

#[test]
fn test_receive_reflection_filename_dotdot_falls_back() {
    // filename ".." has no file_name() component, so the sanitizer falls back
    // to a generated "remote_{timestamp}.md" name instead of a path.
    let dir = tempfile::tempdir().unwrap();
    let bridge = Arc::new(NoOpBridge::new("test".into()));
    let syncer = Syncer::with_forge_dir(bridge, dir.path().to_path_buf());

    let payload = serde_json::json!({
        "content": "dotdot payload",
        "filename": "..",
        "from": "node-a",
    });

    syncer.receive_reflection(&payload).unwrap();

    let remote_dir = dir.path().join("reflections").join("remote");
    let entries: Vec<String> = std::fs::read_dir(&remote_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(entries.len(), 1, "exactly one remote file expected");
    assert!(
        entries[0].starts_with("node-a_remote_"),
        "expected fallback remote_* name with node prefix, got: {}",
        entries[0]
    );
    assert!(entries[0].ends_with(".md"));
}

#[test]
fn test_get_reflections_list_payload_error_when_reflections_is_file() {
    // When the reflections path exists but is a FILE (not a directory),
    // read_md_files fails and the payload carries the error key.
    let dir = tempfile::tempdir().unwrap();
    let bridge = Arc::new(NoOpBridge::new("test".into()));
    let syncer = Syncer::with_forge_dir(bridge, dir.path().to_path_buf());

    std::fs::write(dir.path().join("reflections"), "not a directory").unwrap();

    let payload = syncer.get_reflections_list_payload();
    let reflections = payload["reflections"].as_array().unwrap();
    assert!(reflections.is_empty());
    assert!(
        payload.get("error").is_some(),
        "expected an error key in payload, got: {}",
        payload
    );
}

// --- S8 coverage additions (quality-hardening goal 冲刺 S8) ---

#[tokio::test]
async fn test_s8_share_reflection_evaluates_tracing_field_expressions() {
    // node_id = %self.bridge.local_node_id() only evaluates with a subscriber.
    let _guard = crate::test_support::quiet_trace_guard();
    let bridge = Arc::new(NoOpBridge::new("trace-node".into()));
    let syncer = Syncer::new(bridge);
    let count = syncer
        .share_reflection(serde_json::json!({"insights": ["x"]}))
        .await
        .unwrap();
    assert_eq!(count, 0); // NoOp bridge shares with nobody
}

// ---- S8 Wave 2: receive_reflection / read_reflection_content branches ----

/// receive_reflection without a "from" node: sanitize_node_id maps the empty
/// id to "unknown", so the file is prefixed "unknown_" and the header renders
/// with that fallback node id. (The unprefixed else-arm at syncer.rs:147 is
/// therefore unreachable — sanitize_node_id never returns an empty string.)
#[test]
fn test_s8_receive_reflection_no_from() {
    let dir = tempfile::tempdir().unwrap();
    let bridge = Arc::new(NoOpBridge::new("local".into()));
    let syncer = Syncer::with_forge_dir(bridge, dir.path().to_path_buf());

    let payload = serde_json::json!({
        "content": "# remote insight\nhello",
        "filename": "r8.md",
        "timestamp": "2026-08-26T10:00:00Z",
    });
    syncer.receive_reflection(&payload).unwrap();

    let written = dir
        .path()
        .join("reflections")
        .join("remote")
        .join("unknown_r8.md");
    let body = std::fs::read_to_string(&written)
        .unwrap_or_else(|e| panic!("expected {:?}: {}", written, e));
    assert!(
        body.contains("<!-- Remote reflection from unknown at"),
        "body: {}",
        body
    );
    assert!(body.contains("# remote insight"));
}

/// receive_reflection with "from": filename gets the sanitized node prefix.
#[test]
fn test_s8_receive_reflection_with_from() {
    let dir = tempfile::tempdir().unwrap();
    let bridge = Arc::new(NoOpBridge::new("local".into()));
    let syncer = Syncer::with_forge_dir(bridge, dir.path().to_path_buf());

    let payload = serde_json::json!({
        "content": "peer report",
        "filename": "p1.md",
        "from": "peer-node-1",
    });
    syncer.receive_reflection(&payload).unwrap();

    let written = dir
        .path()
        .join("reflections")
        .join("remote")
        .join("peer-node-1_p1.md");
    assert!(written.exists(), "expected prefixed file at {:?}", written);
}

/// read_reflection_content on a file that does not exist: canonicalize of the
/// file fails and the containment check rejects the unresolved path.
#[test]
fn test_s8_read_reflection_content_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let bridge = Arc::new(NoOpBridge::new("local".into()));
    let syncer = Syncer::with_forge_dir(bridge, dir.path().to_path_buf());
    std::fs::create_dir_all(dir.path().join("reflections")).unwrap();

    let result = syncer.read_reflection_content("missing.md");
    assert!(result.is_err(), "expected err, got {:?}", result);
}

/// read_reflection_content round-trip for an existing report.
#[test]
fn test_s8_read_reflection_content_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let bridge = Arc::new(NoOpBridge::new("local".into()));
    let syncer = Syncer::with_forge_dir(bridge, dir.path().to_path_buf());
    let report = dir.path().join("reflections").join("r1.md");
    std::fs::create_dir_all(report.parent().unwrap()).unwrap();
    std::fs::write(&report, "# report body").unwrap();

    let content = syncer.read_reflection_content("r1.md").unwrap();
    assert_eq!(content, "# report body");
}
