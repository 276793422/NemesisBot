// ---------------------------------------------------------------------------
// 机械拆分：本文件内容由 cluster.rs 原样搬入（纯代码搬家，行为零变更）。
// ---------------------------------------------------------------------------

use super::*;

impl Cluster {
    /// state.toml 回读（集群完备性加固）：sync_loop 周期把注册表
    /// 持久化到 state.toml，但重启后从未回读——discovered 历史在首个 sync
    /// tick 就被「只有本节点」的注册表覆盖（write-only 假持久化）。本函数把
    /// 磁盘 discovered 条目以 **Offline** 状态种回注册表（探针 1/5 降频
    /// 或 announce 到达时复活；**绝不因回读直接 Online**——诚实语义：存活
    /// 未知）。跳过：空 id、本节点、黑名单节点（removed_peers 不复活）、
    /// 已有条目（静态 peers 在 start() 前装载，不覆盖）、不可拨号地址
    /// （空/无 host:port 形态，与 的 rpc_port=0 同语义）、**同地址或
    /// 同名的既有条目**（根修：注册表里已有该物理节点时再
    /// 种一份会造出「同节点双条目」—— RpcClient 直接键命中离线影子就拒绝
    /// 拨号，遮蔽同地址的运行时 id 在线条目）。返回种入数。
    pub(super) fn restore_discovered_from_state(&self) -> usize {
        let state = match crate::cluster_config::load_dynamic_state(&self.dynamic_state_path) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    path = %self.dynamic_state_path.display(),
                    error = %e,
                    "[Cluster] state.toml 回读失败（忽略，按空注册表启动）"
                );
                return 0;
            }
        };
        let mut seeded = 0usize;
        for pc in state.discovered {
            if pc.id.trim().is_empty() || pc.id == self.node_id {
                continue;
            }
            // 旧版 state.toml 可能存过未过身份闸的 id
            // （闸先于本函数存在）——坏 id 不种回注册表，与 announce 闸同源。
            if !crate::cluster_config::is_valid_node_identity(&pc.id) {
                continue;
            }
            if self.removed_peers.read().contains(&pc.id) {
                continue; // 用户显式移除的节点不复活
            }
            if self.registry.get(&pc.id).is_some() {
                continue; // 静态 peers / 已有条目：原值保留
            }
            let addr = pc.address.trim();
            if addr.is_empty() || !addr.contains(':') {
                continue; // 不可拨号 → 不种（state.toml 不存 unusable 条目）
            }
            // 同地址 / 同名既有条目 → 同一物理节点已在注册表（典型：静态
            // peer 按运行时 id 键控，state.toml 还留着升级前的占位名条目，
            // 如 id="Node-A"）。再种一份 = RpcClient 直接键命中离线影子拒绝
            // 拨号（UAT 实证：D 重启后 D→A 全部 duration_ms=0 失败）。
            // 跳过，让既有条目承担探针/announce 复活。
            let name = pc.name.trim();
            let shadows_existing = self.registry.find_by_address(addr).is_some()
                || (!name.is_empty()
                    && self
                        .registry
                        .list_peers()
                        .iter()
                        .any(|p| p.base.name == name));
            if shadows_existing {
                tracing::debug!(
                    id = %pc.id,
                    name = %name,
                    address = %addr,
                    "[Cluster] state.toml 回读跳过：同地址/同名条目已在注册表（防同节点双条目）"
                );
                continue;
            }
            self.registry.upsert(ExtendedNodeInfo {
                base: nemesis_types::cluster::NodeInfo {
                    id: pc.id.clone(),
                    name: if pc.name.trim().is_empty() {
                        pc.id.clone()
                    } else {
                        pc.name.clone()
                    },
                    role: nemesis_types::cluster::NodeRole::from_role_str(&pc.role),
                    address: addr.to_string(),
                    category: pc.category.clone(),
                    last_seen: if pc.status.last_seen.is_empty() {
                        chrono::Local::now().to_rfc3339()
                    } else {
                        pc.status.last_seen.clone()
                    },
                },
                status: NodeStatus::Offline,
                capabilities: Vec::new(),
                tags: pc.tags.clone(),
                addresses: pc.addresses.clone(),
                professions: pc.professions.clone(),
                tier: pc.tier.clone(),
                // agents 不持久化到 state.toml（运行时清单，周期 announce
                // 自会重学——空 = 未宣告，绝不因回读假造清单）。
                agents: Vec::new(),
                // runtime_backend 与 agents 相反是静态身份——持久化并回读
                // （同 professions/tier）。
                runtime_backend: pc.runtime_backend.clone(),
                // build 同纪律：回读种入的 Offline 占位不带构建号（空 =
                // 未宣告），下次 announce/探针到达自会补齐。
                build: String::new(),
                node_type: String::new(),
            });
            seeded += 1;
        }
        if seeded > 0 {
            logger::log_discovery_info(&format!(
                "state.toml restored: seeded {seeded} known peers (Offline, pending probe/announce)"
            ));
            tracing::info!(
                "[Cluster] state.toml 回读：种入 {seeded} 个已知节点（Offline，待探针/announce 复活）"
            );
        }
        seeded
    }
}

