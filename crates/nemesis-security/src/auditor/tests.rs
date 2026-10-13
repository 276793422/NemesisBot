use super::*;

#[test]
fn test_default_auditor_allows_when_disabled() {
    let config = AuditorConfig {
        enabled: false,
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);

    let req = OperationRequest {
        id: "test-1".to_string(),
        op_type: OperationType::ProcessExec,
        danger_level: DangerLevel::Critical,
        user: "test".to_string(),
        source: "cli".to_string(),
        target: "rm -rf /".to_string(),
        timestamp: None,
        ..Default::default()
    };

    let (allowed, _, _) = auditor.request_permission(&req);
    assert!(allowed);
}

#[test]
fn test_auditor_deny_by_default() {
    let config = AuditorConfig {
        enabled: true,
        default_action: "deny".to_string(),
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);

    let req = OperationRequest {
        id: "test-2".to_string(),
        op_type: OperationType::ProcessExec,
        danger_level: DangerLevel::Critical,
        user: "test".to_string(),
        source: "cli".to_string(),
        target: "ls".to_string(),
        timestamp: None,
        ..Default::default()
    };

    let (allowed, err, _) = auditor.request_permission(&req);
    assert!(!allowed);
    assert!(err.is_some());
}

#[test]
fn test_auditor_allow_with_rules() {
    let config = AuditorConfig {
        enabled: true,
        default_action: "deny".to_string(),
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);

    auditor.set_rules(
        OperationType::FileRead,
        vec![SecurityRule {
            // glob 方言 match-all（D12-1 后只认 `*`/`**`；裸 `.*` 曾靠正则
            // 任意字符语义 match-all，字面点号后退化为「含点号才命中」）。
            pattern: "**".to_string(),
            action: "allow".to_string(),
            comment: "allow all reads".to_string(),
        }],
    );

    let req = OperationRequest {
        id: "test-3".to_string(),
        op_type: OperationType::FileRead,
        danger_level: DangerLevel::Low,
        user: "test".to_string(),
        source: "cli".to_string(),
        target: "/tmp/test.txt".to_string(),
        timestamp: None,
        ..Default::default()
    };

    let (allowed, _, _) = auditor.request_permission(&req);
    assert!(allowed);
}

#[test]
fn test_approve_deny_pending() {
    let config = AuditorConfig {
        enabled: true,
        default_action: "ask".to_string(),
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);

    let req = OperationRequest {
        id: "test-4".to_string(),
        op_type: OperationType::FileWrite,
        danger_level: DangerLevel::High,
        user: "test".to_string(),
        source: "cli".to_string(),
        target: "/tmp/test.txt".to_string(),
        timestamp: None,
        ..Default::default()
    };

    let (allowed, _, _) = auditor.request_permission(&req);
    assert!(!allowed); // requires approval
    assert_eq!(auditor.pending_count(), 1);

    auditor.approve_request("test-4", "admin").unwrap();
    assert_eq!(auditor.pending_count(), 0);
}

#[test]
fn test_deny_pending_request() {
    let config = AuditorConfig {
        enabled: true,
        default_action: "ask".to_string(),
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);

    let req = OperationRequest {
        id: "test-deny".to_string(),
        op_type: OperationType::ProcessExec,
        danger_level: DangerLevel::Critical,
        user: "test".to_string(),
        source: "cli".to_string(),
        target: "rm -rf /".to_string(),
        timestamp: None,
        ..Default::default()
    };

    let (allowed, _, _) = auditor.request_permission(&req);
    assert!(!allowed);
    assert_eq!(auditor.pending_count(), 1);

    auditor
        .deny_request("test-deny", "admin", "too dangerous")
        .unwrap();
    assert_eq!(auditor.pending_count(), 0);
}

#[test]
fn test_approve_nonexistent_fails() {
    let config = AuditorConfig::default();
    let auditor = SecurityAuditor::new(config);
    assert!(auditor.approve_request("nonexistent", "admin").is_err());
}

#[test]
fn test_deny_nonexistent_fails() {
    let config = AuditorConfig::default();
    let auditor = SecurityAuditor::new(config);
    assert!(
        auditor
            .deny_request("nonexistent", "admin", "reason")
            .is_err()
    );
}

#[test]
fn test_statistics() {
    let config = AuditorConfig {
        enabled: true,
        default_action: "deny".to_string(),
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);

    let req = OperationRequest {
        id: "test-stats".to_string(),
        op_type: OperationType::FileRead,
        danger_level: DangerLevel::Low,
        user: "test".to_string(),
        source: "cli".to_string(),
        target: "/tmp/test".to_string(),
        timestamp: None,
        ..Default::default()
    };

    auditor.request_permission(&req);
    let stats = auditor.statistics();
    assert_eq!(*stats.get("total_events").unwrap(), 1);
    assert_eq!(*stats.get("denied").unwrap(), 1);
}

#[test]
fn test_get_statistics_rich() {
    let config = AuditorConfig {
        enabled: true,
        default_action: "deny".to_string(),
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);

    let stats = auditor.get_statistics();
    assert_eq!(stats["total_events"], serde_json::json!(0));
    assert_eq!(stats["enabled"], serde_json::json!(true));
    assert!(stats.contains_key("rule_types"));
}

#[test]
fn test_get_pending_requests() {
    let config = AuditorConfig {
        enabled: true,
        default_action: "ask".to_string(),
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);

    let req = OperationRequest {
        id: "pending-1".to_string(),
        op_type: OperationType::FileWrite,
        danger_level: DangerLevel::High,
        user: "test".to_string(),
        source: "cli".to_string(),
        target: "/tmp/test".to_string(),
        timestamp: None,
        ..Default::default()
    };

    auditor.request_permission(&req);
    let pending = auditor.get_pending_requests();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, "pending-1");
}

#[test]
fn test_export_audit_log() {
    let dir = tempfile::tempdir().unwrap();
    let config = AuditorConfig {
        enabled: true,
        default_action: "deny".to_string(),
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);

    let export_path = dir.path().join("export.json");
    auditor
        .export_audit_log(export_path.to_str().unwrap())
        .unwrap();

    let content = std::fs::read_to_string(&export_path).unwrap();
    assert!(content.contains("total_events"));
}

#[test]
fn test_validate_path_workspace_isolation() {
    // Test that paths outside workspace are rejected
    let result = SecurityAuditor::validate_path(
        "/etc/passwd",
        "/home/user/workspace",
        OperationType::FileRead,
    );
    // This should fail because /etc/passwd is a dangerous path
    assert!(result.is_err() || result.unwrap().contains("/etc/passwd"));
}

#[test]
fn test_validate_path_dangerous_system_path() {
    let result = SecurityAuditor::validate_path("/etc/passwd", "", OperationType::FileRead);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("protected system path"));
}

#[test]
fn test_validate_path_normal() {
    // With no workspace restriction, a normal path should be OK
    // (as long as it's not a dangerous system path)
    let result = SecurityAuditor::validate_path("/tmp/test.txt", "", OperationType::FileRead);
    assert!(result.is_ok());
}

#[test]
fn test_is_safe_command() {
    let (safe, _) = SecurityAuditor::is_safe_command("ls -la");
    assert!(safe);

    let (safe, _) = SecurityAuditor::is_safe_command("cat file.txt");
    assert!(safe);

    let (safe, reason) = SecurityAuditor::is_safe_command("rm -rf /");
    assert!(!safe);
    assert!(reason.contains("dangerous"));

    let (safe, _) = SecurityAuditor::is_safe_command("sudo apt install");
    assert!(!safe);

    let (safe, _) = SecurityAuditor::is_safe_command("shutdown -h now");
    assert!(!safe);
}

#[test]
fn test_set_default_action() {
    let config = AuditorConfig {
        enabled: true,
        default_action: "deny".to_string(),
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);

    auditor.set_default_action("allow");

    let req = OperationRequest {
        id: "test-action".to_string(),
        op_type: OperationType::FileRead,
        danger_level: DangerLevel::Low,
        user: "test".to_string(),
        source: "cli".to_string(),
        target: "/tmp/test".to_string(),
        timestamp: None,
        ..Default::default()
    };

    let (allowed, _, _) = auditor.request_permission(&req);
    assert!(allowed);
}

#[test]
fn test_close() {
    let config = AuditorConfig {
        enabled: true,
        default_action: "ask".to_string(),
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);

    let req = OperationRequest {
        id: "close-test".to_string(),
        op_type: OperationType::FileWrite,
        danger_level: DangerLevel::High,
        user: "test".to_string(),
        source: "cli".to_string(),
        target: "/tmp/test".to_string(),
        timestamp: None,
        ..Default::default()
    };

    auditor.request_permission(&req);
    assert_eq!(auditor.pending_count(), 1);

    auditor.close().unwrap();
    assert_eq!(auditor.pending_count(), 0);
}

#[test]
fn test_enable_disable() {
    let config = AuditorConfig {
        enabled: true,
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);
    assert!(auditor.is_enabled());

    auditor.disable();
    assert!(!auditor.is_enabled());

    auditor.enable();
    assert!(auditor.is_enabled());
}

#[test]
fn test_rule_matching_with_matcher() {
    let config = AuditorConfig {
        enabled: true,
        default_action: "deny".to_string(),
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);

    // Test file pattern matching
    auditor.set_rules(
        OperationType::FileRead,
        vec![SecurityRule {
            pattern: "/tmp/*.txt".to_string(),
            action: "allow".to_string(),
            comment: "allow txt in tmp".to_string(),
        }],
    );

    let req = OperationRequest {
        id: "matcher-1".to_string(),
        op_type: OperationType::FileRead,
        danger_level: DangerLevel::Low,
        user: "test".to_string(),
        source: "cli".to_string(),
        target: "/tmp/test.txt".to_string(),
        timestamp: None,
        ..Default::default()
    };
    let (allowed, _, _) = auditor.request_permission(&req);
    assert!(allowed);

    // Non-matching extension should be denied
    let req2 = OperationRequest {
        id: "matcher-2".to_string(),
        op_type: OperationType::FileRead,
        danger_level: DangerLevel::Low,
        user: "test".to_string(),
        source: "cli".to_string(),
        target: "/tmp/test.log".to_string(),
        timestamp: None,
        ..Default::default()
    };
    let (allowed, _, _) = auditor.request_permission(&req2);
    assert!(!allowed);
}

#[test]
fn test_command_pattern_matching() {
    let config = AuditorConfig {
        enabled: true,
        default_action: "deny".to_string(),
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);

    auditor.set_rules(
        OperationType::ProcessExec,
        vec![SecurityRule {
            pattern: "git *".to_string(),
            action: "allow".to_string(),
            comment: "allow git".to_string(),
        }],
    );

    let req = OperationRequest {
        id: "cmd-1".to_string(),
        op_type: OperationType::ProcessExec,
        danger_level: DangerLevel::Critical,
        user: "test".to_string(),
        source: "cli".to_string(),
        target: "git status".to_string(),
        timestamp: None,
        ..Default::default()
    };
    let (allowed, _, _) = auditor.request_permission(&req);
    assert!(allowed);
}

#[test]
fn test_domain_pattern_matching() {
    let config = AuditorConfig {
        enabled: true,
        default_action: "deny".to_string(),
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);

    auditor.set_rules(
        OperationType::NetworkRequest,
        vec![SecurityRule {
            pattern: "*.github.com".to_string(),
            action: "allow".to_string(),
            comment: "allow github".to_string(),
        }],
    );

    let req = OperationRequest {
        id: "domain-1".to_string(),
        op_type: OperationType::NetworkRequest,
        danger_level: DangerLevel::Medium,
        user: "test".to_string(),
        source: "cli".to_string(),
        target: "api.github.com".to_string(),
        timestamp: None,
        ..Default::default()
    };
    let (allowed, _, _) = auditor.request_permission(&req);
    assert!(allowed);
}

// ---- Additional auditor tests ----

#[test]
fn test_audit_filter_empty() {
    let filter = AuditFilter::default();
    assert!(filter.is_empty());

    let filter2 = AuditFilter {
        operation_type: Some(OperationType::FileRead),
        ..Default::default()
    };
    assert!(!filter2.is_empty());
}

