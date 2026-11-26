//! loops handler 单测（`loop` feature 门控）：
//! - list：空集 / 回读摘要面；
//! - create：正常建（session_id sanitize 绑定）/ 会话不在场拒绝 /
//!   goal 缺失拒绝；
//! - get：详情面（progress_log 留痕）/ 不存在诚实错；
//! - pause/resume：状态机（仅 active 可暂停 / 仅 paused+parked 可恢复 /
//!   留痕 progress）；
//! - update：budget 按字段合并（部分更新保留未提及维 / 显式 0 = 设不限 /
//!   停车场解除前置）/ 无字段拒绝；
//! - remove：删除 + 不存在拒绝；
//! - unknown command + commands() 清单钉死。
//!
//! home 单例经 sessions_trash_tests 同款 HomeGuard 重定向到 tempdir
//! （test_home::lock_home 可重入锁跨测试串行）——create 的
//! session_occupied 校验走 PathManager 单例寻址，重定向后 meta 落
//! tempdir，零真实目录触碰。

use super::LoopsHandler;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

struct HomeGuard {
    old: PathBuf,
    _lock: parking_lot::ReentrantMutexGuard<'static, ()>,
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        nemesis_path::default_path_manager().set_home_dir(self.old.clone());
    }
}

/// 重定向单例 home（workspace = `<home>/workspace`，与单例派生规则一致）
/// 并为 `session_key` 造 meta sidecar（session_occupied 的 meta 臂——
/// 新建未发言会话也算在场）。
fn isolate(home: &Path, session_key: &str) -> HomeGuard {
    let lock = crate::test_home::lock_home();
    let old = nemesis_path::default_path_manager().home_dir();
    nemesis_path::default_path_manager().set_home_dir(home.to_path_buf());
    let safe = nemesis_utils::sanitize::sanitize_path_segment(session_key);
    let dir = nemesis_path::default_path_manager().sessions_log_dir();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{safe}.meta.json")), "{}").unwrap();
    HomeGuard { old, _lock: lock }
}

