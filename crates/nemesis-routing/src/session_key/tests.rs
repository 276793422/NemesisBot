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
    assert_eq!(
        DMScope::from_str("per-channel-peer"),
        DMScope::PerChannelPeer
    );
    assert_eq!(
        DMScope::from_str("per-account-channel-peer"),
        DMScope::PerAccountChannelPeer
    );
    assert_eq!(DMScope::from_str(""), DMScope::Main);
    assert_eq!(DMScope::from_str("unknown"), DMScope::Main);
}

// Additional tests to improve coverage

#[test]
fn test_dm_scope_from_str_with_whitespace() {
    assert_eq!(DMScope::from_str("  main  "), DMScope::Main);
    assert_eq!(DMScope::from_str("  PER-PEER  "), DMScope::PerPeer);
    assert_eq!(
        DMScope::from_str("  Per-Channel-Peer  "),
        DMScope::PerChannelPeer
    );
}

#[test]
fn test_peer_session_key_channel_peer_empty_peer_id() {
    let params = SessionKeyParams {
        agent_id: "main".into(),
        channel: "discord".into(),
        account_id: "default".into(),
        peer: Some(RoutePeer {
            kind: "group".into(),
            id: String::new(), // empty peer ID
        }),
        dm_scope: DMScope::Main,
        identity_links: HashMap::new(),
    };
    let key = build_agent_peer_session_key(params);
    // Empty peer ID should be replaced with "unknown"
    assert_eq!(key, "agent:main:discord:group:unknown");
}

#[test]
fn test_peer_session_key_with_whitespace_in_peer_id() {
    let params = SessionKeyParams {
        agent_id: "main".into(),
        channel: "web".into(),
        account_id: "default".into(),
        peer: Some(RoutePeer {
            kind: "direct".into(),
            id: "  user42  ".to_string(), // with whitespace
        }),
        dm_scope: DMScope::PerPeer,
        identity_links: HashMap::new(),
    };
    let key = build_agent_peer_session_key(params);
    // Whitespace should be trimmed
    assert_eq!(key, "agent:main:direct:user42");
}

#[test]
fn test_peer_session_key_case_insensitive() {
    let params = SessionKeyParams {
        agent_id: "Main".into(),
        channel: "Web".into(),
        account_id: "Default".into(),
        peer: Some(RoutePeer {
            kind: "Direct".into(),
            id: "User42".to_string(),
        }),
        dm_scope: DMScope::PerPeer,
        identity_links: HashMap::new(),
    };
    let key = build_agent_peer_session_key(params);
    // Everything should be lowercased
    assert_eq!(key, "agent:main:direct:user42");
}

#[test]
fn test_peer_session_key_empty_agent_id() {
    let params = SessionKeyParams {
        agent_id: String::new(), // empty agent ID
        channel: "web".into(),
        account_id: "default".into(),
        peer: Some(RoutePeer {
            kind: "direct".into(),
            id: "user42".to_string(),
        }),
        dm_scope: DMScope::PerPeer,
        identity_links: HashMap::new(),
    };
    let key = build_agent_peer_session_key(params);
    // Empty agent ID should be normalized to "main"
    assert_eq!(key, "agent:main:direct:user42");
}

#[test]
fn test_peer_session_key_empty_channel() {
    let params = SessionKeyParams {
        agent_id: "main".into(),
        channel: String::new(), // empty channel
        account_id: "default".into(),
        peer: Some(RoutePeer {
            kind: "group".into(),
            id: "guild123".to_string(),
        }),
        dm_scope: DMScope::Main,
        identity_links: HashMap::new(),
    };
    let key = build_agent_peer_session_key(params);
    // Empty channel should be replaced with "unknown"
    assert_eq!(key, "agent:main:unknown:group:guild123");
}

#[test]
fn test_identity_links_with_empty_peer_id() {
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
            id: String::new(), // empty peer ID
        }),
        dm_scope: DMScope::PerChannelPeer,
        identity_links: links,
    };
    let key = build_agent_peer_session_key(params);
    // Empty peer ID should not resolve identity links, fall back to main
    assert_eq!(key, "agent:main:main");
}

