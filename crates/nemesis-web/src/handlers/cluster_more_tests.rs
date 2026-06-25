//! Additional coverage tests for the Cluster WebSocket handler.
//!
//! This file complements `cluster_extra_tests.rs` and focuses on:
//! - `nodes.add` happy path (writes peers.toml, no Cluster required)
//! - `runtime_status` fallback parsing variations (quoted/unquoted values,
//!   missing [node] section, etc.)
//! - `firewall.check` with corrupt/empty cluster config
//! - Additional `config.save` / `config.get` edge cases
//! - `peers` / `identity.*` branches not previously covered
//!
//! All tests avoid requiring a live `Cluster` runtime — they exercise the
//! workspace-file paths which make up the bulk of uncovered lines.

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
// Test infrastructure (mirrors cluster_extra_tests.rs)
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
    });
    RequestContext {
        session_id: "test-session".to_string(),
        chat_id: "test-chat".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
    }
}

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
    });
    RequestContext {
        session_id: "test-session".to_string(),
        chat_id: "test-chat".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
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
    });
    RequestContext {
        session_id: "test-session".to_string(),
        chat_id: "test-chat".to_string(),
        workspace: None,
        home: None,
        state,
    }
}

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
    });
    RequestContext {
        session_id: "test-session".to_string(),
        chat_id: "test-chat".to_string(),
        workspace: Some(ws),
        home: None,
        state,
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
// nodes.add — happy path coverage (writes peers.toml, no Cluster needed)
// =======================================================================

#[tokio::test]
async fn test_nodes_add_with_explicit_id_writes_peer() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({
        "address": "10.0.0.5:12000",
        "name": "edge-1",
        "id": "node-explicit-id",
        "role": "worker",
        "category": "edge",
    });
    let result = handler
        .handle_cmd("nodes.add", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["added"], true);

    let written = std::fs::read_to_string(dir.path().join("cluster/peers.toml")).unwrap();
    assert!(written.contains("[peers.node-explicit-id]"));
    assert!(written.contains("10.0.0.5:12000"));
    assert!(written.contains("category = \"edge\""));
}

#[tokio::test]
async fn test_nodes_add_falls_back_to_name_as_id() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({
        "address": "10.0.0.6:12000",
        "name": "node-by-name",
    });
    handler
        .handle_cmd("nodes.add", Some(data), &ctx)
        .await
        .unwrap();

    let written = std::fs::read_to_string(dir.path().join("cluster/peers.toml")).unwrap();
    assert!(written.contains("[peers.node-by-name]"));
}

#[tokio::test]
async fn test_nodes_add_falls_back_to_address_when_no_name() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({
        "address": "10.0.0.7:12000",
    });
    handler
        .handle_cmd("nodes.add", Some(data), &ctx)
        .await
        .unwrap();

    let written = std::fs::read_to_string(dir.path().join("cluster/peers.toml")).unwrap();
    // sanitize_peer_key keeps alphanumeric + dash; colon becomes dash.
    assert!(written.contains("[peers."));
    assert!(written.contains("10.0.0.7:12000"));
}

#[tokio::test]
async fn test_nodes_add_default_role_and_category() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({
        "address": "10.0.0.8:12000",
        "name": "defaulted",
    });
    handler
        .handle_cmd("nodes.add", Some(data), &ctx)
        .await
        .unwrap();

    let written = std::fs::read_to_string(dir.path().join("cluster/peers.toml")).unwrap();
    assert!(written.contains("role = \"worker\""));
    assert!(written.contains("category = \"general\""));
}

#[tokio::test]
async fn test_nodes_add_trims_explicit_id_whitespace() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({
        "address": "10.0.0.9:12000",
        "id": "   trimmed-id   ",
    });
    handler
        .handle_cmd("nodes.add", Some(data), &ctx)
        .await
        .unwrap();

    let written = std::fs::read_to_string(dir.path().join("cluster/peers.toml")).unwrap();
    assert!(written.contains("[peers.trimmed-id]"));
}

#[tokio::test]
async fn test_nodes_add_overwrites_existing_peer() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    // First add
    let data1 = serde_json::json!({
        "address": "10.0.0.10:12000",
        "id": "dup-id",
    });
    handler
        .handle_cmd("nodes.add", Some(data1), &ctx)
        .await
        .unwrap();

    // Second add with same id, different address
    let data2 = serde_json::json!({
        "address": "10.0.0.11:13000",
        "id": "dup-id",
    });
    handler
        .handle_cmd("nodes.add", Some(data2), &ctx)
        .await
        .unwrap();

    let written = std::fs::read_to_string(dir.path().join("cluster/peers.toml")).unwrap();
    assert!(written.contains("10.0.0.11:13000"));
    // Old address should be gone (overwrite is intentional).
    assert!(!written.contains("10.0.0.10:12000"));
}

#[tokio::test]
async fn test_nodes_add_no_workspace_errors() {
    let handler = cluster::ClusterHandler::new();
    let ctx = make_ctx_no_workspace();

    let data = serde_json::json!({"address": "10.0.0.1:12000"});
    let err = handler
        .handle_cmd("nodes.add", Some(data), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("workspace not configured"));
}

#[tokio::test]
async fn test_nodes_add_missing_data_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd("nodes.add", None, &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing data");
}

#[tokio::test]
async fn test_nodes_add_with_manager_role_in_data() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({
        "address": "10.0.0.20:12000",
        "id": "mgr-1",
        "role": "manager",
    });
    handler
        .handle_cmd("nodes.add", Some(data), &ctx)
        .await
        .unwrap();

    let written = std::fs::read_to_string(dir.path().join("cluster/peers.toml")).unwrap();
    assert!(written.contains("role = \"manager\""));
}

