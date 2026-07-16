use super::*;
use axum::http::HeaderValue;

fn headers_with(sig: Option<&str>) -> HeaderMap {
    let mut h = HeaderMap::new();
    if let Some(s) = sig {
        h.insert("X-Signature", HeaderValue::from_str(s).unwrap());
    }
    h
}

// ---- verify_signature ----------------------------------------------

#[test]
fn hex_signature_validates_when_secret_matches() {
    let body = b"hello world";
    let secret = b"s3cret";
    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(secret).unwrap();
    mac.update(body);
    let hex_sig = hex_str(&mac);
    let h = headers_with(Some(&hex_sig));
    assert!(verify_signature(&h, body, secret).is_ok());
}

/// Compute the expected HMAC hex string without pulling an external hex crate.
fn hex_str(mac: &Hmac<Sha256>) -> String {
    let bytes = mac.clone().finalize().into_bytes();
    hex(&bytes)
}

/// Tiny local hex encoder so the test doesn't pull in the `hex` crate.
fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

#[test]
fn hex_signature_with_sha256_prefix_validates() {
    let body = br#"{"event":"push"}"#;
    let secret = b"kw";
    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(secret).unwrap();
    mac.update(body);
    let hex_sig = hex(&mac.finalize().into_bytes());
    let with_prefix = format!("sha256={}", hex_sig);
    let h = headers_with(Some(&with_prefix));
    assert!(verify_signature(&h, body, secret).is_ok());
}

#[test]
fn uppercase_hex_signature_validates() {
    let body = b"abc";
    let secret = b"k";
    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(secret).unwrap();
    mac.update(body);
    let hex_sig = hex(&mac.finalize().into_bytes()).to_uppercase();
    let h = headers_with(Some(&hex_sig));
    assert!(verify_signature(&h, body, secret).is_ok());
}

#[test]
fn mismatched_signature_is_rejected() {
    let body = b"hello";
    let secret = b"k";
    let h = headers_with(Some("deadbeef".repeat(16).as_str()));
    let err = verify_signature(&h, body, secret).unwrap_err();
    assert!(err.contains("mismatch") || err.contains("length"));
}

#[test]
fn missing_signature_header_is_rejected() {
    let h = headers_with(None);
    let err = verify_signature(&h, b"body", b"k").unwrap_err();
    assert!(err.contains("missing"));
}

#[test]
fn invalid_hex_is_rejected() {
    let h = headers_with(Some("nothex!"));
    let err = verify_signature(&h, b"body", b"k").unwrap_err();
    assert!(err.contains("not valid hex") || err.contains("length"));
}

// ---- WebhookRateLimiter --------------------------------------------

#[tokio::test]
async fn rate_limiter_allows_until_max_then_rejects() {
    let limiter = WebhookRateLimiter::new();
    let ip: IpAddr = "10.0.0.1".parse().unwrap();
    for _ in 0..WEBHOOK_RATE_MAX {
        assert!(limiter.check(ip).await.is_ok());
    }
    let result = limiter.check(ip).await;
    assert!(result.is_err(), "should reject after exceeding max");
    let retry_after = result.unwrap_err();
    assert!(retry_after <= WEBHOOK_RATE_WINDOW);
}

#[tokio::test]
async fn rate_limiter_separates_ips() {
    let limiter = WebhookRateLimiter::new();
    let ip_a: IpAddr = "10.0.0.1".parse().unwrap();
    let ip_b: IpAddr = "10.0.0.2".parse().unwrap();
    for _ in 0..WEBHOOK_RATE_MAX {
        limiter.check(ip_a).await.unwrap();
    }
    // Different IP still allowed
    assert!(limiter.check(ip_b).await.is_ok());
    // Original IP still blocked
    assert!(limiter.check(ip_a).await.is_err());
}

// ---- handle_workflow_run auth + timeout (1c-E6) -------------------