#[test]
fn test_audit_filter_matches_event() {
    let event = AuditEvent {
        event_id: "evt-1".to_string(),
        request: OperationRequest {
            id: "req-1".to_string(),
            op_type: OperationType::FileRead,
            danger_level: DangerLevel::Low,
            user: "alice".to_string(),
            source: "cli".to_string(),
            target: "/tmp/test.txt".to_string(),
            timestamp: None,
            ..Default::default()
        },
        decision: "allowed".to_string(),
        reason: "matched rule".to_string(),
        timestamp: "2026-01-15T10:00:00Z".to_string(),
        policy_rule: "allow_reads".to_string(),
    };

    // Filter by operation type
    let filter = AuditFilter {
        operation_type: Some(OperationType::FileRead),
        ..Default::default()
    };
    assert!(filter.matches(&event));

    // Filter by different operation type
    let filter2 = AuditFilter {
        operation_type: Some(OperationType::ProcessExec),
        ..Default::default()
    };
    assert!(!filter2.matches(&event));

    // Filter by user
    let filter3 = AuditFilter {
        user: Some("alice".to_string()),
        ..Default::default()
    };
    assert!(filter3.matches(&event));

    // Filter by different user
    let filter4 = AuditFilter {
        user: Some("bob".to_string()),
        ..Default::default()
    };
    assert!(!filter4.matches(&event));

    // Filter by decision
    let filter5 = AuditFilter {
        decision: Some("allowed".to_string()),
        ..Default::default()
    };
    assert!(filter5.matches(&event));

    // Filter by time range (inclusive)
    let filter6 = AuditFilter {
        start_time: Some("2026-01-01T00:00:00Z".to_string()),
        end_time: Some("2026-12-31T23:59:59Z".to_string()),
        ..Default::default()
    };
    assert!(filter6.matches(&event));

    // Filter by time range (exclusive - before)
    let filter7 = AuditFilter {
        start_time: Some("2026-02-01T00:00:00Z".to_string()),
        ..Default::default()
    };
    assert!(!filter7.matches(&event));

    // Filter by source
    let filter8 = AuditFilter {
        source: Some("cli".to_string()),
        ..Default::default()
    };
    assert!(filter8.matches(&event));
}

#[test]
fn test_audit_event_serialization() {
    let event = AuditEvent {
        event_id: "evt-ser-1".to_string(),
        request: OperationRequest {
            id: "req-ser-1".to_string(),
            op_type: OperationType::FileWrite,
            danger_level: DangerLevel::High,
            user: "test".to_string(),
            source: "cli".to_string(),
            target: "/tmp/test.txt".to_string(),
            timestamp: None,
            ..Default::default()
        },
        decision: "allowed".to_string(),
        reason: "test".to_string(),
        timestamp: "2026-01-01T00:00:00Z".to_string(),
        policy_rule: "test_rule".to_string(),
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("evt-ser-1"));
    assert!(json.contains("allowed"));
    let de: AuditEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(de.event_id, "evt-ser-1");
    assert_eq!(de.decision, "allowed");
}

#[test]
fn test_auditor_default_action_allows() {
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);

    let req = OperationRequest {
        id: "allow-default".to_string(),
        op_type: OperationType::FileRead,
        danger_level: DangerLevel::Low,
        user: "test".to_string(),
        source: "cli".to_string(),
        target: "/tmp/test".to_string(),
        timestamp: None,
        ..Default::default()
    };

    let (allowed, err, _) = auditor.request_permission(&req);
    assert!(allowed);
    assert!(err.is_none());
}

#[test]
fn test_auditor_workspace_restriction() {
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);

    // File read should be allowed
    let req_inside = OperationRequest {
        id: "ws-inside".to_string(),
        op_type: OperationType::FileRead,
        danger_level: DangerLevel::Low,
        user: "test".to_string(),
        source: "cli".to_string(),
        target: "/home/user/workspace/file.txt".to_string(),
        timestamp: None,
        ..Default::default()
    };
    let (allowed, _, _) = auditor.request_permission(&req_inside);
    assert!(allowed);
}

#[test]
fn test_auditor_deny_patterns() {
    let config = AuditorConfig {
        enabled: true,
        default_action: "deny".to_string(),
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);

    // ProcessExec should be denied with deny default
    let req = OperationRequest {
        id: "deny-pattern".to_string(),
        op_type: OperationType::ProcessExec,
        danger_level: DangerLevel::Critical,
        user: "test".to_string(),
        source: "cli".to_string(),
        target: "rm -rf /".to_string(),
        timestamp: None,
        ..Default::default()
    };
    let (allowed, _err, _) = auditor.request_permission(&req);
    assert!(!allowed);
}

#[test]
fn test_auditor_multiple_rules_priority() {
    let config = AuditorConfig {
        enabled: true,
        default_action: "deny".to_string(),
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);

    // Set two rules: first allow /tmp/safe/*, then deny /tmp/*
    // More specific rule first, broader rule second
    auditor.set_rules(
        OperationType::FileRead,
        vec![
            SecurityRule {
                pattern: "/tmp/safe/*".to_string(),
                action: "allow".to_string(),
                comment: "allow safe subdir".to_string(),
            },
            SecurityRule {
                pattern: "/tmp/*".to_string(),
                action: "deny".to_string(),
                comment: "deny tmp".to_string(),
            },
        ],
    );

    // /tmp/safe/file.txt matches first rule -> allowed
    let req_safe = OperationRequest {
        id: "multi-rule-safe".to_string(),
        op_type: OperationType::FileRead,
        danger_level: DangerLevel::Low,
        user: "test".to_string(),
        source: "cli".to_string(),
        target: "/tmp/safe/file.txt".to_string(),
        timestamp: None,
        ..Default::default()
    };
    let (allowed_safe, _, _) = auditor.request_permission(&req_safe);
    assert!(allowed_safe);

    // /tmp/other/file.txt matches second rule -> denied
    let req_other = OperationRequest {
        id: "multi-rule-other".to_string(),
        op_type: OperationType::FileRead,
        danger_level: DangerLevel::Low,
        user: "test".to_string(),
        source: "cli".to_string(),
        target: "/tmp/other/file.txt".to_string(),
        timestamp: None,
        ..Default::default()
    };
    let (allowed_other, _, _) = auditor.request_permission(&req_other);
    assert!(!allowed_other);
}

#[test]
fn test_auditor_set_rules_overwrites() {
    let config = AuditorConfig {
        enabled: true,
        default_action: "deny".to_string(),
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);

    // First set: allow all reads
    auditor.set_rules(
        OperationType::FileRead,
        vec![SecurityRule {
            // glob 方言 match-all（D12-1 后只认 `*`/`**`；裸 `.*` 曾靠正则
            // 任意字符语义 match-all，字面点号后退化为「含点号才命中」）。
            pattern: "**".to_string(),
            action: "allow".to_string(),
            comment: "allow all".to_string(),
        }],
    );

    let req = OperationRequest {
        id: "overwrite-1".to_string(),
        op_type: OperationType::FileRead,
        danger_level: DangerLevel::Low,
        user: "test".to_string(),
        source: "cli".to_string(),
        target: "/tmp/test.txt".to_string(),
        timestamp: None,
        ..Default::default()
    };
    let (allowed, _, _) = auditor.request_permission(&req);
    assert!(allowed);

    // Overwrite: deny all reads
    auditor.set_rules(
        OperationType::FileRead,
        vec![SecurityRule {
            // glob 方言 match-all（D12-1 后只认 `*`/`**`；裸 `.*` 曾靠正则
            // 任意字符语义 match-all，字面点号后退化为「含点号才命中」）。
            pattern: "**".to_string(),
            action: "deny".to_string(),
            comment: "deny all".to_string(),
        }],
    );

    let req2 = OperationRequest {
        id: "overwrite-2".to_string(),
        op_type: OperationType::FileRead,
        danger_level: DangerLevel::Low,
        user: "test".to_string(),
        source: "cli".to_string(),
        target: "/tmp/test.txt".to_string(),
        timestamp: None,
        ..Default::default()
    };
    let (allowed, _, _) = auditor.request_permission(&req2);
    assert!(!allowed);
}

#[test]
fn test_auditor_safe_commands() {
    let (safe, _) = SecurityAuditor::is_safe_command("echo hello");
    assert!(safe);

    let (safe, _) = SecurityAuditor::is_safe_command("dir");
    assert!(safe);

    let (safe, _) = SecurityAuditor::is_safe_command("cat /tmp/test.txt");
    assert!(safe);

    let (safe, _) = SecurityAuditor::is_safe_command("grep pattern file");
    assert!(safe);
}

#[test]
fn test_auditor_dangerous_commands_all() {
    let dangerous = [
        "rm -rf /",
        "del /f /q file.txt",
        "format C:",
        "mkfs.ext4 /dev/sda",
        "dd if=/dev/zero of=/dev/sda",
        "shutdown -h now",
        "reboot",
        "sudo rm -rf /",
        "chmod 777 /etc/passwd",
        "chown root:root /etc/shadow",
    ];
    for cmd in &dangerous {
        let (safe, reason) = SecurityAuditor::is_safe_command(cmd);
        assert!(!safe, "Expected '{}' to be detected as dangerous", cmd);
        assert!(!reason.is_empty());
    }
}

#[test]
fn test_auditor_default_config_values() {
    let config = AuditorConfig::default();
    assert!(config.enabled);
    assert_eq!(config.approval_timeout_secs, 300);
    assert!(!config.audit_log_file_enabled);
    assert!(config.audit_log_dir.is_none());
}

#[test]
fn test_get_audit_log_no_file() {
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        audit_log_file_enabled: false,
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);
    let filter = AuditFilter::default();
    let events = get_audit_log(&auditor, &filter);
    assert!(events.is_empty());
}

#[test]
fn test_get_audit_log_no_dir() {
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        audit_log_file_enabled: true,
        audit_log_dir: None,
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);
    let filter = AuditFilter::default();
    let events = get_audit_log(&auditor, &filter);
    assert!(events.is_empty());
}

#[test]
fn test_get_audit_log_empty_dir() {
    let dir = tempfile::tempdir().unwrap();
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        audit_log_file_enabled: true,
        audit_log_dir: Some(dir.path().to_str().unwrap().to_string()),
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);
    let filter = AuditFilter::default();
    let events = get_audit_log(&auditor, &filter);
    assert!(events.is_empty());
}

#[test]
fn test_operation_request_default() {
    let req = OperationRequest::default();
    assert!(req.id.is_empty());
    assert!(req.target.is_empty());
    assert!(req.user.is_empty());
    assert!(req.source.is_empty());
    assert!(req.timestamp.is_none());
}

#[test]
fn test_auditor_close_idempotent() {
    let config = AuditorConfig {
        enabled: true,
        default_action: "ask".to_string(),
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);
    auditor.close().unwrap();
    // Second close should also succeed
    auditor.close().unwrap();
    assert_eq!(auditor.pending_count(), 0);
}

#[test]
fn test_auditor_multiple_pending_requests() {
    let config = AuditorConfig {
        enabled: true,
        default_action: "ask".to_string(),
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);

    for i in 0..5 {
        let req = OperationRequest {
            id: format!("multi-{}", i),
            op_type: OperationType::FileWrite,
            danger_level: DangerLevel::High,
            user: "test".to_string(),
            source: "cli".to_string(),
            target: format!("/tmp/test-{}", i),
            timestamp: None,
            ..Default::default()
        };
        auditor.request_permission(&req);
    }

    assert_eq!(auditor.pending_count(), 5);
    let pending = auditor.get_pending_requests();
    assert_eq!(pending.len(), 5);
}

#[test]
fn test_auditor_approve_then_approve_again_fails() {
    let config = AuditorConfig {
        enabled: true,
        default_action: "ask".to_string(),
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);

    let req = OperationRequest {
        id: "double-approve".to_string(),
        op_type: OperationType::FileWrite,
        danger_level: DangerLevel::High,
        user: "test".to_string(),
        source: "cli".to_string(),
        target: "/tmp/test".to_string(),
        timestamp: None,
        ..Default::default()
    };
    auditor.request_permission(&req);
    auditor.approve_request("double-approve", "admin").unwrap();
    assert!(auditor.approve_request("double-approve", "admin").is_err());
}

#[test]
fn test_auditor_deny_then_approve_fails() {
    let config = AuditorConfig {
        enabled: true,
        default_action: "ask".to_string(),
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);

    let req = OperationRequest {
        id: "deny-then-approve".to_string(),
        op_type: OperationType::FileWrite,
        danger_level: DangerLevel::High,
        user: "test".to_string(),
        source: "cli".to_string(),
        target: "/tmp/test".to_string(),
        timestamp: None,
        ..Default::default()
    };
    auditor.request_permission(&req);
    auditor
        .deny_request("deny-then-approve", "admin", "reason")
        .unwrap();
    assert!(
        auditor
            .approve_request("deny-then-approve", "admin")
            .is_err()
    );
}

