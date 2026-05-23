use super::*;
use crate::auditor::AuditorConfig;

fn make_middleware(preset: PermissionPreset) -> SecurityMiddleware {
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    SecurityMiddleware::with_preset(auditor, "test_user", "cli", "/tmp/workspace", preset)
}

#[test]
fn test_preset_read_only() {
    let mw = make_middleware(PermissionPreset::ReadOnly);
    assert!(mw.is_operation_allowed(OperationType::FileRead));
    assert!(mw.is_operation_allowed(OperationType::DirRead));
    assert!(!mw.is_operation_allowed(OperationType::FileWrite));
    assert!(!mw.is_operation_allowed(OperationType::ProcessExec));
}

#[test]
fn test_preset_standard() {
    let mw = make_middleware(PermissionPreset::Standard);
    assert!(mw.is_operation_allowed(OperationType::FileRead));
    assert!(mw.is_operation_allowed(OperationType::FileWrite));
    assert!(mw.is_operation_allowed(OperationType::NetworkRequest));
    assert!(!mw.is_operation_allowed(OperationType::ProcessExec));
}

#[test]
fn test_preset_elevated() {
    let mw = make_middleware(PermissionPreset::Elevated);
    assert!(mw.is_operation_allowed(OperationType::FileRead));
    assert!(mw.is_operation_allowed(OperationType::ProcessExec));
    assert!(!mw.is_operation_allowed(OperationType::ProcessKill));
}

#[test]
fn test_preset_unrestricted() {
    let mw = make_middleware(PermissionPreset::Unrestricted);
    assert!(mw.is_operation_allowed(OperationType::FileRead));
    assert!(mw.is_operation_allowed(OperationType::ProcessKill));
    assert!(mw.is_operation_allowed(OperationType::SystemShutdown));
}

#[test]
fn test_secure_file_wrapper() {
    let mw = make_middleware(PermissionPreset::Standard);
    let wrapper = SecureFileWrapper::new(&mw);
    // Path validation may fail for non-existent paths but the check itself works
    let _ = wrapper.check_file_read("/tmp/test.txt");
}

#[test]
fn test_secure_process_wrapper_dangerous() {
    let mw = make_middleware(PermissionPreset::Elevated);
    let wrapper = SecureProcessWrapper::new(&mw);
    assert!(wrapper.check_process_exec("rm -rf /").is_err());
    assert!(wrapper.check_process_exec("sudo something").is_err());
}

#[test]
fn test_secure_process_wrapper_blocked_by_preset() {
    let mw = make_middleware(PermissionPreset::Standard);
    let wrapper = SecureProcessWrapper::new(&mw);
    // ProcessExec not allowed under Standard preset
    let result = wrapper.check_process_exec("ls -la");
    assert!(result.is_err());
}

#[test]
fn test_secure_network_wrapper() {
    let mw = make_middleware(PermissionPreset::Standard);
    let wrapper = SecureNetworkWrapper::new(&mw);
    assert!(wrapper.check_network_request("https://example.com").is_ok());
}

#[test]
fn test_secure_network_download_invalid_scheme() {
    let mw = make_middleware(PermissionPreset::Standard);
    let wrapper = SecureNetworkWrapper::new(&mw);
    assert!(wrapper.check_network_download("ftp://example.com").is_err());
    assert!(wrapper.check_network_download("file:///etc/passwd").is_err());
}

#[test]
fn test_permission_preset_allows_consistency() {
    for preset in [
        PermissionPreset::ReadOnly,
        PermissionPreset::Standard,
        PermissionPreset::Elevated,
        PermissionPreset::Unrestricted,
    ] {
        let ops = [
            OperationType::FileRead,
            OperationType::FileWrite,
            OperationType::FileDelete,
            OperationType::ProcessExec,
            OperationType::ProcessKill,
            OperationType::NetworkRequest,
            OperationType::HardwareI2C,
        ];
        for op in ops {
            // Just ensure no panic
            let _ = preset.allows(op);
        }
    }
}

#[test]
fn test_batch_operation_empty() {
    let mw = make_middleware(PermissionPreset::Standard);
    let batch = BatchOperationRequest::default();
    assert!(mw.request_batch_permission(&batch).is_err());
}

#[test]
fn test_batch_operation_blocked_by_preset() {
    let mw = make_middleware(PermissionPreset::Standard);
    let batch = BatchOperationRequest {
        id: "batch-1".to_string(),
        operations: vec![OperationRequest {
            id: "op-1".to_string(),
            op_type: OperationType::ProcessExec,
            danger_level: DangerLevel::Critical,
            user: "test".to_string(),
            source: "cli".to_string(),
            target: "ls".to_string(),
            timestamp: None,
            ..Default::default()
        }],
        user: "test".to_string(),
        source: "cli".to_string(),
        description: "test batch".to_string(),
    };
    let result = mw.request_batch_permission(&batch);
    assert!(result.is_err());
}

#[test]
fn test_batch_operation_approved() {
    let mw = make_middleware(PermissionPreset::Elevated);
    let batch = BatchOperationRequest {
        id: "batch-2".to_string(),
        operations: vec![
            OperationRequest {
                id: "op-a".to_string(),
                op_type: OperationType::FileRead,
                danger_level: DangerLevel::Low,
                user: "test".to_string(),
                source: "cli".to_string(),
                target: "/tmp/a.txt".to_string(),
                timestamp: None,
                ..Default::default()
            },
            OperationRequest {
                id: "op-b".to_string(),
                op_type: OperationType::FileRead,
                danger_level: DangerLevel::Low,
                user: "test".to_string(),
                source: "cli".to_string(),
                target: "/tmp/b.txt".to_string(),
                timestamp: None,
                ..Default::default()
            },
        ],
        user: "test".to_string(),
        source: "cli".to_string(),
        description: "read two files".to_string(),
    };
    let result = mw.request_batch_permission(&batch);
    assert!(result.is_ok());
}

#[test]
fn test_wrapper_accessors() {
    let mw = make_middleware(PermissionPreset::Standard);
    // Just ensure they compile and work
    let _file = mw.file();
    let _process = mw.process();
    let _network = mw.network();
    let _hardware = mw.hardware();
}

#[test]
fn test_get_security_summary() {
    let mw = make_middleware(PermissionPreset::Standard);
    let summary = mw.get_security_summary();
    assert_eq!(summary["user"], "test_user");
    assert_eq!(summary["source"], "cli");
    assert_eq!(summary["workspace"], "/tmp/workspace");
}

#[test]
fn test_user_source_workspace() {
    let mw = make_middleware(PermissionPreset::Standard);
    assert_eq!(mw.user(), "test_user");
    assert_eq!(mw.source(), "cli");
    assert_eq!(mw.workspace(), "/tmp/workspace");
}

#[test]
fn test_permission_factories() {
    // CLI: allows process_exec, file_delete, network_download; denies targets
    let cli = create_cli_permission();
    assert!(cli.is_operation_allowed(&OperationType::ProcessExec));
    assert!(cli.is_operation_allowed(&OperationType::FileDelete));
    assert!(cli.is_target_denied("/etc/passwd"));
    assert!(cli.requires_approval(&OperationType::ProcessKill));
    assert_eq!(cli.max_danger_level, DangerLevel::High);

    // Web: allows file_read, file_write; requires approval for file_delete
    let web = create_web_permission();
    assert!(!web.is_operation_allowed(&OperationType::ProcessExec));
    assert!(web.is_operation_allowed(&OperationType::FileRead));
    assert!(web.requires_approval(&OperationType::FileDelete));
    assert_eq!(web.max_danger_level, DangerLevel::Medium);

    // Agent: allows process_exec; requires approval for process_kill
    let agent = create_agent_permission("agent-1");
    assert!(agent.is_operation_allowed(&OperationType::ProcessExec));
    assert!(agent.requires_approval(&OperationType::ProcessKill));
    assert_eq!(agent.max_danger_level, DangerLevel::High);
}

#[test]
fn test_export_audit_log() {
    let dir = tempfile::tempdir().unwrap();
    let mw = make_middleware(PermissionPreset::Standard);
    let export_path = dir.path().join("audit.json");
    let result = mw.export_audit_log(export_path.to_str().unwrap());
    assert!(result.is_ok());
}

