//! history 请求/响应的共享组装件（单一真相源）。
//!
//! `chat.history_request` 的解析与 `history_response` 的 JSON 组装此前
//! 只活在 [`crate::r#loop::AgentLoop`] 内（loop.rs handle_history_request /
//! publish_history_response）。入站过滤链落地（BUG 2026-09-23 项目会话
//! 历史加载修复）后，web 咽喉点的 `HistoryFilter` 与 loop 内路径共用
//! 同一份组装逻辑——**搬移非重写**：字段集、session_id 回显（HD
//! 2026-09-17）、last_seq>0 才下发的省略规则（A1 2026-09-22）原样保留，
//! 后续任何字段演进只改这里，两条路径不可能漂移。
//!
//! transport 不在此层：loop 路径走 `outbound_tx`，过滤路径走
//! `bus.publish_outbound`——两者在 bus 出站汇合（gateway 的 agent 出站
//! 桥同样汇入 `publish_outbound`），投递语义一致。

use serde::Deserialize;

/// `chat.history_request` 的 data 载荷（与 websocket_handler 侧
/// `HistoryReqData` 字段一致；content 是该载荷的 JSON 文本）。
#[derive(Debug, Clone, Deserialize)]
pub struct HistoryRequest {
    #[serde(default)]
    pub request_id: String,
    #[serde(default)]
    pub limit: Option<usize>,
    pub before_index: Option<usize>,
}

impl HistoryRequest {
    /// 解析请求载荷。解析失败由调用方按各自通道诚实回错（loop 路径回
    /// 空页响应，过滤路径同形态）。
    pub fn parse(content: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(content)
    }

    /// 生效读取条数（缺省 20，与 loop 路径历史行为一致）。
    pub fn effective_limit(&self) -> usize {
        self.limit.unwrap_or(20)
    }
}

/// history 响应 JSON 组装（**唯一组装点**）。
///
/// - `session_id`：会话归属回显——前端与当前选中会话比对，快速切换时
///   迟到响应按归属丢弃防串台（HD）。`None` = 解析失败路径（回空串，
///   前端按无归属放行，不因错误响应丢帧）。
/// - `last_seq`：历史读取时刻的环尾 seq——`0` = 未注入回调/环空，序列化
///   时**省略**该字段（前端按无 last_seq 走兜底，旧前端零影响）。
///
/// 返回 `None` 仅当序列化失败（理论不可达：输入均为可序列化类型）；
/// 调用方 error! 后放弃发送。
pub fn history_response_json(
    request_id: &str,
    messages: &[serde_json::Value],
    has_more: bool,
    oldest_index: usize,
    total_count: usize,
    session_id: Option<&str>,
    last_seq: u64,
) -> Option<String> {
    let mut response_data = serde_json::json!({
        "request_id": request_id,
        "messages": messages,
        "has_more": has_more,
        "oldest_index": oldest_index,
        "total_count": total_count,
        "session_id": session_id.unwrap_or(""),
    });
    if last_seq > 0 {
        response_data["last_seq"] = serde_json::Value::from(last_seq);
    }
    serde_json::to_string(&response_data).ok()
}
