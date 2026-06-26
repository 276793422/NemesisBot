//! Workflow REST + WSAPI handlers — milestones 1a-E3/E4, 1b-A1 step 8,
//! 1c-E5 (webhook hardening), 1c-E6 (sync API auth/timeout), 1c-E7 (WSAPI).
//!
//! REST routes:
//!   POST /api/workflow/run              — run synchronously, return final result
//!   POST /api/workflow/start            — start async, return execution_id
//!   GET  /api/workflow/list             — list registered workflows
//!   GET  /api/workflow/status/:id       — get one execution status
//!   GET  /api/workflow/executions       — list executions (with filters)
//!   POST /api/workflow/webhook/:name    — webhook trigger (HMAC + rate-limited)
//!   GET  /api/workflow/webhook/:name    — webhook trigger via query params
//!   GET  /api/workflow/checkpoints/:execution_id              — list checkpoints
//!   GET  /api/workflow/checkpoints/:execution_id/:checkpoint_id — load checkpoint
//!
//! WSAPI commands (module: "workflow"):
//!   workflow.list               — list registered workflows
//!   workflow.start              — async-start a workflow, returns execution_id
//!   workflow.status             — query one execution status
//!   workflow.cancel             — cancel a running execution
//!   workflow.resume             — resume a Waiting execution
//!   workflow.list_executions    — list executions (with filters)
//!   workflow.list_checkpoints   — list checkpoints for an execution (time travel)

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::api_handlers::AppState;
use nemesis_workflow::engine::WorkflowEngine;
use nemesis_workflow::types::TriggerSource;

/// Window for webhook rate limiting (1 minute, per CLAUDE.md plan).
const WEBHOOK_RATE_WINDOW: Duration = Duration::from_secs(60);
/// Max webhook calls per IP inside the window.
const WEBHOOK_RATE_MAX: usize = 60;

/// Helper: 503 JSON body when the workflow engine isn't injected.
fn engine_missing() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({
            "error": "workflow_engine_unavailable",
            "message": "Workflow engine is not configured on this gateway",
        })),
    )
}

/// Helper: map an [`EngineError`] to a status code + JSON body.
fn engine_error(err: nemesis_workflow::engine::EngineError) -> (StatusCode, Json<serde_json::Value>) {
    use nemesis_workflow::engine::EngineError::*;
    let (code, kind) = match &err {
        WorkflowNotFound(_) => (StatusCode::NOT_FOUND, "workflow_not_found"),
        ExecutionNotFound(_) => (StatusCode::NOT_FOUND, "execution_not_found"),
        CycleDetected(_) => (StatusCode::BAD_REQUEST, "cycle_detected"),
        PersistenceError(_) => (StatusCode::INTERNAL_SERVER_ERROR, "persistence_error"),
        ExecutionFailed(_) => (StatusCode::INTERNAL_SERVER_ERROR, "execution_failed"),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "engine_error"),
    };
    (
        code,
        Json(serde_json::json!({
            "error": kind,
            "message": err.to_string(),
        })),
    )
}

/// `POST /api/workflow/run` — run a workflow synchronously and return the
/// final execution result. Body: `{ "name": "...", "input": {...} }`.
///
/// **1c-E6**: adds `X-Auth-Token` auth and a 30s timeout. Longer workflows
/// must use `POST /api/workflow/start` instead.
pub async fn handle_workflow_run(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    // Auth: same convention as /api/internal — empty token means "auth optional".
    let token = headers
        .get("X-Auth-Token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !crate::api_handlers::verify_token(token, &state.auth_token) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "unauthorized"})),
        ));
    }

    let engine = state.workflow_engine.as_ref().ok_or_else(engine_missing)?;

    let name = payload
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "missing_field",
                    "message": "body must include string field 'name'",
                })),
            )
        })?;

    let input = parse_input_object(payload.get("input"));

    let run_future = engine.run(
        name,
        input,
        Some(TriggerSource::Webhook {
            payload: payload.clone(),
        }),
    );

    match tokio::time::timeout(WORKFLOW_RUN_TIMEOUT, run_future).await {
        Ok(result) => {
            let execution = result.map_err(engine_error)?;
            Ok(Json(execution_to_json(&execution)))
        }
        Err(_) => Err((
            StatusCode::GATEWAY_TIMEOUT,
            Json(serde_json::json!({
                "error": "workflow_run_timeout",
                "message": format!(
                    "synchronous workflow run exceeded {}s; use POST /api/workflow/start instead",
                    WORKFLOW_RUN_TIMEOUT_SECS
                ),
                "timeout_secs": WORKFLOW_RUN_TIMEOUT_SECS,
            })),
        )),
    }
}

/// Hard cap for `POST /api/workflow/run`. Workflows that legitimately take
/// longer must use the async `start` endpoint — long synchronous calls hold
/// an HTTP connection open and block clients.
const WORKFLOW_RUN_TIMEOUT: Duration = Duration::from_secs(WORKFLOW_RUN_TIMEOUT_SECS);
const WORKFLOW_RUN_TIMEOUT_SECS: u64 = 30;

