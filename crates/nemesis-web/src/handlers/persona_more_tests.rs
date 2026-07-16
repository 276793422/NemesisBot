//! Handler-level tests for `persona::PersonaHandler`.
//!
//! Complements `persona_extra_tests.rs` by exercising additional handler paths:
//! - `file.save` non-active persona (root not synced)
//! - `file.save` creates parent dirs
//! - `file.get` for non-existent persona dir
//! - `list` with broken PERSONA.json (skipped)
//! - `list` empty workspace (only default after init)
//! - `current` after switching persona
//! - `activate` errors when active.json missing
//! - `remove` switches to default if removing active

#![cfg(test)]

use crate::handlers::persona::PersonaHandler;
use crate::ws_router::ModuleHandler;

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::Arc;
use std::time::Instant;

use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::RequestContext;

use nemesis_services::bot_service::{AgentLoopService, LifecycleService};

struct MockAgentService;
impl LifecycleService for MockAgentService {
    fn start(&self) -> Result<(), String> {
        Ok(())
    }
    fn stop(&self) -> Result<(), String> {
        Ok(())
    }
    fn is_running(&self) -> bool {
        true
    }
}
impl AgentLoopService for MockAgentService {}

// ---------------------------------------------------------------------------
// Test infra
// ---------------------------------------------------------------------------

fn make_ctx(dir: &tempfile::TempDir) -> RequestContext {
    let ws = dir.path().to_string_lossy().to_string();
    make_ctx_inner(&ws, None)
}

fn make_ctx_with_agent(dir: &tempfile::TempDir) -> RequestContext {
    let ws = dir.path().to_string_lossy().to_string();
    make_ctx_inner(
        &ws,
        Some(Arc::new(MockAgentService) as Arc<dyn AgentLoopService>),
    )
}

fn make_ctx_inner(ws: &str, agent: Option<Arc<dyn AgentLoopService>>) -> RequestContext {
    let state = Arc::new(AppState {
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
        agent_service: agent,
        data_store: None,
        memory_manager: None,
        forge: None,
        agent_loop: Arc::new(parking_lot::RwLock::new(None)),
        cluster: None,
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: None,
        chat_secret_store: std::sync::Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::in_memory()),
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        internal_cmd_tx: None,
        estop: None,
        cron: None,
    });
    RequestContext {
        session_id: "test-session".to_string(),
        chat_id: "test-chat".to_string(),
        workspace: Some(ws.to_string()),
        home: Some(ws.to_string()),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

fn seed_default_persona(workspace: &Path) {
    std::fs::write(
        workspace.join("IDENTITY.md"),
        "# Test Identity\n\n- 姓名：Alice\n- 表情符号：🐱\n",
    )
    .unwrap();
    std::fs::write(workspace.join("SOUL.md"), "# Test Soul").unwrap();
    std::fs::write(workspace.join("AGENT.md"), "# Test Agent").unwrap();
    std::fs::write(workspace.join("TOOLS.md"), "# Test Tools").unwrap();
}

fn create_persona_dir(
    workspace: &Path,
    dir_name: &str,
    name: &str,
    emoji: &str,
    description: &str,
) {
    let persona_dir = workspace.join("personas").join(dir_name);
    std::fs::create_dir_all(&persona_dir).unwrap();
    std::fs::write(persona_dir.join("IDENTITY.md"), format!("# {}\n", name)).unwrap();
    std::fs::write(persona_dir.join("SOUL.md"), "# Soul").unwrap();
    std::fs::write(persona_dir.join("AGENT.md"), "# Agent").unwrap();
    std::fs::write(persona_dir.join("TOOLS.md"), "# Tools").unwrap();
    let pj = serde_json::json!({
        "name": name,
        "emoji": emoji,
        "description": description,
    });
    std::fs::write(
        persona_dir.join("PERSONA.json"),
        serde_json::to_string_pretty(&pj).unwrap(),
    )
    .unwrap();
}

fn set_active(workspace: &Path, name: &str) {
    let personas_dir = workspace.join("personas");
    std::fs::create_dir_all(&personas_dir).unwrap();
    let active = serde_json::json!({ "name": name });
    std::fs::write(
        personas_dir.join("_active.json"),
        serde_json::to_string_pretty(&active).unwrap(),
    )
    .unwrap();
}

async fn init_workspace(dir: &tempfile::TempDir) -> RequestContext {
    seed_default_persona(dir.path());
    let ctx = make_ctx(dir);
    let h = PersonaHandler::new();
    let _ = h.handle_cmd("current", None, &ctx).await.unwrap();
    ctx
}

// ---------------------------------------------------------------------------
// file.save — non-active persona does not sync to root
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_file_save_non_active_no_root_sync() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let ctx = init_workspace(&dir).await;
    create_persona_dir(ws, "alt", "Alt", "🐧", "alt persona");

    // Capture original root IDENTITY.md content.
    let original_root = std::fs::read_to_string(ws.join("IDENTITY.md")).unwrap();

    let h = PersonaHandler::new();
    let data = serde_json::json!({
        "name": "alt",
        "file": "IDENTITY.md",
        "content": "# Alt Modified",
    });
    let r = h.handle_cmd("file.save", Some(data), &ctx).await.unwrap().unwrap();
    assert!(r["saved"].as_bool().unwrap());

    // alt is not active → root should be unchanged.
    let root_after = std::fs::read_to_string(ws.join("IDENTITY.md")).unwrap();
    assert_eq!(root_after, original_root);

    // But the alt persona's IDENTITY.md should be modified.
    let alt_id = std::fs::read_to_string(ws.join("personas/alt/IDENTITY.md")).unwrap();
    assert!(alt_id.contains("Alt Modified"));
}

