//! Cluster RPC tool - inter-node communication within the cluster.
//!
//! Provides `ClusterRpcTool` which enables agents to make RPC calls to other
//! nodes in the cluster. Supports both synchronous calls (e.g., `ping`,
//! `get_capabilities`) and asynchronous calls (`peer_chat`) that return a
//! task ID for later correlation via continuation snapshots.

use crate::registry::ContextualTool;
use crate::types::ToolResult;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

/// Information about a peer node in the cluster.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    pub id: String,
    pub name: String,
    pub capabilities: Vec<String>,
    pub status: String,
}

/// Cluster operations that the tool delegates to an external cluster implementation.
///
/// In a full deployment, this trait is implemented by the actual `Cluster` struct.
/// For testing and standalone use, `StubClusterOps` provides a simple mock.
pub trait ClusterOps: Send + Sync {
    /// Get the node ID of this instance.
    fn node_id(&self) -> String;

    /// Check whether the cluster is connected.
    fn is_connected(&self) -> bool;

    /// Get a list of online peers.
    fn get_online_peers(&self) -> Vec<PeerInfo>;

    /// Get all capabilities available in the cluster.
    fn get_capabilities(&self) -> Vec<String>;

    /// Submit an asynchronous task (e.g., peer_chat).
    /// Returns the submitted task ID.
    fn submit_task(
        &self,
        peer_id: &str,
        action: &str,
        payload: &serde_json::Value,
        origin_channel: &str,
        origin_chat_id: &str,
    ) -> Result<String, String>;

    /// Make a synchronous RPC call.
    fn call_with_context(
        &self,
        peer_id: &str,
        action: &str,
        payload: &serde_json::Value,
    ) -> Result<String, String>;

    /// Get local IPs for source injection.
    fn get_local_ips(&self) -> Vec<String>;

    /// Get the RPC port for source injection.
    fn get_rpc_port(&self) -> u16;
}

/// Stub cluster operations for testing and when no cluster is available.
#[derive(Debug, Clone)]
pub struct StubClusterOps {
    pub node_id: String,
    pub connected: bool,
    pub peers: Vec<PeerInfo>,
    pub capabilities: Vec<String>,
}

impl StubClusterOps {
    pub fn new() -> Self {
        Self {
            node_id: String::new(),
            connected: false,
            peers: Vec::new(),
            capabilities: Vec::new(),
        }
    }

    pub fn connected(node_id: &str) -> Self {
        Self {
            node_id: node_id.to_string(),
            connected: true,
            peers: Vec::new(),
            capabilities: Vec::new(),
        }
    }
}

impl Default for StubClusterOps {
    fn default() -> Self {
        Self::new()
    }
}

impl ClusterOps for StubClusterOps {
    fn node_id(&self) -> String {
        self.node_id.clone()
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    fn get_online_peers(&self) -> Vec<PeerInfo> {
        self.peers.clone()
    }

    fn get_capabilities(&self) -> Vec<String> {
        self.capabilities.clone()
    }

    fn submit_task(
        &self,
        peer_id: &str,
        action: &str,
        _payload: &serde_json::Value,
        _origin_channel: &str,
        _origin_chat_id: &str,
    ) -> Result<String, String> {
        let task_id = format!(
            "task-{}-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
            peer_id
        );
        info!(
            task_id = %task_id,
            peer_id = %peer_id,
            action = %action,
            "StubClusterOps: submitted async task"
        );
        Ok(task_id)
    }

    fn call_with_context(
        &self,
        peer_id: &str,
        action: &str,
        _payload: &serde_json::Value,
    ) -> Result<String, String> {
        if !self.connected {
            return Err("cluster not connected".to_string());
        }
        Ok(format!("stub response from {} for action {}", peer_id, action))
    }

    fn get_local_ips(&self) -> Vec<String> {
        vec!["127.0.0.1".to_string()]
    }

    fn get_rpc_port(&self) -> u16 {
        0
    }
}

/// Cluster RPC tool - sends requests to other nodes in the cluster.
pub struct ClusterRpcTool {
    cluster: Arc<dyn ClusterOps>,
    channel: Arc<Mutex<String>>,
    chat_id: Arc<Mutex<String>>,
}

impl ClusterRpcTool {
    /// Create a new cluster RPC tool with stub operations (disconnected).
    pub fn new() -> Self {
        Self {
            cluster: Arc::new(StubClusterOps::new()),
            channel: Arc::new(Mutex::new(String::new())),
            chat_id: Arc::new(Mutex::new(String::new())),
        }
    }

    /// Create with a specific cluster operations provider.
    pub fn with_cluster(cluster: Arc<dyn ClusterOps>) -> Self {
        Self {
            cluster,
            channel: Arc::new(Mutex::new(String::new())),
            chat_id: Arc::new(Mutex::new(String::new())),
        }
    }

    /// Get the current channel context.
    pub async fn channel(&self) -> String {
        self.channel.lock().await.clone()
    }

    /// Get the current chat ID context.
    pub async fn chat_id(&self) -> String {
        self.chat_id.lock().await.clone()
    }

    /// Check if the cluster is connected.
    pub fn is_connected(&self) -> bool {
        self.cluster.is_connected()
    }

