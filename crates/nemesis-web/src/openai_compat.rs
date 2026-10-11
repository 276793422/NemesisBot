//! S5（2026-10-09 高优差距批次）：入站 OpenAI 兼容端点
//! `POST /v1/chat/completions`。
//!
//! 让持有 OpenAI SDK 生态的调用方把本 bot 当作一个 chat completions 后端：
//! `openai` Python/Node SDK、curl、以及大量只认这个 wire 形态的第三方工具
//! 改个 `base_url` 即可接入。鉴权走统一 REST 鉴权中间件（`auth_middleware`
//! 已接受 `Authorization: Bearer`，本路由**不进豁免清单**——触发 agent
//! 执行的端点与 dashboard 同一信任边界；默认部署空 token 恒放行）。
//!
//! ## 执行桥接（与 headless run / 子代理 spawn 同一条治理路径）
//!
//! 每个请求 = 一次**无状态**的分离执行：messages 数组自带全部上下文（
//! OpenAI 合同本就如此），服务端不保留会话。桥接点 =
//! `AgentLoop::run_detached_events`（临时 instance 跑完即弃，安全 8 层、
//! guardian、tier 过滤、args 校验、spill、turn_guard、estop 全部同源
//! 生效——`mcp_serve` 的 tool_run 同款先例）。**不是裸 LLM 代理**：
//! 与 `/api/chat/stream`（直连 provider）不同，本端点走完整 agent 能力
//! （工具、记忆、人格）。
//!
//! ## 映射（诚实边界）
//!
//! - `messages`：system 消息收进「调用方补充指令」段；此前 user/assistant
//!   轮折成对话记录前缀；最后一条消息 = 本次任务。单条 user 消息的干净
//!   情况下任务文本逐字节透传（不包壳）。
//! - `model`：接受并回显，**执行用的是 bot 自己配置的活跃模型**（真实
//!   执行模型在 `nemesis.execution_model` 扩展字段里如实报告）。
//! - `stream:true`：SSE chunk 形态兼容，但 Done 整段一次下发（与 ACP v1
//!   同一边界——分离执行拿不到 token 级增量；OpenAI 客户端只见一个
//!   大 chunk，语义正确）。
//! - usage：分离执行事件流无 token 计量，**不造数**——省略 usage 字段。
//! - `temperature`/`max_tokens` 等其余 OpenAI 字段：忽略（serde 默认）。
//!
//! 错误面一律 OpenAI 形态 `{"error":{"message","type","code"}}`。

use crate::api_handlers::AppState;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use nemesis_agent::r#loop::DetachedOpts;
use nemesis_agent::types::AgentEvent;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Instant;

/// 会话标签（进 session_key `subagent:openai-api:{uuid}` 与 trace_id，
/// 请求日志按此检索）。
const OPENAI_LABEL: &str = "openai-api";

// ---------------------------------------------------------------------------
// 请求/响应 wire 形态
// ---------------------------------------------------------------------------

/// `POST /v1/chat/completions` 请求体。未列出的 OpenAI 字段（temperature
/// 等）由 serde 默认忽略；`stream` 缺省 = false。
#[derive(Debug, serde::Deserialize)]
pub struct OpenAiChatRequest {
    /// 模型名。接受并回显，执行用 bot 活跃模型（模块头诚实边界）。
    #[serde(default)]
    pub model: Option<String>,
    pub messages: Vec<OpenAiMessage>,
    #[serde(default)]
    pub stream: bool,
}

/// OpenAI chat 消息。`content` 兼容三种形态：字符串（主流）、多模态
/// parts 数组（只取 text part 拼接）、null（如带 tool_calls 的 assistant
/// 消息——本端点不消费，取空串）。
#[derive(Debug, serde::Deserialize)]
pub struct OpenAiMessage {
    pub role: String,
    #[serde(default)]
    pub content: Option<serde_json::Value>,
}

impl OpenAiMessage {
    /// 提取纯文本：字符串原样；数组拼 text part；其它/缺失 = 空串。
    pub fn text(&self) -> String {
        match &self.content {
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(serde_json::Value::Array(parts)) => parts
                .iter()
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()).map(str::to_string))
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        }
    }
}

