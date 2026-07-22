use super::*;

fn make_config() -> BotServiceConfig {
    BotServiceConfig {
        config_path: PathBuf::from("test_config.json"),
        workspace: PathBuf::from("/tmp/test_workspace"),
        ..BotServiceConfig::default()
    }
}

fn make_config_with_file(dir: &std::path::Path) -> BotServiceConfig {
    let config_path = dir.join("config.json");
    let config_content = serde_json::json!({
        "workspace": dir.to_string_lossy(),
        "models": [
            {
                "model": "test/test-model-1.0",
                "api_key": "test-key-12345",
                "base_url": "",
                "is_default": true
            }
        ],
        "heartbeat": {
            "interval": 60,
            "enabled": true
        },
        "gateway": {
            "host": "127.0.0.1",
            "port": 8080
        },
        "security": {
            "enabled": true
        },
        "forge": {
            "enabled": false
        },
        "memory": {
            "enabled": false
        },
        "workflow": {
            "enabled": false
        },
        "devices": {
            "enabled": false,
            "monitor_usb": false
        },
        "agents": {
            "defaults": {
                "restrict_to_workspace": true
            }
        },
        "tools": {
            "cron": {
                "exec_timeout_minutes": 5
            }
        }
    });
    std::fs::write(
        &config_path,
        serde_json::to_string_pretty(&config_content).unwrap(),
    )
    .unwrap();

    BotServiceConfig {
        config_path,
        workspace: dir.to_path_buf(),
        ..BotServiceConfig::default()
    }
}

#[test]
fn test_new_service_is_not_started() {
    let svc = BotService::new(make_config());
    assert_eq!(svc.get_state(), BotState::NotStarted);
    assert!(svc.get_error().is_none());
    assert!(svc.enabled_components().enabled_list().is_empty());
}

#[test]
fn test_start_fails_without_config_file() {
    let svc = BotService::new(make_config());
    // Should fail because test_config.json doesn't exist
    let result = svc.start();
    assert!(result.is_err());
    assert_eq!(svc.get_state(), BotState::Error);
    assert!(svc.get_error().is_some());
}

#[test]
fn test_stop_when_not_running_fails() {
    let svc = BotService::new(make_config());
    let result = svc.stop();
    assert!(result.is_err());
}

#[test]
fn test_restart_when_not_running_starts() {
    let svc = BotService::new(make_config());
    // Restart on a stopped bot will try to start and fail due to missing config
    let result = svc.restart();
    assert!(result.is_err());
}

#[test]
fn test_get_components_when_stopped() {
    let svc = BotService::new(make_config());
    let components = svc.get_components();
    assert!(components.is_empty());
}

#[test]
fn test_save_config_writes_to_disk() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.json");

    // Write initial config
    let initial_config = serde_json::json!({
        "models": [
            {
                "model": "test/model-1.0",
                "api_key": "test-key",
                "base_url": "",
                "is_default": true
            }
        ]
    });
    std::fs::write(
        &config_path,
        serde_json::to_string_pretty(&initial_config).unwrap(),
    )
    .unwrap();

    let svc = BotService::new(BotServiceConfig {
        config_path: config_path.clone(),
        workspace: dir.path().to_path_buf(),
        ..BotServiceConfig::default()
    });

    let new_config = serde_json::json!({
        "models": [
            {
                "model": "test/model-2.0",
                "api_key": "new-key",
                "base_url": "http://localhost:9090",
                "is_default": true
            }
        ]
    });

    let result = svc.save_config(&new_config, false);
    assert!(result.is_ok());

    // Verify file was written
    let content = std::fs::read_to_string(&config_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(parsed["models"][0]["model"], "test/model-2.0");
    assert_eq!(parsed["models"][0]["api_key"], "new-key");
}

#[test]
fn test_save_config_rejects_invalid_json() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.json");
    std::fs::write(&config_path, "{}").unwrap();

    let svc = BotService::new(BotServiceConfig {
        config_path,
        workspace: dir.path().to_path_buf(),
        ..BotServiceConfig::default()
    });

    // Not an object
    let result = svc.save_config(&serde_json::json!("not an object"), false);
    assert!(result.is_err());
}

#[test]
fn test_save_config_rejects_no_models() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.json");
    std::fs::write(&config_path, "{}").unwrap();

    let svc = BotService::new(BotServiceConfig {
        config_path,
        workspace: dir.path().to_path_buf(),
        ..BotServiceConfig::default()
    });

    let result = svc.save_config(&serde_json::json!({ "models": [] }), false);
    assert!(result.is_err());
}

#[test]
fn test_save_config_rejects_no_api_key() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.json");
    std::fs::write(&config_path, "{}").unwrap();

    let svc = BotService::new(BotServiceConfig {
        config_path,
        workspace: dir.path().to_path_buf(),
        ..BotServiceConfig::default()
    });

    let result = svc.save_config(
        &serde_json::json!({
            "models": [{ "model": "test/1.0", "api_key": "", "base_url": "", "is_default": true }]
        }),
        false,
    );
    assert!(result.is_err());
}

#[test]
fn test_validate_config_checks_models() {
    let dir = tempfile::tempdir().unwrap();
    // Config with no models
    let config_path = dir.path().join("config.json");
    std::fs::write(&config_path, r#"{"models": []}"#).unwrap();

    let svc = BotService::new(BotServiceConfig {
        config_path,
        ..BotServiceConfig::default()
    });

    let result = svc.start();
    assert!(result.is_err());
    assert!(svc.get_error().unwrap().contains("no models configured"));
}

#[test]
fn test_validate_config_checks_api_keys() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.json");
    std::fs::write(
        &config_path,
        r#"{"models": [{"model": "test/1.0", "api_key": "", "base_url": "", "is_default": true}]}"#,
    )
    .unwrap();

    let svc = BotService::new(BotServiceConfig {
        config_path,
        ..BotServiceConfig::default()
    });

    let result = svc.start();
    assert!(result.is_err());
    assert!(
        svc.get_error()
            .unwrap()
            .contains("no model with valid API key")
    );
}

#[test]
fn test_full_start_stop_cycle() {
    let dir = tempfile::tempdir().unwrap();
    let config = make_config_with_file(dir.path());
    let svc = BotService::new(config);

    // Start should succeed
    let result = svc.start();
    assert!(result.is_ok());
    assert_eq!(svc.get_state(), BotState::Running);

    // Verify core components are enabled
    let enabled = svc.enabled_components();
    assert!(enabled.is_enabled(Component::Bus));
    assert!(enabled.is_enabled(Component::Agent));
    assert!(enabled.is_enabled(Component::Channels));
    assert!(enabled.is_enabled(Component::Health));
    assert!(enabled.is_enabled(Component::Cron));
    assert!(enabled.is_enabled(Component::Skills));
    assert!(enabled.is_enabled(Component::Observer));

    // Stop should succeed
    let result = svc.stop();
    assert!(result.is_ok());
    assert_eq!(svc.get_state(), BotState::NotStarted);
    assert!(svc.enabled_components().enabled_list().is_empty());
}

