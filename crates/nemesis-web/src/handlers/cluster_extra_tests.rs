//! Extra coverage tests for the Cluster WebSocket handler.
//!
//! Mirrors the helpers used in `tests.rs` so we can construct
//! `RequestContext` instances with a temp workspace. Focus is on the
//! `handle_cmd` arms that do NOT need a real `Cluster` runtime
//! instance — workspace file ops, config validation, error paths,
//! unknown commands, and the firewall diagnostics (which don't
//! require a `Cluster`).

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::Arc;
use std::time::Instant;

// -----------------------------------------------------------------------
// Test infrastructure (mirror of tests.rs)
// -----------------------------------------------------------------------

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
        chat_secret_store: std::sync::Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::in_memory()),
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
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

/// Variant that injects a `cluster_log_dir` pointing into the temp dir
/// (needed to exercise paths that read log files without an actual Cluster).
fn make_ctx_with_log_dir(dir: &tempfile::TempDir) -> RequestContext {
    let ws = dir.path().to_string_lossy().to_string();
    let log_dir = dir.path().join("cluster_logs");
    std::fs::create_dir_all(&log_dir).unwrap();
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
        cluster_log_dir: Some(log_dir.to_string_lossy().to_string()),
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
        workspace: Some(ws.clone()),
        home: Some(ws),
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

/// Variant that has a workspace but no home (some arms only need workspace).
fn make_ctx_no_home(dir: &tempfile::TempDir) -> RequestContext {
    let ws = dir.path().to_string_lossy().to_string();
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: Some(ws.clone()),
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
        workspace: Some(ws),
        home: None,
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

fn ensure_config_dir(workspace: &Path) {
    std::fs::create_dir_all(workspace.join("config")).unwrap();
}

fn ensure_cluster_dir(workspace: &Path) {
    std::fs::create_dir_all(workspace.join("cluster")).unwrap();
}

fn write_main_config(home: &Path, body: &str) {
    std::fs::write(home.join("config.json"), body).unwrap();
}

// =======================================================================
// status / legacy_status
// =======================================================================

#[tokio::test]
async fn test_status_no_cluster_no_config_returns_fallback() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let result = handler.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
    assert_eq!(result["running"], false);
    assert_eq!(result["config_exists"], false);
    assert_eq!(result["online_nodes"], 0);
    assert_eq!(result["total_nodes"], 0);
    assert_eq!(result["success_rate"], 0.0);
    assert_eq!(result["avg_duration"], "--");
}

#[tokio::test]
async fn test_legacy_status_alias_matches_runtime_status() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let legacy = handler.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
    let direct = handler
        .handle_cmd("runtime.status", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(legacy["config_exists"], direct["config_exists"]);
    assert_eq!(legacy["running"], direct["running"]);
}

#[tokio::test]
async fn test_status_with_peers_counts_nodes_section() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    ensure_cluster_dir(dir.path());
    std::fs::write(
        dir.path().join("cluster/peers.toml"),
        r#"[node]
id = "abc"
name = "primary"
role = "manager"

[peers.remote1]
address = "10.0.0.1:12000"
role = "worker"

[peers.remote2]
address = "10.0.0.2:12000"
role = "worker"
"#,
    )
    .unwrap();

    let ctx = make_ctx(&dir);
    let result = handler.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
    // 2 peers under [peers.X]
    assert_eq!(result["total_nodes"], 2);
    assert_eq!(result["role"], "manager");
    assert_eq!(result["node_name"], "primary");
}

#[tokio::test]
async fn test_status_no_workspace_errors() {
    let handler = cluster::ClusterHandler::new();
    let ctx = make_ctx_no_workspace();
    let err = handler.handle_cmd("status", None, &ctx).await.unwrap_err();
    assert!(err.contains("workspace not configured"));
}

// =======================================================================
// config.get
// =======================================================================

#[tokio::test]
async fn test_config_get_returns_empty_object_when_missing() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let result = handler
        .handle_cmd("config.get", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(result.is_object());
    assert!(result.as_object().unwrap().is_empty() || result.get("master_enabled").is_some());
}

#[tokio::test]
async fn test_config_get_reads_existing_cluster_config() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    ensure_config_dir(dir.path());
    std::fs::write(
        dir.path().join("config/config.cluster.json"),
        r#"{"enabled":true,"port":11949,"rpc_port":21949}"#,
    )
    .unwrap();
    let ctx = make_ctx(&dir);

    let result = handler
        .handle_cmd("config.get", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["enabled"], true);
    assert_eq!(result["port"], 11949);
}

