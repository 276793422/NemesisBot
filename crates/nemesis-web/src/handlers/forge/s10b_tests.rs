//! S10b (quality-hardening goal 冲刺, web 批次 2): forge handler narrow arms
//! not reached by `forge_extra_tests.rs` (which covers the main command
//! flows), all offline:
//!
//! - `stats` with an existing `config.forge.json` (the load-file arm at 179),
//! - `reflections.list` with a directory inside reflections/ (skip arm),
//! - `reflect` report-write failure (reflections/ is a FILE → Err arm),
//! - `config.save` runtime start/stop arms with a real (non-running) Forge
//!   instance, plus the garbage-config.forge.json silent-skip sub-arms,
//! - `learning.toggle` read-error + parse-error arms,
//! - direct helper coverage: `compute_experience_stats` unreadable file,
//!   `read_recent_experiences` limit offset + nested records,
//!   `count_jsonl_in_subdirs`, `find_latest_file[_path]`,
//!   `read_registry_artifacts`, `read_learning_cycles`.

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

fn build_state(ws: &str, forge: Option<Arc<nemesis_forge::forge::Forge>>) -> AppState {
    AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: Some(ws.to_string()),
        home: Some(ws.to_string()),
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
        forge,
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
    }
}

fn make_ctx(dir: &tempfile::TempDir) -> RequestContext {
    let ws = dir.path().to_string_lossy().to_string();
    RequestContext {
        session_id: "s".to_string(),
        chat_id: "c".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws.clone()),
        state: Arc::new(build_state(&ws, None)),
        auth_method: crate::session::AuthMethod::default(),
    }
}

fn make_ctx_with_forge(dir: &tempfile::TempDir) -> RequestContext {
    let ws = dir.path().to_string_lossy().to_string();
    let forge = Arc::new(nemesis_forge::forge::Forge::new(
        nemesis_forge::config::ForgeConfig::default(),
        dir.path().to_path_buf(),
    ));
    RequestContext {
        session_id: "s".to_string(),
        chat_id: "c".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws.clone()),
        state: Arc::new(build_state(&ws, Some(forge))),
        auth_method: crate::session::AuthMethod::default(),
    }
}

fn write_main_config(workspace: &Path, forge_enabled: bool) {
    std::fs::create_dir_all(workspace.join("config")).unwrap();
    let cfg = serde_json::json!({ "forge": { "enabled": forge_enabled } });
    std::fs::write(
        workspace.join("config.json"),
        serde_json::to_string_pretty(&cfg).unwrap(),
    )
    .unwrap();
}

async fn run(
    ctx: &RequestContext,
    cmd: &str,
    data: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    ForgeHandler::new().handle_cmd(cmd, data, ctx).await
}

// -----------------------------------------------------------------------
// stats / reflections.list / reflect narrow arms
// -----------------------------------------------------------------------

#[tokio::test]
async fn stats_loads_existing_forge_config_file() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    write_main_config(ws, true);
    // Existing config.forge.json → the load arm (not ForgeConfig::default()).
    std::fs::create_dir_all(ws.join("config")).unwrap();
    std::fs::write(
        ws.join("config/config.forge.json"),
        serde_json::to_string_pretty(&serde_json::json!({ "enabled": true, "max_experiences": 5 }))
            .unwrap(),
    )
    .unwrap();
    let ctx = make_ctx(&dir);
    let out = run(&ctx, "stats", None).await.unwrap().unwrap();
    assert_eq!(out["enabled"], serde_json::json!(true));
}

#[tokio::test]
async fn reflections_list_skips_directories() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let refl = ws.join("forge/reflections");
    std::fs::create_dir_all(refl.join("nested-dir")).unwrap();
    std::fs::write(refl.join("report.md"), "# r").unwrap();
    std::fs::write(refl.join("notes.txt"), "x").unwrap();

    let ctx = make_ctx(&dir);
    let out = run(&ctx, "reflections.list", None).await.unwrap().unwrap();
    let reports = out["reports"].as_array().unwrap();
    assert_eq!(reports.len(), 1, "only the .md file counts");
    assert_eq!(reports[0]["name"], serde_json::json!("report.md"));
}

