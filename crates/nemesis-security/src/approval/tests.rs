use super::*;

#[test]
fn test_request_and_approve() {
    let mgr = ApprovalManager::with_default_timeout();
    let id = mgr.request_approval("file_write /etc/hosts", "agent-1");
    let req = mgr.get(&id).unwrap();
    assert_eq!(req.status, ApprovalStatus::Pending);
    assert_eq!(req.requester, "agent-1");

    mgr.approve(&id).unwrap();
    let req = mgr.get(&id).unwrap();
    assert_eq!(req.status, ApprovalStatus::Approved);
}

#[test]
fn test_request_and_deny() {
    let mgr = ApprovalManager::with_default_timeout();
    let id = mgr.request_approval("process_exec rm -rf /", "agent-2");

    mgr.deny(&id, "dangerous operation").unwrap();
    let req = mgr.get(&id).unwrap();
    assert_eq!(req.status, ApprovalStatus::Denied);
    assert_eq!(req.deny_reason.as_deref(), Some("dangerous operation"));
}

#[test]
fn test_approve_nonexistent_fails() {
    let mgr = ApprovalManager::with_default_timeout();
    assert!(mgr.approve("does-not-exist").is_err());
}

#[test]
fn test_double_approve_fails() {
    let mgr = ApprovalManager::with_default_timeout();
    let id = mgr.request_approval("file_read /tmp/test", "agent-3");
    mgr.approve(&id).unwrap();
    // Second approval should fail because status is no longer Pending.
    assert!(mgr.approve(&id).is_err());
}

#[test]
fn test_cleanup_expired() {
    // Use a 0-second timeout so everything is immediately expired.
    let mgr = ApprovalManager::new(0);

    let id1 = mgr.request_approval("op-a", "agent");
    let id2 = mgr.request_approval("op-b", "agent");

    // Give a tiny moment so the timestamps are captured.
    std::thread::sleep(std::time::Duration::from_millis(10));

    let expired = mgr.cleanup_expired();
    assert_eq!(expired, 2);

    assert_eq!(mgr.get(&id1).unwrap().status, ApprovalStatus::Expired);
    assert_eq!(mgr.get(&id2).unwrap().status, ApprovalStatus::Expired);

    // Pending list should be empty after cleanup.
    assert!(mgr.list_pending().is_empty());
}

// ---- Multi-process tests ----

#[test]
fn test_multi_process_start_stop() {
    let mgr = MultiProcessApprovalManager::new(30);
    assert!(!mgr.is_running());
    mgr.start().unwrap();
    assert!(mgr.is_running());
    mgr.stop().unwrap();
    assert!(!mgr.is_running());
}

#[tokio::test]
async fn test_multi_process_not_running_error() {
    let mgr = MultiProcessApprovalManager::new(30);
    let req = MultiApprovalRequest {
        request_id: "test-1".to_string(),
        operation: "file_write".to_string(),
        target: "/tmp/test".to_string(),
        risk_level: "HIGH".to_string(),
        reason: "test".to_string(),
        context: HashMap::new(),
        timeout_seconds: 5,
        timestamp: chrono::Local::now().timestamp(),
    };
    let result = mgr.request_approval(&req).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not running"));
}

#[tokio::test]
async fn test_multi_process_auto_approve_safe() {
    let mgr = MultiProcessApprovalManager::new(30);
    mgr.start().unwrap();

    let req = MultiApprovalRequest {
        request_id: "test-safe".to_string(),
        operation: "file_read".to_string(),
        target: "/tmp/test".to_string(),
        risk_level: "LOW".to_string(),
        reason: "test".to_string(),
        context: HashMap::new(),
        timeout_seconds: 5,
        timestamp: chrono::Local::now().timestamp(),
    };
    let resp = mgr.request_approval(&req).await.unwrap();
    assert!(resp.approved);
    assert!(!resp.timed_out);
}

#[tokio::test]
async fn test_multi_process_reject_dangerous_no_factory() {
    let mgr = MultiProcessApprovalManager::new(30);
    mgr.start().unwrap();

    let req = MultiApprovalRequest {
        request_id: "test-danger".to_string(),
        operation: "process_exec".to_string(),
        target: "rm -rf /".to_string(),
        risk_level: "CRITICAL".to_string(),
        reason: "dangerous".to_string(),
        context: HashMap::new(),
        timeout_seconds: 5,
        timestamp: chrono::Local::now().timestamp(),
    };
    let resp = mgr.request_approval(&req).await.unwrap();
    assert!(!resp.approved);
    assert!(!resp.timed_out);
}

