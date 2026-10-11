//! S5（2026-10-09）：入站 OpenAI 兼容端点测试。
//!
//! 两层：
//! 1. `assemble_task` 纯函数——messages → 分离执行任务的映射规则
//!    （单消息逐字节透传 / system+历史折段 / parts 数组拼接 / 仅 system 回退）；
//! 2. `api_chat_completions` 端点级——真实 AgentLoop（mock provider）驱动，
//!    断言错误面 OpenAI 形态、非流式响应形状、工具计数、SSE 帧形态、
//!    estop / 无 loop 的诚实 503。
//!
//! 鉴权不在本文件：`/v1/chat/completions` 走统一 auth 中间件（不进豁免
//! 清单），真值表断言在 `server::auth_tests`。

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use nemesis_agent::context::RequestContext;
use nemesis_agent::r#loop::{AgentLoop, LlmMessage, LlmProvider, LlmResponse};
use nemesis_agent::types::{AgentConfig, ToolCallInfo, ToolDefinition};
use parking_lot::Mutex as PlMutex;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

// ---------------------------------------------------------------------------
// Harness（AppState 字面量——l1_tests 同款双臂；agent_loop/estop 由用例注入）
// ---------------------------------------------------------------------------

fn make_state(
    agent_loop: Option<Arc<AgentLoop>>,
    estop: Option<Arc<nemesis_agent::estop::EstopState>>,
) -> Arc<AppState> {
    Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: None,
        home: None,
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(PlMutex::new("test-model".to_string())),
        model_base: Arc::new(PlMutex::new(String::new())),
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
        agent_loop: Arc::new(parking_lot::RwLock::new(agent_loop)),
        cluster: None,
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: None,
        #[cfg(feature = "workflow")]
        chat_secret_store: std::sync::Arc::new(
            nemesis_workflow::chat_secrets::ChatSecretStore::in_memory(),
        ),
        #[cfg(not(feature = "workflow"))]
        chat_secret_store: std::sync::Arc::new(()),
        #[cfg(feature = "workflow")]
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        #[cfg(not(feature = "workflow"))]
        webhook_rate_limiter: Arc::new(()),
        internal_cmd_tx: None,
        estop,
        signature_verify: None,
        skills_install_gate: None,
        cron: None,
        board: None,
    })
}

fn script_state(responses: Vec<LlmResponse>) -> Arc<AppState> {
    make_state(
        Some(Arc::new(AgentLoop::new(
            Box::new(ScriptProvider {
                responses: Mutex::new(responses),
            }),
            AgentConfig {
                model: "test-model".to_string(),
                system_prompt: Some("You are a test assistant.".to_string()),
                max_turns: 5,
                ..Default::default()
            },
        ))),
        None,
    )
}

fn body(v: &serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(v).unwrap()
}

async fn resp_json(resp: Response) -> (u16, serde_json::Value) {
    let status = resp.status().as_u16();
    let bytes = http_body_util::BodyExt::collect(resp.into_body())
        .await
        .unwrap()
        .to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

// ---------------------------------------------------------------------------
// Mock provider（credential_injection_tests 同款脚本形态）
// ---------------------------------------------------------------------------

struct ScriptProvider {
    responses: Mutex<Vec<LlmResponse>>,
}

#[async_trait::async_trait]
impl LlmProvider for ScriptProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<nemesis_agent::types::ChatOptions>,
        _tools: Vec<ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        let mut responses = self.responses.lock().unwrap();
        if responses.is_empty() {
            Ok(LlmResponse {
                content: "No more responses".to_string(),
                tool_calls: Vec::new(),
                finished: true,
                reasoning_content: None,
                usage: None,
                raw_request_body: None,
                raw_response_body: None,
            })
        } else {
            Ok(responses.remove(0))
        }
    }
}

struct ErrorProvider;
#[async_trait::async_trait]
impl LlmProvider for ErrorProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<nemesis_agent::types::ChatOptions>,
        _tools: Vec<ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Err("boom: simulated upstream failure".to_string())
    }
}

/// 捕获 execute 的哑工具（工具计数用例）。
struct EchoTool;
#[async_trait::async_trait]
impl nemesis_agent::r#loop::Tool for EchoTool {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok("echoed".to_string())
    }
}

fn done_reply(text: &str) -> LlmResponse {
    LlmResponse {
        content: text.to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }
}

fn tool_call_reply(name: &str, args: &str) -> LlmResponse {
    LlmResponse {
        content: String::new(),
        tool_calls: vec![ToolCallInfo {
            id: "tc_1".to_string(),
            name: name.to_string(),
            arguments: args.to_string(),
        }],
        finished: false,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }
}