impl Cluster {
    /// Spawn the sync loop as a background tokio task.
    ///
    /// Mirrors Go's `Cluster.syncLoop()`. Runs every `broadcast_interval`,
    /// checks for node timeouts and persists state to disk.
    pub(super) fn start_sync_loop(&self) {
        let handle = match tokio::runtime::Handle::try_current() {
            Ok(h) => h,
            Err(_) => return,
        };

        let mut stop_rx = self.stop_tx.subscribe();
        let registry = self.registry.clone();
        let local_node_id = self.node_id.clone();
        let workspace = self.workspace.clone();

        // Interval matches Go's broadcastInterval
        let interval_duration = self.broadcast_interval;

        handle.spawn(async move {
            let mut interval = tokio::time::interval(interval_duration);
            // 默认 Burst 会在挂起恢复后把错过的 announce/
            // 同步 tick 逐个补发（UDP 洪泛对端）——Delay 从当前时刻重排
            // （同：1509 先例）。
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = stop_rx.recv() => {
                        return;
                    }
                    _ = interval.tick() => {
                        // Refresh self first. The sync loop cannot call
                        // sync_local_node_to_registry (it's &self and the
                        // spawned task is 'static), so without this the local
                        // node's last_health_check would age out and check_health
                        // would flip self to Offline.
                        registry.mark_healthy(&local_node_id);

                        // Mark Online peers as Offline if last_health_check is
                        // older than stale_timeout_secs (default 90s = 3×
                        // broadcast_interval, tolerates 2 consecutive dropped
                        // UDP announces). Offline peers STAY in the registry
                        // — users expect the node list to retain known
                        // peers (including self) even when they're offline,
                        // and removing them would cause "select a peer"
                        // lookups to degrade into self-calls.
                        let expired = registry.check_health();
                        for node_id in &expired {
                            logger::log_discovery_info(&format!("Node expired: {}", node_id));
                        }

                        // Sync state to disk
                        let state_path =
                            nemesis_path::resolve_cluster_state_path_in_workspace(&workspace);
                        let state = DynamicState {
                            discovered: registry.list_peers().iter().map(|n| {
                                let mut pc = n.to_peer_config();
                                pc.status.state = n.get_status_string().into();
                                pc
                            }).collect(),
                            last_sync: chrono::Local::now().to_rfc3339(),
                        };
                        if let Err(e) = crate::cluster_config::save_dynamic_state(&state_path, &state) {
                            logger::log_discovery_error(&format!("Failed to sync config: {}", e));
                        }
                    }
                }
            }
        });
    }

    // -- Node management ------------------------------------------------------
}

impl Cluster {
    /// Rebuild the local node's registry entry from current identity fields
    /// and upsert it into the registry so list_nodes() reflects live state.
    pub(super) fn sync_local_node_to_registry(&self) {
        // Preserve the resolved address from the existing registry entry,
        // since self.address may still be 0.0.0.0 before start() resolves it.
        let existing_address = self
            .registry
            .get(&self.node_id)
            .map(|e| e.base.address.clone())
            .unwrap_or_else(|| self.address.clone());

        let role_str = self.role.read().clone();
        let role = nemesis_types::cluster::NodeRole::from_role_str(&role_str);
        let caps = self
            .capabilities
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let info = ExtendedNodeInfo {
            base: nemesis_types::cluster::NodeInfo {
                id: self.node_id.clone(),
                name: self.node_name.read().clone(),
                role,
                address: existing_address,
                category: self.category.read().clone(),
                last_seen: chrono::Local::now().to_rfc3339(),
            },
            status: NodeStatus::Online,
            capabilities: caps,
            tags: self.tags.read().clone(),
            addresses: vec![],
            professions: self.professions.read().clone(),
            tier: self.tier.read().clone(),
            agents: self.current_agent_roster(),
            runtime_backend: self.runtime_backend.read().clone(),
            build: self.build_hash.read().clone(),
            node_type: self.node_type.clone(),
        };
        self.registry.upsert(info);

        // QUIC 插件化 热更钩子：身份热改（8 个 setter 调用点全覆盖——
        // 本函数是它们共同尾站）后名册注册载荷里的成员身份字段同步刷新。
        // registry_start 插件侧幂等停旧起新；无 runtime 上下文（纯同步
        // 测试路径）/ 名册未挂（params None）= 静默跳过。
        if self.rendezvous_params.read().is_some() && tokio::runtime::Handle::try_current().is_ok()
        {
            self.restart_registry_from_params();
        }
    }

    /// Sync state to disk.
    pub fn sync_to_disk(&self) -> Result<(), ConfigError> {
        let nodes = self.registry.list_peers();
        let mut discovered = Vec::new();
        for node in &nodes {
            if node.base.id == self.node_id {
                continue;
            }
            discovered.push(PeerConfig {
                id: node.base.id.clone(),
                name: node.base.name.clone(),
                address: node.base.address.clone(),
                addresses: Vec::new(),
                rpc_port: 0,
                role: String::new(),
                category: node.base.category.clone(),
                tags: node.tags.clone(),
                professions: node.professions.clone(),
                tier: node.tier.clone(),
                runtime_backend: node.runtime_backend.clone(),
                priority: 1,
                enabled: true,
                status: PeerStatus {
                    state: match node.status {
                        NodeStatus::Online => "online".into(),
                        NodeStatus::Offline => "offline".into(),
                        _ => "unknown".into(),
                    },
                    last_seen: node.base.last_seen.clone(),
                    uptime: String::new(),
                    tasks_completed: 0,
                    success_rate: 0.0,
                    avg_response_time: 0,
                    last_error: String::new(),
                },
            });
        }

        let state = DynamicState {
            discovered,
            last_sync: chrono::Local::now().to_rfc3339(),
        };

        crate::cluster_config::save_dynamic_state(&self.dynamic_state_path, &state).map_err(|e| {
            tracing::error!(
                path = %self.dynamic_state_path.display(),
                error = %e,
                "[Cluster] Failed to sync state to disk",
            );
            e
        })
    }

    // -- Peer capability search ------------------------------------------------
}
