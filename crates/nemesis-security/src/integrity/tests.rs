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

// ----：hash 覆盖面缺 source/reason ----
// 旧公式只进 prev/timestamp/operation/tool/user/target/decision 七字段，
// 篡改 jsonl 行的 reason（为何放行/拒绝的取证叙述）不破坏链验证。

use sha2::{Digest, Sha256};

/// 旧七字段公式（测试内复刻，钉死 v0 兼容契约不被静默改动）。
fn legacy_formula_hash(ev: &AuditEvent) -> String {
    let mut hasher = Sha256::new();
    hasher.update(ev.prev_hash.as_bytes());
    hasher.update(ev.timestamp.as_bytes());
    hasher.update(ev.operation.as_bytes());
    hasher.update(ev.tool_name.as_bytes());
    hasher.update(ev.user.as_bytes());
    hasher.update(ev.target.as_bytes());
    hasher.update(ev.decision.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[test]
fn test_reason_tamper_breaks_verification() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("audit.jsonl");
    let config = AuditChainConfig {
        storage_path: path.clone(),
        ..Default::default()
    };
    let chain = AuditChain::new(config);
    chain
        .append(
            "file_read",
            "read_file",
            "u",
            "cli",
            "/tmp/a",
            "allowed",
            "rule match",
        )
        .unwrap();
    chain
        .append(
            "file_write",
            "write_file",
            "u",
            "cli",
            "/tmp/b",
            "denied",
            "workspace boundary",
        )
        .unwrap();

    // 攻击形态：改写磁盘行的 reason（伪造成「user approved」），hash 不动。
    let content = std::fs::read_to_string(&path).unwrap();
    let mut tampered_events = Vec::new();
    for line in content.lines() {
        let mut v: serde_json::Value = serde_json::from_str(line).unwrap();
        if v["decision"] == "denied" {
            v["reason"] = serde_json::json!("user approved");
        }
        tampered_events.push(serde_json::from_str::<AuditEvent>(&v.to_string()).unwrap());
    }
    assert!(!AuditChain::verify_chain(&tampered_events));
}

#[test]
fn test_source_tamper_breaks_verification() {
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
            "u",
            "cli",
            "/tmp",
            "allowed",
            "ok",
        )
        .unwrap();
    let mut tampered = e1.clone();
    tampered.source = "web".to_string();
    assert!(!AuditChain::verify_chain(&[tampered]));
}

#[test]
fn test_appended_events_roundtrip_verify() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("audit.jsonl");
    let config = AuditChainConfig {
        storage_path: path.clone(),
        ..Default::default()
    };
    let chain = AuditChain::new(config);
    chain
        .append(
            "file_read",
            "read_file",
            "u",
            "cli",
            "/tmp/a",
            "allowed",
            "ok",
        )
        .unwrap();
    chain
        .append("exec", "exec", "u", "cli", "ls", "allowed", "ok")
        .unwrap();

    let content = std::fs::read_to_string(&path).unwrap();
    let events: Vec<AuditEvent> = content
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(events.len(), 2);
    // BUG-20261120-002 起 append 产 v2（长度前缀公式）事件。
    assert_eq!(events[0].hash_version, 2);
    assert!(AuditChain::verify_chain(&events));
}

#[test]
fn test_legacy_hash_version_events_verify() {
    // 存量链兼容：hash_version=0 事件按旧七字段公式验证。
    let genesis = "0".repeat(64);
    let mut e1 = AuditEvent {
        id: "l1".to_string(),
        timestamp: "2026-01-01T00:00:00Z".to_string(),
        operation: "file_read".to_string(),
        tool_name: "read_file".to_string(),
        user: "u".to_string(),
        source: "cli".to_string(),
        target: "/tmp/a".to_string(),
        decision: "allowed".to_string(),
        reason: "legacy".to_string(),
        hash: String::new(),
        prev_hash: genesis,
        sign: None,
        hash_version: 0,
    };
    e1.hash = legacy_formula_hash(&e1);
    let mut e2 = AuditEvent {
        prev_hash: e1.hash.clone(),
        operation: "exec".to_string(),
        tool_name: "exec".to_string(),
        target: "ls".to_string(),
        decision: "denied".to_string(),
        reason: "blocked".to_string(),
        ..e1.clone()
    };
    e2.hash = legacy_formula_hash(&e2);
    assert!(AuditChain::verify_chain(&[e1, e2]));
}

