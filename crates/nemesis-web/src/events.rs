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
/// 自更新下载进度（L9 两段式：易逝观测——`publish_ephemeral` 只投在线
/// 订阅者，不进环形缓冲/不参与断线补拉——回放旧进度值无意义）。
pub const EVENT_SELFUPDATE_PROGRESS: &str = "selfupdate-progress";

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
        // BUG-HUNT R8：seq 分配/入队/在线投递必须同一把缓冲写锁串行。原实现
        // fetch_add 在锁外——并发发布可交叠成「T1 得 seq=5、T2 得 seq=6、
        // T2 先抢锁入队」，环形缓冲变 [.., 6, 5]，违反下方字段注释声明的
        // 「seq 升序」不变量：replay_after 按乱序回放、front() 的 gap 判定
        // 失真、SSE id 倒退（浏览器 Last-Event-ID 回退 → 重连重复投递）。
        // chat_event_log::insert_event 同形态早已表锁内分配，此处对齐。
        // send 置于锁内保证 live 流序 == seq 序：broadcast send 非阻塞且不
        // 回调用 EventHub（无重入/锁序风险）；publish 只承载低频事件
        // （status/board/memory/scanner…），锁竞争可忽略，高频易逝流走
        // publish_ephemeral 不过此锁。
        {
            let mut buf = self.replay_buf.write();
            let seq = self.next_seq.fetch_add(1, Ordering::SeqCst) + 1;
            let event = Arc::new(Event {
                seq,
                event_type: event_type.to_string(),
                data,
            });
            buf.push_back(Arc::clone(&event));
            while buf.len() > REPLAY_BUFFER_CAP {
                buf.pop_front();
            }
            // broadcast::send ignores errors when no receivers
            let _ = self.sender.send((*event).clone());
        }
    }

    /// L10（2026-10-13 内部专项 T3）：易逝事件——只投递在线订阅者（SSE
    /// live / WS push），**不盖章 seq、不进补拉环形缓冲**。约定 `seq: 0`
    /// 为 ephemeral 标记：SSE 端点据此不发 `id:`、不推进重放游标、重放
    /// 逻辑天然跳过；断线窗口内的事件不可补 = 设计语义（provider 流式
    /// 增量是易逝观测——重放半截流无意义，且高频 delta 会冲刷 1000 帧
    /// 环、挤掉真正需要补拉的事件）。
    pub fn publish_ephemeral(&self, event_type: &str, data: serde_json::Value) {
        let event = Event {
            seq: 0,
            event_type: event_type.to_string(),
            data,
        };
        // broadcast::send ignores errors when no receivers
        let _ = self.sender.send(event);
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
