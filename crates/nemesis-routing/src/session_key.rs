//! Session key construction and parsing.

use std::collections::HashMap;

use crate::agent_id::normalize_agent_id;
use crate::agent_id::normalize_account_id;

// ---------------------------------------------------------------------------
// DM scope constants
// ---------------------------------------------------------------------------

/// Controls DM session isolation granularity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DMScope {
    /// All DMs collapse to the agent's main session key.
    Main,
    /// One session per peer ID (cross-channel).
    PerPeer,
    /// One session per (channel, peer) pair.
    PerChannelPeer,
    /// One session per (account, channel, peer) triple.
    PerAccountChannelPeer,
}

impl Default for DMScope {
    fn default() -> Self {
        DMScope::Main
    }
}

impl DMScope {
    /// Parse a DM scope from its string representation.
    pub fn from_str(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "per-peer" => DMScope::PerPeer,
            "per-channel-peer" => DMScope::PerChannelPeer,
            "per-account-channel-peer" => DMScope::PerAccountChannelPeer,
            _ => DMScope::Main,
        }
    }
}

// ---------------------------------------------------------------------------
// Peer descriptor
// ---------------------------------------------------------------------------

/// A peer in a routing context (user, group, channel, etc.).
#[derive(Debug, Clone, Default)]
pub struct RoutePeer {
    /// Kind: "direct", "group", "channel".
    pub kind: String,
    /// Unique ID of the peer.
    pub id: String,
}

// ---------------------------------------------------------------------------
// Session-key builder parameters
// ---------------------------------------------------------------------------

/// All inputs needed to build a session key.
#[derive(Debug, Clone, Default)]
pub struct SessionKeyParams {
    pub agent_id: String,
    pub channel: String,
    pub account_id: String,
    pub peer: Option<RoutePeer>,
    pub dm_scope: DMScope,
    /// Identity links map: canonical_name -> list of platform-specific IDs.
    pub identity_links: HashMap<String, Vec<String>>,
}

// ---------------------------------------------------------------------------
// Key builders
// ---------------------------------------------------------------------------

/// Build an agent-scoped main session key: `agent:<agentId>:main`.
pub fn build_agent_main_session_key(agent_id: &str) -> String {
    let agent = normalize_agent_id(agent_id);
    format!("agent:{}:main", agent).to_lowercase()
}

/// Build a simple agent-scoped session key from four components.
///
/// This is a convenience shorthand used by tests and simple callers.
/// For full DM-scope awareness use [`build_agent_peer_session_key`] instead.
pub fn build_agent_session_key(agent_id: &str, channel: &str, peer_kind: &str, peer_id: &str) -> String {
    let agent = normalize_agent_id(agent_id);
    let ch = normalize_channel(channel);
    let kind = if peer_kind.trim().is_empty() { "direct" } else { peer_kind.trim() };
    let pid = if peer_id.trim().is_empty() { "unknown" } else { peer_id.trim() };
    format!("agent:{}:{}:{}:{}", agent, ch, kind, pid).to_lowercase()
}

/// Build an agent peer session key based on agent, channel, peer, and DM scope.
///
/// This mirrors the Go `BuildAgentPeerSessionKey` function exactly:
///
/// * For **group/channel** peers: always per-peer => `agent:<agent>:<channel>:<peerKind>:<peerID>`
/// * For **direct** peers: behaviour depends on `dm_scope`:
///   - `Main` => `agent:<agent>:main`
///   - `PerPeer` => `agent:<agent>:direct:<peerID>`
///   - `PerChannelPeer` => `agent:<agent>:<channel>:direct:<peerID>`
///   - `PerAccountChannelPeer` => `agent:<agent>:<channel>:<account>:direct:<peerID>`
///
/// Identity links are resolved before key construction when the scope is
/// more granular than `Main`.
pub fn build_agent_peer_session_key(params: SessionKeyParams) -> String {
    let agent_id = normalize_agent_id(&params.agent_id);
    let peer = params.peer.unwrap_or(RoutePeer {
        kind: "direct".to_string(),
        id: String::new(),
    });
    let mut peer_kind = peer.kind.trim().to_string();
    if peer_kind.is_empty() {
        peer_kind = "direct".to_string();
    }
    let peer_kind_lower = peer_kind.to_lowercase();

    if peer_kind_lower == "direct" {
        let dm_scope = &params.dm_scope;
        let mut peer_id = peer.id.trim().to_string();

        // Resolve identity links (cross-platform collapse)
        if *dm_scope != DMScope::Main && !peer_id.is_empty() {
            if let Some(linked) = resolve_linked_peer_id(
                &params.identity_links,
                &params.channel,
                &peer_id,
            ) {
                peer_id = linked;
            }
        }
        let peer_id_lower = peer_id.to_lowercase();

        match dm_scope {
            DMScope::PerAccountChannelPeer => {
                if !peer_id_lower.is_empty() {
                    let ch = normalize_channel(&params.channel);
                    let acc = normalize_account_id(&params.account_id);
                    return format!("agent:{}:{}:{}:direct:{}", agent_id, ch, acc, peer_id_lower)
                        .to_lowercase();
                }
            }
            DMScope::PerChannelPeer => {
                if !peer_id_lower.is_empty() {
                    let ch = normalize_channel(&params.channel);
                    return format!("agent:{}:{}:direct:{}", agent_id, ch, peer_id_lower)
                        .to_lowercase();
                }
            }
            DMScope::PerPeer => {
                if !peer_id_lower.is_empty() {
                    return format!("agent:{}:direct:{}", agent_id, peer_id_lower).to_lowercase();
                }
            }
            DMScope::Main => {}
        }
        return build_agent_main_session_key(&params.agent_id);
    }

    // Group/channel peers always get per-peer sessions
    let ch = normalize_channel(&params.channel);
    let peer_id = {
        let pid = peer.id.trim().to_lowercase();
        if pid.is_empty() {
            "unknown".to_string()
        } else {
            pid
        }
    };
    format!("agent:{}:{}:{}:{}", agent_id, ch, peer_kind_lower, peer_id).to_lowercase()
}

