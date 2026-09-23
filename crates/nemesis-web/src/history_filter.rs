//! 只读历史查询过滤器（入站过滤链的第一个真实过滤器）。
//!
//! 拦截 `request_type="history"` 的入站消息，**就地**读主 workspace 的
//! chat_log 并经 bus 出站应答——消息不再进入 bus 扇出，与任何 agent
//! loop 的存亡/忙闲彻底解耦（BUG 2026-09-23 项目会话历史加载修复）：
//!
//! - 场景 A（项目目录缺失）：项目 loop 从未启动，历史照样可读——数据
//!   本就全在主 workspace `logs/session_logs/`，项目归属不再能把只读
//!   历史请求劫持成「项目不可用」错误；
//! - 场景 B（reject 模式 turn 进行中）：history 不再排进项目 loop 的
//!   串行队列，10s 前端围栏内必然应答。
//!
//! 解析/组装走 [`nemesis_agent::history`] 共享件（与 loop 内路径
//! `handle_history_request` 同源，防漂移）；transport 走
//! `bus.publish_outbound`（message_type="history"），与 loop 的
//! outbound_tx 在 gateway 出站桥**同点汇合**，投递语义不变。

use nemesis_agent::history::{HistoryRequest, history_response_json};
use nemesis_bus::{Filter, FilterDecision};
use nemesis_types::channel::{InboundMessage, OutboundMessage};
use tracing::{error, info, warn};

pub struct HistoryFilter {
    bus: std::sync::Arc<nemesis_bus::MessageBus>,
}

impl HistoryFilter {
    pub fn new(bus: std::sync::Arc<nemesis_bus::MessageBus>) -> Self {
        Self { bus }
    }

    /// 就地服务一条 history 请求（解析 → 读页 → 采样 → 组装 → 出站）。
    async fn serve(&self, msg: &InboundMessage) {
        let req = match HistoryRequest::parse(&msg.content) {
            Ok(r) => r,
            Err(e) => {
                // 与 loop 内路径同形态：诚实回空页（request_id=""、
                // session_id=None → 空串），不静默丢。
                warn!("[HistoryFilter] Failed to parse history request: {}", e);
                self.publish(
                    msg.chat_id.clone(),
                    "",
                    &Vec::<serde_json::Value>::new(),
                    false,
                    0,
                    0,
                    None,
                    0,
                );
                return;
            }
        };

        // HD（2026-09-17）：session_id 原样回显——前端按归属丢弃迟到响应。
        let req_session_id = msg
            .metadata
            .get("session_id")
            .map(|s| s.as_str())
            .unwrap_or("")
            .to_string();

        // session_key：web 咽喉点已按 `agent:main:session:{sanitize(sid)}`
        // （legacy 兜底）烙入 InboundMessage——与 loop 内 handle_history_request
        // 的推导公式同源（loop.rs 注释明示 MUST match），直接取用。空键
        // （非 web 直发 bus 的假想发布方）回落 loop 同款推导，语义不漂。
        let session_key = if msg.session_key.is_empty() {
            match msg.metadata.get("session_id") {
                Some(sid) if !sid.is_empty() => format!(
                    "agent:main:session:{}",
                    nemesis_agent::session::SessionStore::sanitize_session_id(sid)
                ),
                _ => "agent:main:session:legacy".to_string(),
            }
        } else {
            msg.session_key.clone()
        };

        // 读历史（_async：阻塞读移出 tokio worker，BUG 2026-09-22）。
        let (page, total_count, has_more, oldest_index) =
            nemesis_agent::chat_log::read_chat_log_async(
                &session_key,
                req.effective_limit(),
                req.before_index,
            )
            .await;

        // A1（2026-09-22）：历史读取**之后**采样环尾 seq（与 loop 内路径
        // 同序——剔除推断的前提是 assistant 入环点在 chat_log 落盘之后）。
        let last_seq = crate::chat_event_log::latest_seq(&session_key);

        let intercepted = self.publish(
            msg.chat_id.clone(),
            &req.request_id,
            &page,
            has_more,
            oldest_index,
            total_count,
            Some(&req_session_id),
            last_seq,
        );
        if intercepted {
            info!(
                session_key = %session_key,
                total_count = total_count,
                "[HistoryFilter] history served in-place（未进入 bus 扇出）"
            );
        }
    }

    /// 组装并发布出站响应。返回 false = 组装失败（已 error!，无帧可发）。
    fn publish(
        &self,
        chat_id: String,
        request_id: &str,
        messages: &[serde_json::Value],
        has_more: bool,
        oldest_index: usize,
        total_count: usize,
        session_id: Option<&str>,
        last_seq: u64,
    ) -> bool {
        let Some(content) = history_response_json(
            request_id,
            messages,
            has_more,
            oldest_index,
            total_count,
            session_id,
            last_seq,
        ) else {
            error!("[HistoryFilter] Failed to marshal history response");
            return false;
        };
        self.bus.publish_outbound(OutboundMessage {
            channel: "web".to_string(),
            chat_id,
            content,
            message_type: "history".to_string(),
            meta: Default::default(),
        });
        true
    }
}

#[async_trait::async_trait]
impl Filter<InboundMessage> for HistoryFilter {
    fn name(&self) -> &'static str {
        "history"
    }

    /// 第一个真实过滤器；`project-route`（ProjectLoopManager 未来迁入）预占 200。
    fn priority(&self) -> i32 {
        100
    }

    async fn inspect(&self, msg: &InboundMessage) -> FilterDecision {
        if msg.metadata.get("request_type").map(String::as_str) != Some("history") {
            return FilterDecision::Pass;
        }
        self.serve(msg).await;
        FilterDecision::Intercepted
    }
}
