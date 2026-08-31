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
        board: None,
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

// =====================================================================
// quality-hardening goal 冲刺 S10a（web 批次 1，2026-08-26）
//
// 上面 58 个测试已覆盖 WSAPI 主干 + verify_signature 基本臂。本批补：
// engine_error 全 7 臂、REST run/start/list/status/executions 实跑、
// webhook 完整流（限流 429 / 签名 401 / signed 成功 / GET 变体 / 私有
// trigger 函数三臂）、decode_signature base64 回退臂、parse_input_object /
// ensure_unified_input 全臂、checkpoint REST（注入 mock store 的错误臂）、
// workflow-chat REST info/verify、WSAPI 成功路径残差（run_now/start/
// status/list_executions/list+get_checkpoint/create/validate/delete）。
// =====================================================================

use nemesis_workflow::types::{NodeDef, TriggerConfig, Workflow};

fn node(node_type: &str, config: HashMap<String, serde_json::Value>) -> NodeDef {
    NodeDef {
        id: "n1".to_string(),
        node_type: node_type.to_string(),
        config,
        depends_on: vec![],
        retry_count: 0,
        timeout: None,
        is_terminal: false,
    }
}

fn workflow_of(name: &str, node_type: &str, config: HashMap<String, serde_json::Value>) -> Workflow {
    Workflow {
        name: name.to_string(),
        description: format!("wf {}", name),
        version: "1.0.0".to_string(),
        triggers: vec![],
        nodes: vec![node(node_type, config)],
        edges: vec![],
        variables: HashMap::new(),
        metadata: HashMap::new(),
    }
}

/// delay 0 秒的工作流：run 同步完成后立即 Completed。
fn instant_workflow(name: &str) -> Workflow {
    let mut cfg = HashMap::new();
    cfg.insert("seconds".to_string(), serde_json::json!(0));
    workflow_of(name, "delay", cfg)
}

fn human_review_workflow(name: &str) -> Workflow {
    workflow_of(name, "human_review", HashMap::new())
}

/// 带 webhook trigger（可选 secret）的工作流。
fn webhook_workflow(name: &str, secret: Option<&str>) -> Workflow {
    let mut wf = instant_workflow(name);
    let mut cfg = HashMap::new();
    if let Some(s) = secret {
        cfg.insert("secret".to_string(), serde_json::json!(s));
    }
    wf.triggers = vec![TriggerConfig {
        trigger_type: "webhook".to_string(),
        config: cfg,
    }];
    wf
}

fn local_addr() -> std::net::SocketAddr {
    "127.0.0.1:59999".parse().unwrap()
}

/// 带 engine 的 REST 用 state（make_test_state 不注入 engine，而 REST
/// handler 收的是 State<Arc<AppState>> 而非 RequestContext）。
fn state_with_engine(engine: Arc<nemesis_workflow::engine::WorkflowEngine>) -> Arc<AppState> {
    make_ctx_with_engine(engine).state.clone()
}

// ---- engine_error 全 7 臂 --------------------------------------------

#[test]
fn engine_error_maps_every_variant_to_status_and_kind() {
    use nemesis_workflow::engine::EngineError;
    let (code, body) = engine_error(EngineError::WorkflowNotFound("wf".into()));
    assert_eq!(code, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "workflow_not_found");

    let (code, body) = engine_error(EngineError::ExecutionNotFound("ex".into()));
    assert_eq!(code, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "execution_not_found");

    let (code, body) = engine_error(EngineError::CycleDetected("c".into()));
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "cycle_detected");

    let (code, body) = engine_error(EngineError::PersistenceError("p".into()));
    assert_eq!(code, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["error"], "persistence_error");

    let (code, body) = engine_error(EngineError::ExecutionFailed("f".into()));
    assert_eq!(code, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["error"], "execution_failed");

    // 兜底臂：其余变体（AlreadyCompleted / UnknownNodeType / InvalidState /
    // RecursionLimitExceeded）都落 500 engine_error。
    for err in [
        EngineError::AlreadyCompleted("a".into()),
        EngineError::UnknownNodeType("u".into()),
        EngineError::InvalidState("i".into()),
        EngineError::RecursionLimitExceeded("r".into()),
    ] {
        let (code, body) = engine_error(err);
        assert_eq!(code, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["error"], "engine_error");
    }
}

// ---- REST run / start / list / status / executions -------------------

#[tokio::test]
async fn rest_run_missing_name_is_400() {
    let state = state_with_engine(build_test_engine());
    let (status, body) = handle_workflow_run(
        axum::extract::State(state),
        HeaderMap::new(),
        Json(serde_json::json!({ "input": {} })),
    )
    .await
    .unwrap_err();
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "missing_field");
}

#[tokio::test]
async fn rest_run_unknown_workflow_maps_404() {
    let state = state_with_engine(build_test_engine());
    let (status, body) = handle_workflow_run(
        axum::extract::State(state),
        HeaderMap::new(),
        Json(serde_json::json!({ "name": "ghost", "input": {} })),
    )
    .await
    .unwrap_err();
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "workflow_not_found");
}

#[tokio::test]
async fn rest_run_success_returns_completed_execution() {
    let engine = build_test_engine();
    engine.register_workflow(instant_workflow("quick")).unwrap();
    let state = state_with_engine(engine);
    let Json(json) = handle_workflow_run(
        axum::extract::State(state),
        HeaderMap::new(), // auth_token 为空 → 匿名放行
        Json(serde_json::json!({ "name": "quick", "input": { "x": 1 } })),
    )
    .await
    .unwrap();
    assert_eq!(json["workflow_name"], "quick");
    assert_eq!(json["state"], "Completed");
    assert!(json["execution_id"].is_string());
    assert!(json["ended_at"].is_string());
    assert!(json["trigger_source"].is_object(), "run passes TriggerSource::Webhook");
}

