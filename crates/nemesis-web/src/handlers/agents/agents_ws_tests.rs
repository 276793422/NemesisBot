//! `agents.*` WSAPI 用例（P2）：未装配诚实报错 / 各命令臂经 bridge /
//! L1 `commands()` 清单。进程级 bridge 槽是共享态——全部用例持
//! [`AGENTS_BRIDGE_TEST_LOCK`] 串行（同 PROJECTS_BRIDGE 家族纪律）。
// 持锁跨 await 是刻意的测试串行化（槽位置换必须覆盖整个 handler await
// 窗口，否则并行用例互相置换 bridge 假绿）；本进程内无其他同锁取用者，
// 无死锁面。editor/skins/upload 测试同款豁免。
#![allow(clippy::await_holding_lock)]

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::RequestContext;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

/// 最小 RequestContext（同 projects_tests.rs make_ctx 形态；agents handler
/// 不消费 ctx 状态，全字段 None/空即可）。TempDir 当场 drop——只用字符串。
fn make_ctx() -> RequestContext {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: Some(ws.clone()),
        home: Some(ws.clone()),
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new("m".to_string())),
        model_base: Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: Arc::new(AtomicBool::new(false)),
        event_hub: Arc::new(EventHub::new()),
        running: Arc::new(AtomicBool::new(true)),
        session_manager: Arc::new(SessionManager::with_default_timeout()),
        inbound_tx: None,
        streaming_provider: None,
        ws_router: None,
        agent_service: None,
        data_store: None,
        memory_manager: None,
        forge: None,
        agent_loop: Arc::new(parking_lot::RwLock::new(None)),
        cluster: None,
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: None,
        #[cfg(feature = "workflow")]
        chat_secret_store: std::sync::Arc::new(
            nemesis_workflow::chat_secrets::ChatSecretStore::in_memory(),
        ),
        #[cfg(not(feature = "workflow"))]
        chat_secret_store: std::sync::Arc::new(()),
        #[cfg(feature = "workflow")]
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        #[cfg(not(feature = "workflow"))]
        webhook_rate_limiter: std::sync::Arc::new(()),
        internal_cmd_tx: None,
        estop: None,
        signature_verify: None,
        skills_install_gate: None,
        cron: None,
        board: None,
    });
    RequestContext {
        session_id: "s".to_string(),
        chat_id: "c".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

/// 假 bridge（录制调用 + 固定应答）——handler 只消费 trait 面，不依赖
/// nemesisbot（依赖方向不许反向）。
struct FakeBridge {
    calls: Mutex<Vec<String>>,
}

impl FakeBridge {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
        })
    }
    fn note(&self, c: &str) {
        self.calls.lock().unwrap().push(c.to_string());
    }
}

fn fake_info(agent_id: &str, kind: &str) -> AgentInfo {
    AgentInfo {
        agent_id: agent_id.to_string(),
        name: "worker".to_string(),
        kind: kind.to_string(),
        status: "active".to_string(),
        exec_root: "/ws/agents/x/home".to_string(),
        home_path: "/ws/agents/x/home".to_string(),
        project_id: None,
        tier: "auto".to_string(),
        runtime_backend: "process".to_string(),
        memory_namespace: format!("agents/{agent_id}"),
        credential_count: 0,
        created_at: "2026-10-16T00:00:00Z".to_string(),
        last_active_at: "2026-10-16T00:00:00Z".to_string(),
    }
}

#[async_trait::async_trait]
impl AgentsBridge for FakeBridge {
    fn list(&self) -> Vec<AgentInfo> {
        self.note("list");
        vec![fake_info("a1", "main"), fake_info("a2", "selfbuilt")]
    }
    fn get(&self, agent_id: &str) -> Result<AgentInfo, String> {
        self.note("get");
        if agent_id == "a1" {
            Ok(fake_info(agent_id, "main"))
        } else {
            Err(format!("agent 不存在: {agent_id}"))
        }
    }
    fn create(&self, name: &str, tier: &str, runtime_backend: &str) -> Result<AgentInfo, String> {
        self.note(&format!("create:{tier}:{runtime_backend}"));
        if name.trim().is_empty() {
            return Err("agent name 不能为空".to_string());
        }
        let mut info = fake_info("a3", "selfbuilt");
        info.name = name.to_string();
        info.tier = tier.to_string();
        info.runtime_backend = if runtime_backend.is_empty() {
            "process".to_string()
        } else {
            runtime_backend.to_string()
        };
        Ok(info)
    }
    fn update_backend(&self, agent_id: &str, runtime_backend: &str) -> Result<AgentInfo, String> {
        self.note(&format!("update:{agent_id}->{runtime_backend}"));
        let mut info = self.get(agent_id)?;
        info.runtime_backend = runtime_backend.to_string();
        Ok(info)
    }
    async fn remove(&self, agent_id: &str) -> Result<(), String> {
        self.note("remove");
        if agent_id == "main" {
            return Err("main agent 是对话组身份投影，不可删除".to_string());
        }
        Ok(())
    }
    async fn sleep(&self, _agent_id: &str) -> Result<bool, String> {
        self.note("sleep");
        Ok(true)
    }
    async fn wake(&self, agent_id: &str) -> Result<(), String> {
        self.note("wake");
        if agent_id == "nope" {
            return Err(format!("agent 不存在: {agent_id}"));
        }
        Ok(())
    }
    fn slots(&self) -> Vec<AgentSlotInfo> {
        self.note("slots");
        vec![AgentSlotInfo {
            agent_id: "a1".to_string(),
            alive: true,
            idle_secs: 3,
        }]
    }
    fn persistent_enabled(&self) -> bool {
        true
    }
    fn loop_for_agent(
        &self,
        agent_id: &str,
    ) -> Result<Arc<nemesis_agent::r#loop::AgentLoop>, String> {
        self.note("loop_for_agent");
        // 真构建不进 web 测试——诚实报错形态即可（resolve_session_loop 的
        // agents 臂契约在 nemesisbot 侧 loop_manager 用例覆盖）。
        Err(format!("agent 不存在: {agent_id}"))
    }
    async fn send_to_agent(&self, agent_id: &str, msg: InboundMessage) -> Result<(), String> {
        self.note("send");
        if agent_id == "nope" {
            return Err(format!("agent 不存在: {agent_id}"));
        }
        assert!(
            msg.session_key.starts_with("agents:"),
            "session_key 必须 agents 形态"
        );
        Ok(())
    }
}

