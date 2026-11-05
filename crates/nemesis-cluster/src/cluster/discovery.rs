// ---------------------------------------------------------------------------
// 机械拆分：本文件内容由 cluster.rs 原样搬入（纯代码搬家，行为零变更）。
// ---------------------------------------------------------------------------

use super::*;

impl Cluster {
    /// Start UDP discovery service.
    ///
    /// Call this after wrapping Cluster in `Arc` and passing the `Arc` as `arc_self`.
    /// Reads the encryption key from the same `token` field in `config.cluster.json`
    /// used for RPC auth. Token 引用解析失败或为空（rpc_auth_unavailable）时
    /// 整体不启动——拒绝无加密发现；仅 config.cluster.json 整个缺失的未配置
    /// 接线才会落到明文运行（一次性 WARN）。
    pub fn start_discovery(&self, arc_self: Arc<dyn ClusterCallbacks>) {
        if self.discovery_running.load(Ordering::SeqCst) {
            tracing::warn!("[Cluster] Discovery already running, skipping");
            return;
        }

        let secret = self.load_discovery_secret();

        // vault fail-closed：token 引用解析失败（load_discovery_secret 或
        // 先前的 load_rpc_auth_token 置位）→ 拒绝无加密发现，宁可不发现。
        // token 为空（rpc_token_missing）同款拒绝——
        // 明文 announce 可被局域网任意主机侦听/伪造，与坏引用同威胁。
        if self.rpc_reference_broken.load(Ordering::SeqCst) {
            tracing::error!(
                "[Cluster] token 引用解析失败 —— fail-closed：UDP discovery 不启动（拒绝无加密运行；请修复引用或运行 `nemesisbot vault set <alias>`）"
            );
            return;
        }
        if self.rpc_token_missing.load(Ordering::SeqCst) {
            tracing::error!(
                "[Cluster] 集群已启用但 token 为空 —— fail-closed：UDP discovery 不启动（拒绝无加密发现；运行 `nemesisbot cluster token generate --save` 后重启）"
            );
            return;
        }

        // 从 config.cluster.json 读取可配置 announce 过期阈值
        // （announce_expiry_secs，≤0 = 关闭过期丢弃）。
        let app_cfg = crate::config_loader::load_app_config(&self.workspace);
        let mut discovery_config = crate::discovery::DiscoveryConfig::with_encryption(
            self.udp_port,
            self.broadcast_interval,
            &secret,
        );
        discovery_config.announce_expiry_secs = app_cfg.announce_expiry_secs;

        // 走到这里 secret 仍可能为空的只剩一条路径：config.cluster.json
        // 整个缺失（load_rpc_auth_token / load_discovery_secret 都早退、两
        // 旗标均未置位）——即集群未配置却手工挂 discovery 的 exotic 接线
        // （测试直驱属此类）。生产路径（gateway / `cluster node`）config
        // 存在且 token 为空时已在上面 token_missing 闸拒绝（
        // 推翻 语义矩阵的「空 token = 明文广播运行」兼容臂：与 RPC
        // fail-closed 对齐，拒绝无加密发现）。
        if secret.is_empty() {
            tracing::warn!(
                "[Cluster] 未配置 discovery token —— UDP 发现以**明文**模式运行 \
                （announce 可被局域网内任意主机侦听/伪造）。生产部署建议在 \
                 config.cluster.json 配置 token（CLI: `nemesisbot cluster token set`）。"
            );
        }

        match crate::discovery::DiscoveryService::new(arc_self, discovery_config) {
            Ok(discovery) => {
                match discovery.start() {
                    Ok(_) => {
                        self.discovery_running.store(true, Ordering::SeqCst);
                        tracing::info!(
                            port = %self.udp_port,
                            encrypted = !secret.is_empty(),
                            "[Cluster] UDP discovery started"
                        );
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "[Cluster] Failed to start UDP discovery");
                    }
                }
                // Store discovery in Cluster for lifecycle management.
                // DiscoveryService holds Arc<dyn ClusterCallbacks> which keeps
                // the Cluster alive via Arc cycle. This cycle is broken when
                // stop_discovery() drops the DiscoveryService.
                *self.discovery.lock() = Some(discovery);
            }
            Err(e) => {
                tracing::error!(error = %e, "[Cluster] Failed to create discovery service");
            }
        }
    }

    /// 解密失败丢弃摘要（token 失配来源账本；discovery 未启动 = None）。
    ///
    /// 异 token 来源互不发现是预期安全行为，日常零日志；
    /// 诊断走此处——`cluster.status` WSAPI 的 `discovery_drops` 字段。
    pub fn discovery_decrypt_drops(&self) -> Option<crate::discovery::DecryptDropSummary> {
        let guard = self.discovery.lock();
        guard.as_ref().map(|d| d.decrypt_drop_summary())
    }

    /// Load the discovery encryption secret from `workspace/config/config.cluster.json`.
    ///
    /// Uses the same `token` field as RPC auth. Returns empty string if no token
    /// is configured, meaning discovery runs without encryption.
    pub(super) fn load_discovery_secret(&self) -> String {
        // 委托 nemesis-path 唯一拼接点。
        let cfg_path = nemesis_path::resolve_cluster_config_path_in_workspace(&self.workspace);
        if !cfg_path.exists() {
            return String::new();
        }

        let raw = match nemesis_utils::fsutil::read_config_string(&cfg_path) {
            Ok(data) => serde_json::from_str::<serde_json::Value>(&data)
                .ok()
                .and_then(|v| v.get("token").and_then(|t| t.as_str()).map(String::from))
                .unwrap_or_default(),
            Err(_) => String::new(),
        };
        if raw.is_empty() {
            return raw;
        }
        // vault：discovery 加密密钥与 RPC token 同字段，同链路解析。
        // 解析失败 = fail-closed：置位 rpc_reference_broken（start_discovery
        // 据此整体不启动），绝不降级为无加密发现。
        match nemesis_config::resolve_secret_field(&raw, "config.cluster.json token (discovery)") {
            Ok(t) => t,
            Err(e) => {
                self.rpc_reference_broken.store(true, Ordering::SeqCst);
                tracing::error!(
                    "[Cluster] discovery 加密密钥引用解析失败: {e} —— fail-closed：发现层不启动（请修复引用或运行 `nemesisbot vault set <alias>`）"
                );
                String::new()
            }
        }
    }
}

impl Cluster {
    /// Get info about a specific node.
    pub fn get_node_info(&self, node_id: &str) -> Option<ExtendedNodeInfo> {
        self.registry.get(node_id)
    }

    /// List all known nodes.
    pub fn list_nodes(&self) -> Vec<ExtendedNodeInfo> {
        self.registry.list_peers()
    }

    /// Register a remote node.
    pub fn register_node(&self, info: ExtendedNodeInfo) {
        self.registry.upsert(info);
    }

    /// Remove a node.
    pub fn remove_node(&self, node_id: &str) -> bool {
        let removed = self.registry.remove(node_id);
        if removed {
            self.removed_peers.write().insert(node_id.to_string());
            crate::logger::log_discovery("removed", "", Some(node_id));
        }
        // Persist deletion to peers.toml so the node does not reappear after
        // restart. `remove_peer_from_file` is idempotent, so calling it even
        // when nothing was removed from the registry is safe — but we always
        // call it to cover the case where the file has a stale entry that the
        // registry already forgot about. The in-memory blacklist above
        // prevents UDP from immediately re-adding the node this session; it
        // clears on restart, allowing re-discovery (matching the "until
        // re-discovered" semantics).
        if let Err(e) =
            crate::cluster_config::remove_peer_from_file(&self.static_config_path, node_id)
        {
            tracing::warn!(
                node_id = node_id,
                error = %e,
                "[Cluster] Failed to remove peer from peers.toml"
            );
        }
        removed
    }

