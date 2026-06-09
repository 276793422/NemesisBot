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
        let now = chrono::Local::now().to_rfc3339();
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

    /// Register or update a peer only if the content has actually changed.
    ///
    /// Returns `true` if a new peer was inserted or an existing peer was
    /// updated with different content. Returns `false` when the incoming
    /// data is identical to what's already stored (health timestamps are
    /// always refreshed regardless).
    pub fn upsert_if_changed(&self, info: ExtendedNodeInfo) -> bool {
        let now = chrono::Local::now().to_rfc3339();
        let mut peers = self.peers.lock();
        if let Some(existing) = peers.get_mut(&info.base.id) {
            if existing.info.content_eq(&info) {
                existing.last_health_check = now;
                existing.consecutive_failures = 0;
                false
            } else {
                existing.info = info;
                existing.last_health_check = now;
                existing.consecutive_failures = 0;
                true
            }
        } else {
            peers.insert(
                info.base.id.clone(),
                PeerEntry {
                    info,
                    last_health_check: now,
                    consecutive_failures: 0,
                },
            );
            true
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
        let now = chrono::Local::now().to_rfc3339();
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
        let now = chrono::Local::now();
        let threshold = now - chrono::Duration::from_std(timeout).unwrap_or(chrono::Duration::seconds(90));

        let mut peers = self.peers.lock();
        let mut expired = Vec::new();

        for (id, entry) in peers.iter_mut() {
            if entry.info.status != NodeStatus::Online {
                continue;
            }
            if let Ok(last_check) = chrono::DateTime::parse_from_rfc3339(&entry.last_health_check) {
                let last_check_utc = last_check.with_timezone(&chrono::Local);
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
        let now = chrono::Local::now();
        let threshold = now - chrono::Duration::seconds(self.health_config.eviction_timeout_secs as i64);

        let mut peers = self.peers.lock();
        let to_evict: Vec<String> = peers
            .iter()
            .filter_map(|(id, entry)| {
                // Only evict peers that are already offline (stale/failed)
                if entry.info.status != NodeStatus::Online {
                    if let Ok(last_check) = chrono::DateTime::parse_from_rfc3339(&entry.last_health_check) {
                        let last_check_utc = last_check.with_timezone(&chrono::Local);
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
        let now = chrono::Local::now();
        let threshold = now - chrono::Duration::seconds(self.health_config.stale_timeout_secs as i64);

        let mut peers = self.peers.lock();
        let mut newly_stale = Vec::new();

        for (id, entry) in peers.iter_mut() {
            // Only transition peers that are currently Online
            if entry.info.status != NodeStatus::Online {
                continue;
            }

            if let Ok(last_check) = chrono::DateTime::parse_from_rfc3339(&entry.last_health_check) {
                let last_check_utc = last_check.with_timezone(&chrono::Local);
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
mod tests;
