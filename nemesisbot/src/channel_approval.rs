//! K4 (b)（devtool-upgrade 阶段 7）：IM 通道审批卡管理器。
//!
//! 实现 `nemesis_security::auditor::ApprovalManager`：auditor 命中
//! `require_approval` 时按来源上下文（[`nemesis_security::auditor::ApprovalContext`]，
//! pipeline 从 `ToolInvocation.metadata` 提取——loop 构造 invocation 时填入）
//! 把审批卡以**普通文本消息**发回发起操作的 IM 对话（telegram/feishu/discord/
//! ... 全通道通用；TG inline keyboard / 飞书互动卡片等 per-channel 富形态是
//! 后续增强，不在本版）。用户在同一对话回复 `/approve <编号>` / `/deny
//! <编号>`，回执 watcher（bus 订阅方）解析并裁决。
//!
//! 消息流（三条订阅方互不干扰）：
//! ```text
//! auditor(request_approval_sync_ctx)
//!   └─ 本管理器: bus.publish_outbound(审批卡) → 通道送达用户
//! 用户回复 /approve <id>
//!   ├─ bus.publish_inbound → AgentLoop gate: parse_approval_reply 静态确认
//!   │   （吞掉，agent 不对回执起对话）
//!   └─ bus.publish_inbound → 本管理器 watcher: 匹配 (channel, chat_id, id)
//!       → 裁决回传（oneshot→mpsc）→ bus.publish_outbound(结果通知)
//! ```
//!
//! watcher 装配纪律：**订阅先于 spawn**（`spawn_watcher`，见其文档）——
//! 任务内订阅存在回执丢失窗口（broadcast 对不存在的订阅者不投递）。
//!
//! 编号 = request_id（uuid）前 8 位；冲突时逐级加长（12/16/全长）。
//! 超时自动拒绝（与 Web 管理器同语义），并向对话发超时通知——诚实且
//! 不悬挂。群聊语义：同 chat 任一成员可批复（v1；sender_id 仅记录）。

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::Duration;

use nemesis_bus::MessageBus;
use nemesis_security::auditor::{ApprovalContext, ApprovalManager, ApprovalVerdict};
use nemesis_types::channel::OutboundMessage;

/// 一条等待裁决的通道审批请求。
struct ChannelPendingEntry {
    /// 来源通道（回执必须同通道——web 里敲 /approve 不到 IM 的审批）。
    channel: String,
    /// 来源对话（回执必须同对话）。
    chat_id: String,
    /// 发起人（v1 仅记录，群聊任一成员可批复）。
    sender_id: String,
    /// P1 审批台账用：全量 request_id（表键 `id` 是短编号/冲突加长编号，
    /// 台账按全量 id 记账、short_id 存表键供 IM 回执重启补裁反查）。
    request_id: String,
    created_at: std::time::Instant,
    tx: mpsc::Sender<ApprovalVerdict>,
}

/// IM 通道审批卡管理器（见模块文档）。
pub struct ChannelApprovalManager {
    bus: std::sync::Arc<MessageBus>,
    pending: Mutex<HashMap<String, ChannelPendingEntry>>,
    /// P1 审批台账（tasks.db approvals 表）。OnceLock 晚绑（web 管理器同
    /// 款时序）；未注入 = 台账关闭（审批照常，仅丢重启补裁能力）。
    journal:
        std::sync::OnceLock<std::sync::Arc<dyn nemesis_types::approval_journal::ApprovalJournal>>,
}

impl ChannelApprovalManager {
    pub fn new(bus: std::sync::Arc<MessageBus>) -> Self {
        Self {
            bus,
            pending: Mutex::new(HashMap::new()),
            journal: std::sync::OnceLock::new(),
        }
    }

    /// P1：注入审批台账（幂等失败 = 重复注入，静默忽略，首个胜出）。
    pub fn set_journal(
        &self,
        journal: std::sync::Arc<dyn nemesis_types::approval_journal::ApprovalJournal>,
    ) {
        let _ = self.journal.set(journal);
    }

