//! Forge - Cluster bridge interface for cross-node learning.
//!
//! Defines the `ClusterForgeBridge` trait that decouples the forge module
//! from the cluster module, preventing circular dependencies.

use async_trait::async_trait;

/// Bridge interface for forge-to-cluster communication.
///
/// Implemented by the cluster side to provide RPC capabilities to forge
/// without creating a circular dependency between the two crates.
#[async_trait]
pub trait ClusterForgeBridge: Send + Sync {
    /// Share a reflection report with all online peers.
    async fn share_reflection(&self, report_json: serde_json::Value) -> Result<usize, String>;

    /// Request reflection reports from all online peers.
    async fn get_remote_reflections(&self) -> Result<Vec<serde_json::Value>, String>;

    /// Get the list of currently online peer node IDs.
    async fn get_online_peers(&self) -> Result<Vec<String>, String>;

    /// Get the local node ID.
    fn local_node_id(&self) -> &str;

    /// Check whether the cluster is currently enabled and connected.
    fn is_cluster_enabled(&self) -> bool;
}

/// A no-op bridge implementation for when clustering is disabled.
pub struct NoOpBridge {
    node_id: String,
}

impl NoOpBridge {
    /// Create a new no-op bridge with the given node ID.
    pub fn new(node_id: String) -> Self {
        Self { node_id }
    }
}

#[async_trait]
impl ClusterForgeBridge for NoOpBridge {
    async fn share_reflection(&self, _report_json: serde_json::Value) -> Result<usize, String> {
        Ok(0)
    }

    async fn get_remote_reflections(&self) -> Result<Vec<serde_json::Value>, String> {
        Ok(Vec::new())
    }

    async fn get_online_peers(&self) -> Result<Vec<String>, String> {
        Ok(Vec::new())
    }

    fn local_node_id(&self) -> &str {
        &self.node_id
    }

    fn is_cluster_enabled(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests;