    /// Remove a node from the blacklist, allowing it to be re-discovered.
    pub fn unban_node(&self, node_id: &str) -> bool {
        self.removed_peers.write().remove(node_id)
    }

    /// Register the node-discovered callback (Swarm). Called once at
    /// gateway assembly; the closure receives `(node_id, role, category)` on
    /// every non-blacklisted discovery/refresh event.
    pub fn set_on_node_discovered(&self, cb: Arc<dyn Fn(&str, &str, &str) + Send + Sync>) {
        *self.on_node_discovered.lock() = Some(cb);
    }

    /// 注册恢复交付回调（语义见字段 doc）。gateway 在
    /// board+cluster 构建下装配时注入；未注入 = 恢复腿保持旧行为（无条件
    /// confirm）。
    pub fn set_on_recovered_delivery(
        &self,
        cb: Arc<dyn Fn(&str, &str, &str, Option<&str>) -> bool + Send + Sync>,
    ) {
        *self.on_recovered_delivery.lock() = Some(cb);
    }

    /// Handle a discovered node (from UDP broadcast or manual config).
    /// 是否 iroh 形态地址——**形态判定是纯字符串语义**（`iroh:` 前缀），
    /// 不依赖漫游栈编译：feature 关的节点同样会**收到**漫游节点的 announce
    /// （混合版本农场），按 TCP 口径拼 `:rpc_port` 会把完整地址损坏成
    /// `iroh:<hex>@http://...:50591`（`test_discovered_node_relay_qualified_
    /// overlay_address_pooling` 形态一钉死）。识别后原样收录即可——拨号
    /// 由候选分派裁决，本节点未编译漫游栈时该候选自然失败，与地址损坏
    /// 是两回事。
    fn addr_is_roaming(a: &str) -> bool {
        crate::is_roaming_address(a)
    }