async fn call(
    cmd: &str,
    data: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    let ctx = make_ctx();
    let handler = AgentsHandler;
    handler.handle_cmd(cmd, data, &ctx).await
}

#[tokio::test]
async fn uninstalled_bridge_is_honest_error() {
    let _g = AGENTS_BRIDGE_TEST_LOCK.lock();
    set_agents_bridge_for_test(None);
    let err = call("list", None).await.expect_err("未装配必须诚实报错");
    assert!(err.contains("未装配"), "错误文案应指明未装配: {err}");
}

#[tokio::test]
async fn list_returns_agents_and_count() {
    let _g = AGENTS_BRIDGE_TEST_LOCK.lock();
    let bridge = FakeBridge::new();
    set_agents_bridge_for_test(Some(bridge.clone()));
    let out = call("list", None).await.unwrap().unwrap();
    assert_eq!(out["count"], 2);
    let agents = out["agents"].as_array().unwrap();
    assert_eq!(agents.len(), 2);
    assert_eq!(agents[0]["kind"], "main");
    assert_eq!(agents[1]["memory_namespace"], "agents/a2");
    assert_eq!(*bridge.calls.lock().unwrap(), vec!["list"]);
    set_agents_bridge_for_test(None);
}

#[tokio::test]
async fn get_missing_agent_propagates_bridge_error() {
    let _g = AGENTS_BRIDGE_TEST_LOCK.lock();
    set_agents_bridge_for_test(Some(FakeBridge::new()));
    // 缺 agent_id 参数 → 参数层报错。
    let err = call("get", Some(serde_json::json!({})))
        .await
        .expect_err("缺参必须报错");
    assert!(err.contains("agent_id"), "缺参文案应点名字段: {err}");
    // bridge 错误透传。
    let err = call("get", Some(serde_json::json!({"agent_id": "nope"})))
        .await
        .expect_err("不存在必须报错");
    assert!(err.contains("nope"));
    set_agents_bridge_for_test(None);
}

#[tokio::test]
async fn create_passes_name_and_default_tier() {
    let _g = AGENTS_BRIDGE_TEST_LOCK.lock();
    set_agents_bridge_for_test(Some(FakeBridge::new()));
    // tier 缺省 → 空串（bridge 侧按 auto 处理）。
    let out = call("create", Some(serde_json::json!({"name": "构建工"})))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["agent"]["name"], "构建工");
    assert_eq!(out["agent"]["tier"], "");
    // tier 显式透传。
    let out = call(
        "create",
        Some(serde_json::json!({"name": "w", "tier": "big"})),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["agent"]["tier"], "big");
    set_agents_bridge_for_test(None);
}

#[tokio::test]
async fn create_passes_runtime_backend_and_update_lands() {
    let _g = AGENTS_BRIDGE_TEST_LOCK.lock();
    set_agents_bridge_for_test(Some(FakeBridge::new()));
    // runtime_backend 缺省 → 空串透传（实现侧按 process 处理）。
    call("create", Some(serde_json::json!({"name": "w1"})))
        .await
        .unwrap()
        .unwrap();
    // 显式 vm 透传 + 响应投影带回档位。
    let out = call(
        "create",
        Some(serde_json::json!({"name": "w2", "runtime_backend": "vm"})),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["agent"]["runtime_backend"], "vm");
    // update 臂：参数透传 + 响应带切档后档案。
    let out = call(
        "update",
        Some(serde_json::json!({"agent_id": "a1", "runtime_backend": "vm"})),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["agent"]["agent_id"], "a1");
    assert_eq!(out["agent"]["runtime_backend"], "vm");
    assert!(
        out["note"].as_str().unwrap().contains("现读生效"),
        "切档注记应说明生效时机"
    );
    set_agents_bridge_for_test(None);
}

