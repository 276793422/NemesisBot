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
    async fn share_reflection(
        &self,
        report_json: serde_json::Value,
    ) -> Result<usize, String>;

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
    async fn share_reflection(
        &self,
        _report_json: serde_json::Value,
    ) -> Result<usize, String> {
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
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_noop_bridge_share() {
        let bridge = NoOpBridge::new("node-1".into());
        let count = bridge.share_reflection(serde_json::json!({})).await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_noop_bridge_get_reflections() {
        let bridge = NoOpBridge::new("node-1".into());
        let reflections = bridge.get_remote_reflections().await.unwrap();
        assert!(reflections.is_empty());
    }

    #[tokio::test]
    async fn test_noop_bridge_local_id() {
        let bridge = NoOpBridge::new("node-1".into());
        assert_eq!(bridge.local_node_id(), "node-1");
    }

    #[test]
    fn test_noop_bridge_is_cluster_enabled() {
        let bridge = NoOpBridge::new("node-1".into());
        assert!(!bridge.is_cluster_enabled());
    }

    // --- Additional bridge tests ---

    #[tokio::test]
    async fn test_noop_bridge_get_online_peers() {
        let bridge = NoOpBridge::new("test-node".into());
        let peers = bridge.get_online_peers().await.unwrap();
        assert!(peers.is_empty());
    }

    #[tokio::test]
    async fn test_noop_bridge_share_returns_zero() {
        let bridge = NoOpBridge::new("node-2".into());
        let report = serde_json::json!({"insights": ["test"], "patterns": ["p1"]});
        let count = bridge.share_reflection(report).await.unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_noop_bridge_different_node_ids() {
        let bridge1 = NoOpBridge::new("alpha".into());
        let bridge2 = NoOpBridge::new("beta".into());
        assert_eq!(bridge1.local_node_id(), "alpha");
        assert_eq!(bridge2.local_node_id(), "beta");
    }

    #[test]
    fn test_noop_bridge_empty_node_id() {
        let bridge = NoOpBridge::new(String::new());
        assert_eq!(bridge.local_node_id(), "");
    }

    #[tokio::test]
    async fn test_noop_bridge_get_remote_reflections_empty() {
        let bridge = NoOpBridge::new("node-test".into());
        let result = bridge.get_remote_reflections().await.unwrap();
        assert!(result.is_empty());
    }
}