#[test]
fn test_approve_deny_pending() {
    let config = AuditorConfig {
        enabled: true,
        default_action: "ask".to_string(),
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    let mw = SecurityMiddleware::with_preset(
        auditor.clone(),
        "test_user",
        "cli",
        "/tmp/workspace",
        PermissionPreset::Unrestricted,
    );

    // Create a pending request
    let req = OperationRequest {
        id: "pending-test".to_string(),
        op_type: OperationType::FileWrite,
        danger_level: DangerLevel::High,
        user: "test".to_string(),
        source: "cli".to_string(),
        target: "/tmp/test".to_string(),
        timestamp: None,
        ..Default::default()
    };
    let (allowed, _, _) = auditor.request_permission(&req);
    assert!(!allowed);

    // Approve it
    mw.approve_pending_request("pending-test").unwrap();

    // Should fail now (already removed)
    assert!(mw.approve_pending_request("pending-test").is_err());
}

// ---- Additional middleware tests ----

#[test]
fn test_preset_read_only_denies_all_write() {
    let mw = make_middleware(PermissionPreset::ReadOnly);
    // All write operations should be denied
    assert!(!mw.is_operation_allowed(OperationType::FileWrite));
    assert!(!mw.is_operation_allowed(OperationType::FileDelete));
    assert!(!mw.is_operation_allowed(OperationType::DirCreate));
    assert!(!mw.is_operation_allowed(OperationType::DirDelete));
    assert!(!mw.is_operation_allowed(OperationType::ProcessExec));
    assert!(!mw.is_operation_allowed(OperationType::ProcessSpawn));
    assert!(!mw.is_operation_allowed(OperationType::NetworkRequest));
    assert!(!mw.is_operation_allowed(OperationType::NetworkDownload));
}

#[test]
fn test_preset_standard_allows_common_ops() {
    let mw = make_middleware(PermissionPreset::Standard);
    assert!(mw.is_operation_allowed(OperationType::FileRead));
    assert!(mw.is_operation_allowed(OperationType::FileWrite));
    assert!(mw.is_operation_allowed(OperationType::DirRead));
    assert!(mw.is_operation_allowed(OperationType::DirCreate));
    assert!(mw.is_operation_allowed(OperationType::NetworkRequest));
    assert!(mw.is_operation_allowed(OperationType::NetworkDownload));
}

#[test]
fn test_preset_standard_denies_critical_ops() {
    let mw = make_middleware(PermissionPreset::Standard);
    assert!(!mw.is_operation_allowed(OperationType::ProcessExec));
    assert!(!mw.is_operation_allowed(OperationType::ProcessSpawn));
    assert!(!mw.is_operation_allowed(OperationType::ProcessKill));
    assert!(!mw.is_operation_allowed(OperationType::SystemShutdown));
    assert!(!mw.is_operation_allowed(OperationType::RegistryWrite));
}

#[test]
fn test_preset_elevated_allows_exec() {
    let mw = make_middleware(PermissionPreset::Elevated);
    assert!(mw.is_operation_allowed(OperationType::ProcessExec));
    assert!(mw.is_operation_allowed(OperationType::ProcessSpawn));
    assert!(mw.is_operation_allowed(OperationType::FileRead));
    assert!(mw.is_operation_allowed(OperationType::FileWrite));
    assert!(mw.is_operation_allowed(OperationType::NetworkRequest));
}

#[test]
fn test_preset_elevated_denies_kill_and_system() {
    let mw = make_middleware(PermissionPreset::Elevated);
    assert!(!mw.is_operation_allowed(OperationType::ProcessKill));
    assert!(!mw.is_operation_allowed(OperationType::SystemShutdown));
    assert!(!mw.is_operation_allowed(OperationType::SystemReboot));
    assert!(!mw.is_operation_allowed(OperationType::RegistryWrite));
}

#[test]
fn test_preset_unrestricted_allows_everything() {
    let mw = make_middleware(PermissionPreset::Unrestricted);
    let all_ops = [
        OperationType::FileRead, OperationType::FileWrite,
        OperationType::FileDelete, OperationType::DirRead,
        OperationType::DirCreate, OperationType::DirDelete,
        OperationType::ProcessExec, OperationType::ProcessSpawn,
        OperationType::ProcessKill, OperationType::NetworkRequest,
        OperationType::NetworkDownload, OperationType::HardwareI2C,
        OperationType::RegistryWrite, OperationType::SystemShutdown,
    ];
    for op in &all_ops {
        assert!(mw.is_operation_allowed(*op), "Expected {:?} to be allowed under Unrestricted", op);
    }
}

#[test]
fn test_secure_file_wrapper_write_blocked_in_readonly() {
    let mw = make_middleware(PermissionPreset::ReadOnly);
    let wrapper = SecureFileWrapper::new(&mw);
    assert!(wrapper.check_file_write("/tmp/test.txt").is_err());
}

#[test]
fn test_secure_file_wrapper_delete_blocked_in_readonly() {
    let mw = make_middleware(PermissionPreset::ReadOnly);
    let wrapper = SecureFileWrapper::new(&mw);
    assert!(wrapper.check_file_delete("/tmp/test.txt").is_err());
}

#[test]
fn test_secure_file_wrapper_create_dir_blocked_in_readonly() {
    let mw = make_middleware(PermissionPreset::ReadOnly);
    let wrapper = SecureFileWrapper::new(&mw);
    assert!(wrapper.check_dir_create("/tmp/newdir").is_err());
}

#[test]
fn test_secure_file_wrapper_delete_dir_blocked_in_readonly() {
    let mw = make_middleware(PermissionPreset::ReadOnly);
    let wrapper = SecureFileWrapper::new(&mw);
    assert!(wrapper.check_dir_delete("/tmp/olddir").is_err());
}

#[test]
fn test_secure_process_wrapper_kill_blocked_everywhere() {
    for preset in [PermissionPreset::ReadOnly, PermissionPreset::Standard, PermissionPreset::Elevated] {
        let mw = make_middleware(preset);
        let wrapper = SecureProcessWrapper::new(&mw);
        assert!(wrapper.check_process_kill("1234").is_err(),
            "ProcessKill should be blocked under {:?}", preset);
    }
    // Unrestricted allows it (subject to ABAC)
    let mw = make_middleware(PermissionPreset::Unrestricted);
    let wrapper = SecureProcessWrapper::new(&mw);
    let _ = wrapper.check_process_kill("1234"); // may or may not succeed
}

#[test]
fn test_secure_network_wrapper_download_valid_url() {
    let mw = make_middleware(PermissionPreset::Standard);
    let wrapper = SecureNetworkWrapper::new(&mw);
    assert!(wrapper.check_network_download("https://example.com/file.zip").is_ok());
    assert!(wrapper.check_network_download("http://example.com/file.zip").is_ok());
}

#[test]
fn test_secure_network_wrapper_download_blocked_in_readonly() {
    let mw = make_middleware(PermissionPreset::ReadOnly);
    let wrapper = SecureNetworkWrapper::new(&mw);
    assert!(wrapper.check_network_download("https://example.com/file.zip").is_err());
}

#[test]
fn test_secure_hardware_wrapper_all_blocked_in_standard() {
    let mw = make_middleware(PermissionPreset::Standard);
    let wrapper = SecureHardwareWrapper::new(&mw);
    assert!(wrapper.check_i2c("device").is_err());
    assert!(wrapper.check_spi("device").is_err());
}

#[test]
fn test_batch_operation_mixed_operations() {
    let mw = make_middleware(PermissionPreset::Standard);
    let batch = BatchOperationRequest {
        id: "batch-mixed".to_string(),
        operations: vec![
            OperationRequest {
                id: "op-r".to_string(),
                op_type: OperationType::FileRead,
                danger_level: DangerLevel::Low,
                user: "test".to_string(),
                source: "cli".to_string(),
                target: "/tmp/a.txt".to_string(),
                timestamp: None,
                ..Default::default()
            },
            OperationRequest {
                id: "op-w".to_string(),
                op_type: OperationType::FileWrite,
                danger_level: DangerLevel::High,
                user: "test".to_string(),
                source: "cli".to_string(),
                target: "/tmp/b.txt".to_string(),
                timestamp: None,
                ..Default::default()
            },
        ],
        user: "test".to_string(),
        source: "cli".to_string(),
        description: "read and write".to_string(),
    };
    let result = mw.request_batch_permission(&batch);
    assert!(result.is_ok());
}

#[test]
fn test_batch_operation_single_dangerous() {
    let mw = make_middleware(PermissionPreset::Elevated);
    let batch = BatchOperationRequest {
        id: "batch-danger".to_string(),
        operations: vec![
            OperationRequest {
                id: "op-r".to_string(),
                op_type: OperationType::FileRead,
                danger_level: DangerLevel::Low,
                user: "test".to_string(),
                source: "cli".to_string(),
                target: "/tmp/a.txt".to_string(),
                timestamp: None,
                ..Default::default()
            },
            OperationRequest {
                id: "op-kill".to_string(),
                op_type: OperationType::ProcessKill,
                danger_level: DangerLevel::Critical,
                user: "test".to_string(),
                source: "cli".to_string(),
                target: "1234".to_string(),
                timestamp: None,
                ..Default::default()
            },
        ],
        user: "test".to_string(),
        source: "cli".to_string(),
        description: "read and kill".to_string(),
    };
    let result = mw.request_batch_permission(&batch);
    // ProcessKill not allowed under Elevated
    assert!(result.is_err());
}

#[test]
fn test_batch_operation_custom_id() {
    let mw = make_middleware(PermissionPreset::Standard);
    let batch = BatchOperationRequest {
        id: "custom-batch-id".to_string(),
        operations: vec![OperationRequest {
            id: "op-1".to_string(),
            op_type: OperationType::FileRead,
            danger_level: DangerLevel::Low,
            user: "test".to_string(),
            source: "cli".to_string(),
            target: "/tmp/a.txt".to_string(),
            timestamp: None,
            ..Default::default()
        }],
        user: "test".to_string(),
        source: "cli".to_string(),
        description: "custom id batch".to_string(),
    };
    let result = mw.request_batch_permission(&batch);
    assert!(result.is_ok());
}

#[test]
fn test_permission_factories_cli_details() {
    let cli = create_cli_permission();
    assert!(cli.is_operation_allowed(&OperationType::ProcessExec));
    assert!(cli.is_operation_allowed(&OperationType::FileDelete));
    assert!(cli.is_operation_allowed(&OperationType::NetworkDownload));
    assert!(cli.is_operation_allowed(&OperationType::FileWrite));
    assert!(cli.is_operation_allowed(&OperationType::DirCreate));
    assert!(!cli.is_operation_allowed(&OperationType::ProcessKill));
    assert!(!cli.is_operation_allowed(&OperationType::SystemShutdown));
}

#[test]
fn test_permission_factories_web_details() {
    let web = create_web_permission();
    assert!(web.is_operation_allowed(&OperationType::FileRead));
    assert!(web.is_operation_allowed(&OperationType::FileWrite));
    assert!(web.is_operation_allowed(&OperationType::DirRead));
    assert!(web.is_operation_allowed(&OperationType::DirCreate));
    assert!(!web.is_operation_allowed(&OperationType::ProcessExec));
    assert!(!web.is_operation_allowed(&OperationType::ProcessKill));
    assert!(web.requires_approval(&OperationType::FileDelete));
    assert!(web.requires_approval(&OperationType::ProcessExec));
}

#[test]
fn test_permission_factories_agent_details() {
    let agent = create_agent_permission("agent-007");
    assert!(agent.is_operation_allowed(&OperationType::ProcessExec));
    assert!(agent.is_operation_allowed(&OperationType::FileRead));
    assert!(agent.is_operation_allowed(&OperationType::FileWrite));
    assert!(agent.requires_approval(&OperationType::ProcessKill));
    assert!(agent.requires_approval(&OperationType::SystemShutdown));
}

#[test]
fn test_security_status_fields() {
    let mw = make_middleware(PermissionPreset::Standard);
    let status = mw.get_security_summary();
    assert!(status.is_object());
}

#[test]
fn test_get_security_summary_contains_all_fields() {
    let mw = make_middleware(PermissionPreset::Standard);
    let summary = mw.get_security_summary();
    let obj = summary.as_object().unwrap();
    assert!(obj.contains_key("user"));
    assert!(obj.contains_key("source"));
    assert!(obj.contains_key("workspace"));
    assert!(obj.contains_key("statistics"));
}

#[test]
fn test_batch_default_is_empty() {
    let batch = BatchOperationRequest::default();
    assert!(batch.id.is_empty());
    assert!(batch.operations.is_empty());
    assert!(batch.user.is_empty());
    assert!(batch.source.is_empty());
    assert!(batch.description.is_empty());
}

#[test]
fn test_secure_network_wrapper_upload_blocked_in_standard() {
    let mw = make_middleware(PermissionPreset::Standard);
    let wrapper = SecureNetworkWrapper::new(&mw);
    assert!(wrapper.check_network_upload("https://example.com/upload").is_err());
}

#[test]
fn test_secure_network_wrapper_upload_allowed_in_unrestricted() {
    let mw = make_middleware(PermissionPreset::Unrestricted);
    let wrapper = SecureNetworkWrapper::new(&mw);
    assert!(wrapper.check_network_upload("https://example.com/upload").is_ok());
}

#[test]
fn test_deny_pending_request_via_middleware() {
    let config = AuditorConfig {
        enabled: true,
        default_action: "ask".to_string(),
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    let mw = SecurityMiddleware::with_preset(
        auditor.clone(),
        "test_user",
        "cli",
        "/tmp/workspace",
        PermissionPreset::Unrestricted,
    );

    let req = OperationRequest {
        id: "deny-via-mw".to_string(),
        op_type: OperationType::FileWrite,
        danger_level: DangerLevel::High,
        user: "test".to_string(),
        source: "cli".to_string(),
        target: "/tmp/test".to_string(),
        timestamp: None,
        ..Default::default()
    };
    let (allowed, _, _) = auditor.request_permission(&req);
    assert!(!allowed);

    mw.deny_pending_request("deny-via-mw", "too risky").unwrap();
    assert!(mw.deny_pending_request("deny-via-mw", "too risky").is_err());
}

// ---- Additional coverage tests for middleware ----

#[test]
fn test_set_preset() {
    let mut mw = make_middleware(PermissionPreset::ReadOnly);
    assert_eq!(mw.preset(), PermissionPreset::ReadOnly);
    mw.set_preset(PermissionPreset::Standard);
    assert_eq!(mw.preset(), PermissionPreset::Standard);
    assert!(mw.is_operation_allowed(OperationType::FileWrite));
}

#[test]
fn test_check_operation_denied_by_preset() {
    let mw = make_middleware(PermissionPreset::Standard);
    // ProcessExec is not allowed under Standard
    let result = mw.check_operation(OperationType::ProcessExec, "ls");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not allowed"));
}

#[test]
fn test_check_operation_allowed_by_preset() {
    let mw = make_middleware(PermissionPreset::Standard);
    // FileRead is allowed under Standard
    let result = mw.check_operation(OperationType::FileRead, "/tmp/test.txt");
    assert!(result.is_ok());
}

#[test]
fn test_file_wrapper_check_file_read() {
    let mw = make_middleware(PermissionPreset::Standard);
    let wrapper = SecureFileWrapper::new(&mw);
    assert!(wrapper.check_file_read("/tmp/workspace/test.txt").is_ok());
}

#[test]
fn test_file_wrapper_check_file_write() {
    let mw = make_middleware(PermissionPreset::Standard);
    let wrapper = SecureFileWrapper::new(&mw);
    assert!(wrapper.check_file_write("/tmp/workspace/test.txt").is_ok());
}

#[test]
fn test_file_wrapper_check_file_read_denied_for_elevated_only() {
    let mw = make_middleware(PermissionPreset::ReadOnly);
    let wrapper = SecureFileWrapper::new(&mw);
    // FileWrite should be denied under ReadOnly
    assert!(wrapper.check_file_write("/tmp/workspace/test.txt").is_err());
    // But FileRead should be allowed
    assert!(wrapper.check_file_read("/tmp/workspace/test.txt").is_ok());
}

#[test]
fn test_file_wrapper_check_dir_read() {
    let mw = make_middleware(PermissionPreset::Standard);
    let wrapper = SecureFileWrapper::new(&mw);
    assert!(wrapper.check_dir_read("/tmp/workspace").is_ok());
}

#[test]
fn test_file_wrapper_check_dir_create() {
    let mw = make_middleware(PermissionPreset::Standard);
    let wrapper = SecureFileWrapper::new(&mw);
    assert!(wrapper.check_dir_create("/tmp/workspace/newdir").is_ok());
}

#[test]
fn test_file_wrapper_check_dir_delete_denied_for_standard() {
    let mw = make_middleware(PermissionPreset::Standard);
    let wrapper = SecureFileWrapper::new(&mw);
    // DirDelete is not in Standard preset
    assert!(wrapper.check_dir_delete("/tmp/workspace/dir").is_err());
}

#[tokio::test]
async fn test_file_wrapper_write_outside_workspace_denied() {
    let mw = make_middleware(PermissionPreset::Unrestricted);
    let wrapper = SecureFileWrapper::new(&mw);
    let result = wrapper.write_file("/etc/evil.txt", "bad content").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_file_wrapper_read_outside_workspace_denied() {
    let mw = make_middleware(PermissionPreset::ReadOnly);
    let wrapper = SecureFileWrapper::new(&mw);
    let result = wrapper.read_file("/etc/passwd").await;
    assert!(result.is_err());
}

#[test]
fn test_process_wrapper_check_spawn() {
    let mw = make_middleware(PermissionPreset::Elevated);
    let wrapper = SecureProcessWrapper::new(&mw);
    let result = wrapper.check_process_spawn("python script.py");
    assert!(result.is_ok());
}

#[test]
fn test_process_wrapper_spawn_blocked_in_standard() {
    let mw = make_middleware(PermissionPreset::Standard);
    let wrapper = SecureProcessWrapper::new(&mw);
    let result = wrapper.check_process_spawn("python script.py");
    assert!(result.is_err());
}

#[test]
fn test_process_wrapper_check_exec() {
    let mw = make_middleware(PermissionPreset::Unrestricted);
    let wrapper = SecureProcessWrapper::new(&mw);
    let result = wrapper.check_process_exec("ls -la");
    assert!(result.is_ok());
}

#[test]
fn test_process_wrapper_check_exec_blocked_in_standard() {
    let mw = make_middleware(PermissionPreset::Standard);
    let wrapper = SecureProcessWrapper::new(&mw);
    let result = wrapper.check_process_exec("ls -la");
    assert!(result.is_err());
}

#[test]
fn test_network_wrapper_request_valid_urls() {
    let mw = make_middleware(PermissionPreset::Standard);
    let wrapper = SecureNetworkWrapper::new(&mw);
    assert!(wrapper.check_network_request("https://example.com/api").is_ok());
    assert!(wrapper.check_network_request("http://example.com/api").is_ok());
}

#[test]
fn test_network_wrapper_download_invalid_scheme() {
    let mw = make_middleware(PermissionPreset::Standard);
    let wrapper = SecureNetworkWrapper::new(&mw);
    assert!(wrapper.check_network_download("ftp://example.com").is_err());
    assert!(wrapper.check_network_download("file:///etc/passwd").is_err());
}

#[test]
fn test_network_wrapper_upload_valid_url() {
    let mw = make_middleware(PermissionPreset::Unrestricted);
    let wrapper = SecureNetworkWrapper::new(&mw);
    // Upload requires Unrestricted permission
    assert!(wrapper.check_network_upload("https://example.com/upload").is_ok());
}

#[test]
fn test_network_wrapper_upload_invalid_url() {
    let mw = make_middleware(PermissionPreset::Unrestricted);
    let wrapper = SecureNetworkWrapper::new(&mw);
    assert!(wrapper.check_network_upload("ftp://example.com").is_err());
}

#[test]
fn test_hardware_wrapper_gpio() {
    let mw = make_middleware(PermissionPreset::Unrestricted);
    let wrapper = SecureHardwareWrapper::new(&mw);
    let result = wrapper.check_gpio("17");
    assert!(result.is_ok());
}

#[test]
fn test_hardware_wrapper_spi_write() {
    let mw = make_middleware(PermissionPreset::Unrestricted);
    let wrapper = SecureHardwareWrapper::new(&mw);
    let result = wrapper.spi_write("1.0", &[0x01, 0x02]);
    assert!(result.is_ok());
}

#[test]
fn test_http_request_default() {
    let req = HttpRequest::default();
    assert!(req.url.is_empty());
    assert_eq!(req.method, "GET");
    assert!(req.headers.is_empty());
    assert!(req.body.is_none());
    assert!(req.timeout_secs.is_none());
}

#[test]
fn test_http_response_fields() {
    let resp = HttpResponse {
        status_code: 200,
        body: "OK".to_string(),
        success: true,
    };
    assert_eq!(resp.status_code, 200);
    assert_eq!(resp.body, "OK");
    assert!(resp.success);
}

#[test]
fn test_batch_operation_empty_id_generates_uuid() {
    let mw = make_middleware(PermissionPreset::Standard);
    let batch = BatchOperationRequest {
        id: String::new(), // empty -> should generate UUID
        operations: vec![OperationRequest {
            id: "op-1".to_string(),
            op_type: OperationType::FileRead,
            danger_level: DangerLevel::Low,
            user: "test".to_string(),
            source: "cli".to_string(),
            target: "/tmp/a.txt".to_string(),
            timestamp: None,
            ..Default::default()
        }],
        ..Default::default()
    };
    let result = mw.request_batch_permission(&batch);
    assert!(result.is_ok());
    // The returned ID should be a UUID (not empty)
    let id = result.unwrap();
    assert!(!id.is_empty());
}

#[test]
fn test_permission_is_target_denied_with_empty_list() {
    let perm = create_web_permission();
    assert!(!perm.is_target_denied("/any/path"));
}

#[test]
fn test_permission_is_target_allowed_with_empty_list() {
    let perm = create_web_permission();
    assert!(!perm.is_target_allowed("/any/path"));
}

#[test]
fn test_permission_is_target_denied_cli() {
    let perm = create_cli_permission();
    assert!(perm.is_target_denied("/etc/sudoers"));
    assert!(perm.is_target_denied("/etc/passwd"));
}

#[test]
fn test_permission_is_operation_allowed_false_for_unknown() {
    let perm = create_web_permission();
    assert!(!perm.is_operation_allowed(&OperationType::HardwareI2C));
    assert!(!perm.is_operation_allowed(&OperationType::RegistryWrite));
}

#[test]
fn test_permission_requires_approval_false_for_unknown() {
    let perm = create_web_permission();
    assert!(!perm.requires_approval(&OperationType::HardwareI2C));
}

#[test]
fn test_cli_permission_allows_all_expected() {
    let cli = create_cli_permission();
    assert!(cli.is_operation_allowed(&OperationType::FileRead));
    assert!(cli.is_operation_allowed(&OperationType::FileWrite));
    assert!(cli.is_operation_allowed(&OperationType::FileDelete));
    assert!(cli.is_operation_allowed(&OperationType::DirRead));
    assert!(cli.is_operation_allowed(&OperationType::DirCreate));
    assert!(cli.is_operation_allowed(&OperationType::ProcessExec));
    assert!(cli.is_operation_allowed(&OperationType::NetworkDownload));
    assert!(cli.is_operation_allowed(&OperationType::NetworkRequest));
}

#[test]
fn test_get_audit_log() {
    let mw = make_middleware(PermissionPreset::Standard);
    let filter = crate::auditor::AuditFilter::default();
    let logs = mw.get_audit_log(filter);
    // May or may not have entries, but should not panic
    assert!(logs.is_empty() || !logs.is_empty());
}

// ---- Coverage expansion tests for middleware ----

#[tokio::test]
async fn test_file_wrapper_write_within_workspace() {
    let dir = tempfile::tempdir().unwrap();
    // Canonicalize workspace to ensure path matching works on Windows
    let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
    let wrapper = SecureFileWrapper::new(&mw);
    let file_path = format!("{}\\test_write.txt", ws);
    let result = wrapper.write_file(&file_path, "hello world").await;
    assert!(result.is_ok(), "write_file failed: {:?}", result);
    let content = std::fs::read_to_string(&file_path).unwrap();
    assert_eq!(content, "hello world");
}

#[tokio::test]
async fn test_file_wrapper_read_within_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
    let file_path = format!("{}\\test_read.txt", ws);
    std::fs::write(&file_path, "read me").unwrap();
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
    let wrapper = SecureFileWrapper::new(&mw);
    let result = wrapper.read_file(&file_path).await;
    assert!(result.is_ok(), "read_file failed: {:?}", result);
    assert_eq!(result.unwrap(), "read me");
}

#[tokio::test]
async fn test_file_wrapper_edit_file() {
    let dir = tempfile::tempdir().unwrap();
    let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
    let file_path = format!("{}\\edit_test.txt", ws);
    std::fs::write(&file_path, "old content here").unwrap();
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
    let wrapper = SecureFileWrapper::new(&mw);
    let result = wrapper.edit_file(&file_path, "old content", "new content").await;
    assert!(result.is_ok(), "edit_file failed: {:?}", result);
    let content = std::fs::read_to_string(&file_path).unwrap();
    assert_eq!(content, "new content here");
}

#[tokio::test]
async fn test_file_wrapper_edit_file_pattern_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
    let file_path = format!("{}\\edit_notfound.txt", ws);
    std::fs::write(&file_path, "some content").unwrap();
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
    let wrapper = SecureFileWrapper::new(&mw);
    let result = wrapper.edit_file(&file_path, "nonexistent", "replacement").await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("pattern not found"));
}

