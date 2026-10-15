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
                finish_reason: None,
                escalated_to: None,
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
        finish_reason: None,
        escalated_to: None,
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
        finish_reason: None,
        escalated_to: None,
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
        ..Default::default()
    }];
    assert_eq!(assemble_task(&msgs), "你好，帮我看看这段日志");
}

#[test]
fn system_history_and_request_are_sectioned() {
    let msgs = vec![
        OpenAiMessage {
            role: "system".to_string(),
            content: Some(serde_json::json!("输出只用英文")),
            ..Default::default()
        },
        OpenAiMessage {
            role: "user".to_string(),
            content: Some(serde_json::json!("第一问")),
            ..Default::default()
        },
        OpenAiMessage {
            role: "assistant".to_string(),
            content: Some(serde_json::json!("第一答")),
            ..Default::default()
        },
        OpenAiMessage {
            role: "user".to_string(),
            content: Some(serde_json::json!("第二问")),
            ..Default::default()
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
        ..Default::default()
    }];
    // 单条消息 + parts 拼接文本 → 透传路径拿到的就是拼接结果。
    assert_eq!(assemble_task(&msgs), "第一段\n第二段");
}

#[test]
fn system_only_falls_back_to_instructions() {
    let msgs = vec![OpenAiMessage {
        role: "system".to_string(),
        content: Some(serde_json::json!("只回答数字")),
        ..Default::default()
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

            tool_fence_profession: None,
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

// ---------------------------------------------------------------------------
// 裸转发 lane（BUG-③A 2026-10-15）：带 tools 的请求 → provider 直转
// ---------------------------------------------------------------------------

use nemesis_providers::failover::FailoverError as SlotError;
use nemesis_providers::http_provider::StreamChunk;
use nemesis_providers::router::LLMProvider as SlotProvider;
use nemesis_providers::types::{
    ChatOptions as SlotChatOptions, LLMResponse as SlotLlmResponse, Message as SlotMessage,
    MessageContent as SlotMessageContent, ToolCall as SlotToolCall,
    ToolDefinition as SlotToolDefinition, UsageInfo,
};

/// 记录请求 + 脚本响应的槽 provider stub（裸 lane 断言「到了 provider、
/// 没进 agent loop」的观测点）。
struct BareSlotProvider {
    seen: Mutex<Vec<BareSeen>>,
    reply: Option<SlotLlmResponse>,
    error: Option<SlotError>,
    stream_chunks: Vec<Result<StreamChunk, SlotError>>,
}

#[derive(Clone)]
struct BareSeen {
    model: String,
    messages: Vec<SlotMessage>,
    tools: usize,
    options: SlotChatOptions,
}

impl BareSlotProvider {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            seen: Mutex::new(Vec::new()),
            reply: None,
            error: None,
            stream_chunks: Vec::new(),
        })
    }

    fn seen(&self) -> Vec<BareSeen> {
        self.seen.lock().unwrap().clone()
    }
}

/// builder：`Arc<BareSlotProvider>` → 定稿（脚本位填好后回 Arc）。
fn slot_with_reply(p: Arc<BareSlotProvider>, reply: SlotLlmResponse) -> Arc<BareSlotProvider> {
    let mut inner = Arc::try_unwrap(p).ok().expect("builder needs sole owner");
    inner.reply = Some(reply);
    Arc::new(inner)
}

fn slot_with_error(p: Arc<BareSlotProvider>, error: SlotError) -> Arc<BareSlotProvider> {
    let mut inner = Arc::try_unwrap(p).ok().expect("builder needs sole owner");
    inner.error = Some(error);
    Arc::new(inner)
}

fn slot_with_stream(
    p: Arc<BareSlotProvider>,
    chunks: Vec<Result<StreamChunk, SlotError>>,
) -> Arc<BareSlotProvider> {
    let mut inner = Arc::try_unwrap(p).ok().expect("builder needs sole owner");
    inner.stream_chunks = chunks;
    Arc::new(inner)
}

