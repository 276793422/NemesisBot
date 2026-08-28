//! S10b (quality-hardening goal 冲刺, web 批次 2): manager-dependent arms of
//! the memory handler + offline-safe model install + helper edge cases.
//!
//! Existing coverage (`tests.rs` + `memory_extra_tests.rs`) runs every command
//! with `memory_manager: None` in the AppState, so all `ctx.state
//! .memory_manager.as_ref()` arms (runtime vector disable, init rollback,
//! semantic search/store) were never executed. This module fills exactly
//! those arms, plus:
//! - `model.install` success arm (offline via `local_model_path` — never
//!   touches the network) and the lock-contention early return,
//! - `entries.store` raw-append dir-creation failure,
//! - `entries.list`/`entries.search` blank-line + contentless-entry parsing,
//! - `stats` memory-dir counting, `count_episodic` flat-file arm,
//! - `collect_files` missing-dir / empty-base arms,
//! - `truncate_entry_content` multibyte char-boundary loop,
//! - `migrate_legacy_vector_store` create-dir failure arm.

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::{AuthMethod, SessionManager};
use crate::ws_router::ModuleHandler;
use nemesis_memory::manager::{Config as MemoryConfig, MemoryManager};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

// -----------------------------------------------------------------------
// Helpers
// -----------------------------------------------------------------------

/// AppState builder with an optional injected MemoryManager rooted at
/// `data_dir` (the manager's own tree — `<ws>/memory_vector` in production).
fn make_ctx_mgr(dir: &tempfile::TempDir, data_dir: &std::path::Path) -> RequestContext {
    let ws = dir.path().to_string_lossy().to_string();
    let mgr = Arc::new(MemoryManager::new(&MemoryConfig::new(data_dir)));
    let state = Arc::new(build_state(&ws, Some(mgr)));
    RequestContext {
        session_id: "test-session".to_string(),
        chat_id: "test-chat".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: AuthMethod::default(),
    }
}

fn build_state(ws: &str, memory_manager: Option<Arc<MemoryManager>>) -> AppState {
    AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: Some(ws.to_string()),
        home: Some(ws.to_string()),
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
        memory_manager,
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
        cron: None,
    }
}

/// Minimal config.json at `home` so `set_main_switch` has a file to edit.
fn write_main_config(home: &std::path::Path, memory_enabled: bool) {
    let cfg = serde_json::json!({ "memory": { "enabled": memory_enabled } });
    std::fs::write(
        home.join("config.json"),
        serde_json::to_string_pretty(&cfg).unwrap(),
    )
    .unwrap();
}

fn emb_config_path(workspace: &std::path::Path) -> std::path::PathBuf {
    workspace.join("config").join("config.enhanced_memory.json")
}

fn write_emb_config(workspace: &std::path::Path, cfg: &serde_json::Value) {
    std::fs::create_dir_all(workspace.join("config")).unwrap();
    std::fs::write(
        emb_config_path(workspace),
        serde_json::to_string_pretty(cfg).unwrap(),
    )
    .unwrap();
}

async fn run(
    ctx: &RequestContext,
    cmd: &str,
    data: serde_json::Value,
) -> Result<Option<serde_json::Value>, String> {
    MemoryHandler.handle_cmd(cmd, Some(data), ctx).await
}

// -----------------------------------------------------------------------
// config.set runtime-control arms (manager present)
// -----------------------------------------------------------------------

#[tokio::test]
async fn config_set_main_disabled_with_manager_turns_vector_off() {
    let dir = tempfile::tempdir().unwrap();
    write_main_config(dir.path(), true);
    let ctx = make_ctx_mgr(&dir, &dir.path().join("memory_vector"));
    let mgr = ctx.state.memory_manager.clone().unwrap();
    mgr.set_vector_enabled(true);
    assert!(mgr.is_vector_enabled());

    let out = run(
        &ctx,
        "config.set",
        serde_json::json!({ "main_enabled": false }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["updated"], serde_json::json!(true));
    // Runtime arm executed: manager vector flag turned off synchronously.
    assert!(!mgr.is_vector_enabled());
    // Persisted switch also flipped.
    let raw =
        serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(dir.path().join("config.json")).unwrap())
            .unwrap();
    assert_eq!(raw["memory"]["enabled"], serde_json::json!(false));
}

