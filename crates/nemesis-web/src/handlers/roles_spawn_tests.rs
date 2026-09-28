//! 角色目录与客户端委派（2026-09-28）：`roles.list` + `chat.spawn` WSAPI
//! 测试——目录表面形状（17 项 + tier + 可见性字段）、参数校验诚实回灌、
//! 未知角色拒绝、spawn 槽未注入的诚实报错、槽注入后的同源派发（捕获
//! SpawnFn 实参：agent_id="client" / channel="web" / depth=1 / role 透传）。
//!
//! 分档降级（Mini 拒 coordinator）与 hidden 配置热生效在 agent 侧
//! `role_catalog_tests.rs` 已钉死——这里只测 handler 自身逻辑与接线。

use super::chat::ChatHandler;
use super::roles::RolesHandler;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

// ---------------------------------------------------------------------------
// Harness（与 chat_mode_tests 同款）
// ---------------------------------------------------------------------------

struct NoopProvider;

#[async_trait::async_trait]
impl nemesis_agent::r#loop::LlmProvider for NoopProvider {
    async fn chat(
        &self,
        _: &str,
        _: Vec<nemesis_agent::r#loop::LlmMessage>,
        _: Option<nemesis_agent::types::ChatOptions>,
        _: Vec<nemesis_agent::types::ToolDefinition>,
    ) -> Result<nemesis_agent::r#loop::LlmResponse, String> {
        Ok(nemesis_agent::r#loop::LlmResponse {
            content: String::new(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

fn make_ctx(
    dir: &tempfile::TempDir,
    agent_loop: Option<Arc<nemesis_agent::r#loop::AgentLoop>>,
) -> RequestContext {
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
        agent_loop: Arc::new(parking_lot::RwLock::new(agent_loop)),
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
        webhook_rate_limiter: Arc::new(()),
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

fn role_agent_loop() -> Arc<nemesis_agent::r#loop::AgentLoop> {
    Arc::new(nemesis_agent::r#loop::AgentLoop::new(
        Box::new(NoopProvider),
        nemesis_agent::types::AgentConfig::default(),
    ))
}

/// SpawnFn 实参捕获记录：(agent_id, task, model, channel, chat_id,
/// tools_profile, depth, background, role)。
type CapturedSpawn = Arc<
    parking_lot::Mutex<
        Vec<(String, String, String, String, String, String, usize, bool, String)>,
    >,
>;

fn recording_spawn(
    captured: CapturedSpawn,
    reply: &'static str,
) -> nemesis_agent::loop_tools::SpawnFn {
    Arc::new(
        move |agent_id: &str,
              task: &str,
              model: &str,
              channel: &str,
              chat_id: &str,
              tools_profile: &str,
              depth: usize,
              background: bool,
              role: &str|
              -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'static>,
        > {
            let captured = captured.clone();
            // 先物化 owned String——async move 会把 &str 引用按值捕获，
            // 引用进块 = future 非 'static（SpawnFn 签名要求 + Send + 'static）。
            let agent_id = agent_id.to_string();
            let task = task.to_string();
            let model = model.to_string();
            let channel = channel.to_string();
            let chat_id = chat_id.to_string();
            let tools_profile = tools_profile.to_string();
            let role = role.to_string();
            Box::pin(async move {
                captured.lock().push((
                    agent_id,
                    task,
                    model,
                    channel,
                    chat_id,
                    tools_profile,
                    depth,
                    background,
                    role,
                ));
                Ok(reply.to_string())
            })
        },
    )
}

// ---------------------------------------------------------------------------
// roles.list
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_requires_agent_loop() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, None);
    let err = RolesHandler
        .handle_cmd("list", None, &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("agent loop not running"));
}

