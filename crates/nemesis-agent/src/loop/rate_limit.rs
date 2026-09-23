//! 轮内预算/限流重试常量与 429 退避梯子（裁决②③④⑧）、RateLimitStatus、限流状态 set/clear/retry_status/send_retry_progress。
//!
//! P1 自 `loop.rs` 物理搬迁（docs/PLAN/2026-09-23_agentloop-god-object-decomposition.md §3.2）；语义零变化。
use super::prelude::*;
use super::*;

/// Grace-round nudge injected when the tool-call budget is exhausted (②).
/// The model gets one extra round to synthesize a final answer from the work
/// already done, instead of hard-stopping with "Max iterations reached". This
/// is a TRANSIENT system message — appended to the built message list for the
/// grace round only, never persisted to instance history or session_log.
pub(crate) const GRACE_ROUND_NUDGE: &str = "工具调用预算已用尽，不要再调用任何工具。请基于已完成的工作给出最终答复：总结完成了什么、还有什么没做、需要用户做哪些决定。";

/// Max retries for transient LLM errors (network / stream / 5xx) before giving
/// up (③). Retries do NOT consume the `turns_used` budget — the increment at
/// the end of an iteration happens once regardless of how many retries it took
/// to get a successful response.
pub(crate) const MAX_TRANSIENT_RETRIES: u32 = 3;

/// 429 限流重试环（2026-09-17 BUG 文档裁决④⑧）：限流重试间隔阶梯（秒）——
/// 前 4 档翻倍爬升抓瞬时抖动，后 6 档每档 +30s 线性放宽等限流窗口过去；
/// 总跨度 885s ≈ 14.8 分钟。上游带 Retry-After 时取 max(上游要求, 阶梯值)
/// （裁决⑧遵从上游，但不低于本地阶梯）。
const RATE_LIMIT_BACKOFF_LADDER: [u64; 10] = [5, 10, 20, 40, 60, 90, 120, 150, 180, 210];

/// 限流分类词表：`rate limited by provider` = FailoverError::RateLimit 的
/// Display 前缀（llm_bridge 保真展平）；`429` / `too many requests` 兜底
/// 裸文本形态（对齐 providers 侧 error_classifier 口径）；`overloaded` =
/// FailoverError::Overloaded 的 Display 尾词（502/503 过载，状态码在展平
/// 时已丢失）——providers 侧既有约定就是按限流对待（error_classifier
/// 「Overloaded treated as rate_limit」），loop 层词表必须同步对齐，否则
/// 过载首败即终局（2026-09-20 BUG 实证：provider codex is overloaded 一次
/// 报死，复杂长链路任一轮踩 503 即全任务报废）。
pub(crate) const RATE_LIMIT_ERROR_KEYWORDS: [&str; 4] = [
    "rate limited by provider",
    "429",
    "too many requests",
    "overloaded",
];

/// 阶梯取值（attempt 从 1 起计；超出阶梯长度取末档）。
pub(crate) fn rate_limit_ladder_secs(attempt: u32) -> u64 {
    RATE_LIMIT_BACKOFF_LADDER[(attempt as usize)
        .saturating_sub(1)
        .min(RATE_LIMIT_BACKOFF_LADDER.len() - 1)]
}

/// 从错误文本提取 ProviderAdapter 折进的 `(retry_after=Ns)` 后缀。
pub(crate) fn extract_retry_after_secs(err: &str) -> Option<u64> {
    let idx = err.find("(retry_after=")?;
    let rest = &err[idx + "(retry_after=".len()..];
    let end = rest.find(|c: char| !c.is_ascii_digit())?;
    rest[..end].parse().ok()
}

/// 限流等待秒数：max(Retry-After, 阶梯值)——上游要求更长就等更长。
pub(crate) fn rate_limit_wait_secs(err: &str, attempt: u32) -> u64 {
    extract_retry_after_secs(err).map_or_else(
        || rate_limit_ladder_secs(attempt),
        |ra| ra.max(rate_limit_ladder_secs(attempt)),
    )
}

