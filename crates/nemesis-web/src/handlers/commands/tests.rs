//! Tests for `CommandsHandler`（自定义 slash 命令表，2026-08-29）。
//! 直连 workspace 参数方法（照 hooks/tests.rs 模式，无需构造 RequestContext）。

use super::*;

fn ws(dir: &tempfile::TempDir) -> String {
    dir.path().to_string_lossy().to_string()
}

fn entry(name: &str, prompt: &str) -> CommandEntry {
    CommandEntry {
        name: name.into(),
        description: format!("desc of {name}"),
        argument_hint: String::new(),
        prompt: prompt.into(),
    }
}

#[test]
fn list_missing_file_returns_empty_not_error() {
    let dir = tempfile::tempdir().unwrap();
    let r = CommandsHandler::new().commands_list(&ws(&dir)).unwrap();
    assert_eq!(r["total"], 0);
    assert!(r["commands"].as_array().unwrap().is_empty());
}

#[test]
fn save_roundtrips_entries_to_disk() {
    let dir = tempfile::tempdir().unwrap();
    let h = CommandsHandler::new();
    let r = h
        .commands_save(&ws(&dir), vec![entry("review", "请审查：$ARGUMENTS")])
        .unwrap();
    assert_eq!(r["saved"], true);
    assert_eq!(r["total"], 1);

    // 落盘到 nemesis-path 真相源路径，list 读回一致。
    let path = resolve_commands_config_path_in_workspace(dir.path());
    assert!(path.exists(), "{}", path.display());
    let listed = h.commands_list(&ws(&dir)).unwrap();
    assert_eq!(listed["commands"][0]["name"], "review");
    assert_eq!(listed["commands"][0]["prompt"], "请审查：$ARGUMENTS");
}

#[test]
fn save_rejects_empty_blank_and_duplicate_names() {
    let dir = tempfile::tempdir().unwrap();
    let h = CommandsHandler::new();

    let err = h
        .commands_save(&ws(&dir), vec![entry("", "p")])
        .unwrap_err();
    assert!(err.contains("名称不能为空"), "{err}");

    let err = h
        .commands_save(&ws(&dir), vec![entry("two words", "p")])
        .unwrap_err();
    assert!(err.contains("不能包含空格"), "{err}");

    let err = h
        .commands_save(
            &ws(&dir),
            vec![entry("dup", "p1"), entry("dup", "p2")],
        )
        .unwrap_err();
    assert!(err.contains("重复"), "{err}");

    // 全部拒绝路径都不落盘。
    assert!(!resolve_commands_config_path_in_workspace(dir.path()).exists());
}

#[test]
fn save_rejects_empty_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let err = CommandsHandler::new()
        .commands_save(&ws(&dir), vec![entry("ok", "   ")])
        .unwrap_err();
    assert!(err.contains("提示词不能为空"), "{err}");
}

// ---------------------------------------------------------------------------
// dispatch 级（handle_cmd 经完整 RequestContext）——直连方法测试抓不到
// match 臂命令名错误（2026-08-29 曾把臂写成 "commands.list" 导致 100%
// unknown command，直连测试全绿）。此处必须走裸名 "list"/"save"。
// ---------------------------------------------------------------------------

fn make_ctx(dir: &tempfile::TempDir) -> RequestContext {
    use crate::api_handlers::AppState;
    use crate::events::EventHub;
    use crate::session::SessionManager;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::time::Instant;

    let ws = dir.path().to_string_lossy().to_string();
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: Some(ws.clone()),
        home: Some(ws.clone()),
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new(String::new())),
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

#[tokio::test]
async fn dispatch_list_and_save_via_bare_cmd_names() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = CommandsHandler::new();

    // 裸名 "list"（不是 "commands.list"）→ 空表。
    let listed = h
        .handle_cmd("list", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(listed["total"], 0);

    // 裸名 "save" → 落盘 + 返回计数。
    let saved = h
        .handle_cmd(
            "save",
            Some(serde_json::json!({
                "commands": [
                    { "name": "review", "description": "d",
                      "argument_hint": "<路径>", "prompt": "请审查 $ARGUMENTS" }
                ]
            })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved["saved"], true);
    assert_eq!(saved["total"], 1);

    // 再 list → 读回。
    let listed = h
        .handle_cmd("list", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(listed["commands"][0]["name"], "review");

    // 未知子命令 → 带模块前缀的 loud 错误。
    let err = h.handle_cmd("bogus", None, &ctx).await.unwrap_err();
    assert!(err.contains("unknown command: commands.bogus"), "{err}");
}
