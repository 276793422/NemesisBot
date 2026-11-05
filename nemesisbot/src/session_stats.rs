//! （2026-11-01）：会话执行统计聚合器——事件侧（AgentEvent broadcast
//! 订阅）+ loop 侧（[`nemesis_agent::SessionStatsSink`] 实现）双路累计，
//! 节流落盘到 session sidecar meta 的 `stats` 字段（写路径单一收口
//! `chat_log::update_session_stats`），读侧（[`nemesis_agent::SessionStatsReader`]
//! 实现）flush 后读盘供 WSAPI `sessions.stats`。
//!
//! 计数分工：工具开始/结束与审批产生/裁决走事件（载体已有，零新接线）；
//! LLM 重试/校验失败/压缩成功走 sink（loop 内部计数器无事件载体）。
//! 审批裁决事件（`ApprovalResolved`）不带 session_key——用
//! `ApprovalRequested` 时记下的 `request_id → session_key` 映射配对归账
//! （设计文档 偏差②）。estop 不入会话统计（偏差①）：全局事件
//! 不归属单会话。
//!
//! 落盘节流：内存累计为主真相源，每会话距上次落盘 ≥5s 或 ≥32 次增量才
//! 写盘（工具调用频繁的会话不每事件一次磁盘 IO）；WSAPI 读路径前强制
//! flush——读到的恒为最新。gateway 停机不挂 flush（丢窗口 ≤5s，验收口径
//! 是重启不归零，持久层满足）。

use nemesis_agent::chat_log::SessionExecStats;
use nemesis_agent::{SessionStatsReader, SessionStatsSink, StatCounter};
use nemesis_types::agent::AgentEvent;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 距上次落盘 ≥ 此间隔才写盘（时间维节流）。
const FLUSH_INTERVAL: Duration = Duration::from_secs(5);
/// 距上次落盘 ≥ 此次数才写盘（次数维节流）。
const FLUSH_PENDING_LIMIT: u32 = 32;
/// 审批 `request_id → session_key` 映射容量上限（未裁决卡泄漏保护；FIFO 淘汰）。
const APPROVAL_MAP_CAP: usize = 1024;

/// 单会话累计账本。
#[derive(Default)]
struct SessionAccum {
    /// 本进程累计（观测用；权威读数走持久层，见 SessionStatsReader 文档）。
    total: SessionExecStats,
    /// 距上次落盘的增量（flush 载荷，落盘后清零）。
    pending: SessionExecStats,
    /// pending 里的增量事件数（节流判定）。
    pending_count: u32,
    /// 上次落盘时刻；None = 本进程从未落盘（首个增量即满足时间维）。
    last_flush: Option<Instant>,
}

impl SessionAccum {
    /// 单字段 +1（total 与 pending 同步；饱和加）。
    fn bump(&mut self, field: fn(&mut SessionExecStats) -> &mut u64) {
        *field(&mut self.total) = field(&mut self.total).saturating_add(1);
        *field(&mut self.pending) = field(&mut self.pending).saturating_add(1);
        self.pending_count += 1;
    }

    /// 节流判定：增量满 [`FLUSH_PENDING_LIMIT`]，或距上次落盘（/首次增量
    /// 时刻）≥ [`FLUSH_INTERVAL`]。
    fn due(&self) -> bool {
        self.pending_count >= FLUSH_PENDING_LIMIT
            || self.pending_count > 0
                && self
                    .last_flush
                    .is_none_or(|t| t.elapsed() >= FLUSH_INTERVAL)
    }

    /// 取走 pending 增量并重置节流锚点（调用方持锁内调用）。
    fn take_pending(&mut self) -> SessionExecStats {
        self.pending_count = 0;
        self.last_flush = Some(Instant::now());
        std::mem::take(&mut self.pending)
    }
}

/// 聚合器共享态。
#[derive(Default)]
struct Inner {
    sessions: HashMap<String, SessionAccum>,
    /// `ApprovalRequested.request_id → session_key`（裁决事件配对归账用）。
    approvals: HashMap<String, String>,
    /// approvals 插入序（FIFO 淘汰用，与 approvals 同步维护）。
    approval_order: VecDeque<String>,
}

impl Inner {
    fn acc(&mut self, session_key: &str) -> &mut SessionAccum {
        self.sessions.entry(session_key.to_string()).or_default()
    }
}

/// 会话执行统计聚合器（gateway 级单例；Sink/Reader 双实现 + 事件泵）。
#[derive(Default)]
pub struct SessionStatsAggregator {
    inner: parking_lot::Mutex<Inner>,
}