    fn journal(
        &self,
    ) -> Option<&std::sync::Arc<dyn nemesis_types::approval_journal::ApprovalJournal>> {
        self.journal.get()
    }

    /// P1：pending 记账（尽力而为，失败 loud error 不阻断审批主链路）。
    fn journal_record_pending(
        &self,
        request_id: &str,
        short_id: &str,
        ctx: &ApprovalContext,
        operation: &str,
        target: &str,
        risk_level: &str,
        reason: &str,
        timeout_secs: u64,
    ) {
        if let Some(j) = self.journal() {
            let entry = nemesis_types::approval_journal::ApprovalJournalEntry {
                id: request_id.to_string(),
                broker: "channel".to_string(),
                short_id: Some(short_id.to_string()),
                operation: operation.to_string(),
                target: target.to_string(),
                risk_level: risk_level.to_string(),
                reason: reason.to_string(),
                payload: serde_json::json!({
                    "channel": ctx.channel,
                    "chat_id": ctx.chat_id,
                    "sender_id": ctx.sender_id,
                    "timeout_secs": timeout_secs,
                }),
                status: nemesis_types::approval_journal::status::PENDING.to_string(),
                created_at: chrono::Local::now().to_rfc3339(),
                decided_at: None,
                decision_note: None,
                decided_by: None,
            };
            if let Err(e) = j.record_pending(&entry) {
                tracing::error!(
                    "[ChannelApproval] approval journal record_pending failed for {request_id}: {e}"
                );
            }
        }
    }

    /// P1：裁决记账。返回「本调用是否赢得裁决状态转移」（R3，2026-10-18
    /// 三方复评：决策侧先记账后通知，按此值拦截败方重复送裁决/发通知）。
    /// 无 journal / 台账 Err = true（审批可用性不绑 tasks.db，裁决照常进行，
    /// 审计缺失 loud 留痕）；`Ok(won)` 如实回传。
    fn journal_record_decision(&self, request_id: &str, approved: bool, decided_by: &str) -> bool {
        let Some(j) = self.journal() else {
            return true;
        };
        let decision = if approved {
            nemesis_types::approval_journal::ApprovalDecision::Approved
        } else {
            nemesis_types::approval_journal::ApprovalDecision::Denied
        };
        match j.record_decision(request_id, decision, None, decided_by) {
            Ok(won) => won,
            Err(e) => {
                tracing::warn!(
                    "[ChannelApproval] approval journal decision record failed for {request_id}: {e}"
                );
                true
            }
        }
    }

    /// 订阅 bus inbound（`spawn_watcher` / 竞态回归测试用）。**订阅必须先于
    /// watcher 首次 poll**：`tokio::spawn` 只入队，若订阅发生在任务内，窗口
    /// 期发布的回执对尚不存在的订阅者直接丢弃（broadcast 语义，bus 侧打
    /// `no inbound receivers` warn）→ ask 等方等满超时被误拒（2026-09-08
    /// 全量实证：channel_card_roundtrip_approve 120s 超时误拒，三次复发根因
    /// 均为此竞态而非负载慢——加宽超时窗口无解）。
    pub fn subscribe_inbound(
        &self,
    ) -> tokio::sync::broadcast::Receiver<nemesis_types::channel::InboundMessage> {
        self.bus.subscribe_inbound()
    }