// =======================================================================
// runtime.status — fallback branch variations
// =======================================================================

#[tokio::test]
async fn test_status_fallback_with_config_cluster_json() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    ensure_config_dir(dir.path());
    std::fs::write(
        dir.path().join("config/config.cluster.json"),
        r#"{"enabled":true,"port":12000,"rpc_port":22000}"#,
    )
    .unwrap();
    let ctx = make_ctx(&dir);

    let result = handler.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
    assert_eq!(result["running"], false);
    assert_eq!(result["config"]["enabled"], true);
    assert_eq!(result["config"]["port"], 12000);
    assert_eq!(result["config_exists"], true);
}

#[tokio::test]
async fn test_status_fallback_peers_no_node_section() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    ensure_cluster_dir(dir.path());
    // peers.toml has only peer entries, no [node] section
    std::fs::write(
        dir.path().join("cluster/peers.toml"),
        r#"[peers.remote1]
address = "10.0.0.1:12000"
role = "worker"

[peers.remote2]
address = "10.0.0.2:12000"
"#,
    )
    .unwrap();
    let ctx = make_ctx(&dir);

    let result = handler.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
    assert_eq!(result["total_nodes"], 2);
    // No [node] section → role/name remain null
    assert!(result["role"].is_null());
    assert!(result["node_name"].is_null());
}

#[tokio::test]
async fn test_status_fallback_node_section_unquoted_values() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    ensure_cluster_dir(dir.path());
    std::fs::write(
        dir.path().join("cluster/peers.toml"),
        r#"[node]
id = abc
name = primary
role = worker

[peers.r1]
address = "10.0.0.1:12000"
"#,
    )
    .unwrap();
    let ctx = make_ctx(&dir);

    let result = handler.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
    assert_eq!(result["role"], "worker");
    assert_eq!(result["node_name"], "primary");
}

#[tokio::test]
async fn test_status_fallback_node_section_breaks_on_next_bracket() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    ensure_cluster_dir(dir.path());
    // [node] followed by another section — verify parsing stops at next [
    std::fs::write(
        dir.path().join("cluster/peers.toml"),
        r#"[node]
name = primary
role = worker

[peers.r1]
address = "10.0.0.1:12000"
"#,
    )
    .unwrap();
    let ctx = make_ctx(&dir);

    let result = handler.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
    assert_eq!(result["role"], "worker");
    assert_eq!(result["node_name"], "primary");
    assert_eq!(result["total_nodes"], 1);
}

#[tokio::test]
async fn test_status_fallback_node_section_empty_values() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    ensure_cluster_dir(dir.path());
    std::fs::write(
        dir.path().join("cluster/peers.toml"),
        r#"[node]
role = ""
name = ""
"#,
    )
    .unwrap();
    let ctx = make_ctx(&dir);

    let result = handler.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
    // Empty values should leave role/name as null (filtered by `if !val.is_empty()`).
    assert!(result["role"].is_null());
    assert!(result["node_name"].is_null());
}

#[tokio::test]
async fn test_status_fallback_empty_peers_file() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    ensure_cluster_dir(dir.path());
    std::fs::write(dir.path().join("cluster/peers.toml"), "").unwrap();
    let ctx = make_ctx(&dir);

    let result = handler.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
    assert_eq!(result["total_nodes"], 0);
}

#[tokio::test]
async fn test_status_fallback_no_config_no_peers_clean_state() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let result = handler.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
    assert_eq!(result["config_exists"], false);
    assert_eq!(result["total_nodes"], 0);
    assert_eq!(result["peers_count"], 0);
    assert!(result["config"].is_null());
}

#[tokio::test]
async fn test_status_with_log_dir_no_cluster_still_uses_fallback() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_log_dir(&dir);

    let result = handler.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
    // No cluster → fallback branch executes; recent_events stays empty array
    // since the cluster-runtime branch isn't taken.
    assert_eq!(result["running"], false);
    assert!(result["recent_events"].as_array().is_some());
}

#[tokio::test]
async fn test_status_with_log_dir_and_cluster_service_running_marker() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_log_dir(&dir);

    let result = handler.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
    // Without cluster_service, running=false in fallback.
    assert_eq!(result["running"], false);
}

// =======================================================================
// config.get — additional branches
// =======================================================================

#[tokio::test]
async fn test_config_get_with_cluster_config_and_master_enabled() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    ensure_config_dir(dir.path());
    std::fs::write(
        dir.path().join("config/config.cluster.json"),
        r#"{"enabled":false,"port":11949}"#,
    )
    .unwrap();
    write_main_config(dir.path(), r#"{"cluster":{"enabled":true}}"#);
    let ctx = make_ctx(&dir);

    let result = handler
        .handle_cmd("config.get", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["enabled"], false);
    assert_eq!(result["port"], 11949);
    assert_eq!(result["master_enabled"], true);
}

#[tokio::test]
async fn test_config_get_invalid_cluster_config_returns_empty() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    ensure_config_dir(dir.path());
    // Invalid JSON content
    std::fs::write(
        dir.path().join("config/config.cluster.json"),
        "this is not json",
    )
    .unwrap();
    let ctx = make_ctx(&dir);

    // Invalid config triggers Err path.
    let err = handler
        .handle_cmd("config.get", None, &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("invalid cluster config"));
}

#[tokio::test]
async fn test_config_get_main_config_invalid_json_skips_master_enabled() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_main_config(dir.path(), "not valid json");
    let ctx = make_ctx(&dir);

    let result = handler
        .handle_cmd("config.get", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    // Invalid main config.json → master_enabled absent (since parse failed → None).
    assert!(result.get("master_enabled").is_none() || result["master_enabled"] == false);
}

