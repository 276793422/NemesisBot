// ---------------------------------------------------------------------------
// Cluster 支撑件（E1 二期 token 回传 + host:port 解析）
// ---------------------------------------------------------------------------

/// E1 二期 token 回传（全自动流转 P5）：把 worker 回调携带的 `usage` 记入
/// master 用量账本（DataStore request_logs）。记账键 = `cluster_rpc:
/// {worker}/{task_id}`（与 worker 侧 `cluster_rpc:{A}/{chat}` 会话键同前缀
/// 家族；per-task 粒度让 token 预算闸能按派发行精确聚合，`{worker}%` LIKE
/// 前缀聚合同时可用）。诚实边界：无 usage（旧 worker / 错误回调）/ 无
/// DataStore / 落库失败 → 静默跳过（warn），绝不影响回调路由。
#[cfg(feature = "cluster")] // 唯一消费点在 peer_chat_callback（集群回调闭包内）
pub(crate) fn record_cluster_usage(
    ds: Option<&std::sync::Arc<nemesis_data::DataStore>>,
    source_node: &str,
    task_id: &str,
    usage: Option<&serde_json::Value>,
) {
    let Some(ds) = ds else { return };
    let Some(usage) = usage else { return };
    if task_id.is_empty() {
        return;
    }
    let get_num = |key: &str| -> i64 {
        usage
            .get(key)
            .and_then(|v| v.as_i64().or_else(|| v.as_u64().map(|n| n as i64)))
            .unwrap_or(0)
    };
    let input = get_num("input_tokens");
    let output = get_num("output_tokens");
    let cost = usage
        .get("cost_usd")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let log = nemesis_data::RequestLog {
        id: 0,
        trace_id: format!("cluster-callback:{task_id}"),
        model: format!("cluster_delegate:{source_node}"),
        provider_type: "cluster".to_string(),
        input_tokens: input,
        output_tokens: output,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
        total_cost_usd: cost,
        latency_ms: 0,
        status_code: 200,
        error_message: None,
        is_streaming: false,
        created_at: chrono::Local::now().timestamp(),
        pricing_model: String::new(),
        input_cost_usd: 0.0,
        output_cost_usd: 0.0,
        cache_creation_cost_usd: 0.0,
        cache_read_cost_usd: 0.0,
        first_token_ms: None,
        session_key: format!("cluster_rpc:{source_node}/{task_id}"),
    };
    if let Err(e) = ds.insert_request_log(&log) {
        tracing::warn!("[Gateway] Failed to record cluster callback usage: {e}");
    }
}

/// Parse "host:port" string into (host, port).
#[cfg(any(feature = "cluster", test))]
pub(crate) fn parse_host_port(addr: &str) -> (String, u16) {
    if let Some(idx) = addr.rfind(':') {
        let host = &addr[..idx];
        let port: u16 = addr[idx + 1..].parse().unwrap_or(0);
        (host.to_string(), port)
    } else {
        (addr.to_string(), 0)
    }
}