    /// 回执 watcher 主循环：解析 `/approve|/deny <id>` 回执并裁决。装配一律
    /// 走 [`Self::spawn_watcher`]（订阅先于 spawn 的正确次序封装）——旧的
    /// 「spawn 后任务内订阅」入口已删除，编译器强制所有调用点走正确次序。
    /// Lagged（慢消费者丢消息）只记 warn 继续，仅 Closed 退出——watcher 是
    /// 通道审批回执的唯一消费者，bus 存活期间必须常驻。
    pub async fn watcher_with_rx(
        self: std::sync::Arc<Self>,
        mut rx: tokio::sync::broadcast::Receiver<nemesis_types::channel::InboundMessage>,
    ) {
        loop {
            let msg = match rx.recv().await {
                Ok(msg) => msg,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    // P2-1：接收侧真实丢失计入 bus 指标。丢的可能是审批
                    // /deny 回执——watcher 侧只能继续（审批最终由超时
                    // 自动拒绝兜底），但丢失必须可观测。
                    self.bus.record_inbound_lagged(n);
                    tracing::warn!(
                        "[ChannelApproval] inbound lagged: {n} messages lost, watcher continues"
                    );
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    tracing::info!("[ChannelApproval] inbound bus closed, watcher exiting");
                    break;
                }
            };
            let Some((verb, id)) = parse_reply(&msg.content) else {
                continue;
            };
            // (是否摘牌, 通知文案, 重启补裁动作)。Race=false：等待方已弃等，
            // 摘牌归超时方。`stale_respond = Some(approved)` = 内存无此编号、
            // 可能是重启前遗留——锁外走台账补裁（记账性质，见 respond_stale_im）。
            let (resolved, notice, stale_respond) = {
                let map = match self.pending.lock() {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                match map.get(id) {
                    Some(entry) if entry.channel == msg.channel && entry.chat_id == msg.chat_id => {
                        tracing::debug!(
                            "[ChannelApproval] {} resolved by {} (initiator {})",
                            id,
                            msg.sender_id,
                            entry.sender_id
                        );
                        // R3（2026-10-18 三方复评）：**裁决状态转移以台账条件
                        // 更新为准——先记账、后送裁决**（Web 管理器同序）。
                        // won=false = 超时分支已抢先记账并通知，此处不再送
                        // 裁决/回执（先到者的通知已摘牌）。记账先行的副作用：
                        // 等待方异常退出时台账已持裁决——审计面可恢复，比
                        // 「送了裁决但没记账」的分叉诚实。
                        let won = self.journal_record_decision(
                            &entry.request_id,
                            verb == "approve",
                            &format!("im:{}", msg.sender_id),
                        );
                        if !won {
                            continue;
                        }
                        let verdict = match verb {
                            "approve" => ApprovalVerdict::approved(),
                            _ => ApprovalVerdict::denied(),
                        };
                        if entry.tx.send(verdict).is_ok() {
                            let action = if verb == "approve" {
                                "已批准"
                            } else {
                                "已拒绝"
                            };
                            (
                                true,
                                format!("🔐 审批 {id}：用户{action}，操作继续。"),
                                None,
                            )
                        } else {
                            // 等待方已超时弃等（recv_timeout 侧负责摘牌，
                            // 这里竞态兜底）——不回执。
                            continue;
                        }
                    }
                    Some(_) => (
                        false,
                        format!("⚠ 审批 {id} 不属于本对话，无法在此批复。"),
                        None,
                    ),
                    None => (false, String::new(), Some(verb == "approve")),
                }
            };
            if let Some(approved) = stale_respond {
                let notice = self.respond_stale_im(id, &msg, approved);
                self.bus.publish_outbound(OutboundMessage::new(
                    &msg.channel,
                    &msg.chat_id,
                    &notice,
                ));
                continue;
            }
            if resolved && let Ok(mut map) = self.pending.lock() {
                map.remove(id);
            }
            self.bus
                .publish_outbound(OutboundMessage::new(&msg.channel, &msg.chat_id, &notice));
        }
    }