    /// iroh 漫游身份 hex 校验归一（64 位 hex，大小写不敏感，统一小写）。
    /// None = 非法——入池 iroh 候选只会浪费拨号预算（诚实裁剪）。
    fn valid_iroh_id(s: Option<String>) -> Option<String> {
        let s = s?;
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return None;
        }
        match crate::roaming::node_id_from_hex(trimmed) {
            Ok(_) => Some(trimmed.to_lowercase()),
            Err(e) => {
                tracing::debug!(value = %trimmed, error = %e,
                    "[Cluster] Ignoring invalid iroh_id in announce");
                None
            }
        }
    }

    pub fn handle_discovered_node(
        &self,
        node_id: &str,
        name: &str,
        addresses: Vec<String>,
        rpc_port: u16,
        role: &str,
        category: &str,
        tags: Vec<String>,
        capabilities: Vec<String>,
        node_type: &str,
    ) -> bool {
        self.handle_discovered_node_impl(
            node_id,
            name,
            addresses,
            rpc_port,
            role,
            category,
            tags,
            capabilities,
            node_type,
            Vec::new(),
            None,
            Vec::new(),
            // 旧 9 参回调无扩展身份字段 → None = 未宣告（诚实，不猜）。
            None,
            None,
            None,
            None,
            "",
        )
    }

    /// [`Self::handle_discovered_node`] 扩展形态：带对端自报职能、档位与
    /// agent 实体清单（集群专业职能框架 + agent roster）。生产 UDP
    /// 发现路径（announce 接收）走本方法；其余路径（桥接/静态）无扩展
    /// 数据，走旧签名。
    ///
    /// `source_addr`： 源地址收割——UDP announce 的包源 IP（发现层亲眼
    /// 可见的可达性证据；载荷地址可能因对端枚举缺口缺失，如 if_addrs 的
    /// APIPA 缺口）。作为最低优先级拨号候选并入地址池；非 UDP 路径
    /// （TCP 探针身份回传/静态装载）传 `None`。
    ///
    /// `iroh_id`：对端自报 iroh 漫游身份 hex（hex 校验合法且 feature
    /// 编译时才入池——拼 `iroh:<hex>` 作拨号候选，池尾追加不抢直连优先级；
    /// 非法值/未编译 = 忽略，debug 留痕）。
    ///
    /// `mandate_pubkey`：对端自报 mandate 签名公钥（a；P-256 SEC1 hex
    /// 66/130。合法形态且 trust.toml 已激活时 TOFU 首见钉存进
    /// `[mandate_keys]`——验签消费在 -b；无 key 旧节点维持现行行为）。
    #[allow(clippy::too_many_arguments)]
    pub fn handle_discovered_node_ex(
        &self,
        node_id: &str,
        name: &str,
        addresses: Vec<String>,
        rpc_port: u16,
        role: &str,
        category: &str,
        tags: Vec<String>,
        capabilities: Vec<String>,
        node_type: &str,
        professions: Vec<String>,
        tier: Option<String>,
        agents: Vec<crate::types::AgentAnnouncement>,
        iroh_id: Option<String>,
        runtime_backend: Option<String>,
        mandate_pubkey: Option<String>,
        source_addr: Option<&str>,
        build: &str,
    ) -> bool {
        self.handle_discovered_node_impl(
            node_id,
            name,
            addresses,
            rpc_port,
            role,
            category,
            tags,
            capabilities,
            node_type,
            professions,
            tier,
            agents,
            iroh_id,
            runtime_backend,
            mandate_pubkey,
            source_addr,
            build,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_discovered_node_impl(
        &self,
        node_id: &str,
        name: &str,
        addresses: Vec<String>,
        rpc_port: u16,
        role: &str,
        category: &str,
        tags: Vec<String>,
        capabilities: Vec<String>,
        node_type: &str,
        professions: Vec<String>,
        tier: Option<String>,
        agents: Vec<crate::types::AgentAnnouncement>,
        iroh_id: Option<String>,
        runtime_backend: Option<String>,
        mandate_pubkey: Option<String>,
        source_addr: Option<&str>,
        build: &str,
    ) -> bool {
        // 身份字符集闸（同族——畸形 announce 不入册）。
        // 含 `:`/`/` 的 id 会把下游复合键协议（session_key 的 `/` 分隔、
        // `__ASYNC__` marker 的 `:` 分隔、连接池键）切坏——段截断/会话串台。
        // 正常生成形态不落此闸（见 is_valid_node_identity 注释）。
        if !crate::cluster_config::is_valid_node_identity(node_id) {
            tracing::warn!(
                "[Cluster] Ignoring announce with malformed node_id {:?} (charset gate)",
                node_id
            );
            return false;
        }

        // Skip blacklisted nodes
        if self.removed_peers.read().contains(node_id) {
            return false;
        }

        // （三期）：iroh 互信 announce 采信闸——revoked 一票
        // 不采信（整条拒绝：身份/地址池含 iroh 候选全不采，last_seen 不刷新
        // → registry 自然 stale → Offline → 派发回避），unknown TOFU 首见钉
        // 存，簿缺席 = regime 未激活全放行（字节级现状，绝不因 announce 建
        // trust.toml）。携合法 iroh hex 才触发（valid_iroh_id on-curve 校验
        // 同款）：无 key 旧节点维持现行行为。
        if let Some(hex) = Self::valid_iroh_id(iroh_id.clone())
            && crate::roaming::trust::trust_book_path(&self.workspace).exists()
        {
            match crate::roaming::trust::announce_gate(&self.workspace, &hex) {
                crate::roaming::trust::AnnounceVerdict::Adopt => {}
                crate::roaming::trust::AnnounceVerdict::Veto(reason) => {
                    tracing::warn!(
                        "[Cluster] announce 来自已吊销节点（{reason}），整条不采信: {node_id} iroh:{hex}"
                    );
                    return false;
                }
            }
        }

        // a（三期）：mandate 签名公钥 TOFU 首见钉存。合法
        // P-256 SEC1 hex 且 trust.toml 已激活（簿存在）才写簿——announce
        // 抵达绝不激活 trust.toml（纪律镜像），无 key 旧节点维持现行
        // 行为。验签消费在 -b；此处只管分发面钉存。Conflict 保留首见
        // （strict TOFU，轮换唯一通道 = 管理面 mandate_pin_forced）；读/
        // 写失败 warn 不拒 announce（分发面 fail-open，自愈重钉）。
        if let Some(pk) = mandate_pubkey.as_deref()
            && crate::roaming::trust::normalize_mandate_pubkey(pk).is_some()
            && crate::roaming::trust::trust_book_path(&self.workspace).exists()
        {
            match crate::roaming::trust::mandate_tofu_pin(&self.workspace, node_id, pk) {
                Ok(outcome) => match outcome {
                    crate::roaming::trust::MandatePinOutcome::Pinned => {
                        tracing::info!("[Cluster] mandate 公钥 TOFU 首见钉存: {node_id}");
                    }
                    crate::roaming::trust::MandatePinOutcome::Conflict { .. } => {
                        tracing::warn!(
                            "[Cluster] mandate 公钥与既有 pin 冲突，保留首见（轮换走管理面 mandate-pin-forced）: {node_id}"
                        );
                    }
                    _ => {}
                },
                Err(e) => {
                    tracing::warn!(
                        "[Cluster] mandate 公钥钉存失败（不拒 announce）: {node_id}: {e}"
                    );
                }
            }
        }

        // 撞名后缀收敛（goal 钉死，确定性规则无需协商协议）。
        // 对方 announce 的显示名与我本地名相同且 id 不同 → 本地名追加
        // `-` + 自己 node_id 前 4 位（走 set_node_name 唯一写入点，下一拍
        // announce 自然携带新名；对端同理收敛，双方各自算出同一结论）。
        // 规则只作用于**自动名**（hostname / `Bot {id8}`）——config /
        // peers.toml 显式名免疫（用户意志优先，重名无害，唯一性靠 node_id）。
        if !name.is_empty()
            && name == self.node_name()
            && node_id != self.node_id
            && !self
                .node_name_locked
                .load(std::sync::atomic::Ordering::Relaxed)
        {
            let suffixed = format!(
                "{}-{}",
                self.node_name(),
                name_suffix_from_node_id(&self.node_id)
            );
            tracing::info!(
                "[Cluster] 撞名收敛：显示名「{name}」与节点 {node_id} 重名，本地名改为「{suffixed}」"
            );
            self.set_node_name(suffixed);
        }

        // （双机混跑）：rpc_port=0 的 announce 造不出可用
        // 记录（ip:0 永不可达）。正常路径不会出现 0——静态 peers 装载按
        // udp+10000 派生恒 >0，新版节点 announce 恒带真实 rpc_port；0 只
        // 来自异版本/畸形 announce。未知节点不登记（state.toml 不再存
        // unusable 条目），已知节点不覆盖（静态条目原值保留）。异版本
        // 混合集群靠静态 peers 显式登记，不经 UDP 发现。
        if rpc_port == 0 {
            tracing::debug!(
                "[Cluster] Ignoring announce without rpc_port (legacy/malformed): {} at {:?}",
                node_id,
                addresses
            );
            return false;
        }

        // 同族（本机）：addresses 为空时下方构造出的记录
        // address=""（直连不可达形态）。**已知节点**只合并身份字段
        // （professions/tier/capabilities/tags/role/name 照常更新），地址保留
        // 原值——身份传播（探针回传在 UDP 隔离拓扑的 purpose）不得因对端 IP
        // 枚举暂空而把可用地址抹成空（静态 peer 的 127.0.0.1:<rpc> 被
        // 探针身份回传覆盖为 ""，后续 peer_chat 全部
        // `connect to: invalid socket address`）。
        // **未知节点**照常入册：空地址记录是桥接拓扑的合法形态——bridge
        // member_sync 广播的成员本就经 via-bridge 可达而非直连（新到空地址
        // 成员若被拒，桥接设备的成员注册就断了，bridge_rpc/member_sync
        // 契约测试钉此语义）。
        if addresses.is_empty()
            && let Some(existing) = self.registry.get(node_id)
        {
            let mut merged = existing;
            merged.base.name = name.into();
            merged.base.role = nemesis_types::cluster::NodeRole::from_role_str(role);
            merged.base.category = category.into();
            merged.base.last_seen = chrono::Local::now().to_rfc3339();
            merged.status = NodeStatus::Online;
            merged.capabilities = capabilities;
            merged.tags = tags.clone();
            merged.professions = professions.clone();
            merged.tier = tier.clone();
            merged.agents = agents.clone();
            merged.runtime_backend = runtime_backend;
            merged.node_type = node_type.to_string();
            merged.build = build.to_string();
            // iroh 漫游身份入池（新增非覆盖——空地址分支保留原地址的
            // 纪律不变；iroh 候选是新通路证据，照常追加）。
            if let Some(hex) = Self::valid_iroh_id(iroh_id) {
                let cand = format!("iroh:{hex}");
                if !merged.addresses.iter().any(|a| a == &cand) {
                    merged.addresses.push(cand);
                }
            }
            return self.registry.upsert_if_changed(merged);
        }

        // iroh 形态地址自带完整寻址（node_id[+直连 socket]），拼
        // `:rpc_port` 会造出 `iroh:<hex>:<port>` 非法地址——原样作
        // primary。TCP 地址照旧拼 port。
        let primary_address = match addresses.first() {
            Some(a) if Self::addr_is_roaming(a) => a.clone(),
            Some(a) => format!("{}:{}", a, rpc_port),
            None => String::new(),
        };

        let was_known = self.registry.get(node_id).is_some();

        // 静态 pin 合并保护：peers.toml 显式配置的对端地址不因 announce
        // 刷新被逐出（静态配置层与发现池层此前无合并保护——Win↔Win 验收
        // 实测：可达 pin 被对端自报的不可达地址挤掉后派发即断）。池序 =
        // pinned（用户显式配置优先）→ 载荷（announce 自报序）→ 源地址。
        // **pin 前置**（多角度审计批中-2）：拨号预算 = best + 3
        // 个 fallback（rpc/client.rs），载荷地址 ≥4 个且无一同网段时 pin
        // 排尾永远轮不到——「静态语义 = 用户显式配置优先」本就该排在自报
        // 载荷前面（merge_real_node_info 内的静态地址保护注释同口径）。
        // 静态语义同时锁 base.address（与 merge_real_node_info 的静态保护
        // 同源）。
        let mut pool: Vec<String> = Vec::new();
        let mut static_base_address: Option<String> = None;
        if self.registry.is_peer_static(node_id)
            && let Some(existing) = self.registry.get(node_id)
        {
            let mut pinned = existing.addresses.clone();
            // iroh 形态 base.address 是原子（node_id+可选 socket），
            // 按冒号拆 host 会拆出 "iroh" 垃圾候选——iroh 形态整条入 pin。
            let host = if Self::addr_is_roaming(&existing.base.address) {
                existing.base.address.clone()
            } else {
                existing
                    .base
                    .address
                    .split(':')
                    .next()
                    .unwrap_or("")
                    .to_string()
            };
            if !host.is_empty() && !pinned.contains(&host) {
                pinned.push(host);
            }
            for a in pinned {
                if !pool.contains(&a) {
                    pool.push(a);
                }
            }
            if !existing.base.address.is_empty() {
                static_base_address = Some(existing.base.address.clone());
            }
        }
        for a in &addresses {
            if !pool.contains(a) {
                pool.push(a.clone());
            }
        }
        // 源地址收割：UDP 包源 IP 是「发现层亲眼可见」的可达性证据，作
        // 为最低优先级候选入池（池序 pin > 载荷 > 源地址，中-2 pin 前置见
        // 上方静态块）——对端唯一可达地址是 link-local（被对端 if_addrs 缺
        // 口排除在载荷外）时的最后一根自愈稻草。
        if let Some(src) = source_addr
            && !src.is_empty()
            && !pool.iter().any(|a| a.as_str() == src)
        {
            pool.push(src.to_string());
        }
        // 池尾追加：iroh 候选是跨网兜底通路（发现层打洞），不抢
        // 局域网直连优先级——池序 pin > 载荷 > 源地址 > iroh。hex 校验
        // 合法（feature 编译）才入池，杜绝垃圾地址每次拨号白撞。
        if let Some(hex) = Self::valid_iroh_id(iroh_id) {
            let cand = format!("iroh:{hex}");
            if !pool.contains(&cand) {
                pool.push(cand);
            }
        }

        let node = ExtendedNodeInfo {
            base: nemesis_types::cluster::NodeInfo {
                id: node_id.into(),
                name: name.into(),
                // announce 携带对端自报 role（权威=对端 peers.toml [node].role）；
                // 曾硬编码 Worker 导致 board.sync 的 coordinator 判定在纯 UDP
                // 发现拓扑下永远失败（补拉死循环），必须走 from_role_str 解析。
                role: nemesis_types::cluster::NodeRole::from_role_str(role),
                address: static_base_address.unwrap_or(primary_address.clone()),
                category: category.into(),
                last_seen: chrono::Local::now().to_rfc3339(),
            },
            status: NodeStatus::Online,
            capabilities,
            tags: tags.clone(),
            // Preserve all addresses for multi-address failover。
            addresses: pool,
            // clone：下方占位升级块（RealNodeInfo）仍要用 professions/tier/agents。
            professions: professions.clone(),
            tier: tier.clone(),
            agents: agents.clone(),
            runtime_backend: runtime_backend.clone(),
            build: build.to_string(),
            node_type: node_type.to_string(),
        };
        let changed = self.registry.upsert_if_changed(node);

        // If a placeholder peer exists at the same address (i.e. a
        // manually-added entry keyed by name/address instead of the real ID),
        // upgrade it now. The placeholder is removed from both registry and
        // peers.toml, leaving only the canonical real_id entry.
        //
        // Note: find_by_address may return the just-inserted real_id entry
        // itself (since it also matches the address). We loop over all matches
        // via list_peers to find any *other* entry at the same address that
        // isn't the real_id and remove it.
        if !primary_address.is_empty() {
            // Collect placeholder (id, name) pairs. A placeholder is a
            // manually/statically-added entry keyed by the human name or address
            // (e.g. "Node-A") rather than the real node_id. Its `name` field is
            // the human-readable name the operator configured in peers.toml —
            // capture it so we can preserve it across the upgrade.
            //
            // 补臂（单机/回环拓扑根修）：自报地址**永不
            // 含回环**（get_all_local_ips 跳过 loopback），而单机多实例的
            // 静态 peer 配的恰是 127.0.0.1——纯地址比对在此拓扑下永不命中，
            // 占位条目永不升级、与真实条目长期并存（占位 id = 人读名），
            // 进而遮蔽 canonical_peer_id 的解析（registry.get 先命中占位 →
            // 返回占位 id 而非运行时 id， 账本身份失配，UAT 实证）。
            // 补名匹配臂：仅对「id==name」的自键占位条目、且本次 announce
            // 携带非空人读名（≠ node_id）时按名归并。真实条目（id≠name）
            // 不参与名合并——同名异 id 的两个真实节点不会被误并。
            let announce_carries_human_name = !name.is_empty() && name != node_id;
            // 本机自条目永不作占位消费（同机双节点实测缺陷）：
            // 对端自报主地址与本机地址相同（同机多实例同 rpc_port / NAT 共
            // 出口 IP+端口）时，地址比对臂会把 registry 的本机自条目当
            // stale 占位吞掉——自条目被删、peers.toml [peers.{对端}] 被写成
            // 本机 name + UDP 派生端口、对端单播从此打回本机 socket（发现
            // 自环， 探针只能走 overlay）。自条目不参与占位匹配。
            let local_node_id = self.node_id.clone();
            let placeholders: Vec<(String, String, Vec<String>)> = self
                .registry
                .list_peers()
                .into_iter()
                .filter(|p| {
                    if p.base.id == node_id {
                        return false;
                    }
                    if p.base.id == local_node_id {
                        return false;
                    }
                    if announce_carries_human_name
                        && p.base.id == p.base.name
                        && p.base.name == name
                    {
                        return true;
                    }
                    // 地址比对臂只消费「真占位」：静态 peers 手写条目 /
                    // id==name（占位键、保守缺省条目）/ 空 name（无身份
                    // 条目）。运行时学来的真实远端条目（announce 人类名、
                    // id≠name、非静态）不得被地址吞并——同机同 rpc_port /
                    // 共享 NAT 出口拓扑下两个真实节点地址相同，吞错 =
                    // 注册表 flapping + peers.toml 反复改写（
                    // 交付后复查，自条目缺陷的姊妹洞）。
                    if !(self.registry.is_peer_static(&p.base.id)
                        || p.base.id == p.base.name
                        || p.base.name.is_empty())
                    {
                        return false;
                    }
                    // 发现②/ 强化：占位匹配对自报**全量**地址逐个比对，
                    // 不再只对 primary——placeholder（静态 peers 手写条目）
                    // 配的地址可能不是自报 primary（多网卡枚举序决定），
                    // 单地址比对会永不归一、双条目并存。
                    let self_reported: Vec<String> = addresses
                        .iter()
                        .map(|a| {
                            if a.contains(':') {
                                a.clone()
                            } else {
                                format!("{}:{}", a, rpc_port)
                            }
                        })
                        .collect();
                    self_reported.iter().any(|cand| {
                        addr_eq(&p.base.address, cand)
                            || p.addresses.iter().any(|a| addr_eq(a, cand))
                    })
                })
                .map(|p| {
                    // 占位可继承的拨号候选：地址池原样 + base.address 的
                    // host 部分（静态 loader 装载的条目地址池常为空，拨号
                    // 走 base.address 的 host 回退分支——不并入则升级后
                    // 回环候选直接丢失）。
                    let mut addrs = p.addresses.clone();
                    let host = p.base.address.split(':').next().unwrap_or("").to_string();
                    if !host.is_empty() && !addrs.contains(&host) {
                        addrs.push(host);
                    }
                    (p.base.id.clone(), p.base.name.clone(), addrs)
                })
                .collect();
            let mut inherited_name: Option<String> = None;
            let mut inherited_static = false;
            let mut inherited_addresses: Vec<String> = Vec::new();
            for (placeholder_id, placeholder_name, placeholder_addrs) in &placeholders {
                tracing::info!(
                    real_id = node_id,
                    placeholder_id = %placeholder_id,
                    address = %primary_address,
                    "[Cluster] UDP discovery upgrading placeholder peer to real ID"
                );
                // Capture static-ness BEFORE removing the placeholder so the
                // upgraded real entry inherits it (static peers stay exempt
                // from UDP-staleness expiry).
                if self.registry.is_peer_static(placeholder_id) {
                    inherited_static = true;
                }
                self.registry.remove(placeholder_id);
                // Use the placeholder's human name (e.g. "Node-A") for the
                // persisted entry when available — the announce sometimes
                // carries node_id as name, which would survive into peers.toml
                // and break name-based lookups after a reload.
                let effective_name = if !placeholder_name.is_empty() {
                    placeholder_name.clone()
                } else {
                    name.to_string()
                };
                self.upgrade_peer_in_peers_toml(
                    placeholder_id,
                    node_id,
                    &RealNodeInfo {
                        id: node_id.into(),
                        name: effective_name,
                        address: primary_address.clone(),
                        // announce 携带的真实 RPC 端口随升级落盘（显式
                        // rpc_port 字段），不再留给装载端按约定猜。
                        rpc_port,
                        // 发现①根修：升级路径同样保全自报全量地址。
                        // 并入占位侧地址（含回环），落盘的候选池
                        // 才是完整选址集合——单机拓扑下回环是唯一可达地址。
                        addresses: {
                            let mut merged = addresses.clone();
                            for a in placeholder_addrs {
                                if !merged.contains(a) {
                                    merged.push(a.clone());
                                }
                            }
                            merged
                        },
                        // 同 handle_discovered_node：role 走对端自报值解析，
                        // 不硬编码（升级后的静态 peer 条目 role 才真实）。
                        role: nemesis_types::cluster::NodeRole::from_role_str(role),
                        category: category.into(),
                        capabilities: Vec::new(),
                        tags: tags.to_vec(),
                        professions: professions.clone(),
                        tier: tier.clone(),
                        agents: agents.clone(),
                        runtime_backend: runtime_backend.clone(),
                        build: build.to_string(),
                        node_type: node_type.into(),
                    },
                );
                if !placeholder_name.is_empty() {
                    inherited_name = Some(placeholder_name.clone());
                }
                for a in placeholder_addrs {
                    if !inherited_addresses.contains(a) {
                        inherited_addresses.push(a.clone());
                    }
                }
            }
            // Preserve the human-readable name across the upgrade. The real
            // entry was inserted above keyed by node_id; if its `name` ended up
            // empty or just the node_id (announce sometimes carries node_id as
            // name), restore the name inherited from the placeholder so that
            // lookups by human name (e.g. cluster_rpc target "Node-A") still
            // resolve via get_peer_info's name fallback.
            if let Some(human_name) = inherited_name
                && let Some(mut info) = self.registry.get(node_id)
                && (info.base.name.is_empty() || info.base.name == node_id)
            {
                info.base.name = human_name;
                self.registry.upsert(info);
            }
            // 地址池继承。占位条目（静态 peers）配的往往是 operator
            // 手写的可达地址（单机拓扑 = 127.0.0.1），而 announce 自报地址
            // 永不含回环——不继承则升级后的真实条目只剩网卡 IP 候选，单机
            // /防火墙拓扑下 RpcClient 选址全部不可达（send_and_receive 的
            // 多地址 failover 也无从兜底，候选池里根本没有回环）。占位侧
            // 地址去重并入真实条目候选池。
            if !inherited_addresses.is_empty()
                && let Some(mut info) = self.registry.get(node_id)
            {
                let mut changed_addr = false;
                for a in &inherited_addresses {
                    if !info.addresses.contains(a) {
                        info.addresses.push(a.clone());
                        changed_addr = true;
                    }
                }
                if changed_addr {
                    self.registry.upsert(info);
                }
            }
            // Preserve static-ness across the upgrade so a configured peer
            // (loaded from peers.toml) doesn't suddenly become UDP-expirable
            // once its placeholder key is replaced by the real node_id.
            if inherited_static {
                self.registry.mark_static(node_id);
            }
        }

        if !changed && was_known {
            tracing::trace!(
                node_id = node_id,
                "[Cluster] Node unchanged, health refreshed"
            );
        } else if was_known {
            logger::log_discovery("updated", &primary_address, Some(node_id));
        } else {
            logger::log_discovery("discovered", &primary_address, Some(node_id));
            tracing::info!(
                node_id = node_id,
                name = name,
                addr = %primary_address,
                category = category,
                "[Cluster] Node discovered: id={}, addr={}",
                node_id,
                primary_address,
            );
        }

        // Swarm: notify the discovery callback (gateway folds the node into
        // board channels; first-join semantics are the closure's job — cluster
        // does not depend on board).
        if let Some(cb) = self.on_node_discovered.lock().clone() {
            cb(node_id, role, category);
        }
        changed
    }
}

impl Cluster {
    /// Merge real node info obtained from RPC `get_info` or UDP AnnounceMessage.
    ///
    /// When a node is manually added (via `nodes.add` or cluster CLI) it is
    /// stored under a placeholder peer_id (user-supplied ID, or the node name,
    /// or the address). The real node ID is only learned when the remote comes
    /// online and we either:
    ///   -: actively call `get_info` RPC, or
    ///   -: passively receive an AnnounceMessage via UDP discovery.
    ///
    /// This function performs the merge:
    ///   1. If an entry with the real_id already exists in the registry,
    ///      update its fields (name, role, address, category, capabilities,
    ///      node_type) and refresh `last_seen`.
    ///   2. Otherwise, search for a placeholder by address. If found, remove
    ///      the placeholder and insert a new entry keyed by real_id.
    ///   3. Otherwise, insert a brand new entry under real_id.
    ///
    /// The peers.toml file is updated to reflect the merge: the placeholder
    /// subtable `[peers.{placeholder}]` is removed and a new subtable
    /// `[peers.{real_id}]` is added (or the existing one updated).
    ///
    /// `status` is NOT overwritten from the incoming data: it is a local
    /// observation (we may have just failed a health check). `addresses` is
    /// refreshed only when the payload carries a non-empty list (发现①：保全
    /// 全量选址候选池；空列表不清空已有集合——旧数据不砸新数据).
    /// The caller can separately mark the node online if warranted.
    ///
    /// Returns the canonical node_id that was written (i.e. `real_id`).
    pub fn merge_real_node_info(&self, info: &RealNodeInfo) -> String {
        // 1. Existing entry with real_id → update fields
        if let Some(mut existing) = self.registry.get(&info.id) {
            existing.base.name = info.name.clone();
            existing.base.role = info.role;
            existing.base.category = info.category.clone();
            // 静态 peer（peers.toml 显式配置）的地址不被对端自报覆盖：
            // 对端自报的 primary 是它 addresses[0]（网卡优先级决定），多网卡
            // 跨网段环境下对本地常不可达（真机三节点实证：master primary=
            // 10.103.x，worker 按它拨号必败 → 探针失败 → Offline →
            // online 门禁拒 transfer）。静态语义=用户显式配置优先，地址
            // 与 mark_static 的免过期保护同源；动态发现节点仍照常更新。
            if !info.address.is_empty() && !self.registry.is_peer_static(&info.id) {
                existing.base.address = info.address.clone();
            }
            // 发现①根修：全量自报数组非空才刷新（primary 保护同上——静态
            // 语义只锁用户显式配置的 base.address；数组是选址候选池，刷新
            // 让 select_best_address 在多网卡下始终拿到最新的可达集合）。
            // 静态 pin 合并保护：静态对端的候选池走**合并**而非整组替换
            // ——自报数组不含用户 pin 的可达地址（枚举缺口/网卡优先级）时，
            // 整组替换会把 pin 逐出（与 handle_discovered_node_impl 同纪律）。
            if !info.addresses.is_empty() {
                if self.registry.is_peer_static(&info.id) {
                    for a in &info.addresses {
                        if !existing.addresses.contains(a) {
                            existing.addresses.push(a.clone());
                        }
                    }
                } else {
                    existing.addresses = info.addresses.clone();
                }
            }
            existing.base.last_seen = chrono::Local::now().to_rfc3339();
            existing.capabilities = info.capabilities.clone();
            existing.tags = info.tags.clone();
            existing.professions = info.professions.clone();
            existing.tier = info.tier.clone();
            existing.agents = info.agents.clone();
            existing.runtime_backend = info.runtime_backend.clone();
            existing.build = info.build.clone();
            existing.node_type = info.node_type.clone();
            let static_pinned_address = if self.registry.is_peer_static(&info.id) {
                Some(existing.base.address.clone())
            } else {
                None
            };
            self.registry.upsert(existing);
            // 盘面臂（横扫姊妹洞）：静态对端的 persist 地址与
            // 内存同纪律——本函数唯一生产调用方（web nodes.ping）每次都会走
            // 到这里，若用入参 info.address 写盘，peers.toml 手写的 pin 地址
            // 被 announce 自报 primary 覆盖（append 路径对已存在键=有意覆
            // 盖），重启后静态语义丢失——内存 2111 保住的 pin 只活到下次重
            // 启。动态条目照旧随自报刷新（语义不动）。
            match static_pinned_address {
                Some(addr) if !addr.is_empty() => {
                    let mut pinned = info.clone();
                    pinned.address = addr;
                    self.persist_real_peer_to_toml(&info.id, &pinned);
                }
                _ => self.persist_real_peer_to_toml(&info.id, info),
            }
            return info.id.clone();
        }

        // 2. Placeholder by address → remove + insert under real_id.
        // 发现②/ 强化：占位匹配不再只比对自报 primary——对端多网卡下
        // primary 可能不是用户静态配置的那个地址（真机实证：placeholder
        // 配 192.168.137.x 而自报 primary=10.103.x → 永不归一，双条目
        // 并存）。改为对自报全量地址逐个匹配，任一命中即归一。
        // 本机自条目豁免（同 announce 漏斗纪律）：对端自报地址与本机地址
        // 相同时不得把自条目当占位消费。地址匹配同样只消费「真占位」
        // （静态 / id==name / 空 name）——运行时学来的真实远端条目不得被
        // 地址吞并（同机同 rpc_port / 共享 NAT 出口拓扑防 flapping）。
        let local_node_id = self.node_id.clone();
        let placeholder_id = self
            .registry
            .find_by_address(&info.address)
            .filter(|p| {
                (self.registry.is_peer_static(&p.base.id)
                    || p.base.id == p.base.name
                    || p.base.name.is_empty())
                    && p.base.id != local_node_id
            })
            .map(|p| p.base.id.clone())
            .or_else(|| {
                info.addresses
                    .iter()
                    .filter_map(|a| {
                        let candidate = if a.contains(':') {
                            a.clone()
                        } else {
                            let port = info.address.rsplit_once(':').map(|(_, p)| p.to_string());
                            match port {
                                Some(p) => format!("{}:{}", a, p),
                                None => a.clone(),
                            }
                        };
                        self.registry
                            .find_by_address(&candidate)
                            .filter(|p| {
                                (self.registry.is_peer_static(&p.base.id)
                                    || p.base.id == p.base.name
                                    || p.base.name.is_empty())
                                    && p.base.id != local_node_id
                            })
                            .map(|p| p.base.id.clone())
                    })
                    .next()
            });

        let node = ExtendedNodeInfo {
            base: nemesis_types::cluster::NodeInfo {
                id: info.id.clone(),
                name: info.name.clone(),
                role: info.role,
                address: info.address.clone(),
                category: info.category.clone(),
                last_seen: chrono::Local::now().to_rfc3339(),
            },
            status: NodeStatus::Online,
            capabilities: info.capabilities.clone(),
            tags: info.tags.clone(),
            addresses: info.addresses.clone(),
            professions: info.professions.clone(),
            tier: info.tier.clone(),
            agents: info.agents.clone(),
            runtime_backend: info.runtime_backend.clone(),
            build: info.build.clone(),
            node_type: info.node_type.clone(),
        };
        self.registry.upsert(node);

        if let Some(placeholder) = placeholder_id {
            if placeholder != info.id {
                tracing::info!(
                    real_id = %info.id,
                    placeholder_id = %placeholder,
                    address = %info.address,
                    "[Cluster] Upgrading placeholder peer to real ID"
                );
                self.registry.remove(&placeholder);
                self.upgrade_peer_in_peers_toml(&placeholder, &info.id, info);
            } else {
                self.persist_real_peer_to_toml(&info.id, info);
            }
        } else {
            // 3. Brand new entry — persist under real_id
            self.persist_real_peer_to_toml(&info.id, info);
        }

        info.id.clone()
    }

    /// Convert an RPC address (`host:rpc_port`) to the UDP address
    /// (`host:udp_port`) for peers.toml write-back. 委托
    /// [`crate::cluster_config:rpc_to_udp_address`]（统一定义，pair 配对
    /// 写盘同源消费）。
    pub(super) fn rpc_to_udp_address(rpc_addr: &str) -> String {
        crate::cluster_config::rpc_to_udp_address(rpc_addr)
    }

    /// Persist the real peer info to peers.toml under `[peers.{real_id}]`.
    fn persist_real_peer_to_toml(&self, real_id: &str, info: &RealNodeInfo) {
        let path = &self.static_config_path;
        let role_str = info.role.as_role_str();
        // peers.toml's `address` field is the UDP host:port — the static loader
        // (gateway.rs) derives rpc_port = udp_port + 10000 from it. info.address
        // is the RPC address (host:rpc_port) used in-memory; convert it back to
        // the UDP address on write so the derivation isn't double-applied on
        // reload (otherwise 21949 → 31949, breaking post-restart callbacks —
        // root cause of cluster-uat).
        let toml_address = Self::rpc_to_udp_address(&info.address);
        if let Err(e) = crate::cluster_config::append_peer_to_file_with_name(
            path,
            real_id,
            &toml_address,
            role_str,
            &info.category,
            Some(&info.name),
            info.rpc_port,
        ) {
            tracing::warn!(
                real_id = real_id,
                error = %e,
                "[Cluster] Failed to persist real peer info to peers.toml"
            );
        }
    }

    /// Remove `[peers.{placeholder}]` and add `[peers.{real_id}]` in peers.toml.
    ///
    /// If both keys sanitize to the same value (i.e. the user already added the
    /// peer with the real ID), this is a no-op aside from a content refresh via
    /// `persist_real_peer_to_toml`. Otherwise the placeholder is stripped AND
    /// the real entry inserted into the same in-memory doc, then persisted with
    /// **one** atomic write（TOCTOU 根修：两段写之间的「两头皆无」窗口是
    /// 并发读盘方假红的根因——见函数尾注）。条目形态走
    /// [`crate::cluster_config:build_peer_entry_table`] 与追加路径同源。
    pub(super) fn upgrade_peer_in_peers_toml(
        &self,
        placeholder: &str,
        real_id: &str,
        info: &RealNodeInfo,
    ) {
        let path = &self.static_config_path;
        // Same key after sanitization → just write the real_id content.
        if crate::cluster_config::sanitize_peer_key(placeholder)
            == crate::cluster_config::sanitize_peer_key(real_id)
        {
            self.persist_real_peer_to_toml(real_id, info);
            return;
        }

        // Load → strip placeholder → atomic write → re-add real_id.
        let mut doc: toml::Value = match (|| -> Result<toml::Value, String> {
            if !path.exists() {
                return Ok(toml::Value::Table(toml::value::Table::new()));
            }
            let content =
                std::fs::read_to_string(path).map_err(|e| format!("read peers.toml: {}", e))?;
            content
                .parse::<toml::Value>()
                .or_else(|_| Ok(toml::Value::Table(toml::value::Table::new())))
        })() {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "[Cluster] Skipping peers.toml upgrade (load failed)");
                return;
            }
        };

        let table = match doc.as_table_mut() {
            Some(t) => t,
            None => return,
        };

        let peers_table = match table.get_mut("peers").and_then(|v| v.as_table_mut()) {
            Some(t) => t,
            None => {
                // No [peers] section yet → just write the real entry via append.
                self.persist_real_peer_to_toml(real_id, info);
                return;
            }
        };

        // 发现②/：优先按字面 placeholder 键删（新写盘路径是字面键）；
        // 找不到再试 sanitize 旧键（旧版代码落盘的有损键存量条目）。
        let placeholder_key = placeholder.to_string();
        let removed = peers_table.remove(&placeholder_key).or_else(|| {
            let legacy = crate::cluster_config::sanitize_peer_key(placeholder);
            if legacy == placeholder_key {
                return None;
            }
            peers_table.remove(&legacy)
        });
        if removed.is_some() {
            tracing::info!(
                placeholder_key = %placeholder_key,
                real_id = real_id,
                "[Cluster] Removed placeholder from peers.toml"
            );
        }

        // TOCTOU 根修：真实条目**并入同一份 doc 后单次原子
        // 写**。旧两段式（先删占位写一次，再追加真实条目又写一次）在两次写
        // 之间留出「既无占位也无真实条目」的窗口—— 探针身份化（probe-identity
        // 改造）后升级路径触发频率大增，并发读盘的 检查恰好落窗即假红。
        // 四审 #6 同族：占位条目（静态 peers 手写）的扩展字段（professions/
        // tier/tags/…）随键迁移**搬运**到新键——removed 旧 table 的非五字段
        // 并入新 entry，operator 手写职能/档位不因升级丢失。
        let mut real_entry = crate::cluster_config::build_peer_entry_table(
            &Self::rpc_to_udp_address(&info.address),
            info.role.as_role_str(),
            &info.category,
            Some(&info.name),
            info.rpc_port,
        );
        if let Some(prev) = removed.as_ref().and_then(|v| v.as_table()) {
            crate::cluster_config::merge_peer_entry_extras(&mut real_entry, prev);
        }
        peers_table.insert(real_id.to_string(), toml::Value::Table(real_entry));

        // Single atomic write of placeholder-strip + real-entry insert.
        let toml_str = match toml::to_string_pretty(&doc) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "[Cluster] Failed to serialize peers.toml after upgrade");
                return;
            }
        };
        if let Err(e) = write_atomic(path, toml_str.as_bytes()) {
            tracing::warn!(error = %e, "[Cluster] Failed to write peers.toml after upgrade");
        }
    }

    // -- Task management ------------------------------------------------------
}