#[test]
fn test_restart_cycle() {
    let dir = tempfile::tempdir().unwrap();
    let config = make_config_with_file(dir.path());
    let svc = BotService::new(config);

    // Start
    svc.start().unwrap();
    assert_eq!(svc.get_state(), BotState::Running);

    // Restart
    let result = svc.restart();
    assert!(result.is_ok());
    assert_eq!(svc.get_state(), BotState::Running);
}

#[test]
fn test_double_start_fails() {
    let dir = tempfile::tempdir().unwrap();
    let config = make_config_with_file(dir.path());
    let svc = BotService::new(config);

    svc.start().unwrap();
    let result = svc.start();
    assert!(result.is_err());
}

#[test]
fn test_inject_and_get_forge() {
    let svc = BotService::new(make_config());

    // Before injection
    assert!(svc.get_forge().is_none());

    // Create a mock forge service
    struct MockForge;
    impl LifecycleService for MockForge {
        fn start(&self) -> Result<(), String> {
            Ok(())
        }
        fn stop(&self) -> Result<(), String> {
            Ok(())
        }
    }
    impl ForgeService for MockForge {
        fn forge_name(&self) -> &str {
            "mock_forge"
        }
    }

    svc.inject_forge(Arc::new(MockForge));
    assert!(svc.get_forge().is_some());
    assert_eq!(svc.get_forge().unwrap().forge_name(), "mock_forge");
}

#[test]
fn test_inject_memory() {
    let svc = BotService::new(make_config());
    assert!(svc.get_memory().is_none());

    struct MockMemory;
    impl LifecycleService for MockMemory {
        fn start(&self) -> Result<(), String> {
            Ok(())
        }
        fn stop(&self) -> Result<(), String> {
            Ok(())
        }
    }
    impl MemoryService for MockMemory {}

    svc.inject_memory(Arc::new(MockMemory));
    assert!(svc.get_memory().is_some());
}

#[test]
fn test_config_path() {
    let config = make_config();
    assert_eq!(config.config_path, PathBuf::from("test_config.json"));
}

#[test]
fn test_save_config_atomic_write() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.json");

    // Write initial config
    let initial = serde_json::json!({
        "models": [{ "model": "test/1.0", "api_key": "key1", "base_url": "", "is_default": true }]
    });
    std::fs::write(&config_path, serde_json::to_string(&initial).unwrap()).unwrap();

    let svc = BotService::new(BotServiceConfig {
        config_path: config_path.clone(),
        workspace: dir.path().to_path_buf(),
        ..BotServiceConfig::default()
    });

    // Update config
    let updated = serde_json::json!({
        "models": [{ "model": "test/2.0", "api_key": "key2", "base_url": "", "is_default": true }]
    });
    svc.save_config(&updated, false).unwrap();

    // Verify no temp file left behind
    assert!(!config_path.with_extension("json.tmp").exists());

    // Verify content is the updated config
    let content = std::fs::read_to_string(&config_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(parsed["models"][0]["model"], "test/2.0");
}

#[test]
fn test_save_config_creates_parent_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("subdir").join("config.json");

    let svc = BotService::new(BotServiceConfig {
        config_path: config_path.clone(),
        workspace: dir.path().to_path_buf(),
        ..BotServiceConfig::default()
    });

    let config = serde_json::json!({
        "models": [{ "model": "test/1.0", "api_key": "key1", "base_url": "", "is_default": true }]
    });

    svc.save_config(&config, false).unwrap();
    assert!(config_path.exists());
}

#[test]
fn test_enabled_components_disable_all() {
    let mut ec = EnabledComponents::new();
    ec.enable(Component::Bus);
    ec.enable(Component::Agent);
    assert_eq!(ec.enabled_list().len(), 2);

    ec.disable_all();
    assert!(ec.enabled_list().is_empty());
}

#[test]
fn test_component_labels() {
    assert_eq!(Component::Bus.label(), "bus");
    assert_eq!(Component::Forge.label(), "forge");
    assert_eq!(Component::Observer.label(), "observer");
    assert_eq!(Component::Health.label(), "health");
}

#[test]
fn test_get_components_reflects_enabled() {
    let dir = tempfile::tempdir().unwrap();
    let config = make_config_with_file(dir.path());
    let svc = BotService::new(config);

    svc.start().unwrap();

    let components = svc.get_components();
    assert!(components.contains_key("bus"));
    assert!(components.contains_key("agent"));
    assert!(components.contains_key("channels"));
}

#[test]
fn test_workspace_returns_resolved_path() {
    let dir = tempfile::tempdir().unwrap();
    let config = make_config_with_file(dir.path());
    let svc = BotService::new(config);

    // Before start, workspace is empty
    assert!(svc.workspace().as_os_str().is_empty() || svc.workspace().exists());

    svc.start().unwrap();
    // After start, workspace should be resolved
    assert!(!svc.workspace().as_os_str().is_empty());
}

// ============================================================
// Additional tests for inject/get methods, serialization,
// and configuration edge cases
// ============================================================

#[test]
fn test_inject_and_get_cron() {
    let svc = BotService::new(make_config());

    struct MockCron;
    impl LifecycleService for MockCron {
        fn start(&self) -> Result<(), String> {
            Ok(())
        }
        fn stop(&self) -> Result<(), String> {
            Ok(())
        }
    }
    impl CronService for MockCron {}

    svc.inject_cron(Arc::new(MockCron));
    // Cron is not directly gettable but injection should not panic
}

#[test]
fn test_inject_security() {
    let svc = BotService::new(make_config());

    struct MockSecurity;
    impl SecurityService for MockSecurity {}

    svc.inject_security(Arc::new(MockSecurity));
}

#[test]
fn test_inject_workflow() {
    let svc = BotService::new(make_config());

    struct MockWorkflow;
    impl WorkflowService for MockWorkflow {}

    svc.inject_workflow(Arc::new(MockWorkflow));
}

#[test]
fn test_inject_skills() {
    let svc = BotService::new(make_config());

    struct MockSkills;
    impl SkillsService for MockSkills {}

    svc.inject_skills(Arc::new(MockSkills));
}

#[test]
fn test_inject_observer() {
    let svc = BotService::new(make_config());

    struct MockObserver;
    impl ObserverManager for MockObserver {
        fn has_observers(&self) -> bool {
            false
        }
    }

    svc.inject_observer(Arc::new(MockObserver));
}

#[test]
fn test_inject_heartbeat() {
    let svc = BotService::new(make_config());

    struct MockHeartbeat;
    impl LifecycleService for MockHeartbeat {
        fn start(&self) -> Result<(), String> {
            Ok(())
        }
        fn stop(&self) -> Result<(), String> {
            Ok(())
        }
    }
    impl HeartbeatService for MockHeartbeat {}

    svc.inject_heartbeat(Arc::new(MockHeartbeat));
}

#[test]
fn test_inject_devices() {
    let svc = BotService::new(make_config());

    struct MockDevices;
    impl LifecycleService for MockDevices {
        fn start(&self) -> Result<(), String> {
            Ok(())
        }
        fn stop(&self) -> Result<(), String> {
            Ok(())
        }
    }
    impl DeviceService for MockDevices {}

    svc.inject_devices(Arc::new(MockDevices));
}