fn make_ctx(home: &Path) -> RequestContext {
    let ws = home.join("workspace").to_string_lossy().to_string();
    let state = Arc::new(AppState {
        cors_manager: None,
        events_snapshot: None,
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
        quic_relay: None,
        mail_plugin: None,
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: None,
        #[cfg(feature = "workflow")]
        chat_secret_store: Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::in_memory()),
        #[cfg(not(feature = "workflow"))]
        chat_secret_store: std::sync::Arc::new(()),
        #[cfg(feature = "workflow")]
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        #[cfg(not(feature = "workflow"))]
        webhook_rate_limiter: std::sync::Arc::new(()),
        internal_cmd_tx: None,
        estop: None,
        estop_audit: None,
        background_registry: None,
        signature_verify: None,
        skills_install_gate: None,
        session_stats: None,
        cron: None,
        board: None,
    });
    RequestContext {
        session_id: "loops".to_string(),
        chat_id: "loops".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

const SESSION_KEY: &str = "agent:main:session:bench-loop-a";

async fn create_loop(ctx: &RequestContext) -> Result<serde_json::Value, String> {
    LoopsHandler
        .handle_cmd(
            "create",
            Some(serde_json::json!({
                "goal": "把仓库 issue 清零",
                "session_id": "bench-loop-a",
            })),
            ctx,
        )
        .await
        .and_then(|v| v.ok_or_else(|| "expected payload".to_string()))
}

#[tokio::test]
async fn list_empty_dir_returns_empty_set() {
    let home = tempfile::tempdir().unwrap();
    let ctx = make_ctx(home.path());
    let out = LoopsHandler
        .handle_cmd("list", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["total"].as_u64(), Some(0));
    assert_eq!(out["loops"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn create_get_list_roundtrip() {
    let home = tempfile::tempdir().unwrap();
    let ctx = make_ctx(home.path());
    let _g = isolate(home.path(), SESSION_KEY);
    let out = create_loop(&ctx).await.unwrap();
    assert_eq!(out["created"].as_bool(), Some(true));
    assert_eq!(out["id"].as_str(), Some("L1"));

    let got = LoopsHandler
        .handle_cmd("get", Some(serde_json::json!({ "id": "L1" })), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got["status"].as_str(), Some("active"));
    assert_eq!(got["goal"].as_str(), Some("把仓库 issue 清零"));
    assert_eq!(got["session_key"].as_str(), Some(SESSION_KEY));
    // create 留痕 progress_log 一条。
    assert_eq!(got["progress_log"].as_array().unwrap().len(), 1);

    let list = LoopsHandler
        .handle_cmd("list", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(list["total"].as_u64(), Some(1));
    assert_eq!(list["loops"][0]["id"].as_str(), Some("L1"));
    assert_eq!(list["loops"][0]["status"].as_str(), Some("active"));
}

#[tokio::test]
async fn create_rejects_unoccupied_session() {
    let home = tempfile::tempdir().unwrap();
    let ctx = make_ctx(home.path());
    // 在场的是别的会话；bench-loop-a 不存在 → create 拒绝（不静默绑死）。
    let _g = isolate(home.path(), "agent:main:session:other");
    let err = create_loop(&ctx).await.unwrap_err();
    assert!(err.contains("不存在"), "unexpected: {err}");
}

#[tokio::test]
async fn create_rejects_missing_goal_or_session() {
    let home = tempfile::tempdir().unwrap();
    let ctx = make_ctx(home.path());
    let _g = isolate(home.path(), SESSION_KEY);
    let err = LoopsHandler
        .handle_cmd(
            "create",
            Some(serde_json::json!({ "session_id": "bench-loop-a" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("goal"), "unexpected: {err}");
    let err = LoopsHandler
        .handle_cmd("create", Some(serde_json::json!({ "goal": "g" })), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("session"), "unexpected: {err}");
}

#[tokio::test]
async fn pause_resume_state_machine_with_audit() {
    let home = tempfile::tempdir().unwrap();
    let ctx = make_ctx(home.path());
    let _g = isolate(home.path(), SESSION_KEY);
    create_loop(&ctx).await.unwrap();

    // active 不能直接 resume（仅 paused/parked 可恢复）。
    let err = LoopsHandler
        .handle_cmd("resume", Some(serde_json::json!({"id": "L1"})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("仅 paused/parked"), "unexpected: {err}");

    LoopsHandler
        .handle_cmd("pause", Some(serde_json::json!({"id": "L1"})), &ctx)
        .await
        .unwrap()
        .unwrap();
    // 暂停后再暂停被拒（仅 active 可暂停）。
    let err = LoopsHandler
        .handle_cmd("pause", Some(serde_json::json!({"id": "L1"})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("仅 active"), "unexpected: {err}");

    let out = LoopsHandler
        .handle_cmd("resume", Some(serde_json::json!({"id": "L1"})), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["status"].as_str(), Some("active"));

    // 留痕：create + paused + resumed 三条 progress。
    let got = LoopsHandler
        .handle_cmd("get", Some(serde_json::json!({"id": "L1"})), &ctx)
        .await
        .unwrap()
        .unwrap();
    let logs: Vec<&str> = got["progress_log"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["text"].as_str().unwrap())
        .collect();
    assert!(logs.iter().any(|t| t.contains("[paused]")));
    assert!(logs.iter().any(|t| t.contains("[resumed]")));
}

#[tokio::test]
async fn parked_loop_resumes_then_budget_widens() {
    let home = tempfile::tempdir().unwrap();
    let ctx = make_ctx(home.path());
    let _g = isolate(home.path(), SESSION_KEY);
    create_loop(&ctx).await.unwrap();
    // 模拟驱动器停车（直接改状态文件——与 sweep_loop Park 臂同形）。
    let loops_dir = home.path().join("workspace").join("loops");
    let mut st = nemesis_loop::load_loop(&loops_dir, "L1").unwrap().unwrap();
    st.status = nemesis_loop::LoopStatus::Parked;
    st.budget.max_turns_total = 1;
    st.spent.turns = 1;
    nemesis_loop::save_loop(&loops_dir, &st).unwrap();

    // parked → resume 合法（解除停车场）。
    let out = LoopsHandler
        .handle_cmd("resume", Some(serde_json::json!({"id": "L1"})), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["status"].as_str(), Some("active"));
    // 人调预算（提高上限）→ update 面可达。
    LoopsHandler
        .handle_cmd(
            "update",
            Some(serde_json::json!({"id": "L1", "budget": {"max_turns_total": 10}})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    let got = LoopsHandler
        .handle_cmd("get", Some(serde_json::json!({"id": "L1"})), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got["budget"]["max_turns_total"].as_u64(), Some(10));
}

// BUG-20261127-020 回归钉：update 的 budget 此前是整体替换语义——部分
// 形态（LoopsView「调预算」按钮只发 max_turns_total）经 serde default
// 全零反序列化后整体写回，把未提及维度的预算静默清成 0=不限（墙钟/token
// 保险丝静默拆除）。修复后按字段合并：缺席 = 保留现值，显式 0 = 刻意设
// 不限；显式清零仍可表达（三字段全显式 0）。

/// 预置三维预算（create 不带 budget = 全零，直接改状态文件补齐）。
fn seed_full_budget(home: &Path) {
    let loops_dir = home.join("workspace").join("loops");
    let mut st = nemesis_loop::load_loop(&loops_dir, "L1").unwrap().unwrap();
    st.budget.max_turns_total = 5;
    st.budget.wall_clock_budget_secs = 3600;
    st.budget.max_tokens = 100_000;
    nemesis_loop::save_loop(&loops_dir, &st).unwrap();
}

async fn get_budget(ctx: &RequestContext) -> serde_json::Value {
    LoopsHandler
        .handle_cmd("get", Some(serde_json::json!({"id": "L1"})), ctx)
        .await
        .unwrap()
        .unwrap()["budget"]
        .clone()
}

#[tokio::test]
async fn update_budget_partial_merges_preserves_other_dims() {
    let home = tempfile::tempdir().unwrap();
    let ctx = make_ctx(home.path());
    let _g = isolate(home.path(), SESSION_KEY);
    create_loop(&ctx).await.unwrap();
    seed_full_budget(home.path());

    // 部分更新：只调 max_turns_total（LoopsView widenBudget 同款形态）。
    LoopsHandler
        .handle_cmd(
            "update",
            Some(serde_json::json!({"id": "L1", "budget": {"max_turns_total": 10}})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    let budget = get_budget(&ctx).await;
    assert_eq!(budget["max_turns_total"].as_u64(), Some(10));
    assert_eq!(
        budget["wall_clock_budget_secs"].as_u64(),
        Some(3600),
        "部分更新不得静默解除墙钟预算（BUG-20261127-020）"
    );
    assert_eq!(
        budget["max_tokens"].as_u64(),
        Some(100_000),
        "部分更新不得静默解除 token 预算（BUG-20261127-020）"
    );
}

#[tokio::test]
async fn update_budget_explicit_zero_sets_unlimited_others_kept() {
    let home = tempfile::tempdir().unwrap();
    let ctx = make_ctx(home.path());
    let _g = isolate(home.path(), SESSION_KEY);
    create_loop(&ctx).await.unwrap();
    seed_full_budget(home.path());

    // 显式 0 = 刻意设不限（该维生效），未提及维保留现值。
    LoopsHandler
        .handle_cmd(
            "update",
            Some(serde_json::json!(
                {"id": "L1", "budget": {"wall_clock_budget_secs": 0}}
            )),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    let budget = get_budget(&ctx).await;
    assert_eq!(budget["wall_clock_budget_secs"].as_u64(), Some(0));
    assert_eq!(budget["max_turns_total"].as_u64(), Some(5));
    assert_eq!(budget["max_tokens"].as_u64(), Some(100_000));
}

#[tokio::test]
async fn update_and_remove_missing_id_honest_error() {
    let home = tempfile::tempdir().unwrap();
    let ctx = make_ctx(home.path());
    let err = LoopsHandler
        .handle_cmd("update", Some(serde_json::json!({"id": "L9"})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("不存在"), "unexpected: {err}");
    let err = LoopsHandler
        .handle_cmd("remove", Some(serde_json::json!({"id": "L9"})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("不存在"), "unexpected: {err}");
}

#[tokio::test]
async fn remove_deletes_state_file() {
    let home = tempfile::tempdir().unwrap();
    let ctx = make_ctx(home.path());
    let _g = isolate(home.path(), SESSION_KEY);
    create_loop(&ctx).await.unwrap();
    let out = LoopsHandler
        .handle_cmd("remove", Some(serde_json::json!({"id": "L1"})), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["removed"].as_bool(), Some(true));
    let list = LoopsHandler
        .handle_cmd("list", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(list["total"].as_u64(), Some(0));
}

#[tokio::test]
async fn unknown_command_honest_error() {
    let home = tempfile::tempdir().unwrap();
    let ctx = make_ctx(home.path());
    let err = LoopsHandler
        .handle_cmd("nope", None, &ctx)
        .await
        .unwrap_err();
    assert!(
        err.contains("unknown command: loops.nope"),
        "unexpected: {err}"
    );
}

#[test]
fn commands_list_pinned() {
    assert_eq!(
        LoopsHandler.commands(),
        &[
            "list", "get", "create", "update", "pause", "resume", "remove"
        ]
    );
}

// 初始排程（2026-11-17 会话资源盘点看板 B3「+ 设置目标」前置）：
// delay_secs / cron 透传 compute_next_wake；缺省 = 不排程（旧行为）。

#[tokio::test]
async fn create_with_delay_schedules_initial_wake() {
    let home = tempfile::tempdir().unwrap();
    let _g = isolate(home.path(), SESSION_KEY);
    let ctx = make_ctx(home.path());
    let out = LoopsHandler
        .handle_cmd(
            "create",
            Some(serde_json::json!({
                "goal": "delay 排程",
                "session_id": "bench-loop-a",
                "delay_secs": 120,
            })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    let id = out["id"].as_str().unwrap().to_string();
    let detail = LoopsHandler
        .handle_cmd("get", Some(serde_json::json!({ "id": id })), &ctx)
        .await
        .unwrap()
        .unwrap();
    // next_wake 是 format_wake_time 人读串（有排程 = 非空）；cron 不设。
    assert!(
        detail["next_wake"].as_str().is_some_and(|s| !s.is_empty()),
        "delay 排程应有 next_wake: {detail}"
    );
    assert_eq!(detail["cron"], serde_json::Value::Null);
}

#[tokio::test]
async fn create_with_cron_sets_cron_field() {
    let home = tempfile::tempdir().unwrap();
    let _g = isolate(home.path(), SESSION_KEY);
    let ctx = make_ctx(home.path());
    let out = LoopsHandler
        .handle_cmd(
            "create",
            Some(serde_json::json!({
                "goal": "cron 排程",
                "session_id": "bench-loop-a",
                "cron": "0 9 * * *",
            })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    let id = out["id"].as_str().unwrap().to_string();
    let detail = LoopsHandler
        .handle_cmd("get", Some(serde_json::json!({ "id": id })), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(detail["cron"].as_str(), Some("0 9 * * *"));
    assert!(detail["next_wake"].as_str().is_some_and(|s| !s.is_empty()));
}

#[tokio::test]
async fn create_invalid_cron_is_loud_error() {
    let home = tempfile::tempdir().unwrap();
    let _g = isolate(home.path(), SESSION_KEY);
    let ctx = make_ctx(home.path());
    let err = LoopsHandler
        .handle_cmd(
            "create",
            Some(serde_json::json!({
                "goal": "坏 cron",
                "session_id": "bench-loop-a",
                "cron": "not a cron",
            })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("排程无效"), "unexpected: {err}");
}

#[tokio::test]
async fn create_without_schedule_keeps_legacy_behavior() {
    let home = tempfile::tempdir().unwrap();
    let _g = isolate(home.path(), SESSION_KEY);
    let ctx = make_ctx(home.path());
    let out = create_loop(&ctx).await.unwrap();
    let id = out["id"].as_str().unwrap().to_string();
    let detail = LoopsHandler
        .handle_cmd("get", Some(serde_json::json!({ "id": id })), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(detail["next_wake"], serde_json::Value::Null);
    assert_eq!(detail["cron"], serde_json::Value::Null);
}