#[tokio::test]
async fn test_config_get_peers_toml_partial_identity() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    ensure_cluster_dir(dir.path());
    // peers.toml with only some identity fields
    std::fs::write(
        dir.path().join("cluster/peers.toml"),
        r#"[node]
id = "n2"
name = "beta"
"#,
    )
    .unwrap();
    let ctx = make_ctx(&dir);

    let result = handler
        .handle_cmd("config.get", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["node_id"], "n2");
    assert_eq!(result["name"], "beta");
}

#[tokio::test]
async fn test_config_get_with_home_no_main_config_master_false() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    // No config.json present.
    let result = handler
        .handle_cmd("config.get", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["master_enabled"], false);
}

#[tokio::test]
async fn test_config_get_no_home_skips_master_enabled() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_no_home(&dir);

    let result = handler
        .handle_cmd("config.get", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(result.get("master_enabled").is_none());
}

// =======================================================================
// config.save — additional validation cases
// =======================================================================

#[tokio::test]
async fn test_config_save_discovery_port_above_range() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let data = serde_json::json!({"cluster": {"discovery_port": 100000}});
    let err = handler
        .handle_cmd("config.save", Some(data), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("discovery_port must be between 1 and 65535"));
}

#[tokio::test]
async fn test_config_save_rpc_port_zero_rejected() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let data = serde_json::json!({"cluster": {"rpc_port": 0}});
    let err = handler
        .handle_cmd("config.save", Some(data), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("rpc_port must be between 1 and 65535"));
}

#[tokio::test]
async fn test_config_save_discovery_port_at_max_boundary() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let data = serde_json::json!({"cluster": {"discovery_port": 65535, "rpc_port": 1024}});
    let result = handler
        .handle_cmd("config.save", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["saved"], true);
}

#[tokio::test]
async fn test_config_save_only_rpc_port_no_discovery() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let data = serde_json::json!({"cluster": {"rpc_port": 20000}});
    let result = handler
        .handle_cmd("config.save", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["saved"], true);
}

#[tokio::test]
async fn test_config_save_only_discovery_port_no_rpc() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let data = serde_json::json!({"cluster": {"discovery_port": 30000}});
    let result = handler
        .handle_cmd("config.save", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["saved"], true);
}

#[tokio::test]
async fn test_config_save_no_cluster_section_succeeds() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let data = serde_json::json!({"enabled": true, "token": "abc"});
    let result = handler
        .handle_cmd("config.save", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["saved"], true);
    let written = std::fs::read_to_string(dir.path().join("config/config.cluster.json")).unwrap();
    assert!(written.contains("\"token\""));
}

#[tokio::test]
async fn test_config_save_empty_cluster_object() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let data = serde_json::json!({"cluster": {}});
    let result = handler
        .handle_cmd("config.save", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["saved"], true);
}

#[tokio::test]
async fn test_config_save_overwrites_existing_file() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    ensure_config_dir(dir.path());
    std::fs::write(
        dir.path().join("config/config.cluster.json"),
        r#"{"old":true}"#,
    )
    .unwrap();
    let ctx = make_ctx(&dir);
    let data = serde_json::json!({"new": true});
    handler
        .handle_cmd("config.save", Some(data), &ctx)
        .await
        .unwrap();
    let written = std::fs::read_to_string(dir.path().join("config/config.cluster.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&written).unwrap();
    assert_eq!(parsed["new"], true);
    assert!(parsed.get("old").is_none());
}

// =======================================================================
// config.set_master_enabled — additional branches
// =======================================================================

#[tokio::test]
async fn test_set_master_enabled_invalid_json_in_config_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_main_config(dir.path(), "not json");
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd(
            "config.set_master_enabled",
            Some(serde_json::json!({"enabled": true})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("invalid config.json"));
}

#[tokio::test]
async fn test_set_master_enabled_invalid_type_for_enabled() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    write_main_config(dir.path(), r#"{"cluster":{}}"#);
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd(
            "config.set_master_enabled",
            Some(serde_json::json!({"enabled": "yes"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("missing or invalid 'enabled'"));
}

// =======================================================================
// peers command — additional branches
// =======================================================================

#[tokio::test]
async fn test_peers_empty_file_returns_empty_string() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    ensure_cluster_dir(dir.path());
    std::fs::write(dir.path().join("cluster/peers.toml"), "").unwrap();
    let ctx = make_ctx(&dir);

    let result = handler.handle_cmd("peers", None, &ctx).await.unwrap().unwrap();
    assert_eq!(result["format"], "toml");
    assert_eq!(result["peers"], "");
}

#[tokio::test]
async fn test_peers_multiline_file_preserved() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    ensure_cluster_dir(dir.path());
    let raw = "[node]\nid=\"n1\"\nname=\"alpha\"\n\n[peers.r1]\naddress=\"1.2.3.4:5\"\n";
    std::fs::write(dir.path().join("cluster/peers.toml"), raw).unwrap();
    let ctx = make_ctx(&dir);

    let result = handler.handle_cmd("peers", None, &ctx).await.unwrap().unwrap();
    assert_eq!(result["peers"], raw);
}

// =======================================================================
// identity.save_file — additional branches
// =======================================================================

#[tokio::test]
async fn test_identity_save_file_creates_cluster_dir_if_missing() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    // No cluster/ dir yet.
    let data = serde_json::json!({"file": "IDENTITY.md", "content": "hi"});
    let result = handler
        .handle_cmd("identity.save_file", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["saved"], true);
    assert!(dir.path().join("cluster/IDENTITY.md").exists());
}

#[tokio::test]
async fn test_identity_save_file_path_traversal_rejected() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    // The allowed list is IDENTITY.md / SOUL.md, so traversal filename is blocked
    // at the allowed-list check before path resolution.
    let data = serde_json::json!({"file": "../IDENTITY.md", "content": "x"});
    let err = handler
        .handle_cmd("identity.save_file", Some(data), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("not allowed"));
}