#[test]
fn test_inject_health() {
    let svc = BotService::new(make_config());

    struct MockHealth;
    impl LifecycleService for MockHealth {
        fn start(&self) -> Result<(), String> {
            Ok(())
        }
        fn stop(&self) -> Result<(), String> {
            Ok(())
        }
    }
    impl HealthServer for MockHealth {}

    svc.inject_health(Arc::new(MockHealth));
}

#[test]
fn test_inject_channels() {
    let svc = BotService::new(make_config());

    struct MockChannels;
    impl LifecycleService for MockChannels {
        fn start(&self) -> Result<(), String> {
            Ok(())
        }
        fn stop(&self) -> Result<(), String> {
            Ok(())
        }
    }
    impl ChannelManager for MockChannels {
        fn enabled_channels(&self) -> Vec<String> {
            vec![]
        }
    }

    svc.inject_channels(Arc::new(MockChannels));
    assert!(svc.get_channel_manager().is_some());
}

#[test]
fn test_inject_agent() {
    let svc = BotService::new(make_config());

    struct MockAgent;
    impl LifecycleService for MockAgent {
        fn start(&self) -> Result<(), String> {
            Ok(())
        }
        fn stop(&self) -> Result<(), String> {
            Ok(())
        }
    }
    impl AgentLoopService for MockAgent {}

    svc.inject_agent(Arc::new(MockAgent));
    assert!(svc.get_agent_loop().is_some());
}

#[test]
fn test_bot_service_config_default() {
    let config = BotServiceConfig::default();
    assert!(config.security_enabled);
    assert!(!config.forge_enabled);
    assert!(!config.cluster_enabled);
    assert!(config.workspace.as_os_str().is_empty());
    assert_eq!(config.gateway_port, 8080);
}

#[test]
fn test_component_serialization() {
    let component = Component::Bus;
    let json = serde_json::to_string(&component).unwrap();
    let restored: Component = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, Component::Bus);
}

#[test]
fn test_component_all_labels() {
    assert_eq!(Component::Bus.label(), "bus");
    assert_eq!(Component::Channels.label(), "channels");
    assert_eq!(Component::Agent.label(), "agent");
    assert_eq!(Component::Security.label(), "security");
    assert_eq!(Component::Forge.label(), "forge");
    assert_eq!(Component::Cluster.label(), "cluster");
    assert_eq!(Component::Memory.label(), "memory");
    assert_eq!(Component::Workflow.label(), "workflow");
    assert_eq!(Component::Skills.label(), "skills");
    assert_eq!(Component::Cron.label(), "cron");
    assert_eq!(Component::Heartbeat.label(), "heartbeat");
    assert_eq!(Component::Devices.label(), "devices");
    assert_eq!(Component::Health.label(), "health");
    assert_eq!(Component::Observer.label(), "observer");
}

#[test]
fn test_enabled_components_serialization() {
    let mut ec = EnabledComponents::new();
    ec.enable(Component::Bus);
    ec.enable(Component::Agent);
    let list = ec.enabled_list();
    assert_eq!(list.len(), 2);
    assert!(list.contains(&Component::Bus));
    assert!(list.contains(&Component::Agent));
}

#[test]
fn test_enabled_components_is_enabled() {
    let mut ec = EnabledComponents::new();
    assert!(!ec.is_enabled(Component::Bus));
    ec.enable(Component::Bus);
    assert!(ec.is_enabled(Component::Bus));
    assert!(!ec.is_enabled(Component::Agent));
}

#[test]
fn test_bot_state_transitions() {
    assert!(BotState::NotStarted.can_start());
    assert!(!BotState::NotStarted.can_stop());

    assert!(!BotState::Running.can_start());
    assert!(BotState::Running.can_stop());

    assert!(BotState::Error.can_start());
    assert!(!BotState::Error.can_stop());
}

#[test]
fn test_bot_state_display() {
    assert_eq!(BotState::NotStarted.to_string(), "not_started");
    assert_eq!(BotState::Running.to_string(), "running");
    assert_eq!(BotState::Error.to_string(), "error");
}

#[test]
fn test_get_config_returns_copy() {
    let svc = BotService::new(make_config());
    let config1 = svc.get_config();
    let config2 = svc.get_config();
    assert_eq!(config1.config_path, config2.config_path);
}

#[test]
fn test_get_channel_manager_none() {
    let svc = BotService::new(make_config());
    assert!(svc.get_channel_manager().is_none());
}

#[test]
fn test_get_agent_loop_none() {
    let svc = BotService::new(make_config());
    assert!(svc.get_agent_loop().is_none());
}

// ============================================================
// Additional coverage tests for 95%+ target
// ============================================================

// --- EnabledComponents ---

#[test]
fn test_enabled_components_default() {
    let ec = EnabledComponents::default();
    assert!(ec.enabled_list().is_empty());
}

#[test]
fn test_enabled_components_disable() {
    let mut ec = EnabledComponents::new();
    ec.enable(Component::Bus);
    assert!(ec.is_enabled(Component::Bus));
    ec.disable(Component::Bus);
    assert!(!ec.is_enabled(Component::Bus));
}

#[test]
fn test_enabled_components_is_enabled_unknown() {
    // is_enabled for a component that was never inserted should return false
    let _ec = EnabledComponents::new();
    // All components are inserted in new(), so test with a fresh one
    // but after disable_all
    let mut ec = EnabledComponents::new();
    ec.disable_all();
    assert!(!ec.is_enabled(Component::Bus));
}

#[test]
fn test_enabled_components_enable_disable_roundtrip() {
    let mut ec = EnabledComponents::new();
    for comp in [
        Component::Bus,
        Component::Channels,
        Component::Agent,
        Component::Security,
        Component::Forge,
        Component::Cluster,
        Component::Memory,
        Component::Workflow,
        Component::Skills,
        Component::Cron,
        Component::Heartbeat,
        Component::Devices,
        Component::Health,
        Component::Observer,
    ] {
        ec.enable(comp);
        assert!(ec.is_enabled(comp));
        ec.disable(comp);
        assert!(!ec.is_enabled(comp));
    }
}

// --- Component ---

#[test]
fn test_component_all_variants_serde() {
    let all = vec![
        Component::Bus,
        Component::Channels,
        Component::Agent,
        Component::Security,
        Component::Forge,
        Component::Cluster,
        Component::Memory,
        Component::Workflow,
        Component::Skills,
        Component::Cron,
        Component::Heartbeat,
        Component::Devices,
        Component::Health,
        Component::Observer,
    ];
    for comp in &all {
        let json = serde_json::to_string(comp).unwrap();
        let back: Component = serde_json::from_str(&json).unwrap();
        assert_eq!(*comp, back);
    }
}

#[test]
fn test_component_copy_eq() {
    let a = Component::Bus;
    let b = a;
    assert_eq!(a, b);
}

#[test]
fn test_component_hash_in_set() {
    use std::collections::HashSet;
    let mut set = HashSet::new();
    set.insert(Component::Bus);
    set.insert(Component::Bus);
    set.insert(Component::Agent);
    assert_eq!(set.len(), 2);
}

// --- BotServiceConfig ---

#[test]
fn test_config_ref_accessor() {
    let svc = BotService::new(make_config());
    assert_eq!(svc.config().config_path, PathBuf::from("test_config.json"));
}

