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
    created_at: std::time::Instant,
    tx: mpsc::Sender<ApprovalVerdict>,
}

/// IM 通道审批卡管理器（见模块文档）。
pub struct ChannelApprovalManager {
    bus: std::sync::Arc<MessageBus>,
    pending: Mutex<HashMap<String, ChannelPendingEntry>>,
}

impl ChannelApprovalManager {
    pub fn new(bus: std::sync::Arc<MessageBus>) -> Self {
        Self {
            bus,
            pending: Mutex::new(HashMap::new()),
        }
    }

    /// 回执 watcher：订阅 bus inbound，解析 `/approve|/deny <id>` 回执并
    /// 裁决。gateway 在装配时 spawn（`tokio::spawn(mgr.clone().watcher())`）。
    pub async fn watcher(self: std::sync::Arc<Self>) {
        let mut rx = self.bus.subscribe_inbound();
        while let Ok(msg) = rx.recv().await {
            let Some((verb, id)) = parse_reply(&msg.content) else {
                continue;
            };
            // (是否摘牌, 通知文案)。Race=false：等待方已弃等，摘牌归超时方。
            let (resolved, notice) = {
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
                            (true, format!("🔐 审批 {id}：用户{action}，操作继续。"))
                        } else {
                            // 等待方已超时弃等（recv_timeout 侧负责摘牌，
                            // 这里竞态兜底）——不回执。
                            continue;
                        }
                    }
                    Some(_) => (false, format!("⚠ 审批 {id} 不属于本对话，无法在此批复。")),
                    None => (
                        false,
                        format!("⚠ 未找到待审批请求 {id}（可能已超时或已处理）。"),
                    ),
                }
            };
            if resolved && let Ok(mut map) = self.pending.lock() {
                map.remove(id);
            }
            self.bus
                .publish_outbound(OutboundMessage::new(&msg.channel, &msg.chat_id, &notice));
        }
    }

    /// 审批卡文本（纯函数便于测试）。
    fn render_card(
        id: &str,
        operation: &str,
        target: &str,
        risk: &str,
        reason: &str,
        timeout: u64,
    ) -> String {
        format!(
            "🔐 编码审批请求\n编号: {id}\n操作: {operation}\n目标: {target}\n风险: {risk}\n原因: {reason}\n\n\
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
                    created_at: std::time::Instant::now(),
                    tx,
                },
            );
            (id, rx)
        };

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

/// 回执语法解析（与 loop 侧 `parse_approval_reply` 同源格式：
/// `/approve <id>` / `/deny <id>`）。返回 (verb, id)。
fn parse_reply(content: &str) -> Option<(&'static str, &str)> {
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

/// 组合审批管理器：按来源上下文分流——web/无上下文 → 默认管理器（现状：
/// Dashboard 卡片），IM 通道 → [`ChannelApprovalManager`]。
pub struct CompositeApprovalManager {
    default: std::sync::Arc<dyn ApprovalManager>,
    channel: std::sync::Arc<ChannelApprovalManager>,
}

impl CompositeApprovalManager {
    pub fn new(
        default: std::sync::Arc<dyn ApprovalManager>,
        channel: std::sync::Arc<ChannelApprovalManager>,
    ) -> Self {
        Self { default, channel }
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