/// OpenAI 形态错误体（`{"error":{"message","type","code"}}`）。
fn error_response(status: axum::http::StatusCode, message: String, err_type: &str) -> Response {
    let body = serde_json::json!({
        "error": { "message": message, "type": err_type, "code": serde_json::Value::Null }
    });
    (status, axum::Json(body)).into_response()
}

// ---------------------------------------------------------------------------
// messages → 分离执行任务
// ---------------------------------------------------------------------------

/// 组装分离执行任务文本（模块头映射规则；纯函数供测试）。
pub(crate) fn assemble_task(messages: &[OpenAiMessage]) -> String {
    let system: Vec<String> = messages
        .iter()
        .filter(|m| m.role.eq_ignore_ascii_case("system"))
        .map(|m| m.text())
        .filter(|t| !t.is_empty())
        .collect();
    let dialogue: Vec<(String, String)> = messages
        .iter()
        .filter(|m| !m.role.eq_ignore_ascii_case("system"))
        .map(|m| (m.role.to_ascii_lowercase(), m.text()))
        .collect();
    let Some((last_role, last_text)) = dialogue.last() else {
        // 只有 system 消息（或全空）：退化为指令本身，诚实地交给 agent。
        return system.join("\n\n");
    };
    let prior = &dialogue[..dialogue.len() - 1];
    if system.is_empty() && prior.is_empty() {
        // 干净路径：单条消息逐字节透传（不包壳）。
        return last_text.clone();
    }
    let mut out = String::new();
    if !system.is_empty() {
        out.push_str("[调用方补充的 system 指令]\n");
        for s in &system {
            out.push_str(s);
            out.push_str("\n\n");
        }
    }
    if !prior.is_empty() {
        out.push_str("[此前对话记录]\n");
        for (role, text) in prior {
            out.push_str(role);
            out.push_str(": ");
            out.push_str(text);
            out.push('\n');
        }
        out.push('\n');
    }
    out.push_str("[当前请求（");
    out.push_str(last_role);
    out.push_str("）]\n");
    out.push_str(last_text);
    out
}

// ---------------------------------------------------------------------------
// 执行 + 响应组装
// ---------------------------------------------------------------------------

/// 分离执行 + Done-first 折叠（与 `AgentLoop::run_detached` 同语义；
/// 附带 tool_calls 计数给扩展元数据）。
async fn run_task(
    agent_loop: &Arc<nemesis_agent::r#loop::AgentLoop>,
    task: &str,
) -> Result<(String, usize), String> {
    let events = agent_loop
        .run_detached_events(
            task,
            DetachedOpts {
                label: Some(OPENAI_LABEL),
                ..Default::default()
            },
        )
        .await;
    let mut done: Option<String> = None;
    let mut error: Option<String> = None;
    let mut tool_calls = 0usize;
    for e in events {
        match e {
            AgentEvent::Done(m) => done = Some(m),
            AgentEvent::Error(e) => error = Some(e),
            AgentEvent::ToolCall(infos) => tool_calls += infos.len(),
            _ => {}
        }
    }
    match (done, error) {
        (Some(m), _) => Ok((m, tool_calls)),
        (None, Some(e)) => Err(e),
        (None, None) => Err("agent produced no output".to_string()),
    }
}

fn completion_id() -> String {
    format!("chatcmpl-nemesis-{}", uuid::Uuid::new_v4().simple())
}

fn active_model_name(state: &AppState) -> String {
    state.model_name.lock().clone()
}

/// 非流式响应体（OpenAI `chat.completion`；`nemesis` 为扩展元数据字段）。
fn completion_json(id: &str, model: &str, content: &str, tool_calls: usize) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "object": "chat.completion",
        "created": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        "model": model,
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": content },
            "finish_reason": "stop"
        }],
        "nemesis": { "tool_calls": tool_calls }
    })
}

/// SSE chunk（`chat.completion.chunk`）。`finish` 些 Some 时 delta 为空、
/// finish_reason 落定（OpenAI 结尾帧形态）。
fn chunk_json(id: &str, model: &str, delta: serde_json::Value, finish: Option<&str>) -> String {
    serde_json::json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        "model": model,
        "choices": [{
            "index": 0,
            "delta": delta,
            "finish_reason": finish.map(|f| serde_json::json!(f)).unwrap_or(serde_json::Value::Null)
        }]
    })
    .to_string()
}