#[tokio::test]
async fn list_returns_full_catalog_with_visibility_fields() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, Some(role_agent_loop()));
    let out = RolesHandler
        .handle_cmd("list", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    // 默认 tier = Big → 全目录 17 项可见（fail-open 与 dispatch 闸同源）。
    assert_eq!(out["tier"], "big");
    let roles = out["roles"].as_array().unwrap();
    assert_eq!(roles.len(), 17, "目录 17 项（10 既有 + 7 新增）");
    for r in roles {
        assert!(!r["slug"].as_str().unwrap().is_empty());
        assert!(!r["description"].as_str().unwrap().is_empty());
        assert!(!r["min_tier"].as_str().unwrap().is_empty());
        assert!(r["visible"].is_boolean());
        assert!(r["hidden"].is_boolean());
    }
    let visible = roles
        .iter()
        .filter(|r| r["visible"].as_bool().unwrap())
        .count();
    assert_eq!(out["visible_count"], visible);
    assert_eq!(visible, 17, "big 档全可见");
    let slugs: Vec<&str> = roles
        .iter()
        .map(|r| r["slug"].as_str().unwrap())
        .collect();
    for expected in ["explorer", "qa", "coordinator", "web_reader", "test_runner", "fork"] {
        assert!(
            slugs.contains(&expected),
            "目录必须含 {expected}，实际: {slugs:?}"
        );
    }
    // 默认态（无 config.json）没有任何 hidden 配置项。
    assert!(roles.iter().all(|r| !r["hidden"].as_bool().unwrap()));
}

// ---------------------------------------------------------------------------
// chat.spawn 参数校验
// ---------------------------------------------------------------------------

#[tokio::test]
async fn spawn_requires_agent_loop() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, None);
    let err = ChatHandler
        .handle_cmd(
            "spawn",
            Some(serde_json::json!({ "session_id": "s1", "task": "t", "role": "qa" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("agent loop not running"));
}

#[tokio::test]
async fn spawn_rejects_missing_session_and_task() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, Some(role_agent_loop()));
    // 缺 session_id（handle_cmd 顶部的通用提取先拦）。
    let err = ChatHandler
        .handle_cmd("spawn", Some(serde_json::json!({ "task": "t" })), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("missing session_id"), "实际: {err}");
    // 缺 task / 空白 task。
    for task in [None, Some(""), Some("   ")] {
        let mut d = serde_json::json!({ "session_id": "s1" });
        if let Some(t) = task {
            d["task"] = serde_json::Value::String(t.to_string());
        }
        let err = ChatHandler
            .handle_cmd("spawn", Some(d), &ctx)
            .await
            .unwrap_err();
        assert!(err.contains("missing task"), "task={task:?} 实际: {err}");
    }
}

