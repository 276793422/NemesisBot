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
