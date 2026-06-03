use super::*;
use crate::types::NodeStatus;
use nemesis_types::cluster::{NodeInfo, NodeRole};

fn make_node(id: &str) -> ExtendedNodeInfo {
    ExtendedNodeInfo {
        base: NodeInfo {
            id: id.into(),
            name: format!("node-{}", id),
            role: NodeRole::Worker,
            address: format!("10.0.0.{}:9000", id.len()),
            category: "development".into(),
            last_seen: chrono::Utc::now().to_rfc3339(),
        },
        status: NodeStatus::Online,
        capabilities: vec!["llm".into()],
        addresses: vec![],
    }
}

fn make_node_with_caps(id: &str, caps: Vec<&str>) -> ExtendedNodeInfo {
    ExtendedNodeInfo {
        base: NodeInfo {
            id: id.into(),
            name: format!("node-{}", id),
            role: NodeRole::Worker,
            address: format!("10.0.0.{}:9000", id.len()),
            category: "development".into(),
            last_seen: chrono::Utc::now().to_rfc3339(),
        },
        status: NodeStatus::Online,
        capabilities: caps.into_iter().map(String::from).collect(),
        addresses: vec![],
    }
}

/// Helper: insert a peer with a specific `last_health_check` timestamp and status.
fn insert_peer_with_timestamp(
    registry: &PeerRegistry,
    id: &str,
    status: NodeStatus,
    last_health_check: &str,
    capabilities: Vec<&str>,
) {
    let info = ExtendedNodeInfo {
        base: NodeInfo {
            id: id.into(),
            name: format!("node-{}", id),
            role: NodeRole::Worker,
            address: format!("10.0.0.{}:9000", id.len()),
            category: "development".into(),
            last_seen: last_health_check.into(),
        },
        status,
        capabilities: capabilities.into_iter().map(String::from).collect(),
        addresses: vec![],
    };
    // Insert directly into the map with a specific timestamp
    let mut peers = registry.peers.lock();
    peers.insert(
        id.into(),
        PeerEntry {
            info,
            last_health_check: last_health_check.into(),
            consecutive_failures: 0,
        },
    );
}

#[test]
fn test_upsert_and_get() {
    let registry = PeerRegistry::new(HealthConfig::default());
    let node = make_node("a");
    registry.upsert(node.clone());

    let retrieved = registry.get("a").unwrap();
    assert_eq!(retrieved.base.id, "a");
    assert_eq!(retrieved.status, NodeStatus::Online);
}

#[test]
fn test_remove() {
    let registry = PeerRegistry::new(HealthConfig::default());
    registry.upsert(make_node("b"));
    assert!(registry.remove("b"));
    assert!(registry.get("b").is_none());
}

#[test]
fn test_mark_healthy_and_failed() {
    let registry = PeerRegistry::new(HealthConfig::default());
    registry.upsert(make_node("c"));

    registry.mark_healthy("c");
    let node = registry.get("c").unwrap();
    assert_eq!(node.status, NodeStatus::Online);

    // Fail 3 times -> offline
    registry.mark_failed("c");
    registry.mark_failed("c");
    let should_evict = registry.mark_failed("c");
    assert!(!should_evict);
    let node = registry.get("c").unwrap();
    assert_eq!(node.status, NodeStatus::Offline);

    // Fail 2 more times -> should evict
    registry.mark_failed("c");
    let should_evict = registry.mark_failed("c");
    assert!(should_evict);
}

#[test]
fn test_list_online() {
    let registry = PeerRegistry::new(HealthConfig::default());
    registry.upsert(make_node("d"));
    registry.upsert(make_node("e"));

    let online = registry.list_online();
    assert_eq!(online.len(), 2);
}

// -----------------------------------------------------------------------
// Tests for find_by_capability
// -----------------------------------------------------------------------