impl Cluster {
    /// Get a peer by ID.
    pub fn get_peer(&self, peer_id: &str) -> Option<ExtendedNodeInfo> {
        self.registry.get(peer_id)
    }

    /// 把调用方给的 target（peer 名或节点 id，人读/机读两种形态都收）归一化
    /// 为注册表权威节点 id（`base.id`，即 worker 侧 `_rpc.from` 的形态）。
    ///
    /// 单一真相源纪律（双身份失配第三处落点）：派发账本
    /// `issue_dispatch.worker_id` 与 worker 上报的传输层身份必须是同一形态
    /// ——matcher 产出本来就是节点 id，人工指派/兜底路径可能给 peer 名
    /// （如 "Alex"），不归一化则 `task.started`/`delivery.files` 的
    /// worker 校验永远失配（名字 ≠ 节点 id）。解析失败（未知 target）返回
    /// None，调用方保留原值（RPC resolver 自身还有 name/id 兜底扫描）。
    pub fn canonical_peer_id(&self, target: &str) -> Option<String> {
        if let Some(info) = self.registry.get(target) {
            // 占位遮蔽防护：get 命中的是「id==name」自键
            // 占位条目（静态 peers 以人读名键控）时，注册表里可能同时存在
            // 同一节点的真实条目（announce 已到、占位按地址升级未命中的
            // 窗口，单机/回环拓扑下该窗口是常态）——此时必须返回真实运行
            // 时 id（worker 侧 `_rpc.from` 的形态），否则派发账本 worker_id
            // 落占位 id、写回评论作者身份失配（统一定义被占位击穿，
            // UAT 实证：作者落 "Node-B" 而非 node-* 运行时 id）。扫描
            // 按 name 精确匹配、排除占位自身；无真实条目 = 维持占位 id
            //（旧行为，纯静态拓扑下这是唯一可用句柄）。
            if info.base.id == info.base.name
                && let Some(real) = self
                    .registry
                    .list_peers()
                    .into_iter()
                    .find(|p| p.base.id != target && p.base.name == target)
            {
                return Some(real.base.id);
            }
            return Some(info.base.id);
        }
        self.registry
            .list_peers()
            .into_iter()
            .find(|p| p.base.id == target || p.base.name == target)
            .map(|p| p.base.id)
            .or_else(|| {
                // 大小写不敏感回落（派发目标校验前置——人读
                // 名手工输入大小写失配是常态，文档承诺「大小写不敏感」；
                // 精确未命中再扫一轮 ASCII 折叠，歧义（两节点仅大小写异）
                // 取先匹配，实际拓扑不存在。
                self.registry
                    .list_peers()
                    .into_iter()
                    .find(|p| {
                        p.base.id.eq_ignore_ascii_case(target)
                            || p.base.name.eq_ignore_ascii_case(target)
                    })
                    .map(|p| p.base.id)
            })
    }