#[tokio::test]
async fn rest_start_engine_missing_missing_name_unknown_and_success() {
    // 1) engine 缺失 → 503
    let (status, body) = handle_workflow_start(
        axum::extract::State(make_test_state("")),
        Json(serde_json::json!({ "name": "wf" })),
    )
    .await
    .unwrap_err();
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"], "workflow_engine_unavailable");

    // 2) 缺 name → 400
    let (status, body) = handle_workflow_start(
        axum::extract::State(state_with_engine(build_test_engine())),
        Json(serde_json::json!({ "input": {} })),
    )
    .await
    .unwrap_err();
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "missing_field");

    // 3) 未知工作流 → 404
    let (status, body) = handle_workflow_start(
        axum::extract::State(state_with_engine(build_test_engine())),
        Json(serde_json::json!({ "name": "ghost" })),
    )
    .await
    .unwrap_err();
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "workflow_not_found");

    // 4) 成功 → execution_id 立即返回，后台任务已登记执行记录
    let engine = build_test_engine();
    engine.register_workflow(instant_workflow("asyncwf")).unwrap();
    let Json(json) = handle_workflow_start(
        axum::extract::State(state_with_engine(engine.clone())),
        Json(serde_json::json!({ "name": "asyncwf" })),
    )
    .await
    .unwrap();
    let id = json["execution_id"].as_str().unwrap().to_string();
    assert_eq!(json["state"], "Running");
    assert!(engine.get_execution(&id).await.is_some());
}

#[tokio::test]
async fn rest_list_engine_missing_and_with_workflows() {
    let (status, _) =
        handle_workflow_list(axum::extract::State(make_test_state(""))).await.unwrap_err();
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    let engine = build_test_engine();
    engine.register_workflow(instant_workflow("listed")).unwrap();
    let Json(json) =
        handle_workflow_list(axum::extract::State(state_with_engine(engine)))
            .await
            .unwrap();
    assert_eq!(json["count"], 1);
    assert_eq!(json["workflows"][0]["name"], "listed");
    assert!(json["trigger_driver_status"].is_object());
}

#[tokio::test]
async fn rest_status_found_and_not_found() {
    let engine = build_test_engine();
    engine.register_workflow(instant_workflow("statwf")).unwrap();
    let exec = engine.run("statwf", HashMap::new(), None).await.unwrap();
    let state = state_with_engine(engine.clone());

    let Json(json) = handle_workflow_status(
        axum::extract::State(state.clone()),
        Path(exec.id.clone()),
    )
    .await
    .unwrap();
    assert_eq!(json["execution_id"], exec.id);
    assert_eq!(json["state"], "Completed");

    let (status, body) =
        handle_workflow_status(axum::extract::State(state), Path("no-such".to_string()))
            .await
            .unwrap_err();
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "execution_not_found");
}

#[tokio::test]
async fn rest_executions_filters_state_and_applies_limit() {
    let engine = build_test_engine();
    engine.register_workflow(instant_workflow("multi")).unwrap();
    for _ in 0..3 {
        engine.run("multi", HashMap::new(), None).await.unwrap();
    }
    let state = state_with_engine(engine);

    // workflow_name + state 双过滤
    let Json(json) = handle_workflow_executions(
        axum::extract::State(state.clone()),
        Query(ExecutionListQuery {
            workflow_name: Some("multi".to_string()),
            state: Some("Completed".to_string()),
            limit: None,
        }),
    )
    .await
    .unwrap();
    assert_eq!(json["total"], 3);
    assert_eq!(json["count"], 3);

    // limit 截断：total 仍 3，count 2
    let Json(json) = handle_workflow_executions(
        axum::extract::State(state.clone()),
        Query(ExecutionListQuery {
            workflow_name: Some("multi".to_string()),
            state: None,
            limit: Some(2),
        }),
    )
    .await
    .unwrap();
    assert_eq!(json["total"], 3);
    assert_eq!(json["count"], 2);
    assert!(json["executions"][0]["has_error"].is_boolean());

    // 不匹配的 state → 空
    let Json(json) = handle_workflow_executions(
        axum::extract::State(state),
        Query(ExecutionListQuery {
            workflow_name: None,
            state: Some("Failed".to_string()),
            limit: None,
        }),
    )
    .await
    .unwrap();
    assert_eq!(json["total"], 0);
}

// ---- webhook 完整流 --------------------------------------------------

#[tokio::test]
async fn webhook_rate_limit_returns_429_on_61st_call_same_ip() {
    // engine 缺失：前 60 次通过限流后在 trigger 阶段 500（InvalidState 经
    // engine_error 兜底臂），第 61 次被限流先拦截 → 429（限流先于签名）。
    let state = make_test_state(""); // 每个 state 一把新限流器
    for i in 0..60 {
        let (status, _) = handle_workflow_webhook(
            axum::extract::State(state.clone()),
            ConnectInfo(local_addr()),
            HeaderMap::new(),
            Path("hookwf".to_string()),
            axum::body::Bytes::from_static(b"{}"),
        )
        .await
        .unwrap_err();
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "call {i}");
    }
    let (status, body) = handle_workflow_webhook(
        axum::extract::State(state),
        ConnectInfo(local_addr()),
        HeaderMap::new(),
        Path("hookwf".to_string()),
        axum::body::Bytes::from_static(b"{}"),
    )
    .await
    .unwrap_err();
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["error"], "rate limited");
    assert!(body["retry_after_secs"].is_u64());
}