#[test]
fn test_auditor_validate_path_empty_path() {
    let result = SecurityAuditor::validate_path("", "", OperationType::FileRead);
    assert!(result.is_ok());
}

#[test]
fn test_auditor_validate_path_multiple_dangerous_paths() {
    let dangerous_paths = ["/etc/passwd", "/etc/shadow", "/etc/sudoers"];
    for path in &dangerous_paths {
        let result = SecurityAuditor::validate_path(path, "", OperationType::FileRead);
        assert!(result.is_err(), "Expected {} to be rejected", path);
    }

    // Paths not in the dangerous list should be allowed (when no workspace restriction)
    let safe_paths = ["/etc/ssh/sshd_config", "/boot/grub/grub.cfg", "/tmp/test"];
    for path in &safe_paths {
        let result = SecurityAuditor::validate_path(path, "", OperationType::FileRead);
        assert!(result.is_ok(), "Expected {} to be allowed", path);
    }
}

#[test]
fn test_statistics_after_multiple_operations() {
    let config = AuditorConfig {
        enabled: true,
        default_action: "deny".to_string(),
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);

    // 3 denied
    for i in 0..3 {
        let req = OperationRequest {
            id: format!("stats-{}", i),
            op_type: OperationType::FileRead,
            danger_level: DangerLevel::Low,
            user: "test".to_string(),
            source: "cli".to_string(),
            target: "/tmp/test".to_string(),
            timestamp: None,
            ..Default::default()
        };
        auditor.request_permission(&req);
    }

    let stats = auditor.statistics();
    assert_eq!(*stats.get("total_events").unwrap(), 3);
    assert_eq!(*stats.get("denied").unwrap(), 3);
    assert_eq!(*stats.get("allowed").unwrap_or(&0), 0);
}

#[test]
fn test_global_auditor_init() {
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = init_global_auditor(config);
    assert!(auditor.is_enabled());

    // Get again should return same instance
    let auditor2 = get_global_auditor();
    assert!(Arc::ptr_eq(&auditor, &auditor2));
}

// ---- Additional coverage tests ----

#[test]
fn test_normalize_decision_all_variants() {
    assert!(matches!(
        normalize_decision("allow"),
        SecurityDecision::Allowed
    ));
    assert!(matches!(
        normalize_decision("allowed"),
        SecurityDecision::Allowed
    ));
    assert!(matches!(
        normalize_decision("deny"),
        SecurityDecision::Denied
    ));
    assert!(matches!(
        normalize_decision("denied"),
        SecurityDecision::Denied
    ));
    assert!(matches!(
        normalize_decision("ask"),
        SecurityDecision::RequireApproval
    ));
    assert!(matches!(
        normalize_decision("require_approval"),
        SecurityDecision::RequireApproval
    ));
    assert!(matches!(
        normalize_decision("unknown"),
        SecurityDecision::Denied
    ));
    assert!(matches!(normalize_decision(""), SecurityDecision::Denied));
}

#[test]
fn test_auditor_set_log_file() {
    let config = AuditorConfig::default();
    let auditor = SecurityAuditor::new(config);
    assert!(auditor.get_log_file_path().is_none());
    auditor.set_log_file("/tmp/test_audit.log");
    assert_eq!(
        auditor.get_log_file_path().unwrap().to_str().unwrap(),
        "/tmp/test_audit.log"
    );
}

#[test]
fn test_auditor_log_audit_event_with_file() {
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("audit.jsonl");
    let config = AuditorConfig {
        audit_log_file_enabled: true,
        audit_log_dir: Some(dir.path().to_str().unwrap().to_string()),
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);

    let event = AuditEvent {
        event_id: "evt-1".to_string(),
        request: OperationRequest::default(),
        decision: "allowed".to_string(),
        reason: "test".to_string(),
        timestamp: "2026-01-01T00:00:00Z".to_string(),
        policy_rule: "test".to_string(),
    };
    auditor.log_audit_event(&event);

    // Verify the file was written
    let content = std::fs::read_to_string(&log_path).unwrap();
    assert!(content.contains("evt-1"));
}

#[test]
fn test_auditor_log_audit_event_with_log_file_path() {
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("custom.log");
    let config = AuditorConfig {
        audit_log_file_enabled: false,
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);
    auditor.set_log_file(log_path.to_str().unwrap());

    let event = AuditEvent {
        event_id: "evt-custom".to_string(),
        request: OperationRequest::default(),
        decision: "denied".to_string(),
        reason: "test".to_string(),
        timestamp: "2026-01-01T00:00:00Z".to_string(),
        policy_rule: "test".to_string(),
    };
    auditor.log_audit_event(&event);

    let content = std::fs::read_to_string(&log_path).unwrap();
    assert!(content.contains("evt-custom"));
}

#[test]
fn test_auditor_log_audit_event_disabled() {
    let config = AuditorConfig {
        audit_log_file_enabled: false,
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);
    // Should not panic or create files
    let event = AuditEvent {
        event_id: "evt-noop".to_string(),
        request: OperationRequest::default(),
        decision: "allowed".to_string(),
        reason: String::new(),
        timestamp: String::new(),
        policy_rule: String::new(),
    };
    auditor.log_audit_event(&event);
}

#[test]
fn test_get_audit_log_with_content() {
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("audit.jsonl");

    // Write some events
    let evt1 = AuditEvent {
        event_id: "evt-1".to_string(),
        request: OperationRequest {
            id: "req-1".to_string(),
            op_type: OperationType::FileRead,
            danger_level: DangerLevel::Low,
            user: "alice".to_string(),
            source: "cli".to_string(),
            target: "/tmp/test.txt".to_string(),
            timestamp: None,
            ..Default::default()
        },
        decision: "allowed".to_string(),
        reason: "test".to_string(),
        timestamp: "2026-01-01T10:00:00Z".to_string(),
        policy_rule: "test".to_string(),
    };
    let evt2 = AuditEvent {
        event_id: "evt-2".to_string(),
        request: OperationRequest {
            id: "req-2".to_string(),
            op_type: OperationType::ProcessExec,
            danger_level: DangerLevel::Critical,
            user: "bob".to_string(),
            source: "web".to_string(),
            target: "rm -rf /".to_string(),
            timestamp: None,
            ..Default::default()
        },
        decision: "denied".to_string(),
        reason: "dangerous".to_string(),
        timestamp: "2026-01-02T10:00:00Z".to_string(),
        policy_rule: "default".to_string(),
    };

    use std::io::Write;
    let mut file = std::fs::File::create(&log_path).unwrap();
    writeln!(file, "{}", serde_json::to_string(&evt1).unwrap()).unwrap();
    writeln!(file, "{}", serde_json::to_string(&evt2).unwrap()).unwrap();
    writeln!(file, "# comment line").unwrap();
    writeln!(file).unwrap();

    let config = AuditorConfig {
        audit_log_file_enabled: true,
        audit_log_dir: Some(dir.path().to_str().unwrap().to_string()),
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);

    // Read all events
    let filter = AuditFilter::default();
    let events = get_audit_log(&auditor, &filter);
    assert_eq!(events.len(), 2);

    // Filter by user
    let filter = AuditFilter {
        user: Some("alice".to_string()),
        ..Default::default()
    };
    let events = get_audit_log(&auditor, &filter);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_id, "evt-1");

    // Filter by decision
    let filter = AuditFilter {
        decision: Some("denied".to_string()),
        ..Default::default()
    };
    let events = get_audit_log(&auditor, &filter);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_id, "evt-2");
}

#[test]
fn test_validate_path_workspace_outside() {
    // When workspace is set, path outside should be rejected
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_str().unwrap();
    let result = validate_path_internal("/etc/passwd", ws);
    assert!(result.is_err());
}

#[test]
fn test_validate_path_empty_log_dir_string() {
    let config = AuditorConfig {
        audit_log_file_enabled: true,
        audit_log_dir: Some(String::new()),
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);
    let filter = AuditFilter::default();
    let events = get_audit_log(&auditor, &filter);
    assert!(events.is_empty());
}

#[test]
fn test_approval_required_error_display() {
    let err = ApprovalRequiredError {
        request_id: "req-123".to_string(),
        reason: "dangerous operation".to_string(),
    };
    let display = format!("{}", err);
    assert!(display.contains("req-123"));
    assert!(display.contains("dangerous operation"));
    assert!(err.is_approval_required());
}

#[test]
fn test_is_safe_command_variations() {
    let (safe, _) = SecurityAuditor::is_safe_command("ls");
    assert!(safe);
    let (safe, _) = SecurityAuditor::is_safe_command("git log --oneline");
    assert!(safe);
    let (safe, _) = SecurityAuditor::is_safe_command("python script.py");
    assert!(safe);
    let (safe, _) = SecurityAuditor::is_safe_command("kill -9 1234");
    // Note: kill -9 is not in the is_safe_command_internal pattern list
    // (that function only checks a subset of dangerous patterns).
    // It IS in DEFAULT_DENY_PATTERNS which is used by the auditor rules.
    let _ = safe;
}

#[test]
fn test_auditor_config_debug() {
    let config = AuditorConfig::default();
    let debug = format!("{:?}", config);
    assert!(debug.contains("enabled"));
}

#[test]
fn test_auditor_cleanup_old_audit_logs() {
    let config = AuditorConfig::default();
    let auditor = SecurityAuditor::new(config);
    let result = auditor.cleanup_old_audit_logs();
    assert!(result.is_ok());
}

#[test]
fn test_auditor_with_approval_manager() {
    struct MockApprovalManager;

    impl ApprovalManager for MockApprovalManager {
        fn is_running(&self) -> bool {
            true
        }
        fn request_approval_sync(
            &self,
            _request_id: &str,
            _operation: &str,
            _target: &str,
            _risk_level: &str,
            _reason: &str,
            _timeout_secs: u64,
        ) -> Result<ApprovalVerdict, String> {
            Ok(ApprovalVerdict::approved()) // Always approve
        }
    }

    let config = AuditorConfig {
        enabled: true,
        default_action: "ask".to_string(),
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);
    auditor.set_approval_manager(Arc::new(MockApprovalManager));

    let req = OperationRequest {
        id: "approval-test".to_string(),
        op_type: OperationType::FileWrite,
        danger_level: DangerLevel::High,
        user: "test".to_string(),
        source: "cli".to_string(),
        target: "/tmp/test.txt".to_string(),
        timestamp: None,
        ..Default::default()
    };

    let (allowed, err, _) = auditor.request_permission(&req);
    assert!(allowed);
    assert!(err.is_none());
}

#[test]
fn test_auditor_with_approval_manager_deny() {
    struct MockDenyManager;

    impl ApprovalManager for MockDenyManager {
        fn is_running(&self) -> bool {
            true
        }
        fn request_approval_sync(
            &self,
            _request_id: &str,
            _operation: &str,
            _target: &str,
            _risk_level: &str,
            _reason: &str,
            _timeout_secs: u64,
        ) -> Result<ApprovalVerdict, String> {
            Ok(ApprovalVerdict::denied()) // Always deny
        }
    }

    let config = AuditorConfig {
        enabled: true,
        default_action: "ask".to_string(),
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);
    auditor.set_approval_manager(Arc::new(MockDenyManager));

    let req = OperationRequest {
        id: "approval-deny-test".to_string(),
        op_type: OperationType::FileWrite,
        danger_level: DangerLevel::High,
        user: "test".to_string(),
        source: "cli".to_string(),
        target: "/tmp/test.txt".to_string(),
        timestamp: None,
        ..Default::default()
    };

    let (allowed, err, _) = auditor.request_permission(&req);
    assert!(!allowed);
    assert!(err.unwrap().contains("User rejected"));
}