#[tokio::test]
async fn test_identity_save_file_lowercase_rejected() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({"file": "identity.md", "content": "x"});
    let err = handler
        .handle_cmd("identity.save_file", Some(data), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("not allowed"));
}

#[tokio::test]
async fn test_identity_save_file_overwrites_existing() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    ensure_cluster_dir(dir.path());
    std::fs::write(dir.path().join("cluster/IDENTITY.md"), "old").unwrap();
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({"file": "IDENTITY.md", "content": "new"});
    handler
        .handle_cmd("identity.save_file", Some(data), &ctx)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("cluster/IDENTITY.md")).unwrap(),
        "new"
    );
}

// =======================================================================
// identity.get_files — additional
// =======================================================================

#[tokio::test]
async fn test_identity_get_files_partial_files() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    ensure_cluster_dir(dir.path());
    std::fs::write(dir.path().join("cluster/IDENTITY.md"), "id-only").unwrap();
    // SOUL.md intentionally missing.
    let ctx = make_ctx(&dir);

    let result = handler
        .handle_cmd("identity.get_files", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["identity"], "id-only");
    assert_eq!(result["soul"], "");
}

// =======================================================================
// firewall.check — additional branches
// =======================================================================

#[tokio::test]
async fn test_firewall_check_invalid_config_falls_back_to_defaults() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    ensure_config_dir(dir.path());
    std::fs::write(
        dir.path().join("config/config.cluster.json"),
        "not json",
    )
    .unwrap();
    let ctx = make_ctx(&dir);

    let result = handler
        .handle_cmd("firewall.check", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    // Invalid JSON → defaults
    assert_eq!(result["udp_port"], 11949);
    assert_eq!(result["tcp_port"], 21949);
}

#[tokio::test]
async fn test_firewall_check_partial_config_uses_defaults_for_missing_ports() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    ensure_config_dir(dir.path());
    std::fs::write(
        dir.path().join("config/config.cluster.json"),
        r#"{"port":11111}"#,
    )
    .unwrap();
    let ctx = make_ctx(&dir);

    let result = handler
        .handle_cmd("firewall.check", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["udp_port"], 11111);
    // rpc_port missing → default
    assert_eq!(result["tcp_port"], 21949);
}

#[tokio::test]
async fn test_firewall_check_all_pass_field_present() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let result = handler
        .handle_cmd("firewall.check", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    // all_pass is a bool regardless of test outcomes.
    assert!(result["all_pass"].is_boolean());
    assert!(result["platform"].is_string());
}

#[tokio::test]
async fn test_firewall_check_tests_have_name_and_pass() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let result = handler
        .handle_cmd("firewall.check", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    let tests = result["tests"].as_array().unwrap();
    for t in tests {
        assert!(t["name"].is_string());
        assert!(t["pass"].is_boolean());
        assert!(t["detail"].is_string());
    }
}

// =======================================================================
// firewall.add_rules — additional branches
// =======================================================================

#[tokio::test]
async fn test_firewall_add_rules_zero_tcp_port_rejected() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({"udp_port": 100, "tcp_port": 0});
    let err = handler
        .handle_cmd("firewall.add_rules", Some(data), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("端口范围无效"));
}

// NOTE: `firewall.add_rules` happy-path tests are intentionally omitted.
// The platform-specific branches (`add_platform_firewall_rules`) invoke
// external processes (netsh on Windows, ufw/iptables on Linux) which can
// block on UAC prompts or hang the test runner. The deterministic error
// paths (zero ports, missing data) are covered above.

// =======================================================================
// events.recent — additional branches
// =======================================================================

