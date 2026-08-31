//! S10b (quality-hardening goal 冲刺, web 批次 2): persona.rs uncovered
//! helpers + offline shop preview/download paths (cache-injected, no real
//! GitHub) + activate/restore archive-directory arms.
//!
//! - `is_agent_file` QUICKSTART/EXECUTIVE skip (125-126)
//! - `parse_agent_from_path` empty-word arm ("foo--bar" → double space, 140)
//! - `copy_dir_recursive` nested-dir branch (162-163)
//! - `extract_identity_info` splitn fallback arm (192-197)
//! - `parse_sections` unclosed frontmatter (447)
//! - `fetch_agent_content` tree-hit-but-missing-id error (296-304) and
//!   CONTENT_CACHE hit (292) — offline via TREE_CACHE/CONTENT_CACHE seeds
//! - `shop.preview`/`shop.download` with a Tools-classified section →
//!   TOOLS.md extra appended (1196-1199, 1261-1265)
//! - `ensure_initialized` archives workspace `memory/` into the default
//!   persona dir (770-775)
//! - `cmd_activate` restore path: stale archive dir cleaned before copy
//!   (956-966)

use super::tests::SHOP_TEST_LOCK;
use super::*;
use crate::ws_router::ModuleHandler;
use std::sync::Arc;

// -----------------------------------------------------------------------
// Pure helpers (direct calls)
// -----------------------------------------------------------------------

#[test]
fn is_agent_file_skips_quickstart_and_executive_prefixes() {
    assert!(is_agent_file("engineering/rust-dev.md"));
    assert!(!is_agent_file("engineering/QUICKSTART-rust.md"));
    assert!(!is_agent_file("engineering/EXECUTIVE-summary.md"));
    assert!(!is_agent_file("docs/README.md"));
}

#[test]
fn parse_agent_from_path_handles_consecutive_dashes() {
    let e = parse_agent_from_path("engineering/foo--bar.md");
    assert_eq!(e.id, "foo--bar");
    // The empty word between the dashes capitalizes to "" → double space.
    assert_eq!(e.name, "Foo  Bar");
    // "engineering" maps to a display category (开发), not passthrough.
    assert_eq!(e.category, "开发");
}

#[test]
fn copy_dir_recursive_copies_nested_dirs() {
    let src = tempfile::tempdir().unwrap();
    let dst_src = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(src.path().join("nested/deeper")).unwrap();
    std::fs::write(src.path().join("nested/deeper/file.txt"), b"x").unwrap();
    std::fs::write(src.path().join("top.md"), b"y").unwrap();

    copy_dir_recursive(src.path(), &dst_src.path().join("out")).unwrap();
    assert!(dst_src.path().join("out/nested/deeper/file.txt").is_file());
    assert!(dst_src.path().join("out/top.md").is_file());
}

#[test]
fn extract_identity_info_splitn_fallback_arm() {
    // No "：" full-width colon, no ": **" / ":**" bold patterns — only a bare
    // "key: value" colon → the splitn fallback (192-197) must still find it.
    let (name, emoji) = extract_identity_info("姓名: 小明\n");
    assert_eq!(name, "小明");
    assert_eq!(emoji, "🤖");
}

#[test]
fn extract_identity_info_full_width_colon_arm_still_wins() {
    let (name, _) = extract_identity_info("**姓名：** 小红\n");
    assert_eq!(name, "小红");
}

#[test]
fn parse_sections_unclosed_frontmatter_uses_whole_content_as_body() {
    let parsed = parse_sections("---\nname: broken\nno closing marker\n");
    // Must not panic; sections may be empty (no headings) — the load-bearing
    // assertion is that the unclosed-FM branch returns instead of slicing.
    assert!(parsed.sections.is_empty());
}

// -----------------------------------------------------------------------
// Offline shop.preview / shop.download (cache injection, SHOP_TEST_LOCK)
// -----------------------------------------------------------------------

const TOOLS_AGENT_MD: &str = "---\nname: Toolsmith\ndescription: uses tools\ntools: everything\n---\n\n# Toolsmith Agent\n\n## 🧰 Tools\n- shell\n- browser\n";

async fn seed_caches() {
    *TREE_CACHE.lock().unwrap() = Some(vec![(
        "engineering/toolsmith.md".to_string(),
        123,
    )]);
    CONTENT_CACHE
        .lock()
        .unwrap()
        .insert("toolsmith".to_string(), TOOLS_AGENT_MD.to_string());
}