/// `POST /api/workflow/start` — start a workflow asynchronously and return
/// the new execution_id. Body: `{ "name": "...", "input": {...} }`.
pub async fn handle_workflow_start(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let engine = state.workflow_engine.as_ref().ok_or_else(engine_missing)?;
    let arc_engine = Arc::clone(engine);

    let name = payload
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "missing_field",
                    "message": "body must include string field 'name'",
                })),
            )
        })?
        .to_string();

    let input = parse_input_object(payload.get("input"));

    let execution_id = WorkflowEngine::start_async(
        arc_engine,
        &name,
        input,
        Some(TriggerSource::Webhook {
            payload: payload.clone(),
        }),
    )
    .await
    .map_err(engine_error)?;

    Ok(Json(serde_json::json!({
        "execution_id": execution_id,
        "workflow_name": name,
        "state": "Running",
    })))
}

/// `GET /api/workflow/list` — list registered workflows with full trigger
/// driver status and next-fire timestamps. The UI reads `trigger_drivers`
/// and the per-workflow `triggers[].driven` field to render status — it
/// never hardcodes "event/message is undriven" knowledge on the client.
pub async fn handle_workflow_list(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let engine = state.workflow_engine.as_ref().ok_or_else(engine_missing)?;
    let summaries = engine.list_workflows_detailed();
    let driver_status_map = nemesis_workflow::all_driver_statuses();
    Ok(Json(serde_json::json!({
        "workflows": summaries,
        "trigger_driver_status": driver_status_map,
        "count": summaries.len(),
    })))
}

/// `GET /api/workflow/status/:id` — get execution status by ID.
pub async fn handle_workflow_status(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let engine = state.workflow_engine.as_ref().ok_or_else(engine_missing)?;
    match engine.get_execution(&id).await {
        Some(exec) => Ok(Json(execution_to_json(&exec))),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "execution_not_found",
                "message": format!("no execution with id {}", id),
            })),
        )),
    }
}

/// Query params for `/api/workflow/executions`.
#[derive(Debug, serde::Deserialize, Default)]
pub struct ExecutionListQuery {
    pub workflow_name: Option<String>,
    pub state: Option<String>,
    pub limit: Option<usize>,
}

/// `GET /api/workflow/executions?workflow_name=&state=&limit=` — list
/// executions, optionally filtered by workflow name / state.
pub async fn handle_workflow_executions(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ExecutionListQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let engine = state.workflow_engine.as_ref().ok_or_else(engine_missing)?;

    // The engine already filters by workflow_name. State filter is applied
    // client-side here because ExecutionState debug-format matching is
    // handler-level policy, not engine-level.
    let mut executions = engine
        .list_executions(q.workflow_name.as_deref())
        .await;

    if let Some(want_state) = &q.state {
        executions.retain(|e| format!("{:?}", e.state) == *want_state);
    }
    let total = executions.len();
    if let Some(limit) = q.limit {
        executions.truncate(limit);
    }

    let rows: Vec<serde_json::Value> = executions
        .iter()
        .map(execution_summary_json)
        .collect();

    Ok(Json(serde_json::json!({
        "executions": rows,
        "count": rows.len(),
        "total": total,
    })))
}

/// `POST /api/workflow/webhook/:name` — trigger a workflow via webhook.
///
/// **1c-E5 hardening**:
/// - HMAC-SHA256 signature verification via `X-Signature` header (only
///   enforced when the workflow defines a webhook `secret` in its trigger
///   config; unsigned workflows stay open)
/// - Per-IP rate limiting (default 60 req/min — see [`WEBHOOK_RATE_MAX`])
/// - Audit log emitted for every accepted / rejected call
/// - Body forwarded to the workflow as `input.payload`
pub async fn handle_workflow_webhook(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
    Path(name): Path<String>,
    body: axum::body::Bytes,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let client_ip = addr.ip();
    let body_bytes = body.as_ref();

    // Rate limit first — even before signature, so abusers can't burn HMAC.
    if let Err(retry_after) = state.webhook_rate_limiter.check(client_ip).await {
        audit_webhook(&state, &name, client_ip, "rate_limited", None);
        return Err(rate_limited(retry_after));
    }

    // Signature: only enforced when the workflow defines a webhook `secret`.
    // Look up workflow → find webhook trigger → read `config.secret`.
    let secret = workflow_webhook_secret(&state, &name).await;
    if let Some(secret) = secret {
        if let Err(reason) = verify_signature(&headers, body_bytes, secret.as_bytes()) {
            audit_webhook(&state, &name, client_ip, "bad_signature", Some(&reason));
            return Err(unauthorized(&reason));
        }
    }

    let payload: serde_json::Value =
        serde_json::from_slice(body_bytes).unwrap_or(serde_json::Value::Null);

    let execution_id = trigger_workflow_via_webhook(&state, &name, payload.clone())
        .await
        .map_err(engine_error)?;

    audit_webhook(&state, &name, client_ip, "accepted", Some(&execution_id));
    Ok(Json(serde_json::json!({
        "execution_id": execution_id,
        "workflow_name": name,
        "state": "Running",
    })))
}

/// `GET /api/workflow/webhook/:name` — same as POST but the payload comes
/// from the query string. Some external services (e.g. Slack's URL
/// verification flow) use GET webhooks. Signed workflows skip signature
/// verification here because GET has no body to HMAC — the `secret`
/// config is ignored on GET.
pub async fn handle_workflow_webhook_get(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<std::net::SocketAddr>,
    Path(name): Path<String>,
    Query(query): Query<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let client_ip = addr.ip();

    if let Err(retry_after) = state.webhook_rate_limiter.check(client_ip).await {
        audit_webhook(&state, &name, client_ip, "rate_limited", None);
        return Err(rate_limited(retry_after));
    }

    let payload = query;
    let execution_id = trigger_workflow_via_webhook(&state, &name, payload.clone())
        .await
        .map_err(engine_error)?;

    audit_webhook(&state, &name, client_ip, "accepted_get", Some(&execution_id));
    Ok(Json(serde_json::json!({
        "execution_id": execution_id,
        "workflow_name": name,
        "state": "Running",
    })))
}