    /// P1 重启补裁（watcher 的 map-miss 兜底）：内存无此编号 → 台账查
    /// pending。命中且 channel/chat_id 与回执来源一致 → 台账裁决落账
    /// （幂等条件更新先到先得），并诚实告知「原操作已随重启失效，仅记账」；
    /// 不一致维持「不属于本对话」；未命中/已裁决维持原「未找到或已处理」。
    /// 持有 pending 锁期间**不调用**（锁外执行——台账写不阻塞其他回执）。
    fn respond_stale_im(
        &self,
        short_id: &str,
        msg: &nemesis_types::channel::InboundMessage,
        approved: bool,
    ) -> String {
        let Some(j) = self.journal() else {
            return format!("⚠ 未找到待审批请求 {short_id}（可能已超时或已处理）。");
        };
        let Some(entry) = j.find_pending(short_id) else {
            return format!("⚠ 未找到待审批请求 {short_id}（可能已超时或已处理）。");
        };
        let (ch, chat) = match (
            entry.payload.get("channel").and_then(|v| v.as_str()),
            entry.payload.get("chat_id").and_then(|v| v.as_str()),
        ) {
            (Some(c), Some(k)) => (c.to_string(), k.to_string()),
            _ => {
                return format!("⚠ 未找到待审批请求 {short_id}（可能已超时或已处理）。");
            }
        };
        if ch != msg.channel || chat != msg.chat_id {
            return format!("⚠ 审批 {short_id} 不属于本对话，无法在此批复。");
        }
        let decision = if approved {
            nemesis_types::approval_journal::ApprovalDecision::Approved
        } else {
            nemesis_types::approval_journal::ApprovalDecision::Denied
        };
        match j.record_decision(&entry.id, decision, None, &format!("im:{}", msg.sender_id)) {
            Ok(true) => {
                tracing::warn!(
                    "[ChannelApproval] stale reply for {} ({}): gateway restarted since this \
                     approval was raised; decision recorded for audit, original operation is gone",
                    short_id,
                    entry.id
                );
                let action = if approved { "批准" } else { "拒绝" };
                format!("🔐 审批 {short_id}：已{action}（重启前请求，原操作已失效，仅记账留痕）。")
            }
            Ok(false) => format!("⚠ 审批 {short_id} 已处理过。"),
            Err(e) => {
                tracing::error!(
                    "[ChannelApproval] stale reply journal record failed for {short_id}: {e}"
                );
                format!("⚠ 审批 {short_id} 记账失败，请稍后重试或改用 Dashboard。")
            }
        }
    }

    /// 装配点入口：**先订阅再 spawn**——订阅在调用线程同步完成（此刻起消息
    /// 进 broadcast 缓冲，不会丢），主循环任务稍后从缓冲取。gateway 与测试
    /// 统一走这里；racy 的「任务内订阅」入口已删除（见 `watcher_with_rx`）。
    pub fn spawn_watcher(self: &std::sync::Arc<Self>) -> tokio::task::JoinHandle<()> {
        let rx = self.subscribe_inbound();
        let mgr = self.clone();
        tokio::spawn(mgr.watcher_with_rx(rx))
    }

    /// 审批卡文本（纯函数便于测试）。自解释三字段（why/impact/reject_note）
    /// 与 Web 审批卡同源派生（`web_approval::approval_card_fields` 单一
    /// 真相源）——IM 用户看到的「为什么/影响/拒绝后果」与 Dashboard 一致。
    fn render_card(
        id: &str,
        operation: &str,
        target: &str,
        risk: &str,
        reason: &str,
        timeout: u64,
    ) -> String {
        let (why, impact, reject_note) =
            crate::web_approval::approval_card_fields(operation, target, risk, reason);
        format!(
            "🔐 编码审批请求\n编号: {id}\n操作: {operation}\n目标: {target}\n风险: {risk}\n\
             为什么: {why}\n影响: {impact}\n拒绝后果: {reject_note}\n\n\
             回复 /approve {id} 批准，/deny {id} 拒绝（{timeout} 秒内有效，超时自动拒绝）。"
        )
    }
}

impl ApprovalManager for ChannelApprovalManager {
    fn is_running(&self) -> bool {
        // bus 句柄恒可用；watcher 是否 spawn 不影响卡片送达（裁决需
        // watcher，但装配点保证同时启动）。
        true
    }

    fn request_approval_sync(
        &self,
        _request_id: &str,
        _operation: &str,
        _target: &str,
        _risk_level: &str,
        _reason: &str,
        _timeout_secs: u64,
    ) -> Result<ApprovalVerdict, String> {
        // 编程错误信号：无来源上下文不该路由到本管理器（组合管理器负责
        // 分流）。诚实报错而非猜默认。
        Err(
            "channel approval manager requires origin context; route via request_approval_sync_ctx"
                .to_string(),
        )
    }