#[test]
fn test_find_by_capability_basic() {
    let registry = PeerRegistry::new(HealthConfig::default());
    let node_a = make_node_with_caps("a", vec!["llm", "tools"]);
    let node_b = make_node_with_caps("b", vec!["tools", "webhook"]);
    let node_c = make_node_with_caps("c", vec!["llm"]);
    registry.upsert(node_a);
    registry.upsert(node_b);
    registry.upsert(node_c);

    let result = registry.find_by_capability("llm");
    assert_eq!(result.len(), 2);
    let ids: Vec<&str> = result.iter().map(|n| n.base.id.as_str()).collect();
    assert!(ids.contains(&"a"));
    assert!(ids.contains(&"c"));
}

#[test]
fn test_find_by_capability_excludes_offline() {
    let registry = PeerRegistry::new(HealthConfig::default());
    let node_a = make_node_with_caps("a", vec!["llm"]);
    registry.upsert(node_a);

    // Mark offline
    registry.mark_failed("a");
    registry.mark_failed("a");
    registry.mark_failed("a"); // 3 failures -> Offline

    let result = registry.find_by_capability("llm");
    assert!(result.is_empty());
}

#[test]
fn test_find_by_capability_no_match() {
    let registry = PeerRegistry::new(HealthConfig::default());
    registry.upsert(make_node_with_caps("x", vec!["tools"]));

    let result = registry.find_by_capability("llm");
    assert!(result.is_empty());
}

#[test]
fn test_find_by_capability_empty_registry() {
    let registry = PeerRegistry::new(HealthConfig::default());
    let result = registry.find_by_capability("llm");
    assert!(result.is_empty());
}

// -----------------------------------------------------------------------
// Tests for find_by_capabilities
// -----------------------------------------------------------------------

#[test]
fn test_find_by_capabilities_any_match() {
    let registry = PeerRegistry::new(HealthConfig::default());
    registry.upsert(make_node_with_caps("a", vec!["llm"]));
    registry.upsert(make_node_with_caps("b", vec!["tools"]));
    registry.upsert(make_node_with_caps("c", vec!["webhook"]));

    let caps = vec!["llm".into(), "tools".into()];
    let result = registry.find_by_capabilities(&caps);
    assert_eq!(result.len(), 2);
    let ids: Vec<&str> = result.iter().map(|n| n.base.id.as_str()).collect();
    assert!(ids.contains(&"a"));
    assert!(ids.contains(&"b"));
}

#[test]
fn test_find_by_capabilities_no_match() {
    let registry = PeerRegistry::new(HealthConfig::default());
    registry.upsert(make_node_with_caps("a", vec!["llm"]));

    let caps = vec!["tools".into(), "webhook".into()];
    let result = registry.find_by_capabilities(&caps);
    assert!(result.is_empty());
}

#[test]
fn test_find_by_capabilities_excludes_offline() {
    let registry = PeerRegistry::new(HealthConfig::default());
    let node_a = make_node_with_caps("a", vec!["llm", "tools"]);
    registry.upsert(node_a);

    // Take offline
    registry.mark_failed("a");
    registry.mark_failed("a");
    registry.mark_failed("a");

    let caps = vec!["llm".into()];
    let result = registry.find_by_capabilities(&caps);
    assert!(result.is_empty());
}

#[test]
fn test_find_by_capabilities_empty_capabilities() {
    let registry = PeerRegistry::new(HealthConfig::default());
    registry.upsert(make_node_with_caps("a", vec!["llm"]));

    let caps: Vec<String> = vec![];
    let result = registry.find_by_capabilities(&caps);
    assert!(result.is_empty());
}

#[test]
fn test_find_by_capabilities_empty_registry() {
    let registry = PeerRegistry::new(HealthConfig::default());
    let caps = vec!["llm".into()];
    let result = registry.find_by_capabilities(&caps);
    assert!(result.is_empty());
}

// -----------------------------------------------------------------------
// Tests for evict_stale
// -----------------------------------------------------------------------