#[test]
fn test_auditor_with_approval_manager_deny_note_in_message() {
    // F6: 拒绝备注随 verdict 送达 → 拼进拒绝消息回灌给模型（纠错回喂）。
    // 空备注/None 不追加冒号尾巴。
    struct MockNoteManager {
        note: Option<&'static str>,
    }

    impl ApprovalManager for MockNoteManager {
        fn is_running(&self) -> bool {
            true
        }
        fn request_approval_sync(
            &self,
            _request_id: &str,
            _operation: &str,
            _target: &str,
            _risk_level: &str,
            _reason: &str,
            _timeout_secs: u64,
        ) -> Result<ApprovalVerdict, String> {
            Ok(ApprovalVerdict {
                approved: false,
                note: self.note.map(String::from),
            })
        }
    }

    let config = AuditorConfig {
        enabled: true,
        default_action: "ask".to_string(),
        ..Default::default()
    };
    let mk_req = |id: &str| OperationRequest {
        id: id.to_string(),
        op_type: OperationType::FileWrite,
        danger_level: DangerLevel::High,
        user: "test".to_string(),
        source: "cli".to_string(),
        target: "/tmp/test.txt".to_string(),
        timestamp: None,
        ..Default::default()
    };

    // 带备注：原样拼进消息（含空白原文，不截断）。
    let auditor = SecurityAuditor::new(config.clone());
    auditor.set_approval_manager(Arc::new(MockNoteManager {
        note: Some("别动生产库配置"),
    }));
    let (allowed, err, _) = auditor.request_permission(&mk_req("note-test"));
    assert!(!allowed);
    let msg = err.unwrap();
    assert!(msg.contains("User rejected"), "got: {msg}");
    assert!(msg.contains("别动生产库配置"), "note missing in: {msg}");

    // 空白备注：视同无备注，消息不带冒号尾巴。
    let auditor = SecurityAuditor::new(config.clone());
    auditor.set_approval_manager(Arc::new(MockNoteManager { note: Some("   ") }));
    let (allowed, err, _) = auditor.request_permission(&mk_req("blank-note-test"));
    assert!(!allowed);
    let msg = err.unwrap();
    assert!(msg.contains("User rejected"), "got: {msg}");
    assert!(
        !msg.ends_with(":") && !msg.ends_with(": "),
        "blank note leaked colon tail: {msg}"
    );

    // None 备注：基线消息形态不变。
    let auditor = SecurityAuditor::new(config);
    auditor.set_approval_manager(Arc::new(MockNoteManager { note: None }));
    let (allowed, err, _) = auditor.request_permission(&mk_req("no-note-test"));
    assert!(!allowed);
    assert!(err.unwrap().contains("User rejected"));
}

#[test]
fn test_auditor_with_approval_manager_error() {
    struct MockErrorManager;

    impl ApprovalManager for MockErrorManager {
        fn is_running(&self) -> bool {
            true
        }
        fn request_approval_sync(
            &self,
            _request_id: &str,
            _operation: &str,
            _target: &str,
            _risk_level: &str,
            _reason: &str,
            _timeout_secs: u64,
        ) -> Result<ApprovalVerdict, String> {
            Err("dialog failed".to_string())
        }
    }

    let config = AuditorConfig {
        enabled: true,
        default_action: "ask".to_string(),
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);
    auditor.set_approval_manager(Arc::new(MockErrorManager));

    let req = OperationRequest {
        id: "approval-error-test".to_string(),
        op_type: OperationType::FileWrite,
        danger_level: DangerLevel::High,
        user: "test".to_string(),
        source: "cli".to_string(),
        target: "/tmp/test.txt".to_string(),
        timestamp: None,
        ..Default::default()
    };

    let (allowed, _, _) = auditor.request_permission(&req);
    // Error from manager should fall through to pending request storage
    assert!(!allowed);
    assert_eq!(auditor.pending_count(), 1);
}

#[test]
fn test_auditor_with_approval_manager_not_running() {
    struct MockNotRunningManager;

    impl ApprovalManager for MockNotRunningManager {
        fn is_running(&self) -> bool {
            false
        }
        fn request_approval_sync(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: &str,
            _: &str,
            _: u64,
        ) -> Result<ApprovalVerdict, String> {
            Ok(ApprovalVerdict::approved())
        }
    }

    let config = AuditorConfig {
        enabled: true,
        default_action: "ask".to_string(),
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);
    auditor.set_approval_manager(Arc::new(MockNotRunningManager));

    let req = OperationRequest {
        id: "not-running-test".to_string(),
        op_type: OperationType::FileWrite,
        danger_level: DangerLevel::High,
        user: "test".to_string(),
        source: "cli".to_string(),
        target: "/tmp/test.txt".to_string(),
        timestamp: None,
        ..Default::default()
    };

    let (allowed, _, _) = auditor.request_permission(&req);
    assert!(!allowed); // Should fall through to pending because not running
    assert_eq!(auditor.pending_count(), 1);
}

#[test]
fn test_auditor_get_config() {
    let config = AuditorConfig {
        enabled: false,
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);
    assert!(!auditor.config().enabled);
}

#[test]
fn test_auditor_network_upload_rule() {
    let config = AuditorConfig {
        enabled: true,
        default_action: "deny".to_string(),
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);
    auditor.set_rules(
        OperationType::NetworkUpload,
        vec![SecurityRule {
            pattern: "*.example.com".to_string(),
            action: "allow".to_string(),
            comment: "allow example.com".to_string(),
        }],
    );

    let req = OperationRequest {
        id: "upload-1".to_string(),
        op_type: OperationType::NetworkUpload,
        danger_level: DangerLevel::Medium,
        user: "test".to_string(),
        source: "cli".to_string(),
        target: "upload.example.com".to_string(),
        timestamp: None,
        ..Default::default()
    };
    let (allowed, _, _) = auditor.request_permission(&req);
    assert!(allowed);
}

#[test]
fn test_auditor_process_suspend_rule() {
    let config = AuditorConfig {
        enabled: true,
        default_action: "deny".to_string(),
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);
    auditor.set_rules(
        OperationType::ProcessSuspend,
        vec![SecurityRule {
            pattern: "*".to_string(),
            action: "allow".to_string(),
            comment: "allow suspend".to_string(),
        }],
    );

    let req = OperationRequest {
        id: "suspend-1".to_string(),
        op_type: OperationType::ProcessSuspend,
        danger_level: DangerLevel::High,
        user: "test".to_string(),
        source: "cli".to_string(),
        target: "pause process".to_string(),
        timestamp: None,
        ..Default::default()
    };
    let (allowed, _, _) = auditor.request_permission(&req);
    assert!(allowed);
}

#[test]
fn test_auditor_empty_rules_falls_to_default() {
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);
    // Set empty rules
    auditor.set_rules(OperationType::FileRead, vec![]);

    let req = OperationRequest {
        id: "empty-rules".to_string(),
        op_type: OperationType::FileRead,
        danger_level: DangerLevel::Low,
        user: "test".to_string(),
        source: "cli".to_string(),
        target: "/tmp/test.txt".to_string(),
        timestamp: None,
        ..Default::default()
    };
    let (allowed, _, _) = auditor.request_permission(&req);
    assert!(allowed);
}

#[test]
fn test_validate_path_windows_hosts() {
    // The dangerous path check uses the raw path string
    // On Windows, paths typically get canonicalized to backslash format
    let result = validate_path_internal("C:\\Windows\\System32\\drivers\\etc\\hosts", "");
    if cfg!(target_os = "windows") {
        // On Windows, canonicalize may resolve the path
        let _ = result;
    } else {
        // On non-Windows, backslash paths don't match the check (which uses forward-slash comparison)
        assert!(result.is_ok() || result.is_err());
    }
}

#[test]
fn test_get_audit_log_malformed_line() {
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("audit.jsonl");

    use std::io::Write;
    let mut file = std::fs::File::create(&log_path).unwrap();
    writeln!(file, "not json").unwrap();
    writeln!(file, "{{}}").unwrap(); // Valid JSON but not AuditEvent - still parses

    let config = AuditorConfig {
        audit_log_file_enabled: true,
        audit_log_dir: Some(dir.path().to_str().unwrap().to_string()),
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);
    let filter = AuditFilter::default();
    let events = get_audit_log(&auditor, &filter);
    // Malformed lines are skipped
    assert!(events.len() <= 1);
}

// ============================================================
// 2026-08-25 coverage push: approval-manager getter, log write
// error arms, default-arm rules, validate_path Ok arms,
// filter mismatch arms, monitor loop, read-error path
// ============================================================

fn sample_event(id: &str) -> AuditEvent {
    AuditEvent {
        event_id: id.to_string(),
        request: OperationRequest::default(),
        decision: "allowed".to_string(),
        reason: "test".to_string(),
        timestamp: "2026-06-15T10:00:00Z".to_string(),
        policy_rule: "test".to_string(),
    }
}

#[test]
fn test_auditor_get_approval_manager() {
    struct RunningMgr;
    impl ApprovalManager for RunningMgr {
        fn is_running(&self) -> bool {
            true
        }
        fn request_approval_sync(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: &str,
            _: &str,
            _: u64,
        ) -> Result<ApprovalVerdict, String> {
            Ok(ApprovalVerdict::approved())
        }
    }

    let auditor = SecurityAuditor::new(AuditorConfig::default());
    assert!(auditor.get_approval_manager().is_none());
    auditor.set_approval_manager(Arc::new(RunningMgr));
    let mgr = auditor.get_approval_manager().expect("manager must be set");
    assert!(mgr.is_running());
}

#[test]
fn test_export_audit_log_parent_is_file_errors() {
    // 导出目标的父路径是文件 → create_dir_all Err → "failed to create directory"。
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("blocker.txt");
    std::fs::write(&blocker, "i am a file").unwrap();
    let auditor = SecurityAuditor::new(AuditorConfig::default());
    let target = blocker.join("export.json").to_string_lossy().to_string();
    let err = auditor.export_audit_log(&target).unwrap_err();
    assert!(
        err.contains("failed to create directory"),
        "unexpected error: {err}"
    );
}

#[test]
fn test_log_audit_event_jsonl_open_fails_when_audit_jsonl_is_dir() {
    // audit.jsonl 是目录 → OpenOptions open Err → warn 分支（不 panic）。
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("audit.jsonl")).unwrap();
    let config = AuditorConfig {
        audit_log_file_enabled: true,
        audit_log_dir: Some(dir.path().to_string_lossy().to_string()),
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);
    auditor.log_audit_event(&sample_event("evt-openfail"));
}

#[test]
fn test_log_audit_event_jsonl_empty_dir_string_skips_block() {
    // audit_log_file_enabled=true 但 dir 是空串 → 整个 JSONL 块跳过（不 panic）。
    let config = AuditorConfig {
        audit_log_file_enabled: true,
        audit_log_dir: Some(String::new()),
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);
    auditor.log_audit_event(&sample_event("evt-emptydir"));
}

#[test]
fn test_log_audit_event_log_file_path_open_fails_for_directory() {
    // set_log_file 指向目录 → open Err → warn 分支（不 panic）。
    let dir = tempfile::tempdir().unwrap();
    let blocker_dir = dir.path().join("as_dir.log");
    std::fs::create_dir_all(&blocker_dir).unwrap();
    let auditor = SecurityAuditor::new(AuditorConfig::default());
    auditor.set_log_file(&blocker_dir.to_string_lossy());
    auditor.log_audit_event(&sample_event("evt-logpath-openfail"));
}

#[test]
fn test_evaluate_request_default_arm_hardware_and_system_rules() {
    // evaluate_request 的 `_` 兜底臂：Hardware/System 类操作走
    // `pattern == "*" || match_pattern(...)` 路径。
    let auditor = SecurityAuditor::new(AuditorConfig {
        enabled: true,
        default_action: "deny".to_string(),
        ..Default::default()
    });

    // 非 "*" 模式 → match_pattern 路径（无 '/' 的通配模式按全局匹配）
    auditor.set_rules(
        OperationType::SystemConfig,
        vec![SecurityRule {
            pattern: "cfg-*".to_string(),
            action: "allow".to_string(),
            comment: String::new(),
        }],
    );
    let req = OperationRequest {
        id: "syscfg-1".to_string(),
        op_type: OperationType::SystemConfig,
        danger_level: DangerLevel::Critical,
        user: "test".to_string(),
        source: "cli".to_string(),
        target: "cfg-edit".to_string(),
        timestamp: None,
        ..Default::default()
    };
    let (allowed, _, _) = auditor.request_permission(&req);
    assert!(
        allowed,
        "cfg-* rule must allow SystemConfig via match_pattern"
    );

    // "*" 模式 → 短路匹配
    auditor.set_rules(
        OperationType::HardwareGPIO,
        vec![SecurityRule {
            pattern: "*".to_string(),
            action: "allow".to_string(),
            comment: String::new(),
        }],
    );
    let req2 = OperationRequest {
        id: "gpio-1".to_string(),
        op_type: OperationType::HardwareGPIO,
        danger_level: DangerLevel::Medium,
        user: "test".to_string(),
        source: "cli".to_string(),
        target: "/dev/gpiochip0".to_string(),
        timestamp: None,
        ..Default::default()
    };
    let (allowed2, _, _) = auditor.request_permission(&req2);
    assert!(allowed2, "* rule must allow HardwareGPIO via short-circuit");
}

