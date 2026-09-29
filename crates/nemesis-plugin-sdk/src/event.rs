//! 观察者事件投影（宿主脱敏投影的反序列化形态 + 助手）。

use serde::{Deserialize, Serialize};

/// 观察者事件投影。
///
/// v1 投递的事件型：`tool_start` / `tool_end`（对齐 agent 工具事件广播）；
/// `turn` / `final` / `error` 为 schema 预留（v1 不投递——宿主无对应广播
/// 承载形态，挂 v2）。未知 `kind` 值原样保留：宿主按合同 minor 演进新增
/// 事件型时，旧插件解析不炸（结构化字段缺省）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObserverEvent {
    /// 事件型（`tool_start` / `tool_end` / 预留型原文）。
    #[serde(rename = "type")]
    pub kind: String,
    /// Unix 毫秒（宿主投递时刻）。
    pub ts_ms: u64,
    /// 会话键（无会话上下文为空串）。
    #[serde(default)]
    pub session_key: String,
    /// 工具名（tool_* 事件）。
    #[serde(default)]
    pub tool: String,
    /// 调用 ID（tool_* 事件）。
    #[serde(default)]
    pub call_id: String,
    /// 执行时长毫秒（tool_end）。
    #[serde(default)]
    pub duration_ms: u64,
    /// 成败（tool_end）。
    #[serde(default)]
    pub ok: bool,
    /// 错误摘要（tool_end 失败时；宿主截断定长，非内容体）。
    #[serde(default)]
    pub error: String,
}

/// 解析事件投影（`Err` = 载荷非合法 JSON 或字段类型不匹配；观察者可自行
/// 降级忽略单条事件——trap 不影响宿主投递循环，只计入丢弃统计）。
pub fn parse_event(event_json: &str) -> Result<ObserverEvent, serde_json::Error> {
    serde_json::from_str(event_json)
}
