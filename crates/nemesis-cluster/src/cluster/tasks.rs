// ---------------------------------------------------------------------------
// 2026-11-05 机械拆分：本文件内容由 cluster.rs 原样搬入（纯代码搬家，行为零变更）。
// ---------------------------------------------------------------------------

use super::*;

impl Cluster {
    /// Submit a new task to the cluster. Returns the task ID.
    pub fn submit_task(
        &self,
        action: &str,
        payload: serde_json::Value,
        original_channel: &str,
        original_chat_id: &str,
    ) -> String {
        let task =
            self.task_manager
                .create_task(action, payload, original_channel, original_chat_id);
        logger::log_task("submitted", &task.id, action);
        task.id
    }

    /// Submit an async peer_chat task to a remote node.
    ///
    /// 1. Creates a local task record
    /// 2. Makes a synchronous RPC call (gets ACK)
    /// 3. Returns the task ID for later continuation
    pub fn submit_peer_chat(
        &self,
        peer_id: &str,
        action: &str,
        payload: serde_json::Value,
        channel: &str,
        chat_id: &str,
    ) -> Result<String, String> {
        // Extract or generate task_id
        let task_id = payload
            .get("task_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let _task_id = if task_id.is_empty() {
            uuid::Uuid::new_v4().to_string()
        } else {
            task_id
        };

        // Create local task with peer_id
        let task = self.task_manager.create_task_with_peer(
            action,
            payload.clone(),
            channel,
            chat_id,
            peer_id,
        );

        // Note: actual RPC call would be async, handled by the caller

        Ok(task.id)
    }

    /// Get a task by ID.
    pub fn get_task(&self, task_id: &str) -> Option<Task> {
        self.task_manager.get_task(task_id)
    }

    /// Assign a task to a specific node.
    pub fn assign_task(&self, task_id: &str, node_id: &str) -> bool {
        self.task_manager.assign_task(task_id, node_id)
    }

    /// Complete a task.
    pub fn complete_task(&self, task_id: &str, result: serde_json::Value) -> bool {
        let ok = self.task_manager.complete_task(task_id, result);
        if ok {
            logger::log_task("completed", task_id, "");
            self.record_route_outcome(task_id, true);
        }
        ok
    }

    /// Fail a task.
    pub fn fail_task(&self, task_id: &str, error: &str) -> bool {
        let ok = self.task_manager.fail_task(task_id, error);
        if ok {
            tracing::warn!(task_id = task_id, error = error, "[Cluster] Task failed",);
            logger::log_task("failed", task_id, error);
            self.record_route_outcome(task_id, false);
        }
        ok
    }

    /// 负载感知路由：记账一次 A 端观测的任务结局（complete/fail 单一漏斗，
    /// 覆盖评审/看板回调臂与 chat 回调臂——两个臂都经本方法收口）。委托
    /// [`record_route_outcome_core`]（记账语义单一真相源，与旁路 settle 点
    /// 经 TaskManager 路由钩子的补记账同核）。
    fn record_route_outcome(&self, task_id: &str, ok: bool) {
        record_route_outcome_core(
            &self.route_stats,
            &self.route_recorded,
            &self.node_id,
            &self.task_manager,
            task_id,
            ok,
        );
    }

    /// 构造期安装 TaskManager 路由钩子：恢复轮询 sweep 三分支、默认回调
    /// handler 兜底臂与 G4 清理 tick 的 H4 安全网判死（都是拿不到
    /// `&Cluster` 的旁路 settle 点）经 `task_manager.fire_route_record(...)`
    /// 补记账。闭包捕获 Arc 化的 route_stats/route_recorded——与回调漏斗
    /// 共用同一状态与去重集，双路径绝不双计。幂等覆盖（重复构造/重启安全）。
    pub(super) fn install_route_record_hook(&self) {
        let stats = Arc::clone(&self.route_stats);
        let recorded = Arc::clone(&self.route_recorded);
        let node_id = self.node_id.clone();
        let task_manager = Arc::clone(&self.task_manager);
        self.task_manager
            .set_route_record_fn(Arc::new(move |task_id, ok| {
                record_route_outcome_core(&stats, &recorded, &node_id, &task_manager, task_id, ok);
            }));
    }

    /// 负载感知路由（最小版）：从在线节点（不含本节点）按健康/负载选一个
    /// 最优 target。评分链（min 逐维裁决，确定性平局）：
    /// ① success_rate 降（零样本 = 1.0 乐观中性，新节点不被饿死）→
    /// ② 在途任务数升（Pending 即在途——`list_pending_tasks` 按 peer 聚合）
    /// → ③ 平均耗时升（零样本 = 0 中性，探索性优先，拿样本即回归真实
    /// 排序）→ ④ node_id 字典序。无在线节点 → None（调用方诚实报错，
    /// 不回落任意节点）。
    pub fn pick_auto_route_target(&self) -> Option<String> {
        let mut pending: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
        for t in self.task_manager.list_pending_tasks() {
            if !t.peer_id.is_empty() {
                *pending.entry(t.peer_id.clone()).or_insert(0) += 1;
            }
        }
        let stats = self.route_stats.lock();
        self.registry
            .list_online_excluding(&self.node_id)
            .into_iter()
            .min_by_key(|p| {
                let s = stats.get(&p.base.id).cloned().unwrap_or_default();
                (
                    std::cmp::Reverse((s.success_rate() * 1_000_000_000.0) as u64),
                    pending.get(&p.base.id).copied().unwrap_or(0),
                    s.avg_response_ms(),
                    p.base.id.clone(),
                )
            })
            .map(|p| p.base.id)
    }

    /// 负载感知路由：路由统计只读快照（诊断/Dashboard 用；键 = peer id；
    /// 内存态重启归零）。返回 owned clone，调用方不持锁。
    pub fn route_stats_snapshot(&self) -> std::collections::HashMap<String, RoutePeerStats> {
        self.route_stats.lock().clone()
    }
}

