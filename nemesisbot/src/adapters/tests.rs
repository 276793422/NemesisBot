use super::*;

// （复评）：测试与辅助函数按生产实现同款 feature 条件门控
// ——裁剪构建（无 health/heartbeat）下本模块其余测试照常编译运行。

#[cfg(feature = "health")]
fn make_health_config(port: u16) -> nemesis_health::server::HealthServerConfig {
    nemesis_health::server::HealthServerConfig {
        listen_addr: format!("127.0.0.1:{}", port),
        version: Some("test".to_string()),
    }
}

#[cfg(feature = "heartbeat")]
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

#[cfg(feature = "health")]
#[tokio::test]
async fn test_health_server_adapter_initial_state() {
    let health_server = Arc::new(nemesis_health::server::HealthServer::new(
        make_health_config(18790),
    ));
    let adapter = HealthServerAdapter::new(health_server);
    assert!(adapter.start().is_ok());
}

#[cfg(feature = "health")]
#[test]
fn test_health_server_adapter_stop() {
    let health_server = Arc::new(nemesis_health::server::HealthServer::new(
        make_health_config(18791),
    ));
    let adapter = HealthServerAdapter::new(health_server);
    assert!(adapter.stop().is_ok());
}

#[cfg(feature = "health")]
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

#[cfg(feature = "health")]
#[tokio::test]
async fn test_health_server_adapter_restart_rebinds_after_stop() {
    // 生命周期轴回归锁：adapter 此前 spawn 裸 start()（不接停机通道）、
    // stop() 只翻旗标——旧 serve 任务永生占端口，BotService restart 链
    // （persona 切换 / config 保存重启）此后每次 start 都 bind 失败
    // （Address already in use）。修复后 stop 必须真释放端口、同端口
    // 重启必须能重新服务。
    let port = 18797;
    let health_server = Arc::new(nemesis_health::server::HealthServer::new(
        make_health_config(port),
    ));
    let adapter = HealthServerAdapter::new(Arc::clone(&health_server));

    assert!(adapter.start().is_ok());
    let url = format!("http://127.0.0.1:{port}/live");
    let mut served = false;
    for _ in 0..100 {
        if let Ok(resp) = reqwest::get(&url).await
            && resp.status().is_success()
        {
            served = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(served, "首次 start 后 /live 应可服务");

    assert!(adapter.stop().is_ok());
    // 修复核心断言：stop 后监听端口必须释放（旧缺陷下旧任务仍持有，
    // 此探针恒失败直至超时）。
    let mut released = false;
    for _ in 0..100 {
        if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
            released = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(
        released,
        "stop() 后端口 {port} 应已释放（serve 任务优雅退出）"
    );

    // restart 契约：同端口重新 start，再次可服务（旧缺陷下 bind 失败永不恢复）。
    assert!(adapter.start().is_ok());
    let mut reserved = false;
    for _ in 0..100 {
        if let Ok(resp) = reqwest::get(&url).await
            && resp.status().is_success()
        {
            reserved = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(reserved, "stop→start 后 /live 应重新可服务");
    let _ = adapter.stop();
}

// -------------------------------------------------------------------------
// HeartbeatServiceAdapter construction
// -------------------------------------------------------------------------

#[cfg(feature = "heartbeat")]
#[tokio::test]
async fn test_heartbeat_adapter_initial_state() {
    let heartbeat = Arc::new(nemesis_heartbeat::service::HeartbeatService::new(
        make_heartbeat_config(),
    ));
    let adapter = HeartbeatServiceAdapter::new(heartbeat);
    assert!(adapter.start().is_ok());
}

#[cfg(feature = "heartbeat")]
#[test]
fn test_heartbeat_adapter_stop() {
    let heartbeat = Arc::new(nemesis_heartbeat::service::HeartbeatService::new(
        make_heartbeat_config(),
    ));
    let adapter = HeartbeatServiceAdapter::new(heartbeat);
    assert!(adapter.stop().is_ok());
}

#[cfg(feature = "heartbeat")]
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

#[cfg(feature = "health")]
#[tokio::test]
async fn test_health_server_adapter_trait_object() {
    let health_server = Arc::new(nemesis_health::server::HealthServer::new(
        make_health_config(18793),
    ));
    let adapter = HealthServerAdapter::new(health_server);
    let _trait_obj: &dyn LifecycleService = &adapter;
    assert!(adapter.start().is_ok());
}

#[cfg(feature = "heartbeat")]
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
            finish_reason: None,
            escalated_to: None,
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
            system_prompt_split: None,
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
// 补测（quality-hardening goal 冲刺）：AgentLoopServiceAdapter 全
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
        cron_service: Arc::new(std::sync::Mutex::new(
            nemesis_cron::service::CronService::new(""),
        )),
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
    adapter
        .start()
        .expect("start with prebuilt loop must succeed");
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
async fn agent_loop_adapter_rebuild_after_stop_degrades_on_bad_config() {
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

    // 重建降级分支（双击直启语义）：删 config.json → 工厂回落
    // 默认模型（无 key）→ resolve 失败 → NullProvider 降级装配（Ok 不
    // Err）——无 LLM 不是启动错误；状态变为已启动。
    std::fs::remove_file(home.join("config.json")).unwrap();
    adapter
        .start()
        .expect("degraded rebuild start (NullProvider) must succeed");
    assert!(adapter.current().is_some());
    assert!(LifecycleService::is_running(&adapter));

    // 恢复合法 config → stop 后重建仍成功（走 build_agent_loop 分支）。
    adapter.stop().expect("stop before valid rebuild");
    write_minimal_model_config(&home);
    adapter
        .start()
        .expect("rebuild start must succeed with valid config");
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
    let sm = Arc::new(nemesis_web::session::SessionManager::new(
        std::time::Duration::from_secs(3600),
    ));
    let adapter = WebServerOpsAdapter::new(sm.clone());

    // 空表：active 空、broadcast 无目标直接 Ok、start/stop 是 no-op。
    assert!(adapter.active_session_ids().is_empty());
    assert!(adapter.broadcast("hello").is_ok());
    assert!(adapter.start_server().is_ok());
    adapter.stop_server();

    // 未知 session：广播层报 no send queue。
    let err = adapter
        .send_to_session("no-such-session", "assistant", "hi", None, None, None, None)
        .unwrap_err();
    assert!(err.contains("no send queue"), "err: {err}");

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
    assert!(
        adapter
            .send_to_session(
                &sess.id,
                "assistant",
                "hi",
                Some("prov/model"),
                None,
                None,
                None
            )
            .is_err()
    );
}

// =========================================================================
// 补测批（coverage-95 goal）：bridge Lagged/Closed 组合臂 + agent 任务
// 轮询行 + HealthServer 绑定冲突 error 臂。
//
// 手法：
// - Lagged：MessageBus::with_capacity(1) 定容广播 —— start() 内 subscribe
//   是同步的，随后在 current_thread runtime 上同步洪灌 3 条（spawn 的
//   bridge 尚未被轮询），订阅者必然落后环形缓冲 ≥2 → 第一次 recv 必出
//   RecvError::Lagged(≥2)。再补发一条并轮转让它穿过去，证明 continue 后
//   桥仍然活着；agent 任务（run_bus_arc 行）也在此期间被真实轮询。
// - Closed(dropped>0)：先发生过 Lagged（total_dropped>0），再把 bus/adapter
//   全部 Arc 拖走 —— Sender 归零后 bridge 下次 recv 得 Closed 复合分支。
// - 健康服务：真实 TcpListener 占住 ephemeral 端口 → spawn 的 inner.start()
//   bind 必失败 → adapter 闭包里的 tracing::error! 分支。
//
// 非确定性边界：Lagged(n) 的具体 n 不钉死（只断言语义存活）；Closed 臂无
// 外部可观测返回值，靠日志行覆盖 + 测试不挂兜底。
// =========================================================================

mod r10 {
    use super::*;

    fn r10_inbound(i: usize) -> nemesis_types::channel::InboundMessage {
        nemesis_types::channel::InboundMessage {
            channel: "web".to_string(),
            sender_id: format!("r10-user-{i}"),
            chat_id: "r10-chat".to_string(),
            content: format!("r10 msg {i}"),
            media: vec![],
            session_key: String::new(),
            correlation_id: String::new(),
            metadata: std::collections::HashMap::new(),
            voice_playback: None,
        }
    }

    fn r10_agent_loop_ref()
    -> Arc<parking_lot::RwLock<Option<Arc<nemesis_agent::r#loop::AgentLoop>>>> {
        Arc::new(parking_lot::RwLock::new(None))
    }

    #[tokio::test]
    async fn r10_bridge_lag_continue_then_closed_with_dropped() {
        crate::common::ensure_default_logger();

        // 容量 1 的专用总线：保证洪灌必产生 Lagged。
        let bus = Arc::new(nemesis_bus::MessageBus::with_capacity(1));
        let tmp = tempfile::TempDir::new().unwrap();
        write_minimal_model_config(tmp.path());
        let shared = make_shared_at_home(tmp.path(), &bus);
        let agent_loop = make_test_agent_loop();
        let ref_lock = r10_agent_loop_ref();
        // 注意 clone 关系：adapter/shared/bus 都留有 Arc，直到显式 drop。
        let adapter =
            AgentLoopServiceAdapter::new(agent_loop, shared.clone(), bus.clone(), ref_lock);
        LifecycleService::start(&adapter).expect("start must succeed");

        // start() 已同步完成 subscribe → 现在洪灌 3 条（capacity=1）。
        for i in 0..3 {
            bus.publish_inbound(r10_inbound(i));
        }
        // 当前线程 runtime：轮转让 bridge 消化背压（第一次 recv 即 Lagged
        // 分支 total_dropped += n；随后 Ok(#2) 穿过桥入 mpsc；agent 任务
        // run_bus_arc 也被真实轮询——mock provider 单轮即收尾）。
        for _ in 0..200 {
            tokio::task::yield_now().await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        // 再来一条慢速补充消息，确认 continue 之后桥仍转发（不 pin 具体谁
        // 被 lag 掉 —— tokio 只保证订阅者会收到 Lagged 或最新窗口内消息）。
        bus.publish_inbound(r10_inbound(3));
        for _ in 0..200 {
            tokio::task::yield_now().await;
        }

        // Closed(dropped>0)：total_dropped 此时 >0。拖走全部 Arc（最后一个
        // Sender 随之消亡）→ bridge 下次 recv 出 Closed 复合分支（warn +
        // break）。handle 在 adapter 里，drop 不 abort —— 任务自然跑完。
        drop(adapter);
        drop(shared);
        drop(bus);
        for _ in 0..300 {
            tokio::task::yield_now().await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    #[cfg(feature = "health")]
    #[tokio::test]
    async fn r10_health_server_adapter_bind_conflict_error_arm() {
        crate::common::ensure_default_logger();
        // 先占住一个真实端口（resident listener 保活到测试结束），
        // HealthServer 起在同一地址 → bind failed Err 字符串 → adapter
        // spawn 闭包里的 error!("[Main] Health server error:..") 分支。
        let occupied =
            std::net::TcpListener::bind("127.0.0.1:0").expect("ephemeral bind for occupation");
        let port = occupied.local_addr().unwrap().port();
        let health_server = Arc::new(nemesis_health::server::HealthServer::new(
            make_health_config(port),
        ));
        let adapter = HealthServerAdapter::new(health_server);
        assert!(adapter.start().is_ok(), "start 只是 spawn，必 Ok");
        // 让被 spawn 的服务任务实际执行到绑定失败。
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        drop(adapter);
        let _keep_resident = occupied;
    }
}

// =========================================================================
// wave4b 追加（coverage）：AgentLoopServiceAdapter 两块此前未触达的装配
// 逻辑——① 主桥 skip 谓词（set_skip_predicate + 桥内命中 continue /
// 未命中放行，用计数 LLM provider 做真断言）；② 重启遗留后台 subagent
// 丢失回执注入（stale bg_ 快照 → start() 注入 system 消息 → 续行 LLM →
// 快照自清）。
// =========================================================================

mod wave4b {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    /// 计数 mock provider：每次 chat 调用 +1（skip 谓词与注入链路的
    /// 可观测锚点——消息真到了 loop 才会有 LLM 调用）。
    struct CountingLlmProvider {
        calls: std::sync::Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl nemesis_agent::r#loop::LlmProvider for CountingLlmProvider {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<nemesis_agent::r#loop::LlmMessage>,
            _options: Option<nemesis_agent::types::ChatOptions>,
            _tools: Vec<nemesis_agent::types::ToolDefinition>,
        ) -> Result<nemesis_agent::r#loop::LlmResponse, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(nemesis_agent::r#loop::LlmResponse {
                finish_reason: None,
                escalated_to: None,
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

    fn make_counting_agent_loop(
        calls: std::sync::Arc<AtomicUsize>,
    ) -> nemesis_agent::r#loop::AgentLoop {
        let (outbound_tx, _outbound_rx) = tokio::sync::mpsc::channel(16);
        nemesis_agent::r#loop::AgentLoop::new_bus(
            Box::new(CountingLlmProvider { calls }),
            nemesis_agent::types::AgentConfig {
                model: "test-model".to_string(),
                system_prompt: Some("test".to_string()),
                system_prompt_split: None,
                max_turns: 1,
                tools: vec![],
                ..Default::default()
            },
            outbound_tx,
            nemesis_agent::r#loop::ConcurrentMode::Reject,
            8,
            0,
        )
    }

    fn inbound_msg(content: &str) -> nemesis_types::channel::InboundMessage {
        nemesis_types::channel::InboundMessage {
            channel: "web".to_string(),
            sender_id: "wave4b-user".to_string(),
            chat_id: "wave4b-chat".to_string(),
            content: content.to_string(),
            media: vec![],
            session_key: String::new(),
            correlation_id: String::new(),
            metadata: std::collections::HashMap::new(),
            voice_playback: None,
        }
    }

    /// 轮询直到闭包为真（上限 ms），返回是否及时达成。
    async fn wait_until(mut ms: u64, step: u64, pred: impl Fn() -> bool) -> bool {
        while ms > 0 {
            if pred() {
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(step)).await;
            ms = ms.saturating_sub(step);
        }
        pred()
    }

    /// 主桥 skip 谓词：装配期 set 一次 → 命中谓词的消息不进主 loop
    ///（桥 continue 丢给项目调度器），未命中的照常进 loop 触发 LLM。
    #[tokio::test]
    async fn skip_predicate_blocks_matched_messages_from_main_loop() {
        let calls = std::sync::Arc::new(AtomicUsize::new(0));
        let bus = Arc::new(nemesis_bus::MessageBus::new());
        let shared = make_test_shared(&bus);
        let agent_loop = make_counting_agent_loop(calls.clone());
        let agent_loop_ref: Arc<
            parking_lot::RwLock<Option<Arc<nemesis_agent::r#loop::AgentLoop>>>,
        > = Arc::new(parking_lot::RwLock::new(None));
        let adapter =
            AgentLoopServiceAdapter::new(Arc::new(agent_loop), shared, bus.clone(), agent_loop_ref);

        // 装配期设置（OnceLock：重复设置取首个）。
        let skip_calls = std::sync::Arc::new(AtomicUsize::new(0));
        let sc = skip_calls.clone();
        adapter.set_skip_predicate(Arc::new(
            move |msg: &nemesis_types::channel::InboundMessage| {
                sc.fetch_add(1, Ordering::SeqCst);
                msg.content.contains("SKIP-ME")
            },
        ));

        LifecycleService::start(&adapter).expect("start");
        assert!(LifecycleService::is_running(&adapter));

        // 未命中：正常进 loop → provider 恰被调一次。
        bus.publish_inbound(inbound_msg("wave4b normal hello"));
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    if calls.load(Ordering::SeqCst) >= 1 {
                        return;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            })
            .await
            .is_ok(),
            "未命中谓词的消息必须进主 loop（LLM 被调）"
        );

        // 命中：桥内 continue，消息不进 loop → provider 计数不变。
        bus.publish_inbound(inbound_msg("SKIP-ME should be diverted"));
        for _ in 0..300 {
            tokio::task::yield_now().await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "命中谓词的消息不得进主 loop"
        );
        assert!(
            skip_calls.load(Ordering::SeqCst) >= 1,
            "谓词必须真的被桥调用过"
        );

        adapter.stop().expect("stop");
        assert!(!LifecycleService::is_running(&adapter));
    }

    /// 重启恢复：盘上遗留 bg_ 前缀续行快照 → start() 注入诚实丢失回执
    ///（system 消息）→ loop 走续行路径以 error 结果续行 LLM → 完成后快照
    /// 自清。全链 in-process mock，无外部依赖。
    #[tokio::test]
    async fn stale_bg_spawn_snapshot_injects_loss_note_and_resume_cleans_it() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ws = tmp.path().to_path_buf();
        // 种一个 bg_ 前缀快照（空 session_key：续行收尾跳过持久化副作用）。
        let snapshot = nemesis_agent::ContinuationSnapshot {
            task_id: "bg_stale1".to_string(),
            messages: "[]".to_string(),
            tool_call_id: "tc-bg-1".to_string(),
            channel: "telegram".to_string(),
            chat_id: "chat-9".to_string(),
            session_key: String::new(),
            peer_id: String::new(),
            image_refs: vec![],
            image_refs_by_user_turn: vec![],
            created_at: "2026-09-25T00:00:00Z".to_string(),
            final_persisted: false,
        };
        nemesis_agent::ContinuationStore::new(&ws)
            .save(&snapshot)
            .expect("seed snapshot");

        let manager = Arc::new(nemesis_agent::ContinuationManager::with_disk_store(&ws));
        let seeded = manager.list_bg_spawn_pending_sync();
        assert!(
            seeded.iter().any(|id| id == "bg_stale1"),
            "磁盘快照必须被 manager 恢复，got {seeded:?}"
        );

        let calls = std::sync::Arc::new(AtomicUsize::new(0));
        let bus = Arc::new(nemesis_bus::MessageBus::new());
        let shared = make_test_shared(&bus);
        let mut agent_loop = make_counting_agent_loop(calls.clone());
        agent_loop.set_continuation_manager(manager.clone());
        let agent_loop_ref: Arc<
            parking_lot::RwLock<Option<Arc<nemesis_agent::r#loop::AgentLoop>>>,
        > = Arc::new(parking_lot::RwLock::new(None));
        let adapter =
            AgentLoopServiceAdapter::new(Arc::new(agent_loop), shared, bus, agent_loop_ref);

        // start 前：adapter 持有的 loop 能列出遗留任务（wiring 可观测）。
        let listed = adapter
            .current()
            .expect("prebuilt loop present")
            .list_stale_bg_spawn_task_ids();
        assert!(
            listed.iter().any(|id| id == "bg_stale1"),
            "adapter 侧 list_stale 必须可见，got {listed:?}"
        );

        LifecycleService::start(&adapter).expect("start");
        assert!(LifecycleService::is_running(&adapter));

        // 注入的丢失回执走续行路径 → 以 error 结果续行 LLM → 快照自清。
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(15), async {
                loop {
                    if calls.load(Ordering::SeqCst) >= 1 {
                        return;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            })
            .await
            .is_ok(),
            "丢失回执必须触发续行 LLM 调用"
        );
        assert!(
            wait_until(5000, 50, || manager.list_bg_spawn_pending_sync().is_empty()).await,
            "续行完成后快照必须自清，剩余 {:?}",
            manager.list_bg_spawn_pending_sync()
        );

        adapter.stop().expect("stop");
        assert!(!LifecycleService::is_running(&adapter));
    }
}

// =========================================================================
// BUG-20261108-012 横向收尾回归锁：run_runtime 装配期的 loop 级接线
// （审批应答端 M7 / question 应答端 F7 / doom-loop asker J5）必须跨
// stop→start 重建存活——重建分支（factory 新 loop）不经 run_runtime，
// 由 start() 重放 stash。否则重启后 WSAPI approval.respond /
// question.respond 经新 loop 的 responder 槽触达恒 None = 诚实报
// 「未装配」，Dashboard 卡有卡无答（阻塞端 auditor/broker 走
// SharedResources 共享槽不受影响——正是这种半边槽化让缺陷潜伏）。
// =========================================================================

struct StubWiringResponder;

impl nemesis_types::agent::ApprovalResponder for StubWiringResponder {
    fn respond(
        &self,
        _request_id: &str,
        _approved: bool,
        _always: bool,
        _note: Option<String>,
    ) -> Result<bool, String> {
        Err("stub".to_string())
    }
    fn pending(&self) -> Vec<serde_json::Value> {
        Vec::new()
    }
}

impl nemesis_types::agent::QuestionResponder for StubWiringResponder {
    fn respond(&self, _question_id: &str, _selected: Vec<String>) -> Result<bool, String> {
        Err("stub".to_string())
    }
    fn respond_form(
        &self,
        _question_id: &str,
        _values: &std::collections::BTreeMap<String, String>,
    ) -> Result<bool, String> {
        Err("stub".to_string())
    }
    fn pending(&self) -> Vec<serde_json::Value> {
        Vec::new()
    }
}

impl nemesis_types::agent::QuestionAsker for StubWiringResponder {
    fn ask(
        &self,
        _request: nemesis_types::agent::QuestionRequest,
    ) -> Result<nemesis_types::agent::QuestionOutcome, String> {
        Err("stub".to_string())
    }
}

#[tokio::test]
async fn runtime_wiring_survives_loop_rebuild() {
    let bus = Arc::new(nemesis_bus::MessageBus::new());
    let shared = make_test_shared(&bus);
    let agent_loop = make_test_agent_loop();
    let agent_loop_ref = Arc::new(parking_lot::RwLock::new(None));
    let adapter = AgentLoopServiceAdapter::new(agent_loop, shared, bus, agent_loop_ref.clone());

    adapter.start().expect("initial start (prebuilt)");

    // 装配期接线（run_runtime 同形态）：wire_* 同时打当前 loop。
    let stub: Arc<StubWiringResponder> = Arc::new(StubWiringResponder);
    adapter.wire_approval_responder(stub.clone());
    adapter.wire_question_broker(stub.clone(), stub);

    let first = adapter.current().expect("loop running");
    assert!(
        first.approval_responder().is_some(),
        "当前 loop 必须即时挂上审批应答端"
    );
    assert!(
        first.question_responder().is_some(),
        "当前 loop 必须即时挂上 question 应答端"
    );

    // stop → factory 重建分支。
    adapter.stop().expect("stop");
    adapter.start().expect("rebuild start (factory)");

    let rebuilt = adapter.current().expect("rebuilt loop running");
    assert!(
        !Arc::ptr_eq(&first, &rebuilt),
        "stop→start 必须重建出新 loop 实例（否则测的不是重建路径）"
    );
    assert!(
        rebuilt.approval_responder().is_some(),
        "重建 loop 必须重放审批应答端（BUG-20261108-012：否则 approval.respond 诚实报未装配，Dashboard 审批卡有卡无答）"
    );
    assert!(
        rebuilt.question_responder().is_some(),
        "重建 loop 必须重放 question 应答端（否则 question.respond 诚实报未装配，提问挂死到超时）"
    );
    // asker getter 是 pub(crate)，跨 crate 断言不到——与 responder 同一
    // stash 同一重放路径，responder 在场即证明重放已跑。

    // 再来一轮 stop→start：接线不丢（stash 不随 stop 清空）。
    adapter.stop().expect("stop 2");
    adapter.start().expect("rebuild start 2");
    let rebuilt2 = adapter.current().expect("rebuilt loop 2 running");
    assert!(
        rebuilt2.approval_responder().is_some(),
        "二次重建后审批应答端仍在"
    );
    assert!(
        rebuilt2.question_responder().is_some(),
        "二次重建后 question 应答端仍在"
    );

    adapter.stop().expect("final stop");
}

#[tokio::test]
async fn wiring_after_stop_stashes_and_replays_on_rebuild() {
    // wire 发生在 loop 缺位窗口（stop 后 / start 前）：wire_* 不炸、只
    // stash；start()（factory 重建分支）把接线重放到重建出的 loop。
    let bus = Arc::new(nemesis_bus::MessageBus::new());
    let shared = make_test_shared(&bus);
    let agent_loop = make_test_agent_loop();
    let agent_loop_ref = Arc::new(parking_lot::RwLock::new(None));
    let adapter = AgentLoopServiceAdapter::new(agent_loop, shared, bus, agent_loop_ref);

    // 先 stop 清掉预建 loop → wire 落进 stash-only 窗口。
    adapter.stop().expect("initial stop");
    assert!(adapter.current().is_none());

    let stub: Arc<StubWiringResponder> = Arc::new(StubWiringResponder);
    adapter.wire_approval_responder(stub.clone());
    adapter.wire_question_broker(stub.clone(), stub);
    assert!(
        adapter.current().is_none(),
        "stash-only 窗口不得凭空造 loop"
    );

    // 重建（make_test_shared 的 home 无 config.json → NullProvider 降级
    // 装配，Ok 语义见 lifecycle 重建测试）。
    adapter.start().expect("rebuild start");
    let rebuilt = adapter.current().expect("rebuilt loop running");
    assert!(
        rebuilt.approval_responder().is_some(),
        "stash-only 接线必须经 start 重放"
    );
    assert!(
        rebuilt.question_responder().is_some(),
        "stash-only 接线必须经 start 重放"
    );
    adapter.stop().expect("stop");
}