async fn trigger_workflow_via_webhook(
    state: &AppState,
    name: &str,
    payload: serde_json::Value,
) -> Result<String, nemesis_workflow::engine::EngineError> {
    let engine = state.workflow_engine.as_ref().ok_or_else(|| {
        nemesis_workflow::engine::EngineError::InvalidState(
            "workflow engine not configured".to_string(),
        )
    })?;
    let arc_engine = Arc::clone(engine);
    let mut input = HashMap::new();
    input.insert("payload".to_string(), payload.clone());
    // Unified `input` field: the "main input string". For webhooks the main
    // input is the payload — if it's already a string use as-is, otherwise
    // JSON-serialise so downstream `{{input}}` always resolves to a string.
    let input_str = match &payload {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    input.insert("input".to_string(), serde_json::Value::String(input_str));
    WorkflowEngine::start_async(
        arc_engine,
        name,
        input,
        Some(TriggerSource::Webhook { payload }),
    )
    .await
}

/// Look up the webhook trigger's `secret` field for the named workflow.
/// Returns None if no secret is configured (unsigned webhook).
async fn workflow_webhook_secret(state: &AppState, name: &str) -> Option<String> {
    let engine = state.workflow_engine.as_ref()?;
    let workflow = engine.get_workflow(name)?;
    let wf = workflow.clone();
    for trigger in &wf.triggers {
        if trigger.trigger_type == "webhook" {
            if let Some(s) = trigger.config.get("secret").and_then(|v| v.as_str()) {
                return Some(s.to_string());
            }
        }
    }
    None
}

/// Verify `X-Signature: <hex HMAC-SHA256(secret, body)>`. Returns
/// `Err(reason)` on missing / malformed / mismatched signature.
fn verify_signature(
    headers: &HeaderMap,
    body: &[u8],
    secret: &[u8],
) -> Result<(), String> {
    let sig = headers
        .get("X-Signature")
        .ok_or_else(|| "missing X-Signature header".to_string())?
        .to_str()
        .map_err(|e| format!("invalid X-Signature header: {}", e))?;
    let sig = sig.trim();

    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(secret)
        .map_err(|e| format!("hmac key error: {}", e))?;
    mac.update(body);
    let expected_bytes = mac.finalize().into_bytes();

    // Accept hex (lower or upper) or raw base64.
    let provided_bytes = decode_signature(sig)
        .ok_or_else(|| "X-Signature is not valid hex or base64".to_string())?;
    if provided_bytes.len() != expected_bytes.len() {
        return Err(format!(
            "signature length mismatch: got {} expected {}",
            provided_bytes.len(),
            expected_bytes.len()
        ));
    }
    // Constant-time compare.
    let mut diff: u8 = 0;
    for (a, b) in provided_bytes.iter().zip(expected_bytes.iter()) {
        diff |= a ^ b;
    }
    if diff != 0 {
        return Err("signature mismatch".to_string());
    }
    Ok(())
}

/// Decode a webhook signature from hex (case-insensitive) or base64.
fn decode_signature(s: &str) -> Option<Vec<u8>> {
    // Strip optional `sha256=` prefix (used by GitHub / GitLab / Slack).
    let s = s.strip_prefix("sha256=").unwrap_or(s).trim();
    // Try hex first.
    if let Ok(bytes) = hex_decode(s) {
        return Some(bytes);
    }
    // Fall back to base64.
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(s).ok()
}

/// Tiny hex decoder (avoids pulling a hex crate just for this).
fn hex_decode(s: &str) -> Result<Vec<u8>, &'static str> {
    if s.len() % 2 != 0 {
        return Err("odd-length hex");
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    for chunk in bytes.chunks(2) {
        let hi = hex_nibble(chunk[0])?;
        let lo = hex_nibble(chunk[1])?;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}
fn hex_nibble(b: u8) -> Result<u8, &'static str> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err("invalid hex char"),
    }
}

/// Per-IP sliding-window webhook rate limiter (1c-E5).
///
/// Keeps a `VecDeque<Instant>` per IP, drops timestamps older than the
/// window on each `check`, and rejects once the queue length exceeds
/// `WEBHOOK_RATE_MAX`. Mutex is held for microseconds; no awaits inside.
pub struct WebhookRateLimiter {
    hits: tokio::sync::Mutex<HashMap<IpAddr, VecDeque<Instant>>>,
}

