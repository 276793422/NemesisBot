//! goals handler 单测（`goals` feature 门控）：
//! - list：空集 / 回读摘要面；
//! - create：正常建（session_id sanitize 绑定 + goal.md 落盘）/ 会话不在场
//!   拒绝 / content 缺失拒绝 / 同会话已有未完成 goal 拒绝 / done 后新建替换；
//! - get：详情面（content + evals）/ 不存在诚实错；
//! - enable/disable：状态机（仅 paused/blocked 可 enable、仅 active 可
//!   disable、done 终态不可转出、blocked 解锁清注记）；
//! - update：content 编辑（done 拒）/ 文件回读；
//! - delete：三清 + 不存在拒绝；
//! - evals：台账投影 / 不存在拒绝；
//! - unknown command + commands() 清单钉死。
//!
//! home 单例经 loops/tests.rs 同款 HomeGuard 重定向到 tempdir
//! （test_home::lock_home 可重入锁跨测试串行）——create 的
//! session_occupied 校验走 PathManager 单例寻址，重定向后 meta 落
//! tempdir，零真实目录触碰。

use super::GoalsHandler;
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
        background_registry: None,
        signature_verify: None,
        skills_install_gate: None,
        session_stats: None,
        cron: None,
        board: None,
    });
    RequestContext {
        session_id: "goals".to_string(),
        chat_id: "goals".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

const SESSION_KEY: &str = "agent:main:session:bench-goal-a";

async fn create_goal(ctx: &RequestContext, session_id: &str) -> Result<serde_json::Value, String> {
    GoalsHandler
        .handle_cmd(
            "create",
            Some(serde_json::json!({
                "content": "修复登录 bug\n验收：测试全绿",
                "session_id": session_id,
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
    let out = GoalsHandler
        .handle_cmd("list", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["total"].as_u64(), Some(0));
    assert_eq!(out["goals"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn create_get_list_roundtrip() {
    let home = tempfile::tempdir().unwrap();
    let ctx = make_ctx(home.path());
    let _g = isolate(home.path(), SESSION_KEY);
    let out = create_goal(&ctx, "bench-goal-a").await.unwrap();
    assert_eq!(out["created"].as_bool(), Some(true));
    assert_eq!(out["id"].as_str(), Some("G1"));

    let got = GoalsHandler
        .handle_cmd("get", Some(serde_json::json!({ "id": "G1" })), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got["status"].as_str(), Some("active"));
    assert_eq!(got["created_by"].as_str(), Some("human"));
    assert_eq!(got["session_key"].as_str(), Some(SESSION_KEY));
    assert_eq!(
        got["content"].as_str(),
        Some("修复登录 bug\n验收：测试全绿")
    );
    assert_eq!(got["content_missing"].as_bool(), Some(false));
    assert_eq!(got["max_rounds"].as_u64(), Some(256));
    assert_eq!(got["evals"].as_array().unwrap().len(), 0);

    let list = GoalsHandler
        .handle_cmd("list", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(list["total"].as_u64(), Some(1));
    assert_eq!(list["goals"][0]["id"].as_str(), Some("G1"));
    assert_eq!(list["goals"][0]["status"].as_str(), Some("active"));
}

#[tokio::test]
async fn create_rejects_unoccupied_session() {
    let home = tempfile::tempdir().unwrap();
    let ctx = make_ctx(home.path());
    let _g = isolate(home.path(), "agent:main:session:other");
    let err = create_goal(&ctx, "bench-goal-a").await.unwrap_err();
    assert!(err.contains("不存在"), "unexpected: {err}");
}

#[tokio::test]
async fn create_rejects_missing_content_or_session() {
    let home = tempfile::tempdir().unwrap();
    let ctx = make_ctx(home.path());
    let _g = isolate(home.path(), SESSION_KEY);
    let err = GoalsHandler
        .handle_cmd(
            "create",
            Some(serde_json::json!({ "session_id": "bench-goal-a" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("content"), "unexpected: {err}");
    let err = GoalsHandler
        .handle_cmd("create", Some(serde_json::json!({ "content": "g" })), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("session"), "unexpected: {err}");
}

#[tokio::test]
async fn create_rejects_existing_incomplete_then_allows_after_done() {
    let home = tempfile::tempdir().unwrap();
    let ctx = make_ctx(home.path());
    let _g = isolate(home.path(), SESSION_KEY);
    create_goal(&ctx, "bench-goal-a").await.unwrap();

    // 同会话第二个 goal（active 在场）= 拒。
    let err = create_goal(&ctx, "bench-goal-a").await.unwrap_err();
    assert!(err.contains("未完成目标"), "unexpected: {err}");

    // paused 同样算未完成。
    GoalsHandler
        .handle_cmd("disable", Some(serde_json::json!({ "id": "G1" })), &ctx)
        .await
        .unwrap()
        .unwrap();
    let err = create_goal(&ctx, "bench-goal-a").await.unwrap_err();
    assert!(err.contains("未完成目标"), "unexpected: {err}");

    // 直接改注册表到 done（模拟评估闸判定）→ 新建放行，id 单调前进。
    let ws = home.path().join("workspace");
    nemesis_goals::update_registry(&ws, |entries| {
        let e = entries.iter_mut().find(|e| e.id == "G1").unwrap();
        e.status = nemesis_goals::GoalStatus::Done;
        Ok(())
    })
    .unwrap();
    let out = create_goal(&ctx, "bench-goal-a").await.unwrap();
    assert_eq!(out["id"].as_str(), Some("G2"));
    // 旧 done 条目保留作历史。
    let list = GoalsHandler
        .handle_cmd("list", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(list["total"].as_u64(), Some(2));
}

#[tokio::test]
async fn enable_disable_state_machine() {
    let home = tempfile::tempdir().unwrap();
    let ctx = make_ctx(home.path());
    let _g = isolate(home.path(), SESSION_KEY);
    create_goal(&ctx, "bench-goal-a").await.unwrap();

    // active 不能 enable（已经是运行态）。
    let err = GoalsHandler
        .handle_cmd("enable", Some(serde_json::json!({"id": "G1"})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("不允许转移"), "unexpected: {err}");

    // active → paused。
    let out = GoalsHandler
        .handle_cmd("disable", Some(serde_json::json!({"id": "G1"})), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["status"].as_str(), Some("paused"));
    // paused 再 disable 拒。
    let err = GoalsHandler
        .handle_cmd("disable", Some(serde_json::json!({"id": "G1"})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("不允许转移"), "unexpected: {err}");
    // paused → enable 恢复。
    let out = GoalsHandler
        .handle_cmd("enable", Some(serde_json::json!({"id": "G1"})), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["status"].as_str(), Some("active"));

    // blocked → enable 解锁并清停车注记。
    let ws = home.path().join("workspace");
    nemesis_goals::update_registry(&ws, |entries| {
        let e = entries.iter_mut().find(|e| e.id == "G1").unwrap();
        e.status = nemesis_goals::GoalStatus::Blocked;
        e.blocked_code = Some("round-limit".to_string());
        e.blocked_message = Some("预算耗尽".to_string());
        Ok(())
    })
    .unwrap();
    let out = GoalsHandler
        .handle_cmd("enable", Some(serde_json::json!({"id": "G1"})), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["status"].as_str(), Some("active"));
    let got = GoalsHandler
        .handle_cmd("get", Some(serde_json::json!({"id": "G1"})), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got["blocked_code"], serde_json::Value::Null);

    // done 终态：enable/disable 都拒。
    nemesis_goals::update_registry(&ws, |entries| {
        let e = entries.iter_mut().find(|e| e.id == "G1").unwrap();
        e.status = nemesis_goals::GoalStatus::Done;
        Ok(())
    })
    .unwrap();
    for cmd in ["enable", "disable"] {
        let err = GoalsHandler
            .handle_cmd(cmd, Some(serde_json::json!({"id": "G1"})), &ctx)
            .await
            .unwrap_err();
        assert!(err.contains("不允许转移"), "unexpected: {err}");
    }
}

#[tokio::test]
async fn update_edits_content_rejects_done() {
    let home = tempfile::tempdir().unwrap();
    let ctx = make_ctx(home.path());
    let _g = isolate(home.path(), SESSION_KEY);
    create_goal(&ctx, "bench-goal-a").await.unwrap();

    GoalsHandler
        .handle_cmd(
            "update",
            Some(serde_json::json!({"id": "G1", "content": "修正后的目标"})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    let got = GoalsHandler
        .handle_cmd("get", Some(serde_json::json!({"id": "G1"})), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got["content"].as_str(), Some("修正后的目标"));

    // done 后 update 拒。
    let ws = home.path().join("workspace");
    nemesis_goals::update_registry(&ws, |entries| {
        let e = entries.iter_mut().find(|e| e.id == "G1").unwrap();
        e.status = nemesis_goals::GoalStatus::Done;
        Ok(())
    })
    .unwrap();
    let err = GoalsHandler
        .handle_cmd(
            "update",
            Some(serde_json::json!({"id": "G1", "content": "x"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("done"), "unexpected: {err}");
}

#[tokio::test]
async fn delete_and_evals_projection() {
    let home = tempfile::tempdir().unwrap();
    let ctx = make_ctx(home.path());
    let _g = isolate(home.path(), SESSION_KEY);
    create_goal(&ctx, "bench-goal-a").await.unwrap();

    // evals：空台账 → 空数组。
    let out = GoalsHandler
        .handle_cmd("evals", Some(serde_json::json!({"id": "G1"})), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["evals"].as_array().unwrap().len(), 0);

    // 追加一条评估记录 → 投影可见。
    let ws = home.path().join("workspace");
    nemesis_goals::append_eval(
        &ws,
        "G1",
        &nemesis_goals::EvalRecord {
            at_ms: 1_000,
            round: 1,
            verdict: "continue".to_string(),
            gap: Some("缺回归测试".to_string()),
            model: "test-model".to_string(),
            duration_ms: 120,
            raw_excerpt: String::new(),
        },
    )
    .unwrap();
    let out = GoalsHandler
        .handle_cmd("evals", Some(serde_json::json!({"id": "G1"})), &ctx)
        .await
        .unwrap()
        .unwrap();
    let evals = out["evals"].as_array().unwrap();
    assert_eq!(evals.len(), 1);
    assert_eq!(evals[0]["verdict"].as_str(), Some("continue"));
    assert_eq!(evals[0]["gap"].as_str(), Some("缺回归测试"));

    // evals：不存在 id 诚实错。
    let err = GoalsHandler
        .handle_cmd("evals", Some(serde_json::json!({"id": "G9"})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("不存在"), "unexpected: {err}");

    // delete 三清。
    let out = GoalsHandler
        .handle_cmd("delete", Some(serde_json::json!({"id": "G1"})), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["removed"].as_bool(), Some(true));
    assert!(nemesis_goals::load_registry(&ws).unwrap().is_empty());
    assert!(nemesis_goals::read_goal_md(&ws, "G1").unwrap().is_none());
    let list = GoalsHandler
        .handle_cmd("list", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(list["total"].as_u64(), Some(0));

    // delete：不存在拒绝。
    let err = GoalsHandler
        .handle_cmd("delete", Some(serde_json::json!({"id": "G9"})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("不存在"), "unexpected: {err}");
}

#[tokio::test]
async fn unknown_command_honest_error() {
    let home = tempfile::tempdir().unwrap();
    let ctx = make_ctx(home.path());
    let err = GoalsHandler
        .handle_cmd("nope", None, &ctx)
        .await
        .unwrap_err();
    assert!(
        err.contains("unknown command: goals.nope"),
        "unexpected: {err}"
    );
}

#[test]
fn commands_list_pinned() {
    assert_eq!(
        GoalsHandler.commands(),
        &[
            "list", "get", "create", "update", "enable", "disable", "delete", "evals"
        ]
    );
}
