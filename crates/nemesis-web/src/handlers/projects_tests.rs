//! L6++ G4（2026-09-08）— projects WSAPI 全命令臂 + resolve_session_loop
//! 两分支 + 项目根切换。
//!
//! bridge 槽是进程级共享态（`PROJECTS_BRIDGE`）——所有触碰它的测试持
//! [`BRIDGE_TEST_LOCK`](crate::handlers::projects::BRIDGE_TEST_LOCK) 串行
//! 并在结束时复位为 None（同 env-test-race 家族纪律）；mock bridge 对非
//! 哨兵 key 一律 None，杜绝与并行跑的其他 handler 测试串台。
//!
//! 装配 happy path（`bind_session` 真写 sidecar）依赖全局 path manager
//! 单例——本 crate 测试不重定向进程单例（mod.rs 既有豁免约定），端到端
//! 绑定覆盖在 nemesisbot manager 测试 + integration-test。

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::Arc;
use std::time::Instant;

use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::handlers::projects::{
    BRIDGE_TEST_LOCK, ProjectInfo, ProjectsBridge, ProjectsHandler, install_projects_bridge,
    project_root_for_session, resolve_session_loop, set_projects_bridge_for_test,
};
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_agent::r#loop::{AgentLoop, LlmMessage, LlmProvider, LlmResponse};
use nemesis_agent::types::{AgentConfig, ChatOptions, ToolDefinition};

// ---------------------------------------------------------------------------
// 夹具
// ---------------------------------------------------------------------------

fn make_ctx(main_loop: Option<Arc<AgentLoop>>) -> RequestContext {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    // TempDir 在此 drop——测试只用 ctx.workspace 做字符串，不落盘。
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
        agent_loop: Arc::new(parking_lot::RwLock::new(main_loop)),
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

/// 项目 loop 替身：`AgentLoop::new` 廉价形态（agent/loop/commands_tests.rs
/// 同款 NoopProvider——construct 即返回，不触碰 LLM/磁盘）。
struct NoopProvider;

#[async_trait::async_trait]
impl LlmProvider for NoopProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<ChatOptions>,
        _tools: Vec<ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Err("not used in projects routing tests".into())
    }
}

fn noop_agent_loop() -> Arc<AgentLoop> {
    Arc::new(AgentLoop::new(Box::new(NoopProvider), AgentConfig::default()))
}

/// 哨兵归属键（唯一，防与并行测试的实际会话撞键）。
const SENTINEL_KEY: &str = "agent:main:session:__proj_g4_sentinel__";
const SENTINEL_PID: &str = "p-g4mock01";
const SENTINEL_PATH: &str = "C:/mock/proj/dir";

struct MockBridge {
    inner: parking_lot::Mutex<MockState>,
    /// 项目 loop 替身（惰性构造，一次共享）。
    project_loop: std::sync::OnceLock<Arc<AgentLoop>>,
}

#[derive(Default)]
struct MockState {
    projects: Vec<ProjectInfo>,
    bound: HashMap<String, String>,
    /// 归属命中但 loop 不在（不可用形态）。
    missing_loops: HashSet<String>,
}

impl MockBridge {
    fn new() -> Self {
        Self {
            inner: parking_lot::Mutex::new(MockState {
                projects: vec![ProjectInfo {
                    id: SENTINEL_PID.to_string(),
                    name: "哨兵项目".to_string(),
                    path: SENTINEL_PATH.to_string(),
                    created_at: "2026-09-08T00:00:00+08:00".to_string(),
                    running: true,
                }],
                bound: HashMap::new(),
                missing_loops: HashSet::new(),
            }),
            project_loop: std::sync::OnceLock::new(),
        }
    }

    fn project_loop(&self) -> Arc<AgentLoop> {
        self.project_loop
            .get_or_init(noop_agent_loop)
            .clone()
    }
}

impl ProjectsBridge for MockBridge {
    fn list(&self) -> Vec<ProjectInfo> {
        self.inner.lock().projects.clone()
    }