/// BUG 2026-09-21 ②：单次上游调用超时默认 180s——429 后连接挂起（不回
/// 响应体）时旧行为可挂任意久（实测 27 分钟缺口）。0 = 关闭超时（退回
/// 旧行为）。`agents.defaults.provider_call_timeout_secs` 覆盖。
pub const DEFAULT_PROVIDER_CALL_TIMEOUT_SECS: u64 = 180;
/// BUG 2026-09-21 ②：限流重试环总预算默认 900s（等待 + 调用累计）——
/// 10 次上限只限次数不限总时长（实测 46 分钟无熔断）。0 = 不限。
/// `agents.defaults.rate_limit_budget_secs` 覆盖。
pub const DEFAULT_RATE_LIMIT_BUDGET_SECS: u64 = 900;
/// [`AgentLoop::retry_status`] 读侧过期阈值：写侧在 turn 被 abort（E-STOP
/// /进程取消）时来不及清——快照超时未更新即视为失效，读侧顺手回收。
pub const RETRY_STATUS_STALE_SECS: u64 = 300;

/// BUG 2026-09-21 ①：单会话限流重试的实时快照（WSAPI `agent.retry_status`
/// 读，前端切回会话后占位区显示「第 N/M 次重试」而非哑转圈）。
#[derive(Debug, Clone)]
pub struct RateLimitStatus {
    pub retry: u32,
    pub max_retries: u32,
    pub wait_secs: u64,
    pub model: String,
    /// 写入时刻（读侧判过期用 [`RETRY_STATUS_STALE_SECS`]）。
    pub updated_at: std::time::Instant,
}

impl AgentLoop {
    /// BUG 2026-09-21 ①：重试环写实时快照（每轮覆盖）。
    pub(crate) fn set_rate_limit_status(&self, session_key: &str, st: RateLimitStatus) {
        self.rate_limit_status
            .lock()
            .insert(session_key.to_string(), st);
    }

    /// BUG 2026-09-21 ①：turn 收尾（成功/耗尽/预算终局）清快照。
    pub(crate) fn clear_rate_limit_status(&self, session_key: &str) {
        self.rate_limit_status.lock().remove(session_key);
    }

    /// BUG 2026-09-21 ①：WSAPI 读取口。超 [`RETRY_STATUS_STALE_SECS`] 未更新
    /// 的快照视为失效（E-STOP abort 清不掉的残留）——返回 None 并顺手回收。
    pub fn retry_status(&self, session_key: &str) -> Option<RateLimitStatus> {
        let map = self.rate_limit_status.lock();
        if let Some(st) = map.get(session_key)
            && st.updated_at.elapsed() <= std::time::Duration::from_secs(RETRY_STATUS_STALE_SECS)
        {
            return Some(st.clone());
        }
        drop(map);
        self.clear_rate_limit_status(session_key);
        None
    }

    /// 429 重试进度对用户可见（裁决③「显式进度」）——经 outbound_tx 直发
    /// 一条状态文本；不置 sent_in_round、不影响终局回复发布。tx 未装配
    /// （B 端 worker 等场景）静默重试。
    pub(crate) async fn send_retry_progress(&self, context: &RequestContext, text: String) {
        if let Some(ref tx) = self.outbound_tx {
            let outbound = nemesis_types::channel::OutboundMessage {
                channel: context.channel.clone(),
                chat_id: context.chat_id.clone(),
                content: text,
                message_type: String::new(),
                meta: nemesis_types::channel::OutboundMeta {
                    model: Some(self.current_display_model()),
                    // L2：会话键随行（web 通道按会话记录，进度条也进历史）。
                    session_key: (!context.session_key.is_empty())
                        .then(|| context.session_key.clone()),
                    source_node: None,
                },
            };
            if let Err(e) = tx.send(outbound).await {
                warn!("[AgentLoop] Failed to send retry progress: {}", e);
            }
        }
    }
}
