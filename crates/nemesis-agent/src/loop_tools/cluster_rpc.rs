// ---------------------------------------------------------------------------
// 机械拆分：本文件内容由 loop_tools.rs 原样搬入（纯代码搬家，行为零变更）。
// ---------------------------------------------------------------------------

use super::*;

/// Configuration for the cluster RPC tool.
#[derive(Debug, Clone)]
pub struct ClusterRpcConfig {
    /// Node ID of the local node.
    pub local_node_id: String,
    /// Default timeout in seconds.
    pub timeout_secs: u64,
    /// Local RPC port (included in payloads so remote nodes can callback).
    pub local_rpc_port: u16,
}

impl Default for ClusterRpcConfig {
    fn default() -> Self {
        Self {
            local_node_id: String::new(),
            timeout_secs: 3600,
            local_rpc_port: 21949,
        }
    }
}

/// Setup the cluster RPC channel for peer-to-peer communication.
///
/// Mirrors Go's `setupClusterRPCChannel`. See the newer `setup_cluster_rpc_channel`
/// function below for the full implementation with continuation manager support.
/// This is a convenience wrapper.
pub fn setup_cluster_rpc_channel_with_config(
    cluster_config: &ClusterRpcConfig,
) -> ClusterRpcChannelConfig {
    let config = ClusterRpcChannelConfig::default();

    tracing::info!(
        local_node_id = %cluster_config.local_node_id,
        timeout_secs = cluster_config.timeout_secs,
        "[ClusterRPC] Cluster RPC channel configured (24h B-side safety net)"
    );

    config
}

/// Register the LLM handler for peer_chat RPC action on the RPC server.
///
/// When a peer_chat request arrives, this handler:
/// 1. Immediately returns ACK to the sender
/// 2. Asynchronously processes the LLM request
/// 3. Calls back to the sender with the response
///
/// This function takes an RPC server and registers the peer_chat handler
/// that will invoke the LLM provider.
pub fn register_peer_chat_handler<F>(
    handlers: &mut std::collections::HashMap<
        String,
        Box<dyn Fn(serde_json::Value) -> Result<serde_json::Value, String> + Send + Sync>,
    >,
    llm_handler: F,
) where
    F: Fn(serde_json::Value) -> Result<serde_json::Value, String> + Send + Sync + 'static,
{
    handlers.insert("peer_chat".to_string(), Box::new(llm_handler));
    handlers.insert(
        "peer_chat_callback".to_string(),
        Box::new(|payload| {
            // Callback handler: receive the response from the remote node
            // and route it through the continuation system
            let task_id = payload
                .get("task_id")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let content = payload
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            tracing::info!(
                task_id = task_id,
                content_len = content.len(),
                "[ClusterRPC] Received peer_chat_callback"
            );
            Ok(serde_json::json!({
                "status": "received",
                "task_id": task_id,
            }))
        }),
    );

    tracing::info!("[ClusterRPC] Registered peer_chat + peer_chat_callback handlers");
}

