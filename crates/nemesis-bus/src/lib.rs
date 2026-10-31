//! NemesisBot - Message Bus
//!
//! Central pub/sub system using tokio broadcast channels.
//! Replaces Go channels with tokio::sync::broadcast for multi-subscriber support.

use nemesis_types::channel::{InboundMessage, OutboundMessage};
use nemesis_types::constants::BUS_CHANNEL_CAPACITY;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

mod filter;
#[cfg(test)]
mod filter_tests;

pub use filter::{Filter, FilterChain, FilterDecision};

/// Message bus for routing messages between channels and agents.
pub struct MessageBus {
    inbound_tx: broadcast::Sender<InboundMessage>,
    outbound_tx: broadcast::Sender<OutboundMessage>,
    closed: AtomicBool,
    /// 发布侧丢失（入站）：发布时**无任何接收者**。tokio broadcast 的
    /// `send` 只在接收者数为 0 时失败——「buffer full 导致 send 丢消息」
    /// 不存在（缓冲满 = 覆盖最旧，丢帧发生在接收端以 `Lagged(n)` 上报，
    /// 经 [`MessageBus::record_inbound_lagged`] 记入下方 lagged 计数）。
    inbound_no_receiver_dropped: AtomicU64,
    /// 发布侧丢失（出站）：语义同上。
    outbound_no_receiver_dropped: AtomicU64,
    /// 接收侧真实丢失（入站）：订阅端 `recv()` 返回 `Lagged(n)` 时经
    /// [`MessageBus::record_inbound_lagged`] 累计（n = 该接收者被覆盖
    /// 跳过的消息数；广播扇出下各接收者独立计数）。
    inbound_lagged: AtomicU64,
    /// 接收侧真实丢失（出站）：语义同上。
    outbound_lagged: AtomicU64,
    /// P6a（2026-09-11 日志降噪）：fan-out 警告每方向只发一次——gateway 启动
    /// 期固定多订阅者（agent loop / web dispatch / cluster 桥各订一份）属正常
    /// 形态，逐订阅 WARN 只是每世代 3 条启动噪音。
    inbound_fanout_warned: AtomicBool,
    outbound_fanout_warned: AtomicBool,
}

impl MessageBus {
    /// Create a new message bus with default capacity.
    pub fn new() -> Self {
        Self::with_capacity(BUS_CHANNEL_CAPACITY)
    }