#[test]
fn test_is_safe_operation() {
    assert!(is_safe_operation("file_read", "LOW"));
    assert!(is_safe_operation("dir_list", "LOW"));
    assert!(is_safe_operation("network_request", "LOW"));
    assert!(!is_safe_operation("file_read", "MEDIUM"));
    assert!(!is_safe_operation("process_exec", "LOW"));
    assert!(!is_safe_operation("file_write", "LOW"));
}

#[test]
fn test_operation_display_name() {
    assert_eq!(operation_display_name("file_read"), "File Read");
    assert_eq!(operation_display_name("process_exec"), "Execute Command");
    assert_eq!(operation_display_name("network_request"), "Network Request");
    assert_eq!(operation_display_name("unknown_op"), "unknown_op");
}

// ---- Additional approval tests ----

#[test]
fn test_approval_status_display() {
    assert_eq!(format!("{}", ApprovalStatus::Pending), "pending");
    assert_eq!(format!("{}", ApprovalStatus::Approved), "approved");
    assert_eq!(format!("{}", ApprovalStatus::Denied), "denied");
    assert_eq!(format!("{}", ApprovalStatus::Expired), "expired");
}

#[test]
fn test_approval_config_default() {
    let config = ApprovalConfig::default();
    assert!(config.enabled);
    assert_eq!(config.timeout, Duration::from_secs(30));
    assert_eq!(config.min_risk_level, "MEDIUM");
    assert_eq!(config.dialog_width, 550);
    assert_eq!(config.dialog_height, 480);
    assert!(config.enable_sound);
    assert!(config.enable_animation);
}

#[test]
fn test_multi_approval_request_validate_valid() {
    let req = MultiApprovalRequest {
        request_id: "req-1".to_string(),
        operation: "file_write".to_string(),
        target: "/tmp/test".to_string(),
        risk_level: "HIGH".to_string(),
        reason: "test reason".to_string(),
        context: HashMap::new(),
        timeout_seconds: 30,
        timestamp: chrono::Local::now().timestamp(),
    };
    assert!(req.validate().is_ok());
}

#[test]
fn test_multi_approval_request_validate_all_risk_levels() {
    for level in &["LOW", "MEDIUM", "HIGH", "CRITICAL"] {
        let req = MultiApprovalRequest {
            request_id: "req-1".to_string(),
            operation: "file_write".to_string(),
            target: "/tmp/test".to_string(),
            risk_level: level.to_string(),
            reason: "test".to_string(),
            context: HashMap::new(),
            timeout_seconds: 30,
            timestamp: 0,
        };
        assert!(
            req.validate().is_ok(),
            "risk_level {} should be valid",
            level
        );
    }
}

#[test]
fn test_multi_approval_request_validate_empty_request_id() {
    let req = MultiApprovalRequest {
        request_id: "".to_string(),
        operation: "file_write".to_string(),
        target: "/tmp/test".to_string(),
        risk_level: "HIGH".to_string(),
        reason: "test".to_string(),
        context: HashMap::new(),
        timeout_seconds: 30,
        timestamp: 0,
    };
    assert!(req.validate().is_err());
    assert!(req.validate().unwrap_err().contains("request_id"));
}

#[test]
fn test_multi_approval_request_validate_empty_operation() {
    let req = MultiApprovalRequest {
        request_id: "req-1".to_string(),
        operation: "".to_string(),
        target: "/tmp/test".to_string(),
        risk_level: "HIGH".to_string(),
        reason: "test".to_string(),
        context: HashMap::new(),
        timeout_seconds: 30,
        timestamp: 0,
    };
    assert!(req.validate().is_err());
    assert!(req.validate().unwrap_err().contains("operation"));
}

#[test]
fn test_multi_approval_request_validate_empty_target() {
    let req = MultiApprovalRequest {
        request_id: "req-1".to_string(),
        operation: "file_write".to_string(),
        target: "".to_string(),
        risk_level: "HIGH".to_string(),
        reason: "test".to_string(),
        context: HashMap::new(),
        timeout_seconds: 30,
        timestamp: 0,
    };
    assert!(req.validate().is_err());
    assert!(req.validate().unwrap_err().contains("target"));
}

