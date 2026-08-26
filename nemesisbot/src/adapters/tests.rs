use super::*;

fn make_health_config(port: u16) -> nemesis_health::server::HealthServerConfig {
    nemesis_health::server::HealthServerConfig {
        listen_addr: format!("127.0.0.1:{}", port),
        version: Some("test".to_string()),
    }
}

fn make_heartbeat_config() -> nemesis_heartbeat::HeartbeatConfig {
    nemesis_heartbeat::HeartbeatConfig::new(
        30,
        true,
        std::env::temp_dir().to_string_lossy().to_string(),
    )
}

// -------------------------------------------------------------------------
// HealthServerAdapter construction
// -------------------------------------------------------------------------

#[tokio::test]
async fn test_health_server_adapter_initial_state() {
    let health_server = Arc::new(nemesis_health::server::HealthServer::new(
        make_health_config(18790),
    ));
    let adapter = HealthServerAdapter::new(health_server);
    assert!(adapter.start().is_ok());
}

#[test]
fn test_health_server_adapter_stop() {
    let health_server = Arc::new(nemesis_health::server::HealthServer::new(
        make_health_config(18791),
    ));
    let adapter = HealthServerAdapter::new(health_server);
    assert!(adapter.stop().is_ok());
}

#[tokio::test]
async fn test_health_server_adapter_start_idempotent() {
    let health_server = Arc::new(nemesis_health::server::HealthServer::new(
        make_health_config(18792),
    ));
    let adapter = HealthServerAdapter::new(health_server);
    assert!(adapter.start().is_ok());
    assert!(adapter.start().is_ok());
    assert!(adapter.stop().is_ok());
}

// -------------------------------------------------------------------------
// HeartbeatServiceAdapter construction
// -------------------------------------------------------------------------

#[tokio::test]
async fn test_heartbeat_adapter_initial_state() {
    let heartbeat = Arc::new(nemesis_heartbeat::service::HeartbeatService::new(
        make_heartbeat_config(),
    ));
    let adapter = HeartbeatServiceAdapter::new(heartbeat);
    assert!(adapter.start().is_ok());
}

#[test]
fn test_heartbeat_adapter_stop() {
    let heartbeat = Arc::new(nemesis_heartbeat::service::HeartbeatService::new(
        make_heartbeat_config(),
    ));
    let adapter = HeartbeatServiceAdapter::new(heartbeat);
    assert!(adapter.stop().is_ok());
}

#[tokio::test]
async fn test_heartbeat_adapter_start_idempotent() {
    let heartbeat = Arc::new(nemesis_heartbeat::service::HeartbeatService::new(
        make_heartbeat_config(),
    ));
    let adapter = HeartbeatServiceAdapter::new(heartbeat);
    assert!(adapter.start().is_ok());
    assert!(adapter.start().is_ok());
    assert!(adapter.stop().is_ok());
}

// -------------------------------------------------------------------------
// ChannelManagerAdapter construction
// -------------------------------------------------------------------------

#[test]
fn test_channel_manager_adapter_enabled_channels() {
    let manager = Arc::new(nemesis_channels::manager::ChannelManager::new());
    let channels = vec!["web".to_string(), "websocket".to_string()];
    let adapter = ChannelManagerAdapter::new(manager, channels.clone());
    assert_eq!(adapter.enabled_channels(), channels);
}

#[test]
fn test_channel_manager_adapter_empty_channels() {
    let manager = Arc::new(nemesis_channels::manager::ChannelManager::new());
    let adapter = ChannelManagerAdapter::new(manager, vec![]);
    assert!(adapter.enabled_channels().is_empty());
}

#[tokio::test]
async fn test_channel_manager_adapter_start() {
    let manager = Arc::new(nemesis_channels::manager::ChannelManager::new());
    let adapter = ChannelManagerAdapter::new(manager, vec!["web".to_string()]);
    assert!(adapter.start().is_ok());
}

#[tokio::test]
async fn test_channel_manager_adapter_stop() {
    let manager = Arc::new(nemesis_channels::manager::ChannelManager::new());
    let adapter = ChannelManagerAdapter::new(manager, vec![]);
    assert!(adapter.stop().is_ok());
}

#[tokio::test]
async fn test_channel_manager_adapter_start_idempotent() {
    let manager = Arc::new(nemesis_channels::manager::ChannelManager::new());
    let adapter = ChannelManagerAdapter::new(manager, vec![]);
    assert!(adapter.start().is_ok());
    assert!(adapter.start().is_ok());
}