#[test]
fn test_unknown_hash_version_fails_closed() {
    let mut ev = AuditEvent {
        id: "x".to_string(),
        timestamp: "t".to_string(),
        operation: "op".to_string(),
        tool_name: "tn".to_string(),
        user: "u".to_string(),
        source: "s".to_string(),
        target: "tg".to_string(),
        decision: "allowed".to_string(),
        reason: "r".to_string(),
        hash: String::new(),
        prev_hash: "0".repeat(64),
        sign: None,
        hash_version: 9,
    };
    // 即便 hash 恰好匹配某个公式，未知版本也必须整链判负。
    ev.hash = legacy_formula_hash(&ev);
    assert!(!AuditChain::verify_chain(&[ev]));
}

#[test]
fn test_compute_event_hash_v1_covers_source_and_reason() {
    let base = AuditEvent {
        id: "e".to_string(),
        timestamp: "t".to_string(),
        operation: "op".to_string(),
        tool_name: "tn".to_string(),
        user: "u".to_string(),
        source: "s".to_string(),
        target: "tg".to_string(),
        decision: "d".to_string(),
        reason: "r".to_string(),
        hash: String::new(),
        prev_hash: "p".to_string(),
        sign: None,
        hash_version: 1,
    };
    let mut other_reason = base.clone();
    other_reason.reason = "r2".to_string();
    let mut other_source = base.clone();
    other_source.source = "s2".to_string();
    assert_ne!(
        AuditChain::compute_event_hash(&base),
        AuditChain::compute_event_hash(&other_reason)
    );
    assert_ne!(
        AuditChain::compute_event_hash(&base),
        AuditChain::compute_event_hash(&other_source)
    );

    // v0 同字段差异不进 hash（旧契约本貌）。
    let mut v0 = base.clone();
    v0.hash_version = 0;
    let mut v0_other_reason = v0.clone();
    v0_other_reason.reason = "r2".to_string();
    assert_eq!(
        AuditChain::compute_event_hash(&v0),
        AuditChain::compute_event_hash(&v0_other_reason)
    );
}

// ---- BUG-20261120-002：v1 裸拼接字段边界歧义 ----
// v1 公式九字段裸拼接：operation="exec"+tool_name="x" 与
// operation="execx"+tool_name="" 产生同一字节流 → 同一哈希。持有盘上写
// 权限的攻击者把内容在字段间重切分（工具名/操作语义被改写），哈希不变、
// 链验证照过。v2 长度前缀使字段边界唯一，同一切分变换哈希必变。