    fn create(&self, name: &str, path: &str) -> Result<ProjectInfo, String> {
        // 错误传播臂：handler 必须原样回显 bridge 的诚实文案。
        if path.trim().is_empty() {
            return Err("项目路径不能为空".to_string());
        }
        let mut s = self.inner.lock();
        let info = ProjectInfo {
            id: format!("p-g4new{:02}", s.projects.len()),
            name: name.to_string(),
            path: path.to_string(),
            created_at: "2026-09-08T00:00:00+08:00".to_string(),
            running: true,
        };
        s.projects.push(info.clone());
        Ok(info)
    }

    fn remove(&self, project_id: &str) -> Result<ProjectInfo, String> {
        let mut s = self.inner.lock();
        let idx = s
            .projects
            .iter()
            .position(|p| p.id == project_id)
            .ok_or_else(|| format!("项目不存在: {project_id}"))?;
        let mut info = s.projects.remove(idx);
        info.running = false;
        Ok(info)
    }

    fn rename(&self, project_id: &str, new_name: &str) -> Result<ProjectInfo, String> {
        let mut s = self.inner.lock();
        let e = s
            .projects
            .iter_mut()
            .find(|p| p.id == project_id)
            .ok_or_else(|| format!("项目不存在: {project_id}"))?;
        e.name = new_name.to_string();
        Ok(e.clone())
    }

    fn owner_of(&self, session_key: &str) -> Option<String> {
        self.inner.lock().bound.get(session_key).cloned()
    }

    fn loop_for_session(&self, project_id: &str) -> Option<Arc<AgentLoop>> {
        let s = self.inner.lock();
        if s.missing_loops.contains(project_id) {
            None
        } else {
            drop(s);
            Some(self.project_loop())
        }
    }

    fn project_path(&self, project_id: &str) -> Option<std::path::PathBuf> {
        self.inner
            .lock()
            .projects
            .iter()
            .find(|p| p.id == project_id)
            .map(|p| std::path::PathBuf::from(&p.path))
    }

    fn bind_session(&self, session_key: &str, project_id: &str) -> Result<(), String> {
        let mut s = self.inner.lock();
        if !s.projects.iter().any(|p| p.id == project_id) {
            return Err(format!("项目不存在: {project_id}"));
        }
        s.bound.insert(session_key.to_string(), project_id.to_string());
        Ok(())
    }

    fn forget_session(&self, session_key: &str) {
        self.inner.lock().bound.remove(session_key);
    }
}

/// bridge 触碰测试的守卫：持锁 + 装 mock，drop 时复位 None 并放锁。
struct BridgeGuard {
    _lock: parking_lot::MutexGuard<'static, ()>,
}

impl BridgeGuard {
    fn with_mock() -> (Self, Arc<MockBridge>) {
        let lock = BRIDGE_TEST_LOCK.lock();
        let mock = Arc::new(MockBridge::new());
        install_projects_bridge(mock.clone());
        (Self { _lock: lock }, mock)
    }

    fn empty() -> Self {
        let lock = BRIDGE_TEST_LOCK.lock();
        set_projects_bridge_for_test(None);
        Self { _lock: lock }
    }
}

impl Drop for BridgeGuard {
    fn drop(&mut self) {
        set_projects_bridge_for_test(None);
    }
}

// ---------------------------------------------------------------------------
// projects.* 全命令臂
// ---------------------------------------------------------------------------

#[tokio::test]
async fn projects_cmds_without_bridge_are_honest_errors() {
    let _g = BridgeGuard::empty();
    let ctx = make_ctx(None);
    let h = ProjectsHandler;
    for cmd in ["list", "create", "remove", "rename", "open_dir"] {
        let data = if cmd == "list" {
            None
        } else {
            Some(serde_json::json!({}))
        };
        let err = h
            .handle_cmd(cmd, data, &ctx)
            .await
            .expect_err("must fail without bridge");
        assert!(
            err.contains("未装配"),
            "{cmd} must report honest not-wired error, got: {err}"
        );
    }
}