/// Build a minimal AppState for workflow handler tests. Most fields
/// are unused by run/list/etc. — they just need to satisfy the struct.
fn make_test_state(auth_token: &str) -> Arc<AppState> {
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::time::Instant;

    Arc::new(AppState {
        auth_token: auth_token.to_string(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: None,
        home: None,
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new(String::new())),
        model_base: Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: Arc::new(AtomicBool::new(false)),
        event_hub: Arc::new(crate::events::EventHub::new()),
        running: Arc::new(AtomicBool::new(true)),
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
        chat_secret_store: std::sync::Arc::new(
            nemesis_workflow::chat_secrets::ChatSecretStore::in_memory(),
        ),
        webhook_rate_limiter: Arc::new(WebhookRateLimiter::new()),
        internal_cmd_tx: None,
        estop: None,
        cron: None,
    })
}

fn auth_headers(token: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    if !token.is_empty() {
        h.insert("X-Auth-Token", HeaderValue::from_str(token).unwrap());
    }
    h
}

#[tokio::test]
async fn workflow_run_rejects_unauthenticated_when_token_configured() {
    let state = make_test_state("expected-token");
    let payload = Json(serde_json::json!({"name": "wf", "input": {}}));
    let result = handle_workflow_run(
        axum::extract::State(state),
        auth_headers("wrong-token"),
        payload,
    )
    .await;
    let (status, _) = result.unwrap_err();
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn workflow_run_accepts_authenticated_request_with_correct_token() {
    let state = make_test_state("expected-token");
    let payload = Json(serde_json::json!({"name": "wf", "input": {}}));
    // auth passes, but engine isn't injected — we should see 503 (engine
    // missing) rather than 401. That proves auth passed.
    let result = handle_workflow_run(
        axum::extract::State(state),
        auth_headers("expected-token"),
        payload,
    )
    .await;
    let (status, body) = result.unwrap_err();
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"], "workflow_engine_unavailable");
}

#[tokio::test]
async fn workflow_run_allows_anon_when_no_token_configured() {
    let state = make_test_state("");
    let payload = Json(serde_json::json!({"name": "wf", "input": {}}));
    // No auth header, but no token configured either — should pass auth
    // and hit the engine-missing path.
    let result = handle_workflow_run(axum::extract::State(state), HeaderMap::new(), payload).await;
    let (status, body) = result.unwrap_err();
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"], "workflow_engine_unavailable");
}

// ---- ModuleHandler (WSAPI) ----------------------------------------

use crate::ws_router::{ModuleHandler, RequestContext};

fn make_ctx_no_engine() -> RequestContext {
    let state = make_test_state("");
    RequestContext {
        session_id: "test-session".to_string(),
        chat_id: "test-chat".to_string(),
        workspace: None,
        home: None,
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

fn make_ctx_with_engine(engine: Arc<nemesis_workflow::engine::WorkflowEngine>) -> RequestContext {
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::time::Instant;
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: None,
        home: None,
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new(String::new())),
        model_base: Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: Arc::new(AtomicBool::new(false)),
        event_hub: Arc::new(crate::events::EventHub::new()),
        running: Arc::new(AtomicBool::new(true)),
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
        workflow_engine: Some(engine),
        chat_secret_store: std::sync::Arc::new(
            nemesis_workflow::chat_secrets::ChatSecretStore::in_memory(),
        ),
        webhook_rate_limiter: Arc::new(WebhookRateLimiter::new()),
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

fn build_test_engine() -> Arc<nemesis_workflow::engine::WorkflowEngine> {
    use nemesis_workflow::engine::WorkflowEngine;
    // Build with no real provider/tools — list still works without them.
    Arc::new(WorkflowEngine::new())
}

#[tokio::test]
async fn wsapi_list_returns_registered_workflows() {
    let engine = build_test_engine();
    let wf = nemesis_workflow::types::Workflow {
        name: "wf_alpha".to_string(),
        description: String::new(),
        version: "1.0.0".to_string(),
        triggers: vec![],
        nodes: vec![nemesis_workflow::types::NodeDef {
            id: "start".to_string(),
            node_type: "delay".to_string(),
            config: HashMap::new(),
            depends_on: vec![],
            retry_count: 0,
            timeout: None,
            is_terminal: false,
        }],
        edges: vec![],
        variables: HashMap::new(),
        metadata: HashMap::new(),
    };
    engine.register_workflow(wf).unwrap();
    let ctx = make_ctx_with_engine(engine);
    let handler = WorkflowHandler;
    let result = handler.handle_cmd("list", None, &ctx).await.unwrap();
    let payload = result.unwrap();
    assert_eq!(payload["count"], 1);
    // After Phase 3, workflows[] holds summary objects, not name strings.
    assert_eq!(payload["workflows"][0]["name"], "wf_alpha");
    assert_eq!(payload["workflows"][0]["node_count"], 1);
    // trigger_driver_status is the global capability declaration.
    assert_eq!(payload["trigger_driver_status"]["cron"]["driven"], true);
    assert_eq!(payload["trigger_driver_status"]["event"]["driven"], true);
}

#[tokio::test]
async fn wsapi_unknown_command_returns_error() {
    // Use a ctx with an engine so we get past the engine-presence check
    // and into the command-dispatch match.
    let engine = build_test_engine();
    let ctx = make_ctx_with_engine(engine);
    let handler = WorkflowHandler;
    let err = handler
        .handle_cmd("frobnicate", None, &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("unknown command"), "got: {}", err);
    assert!(err.contains("workflow.frobnicate"));
}

#[tokio::test]
async fn wsapi_list_returns_error_when_engine_missing() {
    let ctx = make_ctx_no_engine();
    let handler = WorkflowHandler;
    let err = handler.handle_cmd("list", None, &ctx).await.unwrap_err();
    assert!(err.contains("not configured"), "got: {}", err);
}

// --- Coverage gap: previously-untested commands (update/delete + error paths) ---

#[tokio::test]
async fn wsapi_update_overwrites_existing_workflow() {
    let engine = build_test_engine();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_engine_and_defs_dir(engine, dir.path());
    let handler = WorkflowHandler;
    let data = serde_json::json!({
        "name": "wf_up",
        "workflow": sample_workflow_def("wf_up"),
    });
    let r = handler
        .handle_cmd("update", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["updated"], true);
    assert_eq!(r["name"], "wf_up");
}

#[tokio::test]
async fn wsapi_update_missing_data() {
    let engine = build_test_engine();
    let ctx = make_ctx_with_engine(engine);
    let handler = WorkflowHandler;
    let err = handler.handle_cmd("update", None, &ctx).await.unwrap_err();
    assert_eq!(err, "missing data");
}

#[tokio::test]
async fn wsapi_update_missing_workflow_field() {
    let engine = build_test_engine();
    let ctx = make_ctx_with_engine(engine);
    let handler = WorkflowHandler;
    let err = handler
        .handle_cmd("update", Some(serde_json::json!({"name": "x"})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("missing field: workflow"));
}

#[tokio::test]
async fn wsapi_delete_removes_workflow() {
    let engine = build_test_engine();
    let dir = tempfile::tempdir().unwrap();
    engine.set_workflow_defs_dir(dir.path().to_path_buf());
    // Persist first so delete has something to remove.
    engine
        .persist_workflow(serde_json::from_value(sample_workflow_def("wf_del")).unwrap())
        .map_err(|e| e.to_string())
        .unwrap();
    let ctx = make_ctx_with_engine(engine);
    let handler = WorkflowHandler;
    let r = handler
        .handle_cmd("delete", Some(serde_json::json!({"name": "wf_del"})), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["deleted"], true);
}

#[tokio::test]
async fn wsapi_delete_missing_data() {
    let engine = build_test_engine();
    let ctx = make_ctx_with_engine(engine);
    let handler = WorkflowHandler;
    let err = handler.handle_cmd("delete", None, &ctx).await.unwrap_err();
    assert_eq!(err, "missing data");
}

#[tokio::test]
async fn wsapi_delete_missing_name() {
    let engine = build_test_engine();
    let ctx = make_ctx_with_engine(engine);
    let handler = WorkflowHandler;
    let err = handler
        .handle_cmd("delete", Some(serde_json::json!({})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("missing field: name"));
}

#[tokio::test]
async fn wsapi_set_chat_password_missing_chat_index() {
    let engine = build_test_engine();
    let ctx = make_ctx_with_engine(engine);
    let handler = WorkflowHandler;
    let err = handler
        .handle_cmd("set_chat_password", Some(serde_json::json!({})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("missing") || err.contains("chat_index") || err.contains("password"));
}

#[tokio::test]
async fn wsapi_verify_chat_password_missing_fields() {
    let engine = build_test_engine();
    let ctx = make_ctx_with_engine(engine);
    let handler = WorkflowHandler;
    let err = handler
        .handle_cmd("verify_chat_password", Some(serde_json::json!({})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("missing") || err.contains("chat_index") || err.contains("password"));
}

#[tokio::test]
async fn wsapi_status_returns_execution_not_found_for_unknown_id() {
    let engine = build_test_engine();
    let ctx = make_ctx_with_engine(engine);
    let handler = WorkflowHandler;
    let data = Some(serde_json::json!({"execution_id": "no_such_id"}));
    let err = handler.handle_cmd("status", data, &ctx).await.unwrap_err();
    assert!(err.contains("execution_not_found"), "got: {}", err);
}

#[tokio::test]
async fn wsapi_list_executions_returns_empty_for_unknown_workflow() {
    let engine = build_test_engine();
    let ctx = make_ctx_with_engine(engine);
    let handler = WorkflowHandler;
    let data = Some(serde_json::json!({"workflow_name": "ghost_wf"}));
    let result = handler
        .handle_cmd("list_executions", data, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["count"], 0);
    assert_eq!(result["total"], 0);
}

#[tokio::test]
async fn wsapi_start_missing_name_field_returns_error() {
    let engine = build_test_engine();
    let ctx = make_ctx_with_engine(engine);
    let handler = WorkflowHandler;
    let data = Some(serde_json::json!({ /* no name */ }));
    let err = handler.handle_cmd("start", data, &ctx).await.unwrap_err();
    assert!(err.contains("missing field: name"), "got: {}", err);
}

#[tokio::test]
async fn wsapi_list_checkpoints_returns_error_when_no_store_configured() {
    // Default engine has no checkpoint store.
    let engine = build_test_engine();
    let ctx = make_ctx_with_engine(engine);
    let handler = WorkflowHandler;
    let data = Some(serde_json::json!({"execution_id": "any_id"}));
    let err = handler
        .handle_cmd("list_checkpoints", data, &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("checkpoint_store_unavailable"), "got: {}", err);
}

// ---- Phase A: WSAPI get / create / update / delete / validate / run_now

fn sample_workflow_def(name: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "description": "phase a test",
        "version": "1.0.0",
        "triggers": [],
        "nodes": [
            {"id": "n1", "node_type": "delay", "config": {"seconds": 1}}
        ],
        "edges": [],
        "variables": {},
        "metadata": {}
    })
}

fn make_ctx_with_engine_and_defs_dir(
    engine: Arc<nemesis_workflow::engine::WorkflowEngine>,
    dir: &std::path::Path,
) -> RequestContext {
    engine.set_workflow_defs_dir(dir.to_path_buf());
    make_ctx_with_engine(engine)
}

#[tokio::test]
async fn wsapi_get_returns_workflow_and_summary() {
    let engine = build_test_engine();
    engine
        .register_workflow(serde_json::from_value(sample_workflow_def("wf_x")).unwrap())
        .unwrap();
    let ctx = make_ctx_with_engine(engine);
    let handler = WorkflowHandler;
    let data = Some(serde_json::json!({"name": "wf_x"}));
    let payload = handler
        .handle_cmd("get", data, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(payload["workflow"]["name"], "wf_x");
    assert_eq!(payload["summary"]["name"], "wf_x");
    assert_eq!(payload["summary"]["node_count"], 1);
}

#[tokio::test]
async fn wsapi_get_missing_workflow_returns_error() {
    let engine = build_test_engine();
    let ctx = make_ctx_with_engine(engine);
    let handler = WorkflowHandler;
    let data = Some(serde_json::json!({"name": "ghost"}));
    let err = handler.handle_cmd("get", data, &ctx).await.unwrap_err();
    assert!(err.contains("workflow_not_found"));
}

#[tokio::test]
async fn wsapi_create_persists_to_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let engine = build_test_engine();
    let ctx = make_ctx_with_engine_and_defs_dir(engine.clone(), tmp.path());
    let handler = WorkflowHandler;

    let data = Some(serde_json::json!({
        "workflow": sample_workflow_def("wf_new"),
    }));
    let payload = handler
        .handle_cmd("create", data, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(payload["name"], "wf_new");
    assert_eq!(payload["created"], true);

    // File exists on disk.
    let file = tmp.path().join("wf_new.yaml");
    assert!(file.exists(), "expected {:?} to exist", file);

    // Engine memory has the workflow.
    let names = engine.list_workflows();
    assert!(names.contains(&"wf_new".to_string()));
}

#[tokio::test]
async fn wsapi_create_rejects_invalid_workflow() {
    let tmp = tempfile::tempdir().unwrap();
    let engine = build_test_engine();
    let ctx = make_ctx_with_engine_and_defs_dir(engine, tmp.path());
    let handler = WorkflowHandler;

    // Empty nodes list fails validation.
    let data = Some(serde_json::json!({
        "workflow": {
            "name": "broken",
            "description": "",
            "version": "1.0.0",
            "triggers": [],
            "nodes": [],
            "edges": [],
            "variables": {},
            "metadata": {}
        }
    }));
    let err = handler.handle_cmd("create", data, &ctx).await.unwrap_err();
    assert!(err.to_lowercase().contains("node") || err.contains("validate"));
}

#[tokio::test]
async fn wsapi_update_overwrites_existing_file() {
    let tmp = tempfile::tempdir().unwrap();
    let engine = build_test_engine();
    let ctx = make_ctx_with_engine_and_defs_dir(engine, tmp.path());
    let handler = WorkflowHandler;

    // Initial create.
    handler
        .handle_cmd(
            "create",
            Some(serde_json::json!({"workflow": sample_workflow_def("wf_y")})),
            &ctx,
        )
        .await
        .unwrap();

    // Update with different description.
    let mut updated = sample_workflow_def("wf_y");
    updated["description"] = serde_json::json!("updated!");
    let payload = handler
        .handle_cmd(
            "update",
            Some(serde_json::json!({"name": "wf_y", "workflow": updated})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(payload["name"], "wf_y");
    assert_eq!(payload["updated"], true);

    // File content reflects the new description.
    let content = std::fs::read_to_string(tmp.path().join("wf_y.yaml")).unwrap();
    assert!(content.contains("updated!"));
}

#[tokio::test]
async fn wsapi_delete_removes_file_and_memory_entry() {
    let tmp = tempfile::tempdir().unwrap();
    let engine = build_test_engine();
    let ctx = make_ctx_with_engine_and_defs_dir(engine.clone(), tmp.path());
    let handler = WorkflowHandler;

    // Setup: create then delete.
    handler
        .handle_cmd(
            "create",
            Some(serde_json::json!({"workflow": sample_workflow_def("wf_z")})),
            &ctx,
        )
        .await
        .unwrap();
    let file = tmp.path().join("wf_z.yaml");
    assert!(file.exists());

    let payload = handler
        .handle_cmd("delete", Some(serde_json::json!({"name": "wf_z"})), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(payload["name"], "wf_z");
    assert_eq!(payload["deleted"], true);
    assert!(!file.exists());
    assert!(!engine.list_workflows().contains(&"wf_z".to_string()));
}

#[tokio::test]
async fn wsapi_validate_reports_errors() {
    let engine = build_test_engine();
    let ctx = make_ctx_with_engine(engine);
    let handler = WorkflowHandler;

    // Valid workflow: no errors.
    let data = Some(serde_json::json!({
        "workflow": sample_workflow_def("valid_wf"),
    }));
    let payload = handler
        .handle_cmd("validate", data, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(payload["valid"], true);
    assert_eq!(payload["errors"].as_array().unwrap().len(), 0);

    // Invalid workflow (empty nodes).
    let data = Some(serde_json::json!({
        "workflow": {
            "name": "broken",
            "description": "",
            "version": "1.0.0",
            "triggers": [],
            "nodes": [],
            "edges": [],
            "variables": {},
            "metadata": {}
        }
    }));
    let payload = handler
        .handle_cmd("validate", data, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(payload["valid"], false);
    assert!(payload["errors"].as_array().unwrap().len() > 0);
}

#[tokio::test]
async fn wsapi_run_now_missing_name_returns_error() {
    let engine = build_test_engine();
    let ctx = make_ctx_with_engine(engine);
    let handler = WorkflowHandler;
    let err = handler
        .handle_cmd("run_now", Some(serde_json::json!({})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("missing field: name"));
}

#[tokio::test]
async fn wsapi_run_now_unknown_workflow_returns_error() {
    let engine = build_test_engine();
    let ctx = make_ctx_with_engine(engine);
    let handler = WorkflowHandler;
    let data = Some(serde_json::json!({"name": "ghost_wf"}));
    let err = handler.handle_cmd("run_now", data, &ctx).await.unwrap_err();
    // WorkflowNotFound error string mentions the missing name.
    assert!(err.contains("ghost_wf"));
}

#[tokio::test]
async fn wsapi_create_fails_when_defs_dir_not_set() {
    // No defs dir configured → persist should fail with helpful error.
    let engine = build_test_engine();
    let ctx = make_ctx_with_engine(engine);
    let handler = WorkflowHandler;
    let data = Some(serde_json::json!({
        "workflow": sample_workflow_def("no_dir_wf"),
    }));
    let err = handler.handle_cmd("create", data, &ctx).await.unwrap_err();
    assert!(err.contains("workflow_defs_dir"), "got: {}", err);
}

// Helper: build + register a single-node workflow on the engine.
fn reg_workflow(
    engine: &Arc<nemesis_workflow::engine::WorkflowEngine>,
    name: &str,
    node_type: &str,
) {
    let wf = nemesis_workflow::types::Workflow {
        name: name.to_string(),
        description: format!("desc for {}", name),
        version: "1.0.0".to_string(),
        triggers: vec![],
        nodes: vec![nemesis_workflow::types::NodeDef {
            id: "n1".to_string(),
            node_type: node_type.to_string(),
            config: HashMap::new(),
            depends_on: vec![],
            retry_count: 0,
            timeout: None,
            is_terminal: false,
        }],
        edges: vec![],
        variables: HashMap::new(),
        metadata: HashMap::new(),
    };
    engine.register_workflow(wf).unwrap();
}

fn ctx_with_dashboard(engine: Arc<nemesis_workflow::engine::WorkflowEngine>) -> RequestContext {
    let mut ctx = make_ctx_with_engine(engine);
    ctx.auth_method = crate::session::AuthMethod::Dashboard;
    ctx
}

// ---- fire_event ----

#[tokio::test]
async fn wsapi_fire_event_missing_data() {
    let ctx = make_ctx_with_engine(build_test_engine());
    let err = WorkflowHandler
        .handle_cmd("fire_event", None, &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("missing data"));
}

#[tokio::test]
async fn wsapi_fire_event_missing_event_type() {
    let ctx = make_ctx_with_engine(build_test_engine());
    let err = WorkflowHandler
        .handle_cmd("fire_event", Some(serde_json::json!({})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("missing field: event_type"));
}

#[tokio::test]
async fn wsapi_fire_event_publishes_and_reports_matches() {
    let engine = build_test_engine();
    // A workflow whose trigger listens for "ops.deploy" events.
    let wf = nemesis_workflow::types::Workflow {
        name: "reactor".to_string(),
        description: String::new(),
        version: "1.0.0".to_string(),
        triggers: vec![nemesis_workflow::types::TriggerConfig {
            trigger_type: "event".to_string(),
            config: {
                let mut m = HashMap::new();
                m.insert("event_type".to_string(), serde_json::json!("ops.deploy"));
                m
            },
        }],
        nodes: vec![nemesis_workflow::types::NodeDef {
            id: "n1".to_string(),
            node_type: "delay".to_string(),
            config: HashMap::new(),
            depends_on: vec![],
            retry_count: 0,
            timeout: None,
            is_terminal: false,
        }],
        edges: vec![],
        variables: HashMap::new(),
        metadata: HashMap::new(),
    };
    engine.register_workflow(wf).unwrap();
    let ctx = make_ctx_with_engine(engine.clone());
    let r = WorkflowHandler
        .handle_cmd(
            "fire_event",
            Some(serde_json::json!({
                "event_type": "ops.deploy",
                "data": { "region": "us-east" }
            })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["published"], true);
    assert_eq!(r["event_type"], "ops.deploy");
    assert_eq!(r["matched_workflows"][0], "reactor");
}

// ---- resolve_chat_target ----

#[tokio::test]
async fn wsapi_resolve_chat_target_missing_data() {
    let ctx = make_ctx_with_engine(build_test_engine());
    let err = WorkflowHandler
        .handle_cmd("resolve_chat_target", None, &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("missing data"));
}

#[tokio::test]
async fn wsapi_resolve_chat_target_missing_index() {
    let ctx = make_ctx_with_engine(build_test_engine());
    let err = WorkflowHandler
        .handle_cmd("resolve_chat_target", Some(serde_json::json!({})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("missing field: index"));
}

#[tokio::test]
async fn wsapi_resolve_chat_target_not_found() {
    let ctx = make_ctx_with_engine(build_test_engine());
    let r = WorkflowHandler
        .handle_cmd(
            "resolve_chat_target",
            Some(serde_json::json!({"index": "deadbeef"})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["found"], false);
    assert_eq!(r["chat_eligible"], false);
}

#[tokio::test]
async fn wsapi_resolve_chat_target_eligible() {
    let engine = build_test_engine();
    reg_workflow(&engine, "chatable", "delay");
    let index = nemesis_workflow::engine::WorkflowEngine::chat_index("chatable");
    let ctx = make_ctx_with_engine(engine);
    let r = WorkflowHandler
        .handle_cmd(
            "resolve_chat_target",
            Some(serde_json::json!({"index": index})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["found"], true);
    assert_eq!(r["workflow_name"], "chatable");
    assert_eq!(r["chat_eligible"], true);
    assert!(r["reason"].is_null());
}

#[tokio::test]
async fn wsapi_resolve_chat_target_human_review_ineligible() {
    let engine = build_test_engine();
    reg_workflow(&engine, "reviewy", "human_review");
    let index = nemesis_workflow::engine::WorkflowEngine::chat_index("reviewy");
    let ctx = make_ctx_with_engine(engine);
    let r = WorkflowHandler
        .handle_cmd(
            "resolve_chat_target",
            Some(serde_json::json!({"index": index})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["found"], true);
    assert_eq!(r["chat_eligible"], false);
    assert!(r["reason"].as_str().unwrap().contains("human_review"));
}

// ---- cancel / resume (unknown execution_id → engine error) ----

#[tokio::test]
async fn wsapi_cancel_missing_data() {
    let ctx = make_ctx_with_engine(build_test_engine());
    let err = WorkflowHandler
        .handle_cmd("cancel", None, &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("missing data"));
}

#[tokio::test]
async fn wsapi_cancel_missing_exec_id() {
    let ctx = make_ctx_with_engine(build_test_engine());
    let err = WorkflowHandler
        .handle_cmd("cancel", Some(serde_json::json!({})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("missing field: execution_id"));
}

#[tokio::test]
async fn wsapi_cancel_unknown_id_errors() {
    let ctx = make_ctx_with_engine(build_test_engine());
    let err = WorkflowHandler
        .handle_cmd(
            "cancel",
            Some(serde_json::json!({"execution_id": "nope"})),
            &ctx,
        )
        .await
        .unwrap_err();
    // cancel_execution surfaces an EngineError string for unknown ids.
    assert!(!err.is_empty());
}

#[tokio::test]
async fn wsapi_resume_missing_exec_id() {
    let ctx = make_ctx_with_engine(build_test_engine());
    let err = WorkflowHandler
        .handle_cmd("resume", Some(serde_json::json!({})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("missing field: execution_id"));
}

#[tokio::test]
async fn wsapi_resume_unknown_id_errors() {
    let ctx = make_ctx_with_engine(build_test_engine());
    let err = WorkflowHandler
        .handle_cmd(
            "resume",
            Some(serde_json::json!({"execution_id": "ghost"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(!err.is_empty());
}

// ---- get_checkpoint (engine has no store → unavailable) ----

#[tokio::test]
async fn wsapi_get_checkpoint_missing_data() {
    let ctx = make_ctx_with_engine(build_test_engine());
    let err = WorkflowHandler
        .handle_cmd("get_checkpoint", None, &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("missing data"));
}

#[tokio::test]
async fn wsapi_get_checkpoint_missing_checkpoint_id() {
    let ctx = make_ctx_with_engine(build_test_engine());
    let err = WorkflowHandler
        .handle_cmd(
            "get_checkpoint",
            Some(serde_json::json!({"execution_id": "e1"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("missing field: checkpoint_id"));
}

#[tokio::test]
async fn wsapi_get_checkpoint_no_store_configured() {
    let ctx = make_ctx_with_engine(build_test_engine());
    let err = WorkflowHandler
        .handle_cmd(
            "get_checkpoint",
            Some(serde_json::json!({"execution_id": "e1", "checkpoint_id": "c1"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("checkpoint_store_unavailable"));
}

// ---- chat password CRUD ----

#[tokio::test]
async fn wsapi_set_chat_password_requires_dashboard() {
    // WorkflowChat auth (standalone page) must NOT mutate passwords.
    let mut ctx = make_ctx_with_engine(build_test_engine());
    ctx.auth_method = crate::session::AuthMethod::WorkflowChat;
    let err = WorkflowHandler
        .handle_cmd(
            "set_chat_password",
            Some(serde_json::json!({"index": "x", "password": "p"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("permission_denied"));
}

#[tokio::test]
async fn wsapi_set_chat_password_empty_rejected() {
    let engine = build_test_engine();
    let ctx = ctx_with_dashboard(engine);
    let err = WorkflowHandler
        .handle_cmd(
            "set_chat_password",
            Some(serde_json::json!({"index": "x", "password": ""})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("must not be empty"));
}

#[tokio::test]
async fn wsapi_set_then_verify_chat_password_roundtrip() {
    let engine = build_test_engine();
    reg_workflow(&engine, "secret_wf", "delay");
    let index = nemesis_workflow::engine::WorkflowEngine::chat_index("secret_wf");
    // Dashboard session sets the password.
    let ctx = ctx_with_dashboard(engine.clone());
    let r = WorkflowHandler
        .handle_cmd(
            "set_chat_password",
            Some(serde_json::json!({"index": index, "password": "s3cret"})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["set"], true);

    // Wrong password is rejected.
    let err = WorkflowHandler
        .handle_cmd(
            "verify_chat_password",
            Some(serde_json::json!({"index": index, "password": "nope"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert_eq!(err, "unauthorized");

    // Correct password resolves to the workflow metadata.
    let r = WorkflowHandler
        .handle_cmd(
            "verify_chat_password",
            Some(serde_json::json!({"index": index, "password": "s3cret"})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["verified"], true);
    assert_eq!(r["workflow_name"], "secret_wf");
}

#[tokio::test]
async fn wsapi_verify_chat_password_correct_but_no_workflow() {
    // Password is set for an index, but no workflow is registered for it →
    // verify returns workflow_not_found.
    let engine = build_test_engine();
    let ctx = ctx_with_dashboard(engine.clone());
    WorkflowHandler
        .handle_cmd(
            "set_chat_password",
            Some(serde_json::json!({"index": "orphan", "password": "pw"})),
            &ctx,
        )
        .await
        .unwrap();
    let err = WorkflowHandler
        .handle_cmd(
            "verify_chat_password",
            Some(serde_json::json!({"index": "orphan", "password": "pw"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("workflow_not_found_for_index"));
}

#[tokio::test]
async fn wsapi_clear_chat_password_requires_dashboard() {
    let mut ctx = make_ctx_with_engine(build_test_engine());
    ctx.auth_method = crate::session::AuthMethod::WorkflowChat;
    let err = WorkflowHandler
        .handle_cmd(
            "clear_chat_password",
            Some(serde_json::json!({"index": "x"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("permission_denied"));
}

#[tokio::test]
async fn wsapi_clear_chat_password_success() {
    let engine = build_test_engine();
    let ctx = ctx_with_dashboard(engine);
    let r = WorkflowHandler
        .handle_cmd(
            "clear_chat_password",
            Some(serde_json::json!({"index": "idx1"})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["cleared"], true);
    assert_eq!(r["index"], "idx1");
}