#[tokio::test]
async fn webhook_unsigned_workflow_accepted_without_secret() {
    let engine = build_test_engine();
    engine.register_workflow(webhook_workflow("hookopen", None)).unwrap();
    let state = state_with_engine(engine.clone());
    let Json(json) = handle_workflow_webhook(
        axum::extract::State(state),
        ConnectInfo(local_addr()),
        HeaderMap::new(), // 未配置 secret → 不校验签名
        Path("hookopen".to_string()),
        axum::body::Bytes::from_static(b"{\"k\":1}"),
    )
    .await
    .unwrap();
    assert_eq!(json["workflow_name"], "hookopen");
    let id = json["execution_id"].as_str().unwrap().to_string();
    assert!(engine.get_execution(&id).await.is_some());
}

#[tokio::test]
async fn webhook_signed_flow_rejects_then_accepts() {
    let engine = build_test_engine();
    engine
        .register_workflow(webhook_workflow("hooksec", Some("topsecret")))
        .unwrap();
    let state = state_with_engine(engine);
    let body = axum::body::Bytes::from_static(b"{\"event\":\"push\"}");

    // 1) 缺 X-Signature → 401
    let (status, body_out) = handle_workflow_webhook(
        axum::extract::State(state.clone()),
        ConnectInfo(local_addr()),
        HeaderMap::new(),
        Path("hooksec".to_string()),
        body.clone(),
    )
    .await
    .unwrap_err();
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body_out["error"], "signature verification failed");
    assert_eq!(body_out["reason"], "missing X-Signature header");

    // 2) 签名长度不匹配（16 字节 vs 32 字节 HMAC）→ 401
    let (status, _) = handle_workflow_webhook(
        axum::extract::State(state.clone()),
        ConnectInfo(local_addr()),
        headers_with(Some(&"ab".repeat(16))),
        Path("hooksec".to_string()),
        body.clone(),
    )
    .await
    .unwrap_err();
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // 3) 正确 HMAC → 200 + 执行记录
    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(b"topsecret").unwrap();
    mac.update(&body);
    let sig = hex(&mac.finalize().into_bytes());
    let Json(json) = handle_workflow_webhook(
        axum::extract::State(state),
        ConnectInfo(local_addr()),
        headers_with(Some(&sig)),
        Path("hooksec".to_string()),
        body,
    )
    .await
    .unwrap();
    assert_eq!(json["workflow_name"], "hooksec");
    assert!(json["execution_id"].is_string());
}

#[tokio::test]
async fn webhook_get_variant_triggers_via_query_params() {
    let engine = build_test_engine();
    engine.register_workflow(webhook_workflow("hookget", None)).unwrap();
    let state = state_with_engine(engine.clone());
    let Json(json) = handle_workflow_webhook_get(
        axum::extract::State(state),
        ConnectInfo(local_addr()),
        Path("hookget".to_string()),
        Query(serde_json::json!({ "challenge": "abc" })),
    )
    .await
    .unwrap();
    assert_eq!(json["workflow_name"], "hookget");
    assert_eq!(json["state"], "Running");
}

#[tokio::test]
async fn webhook_trigger_private_fn_covers_all_three_arms() {
    // engine 缺失 → InvalidState
    let err = trigger_workflow_via_webhook(
        &make_test_state(""),
        "wf",
        serde_json::Value::Null,
    )
    .await
    .unwrap_err();
    assert!(matches!(
        err,
        nemesis_workflow::engine::EngineError::InvalidState(_)
    ));

    // 有 engine 但工作流不存在：String payload 臂与 object payload 臂都在
    // start_async 内走到 WorkflowNotFound（证明前面的映射已执行）。
    let state = state_with_engine(build_test_engine());
    let err = trigger_workflow_via_webhook(&state, "wf", serde_json::Value::String("hi".into()))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        nemesis_workflow::engine::EngineError::WorkflowNotFound(_)
    ));
    let err = trigger_workflow_via_webhook(&state, "wf", serde_json::json!({ "a": 1 }))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        nemesis_workflow::engine::EngineError::WorkflowNotFound(_)
    ));

    // 成功臂：payload 传进 input.payload / input（字符串化）
    let engine = build_test_engine();
    engine.register_workflow(instant_workflow("hooksuc")).unwrap();
    let state = state_with_engine(engine);
    let id = trigger_workflow_via_webhook(&state, "hooksuc", serde_json::json!({"e": 9}))
        .await
        .unwrap();
    assert!(!id.is_empty());
}

#[tokio::test]
async fn workflow_webhook_secret_reads_trigger_config() {
    let engine = build_test_engine();
    engine
        .register_workflow(webhook_workflow("secyes", Some("s1")))
        .unwrap();
    engine.register_workflow(webhook_workflow("secno", None)).unwrap();
    let state = state_with_engine(engine);
    assert_eq!(workflow_webhook_secret(&state, "secyes").await.as_deref(), Some("s1"));
    assert_eq!(workflow_webhook_secret(&state, "secno").await, None);
    assert_eq!(workflow_webhook_secret(&state, "ghost").await, None);
}

// ---- decode_signature / hex_decode -----------------------------------

#[test]
fn decode_signature_hex_base64_prefix_and_garbage_arms() {
    // hex 优先：全 hex 字符串按 hex 解
    assert_eq!(decode_signature("ff00").as_deref(), Some(&[0xff, 0x00][..]));
    // sha256= 前缀剥离后按 hex 解（GitHub/GitLab/Slack 风格）
    assert_eq!(decode_signature("sha256=4142").as_deref(), Some(&b"AB"[..]));

    // base64 回退臂：含非 hex 字符但合法 base64
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(b"ab"); // "YWI="
    assert_eq!(decode_signature(&b64).as_deref(), Some(&b"ab"[..]));
    assert_eq!(decode_signature("//8=").as_deref(), Some(&[0xff, 0xff][..]));

    // 奇数长度 hex + 非法 base64（长度非 4 倍数）→ None
    assert_eq!(decode_signature("abc"), None);
    // 非法字符（hex 失败 + base64 字母表外）→ None
    assert_eq!(decode_signature("zz!!"), None);
    assert_eq!(decode_signature("===="), None);
}