impl WebhookRateLimiter {
    pub fn new() -> Self {
        Self {
            hits: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Returns `Ok(())` if the call is allowed, or `Err(retry_after)`
    /// with how long the client should wait before retrying.
    pub async fn check(&self, ip: IpAddr) -> Result<(), Duration> {
        let mut hits = self.hits.lock().await;
        let now = Instant::now();
        let queue = hits.entry(ip).or_insert_with(VecDeque::new);
        // Drop timestamps outside the window.
        while let Some(&front) = queue.front() {
            if now.duration_since(front) >= WEBHOOK_RATE_WINDOW {
                queue.pop_front();
            } else {
                break;
            }
        }
        if queue.len() >= WEBHOOK_RATE_MAX {
            // Earliest timestamp still in window tells us when we can slip one in.
            let oldest = *queue.front().unwrap();
            let retry_after = WEBHOOK_RATE_WINDOW
                .saturating_sub(now.duration_since(oldest));
            return Err(retry_after);
        }
        queue.push_back(now);
        Ok(())
    }
}

impl Default for WebhookRateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

/// Emit a structured audit log line for the webhook. Routed to the
/// security audit log via the SSE log layer's target filter.
fn audit_webhook(
    _state: &AppState,
    workflow: &str,
    ip: IpAddr,
    outcome: &str,
    detail: Option<&str>,
) {
    tracing::info!(
        target: "nemesis_security::webhook_audit",
        workflow = %workflow,
        client_ip = %ip,
        outcome = %outcome,
        detail = ?detail,
        "webhook call"
    );
}

fn rate_limited(retry_after: Duration) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::TOO_MANY_REQUESTS,
        Json(serde_json::json!({
            "error": "rate limited",
            "retry_after_secs": retry_after.as_secs(),
        })),
    )
}

fn unauthorized(reason: &str) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({
            "error": "signature verification failed",
            "reason": reason,
        })),
    )
}

/// Pull `input` from a JSON request body into a `HashMap<String, Value>`.
/// Anything that isn't a JSON object becomes `{ "input": <value> }`.
fn parse_input_object(raw: Option<&serde_json::Value>) -> HashMap<String, serde_json::Value> {
    match raw {
        Some(serde_json::Value::Object(obj)) => {
            obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
        }
        Some(other) => {
            let mut m = HashMap::new();
            m.insert("input".to_string(), other.clone());
            m
        }
        None => HashMap::new(),
    }
}

/// Ensure the unified `input` field exists in the map.
///
/// Workflows reference the trigger's main input via `{{input}}`. If the
/// caller already set `input` (e.g. `parse_input_object` saw a non-object
/// value and wrapped it), leave it alone. Otherwise synthesise one from
/// the rest of the map so `{{input}}` always resolves to something.
fn ensure_unified_input(map: &mut HashMap<String, serde_json::Value>) {
    if map.contains_key("input") {
        return;
    }
    let obj: serde_json::Map<String, serde_json::Value> = map
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let serialized = serde_json::Value::Object(obj).to_string();
    map.insert(
        "input".to_string(),
        serde_json::Value::String(serialized),
    );
}

/// Build a detailed JSON view of an Execution for the status endpoint.
fn execution_to_json(exec: &nemesis_workflow::types::Execution) -> serde_json::Value {
    serde_json::json!({
        "execution_id": exec.id,
        "workflow_name": exec.workflow_name,
        "state": format!("{:?}", exec.state),
        "started_at": exec.started_at,
        "ended_at": exec.ended_at,
        "node_results": exec.node_results,
        "error": exec.error,
        "trigger_source": exec.trigger_source,
    })
}

/// Build a compact summary for the list endpoint (no node_results, just flags).
fn execution_summary_json(exec: &nemesis_workflow::types::Execution) -> serde_json::Value {
    serde_json::json!({
        "execution_id": exec.id,
        "workflow_name": exec.workflow_name,
        "state": format!("{:?}", exec.state),
        "started_at": exec.started_at,
        "ended_at": exec.ended_at,
        "has_error": exec.error.is_some(),
    })
}

/// Convenience helper for registering all workflow routes on a Router.
pub fn routes() -> axum::Router<Arc<AppState>> {
    use axum::routing::{get, post};
    axum::Router::new()
        .route("/api/workflow/run", post(handle_workflow_run))
        .route("/api/workflow/start", post(handle_workflow_start))
        .route("/api/workflow/list", get(handle_workflow_list))
        .route("/api/workflow/status/{id}", get(handle_workflow_status))
        .route("/api/workflow/executions", get(handle_workflow_executions))
        .route("/api/workflow/webhook/{name}", post(handle_workflow_webhook))
        .route(
            "/api/workflow/webhook/{name}",
            get(handle_workflow_webhook_get),
        )
        .route(
            "/api/workflow/checkpoints/{execution_id}",
            get(handle_workflow_checkpoints_list),
        )
        .route(
            "/api/workflow/checkpoints/{execution_id}/{checkpoint_id}",
            get(handle_workflow_checkpoint_load),
        )
        // Standalone workflow-chat page (public metadata + password verify).
        // See "Standalone workflow-chat HTTP endpoints" section below for
        // why these are unauthenticated.
        .route("/api/workflow/chat/info", get(handle_workflow_chat_info))
        .route("/api/workflow/chat/verify", post(handle_workflow_chat_verify))
}

/// List every checkpoint (metadata only) for an execution — milestone 1b-A1
/// step 8 ("time travel" read-only API). Returns `CheckpointMeta` records
/// oldest-first so callers can render an audit trail.
pub async fn handle_workflow_checkpoints_list(
    State(state): State<Arc<AppState>>,
    Path(execution_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let engine = match state.workflow_engine.clone() {
        Some(e) => e,
        None => return Err(engine_missing()),
    };
    let store = match engine.checkpoint_store() {
        Some(s) => s,
        None => {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "error": "checkpoint_store_unavailable",
                    "message": "Checkpoint persistence is not enabled on this gateway",
                })),
            ));
        }
    };

    match store.list(&execution_id).await {
        Ok(metas) => Ok(Json(serde_json::json!({
            "execution_id": execution_id,
            "checkpoints": metas,
        }))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": "checkpoint_list_failed",
                "message": e.to_string(),
            })),
        )),
    }
}