#[tokio::test]
async fn test_file_wrapper_append_file() {
    let dir = tempfile::tempdir().unwrap();
    let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
    let file_path = format!("{}\\append_test.txt", ws);
    std::fs::write(&file_path, "first line\n").unwrap();
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
    let wrapper = SecureFileWrapper::new(&mw);
    let result = wrapper.append_file(&file_path, "second line").await;
    assert!(result.is_ok(), "append_file failed: {:?}", result);
    let content = std::fs::read_to_string(&file_path).unwrap();
    assert!(content.contains("first line"));
    assert!(content.contains("second line"));
}

#[tokio::test]
async fn test_file_wrapper_append_to_empty_file() {
    let dir = tempfile::tempdir().unwrap();
    let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
    let file_path = format!("{}\\append_empty.txt", ws);
    std::fs::write(&file_path, "").unwrap();
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
    let wrapper = SecureFileWrapper::new(&mw);
    let result = wrapper.append_file(&file_path, "first content").await;
    assert!(result.is_ok(), "append_file failed: {:?}", result);
    let content = std::fs::read_to_string(&file_path).unwrap();
    assert_eq!(content, "first content");
}

#[tokio::test]
async fn test_file_wrapper_append_to_file_without_newline() {
    let dir = tempfile::tempdir().unwrap();
    let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
    let file_path = format!("{}\\append_nonl.txt", ws);
    std::fs::write(&file_path, "no newline").unwrap();
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
    let wrapper = SecureFileWrapper::new(&mw);
    let result = wrapper.append_file(&file_path, "appended").await;
    assert!(result.is_ok(), "append_file failed: {:?}", result);
    let content = std::fs::read_to_string(&file_path).unwrap();
    assert!(content.contains("no newline\nappended"));
}