    /// Create a new message bus with custom capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        let (inbound_tx, _) = broadcast::channel(capacity);
        let (outbound_tx, _) = broadcast::channel(capacity);
        Self {
            inbound_tx,
            outbound_tx,
            closed: AtomicBool::new(false),
            inbound_no_receiver_dropped: AtomicU64::new(0),
            outbound_no_receiver_dropped: AtomicU64::new(0),
            inbound_lagged: AtomicU64::new(0),
            outbound_lagged: AtomicU64::new(0),
            inbound_fanout_warned: AtomicBool::new(false),
            outbound_fanout_warned: AtomicBool::new(false),
        }
    }

    /// Inbound 订阅者计数（发布侧就绪探测）。
    ///
    /// cluster 恢复链路（T19 二层根修）在 publish 前用本方法等待
    /// AgentLoop 订阅就位——broadcast send 对无接收者是静默丢弃。
    pub fn inbound_receiver_count(&self) -> usize {
        self.inbound_tx.receiver_count()
    }

    /// Publish an inbound message (from channel to agent).
    ///
    /// Tokio broadcast 的 `send` 只在无接收者时失败（缓冲满不丢发布、覆盖
    /// 最旧；慢消费者的丢帧在接收端以 `Lagged` 上报）。无接收者时本方法
    /// 记 warn 并累计发布侧丢失计数。
    pub fn publish_inbound(&self, msg: InboundMessage) {
        if self.closed.load(Ordering::Relaxed) {
            warn!("[Bus] Publish inbound rejected: bus is closed");
            return;
        }
        let receiver_count = self.inbound_tx.receiver_count();
        if receiver_count == 0 {
            warn!(
                "[Bus] publish_inbound: no inbound receivers, message will be dropped (channel={}, chat_id={})",
                msg.channel, msg.chat_id
            );
            self.inbound_no_receiver_dropped
                .fetch_add(1, Ordering::Relaxed);
            return;
        }
        let channel_name = msg.channel.clone();
        if let Err(err) = self.inbound_tx.send(msg) {
            self.inbound_no_receiver_dropped
                .fetch_add(1, Ordering::Relaxed);
            warn!(
                "[Bus] publish_inbound: failed to send inbound message: {}",
                err
            );
        } else {
            debug!("[Bus] Published inbound message, channel={}", channel_name);
        }
    }

    /// Publish an outbound message (from agent to channel).
    ///
    /// 语义同 [`MessageBus::publish_inbound`]：`send` 只在无接收者时失败，
    /// 慢消费者丢帧走接收侧 `record_outbound_lagged`。
    pub fn publish_outbound(&self, msg: OutboundMessage) {
        if self.closed.load(Ordering::Relaxed) {
            warn!("[Bus] Publish outbound rejected: bus is closed");
            return;
        }
        let receiver_count = self.outbound_tx.receiver_count();
        if receiver_count == 0 {
            warn!(
                "[Bus] publish_outbound: no outbound receivers, message will be dropped (channel={}, chat_id={})",
                msg.channel, msg.chat_id
            );
            self.outbound_no_receiver_dropped
                .fetch_add(1, Ordering::Relaxed);
            return;
        }
        let channel_name = msg.channel.clone();
        if let Err(err) = self.outbound_tx.send(msg) {
            self.outbound_no_receiver_dropped
                .fetch_add(1, Ordering::Relaxed);
            warn!(
                "[Bus] publish_outbound: failed to send outbound message: {}",
                err
            );
        } else {
            debug!("[Bus] Published outbound message, channel={}", channel_name);
        }
    }

    /// Subscribe to inbound messages.
    ///
    /// Logs a warning if there are already existing subscribers, because
    /// broadcast is fan-out (every subscriber gets every message), unlike
    /// Go's point-to-point channels. Having multiple subscribers is usually
    /// unintentional and may indicate a bug where a component subscribes
    /// more than once.
    ///
    /// P6a：警告只发一次（每方向）——多订阅者是 gateway 启动的正常形态，
    /// 逐次 WARN 是每世代固定启动噪音。
    pub fn subscribe_inbound(&self) -> broadcast::Receiver<InboundMessage> {
        let existing = self.inbound_tx.receiver_count();
        // BUG-20261030-214：`total` 必须是**订阅后**总数——旧实现打新增前
        // 计数（首个订阅者打 total=0），恒差 1，排障时订阅拓扑口径失真。
        // fan-out warn 里的 existing_receivers 是「既有数」语义，保持不动。
        info!(
            "[Bus] New inbound subscriber, total receivers={}",
            existing + 1
        );
        if existing > 0 && !self.inbound_fanout_warned.swap(true, Ordering::Relaxed) {
            warn!(
                existing_receivers = existing,
                "[Bus] subscribe_inbound: additional subscriber added to broadcast channel; \
                 each subscriber receives every message (fan-out), which may be unintentional \
                 (warned once per bus)"
            );
        }
        self.inbound_tx.subscribe()
    }

    /// Subscribe to outbound messages.
    ///
    /// Logs a warning if there are already existing subscribers for the same
    /// fan-out concern as `subscribe_inbound` (warn-once per bus).
    pub fn subscribe_outbound(&self) -> broadcast::Receiver<OutboundMessage> {
        let existing = self.outbound_tx.receiver_count();
        // BUG-20261030-214：同 subscribe_inbound——total 打订阅后总数。
        info!(
            "[Bus] New outbound subscriber, total receivers={}",
            existing + 1
        );
        if existing > 0 && !self.outbound_fanout_warned.swap(true, Ordering::Relaxed) {
            warn!(
                existing_receivers = existing,
                "[Bus] subscribe_outbound: additional subscriber added to broadcast channel; \
                 each subscriber receives every message (fan-out), which may be unintentional \
                 (warned once per bus)"
            );
        }
        self.outbound_tx.subscribe()
    }

    /// Get a sender for inbound messages (for direct publishing).
    pub fn inbound_sender(&self) -> broadcast::Sender<InboundMessage> {
        self.inbound_tx.clone()
    }

    /// Get a sender for outbound messages (for direct publishing).
    pub fn outbound_sender(&self) -> broadcast::Sender<OutboundMessage> {
        self.outbound_tx.clone()
    }

    /// Check if the bus is closed.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    /// Returns the number of active inbound subscribers.
    pub fn inbound_subscriber_count(&self) -> usize {
        self.inbound_tx.receiver_count()
    }

    /// Returns the number of active outbound subscribers.
    pub fn outbound_subscriber_count(&self) -> usize {
        self.outbound_tx.receiver_count()
    }

    /// Returns the total number of inbound messages dropped at publish time
    /// because no receiver existed（发布侧丢失，语义见字段注释）。
    pub fn no_receiver_dropped_inbound(&self) -> u64 {
        self.inbound_no_receiver_dropped.load(Ordering::Relaxed)
    }

    /// Returns the total number of outbound messages dropped at publish time
    /// because no receiver existed（发布侧丢失，语义见字段注释）。
    pub fn no_receiver_dropped_outbound(&self) -> u64 {
        self.outbound_no_receiver_dropped.load(Ordering::Relaxed)
    }

    /// 接收侧真实丢失记账入口（入站）：订阅端 `recv()` 返回
    /// `RecvError::Lagged(n)` 时调用（n = 该接收者被覆盖跳过的消息数）。
    /// 广播扇出下各接收者独立丢帧，n 直接累加（不按接收者去重——指标语义
    /// 是「消息实际未能送达的次数」）。
    pub fn record_inbound_lagged(&self, n: u64) {
        if n > 0 {
            self.inbound_lagged.fetch_add(n, Ordering::Relaxed);
        }
    }

    /// 接收侧真实丢失记账入口（出站），语义同 [`MessageBus::record_inbound_lagged`]。
    pub fn record_outbound_lagged(&self, n: u64) {
        if n > 0 {
            self.outbound_lagged.fetch_add(n, Ordering::Relaxed);
        }
    }

    /// Returns the total inbound messages reported lost by lagged receivers.
    pub fn lagged_inbound(&self) -> u64 {
        self.inbound_lagged.load(Ordering::Relaxed)
    }

    /// Returns the total outbound messages reported lost by lagged receivers.
    pub fn lagged_outbound(&self) -> u64 {
        self.outbound_lagged.load(Ordering::Relaxed)
    }

    /// Close the bus. No more messages can be published.
    pub fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
    }
}

impl Default for MessageBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;

// 覆盖率补充测试（Default/Debug/Rejected 日志路径）。
#[cfg(test)]
mod filter_cov_tests;

// 覆盖率补充测试（发送失败 / dropped 计数分支）。
#[cfg(test)]
mod bus_cov_tests;