#[tokio::test]
async fn spawn_rejects_invalid_tools_profile() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, Some(role_agent_loop()));
    let err = ChatHandler
        .handle_cmd(
            "spawn",
            Some(serde_json::json!({
                "session_id": "s1", "task": "t", "role": "qa", "tools_profile": "yolo"
            })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(
        err.contains("invalid tools_profile") && err.contains("yolo"),
        "实际: {err}"
    );
}

#[tokio::test]
async fn spawn_rejects_unknown_role() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, Some(role_agent_loop()));
    let err = ChatHandler
        .handle_cmd(
            "spawn",
            Some(serde_json::json!({
                "session_id": "s1", "task": "t", "role": "nope"
            })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(
        err.contains("Unknown role 'nope'") && err.contains("Valid roles:"),
        "未知角色必须回灌合法清单，实际: {err}"
    );
}

// ---------------------------------------------------------------------------
// chat.spawn 槽接线
// ---------------------------------------------------------------------------

#[tokio::test]
async fn spawn_reports_missing_slot_honestly() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, Some(role_agent_loop()));
    // 角色 qa 合法（mini 档，big 下可见），但槽未注入 → 诚实报错不假成功。
    let err = ChatHandler
        .handle_cmd(
            "spawn",
            Some(serde_json::json!({
                "session_id": "s1", "task": "审查这个 diff", "role": "qa"
            })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(
        err.contains("sub-agent spawning is not available"),
        "实际: {err}"
    );
}

#[tokio::test]
async fn spawn_happy_path_invokes_slot_fn_with_client_identity() {
    let dir = tempfile::tempdir().unwrap();
    let al = role_agent_loop();
    let captured: CapturedSpawn = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let slot = Arc::new(std::sync::OnceLock::new());
    slot.set(recording_spawn(captured.clone(), "子代理完成：结论 OK"))
        .ok()
        .unwrap();
    al.set_spawn_slot(slot);
    let ctx = make_ctx(&dir, Some(al));

    // tools_profile 缺省（空）→ 响应归一为 readonly。
    let out = ChatHandler
        .handle_cmd(
            "spawn",
            Some(serde_json::json!({
                "session_id": "s1", "task": "跑一遍单元测试", "role": "qa"
            })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["session_key"], "agent:main:session:s1");
    assert_eq!(out["role"], "qa");
    assert_eq!(out["tools_profile"], "readonly");
    assert_eq!(out["result"], "子代理完成：结论 OK");

    let calls = captured.lock();
    assert_eq!(calls.len(), 1);
    let (agent_id, task, model, channel, chat_id, tools_profile, depth, background, role) =
        &calls[0];
    assert_eq!(agent_id, "client");
    assert_eq!(task, "跑一遍单元测试");
    assert_eq!(model, "", "model 空 = detached 沿用主模型");
    assert_eq!(channel, "web");
    assert_eq!(chat_id, "", "detached 不经通道路由");
    assert_eq!(tools_profile, "");
    assert_eq!(*depth, 1, "深度 = 直接子代理");
    assert!(!background, "客户端委派前台等待最终回复");
    assert_eq!(role, "qa");
}

#[tokio::test]
async fn spawn_full_profile_passthrough() {
    let dir = tempfile::tempdir().unwrap();
    let al = role_agent_loop();
    let captured: CapturedSpawn = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let slot = Arc::new(std::sync::OnceLock::new());
    slot.set(recording_spawn(captured.clone(), "done"))
        .ok()
        .unwrap();
    al.set_spawn_slot(slot);
    let ctx = make_ctx(&dir, Some(al));

    let out = ChatHandler
        .handle_cmd(
            "spawn",
            Some(serde_json::json!({
                "session_id": "s1", "task": "t", "role": "qa", "tools_profile": "full"
            })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["tools_profile"], "full");
    assert_eq!(captured.lock()[0].5, "full");
}

#[tokio::test]
async fn spawn_empty_role_uses_tier_derivation() {
    let dir = tempfile::tempdir().unwrap();
    let al = role_agent_loop();
    let captured: CapturedSpawn = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let slot = Arc::new(std::sync::OnceLock::new());
    slot.set(recording_spawn(captured.clone(), "done"))
        .ok()
        .unwrap();
    al.set_spawn_slot(slot);
    let ctx = make_ctx(&dir, Some(al));

    // 空 role = 缺省（档位推导），check_client_role 必须放行。
    let out = ChatHandler
        .handle_cmd(
            "spawn",
            Some(serde_json::json!({ "session_id": "s1", "task": "t" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["role"], "");
    assert_eq!(captured.lock()[0].8, "");
}

#[tokio::test]
async fn spawn_slot_unset_once_lock_reports_honestly() {
    let dir = tempfile::tempdir().unwrap();
    let al = role_agent_loop();
    // 槽镜像注入了，但 factory 侧闭包从未 set 进 OnceLock → 同样诚实报错
    //（与生产装配次序无关的防御臂）。
    al.set_spawn_slot(Arc::new(std::sync::OnceLock::new()));
    let ctx = make_ctx(&dir, Some(al));
    let err = ChatHandler
        .handle_cmd(
            "spawn",
            Some(serde_json::json!({ "session_id": "s1", "task": "t", "role": "qa" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("sub-agent spawning is not available"));
}