#[test]
fn test_identity_links_with_empty_canonical_name() {
    let mut links = HashMap::new();
    links.insert(
        String::new(), // empty canonical name
        vec!["discord:alice_d".to_string()],
    );

    let params = SessionKeyParams {
        agent_id: "main".into(),
        channel: "discord".into(),
        account_id: "default".into(),
        peer: Some(RoutePeer {
            kind: "direct".into(),
            id: "alice_d".to_string(),
        }),
        dm_scope: DMScope::PerChannelPeer,
        identity_links: links,
    };
    let key = build_agent_peer_session_key(params);
    // Empty canonical name should not match, use original peer ID
    assert_eq!(key, "agent:main:discord:direct:alice_d");
}

#[test]
fn test_identity_links_with_empty_link_entry() {
    let mut links = HashMap::new();
    links.insert(
        "alice".to_string(),
        vec![String::new()], // empty link entry
    );

    let params = SessionKeyParams {
        agent_id: "main".into(),
        channel: "discord".into(),
        account_id: "default".into(),
        peer: Some(RoutePeer {
            kind: "direct".into(),
            id: "alice_d".to_string(),
        }),
        dm_scope: DMScope::PerChannelPeer,
        identity_links: links,
    };
    let key = build_agent_peer_session_key(params);
    // Empty link entry should not match, use original peer ID
    assert_eq!(key, "agent:main:discord:direct:alice_d");
}

#[test]
fn test_identity_links_channel_peer_format() {
    let mut links = HashMap::new();
    links.insert("alice".to_string(), vec!["discord:alice_d".to_string()]);

    let params = SessionKeyParams {
        agent_id: "main".into(),
        channel: "discord".into(),
        account_id: "default".into(),
        peer: Some(RoutePeer {
            kind: "direct".into(),
            id: "alice_d".to_string(),
        }),
        dm_scope: DMScope::PerChannelPeer,
        identity_links: links,
    };
    let key = build_agent_peer_session_key(params);
    // Should match channel:peerID format
    assert_eq!(key, "agent:main:discord:direct:alice");
}

#[test]
fn test_identity_links_bare_peer_id() {
    let mut links = HashMap::new();
    links.insert(
        "alice".to_string(),
        vec!["alice_d".to_string()], // bare peer ID without channel prefix
    );

    let params = SessionKeyParams {
        agent_id: "main".into(),
        channel: "discord".into(),
        account_id: "default".into(),
        peer: Some(RoutePeer {
            kind: "direct".into(),
            id: "alice_d".to_string(),
        }),
        dm_scope: DMScope::PerChannelPeer,
        identity_links: links,
    };
    let key = build_agent_peer_session_key(params);
    // Should match bare peer ID
    assert_eq!(key, "agent:main:discord:direct:alice");
}

#[test]
fn test_identity_links_case_insensitive() {
    let mut links = HashMap::new();
    links.insert("Alice".to_string(), vec!["DISCORD:ALICE_D".to_string()]);

    let params = SessionKeyParams {
        agent_id: "main".into(),
        channel: "Discord".into(),
        account_id: "default".into(),
        peer: Some(RoutePeer {
            kind: "direct".into(),
            id: "alice_d".to_string(),
        }),
        dm_scope: DMScope::PerChannelPeer,
        identity_links: links,
    };
    let key = build_agent_peer_session_key(params);
    // Should match case-insensitively
    assert_eq!(key, "agent:main:discord:direct:alice");
}

#[test]
fn test_identity_links_with_whitespace() {
    let mut links = HashMap::new();
    links.insert("alice".to_string(), vec!["  discord:alice_d  ".to_string()]);

    let params = SessionKeyParams {
        agent_id: "main".into(),
        channel: "discord".into(),
        account_id: "default".into(),
        peer: Some(RoutePeer {
            kind: "direct".into(),
            id: "alice_d".to_string(),
        }),
        dm_scope: DMScope::PerChannelPeer,
        identity_links: links,
    };
    let key = build_agent_peer_session_key(params);
    // Should match with trimmed whitespace
    assert_eq!(key, "agent:main:discord:direct:alice");
}