// ---------------------------------------------------------------------------
// Key parser
// ---------------------------------------------------------------------------

/// Parse an agent session key into `(agent_id, rest)`.
pub fn parse_agent_session_key(session_key: &str) -> Option<(String, String)> {
    let raw = session_key.trim();
    if raw.is_empty() {
        return None;
    }
    let parts: Vec<&str> = raw.splitn(3, ':').collect();
    if parts.len() < 3 || parts[0] != "agent" {
        return None;
    }
    let agent_id = parts[1].to_string();
    let rest = parts[2].to_string();
    if agent_id.is_empty() || rest.is_empty() {
        return None;
    }
    Some((agent_id, rest))
}

/// Check if a session key represents a subagent.
pub fn is_subagent_session_key(session_key: &str) -> bool {
    let lower = session_key.trim().to_lowercase();
    if lower.starts_with("subagent:") {
        return true;
    }
    if let Some((_, rest)) = parse_agent_session_key(session_key) {
        return rest.to_lowercase().starts_with("subagent:");
    }
    false
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Normalize a channel name (empty => "unknown").
fn normalize_channel(channel: &str) -> String {
    let c = channel.trim().to_lowercase();
    if c.is_empty() { "unknown".to_string() } else { c }
}

/// Resolve a peer ID through identity links.
///
/// Searches for the peer ID (both bare and `channel:peerID` forms) across
/// all identity link entries. If a match is found the canonical name is
/// returned, collapsing cross-platform identities.
fn resolve_linked_peer_id(
    identity_links: &HashMap<String, Vec<String>>,
    channel: &str,
    peer_id: &str,
) -> Option<String> {
    if identity_links.is_empty() {
        return None;
    }
    let peer_id = peer_id.trim();
    if peer_id.is_empty() {
        return None;
    }

    // Build candidate set
    let mut candidates: Vec<String> = Vec::new();
    let lower_peer = peer_id.to_lowercase();
    candidates.push(lower_peer.clone());

    let channel_lower = channel.trim().to_lowercase();
    if !channel_lower.is_empty() {
        candidates.push(format!("{}:{}", channel_lower, lower_peer));
    }

    // Search links
    for (canonical, ids) in identity_links {
        let canonical_name = canonical.trim().to_string();
        if canonical_name.is_empty() {
            continue;
        }
        for id in ids {
            let normalized = id.trim().to_lowercase();
            if !normalized.is_empty() && candidates.contains(&normalized) {
                return Some(canonical_name);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_session_key() {
        let key = build_agent_session_key("main", "web", "direct", "user1");
        assert_eq!(key, "agent:main:web:direct:user1");
    }

    #[test]
    fn test_build_session_key_empty() {
        let key = build_agent_session_key("main", "", "", "");
        assert_eq!(key, "agent:main:unknown:direct:unknown");
    }

    #[test]
    fn test_build_main_session_key() {
        let key = build_agent_main_session_key("main");
        assert_eq!(key, "agent:main:main");
    }

    #[test]
    fn test_parse_session_key() {
        let (agent, rest) = parse_agent_session_key("agent:main:web:direct:user1").unwrap();
        assert_eq!(agent, "main");
        assert_eq!(rest, "web:direct:user1");
    }

    #[test]
    fn test_parse_invalid() {
        assert!(parse_agent_session_key("").is_none());
        assert!(parse_agent_session_key("invalid").is_none());
        assert!(parse_agent_session_key("agent:").is_none());
    }

    #[test]
    fn test_is_subagent() {
        assert!(is_subagent_session_key("subagent:123"));
        assert!(is_subagent_session_key("agent:main:subagent:456"));
        assert!(!is_subagent_session_key("agent:main:web:direct:user1"));
    }

    // ---- build_agent_peer_session_key tests ----

    #[test]
    fn test_peer_session_key_group_peer() {
        let params = SessionKeyParams {
            agent_id: "main".into(),
            channel: "discord".into(),
            account_id: "default".into(),
            peer: Some(RoutePeer {
                kind: "group".into(),
                id: "guild123".into(),
            }),
            dm_scope: DMScope::Main,
            identity_links: HashMap::new(),
        };
        let key = build_agent_peer_session_key(params);
        assert_eq!(key, "agent:main:discord:group:guild123");
    }

    #[test]
    fn test_peer_session_key_direct_main_scope() {
        let params = SessionKeyParams {
            agent_id: "main".into(),
            channel: "web".into(),
            account_id: "default".into(),
            peer: Some(RoutePeer {
                kind: "direct".into(),
                id: "user42".into(),
            }),
            dm_scope: DMScope::Main,
            identity_links: HashMap::new(),
        };
        let key = build_agent_peer_session_key(params);
        // Main scope collapses to main session key
        assert_eq!(key, "agent:main:main");
    }

    #[test]
    fn test_peer_session_key_direct_per_peer() {
        let params = SessionKeyParams {
            agent_id: "main".into(),
            channel: "web".into(),
            account_id: "default".into(),
            peer: Some(RoutePeer {
                kind: "direct".into(),
                id: "user42".into(),
            }),
            dm_scope: DMScope::PerPeer,
            identity_links: HashMap::new(),
        };
        let key = build_agent_peer_session_key(params);
        assert_eq!(key, "agent:main:direct:user42");
    }

    #[test]
    fn test_peer_session_key_direct_per_channel_peer() {
        let params = SessionKeyParams {
            agent_id: "main".into(),
            channel: "discord".into(),
            account_id: "default".into(),
            peer: Some(RoutePeer {
                kind: "direct".into(),
                id: "user42".into(),
            }),
            dm_scope: DMScope::PerChannelPeer,
            identity_links: HashMap::new(),
        };
        let key = build_agent_peer_session_key(params);
        assert_eq!(key, "agent:main:discord:direct:user42");
    }

    #[test]
    fn test_peer_session_key_direct_per_account_channel_peer() {
        let params = SessionKeyParams {
            agent_id: "main".into(),
            channel: "discord".into(),
            account_id: "myaccount".into(),
            peer: Some(RoutePeer {
                kind: "direct".into(),
                id: "user42".into(),
            }),
            dm_scope: DMScope::PerAccountChannelPeer,
            identity_links: HashMap::new(),
        };
        let key = build_agent_peer_session_key(params);
        assert_eq!(key, "agent:main:discord:myaccount:direct:user42");
    }

    #[test]
    fn test_peer_session_key_direct_no_peer_id() {
        let params = SessionKeyParams {
            agent_id: "main".into(),
            channel: "web".into(),
            account_id: "default".into(),
            peer: Some(RoutePeer {
                kind: "direct".into(),
                id: String::new(),
            }),
            dm_scope: DMScope::PerPeer,
            identity_links: HashMap::new(),
        };
        let key = build_agent_peer_session_key(params);
        // No peer ID => falls back to main key
        assert_eq!(key, "agent:main:main");
    }

    #[test]
    fn test_peer_session_key_no_peer() {
        let params = SessionKeyParams {
            agent_id: "main".into(),
            channel: "web".into(),
            account_id: "default".into(),
            peer: None,
            dm_scope: DMScope::Main,
            identity_links: HashMap::new(),
        };
        let key = build_agent_peer_session_key(params);
        assert_eq!(key, "agent:main:main");
    }

    #[test]
    fn test_identity_links_resolution() {
        let mut links = HashMap::new();
        links.insert(
            "alice".to_string(),
            vec!["discord:alice_d".to_string(), "slack:alice_s".to_string()],
        );

        let params = SessionKeyParams {
            agent_id: "main".into(),
            channel: "discord".into(),
            account_id: "default".into(),
            peer: Some(RoutePeer {
                kind: "direct".into(),
                id: "alice_d".into(),
            }),
            dm_scope: DMScope::PerChannelPeer,
            identity_links: links,
        };
        let key = build_agent_peer_session_key(params);
        // The peer ID "alice_d" should be resolved to "alice" via the
        // "discord:alice_d" entry in identity links.
        assert_eq!(key, "agent:main:discord:direct:alice");
    }

    #[test]
    fn test_dm_scope_from_str() {
        assert_eq!(DMScope::from_str("main"), DMScope::Main);
        assert_eq!(DMScope::from_str("per-peer"), DMScope::PerPeer);
        assert_eq!(DMScope::from_str("per-channel-peer"), DMScope::PerChannelPeer);
        assert_eq!(DMScope::from_str("per-account-channel-peer"), DMScope::PerAccountChannelPeer);
        assert_eq!(DMScope::from_str(""), DMScope::Main);
        assert_eq!(DMScope::from_str("unknown"), DMScope::Main);
    }
}