/// Load a specific checkpoint's full contents (context + variables + node
/// results) — milestone 1b-A1 step 8. Useful for diffing workflow state at
/// different points in time.
pub async fn handle_workflow_checkpoint_load(
    State(state): State<Arc<AppState>>,
    Path((execution_id, checkpoint_id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let engine = match state.workflow_engine.clone() {
        Some(e) => e,
        None => return Err(engine_missing()),
    };
    let store = match engine.checkpoint_store() {
        Some(s) => s,
        None => {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "error": "checkpoint_store_unavailable",
                    "message": "Checkpoint persistence is not enabled on this gateway",
                })),
            ));
        }
    };

    match store.load(&execution_id, &checkpoint_id).await {
        Ok(cp) => Ok(Json(serde_json::json!({
            "checkpoint": cp,
        }))),
        Err(e) => {
            use nemesis_workflow::checkpoint::StoreError;
            let (code, kind) = match &e {
                StoreError::NotFound { .. } => {
                    (StatusCode::NOT_FOUND, "checkpoint_not_found")
                }
                StoreError::Corrupt(_) => {
                    (StatusCode::INTERNAL_SERVER_ERROR, "checkpoint_corrupt")
                }
                _ => (StatusCode::INTERNAL_SERVER_ERROR, "checkpoint_load_failed"),
            };
            Err((
                code,
                Json(serde_json::json!({
                    "error": kind,
                    "message": e.to_string(),
                })),
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// Standalone workflow-chat HTTP endpoints
//
// These are intentionally public (no `X-Auth-Token` requirement). The
// standalone `/workflow/chat/<index>` page cannot use WSAPI for the initial
// handshake because WSAPI requires an already-authenticated WebSocket —
// and the WS upgrade needs to know whether a password is required before
// accepting the connection. HTTP endpoints break that chicken-and-egg.
//
// Information disclosure: indices are 8-hex chars (sha256(workflow_name)
// truncated), so enumerating them is impractical (2^32 search space). The
// metadata returned (name + description + chat_eligible) is what the page
// needs to render the password form.
// ---------------------------------------------------------------------------

/// Query params for `GET /api/workflow/chat/info`.
#[derive(Debug, serde::Deserialize)]
pub struct ChatInfoQuery {
    pub index: String,
}

/// `GET /api/workflow/chat/info?index=<8hex>` — public metadata used by the
/// standalone workflow-chat page before deciding whether to prompt for a
/// password. Returns `needs_password` so the client knows whether to show
/// the password form or connect the WebSocket immediately.
pub async fn handle_workflow_chat_info(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ChatInfoQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let engine = match state.workflow_engine.clone() {
        Some(e) => e,
        None => return Err(engine_missing()),
    };
    let needs_password = state.chat_secret_store.has_password(&q.index);
    match engine.workflow_by_chat_index(&q.index) {
        None => Ok(Json(serde_json::json!({
            "found": false,
            "chat_eligible": false,
            "needs_password": needs_password,
            "reason": "no workflow matches this index",
        }))),
        Some(name) => {
            let wf = engine
                .get_workflow(&name)
                .ok_or_else(|| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({
                            "error": "workflow_vanished",
                            "message": format!("workflow vanished after resolve: {}", name),
                        })),
                    )
                })?;
            let has_human_review = wf
                .nodes
                .iter()
                .any(|n| n.node_type == "human_review");
            let chat_eligible = !has_human_review;
            let reason = if has_human_review {
                Some(format!(
                    "工作流包含 human_review 节点，聊天测试不支持（v1 暂不处理 Waiting 状态）"
                ))
            } else {
                None
            };
            Ok(Json(serde_json::json!({
                "found": true,
                "workflow_name": name,
                "description": wf.description,
                "chat_eligible": chat_eligible,
                "needs_password": needs_password,
                "reason": reason,
            })))
        }
    }
}

/// `POST /api/workflow/chat/verify` — verify a workflow-chat password.
///
/// Body: `{ "index": string, "password": string }`. On success returns the
/// workflow metadata (same shape as `chat/info` minus `needs_password`,
/// plus `verified: true`). On failure returns 401.
pub async fn handle_workflow_chat_verify(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let engine = match state.workflow_engine.clone() {
        Some(e) => e,
        None => return Err(engine_missing()),
    };
    let index = payload
        .get("index")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "missing_field", "field": "index"})),
            )
        })?;
    let password = payload
        .get("password")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // Always run verify (constant-time on configured indexes; consumes time
    // on missing ones via DECOY_HASH) so the response time doesn't leak
    // whether an index is known.
    if !state.chat_secret_store.verify_password(index, password) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "verified": false,
                "error": "unauthorized",
            })),
        ));
    }

    let name = engine.workflow_by_chat_index(index).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "verified": false,
                "error": "workflow_not_found_for_index",
            })),
        )
    })?;
    let wf = engine.get_workflow(&name).ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": "workflow_vanished",
                "message": format!("workflow vanished after verify: {}", name),
            })),
        )
    })?;
    Ok(Json(serde_json::json!({
        "verified": true,
        "workflow_name": name,
        "description": wf.description,
    })))
}

