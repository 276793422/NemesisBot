//! P30（WS14）Canvas handler — `canvas.close`。
//!
//! v1 形态：canvas 的打开是 agent → 前端的**单向推送**（agent 终答检出合法
//! ```canvas 块 → `AgentEvent::CanvasOpen` → web pump 转 SSE `canvas.open`），
//! 不设 WSAPI 打开命令；`canvas.close` 只作前端关闭回执（审计日志 + ack），
//! 服务端无面板状态可清理——面板生命周期完全在前端本地（useCanvas）。

use crate::ws_router::{ModuleHandler, RequestContext};

pub struct CanvasHandler;

#[async_trait::async_trait]
impl ModuleHandler for CanvasHandler {
    fn module_name(&self) -> &str {
        "canvas"
    }

    fn commands(&self) -> &'static [&'static str] {
        &["close"]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        _ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        match cmd {
            "close" => {
                let data = data.ok_or("missing data")?;
                let session_id = data
                    .get("session_id")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .ok_or("missing session_id")?;
                tracing::info!("[Web/WSAPI] canvas closed: session={session_id}");
                Ok(Some(
                    serde_json::json!({ "closed": true, "session_id": session_id }),
                ))
            }
            _ => Err(format!("unknown command: canvas.{}", cmd)),
        }
    }
}

#[cfg(all(test, feature = "workflow"))]
mod tests;
