use super::*;

#[test]
fn test_append_event() {
    let dir = tempfile::tempdir().unwrap();
    let config = AuditChainConfig {
        storage_path: dir.path().join("audit.jsonl"),
        ..Default::default()
    };
    let chain = AuditChain::new(config);

    let event = chain
        .append(
            "file_read",
            "read_file",
            "user1",
            "cli",
            "/tmp/test.txt",
            "allowed",
            "rule match",
        )
        .unwrap();
    assert_eq!(event.decision, "allowed");
    assert_ne!(event.hash, event.prev_hash);
    assert_eq!(chain.event_count(), 1);
    assert!(event.sign.is_none());
}

#[test]
fn test_append_with_sign() {
    let dir = tempfile::tempdir().unwrap();
    let config = AuditChainConfig {
        storage_path: dir.path().join("audit.jsonl"),
        ..Default::default()
    };
    let chain = AuditChain::new(config);

    let event = chain
        .append_with_sign(
            "file_read",
            "read_file",
            "u",
            "c",
            "/tmp",
            "allowed",
            "",
            Some("sig123".to_string()),
        )
        .unwrap();
    assert_eq!(event.sign, Some("sig123".to_string()));
}

#[test]
fn test_chain_hash_progression() {
    let dir = tempfile::tempdir().unwrap();
    let config = AuditChainConfig {
        storage_path: dir.path().join("audit.jsonl"),
        ..Default::default()
    };
    let chain = AuditChain::new(config);

    let e1 = chain
        .append(
            "file_read",
            "read_file",
            "user1",
            "cli",
            "/tmp/a",
            "allowed",
            "ok",
        )
        .unwrap();
    let e2 = chain
        .append(
            "file_write",
            "write_file",
            "user1",
            "cli",
            "/tmp/b",
            "denied",
            "blocked",
        )
        .unwrap();
    let e3 = chain
        .append(
            "process_exec",
            "exec",
            "user1",
            "cli",
            "ls",
            "allowed",
            "ok",
        )
        .unwrap();

    assert_eq!(e2.prev_hash, e1.hash);
    assert_eq!(e3.prev_hash, e2.hash);
    assert_eq!(chain.event_count(), 3);
}

#[test]
fn test_verify_chain() {
    let dir = tempfile::tempdir().unwrap();
    let config = AuditChainConfig {
        storage_path: dir.path().join("audit.jsonl"),
        ..Default::default()
    };
    let chain = AuditChain::new(config);

    let e1 = chain
        .append("file_read", "read_file", "u", "c", "/tmp", "allowed", "")
        .unwrap();
    let e2 = chain
        .append("file_write", "write_file", "u", "c", "/tmp", "denied", "")
        .unwrap();

    assert!(AuditChain::verify_chain(&[e1.clone(), e2.clone()]));

    // Tamper with an event
    let mut tampered = e2.clone();
    tampered.hash = "tampered".to_string();
    assert!(!AuditChain::verify_chain(&[e1, tampered]));
}

#[test]
fn test_jsonl_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("audit.jsonl");
    let config = AuditChainConfig {
        storage_path: path.clone(),
        ..Default::default()
    };
    let chain = AuditChain::new(config);
    chain
        .append("file_read", "read_file", "u", "c", "/tmp", "allowed", "")
        .unwrap();
    chain
        .append("file_write", "write_file", "u", "c", "/tmp", "denied", "")
        .unwrap();

    let content = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = content.trim().lines().collect();
    assert_eq!(lines.len(), 2);

    for line in lines {
        let _: AuditEvent = serde_json::from_str(line).unwrap();
    }
}

#[test]
fn test_segment_rotation() {
    let dir = tempfile::tempdir().unwrap();
    let config = AuditChainConfig {
        storage_path: dir.path().join("audit.jsonl"),
        max_events_per_segment: 5,
        ..Default::default()
    };
    let chain = AuditChain::new(config);

    for i in 0..12 {
        chain
            .append("test", "tool", "u", "c", &format!("{}", i), "allowed", "")
            .unwrap();
    }

    // Should have rotated at least once
    assert!(chain.segment_count() >= 2);
    assert_eq!(chain.total_event_count(), 12);
}