// ---------------------------------------------------------------------------
// WebSocket API (WSAPI) — milestone 1c-E7
// ---------------------------------------------------------------------------

/// WebSocket handler for the `workflow` module. Mirrors the REST surface
/// above so the Vue dashboard can drive everything via the three-level
/// WebSocket protocol instead of HTTP.
///
/// Commands (cmd field of the WebSocket envelope):
///   list, start, status, cancel, resume, list_executions, list_checkpoints
pub struct WorkflowHandler;

#[async_trait::async_trait]
impl crate::ws_router::ModuleHandler for WorkflowHandler {
    fn module_name(&self) -> &str {
        "workflow"
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &crate::ws_router::RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let engine = ctx.state.workflow_engine.as_ref().ok_or_else(|| {
            "workflow engine is not configured on this gateway".to_string()
        })?;

        match cmd {
            "list" => {
                let summaries = engine.list_workflows_detailed();
                let driver_status_map = nemesis_workflow::all_driver_statuses();
                let store = ctx.state.chat_secret_store.clone();
                let workflows: Vec<serde_json::Value> = summaries
                    .iter()
                    .map(|s| {
                        let mut v = serde_json::to_value(s).unwrap_or(serde_json::Value::Null);
                        if let Some(obj) = v.as_object_mut() {
                            let has = store.has_password(&s.chat_index);
                            obj.insert(
                                "has_chat_password".to_string(),
                                serde_json::Value::Bool(has),
                            );
                        }
                        v
                    })
                    .collect();
                let count = workflows.len();
                Ok(Some(serde_json::json!({
                    "workflows": workflows,
                    "trigger_driver_status": driver_status_map,
                    "count": count,
                })))
            }

            "get" => {
                let data = data.ok_or("missing data")?;
                let name = data
                    .get("name")
                    .and_then(|v| v.as_str())
                    .ok_or("missing field: name")?;
                let wf = engine
                    .get_workflow(name)
                    .ok_or_else(|| format!("workflow_not_found: {}", name))?;
                let summary = engine.build_workflow_summary(&wf);
                Ok(Some(serde_json::json!({
                    "workflow": wf,
                    "summary": summary,
                })))
            }

            "create" => {
                let data = data.ok_or("missing data")?;
                let wf_raw = data
                    .get("workflow")
                    .ok_or("missing field: workflow")?;
                let wf: nemesis_workflow::types::Workflow = serde_json::from_value(wf_raw.clone())
                    .map_err(|e| format!("invalid workflow definition: {}", e))?;
                let name = wf.name.clone();
                engine
                    .persist_workflow(wf)
                    .map_err(|e| e.to_string())?;
                Ok(Some(serde_json::json!({
                    "name": name,
                    "created": true,
                })))
            }

            "update" => {
                let data = data.ok_or("missing data")?;
                let name = data
                    .get("name")
                    .and_then(|v| v.as_str())
                    .ok_or("missing field: name")?
                    .to_string();
                let wf_raw = data
                    .get("workflow")
                    .ok_or("missing field: workflow")?;
                let mut wf: nemesis_workflow::types::Workflow = serde_json::from_value(wf_raw.clone())
                    .map_err(|e| format!("invalid workflow definition: {}", e))?;
                // Force the name to match the URL/param — caller can't rename
                // via update; renames require delete + create.
                wf.name = name.clone();
                engine
                    .persist_workflow(wf)
                    .map_err(|e| e.to_string())?;
                Ok(Some(serde_json::json!({
                    "name": name,
                    "updated": true,
                })))
            }

            "delete" => {
                let data = data.ok_or("missing data")?;
                let name = data
                    .get("name")
                    .and_then(|v| v.as_str())
                    .ok_or("missing field: name")?;
                engine
                    .delete_workflow_file(name)
                    .map_err(|e| e.to_string())?;
                Ok(Some(serde_json::json!({
                    "name": name,
                    "deleted": true,
                })))
            }

            "validate" => {
                let data = data.ok_or("missing data")?;
                let wf_raw = data
                    .get("workflow")
                    .ok_or("missing field: workflow")?;
                let wf: nemesis_workflow::types::Workflow = serde_json::from_value(wf_raw.clone())
                    .map_err(|e| format!("invalid workflow definition: {}", e))?;
                let errors = nemesis_workflow::engine::WorkflowEngine::validate_workflow(&wf);
                Ok(Some(serde_json::json!({
                    "valid": errors.is_empty(),
                    "errors": errors,
                })))
            }

            "run_now" => {
                let data = data.ok_or("missing data")?;
                let name = data
                    .get("name")
                    .and_then(|v| v.as_str())
                    .ok_or("missing field: name")?
                    .to_string();
                let mut input = parse_input_object(data.get("input"));
                ensure_unified_input(&mut input);
                let exec_id = WorkflowEngine::start_async(
                    Arc::clone(engine),
                    &name,
                    input,
                    Some(TriggerSource::WebUI {
                        session_id: ctx.session_id.clone(),
                    }),
                )
                .await
                .map_err(|e| e.to_string())?;
                Ok(Some(serde_json::json!({
                    "execution_id": exec_id,
                    "workflow_name": name,
                    "state": "Running",
                })))
            }

            "start" => {
                let data = data.ok_or("missing data")?;
                let name = data
                    .get("name")
                    .and_then(|v| v.as_str())
                    .ok_or("missing field: name")?
                    .to_string();
                let mut input = parse_input_object(data.get("input"));
                ensure_unified_input(&mut input);
                let exec_id = WorkflowEngine::start_async(
                    Arc::clone(engine),
                    &name,
                    input,
                    Some(TriggerSource::WebUI {
                        session_id: ctx.session_id.clone(),
                    }),
                )
                .await
                .map_err(|e| e.to_string())?;
                Ok(Some(serde_json::json!({
                    "execution_id": exec_id,
                    "workflow_name": name,
                    "state": "Running",
                })))
            }

            // Manually fire a trigger event into the engine's EventDispatcher.
            // Used by the canvas page's "⚡ 模拟事件" button — let users test
            // `event` triggers without needing the real producer (e.g. without
            // waiting for a real workflow.completed lifecycle event).
            //
            // Payload: { event_type: string, data?: Record<string, any> }
            // Response: { event_type, data, matched_workflows: string[], published: true }
            "fire_event" => {
                let data = data.ok_or("missing data")?;
                let event_type = data
                    .get("event_type")
                    .and_then(|v| v.as_str())
                    .ok_or("missing field: event_type")?
                    .to_string();
                let data_map: std::collections::HashMap<String, serde_json::Value> = data
                    .get("data")
                    .and_then(|v| v.as_object())
                    .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                    .unwrap_or_default();
                let event = nemesis_workflow::event_dispatcher::TriggerEvent::new(
                    event_type.clone(),
                    data_map.clone(),
                );
                let matched = engine.workflows_matching_event(&event);
                engine.publish_event(event);
                Ok(Some(serde_json::json!({
                    "event_type": event_type,
                    "data": data_map,
                    "matched_workflows": matched,
                    "published": true,
                })))
            }

            // Resolve an opaque workflow-chat URL index back to a workflow
            // (name + description + chat_eligibility). Used by the
            // `WorkflowChatView` page before it lets the user type — the
            // UI needs the workflow title, and must reject chat-ineligible
            // workflows (those with `human_review` nodes pause indefinitely
            // and would hang the chat UI).
            //
            // Payload: { index: string }
            // Response: { found, workflow_name?, description?, chat_eligible, reason? }
            "resolve_chat_target" => {
                let data = data.ok_or("missing data")?;
                let index = data
                    .get("index")
                    .and_then(|v| v.as_str())
                    .ok_or("missing field: index")?;
                match engine.workflow_by_chat_index(index) {
                    None => Ok(Some(serde_json::json!({
                        "found": false,
                        "chat_eligible": false,
                        "reason": "no workflow matches this index",
                    }))),
                    Some(name) => {
                        let wf = engine
                            .get_workflow(&name)
                            .ok_or_else(|| format!("workflow vanished after resolve: {}", name))?;
                        let has_human_review = wf
                            .nodes
                            .iter()
                            .any(|n| n.node_type == "human_review");
                        let chat_eligible = !has_human_review;
                        let reason = if has_human_review {
                            Some(format!(
                                "工作流包含 human_review 节点，聊天测试不支持（v1 暂不处理 Waiting 状态）"
                            ))
                        } else {
                            None
                        };
                        Ok(Some(serde_json::json!({
                            "found": true,
                            "workflow_name": name,
                            "description": wf.description,
                            "chat_eligible": chat_eligible,
                            "reason": reason,
                        })))
                    }
                }
            }

            "status" => {
                let data = data.ok_or("missing data")?;
                let id = data
                    .get("execution_id")
                    .and_then(|v| v.as_str())
                    .ok_or("missing field: execution_id")?;
                match engine.get_execution(id).await {
                    Some(exec) => Ok(Some(execution_to_json(&exec))),
                    None => Err(format!("execution_not_found: {}", id)),
                }
            }

            "cancel" => {
                let data = data.ok_or("missing data")?;
                let id = data
                    .get("execution_id")
                    .and_then(|v| v.as_str())
                    .ok_or("missing field: execution_id")?;
                let exec = engine
                    .cancel_execution(id)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(Some(execution_to_json(&exec)))
            }

            "resume" => {
                let data = data.ok_or("missing data")?;
                let id = data
                    .get("execution_id")
                    .and_then(|v| v.as_str())
                    .ok_or("missing field: execution_id")?;
                let review = parse_input_object(data.get("review"));
                let exec = engine
                    .resume_execution(id, review)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(Some(execution_to_json(&exec)))
            }

            "list_executions" => {
                let workflow_name = data
                    .as_ref()
                    .and_then(|d| d.get("workflow_name"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let state_filter = data
                    .as_ref()
                    .and_then(|d| d.get("state"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let limit = data
                    .as_ref()
                    .and_then(|d| d.get("limit"))
                    .and_then(|v| v.as_u64())
                    .map(|n| n as usize);

                let mut executions = engine
                    .list_executions(workflow_name.as_deref())
                    .await;
                if let Some(want_state) = &state_filter {
                    executions.retain(|e| format!("{:?}", e.state) == *want_state);
                }
                let total = executions.len();
                if let Some(limit) = limit {
                    executions.truncate(limit);
                }
                let rows: Vec<serde_json::Value> = executions
                    .iter()
                    .map(execution_summary_json)
                    .collect();
                Ok(Some(serde_json::json!({
                    "executions": rows,
                    "count": rows.len(),
                    "total": total,
                })))
            }

            "list_checkpoints" => {
                let data = data.ok_or("missing data")?;
                let exec_id = data
                    .get("execution_id")
                    .and_then(|v| v.as_str())
                    .ok_or("missing field: execution_id")?;
                let store = engine.checkpoint_store().ok_or_else(|| {
                    "checkpoint_store_unavailable: persistence is not enabled".to_string()
                })?;
                let metas = store
                    .list(exec_id)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(Some(serde_json::json!({
                    "execution_id": exec_id,
                    "checkpoints": metas,
                })))
            }

            "get_checkpoint" => {
                let data = data.ok_or("missing data")?;
                let exec_id = data
                    .get("execution_id")
                    .and_then(|v| v.as_str())
                    .ok_or("missing field: execution_id")?;
                let cp_id = data
                    .get("checkpoint_id")
                    .and_then(|v| v.as_str())
                    .ok_or("missing field: checkpoint_id")?;
                let store = engine.checkpoint_store().ok_or_else(|| {
                    "checkpoint_store_unavailable: persistence is not enabled".to_string()
                })?;
                match store.load(exec_id, cp_id).await {
                    Ok(cp) => Ok(Some(serde_json::json!({
                        "checkpoint": cp,
                    }))),
                    Err(e) => Err(e.to_string()),
                }
            }

            "set_chat_password" => {
                // Dashboard-only: a session that connected via the standalone
                // workflow-chat page must not be able to mutate passwords.
                if ctx.auth_method != crate::session::AuthMethod::Dashboard {
                    return Err("permission_denied: set_chat_password requires dashboard auth"
                        .to_string());
                }
                let data = data.ok_or("missing data")?;
                let index = data
                    .get("index")
                    .and_then(|v| v.as_str())
                    .ok_or("missing field: index")?;
                let password = data
                    .get("password")
                    .and_then(|v| v.as_str())
                    .ok_or("missing field: password")?;
                if password.is_empty() {
                    return Err("password must not be empty".to_string());
                }
                let store = ctx.state.chat_secret_store.clone();
                store
                    .set_password(index, password)
                    .map_err(|e| format!("set_chat_password failed: {}", e))?;
                Ok(Some(serde_json::json!({
                    "index": index,
                    "set": true,
                })))
            }

            "clear_chat_password" => {
                if ctx.auth_method != crate::session::AuthMethod::Dashboard {
                    return Err("permission_denied: clear_chat_password requires dashboard auth"
                        .to_string());
                }
                let data = data.ok_or("missing data")?;
                let index = data
                    .get("index")
                    .and_then(|v| v.as_str())
                    .ok_or("missing field: index")?;
                let store = ctx.state.chat_secret_store.clone();
                store
                    .clear_password(index)
                    .map_err(|e| format!("clear_chat_password failed: {}", e))?;
                Ok(Some(serde_json::json!({
                    "index": index,
                    "cleared": true,
                })))
            }

            "verify_chat_password" => {
                // Public: callable from the standalone page. Returns metadata
                // (workflow_name + description) on success so the page can
                // render without a separate resolve call.
                let data = data.ok_or("missing data")?;
                let index = data
                    .get("index")
                    .and_then(|v| v.as_str())
                    .ok_or("missing field: index")?;
                let password = data
                    .get("password")
                    .and_then(|v| v.as_str())
                    .ok_or("missing field: password")?;
                let store = ctx.state.chat_secret_store.clone();
                if !store.verify_password(index, password) {
                    return Err("unauthorized".to_string());
                }
                let name = engine.workflow_by_chat_index(index);
                match name {
                    Some(name) => {
                        let wf = engine.get_workflow(&name);
                        let description = wf
                            .as_ref()
                            .map(|w| w.description.clone())
                            .unwrap_or_default();
                        Ok(Some(serde_json::json!({
                            "index": index,
                            "workflow_name": name,
                            "description": description,
                            "verified": true,
                        })))
                    }
                    None => Err(format!("workflow_not_found_for_index: {}", index)),
                }
            }

            _ => Err(format!("unknown command: workflow.{}", cmd)),
        }
    }
}

#[cfg(test)]
mod tests {
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
            chat_secret_store: std::sync::Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::in_memory()),
            webhook_rate_limiter: Arc::new(WebhookRateLimiter::new()),
            internal_cmd_tx: None,
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
        let result = handle_workflow_run(
            axum::extract::State(state),
            HeaderMap::new(),
            payload,
        )
        .await;
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
            chat_secret_store: std::sync::Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::in_memory()),
            webhook_rate_limiter: Arc::new(WebhookRateLimiter::new()),
            internal_cmd_tx: None,
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
        let err = handler
            .handle_cmd("create", data, &ctx)
            .await
            .unwrap_err();
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
            .handle_cmd(
                "delete",
                Some(serde_json::json!({"name": "wf_z"})),
                &ctx,
            )
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
        let err = handler
            .handle_cmd("run_now", data, &ctx)
            .await
            .unwrap_err();
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
        let err = handler
            .handle_cmd("create", data, &ctx)
            .await
            .unwrap_err();
        assert!(err.contains("workflow_defs_dir"), "got: {}", err);
    }
}

