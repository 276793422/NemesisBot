//! NemesisBot - Message Bus
//!
//! Central pub/sub system using tokio broadcast channels.
//! Replaces Go channels with tokio::sync:broadcast for multi-subscriber support.

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
    /// （日志降噪）：fan-out 警告每方向只发一次——gateway 启动
    /// 期固定多订阅者（agent loop / web dispatch / cluster 桥各订一份）属正常
    /// 形态，逐订阅 WARN 只是每世代 3 条启动噪音。
    inbound_fanout_warned: AtomicBool,
    outbound_fanout_warned: AtomicBool,
    /// 语义断言 panic 态（2026-11-21 msg_class 检测网，B2 显式开关）。
    /// 构造期决议：显式 [`MessageBus::with_semantic_assert`] 注入优先，
    /// 否则读 env [`SEMANTIC_ASSERT_ENV`]（测试 harness 用），都缺 = false
    /// （生产 warn 态兜底观测）。
    semantic_assert_panic: bool,
    /// 语义断言 warn 限频（每 bus 实例只发一次）。
    semantic_warn_emitted: AtomicBool,
}

/// 语义断言开关 env（B2：显式 env 而非 cfg!(test)——nemesis-bus 作为
/// 依赖编进 nemesis-agent 等的测试目标时 `cfg(test)` 恒 false，panic 永不
/// 触发，自证锁测空）。**单一真相源在 nemesis-types**（C1 绊线 admission
/// 闸同键消费，nemesis-agent 不依赖 nemesis-bus——依赖箭头只允许向下）；
/// 此处 re-export 保持既有公共路径稳定。
pub use nemesis_types::constants::SEMANTIC_ASSERT_ENV;

/// 内部形状标记表（检测网数据驱动面，B3 收窄版）。
///
/// 命中任一 = 该消息形状上是内部生产者的产物。User class + 命中 = 绕过
/// 构造器家族的逃逸（构造器烤死 class；serde default 陷阱漏填；
/// `..Default::default()` 漏填），当场抓。
///
/// 收窄纪律（B3）：`sender=="system"` 单条过宽（真实平台用户 sender_id
/// 可能恰为 "system"）——收窄为 `sender=="system" && channel=="system"`
/// 组合 + `channel=="system"`（内部通道名，平台通道不用此名）+ 前缀/键。
fn internal_shape_marker(msg: &InboundMessage) -> Option<String> {
    if msg.sender_id == "system" && msg.channel == "system" {
        return Some("sender=system&channel=system".to_string());
    }
    if msg.channel == "system" {
        return Some("channel=system".to_string());
    }
    for (prefix, name) in [
        ("cron:", "sender cron:*"),
        ("cluster_continuation:", "sender cluster_continuation:*"),
        (
            nemesis_types::constants::SUBAGENT_CONTINUATION_PREFIX,
            "sender subagent_continuation:*",
        ),
    ] {
        if msg.sender_id.starts_with(prefix) {
            return Some(name.to_string());
        }
    }
    if msg.sender_id == "board" {
        return Some("sender=board".to_string());
    }
    for key in [
        "cron_job_id",
        "turn_wal_resume",
        "system_notice",
        "hook_prompt",
        nemesis_types::channel::INSTRUCTION_KIND_STAMP_KEY,
    ] {
        if msg.metadata.contains_key(key) {
            return Some(format!("metadata:{key}"));
        }
    }
    None
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
        let semantic_assert_panic = matches!(
            std::env::var(SEMANTIC_ASSERT_ENV).as_deref(),
            Ok("panic") | Ok("1") | Ok("true")
        );
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
            semantic_assert_panic,
            semantic_warn_emitted: AtomicBool::new(false),
        }
    }

    /// 语义断言 panic 态显式注入（检测网自证锁用；优先于 env）。
    pub fn with_semantic_assert(mut self, panic_on_violation: bool) -> Self {
        self.semantic_assert_panic = panic_on_violation;
        self
    }

    /// Inbound 订阅者计数（发布侧就绪探测）。
    ///
    /// cluster 恢复链路（二层根修）在 publish 前用本方法等待
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
        self.assert_inbound_semantics(&msg);
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
    /// 警告只发一次（每方向）——多订阅者是 gateway 启动的正常形态，
    /// 逐次 WARN 是每世代固定启动噪音。
    pub fn subscribe_inbound(&self) -> broadcast::Receiver<InboundMessage> {
        let existing = self.inbound_tx.receiver_count();
        // `total` 必须是**订阅后**总数——旧实现打新增前
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
        // 同 subscribe_inbound——total 打订阅后总数。
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

    /// 检测网点位 1（2026-11-21 msg_class 架构）：发布入口语义断言。
    ///
    /// User class + 内部形状标记（[`internal_shape_marker`]）= 生产者绕过
    /// 构造器家族的逃逸口（构造器烤死 class；`#[serde(default)]` 陷阱漏
    /// 填；手写字面量漏填）。检查**语义**而非写法——三条逃逸路都抓。
    ///
    /// 处置（B2 显式开关，非 cfg!(test)）：panic 态（构造注入或 env）
    /// → panic 当场炸（回归当场现形）；默认 warn 态 → 限频一条（生产
    /// 兜底观测，不炸运行）。
    fn assert_inbound_semantics(&self, msg: &InboundMessage) {
        if !matches!(msg.msg_class, nemesis_types::channel::MsgClass::User) {
            return;
        }
        let Some(marker) = internal_shape_marker(msg) else {
            return;
        };
        if self.semantic_assert_panic {
            panic!(
                "[Bus] 语义断言失败：User class 消息携带内部形状标记（marker={}, channel={}, sender={}）——内部生产者必须走 InboundMessage 构造器家族（notice/instruction/continuation），禁止字面量构造",
                marker, msg.channel, msg.sender_id
            );
        }
        if !self.semantic_warn_emitted.swap(true, Ordering::Relaxed) {
            warn!(
                "[Bus] 语义断言（warn 态）：User class 消息携带内部形状标记（marker={}, channel={}, sender={}）——内部生产者必须走构造器家族；设 {}=panic 或 with_semantic_assert(true) 可在测试中炸出",
                marker, msg.channel, msg.sender_id, SEMANTIC_ASSERT_ENV
            );
        }
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