    /// RPC 学到的未知对端登记（真实节点 id 首次出现、UDP announce 尚未
    /// 到达时的身份补齐入口）。
    ///
    /// 根修：此前 gateway 在 peer_chat handler 里以硬
    /// 编码缺省值（name=id、addresses=["127.0.0.1"]、role="worker"、
    /// category="general"、rpc_port fallback 21949）直接调
    /// handle_discovered_node。RPC 天然比 announce 先到（广播周期
    /// broadcast_interval + 发送线程 0-5s jitter，TCP 恒抢先）：垃圾条目
    /// 触发地址匹配占位升级臂，把占位条目 operator 配置的
    /// role=coordinator/category/udp 地址语义整体覆盖（inherited_name 只
    /// 救回 name），并随 upgrade_peer_in_peers_toml 落盘 peers.toml 永久
    /// 固化——announce 的后续修正只进 registry + state.toml
    /// （sync_to_disk 不写 peers.toml），节点每次重启装载 peers.toml 垃
    /// 圾复活，worker_sync_once 找不到 coordinator（UAT 实证：B 视
    /// 角 A 条目 = worker/general/127.0.0.1，而 A 实为
    /// coordinator/development）。
    ///
    /// 正确姿势：优先继承同地址占位（静态 peers 人读名条目）的完整身份
    /// ——role/category/name/地址池原样、rpc_port 用 hint 或占位端口派
    /// 生——再走 handle_discovered_node 正常升级路径。继承不到时才退保
    /// 守缺省（与旧行为一致；纯 UDP 集群无静态配置，announce 数秒内修
    /// 正）。返回 true = 本次实际登记/升级了条目。
    pub fn register_rpc_peer(&self, real_id: &str, rpc_port_hint: u16) -> bool {
        if self.registry.get(real_id).is_some() {
            return false;
        }

        // 自键占位（id==name，静态 peers 的人读名条目）。真实条目（id≠
        // name）不参与继承——那是 announce 学来的运行时身份，不是
        // operator 配置的原始语义。static 标记是占位资格的硬条件：保守
        // 缺省插入的条目（name=id，无 static 标记）不得被当占位继承，
        // 否则垃圾缺省身份会自我扩散。
        let placeholders: Vec<crate::types::ExtendedNodeInfo> = self
            .registry
            .list_peers()
            .into_iter()
            .filter(|p| {
                p.base.id == p.base.name
                    && p.base.id != real_id
                    && self.registry.is_peer_static(&p.base.id)
            })
            .collect();

        // 端口提示匹配：占位地址的端口与 hint 同值（占位地址已被静态
        // loader 转成 rpc 形态）或相差 10000（占位地址仍是 udp 形态）都
        // 算命中。
        let port_of = |addr: &str| -> Option<u16> {
            addr.rsplit(':').next().and_then(|p| p.parse::<u16>().ok())
        };
        let matched = if rpc_port_hint > 0 {
            placeholders.iter().find(|p| {
                port_of(&p.base.address)
                    .map(|port| {
                        port == rpc_port_hint || Some(port) == rpc_port_hint.checked_sub(10000)
                    })
                    .unwrap_or(false)
            })
        } else {
            None
        };
        // 无 hint 时的兜底：恰有一个占位才继承（单机/双节点拓扑几乎必
        // 然正确）；多占位无法判定身份，宁可等待 announce，不猜。
        let chosen = matched.or_else(|| {
            if placeholders.len() == 1 {
                placeholders.first()
            } else {
                None
            }
        });

        let Some(ph) = chosen else {
            // 无占位可继承：保守登记（旧行为，仅 hint 可用时）。announce
            // 到达后由 upsert_if_changed 修正为真实自报身份。 纪律：
            // rpc_port 未知（hint=0）登记不可达条目不如不登记。
            if rpc_port_hint == 0 {
                return false;
            }
            return self.handle_discovered_node(
                real_id,
                real_id,
                vec!["127.0.0.1".to_string()],
                rpc_port_hint,
                "worker",
                "general",
                vec![],
                vec![],
                "unknown",
            );
        };

        // 继承占位身份升级。role 走占位的真实角色（死因 = 旧代码在
        // 此硬编码 "worker"，覆盖了 coordinator）；rpc_port：hint 优先，
        // 占位端口已是 rpc 形态（>10000）直接用，udp 形态按 +10000 约定
        // 派生（与静态 loader 同源）。
        let udp_port = port_of(&ph.base.address).unwrap_or(0);
        let rpc_port = if rpc_port_hint > 0 {
            rpc_port_hint
        } else if udp_port > 10000 {
            udp_port
        } else if udp_port > 0 {
            udp_port + 10000
        } else {
            0
        };
        // 占位 base.address 的 host 并入地址池（单机拓扑回环往往是唯一
        // 可达地址，见 handle_discovered_node 升级臂 注记）。
        let host = ph.base.address.split(':').next().unwrap_or("").to_string();
        let mut addresses = ph.addresses.clone();
        if !host.is_empty() && !addresses.contains(&host) {
            addresses.push(host);
        }
        let name = if ph.base.name.is_empty() {
            real_id.to_string()
        } else {
            ph.base.name.clone()
        };
        let role_str = ph.base.role.as_role_str();
        let category = ph.base.category.clone();
        let changed = self.handle_discovered_node(
            real_id,
            &name,
            addresses,
            rpc_port,
            role_str,
            &category,
            ph.tags.clone(),
            vec![],
            &ph.node_type,
        );
        if changed {
            tracing::info!(
                real_id = real_id,
                placeholder = %ph.base.id,
                role = role_str,
                category = %category,
                rpc_port,
                "[Cluster] RPC peer registration inherited placeholder identity"
            );
        }
        changed
    }
}

