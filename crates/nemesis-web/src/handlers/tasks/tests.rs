//! Tests for the tasks handler — P1-2 (2026-08-24 UI entry gap) three-state
//! `max_rounds` parsing. Declared from `tasks.rs` so the private
//! `parse_max_rounds_patch` is reachable without constructing an AppState
//! (same pattern as `models/tests.rs`). The service-level three-state patch
//! semantics (absent/null/set on disk) are covered by
//! `nemesis-cron/src/service/tests.rs`; these tests pin the handler-side
//! parsing contract, in particular the loud rejection of present-but-invalid
//! values that would otherwise degrade to "clear" and silently wipe a job's
//! budget.

use super::*;

fn payload(v: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "max_rounds": v })
}

#[test]
fn max_rounds_absent_means_unchanged() {
    let data = serde_json::json!({ "id": "j1" });
    assert_eq!(parse_max_rounds_patch(&data).unwrap(), None);
}

#[test]
fn max_rounds_null_means_clear() {
    assert_eq!(
        parse_max_rounds_patch(&payload(serde_json::Value::Null)).unwrap(),
        Some(None)
    );
}

#[test]
fn max_rounds_positive_int_sets() {
    for n in [1u64, 5, 20, u32::MAX as u64] {
        assert_eq!(
            parse_max_rounds_patch(&payload(serde_json::json!(n))).unwrap(),
            Some(Some(n as u32)),
            "n={n}"
        );
    }
}

#[test]
fn max_rounds_invalid_values_rejected_loudly() {
    // 0 is filtered to "no budget" downstream (loop.rs `*v > 0`), so
    // accepting it would mean unlimited-while-looking-like-zero.
    // Negative/fractional/string/bool/huge all degrade to "absent"/"clear"
    // under the old silent parse — all must error instead.
    let bad = [
        serde_json::json!(0),
        serde_json::json!(-5),
        serde_json::json!(5.5),
        serde_json::json!("10"),
        serde_json::json!(true),
        serde_json::json!((u32::MAX as u64) + 1),
        serde_json::json!([5]),
        serde_json::json!({ "v": 5 }),
    ];
    for v in bad {
        let err = parse_max_rounds_patch(&payload(v.clone())).unwrap_err();
        assert!(
            err.contains("max_rounds"),
            "value {v}: error should name the field, got '{err}'"
        );
    }
}

/// cron.add collapses the three states into Option<u32> (None = global
/// default): absent and null both land on None via flatten.
#[test]
fn max_rounds_flatten_gives_add_semantics() {
    assert_eq!(
        parse_max_rounds_patch(&serde_json::json!({})).unwrap().flatten(),
        None
    );
    assert_eq!(
        parse_max_rounds_patch(&payload(serde_json::Value::Null))
            .unwrap()
            .flatten(),
        None
    );
    assert_eq!(
        parse_max_rounds_patch(&payload(serde_json::json!(7)))
            .unwrap()
            .flatten(),
        Some(7)
    );
}

// ============================================================
// Phase 3 覆盖率补测（2026-08-25）：cron.toggle / cron.run /
// cron.preview / cron.update 的 schedule 校验臂 / job_to_view 的
// 空 expr 臂。全部经真实 CronService（store 落 tempdir）走
// handle_cmd，钉 WSAPI 契约。
// ============================================================

use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::ModuleHandler;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