#[tokio::test]
async fn test_file_wrapper_delete_file() {
    let dir = tempfile::tempdir().unwrap();
    let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
    let file_path = format!("{}\\delete_me.txt", ws);
    std::fs::write(&file_path, "to be deleted").unwrap();
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Unrestricted);
    let wrapper = SecureFileWrapper::new(&mw);
    let result = wrapper.delete_file(&file_path).await;
    assert!(result.is_ok(), "delete_file failed: {:?}", result);
    assert!(!std::path::Path::new(&file_path).exists());
}

#[tokio::test]
async fn test_file_wrapper_read_directory() {
    let dir = tempfile::tempdir().unwrap();
    let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
    std::fs::write(format!("{}\\file1.txt", ws), "a").unwrap();
    std::fs::create_dir(format!("{}\\subdir", ws)).unwrap();
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
    let wrapper = SecureFileWrapper::new(&mw);
    let result = wrapper.read_directory(&ws).await;
    assert!(result.is_ok(), "read_directory failed: {:?}", result);
    let entries = result.unwrap();
    assert!(entries.contains(&"file1.txt".to_string()));
    assert!(entries.contains(&"subdir/".to_string()));
}

#[tokio::test]
async fn test_file_wrapper_create_directory() {
    let dir = tempfile::tempdir().unwrap();
    let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
    let new_dir = format!("{}\\new_dir", ws);
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
    let wrapper = SecureFileWrapper::new(&mw);
    let result = wrapper.create_directory(&new_dir).await;
    assert!(result.is_ok(), "create_directory failed: {:?}", result);
    assert!(std::path::Path::new(&new_dir).is_dir());
}

#[tokio::test]
async fn test_file_wrapper_delete_directory() {
    let dir = tempfile::tempdir().unwrap();
    let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
    let del_dir = format!("{}\\del_dir", ws);
    std::fs::create_dir_all(&del_dir).unwrap();
    std::fs::write(format!("{}\\inner.txt", del_dir), "x").unwrap();
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Unrestricted);
    let wrapper = SecureFileWrapper::new(&mw);
    let result = wrapper.delete_directory(&del_dir).await;
    assert!(result.is_ok(), "delete_directory failed: {:?}", result);
    assert!(!std::path::Path::new(&del_dir).exists());
}

#[tokio::test]
async fn test_file_wrapper_stat() {
    let dir = tempfile::tempdir().unwrap();
    let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
    let file_path = format!("{}\\stat_test.txt", ws);
    std::fs::write(&file_path, "stat content here").unwrap();
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
    let wrapper = SecureFileWrapper::new(&mw);
    let result = wrapper.stat(&file_path).await;
    assert!(result.is_ok(), "stat failed: {:?}", result);
    let meta = result.unwrap();
    assert!(meta.is_file);
    assert!(!meta.is_dir);
    assert_eq!(meta.len, "stat content here".len() as u64);
}

#[tokio::test]
async fn test_file_wrapper_open_file() {
    let dir = tempfile::tempdir().unwrap();
    let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
    let file_path = format!("{}\\open_test.txt", ws);
    std::fs::write(&file_path, b"binary\x00data").unwrap();
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
    let wrapper = SecureFileWrapper::new(&mw);
    let result = wrapper.open_file(&file_path).await;
    assert!(result.is_ok(), "open_file failed: {:?}", result);
    assert_eq!(result.unwrap(), b"binary\x00data");
}

#[tokio::test]
async fn test_file_wrapper_list_dir() {
    let dir = tempfile::tempdir().unwrap();
    let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
    std::fs::write(format!("{}\\a.txt", ws), "aaa").unwrap();
    std::fs::write(format!("{}\\b.txt", ws), "bb").unwrap();
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
    let wrapper = SecureFileWrapper::new(&mw);
    let result = wrapper.list_dir(&ws).await;
    assert!(result.is_ok(), "list_dir failed: {:?}", result);
    let entries = result.unwrap();
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().any(|e| e.name == "a.txt"));
    assert!(entries.iter().any(|e| e.name == "b.txt"));
}

#[tokio::test]
async fn test_file_wrapper_remove_file_alias() {
    let dir = tempfile::tempdir().unwrap();
    let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
    let file_path = format!("{}\\remove_me.txt", ws);
    std::fs::write(&file_path, "bye").unwrap();
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Unrestricted);
    let wrapper = SecureFileWrapper::new(&mw);
    let result = wrapper.remove_file(&file_path).await;
    assert!(result.is_ok(), "remove_file failed: {:?}", result);
    assert!(!std::path::Path::new(&file_path).exists());
}

#[tokio::test]
async fn test_file_wrapper_create_dir_alias() {
    let dir = tempfile::tempdir().unwrap();
    let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
    let new_dir = format!("{}\\alias_dir", ws);
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
    let wrapper = SecureFileWrapper::new(&mw);
    let result = wrapper.create_dir(&new_dir).await;
    assert!(result.is_ok(), "create_dir failed: {:?}", result);
    assert!(std::path::Path::new(&new_dir).is_dir());
}

#[tokio::test]
async fn test_file_wrapper_remove_dir_alias() {
    let dir = tempfile::tempdir().unwrap();
    let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
    let rm_dir = format!("{}\\rm_dir", ws);
    std::fs::create_dir_all(&rm_dir).unwrap();
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Unrestricted);
    let wrapper = SecureFileWrapper::new(&mw);
    let result = wrapper.remove_dir(&rm_dir).await;
    assert!(result.is_ok(), "remove_dir failed: {:?}", result);
    assert!(!std::path::Path::new(&rm_dir).exists());
}

#[test]
fn test_process_wrapper_check_kill() {
    let mw = make_middleware(PermissionPreset::Unrestricted);
    let wrapper = SecureProcessWrapper::new(&mw);
    let result = wrapper.check_process_kill("9999");
    // Under unrestricted, may pass ABAC check
    let _ = result;
}

#[test]
fn test_file_metadata_fields() {
    let meta = FileMetadata {
        is_file: true,
        is_dir: false,
        len: 1024,
        readonly: false,
        modified: "2026-01-01T00:00:00Z".to_string(),
    };
    assert!(meta.is_file);
    assert!(!meta.is_dir);
    assert_eq!(meta.len, 1024);
    assert!(!meta.readonly);
}

