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
