use super::*;
use crate::registry::Tool;

fn make_connected_tool() -> ClusterRpcTool {
    let mut stub = StubClusterOps::connected("node-1");
    stub.peers = vec![PeerInfo {
        id: "node-2".to_string(),
        name: "Bot2".to_string(),
        capabilities: vec!["chat".to_string(), "tools".to_string()],
        status: "online".to_string(),
    }];
    stub.capabilities = vec![
        "chat".to_string(),
        "tools".to_string(),
        "translate".to_string(),
    ];
    ClusterRpcTool::with_cluster(Arc::new(stub))
}

#[tokio::test]
async fn test_cluster_rpc_disconnected() {
    let tool = ClusterRpcTool::new();
    let result = tool
        .execute(&serde_json::json!({
            "peer_id": "node-2",
            "action": "peer_chat",
            "data": {"message": "hello"}
        }))
        .await;
    assert!(result.is_error);
    assert!(
        result.for_llm.contains("not connected"),
        "Expected 'not connected' error, got: {}",
        result.for_llm
    );
}

#[tokio::test]
async fn test_cluster_rpc_connected_returns_async() {
    let tool = make_connected_tool();
    let result = tool
        .execute(&serde_json::json!({
            "peer_id": "node-2",
            "action": "peer_chat",
            "data": {"message": "hello"}
        }))
        .await;
    assert!(
        !result.is_error,
        "Expected success for connected cluster, got: {}",
        result.for_llm
    );
    assert!(result.is_async, "Expected async result");
    assert!(result.task_id.is_some(), "Expected task ID");
}

#[tokio::test]
async fn test_contextual_tool_set_context() {
    let mut tool = ClusterRpcTool::new();
    let ctx = crate::registry::ToolExecutionContext {
        channel: "rpc".to_string(),
        chat_id: "chat-456".to_string(),
        ..Default::default()
    };
    crate::registry::ContextualTool::set_context(&mut tool, &ctx);

    // Allow a small delay for the mutex to be released
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    let channel = tool.channel().await;
    let chat_id = tool.chat_id().await;
    assert_eq!(channel, "rpc");
    assert_eq!(chat_id, "chat-456");
}

#[tokio::test]
async fn test_missing_peer_id() {
    let tool = make_connected_tool();
    let result = tool
        .execute(&serde_json::json!({
            "action": "peer_chat"
        }))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("peer_id"));
}

#[tokio::test]
async fn test_missing_action() {
    let tool = make_connected_tool();
    let result = tool
        .execute(&serde_json::json!({
            "peer_id": "node-2"
        }))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("action"));
}

#[tokio::test]
async fn test_sync_call_success() {
    let tool = make_connected_tool();
    let result = tool
        .execute(&serde_json::json!({
            "peer_id": "node-2",
            "action": "ping"
        }))
        .await;
    assert!(
        !result.is_error,
        "Expected success for sync call, got: {}",
        result.for_llm
    );
    assert!(result.silent, "Sync call results should be silent");
    assert!(result.for_llm.contains("stub response"));
}

#[test]
fn test_get_available_peers() {
    let tool = make_connected_tool();
    let result = tool.get_available_peers().unwrap();
    assert!(result.contains("node-2"));
    assert!(result.contains("Bot2"));
}

#[test]
fn test_get_available_peers_empty() {
    let tool = ClusterRpcTool::new();
    let result = tool.get_available_peers().unwrap();
    assert_eq!(result, "No other bots currently online");
}

#[test]
fn test_get_capabilities() {
    let tool = make_connected_tool();
    let result = tool.get_capabilities().unwrap();
    assert!(result.contains("chat"));
    assert!(result.contains("translate"));
}

#[test]
fn test_get_capabilities_empty() {
    let tool = ClusterRpcTool::new();
    let result = tool.get_capabilities().unwrap();
    assert_eq!(result, "No capabilities available");
}

#[test]
fn test_is_connected() {
    let disconnected = ClusterRpcTool::new();
    assert!(!disconnected.is_connected());

    let connected = make_connected_tool();
    assert!(connected.is_connected());
}

// --- Additional cluster_rpc tests ---

