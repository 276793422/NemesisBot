//! LLM 消息/响应/Provider trait（LlmMessage/LlmResponse/LlmProvider）、观察者消息值提取、chat_call_bounded、wait_estop_engaged。
//!
//! P1 自 `loop.rs` 物理搬迁（docs/PLAN/2026-09-23_agentloop-god-object-decomposition.md §3.2）；语义零变化。
use super::prelude::*;
use super::*;

/// A simplified LLM message used for building requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmMessage {
    pub role: String,
    pub content: String,
    pub tool_calls: Option<Vec<ToolCallInfo>>,
    pub tool_call_id: Option<String>,
    /// Reasoning content from thinking-mode models, passed back to the API.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reasoning_content: Option<String>,
    /// T5/T6（多模态，goal 2026-09-03）：本条消息携带的图片（已水合的
    /// base64 字节）。历史存储只留路径引用（`ConversationTurn.image_refs`），
    /// build_messages 组包时经 `hydrate_image_refs` 每轮重读填充；文件已删
    /// → 占位文本进 content。旧请求重放/下游适配器忽略此字段即纯文本路径，
    /// `#[serde(default)]` 保旧快照兼容。
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub images: Vec<crate::image_attach::LlmImage>,
}

/// F-J（2026-09-04 四轮盲审）：observer 事件里 images[].data 的省略标记。
pub const OBSERVER_IMAGE_DATA_MARKER: &str =
    "[图片字节已省略：observer 事件不落 base64，路径见 path]";

/// observer 事件的 messages 序列化（F-J 单一真相源）。
///
/// build_messages 水合后的 `LlmMessage.images[].data` 是完整图片 base64
/// （单图 ≤25MB、每消息 ≤8 张）。逐轮 `serde_json::to_value` 若原样发射，
/// 字节会被复制进 observer 事件下游（to_callback_json /
/// request_logger_observer 的 01.AI.Request.raw.json），8×25MB 图 ≈ 每轮
/// ~270MB JSON 的磁盘/内存放大。此处在**发射源**把非空 `data` 替换为
/// [`OBSERVER_IMAGE_DATA_MARKER`]（保留 path/media_type 供排障定位）——
/// 下游消费方（loop_executor::to_callback_json / request_logger_observer）
/// 序列化的是事件里的值，源头干净则全链干净。真实 LLM 请求不受影响
/// （只改序列化副本，`messages` 本体原样上行）。
pub fn observer_msg_values(msgs: &[LlmMessage]) -> Vec<serde_json::Value> {
    msgs.iter()
        .filter_map(|m| {
            let mut v = serde_json::to_value(m).ok()?;
            if let Some(images) = v.get_mut("images").and_then(|i| i.as_array_mut()) {
                for img in images.iter_mut() {
                    let has_data = img
                        .get("data")
                        .and_then(|d| d.as_str())
                        .is_some_and(|s| !s.is_empty());
                    if let (true, Some(obj)) = (has_data, img.as_object_mut()) {
                        obj.insert(
                            "data".to_string(),
                            serde_json::Value::String(OBSERVER_IMAGE_DATA_MARKER.to_string()),
                        );
                    }
                }
            }
            Some(v)
        })
        .collect()
}

/// A simplified LLM response.
#[derive(Debug, Clone)]
pub struct LlmResponse {
    /// Text content of the response. May be empty if tool_calls are present.
    pub content: String,
    /// Tool calls requested by the LLM, if any.
    pub tool_calls: Vec<ToolCallInfo>,
    /// Whether the LLM indicated it is finished (no more tool calls).
    pub finished: bool,
    /// Reasoning content from thinking-mode models.
    pub reasoning_content: Option<String>,
    /// Token usage from the provider response.
    pub usage: Option<crate::loop_executor::ObserverUsageInfo>,
    /// Raw HTTP request body (for raw logging mode).
    pub raw_request_body: Option<serde_json::Value>,
    /// Raw HTTP response body (for raw logging mode).
    pub raw_response_body: Option<String>,
}

/// Trait for LLM providers used by the agent loop.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Send a chat request and return the response, or an error if the call fails.
    ///
    /// The agent loop uses the `Err` variant to detect context-window errors
    /// (token limit, context length exceeded, etc.) and trigger history compression.
    ///
    /// The `options` parameter controls generation parameters (temperature, max_tokens, etc.).
    /// Pass `None` to use provider defaults.
    ///
    /// The `tools` parameter provides tool definitions for function calling.
    async fn chat(
        &self,
        model: &str,
        messages: Vec<LlmMessage>,
        options: Option<crate::types::ChatOptions>,
        tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String>;
}

impl AgentLoop {
    /// BUG 2026-09-21 ②A：单次上游调用统一包超时——挂死连接（429/网络
    /// 黑洞后不回响应体）按超时失败计，不再无限等待（实测挂死 27 分钟）。
    /// 四个调用点（首捕 / 429 重试环 / transient 环 / hook 重呼）统一走
    /// 这里；`call_timeout` = 0 关闭超时（退回旧行为）。超时文案带
    /// "timed out"（transient 判定词表既有成员）：首捕挂死超时后自然落入
    /// transient 环有限重试，不会一次终局也不会无限挂。
    pub(crate) async fn chat_call_bounded(
        call_timeout: u64,
        provider: &std::sync::Arc<dyn LlmProvider>,
        model: &str,
        messages: Vec<LlmMessage>,
        options: Option<crate::types::ChatOptions>,
        tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        let call = provider.chat(model, messages, options, tools);
        if call_timeout == 0 {
            return call.await;
        }
        match tokio::time::timeout(std::time::Duration::from_secs(call_timeout), call).await {
            Ok(r) => r,
            Err(_) => Err(format!(
                "上游调用超时（{call_timeout}s 无响应，upstream timed out）"
            )),
        }
    }

    /// Wait until the e-stop watch flips to engaged, or forever if no
    /// receiver is wired (`None` → pending, i.e. the select arm never fires).
    ///
    /// Extracted verbatim from the estop arm of the primary LLM-call select
    /// (K1b, U14) — the post-LLM hook retry re-call needs the identical
    /// subscribe-window-safe wait (if already engaged at subscribe time,
    /// return immediately instead of waiting for the NEXT change).
    pub(crate) async fn wait_estop_engaged(rx: Option<&mut tokio::sync::watch::Receiver<bool>>) {
        match rx {
            Some(rx) => {
                // 订阅时已经 engaged（checkpoint A → subscribe 之间的窗口）
                // → 立刻 return，否则 changed() 会干等下一次变化、漏掉这次。
                if *rx.borrow() {
                    return;
                }
                while rx.changed().await.is_ok() {
                    if *rx.borrow() {
                        return;
                    }
                }
            }
            None => {
                std::future::pending::<()>().await;
            }
        }
    }
}
