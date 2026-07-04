//! Cluster log reader — reads JSONL log files and aggregates data for Dashboard.
//!
//! Provides pure functions (no state) that read `cluster_YYYY-MM-DD.log` files
//! and return structured data for the WSAPI handlers.

use std::collections::HashMap;
use std::path::Path;

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// A single cluster log event.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ClusterLogEvent {
    pub event: String,
    pub ts: String,
    #[serde(flatten)]
    pub data: serde_json::Value,
}

/// Per-node task statistics aggregated from log events.
#[derive(Debug, Clone, serde::Serialize)]
pub struct NodeStats {
    pub task_count: u32,
    pub success_count: u32,
    pub fail_count: u32,
    pub success_rate: f64,
}

/// Per-task execution details aggregated from log events.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TaskExecutionSummary {
    pub rounds: u32,
    pub tool_calls: u32,
    pub tool_chain: Vec<String>,
}

/// A single hop in an RPC trace chain.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TraceHop {
    pub node: String,
    pub duration_ms: Option<u64>,
    pub ts: String,
}

/// An RPC trace (single or multi-hop).
#[derive(Debug, Clone, serde::Serialize)]
pub struct RpcTrace {
    pub id: String,
    pub hops: Vec<TraceHop>,
    pub failed: bool,
}

/// A formatted event for the ActivityFeed UI.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FormattedEvent {
    pub time: String,
    pub r#type: String,
    pub message: String,
}

/// A connection pair between nodes (from rpc_call events).
#[derive(Debug, Clone, serde::Serialize)]
pub struct RpcConnection {
    pub from: String,
    pub to: String,
    pub last_seen: String,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Read the last `limit` events from cluster log files.
///
/// Reads today's and yesterday's files for cross-midnight continuity.
pub fn read_recent_events(log_dir: &Path, limit: usize) -> Vec<FormattedEvent> {
    let entries = read_log_entries(log_dir, 2);
    let mut events: Vec<FormattedEvent> = entries
        .iter()
        .rev()
        .filter_map(|e| format_event(e))
        .take(limit)
        .collect();
    events.reverse();
    events
}

/// Aggregate per-node task statistics from log events.
///
/// Uses a two-step process:
/// 1. Collect `task_assigned` events to build `task_id → node_id` mapping.
/// 2. Count `task_completed`/`task_failed` events and attribute to nodes.
pub fn aggregate_node_stats(log_dir: &Path) -> HashMap<String, NodeStats> {
    let entries = read_log_entries(log_dir, 7);

    // Step 1: Build task_id → node_id mapping from task_assigned events.
    let mut task_to_node: HashMap<String, String> = HashMap::new();
    for entry in &entries {
        if entry.event == "task_assigned" {
            if let Some(task_id) = entry.data.get("task_id").and_then(|v| v.as_str()) {
                if let Some(node_id) = entry.data.get("action").and_then(|v| v.as_str()) {
                    task_to_node.insert(task_id.to_string(), node_id.to_string());
                }
            }
        }
    }

    // Step 2: Count tasks per node.
    let mut stats: HashMap<String, (u32, u32)> = HashMap::new(); // (success, fail)
    for entry in &entries {
        let task_id = match entry.data.get("task_id").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => continue,
        };

        let node_id = match task_to_node.get(task_id) {
            Some(n) => n.clone(),
            None => continue,
        };

        let counter = stats.entry(node_id).or_insert((0, 0));
        match entry.event.as_str() {
            "task_completed" => counter.0 += 1,
            "task_exec_failed" | "task_failed" => counter.1 += 1,
            _ => {}
        }
    }

    // Build final stats: count how many tasks were assigned to each node.
    let mut node_task_counts: HashMap<String, u32> = HashMap::new();
    for (_, node_id) in &task_to_node {
        *node_task_counts.entry(node_id.clone()).or_insert(0) += 1;
    }

    let mut result: HashMap<String, NodeStats> = HashMap::new();
    for (node_id, count) in &node_task_counts {
        let (success, fail) = stats.get(node_id).copied().unwrap_or((0, 0));
        let success_rate = if success + fail > 0 {
            success as f64 / (success + fail) as f64
        } else {
            0.0
        };
        result.insert(
            node_id.clone(),
            NodeStats {
                task_count: *count,
                success_count: success,
                fail_count: fail,
                success_rate,
            },
        );
    }

