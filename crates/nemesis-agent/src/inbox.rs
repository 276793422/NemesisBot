//! Per-session message inbox (I1 / U7).
//!
//! Two FIFO queues per session key:
//! - `next_turn`: messages that start a NEW conversation turn after the
//!   current one finishes (the "queue" mode promise: busy → queued →
//!   processed later, not bounced with BUSY_MESSAGE).
//! - `next_step`: in-turn interjections (the "steer" promise: a message the
//!   user marks urgent is injected BEFORE the next LLM call of the running
//!   turn, so「! 别删」lands in time).
//!
//! NOT persisted (per roadmap P1.1 scope cut): a restart loses queued
//! messages. Acceptable trade-off, revisit when session events are
//! persisted.
//!
//! Steer detection is a simple, DISCOVERABLE rule: the message starts with
//! `!` (ASCII or full-width ！). The rule is echoed back in the queue
//! receipt so users can find it.

use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

/// Default capacity (total across both queues) per session.
pub const DEFAULT_QUEUE_SIZE: usize = 8;

/// A message parked in the inbox. Round-5 simplification: wraps the ORIGINAL
/// `InboundMessage` whole (H3's passthrough fields — media, correlation id,
/// metadata, voice flag, sender id — all ride for free) plus the session's
/// queue key. A struct-level field copy of InboundMessage would need hand-
/// maintained mapping blocks in loop.rs at every enqueue/drain site and drift
/// whenever InboundMessage gains a field.
#[derive(Debug, Clone)]
pub struct QueuedMessage {
    /// The original inbound message, replayed verbatim (minus the `!` steer
    /// marker when transferring next_step → next_turn).
    pub msg: nemesis_types::channel::InboundMessage,
    /// Queue-receipt timestamp (diagnostics only).
    pub timestamp: String,
    /// A5（2026-10-21 引入）：队列优先级。High 在 next_turn
    /// 队内插队（现有 High 连续段之后、第一个 Normal 之前——High 之间保持
    /// FIFO），且不受软水位约束。来源 = 消息 `metadata["priority"]`
    /// （[`priority_of`] 单点解析；WSAPI `chat.send` / 通道入站可选携带，
    /// v1 不新造内部消息流）。
    pub priority: MessagePriority,
}

/// A5（2026-10-21 引入）：队列消息优先级。借鉴前代实现
/// 抢占调度的 `waiting.push_front` 语义——被让路的消息回到队首而非队尾。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MessagePriority {
    #[default]
    Normal,
    High,
}

impl MessagePriority {
    pub fn is_high(self) -> bool {
        matches!(self, MessagePriority::High)
    }
}

/// 优先级解析单点：`metadata["priority"]`（trim 后大小写不敏感）等于
/// `"high"` = High，其余（缺键/空/未知值）= Normal。
pub fn priority_of(msg: &nemesis_types::channel::InboundMessage) -> MessagePriority {
    match msg
        .metadata
        .get("priority")
        .map(|s| s.trim().to_ascii_lowercase())
    {
        Some(p) if p == "high" => MessagePriority::High,
        _ => MessagePriority::Normal,
    }
}

/// Whether a message is a steer (in-turn interjection) candidate.
pub fn is_steer_message(content: &str) -> bool {
    let t = content.trim_start();
    t.starts_with('!') || t.starts_with('\u{ff01}')
}

/// Strip the steer marker from a message's content (round-5 fix).
///
/// The `!`/`！` prefix is a ROUTING signal, never content — the same user
/// message must reach the model marker-free on ALL three arrival timings:
/// injected in-turn (steer claim, run_loop.rs), replayed post-turn
/// (transfer to next-turn, inbox.rs), and admitted while idle
/// (`process_admitted` pre-persist strip, F2 2026-09-27 — Steer mode only;
/// in Queue/Reject modes the prefix has never been a signal, busy or idle,
/// so it stays literal there). This is the SINGLE owner of the strip rule:
/// `is_steer_message` and this fn share the marker set, so classification
/// and stripping cannot drift apart. Stripping exactly one marker doubles
/// as the escape hatch: `!!x` → `!x` (literal `!`, shell convention).
/// Returns the content unchanged for non-steer messages.
pub fn strip_steer_marker(content: &str) -> &str {
    let t = content.trim_start();
    if let Some(r) = t.strip_prefix('!') {
        r.trim_start()
    } else if let Some(r) = t.strip_prefix('\u{ff01}') {
        r.trim_start()
    } else {
        content
    }
}