#[test]
fn test_multi_approval_request_validate_empty_risk_level() {
    let req = MultiApprovalRequest {
        request_id: "req-1".to_string(),
        operation: "file_write".to_string(),
        target: "/tmp/test".to_string(),
        risk_level: "".to_string(),
        reason: "test".to_string(),
        context: HashMap::new(),
        timeout_seconds: 30,
        timestamp: 0,
    };
    assert!(req.validate().is_err());
    assert!(req.validate().unwrap_err().contains("risk_level"));
}

#[test]
fn test_multi_approval_request_validate_invalid_risk_level() {
    let req = MultiApprovalRequest {
        request_id: "req-1".to_string(),
        operation: "file_write".to_string(),
        target: "/tmp/test".to_string(),
        risk_level: "INVALID".to_string(),
        reason: "test".to_string(),
        context: HashMap::new(),
        timeout_seconds: 30,
        timestamp: 0,
    };
    assert!(req.validate().is_err());
    assert!(req.validate().unwrap_err().contains("invalid risk_level"));
}

#[test]
fn test_multi_approval_request_validate_zero_timeout() {
    let req = MultiApprovalRequest {
        request_id: "req-1".to_string(),
        operation: "file_write".to_string(),
        target: "/tmp/test".to_string(),
        risk_level: "HIGH".to_string(),
        reason: "test".to_string(),
        context: HashMap::new(),
        timeout_seconds: 0,
        timestamp: 0,
    };
    assert!(req.validate().is_err());
    assert!(req.validate().unwrap_err().contains("timeout_seconds"));
}

#[test]
fn test_is_safe_operation_all_safe_ops() {
    assert!(is_safe_operation("file_read", "LOW"));
    assert!(is_safe_operation("dir_list", "LOW"));
    assert!(is_safe_operation("network_request", "LOW"));
    assert!(is_safe_operation("hardware_i2c", "LOW"));
    assert!(is_safe_operation("registry_read", "LOW"));
}

#[test]
fn test_is_safe_operation_non_low_risk() {
    assert!(!is_safe_operation("file_read", "MEDIUM"));
    assert!(!is_safe_operation("file_read", "HIGH"));
    assert!(!is_safe_operation("file_read", "CRITICAL"));
}

#[test]
fn test_is_safe_operation_dangerous_op() {
    assert!(!is_safe_operation("file_write", "LOW"));
    assert!(!is_safe_operation("process_exec", "LOW"));
    assert!(!is_safe_operation("file_delete", "LOW"));
    assert!(!is_safe_operation("registry_write", "LOW"));
}

#[test]
fn test_operation_display_name_all_known_ops() {
    assert_eq!(operation_display_name("file_read"), "File Read");
    assert_eq!(operation_display_name("file_write"), "File Write");
    assert_eq!(operation_display_name("file_delete"), "File Delete");
    assert_eq!(operation_display_name("dir_read"), "Directory Listing");
    assert_eq!(operation_display_name("dir_list"), "Directory Listing");
    assert_eq!(operation_display_name("dir_create"), "Create Directory");
    assert_eq!(operation_display_name("dir_delete"), "Delete Directory");
    assert_eq!(operation_display_name("process_exec"), "Execute Command");
    assert_eq!(operation_display_name("process_spawn"), "Spawn Process");
    assert_eq!(operation_display_name("process_kill"), "Kill Process");
    assert_eq!(operation_display_name("network_request"), "Network Request");
    assert_eq!(operation_display_name("network_download"), "Download File");
    assert_eq!(operation_display_name("network_upload"), "Upload File");
    assert_eq!(operation_display_name("hardware_i2c"), "I2C Access");
    assert_eq!(operation_display_name("hardware_spi"), "SPI Access");
    assert_eq!(operation_display_name("hardware_gpio"), "GPIO Access");
    assert_eq!(operation_display_name("registry_read"), "Registry Read");
    assert_eq!(operation_display_name("registry_write"), "Registry Write");
    assert_eq!(operation_display_name("registry_delete"), "Registry Delete");
    assert_eq!(operation_display_name("system_shutdown"), "System Shutdown");
    assert_eq!(operation_display_name("system_reboot"), "System Reboot");
}