fn slot_reply(content: &str, tool_calls: Vec<SlotToolCall>) -> SlotLlmResponse {
    let finish_reason = if tool_calls.is_empty() {
        "stop"
    } else {
        "tool_calls"
    };
    SlotLlmResponse {
        content: content.to_string(),
        tool_calls,
        finish_reason: finish_reason.to_string(),
        usage: Some(UsageInfo {
            prompt_tokens: 11,
            completion_tokens: 7,
            total_tokens: 18,
            ..Default::default()
        }),
        reasoning_content: None,
        extra: Default::default(),
        raw_request_body: None,
        raw_response_body: None,
    }
}

fn slot_tool_call(id: &str, name: &str, args: &str) -> SlotToolCall {
    SlotToolCall {
        id: id.to_string(),
        call_type: Some("function".to_string()),
        function: Some(nemesis_providers::types::FunctionCall {
            name: name.to_string(),
            arguments: args.to_string(),
        }),
        name: None,
        arguments: None,
    }
}

#[async_trait::async_trait]
impl SlotProvider for BareSlotProvider {
    async fn chat(
        &self,
        messages: &[SlotMessage],
        tools: &[SlotToolDefinition],
        model: &str,
        options: &SlotChatOptions,
    ) -> Result<SlotLlmResponse, SlotError> {
        self.seen.lock().unwrap().push(BareSeen {
            model: model.to_string(),
            messages: messages.to_vec(),
            tools: tools.len(),
            options: options.clone(),
        });
        if let Some(e) = &self.error {
            return Err(e.clone());
        }
        self.reply.clone().ok_or_else(|| SlotError::Format {
            provider: "bare-test".to_string(),
            message: "no scripted reply".to_string(),
        })
    }

    fn chat_stream(
        &self,
        messages: &[SlotMessage],
        tools: &[SlotToolDefinition],
        model: &str,
        options: &SlotChatOptions,
    ) -> tokio::sync::mpsc::Receiver<Result<StreamChunk, SlotError>> {
        self.seen.lock().unwrap().push(BareSeen {
            model: model.to_string(),
            messages: messages.to_vec(),
            tools: tools.len(),
            options: options.clone(),
        });
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        let chunks = self.stream_chunks.clone();
        tokio::spawn(async move {
            for c in chunks {
                if tx.send(c).await.is_err() {
                    break;
                }
            }
        });
        rx
    }

    fn name(&self) -> &str {
        "bare-test"
    }

    fn default_model(&self) -> &str {
        "test-model"
    }
}

/// make_state 的 AppState 是无人共享的新 Arc——就地把 streaming_provider
/// 注入（harness 保持单构造点）。返回同型 Arc<BareSlotProvider> 观测句柄
/// （与注入的 trait object 同一实例）。
fn with_streaming(
    state: Arc<AppState>,
    provider: Arc<BareSlotProvider>,
) -> (Arc<AppState>, Arc<BareSlotProvider>) {
    let mut inner = match Arc::try_unwrap(state) {
        Ok(v) => v,
        Err(_) => panic!("test state must have no other owners"),
    };
    inner.streaming_provider = Some(provider.clone());
    (Arc::new(inner), provider)
}

fn bare_request(stream: bool) -> serde_json::Value {
    serde_json::json!({
        "model": "worker-alias",
        "stream": stream,
        "messages": [{ "role": "user", "content": "编译这个项目" }],
        "tools": [{
            "type": "function",
            "function": {
                "name": "exec",
                "description": "run a shell command",
                "parameters": { "type": "object", "properties": { "cmd": { "type": "string" } } }
            }
        }]
    })
}

async fn resp_parts(resp: Response) -> (u16, http::HeaderMap, serde_json::Value) {
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let bytes = http_body_util::BodyExt::collect(resp.into_body())
        .await
        .unwrap()
        .to_bytes();
    (status, headers, serde_json::from_slice(&bytes).unwrap())
}

