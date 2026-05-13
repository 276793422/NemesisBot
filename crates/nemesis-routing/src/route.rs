//! Route resolver for agent dispatch.

use crate::agent_id::{normalize_account_id, normalize_agent_id, DEFAULT_ACCOUNT_ID, DEFAULT_AGENT_ID};
use crate::session_key::{build_agent_main_session_key, build_agent_peer_session_key, DMScope, RoutePeer, SessionKeyParams};
use std::collections::HashMap;

/// Route input from an inbound message.
#[derive(Debug, Clone)]
pub struct RouteInput {
    pub channel: String,
    pub account_id: String,
    pub peer_kind: Option<String>,
    pub peer_id: Option<String>,
    pub parent_peer_kind: Option<String>,
    pub parent_peer_id: Option<String>,
    pub guild_id: Option<String>,
    pub team_id: Option<String>,
    /// Identity links map: canonical_name -> list of platform-specific IDs.
    pub identity_links: HashMap<String, Vec<String>>,
}

/// Resolved route result.
#[derive(Debug, Clone)]
pub struct ResolvedRoute {
    pub agent_id: String,
    pub channel: String,
    pub account_id: String,
    pub session_key: String,
    pub main_session_key: String,
    pub matched_by: String,
}

/// Agent binding configuration.
#[derive(Debug, Clone)]
pub struct AgentBinding {
    pub agent_id: String,
    pub match_channel: String,
    pub match_account_id: String,
    pub match_peer_kind: Option<String>,
    pub match_peer_id: Option<String>,
    pub match_guild_id: Option<String>,
    pub match_team_id: Option<String>,
}

/// Agent definition.
#[derive(Debug, Clone)]
pub struct AgentDef {
    pub id: String,
    pub is_default: bool,
}

/// Routing configuration.
#[derive(Debug, Clone)]
pub struct RouteConfig {
    pub bindings: Vec<AgentBinding>,
    pub agents: Vec<AgentDef>,
    pub dm_scope: String,
}

/// Route resolver determines which agent handles a message.
///
/// Implements a 7-level priority cascade matching the Go implementation:
/// 1. Peer binding
/// 2. Parent peer binding
/// 3. Guild binding
/// 4. Team binding
/// 5. Account binding
/// 6. Channel wildcard binding (account_id = "*")
/// 7. Default agent
pub struct RouteResolver {
    config: RouteConfig,
}

impl RouteResolver {
    pub fn new(config: RouteConfig) -> Self {
        Self { config }
    }

    /// Resolve the route for an inbound message.
    pub fn resolve(&self, input: &RouteInput) -> ResolvedRoute {
        let channel = input.channel.trim().to_lowercase();
        let account_id = normalize_account_id(&input.account_id);

        // Pre-filter bindings to only those matching channel and account
        let bindings = self.filter_bindings(&channel, &account_id);

        // Priority 1: Peer binding
        if let (Some(kind), Some(id)) = (&input.peer_kind, &input.peer_id) {
            let kind_trimmed = kind.trim();
            let id_trimmed = id.trim();
            if !kind_trimmed.is_empty() && !id_trimmed.is_empty() {
                if let Some(b) = self.find_peer_match(&bindings, kind_trimmed, id_trimmed) {
                    return self.build_route(&b.agent_id, &channel, &account_id, input, "binding.peer");
                }
            }
        }

        // Priority 2: Parent peer binding
        if let (Some(kind), Some(id)) = (&input.parent_peer_kind, &input.parent_peer_id) {
            let kind_trimmed = kind.trim();
            let id_trimmed = id.trim();
            if !kind_trimmed.is_empty() && !id_trimmed.is_empty() {
                if let Some(b) = self.find_peer_match(&bindings, kind_trimmed, id_trimmed) {
                    return self.build_route(&b.agent_id, &channel, &account_id, input, "binding.peer.parent");
                }
            }
        }

        // Priority 3: Guild binding
        if let Some(guild_id) = &input.guild_id {
            let guild_trimmed = guild_id.trim();
            if !guild_trimmed.is_empty() {
                if let Some(b) = self.find_guild_match(&bindings, guild_trimmed) {
                    return self.build_route(&b.agent_id, &channel, &account_id, input, "binding.guild");
                }
            }
        }

        // Priority 4: Team binding
        if let Some(team_id) = &input.team_id {
            let team_trimmed = team_id.trim();
            if !team_trimmed.is_empty() {
                if let Some(b) = self.find_team_match(&bindings, team_trimmed) {
                    return self.build_route(&b.agent_id, &channel, &account_id, input, "binding.team");
                }
            }
        }

        // Priority 5: Account binding (specific account_id, no peer/guild/team)
        if let Some(b) = self.find_account_match(&bindings) {
            return self.build_route(&b.agent_id, &channel, &account_id, input, "binding.account");
        }

        // Priority 6: Channel wildcard binding (account_id = "*", no peer/guild/team)
        if let Some(b) = self.find_channel_wildcard_match(&bindings) {
            return self.build_route(&b.agent_id, &channel, &account_id, input, "binding.channel");
        }

        // Priority 7: Default agent
        let default_agent = self.resolve_default_agent();
        self.build_route(&default_agent, &channel, &account_id, input, "default")
    }

