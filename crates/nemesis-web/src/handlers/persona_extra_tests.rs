//! Comprehensive tests for `PersonaHandler` (persona.rs).
//!
//! Covers `handle_cmd` for all local commands plus path traversal / unknown
//! command / missing workspace / missing data / nonexistent file error paths.
//! Shop commands (browse/search/preview/download) hit the live GitHub API and
//! are therefore omitted from this unit-test file.

#![cfg(test)]

use super::persona::PersonaHandler;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

use nemesis_services::bot_service::{AgentLoopService, LifecycleService};

// Mock agent service — start/stop return Ok

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

// Test infra helpers (mirror tests.rs)

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
        quic_relay: None,
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: None,
        chat_secret_store: std::sync::Arc::new(
            nemesis_workflow::chat_secrets::ChatSecretStore::in_memory(),
        ),
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        internal_cmd_tx: None,
        estop: None,
        background_registry: None,
        signature_verify: None,
        skills_install_gate: None,
        session_stats: None,
        cron: None,
        board: None,
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
        quic_relay: None,
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: None,
        chat_secret_store: std::sync::Arc::new(
            nemesis_workflow::chat_secrets::ChatSecretStore::in_memory(),
        ),
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        internal_cmd_tx: None,
        estop: None,
        background_registry: None,
        signature_verify: None,
        skills_install_gate: None,
        session_stats: None,
        cron: None,
        board: None,
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

/// Seed a fresh workspace with the four required persona files plus an
/// IDENTITY.md whose `姓名`/`表情符号` fields are recognized by
/// `extract_identity_info`.
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

/// Create a persona directory with the four standard files plus
/// PERSONA.json describing it.
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

/// Mark a persona as active by writing `_active.json`.
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

// module name

#[test]
fn test_module_name() {
    let h = PersonaHandler::new();
    assert_eq!(h.module_name(), "persona");
}

// current

#[tokio::test]
async fn test_current_initial_migration() {
    let dir = tempfile::tempdir().unwrap();
    seed_default_persona(dir.path());
    let ctx = make_ctx(&dir);
    let h = PersonaHandler::new();

    let r = h.handle_cmd("current", None, &ctx).await.unwrap().unwrap();
    assert_eq!(r["active_dir"], "default");
    assert_eq!(r["name"], "Alice");
    assert_eq!(r["emoji"], "🐱");
    let files = r["files"].as_array().unwrap();
    assert_eq!(files.len(), 4);
    assert!(files.iter().any(|f| f == "IDENTITY.md"));
    assert!(files.iter().any(|f| f == "SOUL.md"));
    assert!(files.iter().any(|f| f == "AGENT.md"));
    assert!(files.iter().any(|f| f == "TOOLS.md"));
}

#[tokio::test]
async fn test_current_missing_workspace() {
    let ctx = make_ctx_no_workspace();
    let h = PersonaHandler::new();
    let err = h.handle_cmd("current", None, &ctx).await.unwrap_err();
    assert!(err.contains("workspace not configured"));
}

// list

#[tokio::test]
async fn test_list_single_default() {
    let dir = tempfile::tempdir().unwrap();
    seed_default_persona(dir.path());
    let ctx = make_ctx(&dir);
    let h = PersonaHandler::new();

    let r = h.handle_cmd("list", None, &ctx).await.unwrap().unwrap();
    assert_eq!(r["active"], "default");
    let arr = r["personas"].as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["dir"], "default");
    assert!(arr[0]["is_default"].as_bool().unwrap());
    assert!(arr[0]["is_active"].as_bool().unwrap());
}