#[test]
fn test_stub_cluster_ops_default() {
    let ops = StubClusterOps::default();
    assert!(!ops.connected);
    assert!(ops.node_id.is_empty());
    assert!(ops.peers.is_empty());
    assert!(ops.capabilities.is_empty());
}

#[test]
fn test_stub_cluster_ops_connected() {
    let ops = StubClusterOps::connected("test-node");
    assert!(ops.connected);
    assert_eq!(ops.node_id, "test-node");
}

#[test]
fn test_stub_cluster_ops_node_id() {
    let ops = StubClusterOps::connected("my-node");
    assert_eq!(ops.node_id(), "my-node");
}

#[test]
fn test_stub_cluster_ops_is_connected() {
    let connected = StubClusterOps::connected("x");
    assert!(connected.is_connected());
    let disconnected = StubClusterOps::new();
    assert!(!disconnected.is_connected());
}

#[test]
fn test_stub_cluster_ops_get_online_peers() {
    let ops = StubClusterOps {
        node_id: "n1".into(),
        connected: true,
        peers: vec![PeerInfo {
            id: "n2".into(),
            name: "Bot2".into(),
            capabilities: vec!["chat".into()],
            status: "online".into(),
        }],
        capabilities: vec![],
    };
    let peers = ops.get_online_peers();
    assert_eq!(peers.len(), 1);
    assert_eq!(peers[0].id, "n2");
}

#[test]
fn test_stub_cluster_ops_get_capabilities() {
    let ops = StubClusterOps {
        capabilities: vec!["chat".into(), "translate".into()],
        ..StubClusterOps::default()
    };
    let caps = ops.get_capabilities();
    assert_eq!(caps.len(), 2);
}

#[test]
fn test_stub_cluster_ops_get_local_ips() {
    let ops = StubClusterOps::new();
    let ips = ops.get_local_ips();
    assert!(!ips.is_empty());
    assert!(ips.contains(&"127.0.0.1".to_string()));
}

#[test]
fn test_stub_cluster_ops_get_rpc_port() {
    let ops = StubClusterOps::new();
    assert_eq!(ops.get_rpc_port(), 0);
}

#[test]
fn test_stub_cluster_ops_call_with_context() {
    let ops = StubClusterOps::connected("node-1");
    let result = ops.call_with_context("peer", "ping", &serde_json::json!({}));
    assert!(result.is_ok());
    assert!(result.unwrap().contains("stub response"));
}

#[test]
fn test_stub_cluster_ops_call_with_context_disconnected() {
    let ops = StubClusterOps::new();
    let result = ops.call_with_context("peer", "ping", &serde_json::json!({}));
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not connected"));
}

#[test]
fn test_stub_cluster_ops_submit_task() {
    let ops = StubClusterOps::connected("n1");
    let result = ops.submit_task("peer", "action", &serde_json::json!({}), "ch", "chat1");
    assert!(result.is_ok());
    assert!(result.unwrap().starts_with("task-"));
}

#[test]
fn test_peer_info_serialization() {
    let info = PeerInfo {
        id: "node-1".into(),
        name: "Bot1".into(),
        capabilities: vec!["chat".into()],
        status: "online".into(),
    };
    let json = serde_json::to_string(&info).unwrap();
    let back: PeerInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(back.id, "node-1");
    assert_eq!(back.name, "Bot1");
    assert_eq!(back.capabilities.len(), 1);
    assert_eq!(back.status, "online");
}

#[tokio::test]
async fn test_cluster_rpc_set_cluster_ops() {
    let ops = Arc::new(StubClusterOps::connected("new-node"));
    let tool = ClusterRpcTool::with_cluster(ops);
    assert!(tool.is_connected());
}

#[tokio::test]
async fn test_cluster_rpc_non_object_args() {
    let tool = make_connected_tool();
    let result = tool.execute(&serde_json::json!("not an object")).await;
    assert!(result.is_error);
}

// ============================================================
// Additional coverage tests for 95%+ target
// ============================================================

#[test]
fn test_cluster_rpc_tool_metadata() {
    let tool = ClusterRpcTool::new();
    assert_eq!(tool.name(), "cluster_rpc");
    assert!(!tool.description().is_empty());
    let params = tool.parameters();
    assert_eq!(params["type"], "object");
    assert!(params["properties"]["peer_id"].is_object());
    assert!(params["properties"]["action"].is_object());
}