#[test]
fn hex_decode_error_paths() {
    assert_eq!(hex_decode("abc"), Err("odd-length hex"));
    assert_eq!(hex_decode("zz"), Err("invalid hex char"));
    assert_eq!(hex_decode("ZZ"), Err("invalid hex char"));
    assert_eq!(hex_decode("0F"), Ok(vec![0x0f]));
    assert!(hex_decode("").unwrap().is_empty());
}

#[tokio::test]
async fn webhook_rate_limiter_default_impl_allows_first_call() {
    let limiter = WebhookRateLimiter::default();
    let ip: IpAddr = "192.168.0.9".parse().unwrap();
    assert!(limiter.check(ip).await.is_ok());
}

#[test]
fn rate_limited_and_unauthorized_helper_shapes() {
    let (code, body) = rate_limited(Duration::from_secs(42));
    assert_eq!(code, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["error"], "rate limited");
    assert_eq!(body["retry_after_secs"], 42);

    let (code, body) = unauthorized("because");
    assert_eq!(code, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "signature verification failed");
    assert_eq!(body["reason"], "because");
}

// ---- parse_input_object / ensure_unified_input ------------------------

#[test]
fn parse_input_object_object_wrapped_and_none_arms() {
    let obj = parse_input_object(Some(&serde_json::json!({ "a": 1, "b": "x" })));
    assert_eq!(obj.get("a"), Some(&serde_json::json!(1)));
    assert_eq!(obj.len(), 2);

    // 非对象 → 包成 {"input": value}
    let wrapped = parse_input_object(Some(&serde_json::json!("just a string")));
    assert_eq!(wrapped.get("input"), Some(&serde_json::json!("just a string")));

    // None → 空 map
    assert!(parse_input_object(None).is_empty());
}

#[test]
fn ensure_unified_input_keeps_synthesizes_and_empty_arms() {
    // 已有 input → 不动
    let mut m = HashMap::new();
    m.insert("input".to_string(), serde_json::json!("keep"));
    ensure_unified_input(&mut m);
    assert_eq!(m.get("input"), Some(&serde_json::json!("keep")));

    // 没有 input → 由剩余字段合成 JSON 字符串
    let mut m = HashMap::new();
    m.insert("payload".to_string(), serde_json::json!({ "k": 1 }));
    ensure_unified_input(&mut m);
    let synthesized = m.get("input").unwrap().as_str().unwrap();
    assert!(synthesized.contains("payload"), "got {synthesized}");
    assert!(synthesized.contains("k"));

    // 空 map → "{}"
    let mut m = HashMap::new();
    ensure_unified_input(&mut m);
    assert_eq!(m.get("input").unwrap(), &serde_json::json!("{}"));
}

// ---- checkpoint REST（mock store 注入）--------------------------------
//
// 引擎默认不带 store（上面 no-store 臂已测）。这里注入自定义 mock 以打
// 到 list/load 的全部错误映射臂（InMemoryCheckpointStore 无错误路径可走）。

use nemesis_workflow::checkpoint::{
    Checkpoint, CheckpointMeta, CheckpointStore, SerializableContext, StoreError,
};

enum MockLoad {
    Ok(Checkpoint),
    NotFound,
    Corrupt,
    Serialization,
}

enum MockList {
    Ok(Vec<CheckpointMeta>),
    Io,
}

struct MockCheckpointStore {
    load: MockLoad,
    list: MockList,
}

#[async_trait::async_trait]
impl CheckpointStore for MockCheckpointStore {
    async fn save(&self, _checkpoint: Checkpoint) -> Result<String, StoreError> {
        unreachable!("save is not exercised by these tests")
    }
    async fn load(&self, _e: &str, _c: &str) -> Result<Checkpoint, StoreError> {
        match &self.load {
            MockLoad::Ok(cp) => Ok(cp.clone()),
            MockLoad::NotFound => Err(StoreError::NotFound {
                execution_id: "e".to_string(),
                checkpoint_id: "c".to_string(),
            }),
            MockLoad::Corrupt => Err(StoreError::Corrupt("bad json".to_string())),
            MockLoad::Serialization => Err(StoreError::Serialization(
                serde_json::from_str::<serde_json::Value>("{").unwrap_err(),
            )),
        }
    }
    async fn latest(&self, _e: &str) -> Result<Option<Checkpoint>, StoreError> {
        Ok(None)
    }
    async fn list(&self, _e: &str) -> Result<Vec<CheckpointMeta>, StoreError> {
        match &self.list {
            MockList::Ok(m) => Ok(m.clone()),
            MockList::Io => Err(StoreError::Io(std::io::Error::other("disk gone"))),
        }
    }
    async fn delete(&self, _e: &str, _c: &str) -> Result<(), StoreError> {
        Ok(())
    }
    async fn list_executions(&self) -> Result<Vec<String>, StoreError> {
        Ok(Vec::new())
    }
}

fn make_checkpoint(id: &str) -> Checkpoint {
    Checkpoint {
        id: id.to_string(),
        execution_id: "exec-1".to_string(),
        saved_at: chrono::Utc::now(),
        completed_nodes: std::collections::HashSet::new(),
        waiting_node: None,
        parent_execution_id: None,
        trigger_source: None,
        terminal: false,
        context_snapshot: SerializableContext {
            variables: HashMap::new(),
            node_results: HashMap::new(),
            input: HashMap::new(),
        },
        workflow_hash: "hash".to_string(),
    }
}