// ---------------------------------------------------------------------------
// file.save — creates parent dirs
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_file_save_creates_persona_dir_parent() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let ctx = init_workspace(&dir).await;

    // Save a file to a non-active persona that doesn't exist yet — handler
    // creates the personas/<name>/ directory and writes inside it.
    let h = PersonaHandler::new();
    let data = serde_json::json!({
        "name": "brandnew",
        "file": "IDENTITY.md",
        "content": "fresh content",
    });
    let r = h.handle_cmd("file.save", Some(data), &ctx).await.unwrap().unwrap();
    assert!(r["saved"].as_bool().unwrap());

    // The persona dir and file should exist.
    assert!(ws.join("personas/brandnew/IDENTITY.md").exists());

    let _ = ws; // suppress unused warning
}

// ---------------------------------------------------------------------------
// file.get — non-existent persona dir returns empty
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_file_get_unknown_persona_returns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = init_workspace(&dir).await;

    let h = PersonaHandler::new();
    let data = serde_json::json!({ "name": "ghost", "file": "IDENTITY.md" });
    let r = h.handle_cmd("file.get", Some(data), &ctx).await.unwrap().unwrap();
    assert_eq!(r["content"], "");
    assert_eq!(r["name"], "IDENTITY.md");
}

// ---------------------------------------------------------------------------
// list — broken PERSONA.json skips the persona
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_list_skips_persona_with_invalid_json() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let ctx = init_workspace(&dir).await;
    // Create a persona dir but with broken PERSONA.json.
    let broken_dir = ws.join("personas/broken");
    std::fs::create_dir_all(&broken_dir).unwrap();
    std::fs::write(broken_dir.join("PERSONA.json"), "{ broken").unwrap();

    let h = PersonaHandler::new();
    let r = h.handle_cmd("list", None, &ctx).await.unwrap().unwrap();
    let arr = r["personas"].as_array().unwrap();
    // Only "default" should be listed; broken is skipped silently.
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["dir"], "default");
}

// ---------------------------------------------------------------------------
// list — file subset per persona
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_list_partial_files() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let ctx = init_workspace(&dir).await;
    // Create a persona with only IDENTITY.md.
    let persona_dir = ws.join("personas/partial");
    std::fs::create_dir_all(&persona_dir).unwrap();
    std::fs::write(persona_dir.join("IDENTITY.md"), "# Partial").unwrap();
    let pj = serde_json::json!({ "name": "Partial", "emoji": "🤖", "description": "" });
    std::fs::write(
        persona_dir.join("PERSONA.json"),
        serde_json::to_string_pretty(&pj).unwrap(),
    )
    .unwrap();

    let h = PersonaHandler::new();
    let r = h.handle_cmd("list", None, &ctx).await.unwrap().unwrap();
    let arr = r["personas"].as_array().unwrap();
    let partial = arr
        .iter()
        .find(|p| p["dir"] == "partial")
        .expect("partial persona must be listed");
    let files = partial["files"].as_array().unwrap();
    // Only IDENTITY.md exists.
    assert_eq!(files.len(), 1);
    assert_eq!(files[0], "IDENTITY.md");
}