#[tokio::test]
async fn test_events_recent_limit_zero() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_log_dir(&dir);

    let result = handler
        .handle_cmd(
            "events.recent",
            Some(serde_json::json!({"limit": 0})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    // Empty log dir → 0 events regardless.
    assert!(result["events"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_events_recent_invalid_limit_uses_default() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_log_dir(&dir);

    // String limit → ignored, default 50.
    let result = handler
        .handle_cmd(
            "events.recent",
            Some(serde_json::json!({"limit": "not-a-number"})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert!(result["events"].as_array().unwrap().len() <= 50);
}

#[tokio::test]
async fn test_events_recent_large_limit_cap() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_log_dir(&dir);

    let result = handler
        .handle_cmd(
            "events.recent",
            Some(serde_json::json!({"limit": 10000})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    // Empty dir → 0 events regardless of huge limit.
    assert!(result["events"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_events_recent_data_null_uses_default_limit() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_log_dir(&dir);

    let result = handler
        .handle_cmd("events.recent", Some(serde_json::Value::Null), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(result["events"].as_array().unwrap().is_empty());
}

// =======================================================================
// traces — additional branches
// =======================================================================

#[tokio::test]
async fn test_traces_with_empty_log_dir_returns_empty_array() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_log_dir(&dir);

    let result = handler.handle_cmd("traces", None, &ctx).await.unwrap().unwrap();
    let traces = result["traces"].as_array();
    assert!(traces.is_some());
    assert!(traces.unwrap().is_empty());
}

// =======================================================================
// runtime.start / runtime.stop — additional error paths
// =======================================================================

#[tokio::test]
async fn test_runtime_start_no_workspace_still_requires_service() {
    let handler = cluster::ClusterHandler::new();
    let ctx = make_ctx_no_workspace();
    // No workspace → update_cluster_config_enabled is skipped, then service check.
    let err = handler
        .handle_cmd("runtime.start", None, &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "cluster service not available");
}

#[tokio::test]
async fn test_runtime_stop_no_workspace_still_requires_service() {
    let handler = cluster::ClusterHandler::new();
    let ctx = make_ctx_no_workspace();
    let err = handler
        .handle_cmd("runtime.stop", None, &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "cluster service not available");
}

// =======================================================================
// nodes.* — additional field-validation paths
// =======================================================================

#[tokio::test]
async fn test_nodes_detail_missing_data_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd("nodes.detail", None, &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing data");
}

#[tokio::test]
async fn test_nodes_detail_missing_node_id_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd("nodes.detail", Some(serde_json::json!({})), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing node_id");
}

#[tokio::test]
async fn test_nodes_remove_missing_data_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd("nodes.remove", None, &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing data");
}

#[tokio::test]
async fn test_nodes_refresh_missing_data_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd("nodes.refresh", None, &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing data");
}

#[tokio::test]
async fn test_nodes_refresh_missing_node_id_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd("nodes.refresh", Some(serde_json::json!({})), &ctx)
        .await
        .unwrap_err();
    // require_cluster runs first inside nodes_refresh.
    assert_eq!(err, "cluster not available");
}

// =======================================================================
// tasks.* — additional field-validation paths
// =======================================================================

#[tokio::test]
async fn test_tasks_list_with_status_filter_no_cluster_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd(
            "tasks.list",
            Some(serde_json::json!({"status_filter": "running"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert_eq!(err, "cluster not available");
}

#[tokio::test]
async fn test_tasks_list_with_pagination_params_no_cluster_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd(
            "tasks.list",
            Some(serde_json::json!({"offset": 10, "limit": 5})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert_eq!(err, "cluster not available");
}

#[tokio::test]
async fn test_tasks_submit_missing_data_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd("tasks.submit", None, &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing data");
}

// =======================================================================
// topology / snapshots — no-cluster errors (defensive)
// =======================================================================

#[tokio::test]
async fn test_snapshots_list_missing_data_ignored_no_cluster_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd("snapshots.list", None, &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "cluster not available");
}

// =======================================================================
// status command — additional edge cases
// =======================================================================

#[tokio::test]
async fn test_status_no_home_with_workspace_works() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_no_home(&dir);

    // status doesn't use home, only workspace.
    let result = handler.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
    assert_eq!(result["running"], false);
}

#[tokio::test]
async fn test_runtime_status_alias_with_cluster_config_present() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    ensure_config_dir(dir.path());
    std::fs::write(
        dir.path().join("config/config.cluster.json"),
        r#"{"enabled":true}"#,
    )
    .unwrap();
    let ctx = make_ctx(&dir);

    let result = handler
        .handle_cmd("runtime.status", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["config"]["enabled"], true);
    assert_eq!(result["config_exists"], true);
}

// =======================================================================
// module_name sanity (already in extra, kept here for module completeness)
// =======================================================================

#[tokio::test]
async fn test_handler_construction_does_not_panic() {
    let _ = cluster::ClusterHandler::new();
}

#[tokio::test]
async fn test_unknown_command_with_data_still_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let err = handler
        .handle_cmd("bogus.cmd", Some(serde_json::json!({"x":1})), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "unknown command: cluster.bogus.cmd");
}

// =======================================================================
// config.get with both cluster config and peers.toml identity
// =======================================================================

#[tokio::test]
async fn test_config_get_combined_cluster_config_and_peers_identity() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    ensure_config_dir(dir.path());
    ensure_cluster_dir(dir.path());
    std::fs::write(
        dir.path().join("config/config.cluster.json"),
        r#"{"enabled":true,"port":11949}"#,
    )
    .unwrap();
    std::fs::write(
        dir.path().join("cluster/peers.toml"),
        r#"[node]
id = "combo-1"
name = "combined"
role = "manager"
category = "edge"
tags = ["a", "b"]
"#,
    )
    .unwrap();
    let ctx = make_ctx(&dir);

    let result = handler
        .handle_cmd("config.get", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["enabled"], true);
    assert_eq!(result["node_id"], "combo-1");
    assert_eq!(result["name"], "combined");
    assert_eq!(result["role"], "manager");
    assert_eq!(result["category"], "edge");
}

// =======================================================================
// config.save atomic-write fallback coverage
// =======================================================================

#[tokio::test]
async fn test_config_save_to_nested_path_succeeds() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    // config/ dir doesn't exist yet — handler creates it.
    let data = serde_json::json!({"enabled": true});
    let result = handler
        .handle_cmd("config.save", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["saved"], true);
    assert!(dir.path().join("config/config.cluster.json").exists());
}

// =======================================================================
// Additional runtime.status fallback — config exists but unreadable
// =======================================================================

#[tokio::test]
async fn test_status_fallback_config_exists_but_corrupt() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    ensure_config_dir(dir.path());
    std::fs::write(dir.path().join("config/config.cluster.json"), "garbage").unwrap();
    let ctx = make_ctx(&dir);

    // The runtime_status fallback uses serde_json::from_str::<Value>().ok()
    // which silently converts to None on parse failure → config is null,
    // but config_exists is still true (file exists).
    let result = handler.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
    assert_eq!(result["config_exists"], true);
    assert!(result["config"].is_null());
}

// =======================================================================
// nodes.add — peers.toml corrupt-file fallback path
// =======================================================================

#[tokio::test]
async fn test_nodes_add_corrupt_peers_toml_falls_back_to_fresh() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    ensure_cluster_dir(dir.path());
    // Write garbage where peers.toml should be.
    std::fs::write(dir.path().join("cluster/peers.toml"), "this is = = invalid toml").unwrap();
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({
        "address": "10.0.0.30:12000",
        "id": "post-corrupt",
    });
    // append_peer_to_file treats corrupt content as fresh table.
    let result = handler
        .handle_cmd("nodes.add", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["added"], true);
}

// =======================================================================
// Cluster-backed tests — exercise runtime data commands
// =======================================================================

use nemesis_cluster::cluster::Cluster;
use nemesis_cluster::types::{ClusterConfig, ExtendedNodeInfo, NodeStatus};
use nemesis_types::cluster::{NodeInfo, NodeRole};

fn make_ctx_with_cluster(dir: &tempfile::TempDir) -> RequestContext {
    let ws = dir.path().to_string_lossy().to_string();
    let cluster = Arc::new(Cluster::with_workspace(
        ClusterConfig::default(),
        dir.path().to_path_buf(),
    ));
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
        cluster: Some(cluster),
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: None,
        chat_secret_store: std::sync::Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::in_memory()),
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        internal_cmd_tx: None,
    });
    RequestContext {
        session_id: "test-session".to_string(),
        chat_id: "test-chat".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
    }
}

