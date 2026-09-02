//! G2（U9 注入/replay 可见性，2026-08-28）：
//!
//! - `injection_summary`：台账聚合投影（source/index/role/chars），绝不回传原文
//! - `replay_verify`：session store + 台账重建 → 与 request_logs 原始请求
//!   逐字节比对（指纹定位 + 显式 request_id + no_ledger/no_recording/mismatch）
//!
//! 夹具沿用 r4_tests 的 make_ctx 模式（无 memory/loop），台账/请求目录均为
//! 合成构造；store 走真 SessionStore 落盘 → handler 落盘回退路径可读到。

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::RequestContext;
use nemesis_agent::r#loop::LlmMessage;
use nemesis_agent::replay::{InjectionRecord, RequestProjectionRecord, SummaryAsOf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

/// 台账/请求夹具用的原始 session key（':' 会被 sanitizer 换成 '_'）。
const RAW_KEY: &str = "agent:main:session:g2t";
/// handler 收到的 `session` 参数 = session_logs 文件 stem（已 sanitize）。
const STEM: &str = "agent_main_session_g2t";

fn make_ctx(dir: &tempfile::TempDir) -> RequestContext {
    let ws = dir.path().to_string_lossy().to_string();
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
        cron: None,
        board: None,
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

/// 三轮极简历史：无 tool 消息/无 summary → `project_history_for_request`
/// 原样通过，重建即逐字节可复现。
fn seed_store(ws: &std::path::Path) {
    let store =
        nemesis_agent::session::SessionStore::new_with_storage(ws.join("workspace/sessions"));
    let _ = store.get_or_create(RAW_KEY);
    store.add_message(RAW_KEY, "user", "hi");
    store.add_message(RAW_KEY, "assistant", "hello");
    store.add_message(RAW_KEY, "user", "how are you");
    store.save(RAW_KEY).unwrap();
}

fn projection_record(
    round: usize,
    roles: Vec<String>,
    injections: Vec<InjectionRecord>,
) -> RequestProjectionRecord {
    let count = roles.len();
    RequestProjectionRecord {
        trace_id: format!("trace-{}", round),
        session_key: RAW_KEY.to_string(),
        round,
        ts: "2026-08-28T07:00:00+08:00".to_string(),
        messages_count: count,
        roles,
        history_len_at_build: count,
        injections,
        voice_append: None,
        summary_as_of: None,
    }
}

fn write_ledger(ws: &std::path::Path, recs: &[RequestProjectionRecord]) {
    let dir = ws.join("logs/boundary");
    std::fs::create_dir_all(&dir).unwrap();
    let mut body = String::new();
    for r in recs {
        let mut v = serde_json::to_value(r).unwrap();
        v["kind"] = serde_json::json!("request_projection");
        body.push_str(&serde_json::to_string(&v).unwrap());
        body.push('\n');
    }
    std::fs::write(dir.join(format!("{}.replay.jsonl", STEM)), body).unwrap();
}

fn plain_msgs() -> Vec<LlmMessage> {
    vec![
        LlmMessage {
            role: "user".into(),
            content: "hi".into(),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
        },
        LlmMessage {
            role: "assistant".into(),
            content: "hello".into(),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
        },
        LlmMessage {
            role: "user".into(),
            content: "how are you".into(),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
        },
    ]
}

fn make_request_dir(
    ws: &std::path::Path,
    id: &str,
    round: usize,
    msgs: &[LlmMessage],
) -> std::path::PathBuf {
    let dir = ws.join("logs/request_logs").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    let envelope = serde_json::json!({
        "timestamp": "2026-08-28T07:00:00+08:00",
        "round": round,
        "body": {
            "model": "test-model",
            "messages": msgs
                .iter()
                .map(|m| serde_json::to_value(m).unwrap())
                .collect::<Vec<_>>(),
            "tools": [],
            "messages_count": msgs.len(),
            "tools_count": 0,
        },
    });
    std::fs::write(dir.join("00.request.md"), "req").unwrap();
    std::fs::write(dir.join("01.AI.Request.raw.json"), envelope.to_string()).unwrap();
    dir
}

// ---------------------------------------------------------------------------
// injection_summary
// ---------------------------------------------------------------------------

#[test]
fn injection_summary_aggregates_without_raw_content() {
    let dir = tempfile::tempdir().unwrap();
    let _ctx = make_ctx(&dir);
    let ws = dir.path().to_string_lossy().to_string();

    let long_digest = "上下文摘要".repeat(10);
    let mut rec = projection_record(
        1,
        vec!["user".into(), "assistant".into(), "user".into()],
        vec![
            InjectionRecord {
                index: 0,
                role: "user".into(),
                source: "context_digest".into(),
                content: long_digest.clone(),
            },
            InjectionRecord {
                index: 4,
                role: "system".into(),
                source: "grace_nudge".into(),
                content: "请收尾".into(),
            },
        ],
    );
    rec.voice_append = Some(nemesis_agent::replay::VoiceAppend {
        index: 2,
        suffix: "（语音播报后缀）".into(),
    });
    rec.summary_as_of = Some(SummaryAsOf {
        covers_up_to: 2,
        text: "早前对话摘要".into(),
    });
    write_ledger(dir.path(), &[rec]);

    let out = LogsHandler.injection_summary(&ws, STEM).unwrap().unwrap();

    assert_eq!(out["available"], true);
    assert_eq!(out["session"], STEM);
    assert_eq!(out["total_rounds"], 1);
    assert_eq!(out["total_injections"], 2);

    let round = &out["rounds"][0];
    assert_eq!(round["round"], 1);
    assert_eq!(round["trace_id"], "trace-1");
    assert_eq!(round["messages_count"], 3);
    assert_eq!(round["history_len"], 3);
    assert_eq!(round["voice_append"], true);
    assert_eq!(round["summary_used"], true);
    assert_eq!(round["summary_covers_up_to"], 2);

    let injections = round["injections"].as_array().unwrap();
    assert_eq!(injections.len(), 2);
    assert_eq!(injections[0]["source"], "context_digest");
    assert_eq!(injections[0]["index"], 0);
    assert_eq!(injections[0]["role"], "user");
    // chars 计数 = 字符数（非字节数），且绝不回传原文。
    assert_eq!(injections[0]["chars"], long_digest.chars().count());
    assert_eq!(injections[1]["source"], "grace_nudge");
    assert_eq!(injections[1]["chars"], 3);
    assert!(injections[0].get("content").is_none());
    assert!(out.to_string().find("上下文摘要上下文").is_none());
}

#[test]
fn injection_summary_missing_ledger_reports_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    let _ctx = make_ctx(&dir);
    let ws = dir.path().to_string_lossy().to_string();

    let out = LogsHandler.injection_summary(&ws, STEM).unwrap().unwrap();

    assert_eq!(out["available"], false);
    assert_eq!(out["total_rounds"], 0);
    assert_eq!(out["total_injections"], 0);
    assert!(out["rounds"].as_array().unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// replay_verify
// ---------------------------------------------------------------------------

#[test]
fn replay_verify_byte_exact_via_fingerprint() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let ws = dir.path().to_string_lossy().to_string();

    seed_store(dir.path());
    write_ledger(
        dir.path(),
        &[projection_record(
            1,
            vec!["user".into(), "assistant".into(), "user".into()],
            vec![],
        )],
    );
    let msgs = plain_msgs();
    make_request_dir(dir.path(), "2026-08-28_07-00-00_deadbeef", 1, &msgs);

    let out = LogsHandler
        .replay_verify(&ctx, &ws, STEM, 1, None, None)
        .unwrap()
        .unwrap();

    assert_eq!(out["ok"], true, "out = {}", out);
    assert_eq!(out["verdict"], "byte_exact");
    assert_eq!(out["request_id"], "2026-08-28_07-00-00_deadbeef");
}

#[test]
fn replay_verify_explicit_request_id_also_works() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let ws = dir.path().to_string_lossy().to_string();

    seed_store(dir.path());
    write_ledger(
        dir.path(),
        &[projection_record(
            1,
            vec!["user".into(), "assistant".into(), "user".into()],
            vec![],
        )],
    );
    make_request_dir(dir.path(), "2026-08-28_07-00-00_cafebabe", 1, &plain_msgs());

    let out = LogsHandler
        .replay_verify(
            &ctx,
            &ws,
            STEM,
            1,
            Some("2026-08-28_07-00-00_cafebabe"),
            None,
        )
        .unwrap()
        .unwrap();
    assert_eq!(out["ok"], true);
    assert_eq!(out["verdict"], "byte_exact");

    // 非法目录名（路径穿越形态）直接 no_recording，不落盘读。
    let bad = LogsHandler
        .replay_verify(&ctx, &ws, STEM, 1, Some("../evil"), None)
        .unwrap()
        .unwrap();
    assert_eq!(bad["ok"], false);
    assert_eq!(bad["verdict"], "no_recording");
}

#[test]
fn replay_verify_content_mismatch_reports_first_diff() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let ws = dir.path().to_string_lossy().to_string();

    seed_store(dir.path());
    write_ledger(
        dir.path(),
        &[projection_record(
            1,
            vec!["user".into(), "assistant".into(), "user".into()],
            vec![],
        )],
    );
    // 录制里第二条消息内容被改动 → 重建与录制在 index 1 分叉。
    let mut msgs = plain_msgs();
    msgs[1].content = "hello (tampered)".into();
    make_request_dir(dir.path(), "2026-08-28_07-00-00_abcdef01", 1, &msgs);

    let out = LogsHandler
        .replay_verify(&ctx, &ws, STEM, 1, None, None)
        .unwrap()
        .unwrap();

    assert_eq!(out["ok"], false, "out = {}", out);
    assert_eq!(out["verdict"], "mismatch");
    assert_eq!(out["first_diff"]["index"], 1);
    assert_eq!(out["first_diff"]["kind"], "content");
}