/// Cluster RPC tool for inter-node communication.
///
/// Sends a request to a remote node in the cluster and returns the response.
/// When an `rpc_call_fn` is provided, it performs a real RPC call; otherwise
/// it returns an error indicating the cluster is not available.
pub struct ClusterRpcTool {
    config: ClusterRpcConfig,
    /// Stored channel from set_context.
    pub(super) stored_channel: Arc<std::sync::Mutex<String>>,
    /// Stored chat_id from set_context.
    pub(super) stored_chat_id: Arc<std::sync::Mutex<String>>,
    /// Whether the cluster module is enabled and running.
    /// When false, execute() returns immediately with "cluster not enabled" error
    /// instead of attempting network calls that would fail unpredictably.
    /// This guard preserves LLM prompt cache hit rates (tool definition stays in prompt).
    enabled: Arc<std::sync::atomic::AtomicBool>,
    /// Optional RPC call function: (target_node, action, payload) -> Result<serde_json::Value, String>
    rpc_call_fn: Option<
        Arc<
            dyn Fn(
                    &str,
                    &str,
                    serde_json::Value,
                ) -> std::pin::Pin<
                    Box<dyn std::future::Future<Output = Result<serde_json::Value, String>> + Send>,
                > + Send
                + Sync,
        >,
    >,
    /// Returns online peer nodes with their capabilities for dynamic tool description.
    /// Each tuple: (node_id, node_name, capabilities).
    peers_fn: Option<Arc<dyn Fn() -> Vec<(String, String, Vec<String>)> + Send + Sync>>,
    /// 负载感知路由（最小版）：`target="auto"` 的解析器——网关注入的
    /// `Cluster::pick_auto_route_target` 包装（在线节点按健康/负载评分取
    /// 最优，排除本节点）。None = 未装配（execute 对 "auto" 诚实报错，
    /// 不发送）。返回 None = 无可用候选（同样诚实报错）。
    auto_target_fn: Option<Arc<dyn Fn() -> Option<String> + Send + Sync>>,
}

impl ClusterRpcTool {
    /// Create a new cluster RPC tool.
    pub fn new(config: ClusterRpcConfig) -> Self {
        Self {
            config,
            stored_channel: Arc::new(std::sync::Mutex::new(String::new())),
            stored_chat_id: Arc::new(std::sync::Mutex::new(String::new())),
            enabled: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            rpc_call_fn: None,
            peers_fn: None,
            auto_target_fn: None,
        }
    }

    /// Set whether the cluster module is enabled.
    /// When disabled, execute() returns immediately without attempting RPC calls.
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled
            .store(enabled, std::sync::atomic::Ordering::Relaxed);
    }

    /// Check if the cluster module is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Get a clone of the enabled flag Arc for external control.
    /// The ClusterServiceAdapter uses this to toggle the tool's enabled state
    /// without removing the tool from the prompt (preserving LLM cache).
    pub fn enabled_arc(&self) -> Arc<std::sync::atomic::AtomicBool> {
        self.enabled.clone()
    }

    /// Set the RPC call function for performing actual cluster RPC calls.
    ///
    /// The function signature is: `(target_node, action, payload) -> Future<Output = Result<Value, String>>`
    pub fn set_rpc_call_fn(
        &mut self,
        f: Arc<
            dyn Fn(
                    &str,
                    &str,
                    serde_json::Value,
                ) -> std::pin::Pin<
                    Box<dyn std::future::Future<Output = Result<serde_json::Value, String>> + Send>,
                > + Send
                + Sync,
        >,
    ) {
        self.rpc_call_fn = Some(f);
    }

    /// Set the peers function for dynamic tool description.
    ///
    /// The function returns online peer nodes: `Vec<(node_id, node_name, capabilities)>`.
    /// Called each time the LLM requests tool definitions so the peer list stays current.
    pub fn set_peers_fn(
        &mut self,
        f: Arc<dyn Fn() -> Vec<(String, String, Vec<String>)> + Send + Sync>,
    ) {
        self.peers_fn = Some(f);
    }

    /// 负载感知路由（最小版）：注入 `target="auto"` 的解析器（网关侧
    /// `Cluster::pick_auto_route_target` 的包装——在线节点按健康/负载评分
    /// 取最优，排除本节点）。未注入时 execute 对 "auto" 诚实报错。
    pub fn set_auto_target_fn(&mut self, f: Arc<dyn Fn() -> Option<String> + Send + Sync>) {
        self.auto_target_fn = Some(f);
    }
}