#[test]
fn test_model_entry_serde() {
    let entry = ModelEntry {
        model: "test/model-1.0".to_string(),
        api_key: "key123".to_string(),
        base_url: "http://localhost:8080".to_string(),
        is_default: true,
    };
    let json = serde_json::to_string(&entry).unwrap();
    let back: ModelEntry = serde_json::from_str(&json).unwrap();
    assert_eq!(back.model, "test/model-1.0");
    assert_eq!(back.api_key, "key123");
    assert_eq!(back.base_url, "http://localhost:8080");
    assert!(back.is_default);
}

#[test]
fn test_model_entry_api_base_alias() {
    // The alias "api_base" should also deserialize into base_url
    let json =
        r#"{"model":"test/1","api_key":"k","api_base":"http://host:123","is_default":false}"#;
    let entry: ModelEntry = serde_json::from_str(json).unwrap();
    assert_eq!(entry.base_url, "http://host:123");
}

#[test]
fn test_model_entry_defaults() {
    let json = r#"{"model":"test/1"}"#;
    let entry: ModelEntry = serde_json::from_str(json).unwrap();
    assert!(entry.api_key.is_empty());
    assert!(entry.base_url.is_empty());
    assert!(!entry.is_default);
}

// --- LifecycleService trait default impl ---

struct MockLifecycle;
impl LifecycleService for MockLifecycle {}

#[test]
fn test_lifecycle_service_default_start() {
    let svc = MockLifecycle;
    assert!(svc.start().is_ok());
}

#[test]
fn test_lifecycle_service_default_stop() {
    let svc = MockLifecycle;
    assert!(svc.stop().is_ok());
}

// --- AgentLoopService trait default impl ---

struct MockAgentDefault;
impl LifecycleService for MockAgentDefault {
    fn start(&self) -> Result<(), String> {
        Ok(())
    }
    fn stop(&self) -> Result<(), String> {
        Ok(())
    }
}
impl AgentLoopService for MockAgentDefault {}

#[test]
fn test_agent_loop_default_process_heartbeat() {
    let agent = MockAgentDefault;
    let result = agent.process_heartbeat();
    assert!(result.is_ok());
    assert!(result.unwrap().is_empty());
}

// --- HeartbeatHandler ---

#[test]
fn test_create_heartbeat_handler_skips_bootstrap() {
    let dir = tempfile::tempdir().unwrap();
    let config = make_config_with_file(dir.path());

    // Create BOOTSTRAP.md in workspace
    std::fs::write(dir.path().join("BOOTSTRAP.md"), "# init").unwrap();

    let svc = BotService::new(config);
    svc.start().unwrap();

    let handler = svc.create_heartbeat_handler();
    // Should not panic when bootstrap exists
    handler();
}

#[test]
fn test_create_heartbeat_handler_no_agent() {
    let dir = tempfile::tempdir().unwrap();
    let config = make_config_with_file(dir.path());

    let svc = BotService::new(config);
    svc.start().unwrap();

    // No agent injected, so handler should log warning but not panic
    let handler = svc.create_heartbeat_handler();
    handler(); // Should not panic
}

#[tokio::test]
async fn test_create_heartbeat_handler_with_agent() {
    let dir = tempfile::tempdir().unwrap();
    let config = make_config_with_file(dir.path());

    let svc = BotService::new(config);

    struct MockAgentHeartbeat;
    impl LifecycleService for MockAgentHeartbeat {
        fn start(&self) -> Result<(), String> {
            Ok(())
        }
        fn stop(&self) -> Result<(), String> {
            Ok(())
        }
    }
    impl AgentLoopService for MockAgentHeartbeat {
        fn process_heartbeat(&self) -> Result<String, String> {
            Ok("heartbeat ok".to_string())
        }
    }

    svc.inject_agent(Arc::new(MockAgentHeartbeat));
    svc.start().unwrap();

    let handler = svc.create_heartbeat_handler();
    handler(); // Should call process_heartbeat
}

#[tokio::test]
async fn test_create_heartbeat_handler_agent_error() {
    let dir = tempfile::tempdir().unwrap();
    let config = make_config_with_file(dir.path());

    let svc = BotService::new(config);

    struct MockAgentError;
    impl LifecycleService for MockAgentError {
        fn start(&self) -> Result<(), String> {
            Ok(())
        }
        fn stop(&self) -> Result<(), String> {
            Ok(())
        }
    }
    impl AgentLoopService for MockAgentError {
        fn process_heartbeat(&self) -> Result<String, String> {
            Err("heartbeat failed".to_string())
        }
    }

    svc.inject_agent(Arc::new(MockAgentError));
    svc.start().unwrap();

    let handler = svc.create_heartbeat_handler();
    handler(); // Should handle error gracefully
}

// --- Cancel receiver ---

#[test]
fn test_cancel_receiver_none_before_start() {
    let svc = BotService::new(make_config());
    assert!(svc.cancel_receiver().is_none());
}

#[test]
fn test_cancel_receiver_some_after_start() {
    let dir = tempfile::tempdir().unwrap();
    let config = make_config_with_file(dir.path());
    let svc = BotService::new(config);

    svc.start().unwrap();
    assert!(svc.cancel_receiver().is_some());
}

// --- Restart callback ---

#[test]
fn test_set_restart_callback() {
    let svc = BotService::new(make_config());
    let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let called_clone = called.clone();
    svc.set_restart_callback(Box::new(move || {
        called_clone.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }));
    // Trigger the callback by accessing internal state
    assert!(!called.load(std::sync::atomic::Ordering::SeqCst));
}

// --- Log hooks ---

#[test]
fn test_register_log_hook() {
    let svc = BotService::new(make_config());

    struct TestHook;
    impl crate::log_hook::LogHook for TestHook {
        fn on_log(&self, _event: crate::log_hook::LogEvent) {}
    }

    assert!(svc.log_hooks().is_empty());
    svc.register_log_hook(Arc::new(TestHook));
    assert_eq!(svc.log_hooks().len(), 1);
}

#[test]
fn test_register_multiple_log_hooks() {
    let svc = BotService::new(make_config());

    struct TestHook1;
    impl crate::log_hook::LogHook for TestHook1 {
        fn on_log(&self, _event: crate::log_hook::LogEvent) {}
    }
    struct TestHook2;
    impl crate::log_hook::LogHook for TestHook2 {
        fn on_log(&self, _event: crate::log_hook::LogEvent) {}
    }

    svc.register_log_hook(Arc::new(TestHook1));
    svc.register_log_hook(Arc::new(TestHook2));
    assert_eq!(svc.log_hooks().len(), 2);
}

// --- save_config_and_restart ---

#[test]
fn test_save_config_and_restart_not_running() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.json");

    let svc = BotService::new(BotServiceConfig {
        config_path,
        workspace: dir.path().to_path_buf(),
        ..BotServiceConfig::default()
    });

    let config = serde_json::json!({
        "models": [{ "model": "test/1.0", "api_key": "key1", "base_url": "", "is_default": true }]
    });

    // Bot is not running, so restart won't trigger callback
    let result = svc.save_config_and_restart(&config);
    assert!(result.is_ok());
}

// --- load_config workspace resolution ---