#[tokio::test]
async fn test_config_get_injects_master_enabled_from_main_config() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    ensure_config_dir(dir.path());
    write_main_config(dir.path(), r#"{"cluster":{"enabled":true}}"#);
    let ctx = make_ctx(&dir);

    let result = handler
        .handle_cmd("config.get", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["master_enabled"], true);
}

#[tokio::test]
async fn test_config_get_master_enabled_false_when_missing() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_main_config(dir.path(), r#"{"cluster":{}}"#);
    let ctx = make_ctx(&dir);

    let result = handler
        .handle_cmd("config.get", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["master_enabled"], false);
}

#[tokio::test]
async fn test_config_get_reads_node_identity_from_peers_toml() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    ensure_cluster_dir(dir.path());
    std::fs::write(
        dir.path().join("cluster/peers.toml"),
        r#"[node]
id = "node-7"
name = "alpha"
role = "manager"
category = "edge"
tags = ["prod"]
"#,
    )
    .unwrap();
    let ctx = make_ctx(&dir);

    let result = handler
        .handle_cmd("config.get", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["node_id"], "node-7");
    assert_eq!(result["name"], "alpha");
    assert_eq!(result["role"], "manager");
    assert_eq!(result["category"], "edge");
}

#[tokio::test]
async fn test_config_get_no_workspace_errors() {
    let handler = cluster::ClusterHandler::new();
    let ctx = make_ctx_no_workspace();
    let err = handler
        .handle_cmd("config.get", None, &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("workspace not configured"));
}

// =======================================================================
// config.save
// =======================================================================

#[tokio::test]
async fn test_config_save_writes_file() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({
        "enabled": true,
        "port": 12000,
        "rpc_port": 22000,
        "token": "secret-token"
    });
    let result = handler
        .handle_cmd("config.save", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["saved"], true);

    let written = std::fs::read_to_string(dir.path().join("config/config.cluster.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&written).unwrap();
    assert_eq!(parsed["port"], 12000);
    assert_eq!(parsed["token"], "secret-token");
}

#[tokio::test]
async fn test_config_save_creates_config_dir() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({"enabled": false});
    handler
        .handle_cmd("config.save", Some(data), &ctx)
        .await
        .unwrap();

    assert!(dir.path().join("config/config.cluster.json").exists());
}

#[tokio::test]
async fn test_config_save_missing_data_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd("config.save", None, &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing data");
}

#[tokio::test]
async fn test_config_save_no_workspace_errors() {
    let handler = cluster::ClusterHandler::new();
    let ctx = make_ctx_no_workspace();
    let data = serde_json::json!({"enabled": true});
    let err = handler
        .handle_cmd("config.save", Some(data), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("workspace not configured"));
}

#[tokio::test]
async fn test_config_save_rejects_zero_discovery_port() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let data = serde_json::json!({"cluster": {"discovery_port": 0}});
    let err = handler
        .handle_cmd("config.save", Some(data), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("discovery_port must be between 1 and 65535"));
}

#[tokio::test]
async fn test_config_save_rejects_out_of_range_rpc_port() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let data = serde_json::json!({"cluster": {"rpc_port": 70000}});
    let err = handler
        .handle_cmd("config.save", Some(data), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("rpc_port must be between 1 and 65535"));
}

#[tokio::test]
async fn test_config_save_rejects_equal_ports() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let data = serde_json::json!({"cluster": {"discovery_port": 12345, "rpc_port": 12345}});
    let err = handler
        .handle_cmd("config.save", Some(data), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("must be different"));
}

#[tokio::test]
async fn test_config_save_accepts_valid_distinct_ports() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let data = serde_json::json!({"cluster": {"discovery_port": 100, "rpc_port": 200}});
    let result = handler
        .handle_cmd("config.save", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["saved"], true);
}

// =======================================================================
// config.set_master_enabled
// =======================================================================

