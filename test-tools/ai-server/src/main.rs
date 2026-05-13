//! NemesisBot - AI Mock Server
//!
//! OpenAI-compatible HTTP API mock for testing.
//! Implements /v1/chat/completions (with SSE streaming), /v1/models, and /v1/help endpoints.
//!
//! Models:
//!   testai-1.1  — Immediate response "好的，我知道了"
//!   testai-1.2  — 30s delayed response
//!   testai-1.3  — 300s (5min) delayed response
//!   testai-2.0  — Echo model (returns last user message)
//!   testai-3.0  — Cluster model (detects <PEER_CHAT> tags → cluster_rpc tool call)
//!   testai-4.2  — Sleep tool model (30s)
//!   testai-4.3  — Sleep tool model (300s)
//!   testai-5.0  — Security model (detects <FILE_OP> tags → file operation tool calls)
//!
//! Environment variable overrides:
//!   RESPONSE_MODE — "default"/"error"/"multi_tool"/"tool_chain_<name>"
//!   TOOL_ARGS     — JSON object for tool arguments
//!   MAX_ROUNDS    — conversation round limit (default 1)

use std::convert::Infallible;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::body::Body;
use axum::extract::State as AxumState;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{
    sse::{Event, KeepAlive, Sse},
    IntoResponse,
};
use axum::routing::{get, post};
use axum::Json;
use bytes::Bytes;
use clap::Parser;
use futures::stream::{self, Stream};
use serde::{Deserialize, Serialize};
use tokio::time::Duration;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser, Debug)]
#[command(name = "ai-server", about = "OpenAI-compatible mock server for testing")]
struct Args {
    /// Port to listen on
    #[arg(long, default_value_t = 18080)]
    port: u16,
}

// ---------------------------------------------------------------------------
// Model definitions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct ModelDef {
    id: &'static str,
    description: &'static str,
    category: &'static str,
    delay_secs: u64,
}

static MODELS: &[ModelDef] = &[
    ModelDef { id: "testai-1.1", description: "Immediate response", category: "basic", delay_secs: 0 },
    ModelDef { id: "testai-1.2", description: "30s delayed response", category: "timeout", delay_secs: 30 },
    ModelDef { id: "testai-1.3", description: "300s (5min) delayed response", category: "timeout", delay_secs: 300 },
    ModelDef { id: "testai-2.0", description: "Echo model (returns last user message)", category: "basic", delay_secs: 0 },
    ModelDef { id: "testai-3.0", description: "Cluster model (PEER_CHAT detection)", category: "cluster", delay_secs: 0 },
    ModelDef { id: "testai-4.2", description: "Sleep tool model (30s)", category: "tools", delay_secs: 0 },
    ModelDef { id: "testai-4.3", description: "Sleep tool model (300s)", category: "tools", delay_secs: 0 },
    ModelDef { id: "testai-5.0", description: "Security model (FILE_OP detection)", category: "security", delay_secs: 0 },
];

fn find_model(id: &str) -> Option<&'static ModelDef> {
    MODELS.iter().find(|m| m.id == id)
}

// ---------------------------------------------------------------------------
// Environment variable helpers
// ---------------------------------------------------------------------------

fn get_response_mode() -> String {
    std::env::var("RESPONSE_MODE").unwrap_or_else(|_| "default".into())
}

fn get_tool_args() -> serde_json::Value {
    std::env::var("TOOL_ARGS")
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::json!({}))
}

// ---------------------------------------------------------------------------
// OpenAI-compatible request / response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    #[serde(default)]
    tools: Vec<ToolDef>,
    #[serde(default)]
    stream: bool,
}

#[derive(Debug, Deserialize)]
struct ChatMessage {
    role: String,
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    tool_call_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ToolDef {
    #[allow(dead_code)]
    r#type: Option<String>,
    function: ToolFunction,
}

#[derive(Debug, Deserialize)]
struct ToolFunction {
    name: String,
    #[allow(dead_code)]
    description: Option<String>,
    #[allow(dead_code)]
    parameters: Option<serde_json::Value>,
}

// Response types

#[derive(Debug, Serialize)]
struct ChatResponse {
    id: String,
    object: String,
    created: i64,
    model: String,
    choices: Vec<Choice>,
    usage: Usage,
}

#[derive(Debug, Serialize)]
struct Choice {
    index: u32,
    message: ResponseMessage,
    finish_reason: String,
}

#[derive(Debug, Serialize, Clone)]
struct ResponseMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<ToolCallResponse>>,
}