async fn resp_text(resp: Response) -> (u16, http::HeaderMap, String) {
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let bytes = http_body_util::BodyExt::collect(resp.into_body())
        .await
        .unwrap()
        .to_bytes();
    (status, headers, String::from_utf8(bytes.to_vec()).unwrap())
}

#[tokio::test]
async fn bare_tools_request_routes_to_provider_not_agent_loop() {
    // agent loop 在场且脚本会回复「AGENT_MUST_NOT_RUN」——若裸 lane 误入
    // 分离执行路径，回复内容断言会当场红。
    let state = script_state(vec![done_reply("AGENT_MUST_NOT_RUN")]);
    let provider = slot_with_reply(
        BareSlotProvider::new(),
        slot_reply(
            "UPSTREAM_REPLY",
            vec![slot_tool_call(
                "call_1",
                "exec",
                "{\"cmd\":\"cargo build\"}",
            )],
        ),
    );
    let (state, bare) = with_streaming(state, provider);

    let resp = api_chat_completions(&state, &body(&bare_request(false))).await;
    let (status, _headers, v) = resp_parts(resp).await;
    assert_eq!(status, 200);
    assert_eq!(v["choices"][0]["message"]["content"], "UPSTREAM_REPLY");
    assert_eq!(v["choices"][0]["finish_reason"], "tool_calls");
    assert_eq!(v["choices"][0]["message"]["tool_calls"][0]["id"], "call_1");
    assert_eq!(
        v["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
        "{\"cmd\":\"cargo build\"}"
    );
    assert_eq!(v["model"], "worker-alias", "请求 model 回显");
    assert_eq!(v["nemesis"]["lane"], "bare-forward");
    assert_eq!(v["nemesis"]["execution_model"], "test-model");
    assert_eq!(v["usage"]["total_tokens"], 18, "usage 上游有就透传");

    // 观测点：请求确实到了 provider stub（model=活跃模型、tools 原样）。
    let seen = bare.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].model, "test-model");
    assert_eq!(seen[0].tools, 1);
    assert_eq!(seen[0].messages.len(), 1);
}

#[tokio::test]
async fn bare_message_tool_fields_round_trip() {
    let state = script_state(vec![]);
    let provider = slot_with_reply(BareSlotProvider::new(), slot_reply("ok", vec![]));
    let (state, bare) = with_streaming(state, provider);

    let mut req = bare_request(false);
    req["messages"] = serde_json::json!([
        { "role": "user", "content": "编译这个项目" },
        {
            "role": "assistant",
            "content": null,
            "tool_calls": [{
                "id": "tc_9",
                "type": "function",
                "function": { "name": "exec", "arguments": "{\"cmd\":\"ls\"}" }
            }]
        },
        { "role": "tool", "tool_call_id": "tc_9", "content": "compiled ok" }
    ]);
    let resp = api_chat_completions(&state, &body(&req)).await;
    assert_eq!(resp.status(), 200);

    let seen = bare.seen();
    assert_eq!(seen.len(), 1);
    let msgs = &seen[0].messages;
    assert_eq!(msgs.len(), 3);
    assert_eq!(msgs[0].role, "user");
    // assistant：content null → 空文本（provider 层序列化为空串，非丢字段）。
    assert_eq!(msgs[1].role, "assistant");
    assert_eq!(msgs[1].content, SlotMessageContent::Text(String::new()));
    assert_eq!(msgs[1].tool_calls.len(), 1);
    assert_eq!(msgs[1].tool_calls[0].id, "tc_9");
    assert_eq!(
        msgs[1].tool_calls[0].function.as_ref().unwrap().name,
        "exec"
    );
    assert_eq!(
        msgs[1].tool_calls[0].function.as_ref().unwrap().arguments,
        "{\"cmd\":\"ls\"}"
    );
    // tool 结果消息：tool_call_id 保真（丢了这个上游无法对上调用）。
    assert_eq!(msgs[2].role, "tool");
    assert_eq!(msgs[2].tool_call_id.as_deref(), Some("tc_9"));
    assert_eq!(
        msgs[2].content,
        SlotMessageContent::Text("compiled ok".into())
    );
}