#[test]
fn replay_verify_no_ledger_reports_explicitly() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let ws = dir.path().to_string_lossy().to_string();

    let out = LogsHandler
        .replay_verify(&ctx, &ws, STEM, 1, None, None)
        .unwrap()
        .unwrap();
    assert_eq!(out["ok"], false);
    assert_eq!(out["verdict"], "no_ledger");
    assert!(out["note"].as_str().unwrap().contains("台账"));
}

#[test]
fn replay_verify_no_recording_when_request_logs_empty() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let ws = dir.path().to_string_lossy().to_string();

    seed_store(dir.path());
    write_ledger(
        dir.path(),
        &[projection_record(
            1,
            vec!["user".into(), "assistant".into(), "user".into()],
            vec![],
        )],
    );
    // request_logs 目录不存在 → 指纹扫描无果。

    let out = LogsHandler
        .replay_verify(&ctx, &ws, STEM, 1, None, None)
        .unwrap()
        .unwrap();
    assert_eq!(out["ok"], false);
    assert_eq!(out["verdict"], "no_recording");
}

#[test]
fn replay_verify_unavailable_when_history_trimmed() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let ws = dir.path().to_string_lossy().to_string();

    // store 只存了 1 条（等价于裁剪后状态），台账却声明本轮看见了 3 条。
    let store = nemesis_agent::session::SessionStore::new_with_storage(
        dir.path().join("workspace/sessions"),
    );
    let _ = store.get_or_create(RAW_KEY);
    store.add_message(RAW_KEY, "user", "how are you");
    store.save(RAW_KEY).unwrap();

    write_ledger(
        dir.path(),
        &[projection_record(
            1,
            vec!["user".into(), "assistant".into(), "user".into()],
            vec![],
        )],
    );
    let msgs = plain_msgs();
    make_request_dir(dir.path(), "2026-08-28_07-00-00_12345678", 1, &msgs);

    let out = LogsHandler
        .replay_verify(&ctx, &ws, STEM, 1, None, None)
        .unwrap()
        .unwrap();

    assert_eq!(out["ok"], false, "out = {}", out);
    assert_eq!(out["verdict"], "unavailable");
    assert_eq!(out["needed"], 3);
    assert_eq!(out["available"], 1);
}