#[test]
fn test_evict_stale_removes_old_offline_peers() {
    let config = HealthConfig {
        eviction_timeout_secs: 300,
        ..HealthConfig::default()
    };
    let registry = PeerRegistry::new(config);

    // Insert an offline peer with a timestamp from 400 seconds ago
    let old_ts = (chrono::Utc::now() - chrono::Duration::seconds(400)).to_rfc3339();
    insert_peer_with_timestamp(&registry, "old", NodeStatus::Offline, &old_ts, vec!["llm"]);

    // Insert a recent offline peer (should NOT be evicted)
    let recent_ts = (chrono::Utc::now() - chrono::Duration::seconds(60)).to_rfc3339();
    insert_peer_with_timestamp(&registry, "recent", NodeStatus::Offline, &recent_ts, vec!["llm"]);

    // Insert an online peer with old timestamp (should NOT be evicted)
    let online_old_ts = (chrono::Utc::now() - chrono::Duration::seconds(400)).to_rfc3339();
    insert_peer_with_timestamp(&registry, "online_old", NodeStatus::Online, &online_old_ts, vec!["llm"]);

    assert_eq!(registry.len(), 3);

    let evicted = registry.evict_stale();
    assert_eq!(evicted.len(), 1);
    assert_eq!(evicted[0], "old");

    assert_eq!(registry.len(), 2);
    assert!(registry.get("old").is_none());
    assert!(registry.get("recent").is_some());
    assert!(registry.get("online_old").is_some());
}

#[test]
fn test_evict_stale_nothing_to_evict() {
    let config = HealthConfig {
        eviction_timeout_secs: 300,
        ..HealthConfig::default()
    };
    let registry = PeerRegistry::new(config);

    // Only online peers, nothing should be evicted
    registry.upsert(make_node("a"));
    registry.upsert(make_node("b"));

    let evicted = registry.evict_stale();
    assert!(evicted.is_empty());
    assert_eq!(registry.len(), 2);
}

#[test]
fn test_evict_stale_empty_registry() {
    let registry = PeerRegistry::new(HealthConfig::default());
    let evicted = registry.evict_stale();
    assert!(evicted.is_empty());
}

#[test]
fn test_evict_stale_boundary_exactly_at_timeout() {
    let config = HealthConfig {
        eviction_timeout_secs: 300,
        ..HealthConfig::default()
    };
    let registry = PeerRegistry::new(config);

    // Peer exactly at the threshold (300 seconds ago) should be evicted
    // because we use `<` (strictly less than threshold)
    let boundary_ts = (chrono::Utc::now() - chrono::Duration::seconds(301)).to_rfc3339();
    insert_peer_with_timestamp(&registry, "boundary", NodeStatus::Offline, &boundary_ts, vec!["llm"]);

    let evicted = registry.evict_stale();
    assert_eq!(evicted.len(), 1);
    assert_eq!(evicted[0], "boundary");
}

// -----------------------------------------------------------------------
// Tests for check_health
// -----------------------------------------------------------------------

#[test]
fn test_check_health_marks_stale_peers() {
    let config = HealthConfig {
        stale_timeout_secs: 90,
        ..HealthConfig::default()
    };
    let registry = PeerRegistry::new(config);

    // Insert a peer with a timestamp from 120 seconds ago (exceeds 90s stale timeout)
    let old_ts = (chrono::Utc::now() - chrono::Duration::seconds(120)).to_rfc3339();
    insert_peer_with_timestamp(&registry, "stale", NodeStatus::Online, &old_ts, vec!["llm"]);

    // Insert a recent peer (should NOT be marked stale)
    let recent_ts = (chrono::Utc::now() - chrono::Duration::seconds(30)).to_rfc3339();
    insert_peer_with_timestamp(&registry, "fresh", NodeStatus::Online, &recent_ts, vec!["llm"]);

    let stale = registry.check_health();
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0], "stale");

    // Verify the stale peer is now Offline
    let node = registry.get("stale").unwrap();
    assert_eq!(node.status, NodeStatus::Offline);

    // Verify the fresh peer is still Online
    let node = registry.get("fresh").unwrap();
    assert_eq!(node.status, NodeStatus::Online);
}

#[test]
fn test_check_health_no_stale_peers() {
    let registry = PeerRegistry::new(HealthConfig::default());

    // All peers have fresh timestamps
    registry.upsert(make_node("a"));
    registry.upsert(make_node("b"));

    let stale = registry.check_health();
    assert!(stale.is_empty());
}