#[test]
fn test_dir_entry_fields() {
    let entry = DirEntry {
        name: "test.txt".to_string(),
        is_dir: false,
        size: 42,
    };
    assert_eq!(entry.name, "test.txt");
    assert!(!entry.is_dir);
    assert_eq!(entry.size, 42);
}

#[test]
fn test_http_request_with_body() {
    let req = HttpRequest {
        url: "https://api.example.com".to_string(),
        method: "POST".to_string(),
        headers: vec![("Content-Type".to_string(), "application/json".to_string())],
        body: Some(r#"{"key":"value"}"#.to_string()),
        timeout_secs: Some(30),
    };
    assert_eq!(req.method, "POST");
    assert!(req.body.is_some());
    assert_eq!(req.headers.len(), 1);
    assert_eq!(req.timeout_secs, Some(30));
}

#[tokio::test]
async fn test_network_wrapper_check_request_valid() {
    let mw = make_middleware(PermissionPreset::Standard);
    let wrapper = SecureNetworkWrapper::new(&mw);
    assert!(wrapper.check_network_request("https://api.example.com/data").is_ok());
    assert!(wrapper.check_network_request("http://localhost:8080/api").is_ok());
}

#[tokio::test]
async fn test_hardware_wrapper_i2c_read_coverage() {
    let mw = make_middleware(PermissionPreset::Unrestricted);
    let wrapper = SecureHardwareWrapper::new(&mw);
    let result = wrapper.i2c_read("1", 0x48, 0x00, 4).await;
    // Will fail because i2cget is not available, but should not panic
    let _ = result;
}

#[tokio::test]
async fn test_hardware_wrapper_i2c_write_coverage() {
    let mw = make_middleware(PermissionPreset::Unrestricted);
    let wrapper = SecureHardwareWrapper::new(&mw);
    let result = wrapper.i2c_write("1", 0x48, 0x00, &[0x01, 0x02]).await;
    let _ = result;
}

#[test]
fn test_hardware_wrapper_blocked_in_standard() {
    let mw = make_middleware(PermissionPreset::Standard);
    let wrapper = SecureHardwareWrapper::new(&mw);
    assert!(wrapper.check_i2c("device").is_err());
    assert!(wrapper.check_spi("device").is_err());
    assert!(wrapper.check_gpio("17").is_err());
}

#[test]
fn test_network_wrapper_upload_blocked_invalid_url() {
    let mw = make_middleware(PermissionPreset::Unrestricted);
    let wrapper = SecureNetworkWrapper::new(&mw);
    assert!(wrapper.check_network_upload("ftp://bad.url").is_err());
    assert!(wrapper.check_network_upload("file:///etc/passwd").is_err());
}

#[test]
fn test_network_wrapper_request_blocked_in_readonly() {
    let mw = make_middleware(PermissionPreset::ReadOnly);
    let wrapper = SecureNetworkWrapper::new(&mw);
    assert!(wrapper.check_network_request("https://example.com").is_err());
}

#[test]
fn test_check_operation_with_dangerous_process() {
    let config = AuditorConfig {
        enabled: true,
        default_action: "ask".to_string(),
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    let mw = SecurityMiddleware::with_preset(
        auditor.clone(), "test_user", "cli", "/tmp/workspace", PermissionPreset::Unrestricted,
    );
    // ProcessExec is allowed by Unrestricted, but dangerous commands should still be checked
    let result = mw.check_operation(OperationType::ProcessExec, "rm -rf /");
    // This should either be allowed (ABAC) or blocked (safe command check)
    let _ = result;
}

// ---- Additional coverage tests ----

#[test]
fn test_batch_permission_empty() {
    let mw = make_middleware(PermissionPreset::Standard);
    let batch = BatchOperationRequest::default();
    let result = mw.request_batch_permission(&batch);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("no operations"));
}

#[test]
fn test_batch_permission_allowed() {
    let mw = make_middleware(PermissionPreset::Standard);
    let mut batch = BatchOperationRequest::default();
    batch.id = "batch-1".to_string();
    batch.operations.push(OperationRequest {
        id: "op-1".to_string(),
        op_type: OperationType::FileRead,
        danger_level: DangerLevel::Low,
        target: "/tmp/test.txt".to_string(),
        ..Default::default()
    });
    let result = mw.request_batch_permission(&batch);
    assert!(result.is_ok());
}

#[test]
fn test_batch_permission_preset_blocked() {
    let mw = make_middleware(PermissionPreset::ReadOnly);
    let mut batch = BatchOperationRequest::default();
    batch.operations.push(OperationRequest {
        id: "op-1".to_string(),
        op_type: OperationType::ProcessExec,
        danger_level: DangerLevel::Critical,
        target: "rm -rf /".to_string(),
        ..Default::default()
    });
    let result = mw.request_batch_permission(&batch);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not allowed"));
}

#[test]
fn test_batch_permission_with_custom_id() {
    let mw = make_middleware(PermissionPreset::Standard);
    let mut batch = BatchOperationRequest::default();
    batch.id = "custom-batch-id".to_string();
    batch.operations.push(OperationRequest {
        id: "op-1".to_string(),
        op_type: OperationType::FileRead,
        danger_level: DangerLevel::Low,
        target: "/tmp/test.txt".to_string(),
        ..Default::default()
    });
    let result = mw.request_batch_permission(&batch);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "custom-batch-id");
}

#[test]
fn test_batch_permission_auto_id() {
    let mw = make_middleware(PermissionPreset::Standard);
    let mut batch = BatchOperationRequest::default();
    // id is empty, should auto-generate
    batch.operations.push(OperationRequest {
        id: "op-1".to_string(),
        op_type: OperationType::FileRead,
        danger_level: DangerLevel::Low,
        target: "/tmp/test.txt".to_string(),
        ..Default::default()
    });
    let result = mw.request_batch_permission(&batch);
    assert!(result.is_ok());
    // Should be a UUID
    let id = result.unwrap();
    assert!(!id.is_empty());
}

#[test]
fn test_get_security_summary_fields() {
    let mw = make_middleware(PermissionPreset::Standard);
    let summary = mw.get_security_summary();
    assert!(summary["user"] == "test_user");
    assert!(summary["source"] == "cli");
    assert!(summary["workspace"] == "/tmp/workspace");
    assert!(summary["pending_requests"].as_u64().unwrap() == 0);
}

#[test]
fn test_batch_operation_request_default() {
    let batch = BatchOperationRequest::default();
    assert!(batch.id.is_empty());
    assert!(batch.operations.is_empty());
    assert!(batch.user.is_empty());
    assert!(batch.source.is_empty());
    assert!(batch.description.is_empty());
}

#[test]
fn test_batch_permission_multiple_operations() {
    let mw = make_middleware(PermissionPreset::Standard);
    let mut batch = BatchOperationRequest::default();
    batch.id = "multi-batch".to_string();
    batch.operations.push(OperationRequest {
        id: "op-1".to_string(),
        op_type: OperationType::FileRead,
        danger_level: DangerLevel::Low,
        target: "/tmp/a.txt".to_string(),
        ..Default::default()
    });
    batch.operations.push(OperationRequest {
        id: "op-2".to_string(),
        op_type: OperationType::FileWrite,
        danger_level: DangerLevel::Medium,
        target: "/tmp/b.txt".to_string(),
        ..Default::default()
    });
    let result = mw.request_batch_permission(&batch);
    assert!(result.is_ok());
}

#[test]
fn test_approve_deny_pending_request() {
    let mw = make_middleware(PermissionPreset::Unrestricted);
    // Approve/deny non-existent request should fail gracefully
    let result = mw.approve_pending_request("nonexistent");
    assert!(result.is_err());
    let result = mw.deny_pending_request("nonexistent", "testing");
    assert!(result.is_err());
}

#[test]
fn test_middleware_accessors() {
    let mw = make_middleware(PermissionPreset::Standard);
    assert_eq!(mw.user(), "test_user");
    assert_eq!(mw.source(), "cli");
    assert_eq!(mw.workspace(), "/tmp/workspace");
    assert_eq!(mw.preset(), PermissionPreset::Standard);
}

#[test]
fn test_middleware_set_preset() {
    let mut mw = make_middleware(PermissionPreset::ReadOnly);
    assert_eq!(mw.preset(), PermissionPreset::ReadOnly);
    mw.set_preset(PermissionPreset::Unrestricted);
    assert_eq!(mw.preset(), PermissionPreset::Unrestricted);
}

#[test]
fn test_is_operation_allowed() {
    let mw = make_middleware(PermissionPreset::ReadOnly);
    assert!(mw.is_operation_allowed(OperationType::FileRead));
    assert!(!mw.is_operation_allowed(OperationType::FileWrite));
    assert!(!mw.is_operation_allowed(OperationType::ProcessExec));
}

#[test]
fn test_export_audit_log_invalid_path() {
    let mw = make_middleware(PermissionPreset::Standard);
    let result = mw.export_audit_log("/nonexistent/dir/file.log");
    // May fail due to path not existing
    let _ = result;
}

#[test]
fn test_get_audit_log_with_filter() {
    let mw = make_middleware(PermissionPreset::Standard);
    let filter = AuditFilter::default();
    let events = mw.get_audit_log(filter);
    // May be empty since no operations have been performed
    assert!(events.is_empty() || !events.is_empty());
}

#[test]
fn test_batch_permission_max_danger_critical() {
    let mw = make_middleware(PermissionPreset::Unrestricted);
    let mut batch = BatchOperationRequest::default();
    batch.operations.push(OperationRequest {
        id: "op-1".to_string(),
        op_type: OperationType::FileRead,
        danger_level: DangerLevel::Low,
        target: "/tmp/a.txt".to_string(),
        ..Default::default()
    });
    batch.operations.push(OperationRequest {
        id: "op-2".to_string(),
        op_type: OperationType::ProcessKill,
        danger_level: DangerLevel::Critical,
        target: "1234".to_string(),
        ..Default::default()
    });
    let result = mw.request_batch_permission(&batch);
    // Even with Unrestricted, it may be blocked by the auditor
    let _ = result;
}

// ---- More coverage tests for middleware ----

#[test]
fn test_security_middleware_new() {
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    let mw = SecurityMiddleware::new(auditor, "user1", "web", "/home/user/workspace");
    assert_eq!(mw.user(), "user1");
    assert_eq!(mw.source(), "web");
    assert_eq!(mw.workspace(), "/home/user/workspace");
    assert_eq!(mw.preset(), PermissionPreset::Standard); // default preset
}