// ---------------------------------------------------------------------------
// list — non-directory entries in personas/ skipped
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_list_skips_stray_files() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let ctx = init_workspace(&dir).await;
    // Stray file directly under personas/.
    std::fs::write(ws.join("personas/stray.txt"), "ignore").unwrap();

    let h = PersonaHandler::new();
    let r = h.handle_cmd("list", None, &ctx).await.unwrap().unwrap();
    let arr = r["personas"].as_array().unwrap();
    // Only default persona; stray.txt skipped.
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["dir"], "default");
}

// ---------------------------------------------------------------------------
// current — when no IDENTITY.md exists in workspace, defaults are used
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_current_no_identity_uses_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let h = PersonaHandler::new();
    let r = h.handle_cmd("current", None, &ctx).await.unwrap().unwrap();
    assert_eq!(r["active_dir"], "default");
    // No IDENTITY.md → default name.
    assert_eq!(r["name"], "default");
}

// ---------------------------------------------------------------------------
// current — workspace initialized only with SOUL.md (no IDENTITY.md)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_current_with_only_soul_archives_partial() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    std::fs::write(ws.join("SOUL.md"), "# Just Soul").unwrap();

    let ctx = make_ctx(&dir);
    let h = PersonaHandler::new();
    let r = h.handle_cmd("current", None, &ctx).await.unwrap().unwrap();
    assert_eq!(r["active_dir"], "default");

    // SOUL.md was archived into default/.
    let archived = std::fs::read_to_string(ws.join("personas/default/SOUL.md")).unwrap();
    assert!(archived.contains("Just Soul"));
}

// ---------------------------------------------------------------------------
// activate — directory in archive that doesn't exist (HEARTBEAT.md)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_activate_with_heartbeat_archived() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let _ctx = init_workspace(&dir).await;
    // Add HEARTBEAT.md to workspace root.
    std::fs::write(ws.join("HEARTBEAT.md"), "# Heartbeat content").unwrap();
    create_persona_dir(ws, "hb", "HB", "💚", "hb persona");

    let ctx = make_ctx_with_agent(&dir);
    let h = PersonaHandler::new();
    let data = serde_json::json!({ "name": "hb" });
    let r = h.handle_cmd("activate", Some(data), &ctx).await.unwrap().unwrap();
    assert!(r["activated"].as_bool().unwrap());

    // HEARTBEAT.md should be archived into default/ (current was default).
    let archived = std::fs::read_to_string(ws.join("personas/default/HEARTBEAT.md")).unwrap();
    assert!(archived.contains("Heartbeat content"));
}

// ---------------------------------------------------------------------------
// activate — memory directory archived and restored
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_activate_archives_and_restores_memory_dir() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let _ctx = init_workspace(&dir).await;
    // Create memory/ with a file.
    std::fs::create_dir_all(ws.join("memory")).unwrap();
    std::fs::write(ws.join("memory/note.md"), "# Memory").unwrap();

    // Create target persona with its own memory content.
    let target_dir = ws.join("personas/dev");
    std::fs::create_dir_all(target_dir.join("memory")).unwrap();
    std::fs::write(target_dir.join("memory/dev_note.md"), "# Dev Memory").unwrap();
    std::fs::write(target_dir.join("PERSONA.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "name": "Dev", "emoji": "🛠", "description": "dev"
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(target_dir.join("IDENTITY.md"), "# Dev").unwrap();

    let ctx = make_ctx_with_agent(&dir);
    let h = PersonaHandler::new();
    let data = serde_json::json!({ "name": "dev" });
    let r = h.handle_cmd("activate", Some(data), &ctx).await.unwrap().unwrap();
    assert!(r["activated"].as_bool().unwrap());

    // Original memory/ archived into default/.
    let archived = std::fs::read_to_string(ws.join("personas/default/memory/note.md")).unwrap();
    assert!(archived.contains("Memory"));

    // After activation, memory/ in workspace root has dev content + default MEMORY.md.
    assert!(ws.join("memory/dev_note.md").exists());
}

// ---------------------------------------------------------------------------
// activate — default MEMORY.md created for new persona without memory/
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_activate_creates_default_memory_md() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let _ctx = init_workspace(&dir).await;
    create_persona_dir(ws, "fresh", "Fresh", "🌱", "fresh persona");

    let ctx = make_ctx_with_agent(&dir);
    let h = PersonaHandler::new();
    let data = serde_json::json!({ "name": "fresh" });
    let r = h.handle_cmd("activate", Some(data), &ctx).await.unwrap().unwrap();
    assert!(r["activated"].as_bool().unwrap());

    // memory/MEMORY.md should be auto-created.
    let mem_path = ws.join("memory/MEMORY.md");
    assert!(mem_path.exists());
    let mem = std::fs::read_to_string(&mem_path).unwrap();
    assert!(mem.contains("长期记忆"));
}