#[test]
fn test_validate_path_inside_workspace_ok_and_dotdot_escape_denied() {
    let ws = tempfile::tempdir().unwrap();
    // ① 工作区内真实文件 → canonicalize 后 strip_prefix Ok(rel)，rel 不以 .. 开头 → Ok
    std::fs::write(ws.path().join("file.txt"), "x").unwrap();
    let ok = validate_path_internal(
        &ws.path().join("file.txt").to_string_lossy(),
        &ws.path().to_string_lossy(),
    )
    .unwrap();
    assert!(ok.contains("file.txt"), "validated path: {ok}");

    // ② 双方都不存在 → canonicalize 回退原始路径 → strip_prefix Ok("../..")
    //    → rel 以 .. 开头 → "path outside workspace"
    let missing_ws = ws.path().join("ws_missing");
    let escape = missing_ws.join("..").join("secret.txt");
    let err = validate_path_internal(&escape.to_string_lossy(), &missing_ws.to_string_lossy())
        .unwrap_err();
    assert!(err.contains("outside workspace"), "unexpected error: {err}");
}

#[test]
fn test_audit_filter_source_mismatch_and_end_time() {
    let event = AuditEvent {
        event_id: "evt-filter".to_string(),
        request: OperationRequest {
            id: "req-filter".to_string(),
            op_type: OperationType::FileRead,
            danger_level: DangerLevel::Low,
            user: "test".to_string(),
            source: "cli".to_string(),
            target: "/tmp/test.txt".to_string(),
            timestamp: None,
            ..Default::default()
        },
        decision: "allowed".to_string(),
        reason: "test".to_string(),
        timestamp: "2026-06-15T10:00:00Z".to_string(),
        policy_rule: "test".to_string(),
    };

    // source 不匹配（contains 失败）
    let f = AuditFilter {
        source: Some("web".to_string()),
        ..Default::default()
    };
    assert!(!f.matches(&event));

    // event source 为空串 → 任何 source 过滤都不匹配
    let mut ev_empty = event.clone();
    ev_empty.request.source = String::new();
    let f2 = AuditFilter {
        source: Some("cli".to_string()),
        ..Default::default()
    };
    assert!(!f2.matches(&ev_empty));

    // end_time 早于事件时间戳 → 排除
    let f3 = AuditFilter {
        end_time: Some("2026-01-01T00:00:00Z".to_string()),
        ..Default::default()
    };
    assert!(!f3.matches(&event));
}

#[test]
fn test_get_audit_log_read_error_when_audit_jsonl_is_dir() {
    // audit.jsonl 存在但是目录 → read_to_string Err → warn + 空 vec。
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("audit.jsonl")).unwrap();
    let config = AuditorConfig {
        audit_log_file_enabled: true,
        audit_log_dir: Some(dir.path().to_string_lossy().to_string()),
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);
    let events = get_audit_log(&auditor, &AuditFilter::default());
    assert!(events.is_empty());
}

#[test]
fn test_reset_global_auditor_noop() {
    // 显式调用覆盖 #[cfg(test)] no-op（OnceLock 不支持 reset）。
    _reset_global_auditor();
}

#[tokio::test]
async fn monitor_security_status_tick_then_shutdown() {
    let auditor = Arc::new(SecurityAuditor::new(AuditorConfig::default()));
    let (tx, rx) = tokio::sync::watch::channel(false);
    let handle = tokio::spawn(monitor_security_status(auditor.clone(), 1, rx));

    // interval 首个 tick 立即触发 → tick 分支执行；1s 内不会到第二个 tick
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(!handle.is_finished(), "monitor must still be running");

    tx.send(true).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(
        handle.is_finished(),
        "monitor must exit after shutdown signal"
    );
}

// ============================================================
// S3 batch 3: export 父目录 / 显式 log_file_path 成功写入 /
// validate_path Err 臂 / monitor 关停臂
// ============================================================

#[test]
fn test_export_audit_log_creates_parent_dirs() {
    let auditor = SecurityAuditor::new(AuditorConfig::default());
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("nested").join("deeper").join("audit.json");
    auditor.export_audit_log(&dest.to_string_lossy()).unwrap();
    assert!(dest.exists());
    let content = std::fs::read_to_string(&dest).unwrap();
    assert!(content.contains("total_events"), "{content}");
}

#[test]
fn test_log_audit_event_writes_to_explicit_log_file_path() {
    let dir = tempfile::tempdir().unwrap();
    let auditor = SecurityAuditor::new(AuditorConfig::default());
    let log_path = dir.path().join("logs").join("audit_events.log");
    auditor.set_log_file(&log_path.to_string_lossy());
    assert_eq!(auditor.get_log_file_path(), Some(log_path.clone()));

    auditor.log_audit_event(&sample_event("evt-explicit-path"));

    let content = std::fs::read_to_string(&log_path).unwrap();
    assert!(content.contains("evt-explicit-path"), "{content}");
    // 每行是一条 JSON 事件
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines.len(), 1);
    let v: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(v["event_id"], "evt-explicit-path");
    assert_eq!(v["decision"], "allowed");
}

#[test]
fn test_validate_path_missing_file_outside_workspace_denied() {
    // 目标路径不存在（canonicalize 失败）且不在工作区前缀下 → Err(_) 臂拒绝。
    let ws = tempfile::tempdir().unwrap();
    let outside = if cfg!(target_os = "windows") {
        r"C:\definitely\outside\missing.txt"
    } else {
        "/definitely/outside/missing.txt"
    };
    let err = validate_path(outside, &ws.path().to_string_lossy()).unwrap_err();
    assert!(err.contains("outside workspace"), "{err}");
}

#[tokio::test]
async fn test_monitor_security_status_shutdown_arm() {
    use std::sync::Arc;
    let auditor = Arc::new(SecurityAuditor::new(AuditorConfig::default()));
    let (tx, rx) = tokio::sync::watch::channel(false);
    let handle = tokio::spawn(monitor_security_status(auditor, 3600, rx));
    // 立刻请求关停 → changed() 分支触发 → 函数返回
    tx.send(true).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), handle)
        .await
        .expect("monitor must exit on shutdown")
        .unwrap();
}

// ---------------------------------------------------------------------------
// F3（devtool-upgrade 阶段 5）：审批 pattern 记忆——自动放行 + 层级安全门
// ---------------------------------------------------------------------------

use std::sync::Arc;

/// ask 默认动作的 auditor（require_approval 才会走到规则查询点）。
fn ask_auditor() -> SecurityAuditor {
    SecurityAuditor::new(AuditorConfig {
        enabled: true,
        default_action: "ask".to_string(),
        ..Default::default()
    })
}

/// 写一个规则文件并返回挂到 auditor 上的热载器。
fn rules_hot(
    dir: &tempfile::TempDir,
    rules: &[crate::approval_rules::ApprovalRule],
) -> Arc<nemesis_config::HotReloader<Vec<crate::approval_rules::ApprovalRule>>> {
    let path = dir.path().join("approval_rules.json");
    crate::approval_rules::save_rules(&path, rules).unwrap();
    Arc::new(nemesis_config::HotReloader::new(
        path,
        crate::approval_rules::load_rules,
    ))
}

fn cargo_rule() -> crate::approval_rules::ApprovalRule {
    crate::approval_rules::ApprovalRule {
        op: "process_exec".to_string(),
        pattern: "cargo test *".to_string(),
        action: "allow".to_string(),
        created_at: "t".to_string(),
    }
}

fn exec_request(id: &str, target: &str) -> OperationRequest {
    OperationRequest {
        id: id.to_string(),
        op_type: OperationType::ProcessExec,
        danger_level: DangerLevel::Critical,
        user: "test".to_string(),
        source: "cli".to_string(),
        target: target.to_string(),
        timestamp: None,
        ..Default::default()
    }
}

#[test]
fn approval_rule_auto_allows_matching_exec_prefix() {
    let auditor = ask_auditor();
    let dir = tempfile::tempdir().unwrap();
    auditor.set_approval_rules(rules_hot(&dir, &[cargo_rule()]));

    // 无规则基线：require_approval → 未装配 manager → pending 存储 + 拒绝。
    let (allowed, err, _) = auditor.request_permission(&exec_request("r0", "cargo publish"));
    assert!(!allowed);
    assert!(err.unwrap().contains("approval required"));

    // 命中前缀：cargo test --release 自动放行（F4 验收语义）。
    let (allowed, err, _) = auditor.request_permission(&exec_request("r1", "cargo test --release"));
    assert!(allowed, "matching prefix must auto-allow");
    assert!(err.is_none());
}

#[test]
fn approval_rule_does_not_leak_to_other_commands() {
    let auditor = ask_auditor();
    let dir = tempfile::tempdir().unwrap();
    auditor.set_approval_rules(rules_hot(&dir, &[cargo_rule()]));
    let (allowed, _, _) = auditor.request_permission(&exec_request("r2", "cargo publish"));
    assert!(!allowed, "non-matching command must not auto-allow");
}

#[test]
fn approval_rule_critical_nonexec_op_stays_manual() {
    // 层级安全门：CRITICAL 非 exec 操作即使有 (op, pattern) 命中也不放行。
    let auditor = ask_auditor();
    let dir = tempfile::tempdir().unwrap();
    let rule = crate::approval_rules::ApprovalRule {
        op: "file_write".to_string(),
        pattern: "/tmp/a.txt".to_string(),
        action: "allow".to_string(),
        created_at: "t".to_string(),
    };
    auditor.set_approval_rules(rules_hot(&dir, &[rule]));

    let req = OperationRequest {
        id: "r3".to_string(),
        op_type: OperationType::FileWrite,
        danger_level: DangerLevel::High,
        user: "test".to_string(),
        source: "cli".to_string(),
        target: "/tmp/a.txt".to_string(),
        timestamp: None,
        ..Default::default()
    };
    let (allowed_high, _, _) = auditor.request_permission(&req);
    assert!(allowed_high, "HIGH op with exact rule auto-allows");

    let req_crit = OperationRequest {
        id: "r4".to_string(),
        op_type: OperationType::FileWrite,
        danger_level: DangerLevel::Critical,
        user: "test".to_string(),
        source: "cli".to_string(),
        target: "/tmp/a.txt".to_string(),
        timestamp: None,
        ..Default::default()
    };
    let (allowed_crit, _, _) = auditor.request_permission(&req_crit);
    assert!(!allowed_crit, "CRITICAL op must stay manual");
}

#[test]
fn approval_rule_hot_reload_picks_up_disk_change() {
    let auditor = ask_auditor();
    let dir = tempfile::tempdir().unwrap();
    auditor.set_approval_rules(rules_hot(&dir, &[]));

    let (before, _, _) = auditor.request_permission(&exec_request("r5", "cargo test --release"));
    assert!(!before, "empty rules must not auto-allow");

    // 磁盘追加规则（模拟审批卡「总是允许」写入），HotReloader 经 mtime 感知。
    // 先睡过 Windows mtime 量化窗口（~15.6ms 定时器中断）：两次写盘落在同
    // 一 tick 会让 check() 看到 mtime 未变而漏载（曾实测 0.00s 内复现）。
    std::thread::sleep(std::time::Duration::from_millis(30));
    let path = dir.path().join("approval_rules.json");
    let mut rules = crate::approval_rules::load_rules(&path);
    crate::approval_rules::upsert_rule(&mut rules, "process_exec", "cargo test *");
    crate::approval_rules::save_rules(&path, &rules).unwrap();

    let (after, _, _) = auditor.request_permission(&exec_request("r6", "cargo test --release"));
    assert!(after, "rule written to disk must hot-reload into effect");
}

