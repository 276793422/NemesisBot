//! Cluster-specific types.
//!
//! Re-exports canonical types from nemesis-types and adds cluster-level wrappers.

use serde::{Deserialize, Serialize};

// Re-export the canonical types from nemesis-types.
pub use nemesis_types::cluster::{NodeInfo, NodeRole, RpcMessage, Task, TaskStatus};

/// Cluster configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterConfig {
    /// Unique identifier for this node.
    pub node_id: String,
    /// Address to bind to (e.g. "0.0.0.0:9000").
    pub bind_address: String,
    /// Known peer addresses.
    pub peers: Vec<String>,
}

impl Default for ClusterConfig {
    fn default() -> Self {
        Self {
            node_id: String::new(),
            bind_address: "0.0.0.0:9000".into(),
            peers: Vec::new(),
        }
    }
}

/// Node status (extends the nemesis-types NodeRole/NodeInfo).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeStatus {
    Online,
    Offline,
    Connecting,
}

/// Extended node info including status and capabilities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtendedNodeInfo {
    #[serde(flatten)]
    pub base: NodeInfo,
    pub status: NodeStatus,
    pub capabilities: Vec<String>,
    /// All known addresses for this node (for multi-address failover).
    /// The primary address is stored in `base.address`.
    #[serde(default)]
    pub addresses: Vec<String>,
    /// Node type: "agent" (full with LLM) or "node" (lightweight, no LLM).
    #[serde(default)]
    pub node_type: String,
}

impl ExtendedNodeInfo {
    /// Returns true if the node is online.
    ///
    /// Mirrors Go's `Node.IsOnline()`.
    pub fn is_online(&self) -> bool {
        self.status == NodeStatus::Online
    }

    /// Returns the current status as a string.
    ///
    /// Mirrors Go's `Node.GetStatus()`.
    pub fn get_status_string(&self) -> &'static str {
        match self.status {
            NodeStatus::Online => "online",
            NodeStatus::Offline => "offline",
            NodeStatus::Connecting => "connecting",
        }
    }

    /// Update the node status and set last_seen to now.
    ///
    /// Mirrors Go's `Node.SetStatus()`.
    pub fn set_status(&mut self, status: NodeStatus) {
        self.status = status;
        self.base.last_seen = chrono::Local::now().to_rfc3339();
    }

    /// Update the last_seen timestamp and set status to Online.
    ///
    /// Mirrors Go's `Node.UpdateLastSeen()`.
    pub fn update_last_seen(&mut self) {
        self.base.last_seen = chrono::Local::now().to_rfc3339();
        if self.status != NodeStatus::Online {
            self.status = NodeStatus::Online;
        }
    }

    /// Mark the node as offline with an optional reason.
    ///
    /// Mirrors Go's `Node.MarkOffline(reason)`.
    pub fn mark_offline(&mut self, reason: &str) {
        self.status = NodeStatus::Offline;
        // Store reason in last_seen field as a convention (Go stores in LastError)
        // We don't have a dedicated last_error field on ExtendedNodeInfo,
        // so we just update status. The reason is logged by the caller.
        let _ = reason;
    }

    /// Convert to a PeerConfig for TOML serialization.
    ///
    /// Mirrors Go's `Node.ToConfig()`.
    pub fn to_peer_config(&self) -> crate::cluster_config::PeerConfig {
        use crate::cluster_config::{PeerConfig, PeerStatus};

        PeerConfig {
            id: self.base.id.clone(),
            name: self.base.name.clone(),
            address: self.base.address.clone(),
            addresses: self.addresses.clone(),
            rpc_port: 0,
            role: match self.base.role {
                NodeRole::Master => "master".into(),
                NodeRole::Worker => "worker".into(),
            },
            category: self.base.category.clone(),
            priority: 1,
            enabled: true,
            status: PeerStatus {
                state: self.get_status_string().into(),
                last_seen: self.base.last_seen.clone(),
                uptime: format!("{:?}", self.get_uptime()),
                tasks_completed: 0,
                success_rate: 0.0,
                avg_response_time: 0,
                last_error: String::new(),
            },
        }
    }

    /// Check if the node has a specific capability (case-insensitive).
    ///
    /// Mirrors Go's `Node.HasCapability(capability)`.
    pub fn has_capability(&self, capability: &str) -> bool {
        let lower = capability.to_lowercase();
        self.capabilities.iter().any(|c| c.to_lowercase() == lower)
    }

    /// Compare stable content fields, excluding `base.last_seen` timestamp.
    ///
    /// Used by the registry to skip redundant upserts when a periodic
    /// broadcast arrives with identical content.
    pub fn content_eq(&self, other: &ExtendedNodeInfo) -> bool {
        self.base.id == other.base.id
            && self.base.name == other.base.name
            && self.base.role == other.base.role
            && self.base.address == other.base.address
            && self.base.category == other.base.category
            && self.status == other.status
            && self.capabilities == other.capabilities
            && self.addresses == other.addresses
            && self.node_type == other.node_type
    }

    /// Returns the node ID.
    ///
    /// Mirrors Go's `Node.GetID()`.
    pub fn get_id(&self) -> &str {
        &self.base.id
    }

    /// Returns the node name.
    ///
    /// Mirrors Go's `Node.GetName()`.
    pub fn get_name(&self) -> &str {
        &self.base.name
    }

    /// Returns the node address.
    ///
    /// Mirrors Go's `Node.GetAddress()`.
    pub fn get_address(&self) -> &str {
        &self.base.address
    }

    /// Returns the node capabilities.
    ///
    /// Mirrors Go's `Node.GetCapabilities()`.
    pub fn get_capabilities(&self) -> &[String] {
        &self.capabilities
    }

    /// Returns the uptime duration since the node was last seen.
    ///
    /// Mirrors Go's `Node.GetUptime()`. Returns `Duration::ZERO` if
    /// `last_seen` is empty or unparsed.
    pub fn get_uptime(&self) -> std::time::Duration {
        if self.base.last_seen.is_empty() {
            return std::time::Duration::ZERO;
        }

        // Try to parse the last_seen timestamp (RFC 3339 format)
        if let Ok(last_seen) = chrono::DateTime::parse_from_rfc3339(&self.base.last_seen) {
            let now = chrono::Local::now();
            let last_seen_local = last_seen.with_timezone(&chrono::Local);
            if now > last_seen_local {
                (now - last_seen_local)
                    .to_std()
                    .unwrap_or(std::time::Duration::ZERO)
            } else {
                std::time::Duration::ZERO
            }
        } else {
            std::time::Duration::ZERO
        }
    }
}

impl std::fmt::Display for ExtendedNodeInfo {
    /// Mirrors Go's `Node.String()`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Node{{id={}, name={}, address={}, status={}}}",
            self.base.id,
            self.base.name,
            self.base.address,
            self.get_status_string()
        )
    }
}

#[cfg(test)]
mod tests;