    fn request_approval_sync_ctx(
        &self,
        request_id: &str,
        operation: &str,
        target: &str,
        risk_level: &str,
        reason: &str,
        timeout_secs: u64,
        ctx: &ApprovalContext,
    ) -> Result<ApprovalVerdict, String> {
        if ctx.chat_id.is_empty() {
            return Err(
                "approval context has empty chat_id; cannot address approval card".to_string(),
            );
        }

        // 编号 + 摘牌：单锁作用域内定编号并挂表。**先挂表、后发卡**——若
        // 先发卡，极快回执会在 watcher 查表时 entry 尚未入表，落「未找到」
        // 分支吞掉裁决，ask 只能等满超时被误拒（测试实证：channel_card_
        // roundtrip_approve 在全量并行负载下 30s 超时误拒，2026-09-07）。
        // entry 就位后再发卡，卡片到达时刻起任何回执都可被正确裁决。
        let mut id_len = 8usize.min(request_id.len()).max(1);
        let (id, rx) = {
            let mut map = match self.pending.lock() {
                Ok(m) => m,
                Err(_) => return Err("channel approval pending map poisoned".to_string()),
            };
            let id = loop {
                let candidate = &request_id[..id_len];
                if !map.contains_key(candidate) || id_len >= request_id.len() {
                    break candidate.to_string();
                }
                id_len = (id_len * 2).min(request_id.len());
            };
            let (tx, rx) = mpsc::channel::<ApprovalVerdict>();
            map.insert(
                id.clone(),
                ChannelPendingEntry {
                    channel: ctx.channel.clone(),
                    chat_id: ctx.chat_id.clone(),
                    sender_id: ctx.sender_id.clone(),
                    request_id: request_id.to_string(),
                    created_at: std::time::Instant::now(),
                    tx,
                },
            );
            (id, rx)
        };

        // P1：审批台账记账（entry 已入表；尽力而为失败 loud error 不阻断）。
        self.journal_record_pending(
            request_id,
            &id,
            ctx,
            operation,
            target,
            risk_level,
            reason,
            timeout_secs,
        );

        // 审批卡送达发起对话（entry 已入表，快回执可被 watcher 正确裁决）。
        let card = Self::render_card(&id, operation, target, risk_level, reason, timeout_secs);
        self.bus
            .publish_outbound(OutboundMessage::new(&ctx.channel, &ctx.chat_id, &card));

        // 阻塞等裁决（Web 管理器同款：tokio 上下文 block_in_place 让出
        // worker，同步上下文直接 recv）。
        let wait = || rx.recv_timeout(Duration::from_secs(timeout_secs));
        let result = if tokio::runtime::Handle::try_current().is_ok() {
            tokio::task::block_in_place(wait)
        } else {
            wait()
        };

        if let Ok(mut map) = self.pending.lock()
            && let Some(entry) = map.remove(&id)
        {
            tracing::debug!(
                "[ChannelApproval] entry {} removed after wait (waited {:?})",
                id,
                entry.created_at.elapsed()
            );
        }

        match result {
            Ok(verdict) => {
                tracing::info!(
                    "[ChannelApproval] request {} ({}): {}",
                    id,
                    operation,
                    if verdict.approved {
                        "approved"
                    } else {
                        "denied"
                    }
                );
                Ok(verdict)
            }
            Err(_) => {
                tracing::warn!(
                    "[ChannelApproval] request {} ({}) timed out after {}s — denying",
                    id,
                    operation,
                    timeout_secs
                );
                // R3（2026-10-18 三方复评）：**先记台账**（幂等条件更新），
                // 再按记账结果决定后续——Ok(false) = 回执抢先完成裁决（watcher
                // 已记账并通知），不能顺着超时路径发假超时通知。
                let expired_record = self.journal().map(|j| {
                    j.record_decision(
                        request_id,
                        nemesis_types::approval_journal::ApprovalDecision::Expired,
                        Some("审批超时自动拒绝"),
                        "timeout",
                    )
                });
                match expired_record {
                    Some(Ok(false)) => {
                        // 回执赢了竞争：此刻大概率已 tx.send（记账后微秒级）
                        // ——短暂宽限收一次在途裁决恢复真实结果；不补发超时
                        // 通知（回执侧通知已摘牌）。宽限耗尽/通道关闭 = 回执
                        // 侧异常中断：诚实返回拒绝、不补假通知。
                        tracing::info!(
                            "[ChannelApproval] request {id}: reply won the decision race; recovering in-flight verdict"
                        );
                        match rx.recv_timeout(Duration::from_secs(2)) {
                            Ok(verdict) => Ok(verdict),
                            Err(_) => {
                                tracing::warn!(
                                    "[ChannelApproval] request {id}: raced decision not recoverable from channel; returning denial without timeout notice"
                                );
                                Ok(ApprovalVerdict::denied())
                            }
                        }
                    }
                    other => {
                        if let Some(Err(e)) = other {
                            tracing::error!(
                                "[ChannelApproval] approval journal timeout record failed for {request_id}: {e}"
                            );
                        }
                        // None = 无 journal（审批可用性不绑 tasks.db）；Ok(true)
                        // = 正常超时裁决。两者都通知摘牌（诚实不悬挂）。
                        // 超时自动拒绝也是一次裁决——通知对话摘牌（诚实不悬挂）。
                        self.bus.publish_outbound(OutboundMessage::new(
                            &ctx.channel,
                            &ctx.chat_id,
                            &format!("⌛ 审批 {id} 已超时，操作已自动拒绝。"),
                        ));
                        Ok(ApprovalVerdict::denied())
                    }
                }
            }
        }
    }
}