#[tokio::test]
async fn bare_missing_optional_tool_fields_coerced() {
    let state = script_state(vec![]);
    let provider = slot_with_reply(BareSlotProvider::new(), slot_reply("ok", vec![]));
    let (state, bare) = with_streaming(state, provider);

    // OpenAI 规范允许省略 description / parameters——lenient 归一后放行。
    let mut req = bare_request(false);
    req["tools"] = serde_json::json!([
        { "type": "function", "function": { "name": "no_args_tool" } }
    ]);
    let resp = api_chat_completions(&state, &body(&req)).await;
    assert_eq!(resp.status(), 200, "缺省可省字段必须放行");
    let seen = bare.seen();
    assert_eq!(seen[0].tools, 1);
}

#[tokio::test]
async fn bare_non_auto_tool_choice_is_400() {
    let state = script_state(vec![]);
    let provider = slot_with_reply(BareSlotProvider::new(), slot_reply("ok", vec![]));
    let (state, bare) = with_streaming(state, provider);

    // 字符串形态（非 auto）。
    let mut req = bare_request(false);
    req["tool_choice"] = serde_json::json!("required");
    let (status, _, v) = resp_parts(api_chat_completions(&state, &body(&req)).await).await;
    assert_eq!(status, 400);
    assert_eq!(v["error"]["type"], "invalid_request_error");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("tool_choice")
    );

    // 对象形态（具名指定）。
    let mut req = bare_request(false);
    req["tool_choice"] = serde_json::json!({"type": "function", "function": {"name": "exec"}});
    let (status, _, _) = resp_parts(api_chat_completions(&state, &body(&req)).await).await;
    assert_eq!(status, 400);

    // "auto" 显式放行。
    let mut req = bare_request(false);
    req["tool_choice"] = serde_json::json!("auto");
    let resp = api_chat_completions(&state, &body(&req)).await;
    assert_eq!(resp.status(), 200);
    assert_eq!(bare.seen().len(), 1);
}

#[tokio::test]
async fn bare_upstream_error_maps_to_honest_status() {
    // Timeout → 504。
    let state = script_state(vec![]);
    let provider = slot_with_error(
        BareSlotProvider::new(),
        SlotError::Timeout {
            provider: "cc".into(),
            model: "gpt-5.6-sol".into(),
        },
    );
    let (state, _) = with_streaming(state, provider);
    let (status, _, v) =
        resp_parts(api_chat_completions(&state, &body(&bare_request(false))).await).await;
    assert_eq!(status, 504);
    assert_eq!(v["error"]["type"], "timeout");

    // RateLimit → 429。
    let state = script_state(vec![]);
    let provider = slot_with_error(
        BareSlotProvider::new(),
        SlotError::RateLimit {
            provider: "cc".into(),
            model: "gpt-5.6-sol".into(),
            retry_after: Some(30),
        },
    );
    let (state, _) = with_streaming(state, provider);
    let (status, _, v) =
        resp_parts(api_chat_completions(&state, &body(&bare_request(false))).await).await;
    assert_eq!(status, 429);
    assert_eq!(v["error"]["type"], "rate_limit_error");

    // Auth → 502（不是 401——那是 master 侧上游凭据问题，401 会误导
    // worker 以为自己的 master 鉴权断了）。
    let state = script_state(vec![]);
    let provider = slot_with_error(
        BareSlotProvider::new(),
        SlotError::Auth {
            provider: "cc".into(),
            model: "gpt-5.6-sol".into(),
            status: 401,
        },
    );
    let (state, _) = with_streaming(state, provider);
    let (status, _, v) =
        resp_parts(api_chat_completions(&state, &body(&bare_request(false))).await).await;
    assert_eq!(status, 502);
    assert_eq!(v["error"]["type"], "upstream_auth_error");
}