fn make_ctx_with_cluster_and_log_dir(dir: &tempfile::TempDir) -> RequestContext {
    let ws = dir.path().to_string_lossy().to_string();
    let log_dir = dir.path().join("cluster_logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    let cluster = Arc::new(Cluster::with_workspace(
        ClusterConfig::default(),
        dir.path().to_path_buf(),
    ));
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
        cluster: Some(cluster),
        cluster_service: None,
        cluster_log_dir: Some(log_dir.to_string_lossy().to_string()),
        workflow_engine: None,
        chat_secret_store: std::sync::Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::in_memory()),
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        internal_cmd_tx: None,
    });
    RequestContext {
        session_id: "test-session".to_string(),
        chat_id: "test-chat".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
    }
}

fn sample_node(id: &str, name: &str, role: NodeRole, online: bool) -> ExtendedNodeInfo {
    ExtendedNodeInfo {
        base: NodeInfo {
            id: id.to_string(),
            name: name.to_string(),
            role,
            address: "10.0.0.1:12000".to_string(),
            category: "edge".to_string(),
            last_seen: String::new(),
        },
        status: if online { NodeStatus::Online } else { NodeStatus::Offline },
        capabilities: vec!["cluster".to_string()],
        addresses: vec!["10.0.0.1".to_string()],
        node_type: "agent".to_string(),
    }
}

#[tokio::test]
async fn test_rt_status_with_cluster_empty() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_cluster(&dir);

    let result = handler.handle_cmd("runtime.status", None, &ctx).await.unwrap().unwrap();
    assert_eq!(result["running"], false);
    assert_eq!(result["total_nodes"], 0);
    assert_eq!(result["online_nodes"], 0);
    assert_eq!(result["active_tasks"], 0);
    assert_eq!(result["success_rate"], 1.0);
}

#[tokio::test]
async fn test_rt_status_with_cluster_and_log_dir() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_cluster_and_log_dir(&dir);

    let result = handler.handle_cmd("runtime.status", None, &ctx).await.unwrap().unwrap();
    assert!(result["recent_events"].as_array().is_some());
}

#[tokio::test]
async fn test_nodes_list_empty_cluster() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_cluster(&dir);

    let result = handler.handle_cmd("nodes.list", None, &ctx).await.unwrap().unwrap();
    assert!(result["nodes"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_nodes_list_returns_registered_nodes() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_cluster(&dir);
    let c = ctx.state.cluster.as_ref().unwrap();
    c.register_node(sample_node("n1", "alpha", NodeRole::Worker, true));
    c.register_node(sample_node("n2", "beta", NodeRole::Master, false));

    let result = handler.handle_cmd("nodes.list", None, &ctx).await.unwrap().unwrap();
    let nodes = result["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 2);
    let roles: Vec<&str> = nodes.iter().map(|n| n["role"].as_str().unwrap()).collect();
    assert!(roles.contains(&"manager"));
    assert!(roles.contains(&"worker"));
}

#[tokio::test]
async fn test_nodes_list_marks_local_node() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_cluster(&dir);
    let c = ctx.state.cluster.as_ref().unwrap();
    let local_id = c.node_id().to_string();
    c.register_node(sample_node(&local_id, "self", NodeRole::Worker, true));

    let result = handler.handle_cmd("nodes.list", None, &ctx).await.unwrap().unwrap();
    let nodes = result["nodes"].as_array().unwrap();
    let local = nodes.iter().find(|n| n["isLocal"] == true).unwrap();
    assert_eq!(local["id"], local_id);
}

#[tokio::test]
async fn test_nodes_list_with_log_dir_includes_uptime() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_cluster_and_log_dir(&dir);
    ctx.state.cluster.as_ref().unwrap()
        .register_node(sample_node("n1", "alpha", NodeRole::Worker, true));

    let result = handler.handle_cmd("nodes.list", None, &ctx).await.unwrap().unwrap();
    let nodes = result["nodes"].as_array().unwrap();
    assert!(nodes[0].get("taskCount").is_some());
    assert!(nodes[0].get("uptime").map(|v| v.is_string()).unwrap_or(false));
}

#[tokio::test]
async fn test_nodes_detail_unknown_node_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_cluster(&dir);

    let err = handler
        .handle_cmd("nodes.detail", Some(serde_json::json!({"node_id": "missing"})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("node not found"));
}

