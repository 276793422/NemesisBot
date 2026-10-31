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
//! - `temperature`/`max_tokens` 等其余 OpenAI 字段：分离执行 lane 忽略
//!   （serde 默认）。
//!
//! ## 裸转发 lane（2026-10-15 BUG-③A：带 tools 的请求）
//!
//! 请求体带**非空 `tools`** = 调用方自带工具编排（集群 worker 执行的
//! 标志形态）——本 bot 只当**模型代理**，不起 AgentLoop：master 侧的
//! 工具/文件系统/看板对 worker 执行语义整体错位（master detached agent
//! 会用自己的 cluster_rpc 把任务派回 worker、在 master 文件系统上找
//! worker 工作副本——2026-10-14 NB-1 事故根因，详见 BUG 报告归档）。
//! 裸 lane 把请求消息/工具转成 provider 中立类型，调 `AppState`
//! streaming_provider（`default_following` wrapper，空模型名 = 当前默认
//! 模型槽），并把 master 模型卡语义补齐进 ChatOptions（reasoning_effort
//! 恒以 master 卡片为准；max_tokens 未指明时用卡片 max_output_tokens）。
//! 原始响应（含 `tool_calls`/`usage`/`reasoning_content`）重编回 OpenAI
//! 形态透传；`stream:true` 走 `chat_stream` 逐 chunk 重编为 OpenAI SSE。
//! 诚实边界：`tool_choice` 只支持缺省/auto（providers 恒 auto，显式
//! 非 auto 请求 400 拒绝）；协议为图像生成 lane（images-openai）的活跃
//! 模型诚实拒绝（裸 lane 是 chat 通道）。
//!
//! 错误面（两条 lane 一致）一律 OpenAI 形态 `{"error":{"message","type","code"}}`。

use crate::api_handlers::AppState;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use nemesis_agent::r#loop::DetachedOpts;
use nemesis_agent::types::AgentEvent;
use nemesis_providers::types::ChatOptions;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Instant;

/// 会话标签（进 session_key `subagent:openai-api:{uuid}` 与 trace_id，
/// 请求日志按此检索）。
const OPENAI_LABEL: &str = "openai-api";

// ---------------------------------------------------------------------------
// 请求/响应 wire 形态
// ---------------------------------------------------------------------------

/// `POST /v1/chat/completions` 请求体。未列出的 OpenAI 字段由 serde 默认
/// 忽略；`stream` 缺省 = false。
#[derive(Debug, serde::Deserialize)]
pub struct OpenAiChatRequest {
    /// 模型名。接受并回显，执行用 bot 活跃模型（模块头诚实边界）。
    #[serde(default)]
    pub model: Option<String>,
    pub messages: Vec<OpenAiMessage>,
    #[serde(default)]
    pub stream: bool,
    /// 裸转发 lane 判据（2026-10-15 BUG-③A）：非空 = 调用方自带工具编排
    /// （集群 worker 执行形态），走 provider 直转；空 = 历史 AgentLoop
    /// 分离执行路径字节不变。
    #[serde(default)]
    pub tools: Vec<nemesis_providers::types::ToolDefinition>,
    /// 裸 lane 只支持缺省/auto（providers 恒 auto）；显式非 auto = 400。
    #[serde(default)]
    pub tool_choice: Option<serde_json::Value>,
    /// 裸 lane 透传给 provider 的采样参数（分离执行 lane 忽略）。
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub max_tokens: Option<i64>,
    #[serde(default)]
    pub top_p: Option<f64>,
    #[serde(default)]
    pub stop: Option<Vec<String>>,
}

/// OpenAI chat 消息。`content` 兼容三种形态：字符串（主流）、多模态
/// parts 数组（只取 text part 拼接）、null（如带 tool_calls 的 assistant
/// 消息——分离执行 lane 取空串；裸 lane 转空文本 Message）。
#[derive(Debug, Default, serde::Deserialize)]
pub struct OpenAiMessage {
    pub role: String,
    #[serde(default)]
    pub content: Option<serde_json::Value>,
    /// 裸 lane 保真字段：assistant 历史轮的工具调用 + tool 结果回指。
    /// 分离执行 lane 不消费（`.text()` 只取 content）。
    #[serde(default)]
    pub tool_calls: Vec<nemesis_providers::types::ToolCall>,
    #[serde(default)]
    pub tool_call_id: Option<String>,
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