#[test]
fn test_export_load_chain() {
    let dir = tempfile::tempdir().unwrap();
    let config = AuditChainConfig {
        storage_path: dir.path().join("audit.jsonl"),
        ..Default::default()
    };
    let chain = AuditChain::new(config);

    chain
        .append("file_read", "read_file", "u", "c", "/tmp/a", "allowed", "")
        .unwrap();
    chain
        .append("file_write", "write_file", "u", "c", "/tmp/b", "denied", "")
        .unwrap();

    let export_path = dir.path().join("export.json");
    chain.export_chain(&export_path).unwrap();
    assert!(export_path.exists());

    // Load into a new chain
    let config2 = AuditChainConfig {
        storage_path: dir.path().join("audit2.jsonl"),
        ..Default::default()
    };
    let chain2 = AuditChain::new(config2);
    let count = chain2.load_chain(&export_path).unwrap();
    assert_eq!(count, 2);
}

#[test]
fn test_empty_chain_verify() {
    let result = AuditChain::verify_chain(&[]);
    assert!(result);
}

#[test]
fn test_single_event_chain_verify() {
    let dir = tempfile::tempdir().unwrap();
    let config = AuditChainConfig {
        storage_path: dir.path().join("audit.jsonl"),
        ..Default::default()
    };
    let chain = AuditChain::new(config);
    let e = chain
        .append("file_read", "read_file", "u", "c", "/tmp", "allowed", "")
        .unwrap();
    assert!(AuditChain::verify_chain(&[e]));
}

#[test]
fn test_event_has_timestamp() {
    let dir = tempfile::tempdir().unwrap();
    let config = AuditChainConfig {
        storage_path: dir.path().join("audit.jsonl"),
        ..Default::default()
    };
    let chain = AuditChain::new(config);
    let event = chain
        .append(
            "file_read",
            "read_file",
            "user1",
            "cli",
            "/tmp/test",
            "allowed",
            "",
        )
        .unwrap();
    assert!(!event.timestamp.is_empty());
}

#[test]
fn test_load_nonexistent_file() {
    let dir = tempfile::tempdir().unwrap();
    let config = AuditChainConfig {
        storage_path: dir.path().join("audit.jsonl"),
        ..Default::default()
    };
    let chain = AuditChain::new(config);
    let result = chain.load_chain(&dir.path().join("nonexistent.json"));
    assert!(result.is_err());
}

#[test]
fn test_config_default() {
    let config = AuditChainConfig::default();
    assert!(config.enabled);
    assert_eq!(config.max_events_per_segment, 100_000);
}

// ---- Additional integrity tests ----

#[test]
fn test_default_config_function() {
    let config = default_audit_chain_config();
    assert!(config.enabled);
    assert_eq!(config.max_file_size, 50 * 1024 * 1024);
    assert!(!config.verify_on_load);
    assert!(config.signing_key.is_none());
}

#[test]
fn test_audit_event_serialization() {
    let event = AuditEvent {
        id: "test-id".to_string(),
        timestamp: "2026-01-01T00:00:00Z".to_string(),
        operation: "file_read".to_string(),
        tool_name: "read_file".to_string(),
        user: "alice".to_string(),
        source: "cli".to_string(),
        target: "/tmp/test.txt".to_string(),
        decision: "allowed".to_string(),
        reason: "rule match".to_string(),
        hash: "abc123".to_string(),
        prev_hash: "000000".to_string(),
        sign: None,
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("test-id"));
    assert!(json.contains("file_read"));
    assert!(json.contains("alice"));
    assert!(json.contains("allowed"));

    let de: AuditEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(de.id, "test-id");
    assert_eq!(de.operation, "file_read");
    assert!(de.sign.is_none());
}

#[test]
fn test_audit_event_with_sign_serialization() {
    let event = AuditEvent {
        id: "signed-event".to_string(),
        timestamp: "2026-01-01T00:00:00Z".to_string(),
        operation: "process_exec".to_string(),
        tool_name: "exec".to_string(),
        user: "admin".to_string(),
        source: "api".to_string(),
        target: "ls".to_string(),
        decision: "denied".to_string(),
        reason: "dangerous".to_string(),
        hash: "def456".to_string(),
        prev_hash: "abc123".to_string(),
        sign: Some("ed25519_signature_data".to_string()),
    };
    let json = serde_json::to_string(&event).unwrap();
    let de: AuditEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(de.sign, Some("ed25519_signature_data".to_string()));
}