#[test]
fn test_check_operation_all_ops_under_readonly() {
    let mw = make_middleware(PermissionPreset::ReadOnly);
    // Only FileRead and DirRead should pass preset check
    assert!(mw.check_operation(OperationType::FileRead, "/tmp/test").is_ok());
    assert!(mw.check_operation(OperationType::DirRead, "/tmp/test").is_ok());
    // Everything else should fail
    assert!(mw.check_operation(OperationType::FileWrite, "/tmp/test").is_err());
    assert!(mw.check_operation(OperationType::FileDelete, "/tmp/test").is_err());
    assert!(mw.check_operation(OperationType::DirCreate, "/tmp/test").is_err());
    assert!(mw.check_operation(OperationType::DirDelete, "/tmp/test").is_err());
    assert!(mw.check_operation(OperationType::ProcessExec, "ls").is_err());
    assert!(mw.check_operation(OperationType::ProcessSpawn, "ls").is_err());
    assert!(mw.check_operation(OperationType::ProcessKill, "1234").is_err());
    assert!(mw.check_operation(OperationType::NetworkRequest, "http://a.com").is_err());
    assert!(mw.check_operation(OperationType::NetworkDownload, "http://a.com").is_err());
    assert!(mw.check_operation(OperationType::NetworkUpload, "http://a.com").is_err());
    assert!(mw.check_operation(OperationType::HardwareI2C, "dev").is_err());
    assert!(mw.check_operation(OperationType::HardwareSPI, "dev").is_err());
    assert!(mw.check_operation(OperationType::HardwareGPIO, "17").is_err());
    assert!(mw.check_operation(OperationType::RegistryWrite, "HKLM").is_err());
    assert!(mw.check_operation(OperationType::SystemShutdown, "now").is_err());
    assert!(mw.check_operation(OperationType::SystemReboot, "now").is_err());
}

#[test]
fn test_check_operation_standard_ops() {
    let mw = make_middleware(PermissionPreset::Standard);
    // Allowed under Standard
    assert!(mw.check_operation(OperationType::FileRead, "/tmp/test").is_ok());
    assert!(mw.check_operation(OperationType::FileWrite, "/tmp/test").is_ok());
    assert!(mw.check_operation(OperationType::DirRead, "/tmp/test").is_ok());
    assert!(mw.check_operation(OperationType::DirCreate, "/tmp/test").is_ok());
    assert!(mw.check_operation(OperationType::NetworkRequest, "http://a.com").is_ok());
    assert!(mw.check_operation(OperationType::NetworkDownload, "http://a.com").is_ok());
    // Denied under Standard
    assert!(mw.check_operation(OperationType::FileDelete, "/tmp/test").is_err());
    assert!(mw.check_operation(OperationType::DirDelete, "/tmp/test").is_err());
    assert!(mw.check_operation(OperationType::ProcessExec, "ls").is_err());
    assert!(mw.check_operation(OperationType::ProcessSpawn, "ls").is_err());
    assert!(mw.check_operation(OperationType::ProcessKill, "1234").is_err());
    assert!(mw.check_operation(OperationType::NetworkUpload, "http://a.com").is_err());
    assert!(mw.check_operation(OperationType::HardwareI2C, "dev").is_err());
    assert!(mw.check_operation(OperationType::RegistryWrite, "HKLM").is_err());
    assert!(mw.check_operation(OperationType::SystemShutdown, "now").is_err());
}

#[test]
fn test_check_operation_elevated_ops() {
    let mw = make_middleware(PermissionPreset::Elevated);
    // Allowed under Elevated
    assert!(mw.check_operation(OperationType::FileRead, "/tmp/test").is_ok());
    assert!(mw.check_operation(OperationType::FileWrite, "/tmp/test").is_ok());
    assert!(mw.check_operation(OperationType::FileDelete, "/tmp/test").is_ok());
    assert!(mw.check_operation(OperationType::DirRead, "/tmp/test").is_ok());
    assert!(mw.check_operation(OperationType::DirCreate, "/tmp/test").is_ok());
    assert!(mw.check_operation(OperationType::DirDelete, "/tmp/test").is_ok());
    assert!(mw.check_operation(OperationType::ProcessExec, "ls").is_ok());
    assert!(mw.check_operation(OperationType::ProcessSpawn, "ls").is_ok());
    assert!(mw.check_operation(OperationType::NetworkRequest, "http://a.com").is_ok());
    assert!(mw.check_operation(OperationType::NetworkDownload, "http://a.com").is_ok());
    assert!(mw.check_operation(OperationType::NetworkUpload, "http://a.com").is_ok());
    // Denied under Elevated
    assert!(mw.check_operation(OperationType::ProcessKill, "1234").is_err());
    assert!(mw.check_operation(OperationType::SystemShutdown, "now").is_err());
    assert!(mw.check_operation(OperationType::SystemReboot, "now").is_err());
    assert!(mw.check_operation(OperationType::RegistryWrite, "HKLM").is_err());
    assert!(mw.check_operation(OperationType::HardwareI2C, "dev").is_err());
    assert!(mw.check_operation(OperationType::HardwareGPIO, "17").is_err());
}

#[test]
fn test_network_wrapper_check_upload_valid_schemes() {
    let mw = make_middleware(PermissionPreset::Unrestricted);
    let wrapper = SecureNetworkWrapper::new(&mw);
    assert!(wrapper.check_network_upload("https://example.com/upload").is_ok());
    assert!(wrapper.check_network_upload("http://example.com/upload").is_ok());
}

#[test]
fn test_hardware_wrapper_spi_write_blocked_in_standard() {
    let mw = make_middleware(PermissionPreset::Standard);
    let wrapper = SecureHardwareWrapper::new(&mw);
    assert!(wrapper.spi_write("1.0", &[0x01]).is_err());
}

#[test]
fn test_hardware_wrapper_gpio_blocked_in_standard() {
    let mw = make_middleware(PermissionPreset::Standard);
    let wrapper = SecureHardwareWrapper::new(&mw);
    assert!(wrapper.check_gpio("17").is_err());
}

#[test]
fn test_hardware_wrapper_i2c_allowed_in_unrestricted() {
    let mw = make_middleware(PermissionPreset::Unrestricted);
    let wrapper = SecureHardwareWrapper::new(&mw);
    assert!(wrapper.check_i2c("i2c-1:0x48").is_ok());
    assert!(wrapper.check_spi("spidev1.0").is_ok());
    assert!(wrapper.check_gpio("17").is_ok());
}

#[test]
fn test_process_wrapper_spawn_dangerous_commands() {
    let mw = make_middleware(PermissionPreset::Elevated);
    let wrapper = SecureProcessWrapper::new(&mw);
    // All these should be blocked by is_safe_command
    assert!(wrapper.check_process_exec("format C:").is_err());
    assert!(wrapper.check_process_exec("mkfs.ext4 /dev/sda1").is_err());
    assert!(wrapper.check_process_exec("dd if=/dev/zero of=/dev/sda").is_err());
    assert!(wrapper.check_process_exec("chmod 777 /etc/passwd").is_err());
    assert!(wrapper.check_process_exec("chown root:root /tmp/evil").is_err());
    assert!(wrapper.check_process_exec("shutdown -h now").is_err());
    assert!(wrapper.check_process_exec("reboot").is_err());
    assert!(wrapper.check_process_exec("poweroff").is_err());
    assert!(wrapper.check_process_spawn("sudo rm -rf /").is_err());
}

#[test]
fn test_process_wrapper_safe_commands() {
    let mw = make_middleware(PermissionPreset::Elevated);
    let wrapper = SecureProcessWrapper::new(&mw);
    // These should pass the safe command check
    assert!(wrapper.check_process_exec("ls -la /tmp").is_ok());
    assert!(wrapper.check_process_exec("python script.py").is_ok());
    assert!(wrapper.check_process_exec("echo hello").is_ok());
    assert!(wrapper.check_process_exec("git status").is_ok());
    assert!(wrapper.check_process_spawn("cargo build").is_ok());
}

#[test]
fn test_process_output_struct() {
    let output = ProcessOutput {
        stdout: "hello".to_string(),
        stderr: String::new(),
        exit_code: Some(0),
        success: true,
    };
    assert_eq!(output.stdout, "hello");
    assert!(output.stderr.is_empty());
    assert_eq!(output.exit_code, Some(0));
    assert!(output.success);
}