#[tokio::test]
async fn config_set_sub_enabled_local_model_path_passes_model_check_no_manager() {
    let dir = tempfile::tempdir().unwrap();
    write_main_config(dir.path(), false);
    // Active tier points at a real local file → model_ready check passes via
    // the local_model_path arm (no download, no manager → no runtime control,
    // so the sub-switch simply persists).
    let model_file = dir.path().join("fake-model.onnx");
    std::fs::write(&model_file, b"x").unwrap();
    write_emb_config(
        dir.path(),
        &serde_json::json!({
            "enabled": false,
            "active": "small",
            "models": {
                "large": { "name": "l" },
                "medium": { "name": "m" },
                "small": {
                    "name": "fake-small",
                    "dimension": 16,
                    "local_model_path": model_file.to_string_lossy(),
                }
            }
        }),
    );

    let ws = dir.path().to_string_lossy().to_string();
    let state = Arc::new(build_state(&ws, None));
    let ctx = RequestContext {
        session_id: "s".into(),
        chat_id: "c".into(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: AuthMethod::default(),
    };
    let out = run(&ctx, "config.set", serde_json::json!({ "sub_enabled": true }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["updated"], serde_json::json!(true));
    let after = serde_json::from_str::<serde_json::Value>(
        &std::fs::read_to_string(emb_config_path(dir.path())).unwrap(),
    )
    .unwrap();
    assert_eq!(after["enabled"], serde_json::json!(true));
}

#[tokio::test]
async fn config_set_sub_enabled_rolls_back_when_vector_init_fails() {
    let dir = tempfile::tempdir().unwrap();
    write_main_config(dir.path(), true);
    let model_file = dir.path().join("fake-model.onnx");
    std::fs::write(&model_file, b"x").unwrap();
    write_emb_config(
        dir.path(),
        &serde_json::json!({
            "enabled": false,
            "active": "small",
            "models": {
                "large": { "name": "l" },
                "medium": { "name": "m" },
                "small": {
                    "name": "fake-small",
                    "dimension": 16,
                    "local_model_path": model_file.to_string_lossy(),
                }
            }
        }),
    );

    let ctx = make_ctx_mgr(&dir, &dir.path().join("memory_vector"));
    // Manager exists but plugin_onnx.dll is not next to the test exe →
    // init_vector_store_from_config must fail → config rolled back to
    // enabled=false + loud error.
    let err = run(&ctx, "config.set", serde_json::json!({ "sub_enabled": true }))
        .await
        .unwrap_err();
    assert!(
        err.contains("向量存储初始化失败"),
        "unexpected error: {}",
        err
    );
    let after = serde_json::from_str::<serde_json::Value>(
        &std::fs::read_to_string(emb_config_path(dir.path())).unwrap(),
    )
    .unwrap();
    assert_eq!(after["enabled"], serde_json::json!(false));
}

#[tokio::test]
async fn config_set_sub_disabled_with_manager_disables_runtime_vector() {
    let dir = tempfile::tempdir().unwrap();
    write_main_config(dir.path(), true);
    write_emb_config(
        dir.path(),
        &serde_json::json!({
            "enabled": true,
            "active": "small",
            "models": { "large": { "name": "l" }, "medium": { "name": "m" }, "small": { "name": "s" } }
        }),
    );

    let ctx = make_ctx_mgr(&dir, &dir.path().join("memory_vector"));
    let mgr = ctx.state.memory_manager.clone().unwrap();
    mgr.set_vector_enabled(true);

    run(&ctx, "config.set", serde_json::json!({ "sub_enabled": false }))
        .await
        .unwrap()
        .unwrap();
    assert!(!mgr.is_vector_enabled());
    let after = serde_json::from_str::<serde_json::Value>(
        &std::fs::read_to_string(emb_config_path(dir.path())).unwrap(),
    )
    .unwrap();
    assert_eq!(after["enabled"], serde_json::json!(false));
}

// -----------------------------------------------------------------------
// entries.search / entries.store semantic arms (manager + vector flag on)
// -----------------------------------------------------------------------

#[tokio::test]
async fn entries_search_with_enabled_manager_reports_semantic() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_mgr(&dir, &dir.path().join("memory_vector"));
    let mgr = ctx.state.memory_manager.clone().unwrap();
    mgr.set_vector_enabled(true);

    let out = run(
        &ctx,
        "entries.search",
        serde_json::json!({ "query": "hello", "limit": 5 }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["search_type"], serde_json::json!("semantic"));
    assert_eq!(out["total"], serde_json::json!(0));
    assert!(out["results"].is_array());
}

#[tokio::test]
async fn entries_store_with_enabled_manager_uses_live_store() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_mgr(&dir, &dir.path().join("memory_vector"));
    let mgr = ctx.state.memory_manager.clone().unwrap();
    mgr.set_vector_enabled(true);

    let out = run(
        &ctx,
        "entries.store",
        serde_json::json!({ "content": "stored through the live manager" }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["stored"], serde_json::json!(true));
    assert!(out["id"].as_str().is_some_and(|s| !s.is_empty()));
}

#[tokio::test]
async fn entries_store_raw_append_fails_when_dir_path_is_a_file() {
    let dir = tempfile::tempdir().unwrap();
    // `memory_vector/vector` exists as a FILE → create_dir_all fails.
    let vector_dir = dir.path().join("memory_vector").join("vector");
    std::fs::create_dir_all(vector_dir.parent().unwrap()).unwrap();
    std::fs::write(&vector_dir, b"not a dir").unwrap();

    let ctx = make_ctx_mgr(&dir, &dir.path().join("memory_vector"));
    // Manager present but vector flag OFF → falls through to the raw append.
    let err = run(
        &ctx,
        "entries.store",
        serde_json::json!({ "content": "boom" }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("failed to create dir"), "got: {}", err);
}

// -----------------------------------------------------------------------
// entries.list / entries.search JSONL parsing edges
// -----------------------------------------------------------------------

fn write_vector_jsonl(workspace: &std::path::Path, body: &str) {
    let path = vector_store_jsonl_path(&workspace.to_string_lossy()).to_path_buf();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

#[tokio::test]
async fn entries_list_skips_blank_and_garbage_lines() {
    let dir = tempfile::tempdir().unwrap();
    write_vector_jsonl(
        dir.path(),
        concat!(
            "{\"id\":\"a\",\"content\":\"first\"}\n",
            "\n",
            "not-json\n",
            "{\"id\":\"b\",\"content\":\"second\"}\n"
        ),
    );
    let ctx = make_ctx_mgr(&dir, &dir.path().join("memory_vector"));
    let out = run(&ctx, "entries.list", serde_json::json!({}))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["total"], serde_json::json!(2));
    // Most recent first.
    assert_eq!(out["entries"][0]["id"], serde_json::json!("b"));
}

#[tokio::test]
async fn entries_search_keyword_skips_blank_and_contentless_lines() {
    let dir = tempfile::tempdir().unwrap();
    write_vector_jsonl(
        dir.path(),
        concat!(
            "\n",
            "{\"id\":\"x\",\"metadata\":{}}\n",
            "{\"id\":\"y\",\"content\":\"NEEDLE here\"}\n"
        ),
    );
    let ctx = make_ctx_mgr(&dir, &dir.path().join("memory_vector"));
    // Manager present but vector flag off → keyword fallback.
    let out = run(
        &ctx,
        "entries.search",
        serde_json::json!({ "query": "needle" }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["search_type"], serde_json::json!("keyword"));
    assert_eq!(out["total"], serde_json::json!(1));
    assert_eq!(out["results"][0]["id"], serde_json::json!("y"));
}

// -----------------------------------------------------------------------
// model.install: lock contention + offline success arm
// -----------------------------------------------------------------------

#[tokio::test]
async fn model_install_rejects_when_tier_already_locked() {
    let dir = tempfile::tempdir().unwrap();
    write_main_config(dir.path(), true);
    write_emb_config(
        dir.path(),
        &serde_json::json!({ "enabled": false, "active": "small", "models": {
            "large": { "name": "l" }, "medium": { "name": "m" }, "small": { "name": "s" } } }),
    );

    let ctx = make_ctx_mgr(&dir, &dir.path().join("memory_vector"));
    install_locks().lock().unwrap().insert("large".to_string());
    let err = run(&ctx, "model.install", serde_json::json!({ "tier": "large" }))
        .await
        .unwrap_err();
    install_locks().lock().unwrap().remove("large");
    assert!(err.contains("正在安装中"), "got: {}", err);
}

#[tokio::test]
async fn model_install_succeeds_offline_via_local_model_path() {
    let dir = tempfile::tempdir().unwrap();
    write_main_config(dir.path(), true);
    // Local model dir already contains both files → download_model_files
    // short-circuits on the local path and never touches the network.
    let model_dir = dir.path().join("local-model");
    std::fs::create_dir_all(&model_dir).unwrap();
    std::fs::write(model_dir.join("model.onnx"), b"onnx-bytes").unwrap();
    std::fs::write(model_dir.join("tokenizer.json"), b"tok").unwrap();
    write_emb_config(
        dir.path(),
        &serde_json::json!({
            "enabled": false,
            "active": "large",
            "models": {
                "large": { "name": "l" },
                "medium": { "name": "m" },
                "small": {
                    "name": "fake-small",
                    "dimension": 16,
                    "local_model_path": model_dir.join("model.onnx").to_string_lossy(),
                    "local_tokenizer_path": model_dir.join("tokenizer.json").to_string_lossy()
                }
            }
        }),
    );

    let ctx = make_ctx_mgr(&dir, &dir.path().join("memory_vector"));
    let out = run(&ctx, "model.install", serde_json::json!({ "tier": "small" }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["success"], serde_json::json!(true));
    assert_eq!(out["tier"], serde_json::json!("small"));
    assert_eq!(out["dimension"], serde_json::json!(16));
    // Active tier restored to the original value.
    let after = serde_json::from_str::<serde_json::Value>(
        &std::fs::read_to_string(emb_config_path(dir.path())).unwrap(),
    )
    .unwrap();
    assert_eq!(after["active"], serde_json::json!("large"));
    // Lock released after completion.
    assert!(!install_locks().lock().unwrap().contains("small"));
}

// -----------------------------------------------------------------------
// stats: memory/ dir counting
// -----------------------------------------------------------------------

#[tokio::test]
async fn stats_counts_files_under_memory_dir() {
    let dir = tempfile::tempdir().unwrap();
    let mem = dir.path().join("memory");
    std::fs::create_dir_all(mem.join("sub")).unwrap();
    std::fs::write(mem.join("a.md"), b"a").unwrap();
    std::fs::write(mem.join("sub").join("b.md"), b"b").unwrap();

    let ctx = make_ctx_mgr(&dir, &dir.path().join("memory_vector"));
    let out = run(&ctx, "stats", serde_json::json!({}))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["memory_entries"], serde_json::json!(2));
}

// -----------------------------------------------------------------------
// Direct helper edge cases
// -----------------------------------------------------------------------

#[test]
fn count_episodic_flat_file_counts_as_session_and_episode() {
    let dir = tempfile::tempdir().unwrap();
    let epi = dir.path().join("episodic");
    std::fs::create_dir_all(&epi).unwrap();
    // A session dir with two episode files.
    std::fs::create_dir_all(epi.join("sess1")).unwrap();
    std::fs::write(epi.join("sess1").join("e1.json"), b"1").unwrap();
    std::fs::write(epi.join("sess1").join("e2.json"), b"2").unwrap();
    // A flat file directly under episodic/.
    std::fs::write(epi.join("stray.json"), b"s").unwrap();

    let (sessions, episodes) = count_episodic(&epi);
    assert_eq!(sessions, 2, "dir + flat file each count as a session");
    assert_eq!(episodes, 3, "two in dir + one flat file");
}

#[test]
fn collect_files_missing_dir_returns_ok_empty() {
    let dir = tempfile::tempdir().unwrap();
    let mut out = Vec::new();
    collect_files(&dir.path().to_string_lossy(), "does-not-exist", &mut out).unwrap();
    assert!(out.is_empty());
}

#[test]
fn collect_files_empty_base_lists_workspace_root() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("root.txt"), b"x").unwrap();
    let mut out = Vec::new();
    collect_files(&dir.path().to_string_lossy(), "", &mut out).unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["path"], serde_json::json!("root.txt"));
}

#[test]
fn truncate_entry_content_multibyte_truncates_at_char_boundary() {
    // 150 CJK chars = 450 bytes > 200 → truncation path, and byte 200 falls
    // in the middle of a multibyte char → boundary loop must back off.
    let long: String = "忆".repeat(150);
    let entry = serde_json::json!({ "id": "x", "content": long });
    let truncated = truncate_entry_content(entry);
    let content = truncated["content"].as_str().unwrap();
    assert!(content.ends_with("..."));
    // ≤ 200 bytes of payload + the "..." suffix.
    assert!(content.len() <= 203, "len={}", content.len());
    // Must not panic and must decode as valid UTF-8 (implicit via as_str).
}

#[test]
fn truncate_entry_content_short_content_untouched() {
    let entry = serde_json::json!({ "id": "x", "content": "short" });
    let out = truncate_entry_content(entry);
    assert_eq!(out["content"], serde_json::json!("short"));
}

#[test]
fn migrate_legacy_fails_cleanly_when_target_parent_uncreatable() {
    let dir = tempfile::tempdir().unwrap();
    // `memory_vector/vector` as a FILE → create_dir_all(parent) fails.
    let vector_dir = dir.path().join("memory_vector").join("vector");
    std::fs::create_dir_all(vector_dir.parent().unwrap()).unwrap();
    std::fs::write(&vector_dir, b"file").unwrap();
    let legacy = dir.path().join("memory").join("vector").join("vector_store.jsonl");
    std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    std::fs::write(&legacy, b"{\"id\":\"legacy\"}\n").unwrap();

    // Must return without panic and without creating the target.
    migrate_legacy_vector_store(&dir.path().to_string_lossy());
    assert!(!vector_store_jsonl_path(&dir.path().to_string_lossy()).exists());
}

// -----------------------------------------------------------------------
// 条目管理（自动记忆注入 TAB，2026-08-28）：entries.get / delete / update
// -----------------------------------------------------------------------

#[tokio::test]
async fn entries_get_returns_full_untruncated_content_offline() {
    let dir = tempfile::tempdir().unwrap();
    // 300 字符内容：entries.list 会截断到 200+“...”，entries.get 必须给全量。
    let long_content = "记".repeat(300);
    let mut line = serde_json::json!({ "id": "long", "content": long_content }).to_string();
    line.push('\n');
    write_vector_jsonl(dir.path(), &line);

    let ctx = make_ctx_mgr(&dir, &dir.path().join("memory_vector"));

    let listed = run(&ctx, "entries.list", serde_json::json!({}))
        .await
        .unwrap()
        .unwrap();
    let listed_len = listed["entries"][0]["content"].as_str().unwrap().chars().count();
    assert!(listed_len < 300, "list must truncate, got {}", listed_len);

    let out = run(&ctx, "entries.get", serde_json::json!({ "id": "long" }))
        .await
        .unwrap()
        .unwrap();
    let got = out["entry"]["content"].as_str().unwrap();
    assert_eq!(got.chars().count(), 300, "entries.get must return FULL content");

    // 不存在的 id → entry:null，不报错。
    let miss = run(&ctx, "entries.get", serde_json::json!({ "id": "nope" }))
        .await
        .unwrap()
        .unwrap();
    assert!(miss["entry"].is_null());
}

#[tokio::test]
async fn entries_delete_removes_line_offline_jsonl() {
    let dir = tempfile::tempdir().unwrap();
    write_vector_jsonl(
        dir.path(),
        concat!(
            "{\"id\":\"a\",\"content\":\"keep\"}\n",
            "{\"id\":\"b\",\"content\":\"drop\"}\n"
        ),
    );
    let ctx = make_ctx_mgr(&dir, &dir.path().join("memory_vector"));

    let out = run(&ctx, "entries.delete", serde_json::json!({ "id": "b" }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["deleted"], serde_json::json!(true));

    // 行级删除落盘：只剩 a。
    let raw = std::fs::read_to_string(vector_store_jsonl_path(&dir.path().to_string_lossy()))
        .unwrap();
    assert!(raw.contains("\"a\"") && !raw.contains("\"drop\""), "raw: {}", raw);

    // 再删同一个 id → deleted:false（不报错）。
    let again = run(&ctx, "entries.delete", serde_json::json!({ "id": "b" }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(again["deleted"], serde_json::json!(false));
}

#[tokio::test]
async fn entries_update_without_manager_loud_rejects() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let state = Arc::new(build_state(&ws, None));
    let ctx = RequestContext {
        session_id: "t".into(),
        chat_id: "t".into(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: AuthMethod::default(),
    };
    let err = run(
        &ctx,
        "entries.update",
        serde_json::json!({ "id": "x", "content": "new" }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("强化记忆未启用"), "got: {}", err);
}

#[tokio::test]
async fn entries_update_with_enabled_manager_reembeds_via_delete_and_restore() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_mgr(&dir, &dir.path().join("memory_vector"));
    let mgr = ctx.state.memory_manager.clone().unwrap();
    mgr.set_vector_enabled(true);

    let stored = run(&ctx, "entries.store", serde_json::json!({ "content": "old bytes" }))
        .await
        .unwrap()
        .unwrap();
    let old_id = stored["id"].as_str().unwrap().to_string();

    let out = run(
        &ctx,
        "entries.update",
        serde_json::json!({ "id": old_id, "content": "new bytes" }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["updated"], serde_json::json!(true));
    let new_id = out["id"].as_str().unwrap().to_string();
    assert_ne!(new_id, old_id, "update = delete + re-store → new id");

    // 旧 id 已消失，新 id 带新内容可取回。
    let got_old = mgr.get(&old_id).await.unwrap();
    assert!(got_old.is_none(), "old entry must be gone");
    let got_new = mgr.get(&new_id).await.unwrap().expect("new entry present");
    assert_eq!(got_new.content, "new bytes");

    // 不存在的 id → loud 报错（不静默造新条目）。
    let err = run(
        &ctx,
        "entries.update",
        serde_json::json!({ "id": "missing", "content": "x" }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("entry not found"), "got: {}", err);
}

#[tokio::test]
async fn entries_list_paginates_with_offset_and_limit() {
    // 5 条（文件序 1..5，最新在后）→ 倒序后 5,4,3,2,1。
    let dir = tempfile::tempdir().unwrap();
    let body: String = (1..=5)
        .map(|i| format!("{{\"id\":\"e{i}\",\"content\":\"c{i}\"}}\n"))
        .collect();
    write_vector_jsonl(dir.path(), &body);
    let ctx = make_ctx_mgr(&dir, &dir.path().join("memory_vector"));

    // 默认（无 data）→ 旧行为：最多 100 条。
    let all = run(&ctx, "entries.list", serde_json::json!({}))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(all["total"], serde_json::json!(5));
    assert_eq!(all["entries"].as_array().unwrap().len(), 5);
    assert_eq!(all["entries"][0]["id"], serde_json::json!("e5"), "最新在前");

    // offset=1, limit=2 → 第 2、3 条（e4、e3）。
    let page = run(
        &ctx,
        "entries.list",
        serde_json::json!({ "offset": 1, "limit": 2 }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(page["total"], serde_json::json!(5), "total 恒为全量条数");
    let ids: Vec<&str> = page["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["e4", "e3"]);

    // 越界 offset → 空页但 total 不变。
    let tail = run(
        &ctx,
        "entries.list",
        serde_json::json!({ "offset": 100, "limit": 2 }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(tail["entries"].as_array().unwrap().len(), 0);
    assert_eq!(tail["total"], serde_json::json!(5));
}