// -------------------------------------------------------------------------
// AtomicBool ordering test
// -------------------------------------------------------------------------

#[test]
fn test_atomic_bool_swap_behavior() {
    let flag = AtomicBool::new(false);
    assert!(!flag.swap(true, Ordering::SeqCst));
    assert!(flag.swap(true, Ordering::SeqCst));
    assert!(flag.swap(false, Ordering::SeqCst));
    assert!(!flag.swap(false, Ordering::SeqCst));
}

// -------------------------------------------------------------------------
// LifecycleService trait tests
// -------------------------------------------------------------------------

#[tokio::test]
async fn test_health_server_adapter_trait_object() {
    let health_server = Arc::new(nemesis_health::server::HealthServer::new(
        make_health_config(18793),
    ));
    let adapter = HealthServerAdapter::new(health_server);
    let _trait_obj: &dyn LifecycleService = &adapter;
    assert!(adapter.start().is_ok());
}

#[tokio::test]
async fn test_heartbeat_adapter_trait_object() {
    let heartbeat = Arc::new(nemesis_heartbeat::service::HeartbeatService::new(
        make_heartbeat_config(),
    ));
    let adapter = HeartbeatServiceAdapter::new(heartbeat);
    let _trait_obj: &dyn LifecycleService = &adapter;
    assert!(adapter.start().is_ok());
}

#[tokio::test]
async fn test_channel_manager_adapter_trait_object() {
    let manager = Arc::new(nemesis_channels::manager::ChannelManager::new());
    let adapter = ChannelManagerAdapter::new(manager, vec!["web".to_string()]);
    let _trait_obj: &dyn LifecycleService = &adapter;
    assert!(adapter.start().is_ok());
}

// -------------------------------------------------------------------------
// AgentLoopServiceAdapter tests
// -------------------------------------------------------------------------

/// Minimal mock LLM provider for constructing test AgentLoop instances.
struct MockLlmProvider;

#[async_trait::async_trait]
impl nemesis_agent::r#loop::LlmProvider for MockLlmProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<nemesis_agent::r#loop::LlmMessage>,
        _options: Option<nemesis_agent::types::ChatOptions>,
        _tools: Vec<nemesis_agent::types::ToolDefinition>,
    ) -> Result<nemesis_agent::r#loop::LlmResponse, String> {
        Ok(nemesis_agent::r#loop::LlmResponse {
            content: "mock".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

fn make_test_agent_loop() -> Arc<nemesis_agent::r#loop::AgentLoop> {
    let (outbound_tx, _outbound_rx) = tokio::sync::mpsc::channel(16);
    let al = nemesis_agent::r#loop::AgentLoop::new_bus(
        Box::new(MockLlmProvider),
        nemesis_agent::types::AgentConfig {
            model: "test-model".to_string(),
            system_prompt: Some("test".to_string()),
            max_turns: 1,
            tools: vec![],
            ..Default::default()
        },
        outbound_tx,
        nemesis_agent::r#loop::ConcurrentMode::Reject,
        8,
        0,
    );
    Arc::new(al)
}

fn make_test_shared(
    bus: &Arc<nemesis_bus::MessageBus>,
) -> Arc<crate::agent_factory::SharedResources> {
    let (outbound_tx, _outbound_rx) = tokio::sync::mpsc::channel(16);
    Arc::new(crate::agent_factory::SharedResources {
        home: std::path::PathBuf::from("/tmp/test"),
        bus: bus.clone(),
        agent_outbound_tx: outbound_tx,
        cron_service: Arc::new(std::sync::Mutex::new(
            nemesis_cron::service::CronService::new(""),
        )),
        mcp_config_path: std::path::PathBuf::from("/tmp/test/mcp.json"),
        ..Default::default()
    })
}

#[tokio::test]
async fn test_agent_loop_adapter_new() {
    let bus = Arc::new(nemesis_bus::MessageBus::new());
    let shared = make_test_shared(&bus);
    let agent_loop = make_test_agent_loop();
    let agent_loop_ref: Arc<parking_lot::RwLock<Option<Arc<nemesis_agent::r#loop::AgentLoop>>>> =
        Arc::new(parking_lot::RwLock::new(None));
    let adapter = AgentLoopServiceAdapter::new(agent_loop, shared, bus, agent_loop_ref);
    // Has AgentLoop inside but not yet started (no bridge/agent handles)
    assert!(adapter.current().is_some());
    assert!(!LifecycleService::is_running(&adapter));
}