#[tokio::test]
async fn reflect_reports_write_failure_but_returns_analysis() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    // Experiences exist so reflect() proceeds to the report write…
    let exp_dir = ws.join("forge/experiences");
    std::fs::create_dir_all(&exp_dir).unwrap();
    std::fs::write(
        exp_dir.join("experiences.jsonl"),
        concat!(
            "{\"experience\":{\"id\":\"1\",\"tool_name\":\"shell\",\"input_summary\":\"i\",",
            "\"output_summary\":\"o\",\"success\":true,\"duration_ms\":10,",
            "\"timestamp\":\"2026-01-01T00:00:00Z\",\"session_key\":\"s\"},\"dedup_hash\":\"h\"}\n"
        ),
    )
    .unwrap();
    // …but forge/reflections is a FILE → create_dir_all fails → write_report
    // errs → the "报告写入失败" arm.
    std::fs::write(ws.join("forge/reflections"), b"not a dir").unwrap();

    let ctx = make_ctx(&dir);
    let out = run(&ctx, "reflect", None).await.unwrap().unwrap();
    assert_eq!(out["triggered"], serde_json::json!(true));
    let msg = out["message"].as_str().unwrap();
    assert!(msg.contains("报告写入失败"), "got: {}", msg);
}

// -----------------------------------------------------------------------
// config.save runtime arms + garbage sub-arms
// -----------------------------------------------------------------------

#[tokio::test]
async fn config_save_enable_with_forge_instance_triggers_runtime_start() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    write_main_config(ws, false); // was disabled
    let ctx = make_ctx_with_forge(&dir);
    let out = run(
        &ctx,
        "config.save",
        Some(serde_json::json!({ "enabled": true })),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["saved"], serde_json::json!(true));
    // config.forge.json auto-created with enabled=true.
    let fc = std::fs::read_to_string(ws.join("config/config.forge.json")).unwrap();
    assert!(fc.contains("\"enabled\": true"));
    // Runtime start spawned (background; may or may not have flipped the
    // running flag by the time we assert — only assert the config side).
    let cfg: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(ws.join("config.json")).unwrap()).unwrap();
    assert_eq!(cfg["forge"]["enabled"], serde_json::json!(true));
}

#[tokio::test]
async fn config_save_disable_with_forge_instance_takes_stop_path() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    write_main_config(ws, true); // was enabled
    let ctx = make_ctx_with_forge(&dir);
    let out = run(
        &ctx,
        "config.save",
        Some(serde_json::json!({ "enabled": false })),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["saved"], serde_json::json!(true));
    let cfg: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(ws.join("config.json")).unwrap()).unwrap();
    assert_eq!(cfg["forge"]["enabled"], serde_json::json!(false));
}

#[tokio::test]
async fn config_save_with_garbage_forge_config_silently_recreates() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    write_main_config(ws, false);
    std::fs::create_dir_all(ws.join("config")).unwrap();
    std::fs::write(ws.join("config/config.forge.json"), "not json {{{").unwrap();
    let ctx = make_ctx(&dir);
    let out = run(
        &ctx,
        "config.save",
        Some(serde_json::json!({ "enabled": true })),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["saved"], serde_json::json!(true));
    // Unparseable existing file → the update arm skips, file left as-is
    // (documented silent-skip behaviour).
    assert_eq!(
        std::fs::read_to_string(ws.join("config/config.forge.json")).unwrap(),
        "not json {{{"
    );
}

// -----------------------------------------------------------------------
// learning.toggle error arms
// -----------------------------------------------------------------------

#[tokio::test]
async fn learning_toggle_fails_when_forge_config_is_a_directory() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    std::fs::create_dir_all(ws.join("config/config.forge.json")).unwrap();
    let ctx = make_ctx(&dir);
    let err = run(
        &ctx,
        "learning.toggle",
        Some(serde_json::json!({ "enabled": true })),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("failed to read config.forge.json"),
        "got: {}",
        err
    );
}

#[tokio::test]
async fn learning_toggle_fails_when_forge_config_unparseable() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    std::fs::create_dir_all(ws.join("config")).unwrap();
    std::fs::write(ws.join("config/config.forge.json"), "]garbage[").unwrap();
    let ctx = make_ctx(&dir);
    let err = run(
        &ctx,
        "learning.toggle",
        Some(serde_json::json!({ "enabled": false })),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("failed to parse config.forge.json"),
        "got: {}",
        err
    );
}

// -----------------------------------------------------------------------
// Direct helper coverage
// -----------------------------------------------------------------------