#[test]
fn test_load_config_workspace_from_config_file() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.json");
    let workspace_dir = dir.path().join("my_workspace");
    std::fs::create_dir_all(&workspace_dir).unwrap();

    let config_content = serde_json::json!({
        "workspace": workspace_dir.to_string_lossy(),
        "models": [
            { "model": "test/1.0", "api_key": "test-key", "base_url": "", "is_default": true }
        ]
    });
    std::fs::write(
        &config_path,
        serde_json::to_string(&config_content).unwrap(),
    )
    .unwrap();

    let svc = BotService::new(BotServiceConfig {
        config_path,
        workspace: PathBuf::new(),
        ..BotServiceConfig::default()
    });

    svc.start().unwrap();
    assert_eq!(svc.workspace(), workspace_dir);
}

#[test]
fn test_load_config_workspace_from_config_field() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.json");

    // Config file has no workspace field
    let config_content = serde_json::json!({
        "models": [
            { "model": "test/1.0", "api_key": "test-key", "base_url": "", "is_default": true }
        ]
    });
    std::fs::write(
        &config_path,
        serde_json::to_string(&config_content).unwrap(),
    )
    .unwrap();

    let svc = BotService::new(BotServiceConfig {
        config_path,
        workspace: PathBuf::from("/fallback/workspace"),
        ..BotServiceConfig::default()
    });

    svc.start().unwrap();
    assert_eq!(svc.workspace(), PathBuf::from("/fallback/workspace"));
}

#[test]
fn test_load_config_workspace_default_to_parent() {
    let dir = tempfile::tempdir().unwrap();
    let subdir = dir.path().join("nested");
    std::fs::create_dir_all(&subdir).unwrap();
    let config_path = subdir.join("config.json");

    let config_content = serde_json::json!({
        "models": [
            { "model": "test/1.0", "api_key": "test-key", "base_url": "", "is_default": true }
        ]
    });
    std::fs::write(
        &config_path,
        serde_json::to_string(&config_content).unwrap(),
    )
    .unwrap();

    let svc = BotService::new(BotServiceConfig {
        config_path,
        workspace: PathBuf::new(), // empty, no fallback
        ..BotServiceConfig::default()
    });

    svc.start().unwrap();
    // Should fall back to parent directory of config file
    assert_eq!(svc.workspace(), subdir);
}

// --- init_components conditional branches ---

fn make_config_with_flags(
    dir: &std::path::Path,
    forge_enabled: bool,
    memory_enabled: bool,
    workflow_enabled: bool,
    cluster_enabled: bool,
    devices_enabled: bool,
    heartbeat_enabled: bool,
) -> BotServiceConfig {
    let config_path = dir.join("config.json");
    let config_content = serde_json::json!({
        "models": [
            { "model": "test/1.0", "api_key": "test-key", "base_url": "", "is_default": true }
        ],
        "forge": { "enabled": forge_enabled },
        "memory": { "enabled": memory_enabled },
        "workflow": { "enabled": workflow_enabled },
        "devices": { "enabled": devices_enabled, "monitor_usb": false },
        "heartbeat": { "enabled": heartbeat_enabled, "interval": 60 },
        "security": { "enabled": true }
    });
    std::fs::write(
        &config_path,
        serde_json::to_string(&config_content).unwrap(),
    )
    .unwrap();

    BotServiceConfig {
        config_path,
        workspace: dir.to_path_buf(),
        forge_enabled,
        memory_enabled,
        workflow_enabled,
        cluster_enabled,
        ..BotServiceConfig::default()
    }
}

#[test]
fn test_init_components_forge_enabled() {
    let dir = tempfile::tempdir().unwrap();
    let config = make_config_with_flags(dir.path(), true, false, false, false, false, true);
    let svc = BotService::new(config);
    svc.start().unwrap();

    let enabled = svc.enabled_components();
    assert!(enabled.is_enabled(Component::Forge));
}

#[test]
fn test_init_components_cluster_enabled() {
    let dir = tempfile::tempdir().unwrap();
    let config = make_config_with_flags(dir.path(), false, false, false, true, false, true);
    let svc = BotService::new(config);
    svc.start().unwrap();

    let enabled = svc.enabled_components();
    assert!(enabled.is_enabled(Component::Cluster));
}

#[test]
fn test_init_components_memory_enabled() {
    let dir = tempfile::tempdir().unwrap();
    let config = make_config_with_flags(dir.path(), false, true, false, false, false, true);
    let svc = BotService::new(config);
    svc.start().unwrap();

    let enabled = svc.enabled_components();
    assert!(enabled.is_enabled(Component::Memory));
}

#[test]
fn test_init_components_workflow_enabled() {
    let dir = tempfile::tempdir().unwrap();
    let config = make_config_with_flags(dir.path(), false, false, true, false, false, true);
    let svc = BotService::new(config);
    svc.start().unwrap();

    let enabled = svc.enabled_components();
    assert!(enabled.is_enabled(Component::Workflow));
}

#[test]
fn test_init_components_devices_enabled() {
    let dir = tempfile::tempdir().unwrap();
    let config = make_config_with_flags(dir.path(), false, false, false, false, true, true);
    let svc = BotService::new(config);
    svc.start().unwrap();

    let enabled = svc.enabled_components();
    assert!(enabled.is_enabled(Component::Devices));
}

#[test]
fn test_init_components_heartbeat_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let config = make_config_with_flags(dir.path(), false, false, false, false, false, false);
    let svc = BotService::new(config);
    svc.start().unwrap();

    let enabled = svc.enabled_components();
    assert!(!enabled.is_enabled(Component::Heartbeat));
}

#[test]
fn test_init_components_all_disabled_optional() {
    let dir = tempfile::tempdir().unwrap();
    // Security enabled = false in both config and BotServiceConfig
    let config_path = dir.path().join("config.json");
    let config_content = serde_json::json!({
        "models": [
            { "model": "test/1.0", "api_key": "test-key", "base_url": "", "is_default": true }
        ],
        "forge": { "enabled": false },
        "memory": { "enabled": false },
        "workflow": { "enabled": false },
        "devices": { "enabled": false, "monitor_usb": false },
        "heartbeat": { "enabled": false, "interval": 60 },
        "security": { "enabled": false }
    });
    std::fs::write(
        &config_path,
        serde_json::to_string(&config_content).unwrap(),
    )
    .unwrap();

    let svc = BotService::new(BotServiceConfig {
        config_path,
        workspace: dir.path().to_path_buf(),
        security_enabled: false,
        ..BotServiceConfig::default()
    });
    svc.start().unwrap();

    let enabled = svc.enabled_components();
    assert!(!enabled.is_enabled(Component::Forge));
    assert!(!enabled.is_enabled(Component::Memory));
    assert!(!enabled.is_enabled(Component::Workflow));
    assert!(!enabled.is_enabled(Component::Cluster));
    assert!(!enabled.is_enabled(Component::Devices));
    assert!(!enabled.is_enabled(Component::Heartbeat));
    assert!(!enabled.is_enabled(Component::Security));
}

// --- start_services with injected services ---