    // -----------------------------------------------------------------------
    // Filtering helpers
    // -----------------------------------------------------------------------

    /// Filter bindings to those matching the given channel and account ID.
    fn filter_bindings<'a>(
        &'a self,
        channel: &str,
        account_id: &str,
    ) -> Vec<&'a AgentBinding> {
        self.config
            .bindings
            .iter()
            .filter(|b| {
                let match_ch = b.match_channel.trim().to_lowercase();
                if match_ch != channel {
                    return false;
                }
                matches_account_id(&b.match_account_id, account_id)
            })
            .collect()
    }

    /// Find a binding that matches a peer's kind and ID.
    fn find_peer_match<'a>(
        &self,
        bindings: &[&'a AgentBinding],
        peer_kind: &str,
        peer_id: &str,
    ) -> Option<&'a AgentBinding> {
        for b in bindings {
            let bk = match &b.match_peer_kind {
                Some(k) => k.trim().to_lowercase(),
                None => continue,
            };
            let bid = match &b.match_peer_id {
                Some(id) => id.trim().to_string(),
                None => continue,
            };
            if bk.is_empty() || bid.is_empty() {
                continue;
            }
            if bk == peer_kind.to_lowercase() && bid == peer_id {
                return Some(*b);
            }
        }
        None
    }

    /// Find a binding that matches a guild ID.
    fn find_guild_match<'a>(
        &self,
        bindings: &[&'a AgentBinding],
        guild_id: &str,
    ) -> Option<&'a AgentBinding> {
        for b in bindings {
            match &b.match_guild_id {
                Some(g) => {
                    let g = g.trim();
                    if !g.is_empty() && g == guild_id {
                        return Some(*b);
                    }
                }
                None => {}
            }
        }
        None
    }

    /// Find a binding that matches a team ID.
    fn find_team_match<'a>(
        &self,
        bindings: &[&'a AgentBinding],
        team_id: &str,
    ) -> Option<&'a AgentBinding> {
        for b in bindings {
            match &b.match_team_id {
                Some(t) => {
                    let t = t.trim();
                    if !t.is_empty() && t == team_id {
                        return Some(*b);
                    }
                }
                None => {}
            }
        }
        None
    }

    /// Find a binding that matches by account only (no peer/guild/team, account != "*").
    fn find_account_match<'a>(
        &self,
        bindings: &[&'a AgentBinding],
    ) -> Option<&'a AgentBinding> {
        for b in bindings {
            let acc = b.match_account_id.trim();
            if acc == "*" {
                continue;
            }
            // Must not have peer, guild, or team constraints
            if b.match_peer_kind.is_some()
                || b.match_peer_id.is_some()
                || b.match_guild_id.is_some()
                || b.match_team_id.is_some()
            {
                continue;
            }
            return Some(*b);
        }
        None
    }

    /// Find a channel-wildcard binding (account_id = "*", no peer/guild/team).
    fn find_channel_wildcard_match<'a>(
        &self,
        bindings: &[&'a AgentBinding],
    ) -> Option<&'a AgentBinding> {
        for b in bindings {
            let acc = b.match_account_id.trim();
            if acc != "*" {
                continue;
            }
            if b.match_peer_kind.is_some()
                || b.match_peer_id.is_some()
                || b.match_guild_id.is_some()
                || b.match_team_id.is_some()
            {
                continue;
            }
            return Some(*b);
        }
        None
    }

    // -----------------------------------------------------------------------
    // Route construction
    // -----------------------------------------------------------------------

    fn build_route(
        &self,
        agent_id: &str,
        channel: &str,
        account_id: &str,
        input: &RouteInput,
        matched_by: &str,
    ) -> ResolvedRoute {
        let resolved_id = self.pick_agent(agent_id);

        let peer_kind = input
            .peer_kind
            .as_deref()
            .map(|k| k.trim())
            .filter(|k| !k.is_empty())
            .unwrap_or("direct");
        let peer_id = input
            .peer_id
            .as_deref()
            .map(|id| id.trim())
            .filter(|id| !id.is_empty())
            .unwrap_or("unknown");

        // Use the full session key builder with DM scope and identity links,
        // matching Go's inner `choose` closure that calls BuildAgentPeerSessionKey.
        let dm_scope = DMScope::from_str(&self.config.dm_scope);
        let params = SessionKeyParams {
            agent_id: resolved_id.clone(),
            channel: channel.to_string(),
            account_id: account_id.to_string(),
            peer: Some(RoutePeer {
                kind: peer_kind.to_string(),
                id: peer_id.to_string(),
            }),
            dm_scope,
            identity_links: input.identity_links.clone(),
        };
        let session_key = build_agent_peer_session_key(params);
        let main_session_key = build_agent_main_session_key(&resolved_id);

        ResolvedRoute {
            agent_id: resolved_id,
            channel: channel.to_string(),
            account_id: account_id.to_string(),
            session_key,
            main_session_key,
            matched_by: matched_by.to_string(),
        }
    }

    fn pick_agent(&self, agent_id: &str) -> String {
        let normalized = normalize_agent_id(agent_id);
        if self.config.agents.is_empty() {
            return normalized;
        }
        for a in &self.config.agents {
            if normalize_agent_id(&a.id) == normalized {
                return normalized;
            }
        }
        self.resolve_default_agent()
    }

    fn resolve_default_agent(&self) -> String {
        for a in &self.config.agents {
            if a.is_default {
                let id = a.id.trim();
                if !id.is_empty() {
                    return normalize_agent_id(id);
                }
            }
        }
        if let Some(a) = self.config.agents.first() {
            let id = a.id.trim();
            if !id.is_empty() {
                return normalize_agent_id(id);
            }
        }
        DEFAULT_AGENT_ID.to_string()
    }
}

