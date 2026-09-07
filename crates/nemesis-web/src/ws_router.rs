//! WebSocket API router for request/response dispatch.
//!
//! Provides a modular handler registry where each module (models, channels, etc.)
//! registers a `ModuleHandler`. Incoming `type="request"` messages are dispatched
//! to the matching handler, and responses (with `reqId` correlation) are sent back.

use crate::api_handlers::AppState;
use crate::protocol::ProtocolMessage;
use crate::session::AuthMethod;
use crate::websocket_handler::SendQueue;
use std::collections::HashMap;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// ModuleHandler trait
// ---------------------------------------------------------------------------

/// A handler for a specific module's commands.
///
/// Implementations contain business logic and are transport-agnostic.
#[async_trait::async_trait]
pub trait ModuleHandler: Send + Sync {
    /// The module name this handler responds to (e.g., "models", "channels").
    fn module_name(&self) -> &str;

    /// L1（devtool-upgrade 阶段 6）：本模块的静态命令清单（单一真相源——
    /// 与 `handle_cmd` 的 `match cmd` 臂保持一致，运行时 debug warn 纠偏）。
    /// 默认空（渐进补齐）；只列**可枚举的字面量命令**——chat.send 这类走
    /// message 帧不经 request dispatch 的、以及守卫臂动态前缀（如
    /// `board.issue.*` 若改为前缀匹配）不进清单。
    fn commands(&self) -> &'static [&'static str] {
        &[]
    }

    /// Handle a command within this module.
    ///
    /// Returns `Ok(Some(data))` for success with payload, `Ok(None)` for success
    /// with no payload, or `Err(msg)` for failures.
    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String>;
}

// ---------------------------------------------------------------------------
// Request context
// ---------------------------------------------------------------------------

/// Context provided to each handler invocation.
#[derive(Clone)]
pub struct RequestContext {
    /// The WebSocket session ID.
    pub session_id: String,
    /// Chat ID derived from the WebSocket session (e.g. "web:{uuid}").
    /// Used by handlers that need to propagate chat identity to peer_chat
    /// or other downstream tasks that require per-conversation isolation.
    pub chat_id: String,
    /// Optional workspace path.
    pub workspace: Option<String>,
    /// Home directory where config.json resides.
    pub home: Option<String>,
    /// Shared application state.
    pub state: Arc<AppState>,
    /// How this session authenticated at WS upgrade time.
    ///
    /// Gates dashboard-only commands (e.g. `workflow.set_chat_password`)
    /// so a session that connected via the standalone workflow-chat page
    /// cannot mutate passwords.
    pub auth_method: AuthMethod,
}

// ---------------------------------------------------------------------------
// WsRouter
// ---------------------------------------------------------------------------

/// Router that dispatches `type="request"` messages to the appropriate module handler.
pub struct WsRouter {
    handlers: HashMap<String, Arc<dyn ModuleHandler>>,
}

impl WsRouter {
    /// Create a new empty router.
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }

    /// Register a module handler.
    pub fn register(&mut self, handler: Arc<dyn ModuleHandler>) {
        self.handlers
            .insert(handler.module_name().to_string(), handler);
    }

    /// L1：全 router 的命令注册表（module → 静态清单）。按 module 名排序——
    /// 内部是 HashMap，迭代顺序不定；不排序则 OnceLock 快照与文档每次漂移。
    /// `system.commands` 与文档生成的单一数据源。
    pub fn commands_registry(&self) -> Vec<(String, Vec<&'static str>)> {
        let mut out: Vec<(String, Vec<&'static str>)> = self
            .handlers
            .values()
            .map(|h| (h.module_name().to_string(), h.commands().to_vec()))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// Dispatch a request message to the appropriate handler and send the response.
    ///
    /// If no handler is found for the module, sends an error response.
    pub async fn dispatch(
        &self,
        msg: &ProtocolMessage,
        ctx: &RequestContext,
        send_queue: &SendQueue,
    ) {
        let req_id = msg.req_id.as_deref().unwrap_or("");

        let result = match self.handlers.get(&msg.module) {
            Some(handler) => {
                // L1 渐进纠偏：debug 构建下 dispatch 到清单外命令时 warn
                //（清单与 match 臂漂移的运行时信号；release 零开销）。
                #[cfg(debug_assertions)]
                {
                    let listed = handler.commands();
                    if !listed.is_empty() && !listed.contains(&msg.cmd.as_str()) {
                        tracing::warn!(
                            module = %msg.module,
                            cmd = %msg.cmd,
                            "[WSAPI] dispatched command not in static commands() list (list drift)"
                        );
                    }
                }
                handler.handle_cmd(&msg.cmd, msg.data.clone(), ctx).await
            }
            None => Err(format!("unknown module: {}", msg.module)),
        };

        let response = match result {
            Ok(data) => ProtocolMessage::response_ok(&msg.module, &msg.cmd, req_id, data),
            Err(e) => ProtocolMessage::response_err(&msg.module, &msg.cmd, req_id, &e),
        };

        if let Ok(bytes) = response.to_json()
            && let Err(e) = send_queue.send(bytes).await
        {
            tracing::warn!(
                req_id = %req_id,
                error = %e,
                "[WebSocket] Failed to send WS API response"
            );
        }
    }
}

impl Default for WsRouter {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(all(test, feature = "workflow"))]
mod tests;

// S10b (2026-08-26, quality-hardening goal 冲刺 web 批次 2): dispatch
// send-failure arm (dead SendQueue → warn, no panic).
#[cfg(test)]
mod s10b_tests;