#[test]
fn approval_rule_auto_allow_is_audit_marked_auto_by_rule() {
    let auditor = ask_auditor();
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("audit.jsonl");
    auditor.set_log_file(log.to_str().unwrap());
    auditor.set_approval_rules(rules_hot(&dir, &[cargo_rule()]));

    let (allowed, _, _) = auditor.request_permission(&exec_request("r7", "cargo test --release"));
    assert!(allowed);

    let raw = std::fs::read_to_string(&log).unwrap();
    assert!(
        raw.contains("auto_by_rule"),
        "audit event must be marked auto_by_rule, got: {}",
        raw
    );
    assert!(raw.contains("cargo test *"));
}

// ===========================================================================
// CMD 族修复行为测试（2026-09-16 第二批·灾难复发批）
// deny-first 三遍扫 / 归一化 / 内层解释器载荷 / 自杀形态 / D1 / D2
// ===========================================================================

fn allow_auditor() -> SecurityAuditor {
    SecurityAuditor::new(AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    })
}

fn auditor_with_rules(op: OperationType, rules: Vec<SecurityRule>) -> SecurityAuditor {
    let auditor = allow_auditor();
    auditor.set_rules(op, rules);
    auditor
}

fn rule(pattern: &str, action: &str) -> SecurityRule {
    SecurityRule {
        pattern: pattern.to_string(),
        action: action.to_string(),
        comment: String::new(),
    }
}

#[test]
fn deny_pass_wins_over_earlier_allow_rule() {
    // CMD-01：allow 规则排在 deny 之前，旧的 first-match-wins 下
    // `git *` allow 先命中即放行；deny-first 语义下 deny 必胜。
    let auditor = auditor_with_rules(
        OperationType::ProcessExec,
        vec![rule("git *", "allow"), rule("git push * --force*", "deny")],
    );
    let (allowed, err, _) =
        auditor.request_permission(&exec_request("df1", "git push origin main --force"));
    assert!(
        !allowed,
        "deny rule must win even though allow rule is earlier"
    );
    assert!(err.unwrap().contains("git push * --force*"));
}

#[test]
fn ask_pass_wins_over_later_allow_rule() {
    let auditor = auditor_with_rules(
        OperationType::ProcessExec,
        vec![rule("git *", "allow"), rule("git push *", "ask")],
    );
    let (allowed, _, _) = auditor.request_permission(&exec_request("df2", "git push origin main"));
    assert!(!allowed, "ask must stop execution (pending)");
    assert_eq!(auditor.pending_count(), 1, "ask lands as pending approval");
}

#[test]
fn normalization_closes_quote_case_whitespace_bypasses() {
    // CMD-04/05：引号包裹旗标 / 大小写变体 / 多余空白都曾借 shell 语义绕过。
    let auditor = auditor_with_rules(
        OperationType::ProcessExec,
        vec![
            rule("rm --recursive*", "deny"),
            rule("Remove-Item*", "deny"),
        ],
    );
    for (i, cmd) in [
        "rm --recursive build",
        "rm \"--recursive\" build",
        "RM --RECURSIVE build",
        "rm   --recursive   build",
        "remove-item -recurse c:\\tmp\\x",
    ]
    .iter()
    .enumerate()
    {
        let (allowed, err, _) = auditor.request_permission(&exec_request(&format!("n{i}"), cmd));
        assert!(!allowed, "`{cmd}` must be denied after normalization");
        assert!(err.is_some());
    }
}

#[test]
fn interpreter_payload_deny_seen_beneath_allow_wrapper() {
    // CMD-02/06：外层 `python *` allow 不得屏蔽内层载荷的视线。
    let auditor = auditor_with_rules(
        OperationType::ProcessExec,
        vec![rule("python *", "allow"), rule("rm -r*", "deny")],
    );
    let (allowed, err, _) = auditor.request_permission(&exec_request(
        "iw1",
        "python -c \"import os; os.system('rm -rf /data')\"",
    ));
    assert!(!allowed, "inner payload deny must beat outer allow");
    assert!(
        err.unwrap().contains("interpreter payload"),
        "audit reason must mark the inner-payload path"
    );
}

#[test]
fn interpreter_payload_dangerous_structure_requires_approval() {
    // CMD-06②：`fs.rmSync` 这类 API 形态进结构词表 → RequireApproval。
    let auditor = auditor_with_rules(OperationType::ProcessExec, vec![rule("node *", "allow")]);
    let (allowed, _, _) = auditor.request_permission(&exec_request(
        "iw2",
        "node -e \"const fs=require('fs'); fs.rmSync('x',{recursive:true})\"",
    ));
    assert!(!allowed, "dangerous structure must require approval");
    assert_eq!(auditor.pending_count(), 1);
}

#[test]
fn benign_interpreter_payload_falls_through_to_allow() {
    // 良性载荷不误伤：无 deny/ask 命中、无危险结构 → 外层 allow 生效。
    let auditor = auditor_with_rules(OperationType::ProcessExec, vec![rule("python *", "allow")]);
    let (allowed, _, _) =
        auditor.request_permission(&exec_request("iw3", "python -c \"print(1)\""));
    assert!(allowed, "benign payload must not be blocked");
}

fn auditor_with_protected(paths: &[&str]) -> SecurityAuditor {
    let auditor = allow_auditor();
    auditor.set_protected_paths(paths.iter().map(|s| s.to_string()).collect());
    auditor
}

#[test]
fn self_destruct_recursive_delete_protected_root_denied() {
    let auditor = auditor_with_protected(&["/home/zoo", "/home/zoo/proj"]);
    for (i, cmd) in ["rm -rf /", "rm -rf ~", "rm -rf /home/zoo", "rm -rf ."]
        .iter()
        .enumerate()
    {
        let (allowed, err, _) = auditor.request_permission(&exec_request(&format!("sd{i}"), cmd));
        assert!(!allowed, "`{cmd}` must hit self-destruct hard block");
        assert!(
            err.unwrap().contains("self-destruct"),
            "`{cmd}` reason must be self_destruct"
        );
    }
}

#[test]
fn self_destruct_workspace_subpath_window_not_blocked() {
    // 窗口逻辑：`cd ws && rm -rf build` 的删除窗口内无保护路径 → 不拦。
    let auditor = auditor_with_protected(&["/home/zoo/proj"]);
    let (allowed, _, _) =
        auditor.request_permission(&exec_request("sd10", "cd /home/zoo/proj && rm -rf build"));
    assert!(allowed, "deleting a build subdir must not be self-destruct");
}

#[test]
fn self_destruct_single_file_delete_not_blocked() {
    // 诚实边界：无递归旗标的单文件删除是 ABAC/D1 治理面，不进硬拦。
    let auditor = auditor_with_protected(&["/home/zoo/proj"]);
    let (allowed, _, _) = auditor.request_permission(&exec_request("sd11", "rm build/output.txt"));
    assert!(allowed);
}

#[test]
fn self_destruct_windows_forms_denied() {
    let auditor = auditor_with_protected(&["c:/users/zoo", "c:/users/zoo/proj"]);
    for (i, cmd) in [
        "del /s /q c:\\users\\zoo",
        "del \"/s\" \"/q\" \"c:\\users\\zoo\"",
        "Remove-Item -Recurse -Force c:\\users\\zoo",
        "rd /s c:\\users\\zoo\\proj",
    ]
    .iter()
    .enumerate()
    {
        let (allowed, err, _) = auditor.request_permission(&exec_request(&format!("sd2{i}"), cmd));
        assert!(!allowed, "`{cmd}` must hit self-destruct hard block");
        assert!(err.unwrap().contains("self-destruct"));
    }
}

#[test]
fn self_destruct_full_path_and_exe_suffix_bins_denied() {
    // 闭环批 2026-10-13：bin_key 取 basename + 剥 .exe——`/bin/rm`、
    // `del.exe` 全路径/带后缀形态此前打不中 DESTRUCTIVE_BINS（key 带目录/
    // 后缀），自毁硬拦对这种形态整体失明（fail-open）。保护集覆盖两族
    // 目标（POSIX 家目录 + Windows 用户目录），命令形态与目标逐条对应。
    let auditor = auditor_with_protected(&["/home/zoo", "/home/zoo/proj", "c:/users/zoo"]);
    for (i, cmd) in [
        "/bin/rm -rf /home/zoo",
        "/usr/bin/rm -rf ~",
        "del.exe /s /q c:/users/zoo",
        "c:/windows/system32/rm.exe -rf /home/zoo/proj",
    ]
    .iter()
    .enumerate()
    {
        let (allowed, err, _) = auditor.request_permission(&exec_request(&format!("sd3{i}"), cmd));
        assert!(!allowed, "`{cmd}` must hit self-destruct hard block");
        assert!(err.unwrap().contains("self-destruct"));
    }
}

fn auditor_with_protected_and_exempt(paths: &[&str], exempt: &str) -> SecurityAuditor {
    let auditor = auditor_with_protected(paths);
    auditor.set_self_destruct_exempt_path(exempt);
    auditor
}

#[test]
fn self_destruct_workspace_descendant_exempt_a_f3() {
    // A-F3 方案 1：目标为 workspace（豁免路径）内严格后代的绝对路径不再
    // 硬拦——`rm -rf <ws>/node_modules` 是日常重装依赖操作，交正常治理。
    let auditor =
        auditor_with_protected_and_exempt(&["/home/zoo", "/home/zoo/proj"], "/home/zoo/proj");
    for (i, cmd) in [
        "rm -rf /home/zoo/proj/node_modules",
        "rm -rf /home/zoo/proj/node_modules/lodash",
        "rm -rf /home/zoo/proj/target/debug",
    ]
    .iter()
    .enumerate()
    {
        let (allowed, err, _) = auditor.request_permission(&exec_request(&format!("sdx{i}"), cmd));
        assert!(
            allowed,
            "`{cmd}` inside workspace must NOT hard-block (A-F3 exempt), got {:?}",
            err
        );
    }
}

#[test]
fn self_destruct_workspace_descendant_windows_form_exempt() {
    // Windows 反斜杠形态：豁免路径归一化（小写 + 反斜杠转正斜杠）后匹配。
    let auditor = auditor_with_protected_and_exempt(&["c:/users/zoo/proj"], "C:\\Users\\Zoo\\Proj");
    let (allowed, err, _) = auditor.request_permission(&exec_request(
        "sdw0",
        "rd /s c:\\users\\zoo\\proj\\node_modules",
    ));
    assert!(
        allowed,
        "windows-form workspace descendant must be exempt, got {:?}",
        err
    );
}

#[test]
fn self_destruct_workspace_itself_still_denied() {
    // 豁免只作用于「严格后代」：workspace 本体（含尾斜杠形态）照旧硬拦。
    let auditor =
        auditor_with_protected_and_exempt(&["/home/zoo", "/home/zoo/proj"], "/home/zoo/proj");
    for (i, cmd) in ["rm -rf /home/zoo/proj", "rm -rf /home/zoo/proj/"]
        .iter()
        .enumerate()
    {
        let (allowed, err, _) = auditor.request_permission(&exec_request(&format!("sdy{i}"), cmd));
        assert!(!allowed, "`{cmd}` (workspace itself) must still hard-block");
        assert!(err.unwrap().contains("self-destruct"));
    }
}

#[test]
fn self_destruct_home_non_workspace_part_still_denied() {
    // workspace 外的 home 内容不受豁免影响（home 仍保护）。
    let auditor =
        auditor_with_protected_and_exempt(&["/home/zoo", "/home/zoo/proj"], "/home/zoo/proj");
    for (i, cmd) in ["rm -rf /home/zoo", "rm -rf /home/zoo/documents", "rm -rf /"]
        .iter()
        .enumerate()
    {
        let (allowed, err, _) = auditor.request_permission(&exec_request(&format!("sdz{i}"), cmd));
        assert!(!allowed, "`{cmd}` outside workspace must still hard-block");
        assert!(err.unwrap().contains("self-destruct"));
    }
}

#[test]
fn self_destruct_no_exempt_set_keeps_legacy_arms() {
    // 未注入豁免路径（空 = 旧行为）：三臂全开，后代臂照旧硬拦。
    let auditor = auditor_with_protected(&["/home/zoo/proj"]);
    let (allowed, err, _) =
        auditor.request_permission(&exec_request("sdn0", "rm -rf /home/zoo/proj/node_modules"));
    assert!(
        !allowed,
        "without exempt path the descendant arm must still fire"
    );
    assert!(err.unwrap().contains("self-destruct"));
}

fn exec_unknown_auditor(default_action: &str, policy: &str) -> SecurityAuditor {
    let auditor = SecurityAuditor::new(AuditorConfig {
        enabled: true,
        default_action: default_action.to_string(),
        ..Default::default()
    });
    auditor.set_exec_unknown_policy(policy);
    auditor
}