#[async_trait]
impl Tool for ClusterRpcTool {
    fn description(&self) -> String {
        "Send a message to ANOTHER bot in the cluster (never yourself). \
         Returns the remote node's final response text. If the remote node executes an LLM task, \
         the call may take tens of seconds to several minutes. Before calling, use the `message` \
         tool to tell the user \"已联系对方，稍等\" so they know to wait. \
         The remote node only sees your final message text — it does NOT see your prior tool \
         calls, file writes, or command outputs. If you need the remote node to act on something \
         you've produced locally (e.g. code you wrote), include the relevant content directly in \
         the `message` field rather than just referencing it."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        // Dynamically inject online peer list with capabilities into the target description.
        // The peers_fn list already excludes the local node (filtered at the
        // cluster level via get_online_peers_excluding_self), so the LLM should
        // never see itself as a candidate. We also explicitly annotate the
        // local node_id in the description as belt-and-suspenders.
        let self_id_note = if self.config.local_node_id.is_empty() {
            String::new()
        } else {
            format!(
                "\nNote: your own node_id is '{}'. Do NOT select it — this tool is for calling OTHER nodes only.\n",
                self.config.local_node_id
            )
        };

        let target_desc = if let Some(ref peers_fn) = self.peers_fn {
            let peers = peers_fn();
            if peers.is_empty() {
                format!(
                    "Target bot ID (no other peers currently online).{}",
                    self_id_note
                )
            } else {
                let mut desc = format!(
                    "Target bot ID. Available online peers (excluding yourself):\n{}",
                    self_id_note
                );
                for (id, name, caps) in &peers {
                    let caps_str = if caps.is_empty() {
                        "unknown capabilities".to_string()
                    } else {
                        caps.join(", ")
                    };
                    desc.push_str(&format!("- {} ({}): {}\n", id, name, caps_str));
                }
                desc
            }
        } else {
            format!("Target bot ID{}", self_id_note)
        };

        // 负载感知路由（最小版）：auto 入口恒宣告（未装配时 execute 诚实
        // 报错——比 schema 静默不提更利于模型理解失败原因）。
        let target_desc = format!(
            "{target_desc}\nYou may also pass \"auto\": the gateway picks the best online node for you (health/load aware)."
        );

        serde_json::json!({
            "type": "object",
            "properties": {
                "target": {"type": "string", "description": target_desc},
                "message": {"type": "string", "description": "Message to send"},
                "timeout": {"type": "integer", "description": "Timeout in seconds"}
            },
            "required": ["target", "message"]
        })
    }

    fn set_context(&self, channel: &str, chat_id: &str) {
        if let Ok(mut guard) = self.stored_channel.lock() {
            *guard = channel.to_string();
        }
        if let Ok(mut guard) = self.stored_chat_id.lock() {
            *guard = chat_id.to_string();
        }
    }

    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String> {
        // Guard: check if cluster is enabled before any processing.
        // This prevents unpredictable network errors when the cluster module is stopped.
        // The tool definition remains in the prompt for LLM cache hit rate.
        if !self.enabled.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("集群功能未启用，无法调用远程节点。请勿重试。".to_string());
        }

        let val: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid JSON arguments: {}", e))?;

        let target_node = val
            .get("target_node")
            .or_else(|| val.get("target"))
            .or_else(|| val.get("peer_id"))
            .and_then(|v| v.as_str())
            .ok_or("Missing 'target_node' field")?;

        // 负载感知路由（最小版）：target="auto" 由注入的选择器解析成具体
        // 节点（网关侧按健康/负载评分选在线节点，排除本节点）。解析器缺席
        // 或无可用候选都诚实报错不发送——「auto」绝不静默退化为任意节点。
        let auto_resolved;
        let target_node = if target_node.eq_ignore_ascii_case("auto") {
            let f = self.auto_target_fn.as_ref().ok_or_else(|| {
                "target=\"auto\" 自动路由未装配（本实例未接入负载感知选择器）。\
                 请改用具体节点 ID，不要重试 auto。"
                    .to_string()
            })?;
            auto_resolved = f().ok_or_else(|| {
                "target=\"auto\" 自动路由无可用节点（当前无在线候选）。\
                 请稍后重试或改用具体节点 ID。"
                    .to_string()
            })?;
            auto_resolved.as_str()
        } else {
            target_node
        };