#[tokio::test]
async fn test_nodes_detail_known_node_returns_info() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_cluster(&dir);
    ctx.state.cluster.as_ref().unwrap()
        .register_node(sample_node("n7", "gamma", NodeRole::Master, true));

    let result = handler
        .handle_cmd("nodes.detail", Some(serde_json::json!({"node_id": "n7"})), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["id"], "n7");
    assert_eq!(result["name"], "gamma");
    assert_eq!(result["role"], "manager");
    assert_eq!(result["online"], true);
}

#[tokio::test]
async fn test_nodes_remove_unknown_returns_false() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_cluster(&dir);

    let result = handler
        .handle_cmd("nodes.remove", Some(serde_json::json!({"node_id": "ghost"})), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["removed"], false);
}

#[tokio::test]
async fn test_nodes_remove_known_returns_true() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_cluster(&dir);
    let c = ctx.state.cluster.as_ref().unwrap();
    c.register_node(sample_node("n8", "delta", NodeRole::Worker, true));

    let result = handler
        .handle_cmd("nodes.remove", Some(serde_json::json!({"node_id": "n8"})), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["removed"], true);
    assert!(c.get_node_info("n8").is_none());
}

#[tokio::test]
async fn test_tasks_list_empty_cluster() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_cluster(&dir);

    let result = handler.handle_cmd("tasks.list", None, &ctx).await.unwrap().unwrap();
    assert!(result["tasks"].as_array().unwrap().is_empty());
    assert_eq!(result["total"], 0);
    let stats = &result["stats"];
    assert_eq!(stats["queued"], 0);
    assert_eq!(stats["completed"], 0);
    assert_eq!(stats["failed"], 0);
}

#[tokio::test]
async fn test_tasks_list_with_submitted_task() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_cluster(&dir);
    let c = ctx.state.cluster.as_ref().unwrap();
    let _ = c.submit_task("dashboard_test", serde_json::json!({"content":"hi"}), "dashboard", "s");

    let result = handler.handle_cmd("tasks.list", None, &ctx).await.unwrap().unwrap();
    let tasks = result["tasks"].as_array().unwrap();
    assert!(!tasks.is_empty());
    assert_eq!(tasks[0]["status"], "queued");
    assert_eq!(result["stats"]["queued"], 1);
}

#[tokio::test]
async fn test_tasks_list_pagination() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_cluster(&dir);
    let c = ctx.state.cluster.as_ref().unwrap();
    for i in 0..5 {
        c.submit_task("t", serde_json::json!({"i":i}), "dashboard", "s");
    }

    let result = handler
        .handle_cmd("tasks.list", Some(serde_json::json!({"offset":1,"limit":2})), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["total"], 5);
    assert_eq!(result["offset"], 1);
    assert_eq!(result["limit"], 2);
    assert_eq!(result["tasks"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn test_tasks_list_status_filter_no_match() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_cluster(&dir);
    let c = ctx.state.cluster.as_ref().unwrap();
    c.submit_task("t", serde_json::json!({}), "dashboard", "s");

    let result = handler
        .handle_cmd("tasks.list", Some(serde_json::json!({"status_filter":"completed"})), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(result["tasks"].as_array().unwrap().is_empty());
    assert_eq!(result["total"], 0);
}

#[tokio::test]
async fn test_tasks_detail_unknown_task_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_cluster(&dir);

    let err = handler
        .handle_cmd("tasks.detail", Some(serde_json::json!({"task_id":"nope"})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("task not found"));
}

#[tokio::test]
async fn test_tasks_detail_known_task_returns_payload() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_cluster(&dir);
    let c = ctx.state.cluster.as_ref().unwrap();
    let task_id = c.submit_task("dashboard_test", serde_json::json!({"content":"x"}), "dashboard", "s");

    let result = handler
        .handle_cmd("tasks.detail", Some(serde_json::json!({"task_id":task_id})), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["id"], task_id);
    assert_eq!(result["status"], "queued");
    assert_eq!(result["action"], "dashboard_test");
}

#[tokio::test]
async fn test_tasks_cancel_unknown_returns_false() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_cluster(&dir);

    let result = handler
        .handle_cmd("tasks.cancel", Some(serde_json::json!({"task_id":"ghost"})), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["cancelled"], false);
}

#[tokio::test]
async fn test_tasks_cancel_known_returns_true() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_cluster(&dir);
    let c = ctx.state.cluster.as_ref().unwrap();
    let task_id = c.submit_task("t", serde_json::json!({}), "dashboard", "s");

    let result = handler
        .handle_cmd("tasks.cancel", Some(serde_json::json!({"task_id":task_id})), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["cancelled"], true);
}

