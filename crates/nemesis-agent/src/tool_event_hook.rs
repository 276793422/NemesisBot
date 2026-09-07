//! 工具事件广播 hook（M1a，devtool-upgrade 阶段 1）。
//!
//! 实现 [`ToolHook`] 的 around 包装：进入链前发 [`AgentEvent::ToolStarted`]，
//! `next()` 返回后发 [`AgentEvent::ToolFinished`]——纯观察者，不改写结果、
//! 不拦截。事件经 `tokio::sync::broadcast` 通道交给 gateway 侧
//! `SharedResources.agent_event_tx`，`nemesis-web` 的 pump 订阅后路由到
//! Dashboard WS push（`{type:"push", cmd:"tool_event"}`）+ EventHub（SSE）。
//!
//! # 布点语义（诚实边界）
//!
//! around 链在 security 闸 / pre hooks **之后**构造——被安全层或 pre hook
//! 拦停的调用根本不进链，因此**不产生事件**（什么都没执行，如实沉默）。
//! executor 分离（RemoteExecutorTool）与 MCP 工具都走同一条
//! `handle_tool_call` 调度路径，事件自动覆盖。
//!
//! ok 判定沿用 Forge 的启发式（loop.rs:5805 附近先例）：结果不含
//! `SECURITY BLOCKED` 且不含 `Tool error:` 即视为成功。around 链只能看到
//! String 形态的最终结果，这是该层能拿到的全部信息。

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use nemesis_types::agent::AgentEvent;
use tokio::sync::broadcast;

use crate::hooks::{HookToolCall, ToolHook};

/// args 预览字符上限（事件负载防膨胀；中文等多字节字符按 char 截断）。
pub const ARGS_PREVIEW_CAP: usize = 200;
/// result 预览字符上限。
pub const RESULT_PREVIEW_CAP: usize = 1024;

/// 按字符数截断（byte 切片会在多字节字符边界 panic——见项目 str-slice 教训）。
fn truncate_chars(s: &str, cap: usize) -> String {
    if s.chars().count() <= cap {
        return s.to_string();
    }
    let mut out: String = s.chars().take(cap).collect();
    out.push('…');
    out
}

/// 把工具调度事件发布到 broadcast 通道的观察 hook。
pub struct ToolEventHook {
    tx: broadcast::Sender<AgentEvent>,
}

impl ToolEventHook {
    pub fn new(tx: broadcast::Sender<AgentEvent>) -> Self {
        Self { tx }
    }
}

#[async_trait]
impl ToolHook for ToolEventHook {
    fn name(&self) -> String {
        "tool-event".to_string()
    }

    async fn around_tool_use(
        &self,
        call: HookToolCall,
        next: Arc<
            dyn Fn(
                    HookToolCall,
                )
                    -> std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send>>
                + Send
                + Sync,
        >,
    ) -> String {
        let call_id = uuid::Uuid::new_v4().to_string();
        let session_key = call.session_key.clone();
        let chat_id = call.chat_id.clone();
        let tool = call.name.clone();

        // 无订阅者时 send 返回 Err——观察者通道空转是常态，静默忽略。
        let _ = self.tx.send(AgentEvent::ToolStarted {
            session_key: session_key.clone(),
            chat_id: chat_id.clone(),
            call_id: call_id.clone(),
            tool: tool.clone(),
            args_preview: truncate_chars(&call.arguments, ARGS_PREVIEW_CAP),
        });

        let start = Instant::now();
        let result = next(call).await;
        let duration_ms = start.elapsed().as_millis() as u64;

        // Forge 同款启发式（loop.rs）：around 层只见最终 String。
        let ok = !result.contains("SECURITY BLOCKED") && !result.contains("Tool error:");

        let _ = self.tx.send(AgentEvent::ToolFinished {
            session_key,
            chat_id,
            call_id,
            tool,
            duration_ms,
            ok,
            result_preview: truncate_chars(&result, RESULT_PREVIEW_CAP),
        });

        result
    }
}

#[cfg(test)]
mod tests;