#[tokio::test]
async fn projects_list_create_remove_rename_route_through_bridge() {
    let (g, _mock) = BridgeGuard::with_mock();
    let ctx = make_ctx(None);
    let h = ProjectsHandler;

    // list：投影字段齐全。
    let out = h
        .handle_cmd("list", None, &ctx)
        .await
        .expect("list succeeds")
        .expect("list returns payload");
    assert_eq!(out["count"], 1);
    assert_eq!(out["projects"][0]["id"], SENTINEL_PID);
    assert_eq!(out["projects"][0]["running"], true);

    // create：成功臂 + 缺 path 诚实文案透传。
    let out = h
        .handle_cmd(
            "create",
            Some(serde_json::json!({"name":"新项目","path":"C:/x"})),
            &ctx,
        )
        .await
        .expect("create succeeds")
        .expect("create returns payload");
    let new_pid = out["project"]["id"].as_str().unwrap().to_string();
    assert!(new_pid.starts_with("p-g4new"));
    let err = h
        .handle_cmd(
            "create",
            Some(serde_json::json!({"name":"新项目","path":" "})),
            &ctx,
        )
        .await
        .expect_err("empty path must fail honestly");
    assert!(err.contains("路径不能为空"), "honest create error must pass through: {err}");

    // rename：成功臂。
    let out = h
        .handle_cmd(
            "rename",
            Some(serde_json::json!({"project_id": SENTINEL_PID, "name":"改名后"})),
            &ctx,
        )
        .await
        .expect("rename succeeds")
        .expect("rename returns payload");
    assert_eq!(out["project"]["name"], "改名后");

    // remove：成功臂（含诚实 note）+ 未知 id 臂。
    let out = h
        .handle_cmd(
            "remove",
            Some(serde_json::json!({"project_id": new_pid})),
            &ctx,
        )
        .await
        .expect("remove succeeds")
        .expect("remove returns payload");
    assert_eq!(out["removed"]["id"], new_pid);
    // 后端 note 是事后陈述（「未删除」）；「不删除」是前端确认弹窗的事前
    // 承诺文案（goal F-10，G5 面），两表面不同。
    assert!(out["note"].as_str().unwrap().contains("未删除"));
    let err = h
        .handle_cmd(
            "remove",
            Some(serde_json::json!({"project_id": "p-nope"})),
            &ctx,
        )
        .await
        .expect_err("unknown project must fail honestly");
    assert!(err.contains("不存在"), "honest remove error must pass through: {err}");
    drop(g);
}

// ---------------------------------------------------------------------------
// resolve_session_loop 两分支 + 不可用诚实报错
// ---------------------------------------------------------------------------

#[tokio::test]
async fn resolve_loop_bound_session_goes_to_project_loop() {
    let (g, mock) = BridgeGuard::with_mock();
    mock.inner
        .lock()
        .bound
        .insert(SENTINEL_KEY.to_string(), SENTINEL_PID.to_string());
    let main_loop = noop_agent_loop();
    let ctx = make_ctx(Some(main_loop.clone()));

    let resolved = resolve_session_loop(&ctx, SENTINEL_KEY).expect("project loop resolves");
    assert!(
        !Arc::ptr_eq(&resolved, &main_loop),
        "bound session must NOT resolve to the main loop"
    );
    assert!(
        Arc::ptr_eq(&resolved, &mock.project_loop()),
        "bound session must resolve to the project loop handle"
    );
    drop(g);
}

#[tokio::test]
async fn resolve_loop_unbound_session_falls_back_to_main() {
    let (g, _mock) = BridgeGuard::with_mock(); // bridge 在，但哨兵键无归属
    let main_loop = noop_agent_loop();
    let ctx = make_ctx(Some(main_loop.clone()));

    let resolved = resolve_session_loop(&ctx, "agent:main:session:unbound_g4")
        .expect("main loop resolves");
    assert!(
        Arc::ptr_eq(&resolved, &main_loop),
        "unbound session must resolve to the main loop"
    );
    drop(g);
}

