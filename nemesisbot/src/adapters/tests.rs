use super::*;

fn make_health_config(port: u16) -> nemesis_health::server::HealthServerConfig {
    nemesis_health::server::HealthServerConfig {
        listen_addr: format!("127.0.0.1:{}", port),
        version: Some("test".to_string()),
    }
}

fn make_heartbeat_config() -> nemesis_heartbeat::HeartbeatConfig {
    nemesis_heartbeat::HeartbeatConfig::new(30, true, std::env::temp_dir().to_string_lossy().to_string())
}

// -------------------------------------------------------------------------
// HealthServerAdapter construction
// -------------------------------------------------------------------------

#[tokio::test]
async fn test_health_server_adapter_initial_state() {
    let health_server = Arc::new(nemesis_health::server::HealthServer::new(make_health_config(18790)));
    let adapter = HealthServerAdapter::new(health_server);
    assert!(adapter.start().is_ok());
}

#[test]
fn test_health_server_adapter_stop() {
    let health_server = Arc::new(nemesis_health::server::HealthServer::new(make_health_config(18791)));
    let adapter = HealthServerAdapter::new(health_server);
    assert!(adapter.stop().is_ok());
}

#[tokio::test]
async fn test_health_server_adapter_start_idempotent() {
    let health_server = Arc::new(nemesis_health::server::HealthServer::new(make_health_config(18792)));
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
    let heartbeat = Arc::new(nemesis_heartbeat::service::HeartbeatService::new(make_heartbeat_config()));
    let adapter = HeartbeatServiceAdapter::new(heartbeat);
    assert!(adapter.start().is_ok());
}

#[test]
fn test_heartbeat_adapter_stop() {
    let heartbeat = Arc::new(nemesis_heartbeat::service::HeartbeatService::new(make_heartbeat_config()));
    let adapter = HeartbeatServiceAdapter::new(heartbeat);
    assert!(adapter.stop().is_ok());
}

#[tokio::test]
async fn test_heartbeat_adapter_start_idempotent() {
    let heartbeat = Arc::new(nemesis_heartbeat::service::HeartbeatService::new(make_heartbeat_config()));
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
    let health_server = Arc::new(nemesis_health::server::HealthServer::new(make_health_config(18793)));
    let adapter = HealthServerAdapter::new(health_server);
    let _trait_obj: &dyn LifecycleService = &adapter;
    assert!(adapter.start().is_ok());
}

#[tokio::test]
async fn test_heartbeat_adapter_trait_object() {
    let heartbeat = Arc::new(nemesis_heartbeat::service::HeartbeatService::new(make_heartbeat_config()));
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

/// Minimal mock LLM provider for testing the adapter.
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
            content: "mock response".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
        })
    }
}

fn make_test_agent_loop() -> Arc<nemesis_agent::r#loop::AgentLoop> {
    let (outbound_tx, _outbound_rx) = tokio::sync::mpsc::channel(16);
    let agent_loop = nemesis_agent::r#loop::AgentLoop::new_bus(
        Box::new(MockLlmProvider),
        nemesis_agent::types::AgentConfig {
            model: "test-model".to_string(),
            system_prompt: Some("test".to_string()),
            max_turns: 1,
            tools: vec![],
        },
        outbound_tx,
        nemesis_agent::r#loop::ConcurrentMode::Reject,
        8,
    );
    Arc::new(agent_loop)
}

#[tokio::test]
async fn test_agent_loop_adapter_new() {
    let agent_loop = make_test_agent_loop();
    let bus = Arc::new(nemesis_bus::MessageBus::new());
    let adapter = AgentLoopServiceAdapter::new(agent_loop, bus);
    // Not started yet
    assert!(!adapter.started.load(Ordering::SeqCst));
}

#[tokio::test]
async fn test_agent_loop_adapter_start_stop() {
    let agent_loop = make_test_agent_loop();
    let bus = Arc::new(nemesis_bus::MessageBus::new());
    let adapter = AgentLoopServiceAdapter::new(agent_loop.clone(), bus);

    // Start should succeed
    assert!(adapter.start().is_ok());
    // Agent should be running
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(agent_loop.is_running());

    // Stop should succeed
    assert!(adapter.stop().is_ok());
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(!agent_loop.is_running());
}

#[tokio::test]
async fn test_agent_loop_adapter_start_idempotent() {
    let agent_loop = make_test_agent_loop();
    let bus = Arc::new(nemesis_bus::MessageBus::new());
    let adapter = AgentLoopServiceAdapter::new(agent_loop, bus);

    assert!(adapter.start().is_ok());
    assert!(adapter.start().is_ok()); // Second call is a no-op
    assert!(adapter.stop().is_ok());
}

#[tokio::test]
async fn test_agent_loop_adapter_stop_when_not_started() {
    let agent_loop = make_test_agent_loop();
    let bus = Arc::new(nemesis_bus::MessageBus::new());
    let adapter = AgentLoopServiceAdapter::new(agent_loop, bus);
    // Stopping when not started should be a no-op
    assert!(adapter.stop().is_ok());
}

#[tokio::test]
async fn test_agent_loop_adapter_restart() {
    let agent_loop = make_test_agent_loop();
    let bus = Arc::new(nemesis_bus::MessageBus::new());
    let adapter = AgentLoopServiceAdapter::new(agent_loop.clone(), bus);

    // First cycle
    assert!(adapter.start().is_ok());
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(agent_loop.is_running());

    assert!(adapter.stop().is_ok());
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(!agent_loop.is_running());

    // Second cycle — should restart successfully
    assert!(adapter.start().is_ok());
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(agent_loop.is_running());

    assert!(adapter.stop().is_ok());
}

#[tokio::test]
async fn test_agent_loop_adapter_trait_object() {
    let agent_loop = make_test_agent_loop();
    let bus = Arc::new(nemesis_bus::MessageBus::new());
    let adapter = AgentLoopServiceAdapter::new(agent_loop, bus);
    let _trait_obj: &dyn LifecycleService = &adapter;
    assert!(adapter.start().is_ok());
    assert!(adapter.stop().is_ok());
}
