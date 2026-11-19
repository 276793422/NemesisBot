//! Conversation → WebSocket connection router for proactive (cron-initiated)
//! delivery to a specific agent conversation.
//!
//! ## Why this exists
//! The web channel routes outbound strictly by `chat_id = web:<ws_conn_id>`
//! (`crates/nemesis-channels/src/web.rs`), where `<ws_conn_id>` is the
//! server-generated WS connection id minted at upgrade — **unrelated** to the
//! agent `session_key` (`agent:main:session:<conv_id>`), and there is no stored
//! mapping between the two. So a cron job that targets a conversation by
//! `session_key` could persist its reply into the conversation history (the
//! agent writes back by `session_key`) but could NOT live-push it to the open
//! browser tab.
//!
//! ## How it works
//! On every web inbound we already have both the derived `session_key` and the
//! `chat_id` (`web:<ws_conn_id>`) — that pair IS the mapping. `ConvRouter`
//! records the latest binding `session_key → chat_id` so the cron fire handler
//! can populate `InboundMessage.chat_id` and get a real-time push when the tab
//! is open. If the tab is closed/dead, `send_to_session` fails soft and the
//! reply still lands in history. Latest-wins keeps per-conversation entries at
//! one; the table itself is bounded by [`ConvRouter::MAX_BINDINGS`] (no
//! disconnect hook required).
//!
//! ## v1 limitation
//! Latest-wins means only the most-recently-active tab for a conversation
//! receives the live push; a simultaneously-open second tab won't. Full
//! multi-tab fan-out would require outbound broadcast-per-target (future).

use dashmap::DashMap;
use std::sync::Arc;

/// Maps an agent conversation key to the latest live web `chat_id`
/// (`web:<ws_conn_id>`). Share via `Arc<ConvRouter>` (`SharedConvRouter`).
#[derive(Debug)]
pub struct ConvRouter {
    conv_to_chat: DashMap<String, String>,
}

/// Shared handle to a [`ConvRouter`].
pub type SharedConvRouter = Arc<ConvRouter>;

/// 绑定表条目上限（BUG-20261116-020）：key 空间 = session_key 全史（每条
/// web 入站都会 `bind` 一次），latest-wins 只限 per-key 条目数、不限 key
/// 数——24/7 网关下按会话数无界累积（BUG-20261113-170 同族；条目是死
/// 绑定，指向早已关闭的 ws conn id）。超限整体清空（与 effort_overrides /
/// `SESSION_STATE_MAX_ENTRIES` 同纪律）：bind 每次入站即重绑，活跃会话的
/// 下一条消息自愈；清空窗口内 cron 实时推送回落历史投递（软失败语义既有）。
pub const MAX_BINDINGS: usize = 4096;

impl ConvRouter {
    /// Create an empty router.
    pub fn new() -> Self {
        Self {
            conv_to_chat: DashMap::new(),
        }
    }

    /// Record that conversation `session_key` is currently reachable via
    /// `chat_id` (a `web:<ws_conn_id>` value). Latest-wins: a later bind for
    /// the same conversation overwrites. Called on every web inbound.
    ///
    /// 有界纪律（BUG-20261116-020）：新 key 且表已满 [`MAX_BINDINGS`] 时
    /// 整体清空再插入——丢失的绑定在各自下一条入站消息重绑自愈；窗口内
    /// cron 推送回落历史投递。同 key 重绑走 `contains_key` 短路，永不触发
    /// 清空（活跃会话互不驱逐）。
    pub fn bind(&self, session_key: &str, chat_id: &str) {
        if session_key.is_empty() || chat_id.is_empty() {
            return;
        }
        if !self.conv_to_chat.contains_key(session_key) && self.conv_to_chat.len() >= MAX_BINDINGS {
            self.conv_to_chat.clear();
        }
        self.conv_to_chat
            .insert(session_key.to_string(), chat_id.to_string());
    }

    /// Look up the live `chat_id` bound to a conversation, if any. The cron
    /// fire handler uses this to populate `InboundMessage.chat_id` for a
    /// real-time push. `None` → fall back to history-only delivery.
    pub fn target(&self, session_key: &str) -> Option<String> {
        self.conv_to_chat.get(session_key).map(|v| v.clone())
    }

    /// Number of bound conversations (diagnostics / tests).
    pub fn len(&self) -> usize {
        self.conv_to_chat.len()
    }

    /// Whether any conversation is bound.
    pub fn is_empty(&self) -> bool {
        self.conv_to_chat.is_empty()
    }
}

impl Default for ConvRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;

// (, quality-hardening goal 冲刺 web): Default impl.
#[cfg(test)]
mod s10b_tests;