#[tokio::test]
async fn resolve_loop_without_bridge_falls_back_to_main() {
    let _g = BridgeGuard::empty();
    let main_loop = noop_agent_loop();
    let ctx = make_ctx(Some(main_loop.clone()));
    let resolved = resolve_session_loop(&ctx, SENTINEL_KEY).expect("main loop resolves");
    assert!(Arc::ptr_eq(&resolved, &main_loop));
}

#[tokio::test]
async fn resolve_loop_bound_but_unavailable_is_honest_error() {
    let (g, mock) = BridgeGuard::with_mock();
    {
        let mut s = mock.inner.lock();
        s.bound.insert(SENTINEL_KEY.to_string(), SENTINEL_PID.to_string());
        s.missing_loops.insert(SENTINEL_PID.to_string());
    }
    let ctx = make_ctx(None);
    let err = resolve_session_loop(&ctx, SENTINEL_KEY)
        // expect_err 要求 Ok 侧 Debug（Arc<AgentLoop> 未实现）——保持 .err().expect()
        .err()
        .expect("bound-but-missing must fail honestly");
    assert!(
        err.contains("不可用") && err.contains("哨兵项目"),
        "error must name the project honestly: {err}"
    );
    drop(g);
}

// ---------------------------------------------------------------------------
// 项目根切换（fs.tree / chat.todo_get 消费）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn project_root_switches_only_for_bound_sessions() {
    let (g, mock) = BridgeGuard::with_mock();
    assert_eq!(project_root_for_session(SENTINEL_KEY), None, "未绑定 = None");
    mock.inner
        .lock()
        .bound
        .insert(SENTINEL_KEY.to_string(), SENTINEL_PID.to_string());
    assert_eq!(
        project_root_for_session(SENTINEL_KEY).as_deref(),
        Some(SENTINEL_PATH),
        "绑定命中 → 项目目录"
    );
    drop(g);
}

// ---------------------------------------------------------------------------
// sessions.create 带 project_id：未装配 / 未知项目诚实报错
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sessions_create_with_project_id_without_bridge_is_honest_error() {
    let _g = BridgeGuard::empty();
    let ctx = make_ctx(None);
    let h = super::sessions::SessionsHandler;
    let err = h
        .handle_cmd(
            "create",
            Some(serde_json::json!({"project_id": SENTINEL_PID})),
            &ctx,
        )
        .await
        .expect_err("create with project_id must fail without bridge");
    assert!(
        err.contains("未装配"),
        "create with project_id must fail honestly without bridge: {err}"
    );
}

#[tokio::test]
async fn sessions_create_with_project_id_unknown_project_is_honest_error() {
    let (g, _mock) = BridgeGuard::with_mock(); // bridge 在，但 p-nope 未注册
    let ctx = make_ctx(None);
    let h = super::sessions::SessionsHandler;
    let err = h
        .handle_cmd(
            "create",
            Some(serde_json::json!({"project_id": "p-nope"})),
            &ctx,
        )
        .await
        .expect_err("unknown project must fail honestly");
    assert!(err.contains("不存在"), "unknown project must fail honestly: {err}");
    drop(g);
}

// ---------------------------------------------------------------------------
// open_dir：只开注册表已知项目目录（未注册/目录消失诚实报错）。
// 真正 spawn explorer 的成功臂不做单测（副作用=弹窗口）；单测钉目标解析
// 纯函数 + handler 错误透传（解析失败时根本不会走到 spawn）。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn open_dir_unknown_project_is_honest_error_without_spawn() {
    let (g, _mock) = BridgeGuard::with_mock();
    let ctx = make_ctx(None);
    let h = ProjectsHandler;
    let err = h
        .handle_cmd(
            "open_dir",
            Some(serde_json::json!({"project_id": "p-nope"})),
            &ctx,
        )
        .await
        .expect_err("unknown project must fail honestly");
    assert!(err.contains("不存在"), "honest error must pass through: {err}");
    drop(g);
}

#[tokio::test]
async fn open_dir_missing_directory_is_honest_error() {
    let (g, _mock) = BridgeGuard::with_mock(); // 哨兵项目路径 C:/mock/proj/dir 不存在
    let ctx = make_ctx(None);
    let h = ProjectsHandler;
    let err = h
        .handle_cmd(
            "open_dir",
            Some(serde_json::json!({"project_id": SENTINEL_PID})),
            &ctx,
        )
        .await
        .expect_err("missing directory must fail honestly");
    assert!(
        err.contains("不存在") && err.contains("哨兵项目"),
        "error must name the project honestly: {err}"
    );
    drop(g);
}