fn engine_with_mock(load: MockLoad, list: MockList) -> Arc<nemesis_workflow::engine::WorkflowEngine> {
    let engine = build_test_engine();
    engine.set_checkpoint_store(Arc::new(MockCheckpointStore { load, list }));
    engine
}

#[tokio::test]
async fn checkpoints_rest_engine_and_store_missing() {
    // engine 缺失
    let (status, _) = handle_workflow_checkpoints_list(
        axum::extract::State(make_test_state("")),
        Path("exec-1".to_string()),
    )
    .await
    .unwrap_err();
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    // engine 有但无 store
    let (status, body) = handle_workflow_checkpoints_list(
        axum::extract::State(state_with_engine(build_test_engine())),
        Path("exec-1".to_string()),
    )
    .await
    .unwrap_err();
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"], "checkpoint_store_unavailable");

    // load 端点的两个缺失臂
    let (status, _) = handle_workflow_checkpoint_load(
        axum::extract::State(make_test_state("")),
        Path(("exec-1".to_string(), "cp".to_string())),
    )
    .await
    .unwrap_err();
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let (status, body) = handle_workflow_checkpoint_load(
        axum::extract::State(state_with_engine(build_test_engine())),
        Path(("exec-1".to_string(), "cp".to_string())),
    )
    .await
    .unwrap_err();
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"], "checkpoint_store_unavailable");
}

#[tokio::test]
async fn checkpoints_rest_list_ok_and_store_error() {
    // Ok：两条 meta
    let state = state_with_engine(engine_with_mock(
        MockLoad::Ok(make_checkpoint("cp")),
        MockList::Ok(vec![
            CheckpointMeta::from(&make_checkpoint("cp1")),
            CheckpointMeta::from(&make_checkpoint("cp2")),
        ]),
    ));
    let Json(json) = handle_workflow_checkpoints_list(
        axum::extract::State(state),
        Path("exec-1".to_string()),
    )
    .await
    .unwrap();
    assert_eq!(json["execution_id"], "exec-1");
    assert_eq!(json["checkpoints"].as_array().unwrap().len(), 2);

    // store 报 Io 错 → 500 checkpoint_list_failed
    let state = state_with_engine(engine_with_mock(MockLoad::Ok(make_checkpoint("cp")), MockList::Io));
    let (status, body) = handle_workflow_checkpoints_list(
        axum::extract::State(state),
        Path("exec-1".to_string()),
    )
    .await
    .unwrap_err();
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["error"], "checkpoint_list_failed");
}

#[tokio::test]
async fn checkpoints_rest_load_all_four_outcomes() {
    let call = |engine, cp_id: &str| {
        handle_workflow_checkpoint_load(
            axum::extract::State(state_with_engine(engine)),
            Path(("exec-1".to_string(), cp_id.to_string())),
        )
    };

    // Ok
    let Json(json) = call(engine_with_mock(MockLoad::Ok(make_checkpoint("cp9")), MockList::Ok(vec![])), "cp9")
        .await
        .unwrap();
    assert_eq!(json["checkpoint"]["id"], "cp9");

    // NotFound → 404
    let (status, body) =
        call(engine_with_mock(MockLoad::NotFound, MockList::Ok(vec![])), "cp")
            .await
            .unwrap_err();
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "checkpoint_not_found");

    // Corrupt → 500 checkpoint_corrupt
    let (status, body) =
        call(engine_with_mock(MockLoad::Corrupt, MockList::Ok(vec![])), "cp")
            .await
            .unwrap_err();
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["error"], "checkpoint_corrupt");

    // 其它（Serialization）→ 500 checkpoint_load_failed 兜底
    let (status, body) =
        call(engine_with_mock(MockLoad::Serialization, MockList::Ok(vec![])), "cp")
            .await
            .unwrap_err();
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["error"], "checkpoint_load_failed");
}

// ---- workflow-chat REST ----------------------------------------------

#[tokio::test]
async fn chat_info_engine_missing_not_found_eligible_and_human_review() {
    // engine 缺失
    let (status, _) = handle_workflow_chat_info(
        axum::extract::State(make_test_state("")),
        Query(ChatInfoQuery { index: "aabbccdd".to_string() }),
    )
    .await
    .unwrap_err();
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    // 不匹配的 index → found:false（reason 是字符串，不是 null）
    let engine = build_test_engine();
    let Json(json) = handle_workflow_chat_info(
        axum::extract::State(state_with_engine(engine)),
        Query(ChatInfoQuery { index: "ffffffff".to_string() }),
    )
    .await
    .unwrap();
    assert_eq!(json["found"], false);
    assert_eq!(json["chat_eligible"], false);
    assert_eq!(json["needs_password"], false);
    assert!(json["reason"].as_str().unwrap().contains("no workflow"));

    // 命中 + 无 human_review → eligible，reason null
    let engine = build_test_engine();
    engine.register_workflow(instant_workflow("chatable")).unwrap();
    let index = WorkflowEngine::chat_index("chatable");
    let state = state_with_engine(engine);
    let Json(json) = handle_workflow_chat_info(
        axum::extract::State(state.clone()),
        Query(ChatInfoQuery { index: index.clone() }),
    )
    .await
    .unwrap();
    assert_eq!(json["found"], true);
    assert_eq!(json["workflow_name"], "chatable");
    assert_eq!(json["chat_eligible"], true);
    assert!(json["reason"].is_null());

    // needs_password 反映 secret store 状态
    state.chat_secret_store.set_password(&index, "pw").unwrap();
    let Json(json) = handle_workflow_chat_info(
        axum::extract::State(state),
        Query(ChatInfoQuery { index }),
    )
    .await
    .unwrap();
    assert_eq!(json["needs_password"], true);

    // human_review 节点 → ineligible + 中文 reason
    let engine = build_test_engine();
    engine.register_workflow(human_review_workflow("reviewwf")).unwrap();
    let index = WorkflowEngine::chat_index("reviewwf");
    let Json(json) = handle_workflow_chat_info(
        axum::extract::State(state_with_engine(engine)),
        Query(ChatInfoQuery { index }),
    )
    .await
    .unwrap();
    assert_eq!(json["found"], true);
    assert_eq!(json["chat_eligible"], false);
    assert!(json["reason"].as_str().unwrap().contains("human_review"));
}