#[test]
fn compute_experience_stats_unreadable_file_returns_zeros() {
    let dir = tempfile::tempdir().unwrap();
    // experiences.jsonl exists but is a DIRECTORY → read fails → zeros.
    std::fs::create_dir_all(dir.path().join("experiences.jsonl")).unwrap();
    let r = compute_experience_stats(&dir.path().join("experiences.jsonl"));
    assert_eq!(r.total_count, 0);
    assert_eq!(r.success_count, 0);
    assert_eq!(r.avg_duration_ms, 0.0);
}

#[test]
fn read_recent_experiences_applies_limit_from_tail() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("experiences.jsonl");
    let mut body = String::new();
    for i in 0..5 {
        body.push_str(&format!(
            "{{\"experience\":{{\"id\":\"e{}\",\"tool_name\":\"t\",\"success\":true,\"duration_ms\":1}},\"dedup_hash\":\"h\"}}\n",
            i
        ));
    }
    std::fs::write(&p, body).unwrap();

    let recent = read_recent_experiences(&p, 2);
    assert_eq!(recent.len(), 2);
    assert_eq!(recent[0]["id"], serde_json::json!("e3"), "tail window");
    assert_eq!(recent[1]["id"], serde_json::json!("e4"));
    // Missing file → empty.
    let missing = dir.path().join("nope.jsonl");
    assert!(read_recent_experiences(&missing, 5).is_empty());
}

#[test]
fn count_jsonl_in_subdirs_counts_month_dirs_only() {
    let dir = tempfile::tempdir().unwrap();
    let months = dir.path().join("cycles");
    std::fs::create_dir_all(months.join("2026-01")).unwrap();
    std::fs::write(months.join("2026-01/day.jsonl"), "a\nb\n\n").unwrap();
    std::fs::write(months.join("2026-01/day.txt"), "x").unwrap();
    // Loose file directly under cycles/ must be skipped (not a month dir).
    std::fs::write(months.join("loose.jsonl"), "c\nd\n").unwrap();

    assert_eq!(count_jsonl_in_subdirs(&months), 2);
    let missing = dir.path().join("missing");
    assert_eq!(count_jsonl_in_subdirs(&missing), 0);
}

#[test]
fn find_latest_file_prefers_most_recent_matching_ext() {
    let dir = tempfile::tempdir().unwrap();
    let d: PathBuf = dir.path().to_path_buf();
    std::fs::write(d.join("old.md"), b"old").unwrap();
    // Ensure a visible mtime gap.
    std::thread::sleep(std::time::Duration::from_millis(30));
    std::fs::write(d.join("new.md"), b"new").unwrap();
    std::fs::create_dir_all(d.join("subdir")).unwrap();
    std::fs::write(d.join("ignore.txt"), b"x").unwrap();

    let latest = find_latest_file(&d, "md").expect("some md file");
    assert_eq!(latest["name"], serde_json::json!("new.md"));
    assert!(latest["modified"].as_str().is_some());

    assert!(find_latest_file(&d, "xyz").is_none());
    let missing = dir.path().join("missing");
    assert!(find_latest_file(&missing, "md").is_none());
    // Path-level helper also skips non-file entries.
    let p = find_latest_file_path(&d, "md").expect("path");
    assert!(p.ends_with("new.md"));
}

#[test]
fn read_registry_artifacts_parses_array_and_tolerates_garbage() {
    let dir = tempfile::tempdir().unwrap();
    let ok = dir.path().join("ok.json");
    std::fs::write(&ok, r#"[{"id":"a"},{"id":"b"}]"#).unwrap();
    assert_eq!(read_registry_artifacts(&ok).len(), 2);

    let bad = dir.path().join("bad.json");
    std::fs::write(&bad, "{not array}").unwrap();
    assert!(read_registry_artifacts(&bad).is_empty());

    let missing = dir.path().join("missing.json");
    assert!(read_registry_artifacts(&missing).is_empty());
}

#[test]
fn read_learning_cycles_walks_months_and_skips_noise() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("learning");
    std::fs::create_dir_all(base.join("2026-06")).unwrap();
    std::fs::write(base.join("2026-06/a.jsonl"), "{\"n\":1}\n\nnot-json\n").unwrap();
    std::fs::write(base.join("2026-06/b.txt"), "skip").unwrap();
    std::fs::write(base.join("loose.jsonl"), "{\"n\":9}").unwrap(); // skipped: not in month dir

    let cycles = read_learning_cycles(&base);
    assert_eq!(cycles.len(), 1);
    assert_eq!(cycles[0]["n"], serde_json::json!(1));
    let missing = dir.path().join("missing");
    assert!(read_learning_cycles(&missing).is_empty());
}
