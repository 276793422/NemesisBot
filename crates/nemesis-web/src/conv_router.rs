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
//! reply still lands in history. Latest-wins keeps the table bounded (no
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
    pub fn bind(&self, session_key: &str, chat_id: &str) {
        if session_key.is_empty() || chat_id.is_empty() {
            return;
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
mod tests {
    use super::*;

    #[test]
    fn bind_and_target() {
        let r = ConvRouter::new();
        assert_eq!(r.target("agent:main:session:abc"), None);
        r.bind("agent:main:session:abc", "web:deadbeef");
        assert_eq!(r.target("agent:main:session:abc"), Some("web:deadbeef".to_string()));
    }

    #[test]
    fn latest_wins_overwrites() {
        let r = ConvRouter::new();
        r.bind("agent:main:session:abc", "web:1111");
        r.bind("agent:main:session:abc", "web:2222");
        assert_eq!(r.target("agent:main:session:abc"), Some("web:2222".to_string()));
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn empty_inputs_ignored() {
        let r = ConvRouter::new();
        r.bind("", "web:x");
        r.bind("agent:main:session:abc", "");
        assert!(r.is_empty());
    }

    #[test]
    fn distinct_conversations_independent() {
        let r = ConvRouter::new();
        r.bind("agent:main:session:a", "web:1");
        r.bind("agent:main:session:b", "web:2");
        assert_eq!(r.target("agent:main:session:a"), Some("web:1".to_string()));
        assert_eq!(r.target("agent:main:session:b"), Some("web:2".to_string()));
        assert_eq!(r.len(), 2);
    }
}