/// Inbound outcome for a busy session.
#[derive(Debug, PartialEq)]
pub enum EnqueueOutcome {
    /// Stored in the next-turn queue (processed after the current turn).
    QueuedForNextTurn,
    /// Stored in the next-step queue (injected into the running turn).
    QueuedForNextStep,
    /// Both queues full — refused.
    Rejected,
    /// A5：软水位拒绝——队列未到硬上限但已达软水位（容量的 90%），
    /// 普通优先级消息暂缓接收（High 与 steer 插话不受软水位约束）。
    RejectedSoft,
}

/// Read-only inbox snapshot for the dashboard (`agent.inbox_status`).
#[derive(Debug, Clone)]
pub struct InboxStatus {
    /// Messages waiting to start a new turn after the current one.
    pub next_turn: usize,
    /// Steer messages waiting to be injected before the next LLM call.
    pub next_step: usize,
    /// Shared capacity across both queues.
    pub capacity: usize,
    /// Whether the session is currently processing a turn.
    pub busy: bool,
    /// Concurrent mode: "reject" | "queue" | "steer".
    pub mode: &'static str,
}

/// Sixth-batch sweep: `VecDeque` — the queues are FIFOs (push_back/pop_front);
/// the previous `Vec` + `remove(0)` was an O(n) shift per head claim.
#[derive(Default)]
struct SessionQueues {
    next_turn: VecDeque<QueuedMessage>,
    next_step: VecDeque<QueuedMessage>,
}

/// Inboxes for all live sessions.
pub struct Inbox {
    queues: Mutex<HashMap<String, SessionQueues>>,
    capacity: usize,
}

impl Inbox {
    pub fn new(capacity: usize) -> Self {
        Self {
            queues: Mutex::new(HashMap::new()),
            capacity: capacity.max(1),
        }
    }

    /// Park a message for a busy session. Steer candidates go to next_step,
    /// everything else to next_turn; both bounded by the shared capacity.
    pub fn enqueue(&self, session_key: &str, msg: QueuedMessage) -> EnqueueOutcome {
        self.enqueue_for_mode(session_key, msg, true)
    }

    /// Mode-aware enqueue: `steer_enabled=false` (Queue mode) routes EVERY
    /// message to next_turn — the `!`-prefix interjection channel exists
    /// only in Steer mode (second-pass review fix: Queue mode was silently
    /// upgraded to steer behavior for `!` messages).
    pub fn enqueue_for_mode(
        &self,
        session_key: &str,
        msg: QueuedMessage,
        steer_enabled: bool,
    ) -> EnqueueOutcome {
        let steer = steer_enabled && is_steer_message(&msg.msg.content);
        let priority = msg.priority;
        let mut all = self.queues.lock();
        let q = all.entry(session_key.to_string()).or_default();
        let total = q.next_turn.len() + q.next_step.len();
        if total >= self.capacity {
            return EnqueueOutcome::Rejected;
        }
        // A5 软水位：到达容量的 90% 后普通优先级消息不再入队（硬上限
        // 语义不变——所有消息到硬上限一律拒）；High 与 steer 插话不受
        // 软水位约束——高优先级的意义就在满载时段仍能进来。`.max(1)`
        // 防 capacity=1 时 soft_limit=0 把普通消息全拒死。
        let soft_limit = ((self.capacity * 9) / 10).max(1);
        if total >= soft_limit && priority != MessagePriority::High && !steer {
            return EnqueueOutcome::RejectedSoft;
        }
        if steer {
            q.next_step.push_back(msg);
            EnqueueOutcome::QueuedForNextStep
        } else if priority.is_high() {
            // A5 插队：High 插到 next_turn 现有 High 连续段之后、第一个
            // Normal 之前（High 之间保持 FIFO；与前代实现抢占的
            // `waiting.push_front` 同型——让路的消息回队首而非队尾）。
            let insert_at = q
                .next_turn
                .iter()
                .take_while(|m| m.priority.is_high())
                .count();
            q.next_turn.insert(insert_at, msg);
            EnqueueOutcome::QueuedForNextTurn
        } else {
            q.next_turn.push_back(msg);
            EnqueueOutcome::QueuedForNextTurn
        }
    }