#[tokio::test]
async fn test_list_multiple_personas_sorted() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    // First run ensure_initialized via `current` so personas/ exists.
    seed_default_persona(ws);
    {
        let ctx = make_ctx(&dir);
        let h = PersonaHandler::new();
        let _ = h.handle_cmd("current", None, &ctx).await.unwrap();
    }
    create_persona_dir(ws, "zoe", "Zoe", "🦓", "zoe desc");
    create_persona_dir(ws, "alice", "Alice", "🐰", "alice desc");
    set_active(ws, "default");

    let ctx = make_ctx(&dir);
    let h = PersonaHandler::new();
    let r = h.handle_cmd("list", None, &ctx).await.unwrap().unwrap();
    let arr = r["personas"].as_array().unwrap();
    assert_eq!(arr.len(), 3);
    // default must come first regardless of name
    assert_eq!(arr[0]["dir"], "default");
    // remaining sorted by name
    assert_eq!(arr[1]["dir"], "alice");
    assert_eq!(arr[2]["dir"], "zoe");
}

#[tokio::test]
async fn test_list_missing_workspace() {
    let ctx = make_ctx_no_workspace();
    let h = PersonaHandler::new();
    let err = h.handle_cmd("list", None, &ctx).await.unwrap_err();
    assert!(err.contains("workspace not configured"));
}

// file.get

#[tokio::test]
async fn test_file_get_returns_content() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    seed_default_persona(ws);
    // ensure_initialized to create personas/default/
    {
        let ctx = make_ctx(&dir);
        let h = PersonaHandler::new();
        let _ = h.handle_cmd("current", None, &ctx).await.unwrap();
    }
    let ctx = make_ctx(&dir);
    let h = PersonaHandler::new();
    let data = serde_json::json!({ "name": "default", "file": "IDENTITY.md" });
    let r = h
        .handle_cmd("file.get", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["name"], "IDENTITY.md");
    let content = r["content"].as_str().unwrap();
    assert!(content.contains("Alice"));
}

#[tokio::test]
async fn test_file_get_missing_returns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    seed_default_persona(ws);
    {
        let ctx = make_ctx(&dir);
        let h = PersonaHandler::new();
        let _ = h.handle_cmd("current", None, &ctx).await.unwrap();
    }
    let ctx = make_ctx(&dir);
    let h = PersonaHandler::new();
    // 白名单外的文件名一律 loud 拒绝（file 白名单闸）；
    // 「缺失 → 空内容」语义收敛到白名单内但不存在的文件上验证。
    let data = serde_json::json!({ "name": "default", "file": "NONEXISTENT.md" });
    let err = h
        .handle_cmd("file.get", Some(data), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("不是可编辑的人格文件"), "got: {err}");

    let data = serde_json::json!({ "name": "default", "file": "TOOLS.md" });
    std::fs::remove_file(dir.path().join("personas/default/TOOLS.md")).unwrap();
    let r = h
        .handle_cmd("file.get", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["content"], "");
}

#[tokio::test]
async fn test_file_get_missing_data() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = PersonaHandler::new();
    let err = h.handle_cmd("file.get", None, &ctx).await.unwrap_err();
    assert_eq!(err, "missing data");
}

#[tokio::test]
async fn test_file_get_missing_field_name() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = PersonaHandler::new();
    let data = serde_json::json!({ "file": "IDENTITY.md" });
    let err = h
        .handle_cmd("file.get", Some(data), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing field: name");
}

#[tokio::test]
async fn test_file_get_missing_field_file() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = PersonaHandler::new();
    let data = serde_json::json!({ "name": "default" });
    let err = h
        .handle_cmd("file.get", Some(data), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing field: file");
}

#[tokio::test]
async fn test_file_get_path_traversal() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = PersonaHandler::new();
    let data = serde_json::json!({
        "name": "default",
        "file": "../../etc/passwd",
    });
    let err = h
        .handle_cmd("file.get", Some(data), &ctx)
        .await
        .unwrap_err();
    // 白名单闸先于 resolve_path（敌意文件名在触碰
    // 文件系统之前拒绝，比旧的「.. 启发式 canonicalize 拒绝」更早更强）。
    assert!(err.contains("不是可编辑的人格文件"), "got: {}", err);
}

#[tokio::test]
async fn test_file_get_missing_workspace() {
    let ctx = make_ctx_no_workspace();
    let h = PersonaHandler::new();
    let data = serde_json::json!({ "name": "default", "file": "IDENTITY.md" });
    let err = h
        .handle_cmd("file.get", Some(data), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("workspace not configured"));
}

