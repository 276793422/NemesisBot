//! NemesisBot - Message Bus
//!
//! Central pub/sub system using tokio broadcast channels.
//! Replaces Go channels with tokio::sync::broadcast for multi-subscriber support.

use nemesis_types::channel::{InboundMessage, OutboundMessage};
use nemesis_types::constants::BUS_CHANNEL_CAPACITY;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

/// Message bus for routing messages between channels and agents.
pub struct MessageBus {
    inbound_tx: broadcast::Sender<InboundMessage>,
    outbound_tx: broadcast::Sender<OutboundMessage>,
    closed: AtomicBool,
    /// Number of inbound messages dropped because no receivers or buffer full.
    inbound_dropped: AtomicU64,
    /// Number of outbound messages dropped because no receivers or buffer full.
    outbound_dropped: AtomicU64,
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
            inbound_dropped: AtomicU64::new(0),
            outbound_dropped: AtomicU64::new(0),
            inbound_fanout_warned: AtomicBool::new(false),
            outbound_fanout_warned: AtomicBool::new(false),
        }
    }

    /// Publish an inbound message (from channel to agent).
    ///
    /// Unlike Go which blocks when the channel buffer is full, Rust's broadcast
    /// silently drops messages. This method logs a warning when the send fails
    /// (e.g., no receivers or buffer full) and increments a dropped counter.
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
            self.inbound_dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let channel_name = msg.channel.clone();
        if let Err(err) = self.inbound_tx.send(msg) {
            self.inbound_dropped.fetch_add(1, Ordering::Relaxed);
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
    /// Unlike Go which blocks when the channel buffer is full, Rust's broadcast
    /// silently drops messages. This method logs a warning when the send fails
    /// (e.g., no receivers or buffer full) and increments a dropped counter.
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
            self.outbound_dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let channel_name = msg.channel.clone();
        if let Err(err) = self.outbound_tx.send(msg) {
            self.outbound_dropped.fetch_add(1, Ordering::Relaxed);
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
        info!("[Bus] New inbound subscriber, total receivers={}", existing);
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
        info!(
            "[Bus] New outbound subscriber, total receivers={}",
            existing
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

    /// Returns the total number of inbound messages dropped since bus creation.
    pub fn dropped_inbound(&self) -> u64 {
        self.inbound_dropped.load(Ordering::Relaxed)
    }

    /// Returns the total number of outbound messages dropped since bus creation.
    pub fn dropped_outbound(&self) -> u64 {
        self.outbound_dropped.load(Ordering::Relaxed)
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