    /// Claim the batch for the next LLM call of a running turn: ALL pending
    /// next-step messages (interjections first).
    pub fn claim_next_step(&self, session_key: &str) -> Vec<QueuedMessage> {
        let mut all = self.queues.lock();
        match all.get_mut(session_key) {
            Some(q) => std::mem::take(&mut q.next_step).into_iter().collect(),
            None => Vec::new(),
        }
    }

    /// Claim the head of the next-turn queue (one message starts a new turn).
    pub fn claim_next_turn_head(&self, session_key: &str) -> Option<QueuedMessage> {
        let mut all = self.queues.lock();
        all.get_mut(session_key)
            .and_then(|q| q.next_turn.pop_front())
    }

    /// Peek whether a next-step message is pending (turn-escape-hatch check).
    pub fn has_next_step(&self, session_key: &str) -> bool {
        self.queues
            .lock()
            .get(session_key)
            .map(|q| !q.next_step.is_empty())
            .unwrap_or(false)
    }

    /// Abort/cancel handling: unconsumed next-step messages transfer back to
    /// the next-turn queue so they are never lost (order: before the
    /// existing next-turn entries? No — AFTER, preserving the user's
    /// original arrival order of the turn queue, appended after earlier
    /// queued items).
    ///
    /// Round-5 fix: strip the `!` marker on transfer. In-turn claims strip
    /// it (loop.rs uses `strip_steer_marker`); without stripping here too,
    /// a late steer that missed the escape hatch would be replayed with the
    /// literal `!` intact — the same message reaching the model differently
    /// depending on timing.
    pub fn transfer_next_step_to_next_turn(&self, session_key: &str) {
        let mut all = self.queues.lock();
        if let Some(q) = all.get_mut(session_key) {
            let mut stepped = std::mem::take(&mut q.next_step);
            for m in stepped.iter_mut() {
                m.msg.content = strip_steer_marker(&m.msg.content).to_string();
            }
            q.next_turn.append(&mut stepped);
        }
    }

    /// Drop a session's queues entirely (after final consumption).
    pub fn clear(&self, session_key: &str) {
        self.queues.lock().remove(session_key);
    }

    /// Total pending for a session (diagnostics/tests).
    pub fn pending(&self, session_key: &str) -> (usize, usize) {
        self.queues
            .lock()
            .get(session_key)
            .map(|q| (q.next_turn.len(), q.next_step.len()))
            .unwrap_or((0, 0))
    }

    /// Shared capacity across both queues (dashboard `agent.inbox_status`).
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// C4（2026-10-24 cron 合闸）：会话队列里是否已有同一 cron job 的待处
    /// 理消息（next_turn + next_step 都算）。cron 触发间隔小于轮次处理时长
    /// 时，无合闸的 FIFO 会无限积压（每 tick 一条、每条一轮 LLM 全量上下
    /// 文重发）；admission 的 busy 闸用本判定丢弃同 job 重复 tick，把积压
    /// 上界钉在「1 条在途 + 1 条排队」。
    pub fn has_pending_cron_job(&self, session_key: &str, cron_job_id: &str) -> bool {
        let all = self.queues.lock();
        all.get(session_key).is_some_and(|q| {
            q.next_turn
                .iter()
                .any(|m| m.msg.metadata.get("cron_job_id").map(|s| s.as_str()) == Some(cron_job_id))
                || q.next_step.iter().any(|m| {
                    m.msg.metadata.get("cron_job_id").map(|s| s.as_str()) == Some(cron_job_id)
                })
        })
    }
}

pub type SharedInbox = Arc<Inbox>;

#[cfg(test)]
mod tests;