#[derive(Debug, Serialize, Clone)]
struct ToolCallResponse {
    id: String,
    r#type: String,
    function: FunctionResponse,
}

#[derive(Debug, Serialize, Clone)]
struct FunctionResponse {
    name: String,
    arguments: String,
}

#[derive(Debug, Serialize)]
struct Usage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

#[derive(Debug, Serialize)]
struct ModelsResponse {
    object: String,
    data: Vec<ModelEntry>,
}

#[derive(Debug, Serialize)]
struct ModelEntry {
    id: String,
    object: String,
    created: i64,
    owned_by: String,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: ErrorDetail,
}

#[derive(Debug, Serialize)]
struct ErrorDetail {
    message: String,
    r#type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<String>,
}

// ---------------------------------------------------------------------------
// File logger (Go-compatible format)
// ---------------------------------------------------------------------------

fn log_request_to_file(model: &str, raw_body: &[u8]) {
    let log_dir = std::path::Path::new("log").join(model);
    if let Err(e) = std::fs::create_dir_all(&log_dir) {
        tracing::warn!("Failed to create log dir {}: {e}", log_dir.display());
        return;
    }
    let now = chrono::Local::now();
    let filename = format!("{}.log", now.format("%Y%m%d_%H%M%S%.3f"));
    let filepath = log_dir.join(&filename);
    let timestamp = now.format("%Y-%m-%d %H:%M:%S%.3f");
    let body_len = raw_body.len();
    let pretty_body = serde_json::from_slice::<serde_json::Value>(raw_body)
        .map(|v| serde_json::to_string_pretty(&v).unwrap_or_else(|_| String::from_utf8_lossy(raw_body).into()))
        .unwrap_or_else(|_| String::from_utf8_lossy(raw_body).into());
    let content = format!(
        "========================================\n\
         TestAIServer Request Log (Detailed)\n\
         ========================================\n\
         \n\
         Timestamp: {timestamp}\n\
         \n\
         --- Raw Request Body ---\n\
         Length: {body_len} bytes\n\
         \n\
         {pretty_body}\n\
         \n\
         --- Gin Context ---\n\
         Client IP: 127.0.0.1\n\
         Content Length: {body_len}\n\
         Content Type: application/json\n\
         \n\
         ========================================\n\
         End of Log\n\
         ========================================\n"
    );
    if let Err(e) = std::fs::write(&filepath, content) {
        tracing::warn!("Failed to write log {}: {e}", filepath.display());
    }
}

// ---------------------------------------------------------------------------
// Response helpers
// ---------------------------------------------------------------------------

fn make_id() -> String {
    format!("chatcmpl-{}", uuid::Uuid::new_v4().simple())
}

fn make_tool_call_id() -> String {
    format!("call-{}", chrono::Utc::now().timestamp_millis())
}

fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

fn text_response(id: &str, created: i64, model: &str, text: &str) -> ChatResponse {
    ChatResponse {
        id: id.into(),
        object: "chat.completion".into(),
        created,
        model: model.into(),
        choices: vec![Choice {
            index: 0,
            message: ResponseMessage {
                role: "assistant".into(),
                content: Some(text.into()),
                tool_calls: None,
            },
            finish_reason: "stop".into(),
        }],
        usage: Usage { prompt_tokens: 10, completion_tokens: 8, total_tokens: 18 },
    }
}

fn tool_call_response(id: &str, created: i64, model: &str, tool_name: &str, args: &serde_json::Value) -> ChatResponse {
    ChatResponse {
        id: id.into(),
        object: "chat.completion".into(),
        created,
        model: model.into(),
        choices: vec![Choice {
            index: 0,
            message: ResponseMessage {
                role: "assistant".into(),
                content: None,
                tool_calls: Some(vec![ToolCallResponse {
                    id: make_tool_call_id(),
                    r#type: "function".into(),
                    function: FunctionResponse {
                        name: tool_name.into(),
                        arguments: serde_json::to_string(args).unwrap_or_else(|_| "{}".into()),
                    },
                }]),
            },
            finish_reason: "tool_calls".into(),
        }],
        usage: Usage { prompt_tokens: 50, completion_tokens: 20, total_tokens: 70 },
    }
}