#[test]
fn test_audit_event_default_sign_is_none() {
    let json = r#"{"id":"x","timestamp":"","operation":"","tool_name":"","user":"","source":"","target":"","decision":"","reason":"","hash":"","prev_hash":""}"#;
    let event: AuditEvent = serde_json::from_str(json).unwrap();
    assert!(event.sign.is_none());
}

#[test]
fn test_close_prevents_append() {
    let dir = tempfile::tempdir().unwrap();
    let config = AuditChainConfig {
        storage_path: dir.path().join("audit.jsonl"),
        ..Default::default()
    };
    let chain = AuditChain::new(config);
    chain
        .append("op1", "tool1", "u", "c", "/tmp", "allowed", "")
        .unwrap();
    chain.close().unwrap();
    assert!(chain.is_closed());

    let result = chain.append("op2", "tool2", "u", "c", "/tmp", "denied", "");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("closed"));
}

#[test]
fn test_close_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let config = AuditChainConfig {
        storage_path: dir.path().join("audit.jsonl"),
        ..Default::default()
    };
    let chain = AuditChain::new(config);
    chain.close().unwrap();
    chain.close().unwrap(); // Second close should succeed
    assert!(chain.is_closed());
}

#[test]
fn test_current_hash_initial() {
    let dir = tempfile::tempdir().unwrap();
    let config = AuditChainConfig {
        storage_path: dir.path().join("audit.jsonl"),
        ..Default::default()
    };
    let chain = AuditChain::new(config);
    let hash = chain.current_hash();
    assert_eq!(
        hash,
        "0000000000000000000000000000000000000000000000000000000000000000"
    );
}

#[test]
fn test_current_hash_updates() {
    let dir = tempfile::tempdir().unwrap();
    let config = AuditChainConfig {
        storage_path: dir.path().join("audit.jsonl"),
        ..Default::default()
    };
    let chain = AuditChain::new(config);
    let initial = chain.current_hash().clone();
    chain
        .append("file_read", "read_file", "u", "c", "/tmp", "allowed", "")
        .unwrap();
    let after = chain.current_hash();
    assert_ne!(initial, after);
    assert_eq!(after.len(), 64); // SHA256 hex
}

#[test]
fn test_total_event_count() {
    let dir = tempfile::tempdir().unwrap();
    let config = AuditChainConfig {
        storage_path: dir.path().join("audit.jsonl"),
        ..Default::default()
    };
    let chain = AuditChain::new(config);
    assert_eq!(chain.total_event_count(), 0);
    chain.append("op1", "t1", "u", "c", "x", "a", "").unwrap();
    chain.append("op2", "t2", "u", "c", "y", "d", "").unwrap();
    chain.append("op3", "t3", "u", "c", "z", "a", "").unwrap();
    assert_eq!(chain.total_event_count(), 3);
}

#[test]
fn test_size_alias() {
    let dir = tempfile::tempdir().unwrap();
    let config = AuditChainConfig {
        storage_path: dir.path().join("audit.jsonl"),
        ..Default::default()
    };
    let chain = AuditChain::new(config);
    chain.append("op", "tool", "u", "c", "x", "a", "").unwrap();
    assert_eq!(chain.size(), 1);
}

#[test]
fn test_segment_count_initial() {
    let dir = tempfile::tempdir().unwrap();
    let config = AuditChainConfig {
        storage_path: dir.path().join("audit.jsonl"),
        ..Default::default()
    };
    let chain = AuditChain::new(config);
    assert_eq!(chain.segment_count(), 1);
}

#[test]
fn test_get_event_by_index() {
    let dir = tempfile::tempdir().unwrap();
    let config = AuditChainConfig {
        storage_path: dir.path().join("audit.jsonl"),
        ..Default::default()
    };
    let chain = AuditChain::new(config);
    chain
        .append("file_read", "read_file", "u", "c", "/tmp/a", "allowed", "")
        .unwrap();
    chain
        .append("file_write", "write_file", "u", "c", "/tmp/b", "denied", "")
        .unwrap();
    chain
        .append("process_exec", "exec", "u", "c", "ls", "allowed", "")
        .unwrap();

    let e0 = chain.get_event(0).unwrap();
    assert_eq!(e0.operation, "file_read");
    let e1 = chain.get_event(1).unwrap();
    assert_eq!(e1.operation, "file_write");
    let e2 = chain.get_event(2).unwrap();
    assert_eq!(e2.operation, "process_exec");
    assert!(chain.get_event(3).is_none());
}