#[test]
fn test_cluster_rpc_tool_default() {
    let tool = ClusterRpcTool::default();
    assert!(!tool.is_connected());
}

#[tokio::test]
async fn test_cluster_rpc_channel_and_chat_id() {
    let tool = ClusterRpcTool::new();
    assert_eq!(tool.channel().await, "");
    assert_eq!(tool.chat_id().await, "");
}

#[tokio::test]
async fn test_cluster_rpc_set_context_and_read() {
    let mut tool = ClusterRpcTool::new();
    let ctx = crate::registry::ToolExecutionContext {
        channel: "web".to_string(),
        chat_id: "chat-789".to_string(),
        ..Default::default()
    };
    ContextualTool::set_context(&mut tool, &ctx);
    assert_eq!(tool.channel().await, "web");
    assert_eq!(tool.chat_id().await, "chat-789");
}

#[tokio::test]
async fn test_cluster_rpc_missing_peer_id() {
    let tool = make_connected_tool();
    let result = tool
        .execute(&serde_json::json!({"action": "peer_chat"}))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("peer_id"));
}

#[tokio::test]
async fn test_cluster_rpc_empty_peer_id() {
    let tool = make_connected_tool();
    let result = tool
        .execute(&serde_json::json!({"peer_id": "", "action": "peer_chat"}))
        .await;
    assert!(result.is_error);
}

#[tokio::test]
async fn test_cluster_rpc_missing_action() {
    let tool = make_connected_tool();
    let result = tool
        .execute(&serde_json::json!({"peer_id": "node-2"}))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("action"));
}

#[tokio::test]
async fn test_cluster_rpc_empty_action() {
    let tool = make_connected_tool();
    let result = tool
        .execute(&serde_json::json!({"peer_id": "node-2", "action": ""}))
        .await;
    assert!(result.is_error);
}

#[tokio::test]
async fn test_cluster_rpc_sync_call_success() {
    let tool = make_connected_tool();
    let result = tool
        .execute(&serde_json::json!({
            "peer_id": "node-2",
            "action": "ping"
        }))
        .await;
    assert!(!result.is_error);
    assert!(result.silent);
    assert!(result.for_llm.contains("stub response"));
}

#[tokio::test]
async fn test_cluster_rpc_sync_call_with_data() {
    let tool = make_connected_tool();
    let result = tool
        .execute(&serde_json::json!({
            "peer_id": "node-2",
            "action": "get_info",
            "data": {"key": "value"}
        }))
        .await;
    assert!(!result.is_error);
    assert!(result.silent);
}

#[tokio::test]
async fn test_cluster_rpc_async_peer_chat_with_data() {
    let tool = make_connected_tool();
    let result = tool
        .execute(&serde_json::json!({
            "peer_id": "node-2",
            "action": "peer_chat",
            "data": {"message": "hello from test"}
        }))
        .await;
    assert!(!result.is_error);
    assert!(result.is_async);
    let task_id = result.task_id.unwrap();
    assert!(task_id.starts_with("task-"));
}

#[tokio::test]
async fn test_cluster_rpc_async_peer_chat_no_data() {
    let tool = make_connected_tool();
    let result = tool
        .execute(&serde_json::json!({
            "peer_id": "node-2",
            "action": "peer_chat"
        }))
        .await;
    assert!(!result.is_error);
    assert!(result.is_async);
}

#[test]
fn test_stub_cluster_ops_submit_task_disconnected() {
    let ops = StubClusterOps::new();
    // submit_task works even when disconnected (it's just stub)
    let result = ops.submit_task("peer", "action", &serde_json::json!({}), "ch", "chat1");
    assert!(result.is_ok());
    assert!(result.unwrap().starts_with("task-"));
}

#[test]
fn test_peer_info_deserialize() {
    let json = r#"{"id":"n1","name":"Bot1","capabilities":["chat","tools"],"status":"online"}"#;
    let info: PeerInfo = serde_json::from_str(json).unwrap();
    assert_eq!(info.id, "n1");
    assert_eq!(info.capabilities.len(), 2);
}

#[test]
fn test_stub_cluster_default() {
    let ops = StubClusterOps::default();
    assert!(!ops.connected);
    assert!(ops.node_id.is_empty());
}