// file.save

#[tokio::test]
async fn test_file_save_writes_and_reads_back() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    seed_default_persona(ws);
    {
        let ctx = make_ctx(&dir);
        let h = PersonaHandler::new();
        let _ = h.handle_cmd("current", None, &ctx).await.unwrap();
    }
    let ctx = make_ctx(&dir);
    let h = PersonaHandler::new();
    let data = serde_json::json!({
        "name": "default",
        "file": "IDENTITY.md",
        "content": "# Updated by test",
    });
    let r = h
        .handle_cmd("file.save", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(r["saved"].as_bool().unwrap());

    // The active persona is `default`, so the file should be synced to root.
    let root_id = std::fs::read_to_string(ws.join("IDENTITY.md")).unwrap();
    assert!(root_id.contains("Updated by test"));
}

#[tokio::test]
async fn test_file_save_missing_data() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = PersonaHandler::new();
    let err = h.handle_cmd("file.save", None, &ctx).await.unwrap_err();
    assert_eq!(err, "missing data");
}

#[tokio::test]
async fn test_file_save_missing_field_content() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = PersonaHandler::new();
    let data = serde_json::json!({ "name": "default", "file": "IDENTITY.md" });
    let err = h
        .handle_cmd("file.save", Some(data), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing field: content");
}

#[tokio::test]
async fn test_file_save_path_traversal() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = PersonaHandler::new();
    let data = serde_json::json!({
        "name": "..",
        "file": "IDENTITY.md",
        "content": "evil",
    });
    let err = h
        .handle_cmd("file.save", Some(data), &ctx)
        .await
        .unwrap_err();
    // name 闸（slug 验证器）先于 resolve_path——旧路径下
    // 该用例靠「目标不存在 + 相对路径含..」的 canonicalize 启发式兜住；
    // 目标存在时 resolve_path 实际放行（personas/../ = workspace 根），
    // 等于越权写根文件。现在 name='..' 在触碰文件系统之前 loud 拒绝。
    assert!(err.contains("invalid"), "got: {}", err);
}

#[tokio::test]
async fn test_file_save_missing_workspace() {
    let ctx = make_ctx_no_workspace();
    let h = PersonaHandler::new();
    let data = serde_json::json!({
        "name": "default",
        "file": "IDENTITY.md",
        "content": "x",
    });
    let err = h
        .handle_cmd("file.save", Some(data), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("workspace not configured"));
}

// remove

#[tokio::test]
async fn test_remove_persona() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    seed_default_persona(ws);
    {
        let ctx = make_ctx(&dir);
        let h = PersonaHandler::new();
        let _ = h.handle_cmd("current", None, &ctx).await.unwrap();
    }
    create_persona_dir(ws, "dev", "Dev", "🛠", "dev persona");

    let ctx = make_ctx(&dir);
    let h = PersonaHandler::new();
    let data = serde_json::json!({ "name": "dev" });
    let r = h
        .handle_cmd("remove", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(r["removed"].as_bool().unwrap());
    assert!(!ws.join("personas").join("dev").exists());
}

#[tokio::test]
async fn test_remove_default_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    seed_default_persona(ws);
    {
        let ctx = make_ctx(&dir);
        let h = PersonaHandler::new();
        let _ = h.handle_cmd("current", None, &ctx).await.unwrap();
    }
    let ctx = make_ctx(&dir);
    let h = PersonaHandler::new();
    let data = serde_json::json!({ "name": "default" });
    let err = h.handle_cmd("remove", Some(data), &ctx).await.unwrap_err();
    assert_eq!(err, "cannot remove default persona");
}

