//! WSAPI 客户端 + 确定性聊天驱动（2026-09-25 IT base_url 专项：断言硬化）。
//!
//! - [`WsApi`]：reqId 关联的 WSAPI 请求客户端（`models.set_default` 热切
//!   默认模型；同 agent-bench / board_ws_tests 先例）。
//! - [`chat_round_collect_tools`]：显式 session_id 发一轮 chat.send，沿途
//!   收集 `tool_event` push 帧，等本会话 assistant 回复收尾——工具调用是否
//!   真的发生、成败如何，以事件流为准（FILE_OP 驱动模型的最终回复是常量
//!   引导文案，对工具执行结果零信号）。

use std::time::Duration;

use anyhow::{Result, anyhow};
use futures::StreamExt;
use serde_json::{Value, json};
use test_harness::*;

/// 单条工具事件（`tool_event` push 帧的扁平化提取）。
///
/// 帧形：`{type:"push", module:"chat", cmd:"tool_event", data:{kind, data:{...}}}`
/// ——AgentEvent 以 serde `tag="kind", content="data"` 序列化（PascalCase
/// kind：ToolStarted / ToolFinished）。诚实边界：安全层拦停的调用不进
/// around 链，**不产生事件**（tool_event_hook.rs 布点语义）。
#[derive(Debug, Clone)]
pub struct ToolEvent {
    pub kind: String,
    pub tool: String,
    pub ok: bool,
    pub result_preview: String,
}

/// WSAPI 请求客户端（一连接多请求，reqId 关联响应）。
pub struct WsApi {
    stream: WsStream,
    next_id: u32,
}

impl WsApi {
    pub async fn connect() -> Result<Self> {
        Ok(Self {
            stream: ws_connect(WS_PORT, AUTH_TOKEN).await?,
            next_id: 0,
        })
    }

    /// 发一帧 WSAPI request，收同 reqId 的 response。
    /// Err = 传输失败、超时（60s）或服务端 error 字段。
    pub async fn call(
        &mut self,
        module: &str,
        cmd: &str,
        data: Option<Value>,
    ) -> Result<Option<Value>> {
        self.next_id += 1;
        let req_id = format!("it-{}", self.next_id);
        let msg = json!({
            "type": "request",
            "module": module,
            "cmd": cmd,
            "reqId": req_id,
            "data": data,
        });
        ws_send_json(&mut self.stream, &msg).await?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            let next = tokio::time::timeout_at(deadline, self.stream.next())
                .await
                .map_err(|_| anyhow!("WSAPI {module}.{cmd} 响应超时（60s）"))?;
            match next {
                Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                    let Ok(v) = serde_json::from_str::<Value>(&text) else {
                        continue;
                    };
                    if v.get("type").and_then(|t| t.as_str()) == Some("response")
                        && v.get("reqId").and_then(|r| r.as_str()) == Some(req_id.as_str())
                    {
                        if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
                            anyhow::bail!("{module}.{cmd} 失败: {err}");
                        }
                        return Ok(v.get("data").cloned().filter(|d| !d.is_null()));
                    }
                    continue; // 其它 reqId 的 response / push 帧
                }
                Some(Ok(_)) => continue, // ping/pong/binary
                Some(Err(e)) => anyhow::bail!("ws error: {e}"),
                None => anyhow::bail!("ws closed"),
            }
        }
    }

    /// 热切默认模型（`models.set_default`，按 model_name 解析——新会话
    /// 实例按当前默认装配 provider；board_ws_tests / agent-bench 同款）。
    pub async fn set_default_model(&mut self, alias: &str) -> Result<()> {
        self.call("models", "set_default", Some(json!({ "name": alias })))
            .await?;
        Ok(())
    }
}

fn fresh_session_id(prefix: &str) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{prefix}-{}-{}", now.as_millis(), now.subsec_nanos())
}

/// 从 `tool_event` push 帧提取工具事件；形状不符（前端专用注入字段缺席等）
/// 返回 None 由调用方跳过。
fn parse_tool_event(frame: &Value) -> Option<ToolEvent> {
    let data = frame.get("data")?;
    let kind = data.get("kind")?.as_str()?.to_string();
    let inner = data.get("data")?;
    Some(ToolEvent {
        kind,
        tool: inner.get("tool")?.as_str()?.to_string(),
        ok: inner.get("ok").and_then(|o| o.as_bool()).unwrap_or(false),
        result_preview: inner
            .get("result_preview")
            .and_then(|r| r.as_str())
            .unwrap_or("")
            .to_string(),
    })
}

/// 发一轮 chat.send（显式 session_id，新会话零历史串扰），收集沿途
/// `tool_event` push 帧，等本会话 assistant 回复收尾。
/// 返回（回复文本，按到达序的工具事件）。
pub async fn chat_round_collect_tools(
    content: &str,
    timeout_secs: u64,
) -> Result<(String, Vec<ToolEvent>)> {
    let session_id = fresh_session_id("it");
    let mut stream = ws_connect(WS_PORT, AUTH_TOKEN).await?;
    let msg = json!({
        "type": "message",
        "module": "chat",
        "cmd": "send",
        "data": { "content": content, "session_id": session_id },
        "timestamp": chrono::Local::now().to_rfc3339(),
    });
    ws_send_json(&mut stream, &msg).await?;

    let mut events = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        let next = tokio::time::timeout_at(deadline, stream.next())
            .await
            .map_err(|_| anyhow!("chat 回复超时（{timeout_secs}s）"))?;
        match next {
            Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                let Ok(v) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                let msg_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
                let module = v.get("module").and_then(|m| m.as_str()).unwrap_or("");
                let cmd = v.get("cmd").and_then(|c| c.as_str()).unwrap_or("");
                if msg_type == "push" && module == "chat" && cmd == "tool_event" {
                    if let Some(ev) = parse_tool_event(&v) {
                        events.push(ev);
                    }
                    continue;
                }
                if msg_type == "message" && module == "chat" && cmd == "receive" {
                    if v["data"]["session_id"].as_str() != Some(session_id.as_str()) {
                        continue; // 其它会话的广播帧
                    }
                    match v["data"]["role"].as_str() {
                        Some("user") => continue, // 发送回声帧
                        Some("assistant") => {
                            let content = v["data"]["content"].as_str().unwrap_or("").to_string();
                            let _ = stream.close(None).await;
                            return Ok((content, events));
                        }
                        _ => continue,
                    }
                }
                if msg_type == "system" && module == "error" {
                    let err = v["data"]["content"].as_str().unwrap_or("unknown error");
                    anyhow::bail!("Error response: {err}");
                }
            }
            Some(Ok(_)) => continue,
            Some(Err(e)) => anyhow::bail!("ws error: {e}"),
            None => anyhow::bail!("ws closed"),
        }
    }
}
