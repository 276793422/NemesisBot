// ---------------------------------------------------------------------------
// 机械拆分：本文件内容由 cluster.rs 原样搬入（纯代码搬家，行为零变更）。
// ---------------------------------------------------------------------------

use super::*;

impl Cluster {
    pub(super) fn start_health_check_loop(&self, interval: Duration, failure_threshold: u32) {
        let handle = match tokio::runtime::Handle::try_current() {
            Ok(h) => h,
            Err(_) => return, // No runtime available (e.g. in unit tests)
        };

        let mut stop_rx = self.stop_tx.subscribe();
        let registry = self.registry.clone();
        let rpc_client = self.rpc_client.lock().clone();
        let local_node_id = self.node_id.clone();
        // Arc 克隆进任务：探针复活时活读回调槽（gateway 组装晚于 start() 也能读到）
        let on_discovered = self.on_node_discovered.clone();
        // 自根弱引用槽：探针成功时把响应携带的 announce 身份落册（TCP fallback）
        let self_weak_slot = self.self_weak.clone();

        handle.spawn(async move {
            // 错过 tick 用 Delay 追赶语义（不补帧），避免卡顿后连发风暴
            let mut tick = tokio::time::interval(interval);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut tick_count: u64 = 0;
            loop {
                tokio::select! {
                    _ = stop_rx.recv() => {
                        return;
                    }
                    _ = tick.tick() => {
                        tick_count = tick_count.wrapping_add(1);

                        let peers = registry.list_peers();
                        for peer in peers {
                            // 跳过本节点（自 ping 无意义）
                            if peer.base.id == local_node_id {
                                continue;
                            }
                            let online = peer.status == NodeStatus::Online;
                            if !should_probe_peer(online, tick_count) {
                                continue;
                            }

                            let client = match rpc_client.as_ref() {
                                Some(c) => c.clone(),
                                None => return, // RPC client never initialized
                            };
                            let node_id = peer.base.id.clone();
                            let request = crate::rpc_types::RPCRequest {
                                id: uuid::Uuid::new_v4().to_string(),
                                action: crate::rpc_types::ActionType::Custom(
                                    "ping".to_string(),
                                ),
                                payload: serde_json::json!({}),
                                source: local_node_id.clone(),
                                target: Some(node_id.clone()),
                            };
                            // 5s 探针超时：正常 LAN RTT <1ms，5s 足够宽容
                            let result = client
                                .call_probe_with_timeout(&node_id, request, Duration::from_secs(5))
                                .await;

                            match result {
                                Ok(resp) => {
                                    // 交叉校验对端响应 timestamp，漂移过大限频 WARN
                                    // （复用 discovery 的 AnnounceWarnGate，10 分钟冷却）
                                    if let Some(ts) = resp
                                        .result
                                        .as_ref()
                                        .and_then(|r| r.get("timestamp"))
                                        .and_then(|t| t.as_i64())
                                    {
                                        let drift =
                                            crate::discovery::drift_secs(chrono::Utc::now().timestamp(), ts);
                                        if drift > crate::discovery::DRIFT_WARN_THRESHOLD_SECS as u64
                                            && probe_drift_gate().admit(&node_id)
                                        {
                                            tracing::warn!(
                                                node_id = %node_id,
                                                drift_secs = drift,
                                                "[Cluster] Health probe response timestamp drift (peer clock not synced?)"
                                            );
                                        }
                                    }
                                    // 身份回传（TCP fallback）：ping 响应携带
                                    // announce（与 UDP announce 同构的 additive 字段）时，
                                    // 经 apply_probe_announce 走 handle_discovered_node_ex
                                    // 同一入册漏斗。UDP 静默拓扑（CI ghost 进程占端口、
                                    // AP 隔离、跨网段多宿主）里这是身份传播的唯一可靠
                                    // 通道——否则静态 peers.toml 入册的条目会带着创建期
                                    // 的空身份冻结到永远（U 家族 9 连红根因）。
                                    if let Some(ann) = resp.result.as_ref().and_then(|r| r.get("announce"))
                                        && let Some(c) = self_weak_slot.lock().clone().and_then(|w| w.upgrade())
                                    {
                                        c.apply_probe_announce(ann);
                                    }
                                    if registry.record_probe_success(&node_id) {
                                        tracing::info!(
                                            node_id = %node_id,
                                            "[Cluster] Health probe: peer recovered -> Online"
                                        );
                                        logger::log_discovery_info(
                                            "health probe recovered, marked Online",
                                        );
                                        // 复活也是「节点发现」：广播被隔离的拓扑（多宿主
                                        // 主机跨网段、AP 隔离）里 announce 永远不来，探针
                                        // 翻转是唯一的 Offline→Online 信号源——停车场
                                        // sweep / 看板收编必须同样感知（真机缺陷修复）。
                                        Self::fire_recovered_callback(
                                            &on_discovered,
                                            &registry,
                                            &node_id,
                                        );
                                    }
                                }
                                Err(e) => {
                                    if registry.record_probe_failure(&node_id, failure_threshold) {
                                        tracing::warn!(
                                            node_id = %node_id,
                                            error = %e,
                                            threshold = failure_threshold,
                                            "[Cluster] Health probe: peer marked Offline after consecutive failures"
                                        );
                                        logger::log_discovery_info(&format!(
                                            "health probe failed {}x, marked Offline",
                                            failure_threshold
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });
    }
}

impl Cluster {
    /// Mark a node as offline.
    pub fn handle_node_offline(&self, node_id: &str, _reason: &str) {
        if let Some(mut info) = self.registry.get(node_id) {
            tracing::warn!(
                node_id = node_id,
                name = %info.base.name,
                "[Cluster] Node went offline: id={}",
                node_id,
            );
            info.status = NodeStatus::Offline;
            self.registry.upsert(info);
            logger::log_discovery("offline", "", Some(node_id));
        } else {
            tracing::debug!(
                node_id = node_id,
                "[Cluster] Node offline event for unknown node: id={}",
                node_id,
            );
        }
    }
}

impl Cluster {
    /// Temporarily mark a peer as Online so the RPC resolver doesn't block
    /// the call. Used by `nodes.refresh` to bypass the offline-check before
    /// attempting a `get_info` RPC; the caller should restore the original
    /// status if the call fails.
    pub fn mark_peer_online_for_refresh(&self, peer_id: &str) {
        if let Some(mut info) = self.registry.get(peer_id) {
            info.status = NodeStatus::Online;
            self.registry.upsert(info);
        }
    }

    /// Set a peer's status (used to restore state after a refresh attempt).
    pub fn set_peer_status(&self, peer_id: &str, status: NodeStatus) {
        if let Some(mut info) = self.registry.get(peer_id) {
            info.status = status;
            self.registry.upsert(info);
        }
    }

    /// Record a successful connectivity probe (e.g. `nodes_ping` TCP connect
    /// succeeded). Refreshes `last_health_check` to now and ensures the peer
    /// is marked Online. Use this whenever an external reachability check
    /// confirms the peer is alive — independent of the UDP discovery loop.
    ///
    /// Offline→Online 翻转同样触发 `on_node_discovered`（复活也是节点发现，
    /// 与 探针复活同语义，见 `fire_recovered_callback`）；已 Online 的
    /// 重复确认不触发（幂等）。
    pub fn mark_peer_healthy(&self, node_id: &str) {
        let was_offline = self
            .registry
            .get(node_id)
            .map(|info| info.status != NodeStatus::Online)
            .unwrap_or(false);
        self.registry.mark_healthy(node_id);
        if was_offline {
            Self::fire_recovered_callback(&self.on_node_discovered, &self.registry, node_id);
        }
    }

    /// Record a failed connectivity probe (e.g. `nodes_ping` timed out or
    /// TCP RST). Immediately flips the peer to Offline — does NOT go through
    /// the `consecutive_failures` accumulator because the user explicitly
    /// probed and observed failure. Also emits a discovery log entry so the
    /// event surfaces in cluster_{date}.log.
    pub fn mark_peer_offline(&self, node_id: &str, reason: &str) {
        self.registry.mark_offline(node_id, reason);
        crate::logger::log_discovery("offline", reason, Some(node_id));
    }

    /// On-demand frame-level connectivity probe of a single peer（探针
    /// 同款形态：`Custom("ping")` + 5s 超时，`require_online=false`——对端
    /// 被标 Offline 也能探到真话）。返回对端是否帧级应答。
    ///
    /// 与健康检查循环的分工：本方法**不**因单次失败翻转注册表状态（那是
    /// 连败累计的职责，单次失败可能是瞬态抖动）；成功侧顺带
    /// [`Self::mark_peer_healthy`]（幂等，Offline→Online 复活广播照常触发
    /// ——看板停车场 sweep 依赖该信号，业务流主动接触到的复活不该被
    /// sweep 晚一轮才看）。消费方：看板冲突硬解重派前的三轮主动接触
    ///（board conflict_resolver）。
    pub async fn probe_peer(&self, node_id: &str) -> bool {
        let Some(client) = self.rpc_client_arc() else {
            return false;
        };
        let request = crate::rpc_types::RPCRequest {
            id: uuid::Uuid::new_v4().to_string(),
            action: crate::rpc_types::ActionType::Custom("ping".to_string()),
            payload: serde_json::json!({}),
            source: self.node_id.clone(),
            target: Some(node_id.to_string()),
        };
        match client
            .call_probe_with_timeout(node_id, request, std::time::Duration::from_secs(5))
            .await
        {
            Ok(_) => {
                self.mark_peer_healthy(node_id);
                true
            }
            Err(_) => false,
        }
    }

    /// Mark a peer as static/configured (loaded from peers.toml). Static peers
    /// are exempt from UDP-staleness expiry in `check_health` — they have a
    /// known address and stay Online across UDP announce gaps. Call after
    /// loading each static peer at startup.
    pub fn mark_peer_static(&self, node_id: &str) -> bool {
        self.registry.mark_static(node_id)
    }
}

/// 探针调度判定（纯函数，便于单测）：Online 对端每 tick 都探；
/// Offline 对端**前 12 个 tick 全探**（启动冲刺——纯中继/UDP 静默拓扑
/// （跨网自建中继，announce 永远不来）里静态 peer 的 Offline→Online 只能
/// 靠探针，降频 1/5 × 60s interval = 首次上线最长 5 分钟，用户视角等同
/// 「组网失败」；冲刺窗口内全探把首拨号提前到秒级。窗口取 12 = 默认 60s
/// 间隔下覆盖 12 分钟的人尺度启动偏差——用户先起 A 再去配 B 是常态，B 晚
/// 到 5~10 分钟仍在窗口内秒级互），之后每 5 个 tick 探一次（1/5 降频
/// 自愈——不是放弃，是恢复手段，一次成功立即翻回 Online）。
/// `tick` 为 1-based tick 序号。
pub(super) fn should_probe_peer(online: bool, tick: u64) -> bool {
    online || tick <= 12 || tick.is_multiple_of(5)
}

/// 探针 timestamp 漂移 WARN 限频闸门（进程级，每节点 10 分钟冷却）。
pub(super) fn probe_drift_gate() -> &'static crate::discovery::AnnounceWarnGate {
    static GATE: std::sync::OnceLock<crate::discovery::AnnounceWarnGate> =
        std::sync::OnceLock::new();
    GATE.get_or_init(crate::discovery::AnnounceWarnGate::new)
}

/// 负载感知路由记账核心（自由函数，语义统一定义）：task_id → 目标节点
/// 成功/失败 +1，响应时长样本（created_at→completed_at 实测，RFC3339 解析
/// 失败只计胜负不计时——宁缺毋假）随可解析时累加。幂等重放去重：每任务
/// 只计首次 settle——重复回调（送达重试/B 端重发/恢复轮询重放）经
/// settle_task 幂等成功（返回 true）再次进入记账，不去重则同一任务双计、
/// 节点成功率失真。空 peer_id / 本节点自身任务不入账（路由只关心「派给
/// 谁」）。入口三处共用：① 回调漏斗 [`Cluster::record_route_outcome`]；
/// ② 构造期路由钩子（恢复轮询 sweep 三分支 + 默认回调 handler 兜底臂 +
/// 清理 tick 的 安全网判死，经 `TaskManager::fire_route_record`
/// 触达）；③ 两臂全部捕获同一 stats/去重集。
/// 路由去重集容量上限（内存护栏）。条目只在「settle 后短时间内可能收到
/// 重放」的窗口内有意义（送达重试/ 恢复轮询都在分钟级），而任务本体
/// 2h 后即被 清理——集内旧条目是纯死重量。超限整体清空重新计数：
/// 丢的只是远超任何重放窗口的旧任务去重位，最坏情形（窗口外恰好来重放）
/// 是单个样本双计，自限且可忽略。10 万条 ≈ 15MB 上界，远超看板自动化
/// 现实任务量级。
pub(super) const ROUTE_RECORDED_CAP: usize = 100_000;