#[tokio::test]
async fn test_agent_loop_adapter_stop_when_not_started() {
    let bus = Arc::new(nemesis_bus::MessageBus::new());
    let shared = make_test_shared(&bus);
    let agent_loop = make_test_agent_loop();
    let agent_loop_ref: Arc<parking_lot::RwLock<Option<Arc<nemesis_agent::r#loop::AgentLoop>>>> =
        Arc::new(parking_lot::RwLock::new(None));
    let adapter = AgentLoopServiceAdapter::new(agent_loop, shared, bus, agent_loop_ref);
    // Stopping when not fully started should still work (drops inner AgentLoop)
    assert!(adapter.stop().is_ok());
    assert!(adapter.current().is_none());
}

#[tokio::test]
async fn test_agent_loop_adapter_trait_object() {
    let bus = Arc::new(nemesis_bus::MessageBus::new());
    let shared = make_test_shared(&bus);
    let agent_loop = make_test_agent_loop();
    let agent_loop_ref: Arc<parking_lot::RwLock<Option<Arc<nemesis_agent::r#loop::AgentLoop>>>> =
        Arc::new(parking_lot::RwLock::new(None));
    let adapter = AgentLoopServiceAdapter::new(agent_loop, shared, bus, agent_loop_ref);
    let _trait_obj: &dyn LifecycleService = &adapter;
    assert!(!LifecycleService::is_running(&adapter));
}

// =========================================================================
// S11d 补测（quality-hardening goal 冲刺 S11）：AgentLoopServiceAdapter 全
// 生命周期三分支（预建 start / 重建 start / 重建失败 Err）+ stop 幂等 +
// cancel 委托两态 + agent_loop_ref 同步；WebServerOpsAdapter 全方法。
// =========================================================================

/// 写一份可离线构建的迷你模型 config（形态与 agent_factory/tests.rs 同源）。
fn write_minimal_model_config(home: &std::path::Path) {
    let cfg = serde_json::json!({
        "agents": { "defaults": { "llm": "mini-model", "max_tool_iterations": 5 } },
        "model_list": [{
            "model_name": "mini-model",
            "model": "testai/mini-model",
            "api_key": "test-key",
            "api_base": "http://127.0.0.1:9",
            "model_tier": "mini"
        }]
    });
    std::fs::create_dir_all(home).unwrap();
    std::fs::write(home.join("config.json"), cfg.to_string()).unwrap();
}

fn make_shared_at_home(
    home: &std::path::Path,
    bus: &Arc<nemesis_bus::MessageBus>,
) -> Arc<crate::agent_factory::SharedResources> {
    let (outbound_tx, _rx) = tokio::sync::mpsc::channel(16);
    Arc::new(crate::agent_factory::SharedResources {
        home: home.to_path_buf(),
        bus: bus.clone(),
        agent_outbound_tx: outbound_tx,
        cron_service: Arc::new(std::sync::Mutex::new(nemesis_cron::service::CronService::new(
            "",
        ))),
        mcp_config_path: home.join("nonexistent-mcp.json"),
        ..Default::default()
    })
}

#[tokio::test]
async fn agent_loop_adapter_full_lifecycle_with_prebuilt_loop() {
    let bus = Arc::new(nemesis_bus::MessageBus::new());
    let shared = make_test_shared(&bus);
    let agent_loop = make_test_agent_loop();
    let agent_loop_ref: Arc<parking_lot::RwLock<Option<Arc<nemesis_agent::r#loop::AgentLoop>>>> =
        Arc::new(parking_lot::RwLock::new(None));
    let adapter = AgentLoopServiceAdapter::new(agent_loop, shared, bus, agent_loop_ref.clone());

    // 初始：有预建 loop 但未启动（无 bridge handle）。
    assert!(adapter.current().is_some());
    assert!(!LifecycleService::is_running(&adapter));

    // start（预建分支）：bridge/agent 任务落位 + agent_loop_ref 同步 Some。
    adapter.start().expect("start with prebuilt loop must succeed");
    assert!(LifecycleService::is_running(&adapter));
    assert!(adapter.current().is_some());
    assert!(agent_loop_ref.read().is_some());

    // 幂等：already-started 分支直接 Ok（不重复装配）。
    assert!(adapter.start().is_ok());

    // 运行中 cancel 委托到内部 AgentLoop（无活跃会话 → false / 0）。
    assert!(!AgentLoopServiceTrait::cancel_session(
        &adapter,
        "no-such-session"
    ));
    assert_eq!(AgentLoopServiceTrait::cancel_all_sessions(&adapter), 0);

    // stop：停任务 + 丢弃 loop + 清共享 ref。
    adapter.stop().expect("stop must succeed");
    assert!(!LifecycleService::is_running(&adapter));
    assert!(adapter.current().is_none());
    assert!(agent_loop_ref.read().is_none());

    // 幂等：already-stopped 分支直接 Ok。
    assert!(adapter.stop().is_ok());

    // 停止后 cancel 委托走 None 分支 → false / 0。
    assert!(!AgentLoopServiceTrait::cancel_session(&adapter, "s"));
    assert_eq!(AgentLoopServiceTrait::cancel_all_sessions(&adapter), 0);
}