impl Cluster {
    /// Make an RPC call to a peer (synchronous wrapper).
    ///
    /// Mirrors Go's `Cluster.CallWithContext(ctx, peerID, action, payload)`.
    /// Selects a peer from the registry, builds an RPC request, sends it via
    /// the RPC client, and returns the raw response bytes.
    ///
    /// Falls back to the test override if set. Returns an error if no RPC
    /// client is available or if the call fails.
    pub fn call_with_context(
        &self,
        peer_id: &str,
        action: &str,
        payload: serde_json::Value,
    ) -> Result<Vec<u8>, String> {
        // Testing override
        if let Some(ref f) = *self.call_with_context_fn.lock() {
            return f(peer_id, action, payload);
        }

        // Production path: use the async RPC client.
        // Since call_with_context is synchronous, we bridge to async via
        // tokio::runtime::Handle::block_on when a runtime is available.
        let rpc_client = self.rpc_client.lock().clone();
        match rpc_client {
            Some(client) => {
                let request = crate::rpc_types::RPCRequest {
                    id: uuid::Uuid::new_v4().to_string(),
                    action: crate::rpc_types::ActionType::Custom(action.to_string()),
                    payload,
                    source: self.node_id.clone(),
                    target: Some(peer_id.to_string()),
                };

                tracing::debug!(
                    peer_id = peer_id,
                    action = action,
                    request_id = %request.id,
                    "[Cluster] Initiating RPC call_with_context",
                );

                // Try to run within an existing tokio runtime
                match tokio::runtime::Handle::try_current() {
                    Ok(handle) => {
                        // We may be inside a tokio worker thread (e.g. ClusterRpcTool::execute).
                        // Use block_in_place to avoid "Cannot start a runtime from within a runtime" panic.
                        let result = tokio::task::block_in_place(|| {
                            handle.block_on(client.call_with_timeout(
                                peer_id,
                                request,
                                client.timeout(),
                            ))
                        });
                        match result {
                            Ok(response) => {
                                if let Some(ref err) = response.error {
                                    tracing::error!(
                                        peer_id = peer_id,
                                        action = action,
                                        error = %err,
                                        "[Cluster] RPC call returned error",
                                    );
                                    Err(err.clone())
                                } else {
                                    tracing::debug!(
                                        peer_id = peer_id,
                                        action = action,
                                        "[Cluster] RPC call completed successfully",
                                    );
                                    // Serialize the result to bytes (matching Go's []byte return)
                                    match &response.result {
                                        Some(val) => serde_json::to_vec(val)
                                            .map_err(|e| format!("serialize response: {}", e)),
                                        None => Ok(Vec::new()),
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::error!(
                                    peer_id = peer_id,
                                    action = action,
                                    error = %e,
                                    "[Cluster] RPC call failed",
                                );
                                Err(format!("RPC call failed: {}", e))
                            }
                        }
                    }
                    Err(_) => {
                        // No tokio runtime available (e.g. in unit tests or CLI)
                        tracing::warn!(
                            peer_id = peer_id,
                            action = action,
                            "[Cluster] RPC client not available (no tokio runtime)",
                        );
                        Err("RPC client not initialized (no tokio runtime available)".into())
                    }
                }
            }
            None => {
                tracing::error!(
                    peer_id = peer_id,
                    action = action,
                    "[Cluster] RPC client not initialized",
                );
                Err("RPC client not initialized".into())
            }
        }
    }

    /// Make an async RPC call to a peer.
    ///
    /// This is the async counterpart to `call_with_context`, suitable for use
    /// from async contexts. Mirrors Go's `Cluster.CallWithContext` which
    /// natively supports context-based cancellation.
    pub async fn call_with_context_async(
        &self,
        peer_id: &str,
        action: &str,
        payload: serde_json::Value,
        timeout: Duration,
    ) -> Result<Vec<u8>, String> {
        // Testing override
        if let Some(ref f) = *self.call_with_context_fn.lock() {
            return f(peer_id, action, payload);
        }

        let rpc_client = self.rpc_client.lock().clone();
        match rpc_client {
            Some(client) => {
                let request = crate::rpc_types::RPCRequest {
                    id: uuid::Uuid::new_v4().to_string(),
                    action: crate::rpc_types::ActionType::Custom(action.to_string()),
                    payload,
                    source: self.node_id.clone(),
                    target: Some(peer_id.to_string()),
                };

                tracing::debug!(
                    peer_id = peer_id,
                    action = action,
                    timeout_secs = timeout.as_secs(),
                    request_id = %request.id,
                    "[Cluster] Initiating async RPC call",
                );

                match client.call_with_timeout(peer_id, request, timeout).await {
                    Ok(response) => {
                        if let Some(ref err) = response.error {
                            tracing::error!(
                                peer_id = peer_id,
                                action = action,
                                error = %err,
                                "[Cluster] Async RPC call returned error",
                            );
                            Err(err.clone())
                        } else {
                            tracing::debug!(
                                peer_id = peer_id,
                                action = action,
                                "[Cluster] Async RPC call completed successfully",
                            );
                            match &response.result {
                                Some(val) => serde_json::to_vec(val)
                                    .map_err(|e| format!("serialize response: {}", e)),
                                None => Ok(Vec::new()),
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            peer_id = peer_id,
                            action = action,
                            error = %e,
                            "[Cluster] Async RPC call failed",
                        );
                        Err(format!("RPC call failed: {}", e))
                    }
                }
            }
            None => {
                tracing::error!(
                    peer_id = peer_id,
                    action = action,
                    "[Cluster] Async RPC client not initialized",
                );
                Err("RPC client not initialized".into())
            }
        }
    }

    /// Set the testing override for call_with_context.
    pub fn set_call_with_context_fn(
        &self,
        f: Box<dyn Fn(&str, &str, serde_json::Value) -> Result<Vec<u8>, String> + Send + Sync>,
    ) {
        *self.call_with_context_fn.lock() = Some(Arc::from(f));
    }

    // -- Bus integration ------------------------------------------------------

    /// Set the cluster agent task list and work queue for callback routing.
    pub fn set_cluster_task_queue(
        &self,
        task_list: Arc<crate::cluster_task::ClusterTaskList>,
        work_queue: Arc<crate::cluster_task::ClusterWorkQueue>,
    ) {
        *self.cluster_task_list.lock() = Some(task_list);
        *self.cluster_work_queue.lock() = Some(work_queue);
    }

    /// Set the message bus (called by AgentLoop during setup).
    pub fn set_message_bus(&self, bus: Arc<dyn MessageBus>) {
        *self.bus.lock() = Some(bus);
    }

    /// Handle task completion (callback from TaskManager).
    /// Publishes a continuation message to the bus.
    pub fn handle_task_complete(&self, task_id: &str) {
        let task = match self.task_manager.get_task(task_id) {
            Some(t) => t,
            None => return,
        };

        // P1 统一任务层：结果回传 delivery 记账（deliveries 表首个消费者；
        // idempotency_key=task_id 天然去重——G5 恢复轮询/重复回调不重复记账）。
        // 记账失败只留痕不阻断回传（记账是辅助台账，回传是主链路）。
        // 项 C：任务有原始来源通道（original_channel 非空）才有出站通知
        // 义务（notify_expected）——无源任务（纯 RPC/内部）记 none，不进
        // 重试调度。
        if let Some(db) = &self.task_db {
            let payload = task.result.clone().unwrap_or(serde_json::Value::Null);
            let notify_expected = !task.original_channel.is_empty();
            match db.record_delivery(
                "cluster_result",
                Some(task_id),
                task_id,
                &payload,
                notify_expected,
            ) {
                Ok(true) => {}
                Ok(false) => {
                    tracing::debug!("[Cluster] delivery already recorded for task {task_id}")
                }
                Err(e) => {
                    tracing::warn!("[Cluster] delivery record failed for task {task_id}: {e}")
                }
            }
        }

        if task.original_channel.is_empty() {
            return;
        }

        let bus = match self.bus.lock().as_ref() {
            Some(b) => b.clone(),
            None => {
                logger::log_error(
                    "cluster",
                    "bus not set",
                    &format!("task {} completed but bus not available", task_id),
                );
                return;
            }
        };

        bus.publish_inbound(BusInboundMessage {
            channel: "system".into(),
            sender_id: format!("cluster_continuation:{}", task_id),
            chat_id: format!("{}:{}", task.original_channel, task.original_chat_id),
            content: String::new(),
            metadata: std::collections::HashMap::new(),
        });

        logger::log_task("completed", task_id, &task.action);
    }

    // -- Accessors ------------------------------------------------------------

    // 注：config() 已删除（LOW-19，2026-10-17 四轮审计）——返回 OnceLock
    // 静态 DEFAULT 的死 API，与实例真实配置（config.cluster.json / peers.toml
    // 装载态）零关联，唯一「调用方」是断言其返回假默认值的测试；留着只会
    // 误导读者以为能拿到运行时配置。
}

impl Cluster {
    /// Get the RPC channel (may be None if not configured).
    ///
    /// Mirrors Go's `GetRPCChannel() *channels.RPCChannel`.
    pub fn get_rpc_channel(&self) -> Option<Arc<dyn crate::rpc::RpcChannel>> {
        self.rpc_channel.read().clone()
    }

    /// Set the RPC channel and trigger LLM handler registration.
    ///
    /// Called by the agent loop after creating the RPCChannel.
    /// Thread-safety: acquires write lock to set the channel, then releases
    /// before calling `register_peer_chat_handlers()` to avoid deadlock
    /// (register_peer_chatHandlers -> register_rpc_handler -> read lock).
    ///
    /// Mirrors Go's `SetRPCChannel(rpcCh *channels.RPCChannel)`.
    pub fn set_rpc_channel(&self, channel: Arc<dyn crate::rpc::RpcChannel>) {
        *self.rpc_channel.write() = Some(channel);

        // Register peer chat handlers if server is running
        if self.is_running() && self.rpc_server.is_some() {
            self.register_peer_chat_handlers();
        }
    }

    // -- RPC handler registration ---------------------------------------------

    /// Register an RPC handler for a specific action.
    ///
    /// Returns an error if the cluster is not running or the RPC server is not
    /// initialized.
    ///
    /// Mirrors Go's `RegisterRPCHandler(action, handler) error`.
    pub fn register_rpc_handler(
        &self,
        action: &str,
        handler: crate::rpc::server::RpcHandlerFn,
    ) -> Result<(), String> {
        if !self.is_running() {
            return Err("cluster is not running".into());
        }
        let server = match self.rpc_server.as_ref() {
            Some(s) => s,
            None => return Err("RPC server is not initialized".into()),
        };
        server.register_handler(action, handler);
        logger::log_rpc("register_handler", action, "", "", None);
        Ok(())
    }

    /// Register peer chat related handlers when RPCChannel is ready.
    ///
    /// This must be called after both RPC Server and RPC Channel are initialized.
    /// Registers: peer_chat, peer_chat_callback, query_task_result,
    /// confirm_task_delivery, hello, and other custom handlers.
    ///
    /// Mirrors Go's `registerPeerChatHandlers()`.
    pub fn register_peer_chat_handlers(&self) {
        let rpc_channel = self.rpc_channel.read();
        if rpc_channel.is_none() {
            logger::log_rpc(
                "register_peer_chat_handlers",
                "",
                "RPCChannel not ready",
                "",
                None,
            );
            return;
        }
        drop(rpc_channel);

        // Register peer_chat handler (B-side: receive message, ACK, process async)
        if let Err(e) = self.register_rpc_handler("peer_chat", self.build_peer_chat_handler()) {
            logger::log_error("cluster", &format!("register peer_chat: {}", e), "");
        }

        // Register peer_chat_callback handler (A-side: receive result from B)
        if let Err(e) =
            self.register_rpc_handler("peer_chat_callback", self.build_callback_handler())
        {
            logger::log_error(
                "cluster",
                &format!("register peer_chat_callback: {}", e),
                "",
            );
        }

        // Register hello handler
        let node_id = self.node_id.clone();
        if let Err(e) = self.register_rpc_handler(
            "hello",
            Box::new(move |_payload| {
                Ok(serde_json::json!({
                    "node_id": node_id,
                    "status": "online",
                    "message": "hello from cluster node",
                }))
            }),
        ) {
            logger::log_error("cluster", &format!("register hello: {}", e), "");
        }

        // H4: Register query_task_result handler (B-side responds to A's polling)
        if let Err(e) =
            self.register_rpc_handler("query_task_result", self.build_query_task_result_handler())
        {
            logger::log_error("cluster", &format!("register query_task_result: {}", e), "");
        }

        // H4: Register confirm_task_delivery handler
        if let Err(e) = self.register_rpc_handler(
            "confirm_task_delivery",
            self.build_confirm_task_delivery_handler(),
        ) {
            logger::log_error(
                "cluster",
                &format!("register confirm_task_delivery: {}", e),
                "",
            );
        }
    }

    /// Register only the H4 task-recovery handlers (query_task_result +
    /// confirm_task_delivery).
    ///
    /// Gateway mode never calls [`Self::set_rpc_channel`]（该入口在 gateway
    /// 装配流程中无人触发，`register_peer_chat_handlers` 因此不会执行），而
    /// gateway 只自行注册 peer_chat / peer_chat_callback / task_cancel——
    /// 不覆盖本函数注册的两个恢复 handler。缺失时 A 侧
    /// `poll_stale_pending_tasks`（120s 周期）永远收到 "no handler"，B 重启
    /// 丢结果后的 stale 恢复链路断裂（2026-09-01 跨机 E2E 实测发现）。
    /// gateway 在 `Arc<Cluster>` 就绪后调用本函数；与
    /// [`Self::register_peer_chat_handlers`] 共用同一对私有 builder（单一
    /// 真相源），不重复实现。
    pub fn register_task_recovery_handlers(&self) {
        if let Err(e) =
            self.register_rpc_handler("query_task_result", self.build_query_task_result_handler())
        {
            logger::log_error("cluster", &format!("register query_task_result: {}", e), "");
        }
        if let Err(e) = self.register_rpc_handler(
            "confirm_task_delivery",
            self.build_confirm_task_delivery_handler(),
        ) {
            logger::log_error(
                "cluster",
                &format!("register confirm_task_delivery: {}", e),
                "",
            );
        }
    }

    /// Register basic RPC handlers (ping, info, etc.).
    ///
    /// This can be called directly in daemon mode where RPCChannel is not
    /// available. Registers: ping, get_capabilities, get_info, hello,
    /// query_task_result, confirm_task_delivery and other default handlers.
    /// (list_actions is a RpcServer default that dynamically enumerates the
    /// handler map — see rpc/server.rs — so it is not re-registered here.)
    ///
    /// Mirrors Go's `RegisterBasicHandlers() error`.
    pub fn register_basic_handlers(&self) -> Result<(), String> {
        if !self.is_running() {
            return Err("cluster not running".into());
        }

        // ping
        let node_id = self.node_id.clone();
        // 共享槽 Arc：注册可能早于 install_self_weak（gateway 装配顺序），
        // 捕获槽本身而非快照——后装也能被升级到。
        let self_weak_slot = self.self_weak.clone();
        self.register_rpc_handler(
            "ping",
            Box::new(move |_payload| {
                let mut resp = serde_json::json!({
                    "status": "pong",
                    "node_id": node_id,
                    // G2: 供主动探针交叉校验对端时钟漂移（additive 字段，
                    // 旧客户端忽略）。
                    "timestamp": chrono::Utc::now().timestamp(),
                });
                // 探针身份回传（additive 字段，旧客户端忽略）：自报身份与
                // UDP announce 同构（DiscoveryMessage），对端探针成功时经
                // apply_probe_announce 走同一入册漏斗——UDP 隔离拓扑里这是
                // 身份传播的唯一可靠通道。
                let upgraded = self_weak_slot.lock().clone().and_then(|w| w.upgrade());
                if let Some(c) = upgraded {
                    match serde_json::to_value(crate::discovery::build_announce_from_callbacks(c.as_ref())) {
                        Ok(ann) => resp["announce"] = ann,
                        Err(e) => tracing::debug!(error = %e, "[Cluster] announce payload serialize failed"),
                    }
                }
                Ok(resp)
            }),
        )?;

        // get_capabilities — shares Arc with Cluster for real-time reads
        let caps_arc = self.capabilities.clone();
        self.register_rpc_handler(
            "get_capabilities",
            Box::new(move |_payload| {
                let caps = caps_arc.lock().unwrap_or_else(|e| e.into_inner()).clone();
                Ok(serde_json::json!({
                    "capabilities": caps,
                }))
            }),
        )?;

        // get_info — returns data matching DiscoveryMessage broadcast format
        // Static fields (cloned, immutable after startup):
        let node_id = self.node_id.clone();
        let node_name = self.node_name.read().clone();
        let role = self.role.read().clone();
        let category = self.category.read().clone();
        let tags = self.tags.read().clone();
        let professions = self.professions.read().clone();
        let tier = self.tier.read().clone();
        // P5 第 4 步 ③：自报运行时后端形态（静态身份，构造期捕获同 tier）。
        let runtime_backend = self.runtime_backend.read().clone();
        // P2 agent roster：Arc 过闭包读活值（提供者每次现读 AgentManager，
        // 运行期增删 agent 无需重启即反映）。
        let roster_slot = self.agent_roster.clone();
        let node_type = self.node_type.clone();
        let rpc_port = self.rpc_port;
        // Dynamic fields (real-time):
        let caps_arc = self.capabilities.clone();
        self.register_rpc_handler(
            "get_info",
            Box::new(move |_payload| {
                let addresses = network::get_all_local_ips();
                let capabilities = caps_arc.lock().unwrap_or_else(|e| e.into_inner()).clone();
                let agents = roster_slot.read().as_ref().map(|p| p()).unwrap_or_default();
                Ok(serde_json::json!({
                    "version": "1.0",
                    "node_id": node_id,
                    "name": node_name,
                    "addresses": addresses,
                    "rpc_port": rpc_port,
                    "role": role,
                    "category": category,
                    "tags": tags,
                    "capabilities": capabilities,
                    // 职能框架 M2：自报职能/档位（旧对端不读未知键，兼容）。
                    "professions": professions,
                    "tier": tier,
                    // P5 第 4 步 ③：自报运行时后端形态（None = 未宣告，
                    // 旧对端不读未知键，兼容）。
                    "runtime_backend": runtime_backend,
                    // P2：自报 agent 实体清单（announce 子对象同源携带）。
                    "agents": agents,
                    "node_type": node_type,
                    "status": "online",
                }))
            }),
        )?;

        // list_actions 不在此覆盖：RpcServer 默认 handler 已动态枚举 handlers 容器
        // （rpc/server.rs 单一真相源）。旧覆盖版走 actions_schema::builtin_schemas()，
        // 与实际注册面漂移（含未注册的 forge_share/status/llm_proxy、漏 hello 等），
        // 且 schema 输出形态在仓内零消费方，已删除。

        // hello
        let node_id = self.node_id.clone();
        self.register_rpc_handler(
            "hello",
            Box::new(move |_payload| {
                Ok(serde_json::json!({
                    "node_id": node_id,
                    "status": "online",
                    "message": "hello from cluster node",
                }))
            }),
        )?;

        // diagnostics.system — OS, memory, uptime
        self.register_rpc_handler(
            "diagnostics.system",
            Box::new(move |_payload| {
                let os = std::env::consts::OS.to_string();
                let arch = std::env::consts::ARCH.to_string();
                let hostname = crate::diagnostics::get_hostname();
                let (mem_total, mem_used, uptime_secs) =
                    crate::diagnostics::collect_system_metrics();
                let os_version = crate::diagnostics::collect_os_version();
                Ok(serde_json::json!({
                    "os": os, "os_version": os_version, "arch": arch,
                    "hostname": hostname, "uptime_secs": uptime_secs,
                    "memory_total_bytes": mem_total, "memory_used_bytes": mem_used,
                }))
            }),
        )?;

        // diagnostics.network — network interfaces and IPs
        self.register_rpc_handler(
            "diagnostics.network",
            Box::new(move |_payload| {
                let interfaces = network::get_local_network_interfaces();
                let all_ips = network::get_all_local_ips();
                Ok(serde_json::json!({
                    "interfaces": interfaces,
                    "all_ips": all_ips,
                }))
            }),
        )?;

        // diagnostics.cluster_state — peers this node sees
        let registry_arc = self.registry.clone();
        self.register_rpc_handler(
            "diagnostics.cluster_state",
            Box::new(move |_payload| {
                let all_nodes = registry_arc.list_peers();
                let online: Vec<_> = all_nodes
                    .iter()
                    .filter(|n| n.is_online())
                    .map(|n| {
                        serde_json::json!({
                            "id": n.base.id,
                            "name": n.base.name,
                            "address": n.base.address,
                            "role": n.base.role,
                            "last_seen": n.base.last_seen,
                        })
                    })
                    .collect();
                Ok(serde_json::json!({
                    "node_count": all_nodes.len(),
                    "online_count": online.len(),
                    "nodes": online,
                }))
            }),
        )?;

        Ok(())
    }

    /// Register forge-related RPC handlers for cross-node learning.
    ///
    /// This must be called when forge is enabled and the cluster is running.
    /// Registers: forge_share, forge_get_reflections.
    ///
    /// Mirrors Go's `RegisterForgeHandlers()` which is called from bot_service
    /// after Forge is initialized.
    pub fn register_forge_handlers(
        &self,
        provider: Box<dyn crate::handlers::ForgeDataProvider>,
    ) -> Result<(), String> {
        if !self.is_running() {
            return Err("cluster not running".into());
        }

        let node_id = self.node_id.clone();

        // forge_share: receive a remote reflection report
        let provider_share = provider.clone_boxed();
        let node_id_share = node_id.clone();
        self.register_rpc_handler(
            "forge_share",
            Box::new(move |payload| {
                let from = payload
                    .get("from")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");

                tracing::info!(
                    source_node = from,
                    local_node = %node_id_share,
                    "[Cluster] Received forge reflection report from peer"
                );

                if let Err(e) = provider_share.receive_reflection(&payload) {
                    tracing::error!(error = %e, "[Cluster] Failed to store reflection");
                    return Ok(serde_json::json!({
                        "status": "error",
                        "error": format!("Failed to store reflection: {}", e),
                    }));
                }

                Ok(serde_json::json!({
                    "status": "ok",
                    "message": "Reflection received",
                    "node_id": node_id_share,
                    "timestamp": chrono::Local::now().to_rfc3339(),
                }))
            }),
        )?;

        // forge_get_reflections: list available local reflections
        let provider_list = provider.clone_boxed();
        let node_id_list = node_id.clone();
        self.register_rpc_handler(
            "forge_get_reflections",
            Box::new(move |payload| {
                let from = payload
                    .get("from")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");

                tracing::info!(
                    from = from,
                    local_node = %node_id_list,
                    "[Cluster] Reflections list requested by peer"
                );

                let mut result = provider_list.get_reflections_list_payload();

                // If a specific reflection is requested, include its content (sanitized)
                if let Some(filename) = payload.get("filename").and_then(|v| v.as_str())
                    && !filename.is_empty()
                {
                    match provider_list.read_reflection_content(filename) {
                        Ok(content) => {
                            result["content"] =
                                serde_json::Value::String(provider_list.sanitize_content(&content));
                            result["filename"] = serde_json::Value::String(filename.into());
                        }
                        Err(e) => {
                            tracing::error!(
                                filename = filename,
                                error = %e,
                                "[Cluster] Failed to read reflection"
                            );
                            return Ok(serde_json::json!({
                                "status": "error",
                                "error": format!("Failed to read reflection: {}", e),
                            }));
                        }
                    }
                }

                result["node_id"] = serde_json::Value::String(node_id_list.clone());

                Ok(result)
            }),
        )?;

        tracing::info!(
            "[Cluster] Registered forge RPC handlers: forge_share, forge_get_reflections"
        );
        Ok(())
    }

    // -- RPC handler builders (extracted for testability) ----------------------

    /// Build the peer_chat handler (B-side: receive message, ACK, process async).
    ///
    /// **⚠️ 这是 ACK 桩，生产环境会被覆盖，不要在这里加业务逻辑。**
    ///
    /// 注册链路（后注册者覆盖先注册者）：
    ///   1. `register_default_handlers()`（rpc/server.rs:494）注册最初的 ACK 桩
    ///   2. `register_peer_chat_handlers()`（cluster.rs:1630）调用本函数注册此桩，覆盖 #1
    ///   3. **gateway.rs:1189 用真正的 PeerChatHandler 覆盖此桩**（生产路径）
    ///
    /// 真 handler（gateway.rs:1189）做的事情：从 `payload._rpc.from` 提取 source_node_id、
    /// 通过 RpcMeta 传给 PeerChatHandler（PeerChatHandler 从 rpc_meta.from 取 source_node_id、
    /// 从 `_source.chat_id` 取 chat_id，组合成 session_key 用于 LLM 会话隔离）、
    /// 自动注册未知节点到 registry、调用 PeerChatHandler 入队 ClusterTaskList 异步处理、
    /// callback 通过 peer_chat_callback 回 A 端。
    ///
    /// 此桩仅在非 gateway 场景（如独立 cluster daemon、轻量节点）下生效。
    /// **修改 peer_chat 行为的正确位置是 gateway.rs:1189，不是这里。**
    pub(super) fn build_peer_chat_handler(&self) -> crate::rpc::server::RpcHandlerFn {
        let node_id = self.node_id.clone();
        Box::new(move |payload| {
            let content = payload
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if content.is_empty() {
                return Ok(serde_json::json!({
                    "status": "error",
                    "error": "content is required",
                }));
            }
            let task_id = payload
                .get("task_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            tracing::info!(
                node_id = %node_id,
                task_id = %task_id,
                "[Cluster] peer_chat received, returning ACK"
            );

            Ok(serde_json::json!({
                "status": "accepted",
                "task_id": task_id,
            }))
        })
    }

    /// Build the callback handler (A-side: receive result from B).
    pub(super) fn build_callback_handler(&self) -> crate::rpc::server::RpcHandlerFn {
        let task_manager = self.task_manager.clone();
        let cluster_task_list = self.cluster_task_list.lock().clone();
        let cluster_work_queue = self.cluster_work_queue.lock().clone();
        Box::new(move |payload| {
            let task_id = payload
                .get("task_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if task_id.is_empty() {
                return Ok(serde_json::json!({
                    "status": "error",
                    "error": "task_id is required",
                }));
            }

            let status = payload
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("success");
            let response = payload
                .get("response")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let error = payload.get("error").and_then(|v| v.as_str()).unwrap_or("");

            tracing::info!(
                task_id = %task_id,
                status = %status,
                "[Cluster] peer_chat_callback received"
            );

            // Check if this callback is for a cluster agent task (B-side forwarding).
            if let (Some(tl), Some(wq)) = (&cluster_task_list, &cluster_work_queue)
                && let Some(parent_id) = tl.find_by_child_task_id(task_id)
            {
                tracing::info!(
                    child_task_id = %task_id,
                    parent_task_id = %parent_id,
                    "[Cluster] Routing callback to cluster agent task"
                );
                if let Err(e) = tl.inject_callback(&parent_id, response) {
                    tracing::error!(
                        parent_task_id = %parent_id,
                        error = %e,
                        "[Cluster] inject_callback persisted to memory but disk persist failed — crash before resume loses the callback result"
                    );
                }
                if let Err(e) = wq.submit(parent_id) {
                    tracing::error!(error = %e, "[Cluster] Failed to re-submit task to work queue");
                }
                return Ok(serde_json::json!({
                    "status": "accepted",
                    "task_id": task_id,
                }));
            }

            // Fall through to main agent's TaskManager (A-side continuation).
            task_manager.complete_callback(task_id, status, response, error);
            // 负载感知路由：默认 handler 兜底臂（gateway 覆盖注册失败时
            // 服务）同样记账——经路由钩子与回调漏斗共用同一去重集，判据
            // 对齐 Route 3（status=="error" 记失败）。
            task_manager.fire_route_record(task_id, status != "error");

            Ok(serde_json::json!({
                "status": "accepted",
                "task_id": task_id,
            }))
        })
    }

    /// Build the query_task_result handler (B-side responds to A's polling).
    ///
    /// Mirrors Go's `buildQueryTaskResultHandler()`.
    pub(super) fn build_query_task_result_handler(&self) -> crate::rpc::server::RpcHandlerFn {
        let result_store = self.result_store.clone();
        Box::new(move |payload| {
            let task_id = payload
                .get("task_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if task_id.is_empty() {
                return Ok(serde_json::json!({
                    "status": "error",
                    "error": "task_id is required",
                }));
            }

            match result_store.get(task_id) {
                Some(entry) => {
                    // G5 修复（2026-09-01）：set_running 占位条目
                    // （gateway ClusterResultPersisterAdapter，result 内含
                    // "status":"running"）曾在此被误报为 done+空回复 —— A 端
                    // 恢复轮询会拿着空内容过早完成任务。running 条目必须
                    // 原样上报，让 A 继续等。
                    let inner_status = entry
                        .result
                        .get("status")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if inner_status == "running" {
                        return Ok(serde_json::json!({
                            "status": "running",
                            "task_id": task_id,
                        }));
                    }
                    let result_status = if entry.success { "success" } else { "error" };
                    // 键归一：写端（adapter set_result）现为 "response"；
                    // 保留 "content" 回退兼容修复前落盘的旧结果。
                    let response = entry
                        .result
                        .get("response")
                        .or_else(|| entry.result.get("content"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let error = entry
                        .result
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    Ok(serde_json::json!({
                        "status": "done",
                        "task_id": task_id,
                        "result_status": result_status,
                        "response": response,
                        "error": error,
                    }))
                }
                None => Ok(serde_json::json!({
                    "status": "not_found",
                    "task_id": task_id,
                })),
            }
        })
    }

    /// Build the confirm_task_delivery handler (A confirms it received result).
    ///
    /// Mirrors Go's `buildConfirmTaskDeliveryHandler()`.
    pub(super) fn build_confirm_task_delivery_handler(&self) -> crate::rpc::server::RpcHandlerFn {
        let result_store = self.result_store.clone();
        Box::new(move |payload| {
            let task_id = payload
                .get("task_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if task_id.is_empty() {
                return Ok(serde_json::json!({
                    "status": "error",
                    "error": "task_id is required",
                }));
            }

            result_store.remove(task_id);

            Ok(serde_json::json!({
                "status": "confirmed",
                "task_id": task_id,
            }))
        })
    }
}

impl Cluster {
    /// Confirm task delivery to the B-node, allowing it to clean up the result.
    ///
    /// Mirrors Go's `Cluster.confirmDelivery(peerID, taskID)`.
    pub fn confirm_delivery(&self, peer_id: &str, task_id: &str) {
        let payload = serde_json::json!({"task_id": task_id});
        // Clone the override OUT of the lock (same pattern as poll_stale_pending_tasks
        // and handle_task_complete). Binding the guard here would hold the
        // non-reentrant parking_lot Mutex across call_with_context, which re-locks
        // the same mutex at its override check → guaranteed self-deadlock on the
        // fallback branch (the production path when no test override is set).
        // (BUG #19, quality-hardening goal 冲刺 S4)
        let call_fn = self.call_with_context_fn.lock().clone();
        if let Some(f) = call_fn.as_ref() {
            let _ = f(peer_id, "confirm_task_delivery", payload);
        } else {
            // Fallback: try through call_with_context
            let _ = self.call_with_context(
                peer_id,
                "confirm_task_delivery",
                serde_json::json!({"task_id": task_id}),
            );
        }
    }
}

fn record_route_outcome_core(
    stats: &Mutex<std::collections::HashMap<String, RoutePeerStats>>,
    recorded: &Mutex<std::collections::HashSet<String>>,
    node_id: &str,
    task_manager: &Arc<TaskManager>,
    task_id: &str,
    ok: bool,
) {
    {
        let mut recorded = recorded.lock();
        if recorded.len() >= ROUTE_RECORDED_CAP {
            recorded.clear();
        }
        if !recorded.insert(task_id.to_string()) {
            return;
        }
    }
    let Some(task) = task_manager.get_task(task_id) else {
        return;
    };
    if task.peer_id.is_empty() || task.peer_id == node_id {
        return;
    }
    let duration_ms = task.completed_at.as_deref().and_then(|done| {
        let done = chrono::DateTime::parse_from_rfc3339(done).ok()?;
        let created = chrono::DateTime::parse_from_rfc3339(&task.created_at).ok()?;
        let ms = (done - created).num_milliseconds();
        u64::try_from(ms).ok()
    });
    let mut stats = stats.lock();
    let s = stats.entry(task.peer_id.clone()).or_default();
    if ok {
        s.completed += 1;
    } else {
        s.failed += 1;
    }
    if let Some(ms) = duration_ms {
        s.response_samples += 1;
        s.total_response_ms += ms;
    }
}

/// Poll stale pending tasks: query the B-node for any task that has been
/// pending for longer than 2 minutes. If the B-node reports it done, complete
/// the task locally; if not found, fail it; if older than the safety net,
/// time it out.
///
/// Uses the real RPC client when available, falling back to the synchronous
/// test override (`call_fn`).  This matches Go's `pollStalePendingTasks`
/// which calls `c.CallWithContext()`.
/// G5: `bus` 非空时，done / not_found / 安全网超时分支会按 gateway Route 2
/// 的形状向总线重发 `cluster_continuation:{task_id}`（含真实响应内容 +
/// metadata），唤醒 A 端主 agent 的续行恢复——生产装配不接
/// TaskManager.on_complete，仅靠 complete_callback 只会更新任务状态、不会
/// 唤醒 agent。（CD2，2026-09-17：安全网分支补齐 publish。）
///
/// CD3: `delivery_cb` 非空时，done 分支先触发交付回调（看板写回 / chat
/// 交付判定，路由在 gateway 注入的闭包里），回调返回 true 才向 worker 发
/// confirm 删除其本地副本；false 跳过 confirm（宁留勿丢）。
pub(super) async fn poll_stale_pending_tasks(
    task_manager: &Arc<TaskManager>,
    call_fn: &Option<
        Arc<dyn Fn(&str, &str, serde_json::Value) -> Result<Vec<u8>, String> + Send + Sync>,
    >,
    rpc_client: Option<&RpcClient>,
    safety_net: chrono::Duration,
    // G5+P1（T19 根修）：bus / delivery_cb 传句柄不传快照——publish 与交付
    // 回调在分支内**取用时现读**。此前每 tick 开头快照一次，而首 tick 快照
    // 必早于 set_message_bus / set_on_recovered_delivery（gateway 装配顺序
    // cluster.start() 先于二者），秒级 RPC 查询跨越注入窗口后 done 分支拿
    // 过期 None 静默跳过 publish。P1 sqlite 持久化使在途任务直接出现在首
    // tick（此前 in-memory store 重启为空、G5 重建注册只能赶上第二 tick），
    // 该竞态被 T19 实证暴露。
    bus: &Arc<Mutex<Option<Arc<dyn MessageBus>>>>,
    include_young: bool,
    // CD3（2026-09-17）：恢复交付回调——done 分支 confirm 前置闸，见
    // Cluster::set_on_recovered_delivery。未注册 = 旧行为（无条件 confirm）。
    delivery_cb: &Arc<
        Mutex<Option<Arc<dyn Fn(&str, &str, &str, Option<&str>) -> bool + Send + Sync>>>,
    >,
) {
    let tasks = task_manager.list_pending_tasks();

    if !tasks.is_empty() {
        tracing::debug!(count = tasks.len(), "[Cluster] Polling stale pending tasks",);
    }

    for task in tasks {
        // Parse created_at (RFC 3339) and compute age.
        let created = match chrono::DateTime::parse_from_rfc3339(&task.created_at) {
            Ok(dt) => dt.with_timezone(&chrono::Local),
            Err(_) => continue,
        };
        let age = chrono::Local::now() - created;

        // Skip tasks younger than 2 minutes. P6b（2026-09-11 日志）：恢复
        // loop 的首 tick 传 include_young=true 绕过此闸——重启后立即查询，
        // 不用白等 2 分钟（tokio interval 首 tick 本就即时，此前被这个
        // 年龄闸架空）。
        if !include_young && age < chrono::Duration::minutes(2) {
            continue;
        }

        // Timeout tasks past the safety net (G4: 可配置，默认 max(24h, 2×LLM超时)).
        if age > safety_net {
            // CD2（2026-09-17）：错误文案先落变量——complete_callback 与
            // publish_continuation_to_bus 两处共用，保证任务状态与 agent
            // 唤醒消息里的错误一致。
            let timeout_error = format!(
                "task timed out: no response within {}s safety net",
                safety_net.num_seconds()
            );
            tracing::warn!(
                task_id = %task.id,
                age_secs = age.num_seconds(),
                safety_net_secs = safety_net.num_seconds(),
                "[Cluster] Timing out stale task after safety-net timeout",
            );
            task_manager.complete_callback(&task.id, "error", "", &timeout_error);
            // 负载感知路由：安全网判死也是任务终态 settle，经路由钩子补记账
            // 失败样本（慢/坏节点信号）；与回调漏斗共用同一去重集，不双计。
            task_manager.fire_route_record(&task.id, false);
            // CD2（2026-09-17）：安全网超时同样要发 bus 唤醒 agent（error
            // 形态，形状对齐下方 not_found 分支）。此前只 complete_callback
            // 不 publish——任务状态标 Failed 了，但等回复的 agent 永远不知
            // 道，续行快照悬挂到永远。
            // 先落绑再判：MutexGuard 临时值不得跨 await（非 Send）。
            let bus_snapshot = bus.lock().clone();
            if let Some(bus) = bus_snapshot {
                publish_continuation_to_bus(
                    bus.as_ref(),
                    &task.id,
                    "",
                    "error",
                    &task.peer_id,
                    Some(&timeout_error),
                )
                .await;
            }
            continue;
        }

        // Need a peer_id to query.
        if task.peer_id.is_empty() {
            continue;
        }

        // Query the remote peer for the task result.
        let payload = serde_json::json!({"task_id": task.id});

        // Prefer real RPC client (matching Go's c.CallWithContext), fall back
        // to synchronous test override.
        let result = if let Some(client) = rpc_client {
            let request = crate::rpc_types::RPCRequest {
                id: uuid::Uuid::new_v4().to_string(),
                action: crate::rpc_types::ActionType::Custom("query_task_result".to_string()),
                payload: payload.clone(),
                source: String::new(),
                target: Some(task.peer_id.clone()),
            };
            match client
                .call_with_timeout(&task.peer_id, request, Duration::from_secs(30))
                .await
            {
                Ok(resp) => {
                    if let Some(ref err) = resp.error {
                        tracing::warn!(
                            task_id = %task.id,
                            error = %err,
                            "[Cluster] query_task_result returned error"
                        );
                        continue;
                    }
                    match resp.result {
                        Some(val) => val.to_string().into_bytes(),
                        None => continue,
                    }
                }
                Err(_) => continue,
            }
        } else if let Some(call) = call_fn {
            match call(&task.peer_id, "query_task_result", payload) {
                Ok(data) => data,
                Err(_) => continue,
            }
        } else {
            // No client available, skip.
            continue;
        };

        let resp: serde_json::Value = match serde_json::from_slice(&result) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let status = resp.get("status").and_then(|v| v.as_str()).unwrap_or("");

        match status {
            "running" => {
                // Still running, nothing to do.
                continue;
            }
            "done" => {
                let result_status = string_value(resp.get("result_status"));
                let response = string_value(resp.get("response"));
                let error = string_value(resp.get("error"));
                tracing::info!(
                    task_id = %task.id,
                    result_status = %result_status,
                    peer_id = %task.peer_id,
                    "[Cluster] Stale task recovered from peer",
                );
                task_manager.complete_callback(&task.id, &result_status, &response, &error);
                // 负载感知路由：恢复回灌 settle 补记账（判据对齐 gateway
                // Route 3：status=="error" 记失败，其余记成功）。
                task_manager.fire_route_record(&task.id, result_status != "error");
                // G5: 唤醒 A 端主 agent（形状对齐 gateway Route 2）。
                // 句柄现读（T19 根修）：RPC 查询跨越 set_message_bus 注入
                // 窗口时，tick 开头的旧快照是 None——必须在 publish 前重读。
                // 先落绑再判：MutexGuard 临时值不得跨 await（非 Send）。
                let bus_snapshot = bus.lock().clone();
                if let Some(bus) = bus_snapshot {
                    publish_continuation_to_bus(
                        bus.as_ref(),
                        &task.id,
                        &response,
                        &result_status,
                        &task.peer_id,
                        if error.is_empty() { None } else { Some(&error) },
                    )
                    .await;
                }
                // CD3（2026-09-17）：交付回调成功（或未注册回调 = 旧行为）才
                // confirm——写回成功才删 worker 端副本，宁留勿丢。回调返回
                // false（如看板写回失败）时跳过 confirm + WARN，副本走 worker
                // 端 7 天 TTL 兜底（TaskManager 已完成，下轮 poll 不会再查）。
                let delivery_ok = match delivery_cb.lock().clone() {
                    Some(cb) => {
                        let ok = cb(
                            &task.id,
                            &result_status,
                            &response,
                            if error.is_empty() { None } else { Some(&error) },
                        );
                        if !ok {
                            tracing::warn!(
                                task_id = %task.id,
                                "[Cluster] recovered delivery callback reported failure; \
                                 skipping confirm (worker copy kept until TTL)"
                            );
                        }
                        ok
                    }
                    None => true,
                };
                if delivery_ok {
                    // Best-effort delivery confirmation
                    confirm_delivery_with(call_fn, rpc_client, &task.peer_id, &task.id).await;
                }
            }
            "not_found" => {
                tracing::warn!(
                    task_id = %task.id,
                    peer_id = %task.peer_id,
                    "[Cluster] Stale task not found on remote peer",
                );
                task_manager.complete_callback(&task.id, "error", "", "remote task not found");
                // 负载感知路由：worker 丢任务（典型：重启）= 该节点失败样本，
                // 经路由钩子补记账。
                task_manager.fire_route_record(&task.id, false);
                // N3c（2026-10-12）：not_found = worker 明确说「没这任务」
                //（典型：worker 重启丢内存任务表）——确定性判死信号，board
                // 派发必须立即走交付回调（write_back FAILED + ⛔ 评论 +
                // in_review + spawn 评审），不等 dispatch_timeout sweep
                //（否则任务已死、派发行悬 dispatched、评审链最长空转 3600s）。
                // worker 端已无副本，无需 confirm；回调失败（写回竞态）时
                // 派发行仍 dispatched，sweep 照常兜底，宁留勿丢语义不变。
                if let Some(cb) = delivery_cb.lock().clone() {
                    let ok = cb(&task.id, "error", "", Some("remote task not found"));
                    if !ok {
                        tracing::warn!(
                            task_id = %task.id,
                            "[Cluster] recovered delivery callback reported failure \
                             (not_found); dispatch row stays for sweep fallback"
                        );
                    }
                }
                // G5: not_found 同样要唤醒 agent（带 error metadata），
                // 否则续行快照永远悬着直到安全网清理。句柄现读（T19 根修）。
                // 先落绑再判：MutexGuard 临时值不得跨 await（非 Send）。
                let bus_snapshot = bus.lock().clone();
                if let Some(bus) = bus_snapshot {
                    publish_continuation_to_bus(
                        bus.as_ref(),
                        &task.id,
                        "",
                        "error",
                        &task.peer_id,
                        Some("remote task not found"),
                    )
                    .await;
                }
            }
            _ => {
                // Unknown status, skip.
                continue;
            }
        }
    }
}

/// G5: 按 gateway Route 2 的形状把恢复出的任务结果发回总线，唤醒 A 端
/// 主 agent 的 `cluster_continuation` 续行恢复。
///
/// - `sender_id` = `cluster_continuation:{task_id}`（AgentLoop 拦截前缀）
/// - `content` = B 端真实响应（恢复的 LLM 回复）
/// - `metadata` = status / source_node / error（与 Route 2 字段一致）
///
/// T19 二层根修（2026-10-16）：恢复首 tick 的 publish 可能早于 AgentLoop
/// 订阅总线（gateway 装配顺序 cluster.start() → … → AgentLoop，实测订阅
/// 晚 ~131ms；P1 sqlite 持久化把恢复动作从第二 tick 提到首 tick 后暴露
/// ——此前动作落 120s 后的第二 tick，订阅早已就位，两层竞态同被掩盖）。
/// broadcast send 对无接收者是**静默丢弃**（publish_inbound 返回 ()），
/// 故发布前等待接收者就位，上界 15s（150×100ms 轮询）——超时照发不吞
/// （宁试勿丢；总线侧仍有 no-receiver WARN 可观测）。
async fn publish_continuation_to_bus(
    bus: &dyn MessageBus,
    task_id: &str,
    content: &str,
    status: &str,
    source_node: &str,
    error: Option<&str>,
) {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while bus.inbound_receiver_count() == 0 && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("status".to_string(), status.to_string());
    metadata.insert("source_node".to_string(), source_node.to_string());
    if let Some(err) = error {
        metadata.insert("error".to_string(), err.to_string());
    }
    bus.publish_inbound(BusInboundMessage {
        channel: "system".into(),
        sender_id: format!("cluster_continuation:{}", task_id),
        chat_id: String::new(),
        content: content.to_string(),
        metadata,
    });
}

/// Notify the B-node that the task result was received.
/// Uses the RPC client if available, otherwise the synchronous test override.
pub(super) async fn confirm_delivery_with(
    call_fn: &Option<
        Arc<dyn Fn(&str, &str, serde_json::Value) -> Result<Vec<u8>, String> + Send + Sync>,
    >,
    rpc_client: Option<&RpcClient>,
    peer_id: &str,
    task_id: &str,
) {
    let payload = serde_json::json!({"task_id": task_id});

    if let Some(client) = rpc_client {
        let request = crate::rpc_types::RPCRequest {
            id: uuid::Uuid::new_v4().to_string(),
            action: crate::rpc_types::ActionType::Custom("confirm_task_delivery".to_string()),
            payload,
            source: String::new(),
            target: Some(peer_id.to_string()),
        };
        let _ = client
            .call_with_timeout(peer_id, request, Duration::from_secs(30))
            .await;
    } else if let Some(call) = call_fn {
        let _ = call(peer_id, "confirm_task_delivery", payload);
    }
}

/// Extract a string value from a JSON Value, returning "" for null / missing.
pub(super) fn string_value(v: Option<&serde_json::Value>) -> String {
    match v {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Null) => String::new(),
        Some(other) => {
            // For non-string scalars, return the JSON representation without quotes.
            if other.is_number() || other.is_boolean() {
                other.to_string()
            } else {
                other.as_str().unwrap_or("").to_string()
            }
        }
        None => String::new(),
    }
}

// ---------------------------------------------------------------------------
// ClusterCallbacks implementation (for discovery service integration)
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// PeerResolver implementation
// ---------------------------------------------------------------------------

/// Adapts the Cluster's registry to the `PeerResolver` trait needed by `RpcClient`.
pub(super) struct ClusterPeerResolver {
    pub(super) registry: Arc<PeerRegistry>,
    pub(super) node_id: String,
    /// 本节点 RPC 端口（R4 自投递防护：候选端点 ip:port 与自身 endpoint
    /// 全匹配 = 注册表混池污染面，剔除——只按裸 IP 判定会误杀同机多节点
    /// 拓扑，端口是它们的唯一区分维度）。
    pub(super) rpc_port: u16,
}

/// R4 纯函数核：候选端点是否落在本节点自身 endpoint 上。
///
/// 判定 = **端口全匹配** 且（主机为回环 或 主机在本机 IP 集合内）。
/// 同机多节点（cluster-uat / 本地双实例）靠端口区分——裸 IP 判定会把
/// 合法同机对端误杀。`host` 允许携带端口（取 host 部分）。
pub(crate) fn is_self_endpoint(host: &str, port: u16, own_port: u16, own_ips: &[String]) -> bool {
    if port != own_port {
        return false;
    }
    let host_only = host.split(':').next().unwrap_or(host);
    if let Ok(ip) = host_only.parse::<std::net::IpAddr>()
        && ip.is_loopback()
    {
        return true;
    }
    own_ips.iter().any(|o| o.eq_ignore_ascii_case(host_only))
}

impl PeerResolver for ClusterPeerResolver {
    fn get_peer_info(&self, peer_id: &str) -> Option<(Vec<String>, u16, bool)> {
        // 1. Direct lookup by key (e.g. "Node-A" or a node_id)
        if let Some(info) = self.registry.get(peer_id) {
            return self.resolve_dial_candidates(&info);
        }

        // 2. Fallback: scan all peers for matching node_id or name.
        //    This handles cases where the caller uses a node_id (e.g. "node-laptop-xxx")
        //    but the registry key is a peer name (e.g. "Node-A").
        let all = self.registry.list_peers();
        for info in &all {
            if info.base.id == peer_id || info.base.name == peer_id {
                return self.resolve_dial_candidates(info);
            }
        }

        None
    }

    fn get_local_interfaces(&self) -> Vec<LocalNetworkInterface> {
        network::get_local_network_interfaces()
            .into_iter()
            .map(|iface| LocalNetworkInterface {
                ip: iface.ip,
                mask: iface.mask,
            })
            .collect()
    }

    fn get_node_id(&self) -> String {
        self.node_id.clone()
    }
}

impl ClusterPeerResolver {
    /// 由注册表条目推导拨号候选 `(addresses, rpc_port, is_online)`。
    ///
    /// R3 拨号兜底：`base.address`（静态 peers.toml 的地址 / announce
    /// primary）**始终参与拨号**——池非空也不丢（旧实现池非空时完全不
    /// 消费 base.address，静态 pin 被池刷新挤掉后即彻底失联）。host 去重
    /// 后追加在池尾兜底。
    ///
    /// R4 自投递防护：候选端点与本节点自身 endpoint（本机 IP × 本节点
    /// RPC 端口，含回环）全匹配 = 注册表把本节点历史上报地址混进对端池
    /// （Win↔Win 验收实证的自投递劫持面：同端口布局下拨回自己 RPC
    /// server，dispatch 报成功实际本节点执行、交付作者身份漂移）。剔除
    /// 后全空 = 该对端在非自端点意义上不可解析，返回 `None`（诚实失败，
    /// 上层派发失败路径照常留痕），同时 WARN 留诊断现场。
    ///
    /// 自派豁免：条目 id 与本节点 id **全等** = 显式自投递（file: 锚点单
    /// 「只能自派」拓扑约束的合法形态），端点自碰撞是预期而非池污染——
    /// 原样放行全部候选，拨回本节点 RPC server 走正常 peer_chat 语义
    /// （作者=执行者，无身份漂移；任务完成回调按 source_node_id 回程，
    /// 同一豁免覆盖）。防护的真实目标是「别人的单子误投给自己」（id
    /// 不等但候选全撞本地端点），豁免只钉住 id 全等这一条路。
    fn resolve_dial_candidates(&self, info: &ExtendedNodeInfo) -> Option<(Vec<String>, u16, bool)> {
        let is_online = info.status == NodeStatus::Online;
        // A1（QUIC 中继钉址实测 2026-10-21）：iroh 形态 base.address 是原子
        // （node_id+可选 relay_url），按冒号拆 host 会拆出
        // `iroh:<hex>@http://127.0.0.1` 截断垃圾候选（全址 best 失败后 junk
        // 白撞一次拨号 + 失败链 last error 用垃圾地址误导排障）——与
        // handle_discovered_node_impl pin 路径（:1739 同病同修「整条入 pin」）
        // 同族。iroh 形态整条作 host 候选参与去重，port=0（iroh 地址内无
        // RPC 端口语义；`:33440` 是中继端口，绝不能当 rpc_port 派生给
        // host-only 候选拼对）。
        let (host, port) = if crate::is_roaming_address(&info.base.address) {
            (info.base.address.clone(), 0)
        } else {
            parse_host_port(&info.base.address)
        };
        let mut addresses = info.addresses.clone();
        // iroh 原子 host 按整串相等去重（`split(':').next()` 对 iroh 形态
        // 恒得 "iroh"，会把整条重复候选拱进池）。
        let host_in_pool = if crate::is_roaming_address(&host) {
            addresses.iter().any(|a| a == &host)
        } else {
            addresses
                .iter()
                .any(|a| a.split(':').next() == Some(host.as_str()))
        };
        if !host.is_empty() && !host_in_pool {
            addresses.push(host);
        }

        if info.base.id == self.node_id {
            tracing::info!(
                peer_id = %info.base.id,
                own_rpc_port = self.rpc_port,
                candidates = ?addresses,
                "[Cluster] Self-entry dial exemption: explicit self-dispatch, keeping all candidates"
            );
            return Some((addresses, port, is_online));
        }

        let own_ips = network::get_all_local_ips();
        let filtered: Vec<String> = addresses
            .iter()
            .filter(|a| {
                // 裸 IP 候选的端口配对与本 resolver 返回的 rpc_port 一致
                // （dispatch 侧按同语义拼接 ip:port）；带端口条目按自带端口。
                let (h, p) = parse_host_port(a);
                let effective_port = if a.contains(':') { p } else { port };
                !is_self_endpoint(&h, effective_port, self.rpc_port, &own_ips)
            })
            .cloned()
            .collect();
        // 全滤空 WARN 覆盖对称（多角度审计批低-4 + 闭环批低-4 补全）：判据
        // = 「过滤后为空」。三形态诚实失败（返回 `None`，上层派发失败路径
        // 照常留痕）：① 过滤前有候选但全为自端点（池污染）；② 池原本为空、
        // 唯一候选是 base.host 且恰为自端点（返回 Some 空池 = 客户端拿 ""
        // 拨号一次才失败，错误文案误导为连接问题而非自碰撞）；③ 连候选都
        // 没有（announce/静态 pin 全缺）——Some 空池同样是拿 "" 空拨一次，
        // 与①②同罪，统一 None。
        if filtered.is_empty() {
            if addresses.is_empty() {
                tracing::warn!(
                    peer_id = %info.base.id,
                    "[Cluster] No dial candidates at all for peer (no address pool, \
                     no usable base address); cannot resolve"
                );
            } else {
                tracing::warn!(
                    peer_id = %info.base.id,
                    raw_candidates = ?addresses,
                    own_rpc_port = self.rpc_port,
                    "[Cluster] All dial candidates for peer collide with the local endpoint \
                     (registry address-pool pollution?); refusing to dial self"
                );
            }
            return None;
        }
        Some((filtered, port, is_online))
    }
}

pub(super) fn parse_host_port(addr: &str) -> (String, u16) {
    if let Some(idx) = addr.rfind(':') {
        let host = &addr[..idx];
        let port_str = &addr[idx + 1..];
        let port = port_str.parse().unwrap_or(DEFAULT_RPC_PORT);
        (host.into(), port)
    } else {
        (addr.into(), DEFAULT_RPC_PORT)
    }
}

/// Generate a node ID based on hostname and timestamp.
pub(super) fn generate_node_id() -> String {
    let hostname = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown".into());
    format!("node-{}-{}", hostname.to_lowercase(), uuid::Uuid::new_v4())
}

/// 批次四：显示名 hostname 链（COMPUTERNAME → HOSTNAME，与
/// `generate_node_id` 同款）。空 / `unknown`（纯容器环境）= None——
/// 调用方落 `Bot {id8}` 兜底。
pub(crate) fn hostname_display_name() -> Option<String> {
    let h = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_default();
    let h = h.trim().to_string();
    if h.is_empty() || h.eq_ignore_ascii_case("unknown") {
        None
    } else {
        Some(h)
    }
}

/// 批次四：显示名自动解析链（goal 钉死优先级）的**纯函数核**：
/// `config.cluster.json node_name` 非空 →（用它，显式免疫撞名后缀）；
/// 否则 hostname 链（由调用方注入 `hostname_display_name()` 结果）；
/// 空 / `unknown` → `Bot {id8}` 兜底（纯容器环境）。
/// 返回 `(名字, 显式锁定)`——锁定名不被撞名后缀收敛（重名无害，唯一性
/// 靠 node_id；后缀只作用于自动名）。
///
/// hostname 作为参数注入而非函数内读 env：env 是进程全局，并行测试
/// 注入 `COMPUTERNAME` 会互相污染（且 Rust 2024 `set_var` 是 unsafe）——
/// 三元纯函数让优先级链可零 env 依赖直测（见 `cluster/tests.rs`）。
pub(crate) fn resolve_auto_node_name(
    config_name: &str,
    hostname: Option<String>,
    node_id: &str,
) -> (String, bool) {
    let cn = config_name.trim();
    if !cn.is_empty() {
        return (cn.to_string(), true);
    }
    if let Some(h) = hostname {
        return (h, false);
    }
    // node_id 可能含多字节文本（中文主机名等），裸字节截断会 panic
    // （2026-10-11 横扫修复），走边界安全版。
    (
        format!(
            "Bot {}",
            nemesis_utils::string_utils::truncate_bytes_at_boundary(node_id, 8)
        ),
        false,
    )
}

/// 批次四：解析链的 env 薄包装——生产路径（`new` / `with_workspace`）
/// 调这个；测试直接调 `resolve_auto_node_name` 纯函数核。
pub(crate) fn resolve_node_name_from_env(config_name: &str, node_id: &str) -> (String, bool) {
    resolve_auto_node_name(config_name, hostname_display_name(), node_id)
}

/// 批次四：撞名后缀来源——node_id **末段**前 4 字符。
///
/// goal 字面是「node_id 前 4 位」，但生产 id 形如 `node-{hostname}-{uuid}`
/// （`generate_node_id`），前 4 位恒为 `"node"`——真机撞名双方各自算出的
/// 后缀相同，收敛失效（2026-09-19 真机彩排实测）。末段（uuid 段）前 4
/// 保留 goal 意图：稳定（id 不变则后缀不变，重启不变）+ 有区分度
/// （uuid 随机段；4 hex 碰撞 ≈ 1/65536，对「显示名给人看」量级足够）。
pub(crate) fn name_suffix_from_node_id(node_id: &str) -> String {
    let last = node_id.rsplit('-').next().unwrap_or(node_id);
    last.chars().take(4).collect()
}

/// Atomic write helper — REL-002（2026-09-23）起委托统一 helper
/// `nemesis_utils::write_file_atomic`（唯一临时名 + sync_all + 失败清理 +
/// unix 0600 创建即挂）。保留签名兼容：`merge_real_node_info` 重写
/// peers.toml（占位 subtable 移除后回填 real_id）的唯一调用点。
pub(super) fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), data, 0o600)
        .map_err(std::io::Error::other)
}