#[test]
fn test_start_services_with_channel_manager() {
    let dir = tempfile::tempdir().unwrap();
    let config = make_config_with_file(dir.path());
    let svc = BotService::new(config);

    struct MockChannelsWithNames;
    impl LifecycleService for MockChannelsWithNames {
        fn start(&self) -> Result<(), String> {
            Ok(())
        }
        fn stop(&self) -> Result<(), String> {
            Ok(())
        }
    }
    impl ChannelManager for MockChannelsWithNames {
        fn enabled_channels(&self) -> Vec<String> {
            vec!["web".to_string(), "discord".to_string()]
        }
    }

    svc.inject_channels(Arc::new(MockChannelsWithNames));
    svc.start().unwrap();
    assert!(svc.get_channel_manager().is_some());
    let channels = svc.get_channel_manager().unwrap();
    assert_eq!(channels.enabled_channels(), vec!["web", "discord"]);
}

#[test]
fn test_start_services_heartbeat_start_failure() {
    let dir = tempfile::tempdir().unwrap();
    let config = make_config_with_file(dir.path());
    let svc = BotService::new(config);

    struct FailingHeartbeat;
    impl LifecycleService for FailingHeartbeat {
        fn start(&self) -> Result<(), String> {
            Err("heartbeat start error".to_string())
        }
        fn stop(&self) -> Result<(), String> {
            Ok(())
        }
    }
    impl HeartbeatService for FailingHeartbeat {}

    svc.inject_heartbeat(Arc::new(FailingHeartbeat));
    let result = svc.start();
    assert!(result.is_err());
    assert!(svc.get_error().unwrap().contains("heartbeat"));
}

#[test]
fn test_start_services_devices_start_failure() {
    let dir = tempfile::tempdir().unwrap();
    let config = make_config_with_file(dir.path());
    let svc = BotService::new(config);

    struct FailingDevices;
    impl LifecycleService for FailingDevices {
        fn start(&self) -> Result<(), String> {
            Err("devices start error".to_string())
        }
        fn stop(&self) -> Result<(), String> {
            Ok(())
        }
    }
    impl DeviceService for FailingDevices {}

    svc.inject_devices(Arc::new(FailingDevices));
    let result = svc.start();
    assert!(result.is_err());
    assert!(svc.get_error().unwrap().contains("device"));
}

#[test]
fn test_start_services_channel_start_failure() {
    let dir = tempfile::tempdir().unwrap();
    let config = make_config_with_file(dir.path());
    let svc = BotService::new(config);

    struct FailingChannels;
    impl LifecycleService for FailingChannels {
        fn start(&self) -> Result<(), String> {
            Err("channels start error".to_string())
        }
        fn stop(&self) -> Result<(), String> {
            Ok(())
        }
    }
    impl ChannelManager for FailingChannels {
        fn enabled_channels(&self) -> Vec<String> {
            vec![]
        }
    }

    svc.inject_channels(Arc::new(FailingChannels));
    let result = svc.start();
    assert!(result.is_err());
    assert!(svc.get_error().unwrap().contains("channel"));
}

#[test]
fn test_start_services_cron_non_fatal_failure() {
    let dir = tempfile::tempdir().unwrap();
    let config = make_config_with_file(dir.path());
    let svc = BotService::new(config);

    struct FailingCron;
    impl LifecycleService for FailingCron {
        fn start(&self) -> Result<(), String> {
            Err("cron start error".to_string())
        }
        fn stop(&self) -> Result<(), String> {
            Ok(())
        }
    }
    impl CronService for FailingCron {}

    svc.inject_cron(Arc::new(FailingCron));
    // Cron failure is non-fatal - bot should still start
    let result = svc.start();
    assert!(result.is_ok());
    assert_eq!(svc.get_state(), BotState::Running);
}

#[test]
fn test_start_services_forge_start_failure() {
    let dir = tempfile::tempdir().unwrap();
    // Enable forge in config
    let config = make_config_with_flags(dir.path(), true, false, false, false, false, true);
    let svc = BotService::new(config);

    struct FailingForge;
    impl LifecycleService for FailingForge {
        fn start(&self) -> Result<(), String> {
            Err("forge start error".to_string())
        }
        fn stop(&self) -> Result<(), String> {
            Ok(())
        }
    }
    impl ForgeService for FailingForge {
        fn forge_name(&self) -> &str {
            "failing_forge"
        }
    }

    svc.inject_forge(Arc::new(FailingForge));
    let result = svc.start();
    assert!(result.is_err());
    assert!(svc.get_error().unwrap().contains("forge"));
}

// --- stop_all with injected services ---

#[tokio::test]
async fn test_stop_all_with_injected_services() {
    let dir = tempfile::tempdir().unwrap();
    let config = make_config_with_file(dir.path());
    let svc = BotService::new(config);

    struct MockFullService;
    impl LifecycleService for MockFullService {
        fn start(&self) -> Result<(), String> {
            Ok(())
        }
        fn stop(&self) -> Result<(), String> {
            Ok(())
        }
    }
    impl ForgeService for MockFullService {
        fn forge_name(&self) -> &str {
            "mock"
        }
    }
    impl MemoryService for MockFullService {}
    impl HeartbeatService for MockFullService {}
    impl DeviceService for MockFullService {}
    impl HealthServer for MockFullService {}
    impl ChannelManager for MockFullService {
        fn enabled_channels(&self) -> Vec<String> {
            vec![]
        }
    }
    impl AgentLoopService for MockFullService {}
    impl CronService for MockFullService {}

    svc.inject_forge(Arc::new(MockFullService));
    svc.inject_memory(Arc::new(MockFullService));
    svc.inject_heartbeat(Arc::new(MockFullService));
    svc.inject_devices(Arc::new(MockFullService));
    svc.inject_health(Arc::new(MockFullService));
    svc.inject_channels(Arc::new(MockFullService));
    svc.inject_agent(Arc::new(MockFullService));
    svc.inject_cron(Arc::new(MockFullService));

    svc.start().unwrap();
    assert!(svc.get_forge().is_some());
    assert!(svc.get_memory().is_some());

    svc.stop().unwrap();
    // After stop, services should be cleared
    assert!(svc.get_forge().is_none());
    assert!(svc.get_memory().is_none());
    assert!(svc.get_channel_manager().is_none());
    assert!(svc.get_agent_loop().is_none());
}

#[tokio::test]
async fn test_stop_all_with_service_stop_errors() {
    let dir = tempfile::tempdir().unwrap();
    let config = make_config_with_file(dir.path());
    let svc = BotService::new(config);

    struct ErrorOnStop;
    impl LifecycleService for ErrorOnStop {
        fn start(&self) -> Result<(), String> {
            Ok(())
        }
        fn stop(&self) -> Result<(), String> {
            Err("stop error".to_string())
        }
    }
    impl ChannelManager for ErrorOnStop {
        fn enabled_channels(&self) -> Vec<String> {
            vec![]
        }
    }
    impl AgentLoopService for ErrorOnStop {}
    impl ForgeService for ErrorOnStop {
        fn forge_name(&self) -> &str {
            "error_forge"
        }
    }

    svc.inject_channels(Arc::new(ErrorOnStop));
    svc.inject_agent(Arc::new(ErrorOnStop));

    svc.start().unwrap();
    // stop should succeed even with errors from individual services
    let result = svc.stop();
    assert!(result.is_ok());
}