/// v2 公式（测试内复刻，钉死长度前缀契约不被静默改动）。
fn v2_formula_hash(ev: &AuditEvent) -> String {
    let mut hasher = Sha256::new();
    for field in [
        &ev.prev_hash,
        &ev.timestamp,
        &ev.operation,
        &ev.tool_name,
        &ev.user,
        &ev.target,
        &ev.decision,
        &ev.source,
        &ev.reason,
    ] {
        hasher.update((field.len() as u64).to_le_bytes());
        hasher.update(field.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

fn repartition_base(hash_version: u8) -> AuditEvent {
    AuditEvent {
        id: "e".to_string(),
        timestamp: "t".to_string(),
        operation: "exec".to_string(),
        tool_name: "x".to_string(),
        user: "u".to_string(),
        source: "s".to_string(),
        target: "tg".to_string(),
        decision: "d".to_string(),
        reason: "r".to_string(),
        hash: String::new(),
        prev_hash: "p".to_string(),
        sign: None,
        hash_version,
    }
}

#[test]
fn test_v2_hash_rejects_field_repartition() {
    // 切分 A：operation="exec", tool_name="x"（字节流含 "execx" 段）。
    let a = repartition_base(2);
    // 切分 B：同一字节段在 operation/tool_name 间重切——语义完全不同
    // （操作名与工具名互换归属），字节流总量相同。
    let mut b = repartition_base(2);
    b.operation = "execx".to_string();
    b.tool_name = String::new();

    // v2（长度前缀）：切分变化 → 哈希必变 → 篡改必检出。
    assert_ne!(
        AuditChain::compute_event_hash(&a),
        AuditChain::compute_event_hash(&b)
    );
    assert_eq!(AuditChain::compute_event_hash(&a), v2_formula_hash(&a));

    // 对照：v1（裸拼接）下同一切分变换哈希不变（歧义本貌——正是被修的洞）。
    let mut a1 = a.clone();
    a1.hash_version = 1;
    let mut b1 = b.clone();
    b1.hash_version = 1;
    assert_eq!(
        AuditChain::compute_event_hash(&a1),
        AuditChain::compute_event_hash(&b1)
    );
}

#[test]
fn test_v2_append_produces_prefix_formula_and_verifies() {
    let dir = tempfile::tempdir().unwrap();
    let config = AuditChainConfig {
        storage_path: dir.path().join("audit.jsonl"),
        ..Default::default()
    };
    let chain = AuditChain::new(config);
    let e1 = chain
        .append("exec", "exec", "u", "cli", "ls -la", "allowed", "ok")
        .unwrap();
    let e2 = chain
        .append(
            "file_write",
            "write_file",
            "u",
            "cli",
            "/tmp/b",
            "denied",
            "no",
        )
        .unwrap();
    assert_eq!(e1.hash_version, 2);
    assert_eq!(e1.hash, v2_formula_hash(&e1));
    assert_eq!(e2.hash, v2_formula_hash(&e2));
    assert!(AuditChain::verify_chain(&[e1, e2]));
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

// ---- BUG-20261120-001：段游标不回载 ----
// load_segments 旧实现只回载 last_hash/total_events，segment_count/event_count
// 保持 new() 初值（1/0）。发生过段轮转的链重启后 append 回写主文件——新事件
// 物理位置落在段中文件**前面**，盘上物理序与链序永久错位，verify 恒红无自愈。

#[test]
fn test_rotation_restart_resumes_current_segment() {
    let dir = tempfile::tempdir().unwrap();
    let make_config = || AuditChainConfig {
        storage_path: dir.path().join("audit.jsonl"),
        max_events_per_segment: 3,
        ..Default::default()
    };

    // 进程 A：写 4 事件（3 触发轮转 → seg0002 持有第 4 事件）。
    let chain_a = AuditChain::new(make_config());
    let mut last = None;
    for i in 0..4 {
        last = Some(
            chain_a
                .append("test", "tool", "u", "c", &format!("a{i}"), "allowed", "")
                .unwrap(),
        );
    }
    let tail_a = last.unwrap().hash;
    let seg2_path = dir.path().join("audit_seg0002.jsonl");
    assert!(seg2_path.exists(), "rotation should have created seg0002");

    // 进程 B（重启）：load_segments 回载后 append 必须续写 seg0002，
    // 且物理序（main → seg0002）与链序一致、全链 verify 过。
    let chain_b = AuditChain::new(make_config());
    let loaded = chain_b.load_segments().unwrap();
    assert_eq!(loaded, 4);
    assert_eq!(chain_b.current_hash(), tail_a);
    assert_eq!(
        chain_b.segment_count(),
        2,
        "segment cursor must be restored"
    );
    assert_eq!(
        chain_b.event_count(),
        1,
        "current segment line count must be restored"
    );

    let e5 = chain_b
        .append("test", "tool", "u", "c", "a4", "allowed", "")
        .unwrap();
    assert_eq!(
        e5.prev_hash, tail_a,
        "new event must chain onto restored tail"
    );
    // 落点必须 seg0002（回写主文件 = BUG-20261120-001 本貌）。
    let seg2_content = std::fs::read_to_string(&seg2_path).unwrap();
    assert_eq!(
        seg2_content.lines().count(),
        2,
        "new event must land in seg0002"
    );

    // 物理序 = 消费面读序（main → seg0002 升序），全链验证过。
    let mut all = Vec::new();
    for p in [dir.path().join("audit.jsonl"), seg2_path] {
        for line in std::fs::read_to_string(&p).unwrap().lines() {
            all.push(serde_json::from_str::<AuditEvent>(line).unwrap());
        }
    }
    assert_eq!(all.len(), 5);
    assert!(AuditChain::verify_chain(&all));
}

#[test]
fn test_rotation_restart_single_segment_restores_event_count() {
    // 未轮转链（只有主文件）重启：event_count 必须回载主文件行数——
    // 旧实现回 0，轮转计数每次重启清零，段上限可能永不触发。
    let dir = tempfile::tempdir().unwrap();
    let make_config = || AuditChainConfig {
        storage_path: dir.path().join("audit.jsonl"),
        max_events_per_segment: 5,
        ..Default::default()
    };

    let chain_a = AuditChain::new(make_config());
    for i in 0..3 {
        chain_a
            .append("test", "tool", "u", "c", &format!("b{i}"), "allowed", "")
            .unwrap();
    }
    assert_eq!(chain_a.segment_count(), 1);

    let chain_b = AuditChain::new(make_config());
    chain_b.load_segments().unwrap();
    assert_eq!(chain_b.segment_count(), 1);
    assert_eq!(
        chain_b.event_count(),
        3,
        "main-file line count must be restored"
    );
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
        hash_version: 0,
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
        hash_version: 0,
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

// ============================================================
// batch 3: rotated-segment export / tampered load_chain /
// verify_range edge arms / cross-segment get_event / fresh close
// ============================================================

#[test]
fn test_export_chain_with_rotated_segments() {
    let dir = tempfile::tempdir().unwrap();
    let config = AuditChainConfig {
        storage_path: dir.path().join("audit.jsonl"),
        max_events_per_segment: 2,
        ..Default::default()
    };
    let chain = AuditChain::new(config);
    for i in 0..5 {
        chain
            .append("test", "tool", "u", "c", &format!("f{i}"), "allowed", "")
            .unwrap();
    }
    assert!(
        chain.segment_count() >= 2,
        "segments: {}",
        chain.segment_count()
    );

    // 导出会合并主文件 + 所有 _seg 段文件
    let export_path = dir.path().join("rotated_export.json");
    chain.export_chain(&export_path).unwrap();
    let content = std::fs::read_to_string(&export_path).unwrap();
    let events: Vec<AuditEvent> = serde_json::from_str(&content).unwrap();
    assert_eq!(events.len(), 5);
    for (i, e) in events.iter().enumerate() {
        assert_eq!(e.target, format!("f{i}"), "event {i}: {:?}", e.target);
    }
}

#[test]
fn test_load_chain_tampered_hash_errors() {
    let dir = tempfile::tempdir().unwrap();
    let config = AuditChainConfig {
        storage_path: dir.path().join("audit.jsonl"),
        ..Default::default()
    };
    let chain = AuditChain::new(config);
    chain.append("op1", "t1", "u", "c", "x", "a", "").unwrap();
    chain.append("op2", "t2", "u", "c", "y", "d", "").unwrap();

    let export_path = dir.path().join("tampered.json");
    chain.export_chain(&export_path).unwrap();

    // 篡改第二个事件的 hash → verify_chain 失败 → load_chain 报错
    let mut events: Vec<AuditEvent> =
        serde_json::from_str(&std::fs::read_to_string(&export_path).unwrap()).unwrap();
    events[1].hash = vec!['0'; events[1].hash.len()].into_iter().collect();
    std::fs::write(&export_path, serde_json::to_string(&events).unwrap()).unwrap();

    let config2 = AuditChainConfig {
        storage_path: dir.path().join("audit2.jsonl"),
        ..Default::default()
    };
    let chain2 = AuditChain::new(config2);
    let err = chain2.load_chain(&export_path).unwrap_err();
    assert!(err.contains("chain verification failed"), "{err}");
}

#[test]
fn test_verify_range_single_event_and_out_of_range() {
    let dir = tempfile::tempdir().unwrap();
    let config = AuditChainConfig {
        storage_path: dir.path().join("audit.jsonl"),
        ..Default::default()
    };
    let chain = AuditChain::new(config);
    chain.append("op1", "t1", "u", "c", "x", "a", "").unwrap();
    chain.append("op2", "t2", "u", "c", "y", "d", "").unwrap();
    chain.append("op3", "t3", "u", "c", "z", "a", "").unwrap();

    // 单事件区间（len<2）→ 直接 Ok(true)
    assert!(chain.verify_range(0, 0).unwrap());
    // from 超出事件总数 → 空区间 → Ok(true)
    assert!(chain.verify_range(10, 12).unwrap());
}

#[test]
fn test_get_event_across_rotated_segments_and_close() {
    let dir = tempfile::tempdir().unwrap();
    let config = AuditChainConfig {
        storage_path: dir.path().join("audit.jsonl"),
        max_events_per_segment: 2,
        ..Default::default()
    };
    let chain = AuditChain::new(config);
    for i in 0..5 {
        chain
            .append("test", "tool", "u", "c", &format!("f{i}"), "allowed", "")
            .unwrap();
    }
    // 索引 4 落在第二个段文件里
    let e4 = chain.get_event(4).unwrap();
    assert_eq!(e4.target, "f4");
    assert!(chain.get_event(5).is_none());

    // 全新链 close（was_closed=false 路径）+ 幂等
    assert!(!chain.is_closed());
    chain.close().unwrap();
    assert!(chain.is_closed());
    chain.close().unwrap();
}

// ---------------------------------------------------------------------------
// append 全程串行并发回归钉（横扫九期）：链不变量
// 「行 i.prev == 行 i-1.hash」——修复前 last_hash 读/写两次独立取锁，
// 并发 append 各读到同一 prev ⇒ 链分叉（verify 必 FAIL）。
// ---------------------------------------------------------------------------

#[test]
fn concurrent_appends_keep_chain_linked() {
    let dir = tempfile::tempdir().unwrap();
    let config = AuditChainConfig {
        storage_path: dir.path().join("audit.jsonl"),
        ..Default::default()
    };
    let chain = std::sync::Arc::new(AuditChain::new(config));

    // 4 线程 × 25 次 append——修复前高概率分叉（verify 报 prev 断链）。
    std::thread::scope(|s| {
        for t in 0..4 {
            let chain = chain.clone();
            s.spawn(move || {
                for i in 0..25 {
                    chain
                        .append(
                            "file_read",
                            "read_file",
                            &format!("user-{t}"),
                            "test",
                            &format!("/tmp/{t}/{i}"),
                            "allowed",
                            "pin",
                        )
                        .expect("append ok");
                }
            });
        }
    });

    assert_eq!(chain.total_event_count(), 100);
    assert_eq!(chain.event_count(), 100);

    // 读回 JSONL 逐行验链：prev 链必须完整（分叉 = 某行 prev 不等于前一行 hash）。
    let content = std::fs::read_to_string(dir.path().join("audit.jsonl")).unwrap();
    let events: Vec<AuditEvent> = content
        .lines()
        .map(|l| serde_json::from_str(l).expect("parse event"))
        .collect();
    assert_eq!(events.len(), 100);
    assert!(
        AuditChain::verify_chain(&events),
        "并发 append 后链必须完整"
    );
}

#[test]
fn test_append_persist_failure_does_not_advance_chain() {
    // BUG（横扫角度 A）：落盘失败曾静默吞 + 内存链照常推进 +
    // 恒返回 Ok——内存与盘脱节后 verify 按盘上行校验从此必 FAIL，事发零
    // 可见。盘先行后：落盘失败必须返回 Err、内存链尾不动（下个事件重用
    // 同一 prev_hash，内存==磁盘保持一致），恢复后链继续且盘上可验。
    let dir = tempfile::tempdir().unwrap();
    let config = AuditChainConfig {
        storage_path: dir.path().join("missing_dir").join("audit.jsonl"),
        ..Default::default()
    };
    let chain = AuditChain::new(config);
    let genesis = chain.current_hash();

    // 父目录不存在 → open 必失败（全平台确定性注入，不玩权限游戏）
    let err = chain
        .append("file_read", "read_file", "u", "cli", "/t", "allowed", "r")
        .expect_err("落盘失败必须返回 Err，不得谎报 Ok");
    assert!(!err.is_empty());
    assert_eq!(chain.current_hash(), genesis, "落盘失败不得推进内存链尾");
    assert_eq!(chain.event_count(), 0, "落盘失败不得计数");
    assert_eq!(chain.total_event_count(), 0);

    // 恢复（父目录就位）→ 追加成功且链尾正确衔接失败前的 genesis
    std::fs::create_dir_all(dir.path().join("missing_dir")).unwrap();
    let ev1 = chain
        .append("file_read", "read_file", "u", "cli", "/t", "allowed", "r")
        .expect("恢复后 append 应成功");
    assert_eq!(ev1.prev_hash, genesis, "恢复后首事件必须重用失败前的链尾");
    let ev2 = chain
        .append("file_read", "read_file", "u", "cli", "/t2", "allowed", "r")
        .expect("第二次 append 应成功");
    assert_eq!(ev2.prev_hash, ev1.hash);
    assert_eq!(chain.event_count(), 2);

    // 盘上链可独立校验（防篡改语义在失败恢复后完整）
    let content =
        std::fs::read_to_string(dir.path().join("missing_dir").join("audit.jsonl")).unwrap();
    let events: Vec<AuditEvent> = content
        .lines()
        .map(|l| serde_json::from_str(l).expect("parse event"))
        .collect();
    assert_eq!(events.len(), 2);
    assert!(
        AuditChain::verify_chain(&events),
        "失败恢复后的盘上链必须 verify 通过"
    );
}

/// 链尾回载（重启断链）。链状态（last_hash）是进程内存
/// 态而 storage_path 固定路径续写——重启后不回载则首个事件 prev_hash=全零，
/// 与盘上既有链断裂，verify_chain 全量校验恒 FAIL。回归锁：第一生命周期
/// 落两个事件 → 模拟重启（全新 AuditChain + load_segments）→ 续写一个
/// 事件 → 全量读盘 verify 必须通过（含重启衔接点）。
#[test]
fn test_bug374_chain_restart_replays_tail_from_disk() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("audit.jsonl");
    let make_config = || AuditChainConfig {
        storage_path: path.clone(),
        ..Default::default()
    };

    // 第一生命周期：落两个事件
    let chain = AuditChain::new(make_config());
    chain
        .append("file_read", "read_file", "u", "cli", "/t1", "allowed", "r1")
        .unwrap();
    let ev2 = chain
        .append("file_read", "read_file", "u", "cli", "/t2", "allowed", "r2")
        .unwrap();

    // 模拟网关重启：全新实例（内存链尾回全零）+ 盘上回载
    let restarted = AuditChain::new(make_config());
    assert_eq!(
        restarted.current_hash(),
        "0000000000000000000000000000000000000000000000000000000000000000",
        "新实例链尾必须从全零起步（回载前）"
    );
    let loaded = restarted.load_segments().unwrap();
    assert_eq!(loaded, 2, "回载必须读到盘上 2 个事件");
    assert_eq!(
        restarted.current_hash(),
        ev2.hash,
        "回载后链尾必须接上盘上最后一行"
    );

    // 重启后续写：prev 必须衔接回载链尾，不得回全零
    let ev3 = restarted
        .append("file_read", "read_file", "u", "cli", "/t3", "allowed", "r3")
        .unwrap();
    assert_eq!(
        ev3.prev_hash, ev2.hash,
        "重启后首事件 prev_hash 必须衔接盘上链尾（修复前为全零=断链）"
    );

    // 全量读盘 verify：重启衔接点前后链完整
    let content = std::fs::read_to_string(&path).unwrap();
    let events: Vec<AuditEvent> = content
        .lines()
        .map(|l| serde_json::from_str(l).expect("parse event"))
        .collect();
    assert_eq!(events.len(), 3);
    assert!(
        AuditChain::verify_chain(&events),
        "跨重启的盘上链必须 verify 通过"
    );
}

/// 反向语义：回载缺失形态（不调 load_segments 直接续写）
/// 必然产生断链——钉住缺陷机理本身，防未来有人误删回载接线后测试静默变绿。
#[test]
fn test_bug374_without_replay_chain_breaks() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("audit.jsonl");
    let make_config = || AuditChainConfig {
        storage_path: path.clone(),
        ..Default::default()
    };

    let chain = AuditChain::new(make_config());
    chain
        .append("file_read", "read_file", "u", "cli", "/t1", "allowed", "r1")
        .unwrap();

    // 不回载直接续写（旧缺陷形态）：盘上链在衔接点断裂
    let restarted = AuditChain::new(make_config());
    restarted
        .append("file_read", "read_file", "u", "cli", "/t2", "allowed", "r2")
        .unwrap();

    let content = std::fs::read_to_string(&path).unwrap();
    let events: Vec<AuditEvent> = content
        .lines()
        .map(|l| serde_json::from_str(l).expect("parse event"))
        .collect();
    assert_eq!(events.len(), 2);
    assert!(
        !AuditChain::verify_chain(&events),
        "无回载续写的盘上链必须 verify FAIL（缺陷机理钉死）"
    );
}

// ---------------------------------------------------------------------------
// BUG-20261114-005 回归锁：撕裂尾自愈（BUG-20261113-168 event_ledger 同族）。
// writeln 撕裂（磁盘满中途落盘/断电）留半行残迹 ⇒ 旧行为下一笔追加拼行 =
// 新事件盘上不可解析但内存已推进 ⇒ 解析视图永久断链（verify 恒红无修复）。
// ---------------------------------------------------------------------------

fn write_torn_bytes(path: &std::path::Path, bytes: &[u8]) {
    use std::io::Write as _;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .expect("open for torn bytes");
    f.write_all(bytes).expect("write torn bytes");
    f.sync_all().expect("sync torn bytes");
}

fn read_parsed_events(path: &std::path::Path) -> Vec<AuditEvent> {
    std::fs::read_to_string(path)
        .expect("read segment")
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

#[test]
fn test_heal_torn_half_line_truncates_and_chain_continues() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("audit.jsonl");
    let chain = AuditChain::new(AuditChainConfig {
        storage_path: path.clone(),
        ..Default::default()
    });

    chain
        .append("file_read", "read_file", "u", "cli", "/t1", "allowed", "r1")
        .unwrap();
    chain
        .append("file_read", "read_file", "u", "cli", "/t2", "allowed", "r2")
        .unwrap();

    // 模拟撕裂写：半行残迹（无换行、不可解析）。
    write_torn_bytes(&path, b"{\"id\":\"torn");

    // 下一笔追加必须先自愈（截残迹）再干净落链。
    chain
        .append("file_read", "read_file", "u", "cli", "/t3", "allowed", "r3")
        .unwrap();

    let events = read_parsed_events(&path);
    assert_eq!(events.len(), 3, "半行残迹被截，三事件全可解析");
    assert!(
        AuditChain::verify_chain(&events),
        "自愈后链必须完整（不再永久红）"
    );
    assert_eq!(chain.total_event_count(), 3);
}

#[test]
fn test_heal_restores_complete_event_missing_newline() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("audit.jsonl");
    let chain = AuditChain::new(AuditChainConfig {
        storage_path: path.clone(),
        ..Default::default()
    });

    chain
        .append("file_read", "read_file", "u", "cli", "/t1", "allowed", "r1")
        .unwrap();
    let e2 = chain
        .append("file_read", "read_file", "u", "cli", "/t2", "allowed", "r2")
        .unwrap();

    // 模拟「content 落盘、换行丢失」撕裂形态：完整事件 JSON 无换行。
    // 该事件 prev=e2.hash、hash 按同一公式计算——盘上合法链尾，只是丢换行。
    let torn = AuditEvent {
        id: uuid::Uuid::new_v4().to_string(),
        timestamp: chrono::Local::now().to_rfc3339(),
        operation: "file_write".into(),
        tool_name: "write_file".into(),
        user: "u".into(),
        source: "cli".into(),
        target: "/t3".into(),
        decision: "allowed".into(),
        reason: "r3".into(),
        hash: String::new(),
        prev_hash: e2.hash.clone(),
        sign: None,
        hash_version: 1,
    };
    let mut torn_hasher = sha2::Sha256::new();
    torn_hasher.update(torn.prev_hash.as_bytes());
    torn_hasher.update(torn.timestamp.as_bytes());
    torn_hasher.update(torn.operation.as_bytes());
    torn_hasher.update(torn.tool_name.as_bytes());
    torn_hasher.update(torn.user.as_bytes());
    torn_hasher.update(torn.target.as_bytes());
    torn_hasher.update(torn.decision.as_bytes());
    torn_hasher.update(torn.source.as_bytes());
    torn_hasher.update(torn.reason.as_bytes());
    let torn_json = {
        let mut t = torn.clone();
        t.hash = format!("{:x}", torn_hasher.finalize());
        serde_json::to_string(&t).unwrap()
    };
    write_torn_bytes(&path, torn_json.as_bytes()); // 无换行

    // 下一笔追加：先补换行恢复残迹事件 + 采纳进链尾，再落自己的事件。
    chain
        .append("file_read", "read_file", "u", "cli", "/t4", "allowed", "r4")
        .unwrap();

    let events = read_parsed_events(&path);
    assert_eq!(events.len(), 4, "丢换行的完整事件被恢复入链");
    assert!(
        AuditChain::verify_chain(&events),
        "恢复 + 采纳 + 续写后链完整"
    );
    assert_eq!(chain.total_event_count(), 4, "恢复事件计入链计数");
}