#[tokio::test]
async fn chat_verify_missing_index_wrong_password_orphan_and_success() {
    // engine 缺失
    let (status, _) = handle_workflow_chat_verify(
        axum::extract::State(make_test_state("")),
        Json(serde_json::json!({ "index": "x", "password": "y" })),
    )
    .await
    .unwrap_err();
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    // 缺 index 字段 → 400
    let (status, body) = handle_workflow_chat_verify(
        axum::extract::State(state_with_engine(build_test_engine())),
        Json(serde_json::json!({ "password": "y" })),
    )
    .await
    .unwrap_err();
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["field"], "index");

    // 错密码 → 401（不泄露 index 是否存在）
    let engine = build_test_engine();
    engine.register_workflow(instant_workflow("chatverifiable")).unwrap();
    let index = WorkflowEngine::chat_index("chatverifiable");
    let state = state_with_engine(engine);
    state.chat_secret_store.set_password(&index, "pw123").unwrap();
    let (status, body) = handle_workflow_chat_verify(
        axum::extract::State(state.clone()),
        Json(serde_json::json!({ "index": index, "password": "WRONG" })),
    )
    .await
    .unwrap_err();
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["verified"], false);

    // 正确密码但 index 无对应工作流 → 404
    let state2 = state_with_engine(build_test_engine());
    state2.chat_secret_store.set_password("deadbeef", "pw").unwrap();
    let (status, body) = handle_workflow_chat_verify(
        axum::extract::State(state2),
        Json(serde_json::json!({ "index": "deadbeef", "password": "pw" })),
    )
    .await
    .unwrap_err();
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "workflow_not_found_for_index");

    // 成功 → verified:true + 元数据
    let Json(json) = handle_workflow_chat_verify(
        axum::extract::State(state),
        Json(serde_json::json!({ "index": index, "password": "pw123" })),
    )
    .await
    .unwrap();
    assert_eq!(json["verified"], true);
    assert_eq!(json["workflow_name"], "chatverifiable");
}

// ---- WSAPI 成功路径残差 ----------------------------------------------