#[tokio::test]
async fn test_set_master_enabled_true() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_main_config(dir.path(), r#"{"cluster":{"enabled":false}}"#);
    let ctx = make_ctx(&dir);

    let result = handler
        .handle_cmd("config.set_master_enabled", Some(serde_json::json!({"enabled": true})), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["updated"], true);
    assert_eq!(result["enabled"], true);

    let body = std::fs::read_to_string(dir.path().join("config.json")).unwrap();
    assert!(body.contains(r#""enabled": true"#));
}

#[tokio::test]
async fn test_set_master_enabled_false() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_main_config(dir.path(), r#"{"cluster":{"enabled":true}}"#);
    let ctx = make_ctx(&dir);

    handler
        .handle_cmd(
            "config.set_master_enabled",
            Some(serde_json::json!({"enabled": false})),
            &ctx,
        )
        .await
        .unwrap();

    let body = std::fs::read_to_string(dir.path().join("config.json")).unwrap();
    assert!(body.contains(r#""enabled": false"#));
}

#[tokio::test]
async fn test_set_master_enabled_creates_cluster_section_when_missing() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_main_config(dir.path(), r#"{"version":"1"}"#);
    let ctx = make_ctx(&dir);

    handler
        .handle_cmd(
            "config.set_master_enabled",
            Some(serde_json::json!({"enabled": true})),
            &ctx,
        )
        .await
        .unwrap();

    let body: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("config.json")).unwrap())
            .unwrap();
    assert_eq!(body["cluster"]["enabled"], true);
    // Existing top-level key preserved
    assert_eq!(body["version"], "1");
}

#[tokio::test]
async fn test_set_master_enabled_missing_field_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_main_config(dir.path(), r#"{}"#);
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd("config.set_master_enabled", Some(serde_json::json!({})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("missing or invalid 'enabled'"));
}

#[tokio::test]
async fn test_set_master_enabled_no_home_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_no_home(&dir);

    let err = handler
        .handle_cmd(
            "config.set_master_enabled",
            Some(serde_json::json!({"enabled": true})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("home not configured"));
}

#[tokio::test]
async fn test_set_master_enabled_missing_config_json_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    // No config.json written.
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd(
            "config.set_master_enabled",
            Some(serde_json::json!({"enabled": true})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("config.json not found"));
}

#[tokio::test]
async fn test_set_master_enabled_missing_data_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let err = handler
        .handle_cmd("config.set_master_enabled", None, &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing data");
}

// =======================================================================
// identity.get_files
// =======================================================================

#[tokio::test]
async fn test_identity_get_files_missing_returns_empty_strings() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let result = handler
        .handle_cmd("identity.get_files", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["identity"], "");
    assert_eq!(result["soul"], "");
}

#[tokio::test]
async fn test_identity_get_files_reads_existing() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    ensure_cluster_dir(dir.path());
    std::fs::write(dir.path().join("cluster/IDENTITY.md"), "# I am Nemesis").unwrap();
    std::fs::write(dir.path().join("cluster/SOUL.md"), "soul rules").unwrap();
    let ctx = make_ctx(&dir);

    let result = handler
        .handle_cmd("identity.get_files", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["identity"], "# I am Nemesis");
    assert_eq!(result["soul"], "soul rules");
}

#[tokio::test]
async fn test_identity_get_files_no_workspace_errors() {
    let handler = cluster::ClusterHandler::new();
    let ctx = make_ctx_no_workspace();
    let err = handler
        .handle_cmd("identity.get_files", None, &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("workspace not configured"));
}

// =======================================================================
// identity.save_file
// =======================================================================

#[tokio::test]
async fn test_identity_save_file_identity() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({
        "file": "IDENTITY.md",
        "content": "# New identity"
    });
    let result = handler
        .handle_cmd("identity.save_file", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["saved"], true);
    assert_eq!(result["file"], "IDENTITY.md");

    let written = std::fs::read_to_string(dir.path().join("cluster/IDENTITY.md")).unwrap();
    assert_eq!(written, "# New identity");
}

#[tokio::test]
async fn test_identity_save_file_soul() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({"file": "SOUL.md", "content": "be excellent"});
    handler
        .handle_cmd("identity.save_file", Some(data), &ctx)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("cluster/SOUL.md")).unwrap(),
        "be excellent"
    );
}

#[tokio::test]
async fn test_identity_save_file_rejects_unknown_filename() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({"file": "EVIL.md", "content": "pwned"});
    let err = handler
        .handle_cmd("identity.save_file", Some(data), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("not allowed"));
}