// ---------------------------------------------------------------------------
// Account matching helper
// ---------------------------------------------------------------------------

/// Check whether a binding's match_account_id field is compatible with the
/// actual account ID. Mirrors the Go `matchesAccountID` function.
fn matches_account_id(match_account_id: &str, actual: &str) -> bool {
    let trimmed = match_account_id.trim();
    if trimmed.is_empty() {
        return actual == DEFAULT_ACCOUNT_ID;
    }
    if trimmed == "*" {
        return true;
    }
    trimmed.to_lowercase() == actual.to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_route() {
        let resolver = RouteResolver::new(RouteConfig {
            bindings: vec![],
            agents: vec![AgentDef { id: "main".to_string(), is_default: true }],
            dm_scope: "main".to_string(),
        });
        let input = RouteInput {
            channel: "web".to_string(),
            account_id: "default".to_string(),
            peer_kind: Some("direct".to_string()),
            peer_id: Some("user1".to_string()),
            parent_peer_kind: None,
            parent_peer_id: None,
            guild_id: None,
            team_id: None,
            identity_links: HashMap::new(),
        };
        let route = resolver.resolve(&input);
        assert_eq!(route.agent_id, "main");
        assert_eq!(route.matched_by, "default");
    }

    #[test]
    fn test_peer_binding() {
        let resolver = RouteResolver::new(RouteConfig {
            bindings: vec![AgentBinding {
                agent_id: "special".to_string(),
                match_channel: "web".to_string(),
                match_account_id: "*".to_string(),
                match_peer_kind: Some("direct".to_string()),
                match_peer_id: Some("vip-user".to_string()),
                match_guild_id: None,
                match_team_id: None,
            }],
            agents: vec![
                AgentDef { id: "main".to_string(), is_default: true },
                AgentDef { id: "special".to_string(), is_default: false },
            ],
            dm_scope: "main".to_string(),
        });
        let input = RouteInput {
            channel: "web".to_string(),
            account_id: "default".to_string(),
            peer_kind: Some("direct".to_string()),
            peer_id: Some("vip-user".to_string()),
            parent_peer_kind: None,
            parent_peer_id: None,
            guild_id: None,
            team_id: None,
            identity_links: HashMap::new(),
        };
        let route = resolver.resolve(&input);
        assert_eq!(route.agent_id, "special");
        assert_eq!(route.matched_by, "binding.peer");
    }

    #[test]
    fn test_guild_binding() {
        let resolver = RouteResolver::new(RouteConfig {
            bindings: vec![AgentBinding {
                agent_id: "guild-agent".to_string(),
                match_channel: "discord".to_string(),
                match_account_id: "*".to_string(),
                match_peer_kind: None,
                match_peer_id: None,
                match_guild_id: Some("guild-123".to_string()),
                match_team_id: None,
            }],
            agents: vec![
                AgentDef { id: "main".to_string(), is_default: true },
                AgentDef { id: "guild-agent".to_string(), is_default: false },
            ],
            dm_scope: "main".to_string(),
        });
        let input = RouteInput {
            channel: "discord".to_string(),
            account_id: "default".to_string(),
            peer_kind: None,
            peer_id: None,
            parent_peer_kind: None,
            parent_peer_id: None,
            guild_id: Some("guild-123".to_string()),
            team_id: None,
            identity_links: HashMap::new(),
        };
        let route = resolver.resolve(&input);
        assert_eq!(route.agent_id, "guild-agent");
        assert_eq!(route.matched_by, "binding.guild");
    }

    #[test]
    fn test_team_binding() {
        let resolver = RouteResolver::new(RouteConfig {
            bindings: vec![AgentBinding {
                agent_id: "team-agent".to_string(),
                match_channel: "slack".to_string(),
                match_account_id: "*".to_string(),
                match_peer_kind: None,
                match_peer_id: None,
                match_guild_id: None,
                match_team_id: Some("team-456".to_string()),
            }],
            agents: vec![
                AgentDef { id: "main".to_string(), is_default: true },
                AgentDef { id: "team-agent".to_string(), is_default: false },
            ],
            dm_scope: "main".to_string(),
        });
        let input = RouteInput {
            channel: "slack".to_string(),
            account_id: "default".to_string(),
            peer_kind: None,
            peer_id: None,
            parent_peer_kind: None,
            parent_peer_id: None,
            guild_id: None,
            team_id: Some("team-456".to_string()),
            identity_links: HashMap::new(),
        };
        let route = resolver.resolve(&input);
        assert_eq!(route.agent_id, "team-agent");
        assert_eq!(route.matched_by, "binding.team");
    }

    #[test]
    fn test_account_binding() {
        let resolver = RouteResolver::new(RouteConfig {
            bindings: vec![AgentBinding {
                agent_id: "vip-agent".to_string(),
                match_channel: "web".to_string(),
                match_account_id: "vip-account".to_string(),
                match_peer_kind: None,
                match_peer_id: None,
                match_guild_id: None,
                match_team_id: None,
            }],
            agents: vec![
                AgentDef { id: "main".to_string(), is_default: true },
                AgentDef { id: "vip-agent".to_string(), is_default: false },
            ],
            dm_scope: "main".to_string(),
        });
        let input = RouteInput {
            channel: "web".to_string(),
            account_id: "vip-account".to_string(),
            peer_kind: None,
            peer_id: None,
            parent_peer_kind: None,
            parent_peer_id: None,
            guild_id: None,
            team_id: None,
            identity_links: HashMap::new(),
        };
        let route = resolver.resolve(&input);
        assert_eq!(route.agent_id, "vip-agent");
        assert_eq!(route.matched_by, "binding.account");
    }

    #[test]
    fn test_channel_wildcard_binding() {
        let resolver = RouteResolver::new(RouteConfig {
            bindings: vec![AgentBinding {
                agent_id: "catch-all".to_string(),
                match_channel: "web".to_string(),
                match_account_id: "*".to_string(),
                match_peer_kind: None,
                match_peer_id: None,
                match_guild_id: None,
                match_team_id: None,
            }],
            agents: vec![
                AgentDef { id: "main".to_string(), is_default: true },
                AgentDef { id: "catch-all".to_string(), is_default: false },
            ],
            dm_scope: "main".to_string(),
        });
        let input = RouteInput {
            channel: "web".to_string(),
            account_id: "some-account".to_string(),
            peer_kind: None,
            peer_id: None,
            parent_peer_kind: None,
            parent_peer_id: None,
            guild_id: None,
            team_id: None,
            identity_links: HashMap::new(),
        };
        let route = resolver.resolve(&input);
        assert_eq!(route.agent_id, "catch-all");
        assert_eq!(route.matched_by, "binding.channel");
    }

    #[test]
    fn test_parent_peer_binding() {
        let resolver = RouteResolver::new(RouteConfig {
            bindings: vec![AgentBinding {
                agent_id: "parent-agent".to_string(),
                match_channel: "discord".to_string(),
                match_account_id: "*".to_string(),
                match_peer_kind: Some("group".to_string()),
                match_peer_id: Some("parent-group".to_string()),
                match_guild_id: None,
                match_team_id: None,
            }],
            agents: vec![
                AgentDef { id: "main".to_string(), is_default: true },
                AgentDef { id: "parent-agent".to_string(), is_default: false },
            ],
            dm_scope: "main".to_string(),
        });
        let input = RouteInput {
            channel: "discord".to_string(),
            account_id: "default".to_string(),
            peer_kind: Some("direct".to_string()),
            peer_id: Some("user1".to_string()),
            parent_peer_kind: Some("group".to_string()),
            parent_peer_id: Some("parent-group".to_string()),
            guild_id: None,
            team_id: None,
            identity_links: HashMap::new(),
        };
        let route = resolver.resolve(&input);
        // Peer match should fail (no binding for direct/user1), but parent peer should match
        assert_eq!(route.agent_id, "parent-agent");
        assert_eq!(route.matched_by, "binding.peer.parent");
    }

    #[test]
    fn test_matches_account_id() {
        assert!(matches_account_id("*", "anyone"));
        assert!(matches_account_id("", "default"));
        assert!(matches_account_id("myaccount", "MyAccount"));
        assert!(!matches_account_id("other", "myaccount"));
        assert!(!matches_account_id("", "non-default"));
    }

    #[test]
    fn test_resolve_same_priority_first_match() {
        // Two bindings at the same priority (account level), first should win
        let resolver = RouteResolver::new(RouteConfig {
            bindings: vec![
                AgentBinding {
                    agent_id: "first-agent".to_string(),
                    match_channel: "web".to_string(),
                    match_account_id: "shared-account".to_string(),
                    match_peer_kind: None,
                    match_peer_id: None,
                    match_guild_id: None,
                    match_team_id: None,
                },
                AgentBinding {
                    agent_id: "second-agent".to_string(),
                    match_channel: "web".to_string(),
                    match_account_id: "shared-account".to_string(),
                    match_peer_kind: None,
                    match_peer_id: None,
                    match_guild_id: None,
                    match_team_id: None,
                },
            ],
            agents: vec![
                AgentDef { id: "first-agent".to_string(), is_default: true },
                AgentDef { id: "second-agent".to_string(), is_default: false },
            ],
            dm_scope: "main".to_string(),
        });
        let input = RouteInput {
            channel: "web".to_string(),
            account_id: "shared-account".to_string(),
            peer_kind: None,
            peer_id: None,
            parent_peer_kind: None,
            parent_peer_id: None,
            guild_id: None,
            team_id: None,
            identity_links: HashMap::new(),
        };
        let route = resolver.resolve(&input);
        assert_eq!(route.agent_id, "first-agent");
        assert_eq!(route.matched_by, "binding.account");
    }

    #[test]
    fn test_pick_agent_unknown_falls_back_to_default() {
        let resolver = RouteResolver::new(RouteConfig {
            bindings: vec![AgentBinding {
                agent_id: "unknown-agent".to_string(),
                match_channel: "web".to_string(),
                match_account_id: "*".to_string(),
                match_peer_kind: None,
                match_peer_id: None,
                match_guild_id: None,
                match_team_id: None,
            }],
            agents: vec![AgentDef { id: "main".to_string(), is_default: true }],
            dm_scope: "main".to_string(),
        });
        let input = RouteInput {
            channel: "web".to_string(),
            account_id: "default".to_string(),
            peer_kind: None,
            peer_id: None,
            parent_peer_kind: None,
            parent_peer_id: None,
            guild_id: None,
            team_id: None,
            identity_links: HashMap::new(),
        };
        let route = resolver.resolve(&input);
        // "unknown-agent" not in agents list, should fall back to default "main"
        assert_eq!(route.agent_id, "main");
    }

    #[test]
    fn test_find_peer_match_empty_kind_or_id() {
        let resolver = RouteResolver::new(RouteConfig {
            bindings: vec![AgentBinding {
                agent_id: "peer-agent".to_string(),
                match_channel: "web".to_string(),
                match_account_id: "*".to_string(),
                match_peer_kind: Some(String::new()),  // empty kind
                match_peer_id: Some("user1".to_string()),
                match_guild_id: None,
                match_team_id: None,
            }],
            agents: vec![AgentDef { id: "main".to_string(), is_default: true }],
            dm_scope: "main".to_string(),
        });
        let input = RouteInput {
            channel: "web".to_string(),
            account_id: "default".to_string(),
            peer_kind: Some(String::new()),
            peer_id: Some("user1".to_string()),
            parent_peer_kind: None,
            parent_peer_id: None,
            guild_id: None,
            team_id: None,
            identity_links: HashMap::new(),
        };
        let route = resolver.resolve(&input);
        // Empty kind/id in input should not match peer binding
        assert_eq!(route.matched_by, "default");
    }

    #[test]
    fn test_filter_bindings_non_matching_channel() {
        let resolver = RouteResolver::new(RouteConfig {
            bindings: vec![AgentBinding {
                agent_id: "web-agent".to_string(),
                match_channel: "web".to_string(),
                match_account_id: "*".to_string(),
                match_peer_kind: None,
                match_peer_id: None,
                match_guild_id: None,
                match_team_id: None,
            }],
            agents: vec![AgentDef { id: "main".to_string(), is_default: true }],
            dm_scope: "main".to_string(),
        });
        let input = RouteInput {
            channel: "discord".to_string(),  // different channel
            account_id: "default".to_string(),
            peer_kind: None,
            peer_id: None,
            parent_peer_kind: None,
            parent_peer_id: None,
            guild_id: None,
            team_id: None,
            identity_links: HashMap::new(),
        };
        let route = resolver.resolve(&input);
        assert_eq!(route.agent_id, "main");
        assert_eq!(route.matched_by, "default");
    }

    #[test]
    fn test_resolve_no_agents_defined() {
        let resolver = RouteResolver::new(RouteConfig {
            bindings: vec![],
            agents: vec![],
            dm_scope: "main".to_string(),
        });
        let input = RouteInput {
            channel: "web".to_string(),
            account_id: "default".to_string(),
            peer_kind: None,
            peer_id: None,
            parent_peer_kind: None,
            parent_peer_id: None,
            guild_id: None,
            team_id: None,
            identity_links: HashMap::new(),
        };
        let route = resolver.resolve(&input);
        assert_eq!(route.agent_id, "main");
    }

    #[test]
    fn test_resolve_uses_first_agent_as_default_when_none_marked() {
        let resolver = RouteResolver::new(RouteConfig {
            bindings: vec![],
            agents: vec![
                AgentDef { id: "first".to_string(), is_default: false },
                AgentDef { id: "second".to_string(), is_default: false },
            ],
            dm_scope: "main".to_string(),
        });
        let input = RouteInput {
            channel: "web".to_string(),
            account_id: "default".to_string(),
            peer_kind: None,
            peer_id: None,
            parent_peer_kind: None,
            parent_peer_id: None,
            guild_id: None,
            team_id: None,
            identity_links: HashMap::new(),
        };
        let route = resolver.resolve(&input);
        assert_eq!(route.agent_id, "first");
    }

    #[test]
    fn test_matches_account_id_empty_matches_default() {
        assert!(matches_account_id("", "default"));
        assert!(!matches_account_id("", "other"));
    }

    #[test]
    fn test_matches_account_id_wildcard() {
        assert!(matches_account_id("*", "anything"));
        assert!(matches_account_id("*", ""));
    }

    #[test]
    fn test_matches_account_id_case_insensitive() {
        assert!(matches_account_id("MyAccount", "myaccount"));
        assert!(matches_account_id("myaccount", "MYACCOUNT"));
    }
}