#[tokio::test]
async fn test_remove_nonexistent() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    seed_default_persona(ws);
    {
        let ctx = make_ctx(&dir);
        let h = PersonaHandler::new();
        let _ = h.handle_cmd("current", None, &ctx).await.unwrap();
    }
    let ctx = make_ctx(&dir);
    let h = PersonaHandler::new();
    let data = serde_json::json!({ "name": "ghost" });
    let err = h.handle_cmd("remove", Some(data), &ctx).await.unwrap_err();
    assert!(err.contains("persona 'ghost' not found"), "got: {}", err);
}

#[tokio::test]
async fn test_remove_missing_data() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = PersonaHandler::new();
    let err = h.handle_cmd("remove", None, &ctx).await.unwrap_err();
    assert_eq!(err, "missing data");
}

#[tokio::test]
async fn test_remove_missing_workspace() {
    let ctx = make_ctx_no_workspace();
    let h = PersonaHandler::new();
    let data = serde_json::json!({ "name": "dev" });
    let err = h.handle_cmd("remove", Some(data), &ctx).await.unwrap_err();
    assert!(err.contains("workspace not configured"));
}

// activate / restore (require agent_service)

#[tokio::test]
async fn test_activate_switches_persona() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    seed_default_persona(ws);
    {
        let ctx = make_ctx(&dir);
        let h = PersonaHandler::new();
        let _ = h.handle_cmd("current", None, &ctx).await.unwrap();
    }
    create_persona_dir(ws, "dev", "Dev", "🛠", "dev persona");
    // Switch root files to something recognizable so we can detect a switch.
    std::fs::write(ws.join("IDENTITY.md"), "# I am DEFAULT").unwrap();

    let ctx = make_ctx_with_agent(&dir);
    let h = PersonaHandler::new();
    let data = serde_json::json!({ "name": "dev" });
    let r = h
        .handle_cmd("activate", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(r["activated"].as_bool().unwrap());
    assert_eq!(r["name"], "dev");

    // Root IDENTITY.md should now be the dev persona's content.
    let new_root = std::fs::read_to_string(ws.join("IDENTITY.md")).unwrap();
    assert!(new_root.contains("Dev"), "got: {}", new_root);

    // _active.json reflects the new persona.
    let active: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(ws.join("personas/_active.json")).unwrap())
            .unwrap();
    assert_eq!(active["name"], "dev");
}

#[tokio::test]
async fn test_activate_same_persona_noop_archive() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    seed_default_persona(ws);
    {
        let ctx = make_ctx(&dir);
        let h = PersonaHandler::new();
        let _ = h.handle_cmd("current", None, &ctx).await.unwrap();
    }
    let ctx = make_ctx_with_agent(&dir);
    let h = PersonaHandler::new();
    // activating current persona is a valid no-op for the archive step
    let data = serde_json::json!({ "name": "default" });
    let r = h
        .handle_cmd("activate", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(r["activated"].as_bool().unwrap());
}

#[tokio::test]
async fn test_activate_unknown_persona() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    seed_default_persona(ws);
    {
        let ctx = make_ctx(&dir);
        let h = PersonaHandler::new();
        let _ = h.handle_cmd("current", None, &ctx).await.unwrap();
    }
    let ctx = make_ctx_with_agent(&dir);
    let h = PersonaHandler::new();
    let data = serde_json::json!({ "name": "ghost" });
    let err = h
        .handle_cmd("activate", Some(data), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("persona 'ghost' not found"), "got: {}", err);
}

#[tokio::test]
async fn test_activate_missing_data() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = PersonaHandler::new();
    let err = h.handle_cmd("activate", None, &ctx).await.unwrap_err();
    assert_eq!(err, "missing data");
}

#[tokio::test]
async fn test_activate_missing_field_name() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = PersonaHandler::new();
    let data = serde_json::json!({});
    let err = h
        .handle_cmd("activate", Some(data), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing field: name");
}