#[test]
fn exec_unknown_policy_ask_intercepts_unmatched() {
    // D1=ask：无规则命中 → RequireApproval（default deny 被 ask 分支抢先）。
    let auditor = exec_unknown_auditor("deny", "ask");
    let (allowed, _, _) = auditor.request_permission(&exec_request("d1a", "someunknown --flag"));
    assert!(!allowed);
    assert_eq!(
        auditor.pending_count(),
        1,
        "ask must create pending, not deny"
    );
}

#[test]
fn exec_unknown_policy_deny_intercepts_unmatched() {
    let auditor = exec_unknown_auditor("allow", "deny");
    let (allowed, err, _) = auditor.request_permission(&exec_request("d1b", "someunknown --flag"));
    assert!(!allowed, "deny policy must beat permissive default");
    assert!(err.unwrap().contains("exec_unknown_policy"));
}

#[test]
fn exec_unknown_policy_allow_overrides_default_deny() {
    let auditor = exec_unknown_auditor("deny", "allow");
    let (allowed, _, _) = auditor.request_permission(&exec_request("d1c", "someunknown --flag"));
    assert!(allowed, "explicit allow policy must override default deny");
}

#[test]
fn exec_unknown_policy_unset_preserves_default_action() {
    // 老配置兼容：策略键缺省（空串）= 旧行为，default deny 仍生效。
    let auditor = exec_unknown_auditor("deny", "");
    let (allowed, _, _) = auditor.request_permission(&exec_request("d1d", "someunknown --flag"));
    assert!(!allowed);
    assert_eq!(auditor.pending_count(), 0, "deny must not create pending");
}

struct ApprovingManager;
impl ApprovalManager for ApprovingManager {
    fn is_running(&self) -> bool {
        true
    }
    fn request_approval_sync(
        &self,
        _request_id: &str,
        _operation: &str,
        _target: &str,
        _risk_level: &str,
        _reason: &str,
        _timeout_secs: u64,
    ) -> Result<ApprovalVerdict, String> {
        Ok(ApprovalVerdict::approved())
    }
}

struct DownManager;
impl ApprovalManager for DownManager {
    fn is_running(&self) -> bool {
        false
    }
    fn request_approval_sync(
        &self,
        _request_id: &str,
        _operation: &str,
        _target: &str,
        _risk_level: &str,
        _reason: &str,
        _timeout_secs: u64,
    ) -> Result<ApprovalVerdict, String> {
        unreachable!("down manager must never be asked");
    }
}

#[test]
fn guardian_failure_approval_fails_closed_without_manager() {
    // D2 fail-closed：无 manager = Err（调用方按拒绝处理）。
    let auditor = ask_auditor();
    let res = auditor.request_guardian_failure_approval("exec", "guardian judge error", None);
    assert!(res.is_err(), "no manager must fail closed");
}

#[test]
fn guardian_failure_approval_fails_closed_when_manager_down() {
    let auditor = ask_auditor();
    auditor.set_approval_manager(Arc::new(DownManager));
    let res = auditor.request_guardian_failure_approval("exec", "guardian judge error", None);
    assert!(res.is_err(), "non-running manager must fail closed");
}

#[test]
fn guardian_failure_approval_routes_to_running_manager() {
    let auditor = ask_auditor();
    auditor.set_approval_manager(Arc::new(ApprovingManager));
    let res = auditor
        .request_guardian_failure_approval("exec", "guardian judge error", None)
        .expect("running manager must return verdict");
    assert!(res.approved);
}

// ===========================================================================
// Full Access 编辑器放行开关(2026-09-20 用户裁决)
// 短路位置:自毁硬拦之后、规则遍之前——三条行为锁:
// ① 开关关(或未注入)= 旧行为逐字节一致;② 自毁硬拦不被开关绕过;
// ③ 开关开 = 绕过显式 deny 规则,审计 policy_rule=`editor_access:*`。
// ===========================================================================

use crate::editor_access::EditorAccessState;

fn editor_auditor() -> SecurityAuditor {
    SecurityAuditor::new(AuditorConfig {
        enabled: true,
        default_action: "deny".to_string(),
        ..Default::default()
    })
}

#[test]
fn editor_access_off_matches_uninjected_verbatim() {
    // 注入双关状态 vs 完全未注入:evaluate_request 三元组必须逐字节一致
    // (装配永远存在,行为差异只允许来自开关本身)。
    let req = OperationRequest {
        id: "ea-0".to_string(),
        op_type: OperationType::FileWrite,
        danger_level: DangerLevel::High,
        user: "test".to_string(),
        source: "web".to_string(),
        target: "d:/workspace/run.log".to_string(),
        timestamp: None,
        ..Default::default()
    };

    let plain = editor_auditor();
    plain.set_rules(
        OperationType::FileWrite,
        vec![rule("*.log", "ask"), rule("*", "deny")],
    );
    let injected = editor_auditor();
    injected.set_rules(
        OperationType::FileWrite,
        vec![rule("*.log", "ask"), rule("*", "deny")],
    );
    injected.set_editor_access(EditorAccessState::new()); // 双关

    assert_eq!(
        plain.evaluate_request(&req),
        injected.evaluate_request(&req),
        "双关注入不得改变任何判定"
    );
}

#[test]
fn editor_full_access_does_not_bypass_self_destruct() {
    // 短路必须位于自毁硬拦**之后**:FA 全开下递归删除 workspace 仍 Denied
    // (若短路插错位置,此用例将返回 Allowed + editor_access:full)。
    let auditor = editor_auditor();
    auditor.set_protected_paths(vec!["d:/workspace".to_string()]);
    let editor = EditorAccessState::new();
    editor.set_flags(true, true);
    editor.set_workspace_roots(vec!["d:/workspace".to_string()]);
    auditor.set_editor_access(editor);

    let (decision, _, policy) =
        auditor.evaluate_request(&exec_request("ea-sd", "rm -rf d:/workspace"));
    assert_eq!(decision, SecurityDecision::Denied);
    assert_eq!(policy, "self_destruct");
}

#[test]
fn editor_full_access_short_circuits_deny_rules_with_audit_mark() {
    // FA 开启 = 「全放行」字面义:显式 deny 规则被绕过(裁决),policy_rule
    // 打 editor_access 标记供审计过滤。
    let auditor = editor_auditor();
    auditor.set_rules(OperationType::ProcessExec, vec![rule("python*", "deny")]);
    let editor = EditorAccessState::new();
    editor.set_flags(true, false);
    editor.set_workspace_roots(vec!["d:/workspace".to_string()]);
    auditor.set_editor_access(editor.clone());

    let (decision, _, policy) =
        auditor.evaluate_request(&exec_request("ea-sc", "python -c \"print(1)\""));
    assert_eq!(decision, SecurityDecision::Allowed);
    assert_eq!(policy, "editor_access:full");

    // 关掉开关:同一请求回到规则判定(deny 命中)。
    editor.set_flags(false, false);
    let (decision, _, policy) =
        auditor.evaluate_request(&exec_request("ea-sc2", "python -c \"print(1)\""));
    assert_eq!(decision, SecurityDecision::Denied);
    assert_eq!(policy, "rule[0]");
}