// ---------------------------------------------------------------------------
// assemble_task 纯函数
// ---------------------------------------------------------------------------

#[test]
fn single_message_passes_through_verbatim() {
    let msgs = vec![OpenAiMessage {
        role: "user".to_string(),
        content: Some(serde_json::json!("你好，帮我看看这段日志")),
    }];
    assert_eq!(assemble_task(&msgs), "你好，帮我看看这段日志");
}

#[test]
fn system_history_and_request_are_sectioned() {
    let msgs = vec![
        OpenAiMessage {
            role: "system".to_string(),
            content: Some(serde_json::json!("输出只用英文")),
        },
        OpenAiMessage {
            role: "user".to_string(),
            content: Some(serde_json::json!("第一问")),
        },
        OpenAiMessage {
            role: "assistant".to_string(),
            content: Some(serde_json::json!("第一答")),
        },
        OpenAiMessage {
            role: "user".to_string(),
            content: Some(serde_json::json!("第二问")),
        },
    ];
    let out = assemble_task(&msgs);
    assert!(out.contains("[调用方补充的 system 指令]"), "{out}");
    assert!(out.contains("输出只用英文"), "{out}");
    assert!(out.contains("[此前对话记录]"), "{out}");
    assert!(out.contains("user: 第一问"), "{out}");
    assert!(out.contains("assistant: 第一答"), "{out}");
    assert!(out.contains("[当前请求（user）]"), "{out}");
    assert!(out.contains("第二问"), "{out}");
    // 干净透传不成立（有多段），当前请求必须是最后一段。
    assert!(out.ends_with("第二问"), "{out}");
}

#[test]
fn parts_array_content_joins_text_parts() {
    let msgs = vec![OpenAiMessage {
        role: "user".to_string(),
        content: Some(serde_json::json!([
            {"type": "text", "text": "第一段"},
            {"type": "image_url", "image_url": {"url": "data:image/png;base64,xxx"}},
            {"type": "text", "text": "第二段"}
        ])),
    }];
    // 单条消息 + parts 拼接文本 → 透传路径拿到的就是拼接结果。
    assert_eq!(assemble_task(&msgs), "第一段\n第二段");
}

#[test]
fn system_only_falls_back_to_instructions() {
    let msgs = vec![OpenAiMessage {
        role: "system".to_string(),
        content: Some(serde_json::json!("只回答数字")),
    }];
    assert_eq!(assemble_task(&msgs), "只回答数字");
}

// ---------------------------------------------------------------------------
// 端点级：错误面
// ---------------------------------------------------------------------------

#[tokio::test]
async fn malformed_body_is_openai_shaped_400() {
    let state = script_state(vec![]);
    let resp = api_chat_completions(&state, b"not-json").await;
    let (status, v) = resp_json(resp).await;
    assert_eq!(status, 400);
    assert_eq!(v["error"]["type"], "invalid_request_error");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("invalid request body"),
        "{}",
        v["error"]["message"]
    );
}

#[tokio::test]
async fn empty_messages_is_400() {
    let state = script_state(vec![]);
    let resp = api_chat_completions(&state, &body(&serde_json::json!({"messages": []}))).await;
    let (status, v) = resp_json(resp).await;
    assert_eq!(status, 400);
    assert_eq!(v["error"]["type"], "invalid_request_error");
}

#[tokio::test]
async fn missing_agent_loop_is_honest_503() {
    let state = make_state(None, None);
    let resp = api_chat_completions(
        &state,
        &body(&serde_json::json!({"messages": [{"role": "user", "content": "hi"}]})),
    )
    .await;
    let (status, v) = resp_json(resp).await;
    assert_eq!(status, 503);
    assert_eq!(v["error"]["type"], "server_error");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("agent loop not running"),
        "{}",
        v["error"]["message"]
    );
}

#[tokio::test]
async fn engaged_estop_is_503_before_loop_dispatch() {
    // estop 与 loop 同时在场：estop 先拦（不烧 token）。
    let estop = Arc::new(nemesis_agent::estop::EstopState::new());
    estop.trigger();
    let state = script_state(vec![done_reply("SHOULD_NOT_RUN")]);
    // 把同一个 loop 放进带 estop 的 state（script_state 里 estop 是 None，重造）。
    let loop_arc = state.agent_loop.read().clone();
    let state = make_state(loop_arc, Some(estop));
    let resp = api_chat_completions(
        &state,
        &body(&serde_json::json!({"messages": [{"role": "user", "content": "hi"}]})),
    )
    .await;
    let (status, v) = resp_json(resp).await;
    assert_eq!(status, 503);
    assert!(
        v["error"]["message"].as_str().unwrap().contains("急停"),
        "{}",
        v["error"]["message"]
    );
}