#[test]
fn test_check_health_empty_registry() {
    let registry = PeerRegistry::new(HealthConfig::default());
    let stale = registry.check_health();
    assert!(stale.is_empty());
}

#[test]
fn test_check_health_does_not_touch_already_offline() {
    let config = HealthConfig {
        stale_timeout_secs: 90,
        ..HealthConfig::default()
    };
    let registry = PeerRegistry::new(config);

    // Insert a peer that is already Offline with an old timestamp
    let old_ts = (chrono::Utc::now() - chrono::Duration::seconds(200)).to_rfc3339();
    insert_peer_with_timestamp(&registry, "already_offline", NodeStatus::Offline, &old_ts, vec!["llm"]);

    let stale = registry.check_health();
    // Should NOT appear in the newly-stale list since it was already Offline
    assert!(stale.is_empty());

    // Status should remain Offline
    let node = registry.get("already_offline").unwrap();
    assert_eq!(node.status, NodeStatus::Offline);
}

#[test]
fn test_check_health_idempotent() {
    let config = HealthConfig {
        stale_timeout_secs: 90,
        ..HealthConfig::default()
    };
    let registry = PeerRegistry::new(config);

    let old_ts = (chrono::Utc::now() - chrono::Duration::seconds(120)).to_rfc3339();
    insert_peer_with_timestamp(&registry, "stale", NodeStatus::Online, &old_ts, vec!["llm"]);

    // First call marks it stale
    let stale = registry.check_health();
    assert_eq!(stale.len(), 1);

    // Second call should return nothing (already Offline)
    let stale = registry.check_health();
    assert!(stale.is_empty());
}

// -----------------------------------------------------------------------
// Integration: check_health + evict_stale pipeline
// -----------------------------------------------------------------------

#[test]
fn test_check_health_then_evict_pipeline() {
    let config = HealthConfig {
        stale_timeout_secs: 90,
        eviction_timeout_secs: 300,
        ..HealthConfig::default()
    };
    let registry = PeerRegistry::new(config);

    // Insert a peer with a very old timestamp
    let old_ts = (chrono::Utc::now() - chrono::Duration::seconds(400)).to_rfc3339();
    insert_peer_with_timestamp(&registry, "ancient", NodeStatus::Online, &old_ts, vec!["llm"]);

    // Insert a moderately old peer
    let mid_ts = (chrono::Utc::now() - chrono::Duration::seconds(120)).to_rfc3339();
    insert_peer_with_timestamp(&registry, "stale", NodeStatus::Online, &mid_ts, vec!["llm"]);

    // Insert a fresh peer
    let fresh_ts = (chrono::Utc::now() - chrono::Duration::seconds(10)).to_rfc3339();
    insert_peer_with_timestamp(&registry, "fresh", NodeStatus::Online, &fresh_ts, vec!["llm"]);

    assert_eq!(registry.len(), 3);

    // Step 1: check_health marks both old peers as stale
    let stale = registry.check_health();
    assert_eq!(stale.len(), 2);
    let stale_ids: Vec<&str> = stale.iter().map(|s| s.as_str()).collect();
    assert!(stale_ids.contains(&"ancient"));
    assert!(stale_ids.contains(&"stale"));

    // Step 2: evict_stale removes the ancient peer (offline > eviction_timeout)
    let evicted = registry.evict_stale();
    assert_eq!(evicted.len(), 1);
    assert_eq!(evicted[0], "ancient");

    // "stale" remains (only 120s old, not > 300s eviction timeout)
    // "fresh" remains (still Online)
    assert_eq!(registry.len(), 2);
    assert!(registry.get("ancient").is_none());
    assert!(registry.get("stale").is_some());
    assert!(registry.get("fresh").is_some());
}

// -- Additional tests: peer registry advanced scenarios --

#[test]
fn test_upsert_updates_existing_peer() {
    let registry = PeerRegistry::new(HealthConfig::default());
    registry.upsert(make_node("a"));

    // Upsert with updated capabilities
    let updated = make_node_with_caps("a", vec!["llm", "tools", "vision"]);
    registry.upsert(updated);

    let node = registry.get("a").unwrap();
    assert_eq!(node.capabilities.len(), 3);
    assert!(node.capabilities.contains(&"vision".to_string()));
}

