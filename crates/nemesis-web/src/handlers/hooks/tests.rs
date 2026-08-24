//! Tests for `HooksHandler` (P4). Private-method tests run under every
//! feature combo (module declared from `hooks.rs`, same pattern as
//! `models/tests.rs`); one dispatch-level test proves the `ctx.home` wiring.

use super::*;

fn home_str(dir: &tempfile::TempDir) -> String {
    dir.path().to_string_lossy().to_string()
}

fn hooks_path(home: &std::path::Path) -> std::path::PathBuf {
    home.join("config").join(cc_hooks::HOOKS_FILE)
}

/// Working CC config: 2 PreToolUse scripts in one group + 1 Stop script.
const GOOD: &str = r#"{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash|Write",
        "hooks": [
          { "type": "command", "command": "python check.py", "timeout": 30 },
          { "type": "command", "command": "echo hi" }
        ]
      }
    ],
    "Stop": [
      { "hooks": [ { "type": "command", "command": "echo done" } ] }
    ]
  }
}"#;

#[tokio::test]
async fn get_missing_file_returns_template_not_error() {
    let dir = tempfile::tempdir().unwrap();
    let h = HooksHandler;
    let r = h.get(&home_str(&dir)).unwrap().unwrap();
    assert_eq!(r["exists"], false, "fresh home is normal, not an error");
    // Template must itself be valid CC format the user can save as-is.
    let content = r["content"].as_str().unwrap();
    assert!(cc_hooks::parse_cc_hooks(content).is_ok());
    assert_eq!(r["valid"], true);
    assert_eq!(r["error"], serde_json::Value::Null);
    assert_eq!(r["summary"]["total"], 0);
    for ev in ["PreToolUse", "PostToolUse", "SessionStart", "UserPromptSubmit", "Stop"] {
        assert_eq!(r["summary"][ev], 0, "template names all five events: {ev}");
    }
}

#[tokio::test]
async fn get_existing_file_returns_content_and_summary() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("config")).unwrap();
    std::fs::write(hooks_path(dir.path()), GOOD).unwrap();
    let h = HooksHandler;
    let r = h.get(&home_str(&dir)).unwrap().unwrap();
    assert_eq!(r["exists"], true);
    assert_eq!(r["valid"], true);
    assert_eq!(r["error"], serde_json::Value::Null);
    // Text round-trips verbatim (byte-for-byte what is on disk).
    assert_eq!(r["content"].as_str().unwrap(), GOOD);
    assert_eq!(r["summary"]["PreToolUse"], 2);
    assert_eq!(r["summary"]["PostToolUse"], 0);
    assert_eq!(r["summary"]["Stop"], 1);
    assert_eq!(r["summary"]["total"], 3);
}

#[tokio::test]
async fn get_invalid_file_returns_raw_content_plus_error() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("config")).unwrap();
    // Valid JSON, wrong shape — parse_cc_hooks must reject, get must still
    // hand the original text back so the user can fix it in the editor.
    let broken = r#"{ "hooks": { "PreToolUse": "not-an-array" } }"#;
    std::fs::write(hooks_path(dir.path()), broken).unwrap();
    let h = HooksHandler;
    let r = h.get(&home_str(&dir)).unwrap().unwrap();
    assert_eq!(r["exists"], true);
    assert_eq!(r["valid"], false);
    assert!(r["error"].as_str().unwrap().contains("not CC hooks format"));
    assert_eq!(r["content"].as_str().unwrap(), broken);
    assert_eq!(r["summary"], serde_json::Value::Null, "no counts for a broken file");
}

#[tokio::test]
async fn set_writes_verbatim_and_reports_summary() {
    let dir = tempfile::tempdir().unwrap();
    let h = HooksHandler;
    let r = h.set(&home_str(&dir), GOOD).unwrap().unwrap();
    assert_eq!(r["written"], true);
    assert_eq!(r["summary"]["PreToolUse"], 2);
    assert_eq!(r["summary"]["total"], 3);
    // Verbatim write: byte-for-byte, key order preserved (no pretty re-shuffle).
    assert_eq!(std::fs::read_to_string(hooks_path(dir.path())).unwrap(), GOOD);
    // get now sees it.
    let g = h.get(&home_str(&dir)).unwrap().unwrap();
    assert_eq!(g["exists"], true);
    assert_eq!(g["valid"], true);
}

#[tokio::test]
async fn set_rejects_bad_json_without_touching_disk() {
    let dir = tempfile::tempdir().unwrap();
    let h = HooksHandler;
    // Syntax error.
    let err = h.set(&home_str(&dir), "{ not json").unwrap_err();
    assert!(err.contains("invalid JSON"), "got: {err}");
    // Semantic error: valid JSON, non-CC shape.
    let err = h.set(&home_str(&dir), r#"{ "hooks": { "Stop": 5 } }"#).unwrap_err();
    assert!(err.contains("not CC hooks format"), "got: {err}");
    // Neither attempt created the file.
    assert!(!hooks_path(dir.path()).exists(), "validation must precede any write");
}

#[tokio::test]
async fn set_accepts_bare_top_level_form() {
    // parse_cc_hooks also accepts the bare form (hand-written files often
    // omit the outer "hooks" key) — set must not reject it.
    let dir = tempfile::tempdir().unwrap();
    let h = HooksHandler;
    let bare = r#"{ "PreToolUse": [ { "hooks": [ { "type": "command", "command": "echo x" } ] } ] }"#;
    let r = h.set(&home_str(&dir), bare).unwrap().unwrap();
    assert_eq!(r["summary"]["total"], 1);
}

// ---------------------------------------------------------------------------
// Dispatch wiring (ctx.home path)
// ---------------------------------------------------------------------------

/// Same AppState scaffold as coding/models tests (agent_loop None etc.).
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
async fn dispatch_get_and_set_via_ctx_home() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = HooksHandler;

    // get through dispatch: template for a fresh home.
    let r = h
        .handle_cmd("get", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["exists"], false);

    // set through dispatch: data.content payload shape.
    let r = h
        .handle_cmd(
            "set",
            Some(serde_json::json!({ "content": GOOD })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["written"], true);
    assert!(hooks_path(dir.path()).exists());

    // Unknown command errors.
    assert!(h.handle_cmd("bogus", None, &ctx).await.is_err());
}