#[tokio::test]
async fn bare_stream_emits_openai_frames() {
    let state = script_state(vec![]);
    let provider = slot_with_stream(
        BareSlotProvider::new(),
        vec![
            Ok(StreamChunk {
                delta: "HE".to_string(),
                tool_calls: Vec::new(),
                finish_reason: None,
                usage: None,
                reasoning_content: None,
            }),
            Ok(StreamChunk {
                delta: "LLO".to_string(),
                tool_calls: vec![slot_tool_call("call_s", "exec", "{}")],
                finish_reason: Some("tool_calls".to_string()),
                usage: Some(UsageInfo {
                    prompt_tokens: 3,
                    completion_tokens: 2,
                    total_tokens: 5,
                    ..Default::default()
                }),
                reasoning_content: None,
            }),
        ],
    );
    let (state, _) = with_streaming(state, provider);

    let resp = api_chat_completions(&state, &body(&bare_request(true))).await;
    let (status, headers, text) = resp_text(resp).await;
    assert_eq!(status, 200);
    assert!(
        headers
            .get("content-type")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("")
            .starts_with("text/event-stream"),
        "流式响应必须是 SSE"
    );
    assert_eq!(
        headers
            .get("x-nemesis-execution-model")
            .and_then(|h| h.to_str().ok()),
        Some("test-model")
    );
    // delta 帧：content 分帧下发。
    assert!(text.contains("\"content\":\"HE\""), "{text}");
    assert!(text.contains("\"content\":\"LLO\""), "{text}");
    // tool_calls delta：完整形态（index/id/type/function.name/arguments）。
    assert!(text.contains("\"name\":\"exec\""), "{text}");
    assert!(text.contains("\"id\":\"call_s\""), "{text}");
    // 终帧：finish_reason + usage。
    assert!(text.contains("\"finish_reason\":\"tool_calls\""), "{text}");
    assert!(text.contains("\"total_tokens\":5"), "{text}");
    assert!(text.contains("data: [DONE]"), "{text}");
}

#[tokio::test]
async fn bare_stream_first_chunk_error_is_json_not_sse() {
    let state = script_state(vec![]);
    let provider = slot_with_stream(
        BareSlotProvider::new(),
        vec![Err(SlotError::Timeout {
            provider: "cc".into(),
            model: "gpt-5.6-sol".into(),
        })],
    );
    let (state, _) = with_streaming(state, provider);

    // 起流失败不 commit SSE——错误面保持 JSON 形态（resp_parts 的 JSON
    // 解析即判别器：半个 SSE 流在这里直接 unwrap 崩）。
    let (status, _, v) =
        resp_parts(api_chat_completions(&state, &body(&bare_request(true))).await).await;
    assert_eq!(status, 504);
    assert_eq!(v["error"]["type"], "timeout");
}

#[tokio::test]
async fn bare_stream_midstream_error_gets_inline_note_and_stop() {
    let state = script_state(vec![]);
    let provider = slot_with_stream(
        BareSlotProvider::new(),
        vec![
            Ok(StreamChunk {
                delta: "PART".to_string(),
                tool_calls: Vec::new(),
                finish_reason: None,
                usage: None,
                reasoning_content: None,
            }),
            Err(SlotError::Unknown {
                provider: "cc".into(),
                message: "connect error: connection reset".into(),
            }),
        ],
    );
    let (state, _) = with_streaming(state, provider);

    // 中途断流：带内注记（worker 可见）+ 合成 stop 终态（worker 侧 J1
    // 否则把无终态断流当完整 stop 静默截断）。
    let (status, _, text) =
        resp_text(api_chat_completions(&state, &body(&bare_request(true))).await).await;
    assert_eq!(status, 200);
    assert!(text.contains("PART"), "{text}");
    assert!(
        text.contains("nemesis bare-forward upstream error"),
        "断流必须带内可见：{text}"
    );
    assert!(text.contains("\"finish_reason\":\"stop\""), "{text}");
    assert!(text.contains("data: [DONE]"), "{text}");
}