#[tokio::test]
async fn test_topology_empty_cluster() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_cluster(&dir);

    let result = handler.handle_cmd("topology", None, &ctx).await.unwrap().unwrap();
    assert!(result["nodes"].as_array().unwrap().is_empty());
    assert!(result["connections"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_topology_with_online_nodes_full_mesh() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_cluster(&dir);
    let c = ctx.state.cluster.as_ref().unwrap();
    c.register_node(sample_node("n1", "a", NodeRole::Worker, true));
    c.register_node(sample_node("n2", "b", NodeRole::Worker, true));
    c.register_node(sample_node("n3", "c", NodeRole::Worker, false));

    let result = handler.handle_cmd("topology", None, &ctx).await.unwrap().unwrap();
    assert_eq!(result["nodes"].as_array().unwrap().len(), 3);
    // No log dir → full mesh fallback for 2 online nodes → 1 connection
    assert_eq!(result["connections"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn test_snapshots_list_empty_cache_dir() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_cluster(&dir);

    let result = handler.handle_cmd("snapshots.list", None, &ctx).await.unwrap().unwrap();
    assert!(result["snapshots"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_snapshots_list_with_files() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_cluster(&dir);
    let c = ctx.state.cluster.as_ref().unwrap();
    let cache_dir = c.continuation_store().cache_dir();
    std::fs::create_dir_all(&cache_dir).unwrap();
    std::fs::write(cache_dir.join("task1.json"), r#"{"foo":"bar"}"#).unwrap();
    std::fs::write(cache_dir.join("task2.json"), "{}").unwrap();
    std::fs::write(cache_dir.join("readme.txt"), "hi").unwrap();

    let result = handler.handle_cmd("snapshots.list", None, &ctx).await.unwrap().unwrap();
    let snaps = result["snapshots"].as_array().unwrap();
    assert_eq!(snaps.len(), 2);
    let names: Vec<&str> = snaps.iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"task1.json"));
    assert!(names.contains(&"task2.json"));
}

#[tokio::test]
async fn test_snapshots_cleanup_removes_all() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_cluster(&dir);
    let c = ctx.state.cluster.as_ref().unwrap();
    let cache_dir = c.continuation_store().cache_dir();
    std::fs::create_dir_all(&cache_dir).unwrap();
    std::fs::write(cache_dir.join("old.json"), "{}").unwrap();

    let result = handler.handle_cmd("snapshots.cleanup", None, &ctx).await.unwrap().unwrap();
    assert_eq!(result["removed"], 1);
}

#[tokio::test]
async fn test_node_update_identity_name_only() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_cluster(&dir);

    let result = handler
        .handle_cmd("node.update_identity", Some(serde_json::json!({"name":"new-name"})), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["name"], "new-name");
    assert_eq!(result["current_name"], "new-name");
}

#[tokio::test]
async fn test_node_update_identity_role_worker() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_cluster(&dir);

    let result = handler
        .handle_cmd("node.update_identity", Some(serde_json::json!({"role":"worker"})), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["role"], "worker");
}

#[tokio::test]
async fn test_node_update_identity_role_manager() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_cluster(&dir);

    let result = handler
        .handle_cmd("node.update_identity", Some(serde_json::json!({"role":"manager"})), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["role"], "manager");
}

#[tokio::test]
async fn test_node_update_identity_invalid_role_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_cluster(&dir);

    let err = handler
        .handle_cmd("node.update_identity", Some(serde_json::json!({"role":"supervisor"})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("role must be 'manager' or 'worker'"));
}

#[tokio::test]
async fn test_node_update_identity_empty_name_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_cluster(&dir);

    let err = handler
        .handle_cmd("node.update_identity", Some(serde_json::json!({"name":"   "})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("name cannot be empty"));
}

#[tokio::test]
async fn test_node_update_identity_empty_category_errors() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_cluster(&dir);

    let err = handler
        .handle_cmd("node.update_identity", Some(serde_json::json!({"category":""})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("category cannot be empty"));
}

#[tokio::test]
async fn test_node_update_identity_category() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_cluster(&dir);

    let result = handler
        .handle_cmd("node.update_identity", Some(serde_json::json!({"category":"production"})), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["category"], "production");
}

#[tokio::test]
async fn test_node_update_identity_tags_filters_empty() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_cluster(&dir);

    let result = handler
        .handle_cmd("node.update_identity", Some(serde_json::json!({"tags":["prod","edge",""]})), &ctx)
        .await
        .unwrap()
        .unwrap();
    let tags: Vec<&str> = result["tags"].as_array().unwrap()
        .iter().map(|t| t.as_str().unwrap()).collect();
    assert_eq!(tags, vec!["prod", "edge"]);
}

#[tokio::test]
async fn test_node_update_identity_all_fields() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_cluster(&dir);

    let result = handler
        .handle_cmd(
            "node.update_identity",
            Some(serde_json::json!({"name":"all-in-one","role":"manager","category":"edge","tags":["a","b"]})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["name"], "all-in-one");
    assert_eq!(result["role"], "manager");
    assert_eq!(result["category"], "edge");
}

#[tokio::test]
async fn test_node_update_identity_no_fields_returns_current() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_cluster(&dir);

    let result = handler
        .handle_cmd("node.update_identity", Some(serde_json::json!({})), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(result.get("current_name").is_some());
    assert!(result.get("current_role").is_some());
}

#[tokio::test]
async fn test_node_update_identity_persists_to_peers_toml() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("cluster")).unwrap();
    std::fs::write(
        dir.path().join("cluster/peers.toml"),
        "[node]\nid = \"existing\"\nname = \"old\"\nrole = \"worker\"\n",
    )
    .unwrap();
    let ctx = make_ctx_with_cluster(&dir);

    handler
        .handle_cmd("node.update_identity", Some(serde_json::json!({"name":"renamed"})), &ctx)
        .await
        .unwrap();

    let written = std::fs::read_to_string(dir.path().join("cluster/peers.toml")).unwrap();
    assert!(written.contains("renamed"));
}

#[tokio::test]
async fn test_config_get_with_cluster_returns_runtime_identity() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_cluster(&dir);

    let result = handler.handle_cmd("config.get", None, &ctx).await.unwrap().unwrap();
    assert!(result.get("node_id").is_some());
    assert!(result.get("name").is_some());
    assert!(result.get("role").is_some());
    assert!(result.get("capabilities").map(|v| v.is_array()).unwrap_or(false));
}

#[tokio::test]
async fn test_topology_with_log_dir_uses_rpc_connections() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_cluster_and_log_dir(&dir);
    ctx.state.cluster.as_ref().unwrap()
        .register_node(sample_node("n1", "a", NodeRole::Worker, true));

    let result = handler.handle_cmd("topology", None, &ctx).await.unwrap().unwrap();
    assert!(result["connections"].as_array().unwrap().is_empty());
    assert!(result["traces"].as_array().is_some());
}