#[test]
fn test_resolve_linked_peer_id_empty_identity_links() {
    let empty_links: HashMap<String, Vec<String>> = HashMap::new();
    let params = SessionKeyParams {
        agent_id: "main".into(),
        channel: "discord".into(),
        account_id: "default".into(),
        peer: Some(RoutePeer {
            kind: "direct".into(),
            id: "alice".to_string(),
        }),
        dm_scope: DMScope::PerPeer,
        identity_links: empty_links,
    };
    let key = build_agent_peer_session_key(params);
    // Empty identity links should not crash, just use original peer ID
    assert_eq!(key, "agent:main:direct:alice");
}

#[test]
fn test_resolve_linked_peer_id_empty_peer_id_with_links() {
    let mut links = HashMap::new();
    links.insert("alice".to_string(), vec!["discord:alice_d".to_string()]);

    let params = SessionKeyParams {
        agent_id: "main".into(),
        channel: "discord".into(),
        account_id: "default".into(),
        peer: Some(RoutePeer {
            kind: "direct".into(),
            id: String::new(), // empty peer ID
        }),
        dm_scope: DMScope::PerPeer,
        identity_links: links,
    };
    let key = build_agent_peer_session_key(params);
    // Empty peer ID should fall back to main session
    assert_eq!(key, "agent:main:main");
}

#[test]
fn test_resolve_linked_peer_id_empty_channel() {
    let mut links = HashMap::new();
    links.insert("alice".to_string(), vec!["discord:alice_d".to_string()]);

    let params = SessionKeyParams {
        agent_id: "main".into(),
        channel: String::new(), // empty channel
        account_id: "default".into(),
        peer: Some(RoutePeer {
            kind: "direct".into(),
            id: "alice_d".to_string(),
        }),
        dm_scope: DMScope::PerChannelPeer,
        identity_links: links,
    };
    let key = build_agent_peer_session_key(params);
    // Empty channel should not build channel:peerID format candidate
    // The link "discord:alice_d" won't match "alice_d" (bare) because it's formatted as "channel:peerID"
    // So it won't resolve, should use original peer ID
    assert_eq!(key, "agent:main:unknown:direct:alice_d");
}

#[test]
fn test_resolve_linked_peer_id_empty_channel_bare_match() {
    let mut links = HashMap::new();
    links.insert(
        "alice".to_string(),
        vec!["alice_d".to_string()], // bare peer ID (no channel prefix)
    );

    let params = SessionKeyParams {
        agent_id: "main".into(),
        channel: String::new(), // empty channel
        account_id: "default".into(),
        peer: Some(RoutePeer {
            kind: "direct".into(),
            id: "alice_d".to_string(),
        }),
        dm_scope: DMScope::PerChannelPeer,
        identity_links: links,
    };
    let key = build_agent_peer_session_key(params);
    // Empty channel should still allow bare peer ID matching
    assert_eq!(key, "agent:main:unknown:direct:alice");
}

#[test]
fn test_resolve_linked_peer_id_no_match_found() {
    let mut links = HashMap::new();
    links.insert("alice".to_string(), vec!["discord:alice_d".to_string()]);

    let params = SessionKeyParams {
        agent_id: "main".into(),
        channel: "slack".into(), // different channel
        account_id: "default".into(),
        peer: Some(RoutePeer {
            kind: "direct".into(),
            id: "alice_s".to_string(), // different peer ID
        }),
        dm_scope: DMScope::PerChannelPeer,
        identity_links: links,
    };
    let key = build_agent_peer_session_key(params);
    // No match found, should use original peer ID
    assert_eq!(key, "agent:main:slack:direct:alice_s");
}

#[test]
fn test_resolve_linked_peer_id_multiple_links() {
    let mut links = HashMap::new();
    links.insert(
        "alice".to_string(),
        vec![
            "discord:alice_d".to_string(),
            "slack:alice_s".to_string(),
            "telegram:alice_t".to_string(),
        ],
    );

    // Should match first entry
    let params = SessionKeyParams {
        agent_id: "main".into(),
        channel: "discord".into(),
        account_id: "default".into(),
        peer: Some(RoutePeer {
            kind: "direct".into(),
            id: "alice_d".to_string(),
        }),
        dm_scope: DMScope::PerChannelPeer,
        identity_links: links.clone(),
    };
    let key = build_agent_peer_session_key(params);
    assert_eq!(key, "agent:main:discord:direct:alice");

    // Should match second entry
    let params2 = SessionKeyParams {
        agent_id: "main".into(),
        channel: "slack".into(),
        account_id: "default".into(),
        peer: Some(RoutePeer {
            kind: "direct".into(),
            id: "alice_s".to_string(),
        }),
        dm_scope: DMScope::PerChannelPeer,
        identity_links: links,
    };
    let key2 = build_agent_peer_session_key(params2);
    assert_eq!(key2, "agent:main:slack:direct:alice");
}

