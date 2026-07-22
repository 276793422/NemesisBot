use super::*;
use chrono::Local;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_short_id() {
    assert_eq!(short_id("12345678"), "12345678");
    assert_eq!(short_id("1234567890"), "12345678");
    assert_eq!(short_id("short"), "short");
}

#[test]
fn test_read_recent_events_empty_directory() {
    let temp_dir = TempDir::new().unwrap();
    let log_dir = temp_dir.path();

    let events = read_recent_events(log_dir, 10);
    assert_eq!(
        events.len(),
        0,
        "No events should be read from empty directory"
    );
}

#[test]
fn test_read_recent_events_with_events() {
    let temp_dir = TempDir::new().unwrap();
    let log_dir = temp_dir.path();

    // Create a log file with some events
    let now = Local::now();
    let date_str = now.format("%Y-%m-%d").to_string();
    let log_file = log_dir.join(format!("cluster_{}.log", date_str));

    let log_content = r#"{"event":"cluster_start","ts":"2024-01-01T10:00:00+00:00","node_id":"node1"}
{"event":"node_discovered","ts":"2024-01-01T10:01:00+00:00","node_id":"node2","peer_addr":"10.0.0.1:9000"}
{"event":"task_submitted","ts":"2024-01-01T10:02:00+00:00","task_id":"task-123","action":"peer_chat"}
"#;

    fs::write(&log_file, log_content).unwrap();

    let events = read_recent_events(log_dir, 10);
    assert!(!events.is_empty(), "Should read events from log file");
}

