//! LLM 代理请求用量记账（代理请求用量记账）。
//!
//! 入站 LLM 代理面的两个裸转发端点（`/v1/chat/completions` 带 tools lane、
//! `/api/chat/stream`）不经过 AgentLoop，organ 5（loop observer 的每轮
//! 记账）不覆盖——usage 只透传给调用方，本机 usage.db 零记录。本模块是
//! 这两个端点的落账出口：在转发完成点把上游透传的 usage 写进
//! `insert_request_log` 单一漏斗，session_key 带 `proxy:` 前缀供使用统计
//! 页按会话筛出（汇总/趋势自动包含，前端零改动）。
//!
//! 诚实边界：
//! - 不带 `tools` 的 detached lane（`subagent:openai-api:*`）走 organ 5
//!   已记账，**不得**调用本模块（否则双记）。
//! - SSE client 中途断开时 async_stream 被 drop，无收尾点——该请求丢账
//!   （v1 接受；上游终帧 / 上游错误 / 无终态路径均有记录点）。
//! - `data_store` 未装配（usage-store 关）= no-op；记账失败只 warn，
//!   绝不影响代理响应（记账是旁路，不是关键路径）。
//! - 与 organ 5 的刻意偏差：上游错误也记一笔（status=500、token 全 0、
//!   error_message 在账）——代理错误本身是值得观测的请求事件。
//! - 与 organ 5 的另一处差异：provider_type 恒空串（同 organ 5 口径，
//!   该列自 起未再消费）。

use crate::api_handlers::AppState;
use nemesis_providers::types::UsageInfo;

/// 一次代理转发的记账载荷（调用点持有的一切转发事实）。
pub(crate) struct ProxyUsageRecord<'a> {
    /// 实际执行模型（账本不变量：model 记真实调用模型，计价才准；
    /// 调用方点名的 model 只在响应里回显，不进账本）。
    pub execution_model: &'a str,
    /// 上游透传 usage，不造数；None = token 全 0。
    pub usage: Option<&'a UsageInfo>,
    pub latency_ms: u64,
    pub is_streaming: bool,
    /// 200 = 转发成功；500 = 上游错误 / 断流 / 无终态。
    pub status_code: i32,
    pub error_message: Option<&'a str>,
    /// `proxy:openai-api:{uuid}` / `proxy:chat-stream:{uuid}`（§2.4 命名）。
    pub session_key: String,
    /// `openai-proxy-{uuid simple}` / `chat-stream-proxy-{uuid simple}`。
    pub trace_id: String,
}

/// 落账单一出口。`ds` 传 `state.data_store.as_deref()`——async_stream
/// 块内借不了 `&AppState`（'static 约束），调用方 clone
/// `Option<Arc<DataStore>>` 进块后 `.as_deref()`。
pub(crate) fn record_proxy_usage(ds: Option<&nemesis_data::DataStore>, rec: ProxyUsageRecord<'_>) {
    let Some(ds) = ds else {
        return; // usage-store 关 / 未装配：诚实 no-op，请求不失败
    };
    let usage = rec.usage;
    let input = usage.map(|u| u.prompt_tokens).unwrap_or(0);
    let output = usage.map(|u| u.completion_tokens).unwrap_or(0);
    let cache_creation = usage.and_then(|u| u.cache_creation_tokens).unwrap_or(0);
    // cache_read 口径与 organ 5 同源：cache_read_tokens 优先，回落
    // cached_tokens 别名（OpenAI prompt_cache_hit_tokens 的落点）。
    let cache_read = usage
        .and_then(|u| u.cache_read_tokens.or(u.cached_tokens))
        .unwrap_or(0);
    // 与 organ 5 同一计价管线；未命中价目条目 → 全 0 + 空条目名。
    let breakdown = ds
        .compute_cost_breakdown(
            rec.execution_model,
            input,
            output,
            cache_creation,
            cache_read,
        )
        .unwrap_or_default();
    let log = nemesis_data::RequestLog {
        id: 0,
        trace_id: rec.trace_id,
        model: rec.execution_model.to_string(),
        provider_type: String::new(),
        input_tokens: input,
        output_tokens: output,
        cache_creation_tokens: cache_creation,
        cache_read_tokens: cache_read,
        total_cost_usd: breakdown.total_cost_usd,
        latency_ms: rec.latency_ms as i64,
        status_code: rec.status_code,
        error_message: rec.error_message.map(|e| e.to_string()),
        is_streaming: rec.is_streaming,
        created_at: chrono::Local::now().timestamp(),
        pricing_model: breakdown.pricing_model,
        input_cost_usd: breakdown.input_cost_usd,
        output_cost_usd: breakdown.output_cost_usd,
        cache_creation_cost_usd: breakdown.cache_creation_cost_usd,
        cache_read_cost_usd: breakdown.cache_read_cost_usd,
        // 代理转发 v1 不测 TTFT（上游 chunk 流无首帧时间戳透出）。
        first_token_ms: None,
        session_key: rec.session_key,
    };
    if let Err(e) = ds.insert_request_log(&log) {
        tracing::warn!("[proxy-usage] 记账失败（不影响代理响应）：{e}");
    }
}

/// 执行模型解析单一真相源：调用方点名优先（sse_chat 的 req.model），
/// 空 = master 活跃模型（AppState 展示字段与 set_default 同一更新点），
/// 再空 = 默认槽文本（default-following 语义的可读名）。bare lane 恒传
/// `None`（执行模型 = master 活跃模型，不消费调用方回显名）。
pub(crate) fn proxy_execution_model(state: &AppState, requested: Option<&str>) -> String {
    if let Some(r) = requested
        && !r.trim().is_empty()
    {
        return r.to_string();
    }
    let m = crate::openai_compat::active_model_name(state);
    if m.is_empty() {
        nemesis_providers::default_slot::current()
            .map(|(_, model, _)| model)
            .unwrap_or(m)
    } else {
        m
    }
}