#[test]
fn test_approval_manager_new_custom_timeout() {
    let mgr = ApprovalManager::new(60);
    // Should work normally
    let id = mgr.request_approval("file_write", "agent-1");
    let req = mgr.get(&id).unwrap();
    assert_eq!(req.status, ApprovalStatus::Pending);
}

#[test]
fn test_approval_manager_request_generates_unique_ids() {
    let mgr = ApprovalManager::with_default_timeout();
    let id1 = mgr.request_approval("op1", "agent-1");
    let id2 = mgr.request_approval("op2", "agent-2");
    assert_ne!(id1, id2);
}

#[test]
fn test_approval_manager_deny_sets_reason() {
    let mgr = ApprovalManager::with_default_timeout();
    let id = mgr.request_approval("file_delete", "agent-1");
    mgr.deny(&id, "dangerous").unwrap();
    let req = mgr.get(&id).unwrap();
    assert_eq!(req.status, ApprovalStatus::Denied);
    assert_eq!(req.deny_reason.as_deref(), Some("dangerous"));
}

#[test]
fn test_approval_manager_deny_nonexistent() {
    let mgr = ApprovalManager::with_default_timeout();
    assert!(mgr.deny("nonexistent", "reason").is_err());
}

#[test]
fn test_approval_manager_deny_already_denied() {
    let mgr = ApprovalManager::with_default_timeout();
    let id = mgr.request_approval("op", "agent");
    mgr.deny(&id, "first").unwrap();
    assert!(mgr.deny(&id, "second").is_err());
}

#[test]
fn test_approval_manager_total_count() {
    let mgr = ApprovalManager::with_default_timeout();
    assert_eq!(mgr.total_count(), 0);
    let id1 = mgr.request_approval("op1", "agent");
    let _id2 = mgr.request_approval("op2", "agent");
    assert_eq!(mgr.total_count(), 2);
    mgr.approve(&id1).unwrap();
    assert_eq!(mgr.total_count(), 2); // Still tracked
}

#[test]
fn test_approval_manager_list_pending_after_approve() {
    let mgr = ApprovalManager::with_default_timeout();
    let id1 = mgr.request_approval("op1", "agent");
    let _id2 = mgr.request_approval("op2", "agent");
    assert_eq!(mgr.list_pending().len(), 2);
    mgr.approve(&id1).unwrap();
    assert_eq!(mgr.list_pending().len(), 1);
}

#[test]
fn test_approval_manager_cleanup_does_not_affect_non_pending() {
    let mgr = ApprovalManager::new(0);
    let id = mgr.request_approval("op", "agent");
    mgr.approve(&id).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));
    let expired = mgr.cleanup_expired();
    assert_eq!(expired, 0); // Already approved, not expired
}

#[test]
fn test_multi_process_set_get_config() {
    let mgr = MultiProcessApprovalManager::new(30);
    let custom_config = ApprovalConfig {
        enabled: false,
        timeout: Duration::from_secs(60),
        min_risk_level: "HIGH".to_string(),
        dialog_width: 800,
        dialog_height: 600,
        enable_sound: false,
        enable_animation: false,
    };
    mgr.set_config(custom_config.clone());
    let retrieved = mgr.get_config();
    assert!(!retrieved.enabled);
    assert_eq!(retrieved.timeout, Duration::from_secs(60));
    assert_eq!(retrieved.min_risk_level, "HIGH");
    assert_eq!(retrieved.dialog_width, 800);
}

#[test]
fn test_multi_process_default_timeout() {
    let mgr = MultiProcessApprovalManager::with_default_timeout();
    mgr.start().unwrap();
    assert!(mgr.is_running());
    mgr.stop().unwrap();
}

#[tokio::test]
async fn test_multi_process_safe_ops_various() {
    let mgr = MultiProcessApprovalManager::new(30);
    mgr.start().unwrap();

    for (op, expected) in [
        ("file_read", true),
        ("dir_list", true),
        ("network_request", true),
        ("hardware_i2c", true),
        ("registry_read", true),
    ] {
        let req = MultiApprovalRequest {
            request_id: format!("test-{}", op),
            operation: op.to_string(),
            target: "/tmp/test".to_string(),
            risk_level: "LOW".to_string(),
            reason: "test".to_string(),
            context: HashMap::new(),
            timeout_seconds: 5,
            timestamp: chrono::Local::now().timestamp(),
        };
        let resp = mgr.request_approval(&req).await.unwrap();
        assert_eq!(
            resp.approved, expected,
            "op {} should be approved={}",
            op, expected
        );
    }
}