fn persona_ctx(dir: &tempfile::TempDir) -> RequestContext {
    let ws = dir.path().to_string_lossy().to_string();
    let state = Arc::new(crate::api_handlers::AppState {
        auth_token: String::new(),
        session_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        workspace: Some(ws.clone()),
        home: Some(ws.clone()),
        version: "test".to_string(),
        start_time: std::time::Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new("m".to_string())),
        model_base: Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        event_hub: Arc::new(crate::events::EventHub::new()),
        running: Arc::new(std::sync::atomic::AtomicBool::new(true)),
        session_manager: Arc::new(crate::session::SessionManager::with_default_timeout()),
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
        session_id: "s".to_string(),
        chat_id: "c".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

#[tokio::test]
async fn fetch_agent_content_unknown_id_fails_offline_from_tree_cache() {
    let _g = SHOP_TEST_LOCK.lock().await;
    seed_caches().await;
    // "unknown" is not in TREE_CACHE and not in CONTENT_CACHE → tree lookup
    // finds no match → error WITHOUT any network call.
    let err = fetch_agent_content("unknown").await.unwrap_err();
    assert!(
        err.contains("not found in repository"),
        "got: {}",
        err
    );
}

#[tokio::test]
async fn shop_preview_appends_tools_extra_to_default_tools_md() {
    let _g = SHOP_TEST_LOCK.lock().await;
    seed_caches().await;
    let dir = tempfile::tempdir().unwrap();
    let ctx = persona_ctx(&dir);
    let out = PersonaHandler
        .handle_cmd(
            "shop.preview",
            Some(serde_json::json!({ "id": "toolsmith" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["name"], serde_json::json!("Toolsmith"));
    let tools = out["converted"]["TOOLS.md"].as_str().unwrap();
    assert!(
        tools.contains("工具使用"),
        "TOOLS extra section missing: {}",
        tools
    );
    assert!(
        tools.contains("## 🧰 Tools"),
        "persona tools section not carried over"
    );
}

#[tokio::test]
async fn shop_download_writes_tools_md_with_extra_section() {
    let _g = SHOP_TEST_LOCK.lock().await;
    seed_caches().await;
    let dir = tempfile::tempdir().unwrap();
    let ctx = persona_ctx(&dir);
    let out = PersonaHandler
        .handle_cmd(
            "shop.download",
            Some(serde_json::json!({ "id": "toolsmith" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["id"], serde_json::json!("toolsmith"));
    let written = std::fs::read_to_string(
        dir.path().join("personas/toolsmith/TOOLS.md"),
    )
    .unwrap();
    assert!(written.contains("工具使用"));
    assert!(written.contains("## 🧰 Tools"));
    // Other persona files written too.
    assert!(dir.path().join("personas/toolsmith/IDENTITY.md").is_file());
    assert!(dir.path().join("personas/toolsmith/SOUL.md").is_file());
}

// -----------------------------------------------------------------------
// ensure_initialized / cmd_activate archive-directory arms
// -----------------------------------------------------------------------

#[test]
fn ensure_initialized_archives_workspace_memory_dir_into_default() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    std::fs::write(ws.join("IDENTITY.md"), "id").unwrap();
    std::fs::create_dir_all(ws.join("memory")).unwrap();
    std::fs::write(ws.join("memory/note.md"), "note").unwrap();

    PersonaHandler
        .cmd_list(&ws.to_string_lossy())
        .expect("cmd_list triggers ensure_initialized");

    let archived = ws.join("personas/default/memory/note.md");
    assert!(archived.is_file(), "memory/ must be archived into default");
    assert_eq!(std::fs::read_to_string(archived).unwrap(), "note");
}

#[test]
fn cmd_activate_restore_replaces_stale_archive_dir_contents() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    // Workspace root: current persona files + memory/ tree.
    std::fs::create_dir_all(ws.join("memory")).unwrap();
    std::fs::write(ws.join("memory/keep.md"), "keep").unwrap();
    std::fs::write(ws.join("IDENTITY.md"), "custom-identity").unwrap();
    // personas/custom: active persona with a STALE archived memory dir.
    let custom = ws.join("personas/custom");
    std::fs::create_dir_all(custom.join("memory")).unwrap();
    std::fs::write(custom.join("memory/stale.md"), "stale").unwrap();
    // personas/default: the restore target with its own files.
    let default = ws.join("personas/default");
    std::fs::create_dir_all(&default).unwrap();
    std::fs::write(default.join("IDENTITY.md"), "default-identity").unwrap();
    // _active.json → custom.
    std::fs::write(
        ws.join("personas/_active.json"),
        r#"{ "name": "custom" }"#,
    )
    .unwrap();

    PersonaHandler
        .cmd_activate(&ws.to_string_lossy(), "default")
        .expect("activate default (restore)");

    // Archive dir cleaned + re-copied from the workspace tree.
    assert!(custom.join("memory/keep.md").is_file());
    assert!(
        !custom.join("memory/stale.md").exists(),
        "stale archive content must be removed before copy"
    );
    // Workspace now runs the default persona.
    assert_eq!(
        std::fs::read_to_string(ws.join("IDENTITY.md")).unwrap(),
        "default-identity"
    );
}