#[tokio::test]
async fn wsapi_run_now_start_and_status_success() {
    let engine = build_test_engine();
    engine.register_workflow(instant_workflow("runnow")).unwrap();
    let ctx = make_ctx_with_engine(engine.clone());
    let handler = WorkflowHandler;

    // run_now：input 非对象 → parse_input_object 包成 {"input": ...}，
    // ensure_unified_input 看到已有 input 不再合成
    let payload = handler
        .handle_cmd(
            "run_now",
            Some(serde_json::json!({ "name": "runnow", "input": "plain" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    let id = payload["execution_id"].as_str().unwrap().to_string();
    assert_eq!(payload["state"], "Running");
    assert!(engine.get_execution(&id).await.is_some());

    // status：真实执行记录（既有测试只测了 not-found 臂）
    let payload = handler
        .handle_cmd("status", Some(serde_json::json!({ "execution_id": id })), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(payload["execution_id"], id);

    // start 命令成功臂
    let payload = handler
        .handle_cmd("start", Some(serde_json::json!({ "name": "runnow" })), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(payload["execution_id"].is_string());
}

#[tokio::test]
async fn wsapi_list_executions_filter_and_limit_with_real_runs() {
    let engine = build_test_engine();
    engine.register_workflow(instant_workflow("batchwf")).unwrap();
    for _ in 0..3 {
        engine.run("batchwf", HashMap::new(), None).await.unwrap();
    }
    let ctx = make_ctx_with_engine(engine);
    let handler = WorkflowHandler;

    let payload = handler
        .handle_cmd(
            "list_executions",
            Some(serde_json::json!({ "workflow_name": "batchwf", "state": "Completed" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(payload["total"], 3);
    assert_eq!(payload["count"], 3);

    let payload = handler
        .handle_cmd(
            "list_executions",
            Some(serde_json::json!({ "workflow_name": "batchwf", "limit": 2 })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(payload["total"], 3);
    assert_eq!(payload["count"], 2);
}

#[tokio::test]
async fn wsapi_list_and_get_checkpoints_with_mock_store() {
    let ctx = make_ctx_with_engine(engine_with_mock(
        MockLoad::Ok(make_checkpoint("cp5")),
        MockList::Ok(vec![CheckpointMeta::from(&make_checkpoint("cp5"))]),
    ));
    let handler = WorkflowHandler;

    let payload = handler
        .handle_cmd(
            "list_checkpoints",
            Some(serde_json::json!({ "execution_id": "exec-1" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(payload["checkpoints"].as_array().unwrap().len(), 1);

    let payload = handler
        .handle_cmd(
            "get_checkpoint",
            Some(serde_json::json!({ "execution_id": "exec-1", "checkpoint_id": "cp5" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(payload["checkpoint"]["id"], "cp5");

    // store 报错 → Err 字符串（Display 文本）
    let ctx = make_ctx_with_engine(engine_with_mock(MockLoad::NotFound, MockList::Io));
    let err = handler
        .handle_cmd(
            "list_checkpoints",
            Some(serde_json::json!({ "execution_id": "exec-1" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("IO error"), "got: {err}");
    let err = handler
        .handle_cmd(
            "get_checkpoint",
            Some(serde_json::json!({ "execution_id": "e", "checkpoint_id": "c" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("checkpoint not found"), "got: {err}");
}

#[tokio::test]
async fn wsapi_create_and_validate_missing_data_arms() {
    let ctx = make_ctx_with_engine(build_test_engine());
    let handler = WorkflowHandler;

    assert_eq!(handler.handle_cmd("create", None, &ctx).await.unwrap_err(), "missing data");
    assert!(handler
        .handle_cmd("create", Some(serde_json::json!({})), &ctx)
        .await
        .unwrap_err()
        .contains("missing field: workflow"));
    assert_eq!(handler.handle_cmd("validate", None, &ctx).await.unwrap_err(), "missing data");
    assert!(handler
        .handle_cmd("validate", Some(serde_json::json!({})), &ctx)
        .await
        .unwrap_err()
        .contains("missing field: workflow"));
}

#[tokio::test]
async fn wsapi_delete_without_defs_dir_succeeds_and_deregisters() {
    // defs_dir 未设 → delete_workflow_file 只清内存注册表，直接 Ok
    let engine = build_test_engine();
    engine.register_workflow(instant_workflow("delper")).unwrap();
    let ctx = make_ctx_with_engine(engine.clone());
    let r = WorkflowHandler
        .handle_cmd("delete", Some(serde_json::json!({ "name": "delper" })), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["deleted"], true);
    assert_eq!(r["name"], "delper");
    // 注册表里也没了
    let payload = WorkflowHandler
        .handle_cmd("list", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(payload["count"], 0);
}

// ---------------------------------------------------------------------------
// S10a 补充批次 2：llvm-cov 复测后仍缺的单行/单臂（504 超时 / GET 限流 /
// 内容不匹配签名 / create+update+validate 成功臂 / delete Err 闭包 /
// run_now 启动失败闭包 / chat password set+clear 臂）
// ---------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn rest_run_sync_timeout_returns_504_with_paused_clock() {
    // 暂停时钟下 tokio 自动推进：30s 超时先于 35s delay 触发，覆盖
    // handle_workflow_run 的 Err(_) => 504 workflow_run_timeout 臂。
    let engine = build_test_engine();
    let mut slow_cfg = HashMap::new();
    slow_cfg.insert("seconds".to_string(), serde_json::json!(35));
    engine
        .register_workflow(workflow_of("slowwf", "delay", slow_cfg))
        .unwrap();
    let state = state_with_engine(engine);
    let (status, body) = handle_workflow_run(
        axum::extract::State(state),
        HeaderMap::new(),
        Json(serde_json::json!({ "name": "slowwf" })),
    )
    .await
    .unwrap_err();
    assert_eq!(status, StatusCode::GATEWAY_TIMEOUT);
    assert_eq!(body["error"], "workflow_run_timeout");
    assert_eq!(body["timeout_secs"], 30);
}

#[tokio::test]
async fn webhook_get_variant_rate_limited_on_61st_call() {
    // GET 入口有自己独立的限流检查（POST 的 429 测试不经过这里）。
    let engine = build_test_engine();
    engine.register_workflow(webhook_workflow("hookgetrl", None)).unwrap();
    let state = state_with_engine(engine);
    for _ in 0..60 {
        let _ = handle_workflow_webhook_get(
            axum::extract::State(state.clone()),
            ConnectInfo(local_addr()),
            Path("hookgetrl".to_string()),
            Query(serde_json::json!({})),
        )
        .await;
    }
    let (status, body) = handle_workflow_webhook_get(
        axum::extract::State(state),
        ConnectInfo(local_addr()),
        Path("hookgetrl".to_string()),
        Query(serde_json::json!({})),
    )
    .await
    .unwrap_err();
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["error"], "rate limited");
    assert!(body["retry_after_secs"].is_number());
}

#[tokio::test]
async fn webhook_signed_correct_length_wrong_content_is_mismatch() {
    // 签名长度对（32 字节 hex）但内容不匹配 → diff != 0 → 401 signature mismatch。
    let engine = build_test_engine();
    engine
        .register_workflow(webhook_workflow("hookmm", Some("topsecret")))
        .unwrap();
    let state = state_with_engine(engine);
    let body = axum::body::Bytes::from_static(b"{\"event\":\"push\"}");
    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(b"topsecret").unwrap();
    // 对另一段 payload 签名，长度与真签名一致
    mac.update(b"{\"event\":\"forged\"}");
    let sig = hex(&mac.finalize().into_bytes());
    let (status, body_out) = handle_workflow_webhook(
        axum::extract::State(state),
        ConnectInfo(local_addr()),
        headers_with(Some(&sig)),
        Path("hookmm".to_string()),
        body,
    )
    .await
    .unwrap_err();
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body_out["reason"], "signature mismatch");
}

#[tokio::test]
async fn wsapi_create_update_validate_and_delete_error_with_defs_dir() {
    let engine = build_test_engine();
    let tmp = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_engine_and_defs_dir(engine.clone(), tmp.path());
    let handler = WorkflowHandler;

    // create 成功臂（serde 解析 + persist 落盘）
    let payload = handler
        .handle_cmd(
            "create",
            Some(serde_json::json!({ "workflow": sample_workflow_def("made_wf") })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(payload["created"], true);
    assert_eq!(payload["name"], "made_wf");
    assert!(tmp.path().join("made_wf.yaml").exists());
    assert!(engine.get_workflow("made_wf").is_some());

    // update 成功臂（名字强制对齐 + persist 覆盖）
    let payload = handler
        .handle_cmd(
            "update",
            Some(serde_json::json!({
                "name": "made_wf",
                "workflow": sample_workflow_def("renamed_ignored"),
            })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(payload["updated"], true);
    assert_eq!(payload["name"], "made_wf");
    assert!(engine.get_workflow("renamed_ignored").is_none());
    assert!(engine.get_workflow("made_wf").is_some());

    // validate 成功臂（拿到 errors 数组）
    let payload = handler
        .handle_cmd(
            "validate",
            Some(serde_json::json!({ "workflow": sample_workflow_def("made_wf") })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert!(payload["errors"].is_array());

    // 畸形 workflow 定义 → create/update/validate 三处 map_err 闭包
    // （"invalid workflow definition: ..."，只测过成功解析路径）
    let bad = serde_json::json!({ "nodes": "not-an-array" });
    for cmd in ["create", "update", "validate"] {
        let data = if cmd == "update" {
            serde_json::json!({ "name": "made_wf", "workflow": bad })
        } else {
            serde_json::json!({ "workflow": bad })
        };
        let err = handler.handle_cmd(cmd, Some(data), &ctx).await.unwrap_err();
        assert!(
            err.contains("invalid workflow definition"),
            "{cmd} err: {err}"
        );
    }

    // defs_dir 未设的 engine 上 update → persist_workflow 报
    // PersistenceError → update 臂的 persist map_err 闭包
    let bare_engine = build_test_engine();
    bare_engine.register_workflow(instant_workflow("bare_wf")).unwrap();
    let bare_ctx = make_ctx_with_engine(bare_engine);
    let err = handler
        .handle_cmd(
            "update",
            Some(serde_json::json!({
                "name": "bare_wf",
                "workflow": sample_workflow_def("bare_wf"),
            })),
            &bare_ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("workflow_defs_dir not set"), "err: {err}");

    // defs_dir 里放一个与目标同名的「目录」：delete_workflow_file 的
    // exists() 通过但 remove_file 对目录失败（Windows ERROR_ACCESS_DENIED）
    // → delete 臂的 map_err 闭包
    let dir_engine = build_test_engine();
    let tmp2 = tempfile::tempdir().unwrap();
    let ctx2 = make_ctx_with_engine_and_defs_dir(dir_engine, tmp2.path());
    std::fs::create_dir_all(tmp2.path().join("trap_wf.yaml")).unwrap();
    let err = handler
        .handle_cmd("delete", Some(serde_json::json!({ "name": "trap_wf" })), &ctx2)
        .await
        .unwrap_err();
    assert!(!err.is_empty(), "expected remove failure, got ok");

    // delete 成功 → 再 delete 一次：注册表已空 + 文件已删，仍是幂等 Ok
    // （delete_workflow_file 对缺失目标返回 Ok；Err 闭包只在真实 IO 失败时触达）
    let payload = handler
        .handle_cmd("delete", Some(serde_json::json!({ "name": "made_wf" })), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(payload["deleted"], true);
    let payload = handler
        .handle_cmd("delete", Some(serde_json::json!({ "name": "made_wf" })), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(payload["deleted"], true);
    assert!(engine.get_workflow("made_wf").is_none());
}

#[tokio::test]
async fn wsapi_run_now_and_start_unknown_workflow_map_start_errors() {
    // run_now / start 两臂各自的 start_async map_err 闭包：
    // WorkflowNotFound 由 start_async 同步返回。
    // （未知 node_type 是异步执行期失败，start_async 仍返回 Ok，盖不到这里。）
    let engine = build_test_engine();
    let ctx = make_ctx_with_engine(engine);
    for cmd in ["run_now", "start"] {
        let err = WorkflowHandler
            .handle_cmd(cmd, Some(serde_json::json!({ "name": "ghost_wf" })), &ctx)
            .await
            .unwrap_err();
        assert!(
            err.to_lowercase().contains("not found"),
            "{cmd} err: {err}"
        );
    }
}

#[tokio::test]
async fn wsapi_chat_password_set_and_clear_require_dashboard_auth() {
    let engine = build_test_engine();
    let mut ctx = make_ctx_with_engine(engine);
    let handler = WorkflowHandler;

    // AuthMethod::default() 是 Dashboard；用 WorkflowChat（独立聊天页会话）
    // 才会命中 permission_denied 闸。
    ctx.auth_method = crate::session::AuthMethod::WorkflowChat;
    let err = handler
        .handle_cmd(
            "set_chat_password",
            Some(serde_json::json!({ "index": "aabbccdd", "password": "p" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("permission_denied"));
    let err = handler
        .handle_cmd(
            "clear_chat_password",
            Some(serde_json::json!({ "index": "aabbccdd" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("permission_denied"));

    // Dashboard 会话：缺字段 / 空密码 / set 成功 / clear 成功
    ctx.auth_method = crate::session::AuthMethod::Dashboard;
    let err = handler
        .handle_cmd("set_chat_password", Some(serde_json::json!({})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("missing field"));
    let err = handler
        .handle_cmd(
            "set_chat_password",
            Some(serde_json::json!({ "index": "aabbccdd", "password": "" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("password must not be empty"));
    let payload = handler
        .handle_cmd(
            "set_chat_password",
            Some(serde_json::json!({ "index": "aabbccdd", "password": "secret123" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(payload["set"], true);
    // 空 index 穿过 handler 的字段提取 → store 端报
    // "index cannot be empty" → set 臂的 map_err 闭包
    let err = handler
        .handle_cmd(
            "set_chat_password",
            Some(serde_json::json!({ "index": "", "password": "x" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("index cannot be empty"), "err: {err}");
    let payload = handler
        .handle_cmd(
            "clear_chat_password",
            Some(serde_json::json!({ "index": "aabbccdd" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(payload["cleared"], true);
}