#[tokio::test]
async fn open_dir_resolves_only_registered_paths() {
    let (g, mock) = BridgeGuard::with_mock();
    // 注册一个真实存在的临时目录 → 解析成功且原样返回。
    let dir = tempfile::tempdir().unwrap();
    let created = mock.create("tmp-项目", &dir.path().to_string_lossy()).unwrap();
    let target = super::projects::resolve_open_target(mock.as_ref(), &created.id)
        .expect("registered existing dir must resolve");
    assert_eq!(target, dir.path());

    // 未注册 id → Err（防任意路径打开的闸门）。
    let err = super::projects::resolve_open_target(mock.as_ref(), "p-arbitrary")
        .expect_err("unregistered id must be rejected");
    assert!(err.contains("不存在"), "{err}");
    drop(g);
}

// ---------------------------------------------------------------------------
// G6（2026-09-08）：rewind/redo/file_diff 路由项目会话——三命令的 undo 栈
// / checkpoint store 住 loop 实例，项目会话必须由项目 loop 执行。回归锚：
// 曾硬编码主槽，项目会话 rewind 拿主 loop 的空 checkpoint 索引 → 文件恢复
// 静默 no-op（applied + 空 restored_files + 文件未回滚）。
// ---------------------------------------------------------------------------

/// 归属命中但项目 loop 不可用 → 三命令都必须诚实报「项目不可用」，
/// 绝不能落回主 loop（硬编码时代的行为）。
#[tokio::test]
async fn sessions_rewind_redo_filediff_bound_session_route_to_project_loop() {
    let (g, mock) = BridgeGuard::with_mock();
    {
        let mut s = mock.inner.lock();
        s.bound.insert(SENTINEL_KEY.to_string(), SENTINEL_PID.to_string());
        s.missing_loops.insert(SENTINEL_PID.to_string());
    }
    // 主槽在场——若命令仍走主槽，错误会是「未装配」而非「项目不可用」。
    let ctx = make_ctx(Some(noop_agent_loop()));
    let h = super::sessions::SessionsHandler;
    let sid = SENTINEL_KEY.strip_prefix("agent:main:session:").unwrap();
    let cases: Vec<(&str, serde_json::Value)> = vec![
        ("rewind_to_message", serde_json::json!({"session_id": sid, "message_index": 0})),
        ("redo", serde_json::json!({"session_id": sid})),
        ("file_diff", serde_json::json!({"session_id": sid, "path": "x.txt"})),
    ];
    for (cmd, data) in cases {
        let err = h
            .handle_cmd(cmd, Some(data), &ctx)
            .await
            .err()
            .unwrap_or_else(|| panic!("{cmd} must fail for unavailable project loop"));
        assert!(
            err.contains("不可用") && err.contains("哨兵项目"),
            "{cmd} must route bound sessions to the project loop (honest unavailable error), got: {err}"
        );
    }
    drop(g);
}

/// 行为保持钉：无归属会话的 rewind 仍走主 loop（noop loop 无该会话
/// jsonl → 「没有可回退的消息」，而非项目路由错误）。
#[tokio::test]
async fn sessions_rewind_unbound_session_keeps_main_loop() {
    let (g, _mock) = BridgeGuard::with_mock(); // bridge 在，哨兵键无归属
    let ctx = make_ctx(Some(noop_agent_loop()));
    let h = super::sessions::SessionsHandler;
    let err = h
        .handle_cmd(
            "rewind_to_message",
            Some(serde_json::json!({"session_id": "unbound_g6", "message_index": 0})),
            &ctx,
        )
        .await
        .expect_err("rewind on empty session must fail");
    assert!(
        err.contains("没有可回退"),
        "unbound session rewind must go through the main loop, got: {err}"
    );
    drop(g);
}