/// 回执语法解析（单一真相源 = `nemesis_web::remote_approval::parse_approval_reply`
/// ——M5 起看板评论回源共用同一格式；此处只做 verb 形态适配）。返回 (verb, id)。
fn parse_reply(content: &str) -> Option<(&'static str, &str)> {
    #[cfg(feature = "cluster")]
    {
        nemesis_web::remote_approval::parse_approval_reply(content)
            .map(|(approved, id)| (if approved { "approve" } else { "deny" }, id))
    }
    #[cfg(not(feature = "cluster"))]
    {
        let trimmed = content.trim();
        if let Some(rest) = trimmed.strip_prefix("/approve ") {
            let id = rest.trim();
            (!id.is_empty() && !id.chars().any(char::is_whitespace)).then_some(("approve", id))
        } else if let Some(rest) = trimmed.strip_prefix("/deny ") {
            let id = rest.trim();
            (!id.is_empty() && !id.chars().any(char::is_whitespace)).then_some(("deny", id))
        } else {
            None
        }
    }
}

/// M5-3（2026-10-10 机制加固）：cluster 通道 ask 的看板可见化通知载荷。
/// 触发点 = [`CompositeApprovalManager::request_approval_sync_ctx`] 命中
/// `channel == "cluster"` 的 ask（worker 执行看板任务时的 ABAC 审批）。
#[derive(Debug, Clone)]
#[cfg_attr(not(feature = "cluster"), allow(dead_code))]
pub struct BoardAskNotice {
    pub request_id: String,
    pub operation: String,
    pub target: String,
    pub risk_level: String,
    pub reason: String,
    pub timeout_secs: u64,
    /// 触发 ask 的 cluster 会话键（worker 侧形态
    /// `cluster_rpc:{source_node}/board:{n}`——派发 chat_id 恒为裸数字
    /// 单号，含单号可反查 issue；parser 容忍旧 `board:NB-{n}` 形态）。
    pub chat_id: String,
}

