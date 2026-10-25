//! BUG-20261025-50：路由 config → 生产 RouteResolver 映射测试（无 feature
//! 门——纯映射逻辑默认构建必跑）。覆盖：缺省 config 与 new_bus 硬编码默认
//! 同形（零回归锚）、session.dm_scope 生效、bindings 映射与路由、agents.list
//! 对未知绑定 agent 的默认回退、guild/team 空串 → 无约束（account 级可命中）。

use crate::agent_factory::build_route_resolver;
use nemesis_routing::RouteInput;
use std::collections::HashMap;

fn dm_input(channel: &str, peer_kind: &str, peer_id: &str) -> RouteInput {
    RouteInput {
        channel: channel.to_string(),
        account_id: String::new(),
        peer_kind: Some(peer_kind.to_string()),
        peer_id: Some(peer_id.to_string()),
        parent_peer_kind: None,
        parent_peer_id: None,
        guild_id: None,
        team_id: None,
        identity_links: HashMap::new(),
    }
}

/// 零回归锚：全缺省 config（空 dm_scope / 空绑定 / 空 agents.list）解析出
/// 与旧 new_bus 硬编码默认（main agent + Main scope）逐字节相同的键。
#[test]
fn r50_default_config_matches_legacy_hardcoded_behavior() {
    let resolver = build_route_resolver(&nemesis_config::Config::default());
    let route = resolver.resolve(&dm_input("telegram", "direct", "123"));
    assert_eq!(route.agent_id, "main");
    assert_eq!(route.session_key, "agent:main:main");
    assert_eq!(route.matched_by, "default");
}

/// session.dm_scope 只声名未读取的根修：显式 per-channel-peer 直接改变
/// DM 会话键粒度。
#[test]
fn r50_dm_scope_from_config_is_honored() {
    let mut cfg = nemesis_config::Config::default();
    cfg.session.dm_scope = "per-channel-peer".to_string();
    let resolver = build_route_resolver(&cfg);
    let route = resolver.resolve(&dm_input("telegram", "direct", "Alice"));
    assert_eq!(route.agent_id, "main");
    assert_eq!(route.session_key, "agent:main:telegram:direct:alice");
    assert_eq!(route.matched_by, "default");
}

/// config bindings → routing AgentBinding 映射：peer 绑定命中后路由到绑定
/// agent；agents.list 为空时保留绑定原始 id（pick_agent 空列表不回退）。
#[test]
fn r50_binding_maps_and_routes_to_bound_agent() {
    let mut cfg = nemesis_config::Config::default();
    cfg.bindings.push(nemesis_config::AgentBinding {
        agent_id: "support".to_string(),
        r#match: nemesis_config::BindingMatch {
            channel: "telegram".to_string(),
            account_id: String::new(),
            peer: Some(nemesis_config::PeerMatch {
                kind: "group".to_string(),
                id: "G-1".to_string(),
            }),
            guild_id: String::new(),
            team_id: String::new(),
        },
    });
    let resolver = build_route_resolver(&cfg);
    let route = resolver.resolve(&dm_input("telegram", "group", "G-1"));
    assert_eq!(route.matched_by, "binding.peer");
    assert_eq!(route.agent_id, "support");
    assert_eq!(route.session_key, "agent:support:telegram:group:g-1");
}

/// agents.list 非空时 pick_agent 按其校验：绑定指向不存在的 agent → 回退
/// 默认（首个 default 条目），session 键跟着默认 agent 走。
#[test]
fn r50_agents_list_coerces_unknown_binding_agent_to_default() {
    let mut cfg = nemesis_config::Config::default();
    cfg.agents.list = vec![nemesis_config::AgentConfigEntry {
        id: "main".to_string(),
        default: true,
        ..Default::default()
    }];
    cfg.bindings.push(nemesis_config::AgentBinding {
        agent_id: "ghost".to_string(),
        r#match: nemesis_config::BindingMatch {
            channel: "telegram".to_string(),
            account_id: String::new(),
            peer: Some(nemesis_config::PeerMatch {
                kind: "group".to_string(),
                id: "G-1".to_string(),
            }),
            guild_id: String::new(),
            team_id: String::new(),
        },
    });
    let resolver = build_route_resolver(&cfg);
    let route = resolver.resolve(&dm_input("telegram", "group", "G-1"));
    assert_eq!(route.matched_by, "binding.peer");
    assert_eq!(route.agent_id, "main");
    assert_eq!(route.session_key, "agent:main:telegram:group:g-1");
}

/// guild/team 空串 = 无约束（→ Option None）：不带 peer 元数据的消息可落
/// account 级绑定，而不是被 Some("") 死绑置换。
#[test]
fn r50_guild_empty_string_maps_to_no_constraint() {
    let mut cfg = nemesis_config::Config::default();
    cfg.bindings.push(nemesis_config::AgentBinding {
        agent_id: "helper".to_string(),
        r#match: nemesis_config::BindingMatch {
            channel: "telegram".to_string(),
            account_id: String::new(),
            peer: None,
            guild_id: String::new(),
            team_id: String::new(),
        },
    });
    let resolver = build_route_resolver(&cfg);
    let input = RouteInput {
        channel: "telegram".to_string(),
        account_id: String::new(),
        peer_kind: None,
        peer_id: None,
        parent_peer_kind: None,
        parent_peer_id: None,
        guild_id: None,
        team_id: None,
        identity_links: HashMap::new(),
    };
    let route = resolver.resolve(&input);
    assert_eq!(route.matched_by, "binding.account");
    assert_eq!(route.agent_id, "helper");
}