#[tokio::test]
async fn remove_main_is_rejected_by_bridge() {
    let _g = AGENTS_BRIDGE_TEST_LOCK.lock();
    set_agents_bridge_for_test(Some(FakeBridge::new()));
    let err = call("remove", Some(serde_json::json!({"agent_id": "main"})))
        .await
        .expect_err("main 不可删");
    assert!(err.contains("不可删除"));
    let out = call("remove", Some(serde_json::json!({"agent_id": "a2"})))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["removed"], "a2");
    set_agents_bridge_for_test(None);
}

#[test]
fn l1_commands_registry_lists_all_agents_commands() {
    let handler = AgentsHandler;
    assert_eq!(handler.module_name(), "agents");
    let cmds = handler.commands();
    assert_eq!(
        cmds,
        &[
            "list", "get", "create", "update", "remove", "sleep", "wake", "slots", "send"
        ]
    );
}

// -- G1 会话面（agents.send + 会话键形态）-----------------------------------

#[test]
fn agent_session_key_roundtrip_owner_parse() {
    let key = agent_session_key("abc123", "default");
    assert_eq!(key, "agents:abc123:session:default");
    assert_eq!(agent_owner_of(&key).as_deref(), Some("abc123"));
    // sid 消毒：非安全字符折叠为 '_'（同主会话纪律）。
    let key = agent_session_key("abc123", "a/b c");
    assert_eq!(key, "agents:abc123:session:a_b_c");
    assert_eq!(agent_owner_of(&key).as_deref(), Some("abc123"));
    // 非 agent 形态一律 None。
    assert_eq!(agent_owner_of("agent:main:session:default"), None);
    assert_eq!(agent_owner_of("proj:1:session:x"), None);
    assert_eq!(agent_owner_of("agents::session:x"), None, "空 id 不是归属");
    assert_eq!(agent_owner_of("agents:noid"), None);
}

#[tokio::test]
async fn send_routes_to_bridge_with_derived_session_key() {
    let _g = AGENTS_BRIDGE_TEST_LOCK.lock();
    set_agents_bridge_for_test(Some(FakeBridge::new()));
    // 成功臂：session_id 缺省 → default；用户行已入环（chat_event_log 在
    // 内存环——此处不断言环内容，fake 断言 session_key 形态即可）。
    let out = call(
        "send",
        Some(serde_json::json!({"agent_id": "a2", "content": "干活"})),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["accepted"], true);
    assert_eq!(out["session_id"], "default");
    // 显式 session_id 透传。
    let out = call(
        "send",
        Some(serde_json::json!({"agent_id": "a2", "content": "hi", "session_id": "task-7"})),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["session_id"], "task-7");
    // bridge 错误透传。
    let err = call(
        "send",
        Some(serde_json::json!({"agent_id": "nope", "content": "hi"})),
    )
    .await
    .expect_err("不存在必须报错");
    assert!(err.contains("nope"));
    // content 空白拒绝。
    let err = call(
        "send",
        Some(serde_json::json!({"agent_id": "a2", "content": "   "})),
    )
    .await
    .expect_err("空白 content 必须拒绝");
    assert!(err.contains("content"));
    set_agents_bridge_for_test(None);
}

// -- T9 生命周期面（sleep/wake/slots）--------------------------------------

#[tokio::test]
async fn sleep_reports_slept_and_note() {
    let _g = AGENTS_BRIDGE_TEST_LOCK.lock();
    set_agents_bridge_for_test(Some(FakeBridge::new()));
    let out = call("sleep", Some(serde_json::json!({"agent_id": "a2"})))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["agent_id"], "a2");
    assert_eq!(out["slept"], true);
    set_agents_bridge_for_test(None);
}

#[tokio::test]
async fn wake_propagates_bridge_error_and_success() {
    let _g = AGENTS_BRIDGE_TEST_LOCK.lock();
    set_agents_bridge_for_test(Some(FakeBridge::new()));
    let err = call("wake", Some(serde_json::json!({"agent_id": "nope"})))
        .await
        .expect_err("不存在必须报错");
    assert!(err.contains("nope"));
    let out = call("wake", Some(serde_json::json!({"agent_id": "a1"})))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["woken"], true);
    set_agents_bridge_for_test(None);
}

#[tokio::test]
async fn slots_reports_enabled_and_snapshot() {
    let _g = AGENTS_BRIDGE_TEST_LOCK.lock();
    set_agents_bridge_for_test(Some(FakeBridge::new()));
    let out = call("slots", None).await.unwrap().unwrap();
    assert_eq!(out["persistent_enabled"], true);
    let slots = out["slots"].as_array().unwrap();
    assert_eq!(slots.len(), 1);
    assert_eq!(slots[0]["agent_id"], "a1");
    assert_eq!(slots[0]["alive"], true);
    set_agents_bridge_for_test(None);
}
