//! 子代理 WSAPI handler 测试（2026-11-03）。handler 走全局注册表
//! （`subagent_registry::global()`，测试进程未 init = 纯内存态）——断言
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
    make_ctx_with_estop(None)
}

/// steer 急停闸测试用：注入已触发的 EstopState。
fn make_ctx_with_estop(estop: Option<Arc<nemesis_agent::estop::EstopState>>) -> RequestContext {
    let state = Arc::new(AppState {
        cors_manager: None,
        events_snapshot: None,
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
        mail_plugin: None,
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
        estop,
        estop_audit: None,
        background_registry: None,
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
    begin_with_loop_origin(tag, i, &format!("hdn:{tag}"), background)
}

/// steer 测试用：显式 loop_origin（handler 路由按注册表 loop_origin 选 loop，
/// "main" 落 ctx.state.agent_loop 主槽）。
fn begin_with_loop_origin(tag: &str, i: usize, loop_origin: &str, background: bool) -> String {
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
            loop_origin: loop_origin.to_string(),
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
    assert_eq!(
        h.commands(),
        &[
            "list",
            "detail",
            "events",
            "cancel",
            "archive",
            "unarchive",
            "remove",
            "steer"
        ]
    );
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

// ===========================================================================
// 归档三动词 + list archived 过滤（2026-11-15）。全局注册表共享（同二进制
// 其他测试可能写入/归档行）——断言一律按 marker loop_origin 过滤；
// archived_count 断言「与过滤参数无关 + ≥本测试归档数」不下确界。
// ===========================================================================

#[tokio::test]
async fn archive_unarchive_remove_full_flow() {
    let ctx = make_ctx();
    let h = SubagentsHandler;
    let g = nemesis_agent::subagent_registry::global();
    let id0 = begin_via_global("ar", 0, false);
    let id1 = begin_via_global("ar", 1, false);
    for id in [&id0, &id1] {
        g.finish(
            id,
            nemesis_agent::subagent_registry::SubagentStatus::Done,
            None,
            None,
            None,
        );
    }

    // 缺 ids / 空 ids = 诚实报错。
    let err = h.handle_cmd("archive", None, &ctx).await.unwrap_err();
    assert!(err.contains("requires {ids"), "err={err}");
    let err = h
        .handle_cmd("remove", Some(serde_json::json!({ "ids": [] })), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("requires {ids"), "err={err}");

    // archive 两行 → 缺省列表（工作面）不再出现。
    let out = h
        .handle_cmd(
            "archive",
            Some(serde_json::json!({ "ids": [id0, id1] })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["changed"], serde_json::json!(2));
    let out = h
        .handle_cmd(
            "list",
            Some(serde_json::json!({ "loop_origin": "hdn:ar" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert!(
        out["tasks"].as_array().unwrap().is_empty(),
        "缺省过滤后归档行从工作面消失: {out}"
    );

    // "only"：归档行可见，投影带 archived_at。
    let out = h
        .handle_cmd(
            "list",
            Some(serde_json::json!({ "loop_origin": "hdn:ar", "archived": "only" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    let tasks = out["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 2);
    assert!(tasks[0]["archived_at"].is_u64(), "task={}", tasks[0]);

    // "all"：不过滤（工作面 + 归档行都在）。
    let out = h
        .handle_cmd(
            "list",
            Some(serde_json::json!({ "loop_origin": "hdn:ar", "archived": "all" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["tasks"].as_array().unwrap().len(), 2);

    // archived_count：全库投影——与过滤参数组合无关，且 ≥ 本测试归档的 2。
    let out1 = h
        .handle_cmd("list", Some(serde_json::json!({ "archived": "no" })), &ctx)
        .await
        .unwrap()
        .unwrap();
    let out2 = h
        .handle_cmd(
            "list",
            Some(serde_json::json!({ "archived": "only", "status": "running", "origin": "delegate" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert!(out1["archived_count"].is_u64(), "out1={out1}");
    assert!(out1["archived_count"].as_u64().unwrap() >= 2, "out1={out1}");
    assert_eq!(
        out1["archived_count"], out2["archived_count"],
        "全库投影不随过滤参数漂移"
    );

    // unarchive 一行 → 工作面回来。
    let out = h
        .handle_cmd("unarchive", Some(serde_json::json!({ "ids": [id0] })), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["changed"], serde_json::json!(1));
    let out = h
        .handle_cmd(
            "list",
            Some(serde_json::json!({ "loop_origin": "hdn:ar" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    let ids: Vec<&str> = out["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["id"].as_str())
        .collect();
    assert_eq!(ids, vec![id0.as_str()], "只有恢复行回到工作面");

    // remove 两行 → detail 诚实 miss（物理摘除）。
    let out = h
        .handle_cmd(
            "remove",
            Some(serde_json::json!({ "ids": [id0, id1] })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["changed"], serde_json::json!(2));
    let err = h
        .handle_cmd("detail", Some(serde_json::json!({ "id": id0 })), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("task not found"), "err={err}");
}

#[tokio::test]
async fn archive_skips_running_rows_silently() {
    let ctx = make_ctx();
    let h = SubagentsHandler;
    let g = nemesis_agent::subagent_registry::global();
    let idr = begin_via_global("arr", 0, false); // 保持 Running
    let idt = begin_via_global("arr", 1, false);
    g.finish(
        &idt,
        nemesis_agent::subagent_registry::SubagentStatus::Done,
        None,
        None,
        None,
    );

    // 批量含运行中：只归档终态行，计数如实（1）。
    let out = h
        .handle_cmd(
            "archive",
            Some(serde_json::json!({ "ids": [idr, idt] })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["changed"], serde_json::json!(1));

    // 工作面只剩运行中行（终态行已入归档）。
    let out = h
        .handle_cmd(
            "list",
            Some(serde_json::json!({ "loop_origin": "hdn:arr" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    let ids: Vec<&str> = out["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["id"].as_str())
        .collect();
    assert_eq!(ids, vec![idr.as_str()], "运行中行留在工作面: {ids:?}");

    // 清理：终止运行行（不留跨测试 Running）。
    h.handle_cmd("cancel", Some(serde_json::json!({ "id": idr })), &ctx)
        .await
        .unwrap()
        .unwrap();
}

// ===========================================================================
// steer（飞行中驾驶，2026-11-16）。参数/未知任务/终态/急停全是 handler
// 前置闸；成功路径挂真 AgentLoop（loop_origin 默认 "main"）+ 注册表
// Running 行（loop_origin 同值），断言注入落该 loop 的 inbox next_step。
// ===========================================================================

/// 真循环成功路径用：不做事的 provider（steer 不触发 LLM 调用）。
struct NoopProvider;

#[async_trait::async_trait]
impl nemesis_agent::r#loop::LlmProvider for NoopProvider {
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
            content: "ok".to_string(),
            tool_calls: vec![],
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

fn make_ctx_with_loop(al: Arc<nemesis_agent::r#loop::AgentLoop>) -> RequestContext {
    let ctx = make_ctx();
    *ctx.state.agent_loop.write() = Some(al);
    ctx
}

#[tokio::test]
async fn steer_requires_id_and_instruction() {
    let ctx = make_ctx();
    let h = SubagentsHandler;
    let err = h.handle_cmd("steer", None, &ctx).await.unwrap_err();
    assert!(err.contains("requires {id, instruction}"), "err={err}");
    let err = h
        .handle_cmd(
            "steer",
            Some(serde_json::json!({ "id": "subagent:x" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("requires {id, instruction}"), "err={err}");
    let err = h
        .handle_cmd(
            "steer",
            Some(serde_json::json!({ "id": "subagent:x", "instruction": "   " })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("requires {id, instruction}"), "err={err}");
    let err = h
        .handle_cmd(
            "steer",
            Some(serde_json::json!({ "id": "", "instruction": "go" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("requires {id, instruction}"), "err={err}");
}

#[tokio::test]
async fn steer_honest_miss_and_terminal_pre_reject() {
    let ctx = make_ctx();
    let h = SubagentsHandler;
    let err = h
        .handle_cmd(
            "steer",
            Some(serde_json::json!({ "id": "subagent:no-such", "instruction": "go" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("task not found"), "err={err}");

    // 终态前置拒绝发生在 loop 路由之前（ctx.state.agent_loop 是 None——
    // 若路由先行会误导性地报 "agent loop not running"）。
    let id = begin_with_loop_origin("st", 0, "main", false);
    nemesis_agent::subagent_registry::global().finish(
        &id,
        nemesis_agent::subagent_registry::SubagentStatus::Done,
        Some(1),
        Some("done".to_string()),
        None,
    );
    let err = h
        .handle_cmd(
            "steer",
            Some(serde_json::json!({ "id": id, "instruction": "go" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("终态"), "err={err}");
}

#[tokio::test]
async fn steer_frozen_while_estop_engaged_then_routes_after_release() {
    let estop = Arc::new(nemesis_agent::estop::EstopState::new());
    estop.trigger();
    let ctx = make_ctx_with_estop(Some(estop.clone()));
    let h = SubagentsHandler;
    let id = begin_with_loop_origin("ste", 0, "main", false);

    let err = h
        .handle_cmd(
            "steer",
            Some(serde_json::json!({ "id": id, "instruction": "go" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("急停"), "err={err}");

    // 释放后：estop 闸放行，路由到主 loop——ctx.state.agent_loop 是 None，
    // 诚实报 "agent loop not running"（路由面可达性验证）。
    estop.release();
    let err = h
        .handle_cmd(
            "steer",
            Some(serde_json::json!({ "id": id, "instruction": "go" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("agent loop not running"), "err={err}");
}

#[tokio::test]
async fn steer_success_lands_in_main_loop_inbox_next_step() {
    let id = begin_with_loop_origin("sts", 0, "main", false);
    let al = Arc::new(nemesis_agent::r#loop::AgentLoop::new(
        Box::new(NoopProvider),
        nemesis_agent::types::AgentConfig::default(),
    ));
    let ctx = make_ctx_with_loop(al.clone());
    let h = SubagentsHandler;

    let out = h
        .handle_cmd(
            "steer",
            Some(serde_json::json!({ "id": id, "instruction": "转向：只做调查" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["steered"], serde_json::json!(true));
    assert_eq!(out["id"], serde_json::json!(id));
    assert!(
        out["reply"].as_str().unwrap().contains("转向"),
        "ack 带指令回显: {}",
        out["reply"]
    );

    // 注入面真相：子代理会话键的 next_step +1（loop 每轮开头 claim）。
    let pending = al.inbox_handle().pending(&id);
    assert_eq!(pending, (0, 1), "steer 落 next_step: {pending:?}");

    // 清理：终态（不留跨测试 Running 行）。
    nemesis_agent::subagent_registry::global().finish(
        &id,
        nemesis_agent::subagent_registry::SubagentStatus::Done,
        Some(1),
        None,
        None,
    );
}