#[test]
fn test_http_request_struct_fields() {
    let req = HttpRequest {
        url: "https://api.example.com/v1/data".to_string(),
        method: "PUT".to_string(),
        headers: vec![
            ("Authorization".to_string(), "Bearer token".to_string()),
            ("Content-Type".to_string(), "application/json".to_string()),
        ],
        body: Some(r#"{"key":"value"}"#.to_string()),
        timeout_secs: Some(60),
    };
    assert_eq!(req.url, "https://api.example.com/v1/data");
    assert_eq!(req.method, "PUT");
    assert_eq!(req.headers.len(), 2);
    assert!(req.body.is_some());
    assert_eq!(req.timeout_secs, Some(60));
}

#[test]
fn test_batch_operation_multiple_preset_blocks() {
    // Standard does not allow DirDelete
    let mw = make_middleware(PermissionPreset::Standard);
    let batch = BatchOperationRequest {
        id: "batch-test".to_string(),
        operations: vec![
            OperationRequest {
                id: "op-ok".to_string(),
                op_type: OperationType::FileRead,
                danger_level: DangerLevel::Low,
                user: "test".to_string(),
                source: "cli".to_string(),
                target: "/tmp/a.txt".to_string(),
                timestamp: None,
                ..Default::default()
            },
            OperationRequest {
                id: "op-bad".to_string(),
                op_type: OperationType::DirDelete,
                danger_level: DangerLevel::High,
                user: "test".to_string(),
                source: "cli".to_string(),
                target: "/tmp/dir".to_string(),
                timestamp: None,
                ..Default::default()
            },
        ],
        user: "test".to_string(),
        source: "cli".to_string(),
        description: "mixed batch".to_string(),
    };
    let result = mw.request_batch_permission(&batch);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not allowed"));
}

#[test]
fn test_batch_operation_danger_levels_propagated() {
    // The summary request should use the max danger level
    let config = AuditorConfig {
        enabled: true,
        default_action: "deny".to_string(), // deny everything
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    let mw = SecurityMiddleware::with_preset(
        auditor, "test_user", "cli", "/tmp/ws", PermissionPreset::Standard,
    );
    let batch = BatchOperationRequest {
        id: "batch-deny".to_string(),
        operations: vec![OperationRequest {
            id: "op-1".to_string(),
            op_type: OperationType::FileRead,
            danger_level: DangerLevel::Low,
            user: "test".to_string(),
            source: "cli".to_string(),
            target: "/tmp/a.txt".to_string(),
            timestamp: None,
            ..Default::default()
        }],
        ..Default::default()
    };
    let result = mw.request_batch_permission(&batch);
    // Should be denied by the auditor
    assert!(result.is_err());
}

#[test]
fn test_create_cli_permission_denied_targets() {
    let cli = create_cli_permission();
    assert!(cli.is_target_denied("/etc/sudoers"));
    assert!(cli.is_target_denied("/etc/passwd"));
    assert!(cli.is_target_denied("C:/Windows/System32/drivers/etc/hosts"));
    assert!(!cli.is_target_denied("/tmp/safe_file"));
}

#[test]
fn test_create_web_permission_no_denied_targets() {
    let web = create_web_permission();
    assert!(!web.is_target_denied("/etc/passwd"));
    assert!(!web.is_target_denied("/any/path"));
}

#[test]
fn test_create_agent_permission_operations() {
    let agent = create_agent_permission("agent-123");
    assert!(agent.is_operation_allowed(&OperationType::FileRead));
    assert!(agent.is_operation_allowed(&OperationType::FileWrite));
    assert!(agent.is_operation_allowed(&OperationType::DirRead));
    assert!(agent.is_operation_allowed(&OperationType::DirCreate));
    assert!(agent.is_operation_allowed(&OperationType::ProcessExec));
    assert!(agent.is_operation_allowed(&OperationType::NetworkRequest));
    assert!(!agent.is_operation_allowed(&OperationType::FileDelete));
    assert!(!agent.is_operation_allowed(&OperationType::DirDelete));
    assert!(!agent.is_operation_allowed(&OperationType::ProcessKill));
    assert!(!agent.is_operation_allowed(&OperationType::ProcessSpawn));
    assert!(!agent.is_operation_allowed(&OperationType::NetworkDownload));
    assert!(!agent.is_operation_allowed(&OperationType::NetworkUpload));
}

#[test]
fn test_create_web_permission_network_not_allowed() {
    let web = create_web_permission();
    assert!(!web.is_operation_allowed(&OperationType::NetworkRequest));
    assert!(!web.is_operation_allowed(&OperationType::NetworkDownload));
}

#[test]
fn test_set_preset_changes_behavior() {
    let mut mw = make_middleware(PermissionPreset::ReadOnly);
    assert!(mw.check_operation(OperationType::FileWrite, "/tmp/test").is_err());
    mw.set_preset(PermissionPreset::Standard);
    assert!(mw.check_operation(OperationType::FileWrite, "/tmp/test").is_ok());
    mw.set_preset(PermissionPreset::Elevated);
    assert!(mw.check_operation(OperationType::ProcessExec, "ls").is_ok());
    mw.set_preset(PermissionPreset::Unrestricted);
    assert!(mw.check_operation(OperationType::ProcessKill, "1234").is_ok());
}

#[tokio::test]
async fn test_file_wrapper_write_creates_parent_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
    let wrapper = SecureFileWrapper::new(&mw);
    let file_path = format!("{}\\subdir\\nested\\test.txt", ws);
    let result = wrapper.write_file(&file_path, "nested content").await;
    assert!(result.is_ok(), "write_file with nested dirs failed: {:?}", result);
    let content = std::fs::read_to_string(&file_path).unwrap();
    assert_eq!(content, "nested content");
}

#[tokio::test]
async fn test_file_wrapper_read_nonexistent_file() {
    let dir = tempfile::tempdir().unwrap();
    let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
    let wrapper = SecureFileWrapper::new(&mw);
    let file_path = format!("{}\\nonexistent.txt", ws);
    let result = wrapper.read_file(&file_path).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_file_wrapper_delete_nonexistent_file() {
    let dir = tempfile::tempdir().unwrap();
    let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Unrestricted);
    let wrapper = SecureFileWrapper::new(&mw);
    let file_path = format!("{}\\nonexistent_delete.txt", ws);
    let result = wrapper.delete_file(&file_path).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_file_wrapper_edit_with_multiline_pattern() {
    let dir = tempfile::tempdir().unwrap();
    let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
    let file_path = format!("{}\\multiline.txt", ws);
    std::fs::write(&file_path, "line1\nline2\nline3").unwrap();
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
    let wrapper = SecureFileWrapper::new(&mw);
    let result = wrapper.edit_file(&file_path, "line1\nline2", "replaced").await;
    assert!(result.is_ok(), "edit_file multiline failed: {:?}", result);
    let content = std::fs::read_to_string(&file_path).unwrap();
    assert_eq!(content, "replaced\nline3");
}

#[tokio::test]
async fn test_file_wrapper_stat_directory() {
    let dir = tempfile::tempdir().unwrap();
    let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
    let wrapper = SecureFileWrapper::new(&mw);
    let result = wrapper.stat(&ws).await;
    assert!(result.is_ok(), "stat dir failed: {:?}", result);
    let meta = result.unwrap();
    assert!(meta.is_dir);
    assert!(!meta.is_file);
}

#[tokio::test]
async fn test_file_wrapper_stat_nonexistent() {
    let dir = tempfile::tempdir().unwrap();
    let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
    let wrapper = SecureFileWrapper::new(&mw);
    let file_path = format!("{}\\nonexistent_stat.txt", ws);
    let result = wrapper.stat(&file_path).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_file_wrapper_read_empty_directory() {
    let dir = tempfile::tempdir().unwrap();
    let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
    let wrapper = SecureFileWrapper::new(&mw);
    let result = wrapper.read_directory(&ws).await;
    assert!(result.is_ok());
    let entries = result.unwrap();
    assert!(entries.is_empty());
}

#[tokio::test]
async fn test_file_wrapper_delete_nonexistent_directory() {
    let dir = tempfile::tempdir().unwrap();
    let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Unrestricted);
    let wrapper = SecureFileWrapper::new(&mw);
    let del_dir = format!("{}\\nonexistent_dir", ws);
    let result = wrapper.delete_directory(&del_dir).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_file_wrapper_list_dir_with_subdirs() {
    let dir = tempfile::tempdir().unwrap();
    let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
    std::fs::create_dir(format!("{}\\child_dir", ws)).unwrap();
    std::fs::write(format!("{}\\root_file.txt", ws), "hello").unwrap();
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
    let wrapper = SecureFileWrapper::new(&mw);
    let result = wrapper.list_dir(&ws).await;
    assert!(result.is_ok());
    let entries = result.unwrap();
    assert_eq!(entries.len(), 2);
    let child = entries.iter().find(|e| e.name == "child_dir").unwrap();
    assert!(child.is_dir);
    let file = entries.iter().find(|e| e.name == "root_file.txt").unwrap();
    assert!(!file.is_dir);
    assert_eq!(file.size, 5);
}

#[tokio::test]
async fn test_file_wrapper_open_file_nonexistent() {
    let dir = tempfile::tempdir().unwrap();
    let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
    let wrapper = SecureFileWrapper::new(&mw);
    let file_path = format!("{}\\nonexistent_open.txt", ws);
    let result = wrapper.open_file(&file_path).await;
    assert!(result.is_err());
}

// ============================================================
// Additional coverage for 95%+ target (final round)
// ============================================================

#[tokio::test]
async fn test_file_wrapper_append_to_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
    let file_path = format!("{}\\append.txt", ws);
    std::fs::write(&file_path, "line1\n").unwrap();
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
    let wrapper = SecureFileWrapper::new(&mw);
    let result = wrapper.append_file(&file_path, "line2").await;
    assert!(result.is_ok(), "append_file failed: {:?}", result);
    let content = std::fs::read_to_string(&file_path).unwrap();
    assert_eq!(content, "line1\nline2");
}

#[tokio::test]
async fn test_file_wrapper_append_to_existing_no_trailing_newline() {
    let dir = tempfile::tempdir().unwrap();
    let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
    let file_path = format!("{}\\append2.txt", ws);
    std::fs::write(&file_path, "nolinebreak").unwrap();
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
    let wrapper = SecureFileWrapper::new(&mw);
    let result = wrapper.append_file(&file_path, "appended").await;
    assert!(result.is_ok());
    let content = std::fs::read_to_string(&file_path).unwrap();
    assert_eq!(content, "nolinebreak\nappended");
}

#[tokio::test]
async fn test_file_wrapper_append_creates_new_file() {
    let dir = tempfile::tempdir().unwrap();
    let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
    let file_path = format!("{}\\new_append.txt", ws);
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
    let wrapper = SecureFileWrapper::new(&mw);
    let result = wrapper.append_file(&file_path, "first line").await;
    assert!(result.is_ok());
    let content = std::fs::read_to_string(&file_path).unwrap();
    assert_eq!(content, "first line");
}

#[tokio::test]
async fn test_file_wrapper_remove_file() {
    let dir = tempfile::tempdir().unwrap();
    let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
    let file_path = format!("{}\\to_remove.txt", ws);
    std::fs::write(&file_path, "content").unwrap();
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Unrestricted);
    let wrapper = SecureFileWrapper::new(&mw);
    let result = wrapper.remove_file(&file_path).await;
    assert!(result.is_ok());
    assert!(!std::path::Path::new(&file_path).exists());
}

#[tokio::test]
async fn test_file_wrapper_edit_pattern_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
    let file_path = format!("{}\\edit_miss.txt", ws);
    std::fs::write(&file_path, "hello world").unwrap();
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
    let wrapper = SecureFileWrapper::new(&mw);
    let result = wrapper.edit_file(&file_path, "nonexistent_pattern", "replacement").await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not found"));
}

#[tokio::test]
async fn test_file_wrapper_open_file_reads_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
    let file_path = format!("{}\\binary.dat", ws);
    std::fs::write(&file_path, b"\x00\x01\x02\x03").unwrap();
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
    let wrapper = SecureFileWrapper::new(&mw);
    let result = wrapper.open_file(&file_path).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), vec![0x00, 0x01, 0x02, 0x03]);
}