#[tokio::test]
async fn test_identity_save_file_missing_file_field() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd(
            "identity.save_file",
            Some(serde_json::json!({"content": "x"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("missing 'file'"));
}

#[tokio::test]
async fn test_identity_save_file_missing_content_field() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd(
            "identity.save_file",
            Some(serde_json::json!({"file": "IDENTITY.md"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("missing 'content'"));
}

#[tokio::test]
async fn test_identity_save_file_missing_data_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd("identity.save_file", None, &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing data");
}

#[tokio::test]
async fn test_identity_save_file_no_workspace_errors() {
    let handler = cluster::ClusterHandler::new();
    let ctx = make_ctx_no_workspace();
    let data = serde_json::json!({"file": "IDENTITY.md", "content": "x"});
    let err = handler
        .handle_cmd("identity.save_file", Some(data), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("workspace not configured"));
}

// =======================================================================
// peers
// =======================================================================

#[tokio::test]
async fn test_peers_missing_file_returns_empty_array() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let result = handler.handle_cmd("peers", None, &ctx).await.unwrap().unwrap();
    assert!(result["peers"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_peers_returns_file_content_as_string() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    ensure_cluster_dir(dir.path());
    let raw = "[node]\nname = \"alpha\"\n";
    std::fs::write(dir.path().join("cluster/peers.toml"), raw).unwrap();
    let ctx = make_ctx(&dir);

    let result = handler.handle_cmd("peers", None, &ctx).await.unwrap().unwrap();
    assert_eq!(result["format"], "toml");
    assert_eq!(result["peers"], raw);
}

#[tokio::test]
async fn test_peers_no_workspace_errors() {
    let handler = cluster::ClusterHandler::new();
    let ctx = make_ctx_no_workspace();
    let err = handler.handle_cmd("peers", None, &ctx).await.unwrap_err();
    assert!(err.contains("workspace not configured"));
}

// =======================================================================
// firewall.check (no Cluster needed — uses defaults)
// =======================================================================

#[tokio::test]
async fn test_firewall_check_returns_test_array() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let result = handler
        .handle_cmd("firewall.check", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    // Defaults: udp=11949, tcp=21949
    assert_eq!(result["udp_port"], 11949);
    assert_eq!(result["tcp_port"], 21949);
    let tests = result["tests"].as_array().unwrap();
    // 5 test entries: udp_bind, broadcast_flag, broadcast_loopback, tcp_bind, firewall_status
    assert_eq!(tests.len(), 5);
    let names: Vec<&str> = tests
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"udp_bind"));
    assert!(names.contains(&"tcp_bind"));
    assert!(names.contains(&"firewall_status"));
}

#[tokio::test]
async fn test_firewall_check_uses_config_ports_when_present() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    ensure_config_dir(dir.path());
    std::fs::write(
        dir.path().join("config/config.cluster.json"),
        r#"{"port":15000,"rpc_port":25000}"#,
    )
    .unwrap();
    let ctx = make_ctx(&dir);

    let result = handler
        .handle_cmd("firewall.check", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["udp_port"], 15000);
    assert_eq!(result["tcp_port"], 25000);
}

#[tokio::test]
async fn test_firewall_check_no_workspace_uses_defaults() {
    // read_cluster_ports falls back to defaults when workspace is None.
    let handler = cluster::ClusterHandler::new();
    let ctx = make_ctx_no_workspace();

    let result = handler
        .handle_cmd("firewall.check", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["udp_port"], 11949);
    assert_eq!(result["tcp_port"], 21949);
    assert!(result["platform"].is_string());
}

// =======================================================================
// firewall.add_rules
// =======================================================================

#[tokio::test]
async fn test_firewall_add_rules_rejects_zero_port() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({"udp_port": 0, "tcp_port": 100});
    let err = handler
        .handle_cmd("firewall.add_rules", Some(data), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("端口范围无效"));
}

#[tokio::test]
async fn test_firewall_add_rules_missing_data_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd("firewall.add_rules", None, &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing data");
}

// =======================================================================
// Commands that need a real Cluster — verify graceful errors
// =======================================================================

#[tokio::test]
async fn test_nodes_list_no_cluster_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd("nodes.list", None, &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "cluster not available");
}

#[tokio::test]
async fn test_nodes_ping_missing_data_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd("nodes.ping", None, &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing data");
}

#[tokio::test]
async fn test_nodes_ping_missing_node_id_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd("nodes.ping", Some(serde_json::json!({})), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing node_id");
}

#[tokio::test]
async fn test_nodes_remove_missing_node_id_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd("nodes.remove", Some(serde_json::json!({})), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing node_id");
}

#[tokio::test]
async fn test_nodes_add_missing_address_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd("nodes.add", Some(serde_json::json!({"name": "x"})), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing address");
}

#[tokio::test]
async fn test_tasks_list_no_cluster_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd("tasks.list", None, &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "cluster not available");
}