#[tokio::test]
async fn test_activate_without_agent_service_errors_after_switch() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    seed_default_persona(ws);
    {
        let ctx = make_ctx(&dir);
        let h = PersonaHandler::new();
        let _ = h.handle_cmd("current", None, &ctx).await.unwrap();
    }
    create_persona_dir(ws, "dev", "Dev", "🛠", "dev persona");

    let ctx = make_ctx(&dir); // No agent_service injected
    let h = PersonaHandler::new();
    let data = serde_json::json!({ "name": "dev" });
    let err = h
        .handle_cmd("activate", Some(data), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("Agent not available"), "got: {}", err);
    // But the file switch still happened (restart_agent is the last step).
    let root = std::fs::read_to_string(ws.join("IDENTITY.md")).unwrap();
    assert!(root.contains("Dev"), "got: {}", root);
}

#[tokio::test]
async fn test_activate_missing_workspace() {
    let ctx = make_ctx_no_workspace();
    let h = PersonaHandler::new();
    let data = serde_json::json!({ "name": "dev" });
    let err = h
        .handle_cmd("activate", Some(data), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("workspace not configured"));
}

#[tokio::test]
async fn test_restore_activated() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    seed_default_persona(ws);
    {
        let ctx = make_ctx(&dir);
        let h = PersonaHandler::new();
        let _ = h.handle_cmd("current", None, &ctx).await.unwrap();
    }
    create_persona_dir(ws, "dev", "Dev", "🛠", "dev persona");

    let ctx = make_ctx_with_agent(&dir);
    let h = PersonaHandler::new();
    let data = serde_json::json!({ "name": "dev" });
    let _ = h.handle_cmd("activate", Some(data), &ctx).await.unwrap();

    let r = h.handle_cmd("restore", None, &ctx).await.unwrap().unwrap();
    assert!(r["activated"].as_bool().unwrap());
    assert_eq!(r["name"], "default");
    let active: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(ws.join("personas/_active.json")).unwrap())
            .unwrap();
    assert_eq!(active["name"], "default");
}

#[tokio::test]
async fn test_restore_without_agent_service_errors() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    seed_default_persona(ws);
    {
        let ctx = make_ctx(&dir);
        let h = PersonaHandler::new();
        let _ = h.handle_cmd("current", None, &ctx).await.unwrap();
    }
    let ctx = make_ctx(&dir); // No agent_service
    let h = PersonaHandler::new();
    let err = h.handle_cmd("restore", None, &ctx).await.unwrap_err();
    assert!(err.contains("Agent not available"), "got: {}", err);
}

#[tokio::test]
async fn test_restore_missing_workspace() {
    let ctx = make_ctx_no_workspace();
    let h = PersonaHandler::new();
    let err = h.handle_cmd("restore", None, &ctx).await.unwrap_err();
    assert!(err.contains("workspace not configured"));
}

// shop.refresh (no network)

#[tokio::test]
async fn test_shop_refresh_no_network() {
    // refresh 清空三级全局缓存，必须与 persona/tests.rs 的种子测试互斥。
    let _shop_guard = crate::handlers::persona::tests::SHOP_TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = PersonaHandler::new();
    let r = h
        .handle_cmd("shop.refresh", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(r["refreshed"].as_bool().unwrap());
}

#[tokio::test]
async fn test_shop_refresh_missing_workspace() {
    let ctx = make_ctx_no_workspace();
    let h = PersonaHandler::new();
    let err = h.handle_cmd("shop.refresh", None, &ctx).await.unwrap_err();
    assert!(err.contains("workspace not configured"));
}

// shop.preview/download without network still validate inputs

#[tokio::test]
async fn test_shop_preview_missing_data() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = PersonaHandler::new();
    let err = h.handle_cmd("shop.preview", None, &ctx).await.unwrap_err();
    assert_eq!(err, "missing data");
}

#[tokio::test]
async fn test_shop_preview_missing_field_id() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = PersonaHandler::new();
    let data = serde_json::json!({});
    let err = h
        .handle_cmd("shop.preview", Some(data), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing field: id");
}

#[tokio::test]
async fn test_shop_download_missing_data() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = PersonaHandler::new();
    let err = h.handle_cmd("shop.download", None, &ctx).await.unwrap_err();
    assert_eq!(err, "missing data");
}

