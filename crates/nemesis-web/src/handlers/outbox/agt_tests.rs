//! outbox.rs AGT 覆率批次（2026-09-25）。CD6 死信面 handler 此前无任何测试
//! 模块，本批补全确定性臂：
//! - dead_list：只列 dead 态条目（pending 不混入），entry_json 字段透传
//! - dead_replay：dead → pending 全链（attempts / 退避窗口 / 错误清零、
//!   created_at 刷新落盘）+ 未知 task_id 诚实报错
//! - 缺参 / 错型 task_id bail、未知命令、workspace 未配置 bail
//!
//! 磁盘布局唯一真相源 nemesis-cluster::outbox：`<ws>/cluster/outbox/
//! <task_id>/entry.json`，本测试直接落盘构造，不需要活实例（与生产约束
//! 一致：web 层只持 workspace 根）。

use super::*;
use crate::api_handlers::AppState;
use crate::ws_router::RequestContext;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

fn agt_ctx(ws: Option<&str>) -> RequestContext {
    RequestContext {
        session_id: "agt".to_string(),
        chat_id: "agt".to_string(),
        workspace: ws.map(|s| s.to_string()),
        home: ws.map(|s| s.to_string()),
        state: std::sync::Arc::new(AppState {
            auth_token: String::new(),
            session_count: std::sync::Arc::new(AtomicUsize::new(0)),
            workspace: ws.map(|s| s.to_string()),
            home: ws.map(|s| s.to_string()),
            version: "test".to_string(),
            start_time: Instant::now(),
            model_name: std::sync::Arc::new(parking_lot::Mutex::new("m".to_string())),
            model_base: std::sync::Arc::new(parking_lot::Mutex::new(String::new())),
            model_has_key: std::sync::Arc::new(AtomicBool::new(false)),
            event_hub: std::sync::Arc::new(crate::events::EventHub::new()),
            running: std::sync::Arc::new(AtomicBool::new(true)),
            session_manager: std::sync::Arc::new(
                crate::session::SessionManager::with_default_timeout(),
            ),
            inbound_tx: None,
            streaming_provider: None,
            ws_router: None,
            agent_service: None,
            data_store: None,
            memory_manager: None,
            forge: None,
            agent_loop: std::sync::Arc::new(parking_lot::RwLock::new(None)),
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
            webhook_rate_limiter: std::sync::Arc::new(
                crate::handlers::workflow::WebhookRateLimiter::new(),
            ),
            #[cfg(not(feature = "workflow"))]
            webhook_rate_limiter: std::sync::Arc::new(()),
            internal_cmd_tx: None,
            estop: None,
            signature_verify: None,
            cron: None,
            board: None,
        }),
        auth_method: crate::session::AuthMethod::default(),
    }
}

/// 落一个 outbox 条目目录（entry.json 为死信/非死信状态载体）。
fn seed_entry(root: &std::path::Path, task_id: &str, state: &str) {
    let dir = root.join(task_id);
    std::fs::create_dir_all(&dir).unwrap();
    let entry = serde_json::json!({
        "task_id": task_id,
        "source_node": "nodeA",
        "state": state,
        "created_at": "2026-09-25T00:00:00+08:00",
        "attempts": 1001,
        "total_bytes": 42,
        "last_error": "boom",
        "next_retry_at": "2026-09-26T00:00:00+08:00",
    });
    std::fs::write(dir.join("entry.json"), entry.to_string()).unwrap();
}

#[tokio::test]
async fn agt_dead_list_and_replay_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let root = dir.path().join("cluster").join("outbox");
    seed_entry(&root, "t1", "dead");
    seed_entry(&root, "t2", "pending");
    let ctx = agt_ctx(Some(&ws));

    // dead_list：只列 dead 态（pending 不混入），字段透传。
    let out = OutboxHandler
        .handle_cmd("dead_list", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    let list = out["dead_letters"].as_array().unwrap();
    assert_eq!(list.len(), 1, "{out}");
    assert_eq!(list[0]["task_id"], "t1");
    assert_eq!(list[0]["state"], "dead");
    assert_eq!(list[0]["attempts"], 1001);
    assert_eq!(list[0]["last_error"], "boom");

    // dead_replay：dead → pending，attempts / 退避 / 错误清零。
    let out = OutboxHandler
        .handle_cmd(
            "dead_replay",
            Some(serde_json::json!({ "task_id": "t1" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    let replayed = &out["replayed"];
    assert_eq!(replayed["task_id"], "t1", "{out}");
    assert_eq!(replayed["state"], "pending");
    assert_eq!(replayed["attempts"], 0);
    assert!(replayed["last_error"].is_null(), "{out}");
    assert!(replayed["next_retry_at"].is_null(), "{out}");

    // 落盘态已翻转，dead_list 归零。
    let raw = std::fs::read_to_string(root.join("t1").join("entry.json")).unwrap();
    assert!(
        raw.contains("\"state\":\"pending\"") || raw.contains("\"state\": \"pending\""),
        "{raw}"
    );
    let out = OutboxHandler
        .handle_cmd("dead_list", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["dead_letters"].as_array().unwrap().len(), 0, "{out}");

    // 未知 task_id：诚实报错。
    let err = OutboxHandler
        .handle_cmd(
            "dead_replay",
            Some(serde_json::json!({ "task_id": "ghost" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("发件箱条目不存在"), "{err}");
}

#[tokio::test]
async fn agt_dead_replay_param_guards_and_unknown_command() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let ctx = agt_ctx(Some(&ws));

    // task_id 缺 / 错型 → 同一 bail。
    for data in [
        None,
        Some(serde_json::json!({})),
        Some(serde_json::json!({ "task_id": 5 })),
    ] {
        let err = OutboxHandler
            .handle_cmd("dead_replay", data, &ctx)
            .await
            .unwrap_err();
        assert_eq!(err, "missing 'task_id' field");
    }

    // 未知命令。
    let err = OutboxHandler
        .handle_cmd("nope", None, &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "outbox: unknown command 'nope'");

    // workspace 未配置 → outbox_root bail。
    let ctx = agt_ctx(None);
    let err = OutboxHandler
        .handle_cmd("dead_list", None, &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "workspace not configured");
}