#[tokio::test]
async fn test_multi_process_dangerous_ops_various() {
    let mgr = MultiProcessApprovalManager::new(30);
    mgr.start().unwrap();

    for op in [
        "process_exec",
        "file_delete",
        "registry_write",
        "system_shutdown",
    ] {
        let req = MultiApprovalRequest {
            request_id: format!("test-{}", op),
            operation: op.to_string(),
            target: "dangerous_target".to_string(),
            risk_level: "CRITICAL".to_string(),
            reason: "test".to_string(),
            context: HashMap::new(),
            timeout_seconds: 5,
            timestamp: chrono::Local::now().timestamp(),
        };
        let resp = mgr.request_approval(&req).await.unwrap();
        assert!(!resp.approved, "op {} should be denied", op);
        assert!(!resp.timed_out);
    }
}

#[test]
fn test_multi_approval_response_fields() {
    let resp = MultiApprovalResponse {
        request_id: "test-1".to_string(),
        approved: true,
        timed_out: false,
        duration_seconds: 1.5,
        response_time: 1700000000,
    };
    assert_eq!(resp.request_id, "test-1");
    assert!(resp.approved);
    assert!(!resp.timed_out);
    assert_eq!(resp.duration_seconds, 1.5);
}

#[test]
fn test_multi_approval_request_context() {
    let mut ctx = HashMap::new();
    ctx.insert("source_ip".to_string(), "192.168.1.1".to_string());
    ctx.insert("session_id".to_string(), "abc123".to_string());
    let req = MultiApprovalRequest {
        request_id: "ctx-test".to_string(),
        operation: "file_write".to_string(),
        target: "/tmp/test".to_string(),
        risk_level: "HIGH".to_string(),
        reason: "test".to_string(),
        context: ctx,
        timeout_seconds: 30,
        timestamp: 0,
    };
    assert_eq!(req.context.len(), 2);
    assert_eq!(
        req.context.get("source_ip"),
        Some(&"192.168.1.1".to_string())
    );
}

// ============================================================
// Additional tests for 95%+ coverage
// ============================================================

#[tokio::test]
async fn test_multi_process_with_child_factory_popup_not_supported() {
    struct FailFactory;
    impl ChildProcessFactory for FailFactory {
        fn spawn_child(
            &self,
            _window_type: &str,
            _data: HashMap<String, serde_json::Value>,
        ) -> Result<(String, oneshot::Receiver<serde_json::Value>), String> {
            Err("popup not supported on this platform".to_string())
        }
    }

    let mgr = MultiProcessApprovalManager::new(30);
    mgr.set_child_factory(Arc::new(FailFactory));
    mgr.start().unwrap();

    let req = MultiApprovalRequest {
        request_id: "popup-fail".to_string(),
        operation: "file_write".to_string(),
        target: "/tmp/test".to_string(),
        risk_level: "HIGH".to_string(),
        reason: "test".to_string(),
        context: HashMap::new(),
        timeout_seconds: 5,
        timestamp: chrono::Local::now().timestamp(),
    };
    let resp = mgr.request_approval(&req).await.unwrap();
    assert!(!resp.approved);
    assert!(!resp.timed_out);
}

#[tokio::test]
async fn test_multi_process_with_child_factory_spawn_error() {
    struct ErrorFactory;
    impl ChildProcessFactory for ErrorFactory {
        fn spawn_child(
            &self,
            _window_type: &str,
            _data: HashMap<String, serde_json::Value>,
        ) -> Result<(String, oneshot::Receiver<serde_json::Value>), String> {
            Err("generic spawn error".to_string())
        }
    }

    let mgr = MultiProcessApprovalManager::new(30);
    mgr.set_child_factory(Arc::new(ErrorFactory));
    mgr.start().unwrap();

    let req = MultiApprovalRequest {
        request_id: "spawn-err".to_string(),
        operation: "file_write".to_string(),
        target: "/tmp/test".to_string(),
        risk_level: "HIGH".to_string(),
        reason: "test".to_string(),
        context: HashMap::new(),
        timeout_seconds: 5,
        timestamp: chrono::Local::now().timestamp(),
    };
    let result = mgr.request_approval(&req).await;
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .contains("failed to create approval window")
    );
}