// ---------------------------------------------------------------------------
// 端点级：非流式成功路径
// ---------------------------------------------------------------------------

#[tokio::test]
async fn non_stream_completion_shape() {
    let state = script_state(vec![done_reply("HELLO_REPLY")]);
    let resp = api_chat_completions(
        &state,
        &body(&serde_json::json!({
            "model": "caller-requested-model",
            "messages": [{"role": "user", "content": "hi"}]
        })),
    )
    .await;
    let (status, v) = resp_json(resp).await;
    assert_eq!(status, 200);
    assert!(v["id"].as_str().unwrap().starts_with("chatcmpl-nemesis-"));
    assert_eq!(v["object"], "chat.completion");
    // model 回显请求值。
    assert_eq!(v["model"], "caller-requested-model");
    // 真实执行模型在扩展字段如实报告（不冒充请求的 model 名）。
    assert_eq!(v["nemesis"]["execution_model"], "test-model");
    let choice = &v["choices"][0];
    assert_eq!(choice["index"], 0);
    assert_eq!(choice["finish_reason"], "stop");
    assert_eq!(choice["message"]["role"], "assistant");
    assert_eq!(choice["message"]["content"], "HELLO_REPLY");
    assert_eq!(v["nemesis"]["tool_calls"], 0);
    // 无 token 计量 = 不造 usage 数。
    assert!(v.get("usage").is_none());
}

#[tokio::test]
async fn tool_calls_counted_in_extension_metadata() {
    let mut agent_loop = AgentLoop::new(
        Box::new(ScriptProvider {
            responses: Mutex::new(vec![
                tool_call_reply("echotool", r#"{"x":1}"#),
                done_reply("AFTER_TOOL"),
            ]),
        }),
        AgentConfig {
            model: "test-model".to_string(),
            system_prompt: None,
            max_turns: 5,
            tools: vec!["echotool".to_string()],
            models: Default::default(),
        },
    );
    agent_loop.register_tool("echotool".to_string(), Box::new(EchoTool));
    let state = make_state(Some(Arc::new(agent_loop)), None);
    let resp = api_chat_completions(
        &state,
        &body(&serde_json::json!({"messages": [{"role": "user", "content": "go"}]})),
    )
    .await;
    let (status, v) = resp_json(resp).await;
    assert_eq!(status, 200, "{}", v);
    assert_eq!(v["nemesis"]["tool_calls"], 1);
    assert_eq!(v["choices"][0]["message"]["content"], "AFTER_TOOL");
}

#[tokio::test]
async fn agent_error_maps_to_500() {
    let state = make_state(
        Some(Arc::new(AgentLoop::new(
            Box::new(ErrorProvider),
            AgentConfig::default(),
        ))),
        None,
    );
    let resp = api_chat_completions(
        &state,
        &body(&serde_json::json!({"messages": [{"role": "user", "content": "hi"}]})),
    )
    .await;
    let (status, v) = resp_json(resp).await;
    assert_eq!(status, 500);
    assert_eq!(v["error"]["type"], "server_error");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("agent error"),
        "{}",
        v["error"]["message"]
    );
}

// ---------------------------------------------------------------------------
// 端点级：stream:true（SSE 帧形态）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stream_frames_are_chunk_shaped_with_done() {
    let state = script_state(vec![done_reply("STREAM_REPLY")]);
    let resp = api_chat_completions(
        &state,
        &body(&serde_json::json!({
            "stream": true,
            "messages": [{"role": "user", "content": "hi"}]
        })),
    )
    .await;
    assert_eq!(resp.status(), 200);
    assert!(
        resp.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .starts_with("text/event-stream"),
        "content-type: {:?}",
        resp.headers().get("content-type")
    );
    // 扩展元数据挂头（SSE body 被协议占用）。
    assert_eq!(
        resp.headers()
            .get("x-nemesis-execution-model")
            .and_then(|v| v.to_str().ok()),
        Some("test-model")
    );
    let bytes = http_body_util::BodyExt::collect(resp.into_body())
        .await
        .unwrap()
        .to_bytes();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(
        text.contains("\"object\":\"chat.completion.chunk\""),
        "{text}"
    );
    assert!(text.contains("STREAM_REPLY"), "{text}");
    assert!(text.contains("\"finish_reason\":\"stop\""), "{text}");
    assert!(text.contains("data: [DONE]"), "{text}");
    // 请求未带 model → 回退活跃模型名。
    assert!(text.contains("test-model"), "{text}");
}