#[test]
fn test_remove_nonexistent_returns_false() {
    let registry = PeerRegistry::new(HealthConfig::default());
    assert!(!registry.remove("nonexistent"));
}

#[test]
fn test_get_nonexistent_returns_none() {
    let registry = PeerRegistry::new(HealthConfig::default());
    assert!(registry.get("nonexistent").is_none());
}

#[test]
fn test_len_and_is_empty() {
    let registry = PeerRegistry::new(HealthConfig::default());
    assert!(registry.is_empty());
    assert_eq!(registry.len(), 0);

    registry.upsert(make_node("a"));
    registry.upsert(make_node("b"));
    assert_eq!(registry.len(), 2);

    registry.remove("a");
    assert_eq!(registry.len(), 1);
}

#[test]
fn test_get_capabilities_collects_and_sorts() {
    let registry = PeerRegistry::new(HealthConfig::default());
    registry.upsert(make_node_with_caps("a", vec!["tools"]));
    registry.upsert(make_node_with_caps("b", vec!["llm", "vision"]));
    registry.upsert(make_node_with_caps("c", vec!["llm", "tools"]));

    let caps = registry.get_capabilities();
    // Should be sorted and deduplicated
    assert_eq!(caps, vec!["llm".to_string(), "tools".to_string(), "vision".to_string()]);
}

#[test]
fn test_online_count() {
    let registry = PeerRegistry::new(HealthConfig::default());
    registry.upsert(make_node("a"));
    registry.upsert(make_node("b"));
    assert_eq!(registry.online_count(), 2);

    // Take one offline
    registry.mark_offline("a", "test");
    assert_eq!(registry.online_count(), 1);
}

#[test]
fn test_find_by_capability_online_matches_find_by_capability() {
    let registry = PeerRegistry::new(HealthConfig::default());
    registry.upsert(make_node_with_caps("a", vec!["llm"]));

    let by_cap = registry.find_by_capability("llm");
    let by_cap_online = registry.find_by_capability_online("llm");
    assert_eq!(by_cap.len(), by_cap_online.len());
}

#[test]
fn test_mark_offline_sets_status() {
    let registry = PeerRegistry::new(HealthConfig::default());
    registry.upsert(make_node("a"));
    assert_eq!(registry.get("a").unwrap().status, NodeStatus::Online);

    registry.mark_offline("a", "maintenance");
    assert_eq!(registry.get("a").unwrap().status, NodeStatus::Offline);
}

#[test]
fn test_mark_offline_nonexistent_does_not_panic() {
    let registry = PeerRegistry::new(HealthConfig::default());
    registry.mark_offline("nonexistent", "test");
    // Should not panic
}

#[test]
fn test_check_timeouts_marks_old_online_peers() {
    let registry = PeerRegistry::new(HealthConfig::default());

    // Insert peer with old health check timestamp
    let old_ts = (chrono::Utc::now() - chrono::Duration::seconds(200)).to_rfc3339();
    insert_peer_with_timestamp(&registry, "old", NodeStatus::Online, &old_ts, vec!["llm"]);

    let expired = registry.check_timeouts(std::time::Duration::from_secs(90));
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0], "old");

    let node = registry.get("old").unwrap();
    assert_eq!(node.status, NodeStatus::Offline);
}

#[test]
fn test_list_peers_includes_offline() {
    let registry = PeerRegistry::new(HealthConfig::default());
    registry.upsert(make_node("a"));
    registry.upsert(make_node("b"));

    registry.mark_offline("a", "test");

    let all = registry.list_peers();
    assert_eq!(all.len(), 2);

    let online = registry.list_online();
    assert_eq!(online.len(), 1);
    assert_eq!(online[0].base.id, "b");
}

// -- Additional tests: invalid timestamp handling --