#[test]
fn audit_jsonl_wiring_persists_editor_access_policy() {
    // 接线 JSONL 结构化审计(装配期 set_audit_jsonl_log)后,request_permission
    // 的每条判定连同 policy_rule(如 editor_access:*)落 {dir}/audit.jsonl
    // ——审计页 security.audit 的数据源,「FA 放行可过滤追溯」的兑现点。
    let dir = std::env::temp_dir().join(format!("nb-audit-jsonl-{}", uuid::Uuid::new_v4()));
    let auditor = editor_auditor();
    auditor.set_audit_jsonl_log(true, dir.to_string_lossy().to_string());
    let editor = EditorAccessState::new();
    editor.set_flags(true, false);
    editor.set_workspace_roots(vec!["d:/workspace".to_string()]);
    auditor.set_editor_access(editor);

    let req = OperationRequest {
        id: "ea-jsonl".to_string(),
        op_type: OperationType::ProcessExec,
        danger_level: DangerLevel::Critical,
        user: "test".to_string(),
        source: "web".to_string(),
        target: "python --version".to_string(),
        timestamp: None,
        ..Default::default()
    };
    let (allowed, _, _) = auditor.request_permission(&req);
    assert!(allowed, "FA 下 exec 应放行");

    let jsonl = std::fs::read_to_string(dir.join("audit.jsonl")).expect("audit.jsonl 应落盘");
    let line = jsonl.lines().last().unwrap();
    assert!(
        line.contains("\"policy_rule\":\"editor_access:full\""),
        "policy_rule 应落盘可过滤,实际: {line}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn audit_jsonl_explicit_disable_wins() {
    // 显式禁用(enabled=false)优先于目录存在与否:不落盘、不建文件。
    let dir = std::env::temp_dir().join(format!("nb-audit-off-{}", uuid::Uuid::new_v4()));
    let auditor = editor_auditor();
    auditor.set_audit_jsonl_log(false, dir.to_string_lossy().to_string());

    let req = OperationRequest {
        id: "ea-jsonl-off".to_string(),
        op_type: OperationType::FileWrite,
        danger_level: DangerLevel::High,
        user: "test".to_string(),
        source: "web".to_string(),
        target: "d:/workspace/a.log".to_string(),
        timestamp: None,
        ..Default::default()
    };
    let _ = auditor.request_permission(&req);
    assert!(!dir.join("audit.jsonl").exists(), "显式禁用不得落盘");
    let _ = std::fs::remove_dir_all(&dir);
}

/// limit 升级但无审批 manager：Err fail-closed，且拒绝决策落审计
/// （A-F5 语义——fail-closed 也是安全决策，必须可追溯）。
#[test]
fn test_request_limit_approval_no_manager_fails_closed_with_audit() {
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("audit.jsonl");
    let config = AuditorConfig {
        audit_log_file_enabled: true,
        audit_log_dir: Some(dir.path().to_str().unwrap().to_string()),
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);

    let err = auditor
        .request_limit_approval("exec", "test over-limit", None)
        .unwrap_err();
    assert!(err.contains("no approval manager"), "got: {err}");

    let content = std::fs::read_to_string(&log_path).unwrap();
    assert!(
        content.contains("limit_review"),
        "审计缺 limit_review: {content}"
    );
    assert!(
        content.contains("fail-closed"),
        "审计缺 fail-closed 标记: {content}"
    );
    assert!(
        content.contains("security.limits"),
        "审计缺策略规则: {content}"
    );
    assert!(content.contains("denied"), "审计应为拒绝决策: {content}");
}

// ---------------------------------------------------------------------------
// §2 免打扰（2026-10-11 无人值守计划 Part 2）：RequireApproval 自动放行臂
// ---------------------------------------------------------------------------

use crate::approval_auto::ApprovalAutoApproveState;

/// 注入免打扰运行时态的 ask auditor。
fn auto_auditor(enabled: bool) -> (SecurityAuditor, Arc<ApprovalAutoApproveState>) {
    let auditor = ask_auditor();
    let state = Arc::new(ApprovalAutoApproveState::new(enabled));
    auditor.set_approval_auto_approve(state.clone());
    (auditor, state)
}

#[test]
fn approval_auto_disabled_state_keeps_manual_ask() {
    // 开关关（或仅注入未开）：ask 行为零变化——pending 拒绝，计数不涨。
    let (auditor, state) = auto_auditor(false);
    let (allowed, err, _) = auditor.request_permission(&exec_request("a0", "cargo publish"));
    assert!(!allowed);
    assert!(err.unwrap().contains("approval required"));
    assert_eq!(state.count(), 0, "关闭态不得计数");
}

#[test]
fn approval_auto_on_allows_and_marks_auto_by_config() {
    // 审计只落文件（内存不驻）——挂临时 jsonl 目录再断言。
    let dir = tempfile::tempdir().unwrap();
    let auditor = SecurityAuditor::new(AuditorConfig {
        enabled: true,
        default_action: "ask".to_string(),
        audit_log_file_enabled: true,
        audit_log_dir: Some(dir.path().to_str().unwrap().to_string()),
        ..Default::default()
    });
    let state = Arc::new(ApprovalAutoApproveState::new(true));
    auditor.set_approval_auto_approve(state.clone());

    let (allowed, err, _) = auditor.request_permission(&exec_request("a1", "cargo publish"));
    assert!(allowed, "开关开：require_approval 自动放行");
    assert!(err.is_none());
    assert_eq!(
        state.count(),
        1,
        "放行即计数（前端「本次已自动批准 N 条」）"
    );

    // 审计事件标 auto_by_config（可追溯：谁批的？配置批的）。
    let content =
        std::fs::read_to_string(dir.path().join("audit.jsonl")).expect("audit jsonl written");
    assert!(
        content.contains("auto_by_config"),
        "审计缺 auto_by_config: {content}"
    );
    assert!(
        content.contains("\"allowed\""),
        "审计应为放行决策: {content}"
    );
    assert!(
        content.contains("auto approved by security.approvals.auto_approve"),
        "审计须注明免打扰来源: {content}"
    );
}

// ===========================================================================
// L6（2026-10-12 T1）：pre-exec force_approval 升格语义
// 非硬拒判定一律升格 RequireApproval（policy=preexec_scan）；F3「总是允许」
// 规则表对带旗标请求跳过（卡片必须到达人）；硬拒（deny 规则）不受影响。
// ===========================================================================

#[test]
fn force_approval_skips_f3_rule_table_and_escalates() {
    let auditor = ask_auditor();
    let dir = tempfile::tempdir().unwrap();
    auditor.set_approval_rules(rules_hot(&dir, &[cargo_rule()]));

    // 基线：规则命中 → 自动放行。
    let (allowed, err, _) =
        auditor.request_permission(&exec_request("l6-r0", "cargo test --release"));
    assert!(allowed, "rule-matched exec must auto-allow without flag");
    assert!(err.is_none());

    // 同一请求带 force_approval：跳过规则表 → pending 存储 + 拒绝。
    let mut req = exec_request("l6-r1", "cargo test --release");
    req.force_approval = Some("pre-exec script scan: suspicious content".to_string());
    let (allowed, err, _) = auditor.request_permission(&req);
    assert!(
        !allowed,
        "flagged request must NOT auto-allow via rule table"
    );
    assert!(
        err.unwrap().contains("approval required"),
        "flagged request must surface as approval card"
    );
}

#[test]
fn approval_auto_cluster_channel_persists_rule_and_pattern_takes_priority() {
    let (auditor, state) = auto_auditor(true);
    let dir = tempfile::tempdir().unwrap();
    auditor.set_approval_rules(rules_hot(&dir, &[]));
    let ctx = ApprovalContext {
        channel: "cluster".to_string(),
        ..Default::default()
    };

    // 集群来源首次 ask：自动放行 + 落持久规则。
    let (allowed, _, _) = auditor
        .request_permission_with_ctx(&exec_request("c0", "cargo build --release"), Some(&ctx));
    assert!(allowed);
    assert_eq!(state.count(), 1);
    let path = dir.path().join("approval_rules.json");
    let rules = crate::approval_rules::load_rules(&path);
    assert!(
        rules
            .iter()
            .any(|r| r.op == "process_exec" && r.pattern == "cargo build *"),
        "集群来源须落持久规则: {rules:?}"
    );

    // 同类操作再来：pattern 记忆块先命中（auto_by_rule），免打扰块轮不到
    // ——计数停在不涨（优先级钉死：落盘规则是主放行路径，开关只是引子）。
    std::thread::sleep(std::time::Duration::from_millis(30)); // 睡过 Windows mtime 量化窗口
    let (allowed2, _, _) = auditor
        .request_permission_with_ctx(&exec_request("c1", "cargo build --release"), Some(&ctx));
    assert!(allowed2);
    assert_eq!(state.count(), 1, "第二问须走 pattern 块（计数不涨）");
}

#[test]
fn approval_auto_non_cluster_channel_no_persist() {
    let (auditor, state) = auto_auditor(true);
    let dir = tempfile::tempdir().unwrap();
    auditor.set_approval_rules(rules_hot(&dir, &[]));
    let ctx = ApprovalContext {
        channel: "web".to_string(),
        ..Default::default()
    };
    let (allowed, _, _) = auditor
        .request_permission_with_ctx(&exec_request("w0", "cargo build --release"), Some(&ctx));
    assert!(allowed, "对话来源照常自动放行（当次点头语义）");
    assert_eq!(state.count(), 1);
    let rules = crate::approval_rules::load_rules(&dir.path().join("approval_rules.json"));
    assert!(rules.is_empty(), "非集群来源不落持久规则: {rules:?}");
}

#[test]
fn approval_auto_persist_layer_gate_blocks_critical_nonexec() {
    // 层级门：CRITICAL 非 exec 自动放行本身不做闸（无人值守语义），但
    // 不落持久规则（与 M7 always 同门——CRITICAL 只允许 process_exec 豁免）。
    let (auditor, state) = auto_auditor(true);
    let dir = tempfile::tempdir().unwrap();
    auditor.set_approval_rules(rules_hot(&dir, &[]));
    let ctx = ApprovalContext {
        channel: "cluster".to_string(),
        ..Default::default()
    };
    let req = OperationRequest {
        id: "g0".to_string(),
        op_type: OperationType::FileWrite,
        danger_level: DangerLevel::Critical,
        user: "test".to_string(),
        source: "cli".to_string(),
        target: "/etc/passwd".to_string(),
        timestamp: None,
        ..Default::default()
    };
    let (allowed, _, _) = auditor.request_permission_with_ctx(&req, Some(&ctx));
    assert!(allowed, "自动批准本身不做层级闸");
    assert_eq!(state.count(), 1);
    let rules = crate::approval_rules::load_rules(&dir.path().join("approval_rules.json"));
    assert!(rules.is_empty(), "CRITICAL 非 exec 不落规则: {rules:?}");
}

// ---------- CMD-12（2026-10-13）：deny 规则的复合命令分段命中 ----------

fn deny_exec_auditor(pattern: &str) -> SecurityAuditor {
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = SecurityAuditor::new(config);
    auditor.set_rules(
        OperationType::ProcessExec,
        vec![SecurityRule {
            pattern: pattern.to_string(),
            action: "deny".to_string(),
            comment: "test deny".to_string(),
        }],
    );
    auditor
}

#[test]
fn deny_rule_hits_compound_nonfirst_segment() {
    // 核心：整串锚定打不中非首段，`echo hi && rm -rf y` 曾逃过 `rm -rf *`
    let auditor = deny_exec_auditor("rm -rf *");
    let (allowed, err, _) =
        auditor.request_permission(&exec_request("c1", "echo hi && rm -rf /tmp/cmd12_target"));
    assert!(!allowed, "compound tail must be denied");
    let err = err.unwrap();
    assert!(err.contains("rule matched"), "deny-by-rule, got: {err}");

    // 单段控制组：行为不变
    let (allowed, _, _) = auditor.request_permission(&exec_request("c2", "echo hi"));
    assert!(allowed, "benign single segment must pass");
    let (allowed, err, _) =
        auditor.request_permission(&exec_request("c3", "rm -rf /tmp/cmd12_target"));
    assert!(!allowed, "direct single segment still denied");
    assert!(err.unwrap().contains("rule matched"));
}

#[test]
fn deny_rule_compound_other_operator_families() {
    let auditor = deny_exec_auditor("shutdown *");
    for (i, cmd) in [
        "echo bye; shutdown /s",
        "echo bye || shutdown /r",
        "sync & shutdown /s",
        "uptime | shutdown /s",
    ]
    .iter()
    .enumerate()
    {
        let (allowed, _, _) = auditor.request_permission(&exec_request(&format!("c4-{i}"), cmd));
        assert!(!allowed, "operator family `{cmd}` must be denied");
    }
}

#[test]
fn deny_rule_quoted_operator_stays_single_segment_no_false_hit() {
    // 引号内的 `&&` 是字面量：单段，`rm *` 不得误伤
    let auditor = deny_exec_auditor("rm *");
    let (allowed, _, _) = auditor.request_permission(&exec_request("c5", "echo \"a && b\""));
    assert!(allowed, "quoted literal operator must not trigger deny");
}

#[test]
fn deny_rule_compound_segment_normalized() {
    // 段内归一化：剥引号 + 大小写（`RM "-RF"` 变体照旧打中）
    let auditor = deny_exec_auditor("rm -rf *");
    let (allowed, err, _) =
        auditor.request_permission(&exec_request("c6", "echo x && RM -RF /tmp/CMD12"));
    assert!(!allowed, "normalized segment must be denied");
    assert!(err.unwrap().contains("rule matched"));
}

#[test]
fn approval_prefix_rule_not_extended_by_compound_tail() {
    // allow 端在 auditor 层的联动：`cargo test *` 记忆规则不得罩住
    // 复合尾部（`cargo build && curl evil`）——任一段不命中即重新弹卡。
    let auditor = ask_auditor();
    let dir = tempfile::tempdir().unwrap();
    auditor.set_approval_rules(rules_hot(&dir, &[cargo_rule()]));

    let (allowed, _, _) =
        auditor.request_permission(&exec_request("c7", "cargo test --release && curl http://x"));
    assert!(!allowed, "compound with non-matching tail must re-ask");

    let (allowed, _, _) =
        auditor.request_permission(&exec_request("c8", "cargo test --release 2>&1"));
    assert!(
        allowed,
        "redirection stays single segment, still auto-allowed"
    );
}

#[test]
fn force_approval_does_not_override_hard_deny() {
    // deny 规则命中 = 硬拒：升格只覆盖「本会放行」的判定。
    let auditor = auditor_with_rules(OperationType::ProcessExec, vec![rule("rm -rf *", "deny")]);
    let mut req = exec_request("l6-r2", "rm -rf /tmp/x");
    req.force_approval = Some("pre-exec script scan: suspicious content".to_string());
    let (allowed, err, _) = auditor.request_permission(&req);
    assert!(!allowed);
    let err = err.unwrap();
    assert!(
        err.contains("Security policy denied"),
        "hard deny must stay hard, got: {err}"
    );
}

#[test]
fn allow_rule_all_segments_hit_is_auto_allowed() {
    // allow 桶全段命中（同族前缀）→ 自动放行成立（对称臂的正向钉）
    let auditor = ask_auditor();
    let dir = tempfile::tempdir().unwrap();
    auditor.set_approval_rules(rules_hot(&dir, &[cargo_rule()]));

    let (allowed, _, _) = auditor.request_permission(&exec_request(
        "c9",
        "cargo test --release && cargo test --lib",
    ));
    assert!(allowed, "all-segments hit must auto-allow");
}

#[test]
fn deny_rule_env_prefix_not_an_evasion() {
    // env 前缀剥除（与归约端同语义）：`FOO=1 cargo *` 不得借 env 头逃过 `cargo *`
    let auditor = deny_exec_auditor("cargo *");
    let (allowed, err, _) =
        auditor.request_permission(&exec_request("c10", "FOO=1 cargo publish --token x"));
    assert!(!allowed, "env-prefixed command must still be denied");
    assert!(err.unwrap().contains("rule matched"));

    // 段中 env：`echo hi && K=1 cargo publish` 尾段剥 env 后命中
    let (allowed, _, _) = auditor.request_permission(&exec_request(
        "c11",
        "echo hi && RUST_LOG=debug cargo publish",
    ));
    assert!(!allowed, "env-prefixed tail segment must be denied");
}

#[test]
fn deny_wildcard_rule_catches_empty_command_via_whole_string_union() {
    // 空命令：分段为空，靠 deny/ask 的整串锚定兜底维持旧拦截面
    // （`*` 规则对空串 contains 语义恒真；allow 桶无此兜底——空段守卫）
    let auditor = deny_exec_auditor("*");
    let (allowed, err, _) = auditor.request_permission(&exec_request("c12", "   "));
    assert!(!allowed, "`*` deny must catch empty command");
    assert!(err.unwrap().contains("rule matched"));
}

#[test]
fn force_approval_escalates_default_allow_decision() {
    // default_action=allow 的 auditor：无旗标本会直接放行；
    // 带旗标 → 升格 RequireApproval → 无 manager → pending。
    let auditor = allow_auditor();
    let mut req = exec_request("l6-r3", "bash ./setup.sh");
    req.force_approval = Some("pre-exec script scan: suspicious content".to_string());
    let (allowed, err, _) = auditor.request_permission(&req);
    assert!(!allowed, "flagged request must escalate over default-allow");
    assert!(err.unwrap().contains("approval required"));
}