/// 组合审批管理器：按来源上下文分流——web/无上下文 → 默认管理器（现状：
/// Dashboard 卡片），IM 通道 → [`ChannelApprovalManager`]；cluster 通道 →
/// 默认管理器 + [`BoardAskNotice`] 通知（M5-3：审批卡落本节点 Dashboard
/// SSE 可见 + 看板注记，不再无声等超时）。
pub struct CompositeApprovalManager {
    default: std::sync::Arc<dyn ApprovalManager>,
    channel: std::sync::Arc<ChannelApprovalManager>,
    /// M5-3：cluster ask 观察者（gateway 装配时安装；None = 只改路由不通知）。
    board_notifier: std::sync::Mutex<Option<std::sync::Arc<dyn Fn(&BoardAskNotice) + Send + Sync>>>,
}

impl CompositeApprovalManager {
    pub fn new(
        default: std::sync::Arc<dyn ApprovalManager>,
        channel: std::sync::Arc<ChannelApprovalManager>,
    ) -> Self {
        Self {
            default,
            channel,
            board_notifier: std::sync::Mutex::new(None),
        }
    }

    /// M5-3：安装 cluster ask 观察者（gateway 装配调用一次；重复安装覆盖
    /// ——同刻只有一个 gateway 装配流程，覆盖无实害）。
    #[cfg_attr(not(feature = "cluster"), allow(dead_code))]
    pub fn set_board_notifier(
        &self,
        notifier: std::sync::Arc<dyn Fn(&BoardAskNotice) + Send + Sync>,
    ) {
        *self
            .board_notifier
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(notifier);
    }

    /// M5-3：fire 观察者（通知失败不阻塞审批等待——可见化是旁路不是主路径）。
    fn notify_board(&self, notice: &BoardAskNotice) {
        if let Ok(guard) = self.board_notifier.lock()
            && let Some(f) = guard.as_ref()
        {
            f(notice);
        }
    }
}

impl ApprovalManager for CompositeApprovalManager {
    fn is_running(&self) -> bool {
        self.default.is_running() || self.channel.is_running()
    }

    fn request_approval_sync(
        &self,
        request_id: &str,
        operation: &str,
        target: &str,
        risk_level: &str,
        reason: &str,
        timeout_secs: u64,
    ) -> Result<ApprovalVerdict, String> {
        // 无上下文的 ask（旧调用方）→ 默认管理器（现状不变）。
        self.default.request_approval_sync(
            request_id,
            operation,
            target,
            risk_level,
            reason,
            timeout_secs,
        )
    }

    fn request_approval_sync_ctx(
        &self,
        request_id: &str,
        operation: &str,
        target: &str,
        risk_level: &str,
        reason: &str,
        timeout_secs: u64,
        ctx: &ApprovalContext,
    ) -> Result<ApprovalVerdict, String> {
        // M5-3（2026-10-10 机制加固）：cluster 通道（worker 执行看板任务
        // 的 ABAC ask）此前落通道卡片——"cluster" 通道无出站投递面，卡静默
        // 丢弃、ask 无声等满超时被拒。改走默认管理器（本节点 Dashboard
        // SSE 审批卡，人可当场批复）+ fire 看板注记观察者（master issue
        // 线程留痕）。
        if ctx.channel == "cluster" {
            self.notify_board(&BoardAskNotice {
                request_id: request_id.to_string(),
                operation: operation.to_string(),
                target: target.to_string(),
                risk_level: risk_level.to_string(),
                reason: reason.to_string(),
                timeout_secs,
                chat_id: ctx.chat_id.clone(),
            });
            return self.default.request_approval_sync(
                request_id,
                operation,
                target,
                risk_level,
                reason,
                timeout_secs,
            );
        }
        // web 通道 = Dashboard 卡片（默认管理器的领地）；空 channel/chat =
        // 无可寻址对话 → 默认管理器。其余（telegram/feishu/discord/...）
        // → 通道卡片。
        if ctx.channel.is_empty() || ctx.chat_id.is_empty() || ctx.channel == "web" {
            self.default.request_approval_sync(
                request_id,
                operation,
                target,
                risk_level,
                reason,
                timeout_secs,
            )
        } else {
            self.channel.request_approval_sync_ctx(
                request_id,
                operation,
                target,
                risk_level,
                reason,
                timeout_secs,
                ctx,
            )
        }
    }
}

#[cfg(test)]
mod tests;