// ---------------------------------------------------------------------------
// restore — restoring back from non-default persona
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_restore_switches_back_to_default() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let _ctx = init_workspace(&dir).await;
    create_persona_dir(ws, "temp", "Temp", "⏳", "temp persona");

    let ctx_agent = make_ctx_with_agent(&dir);
    let h = PersonaHandler::new();
    let data = serde_json::json!({ "name": "temp" });
    let _ = h.handle_cmd("activate", Some(data), &ctx_agent).await.unwrap();

    // Now restore to default.
    let r = h.handle_cmd("restore", None, &ctx_agent).await.unwrap().unwrap();
    assert!(r["activated"].as_bool().unwrap());
    assert_eq!(r["name"], "default");

    // _active.json should reflect default.
    let active: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(ws.join("personas/_active.json")).unwrap())
            .unwrap();
    assert_eq!(active["name"], "default");
}

// ---------------------------------------------------------------------------
// remove — removing active persona switches to default
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_remove_active_switches_to_default() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let _ctx = init_workspace(&dir).await;
    create_persona_dir(ws, "go", "Go", "🟢", "go persona");

    // First activate "go".
    {
        let ctx_agent = make_ctx_with_agent(&dir);
        let h = PersonaHandler::new();
        let data = serde_json::json!({ "name": "go" });
        let _ = h.handle_cmd("activate", Some(data), &ctx_agent).await.unwrap();
    }

    // Now remove "go" while it's active. cmd_remove internally calls cmd_activate("default").
    // Since cmd_remove doesn't call restart_agent itself, no agent_service needed.
    let ctx = make_ctx(&dir);
    let h = PersonaHandler::new();
    let data = serde_json::json!({ "name": "go" });
    let r = h.handle_cmd("remove", Some(data), &ctx).await.unwrap().unwrap();
    assert!(r["removed"].as_bool().unwrap());

    // Active should be default now.
    let active: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(ws.join("personas/_active.json")).unwrap())
            .unwrap();
    assert_eq!(active["name"], "default");

    // Persona dir removed.
    assert!(!ws.join("personas/go").exists());
}

// ---------------------------------------------------------------------------
// remove — non-active persona
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_remove_non_active_persona() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let ctx = init_workspace(&dir).await;
    create_persona_dir(ws, "extra", "Extra", "✨", "extra persona");
    set_active(ws, "default");

    let h = PersonaHandler::new();
    let data = serde_json::json!({ "name": "extra" });
    let r = h.handle_cmd("remove", Some(data), &ctx).await.unwrap().unwrap();
    assert!(r["removed"].as_bool().unwrap());

    // Active should still be default.
    let active: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(ws.join("personas/_active.json")).unwrap())
            .unwrap();
    assert_eq!(active["name"], "default");
}

// ---------------------------------------------------------------------------
// file.get — corrupted PERSONA.json handled
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_file_get_returns_existing_content() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let ctx = init_workspace(&dir).await;
    // Write AGENT.md into default.
    std::fs::write(ws.join("personas/default/AGENT.md"), "# Custom Agent").unwrap();

    let h = PersonaHandler::new();
    let data = serde_json::json!({ "name": "default", "file": "AGENT.md" });
    let r = h.handle_cmd("file.get", Some(data), &ctx).await.unwrap().unwrap();
    assert_eq!(r["content"], "# Custom Agent");
}