#[tokio::test]
async fn test_shop_download_missing_field_id() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = PersonaHandler::new();
    let data = serde_json::json!({});
    let err = h
        .handle_cmd("shop.download", Some(data), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing field: id");
}

// unknown command

#[tokio::test]
async fn test_unknown_command() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = PersonaHandler::new();
    let err = h.handle_cmd("bogus", None, &ctx).await.unwrap_err();
    assert_eq!(err, "unknown command: persona.bogus");
}

#[tokio::test]
async fn test_unknown_command_missing_workspace_still_workspace_first() {
    let ctx = make_ctx_no_workspace();
    let h = PersonaHandler::new();
    let err = h.handle_cmd("bogus", None, &ctx).await.unwrap_err();
    assert!(err.contains("workspace not configured"));
}

// persona 面用户可控 name/id/file 参数校验闸

/// 计数 mock：restart = stop + start 各 +1，用于 remove-active 重启断言。
struct CountingAgentService(std::sync::atomic::AtomicUsize);

impl LifecycleService for CountingAgentService {
    fn start(&self) -> Result<(), String> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
    fn stop(&self) -> Result<(), String> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
    fn is_running(&self) -> bool {
        true
    }
}
impl AgentLoopService for CountingAgentService {}

fn make_ctx_with_counting_agent(ws: &str, counter: Arc<CountingAgentService>) -> RequestContext {
    make_ctx_inner(ws, Some(counter as Arc<dyn AgentLoopService>))
}

async fn seeded_workspace(dir: &tempfile::TempDir) -> std::path::PathBuf {
    let ws = dir.path().to_path_buf();
    seed_default_persona(&ws);
    let ctx = make_ctx(dir);
    let h = PersonaHandler::new();
    let _ = h.handle_cmd("current", None, &ctx).await.unwrap();
    ws
}

#[tokio::test]
async fn test_remove_empty_name_rejected_store_intact() {
    let dir = tempfile::tempdir().unwrap();
    let ws = seeded_workspace(&dir).await;
    create_persona_dir(&ws, "go", "Go", "🟢", "go persona");

    let h = PersonaHandler::new();
    // name: "" 经 resolve_path("personas/") 命中 personas 目录本身——修复前
    // remove_dir_all 全灭人格库（default 档案 + 激活标记 + go）。
    let err = h
        .handle_cmd(
            "remove",
            Some(serde_json::json!({"name": ""})),
            &make_ctx(&dir),
        )
        .await
        .unwrap_err();
    assert!(err.contains("invalid"), "err: {err}");

    // 人格库完好：激活标记 + default + go 全在。
    assert!(ws.join("personas/_active.json").exists());
    assert!(ws.join("personas/default").exists());
    assert!(ws.join("personas/go").exists());
}

#[tokio::test]
async fn test_remove_path_form_name_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let ws = seeded_workspace(&dir).await;

    let h = PersonaHandler::new();
    for bad in ["../x", "a/b", "a\\b", "..", "con", "x."] {
        let err = h
            .handle_cmd(
                "remove",
                Some(serde_json::json!({"name": bad})),
                &make_ctx(&dir),
            )
            .await
            .unwrap_err();
        assert!(err.contains("invalid"), "name {bad:?} 必须拒绝，err: {err}");
    }
    assert!(ws.join("personas/_active.json").exists());
}

#[tokio::test]
async fn test_activate_empty_name_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let ws = seeded_workspace(&dir).await;
    let root_identity = std::fs::read_to_string(ws.join("IDENTITY.md")).unwrap();

    let h = PersonaHandler::new();
    let err = h
        .handle_cmd(
            "activate",
            Some(serde_json::json!({"name": ""})),
            &make_ctx(&dir),
        )
        .await
        .unwrap_err();
    assert!(err.contains("invalid"), "err: {err}");

    // 工作区未被清空、激活标记未被写成空名。
    assert_eq!(
        std::fs::read_to_string(ws.join("IDENTITY.md")).unwrap(),
        root_identity
    );
    let active: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(ws.join("personas/_active.json")).unwrap())
            .unwrap();
    assert_eq!(active["name"], "default");
}

