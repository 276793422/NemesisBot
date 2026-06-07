//! Cluster logger - structured logging utilities for cluster events.
//!
//! Each function emits both a `tracing` log (for console/log collection) and a
//! structured JSONL entry via `cluster_log::write_cluster_log()` (for Dashboard).

use crate::cluster_log;

/// Log a cluster lifecycle event (start, stop, node join/leave).
pub fn log_lifecycle(event: &str, node_id: &str, details: &str) {
    tracing::info!(
        event = event,
        node_id = node_id,
        details = details,
        "[cluster] {}",
        event
    );

    // Map event name to log event type.
    let log_event = match event {
        "start" => "cluster_start",
        "stop" => "cluster_stop",
        _ => event,
    };
    cluster_log::write_cluster_log(
        log_event,
        serde_json::json!({
            "node_id": node_id,
            "details": details,
        }),
    );
}

/// Log an RPC event (request sent, response received).
pub fn log_rpc(direction: &str, action: &str, request_id: &str, source: &str, target: Option<&str>) {
    tracing::debug!(
        direction = direction,
        action = action,
        request_id = request_id,
        source = source,
        target = target.unwrap_or("broadcast"),
        "[cluster-rpc] {} {} {}",
        direction,
        action,
        request_id
    );

    cluster_log::write_cluster_log(
        "rpc_call",
        serde_json::json!({
            "direction": direction,
            "action": action,
            "request_id": request_id,
            "source": source,
            "target": target.unwrap_or("broadcast"),
        }),
    );
}

/// Log a task event (created, assigned, completed, failed, etc.).
pub fn log_task(event: &str, task_id: &str, action: &str) {
    tracing::info!(
        event = event,
        task_id = task_id,
        action = action,
        "[cluster-task] {} task {} ({})",
        event,
        task_id,
        action
    );

    let log_event = match event {
        "submitted" => "task_submitted",
        "assigned" => "task_assigned",
        "completed" => "task_completed",
        "failed" => "task_failed",
        "timeout" => "task_timeout",
        "cancelled" => "task_cancelled",
        "exec_start" => "task_exec_start",
        "exec_resume" => "task_exec_resume",
        "exec_done" => "task_exec_done",
        "exec_async" => "task_exec_async",
        "exec_failed" => "task_exec_failed",
        _ => event,
    };
    cluster_log::write_cluster_log(
        log_event,
        serde_json::json!({
            "task_id": task_id,
            "action": action,
        }),
    );
}

/// Log a discovery event (peer found, peer lost, peer removed).
pub fn log_discovery(event: &str, peer_addr: &str, node_id: Option<&str>) {
    tracing::info!(
        event = event,
        peer_addr = peer_addr,
        node_id = node_id.unwrap_or("unknown"),
        "[cluster-discovery] {} peer {}",
        event,
        peer_addr
    );

    let log_event = match event {
        "discovered" => "node_discovered",
        "updated" => "node_updated",
        "offline" => "node_offline",
        "removed" => "node_removed",
        _ => event,
    };
    cluster_log::write_cluster_log(
        log_event,
        serde_json::json!({
            "peer_addr": peer_addr,
            "node_id": node_id.unwrap_or("unknown"),
        }),
    );
}

/// Log an info-level discovery event (mirrors Go's ClusterLogger.DiscoveryInfo).
pub fn log_discovery_info(msg: &str) {
    tracing::info!("[cluster-discovery] {}", msg);
}

/// Log an error-level discovery event (mirrors Go's ClusterLogger.DiscoveryError).
pub fn log_discovery_error(msg: &str) {
    tracing::error!("[cluster-discovery] {}", msg);
}

/// Log an error within the cluster subsystem.
pub fn log_error(component: &str, error: &str, context: &str) {
    tracing::error!(
        component = component,
        error = error,
        context = context,
        "[cluster-error] {} error: {} (context: {})",
        component,
        error,
        context
    );
}

#[cfg(test)]
mod tests;
