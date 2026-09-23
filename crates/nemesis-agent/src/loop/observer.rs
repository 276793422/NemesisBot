//! 轮内用量观察（P2-4 器官 5 自 `loop/run_loop.rs` 内联块收编；docs/PLAN/
//! 2026-09-23_agentloop-god-object-decomposition.md §4.2 表行 5）。
//! LlmResponse observer 事件发射 + data store 计价明细落账（RequestLog，
//! A3 分项计价 + 实际命中条目名）。语义零变化：`&mut LlmResponse` 仅为
//! `raw_request_body/raw_response_body` 的 `take()`（大报文进 observer，
//! 不留副本）；无出口——纯观察器官，骨架 `st.turns_used += 1` 后调用。
use super::*;

impl AgentLoop {
    /// 器官 5（§4.2）：LlmResponse observer 事件 + data store 计价明细。
    /// 调用点在 `st.turns_used += 1` 之后，`turns_used` 即当前轮次（1 起）。
    pub(crate) async fn record_round_usage(
        &self,
        context: &RequestContext,
        trace_id: &str,
        response: &mut LlmResponse,
        round_duration: std::time::Duration,
        turns_used: u32,
    ) {
        // Emit LLM response observer event.
        let tc_values: Vec<serde_json::Value> = response
            .tool_calls
            .iter()
            .filter_map(|tc| serde_json::to_value(tc).ok())
            .collect();
        let tc_count = response.tool_calls.len();
        self.emit_observer_sync(crate::loop_executor::ObserverEvent::LlmResponse {
            trace_id: trace_id.to_string(),
            round: turns_used,
            duration_ms: round_duration.as_millis() as u64,
            has_tool_calls: !response.tool_calls.is_empty(),
            content: response.content.clone(),
            tool_calls: tc_values,
            tool_calls_count: tc_count,
            finish_reason: if response.finished {
                Some("stop".to_string())
            } else {
                None
            },
            usage: response.usage.clone(),
            raw_request_body: response.raw_request_body.take(),
            raw_response_body: response.raw_response_body.take(),
        })
        .await;

        // Record usage statistics if data store is available.
        if let Some(ref ds) = self.data_store
            && let Some(ref usage) = response.usage
        {
            let model_name = self.active_model.read().clone();
            let cache_creation = usage.cache_creation_tokens.unwrap_or(0);
            let cache_read = usage.cache_read_tokens.or(usage.cached_tokens).unwrap_or(0);
            // A3：分项计价 + 实际命中条目名（未命中 → 空名 + 全 0，
            // 明细行可区分「未命中」与「命中免费条目」）。
            let breakdown = ds.compute_cost_breakdown(
                &model_name,
                usage.prompt_tokens,
                usage.completion_tokens,
                cache_creation,
                cache_read,
            );
            let empty = nemesis_data::CostBreakdown::default();
            let bd = breakdown.as_ref().unwrap_or(&empty);
            let log = nemesis_data::RequestLog {
                id: 0,
                trace_id: trace_id.to_string(),
                model: model_name.clone(),
                provider_type: String::new(),
                input_tokens: usage.prompt_tokens,
                output_tokens: usage.completion_tokens,
                cache_creation_tokens: cache_creation,
                cache_read_tokens: cache_read,
                total_cost_usd: bd.total_cost_usd,
                latency_ms: round_duration.as_millis() as i64,
                status_code: if response.content.starts_with("Error:") {
                    500
                } else {
                    200
                },
                error_message: None,
                is_streaming: false,
                created_at: chrono::Local::now().timestamp(),
                pricing_model: bd.pricing_model.clone(),
                input_cost_usd: bd.input_cost_usd,
                output_cost_usd: bd.output_cost_usd,
                cache_creation_cost_usd: bd.cache_creation_cost_usd,
                cache_read_cost_usd: bd.cache_read_cost_usd,
                // provider trait 无流式通路，TTFT 无从测量（列留 NULL）。
                first_token_ms: None,
                session_key: context.session_key.clone(),
            };
            if let Err(e) = ds.insert_request_log(&log) {
                tracing::warn!("[AgentLoop] Failed to record usage: {e}");
            }
        }
    }
}