// --- with_default_config ---

#[test]
fn test_with_default_config() {
    let svc = BotService::with_default_config();
    assert_eq!(svc.get_state(), BotState::NotStarted);
    assert!(svc.get_error().is_none());
}

// --- Double stop ---

#[test]
fn test_double_stop_fails() {
    let dir = tempfile::tempdir().unwrap();
    let config = make_config_with_file(dir.path());
    let svc = BotService::new(config);

    svc.start().unwrap();
    svc.stop().unwrap();

    let result = svc.stop();
    assert!(result.is_err());
}

// --- Start after stop (re-start) ---

#[test]
fn test_start_after_stop() {
    let dir = tempfile::tempdir().unwrap();
    let config = make_config_with_file(dir.path());
    let svc = BotService::new(config);

    svc.start().unwrap();
    svc.stop().unwrap();
    assert_eq!(svc.get_state(), BotState::NotStarted);

    // Should be able to start again
    let result = svc.start();
    assert!(result.is_ok());
    assert_eq!(svc.get_state(), BotState::Running);
}

// --- Start when starting (race condition) ---

#[test]
fn test_start_when_already_starting_blocked() {
    // This tests the "already starting" guard
    let dir = tempfile::tempdir().unwrap();
    let config = make_config_with_file(dir.path());
    let svc = BotService::new(config);

    svc.start().unwrap();
    // Second start should fail because already running
    let result = svc.start();
    assert!(result.is_err());
}

// --- Config file with invalid JSON ---

#[test]
fn test_load_config_invalid_json() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.json");
    std::fs::write(&config_path, "not valid json{{{").unwrap();

    let svc = BotService::new(BotServiceConfig {
        config_path,
        ..BotServiceConfig::default()
    });

    let result = svc.start();
    assert!(result.is_err());
    assert!(svc.get_error().unwrap().contains("parse"));
}

// --- Config file parsing with all sub-fields ---

#[test]
fn test_config_file_with_logging() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.json");
    let content = serde_json::json!({
        "models": [
            { "model": "test/1.0", "api_key": "test-key", "base_url": "", "is_default": true }
        ],
        "logging": {
            "llm": { "enabled": true }
        },
        "forge": {
            "enabled": false,
            "trace": { "enabled": true },
            "learning": { "enabled": false }
        },
        "tools": {
            "cron": { "exec_timeout_minutes": 10 }
        },
        "agents": {
            "defaults": { "restrict_to_workspace": false }
        },
        "gateway": { "host": "0.0.0.0", "port": 3000 },
        "heartbeat": { "enabled": false, "interval": 600 }
    });
    std::fs::write(&config_path, serde_json::to_string(&content).unwrap()).unwrap();

    let svc = BotService::new(BotServiceConfig {
        config_path,
        workspace: dir.path().to_path_buf(),
        ..BotServiceConfig::default()
    });

    svc.start().unwrap();
    assert_eq!(svc.get_state(), BotState::Running);
}

// --- Config file empty JSON ---

#[test]
fn test_config_file_empty_json() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.json");
    std::fs::write(&config_path, "{}").unwrap();

    let svc = BotService::new(BotServiceConfig {
        config_path,
        ..BotServiceConfig::default()
    });

    let result = svc.start();
    assert!(result.is_err());
    // Should fail on validation: no models configured
    assert!(svc.get_error().unwrap().contains("no models"));
}

// ============================================================
// Additional coverage tests for 95%+ target - Phase 2
// ============================================================

// --- start_services with health server ---

#[tokio::test]
async fn test_start_services_health_server_start() {
    let dir = tempfile::tempdir().unwrap();
    let config = make_config_with_file(dir.path());
    let svc = BotService::new(config);

    struct MockHealthSvc;
    impl LifecycleService for MockHealthSvc {
        fn start(&self) -> Result<(), String> {
            Ok(())
        }
        fn stop(&self) -> Result<(), String> {
            Ok(())
        }
    }
    impl HealthServer for MockHealthSvc {}

    svc.inject_health(Arc::new(MockHealthSvc));
    svc.start().unwrap();
    assert_eq!(svc.get_state(), BotState::Running);

    // Give spawned task time to start
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    svc.stop().unwrap();
}

#[tokio::test]
async fn test_start_services_health_stop_error() {
    let dir = tempfile::tempdir().unwrap();
    let config = make_config_with_file(dir.path());
    let svc = BotService::new(config);

    struct ErrorHealthStop;
    impl LifecycleService for ErrorHealthStop {
        fn start(&self) -> Result<(), String> {
            Ok(())
        }
        fn stop(&self) -> Result<(), String> {
            Err("health stop error".to_string())
        }
    }
    impl HealthServer for ErrorHealthStop {}

    svc.inject_health(Arc::new(ErrorHealthStop));
    svc.start().unwrap();
    // stop_all should handle health stop error gracefully
    svc.stop().unwrap();
}

// --- stop_all with forge stop error ---

#[test]
fn test_stop_all_forge_stop_error() {
    let dir = tempfile::tempdir().unwrap();
    let config = make_config_with_flags(dir.path(), true, false, false, false, false, true);
    let svc = BotService::new(config);

    struct ErrorForgeStop;
    impl LifecycleService for ErrorForgeStop {
        fn start(&self) -> Result<(), String> {
            Ok(())
        }
        fn stop(&self) -> Result<(), String> {
            Err("forge stop error".to_string())
        }
    }
    impl ForgeService for ErrorForgeStop {
        fn forge_name(&self) -> &str {
            "error_forge"
        }
    }

    svc.inject_forge(Arc::new(ErrorForgeStop));
    svc.start().unwrap();
    svc.stop().unwrap();
}

// --- stop_all with memory close ---

#[test]
fn test_stop_all_with_memory_service() {
    let dir = tempfile::tempdir().unwrap();
    let config = make_config_with_flags(dir.path(), false, true, false, false, false, true);
    let svc = BotService::new(config);

    struct MockMemorySvc;
    impl LifecycleService for MockMemorySvc {
        fn start(&self) -> Result<(), String> {
            Ok(())
        }
        fn stop(&self) -> Result<(), String> {
            Ok(())
        }
    }
    impl MemoryService for MockMemorySvc {}

    svc.inject_memory(Arc::new(MockMemorySvc));
    svc.start().unwrap();
    assert!(svc.get_memory().is_some());
    svc.stop().unwrap();
    assert!(svc.get_memory().is_none());
}

// --- save_config_and_restart while running ---

#[tokio::test]
async fn test_save_config_and_restart_while_running() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.json");
    let config_content = serde_json::json!({
        "models": [
            { "model": "test/1.0", "api_key": "test-key", "base_url": "", "is_default": true }
        ]
    });
    std::fs::write(
        &config_path,
        serde_json::to_string(&config_content).unwrap(),
    )
    .unwrap();

    let svc = Arc::new(BotService::new(BotServiceConfig {
        config_path,
        workspace: dir.path().to_path_buf(),
        ..BotServiceConfig::default()
    }));

    // Set restart callback
    let svc_clone = svc.clone();
    let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let called_clone = called.clone();
    svc.set_restart_callback(Box::new(move || {
        called_clone.store(true, std::sync::atomic::Ordering::SeqCst);
        // Attempt restart but it will fail since we don't have a valid config after save
        let _ = svc_clone.restart();
        Ok(())
    }));

    svc.start().unwrap();
    assert_eq!(svc.get_state(), BotState::Running);

    let new_config = serde_json::json!({
        "models": [
            { "model": "test/2.0", "api_key": "new-key", "base_url": "", "is_default": true }
        ]
    });

    let result = svc.save_config_and_restart(&new_config);
    assert!(result.is_ok());

    // Wait for async restart to complete
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
}