        // Guard: reject self-invocation. The LLM should never target the local
        // node via cluster_rpc — doing so creates a nested child task that loops
        // back to this same node, which serves no purpose and corrupts the
        // continuation history. The peers list passed to the LLM already
        // excludes self, but this is the hard backstop in case of cache lag,
        // stale tool definitions, or unexpected LLM behavior.
        if !self.config.local_node_id.is_empty() && target_node == self.config.local_node_id {
            return Err(format!(
                "不能通过 cluster_rpc 调用本节点（{}）。这个工具用于和其他节点通信；\
                 如果需要在本地执行操作，请直接使用本地工具（exec/filesystem/etc），\
                 不要重试 cluster_rpc。",
                target_node
            ));
        }

        // Extract message content: check "message" first, then "data.content" (testai-3.0 format)
        let message = val
            .get("message")
            .and_then(|v| v.as_str())
            .or_else(|| {
                val.get("data")
                    .and_then(|d| d.get("content"))
                    .and_then(|v| v.as_str())
            })
            .unwrap_or("");

        let rpc_call = match &self.rpc_call_fn {
            Some(f) => f,
            None => {
                return Err(format!(
                    "Cluster RPC is not available (no RPC client configured). Cannot reach node '{}'.",
                    target_node
                ));
            }
        };

        // Build the payload with context information
        let channel = if context.channel.is_empty() {
            self.stored_channel
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        } else {
            context.channel.clone()
        };

        let chat_id = if context.chat_id.is_empty() {
            self.stored_chat_id
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        } else {
            context.chat_id.clone()
        };

        // 方案 D：检测并改写 chat_id 加 origin 前缀，防止多跳链路撞车。
        //
        // 传播规则：
        //   - chat_id 已经以 "cluster:" 开头 → 已经被上游节点标记过，原样传播
        //   - chat_id 为空 → 不改写（避免出现 "cluster:node-X:" 这种尾部空值）
        //   - 其他 → 改写为 "cluster:{local_node_id}:{chat_id}"，把本节点 ID 嵌进去
        //
        // 下游节点 peer_chat_handler 用 `cluster_rpc:{source_node_id}/{chat_id}` 组 session_key，
        // 多跳链路（A→B→C→D）中 C 端 session_key 会含原始 A 的 node_id，跟 Q→B→C→D 链的
        // C 端 session_key 自然区分开。
        let propagated_chat_id = if chat_id.starts_with("cluster:") || chat_id.is_empty() {
            chat_id
        } else {
            format!("cluster:{}:{}", self.config.local_node_id, chat_id)
        };

        let payload = serde_json::json!({
            "content": message,
            "channel": channel,
            "chat_id": propagated_chat_id,
            "timeout": self.config.timeout_secs,
            "_source_rpc_port": self.config.local_rpc_port,
        });

        let result = rpc_call(target_node, "peer_chat", payload).await?;

        // Check if the response is an async ACK from PeerChatHandler.
        // ACK format: {"status": "accepted", "task_id": "auto-xxx"}
        // In this case, return __ASYNC__ marker so AgentLoop saves a continuation snapshot.
        let status = result.get("status").and_then(|v| v.as_str()).unwrap_or("");
        if status == "accepted" {
            let task_id = result
                .get("task_id")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            tracing::info!(
                task_id = %task_id,
                target = %target_node,
                "[ClusterRPC] Peer chat ACK received, returning async marker"
            );

            // Plan C (template-based UX): resolve the peer's display name
            // (e.g. "Alex") here and pass it through the `__ASYNC__` marker
            // so loop.rs can render a human-friendly "waiting" message
            // without a cluster registry lookup (nemesis-agent deliberately
            // does not depend on nemesis-cluster).
            //
            // Falls back to the bare node ID if the peer is not in the
            // online list (just went offline, or peers_fn unset in tests).
            // Colons in the name are stripped because the marker is
            // `:`-delimited and splitn would otherwise mis-parse.
            let target_name = self
                .peers_fn
                .as_ref()
                .and_then(|f| f().into_iter().find(|(id, _, _)| id == target_node))
                .map(|(_, name, _)| name)
                .filter(|n| !n.is_empty())
                .map(|n| n.replace(':', ""))
                .unwrap_or_else(|| target_node.to_string());

            return Ok(format!(
                "__ASYNC__:{}:{}:{}",
                task_id, target_node, target_name
            ));
        }