/// Real node info obtained from RPC `get_info` or UDP AnnounceMessage.
///
/// Carries the authoritative identity of a remote node. Used by
/// `Cluster::merge_real_node_info` to upgrade placeholder peer entries
/// (created by manual `nodes.add`) to the remote's real ID, and to refresh
/// fields whenever the remote broadcasts a new state.
///
/// `addresses` 是对端自报的**全量**地址列表（网卡枚举序）。发现①根修
/// （真机三节点）：此前 RPC merge 路径建条目时丢弃该列表
/// （`Vec::new()`），`get_peer_info` 回落单地址后 `select_best_address`
/// 的 `len<=1` 短路使子网匹配智能选址从未运行——多网卡环境下对端只能
/// 拿 primary（枚举运气）盲拨不可达网段。保全全量后多地址 failover
/// 与子网优选恢复工作。
#[derive(Debug, Clone)]
pub struct RealNodeInfo {
    pub id: String,
    pub name: String,
    pub address: String,
    /// 对端真实 RPC 端口（announce 携带）。>0 时随占位升级显式落盘
    /// peers.toml（`rpc_port` 字段），不再依赖 `udp+10000` 约定推导。
    pub rpc_port: u16,
    /// 全量自报地址（host 形态，无端口）；空 = 来源未携带（旧行为回落单地址）。
    pub addresses: Vec<String>,
    pub role: nemesis_types::cluster::NodeRole,
    pub category: String,
    pub capabilities: Vec<String>,
    pub tags: Vec<String>,
    /// 对端自报职能（集群专业职能框架；get_info/announce 携带）。
    pub professions: Vec<String>,
    /// 对端自报档位（role_tier 口径；None = 未宣告）。
    pub tier: Option<String>,
    /// 对端自报 agent 实体清单（get_info 携带；merge 进
    /// 注册表 ExtendedNodeInfo）。
    pub agents: Vec<crate::types::AgentAnnouncement>,
    /// 对端自报运行时后端形态（get_info/announce 携带；
    /// None = 未宣告）。本批只宣告不做 matcher 闸（上位）。
    pub runtime_backend: Option<String>,
    /// 对端自报构建号（运行时硬化批；announce/探针/get_info 携带；
    /// merge 进注册表 ExtendedNodeInfo）。不落 peers.toml（写盘手选字段，
    /// 下次 announce 自会重学）。
    pub build: String,
    pub node_type: String,
}

