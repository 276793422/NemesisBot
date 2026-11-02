//! L14（2026-11-01）：会话执行统计 sink——loop 侧三个无事件载体的计数
//! （LLM 重试 / args 校验失败回灌 / 压缩成功）经本 trait 上抛给宿主聚合器。
//!
//! 分工：有事件载体的计数（工具开始/结束、审批产生/裁决）由宿主
//! （nemesisbot `session_stats::SessionStatsAggregator`）订阅
//! `AgentEvent` broadcast 自行累计，不经本 trait；本 trait 只补
//! loop 内部计数器的缺口。`None`（standalone / 未注入）= 不统计，
//! 调用点零开销。

/// L14：可累加的 loop 内部计数器种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatCounter {
    /// LLM 瞬态/限流/上下文错误三环的每次重试发起（含最终失败的重试）。
    Retries,
    /// args schema 校验失败回灌次数（`validation_failures` 递增处）。
    ValidationFailures,
    /// 上下文压缩成功采用次数（auto + manual + 上下文错误环内 force）。
    Compactions,
}

/// L14：宿主侧统计聚合器的 loop 侧上报口。实现方必须内部无锁竞争
/// 热点（推荐 `Mutex<HashMap>` + 节流落盘），不得阻塞调用线程。
pub trait SessionStatsSink: Send + Sync {
    fn bump(&self, session_key: &str, counter: StatCounter);
}

/// L14：宿主聚合器的读侧——先把未落盘增量刷进 sidecar meta，再读回
/// 持久层总数。读盘而非读内存：重启后进程内累计清零而持久层保留，
/// 读盘口径在「进程内」与「跨重启」两态下都拿到完整真相。
pub trait SessionStatsReader: Send + Sync {
    fn flush_and_read(&self, session_key: &str) -> crate::chat_log::SessionExecStats;
}