/// Check if any message is a tool result (second+ round).
fn has_tool_results(messages: &[ChatMessage]) -> bool {
    messages.iter().any(|m| m.tool_call_id.is_some())
}

/// Get last user message content.
fn last_user_content(messages: &[ChatMessage]) -> Option<String> {
    messages.iter().rev().find(|m| m.role == "user").and_then(|m| m.content.clone())
}

// ---------------------------------------------------------------------------
// Extract <TAG>...</TAG> content
// ---------------------------------------------------------------------------

fn extract_tag(content: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = content.find(&open)?;
    let rest = &content[start + open.len()..];
    let end = rest.find(&close)?;
    Some(rest[..end].trim().to_string())
}

// ---------------------------------------------------------------------------
// FILE_OP operation mapping (testai-5.0)
// ---------------------------------------------------------------------------

fn file_op_to_tool(op: &str) -> Option<&'static str> {
    match op {
        "file_read" => Some("read_file"),
        "file_write" => Some("write_file"),
        "file_delete" => Some("delete_file"),
        "file_append" => Some("append_file"),
        "dir_create" => Some("create_dir"),
        "dir_delete" => Some("delete_dir"),
        "dir_list" => Some("list_dir"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Model-specific response logic
// ---------------------------------------------------------------------------

fn generate_model_response(model_id: &str, req: &ChatRequest, id: &str, created: i64) -> ChatResponse {
    // Check environment variable overrides first
    let mode = get_response_mode();
    if mode != "default" {
        return generate_env_response(&mode, req, id, created);
    }

    // Model-specific behavior
    match model_id {
        "testai-1.1" => text_response(id, created, model_id, "好的，我知道了"),

        "testai-1.2" | "testai-1.3" => {
            // Delay is handled by tokio::time::sleep in the handler
            text_response(id, created, model_id, "好的，我知道了")
        }

        "testai-2.0" => {
            let content = last_user_content(&req.messages).unwrap_or_default();
            text_response(id, created, model_id, &content)
        }

        "testai-3.0" => {
            // Cluster model: detect <PEER_CHAT> tags
            if let Some(content) = last_user_content(&req.messages) {
                if let Some(json_str) = extract_tag(&content, "PEER_CHAT") {
                    if let Ok(peer_data) = serde_json::from_str::<serde_json::Value>(&json_str) {
                        let args = serde_json::json!({
                            "peer_id": peer_data["peer_id"].as_str().unwrap_or("target-agent-id"),
                            "action": "peer_chat",
                            "data": {
                                "type": "chat",
                                "content": peer_data["content"].as_str().unwrap_or("hello")
                            }
                        });
                        return tool_call_response(id, created, model_id, "cluster_rpc", &args);
                    }
                }
            }
            // No PEER_CHAT tag: return user message directly
            let content = last_user_content(&req.messages).unwrap_or_default();
            text_response(id, created, model_id, &content)
        }

        "testai-4.2" => {
            if has_tool_results(&req.messages) {
                text_response(id, created, model_id, "工作完成")
            } else {
                tool_call_response(id, created, model_id, "sleep", &serde_json::json!({"duration": 30}))
            }
        }

        "testai-4.3" => {
            if has_tool_results(&req.messages) {
                text_response(id, created, model_id, "工作完成")
            } else {
                tool_call_response(id, created, model_id, "sleep", &serde_json::json!({"duration": 300}))
            }
        }

        "testai-5.0" => {
            // Security model: detect <FILE_OP> tags
            if let Some(content) = last_user_content(&req.messages) {
                if let Some(json_str) = extract_tag(&content, "FILE_OP") {
                    if let Ok(file_op) = serde_json::from_str::<serde_json::Value>(&json_str) {
                        let operation = file_op["operation"].as_str().unwrap_or("file_read");
                        if let Some(tool_name) = file_op_to_tool(operation) {
                            let mut args = serde_json::json!({
                                "path": file_op["path"].as_str().unwrap_or("/tmp/test.txt"),
                            });
                            if let Some(content_val) = file_op.get("content") {
                                args["content"] = content_val.clone();
                            }
                            return tool_call_response(id, created, model_id, tool_name, &args);
                        }
                    }
                }
            }
            text_response(id, created, model_id, "No file operation detected.")
        }

        // Default/fallback: if tools provided, call first one; else text
        _ => generate_default_response(req, id, created, model_id),
    }
}

/// Generate response based on RESPONSE_MODE env var.
fn generate_env_response(mode: &str, req: &ChatRequest, id: &str, created: i64) -> ChatResponse {
    if has_tool_results(&req.messages) {
        return text_response(id, created, &req.model, "Tool execution completed successfully.");
    }

    if let Some(tool_name) = mode.strip_prefix("tool_chain_") {
        let args = get_tool_args();
        return tool_call_response(id, created, &req.model, tool_name, &args);
    }

    if mode == "multi_tool" && req.tools.len() >= 2 {
        let tool_calls: Vec<ToolCallResponse> = req.tools.iter().take(2).map(|t| ToolCallResponse {
            id: make_tool_call_id(),
            r#type: "function".into(),
            function: FunctionResponse { name: t.function.name.clone(), arguments: "{}".into() },
        }).collect();
        return ChatResponse {
            id: id.into(),
            object: "chat.completion".into(),
            created,
            model: req.model.clone(),
            choices: vec![Choice {
                index: 0,
                message: ResponseMessage {
                    role: "assistant".into(),
                    content: None,
                    tool_calls: Some(tool_calls),
                },
                finish_reason: "tool_calls".into(),
            }],
            usage: Usage { prompt_tokens: 50, completion_tokens: 30, total_tokens: 80 },
        };
    }

    generate_default_response(req, id, created, &req.model)
}

fn generate_default_response(req: &ChatRequest, id: &str, created: i64, model: &str) -> ChatResponse {
    if has_tool_results(&req.messages) {
        return text_response(id, created, model, "Tool execution completed successfully.");
    }
    if !req.tools.is_empty() {
        return tool_call_response(id, created, model, &req.tools[0].function.name, &serde_json::json!({}));
    }
    text_response(id, created, model, "Hello! I'm a test AI.")
}

// ---------------------------------------------------------------------------
// SSE streaming helpers
// ---------------------------------------------------------------------------

fn chat_response_to_sse(resp: &ChatResponse) -> Vec<Event> {
    let mut events = Vec::new();

    // Send role first
    let role_data = serde_json::json!({
        "id": resp.id,
        "object": "chat.completion.chunk",
        "created": resp.created,
        "model": resp.model,
        "choices": [{
            "index": 0,
            "delta": {"role": "assistant"},
            "finish_reason": null
        }]
    });
    events.push(Event::default().data(role_data.to_string()));

    // Stream content character by character with 10ms delay
    if let Some(content) = &resp.choices[0].message.content {
        for ch in content.chars() {
            let chunk_data = serde_json::json!({
                "id": resp.id,
                "object": "chat.completion.chunk",
                "created": resp.created,
                "model": resp.model,
                "choices": [{
                    "index": 0,
                    "delta": {"content": ch.to_string()},
                    "finish_reason": null
                }]
            });
            events.push(Event::default().data(chunk_data.to_string()));
        }
    }

    // Stream tool calls if present
    if let Some(tool_calls) = &resp.choices[0].message.tool_calls {
        let tc_data = serde_json::json!({
            "id": resp.id,
            "object": "chat.completion.chunk",
            "created": resp.created,
            "model": resp.model,
            "choices": [{
                "index": 0,
                "delta": {"tool_calls": tool_calls},
                "finish_reason": null
            }]
        });
        events.push(Event::default().data(tc_data.to_string()));
    }

    // Finish
    let reason = &resp.choices[0].finish_reason;
    let finish_data = serde_json::json!({
        "id": resp.id,
        "object": "chat.completion.chunk",
        "created": resp.created,
        "model": resp.model,
        "choices": [{
            "index": 0,
            "delta": {},
            "finish_reason": reason
        }]
    });
    events.push(Event::default().data(finish_data.to_string()));

    // [DONE]
    events.push(Event::default().data("[DONE]"));

    events
}

// ---------------------------------------------------------------------------
// AppState
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct AppState {
    start_time: i64,
    request_count: Arc<AtomicU64>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn chat_completions(
    AxumState(state): AxumState<Arc<AppState>>,
    body: axum::body::Bytes,
) -> axum::response::Response {
    let req: ChatRequest = match serde_json::from_slice(&body) {
        Ok(r) => {
            log_request_to_file(&r.model, &body);
            r
        }
        Err(e) => {
            tracing::error!("Failed to parse request: {e}");
            let err = ErrorResponse {
                error: ErrorDetail {
                    message: format!("Invalid JSON: {e}"),
                    r#type: "invalid_request_error".into(),
                    code: Some("invalid_json".into()),
                },
            };
            return (StatusCode::BAD_REQUEST, Json(err)).into_response();
        }
    };

    let _round = state.request_count.fetch_add(1, Ordering::SeqCst);
    let response_id = make_id();
    let created = now_ts();
    let model_id = req.model.clone();

    // Check env error mode
    if get_response_mode() == "error" {
        let err = ErrorResponse {
            error: ErrorDetail {
                message: "Simulated API error for testing".into(),
                r#type: "server_error".into(),
                code: Some("internal_error".into()),
            },
        };
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(err)).into_response();
    }

    // Apply model-specific delay
    let delay_secs = find_model(&model_id).map(|m| m.delay_secs).unwrap_or(0);
    if delay_secs > 0 {
        tracing::info!("Model {} sleeping {}s", model_id, delay_secs);
        tokio::time::sleep(Duration::from_secs(delay_secs)).await;
    }

    // Generate response
    let resp = generate_model_response(&model_id, &req, &response_id, created);

    // Streaming or non-streaming
    if req.stream {
        let events = chat_response_to_sse(&resp);
        let stream = stream::iter(events).map(move |evt| {
            let fut = async move {
                tokio::time::sleep(Duration::from_millis(10)).await;
                Ok::<_, Infallible>(evt)
            };
            fut
        });
        let sse = Sse::new(stream).keep_alive(KeepAlive::default());
        sse.into_response()
    } else {
        Json(resp).into_response()
    }
}

async fn list_models(AxumState(state): AxumState<Arc<AppState>>) -> impl IntoResponse {
    let data: Vec<ModelEntry> = MODELS
        .iter()
        .map(|m| ModelEntry {
            id: m.id.to_string(),
            object: "model".into(),
            created: state.start_time,
            owned_by: "test".into(),
        })
        .collect();
    Json(ModelsResponse { object: "list".into(), data })
}

async fn help_endpoint() -> impl IntoResponse {
    let mut lines = vec![
        "=== TestAIServer Help ===".to_string(),
        String::new(),
        "Models:".to_string(),
    ];
    for m in MODELS {
        lines.push(format!("  {} - {}", m.id, m.description));
    }
    lines.push(String::new());
    lines.push("Categories: basic, timeout, cluster, tools, security".to_string());
    lines.push(String::new());
    lines.push("Environment variables:".to_string());
    lines.push("  RESPONSE_MODE — override model behavior (default/error/multi_tool/tool_chain_<name>)".to_string());
    lines.push("  TOOL_ARGS     — JSON tool arguments for tool_chain mode".to_string());
    let text = lines.join("\n");
    Json(serde_json::json!({ "help": text }))
}

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let state = Arc::new(AppState {
        start_time: now_ts(),
        request_count: Arc::new(AtomicU64::new(0)),
    });

    let app = axum::Router::new()
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/models", get(list_models))
        .route("/v1/help", get(help_endpoint))
        .route("/health", get(health))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], args.port));
    tracing::info!("AI mock server listening on {addr}");
    tracing::info!("Models: {}", MODELS.iter().map(|m| m.id).collect::<Vec<_>>().join(", "));
    tracing::info!("Response mode: {}", get_response_mode());

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("Failed to bind to {addr}: {e}");
            std::process::exit(1);
        }
    };

    if let Err(e) = axum::serve(listener, app).await {
        tracing::error!("Server error: {e}");
        std::process::exit(1);
    }
}
