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
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(0),
            peer_id
        );
        info!(
            task_id = %task_id,
            peer_id = %peer_id,
            action = %action,
            "[Tools] StubClusterOps submitted async task"
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
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(0)
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
                    "[Tools] Async peer_chat task submitted"
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
                warn!(error = %e, peer_id = %peer_id, "[Tools] Failed to submit peer_chat task");
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
        "Call other bots in the cluster via RPC. Returns the remote bot's final response text — that text is the only thing you (and the user) will see from the remote side; it will NOT include tool calls, files written, or command outputs that the remote bot produced internally. If the remote bot runs a full agent loop (LLM task executor), the call may take tens of seconds to several minutes; consider telling the user you have contacted the peer and are waiting before calling. Parameters: peer_id (string, required), action (string, required), data (object, optional)"
    }

    fn parameters(&self) -> serde_json::Value {
        // Dynamically inject online peer list with capabilities into peer_id description,
        // so the LLM can make informed routing decisions.
        let peer_desc = if self.cluster.is_connected() {
            let peers = self.cluster.get_online_peers();
            if peers.is_empty() {
                "ID of the peer bot to call (no peers currently online)".to_string()
            } else {
                let mut desc = "ID of the peer bot to call. Available online peers:\n".to_string();
                for p in &peers {
                    let caps = if p.capabilities.is_empty() {
                        "unknown capabilities".to_string()
                    } else {
                        p.capabilities.join(", ")
                    };
                    desc.push_str(&format!(
                        "- {} ({}): {}\n",
                        p.id, p.name, caps
                    ));
                }
                desc
            }
        } else {
            "ID of the peer bot to call (cluster not connected)".to_string()
        };

        serde_json::json!({
            "type": "object",
            "properties": {
                "peer_id": {
                    "type": "string",
                    "description": peer_desc
                },
                "action": {
                    "type": "string",
                    "description": "RPC action to perform (e.g. peer_chat, ping, get_capabilities)"
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
            "[Tools] Executing cluster RPC"
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
mod tests;
