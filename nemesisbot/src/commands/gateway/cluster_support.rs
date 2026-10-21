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

// ---------------------------------------------------------------------------
// P4 交付门（持久 Agent 实体，2026-10-17）：A 端交付分支记账
// ---------------------------------------------------------------------------

/// 记账内核（只做 DB 写，审计在 audited shim——inner 不引用 optional
/// 依赖类型，cluster 单门可编译）。诚实边界对齐 `record_cluster_usage`：
/// 无台账（`Cluster::new` 测试路径）/ 无交付（error/取消回调）/ 空
/// task_id → `Ok(false)` 静默；落库失败 → Err 由调用方决定 warn。
#[cfg(feature = "cluster")]
fn branch_delivery_inner(
    task_db: Option<&std::sync::Arc<nemesis_taskstore::TaskDb>>,
    task_id: &str,
    source_node: &str,
    delivery: Option<&serde_json::Value>,
) -> Result<bool, String> {
    let Some(db) = task_db else {
        return Ok(false);
    };
    let Some(delivery) = delivery.filter(|v| v.is_object()) else {
        return Ok(false);
    };
    if task_id.is_empty() {
        return Ok(false);
    }
    db.record_delivery(
        "branch",
        Some(task_id),
        &format!("{task_id}:branch"),
        delivery,
        false,
    )
    .map_err(|e| {
        tracing::warn!(
            task_id = %task_id,
            source_node = %source_node,
            error = %e,
            "[Gateway] 交付分支记账失败（回调路由不受影响）"
        );
        e
    })
}

/// A 端交付分支记账（cluster 门；security 关闭形态）：deliveries 表入账，
/// 不做审计链留痕。
#[cfg(all(feature = "cluster", not(feature = "security")))]
pub(crate) fn record_branch_delivery(
    task_db: Option<&std::sync::Arc<nemesis_taskstore::TaskDb>>,
    source_node: &str,
    task_id: &str,
    delivery: Option<&serde_json::Value>,
) {
    let _ = branch_delivery_inner(task_db, task_id, source_node, delivery);
}

/// A 端交付分支记账 + 审计留痕（all(cluster, security) 门）：deliveries 入
/// 账后 `append_direct_audit_event`（F8 先例——非工具管线的直接审计记账，
/// Merkle append-only；审计链未启用 = None，tracing 兜底可见）。
#[cfg(all(feature = "cluster", feature = "security"))]
pub(crate) fn record_branch_delivery_audited(
    task_db: Option<&std::sync::Arc<nemesis_taskstore::TaskDb>>,
    plugin: Option<&std::sync::Arc<nemesis_security::pipeline::SecurityPlugin>>,
    source_node: &str,
    task_id: &str,
    delivery: Option<&serde_json::Value>,
) {
    let recorded = branch_delivery_inner(task_db, task_id, source_node, delivery);
    let Some(plugin) = plugin else { return };
    let (decision, reason) = match recorded {
        Ok(true) => ("allow", "delivery branch recorded to ledger"),
        Ok(false) => (
            "allow",
            "delivery branch skipped (no ledger / no payload / duplicate)",
        ),
        Err(ref e) => ("deny", e.as_str()),
    };
    if plugin
        .append_direct_audit_event(
            "delivery_record",
            "delivery_gate",
            "system",
            "cluster_callback",
            &format!("{source_node}/{task_id}"),
            decision,
            reason,
        )
        .is_none()
    {
        tracing::debug!(
            task_id = %task_id,
            "[Gateway] 审计链未启用，交付记账仅 tracing 可见"
        );
    }
}
