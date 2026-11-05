//! 子代理 WSAPI handler 测试（2026-11-03）。handler 走全局注册表
//! （`subagent_registry::global`，测试进程未 init = 纯内存态）——断言
//! 一律按唯一 marker 过滤，不假设全局表为空（同二进制其他测试可能写入）。

use super::subagents::SubagentsHandler;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

fn make_ctx() -> RequestContext {
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: None,
        home: None,
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
        session_stats: None,
        cron: None,
        board: None,
    });
    RequestContext {
        session_id: "t".to_string(),
        chat_id: "c-t".to_string(),
        workspace: None,
        home: None,
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

fn marker_id(tag: &str, i: usize) -> String {
    format!("subagent:hdn-{tag}-{i}")
}

fn begin_via_global(tag: &str, i: usize, background: bool) -> String {
    let id = marker_id(tag, i);
    nemesis_agent::subagent_registry::global().begin(
        nemesis_agent::subagent_registry::SubagentBegin {
            id: id.clone(),
            parent_session_key: "agent_main_session_hdl".to_string(),
            parent_chat_id: String::new(),
            origin: "model_spawn".to_string(),
            agent_id: "t-agent".to_string(),
            task: "handler 测试任务".to_string(),
            role: String::new(),
            tools_profile: "readonly".to_string(),
            depth: 1,
            background,
            loop_origin: format!("hdn:{tag}"),
            bg_task_id: background.then(|| format!("bg-{tag}-{i}")),
        },
    );
    id
}

#[tokio::test]
async fn list_filters_by_origin_and_loop_origin() {
    let ctx = make_ctx();
    let h = SubagentsHandler;
    let id0 = begin_via_global("f1", 0, false);
    let id1 = begin_via_global("f1", 1, true);

    let out = h
        .handle_cmd(
            "list",
            Some(serde_json::json!({ "loop_origin": "hdn:f1" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    let tasks = out["tasks"].as_array().unwrap();
    let ids: Vec<&str> = tasks.iter().filter_map(|t| t["id"].as_str()).collect();
    assert_eq!(ids.len(), 2, "exactly our two entries: {ids:?}");
    assert!(ids.contains(&id0.as_str()) && ids.contains(&id1.as_str()));

    // origin 过滤（marker 条目全是 model_spawn）。
    let out = h
        .handle_cmd(
            "list",
            Some(serde_json::json!({ "origin": "model_spawn", "loop_origin": "hdn:f1" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["tasks"].as_array().unwrap().len(), 2);

    // status 过滤（snake_case 串，Running 态全命中）。
    let out = h
        .handle_cmd(
            "list",
            Some(serde_json::json!({ "status": "running", "loop_origin": "hdn:f1" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["tasks"].as_array().unwrap().len(), 2);

    // 字段投影：running 布尔 + bg_task_id 透传。
    let row = out["tasks"].as_array().unwrap()[0].clone();
    assert_eq!(row["running"], serde_json::json!(true));
    assert!(row["bg_task_id"].is_string() || row["bg_task_id"].is_null());

    // finish 后 status 过滤能区分终态。
    nemesis_agent::subagent_registry::global().finish(
        &id1,
        nemesis_agent::subagent_registry::SubagentStatus::Done,
        Some(2),
        Some("done".to_string()),
        None,
    );
    let out = h
        .handle_cmd(
            "list",
            Some(serde_json::json!({ "loop_origin": "hdn:f1", "status": "done" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    let tasks = out["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["id"], serde_json::json!(id1));
    assert_eq!(tasks[0]["running"], serde_json::json!(false));
    assert!(tasks[0]["duration_ms"].is_number(), "终态行带时长");
}

#[tokio::test]
async fn detail_requires_id_and_honest_miss() {
    let ctx = make_ctx();
    let h = SubagentsHandler;
    let err = h.handle_cmd("detail", None, &ctx).await.unwrap_err();
    assert!(err.contains("requires {id}"), "err={err}");

    let err = h
        .handle_cmd(
            "detail",
            Some(serde_json::json!({ "id": "subagent:no-such" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("task not found"), "err={err}");

    let id = begin_via_global("d1", 0, false);
    let out = h
        .handle_cmd("detail", Some(serde_json::json!({ "id": id })), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["id"], serde_json::json!(id));
    assert_eq!(out["loop_origin"], serde_json::json!("hdn:d1"));
}

#[tokio::test]
async fn cancel_honest_miss_and_hit_flows() {
    let ctx = make_ctx();
    let h = SubagentsHandler;
    let err = h
        .handle_cmd(
            "cancel",
            Some(serde_json::json!({ "id": "subagent:no-such" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("task not found"), "err={err}");

    let err = h.handle_cmd("cancel", None, &ctx).await.unwrap_err();
    assert!(err.contains("requires {id}"), "err={err}");

    let id = begin_via_global("c1", 0, false);
    let out = h
        .handle_cmd("cancel", Some(serde_json::json!({ "id": id })), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["cancelled"], serde_json::json!(true));
    // 注册表状态已翻转（再 detail 可见 Cancelled）。
    let out = h
        .handle_cmd("detail", Some(serde_json::json!({ "id": id })), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["status"], serde_json::json!("cancelled"));
}

#[tokio::test]
async fn commands_surface_and_unknown_command_rejected() {
    let ctx = make_ctx();
    let h = SubagentsHandler;
    assert_eq!(h.module_name(), "subagents");
    assert_eq!(h.commands(), &["list", "detail", "events", "cancel"]);
    let err = h.handle_cmd("nope", None, &ctx).await.unwrap_err();
    assert!(err.contains("unknown command: subagents.nope"), "err={err}");
}

// events（回放面）：registry id == 环键（`subagent:{uuid}`），泵侧
// record_tool 落环 → replay_after 原样读回。环是进程级全局表——测试
// 须持 GLOBAL_TABLE_LOCK（chat_sync_tests 同款），防并行测试串扰。

#[tokio::test]
async fn events_replays_ring_under_registry_id() {
    use crate::chat_event_log::test_support::GLOBAL_TABLE_LOCK;
    let _guard = GLOBAL_TABLE_LOCK.lock().await;
    let ctx = make_ctx();
    let h = SubagentsHandler;

    // 诚实 miss：未知 id / 缺 id。
    let err = h
        .handle_cmd(
            "events",
            Some(serde_json::json!({ "id": "subagent:missing" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("task not found"), "err={err}");
    let err = h.handle_cmd("events", None, &ctx).await.unwrap_err();
    assert!(err.contains("requires {id}"), "err={err}");

    // 入册 + 落环（泵同款 record_tool 调用形态）→ 回放含两帧 + running 投影。
    let id = begin_via_global("e1", 0, false);
    crate::chat_event_log::record_tool(
        &id,
        serde_json::json!({"module": "chat", "cmd": "tool_event", "data": {"tool": "exec"}}),
    );
    crate::chat_event_log::record_tool(
        &id,
        serde_json::json!({"module": "chat", "cmd": "tool_event", "data": {"tool": "read_file"}}),
    );
    let out = h
        .handle_cmd("events", Some(serde_json::json!({ "id": id })), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["id"], serde_json::json!(id));
    assert_eq!(out["after_seq"], 0);
    assert_eq!(out["gap"], false);
    assert_eq!(out["running"], serde_json::json!(true));
    let events = out["events"].as_array().unwrap();
    assert_eq!(events.len(), 2, "两帧全回放: {events:?}");

    // after_seq 增量语义（追平 → 空数组）。
    let last = events.last().unwrap()["seq"].as_u64().unwrap();
    let out = h
        .handle_cmd(
            "events",
            Some(serde_json::json!({ "id": id, "after_seq": last })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert!(out["events"].as_array().unwrap().is_empty());

    // 终态投影：finish 后 running=false（前端据此停轮询）。
    nemesis_agent::subagent_registry::global().finish(
        &id,
        nemesis_agent::subagent_registry::SubagentStatus::Done,
        Some(3),
        Some("ok".to_string()),
        None,
    );
    let out = h
        .handle_cmd("events", Some(serde_json::json!({ "id": id })), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["running"], serde_json::json!(false));
}