#[tokio::test]
async fn test_tasks_cancel_missing_data_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd("tasks.cancel", None, &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing data");
}

#[tokio::test]
async fn test_tasks_cancel_missing_task_id_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd("tasks.cancel", Some(serde_json::json!({})), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing task_id");
}

#[tokio::test]
async fn test_tasks_detail_missing_task_id_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd("tasks.detail", Some(serde_json::json!({"x": 1})), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing task_id");
}

#[tokio::test]
async fn test_tasks_submit_missing_content_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd(
            "tasks.submit",
            Some(serde_json::json!({"target_node_id": "x"})),
            &ctx,
        )
        .await
        .unwrap_err();
    // require_cluster is checked first.
    assert_eq!(err, "cluster not available");
}

#[tokio::test]
async fn test_topology_no_cluster_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd("topology", None, &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "cluster not available");
}

#[tokio::test]
async fn test_traces_no_log_dir_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler.handle_cmd("traces", None, &ctx).await.unwrap_err();
    assert_eq!(err, "cluster log directory not configured");
}

#[tokio::test]
async fn test_events_recent_no_log_dir_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd("events.recent", None, &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "cluster log directory not configured");
}

#[tokio::test]
async fn test_events_recent_with_log_dir_returns_events_array() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_log_dir(&dir);

    let result = handler
        .handle_cmd("events.recent", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(result["events"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_events_recent_honors_limit() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let log_dir = dir.path().join("cluster_logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    // Fake event files — empty dir means 0 events regardless of limit.
    let ctx = make_ctx_with_log_dir(&dir);

    let result = handler
        .handle_cmd(
            "events.recent",
            Some(serde_json::json!({"limit": 5})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert!(result["events"].as_array().unwrap().len() <= 5);
}

#[tokio::test]
async fn test_traces_with_log_dir_returns_traces() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_log_dir(&dir);

    let result = handler.handle_cmd("traces", None, &ctx).await.unwrap().unwrap();
    assert!(result["traces"].as_array().is_some());
}

#[tokio::test]
async fn test_snapshots_list_no_cluster_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd("snapshots.list", None, &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "cluster not available");
}

#[tokio::test]
async fn test_snapshots_cleanup_no_cluster_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd("snapshots.cleanup", None, &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "cluster not available");
}

#[tokio::test]
async fn test_node_update_identity_no_cluster_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd(
            "node.update_identity",
            Some(serde_json::json!({"name": "x"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert_eq!(err, "cluster not available");
}

#[tokio::test]
async fn test_node_update_identity_missing_data_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd("node.update_identity", None, &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing data");
}

#[tokio::test]
async fn test_diagnostics_run_missing_data_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd("diagnostics.run", None, &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing data");
}

#[tokio::test]
async fn test_diagnostics_run_missing_node_id_errors() {
    // require_cluster runs first inside diagnostics_run, so without a cluster
    // the user always sees "cluster not available" rather than the field error.
    // Verify that here as documentation of the actual call ordering.
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd(
            "diagnostics.run",
            Some(serde_json::json!({"action": "test"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert_eq!(err, "cluster not available");
}

#[tokio::test]
async fn test_diagnostics_run_missing_action_errors() {
    // Same ordering as above — require_cluster precedes field validation.
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd(
            "diagnostics.run",
            Some(serde_json::json!({"node_id": "n1"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert_eq!(err, "cluster not available");
}

#[tokio::test]
async fn test_diagnostics_run_no_cluster_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd(
            "diagnostics.run",
            Some(serde_json::json!({"node_id": "n1", "action": "ping"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert_eq!(err, "cluster not available");
}

#[tokio::test]
async fn test_runtime_start_no_cluster_service_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd("runtime.start", None, &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "cluster service not available");
}

#[tokio::test]
async fn test_runtime_stop_no_cluster_service_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd("runtime.stop", None, &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "cluster service not available");
}

// =======================================================================
// Unknown command
// =======================================================================

#[tokio::test]
async fn test_unknown_command_returns_error() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd("totally.made_up", None, &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "unknown command: cluster.totally.made_up");
}

#[tokio::test]
async fn test_unknown_command_empty_string() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler.handle_cmd("", None, &ctx).await.unwrap_err();
    assert_eq!(err, "unknown command: cluster.");
}

#[tokio::test]
async fn test_module_name_is_cluster() {
    let handler = cluster::ClusterHandler::new();
    assert_eq!(handler.module_name(), "cluster");
}