    /// Get available peer nodes.
    ///
    /// Returns a JSON-formatted string listing all online peers with their
    /// ID, name, capabilities, and status.
    pub fn get_available_peers(&self) -> Result<String, String> {
        let peers = self.cluster.get_online_peers();
        if peers.is_empty() {
            return Ok("No other bots currently online".to_string());
        }

        serde_json::to_string_pretty(&peers)
            .map_err(|e| format!("failed to marshal peers: {}", e))
    }

    /// Get all available capabilities in the cluster.
    ///
    /// Returns a JSON-formatted string listing all capabilities.
    pub fn get_capabilities(&self) -> Result<String, String> {
        let caps = self.cluster.get_capabilities();
        if caps.is_empty() {
            return Ok("No capabilities available".to_string());
        }

        serde_json::to_string_pretty(&caps)
            .map_err(|e| format!("failed to marshal capabilities: {}", e))
    }

    /// Execute an asynchronous peer_chat call (non-blocking).
    ///
    /// Injects source information (node ID, local IPs, RPC port) into the payload,
    /// generates a task ID, and submits the task to the cluster. Returns an
    /// `AsyncToolResult` with the task ID for the caller to correlate with
    /// a continuation snapshot.
    fn execute_async_peer_chat(
        &self,
        peer_id: &str,
        mut payload: serde_json::Value,
    ) -> ToolResult {
        // 1. Inject source information
        payload["_source"] = serde_json::json!({
            "node_id": self.cluster.node_id(),
            "addresses": self.cluster.get_local_ips(),
            "rpc_port": self.cluster.get_rpc_port(),
        });

        // 2. Generate and inject task_id
        let task_id = format!(
            "task-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        );
        payload["task_id"] = serde_json::json!(task_id);

        // 3. Get origin channel/chat_id context
        let origin_channel = self
            .channel
            .try_lock()
            .map(|g| g.clone())
            .unwrap_or_default();
        let origin_chat_id = self
            .chat_id
            .try_lock()
            .map(|g| g.clone())
            .unwrap_or_default();

        // 4. Submit async task
        match self
            .cluster
            .submit_task(peer_id, "peer_chat", &payload, &origin_channel, &origin_chat_id)
        {
            Ok(submitted_id) => {
                info!(
                    task_id = %submitted_id,
                    peer_id = %peer_id,
                    "Async peer_chat task submitted"
                );
                let msg = format!(
                    "peer_chat task submitted to {} (task_id: {}), waiting for callback...",
                    peer_id, submitted_id
                );
                let mut result = ToolResult::async_result(&msg);
                result.task_id = Some(submitted_id);
                result
            }
            Err(e) => {
                warn!(error = %e, peer_id = %peer_id, "Failed to submit peer_chat task");
                ToolResult::error(&format!("Failed to submit task: {}", e))
            }
        }
    }
}

impl Default for ClusterRpcTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl crate::registry::Tool for ClusterRpcTool {
    fn name(&self) -> &str {
        "cluster_rpc"
    }

    fn description(&self) -> &str {
        "Call other bots in the cluster via RPC. Parameters: peer_id (string, required), action (string, required), data (object, optional)"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "peer_id": {
                    "type": "string",
                    "description": "ID of the peer bot to call"
                },
                "action": {
                    "type": "string",
                    "description": "RPC action to perform"
                },
                "data": {
                    "type": "object",
                    "description": "Optional data payload for the RPC call"
                }
            },
            "required": ["peer_id", "action"]
        })
    }

    async fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let peer_id = match args["peer_id"].as_str() {
            Some(id) if !id.is_empty() => id,
            _ => return ToolResult::error("peer_id is required"),
        };

        let action = match args["action"].as_str() {
            Some(a) if !a.is_empty() => a,
            _ => return ToolResult::error("action is required"),
        };

        // Check if cluster is connected
        if !self.cluster.is_connected() {
            return ToolResult::error(
                "cluster is not connected. Enable cluster mode first.",
            );
        }

        // Extract payload
        let payload = if args["data"].is_object() {
            args["data"].clone()
        } else {
            serde_json::json!({})
        };

        debug!(
            peer_id = %peer_id,
            action = %action,
            "Executing cluster RPC"
        );

        // peer_chat goes through the async (non-blocking) path
        if action == "peer_chat" {
            return self.execute_async_peer_chat(peer_id, payload);
        }

        // Synchronous path (ping, get_capabilities, etc.)
        match self.cluster.call_with_context(peer_id, action, &payload) {
            Ok(response) => ToolResult::silent(&response),
            Err(e) => ToolResult::error(&format!("RPC call failed: {}", e)),
        }
    }
}

impl ContextualTool for ClusterRpcTool {
    fn set_context(&mut self, ctx: &crate::registry::ToolExecutionContext) {
        // Use try_lock to avoid blocking since set_context is sync
        if let Ok(mut ch) = self.channel.try_lock() {
            *ch = ctx.channel.clone();
        }
        if let Ok(mut cid) = self.chat_id.try_lock() {
            *cid = ctx.chat_id.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::Tool;

    fn make_connected_tool() -> ClusterRpcTool {
        let mut stub = StubClusterOps::connected("node-1");
        stub.peers = vec![
            PeerInfo {
                id: "node-2".to_string(),
                name: "Bot2".to_string(),
                capabilities: vec!["chat".to_string(), "tools".to_string()],
                status: "online".to_string(),
            },
        ];
        stub.capabilities = vec!["chat".to_string(), "tools".to_string(), "translate".to_string()];
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
}