// ---------------------------------------------------------------------------
// file.save — then file.get verifies content
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_file_save_then_get_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = init_workspace(&dir).await;

    let h = PersonaHandler::new();
    let save_data = serde_json::json!({
        "name": "default",
        "file": "SOUL.md",
        "content": "# Updated Soul\n\nNew soul rules.",
    });
    let r = h
        .handle_cmd("file.save", Some(save_data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(r["saved"].as_bool().unwrap());

    let get_data = serde_json::json!({ "name": "default", "file": "SOUL.md" });
    let r = h
        .handle_cmd("file.get", Some(get_data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["content"], "# Updated Soul\n\nNew soul rules.");
}

// ---------------------------------------------------------------------------
// current — multiple personas (default + extra)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_current_after_extra_persona_added() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let ctx = init_workspace(&dir).await;
    create_persona_dir(ws, "alt", "Alt", "🎯", "alt persona");
    set_active(ws, "alt");

    let h = PersonaHandler::new();
    let r = h.handle_cmd("current", None, &ctx).await.unwrap().unwrap();
    assert_eq!(r["active_dir"], "alt");
    assert_eq!(r["name"], "Alt");
    assert_eq!(r["emoji"], "🎯");
}

// ---------------------------------------------------------------------------
// shop.browse / shop.search / shop.preview / shop.download missing workspace
// ---------------------------------------------------------------------------

fn make_ctx_no_workspace() -> RequestContext {
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: None,
        home: None,
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
        chat_secret_store: std::sync::Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::in_memory()),
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        internal_cmd_tx: None,
        estop: None,
        cron: None,
    });
    RequestContext {
        session_id: "test-session".to_string(),
        chat_id: "test-chat".to_string(),
        workspace: None,
        home: None,
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

#[tokio::test]
async fn test_shop_browse_missing_workspace() {
    let ctx = make_ctx_no_workspace();
    let h = PersonaHandler::new();
    let err = h.handle_cmd("shop.browse", None, &ctx).await.unwrap_err();
    assert!(err.contains("workspace not configured"));
}

#[tokio::test]
async fn test_shop_search_missing_workspace() {
    let ctx = make_ctx_no_workspace();
    let h = PersonaHandler::new();
    let err = h
        .handle_cmd("shop.search", Some(serde_json::json!({"query": "x"})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("workspace not configured"));
}

#[tokio::test]
async fn test_shop_preview_missing_workspace() {
    let ctx = make_ctx_no_workspace();
    let h = PersonaHandler::new();
    let err = h
        .handle_cmd("shop.preview", Some(serde_json::json!({"id": "x"})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("workspace not configured"));
}

#[tokio::test]
async fn test_shop_download_missing_workspace() {
    let ctx = make_ctx_no_workspace();
    let h = PersonaHandler::new();
    let err = h
        .handle_cmd("shop.download", Some(serde_json::json!({"id": "x"})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("workspace not configured"));
}

// ---------------------------------------------------------------------------
// shop.browse with division filter (no network, just param parsing)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_shop_browse_with_division_filter() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = PersonaHandler::new();
    let data = serde_json::json!({ "division": "开发" });
    // Without network this will error, but we just verify no panic on param parsing.
    let _ = h.handle_cmd("shop.browse", Some(data), &ctx).await;
}

#[tokio::test]
async fn test_shop_search_with_query() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = PersonaHandler::new();
    let data = serde_json::json!({ "query": "engineer" });
    let _ = h.handle_cmd("shop.search", Some(data), &ctx).await;
}

// ---------------------------------------------------------------------------
// Unknown command on persona
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_persona_unknown_command_with_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = PersonaHandler::new();
    let err = h.handle_cmd("nope", None, &ctx).await.unwrap_err();
    assert_eq!(err, "unknown command: persona.nope");
}

// ---------------------------------------------------------------------------
// shop.refresh happy path (no network needed)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_persona_shop_refresh_clears_caches() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = PersonaHandler::new();
    let r = h.handle_cmd("shop.refresh", None, &ctx).await.unwrap().unwrap();
    assert!(r["refreshed"].as_bool().unwrap());

    // Calling again should still succeed (idempotent).
    let r = h.handle_cmd("shop.refresh", None, &ctx).await.unwrap().unwrap();
    assert!(r["refreshed"].as_bool().unwrap());
}
