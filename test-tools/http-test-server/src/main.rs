//! Unified HTTP Test Server for NemesisBot testing.
//!
//! Merges the Go test_http_server, auth_test_server, and channel_webhook_server
//! into a single Rust server with all endpoints.
//!
//! Usage:
//!   http-test-server              # Start on port 8081
//!   http-test-server 9090         # Start on custom port

use axum::{
    Router,
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Json},
    routing::{get, post},
};
use clap::Parser;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tower_http::trace::TraceLayer;
use tracing::info;

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
struct RequestLog {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: String,
    timestamp: String,
    query_params: HashMap<String, String>,
}

struct AppState {
    requests: Mutex<Vec<RequestLog>>,
    callbacks: Mutex<HashMap<String, tokio::sync::oneshot::Sender<String>>>,
}

impl AppState {
    fn new() -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            callbacks: Mutex::new(HashMap::new()),
        }
    }
}

// ---------------------------------------------------------------------------
// Query params
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CallbackParams {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FileParams {
    file: Option<String>,
}

// ---------------------------------------------------------------------------
// Handlers — General
// ---------------------------------------------------------------------------

async fn handle_root(State(_state): State<Arc<AppState>>) -> impl IntoResponse {
    info!("GET /");
    Json(json!({
        "status": "running",
        "server": "NemesisBot HTTP Test Server",
    }))
}

async fn handle_echo(
    State(_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
    body: Bytes,
) -> impl IntoResponse {
    let hdrs: HashMap<String, String> = headers
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();

    info!("POST /echo ({} bytes)", body.len());

    Json(json!({
        "method": "POST",
        "path": "/echo",
        "headers": hdrs,
        "body": String::from_utf8_lossy(&body),
        "query": query,
    }))
}

async fn handle_delay(Path(secs): Path<u64>) -> impl IntoResponse {
    info!("GET /delay/{}", secs);
    tokio::time::sleep(Duration::from_secs(secs)).await;
    (StatusCode::OK, format!("Delayed {} seconds\n", secs))
}

async fn handle_status(Path(code): Path<u16>) -> impl IntoResponse {
    info!("GET /status/{}", code);
    let status = StatusCode::from_u16(code).unwrap_or(StatusCode::OK);
    (status, format!("Status {}\n", code))
}

async fn handle_health() -> impl IntoResponse {
    Json(json!({ "status": "ok" }))
}

// ---------------------------------------------------------------------------
// Handlers — OAuth
// ---------------------------------------------------------------------------

async fn handle_oauth_callback(
    State(state): State<Arc<AppState>>,
    Query(params): Query<CallbackParams>,
) -> Html<String> {
    info!("GET /oauth/callback code={:?} state={:?}", params.code, params.state);

    if let Some(_error) = &params.error {
        return Html("<html><body><h2>Error</h2></body></html>".to_string());
    }

    let code = params.code.clone().unwrap_or_default();
    let state_val = params.state.clone().unwrap_or_default();

    // Send code to callback channel if registered
    if !state_val.is_empty() {
        let mut callbacks = state.callbacks.lock().await;
        if let Some(tx) = callbacks.remove(&state_val) {
            let _ = tx.send(code.clone());
        }
    }

    Html(format!(
        "<html><body><h2>Authentication successful!</h2><p>Code: {}</p><p>You can close this window.</p></body></html>",
        code
    ))
}

async fn handle_oauth_device() -> impl IntoResponse {
    info!("POST /oauth/device");
    let ts = chrono::Utc::now().timestamp();
    Json(json!({
        "device_auth_id": format!("test_device_{}", ts),
        "user_code": "TEST-CODE",
        "interval": 5,
    }))
}

async fn handle_oauth_token() -> impl IntoResponse {
    info!("POST /oauth/token");
    let mock_token = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ0ZXN0X3VzZXIiLCJleHAiOjE5OTk5OTk5OTl9.signature";
    Json(json!({
        "access_token": mock_token,
        "refresh_token": format!("refresh_{}", mock_token),
        "token_type": "Bearer",
        "expires_in": 3600,
        "id_token": mock_token,
    }))
}

// ---------------------------------------------------------------------------
// Handlers — Webhook
// ---------------------------------------------------------------------------

async fn handle_webhook(headers: HeaderMap, body: Bytes) -> impl IntoResponse {
    info!("POST /webhook ({} bytes)", body.len());
    let hdrs: HashMap<String, String> = headers
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    Json(json!({
        "webhook_received": true,
        "body": String::from_utf8_lossy(&body),
        "headers": hdrs,
    }))
}

// ---------------------------------------------------------------------------
// Handlers — Channel Webhooks
// ---------------------------------------------------------------------------

async fn handle_telegram_webhook(_body: String) -> impl IntoResponse {
    info!("POST /telegram/webhook");
    Json(json!({
        "ok": true,
        "result": true,
        "webhook_received": true,
    }))
}

async fn handle_line_webhook(_body: String) -> impl IntoResponse {
    info!("POST /line/webhook");
    Json(json!({
        "webhook_received": true,
    }))
}

async fn handle_slack_events(_body: String) -> impl IntoResponse {
    info!("POST /slack/events");
    Json(json!({ "ok": true }))
}

async fn handle_discord_webhook(_body: String) -> impl IntoResponse {
    info!("POST /discord/webhook");
    Json(json!({ "message": "Webhook received" }))
}

async fn handle_onebot_ws() -> impl IntoResponse {
    info!("GET /onebot/ws");
    (StatusCode::OK, "WebSocket endpoint - would upgrade here\n")
}

// ---------------------------------------------------------------------------
// Handlers — Files
// ---------------------------------------------------------------------------

async fn handle_file_download(Query(params): Query<FileParams>) -> impl IntoResponse {
    let filename = params.file.unwrap_or_else(|| "test.txt".to_string());
    info!("GET /files/download?file={}", filename);
    (
        StatusCode::OK,
        [
            ("content-type", "application/octet-stream".to_string()),
            ("content-disposition", format!("attachment; filename={}", filename)),
        ],
        "Mock file content".to_string(),
    )
}

// ---------------------------------------------------------------------------
// Handlers — External Channel
// ---------------------------------------------------------------------------

async fn handle_external_input(body: String) -> impl IntoResponse {
    info!("POST /external/input");
    let parsed: Value = serde_json::from_str(&body).unwrap_or(json!({}));
    Json(parsed)
}

async fn handle_external_output(_body: String) -> impl IntoResponse {
    info!("POST /external/output");
    StatusCode::OK
}

// ---------------------------------------------------------------------------
// Handlers — MCP API
// ---------------------------------------------------------------------------

async fn handle_tools_list() -> impl IntoResponse {
    info!("GET /api/tools/list");
    Json(json!({
        "tools": [{
            "name": "test_tool",
            "description": "A test tool",
            "inputSchema": { "type": "object" }
        }]
    }))
}

async fn handle_resources_list() -> impl IntoResponse {
    info!("GET /api/resources/list");
    Json(json!({
        "resources": [{
            "uri": "file:///test.txt",
            "name": "Test File",
            "mimeType": "text/plain",
            "description": "A test file"
        }]
    }))
}

async fn handle_prompts_list() -> impl IntoResponse {
    info!("GET /api/prompts/list");
    Json(json!({
        "prompts": [{
            "name": "test_prompt",
            "description": "A test prompt",
            "arguments": []
        }]
    }))
}

// ---------------------------------------------------------------------------
// Handlers — Request log
// ---------------------------------------------------------------------------

async fn handle_requests(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let requests = state.requests.lock().await;
    Json(json!({ "requests": *requests }))
}

async fn handle_clear_requests(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let mut requests = state.requests.lock().await;
    requests.clear();
    Json(json!({ "cleared": true }))
}

// ---------------------------------------------------------------------------
// CLI & main
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(name = "http-test-server")]
#[command(about = "Unified HTTP test server for NemesisBot testing")]
struct Cli {
    /// Port to listen on (default: 8081)
    port: Option<u16>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter("http_test_server=info")
        .init();

    let cli = Cli::parse();
    let port = cli.port.unwrap_or(8081);

    let state = Arc::new(AppState::new());

    let app = Router::new()
        // General
        .route("/", get(handle_root))
        .route("/health", get(handle_health))
        .route("/echo", post(handle_echo))
        .route("/delay/{secs}", get(handle_delay))
        .route("/status/{code}", get(handle_status))
        // OAuth
        .route("/oauth/callback", get(handle_oauth_callback))
        .route("/oauth/device", post(handle_oauth_device))
        .route("/oauth/token", post(handle_oauth_token))
        // Webhook
        .route("/webhook", post(handle_webhook))
        // Channel webhooks
        .route("/telegram/webhook", post(handle_telegram_webhook))
        .route("/line/webhook", post(handle_line_webhook))
        .route("/slack/events", post(handle_slack_events))
        .route("/discord/webhook", post(handle_discord_webhook))
        .route("/onebot/ws", get(handle_onebot_ws))
        // Files
        .route("/files/download", get(handle_file_download))
        // External channel
        .route("/external/input", post(handle_external_input))
        .route("/external/output", post(handle_external_output))
        // MCP API
        .route("/api/tools/list", get(handle_tools_list))
        .route("/api/resources/list", get(handle_resources_list))
        .route("/api/prompts/list", get(handle_prompts_list))
        // Request log
        .route("/requests", get(handle_requests))
        .route("/requests/clear", post(handle_clear_requests))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr = format!("0.0.0.0:{}", port);
    info!("HTTP test server starting on {}", addr);
    info!("Endpoints:");
    info!("  General:  GET / | POST /echo | GET /delay/{{secs}} | GET /status/{{code}}");
    info!("  OAuth:    GET /oauth/callback | POST /oauth/device | POST /oauth/token");
    info!("  Webhook:  POST /webhook | POST /telegram/webhook | POST /slack/events");
    info!("  Channels: POST /discord/webhook | POST /line/webhook | GET /onebot/ws");
    info!("  Files:    GET /files/download?file=test.txt");
    info!("  External: POST /external/input | POST /external/output");
    info!("  MCP:      GET /api/tools/list | GET /api/resources/list | GET /api/prompts/list");
    info!("  Log:      GET /requests | POST /requests/clear");

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
