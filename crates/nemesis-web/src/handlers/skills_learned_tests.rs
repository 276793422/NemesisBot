//! N3 learned-skills manifest（2026-10-13 机会项）WSAPI 面测试。
//!
//! 覆盖：`skills.learned_list`（空账/有条目 + on_disk 幽灵核对）、
//! `skills.learned_remove`（happy 清目录+账目 / 非 manifest 成员拒绝
//! （registry 技能保护面）/ 路径穿越拒绝 / 缺参校验）、commands()
//! 清单收录（L1 漂移闸）。

use super::skills::SkillsHandler;
use crate::ws_router::ModuleHandler;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::RequestContext;

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
        signature_verify: None,
        skills_install_gate: None,
        cron: None,
        board: None,
    });
    RequestContext {
        session_id: "test-session".to_string(),
        chat_id: "test-chat".to_string(),
        workspace: Some(ws),
        home: None,
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

/// 造一个 manifest 条目（便捷：直接调 nemesis_skills::learned::record_learned）。
fn seed_learned(ws: &Path, name: &str, desc: &str) {
    nemesis_skills::learned::record_learned(ws, name, desc).unwrap();
}

// ---------------------------------------------------------------------------
// commands() 清单（L1 漂移闸）
// ---------------------------------------------------------------------------

#[test]
fn commands_list_includes_learned_surface() {
    let handler = SkillsHandler::new();
    assert!(handler.commands().contains(&"learned_list"));
    assert!(handler.commands().contains(&"learned_remove"));
}

// ---------------------------------------------------------------------------
// learned_list
// ---------------------------------------------------------------------------

#[tokio::test]
async fn learned_list_empty_without_manifest() {
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let result = handler
        .handle_cmd("learned_list", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(result["entries"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn learned_list_reports_on_disk_flag() {
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    seed_learned(ws, "real-one", "exists on disk");
    seed_learned(ws, "ghost-one", "manifest-only entry");
    // 只有 real-one 落了目录；ghost-one 只在账上。
    std::fs::create_dir_all(ws.join("skills/real-one")).unwrap();
    let ctx = make_ctx(&dir);

    let result = handler
        .handle_cmd("learned_list", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    let entries = result["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    let real = entries.iter().find(|e| e["name"] == "real-one").unwrap();
    assert_eq!(real["description"], "exists on disk");
    assert!(real["on_disk"].as_bool().unwrap());
    let ghost = entries.iter().find(|e| e["name"] == "ghost-one").unwrap();
    assert!(!ghost["on_disk"].as_bool().unwrap(), "幽灵条目必须如实标记");
}

// ---------------------------------------------------------------------------
// learned_remove
// ---------------------------------------------------------------------------

#[tokio::test]
async fn learned_remove_happy_clears_dir_and_manifest() {
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    seed_learned(ws, "doomed", "will be removed");
    std::fs::create_dir_all(ws.join("skills/doomed")).unwrap();
    std::fs::write(ws.join("skills/doomed/SKILL.md"), "# doomed").unwrap();
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "name": "doomed" });
    let result = handler
        .handle_cmd("learned_remove", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(result["removed"].as_bool().unwrap());
    assert!(!ws.join("skills/doomed").exists(), "目录必须一起清");
    let manifest = nemesis_skills::learned::LearnedManifest::load(ws);
    assert!(manifest.get("doomed").is_none(), "账目必须同步清");
}

#[tokio::test]
async fn learned_remove_rejects_non_manifest_member() {
    // 目录在但账上没有（registry 装的技能形态）→ 拒绝且目录原样保留。
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    std::fs::create_dir_all(ws.join("skills/registry-skill")).unwrap();
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "name": "registry-skill" });
    let result = handler.handle_cmd("learned_remove", Some(data), &ctx).await;
    let err = result.unwrap_err();
    assert!(
        err.contains("not in the learned manifest"),
        "必须点明护栏: {}",
        err
    );
    assert!(
        ws.join("skills/registry-skill").exists(),
        "非成员目录不得被碰"
    );
}

#[tokio::test]
async fn learned_remove_rejects_traversal_name() {
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({ "name": "../escape" });
    let result = handler.handle_cmd("learned_remove", Some(data), &ctx).await;
    let err = result.unwrap_err();
    assert!(err.contains("invalid skill name"), "got: {}", err);
}

#[tokio::test]
async fn learned_remove_missing_data_and_name() {
    let handler = SkillsHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let result = handler.handle_cmd("learned_remove", None, &ctx).await;
    assert_eq!(result.unwrap_err(), "missing data");

    let data = serde_json::json!({ "other": "x" });
    let result = handler.handle_cmd("learned_remove", Some(data), &ctx).await;
    assert_eq!(result.unwrap_err(), "missing field: name");
}