impl SessionStatsAggregator {
    /// 处理一个 agent 事件（事件泵调用）。
    fn observe(&self, ev: &AgentEvent) {
        let touched = match ev {
            AgentEvent::ToolStarted { session_key, .. } => {
                self.inner
                    .lock()
                    .acc(session_key)
                    .bump(|s| &mut s.tool_calls);
                Some(session_key.clone())
            }
            AgentEvent::ToolFinished {
                session_key, ok, ..
            } => {
                if !ok {
                    self.inner
                        .lock()
                        .acc(session_key)
                        .bump(|s| &mut s.tool_failures);
                    Some(session_key.clone())
                } else {
                    // 成功的工具结束也是「该会话有了新事件」——时间维节流
                    // 借它补判，让 5s 窗口在长成功流里也能推进。
                    Some(session_key.clone())
                }
            }
            AgentEvent::ApprovalRequested {
                session_key,
                request_id,
                ..
            } => {
                {
                    let mut inner = self.inner.lock();
                    inner.acc(session_key).bump(|s| &mut s.approvals_total);
                    // 记配对映射（容量上限 FIFO 淘汰，防未裁决卡泄漏）。
                    if !inner.approvals.contains_key(request_id) {
                        inner.approval_order.push_back(request_id.clone());
                    }
                    inner
                        .approvals
                        .insert(request_id.clone(), session_key.clone());
                    while inner.approval_order.len() > APPROVAL_MAP_CAP
                        && let Some(id) = inner.approval_order.pop_front()
                    {
                        inner.approvals.remove(&id);
                    }
                }
                Some(session_key.clone())
            }
            AgentEvent::ApprovalResolved {
                request_id,
                decision,
            } => {
                // 映射 miss（重启丢窗口 / 上限淘汰）= 诚实放弃归账——
                // approvals_total 已在请求时落，缺的只是 decision 归类。
                let mut inner = self.inner.lock();
                if let Some(session_key) = inner.approvals.remove(request_id) {
                    inner.acc(&session_key).bump(if decision == "approved" {
                        |s| &mut s.approvals_approved
                    } else {
                        // denied / timeout / orphaned 都归「未批准」。
                        |s| &mut s.approvals_denied
                    });
                    Some(session_key)
                } else {
                    None
                }
            }
            _ => None,
        };
        // 懒节流：被本事件触及的会话到达落盘条件就写（无独立 sweeper——
        // 无事件的会话没有增量可刷）。
        if let Some(key) = touched {
            self.maybe_flush(&key);
        }
    }

    /// 到达节流条件则把 pending 增量落盘（磁盘 IO 在锁外）。
    fn maybe_flush(&self, session_key: &str) {
        let delta = {
            let mut inner = self.inner.lock();
            let Some(acc) = inner.sessions.get_mut(session_key) else {
                return;
            };
            if !acc.due() {
                return;
            }
            acc.take_pending()
        };
        write_delta(session_key, &delta);
    }

    /// 强制落盘指定会话的未写增量（读路径前调；幂等）。
    fn flush_session(&self, session_key: &str) {
        let delta = {
            let mut inner = self.inner.lock();
            let Some(acc) = inner.sessions.get_mut(session_key) else {
                return;
            };
            if acc.pending_count == 0 {
                return;
            }
            acc.take_pending()
        };
        write_delta(session_key, &delta);
    }

    /// 事件泵：订阅 broadcast 直到通道关闭（gateway 停机时发送端 drop）。
    /// Lagged（消费跟不上被覆盖）= 跳过继续——统计是尽力而为的观测面，
    /// 丢帧不补偿（诚实边界）。
    pub async fn run(self: Arc<Self>, mut rx: tokio::sync::broadcast::Receiver<AgentEvent>) {
        loop {
            match rx.recv().await {
                Ok(ev) => self.observe(&ev),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("[session_stats] event lagged, {n} frames dropped");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    }
}

impl SessionStatsSink for SessionStatsAggregator {
    fn bump(&self, session_key: &str, counter: StatCounter) {
        let field = match counter {
            StatCounter::Retries => {
                (|s: &mut SessionExecStats| &mut s.retries) as fn(&mut SessionExecStats) -> &mut u64
            }
            StatCounter::ValidationFailures => {
                (|s: &mut SessionExecStats| &mut s.validation_failures)
                    as fn(&mut SessionExecStats) -> &mut u64
            }
            StatCounter::Compactions => {
                (|s: &mut SessionExecStats| &mut s.compactions)
                    as fn(&mut SessionExecStats) -> &mut u64
            }
        };
        self.inner.lock().acc(session_key).bump(field);
    }
}

impl SessionStatsReader for SessionStatsAggregator {
    fn flush_and_read(&self, session_key: &str) -> SessionExecStats {
        self.flush_session(session_key);
        nemesis_agent::chat_log::read_session_stats(session_key)
    }
}

/// 增量落盘（持锁外执行——磁盘 IO 不进临界区）。meta 写失败 WARN 留痕，
/// 增量已从内存 pending 清掉——丢失窗口与设计口径一致（≤一次节流窗），
/// 不做重试队列（下次事件重开增量）。
fn write_delta(session_key: &str, delta: &SessionExecStats) {
    if delta.is_zero() {
        return;
    }
    let d = delta.clone();
    nemesis_agent::chat_log::update_session_stats(session_key, move |s| s.add(&d));
}

/// gateway 装配：建聚合器 + spawn 事件泵。调用方持有返回的 Arc 用于
/// loop sink 注入与 AppState 注入。
pub fn spawn_session_stats(
    tx: &tokio::sync::broadcast::Sender<AgentEvent>,
) -> Arc<SessionStatsAggregator> {
    let agg = Arc::new(SessionStatsAggregator::default());
    let rx = tx.subscribe();
    tokio::spawn(agg.clone().run(rx));
    agg
}

#[cfg(test)]
mod tests;