        // Synchronous response — extract content field
        let content = result.get("content").and_then(|v| v.as_str()).unwrap_or("");

        Ok(content.to_string())
    }
}

// ===========================================================================
// Spawn tool (sub-agent management)
// ===========================================================================
/// Configuration for setting up the cluster RPC channel.
///
/// Mirrors Go's `channels.RPCChannelConfig`:
/// - `request_timeout`: B-side safety net (24 hours default)
/// - `cleanup_interval`: How often to clean up stale requests
#[derive(Debug, Clone)]
pub struct ClusterRpcChannelConfig {
    /// Request timeout for the RPC channel (B-side safety net).
    pub request_timeout: Duration,
    /// How often to clean up stale pending requests.
    pub cleanup_interval: Duration,
}

impl Default for ClusterRpcChannelConfig {
    fn default() -> Self {
        Self {
            request_timeout: Duration::from_secs(
                nemesis_types::constants::RPC_CHANNEL_TIMEOUT_SECS,
            ),
            cleanup_interval: Duration::from_secs(nemesis_types::constants::CLEANUP_INTERVAL_SECS),
        }
    }
}

/// Result of setting up the cluster RPC channel.
///
/// Contains both the channel configuration and the continuation manager
/// (if provided), so the caller can properly wire everything together.
pub struct ClusterRpcChannelSetup {
    /// The channel configuration.
    pub config: ClusterRpcChannelConfig,
    /// The continuation manager (if provided) for handling async RPC results.
    pub continuation_manager: Option<Arc<crate::loop_continuation::ContinuationManager>>,
}

/// Set up the cluster RPC channel for peer-to-peer bot communication.
///
/// Mirrors Go's `setupClusterRPCChannel`. This function:
/// 1. Creates an RPC channel configuration with a 24-hour timeout (B-side safety net)
/// 2. The continuation manager is stored for async callback handling
/// 3. The returned `ClusterRpcChannelSetup` should be used by the caller to wire
///    the channel manager, cluster instance, and continuation system together.
///
/// # Note
/// In the Go implementation, this function creates an RPCChannel and sets it
/// on the Cluster instance. In Rust, the channel and cluster are managed
/// separately. This function returns the setup needed to wire them together.
///
/// # Arguments
/// * `continuation_manager` - The continuation manager for handling async RPC results
///
/// # Returns
/// A `ClusterRpcChannelSetup` with the channel configuration and continuation manager.
pub fn setup_cluster_rpc_channel(
    continuation_manager: Option<Arc<crate::loop_continuation::ContinuationManager>>,
) -> ClusterRpcChannelSetup {
    let config = ClusterRpcChannelConfig::default();

    if let Some(ref cm) = continuation_manager {
        info!(
            "[AgentTools] RPC channel for peer chat configured with continuation manager (timeout={:?}, cleanup={:?})",
            config.request_timeout, config.cleanup_interval
        );
        // The continuation manager is ready to save snapshots when async
        // cluster_rpc tools are invoked. It will be used by the executor
        // to save continuation snapshots and handle async callbacks.
        let _ = cm; // Available for caller to wire up
    } else {
        info!(
            "[AgentTools] RPC channel for peer chat configured without continuation manager (timeout={:?}, cleanup={:?})",
            config.request_timeout, config.cleanup_interval
        );
    }

    ClusterRpcChannelSetup {
        config,
        continuation_manager,
    }
}

// ===========================================================================
// register_shared_tools -- register tools across all agents (mirrors Go's registerSharedTools)
// ===========================================================================