#[test]
fn test_read_recent_events_respects_limit() {
    let temp_dir = TempDir::new().unwrap();
    let log_dir = temp_dir.path();

    // Create a log file with many events
    let now = Local::now();
    let date_str = now.format("%Y-%m-%d").to_string();
    let log_file = log_dir.join(format!("cluster_{}.log", date_str));

    let mut log_content = String::new();
    for i in 1..=20 {
        log_content.push_str(&format!(r#"{{"event":"task_submitted","ts":"2024-01-01T10:00:{}+00:00","task_id":"task-{}","action":"test"}}
"#, i, i));
    }

    fs::write(&log_file, log_content).unwrap();

    let events = read_recent_events(log_dir, 5);
    assert_eq!(events.len(), 5, "Should respect limit parameter");
}

#[test]
fn test_aggregate_node_stats_empty_directory() {
    let temp_dir = TempDir::new().unwrap();
    let log_dir = temp_dir.path();

    let stats = aggregate_node_stats(log_dir);
    assert_eq!(
        stats.len(),
        0,
        "No stats should be aggregated from empty directory"
    );
}

#[test]
fn test_aggregate_node_stats_with_task_events() {
    let temp_dir = TempDir::new().unwrap();
    let log_dir = temp_dir.path();

    // Create log files with task events
    let now = Local::now();
    let date_str = now.format("%Y-%m-%d").to_string();
    let log_file = log_dir.join(format!("cluster_{}.log", date_str));

    let log_content = r#"{"event":"task_assigned","ts":"2024-01-01T10:00:00+00:00","task_id":"task-1","action":"node1"}
{"event":"task_assigned","ts":"2024-01-01T10:01:00+00:00","task_id":"task-2","action":"node1"}
{"event":"task_assigned","ts":"2024-01-01T10:02:00+00:00","task_id":"task-3","action":"node2"}
{"event":"task_completed","ts":"2024-01-01T10:03:00+00:00","task_id":"task-1","action":"peer_chat"}
{"event":"task_completed","ts":"2024-01-01T10:04:00+00:00","task_id":"task-2","action":"peer_chat"}
{"event":"task_failed","ts":"2024-01-01T10:05:00+00:00","task_id":"task-3","action":"peer_chat"}
"#;

    fs::write(&log_file, log_content).unwrap();

    let stats = aggregate_node_stats(log_dir);

    assert!(stats.contains_key("node1"), "Should have stats for node1");
    assert!(stats.contains_key("node2"), "Should have stats for node2");

    let node1_stats = &stats["node1"];
    assert_eq!(node1_stats.task_count, 2);
    assert_eq!(node1_stats.success_count, 2);
    assert_eq!(node1_stats.fail_count, 0);
    assert_eq!(node1_stats.success_rate, 1.0);

    let node2_stats = &stats["node2"];
    assert_eq!(node2_stats.task_count, 1);
    assert_eq!(node2_stats.success_count, 0);
    assert_eq!(node2_stats.fail_count, 1);
    assert_eq!(node2_stats.success_rate, 0.0);
}

#[test]
fn test_aggregate_task_summaries_empty_input() {
    let temp_dir = TempDir::new().unwrap();
    let log_dir = temp_dir.path();

    let summaries = aggregate_task_summaries(log_dir, &[]);
    assert_eq!(
        summaries.len(),
        0,
        "Empty task ID list should return empty summaries"
    );
}

#[test]
fn test_aggregate_task_summaries_with_events() {
    let temp_dir = TempDir::new().unwrap();
    let log_dir = temp_dir.path();

    // Create log file with task events
    let now = Local::now();
    let date_str = now.format("%Y-%m-%d").to_string();
    let log_file = log_dir.join(format!("cluster_{}.log", date_str));

    let log_content = r#"{"event":"task_llm_start","ts":"2024-01-01T10:00:00+00:00","task_id":"task-1"}
{"event":"task_tool_call","ts":"2024-01-01T10:01:00+00:00","task_id":"task-1","tool":"search"}
{"event":"task_llm_start","ts":"2024-01-01T10:02:00+00:00","task_id":"task-1"}
{"event":"task_tool_call","ts":"2024-01-01T10:03:00+00:00","task_id":"task-1","tool":"calculate"}
{"event":"task_tool_call","ts":"2024-01-01T10:04:00+00:00","task_id":"task-1","tool":"format"}
"#;

    fs::write(&log_file, log_content).unwrap();

    let summaries = aggregate_task_summaries(log_dir, &["task-1".to_string()]);

    assert!(summaries.contains_key("task-1"));
    let summary = &summaries["task-1"];
    assert_eq!(summary.rounds, 2);
    assert_eq!(summary.tool_calls, 3);
    assert_eq!(summary.tool_chain.len(), 3);
}

#[test]
fn test_reconstruct_traces_empty_directory() {
    let temp_dir = TempDir::new().unwrap();
    let log_dir = temp_dir.path();

    let traces = reconstruct_traces(log_dir);
    assert_eq!(
        traces.len(),
        0,
        "No traces should be reconstructed from empty directory"
    );
}

#[test]
fn test_reconstruct_traces_with_rpc_events() {
    let temp_dir = TempDir::new().unwrap();
    let log_dir = temp_dir.path();

    // Create log file with RPC events
    let now = Local::now();
    let date_str = now.format("%Y-%m-%d").to_string();
    let log_file = log_dir.join(format!("cluster_{}.log", date_str));

    let log_content = r#"{"event":"rpc_call","ts":"2024-01-01T10:00:00+00:00","direction":"outbound","request_id":"req-1","source":"node-a","target":"node-b","action":"peer_chat"}
{"event":"rpc_call","ts":"2024-01-01T10:01:00+00:00","direction":"inbound","request_id":"req-2","source":"node-c","target":"node-a","action":"ping"}
{"event":"rpc_call","ts":"2024-01-01T10:02:00+00:00","direction":"outbound","request_id":"req-3","source":"node-b","target":"node-c","action":"status"}
"#;

    fs::write(&log_file, log_content).unwrap();

    let traces = reconstruct_traces(log_dir);
    assert_eq!(traces.len(), 2, "Should only include outbound RPC calls");
}

#[test]
fn test_read_rpc_connections_empty_directory() {
    let temp_dir = TempDir::new().unwrap();
    let log_dir = temp_dir.path();

    let connections = read_rpc_connections(log_dir);
    assert_eq!(
        connections.len(),
        0,
        "No connections should be read from empty directory"
    );
}

#[test]
fn test_read_rpc_connections_with_events() {
    let temp_dir = TempDir::new().unwrap();
    let log_dir = temp_dir.path();

    // Create log file with RPC events
    let now = Local::now();
    let date_str = now.format("%Y-%m-%d").to_string();
    let log_file = log_dir.join(format!("cluster_{}.log", date_str));

    let log_content = r#"{"event":"rpc_call","ts":"2024-01-01T10:00:00+00:00","source":"node-a","target":"node-b","action":"peer_chat","direction":"outbound"}
{"event":"rpc_call","ts":"2024-01-01T10:01:00+00:00","source":"node-b","target":"node-c","action":"ping","direction":"outbound"}
{"event":"rpc_call","ts":"2024-01-01T10:02:00+00:00","source":"broadcast","target":"node-a","action":"test","direction":"outbound"}
"#;

    fs::write(&log_file, log_content).unwrap();

    let connections = read_rpc_connections(log_dir);
    assert_eq!(connections.len(), 2, "Should exclude broadcast connections");
}

#[test]
fn test_format_event_all_event_types() {
    let temp_dir = TempDir::new().unwrap();
    let log_dir = temp_dir.path();

    // Create log file with various event types
    let now = Local::now();
    let date_str = now.format("%Y-%m-%d").to_string();
    let log_file = log_dir.join(format!("cluster_{}.log", date_str));

    let log_content = r#"{"event":"cluster_start","ts":"2024-01-01T10:00:00+00:00","node_id":"node1"}
{"event":"node_discovered","ts":"2024-01-01T10:01:00+00:00","node_id":"node2","peer_addr":"10.0.0.1:9000"}
{"event":"task_completed","ts":"2024-01-01T10:02:00+00:00","task_id":"task-123","action":"peer_chat"}
{"event":"rpc_call","ts":"2024-01-01T10:03:00+00:00","direction":"outbound","source":"node-a","target":"node-b","action":"peer_chat","request_id":"req-1","duration_ms":123,"success":true}
{"event":"rpc_call","ts":"2024-01-01T10:04:00+00:00","direction":"inbound","source":"node-c","target":"node-a","action":"diagnostics.system","request_id":"req-2","duration_ms":5,"success":true}
{"event":"rpc_call","ts":"2024-01-01T10:05:00+00:00","direction":"outbound","source":"node-a","target":"node-d","action":"diagnostics.network","request_id":"req-3","duration_ms":2000,"success":false,"error":"timeout"}
"#;

    fs::write(&log_file, log_content).unwrap();

    let events = read_recent_events(log_dir, 10);
    assert!(events.len() >= 3, "Should format multiple event types");
}

/// Verify rpc_call formatting: outbound includes source → target with duration,
/// inbound renders as "→ 本机", and failures get the ✗ marker.
#[test]
fn test_format_event_rpc_call_directions_and_status() {
    let temp_dir = TempDir::new().unwrap();
    let log_dir = temp_dir.path();
    let now = Local::now();
    let date_str = now.format("%Y-%m-%d").to_string();
    let log_file = log_dir.join(format!("cluster_{}.log", date_str));

    let log_content = r#"{"event":"rpc_call","ts":"2024-01-01T10:00:00+00:00","direction":"outbound","source":"node-a","target":"node-b","action":"diagnostics.system","request_id":"req-1","duration_ms":42,"success":true}
{"event":"rpc_call","ts":"2024-01-01T10:01:00+00:00","direction":"inbound","source":"node-c","target":"node-a","action":"diagnostics.network","request_id":"req-2","duration_ms":7,"success":true}
{"event":"rpc_call","ts":"2024-01-01T10:02:00+00:00","direction":"outbound","source":"node-a","target":"node-d","action":"diagnostics.cluster_state","request_id":"req-3","duration_ms":3000,"success":false,"error":"timeout"}
"#;
    fs::write(&log_file, log_content).unwrap();

    let events = read_recent_events(log_dir, 10);
    // read_recent_events returns most-recent-first; collect messages by type.
    let messages: Vec<String> = events.into_iter().map(|e| e.message).collect();

    // Outbound success: "RPC {source} → {target} ({action}) · {ms}ms"
    let outbound_ok = messages.iter().find(|m| m.contains("node-a → node-b"));
    assert!(
        outbound_ok.is_some(),
        "outbound success entry missing, got: {:?}",
        messages
    );
    let outbound_ok = outbound_ok.unwrap();
    assert!(
        outbound_ok.contains("diagnostics.system"),
        "action in: {}",
        outbound_ok
    );
    assert!(outbound_ok.contains("42ms"), "duration in: {}", outbound_ok);
    assert!(
        !outbound_ok.contains('✗'),
        "success should not have marker: {}",
        outbound_ok
    );

    // Inbound: "RPC {source} → 本机 ({action}) · {ms}ms"
    let inbound = messages.iter().find(|m| m.contains("node-c → 本机"));
    assert!(
        inbound.is_some(),
        "inbound entry missing (was previously hidden), got: {:?}",
        messages
    );
    let inbound = inbound.unwrap();
    assert!(
        inbound.contains("diagnostics.network"),
        "action in: {}",
        inbound
    );
    assert!(inbound.contains("7ms"), "duration in: {}", inbound);

    // Outbound failure: should have ✗ marker.
    let outbound_fail = messages.iter().find(|m| m.contains("node-a → node-d"));
    assert!(
        outbound_fail.is_some(),
        "outbound failure entry missing, got: {:?}",
        messages
    );
    assert!(
        outbound_fail.unwrap().contains('✗'),
        "failed rpc_call should have ✗ marker"
    );
}

/// Startup-time entries from `logger::log_rpc("register_handler", ...)` have
/// direction values other than "outbound"/"inbound". These must be hidden
/// from the dashboard (historically they were; my reader change must not
/// accidentally surface them as inbound).
#[test]
fn test_format_event_rpc_call_unknown_direction_hidden() {
    let temp_dir = TempDir::new().unwrap();
    let log_dir = temp_dir.path();
    let now = Local::now();
    let date_str = now.format("%Y-%m-%d").to_string();
    let log_file = log_dir.join(format!("cluster_{}.log", date_str));

    let log_content = r#"{"event":"rpc_call","ts":"2024-01-01T10:00:00+00:00","direction":"register_handler","action":"ping","request_id":"","source":"","target":"broadcast"}
{"event":"rpc_call","ts":"2024-01-01T10:01:00+00:00","direction":"register_peer_chat_handlers","action":"","request_id":"RPCChannel not ready","source":"","target":"broadcast"}
"#;
    fs::write(&log_file, log_content).unwrap();

    let events = read_recent_events(log_dir, 10);
    assert!(
        events.is_empty(),
        "non-outbound/inbound rpc_call entries must be hidden, got: {:?}",
        events
    );
}
