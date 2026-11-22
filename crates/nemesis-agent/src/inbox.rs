//! Per-session message inbox.
//!
//! Two FIFO queues per session key:
//! - `next_turn`: messages that start a NEW conversation turn after the
//!   current one finishes (the "queue" mode promise: busy → queued →
//!   processed later, not bounced with BUSY_MESSAGE).
//! - `next_step`: in-turn interjections (the "steer" promise: a message the
//!   user marks urgent is injected BEFORE the next LLM call of the running
//!   turn, so「! 别删」lands in time).
//!
//! NOT persisted (per roadmap.1 scope cut): a restart loses queued
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
/// `InboundMessage` whole ('s passthrough fields — media, correlation id,
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
    /// （引入）：队列优先级。High 在 next_turn
    /// 队内插队（现有 High 连续段之后、第一个 Normal 之前——High 之间保持
    /// FIFO），且不受软水位约束。来源 = 消息 `metadata["priority"]`
    /// （[`priority_of`] 单点解析；WSAPI `chat.send` / 通道入站可选携带，
    /// v1 不新造内部消息流）。
    pub priority: MessagePriority,
}

/// （引入）：队列消息优先级。借鉴前代实现
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
/// (`process_admitted` pre-persist strip, — Steer mode only;
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
    /// 软水位拒绝——队列未到硬上限但已达软水位（容量的 90%），
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
    // 系统通告 stash（2026-11-21 幻影通告根治）：sender 带系统通告 metadata
    // 的消息 busy 时不再排队/回执/弹回，原文暂存于此，轮边界合并注入。
    // 独立于 next_turn/next_step——那两条队列的消费语义是「物化 user 行」，
    // 通告的消费语义是「合并 system 注记」，混入任何一条都会复发幻影。
    notices: Vec<NoticeEntry>,
}

/// 单条暂存通告（原文 + 来源路由 + kind）：合并注入的徽标帧 / 收口通告轮的
/// 出站回复按**最后一条**通告的 channel/chat_id 路由（与 steer claim 的
/// 帧路由同规则——注入事件归属这批消息）。`kind` 来自 msg_class
/// （A2 显示保真 + `.notices` 侧车还原重建用，msg_class 单轨 §10-7/8）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NoticeEntry {
    pub kind: nemesis_types::channel::NoticeKind,
    pub channel: String,
    pub chat_id: String,
    pub content: String,
}

/// 通告 stash 侧车行格式（msg_class 单轨 §10-8）：session_key 包络——
/// 扫描器按首行还原真实会话键（与 pending 侧车 first-line 惯例同型）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PersistedNotice {
    pub session_key: String,
    pub entry: NoticeEntry,
}

/// drain 合并产物：`content` = 合并文本；`channel`/`chat_id` = 最后一条
/// 通告的来源路由（通告轮回复回到最后通告的通道）；`count` = 本次合并的
/// 通告条数（注记文案用）。
#[derive(Debug, Clone)]
pub(crate) struct NoticeMerge {
    pub(crate) channel: String,
    pub(crate) chat_id: String,
    pub(crate) content: String,
    pub(crate) count: usize,
}