#[test]
fn test_resolve_linked_peer_id_whitespace_in_channel() {
    let mut links = HashMap::new();
    links.insert("alice".to_string(), vec!["discord:alice_d".to_string()]);

    let params = SessionKeyParams {
        agent_id: "main".into(),
        channel: "  discord  ".to_string(), // with whitespace
        account_id: "default".into(),
        peer: Some(RoutePeer {
            kind: "direct".into(),
            id: "alice_d".to_string(),
        }),
        dm_scope: DMScope::PerChannelPeer,
        identity_links: links,
    };
    let key = build_agent_peer_session_key(params);
    // Should trim whitespace from channel before building candidate
    assert_eq!(key, "agent:main:discord:direct:alice");
}

#[test]
fn test_resolve_linked_peer_id_whitespace_in_peer_id() {
    let mut links = HashMap::new();
    links.insert("alice".to_string(), vec!["discord:alice_d".to_string()]);

    let params = SessionKeyParams {
        agent_id: "main".into(),
        channel: "discord".into(),
        account_id: "default".into(),
        peer: Some(RoutePeer {
            kind: "direct".into(),
            id: "  alice_d  ".to_string(), // with whitespace
        }),
        dm_scope: DMScope::PerPeer,
        identity_links: links,
    };
    let key = build_agent_peer_session_key(params);
    // Should trim whitespace from peer ID before matching
    assert_eq!(key, "agent:main:direct:alice");
}

#[test]
fn test_peer_session_kind_empty_string() {
    let params = SessionKeyParams {
        agent_id: "main".into(),
        channel: "web".into(),
        account_id: "default".into(),
        peer: Some(RoutePeer {
            kind: String::new(), // empty kind
            id: "user42".to_string(),
        }),
        dm_scope: DMScope::Main,
        identity_links: HashMap::new(),
    };
    let key = build_agent_peer_session_key(params);
    // Empty kind should default to "direct" for main scope
    assert_eq!(key, "agent:main:main");
}

#[test]
fn test_peer_session_whitespace_kind() {
    let params = SessionKeyParams {
        agent_id: "main".into(),
        channel: "web".into(),
        account_id: "default".into(),
        peer: Some(RoutePeer {
            kind: "  ".to_string(), // whitespace only
            id: "user42".to_string(),
        }),
        dm_scope: DMScope::Main,
        identity_links: HashMap::new(),
    };
    let key = build_agent_peer_session_key(params);
    // Whitespace only kind should default to "direct"
    assert_eq!(key, "agent:main:main");
}

#[test]
fn test_build_session_key_uppercase_agent() {
    let key = build_agent_session_key("MAIN", "WEB", "DIRECT", "USER1");
    // Should normalize to lowercase
    assert_eq!(key, "agent:main:web:direct:user1");
}

#[test]
fn test_build_main_session_key_uppercase_agent() {
    let key = build_agent_main_session_key("MAIN");
    // Should normalize to lowercase
    assert_eq!(key, "agent:main:main");
}

#[test]
fn test_parse_session_key_uppercase_agent() {
    let (agent, rest) = parse_agent_session_key("agent:MAIN:web:direct:user1").unwrap();
    // Should preserve original case in agent part
    assert_eq!(agent, "MAIN");
    assert_eq!(rest, "web:direct:user1");
}

#[test]
fn test_is_subagent_uppercase() {
    assert!(is_subagent_session_key("SUBAGENT:123"));
    // The rest part needs to start with "subagent:", not just contain it
    assert!(is_subagent_session_key("agent:main:subagent:456"));
    assert!(!is_subagent_session_key("AGENT:MAIN:WEB:DIRECT:USER1"));
}
