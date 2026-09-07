//! SSE Event Hub for server-sent events.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use tokio::sync::broadcast;

/// Event type constants.
pub const EVENT_LOG: &str = "log";
pub const EVENT_STATUS: &str = "status";
pub const EVENT_SECURITY_ALERT: &str = "security-alert";
pub const EVENT_SCANNER_PROGRESS: &str = "scanner-progress";
pub const EVENT_CLUSTER_EVENT: &str = "cluster-event";
pub const EVENT_HEARTBEAT: &str = "heartbeat";
/// Chat streaming delta — published for each streamed LLM token chunk.
pub const EVENT_CHAT_STREAM: &str = "chat-stream";

/// L2（devtool-upgrade 阶段 6）：断线补拉环形缓冲容量。1000 条事件窗口
///（status 每 5s 一条 ≈ 83 分钟回放窗口）；滑出窗口的 seq 不可补——
/// SSE 端点据此发 `resync` 提示事件，前端全量刷新兜底。
pub const REPLAY_BUFFER_CAP: usize = 1000;

/// A server-sent event.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Event {
    /// L2：单调递增序号（1 起）。断线补拉游标——SSE 每帧 `id:` 字段、
    /// 浏览器重连自动回传 `Last-Event-ID`，服务端按 seq>last 从环形缓冲
    /// 重放。WS chat 推送帧的 `data.seq` 同源不同流（会话事件独立环形，
    /// 见 chat_event_log）。
    pub seq: u64,
    pub event_type: String,
    pub data: serde_json::Value,
}

/// Event hub that manages SSE subscribers and broadcasts events.
pub struct EventHub {
    sender: broadcast::Sender<Event>,
    subscriber_count: Arc<AtomicUsize>,
    next_seq: AtomicU64,
    /// L2：最近 REPLAY_BUFFER_CAP 条事件（seq 升序）——断线补拉唯一数据源。
    replay_buf: parking_lot::RwLock<VecDeque<Arc<Event>>>,
}

impl EventHub {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(256);
        Self {
            sender,
            subscriber_count: Arc::new(AtomicUsize::new(0)),
            next_seq: AtomicU64::new(0),
            replay_buf: parking_lot::RwLock::new(VecDeque::new()),
        }
    }

    /// Subscribe to events. Returns a receiver.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.subscriber_count.fetch_add(1, Ordering::SeqCst);
        self.sender.subscribe()
    }

    /// Unsubscribe (decrement counter).
    pub fn unsubscribe(&self) {
        self.subscriber_count.fetch_sub(1, Ordering::SeqCst);
    }

    /// Publish an event to all subscribers.
    ///
    /// L2：发布即盖章单调 seq 并进环形缓冲（无订阅者也落——补拉依赖缓冲
    /// 而非 broadcast 通道；broadcast 256 槽只服务在线订阅者）。
    pub fn publish(&self, event_type: &str, data: serde_json::Value) {
        let seq = self.next_seq.fetch_add(1, Ordering::SeqCst) + 1;
        let event = Arc::new(Event {
            seq,
            event_type: event_type.to_string(),
            data,
        });
        {
            let mut buf = self.replay_buf.write();
            buf.push_back(Arc::clone(&event));
            while buf.len() > REPLAY_BUFFER_CAP {
                buf.pop_front();
            }
        }
        // broadcast::send ignores errors when no receivers
        let _ = self.sender.send((*event).clone());
    }

    /// Latest published seq（0 = 尚未发布过任何事件）。
    pub fn latest_seq(&self) -> u64 {
        self.next_seq.load(Ordering::SeqCst)
    }

    /// L2 断线补拉：返回 seq > `after` 的事件（时间序）。
    ///
    /// `gap=true` 表示 `(after, latest]` 里有事件已滑出环形缓冲（或 `after`
    /// 超前于服务端——网关重启 seq 重置），调用方应发 `resync` 提示让前端
    /// 全量刷新兜底，不能假装补齐。
    pub fn replay_after(&self, after: u64) -> (Vec<Arc<Event>>, bool) {
        let latest = self.latest_seq();
        if after >= latest {
            // 追平 → 无可补；超前（重启后 seq 重置）→ gap 提示重同步。
            return (Vec::new(), after > latest);
        }
        let buf = self.replay_buf.read();
        let events: Vec<Arc<Event>> = buf.iter().filter(|e| e.seq > after).cloned().collect();
        // 空缓冲但 latest>after 理论不可达（发布过的必在缓冲），保守按 gap。
        let gap = buf.front().is_none_or(|oldest| oldest.seq > after + 1);
        (events, gap)
    }

    /// Get the number of active subscribers.
    pub fn subscriber_count(&self) -> usize {
        self.subscriber_count.load(Ordering::SeqCst)
    }
}

impl Default for EventHub {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