    /// 裸转发 lane：转 provider 中立 Message（content 三态 → MessageContent，
    /// tool_calls / tool_call_id 保真——wire 适配归 provider 层）。
    fn to_provider_message(&self) -> nemesis_providers::types::Message {
        let content = match &self.content {
            Some(v) => normalize_openai_content(v).unwrap_or_else(|reason| {
                // 兜底不吞文本（BUG-b 2026-10-30）：归一失败（未知/畸形
                // part）退化为纯文本（text() 的数组 text-part 拼接语义），
                // 绝不静默清空整条消息。原先 `unwrap_or_default()` 会把
                // OpenAI `image_url` parts 连同 text part 一起清成空串。
                tracing::warn!(
                    "[OpenAI-compat] 多模态 content 归一失败，退化为纯文本透传: {reason}"
                );
                nemesis_providers::types::MessageContent::Text(self.text())
            }),
            // null / 缺失 → 空文本（tool 结果消息常态；MessageContent 反
            // 序列化本就宽容，走到这里的是非 string/array 形态）。
            None => nemesis_providers::types::MessageContent::default(),
        };
        nemesis_providers::types::Message {
            role: self.role.clone(),
            content,
            tool_calls: self.tool_calls.clone(),
            tool_call_id: self.tool_call_id.clone(),
            ..Default::default()
        }
    }
}

/// OpenAI wire 多模态 content 归一（BUG-b 2026-10-30，纯函数供测试）：
/// OpenAI 形态 `{"type":"image_url","image_url":{"url":...}}` 与内部
/// [`ContentPart`] serde tag（`type:"image"`）不同，直接 `from_value` 会
/// 整体失败 → 整条消息被清空。本函数把 string / text part / image_url
/// part 归一为内部形态；未知或畸形 part 诚实报 Err（交由调用方兜底），
/// 不做静默丢弃。
pub(crate) fn normalize_openai_content(
    v: &serde_json::Value,
) -> Result<nemesis_providers::types::MessageContent, String> {
    use nemesis_providers::types::{ContentPart, ImageSource, MessageContent};
    match v {
        serde_json::Value::String(s) => Ok(MessageContent::Text(s.clone())),
        serde_json::Value::Array(parts) => {
            let mut out: Vec<ContentPart> = Vec::with_capacity(parts.len());
            for p in parts {
                let ty = p.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match ty {
                    "text" => {
                        let text = p
                            .get("text")
                            .and_then(|t| t.as_str())
                            .ok_or_else(|| format!("text part 缺 `text` 字段: {p}"))?;
                        out.push(ContentPart::Text {
                            text: text.to_string(),
                        });
                    }
                    "image_url" => {
                        let img = p
                            .get("image_url")
                            .ok_or_else(|| format!("image_url part 缺 `image_url` 对象: {p}"))?;
                        let url = img
                            .get("url")
                            .and_then(|u| u.as_str())
                            .ok_or_else(|| format!("image_url.url 缺失或非字符串: {p}"))?;
                        // data URI（data:<media_type>;base64,<data>）拆
                        // Base64；其余按 http(s) URL 原样透传交 provider。
                        let image = if let Some(rest) = url.strip_prefix("data:") {
                            let (meta, data) = rest.split_once(";base64,").ok_or_else(|| {
                                format!("image_url data URI 非 base64 形态: {url}")
                            })?;
                            let media_type = meta.split(';').next().unwrap_or("").to_string();
                            if media_type.is_empty() || data.is_empty() {
                                return Err(format!("image_url data URI 不完整: {url}"));
                            }
                            ImageSource::Base64 {
                                media_type,
                                data: data.to_string(),
                            }
                        } else {
                            ImageSource::Url(url.to_string())
                        };
                        let detail = img
                            .get("detail")
                            .and_then(|d| d.as_str())
                            .map(|s| s.to_string())
                            .and_then(|s| {
                                serde_json::from_value::<nemesis_providers::types::ImageDetail>(
                                    serde_json::Value::String(s),
                                )
                                .ok()
                            });
                        out.push(ContentPart::Image { image, detail });
                    }
                    other => {
                        return Err(format!("未支持的 content part type `{other}`: {p}"));
                    }
                }
            }
            Ok(MessageContent::Parts(out))
        }
        _ => Err(format!("不支持的 content 形态: {v}")),
    }
}

