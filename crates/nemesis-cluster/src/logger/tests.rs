use super::*;

#[test]
fn test_log_functions_dont_panic() {
    // Just ensure these don't panic
    log_lifecycle("start", "node-1", "Cluster started");
    log_rpc("outgoing", "peer_chat", "req-1", "node-a", Some("node-b"));
    log_task("created", "task-1", "peer_chat");
    log_discovery("found", "10.0.0.1:9000", Some("node-2"));
    log_error("rpc", "connection refused", "dialing peer");
}

// -- Additional tests for uncovered functions --

#[test]
fn test_log_discovery_info_does_not_panic() {
    log_discovery_info("discovery scan completed");
    log_discovery_info("another info message");
}

#[test]
fn test_log_discovery_error_does_not_panic() {
    log_discovery_error("failed to bind UDP socket");
    log_discovery_error("connection timeout");
}

#[test]
fn test_log_rpc_without_target() {
    log_rpc("incoming", "ping", "req-2", "node-b", None);
}

#[test]
fn test_log_discovery_without_node_id() {
    log_discovery("timeout", "10.0.0.1:9000", None);
}