#[test]
fn test_verify_range() {
    let dir = tempfile::tempdir().unwrap();
    let config = AuditChainConfig {
        storage_path: dir.path().join("audit.jsonl"),
        ..Default::default()
    };
    let chain = AuditChain::new(config);
    chain.append("op1", "t1", "u", "c", "x", "a", "").unwrap();
    chain.append("op2", "t2", "u", "c", "y", "d", "").unwrap();
    chain.append("op3", "t3", "u", "c", "z", "a", "").unwrap();

    let result = chain.verify_range(0, 2).unwrap();
    assert!(result);
}

#[test]
fn test_verify_chain_tampered_prev_hash() {
    let dir = tempfile::tempdir().unwrap();
    let config = AuditChainConfig {
        storage_path: dir.path().join("audit.jsonl"),
        ..Default::default()
    };
    let chain = AuditChain::new(config);
    let e1 = chain.append("op1", "t1", "u", "c", "x", "a", "").unwrap();
    let mut e2 = chain.append("op2", "t2", "u", "c", "y", "d", "").unwrap();

    // Tamper with prev_hash
    e2.prev_hash = "tampered".to_string();
    assert!(!AuditChain::verify_chain(&[e1, e2]));
}

#[test]
fn test_load_segments_from_disk() {
    let dir = tempfile::tempdir().unwrap();
    let config = AuditChainConfig {
        storage_path: dir.path().join("audit.jsonl"),
        ..Default::default()
    };
    let chain = AuditChain::new(config);
    chain.append("op1", "t1", "u", "c", "x", "a", "").unwrap();
    chain.append("op2", "t2", "u", "c", "y", "d", "").unwrap();

    // Create a new chain and load from the same file
    let config2 = AuditChainConfig {
        storage_path: dir.path().join("audit.jsonl"),
        ..Default::default()
    };
    let chain2 = AuditChain::new(config2);
    let count = chain2.load_segments().unwrap();
    assert_eq!(count, 2);
    assert_eq!(chain2.total_event_count(), 2);
}

#[test]
fn test_load_segments_empty() {
    let dir = tempfile::tempdir().unwrap();
    let config = AuditChainConfig {
        storage_path: dir.path().join("nonexistent.jsonl"),
        ..Default::default()
    };
    let chain = AuditChain::new(config);
    let count = chain.load_segments().unwrap();
    assert_eq!(count, 0);
}

#[test]
fn test_event_id_is_uuid() {
    let dir = tempfile::tempdir().unwrap();
    let config = AuditChainConfig {
        storage_path: dir.path().join("audit.jsonl"),
        ..Default::default()
    };
    let chain = AuditChain::new(config);
    let event = chain.append("op", "tool", "u", "c", "x", "a", "").unwrap();
    // UUID v4 format: 8-4-4-4-12 hex chars
    let parts: Vec<&str> = event.id.split('-').collect();
    assert_eq!(parts.len(), 5);
}

#[test]
fn test_multiple_events_unique_ids() {
    let dir = tempfile::tempdir().unwrap();
    let config = AuditChainConfig {
        storage_path: dir.path().join("audit.jsonl"),
        ..Default::default()
    };
    let chain = AuditChain::new(config);
    let e1 = chain.append("op1", "t1", "u", "c", "x", "a", "").unwrap();
    let e2 = chain.append("op2", "t2", "u", "c", "y", "d", "").unwrap();
    assert_ne!(e1.id, e2.id);
}

#[test]
fn test_config_custom_max_file_size() {
    let config = AuditChainConfig {
        max_file_size: 1024,
        ..Default::default()
    };
    assert_eq!(config.max_file_size, 1024);
}

#[test]
fn test_config_custom_signing_key() {
    let config = AuditChainConfig {
        signing_key: Some("my-secret-key".to_string()),
        ..Default::default()
    };
    assert_eq!(config.signing_key, Some("my-secret-key".to_string()));
}

#[test]
fn test_export_empty_chain() {
    let dir = tempfile::tempdir().unwrap();
    let config = AuditChainConfig {
        storage_path: dir.path().join("audit.jsonl"),
        ..Default::default()
    };
    let chain = AuditChain::new(config);
    let export_path = dir.path().join("export.json");
    chain.export_chain(&export_path).unwrap();
    assert!(export_path.exists());
    let content = std::fs::read_to_string(&export_path).unwrap();
    assert_eq!(content, "[]");
}
