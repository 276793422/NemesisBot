// ---------------------------------------------------------------------------
// 机械拆分：本文件内容由 cluster.rs 原样搬入（纯代码搬家，行为零变更）。
// ---------------------------------------------------------------------------

use super::*;

impl Cluster {
    /// Spawn the recovery loop as a background tokio task.
    ///
    /// Runs every 2 minutes, polling B-nodes for stale pending tasks whose
    /// results may have been lost (e.g. callback failure). Uses the real
    /// RPC client when available, falling back to the test override.
    pub(super) fn start_recovery_loop(&self) {
        // Only spawn if we're inside a tokio runtime
        let handle = match tokio::runtime::Handle::try_current() {
            Ok(h) => h,
            Err(_) => return, // No runtime available (e.g. in unit tests)
        };

        let mut stop_rx = self.stop_tx.subscribe();
        let task_manager = self.task_manager.clone();
        let call_fn = self.call_with_context_fn.lock().clone();
        let rpc_client = self.rpc_client.lock().clone();
        let bus = self.bus.clone();
        // 恢复交付回调槽与 bus 同款——每 tick 快照读取，兼容 set 在
        // start() 之后的装配顺序。
        let on_recovered_delivery = self.on_recovered_delivery.clone();
        let safety_net = self.task_manager.pending_timeout();

        handle.spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(120));
            // interval 默认 Burst——进程挂起（VM pause/容器
            // 冻结）恢复后对每个错过的 tick 立即补发一轮恢复查询，对端被 RPC
            // 洪泛。Delay = 错过即从当前时刻重排（同：1509 广播循环先例）。
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            // 首 tick 即时（tokio interval 语义），且绕过 2 分钟年龄闸
            // ——重启后立即发起恢复查询，不用白等一个周期。后续 tick 恢复
            // 常规年龄闸（避免把刚派发的任务误查）。
            let mut first_tick = true;
            loop {
                tokio::select! {
                    _ = stop_rx.recv() => {
                        return;
                    }
                    _ = interval.tick() => {
                        // （根修）：bus / delivery_cb 以句柄传入，
                        // poll 内部分支取用时现读——不再做 tick 级快照
                        //（首 tick 快照必早于 set_message_bus 注入窗口，
                        // 见 poll_stale_pending_tasks 签名注释）。
                        poll_stale_pending_tasks(
                            &task_manager,
                            &call_fn,
                            rpc_client.clone(),
                            safety_net,
                            &bus,
                            first_tick,
                            &on_recovered_delivery,
                        ).await;
                        first_tick = false;
                    }
                }
            }
        });
    }

    /// 主动健康探针循环。
    ///
    /// 每个 tick 对所有 **Online** 对端发 frame 级 ping（5s 超时）；**Offline**
    /// 对端每 5 个 tick 探一次（自愈检测——一次成功立即翻回 Online）。连续
    /// `failure_threshold` 次失败把 Online 节点翻成 Offline。
    ///
    /// 探针走 AEAD 加密的 RPC 通道：**"TCP 通但握手/解密失败"同样计为失败**
    /// —— 探的是可通信性而非端口存活。跳过本节点；`interval=0` 不进入本循环
    /// （调用方 `start()` 已判断）。
    /// 探针 / 手动 ping 复活对端时触发节点发现回调（统一定义，两个调用方：
    /// 探针循环任务 + `mark_peer_healthy`）。
    ///
    /// 广播被隔离的部署形态（多宿主主机跨网段、AP 隔离）里 announce 永远
    /// 不来，Offline→Online 的探针翻转是唯一的「节点上线」信号——停车场
    /// sweep / 看板收编必须同样感知，否则静态 peer 部署下停车场永不复活。
    /// 幂等由调用方转移门保证（`record_probe_success` 只在翻转时返回 true；
    /// `mark_peer_healthy` 自查翻转前状态）+ gateway 侧 park_sweep_gate 节流。
    pub(super) fn fire_recovered_callback(
        on_discovered: &Mutex<Option<Arc<dyn Fn(&str, &str, &str) + Send + Sync>>>,
        registry: &PeerRegistry,
        node_id: &str,
    ) {
        if let Some(cb) = on_discovered.lock().clone()
            && let Some(info) = registry.get(node_id)
        {
            cb(node_id, info.base.role.as_role_str(), &info.base.category);
        }
    }
}

impl Cluster {
    /// Poll stale pending tasks and recover results from B-nodes.
    ///
    /// Queries tasks that have been pending for more than 2 minutes. If the
    /// remote B-node reports a completed result, completes the task locally.
    /// If the task is not found on the B-node, fails it locally. Tasks older
    /// than the pending safety net (max(24h, 2×LLM timeout)) time out.
    ///
    /// Mirrors Go's `Cluster.pollStalePendingTasks()`.
    pub async fn poll_stale_pending_tasks(&self) {
        let call_fn = self.call_with_context_fn.lock().clone();
        let rpc_client = self.rpc_client.lock().clone();
        let safety_net = self.task_manager.pending_timeout();
        // 交付回调与 bus 均以句柄传入（根修：分支内现读，
        // set 可发生在 start 之后，同 gateway 装配顺序语义）。
        poll_stale_pending_tasks(
            &self.task_manager,
            &call_fn,
            rpc_client,
            safety_net,
            &self.bus,
            false,
            &self.on_recovered_delivery,
        )
        .await;
    }
}

// ---------------------------------------------------------------------------
// Recovery loop free functions
// ---------------------------------------------------------------------------

/// A 端 stale-task 安全网超时 = max(24h, 2 × B 端 LLM 有效超时)。
///
/// `llm_timeout_secs == 0`（B 端不设超时）按 peer_chat 协议默认
/// [`crate::rpc:peer_chat_handler::DEFAULT_LLM_TIMEOUT`]（2h）计算。
/// 超时阶梯不变量：provider 单请求超时 < B 端 LLM 处理超时 < **本安全网**
/// < 结果存储 TTL（7 天）—— 安全网触发时 B 端结果尚未被 TTL 清掉，
/// A 端 poll 仍能捞回真实结果。
pub fn stale_task_safety_net(llm_timeout_secs: u64) -> chrono::Duration {
    const FLOOR: chrono::Duration = chrono::Duration::hours(24);
    let effective = if llm_timeout_secs == 0 {
        crate::rpc::peer_chat_handler::DEFAULT_LLM_TIMEOUT
    } else {
        Duration::from_secs(llm_timeout_secs)
    };
    let doubled = effective.saturating_mul(2);
    chrono::Duration::from_std(doubled)
        .unwrap_or(FLOOR)
        .max(FLOOR)
}