    // Also add nodes that appear in stats but not in task_to_node (edge case).
    for (node_id, (success, fail)) in &stats {
        if !result.contains_key(node_id) {
            let total = success + fail;
            let success_rate = if total > 0 {
                *success as f64 / total as f64
            } else {
                0.0
            };
            result.insert(
                node_id.clone(),
                NodeStats {
                    task_count: total,
                    success_count: *success,
                    fail_count: *fail,
                    success_rate,
                },
            );
        }
    }

    result
}

/// Aggregate per-task execution details for a set of task IDs.
///
/// Single-pass: reads the log file once and groups all events by task_id.
pub fn aggregate_task_summaries(
    log_dir: &Path,
    task_ids: &[String],
) -> HashMap<String, TaskExecutionSummary> {
    if task_ids.is_empty() {
        return HashMap::new();
    }

    let entries = read_log_entries(log_dir, 7);
    let id_set: HashMap<&str, bool> = task_ids.iter().map(|s| (s.as_str(), true)).collect();

    let mut rounds: HashMap<&str, u32> = HashMap::new();
    let mut tool_counts: HashMap<&str, u32> = HashMap::new();
    let mut tool_chains: HashMap<&str, Vec<String>> = HashMap::new();

    for entry in &entries {
        let task_id = match entry.data.get("task_id").and_then(|v| v.as_str()) {
            Some(id) if id_set.contains_key(id) => id,
            _ => continue,
        };

        match entry.event.as_str() {
            "task_llm_start" => {
                *rounds.entry(task_id).or_insert(0) += 1;
            }
            "task_tool_call" => {
                *tool_counts.entry(task_id).or_insert(0) += 1;
                if let Some(tool_name) = entry.data.get("tool").and_then(|v| v.as_str()) {
                    tool_chains
                        .entry(task_id)
                        .or_default()
                        .push(tool_name.to_string());
                }
            }
            _ => {}
        }
    }

    let mut result = HashMap::new();
    for id in task_ids {
        result.insert(
            id.clone(),
            TaskExecutionSummary {
                rounds: rounds.get(id.as_str()).copied().unwrap_or(0),
                tool_calls: tool_counts.get(id.as_str()).copied().unwrap_or(0),
                tool_chain: tool_chains.get(id.as_str()).cloned().unwrap_or_default(),
            },
        );
    }
    result
}

/// Reconstruct RPC traces from log events.
///
/// Currently returns single-hop traces from `rpc_call` events.
/// When `parent_request_id` is added to rpc_call events, this will produce
/// multi-hop chains without API changes.
pub fn reconstruct_traces(log_dir: &Path) -> Vec<RpcTrace> {
    let entries = read_log_entries(log_dir, 7);

    let mut traces: Vec<RpcTrace> = Vec::new();
    for entry in &entries {
        if entry.event != "rpc_call" {
            continue;
        }

        let direction = entry.data.get("direction").and_then(|v| v.as_str()).unwrap_or("");
        if direction != "outbound" {
            continue;
        }

        let request_id = match entry.data.get("request_id").and_then(|v| v.as_str()) {
            Some(id) => id.to_string(),
            None => continue,
        };
        let source = entry
            .data
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let target = entry
            .data
            .get("target")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let ts = entry.ts.clone();

        traces.push(RpcTrace {
            id: request_id,
            hops: vec![
                TraceHop {
                    node: source,
                    duration_ms: None,
                    ts: ts.clone(),
                },
                TraceHop {
                    node: target,
                    duration_ms: None,
                    ts,
                },
            ],
            failed: false,
        });
    }

    // Keep only the last 50 traces.
    if traces.len() > 50 {
        let start = traces.len() - 50;
        traces = traces.split_off(start);
    }

    traces
}