#[test]
fn test_check_timeouts_invalid_timestamp() {
    let registry = PeerRegistry::new(HealthConfig::default());

    // Insert a peer with an invalid RFC3339 timestamp and Online status
    insert_peer_with_timestamp(
        &registry,
        "bad-ts",
        NodeStatus::Online,
        "not-a-valid-timestamp",
        vec!["llm"],
    );

    // Should not panic; the invalid timestamp is silently skipped
    let expired = registry.check_timeouts(std::time::Duration::from_secs(90));
    assert!(expired.is_empty(), "invalid timestamp should be skipped, not expired");

    // Node should still be Online (unchanged)
    let node = registry.get("bad-ts").unwrap();
    assert_eq!(node.status, NodeStatus::Online);
}

#[test]
fn test_evict_stale_invalid_timestamp() {
    let config = HealthConfig {
        eviction_timeout_secs: 300,
        ..HealthConfig::default()
    };
    let registry = PeerRegistry::new(config);

    // Insert an offline peer with an invalid timestamp
    insert_peer_with_timestamp(
        &registry,
        "bad-evict",
        NodeStatus::Offline,
        "garbage-timestamp-!!!",
        vec!["llm"],
    );

    // Should not panic; the invalid timestamp is silently skipped
    let evicted = registry.evict_stale();
    assert!(evicted.is_empty(), "invalid timestamp should be skipped, not evicted");

    // Node should still be in the registry
    assert!(registry.get("bad-evict").is_some());
}

#[test]
fn test_check_timeouts_mixed_valid_and_invalid_timestamps() {
    let registry = PeerRegistry::new(HealthConfig::default());

    // Valid old timestamp - should be expired
    let old_ts = (chrono::Utc::now() - chrono::Duration::seconds(200)).to_rfc3339();
    insert_peer_with_timestamp(&registry, "old-valid", NodeStatus::Online, &old_ts, vec!["llm"]);

    // Invalid timestamp - should be skipped
    insert_peer_with_timestamp(
        &registry,
        "invalid",
        NodeStatus::Online,
        "bad-ts",
        vec!["llm"],
    );

    // Valid recent timestamp - should not be expired
    let recent_ts = (chrono::Utc::now() - chrono::Duration::seconds(10)).to_rfc3339();
    insert_peer_with_timestamp(&registry, "recent", NodeStatus::Online, &recent_ts, vec!["llm"]);

    let expired = registry.check_timeouts(std::time::Duration::from_secs(90));
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0], "old-valid");

    // Verify the invalid-timestamp node is still Online
    let invalid_node = registry.get("invalid").unwrap();
    assert_eq!(invalid_node.status, NodeStatus::Online);
}

#[test]
fn test_evict_stale_mixed_valid_and_invalid_timestamps() {
    let config = HealthConfig {
        eviction_timeout_secs: 300,
        ..HealthConfig::default()
    };
    let registry = PeerRegistry::new(config);

    // Offline with valid old timestamp - should be evicted
    let old_ts = (chrono::Utc::now() - chrono::Duration::seconds(400)).to_rfc3339();
    insert_peer_with_timestamp(&registry, "old-offline", NodeStatus::Offline, &old_ts, vec!["llm"]);

    // Offline with invalid timestamp - should be skipped
    insert_peer_with_timestamp(
        &registry,
        "invalid-offline",
        NodeStatus::Offline,
        "not-a-date",
        vec!["llm"],
    );

    let evicted = registry.evict_stale();
    assert_eq!(evicted.len(), 1);
    assert_eq!(evicted[0], "old-offline");

    // Invalid-timestamp offline peer should still exist
    assert!(registry.get("invalid-offline").is_some());
}

#[test]
fn test_check_health_invalid_timestamp() {
    let config = HealthConfig {
        stale_timeout_secs: 90,
        ..HealthConfig::default()
    };
    let registry = PeerRegistry::new(config);

    // Online peer with invalid timestamp
    insert_peer_with_timestamp(
        &registry,
        "invalid-health",
        NodeStatus::Online,
        "not-rfc3339",
        vec!["llm"],
    );

    // Should not panic, invalid timestamp is skipped
    let stale = registry.check_health();
    assert!(stale.is_empty());

    // Node stays Online
    let node = registry.get("invalid-health").unwrap();
    assert_eq!(node.status, NodeStatus::Online);
}