#[tokio::test]
async fn test_multi_process_with_child_factory_approved() {
    struct ApproveFactory;
    impl ChildProcessFactory for ApproveFactory {
        fn spawn_child(
            &self,
            _window_type: &str,
            _data: HashMap<String, serde_json::Value>,
        ) -> Result<(String, oneshot::Receiver<serde_json::Value>), String> {
            let (tx, rx) = oneshot::channel();
            tx.send(serde_json::json!({"approved": true})).unwrap();
            Ok(("child-1".to_string(), rx))
        }
    }

    let mgr = MultiProcessApprovalManager::new(30);
    mgr.set_child_factory(Arc::new(ApproveFactory));
    mgr.start().unwrap();

    let req = MultiApprovalRequest {
        request_id: "factory-approve".to_string(),
        operation: "file_write".to_string(),
        target: "/tmp/test".to_string(),
        risk_level: "HIGH".to_string(),
        reason: "test".to_string(),
        context: HashMap::new(),
        timeout_seconds: 5,
        timestamp: chrono::Local::now().timestamp(),
    };
    let resp = mgr.request_approval(&req).await.unwrap();
    assert!(resp.approved);
    assert!(!resp.timed_out);
    assert!(resp.duration_seconds >= 0.0);
}

#[tokio::test]
async fn test_multi_process_with_child_factory_denied() {
    struct DenyFactory;
    impl ChildProcessFactory for DenyFactory {
        fn spawn_child(
            &self,
            _window_type: &str,
            _data: HashMap<String, serde_json::Value>,
        ) -> Result<(String, oneshot::Receiver<serde_json::Value>), String> {
            let (tx, rx) = oneshot::channel();
            tx.send(serde_json::json!({"approved": false})).unwrap();
            Ok(("child-1".to_string(), rx))
        }
    }

    let mgr = MultiProcessApprovalManager::new(30);
    mgr.set_child_factory(Arc::new(DenyFactory));
    mgr.start().unwrap();

    let req = MultiApprovalRequest {
        request_id: "factory-deny".to_string(),
        operation: "file_delete".to_string(),
        target: "/tmp/test".to_string(),
        risk_level: "HIGH".to_string(),
        reason: "test".to_string(),
        context: HashMap::new(),
        timeout_seconds: 5,
        timestamp: chrono::Local::now().timestamp(),
    };
    let resp = mgr.request_approval(&req).await.unwrap();
    assert!(!resp.approved);
    assert!(!resp.timed_out);
}

#[tokio::test]
async fn test_multi_process_with_child_factory_channel_dropped() {
    struct DropFactory;
    impl ChildProcessFactory for DropFactory {
        fn spawn_child(
            &self,
            _window_type: &str,
            _data: HashMap<String, serde_json::Value>,
        ) -> Result<(String, oneshot::Receiver<serde_json::Value>), String> {
            let (_tx, rx) = oneshot::channel();
            // Drop the sender so the receiver returns Err
            Ok(("child-1".to_string(), rx))
        }
    }

    let mgr = MultiProcessApprovalManager::new(30);
    mgr.set_child_factory(Arc::new(DropFactory));
    mgr.start().unwrap();

    let req = MultiApprovalRequest {
        request_id: "channel-drop".to_string(),
        operation: "file_write".to_string(),
        target: "/tmp/test".to_string(),
        risk_level: "HIGH".to_string(),
        reason: "test".to_string(),
        context: HashMap::new(),
        timeout_seconds: 5,
        timestamp: chrono::Local::now().timestamp(),
    };
    let result = mgr.request_approval(&req).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("empty result"));
}