/// Extract unique RPC connection pairs from log events.
///
/// Returns `{from, to}` pairs that have recent RPC activity.
pub fn read_rpc_connections(log_dir: &Path) -> Vec<RpcConnection> {
    let entries = read_log_entries(log_dir, 7);

    let mut seen: HashMap<(String, String), String> = HashMap::new(); // (from,to) → last_seen
    for entry in &entries {
        if entry.event != "rpc_call" {
            continue;
        }
        let source = entry
            .data
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let target = entry
            .data
            .get("target")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if source.is_empty() || target.is_empty() || source == "broadcast" || target == "broadcast"
        {
            continue;
        }
        seen.insert((source, target), entry.ts.clone());
    }

    seen.into_iter()
        .map(|((from, to), last_seen)| RpcConnection {
            from,
            to,
            last_seen,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Read log entries from the last `days` days (1 = today, 2 = today + yesterday, etc.).
fn read_log_entries(log_dir: &Path, days: u32) -> Vec<ClusterLogEvent> {
    let mut all_entries = Vec::new();

    for offset in 0..days {
        let date = chrono::Local::now() - chrono::Duration::days(offset as i64);
        let filename = format!("cluster_{}.log", date.format("%Y-%m-%d"));
        let path = log_dir.join(&filename);

        if !path.exists() {
            continue;
        }

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
                let event = val
                    .get("event")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let ts = val
                    .get("ts")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                all_entries.push(ClusterLogEvent {
                    event,
                    ts,
                    data: val,
                });
            }
        }
    }

    all_entries
}

/// Format a log event into a human-readable ActivityFeed entry.
fn format_event(entry: &ClusterLogEvent) -> Option<FormattedEvent> {
    let ts = &entry.ts;
    let time = if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts) {
        dt.with_timezone(&chrono::Local).format("%H:%M:%S").to_string()
    } else if ts.len() >= 19 {
        ts[11..19].to_string()
    } else {
        ts.clone()
    };

    let task_id = entry
        .data
        .get("task_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let node_id = entry
        .data
        .get("node_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let (r#type, message) = match entry.event.as_str() {
        "cluster_start" => ("system", format!("集群启动 ({})", node_id)),
        "cluster_stop" => ("system", "集群停止".to_string()),
        "node_discovered" | "node_updated" => {
            let addr = entry.data.get("peer_addr").and_then(|v| v.as_str())
                .or_else(|| entry.data.get("details").and_then(|v| v.as_str()))
                .unwrap_or(node_id);
            ("node_online", format!("节点上线 {}", addr))
        }
        "node_offline" => {
            let addr = entry.data.get("peer_addr").and_then(|v| v.as_str())
                .unwrap_or(node_id);
            ("node_offline", format!("节点离线 {}", addr))
        }
        "node_removed" => ("node_offline", format!("节点移除 {}", node_id)),
        "task_submitted" => (
            "task_start",
            format!(
                "任务提交 {} ({})",
                short_id(task_id),
                entry
                    .data
                    .get("action")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
            ),
        ),
        "task_assigned" => (
            "task_start",
            format!("任务分配 {} → {}", short_id(task_id), node_id),
        ),
        "task_exec_start" => (
            "task_start",
            format!(
                "任务开始执行 {}",
                short_id(task_id),
            ),
        ),
        "task_exec_resume" => (
            "task_start",
            format!("任务恢复执行 {}", short_id(task_id)),
        ),
        "task_exec_done" => (
            "task_complete",
            format!(
                "任务完成 {} ({})",
                short_id(task_id),
                entry
                    .data
                    .get("action")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
            ),
        ),
        "task_exec_async" => (
            "task_start",
            format!("任务异步等待 {}", short_id(task_id)),
        ),
        "task_completed" => (
            "task_complete",
            format!("任务完成 {}", short_id(task_id)),
        ),
        "task_failed" | "task_exec_failed" => (
            "task_fail",
            format!(
                "任务失败 {} ({})",
                short_id(task_id),
                entry
                    .data
                    .get("action")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
            ),
        ),
        "task_timeout" => (
            "task_fail",
            format!(
                "任务超时 {} ({})",
                short_id(task_id),
                entry
                    .data
                    .get("action")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
            ),
        ),
        "task_cancelled" => ("task_fail", format!("任务取消 {}", short_id(task_id))),
        "rpc_call" => {
            let direction = entry
                .data
                .get("direction")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let duration_ms = entry
                .data
                .get("duration_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let success = entry
                .data
                .get("success")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let status_icon = if success { "" } else { " ✗" };
            let source = entry
                .data
                .get("source")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let action = entry
                .data
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if direction == "outbound" {
                let target = entry
                    .data
                    .get("target")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                (
                    "rpc",
                    format!(
                        "RPC {} → {} ({}) · {}ms{}",
                        source, target, action, duration_ms, status_icon
                    ),
                )
            } else if direction == "inbound" {
                (
                    "rpc_in",
                    format!(
                        "RPC {} → 本机 ({}) · {}ms{}",
                        source, action, duration_ms, status_icon
                    ),
                )
            } else {
                // Unknown direction (e.g. "register_handler" written by
                // logger::log_rpc at startup) — hide from dashboard.
                return None;
            }
        }
        _ => return None,
    };

    Some(FormattedEvent {
        time,
        r#type: r#type.to_string(),
        message,
    })
}

/// Truncate an ID to 8 characters for display.
fn short_id(id: &str) -> &str {
    if id.len() > 8 {
        &id[..8]
    } else {
        id
    }
}

#[cfg(test)]
mod tests;
