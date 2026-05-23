//! Cluster logger - structured logging utilities for cluster events.

use tracing;

/// Log a cluster lifecycle event (start, stop, node join/leave).
pub fn log_lifecycle(event: &str, node_id: &str, details: &str) {
    tracing::info!(
        event = event,
        node_id = node_id,
        details = details,
        "[cluster] {}",
        event
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
}

/// Log a task event (created, assigned, completed, failed).
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
}

/// Log a discovery event (peer found, peer lost).
pub fn log_discovery(event: &str, peer_addr: &str, node_id: Option<&str>) {
    tracing::info!(
        event = event,
        peer_addr = peer_addr,
        node_id = node_id.unwrap_or("unknown"),
        "[cluster-discovery] {} peer {}",
        event,
        peer_addr
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