#[tokio::test]
async fn test_file_save_traversal_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let ws = seeded_workspace(&dir).await;

    let h = PersonaHandler::new();
    let err = h
        .handle_cmd(
            "file.save",
            Some(serde_json::json!({
                "name": "default",
                "file": "../config.json",
                "content": "{\"security\":{\"enabled\":false}}",
            })),
            &make_ctx(&dir),
        )
        .await
        .unwrap_err();
    assert!(err.contains("不是可编辑的人格文件"), "err: {err}");
    assert!(!ws.join("config.json").exists(), "越界文件不得落盘");
}

#[tokio::test]
async fn test_file_get_and_nonwhitelisted_files_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let _ws = seeded_workspace(&dir).await;

    let h = PersonaHandler::new();
    let ctx = make_ctx(&dir);
    // workspace 内相对逃逸（personas/../…）也被白名单拦下。
    for bad in [
        "../config.json",
        "../../IDENTITY.md",
        "HEARTBEAT.md",
        "sub/IDENTITY.md",
    ] {
        let err = h
            .handle_cmd(
                "file.get",
                Some(serde_json::json!({"name": "default", "file": bad})),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(err.contains("不是可编辑的人格文件"), "file {bad:?}: {err}");
    }
    // 白名单内四文件照常。
    let ok = h
        .handle_cmd(
            "file.get",
            Some(serde_json::json!({"name": "default", "file": "SOUL.md"})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert!(ok["content"].is_string());
}

#[tokio::test]
async fn test_shop_download_invalid_id_rejected_before_fetch() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = PersonaHandler::new();
    // 验证器先于任何网络语句（对齐 skills installer 纪律）——无种子 ctx
    // 直接拒绝且不触网。
    let err = h
        .handle_cmd(
            "shop.download",
            Some(serde_json::json!({"id": "../evil"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("invalid"), "err: {err}");
    let err = h
        .handle_cmd(
            "shop.download",
            Some(serde_json::json!({"id": "a/b"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("invalid"), "err: {err}");
}

#[tokio::test]
async fn test_remove_active_persona_restarts_agent() {
    let dir = tempfile::tempdir().unwrap();
    let ws = seeded_workspace(&dir).await;
    create_persona_dir(&ws, "go", "Go", "🟢", "go persona");

    let counter = Arc::new(CountingAgentService(std::sync::atomic::AtomicUsize::new(0)));
    let ctx = make_ctx_with_counting_agent(&ws.to_string_lossy(), counter.clone());
    let h = PersonaHandler::new();

    // activate "go" → restart #1（stop+start = 2 次）。
    let _ = h
        .handle_cmd("activate", Some(serde_json::json!({"name": "go"})), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(counter.0.load(std::sync::atomic::Ordering::SeqCst), 2);

    // remove-active → 文件面落 default + agent 重启 #2（此前漏）。
    let _ = h
        .handle_cmd("remove", Some(serde_json::json!({"name": "go"})), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        counter.0.load(std::sync::atomic::Ordering::SeqCst),
        4,
        "remove-active 必须重启 agent"
    );
    let active: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(ws.join("personas/_active.json")).unwrap())
            .unwrap();
    assert_eq!(active["name"], "default");
}

#[tokio::test]
async fn test_remove_non_active_persona_no_restart() {
    let dir = tempfile::tempdir().unwrap();
    let ws = seeded_workspace(&dir).await;
    create_persona_dir(&ws, "extra", "Extra", "✨", "extra persona");
    set_active(&ws, "default");

    let counter = Arc::new(CountingAgentService(std::sync::atomic::AtomicUsize::new(0)));
    let ctx = make_ctx_with_counting_agent(&ws.to_string_lossy(), counter.clone());
    let h = PersonaHandler::new();
    let _ = h
        .handle_cmd("remove", Some(serde_json::json!({"name": "extra"})), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        counter.0.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "非活动人格删除不影响 agent，不得重启"
    );
}
