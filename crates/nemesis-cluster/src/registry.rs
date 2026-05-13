//! Peer registry - tracks known cluster nodes with health information.
//!
//! Maintains a set of known peers with their status, last-seen timestamps,
//! and capabilities. Supports health-check-based eviction and capability queries.

use std::collections::HashMap;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::types::{ExtendedNodeInfo, NodeStatus};

/// Configuration for peer health-checking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthConfig {
    /// How often to ping peers (seconds).
    pub check_interval_secs: u64,
    /// How long before a peer is considered stale.
    pub stale_timeout_secs: u64,
    /// How long before a stale peer is removed entirely.
    pub eviction_timeout_secs: u64,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            check_interval_secs: 30,
            stale_timeout_secs: 90,
            eviction_timeout_secs: 300,
        }
    }
}

/// The peer registry tracks known cluster nodes.
pub struct PeerRegistry {
    peers: Mutex<HashMap<String, PeerEntry>>,
    health_config: HealthConfig,
}

/// Extended peer entry with health tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerEntry {
    /// The node information.
    pub info: ExtendedNodeInfo,
    /// When the peer was last seen (RFC3339).
    pub last_health_check: String,
    /// Number of failed health checks.
    pub consecutive_failures: u32,
}

impl PeerRegistry {
    /// Create a new empty peer registry.
    pub fn new(health_config: HealthConfig) -> Self {
        Self {
            peers: Mutex::new(HashMap::new()),
            health_config,
        }
    }

    /// Register or update a peer.
    pub fn upsert(&self, info: ExtendedNodeInfo) {
        let now = chrono::Utc::now().to_rfc3339();
        let mut peers = self.peers.lock();
        if let Some(existing) = peers.get_mut(&info.base.id) {
            existing.info = info;
            existing.last_health_check = now;
            existing.consecutive_failures = 0;
        } else {
            peers.insert(
                info.base.id.clone(),
                PeerEntry {
                    info,
                    last_health_check: now,
                    consecutive_failures: 0,
                },
            );
        }
    }

    /// Remove a peer by node ID.
    pub fn remove(&self, node_id: &str) -> bool {
        self.peers.lock().remove(node_id).is_some()
    }

    /// Get info about a specific peer.
    pub fn get(&self, node_id: &str) -> Option<ExtendedNodeInfo> {
        self.peers.lock().get(node_id).map(|e| e.info.clone())
    }

    /// Record a successful health check for a peer.
    pub fn mark_healthy(&self, node_id: &str) {
        let now = chrono::Utc::now().to_rfc3339();
        if let Some(entry) = self.peers.lock().get_mut(node_id) {
            entry.last_health_check = now;
            entry.consecutive_failures = 0;
            entry.info.status = NodeStatus::Online;
        }
    }

    /// Record a failed health check. Returns `true` if the peer should be evicted.
    pub fn mark_failed(&self, node_id: &str) -> bool {
        if let Some(entry) = self.peers.lock().get_mut(node_id) {
            entry.consecutive_failures += 1;
            if entry.consecutive_failures >= 3 {
                entry.info.status = NodeStatus::Offline;
                return entry.consecutive_failures >= 5;
            }
        }
        false
    }

    /// List all known peers.
    pub fn list_peers(&self) -> Vec<ExtendedNodeInfo> {
        self.peers.lock().values().map(|e| e.info.clone()).collect()
    }

    /// List only online peers.
    pub fn list_online(&self) -> Vec<ExtendedNodeInfo> {
        self.peers
            .lock()
            .values()
            .filter(|e| e.info.status == NodeStatus::Online)
            .map(|e| e.info.clone())
            .collect()
    }

    /// Return the number of registered peers.
    pub fn len(&self) -> usize {
        self.peers.lock().len()
    }