#[tokio::test]
async fn test_multi_process_with_child_factory_timeout() {
    // We need the sender to stay alive (not dropped) but never send.
    // Use a shared sender that we keep alive.
    let sender = Arc::new(Mutex::new(None));
    let sender_clone = sender.clone();

    struct TimeoutFactory {
        sender: Arc<Mutex<Option<oneshot::Sender<serde_json::Value>>>>,
    }
    impl ChildProcessFactory for TimeoutFactory {
        fn spawn_child(
            &self,
            _window_type: &str,
            _data: HashMap<String, serde_json::Value>,
        ) -> Result<(String, oneshot::Receiver<serde_json::Value>), String> {
            let (tx, rx) = oneshot::channel();
            *self.sender.lock() = Some(tx);
            Ok(("child-1".to_string(), rx))
        }
    }

    let mgr = MultiProcessApprovalManager::new(1);
    mgr.set_child_factory(Arc::new(TimeoutFactory {
        sender: sender_clone,
    }));
    mgr.start().unwrap();

    let req = MultiApprovalRequest {
        request_id: "timeout-test".to_string(),
        operation: "file_write".to_string(),
        target: "/tmp/test".to_string(),
        risk_level: "HIGH".to_string(),
        reason: "test".to_string(),
        context: HashMap::new(),
        timeout_seconds: 1,
        timestamp: chrono::Local::now().timestamp(),
    };
    let resp = mgr.request_approval(&req).await.unwrap();
    assert!(!resp.approved);
    assert!(resp.timed_out);
    // Keep sender alive through the test
    let _ = sender;
}

#[tokio::test]
async fn test_multi_process_with_child_factory_data_field_approved() {
    // Test the data.approved fallback path
    struct DataFieldFactory;
    impl ChildProcessFactory for DataFieldFactory {
        fn spawn_child(
            &self,
            _window_type: &str,
            _data: HashMap<String, serde_json::Value>,
        ) -> Result<(String, oneshot::Receiver<serde_json::Value>), String> {
            let (tx, rx) = oneshot::channel();
            // Use data.approved format instead of top-level approved
            tx.send(serde_json::json!({"data": {"approved": true}}))
                .unwrap();
            Ok(("child-1".to_string(), rx))
        }
    }

    let mgr = MultiProcessApprovalManager::new(30);
    mgr.set_child_factory(Arc::new(DataFieldFactory));
    mgr.start().unwrap();

    let req = MultiApprovalRequest {
        request_id: "data-field".to_string(),
        operation: "file_write".to_string(),
        target: "/tmp/test".to_string(),
        risk_level: "HIGH".to_string(),
        reason: "test".to_string(),
        context: HashMap::new(),
        timeout_seconds: 5,
        timestamp: chrono::Local::now().timestamp(),
    };
    let resp = mgr.request_approval(&req).await.unwrap();
    assert!(resp.approved);
}

#[tokio::test]
async fn test_multi_process_with_child_factory_no_approved_field() {
    // When the result has no approved field, default to false
    struct NoApprovedFieldFactory;
    impl ChildProcessFactory for NoApprovedFieldFactory {
        fn spawn_child(
            &self,
            _window_type: &str,
            _data: HashMap<String, serde_json::Value>,
        ) -> Result<(String, oneshot::Receiver<serde_json::Value>), String> {
            let (tx, rx) = oneshot::channel();
            tx.send(serde_json::json!({"status": "ok"})).unwrap();
            Ok(("child-1".to_string(), rx))
        }
    }

    let mgr = MultiProcessApprovalManager::new(30);
    mgr.set_child_factory(Arc::new(NoApprovedFieldFactory));
    mgr.start().unwrap();

    let req = MultiApprovalRequest {
        request_id: "no-approved".to_string(),
        operation: "file_write".to_string(),
        target: "/tmp/test".to_string(),
        risk_level: "HIGH".to_string(),
        reason: "test".to_string(),
        context: HashMap::new(),
        timeout_seconds: 5,
        timestamp: chrono::Local::now().timestamp(),
    };
    let resp = mgr.request_approval(&req).await.unwrap();
    // No approved field means default false
    assert!(!resp.approved);
}

#[test]
fn test_approval_request_debug() {
    let req = ApprovalRequest {
        id: "req-123".to_string(),
        operation: "file_write".to_string(),
        requester: "agent-1".to_string(),
        timestamp: "2026-01-01T00:00:00Z".to_string(),
        status: ApprovalStatus::Pending,
        deny_reason: None,
    };
    let debug = format!("{:?}", req);
    assert!(debug.contains("req-123"));
    assert!(debug.contains("file_write"));
}