/// 单条通告 stash 截断上限（字符）：通告自带后台输出尾部，病态超长不进
/// stash（截断带诚实注记）。正常通告 ~200-500 字符，4K 是余量不是常态。
const NOTICE_STASH_ENTRY_CAP: usize = 4000;
/// stash 条数上限：超过即丢最旧（通告是信息不是指令，丢最旧 + drain 侧
/// 注记披露；与队列容量语义同型）。
const NOTICE_STASH_CAP: usize = 32;
/// drain 合并文本总长上限（字符）：合并注入进模型上下文，病态总量截断
/// 并注明丢弃条数，不静默。
const NOTICE_MERGE_TOTAL_CAP: usize = 24_000;

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

    /// A3 写穿镜像（msg_class 单轨 §10-8，2026-11-21 拍板）：持有区活状态
    /// 每次突变后全量镜像到侧车（内存态 = 盘上态，无追加式漂移；空 = 删）。
    /// 两族各一视图：
    ///   - `.iqueue`：next_turn（含 next_step 兜底）中 Instruction 类条目按
    ///     序序列化为原始 InboundMessage 行（hydrate 侧经 legacy_class_coerce
    ///     兼容旧格式）；User 类排队条目不落盘——那是用户消息语义（有 ⏳
    ///     回执与弹回留痕），崩溃丢失可接受，不属于 A3 三持有区。
    ///   - `.notices`：stash 全量 PersistedNotice 包络行（session_key 在行内
    ///     ——扫描器按首行还原会话键）。
    ///
    /// 调用方必须持 `queues` 锁（镜像读的是突变后的确定状态）。写失败 =
    /// chat_log 层响亮 warn 不炸热路径。
    fn mirror_session_state(&self, session_key: &str, q: &SessionQueues) {
        let mut iqueue_rows: Vec<&crate::inbox::QueuedMessage> = q.next_turn.iter().collect();
        iqueue_rows.extend(q.next_step.iter());
        let iqueue_lines: Vec<String> = iqueue_rows
            .iter()
            .filter(|m| {
                matches!(
                    m.msg.msg_class,
                    nemesis_types::channel::MsgClass::Instruction(_)
                )
            })
            .filter_map(|m| serde_json::to_string(&m.msg).ok())
            .collect();
        crate::chat_log::mirror_write(
            &crate::chat_log::instruction_queue_path(session_key),
            &iqueue_lines,
        );
        let notice_lines: Vec<String> = q
            .notices
            .iter()
            .filter_map(|e| {
                serde_json::to_string(&PersistedNotice {
                    session_key: session_key.to_string(),
                    entry: e.clone(),
                })
                .ok()
            })
            .collect();
        crate::chat_log::mirror_write(
            &crate::chat_log::notices_stash_path(session_key),
            &notice_lines,
        );
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
        // 软水位：到达容量的 90% 后普通优先级消息不再入队（硬上限
        // 语义不变——所有消息到硬上限一律拒）；High 与 steer 插话不受
        // 软水位约束——高优先级的意义就在满载时段仍能进来。`.max(1)`
        // 防 capacity=1 时 soft_limit=0 把普通消息全拒死。
        let soft_limit = ((self.capacity * 9) / 10).max(1);
        if total >= soft_limit && priority != MessagePriority::High && !steer {
            return EnqueueOutcome::RejectedSoft;
        }
        if steer {
            q.next_step.push_back(msg);
            self.mirror_session_state(session_key, q);
            EnqueueOutcome::QueuedForNextStep
        } else if priority.is_high() {
            // 插队：High 插到 next_turn 现有 High 连续段之后、第一个
            // Normal 之前（High 之间保持 FIFO；与前代实现抢占的
            // `waiting.push_front` 同型——让路的消息回队首而非队尾）。
            let insert_at = q
                .next_turn
                .iter()
                .take_while(|m| m.priority.is_high())
                .count();
            q.next_turn.insert(insert_at, msg);
            self.mirror_session_state(session_key, q);
            EnqueueOutcome::QueuedForNextTurn
        } else {
            q.next_turn.push_back(msg);
            self.mirror_session_state(session_key, q);
            EnqueueOutcome::QueuedForNextTurn
        }
    }

    /// Programmatic steer push（项3 子代理飞行中驾驶）：直接把消息插进
    /// next_step 队列，不经 `!` 标记检测（调用方是 WSAPI/工具程序化面，
    /// 语义本就是 steer，不依赖前缀路由），也不受软水位约束（steer 插话
    /// 豁免，与 [`Self::enqueue_for_mode`] 同规则）；硬上限仍然生效——
    /// 两队合计到容量 = [`EnqueueOutcome::Rejected`]，由调用方映射为诚实
    /// 拒绝。消费点不变：轮边界 `claim_next_step`（claim_steer_messages）。
    pub fn enqueue_steer(&self, session_key: &str, msg: QueuedMessage) -> EnqueueOutcome {
        let mut all = self.queues.lock();
        let q = all.entry(session_key.to_string()).or_default();
        let total = q.next_turn.len() + q.next_step.len();
        if total >= self.capacity {
            return EnqueueOutcome::Rejected;
        }
        q.next_step.push_back(msg);
        self.mirror_session_state(session_key, q);
        EnqueueOutcome::QueuedForNextStep
    }

    /// Claim the batch for the next LLM call of a running turn: ALL pending
    /// next-step messages (interjections first).
    pub fn claim_next_step(&self, session_key: &str) -> Vec<QueuedMessage> {
        let mut all = self.queues.lock();
        match all.get_mut(session_key) {
            Some(q) => {
                let out = std::mem::take(&mut q.next_step).into_iter().collect();
                self.mirror_session_state(session_key, q);
                out
            }
            None => Vec::new(),
        }
    }

    /// Claim the head of the next-turn queue (one message starts a new turn).
    pub fn claim_next_turn_head(&self, session_key: &str) -> Option<QueuedMessage> {
        let mut all = self.queues.lock();
        let claimed = all
            .get_mut(session_key)
            .and_then(|q| q.next_turn.pop_front());
        if claimed.is_some()
            && let Some(q) = all.get(session_key)
        {
            self.mirror_session_state(session_key, q);
        }
        claimed
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
            self.mirror_session_state(session_key, q);
        }
    }

    /// Drop a session's queues entirely (after final consumption).
    pub fn clear(&self, session_key: &str) {
        let mut all = self.queues.lock();
        if all.remove(session_key).is_some() {
            // A3 镜像级联：持有区清空 = 两族侧车删（镜像语义：内存空 =
            // 盘上空——不删会话重启后 hydrate 出幽灵重放）。
            crate::chat_log::mirror_write(
                &crate::chat_log::instruction_queue_path(session_key),
                &[],
            );
            crate::chat_log::mirror_write(&crate::chat_log::notices_stash_path(session_key), &[]);
        }
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

    /// （cron 合闸）：会话队列里是否已有同一 cron job 的待处
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

    /// 系统通告暂存（2026-11-21 幻影通告根治）：busy 期间通告不排队不回执，
    /// 原文进 stash 等轮边界合并注入。单条超长截断（诚实注记）、条数超限
    /// 丢最旧——通告是信息不是指令，合闸优先于完整保留。
    pub(crate) fn stash_notice(&self, session_key: &str, entry: NoticeEntry) {
        let mut all = self.queues.lock();
        let q = all.entry(session_key.to_string()).or_default();
        let mut entry = entry;
        let mut text = entry.content.trim().to_string();
        if text.chars().count() > NOTICE_STASH_ENTRY_CAP {
            text = text
                .chars()
                .take(NOTICE_STASH_ENTRY_CAP)
                .collect::<String>()
                + " …[通告超长已截断]";
        }
        entry.content = text;
        q.notices.push(entry);
        while q.notices.len() > NOTICE_STASH_CAP {
            q.notices.remove(0);
        }
        self.mirror_session_state(session_key, q);
    }

    /// 排空 stash（轮边界/turn 收口各一处调用）：返回合并产物（最后一条
    /// 通告的来源路由 + 合并文本）；空 stash 返回 None。合并超总量截断并
    /// 注明丢弃条数（不静默）。调用即清空——重复调用天然幂等。
    pub(crate) fn drain_notices_merged(&self, session_key: &str) -> Option<NoticeMerge> {
        let mut all = self.queues.lock();
        let q = all.get_mut(session_key)?;
        if q.notices.is_empty() {
            return None;
        }
        let notices = std::mem::take(&mut q.notices);
        self.mirror_session_state(session_key, q);
        let last = notices.last().cloned();
        last.map(|l| NoticeMerge {
            channel: l.channel,
            chat_id: l.chat_id,
            count: notices.len(),
            content: Self::merge_notice_texts(&notices),
        })
    }

    /// stash 是否有待注入通告（通告闸孤儿 kick 与轮尾 housekeeping 用）。
    pub(crate) fn has_notices(&self, session_key: &str) -> bool {
        self.queues
            .lock()
            .get(session_key)
            .is_some_and(|q| !q.notices.is_empty())
    }

    /// 合并文本（独立纯函数：截断注记逻辑单测可直达）。
    fn merge_notice_texts(notices: &[NoticeEntry]) -> String {
        let joined = notices
            .iter()
            .map(|n| n.content.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        if joined.chars().count() <= NOTICE_MERGE_TOTAL_CAP {
            return joined;
        }
        let mut kept: Vec<&str> = Vec::new();
        let mut used = 0usize;
        for n in notices {
            let len = n.content.chars().count();
            if used + len > NOTICE_MERGE_TOTAL_CAP {
                break;
            }
            used += len;
            kept.push(n.content.as_str());
        }
        let dropped = notices.len() - kept.len();
        kept.join("\n\n") + &format!("\n\n…[另有 {dropped} 条通告因合并超长未注入]")
    }
}

pub type SharedInbox = Arc<Inbox>;

/// 合并通告统一框注（单一真相源）：轮边界注入（claim_steer_messages
/// 通告 drain）与收口合成通告轮（admission::build_merged_notice_message）
/// 共用同一文案——模型对同类输入看到同一形态，不因注入时点分叉。
pub(crate) fn frame_merged_notices(merged: &str) -> String {
    format!(
        "[系统通告] 以下是本回合期间累积的系统通知（合并注入），请结合当前任务跟进：\n\n{merged}"
    )
}

#[cfg(test)]
mod tests;