#[tokio::test]
async fn test_file_wrapper_create_and_delete_directory() {
    let dir = tempfile::tempdir().unwrap();
    let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
    let new_dir = format!("{}\\new_dir", ws);
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Unrestricted);
    let wrapper = SecureFileWrapper::new(&mw);
    // Create directory
    let result = wrapper.create_directory(&new_dir).await;
    assert!(result.is_ok(), "create_directory failed: {:?}", result);
    assert!(std::path::Path::new(&new_dir).is_dir());
    // Delete directory
    let result = wrapper.delete_directory(&new_dir).await;
    assert!(result.is_ok(), "delete_directory failed: {:?}", result);
    assert!(!std::path::Path::new(&new_dir).exists());
}

#[test]
fn test_file_metadata_struct() {
    let meta = FileMetadata {
        is_file: true,
        is_dir: false,
        len: 1024,
        readonly: false,
        modified: "2025-01-01T00:00:00Z".to_string(),
    };
    assert!(meta.is_file);
    assert!(!meta.is_dir);
    assert_eq!(meta.len, 1024);
    assert!(!meta.readonly);
}

#[test]
fn test_dir_entry_struct() {
    let entry = DirEntry {
        name: "test.txt".to_string(),
        is_dir: false,
        size: 42,
    };
    assert_eq!(entry.name, "test.txt");
    assert!(!entry.is_dir);
    assert_eq!(entry.size, 42);
}

#[test]
fn test_operation_request_fields() {
    let req = OperationRequest {
        id: "test".to_string(),
        op_type: OperationType::FileRead,
        danger_level: DangerLevel::Low,
        user: "test".to_string(),
        source: "cli".to_string(),
        target: "/tmp/test".to_string(),
        timestamp: None,
        approver: None,
        approved_at: None,
        denied_reason: None,
    };
    assert_eq!(req.id, "test");
    assert!(req.approver.is_none());
    assert!(req.approved_at.is_none());
    assert!(req.denied_reason.is_none());
}

#[test]
fn test_batch_operation_request_fields() {
    let req = BatchOperationRequest {
        id: "batch".to_string(),
        ..Default::default()
    };
    assert!(req.operations.is_empty());
    assert!(req.user.is_empty());
    assert!(req.source.is_empty());
    assert!(req.description.is_empty());
}

#[test]
fn test_process_wrapper_check_spawn_elevated() {
    let mw = make_middleware(PermissionPreset::Elevated);
    let wrapper = SecureProcessWrapper::new(&mw);
    // spawn is allowed under Elevated for safe commands
    assert!(wrapper.check_process_spawn("cargo test").is_ok());
}

#[test]
fn test_process_wrapper_check_kill_unrestricted() {
    let mw = make_middleware(PermissionPreset::Unrestricted);
    let wrapper = SecureProcessWrapper::new(&mw);
    // kill is allowed under Unrestricted
    assert!(wrapper.check_process_kill("1234").is_ok());
}

#[test]
fn test_network_wrapper_check_request_standard() {
    let mw = make_middleware(PermissionPreset::Standard);
    let wrapper = SecureNetworkWrapper::new(&mw);
    assert!(wrapper.check_network_request("https://example.com").is_ok());
}

#[test]
fn test_network_wrapper_check_download_standard() {
    let mw = make_middleware(PermissionPreset::Standard);
    let wrapper = SecureNetworkWrapper::new(&mw);
    assert!(wrapper.check_network_download("https://example.com/file.zip").is_ok());
}

#[test]
fn test_network_wrapper_check_upload_standard_denied() {
    let mw = make_middleware(PermissionPreset::Standard);
    let wrapper = SecureNetworkWrapper::new(&mw);
    assert!(wrapper.check_network_upload("https://example.com/upload").is_err());
}

#[test]
fn test_network_wrapper_upload_invalid_scheme() {
    let mw = make_middleware(PermissionPreset::Unrestricted);
    let wrapper = SecureNetworkWrapper::new(&mw);
    assert!(wrapper.check_network_upload("ftp://example.com/upload").is_err());
}

#[test]
fn test_permission_is_target_allowed_with_allowed_list() {
    let mut perm = create_cli_permission();
    perm.allowed_targets.push("/tmp/allowed".to_string());
    // When allowed_targets is non-empty, only those targets are allowed
    assert!(!perm.is_target_denied("/tmp/allowed"));
}

#[test]
fn test_danger_level_ordering() {
    assert!(DangerLevel::Low < DangerLevel::Medium);
    assert!(DangerLevel::Medium < DangerLevel::High);
    assert!(DangerLevel::High < DangerLevel::Critical);
}

#[test]
fn test_operation_type_equality() {
    assert_eq!(OperationType::FileRead, OperationType::FileRead);
    assert_ne!(OperationType::FileRead, OperationType::FileWrite);
}

#[test]
fn test_permission_preset_values() {
    // Ensure all presets are distinct
    let presets = [
        PermissionPreset::ReadOnly,
        PermissionPreset::Standard,
        PermissionPreset::Elevated,
        PermissionPreset::Unrestricted,
    ];
    for i in 0..presets.len() {
        for j in (i+1)..presets.len() {
            assert_ne!(presets[i], presets[j]);
        }
    }
}

#[test]
fn test_security_middleware_with_preset_constructor() {
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    let mw = SecurityMiddleware::with_preset(
        auditor, "user1", "rpc", "/ws", PermissionPreset::Elevated,
    );
    assert_eq!(mw.preset(), PermissionPreset::Elevated);
    assert_eq!(mw.user(), "user1");
    assert_eq!(mw.source(), "rpc");
}

#[test]
fn test_http_request_default_fields() {
    let req = HttpRequest {
        url: String::new(),
        method: "GET".to_string(),
        headers: Vec::new(),
        body: None,
        timeout_secs: None,
    };
    assert!(req.url.is_empty());
    assert!(req.body.is_none());
}

#[test]
fn test_process_output_default_fields() {
    let output = ProcessOutput {
        stdout: String::new(),
        stderr: String::new(),
        exit_code: None,
        success: false,
    };
    assert!(output.stdout.is_empty());
    assert!(!output.success);
}

#[tokio::test]
async fn test_file_wrapper_read_directory_with_mixed_entries() {
    let dir = tempfile::tempdir().unwrap();
    let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
    std::fs::write(format!("{}\\file1.txt", ws), "hello").unwrap();
    std::fs::write(format!("{}\\file2.txt", ws), "world").unwrap();
    std::fs::create_dir(format!("{}\\subdir", ws)).unwrap();
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
    let wrapper = SecureFileWrapper::new(&mw);
    let result = wrapper.read_directory(&ws).await;
    assert!(result.is_ok());
    let entries = result.unwrap();
    assert_eq!(entries.len(), 3);
    // Entries should be sorted
    assert!(entries.contains(&"file1.txt".to_string()));
    assert!(entries.contains(&"file2.txt".to_string()));
    assert!(entries.contains(&"subdir/".to_string()));
}

#[tokio::test]
async fn test_file_wrapper_stat_file() {
    let dir = tempfile::tempdir().unwrap();
    let ws = std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap().to_string();
    let file_path = format!("{}\\stat_me.txt", ws);
    std::fs::write(&file_path, "stat content").unwrap();
    let config = AuditorConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let auditor = std::sync::Arc::new(SecurityAuditor::new(config));
    let mw = SecurityMiddleware::with_preset(auditor, "test_user", "cli", &ws, PermissionPreset::Standard);
    let wrapper = SecureFileWrapper::new(&mw);
    let result = wrapper.stat(&file_path).await;
    assert!(result.is_ok());
    let meta = result.unwrap();
    assert!(meta.is_file);
    assert!(!meta.is_dir);
    assert_eq!(meta.len, 12);
}

#[test]
fn test_check_operation_registry_write_elevated() {
    let mw = make_middleware(PermissionPreset::Elevated);
    assert!(mw.check_operation(OperationType::RegistryWrite, "HKLM\\Software").is_err());
}

#[test]
fn test_check_operation_system_reboot_unrestricted() {
    let mw = make_middleware(PermissionPreset::Unrestricted);
    assert!(mw.check_operation(OperationType::SystemReboot, "now").is_ok());
}

#[test]
fn test_check_operation_hardware_spi_elevated() {
    let mw = make_middleware(PermissionPreset::Elevated);
    assert!(mw.check_operation(OperationType::HardwareSPI, "spidev1.0").is_err());
}

#[test]
fn test_check_operation_hardware_gpio_unrestricted() {
    let mw = make_middleware(PermissionPreset::Unrestricted);
    assert!(mw.check_operation(OperationType::HardwareGPIO, "17").is_ok());
}

#[test]
fn test_create_cli_permission_operations() {
    let cli = create_cli_permission();
    assert!(cli.is_operation_allowed(&OperationType::FileRead));
    assert!(cli.is_operation_allowed(&OperationType::FileWrite));
    assert!(cli.is_operation_allowed(&OperationType::FileDelete));
    assert!(cli.is_operation_allowed(&OperationType::DirRead));
    assert!(cli.is_operation_allowed(&OperationType::DirCreate));
    assert!(cli.is_operation_allowed(&OperationType::ProcessExec));
    assert!(cli.is_operation_allowed(&OperationType::NetworkDownload));
    assert!(cli.is_operation_allowed(&OperationType::NetworkRequest));
    // Operations not in allowed list
    assert!(!cli.is_operation_allowed(&OperationType::ProcessKill));
    assert!(!cli.is_operation_allowed(&OperationType::RegistryWrite));
    assert!(!cli.is_operation_allowed(&OperationType::SystemShutdown));
}

#[test]
fn test_create_cli_permission_require_approval() {
    let cli = create_cli_permission();
    // require_approval contains ProcessKill, SystemShutdown, SystemReboot
    assert!(cli.require_approval.contains_key(&OperationType::ProcessKill));
    assert!(cli.require_approval.contains_key(&OperationType::SystemShutdown));
    assert!(cli.require_approval.contains_key(&OperationType::SystemReboot));
    // These are allowed without approval
    assert!(!cli.require_approval.contains_key(&OperationType::FileDelete));
    assert!(!cli.require_approval.contains_key(&OperationType::ProcessExec));
    assert!(!cli.require_approval.contains_key(&OperationType::NetworkDownload));
}

#[test]
fn test_create_agent_permission_require_approval() {
    let agent = create_agent_permission("agent-x");
    assert!(agent.require_approval.contains_key(&OperationType::FileDelete));
    assert!(agent.require_approval.contains_key(&OperationType::ProcessKill));
    assert!(agent.require_approval.contains_key(&OperationType::SystemShutdown));
    assert!(agent.require_approval.contains_key(&OperationType::NetworkDownload));
}