#[tokio::test]
async fn agent_loop_adapter_rebuild_after_stop_and_err_on_bad_config() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().to_path_buf();
    write_minimal_model_config(&home);

    let bus = Arc::new(nemesis_bus::MessageBus::new());
    let shared = make_shared_at_home(&home, &bus);
    let agent_loop = make_test_agent_loop();
    let agent_loop_ref: Arc<parking_lot::RwLock<Option<Arc<nemesis_agent::r#loop::AgentLoop>>>> =
        Arc::new(parking_lot::RwLock::new(None));
    let adapter = AgentLoopServiceAdapter::new(agent_loop, shared, bus, agent_loop_ref.clone());

    adapter.start().expect("initial start (prebuilt)");
    adapter.stop().expect("stop before rebuild");
    assert!(adapter.current().is_none());

    // 重建失败分支：删 config.json → 工厂回落默认模型（无 key）→ Err；
    // 状态保持未启动（agent_loop 仍 None，agent_loop_ref 仍 None）。
    std::fs::remove_file(home.join("config.json")).unwrap();
    let err = adapter.start().unwrap_err();
    assert!(
        err.contains("Failed to build agent loop"),
        "err should be wrapped factory failure, got: {err}"
    );
    assert!(adapter.current().is_none());
    assert!(!LifecycleService::is_running(&adapter));
    assert!(agent_loop_ref.read().is_none());

    // 恢复合法 config → 重建成功（走 build_agent_loop 分支）。
    write_minimal_model_config(&home);
    adapter.start().expect("rebuild start must succeed with valid config");
    assert!(LifecycleService::is_running(&adapter));
    assert!(adapter.current().is_some());
    assert!(agent_loop_ref.read().is_some());
    adapter.stop().expect("final stop");
}

// -------------------------------------------------------------------------
// WebServerOpsAdapter（block_in_place 需 multi_thread runtime）
// -------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_server_ops_adapter_all_methods_on_empty_and_registered_sessions() {
    let sm = Arc::new(nemesis_web::session::SessionManager::new(std::time::Duration::from_secs(
        3600,
    )));
    let adapter = WebServerOpsAdapter::new(sm.clone());

    // 空表：active 空、broadcast 无目标直接 Ok、start/stop 是 no-op。
    assert!(adapter.active_session_ids().is_empty());
    assert!(adapter.broadcast("hello").is_ok());
    assert!(adapter.start_server().is_ok());
    adapter.stop_server();

    // 未知 session：广播层报 no send queue。
    let err = adapter
        .send_to_session("no-such-session", "assistant", "hi", None)
        .unwrap_err();
    assert!(
        err.contains("no send queue"),
        "err: {err}"
    );

    // history：坏 JSON → unmarshal 错误；合法 JSON + 未知 session → 广播错误。
    assert!(
        adapter
            .send_history_to_session("s", "not-json")
            .unwrap_err()
            .contains("unmarshal")
    );
    assert!(
        adapter
            .send_history_to_session("s", r#"{"a":1}"#)
            .unwrap_err()
            .contains("no send queue")
    );

    // 注册了 session 但无 WS 发送队列：active 命中、broadcast/send 走 ? 传播 Err。
    let sess = sm.create_session();
    let ids = adapter.active_session_ids();
    assert_eq!(ids, vec![sess.id.clone()]);
    assert!(adapter.broadcast("boom").is_err());
    assert!(adapter
        .send_to_session(&sess.id, "assistant", "hi", Some("prov/model"))
        .is_err());
}