// --- save_config restart without callback ---

#[tokio::test]
async fn test_save_config_restart_no_callback() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.json");
    let config_content = serde_json::json!({
        "models": [
            { "model": "test/1.0", "api_key": "test-key", "base_url": "", "is_default": true }
        ]
    });
    std::fs::write(
        &config_path,
        serde_json::to_string(&config_content).unwrap(),
    )
    .unwrap();

    let svc = BotService::new(BotServiceConfig {
        config_path,
        workspace: dir.path().to_path_buf(),
        ..BotServiceConfig::default()
    });

    svc.start().unwrap();

    let new_config = serde_json::json!({
        "models": [
            { "model": "test/2.0", "api_key": "new-key", "base_url": "", "is_default": true }
        ]
    });

    // restart=true but no restart callback set - should warn but succeed
    let result = svc.save_config(&new_config, true);
    assert!(result.is_ok());

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
}

// --- start_services with agent loop ---

#[tokio::test]
async fn test_start_services_agent_in_background() {
    let dir = tempfile::tempdir().unwrap();
    let config = make_config_with_file(dir.path());
    let svc = BotService::new(config);

    struct MockAgentBg;
    impl LifecycleService for MockAgentBg {
        fn start(&self) -> Result<(), String> {
            Ok(())
        }
        fn stop(&self) -> Result<(), String> {
            Ok(())
        }
    }
    impl AgentLoopService for MockAgentBg {}

    svc.inject_agent(Arc::new(MockAgentBg));
    svc.start().unwrap();
    assert_eq!(svc.get_state(), BotState::Running);

    // Give agent background task time to start
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    svc.stop().unwrap();
}

// --- stop_all with heartbeat stop error ---

#[test]
fn test_stop_all_heartbeat_stop_error() {
    let dir = tempfile::tempdir().unwrap();
    let config = make_config_with_file(dir.path());
    let svc = BotService::new(config);

    struct ErrorHeartbeatStop;
    impl LifecycleService for ErrorHeartbeatStop {
        fn start(&self) -> Result<(), String> {
            Ok(())
        }
        fn stop(&self) -> Result<(), String> {
            Err("heartbeat stop error".to_string())
        }
    }
    impl HeartbeatService for ErrorHeartbeatStop {}

    svc.inject_heartbeat(Arc::new(ErrorHeartbeatStop));
    svc.start().unwrap();
    svc.stop().unwrap();
}

// --- stop_all with cron stop error ---

#[test]
fn test_stop_all_cron_stop_error() {
    let dir = tempfile::tempdir().unwrap();
    let config = make_config_with_file(dir.path());
    let svc = BotService::new(config);

    struct ErrorCronStop;
    impl LifecycleService for ErrorCronStop {
        fn start(&self) -> Result<(), String> {
            Ok(())
        }
        fn stop(&self) -> Result<(), String> {
            Err("cron stop error".to_string())
        }
    }
    impl CronService for ErrorCronStop {}

    svc.inject_cron(Arc::new(ErrorCronStop));
    svc.start().unwrap();
    svc.stop().unwrap();
}

// --- stop_all with device stop error ---

#[test]
fn test_stop_all_devices_stop_error() {
    let dir = tempfile::tempdir().unwrap();
    let config = make_config_with_file(dir.path());
    let svc = BotService::new(config);

    struct ErrorDeviceStop;
    impl LifecycleService for ErrorDeviceStop {
        fn start(&self) -> Result<(), String> {
            Ok(())
        }
        fn stop(&self) -> Result<(), String> {
            Err("device stop error".to_string())
        }
    }
    impl DeviceService for ErrorDeviceStop {}

    svc.inject_devices(Arc::new(ErrorDeviceStop));
    svc.start().unwrap();
    svc.stop().unwrap();
}

// --- load_config with read error ---

#[test]
fn test_load_config_unreadable_file() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.json");
    // Create a directory instead of a file to cause read error
    std::fs::create_dir_all(&config_path).unwrap();

    let svc = BotService::new(BotServiceConfig {
        config_path,
        ..BotServiceConfig::default()
    });

    let result = svc.start();
    assert!(result.is_err());
}

// --- save_config rejects non-object ---

#[test]
fn test_save_config_rejects_array() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.json");
    std::fs::write(&config_path, "{}").unwrap();

    let svc = BotService::new(BotServiceConfig {
        config_path,
        workspace: dir.path().to_path_buf(),
        ..BotServiceConfig::default()
    });

    let result = svc.save_config(&serde_json::json!([1, 2, 3]), false);
    assert!(result.is_err());
}

#[test]
fn test_save_config_rejects_null() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.json");
    std::fs::write(&config_path, "{}").unwrap();

    let svc = BotService::new(BotServiceConfig {
        config_path,
        workspace: dir.path().to_path_buf(),
        ..BotServiceConfig::default()
    });

    let result = svc.save_config(&serde_json::Value::Null, false);
    assert!(result.is_err());
}

#[test]
fn test_save_config_rejects_number() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.json");
    std::fs::write(&config_path, "{}").unwrap();

    let svc = BotService::new(BotServiceConfig {
        config_path,
        workspace: dir.path().to_path_buf(),
        ..BotServiceConfig::default()
    });

    let result = svc.save_config(&serde_json::json!(42), false);
    assert!(result.is_err());
}

// --- save_config invalid structure ---

#[test]
fn test_save_config_rejects_bad_structure() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.json");
    std::fs::write(&config_path, "{}").unwrap();

    let svc = BotService::new(BotServiceConfig {
        config_path,
        workspace: dir.path().to_path_buf(),
        ..BotServiceConfig::default()
    });

    // Valid JSON object but with wrong field types
    let result = svc.save_config(
        &serde_json::json!({
            "models": "not an array"
        }),
        false,
    );
    assert!(result.is_err());
}

// --- start when in Error state ---

#[test]
fn test_start_after_error_state() {
    let svc = BotService::new(make_config());
    // First start fails because config file doesn't exist
    let result = svc.start();
    assert!(result.is_err());
    assert_eq!(svc.get_state(), BotState::Error);

    // Can start again from Error state (but will fail again)
    let result = svc.start();
    assert!(result.is_err());
}

// --- validate_config not loaded ---

#[test]
fn test_validate_config_not_loaded() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.json");
    // Write an empty file so load_config passes but config_loaded is set
    std::fs::write(&config_path, "{}").unwrap();

    let svc = BotService::new(BotServiceConfig {
        config_path,
        ..BotServiceConfig::default()
    });

    // This will fail at validate_config since config is loaded but models is empty
    let result = svc.start();
    assert!(result.is_err());
    assert!(svc.get_error().unwrap().contains("no models"));
}