async fn make_ctx_with_cron(dir: &tempfile::TempDir) -> RequestContext {
    let ws = dir.path().to_string_lossy().to_string();
    let svc = Arc::new(std::sync::Mutex::new(CronService::new(
        dir.path().join("cron_jobs.json").to_string_lossy().as_ref(),
    )));
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: Some(ws.clone()),
        home: Some(ws.clone()),
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new("test-model".to_string())),
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
        chat_secret_store: std::sync::Arc::new(
            nemesis_workflow::chat_secrets::ChatSecretStore::in_memory(),
        ),
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        internal_cmd_tx: None,
        estop: None,
        cron: Some(svc),
    });
    RequestContext {
        session_id: "test-session".to_string(),
        chat_id: "test-chat".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

async fn add_job(ctx: &RequestContext, name: &str) -> String {
    let h = TasksHandler;
    let out = h
        .handle_cmd(
            "cron.add",
            Some(serde_json::json!({
                "name": name,
                "cron": "0 3 * * *",
                "prompt": "nightly report",
            })),
            ctx,
        )
        .await
        .unwrap()
        .unwrap();
    out["job"]["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn cron_toggle_flips_and_reports_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_cron(&dir).await;
    let h = TasksHandler;
    let id = add_job(&ctx, "nightly").await;

    // 新建默认 enabled=true → 第一次 toggle → false。
    let out = h
        .handle_cmd("cron.toggle", Some(serde_json::json!({ "id": id })), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["id"], id);
    assert_eq!(out["enabled"], false);

    // 未知 id。
    let err = h
        .handle_cmd("cron.toggle", Some(serde_json::json!({ "id": "nope" })), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("not found"), "{err}");
}

#[tokio::test]
async fn cron_run_executes_without_handler_and_reports_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_cron(&dir).await;
    let h = TasksHandler;
    let id = add_job(&ctx, "runner").await;

    // 无 on_job handler（gateway 未装配）→ execute_job 记 "executed" 后 Ok。
    let out = h
        .handle_cmd("cron.run", Some(serde_json::json!({ "id": id })), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["ran"], true);
    assert_eq!(out["id"], id);

    // last_status 落 "executed"。
    let list = h.handle_cmd("cron.list", None, &ctx).await.unwrap().unwrap();
    let job = list["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|j| j["id"] == id.as_str())
        .unwrap();
    assert_eq!(job["last_status"], "executed");

    // 未知 id。
    let err = h
        .handle_cmd("cron.run", Some(serde_json::json!({ "id": "ghost" })), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("not found"), "{err}");
}

#[tokio::test]
async fn cron_preview_valid_and_invalid() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_cron(&dir).await;
    let h = TasksHandler;

    let ok = h
        .handle_cmd(
            "cron.preview",
            Some(serde_json::json!({ "cron": "0 3 * * *" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ok["valid"], true);
    assert!(ok["description"].as_str().is_some_and(|s| !s.is_empty()));
    assert!(ok["next_run_at_ms"].is_number(), "valid 预览必须给下次时间");

    let bad = h
        .handle_cmd(
            "cron.preview",
            Some(serde_json::json!({ "cron": "not a cron" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(bad["valid"], false);
    assert!(bad["description"].as_str().is_some_and(|s| !s.is_empty()));
    assert!(bad["next_run_at_ms"].is_null());
}

#[tokio::test]
async fn cron_update_rejects_invalid_schedule_and_patches_name() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_cron(&dir).await;
    let h = TasksHandler;
    let id = add_job(&ctx, "to-patch").await;

    // 无效 cron 表达式 → validate_schedule Err 透传（不静默吞）。
    let err = h
        .handle_cmd(
            "cron.update",
            Some(serde_json::json!({ "id": id, "cron": "99 bad *" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("invalid cron"), "{err}");

    // 合法 patch：改名 + 换表达式。
    let out = h
        .handle_cmd(
            "cron.update",
            Some(serde_json::json!({ "id": id, "name": "renamed", "cron": "30 4 * * *" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["updated"], true);
    assert_eq!(out["job"]["name"], "renamed");
    assert_eq!(out["job"]["cron"], "30 4 * * *");
}

#[tokio::test]
async fn cron_list_view_of_exprless_job_has_empty_description() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_cron(&dir).await;

    // 直接经 service 塞一个无 expr 的 job（kind="at"）——cron.add 路径
    // 永远带 expr，空 expr 视图臂只有这种历史/服务侧数据能触发。
    let svc = ctx.state.cron.as_ref().unwrap();
    svc.lock().unwrap()
        .add_job_ext(
            "one-shot",
            CronSchedule {
                kind: "at".to_string(),
                at_ms: Some(1),
                every_ms: None,
                expr: None,
                tz: None,
            },
            "hello",
            true,
            None,
            None,
            None,
            None,
            true,
        )
        .unwrap();

    let h = TasksHandler;
    let out = h.handle_cmd("cron.list", None, &ctx).await.unwrap().unwrap();
    assert_eq!(out["total"], 1);
    let job = &out["jobs"][0];
    assert_eq!(job["cron"], "");
    assert_eq!(job["description"], "", "空 expr 必须 description 也为空");
}
