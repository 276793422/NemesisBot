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