#[test]
fn test_multi_approval_request_debug() {
    let req = MultiApprovalRequest {
        request_id: "multi-123".to_string(),
        operation: "file_delete".to_string(),
        target: "/tmp/test".to_string(),
        risk_level: "CRITICAL".to_string(),
        reason: "dangerous".to_string(),
        context: HashMap::new(),
        timeout_seconds: 30,
        timestamp: 1700000000,
    };
    let debug = format!("{:?}", req);
    assert!(debug.contains("multi-123"));
    assert!(debug.contains("CRITICAL"));
}

#[test]
fn test_multi_approval_response_debug() {
    let resp = MultiApprovalResponse {
        request_id: "resp-1".to_string(),
        approved: true,
        timed_out: false,
        duration_seconds: 2.5,
        response_time: 1700000000,
    };
    let debug = format!("{:?}", resp);
    assert!(debug.contains("resp-1"));
}

#[test]
fn test_approval_config_clone() {
    let config = ApprovalConfig::default();
    let cloned = config.clone();
    assert_eq!(cloned.enabled, config.enabled);
    assert_eq!(cloned.timeout, config.timeout);
    assert_eq!(cloned.min_risk_level, config.min_risk_level);
}

#[tokio::test]
async fn test_multi_process_stop_when_not_started() {
    let mgr = MultiProcessApprovalManager::new(30);
    // Stop when not started should succeed
    mgr.stop().unwrap();
    assert!(!mgr.is_running());
}

#[tokio::test]
async fn test_multi_process_start_idempotent() {
    let mgr = MultiProcessApprovalManager::new(30);
    mgr.start().unwrap();
    assert!(mgr.is_running());
    mgr.start().unwrap(); // Start again
    assert!(mgr.is_running());
    mgr.stop().unwrap();
}

#[test]
fn test_approval_manager_get_nonexistent() {
    let mgr = ApprovalManager::with_default_timeout();
    assert!(mgr.get("nonexistent").is_none());
}

#[test]
fn test_approval_manager_approve_after_deny_fails() {
    let mgr = ApprovalManager::with_default_timeout();
    let id = mgr.request_approval("op", "agent");
    mgr.deny(&id, "nope").unwrap();
    // Trying to approve a denied request should fail
    assert!(mgr.approve(&id).is_err());
}

#[test]
fn test_approval_manager_deny_after_approve_fails() {
    let mgr = ApprovalManager::with_default_timeout();
    let id = mgr.request_approval("op", "agent");
    mgr.approve(&id).unwrap();
    // Trying to deny an approved request should fail
    assert!(mgr.deny(&id, "reason").is_err());
}

#[tokio::test]
async fn test_multi_process_uses_request_timeout_when_set() {
    // Keep sender alive to trigger timeout rather than "empty result"
    let sender = Arc::new(Mutex::new(None));
    let sender_clone = sender.clone();

    struct TimeoutFactory {
        sender: Arc<Mutex<Option<oneshot::Sender<serde_json::Value>>>>,
    }
    impl ChildProcessFactory for TimeoutFactory {
        fn spawn_child(
            &self,
            _window_type: &str,
            _data: HashMap<String, serde_json::Value>,
        ) -> Result<(String, oneshot::Receiver<serde_json::Value>), String> {
            let (tx, rx) = oneshot::channel();
            *self.sender.lock() = Some(tx);
            Ok(("child-1".to_string(), rx))
        }
    }

    let mgr = MultiProcessApprovalManager::new(600); // 10 min default timeout
    mgr.set_child_factory(Arc::new(TimeoutFactory {
        sender: sender_clone,
    }));
    mgr.start().unwrap();

    let req = MultiApprovalRequest {
        request_id: "custom-timeout".to_string(),
        operation: "file_write".to_string(),
        target: "/tmp/test".to_string(),
        risk_level: "HIGH".to_string(),
        reason: "test".to_string(),
        context: HashMap::new(),
        timeout_seconds: 1, // Use 1s timeout from request
        timestamp: chrono::Local::now().timestamp(),
    };
    let start = std::time::Instant::now();
    let resp = mgr.request_approval(&req).await.unwrap();
    let elapsed = start.elapsed();
    assert!(resp.timed_out);
    assert!(
        elapsed.as_secs() < 5,
        "Should timeout in ~1s, not 600s (elapsed: {:?}",
        elapsed
    );
    let _ = sender;
}