/// OpenAI 规范允许 `tools[]` 条目省略 `function.description`（可空）与
/// `function.parameters`（无参函数可缺）；内部 `ToolFunctionDefinition`
/// 两字段必填——就地把缺失项补默认（lenient 归一，纯函数供测试）。其余
/// 形态错误（无 function 对象等）不救，交给 serde 诚实 400。
fn coerce_tools_lenient(v: &mut serde_json::Value) {
    let Some(tools) = v.get_mut("tools").and_then(|t| t.as_array_mut()) else {
        return;
    };
    for t in tools {
        let Some(func) = t.get_mut("function").and_then(|f| f.as_object_mut()) else {
            continue;
        };
        func.entry("description".to_string())
            .or_insert(serde_json::Value::String(String::new()));
        func.entry("parameters".to_string())
            .or_insert(serde_json::json!({ "type": "object", "properties": {} }));
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
    //    提取器的纯文本 422）。tools 条目先做 lenient 归一（OpenAI 规范
    //    允许省略 description/parameters），再进 typed 反序列化。
    let req: OpenAiChatRequest = {
        let mut raw: serde_json::Value = match serde_json::from_slice(body) {
            Ok(r) => r,
            Err(e) => {
                return error_response(
                    axum::http::StatusCode::BAD_REQUEST,
                    format!("invalid request body: {e}"),
                    "invalid_request_error",
                );
            }
        };
        coerce_tools_lenient(&mut raw);
        match serde_json::from_value(raw) {
            Ok(r) => r,
            Err(e) => {
                return error_response(
                    axum::http::StatusCode::BAD_REQUEST,
                    format!("invalid request body: {e}"),
                    "invalid_request_error",
                );
            }
        }
    };
    if req.messages.is_empty() {
        return error_response(
            axum::http::StatusCode::BAD_REQUEST,
            "messages must not be empty".to_string(),
            "invalid_request_error",
        );
    }

    // 2. estop 闸（急停时诚实 503，不让请求进 agent 烧 token；裸转发
    //    lane 同闸——worker 的 LLM 消耗也是 agent 活动）。
    if let Some(estop) = &state.estop
        && estop.is_engaged()
    {
        return error_response(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "急停（estop）已触发：agent 活动已冻结，释放急停后恢复".to_string(),
            "server_error",
        );
    }

    // 2.5 裸转发 lane（BUG-③A）：带非空 tools 的请求 = 调用方自带工具
    //    编排（集群 worker 执行形态）——master 只当模型代理，不起
    //    AgentLoop（不需要 agent 在场，也不该把 worker 任务喂进 master
    //    的工具/文件系统/看板；模块头「裸转发 lane」节）。
    if !req.tools.is_empty() {
        if let Some(tc) = req.tool_choice.as_ref().and_then(|v| v.as_str()) {
            if !tc.eq_ignore_ascii_case("auto") {
                return error_response(
                    axum::http::StatusCode::BAD_REQUEST,
                    format!("bare lane supports tool_choice omitted/\"auto\" only, got \"{tc}\""),
                    "invalid_request_error",
                );
            }
        } else if req.tool_choice.is_some() {
            // 对象形态 {"type":"function",...} / 具名指定：同不支持。
            return error_response(
                axum::http::StatusCode::BAD_REQUEST,
                "bare lane supports tool_choice omitted/\"auto\" only".to_string(),
                "invalid_request_error",
            );
        }
        return bare_forward(state, req).await;
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

// ---------------------------------------------------------------------------
// 裸转发 lane（BUG-③A，2026-10-15）
// ---------------------------------------------------------------------------

/// `FailoverError` → (HTTP 状态码, OpenAI error type)。调用方鉴权已过
/// REST 中间件——Auth/Billing 是 **master 侧上游凭据**问题，映射 502 而
/// 非 401（401 会让 worker 误以为是自己到 master 的鉴权断了）。
fn bare_error_status(
    e: &nemesis_providers::failover::FailoverError,
) -> (axum::http::StatusCode, &'static str) {
    use nemesis_providers::failover::FailoverError as FE;
    match e {
        FE::Timeout { .. } => (axum::http::StatusCode::GATEWAY_TIMEOUT, "timeout"),
        FE::RateLimit { .. } => (
            axum::http::StatusCode::TOO_MANY_REQUESTS,
            "rate_limit_error",
        ),
        FE::Auth { .. } | FE::Billing { .. } => {
            (axum::http::StatusCode::BAD_GATEWAY, "upstream_auth_error")
        }
        FE::Overloaded { .. } => (axum::http::StatusCode::SERVICE_UNAVAILABLE, "server_error"),
        FE::Format { .. } | FE::Unknown { .. } => {
            (axum::http::StatusCode::BAD_GATEWAY, "server_error")
        }
    }
}

/// master 模型卡语义（纯函数供测试）：`(reasoning_effort, max_output_tokens)`。
/// 「统一走 master 提供的 LLM」的语义补齐——worker 请求不带 effort，模型
/// 档位由 master 卡片说了算；max_tokens 未指明时用卡片输出上限。
fn model_card_semantics(cfg: &serde_json::Value, model: &str) -> (Option<String>, Option<i64>) {
    let effort = nemesis_types::capability::resolve_reasoning_effort(cfg, model);
    let max_out = nemesis_types::capability::resolve_max_output_tokens(cfg, model);
    (effort, max_out)
}

/// 裸 lane 的 ChatOptions：调用方采样参数透传 + master 卡片语义注入。
/// config.json 缺失/解析失败/无该模型条目 = 诚实跳过注入（options 只剩
/// 调用方字段——不造语义）。
fn bare_chat_options(state: &AppState, model: &str, req: &OpenAiChatRequest) -> ChatOptions {
    let mut options = ChatOptions {
        temperature: req.temperature,
        max_tokens: req.max_tokens,
        top_p: req.top_p,
        stop: req.stop.clone(),
        extra: Default::default(),
        reasoning_effort: None,
    };
    let Some(home) = state.home.as_deref() else {
        return options;
    };
    let Ok(raw) =
        nemesis_utils::fsutil::read_config_string(std::path::Path::new(home).join("config.json"))
    else {
        return options;
    };
    let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return options;
    };
    let (effort, max_out) = model_card_semantics(&cfg, model);
    // master 卡片 effort 恒赢（上游是 master 选的模型，档位跟着模型走）。
    options.reasoning_effort = effort;
    if options.max_tokens.is_none() {
        options.max_tokens = max_out;
    }
    options
}

/// 裸转发 lane 主体：provider 槽直转（模块头「裸转发 lane」节）。
async fn bare_forward(state: &AppState, req: OpenAiChatRequest) -> Response {
    // provider 槽：streaming_provider 是 default_following wrapper——空
    // 模型名路由到当前默认槽（set_default 热切即时生效），槽未装过走
    // 启动捕获降级（NullProvider 时 chat() 诚实报错 → 下错误面）。
    let Some(provider) = state.streaming_provider.clone() else {
        return error_response(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "provider not assembled (bare lane unavailable in relay/test assembly)".to_string(),
            "server_error",
        );
    };
    let started = Instant::now();
    // 执行模型 = master 活跃模型（AppState 展示字段与 set_default 同一
    // 更新点；空名兜底默认槽文本）。
    let execution_model = {
        let m = active_model_name(state);
        if m.is_empty() {
            nemesis_providers::default_slot::current()
                .map(|(_, model, _)| model)
                .unwrap_or(m)
        } else {
            m
        }
    };

    // 协议守卫（belt-and-suspenders——set_default 已拒 images-openai，
    // 这里防手工改 config 后热切绕过）：裸 lane 是 chat 通道。
    if let Some(home) = state.home.as_deref()
        && let Ok(raw) = nemesis_utils::fsutil::read_config_string(
            std::path::Path::new(home).join("config.json"),
        )
        && let Ok(cfg) = serde_json::from_str::<nemesis_config::Config>(&raw)
        && let Ok(res) = nemesis_config::resolve_model_config(&cfg, &execution_model)
        && res.protocol == "images-openai"
    {
        return error_response(
            axum::http::StatusCode::BAD_REQUEST,
            format!(
                "active model '{execution_model}' is an image-generation lane; bare lane is chat-only"
            ),
            "invalid_request_error",
        );
    }

    let messages: Vec<nemesis_providers::types::Message> = req
        .messages
        .iter()
        .map(|m| m.to_provider_message())
        .collect();
    let options = bare_chat_options(state, &execution_model, &req);
    tracing::info!(
        "[OpenAI-compat] 裸转发 lane：model={execution_model} provider={} tools={} messages={} stream={}",
        provider.name(),
        req.tools.len(),
        messages.len(),
        req.stream
    );

    if req.stream {
        return bare_forward_stream(
            state,
            provider.as_ref(),
            req,
            messages,
            options,
            execution_model,
            started,
        )
        .await;
    }

    match provider
        .chat(&messages, &req.tools, &execution_model, &options)
        .await
    {
        Ok(resp) => {
            let duration_ms = started.elapsed().as_millis() as u64;
            tracing::info!(
                "[OpenAI-compat] 裸转发完成：model={execution_model} tool_calls={} finish={} usage={} {}ms",
                resp.tool_calls.len(),
                resp.finish_reason,
                resp.usage.as_ref().map(|u| u.total_tokens).unwrap_or(0),
                duration_ms
            );
            let id = completion_id();
            let mut body = bare_completion_json(&id, &req, &resp, duration_ms);
            body["nemesis"]["execution_model"] = serde_json::json!(execution_model);
            axum::Json(body).into_response()
        }
        Err(e) => {
            let (status, err_type) = bare_error_status(&e);
            tracing::warn!("[OpenAI-compat] 裸转发失败：model={execution_model} {status} {e}");
            error_response(status, format!("upstream error: {e}"), err_type)
        }
    }
}

/// 裸 lane 非流式响应体（OpenAI `chat.completion`；`nemesis` 扩展字段带
/// execution_model + lane 标记）。usage 上游有就透传（不造数）。
fn bare_completion_json(
    id: &str,
    req: &OpenAiChatRequest,
    resp: &nemesis_providers::types::LLMResponse,
    duration_ms: u64,
) -> serde_json::Value {
    let mut message = serde_json::json!({ "role": "assistant", "content": resp.content });
    if !resp.tool_calls.is_empty() {
        message["tool_calls"] =
            serde_json::to_value(&resp.tool_calls).unwrap_or(serde_json::Value::Null);
    }
    if let Some(rc) = &resp.reasoning_content {
        message["reasoning_content"] = serde_json::json!(rc);
    }
    let mut body = serde_json::json!({
        "id": id,
        "object": "chat.completion",
        "created": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        // 回显请求的 model（与分离执行 lane 同语义）；真实执行模型在
        // nemesis.execution_model。
        "model": req.model.clone().unwrap_or_default(),
        "choices": [{
            "index": 0,
            "message": message,
            "finish_reason": resp.finish_reason,
        }],
        "nemesis": { "lane": "bare-forward", "duration_ms": duration_ms },
    });
    if let Some(u) = &resp.usage {
        body["usage"] = serde_json::to_value(u).unwrap_or(serde_json::Value::Null);
    }
    body
}

/// StreamChunk → OpenAI SSE delta 帧（`chat.completion.chunk`）。
fn bare_chunk_json(
    id: &str,
    model: &str,
    delta: serde_json::Value,
    finish: Option<&str>,
) -> String {
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

/// StreamChunk → OpenAI SSE 终帧（空 delta + finish_reason + usage）。
fn bare_final_chunk_json(
    id: &str,
    model: &str,
    finish: &str,
    usage: Option<&nemesis_providers::types::UsageInfo>,
) -> String {
    let mut v = serde_json::json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        "model": model,
        "choices": [{
            "index": 0,
            "delta": {},
            "finish_reason": finish,
        }]
    });
    if let Some(u) = usage {
        v["usage"] = serde_json::to_value(u).unwrap_or(serde_json::Value::Null);
    }
    v.to_string()
}

/// 完整 ToolCall 列表 → OpenAI delta.tool_calls 数组（index/id/function
/// 完整形态；arguments 一次整发——worker 侧解析器按字符串累积，等价）。
fn tool_calls_delta_json(tool_calls: &[nemesis_providers::types::ToolCall]) -> serde_json::Value {
    serde_json::Value::Array(
        tool_calls
            .iter()
            .enumerate()
            .map(|(i, tc)| {
                serde_json::json!({
                    "index": i,
                    "id": tc.id,
                    "type": tc.call_type.clone().unwrap_or_else(|| "function".to_string()),
                    "function": {
                        "name": tc.function.as_ref().map(|f| f.name.clone()).unwrap_or_default(),
                        "arguments": tc.function.as_ref().map(|f| f.arguments.clone()).unwrap_or_default(),
                    },
                })
            })
            .collect(),
    )
}

/// 裸 lane 流式透传：`chat_stream` receiver → OpenAI SSE。首帧即 Err =
/// 上游起流失败 → 诚实 JSON 错误（不吐半个 SSE 流）；中途 Err = 带内
/// 注记 + 合成 stop 收尾（worker 侧 J1 否则会把断流当完整 stop 静默截断）。
async fn bare_forward_stream(
    state: &AppState,
    provider: &dyn nemesis_providers::router::LLMProvider,
    req: OpenAiChatRequest,
    messages: Vec<nemesis_providers::types::Message>,
    options: ChatOptions,
    execution_model: String,
    started: Instant,
) -> Response {
    let mut rx = provider.chat_stream(&messages, &req.tools, &execution_model, &options);
    let id = completion_id();
    let model_echo = req.model.clone().unwrap_or_default();
    // stream 块内多次使用（tracing），头写入还要一份——clone 进块。
    let execution_model_stream = execution_model.clone();

    // 首帧判定：起流失败不 commit SSE（错误面保持 JSON 形态）；唯「流式
    // 不被该 provider 支持」（trait 缺省实现，BUG-a 2026-10-30）例外——
    // 回落非流式合成 SSE，不把本地能力缺口伪装成上游故障（此前直接 502
    // "not implemented"，请求死在本进程未出网；agent 链路 llm_bridge 同判
    // 同源回落，两条链路行为对齐）。
    let first = match rx.recv().await {
        Some(Ok(c)) => c,
        Some(Err(e)) => {
            if nemesis_providers::router::is_stream_unsupported(&e) {
                tracing::info!(
                    "[OpenAI-compat] 裸转发流式不被支持，回落非流式：model={execution_model}"
                );
                return bare_fallback_non_stream(
                    provider,
                    &req,
                    messages,
                    options,
                    execution_model,
                    started,
                )
                .await;
            }
            let (status, err_type) = bare_error_status(&e);
            tracing::warn!(
                "[OpenAI-compat] 裸转发流式起流失败：model={execution_model} {status} {e}"
            );
            return error_response(status, format!("upstream error: {e}"), err_type);
        }
        None => {
            return error_response(
                axum::http::StatusCode::BAD_GATEWAY,
                "upstream produced no output".to_string(),
                "server_error",
            );
        }
    };

    let stream = async_stream::stream! {
        let mut pending: Option<nemesis_providers::http_provider::StreamChunk> = Some(first);
        let mut finish_sent = false;
        loop {
            let chunk = match pending.take() {
                Some(c) => c,
                None => match rx.recv().await {
                    Some(Ok(c)) => c,
                    Some(Err(e)) => {
                        // 中途断流：带内注记 + 合成 stop（worker 侧 J1 把
                        // 无终态断流当完整 stop——静默截断不可见）。
                        tracing::warn!(
                            "[OpenAI-compat] 裸转发流中断：model={execution_model_stream} {e}"
                        );
                        yield Ok::<_, Infallible>(SseEvent::default().data(bare_chunk_json(
                            &id,
                            &model_echo,
                            serde_json::json!({ "content": format!("\n\n[nemesis bare-forward upstream error: {e}]") }),
                            None,
                        )));
                        yield Ok(SseEvent::default().data(bare_final_chunk_json(
                            &id, &model_echo, "stop", None,
                        )));
                        finish_sent = true;
                        break;
                    }
                    None => break,
                },
            };
            // delta 帧：content / reasoning_content / tool_calls 同帧可并。
            let mut delta = serde_json::Map::new();
            if !chunk.delta.is_empty() {
                delta.insert("content".to_string(), serde_json::json!(chunk.delta));
            }
            if let Some(rc) = &chunk.reasoning_content {
                delta.insert("reasoning_content".to_string(), serde_json::json!(rc));
            }
            if !chunk.tool_calls.is_empty() {
                delta.insert(
                    "tool_calls".to_string(),
                    tool_calls_delta_json(&chunk.tool_calls),
                );
            }
            if !delta.is_empty() {
                yield Ok(SseEvent::default().data(bare_chunk_json(
                    &id,
                    &model_echo,
                    serde_json::Value::Object(delta),
                    None,
                )));
            }
            if let Some(f) = &chunk.finish_reason {
                // 终帧：空 delta + finish_reason + usage（OpenAI wire 形态）。
                yield Ok(SseEvent::default().data(bare_final_chunk_json(
                    &id,
                    &model_echo,
                    f,
                    chunk.usage.as_ref(),
                )));
                finish_sent = true;
                break;
            }
        }
        if !finish_sent {
            // receiver 耗尽无终态（provider J1 保证合成终帧，防御性兜底）。
            yield Ok(SseEvent::default().data(bare_final_chunk_json(
                &id, &model_echo, "stop", None,
            )));
        }
        yield Ok(SseEvent::default().data("[DONE]"));
        tracing::info!(
            "[OpenAI-compat] 裸转发流式完成：model={execution_model_stream} {}ms",
            started.elapsed().as_millis()
        );
    };

    let mut response = Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response();
    // 扩展元数据挂响应头（SSE body 已被协议占用）。
    if let Ok(v) = http::HeaderValue::from_str(&execution_model) {
        response
            .headers_mut()
            .insert("x-nemesis-execution-model", v);
    }
    let _ = state;
    response
}

/// 裸 lane 流式回落（BUG-a 2026-10-30）：provider 无流式 lane（`chat_stream`
/// trait 缺省实现）时改走 `chat()` 非流式，成功后合成 OpenAI SSE 完整流
/// （全量 content 单 delta 帧 + tool_calls delta 帧若有 + 终帧
/// finish_reason/usage + `[DONE]`；语义对齐 llm_bridge `fallback_non_stream`
/// ——非流式 provider 也产生一次完整「流」，调用方无差别）。回落后的
/// `chat()` 失败 = 真实上游错误，诚实上报不伪装。
async fn bare_fallback_non_stream(
    provider: &dyn nemesis_providers::router::LLMProvider,
    req: &OpenAiChatRequest,
    messages: Vec<nemesis_providers::types::Message>,
    options: ChatOptions,
    execution_model: String,
    started: Instant,
) -> Response {
    match provider
        .chat(&messages, &req.tools, &execution_model, &options)
        .await
    {
        Ok(resp) => {
            let id = completion_id();
            let model_echo = req.model.clone().unwrap_or_default();
            let execution_model_stream = execution_model.clone();
            let stream = async_stream::stream! {
                if !resp.content.is_empty() {
                    yield Ok::<_, Infallible>(SseEvent::default().data(bare_chunk_json(
                        &id,
                        &model_echo,
                        serde_json::json!({ "content": resp.content }),
                        None,
                    )));
                }
                if !resp.tool_calls.is_empty() {
                    yield Ok(SseEvent::default().data(bare_chunk_json(
                        &id,
                        &model_echo,
                        serde_json::json!({ "tool_calls": tool_calls_delta_json(&resp.tool_calls) }),
                        None,
                    )));
                }
                yield Ok(SseEvent::default().data(bare_final_chunk_json(
                    &id,
                    &model_echo,
                    &resp.finish_reason,
                    resp.usage.as_ref(),
                )));
                yield Ok(SseEvent::default().data("[DONE]"));
                tracing::info!(
                    "[OpenAI-compat] 裸转发非流式回落完成：model={execution_model_stream} {}ms",
                    started.elapsed().as_millis()
                );
            };
            let mut response = Sse::new(stream)
                .keep_alive(KeepAlive::default())
                .into_response();
            if let Ok(v) = http::HeaderValue::from_str(&execution_model) {
                response
                    .headers_mut()
                    .insert("x-nemesis-execution-model", v);
            }
            response
        }
        Err(e) => {
            let (status, err_type) = bare_error_status(&e);
            tracing::warn!(
                "[OpenAI-compat] 裸转发非流式回落失败：model={execution_model} {status} {e}"
            );
            error_response(status, format!("upstream error: {e}"), err_type)
        }
    }
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