#[tokio::test]
async fn bare_no_streaming_provider_is_503() {
    let state = script_state(vec![done_reply("AGENT_MUST_NOT_RUN")]);
    let (status, _, v) =
        resp_parts(api_chat_completions(&state, &body(&bare_request(false))).await).await;
    assert_eq!(status, 503);
    assert_eq!(v["error"]["type"], "server_error");
}

#[tokio::test]
async fn bare_estop_still_gates_bare_lane() {
    let estop = Arc::new(nemesis_agent::estop::EstopState::new());
    estop.trigger();
    let state = make_state(None, Some(estop));
    let provider = slot_with_reply(BareSlotProvider::new(), slot_reply("ok", vec![]));
    let (state, bare) = with_streaming(state, provider);

    // estop 闸在裸 lane 分流**之前**——worker 的 LLM 消耗也是 agent 活动。
    let (status, _, _) =
        resp_parts(api_chat_completions(&state, &body(&bare_request(false))).await).await;
    assert_eq!(status, 503);
    assert!(bare.seen().is_empty(), "estop 下 provider 不得被调用");
}

// ---------------------------------------------------------------------------
// 裸 lane 模型卡语义注入（纯函数 + options 装配）
// ---------------------------------------------------------------------------

#[test]
fn model_card_semantics_reads_effort_and_max_output() {
    let cfg = serde_json::json!({
        "model_list": [{
            "model_name": "gpt-5.6-sol",
            "reasoning_effort": "high",
            "max_output_tokens": 32768
        }]
    });
    let (effort, max_out) = super::model_card_semantics(&cfg, "gpt-5.6-sol");
    assert_eq!(effort.as_deref(), Some("high"));
    assert_eq!(max_out, Some(32768));

    // 无该模型条目 → 全 None（诚实跳过，不造语义）。
    let (effort, max_out) = super::model_card_semantics(&cfg, "other-model");
    assert_eq!(effort, None);
    assert_eq!(max_out, None);
}

#[tokio::test]
async fn bare_chat_options_merge_caller_params_and_master_card() {
    let dir = std::env::temp_dir().join(format!(
        "nemesis-web-baretest-{}-card-semantics",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("config.json"),
        serde_json::json!({
            "model_list": [{
                "model_name": "test-model",
                "reasoning_effort": "high",
                "max_output_tokens": 32768
            }]
        })
        .to_string(),
    )
    .unwrap();

    let state = script_state(vec![]);
    let provider = slot_with_reply(BareSlotProvider::new(), slot_reply("ok", vec![]));
    let (state, bare) = with_streaming(state, provider);
    // 注入 home（harness 字面量保持单构造点，就地改）。
    let mut inner = Arc::try_unwrap(state).ok().expect("single owner");
    inner.home = Some(dir.to_string_lossy().to_string());
    let state = Arc::new(inner);

    // 请求带 temperature、不带 max_tokens → 采样透传 + 卡片 effort/max
    // 注入（「统一走 master 提供的 LLM」= 档位由 master 卡片说了算）。
    let mut req = bare_request(false);
    req["temperature"] = serde_json::json!(0.5);
    let resp = api_chat_completions(&state, &body(&req)).await;
    assert_eq!(resp.status(), 200);
    let seen = bare.seen();
    assert_eq!(seen[0].options.temperature, Some(0.5));
    assert_eq!(seen[0].options.reasoning_effort.as_deref(), Some("high"));
    assert_eq!(seen[0].options.max_tokens, Some(32768));

    // 请求显式 max_tokens → 请求值赢（不覆盖调用方意志）。
    let mut req = bare_request(false);
    req["max_tokens"] = serde_json::json!(1024);
    let resp = api_chat_completions(&state, &body(&req)).await;
    assert_eq!(resp.status(), 200);
    let seen = bare.seen();
    assert_eq!(seen[1].options.max_tokens, Some(1024));

    let _ = std::fs::remove_dir_all(&dir);
}
