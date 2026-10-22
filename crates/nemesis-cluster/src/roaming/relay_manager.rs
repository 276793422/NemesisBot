//! QUIC 中继生命周期管理（2026-10-23 与集群解耦）。
//!
//! [`QuicRelayManager`] 持有中继服务器句柄槽，宿主（nemesisbot gateway /
//! 独立 `quic relay` 进程）按自身生命周期装配与关停——中继不再寄生于
//! `Cluster`：集群未启用/已关闭时中继照起，中继启停不触碰集群。
//!
//! 配置单一真相源 = `config.quic.json` `relay` 段（[`super::quic_config`]）；
//! 停机语义与原 `Cluster` 内联实现一一对应（幂等 + 并发双跑后到者关停；
//! 优雅 spawn 停机——estop 不冻结中继，中继是传输设施不是 agent 活动面）。

use parking_lot::Mutex;

/// QUIC 中继服务器生命周期管理（feature=quic-relay）。
pub struct QuicRelayManager {
    workspace: std::path::PathBuf,
    relay_server: Mutex<Option<super::relay_server::RelayServerHandle>>,
}

impl QuicRelayManager {
    pub fn new(workspace: impl Into<std::path::PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
            relay_server: Mutex::new(None),
        }
    }

    /// 中继服务器随 gateway 进程内启动。
    ///
    /// 闸：config.quic.json `relay.enabled`（独立命令 `nemesisbot quic relay`
    /// 不受此闸约束——CLI 即显式意志）。不要求集群启用（VPS 形态：只当中继
    /// 服务器不开集群是合法组合）。幂等：已启动直接 Ok(true)。启动失败
    ///（端口占用/证书非法）loud 报错，调用方 warn 不炸宿主——中继是叠加层
    /// 设施。
    ///
    /// fail-closed（2026-10-21 三路收口）：`relay.token` 空 = 拒绝启动——
    /// 跳板无鉴权裸奔 = 公网任何人可白蹭带宽/转自方流量 + 已知 node ID 的
    /// 存在性探测面。与独立入口 `quic relay` / `gateway --quic-relay`
    ///（`run_relay_server` 顶部闸）同一纪律。
    ///
    /// 返回 Ok(true)=已启动（含幂等命中）；Ok(false)=开关未开；
    /// Err=装配失败（含 token 空 fail-closed）。
    pub async fn setup(&self) -> Result<bool, String> {
        let cfg_path = nemesis_path::resolve_quic_config_path_in_workspace(&self.workspace);
        let quic_cfg = super::quic_config::resolved_quic(&cfg_path);
        if !quic_cfg.relay.enabled {
            tracing::debug!(
                "[QuicRelay] Self-hosted relay not enabled (config.quic.json relay.enabled is not true)"
            );
            return Ok(false);
        }
        if quic_cfg.relay.token.trim().is_empty() {
            return Err(
                "config.quic.json relay.token 为空：无鉴权中继拒绝启动（fail-closed）。请在 \
                 config.quic.json relay.token 配置预共享令牌（客户端侧 client.relay_token \
                 需配对同值）。"
                    .to_string(),
            );
        }
        if self.relay_server.lock().is_some() {
            return Ok(true);
        }
        let handle =
            super::relay_server::spawn_relay_server(&quic_cfg.relay, &self.workspace).await?;
        tracing::info!(
            http = ?handle.http_addr,
            quic = ?handle.quic_addr,
            url = %super::relay_server::advertised_relay_url(&quic_cfg.relay),
            "[QuicRelay] Self-hosted relay server started"
        );
        let mut dup = None;
        {
            let mut slot = self.relay_server.lock();
            if slot.is_some() {
                dup = Some(handle);
            } else {
                *slot = Some(handle);
            }
        } // 守卫作用域收口——不跨 await。
        if let Some(mut d) = dup {
            // 并发双跑（理论态）：先到者保留，后到者关停。
            d.shutdown().await;
        }
        Ok(true)
    }

    /// 运行时停自建中继（WSAPI `quic.relay.disable` 实时开关臂；幂等——句柄
    /// 槽取空即无操作）。优雅 spawn 停机（estop 不冻结中继——中继是传输
    /// 设施不是 agent 活动面）。
    pub fn stop(&self) {
        if let Some(mut relay) = self.relay_server.lock().take()
            && let Ok(h) = tokio::runtime::Handle::try_current()
        {
            tracing::info!("[QuicRelay] Self-hosted relay server stopping (runtime disable)");
            h.spawn(async move { relay.shutdown().await });
        }
    }

    /// 中继运行状态（WSAPI/状态面用）：(运行中, http_addr, quic_addr)。
    pub fn status(
        &self,
    ) -> (
        bool,
        Option<std::net::SocketAddr>,
        Option<std::net::SocketAddr>,
    ) {
        match self.relay_server.lock().as_ref() {
            Some(h) => (h.running(), h.http_addr, h.quic_addr),
            None => (false, None, None),
        }
    }

    /// 服务端全量名册快照（WSAPI `quic.registry.members` 数据源——云端
    /// 全量视图；客户端集群视图经 features 严格过滤另行形成，两视图分工
    /// 见 rendezvous 模块文档）。中继未运行或注册表未拉起（registry_port=0）
    /// = 空 vec。
    pub fn registry_members(&self) -> Vec<crate::roaming::rendezvous::RendezvousMember> {
        self.relay_server
            .lock()
            .as_ref()
            .and_then(|h| h.registry.as_ref())
            .map(|reg| reg.live_members())
            .unwrap_or_default()
    }
}

/// QUIC 中继 enabled 快速判定（纯函数，不装配）：config.quic.json
/// `relay.enabled` 严格为 JSON true 才开。缺文件 / 坏 JSON / 缺段 /
/// false → false。实现委托 `quic_config::resolved_quic`（单一真相源——
/// gateway 装配闸与 WSAPI/CLI 状态面同读此形）。
pub fn relay_enabled_in_config(quic_config_path: &std::path::Path) -> bool {
    super::quic_config::resolved_quic(quic_config_path)
        .relay
        .enabled
}