/// Address comparison used by UDP-triggered placeholder upgrade.
///
/// Returns true if `cand` and `needle` resolve to the same host[:port],
/// case-insensitive on host, exact on port (with missing-port treated as
/// wildcard). Defined here in addition to `registry::addr_matches` because
/// the latter is private to the registry module.
pub(super) fn addr_eq(cand: &str, needle: &str) -> bool {
    let cand_lc = cand.trim().to_lowercase();
    let needle_lc = needle.trim().to_lowercase();
    if cand_lc.is_empty() || needle_lc.is_empty() {
        return false;
    }
    let (ch, cp) = match cand_lc.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && !h.is_empty() => (h, Some(p)),
        _ => (cand_lc.as_str(), None),
    };
    let (nh, np) = match needle_lc.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && !h.is_empty() => (h, Some(p)),
        _ => (needle_lc.as_str(), None),
    };
    // 严格语义：host 必须相等，port 必须匹配（都存在且相等，或都不存在）。
    // 不再容忍"一边带 port 一边不带"——这是 placeholder filter 误删同 host
    // 不同 port peer 的根因（cluster-uat 历史 bug：addresses 字段是 host-only
    // 列表，跟 host:rpc_port 比较时宽松规则会判定相等，导致后续加入的 peer
    // 把前面同 host 的 placeholder 当重复项删掉）。
    ch == nh && cp == np
}