    /// Return whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.peers.lock().is_empty()
    }

    /// Find all online peers that have a specific capability.
    pub fn find_by_capability(&self, capability: &str) -> Vec<ExtendedNodeInfo> {
        self.peers
            .lock()
            .values()
            .filter(|e| {
                e.info.status == NodeStatus::Online
                    && e.info.capabilities.iter().any(|c| c == capability)
            })
            .map(|e| e.info.clone())
            .collect()
    }

    /// Find all online peers that have ANY of the specified capabilities.
    pub fn find_by_capabilities(&self, capabilities: &[String]) -> Vec<ExtendedNodeInfo> {
        self.peers
            .lock()
            .values()
            .filter(|e| {
                e.info.status == NodeStatus::Online
                    && e.info
                        .capabilities
                        .iter()
                        .any(|c| capabilities.contains(c))
            })
            .map(|e| e.info.clone())
            .collect()
    }

    /// Collect all unique capabilities across all nodes (online and offline).
    ///
    /// Mirrors Go's `Registry.GetCapabilities()`.
    pub fn get_capabilities(&self) -> Vec<String> {
        let mut cap_set = std::collections::HashSet::new();
        for entry in self.peers.lock().values() {
            for cap in &entry.info.capabilities {
                cap_set.insert(cap.clone());
            }
        }
        let mut caps: Vec<String> = cap_set.into_iter().collect();
        caps.sort();
        caps
    }

    /// Find all online peers that have a specific capability.
    ///
    /// This is identical to `find_by_capability` (which already filters online
    /// peers) and is provided for API parity with Go's `FindByCapabilityOnline`.
    pub fn find_by_capability_online(&self, capability: &str) -> Vec<ExtendedNodeInfo> {
        self.find_by_capability(capability)
    }

    /// Mark a peer as offline with an optional reason.
    ///
    /// Mirrors Go's `Registry.MarkOffline(nodeID, reason)`.
    pub fn mark_offline(&self, node_id: &str, _reason: &str) {
        if let Some(entry) = self.peers.lock().get_mut(node_id) {
            entry.info.status = NodeStatus::Offline;
            entry.consecutive_failures = 0;
        }
    }

    /// Check all online nodes and mark those as offline whose last health check
    /// is older than the given timeout duration.
    ///
    /// Returns the list of expired node IDs.
    /// Mirrors Go's `Registry.CheckTimeouts(timeout)`.
    pub fn check_timeouts(&self, timeout: std::time::Duration) -> Vec<String> {
        let now = chrono::Utc::now();
        let threshold = now - chrono::Duration::from_std(timeout).unwrap_or(chrono::Duration::seconds(90));

        let mut peers = self.peers.lock();
        let mut expired = Vec::new();

        for (id, entry) in peers.iter_mut() {
            if entry.info.status != NodeStatus::Online {
                continue;
            }
            if let Ok(last_check) = chrono::DateTime::parse_from_rfc3339(&entry.last_health_check) {
                let last_check_utc = last_check.with_timezone(&chrono::Utc);
                if last_check_utc < threshold {
                    entry.info.status = NodeStatus::Offline;
                    expired.push(id.clone());
                }
            }
        }

        expired
    }

    /// Return the count of online peers.
    ///
    /// Mirrors Go's `Registry.OnlineCount()`.
    pub fn online_count(&self) -> usize {
        self.peers
            .lock()
            .values()
            .filter(|e| e.info.status == NodeStatus::Online)
            .count()
    }

    /// Check all peers and remove those that have been offline/failed for longer
    /// than the eviction timeout.
    ///
    /// Uses `HealthConfig.eviction_timeout_secs` to compute the threshold.
    /// Compares `last_health_check` timestamp against `now - eviction_timeout`.
    /// Returns the list of evicted node IDs.
    pub fn evict_stale(&self) -> Vec<String> {
        let now = chrono::Utc::now();
        let threshold = now - chrono::Duration::seconds(self.health_config.eviction_timeout_secs as i64);

        let mut peers = self.peers.lock();
        let to_evict: Vec<String> = peers
            .iter()
            .filter_map(|(id, entry)| {
                // Only evict peers that are already offline (stale/failed)
                if entry.info.status != NodeStatus::Online {
                    if let Ok(last_check) = chrono::DateTime::parse_from_rfc3339(&entry.last_health_check) {
                        let last_check_utc = last_check.with_timezone(&chrono::Utc);
                        if last_check_utc < threshold {
                            return Some(id.clone());
                        }
                    }
                }
                None
            })
            .collect();

        for id in &to_evict {
            peers.remove(id);
        }

        to_evict
    }

    /// Mark peers as stale/offline if `now - last_health_check > stale_timeout_secs`.
    ///
    /// Returns the list of newly stale node IDs (peers that were Online and
    /// are now marked Offline because their last health check is too old).
    pub fn check_health(&self) -> Vec<String> {
        let now = chrono::Utc::now();
        let threshold = now - chrono::Duration::seconds(self.health_config.stale_timeout_secs as i64);

        let mut peers = self.peers.lock();
        let mut newly_stale = Vec::new();

        for (id, entry) in peers.iter_mut() {
            // Only transition peers that are currently Online
            if entry.info.status != NodeStatus::Online {
                continue;
            }

            if let Ok(last_check) = chrono::DateTime::parse_from_rfc3339(&entry.last_health_check) {
                let last_check_utc = last_check.with_timezone(&chrono::Utc);
                if last_check_utc < threshold {
                    entry.info.status = NodeStatus::Offline;
                    newly_stale.push(id.clone());
                }
            }
        }

        newly_stale
    }
}

#[cfg(test)]
mod tests {
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
}