/// 端点主体（handler 是薄壳；测试直接调本函数断言状态码 + JSON）。
pub(crate) async fn api_chat_completions(state: &AppState, body: &[u8]) -> Response {
    // 1. 解析请求（手解析——错误面必须是 OpenAI 形态 JSON，不是 axum
    //    提取器的纯文本 422）。
    let req: OpenAiChatRequest = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => {
            return error_response(
                axum::http::StatusCode::BAD_REQUEST,
                format!("invalid request body: {e}"),
                "invalid_request_error",
            );
        }
    };
    if req.messages.is_empty() {
        return error_response(
            axum::http::StatusCode::BAD_REQUEST,
            "messages must not be empty".to_string(),
            "invalid_request_error",
        );
    }

    // 2. estop 闸（急停时诚实 503，不让请求进 agent 烧 token）。
    if let Some(estop) = &state.estop
        && estop.is_engaged()
    {
        return error_response(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "急停（estop）已触发：agent 活动已冻结，释放急停后恢复".to_string(),
            "server_error",
        );
    }

    // 3. agent loop 在场（headless/未装配 = 诚实 503，不裸调 provider）。
    let Some(agent_loop) = state.agent_loop.read().clone() else {
        return error_response(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "agent loop not running".to_string(),
            "server_error",
        );
    };

    // 4. messages → 任务 → 分离执行（安全 8 层与 gateway 同源）。
    let task = assemble_task(&req.messages);
    let started = Instant::now();
    let (content, tool_calls) = match run_task(&agent_loop, &task).await {
        Ok(ok) => ok,
        Err(e) => {
            tracing::warn!("[OpenAI-compat] agent 执行失败: {e}");
            return error_response(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("agent error: {e}"),
                "server_error",
            );
        }
    };
    let duration_ms = started.elapsed().as_millis() as u64;

    let id = completion_id();
    // 回显请求的 model；缺省用活跃模型名。真实执行模型在扩展字段如实报告。
    let model = req
        .model
        .clone()
        .unwrap_or_else(|| active_model_name(state));
    let execution_model = active_model_name(state);

    if !req.stream {
        let mut body = completion_json(&id, &model, &content, tool_calls);
        body["nemesis"]["execution_model"] = serde_json::json!(execution_model);
        body["nemesis"]["duration_ms"] = serde_json::json!(duration_ms);
        return axum::Json(body).into_response();
    }

    // stream:true —— chunk 形态兼容；Done 整段一次下发（诚实边界见模块头）。
    let chunks = vec![
        chunk_json(
            &id,
            &model,
            serde_json::json!({ "role": "assistant", "content": "" }),
            None,
        ),
        chunk_json(&id, &model, serde_json::json!({ "content": content }), None),
        chunk_json(&id, &model, serde_json::json!({}), Some("stop")),
    ];
    let mut frames: Vec<Result<SseEvent, Infallible>> = Vec::with_capacity(chunks.len() + 2);
    for c in chunks {
        frames.push(Ok(SseEvent::default().data(c)));
    }
    frames.push(Ok(SseEvent::default().data("[DONE]")));
    let mut response = Sse::new(futures::stream::iter(frames))
        .keep_alive(KeepAlive::default())
        .into_response();
    // 扩展元数据挂响应头（SSE body 已被协议占用）。
    if let Ok(v) = http::HeaderValue::from_str(&execution_model) {
        response
            .headers_mut()
            .insert("x-nemesis-execution-model", v);
    }
    if let Ok(v) = http::HeaderValue::from_str(&tool_calls.to_string()) {
        response.headers_mut().insert("x-nemesis-tool-calls", v);
    }
    if let Ok(v) = http::HeaderValue::from_str(&duration_ms.to_string()) {
        response.headers_mut().insert("x-nemesis-duration-ms", v);
    }
    response
}

/// axum handler 薄壳。
pub async fn handle_chat_completions(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    body: axum::body::Bytes,
) -> Response {
    api_chat_completions(&state, &body).await
}

#[cfg(test)]
mod tests;
