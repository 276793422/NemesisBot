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

// ============================================================
// S4 coverage: unknown lifecycle event arm + field lines under a
// no-op subscriber (field expressions only evaluate when the
// callsite is enabled).
// ============================================================

struct S4AllEventsSubscriber;
impl tracing::Subscriber for S4AllEventsSubscriber {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::Id {
        tracing::Id::from_u64(1)
    }
    fn record(&self, _span: &tracing::Id, _values: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _span: &tracing::Id, _follows: &tracing::Id) {}
    fn event(&self, _event: &tracing::Event<'_>) {}
    fn enter(&self, _span: &tracing::Id) {}
    fn exit(&self, _span: &tracing::Id) {}
}

fn s4_tracing_subscriber() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = tracing::subscriber::set_global_default(S4AllEventsSubscriber);
    });
}

/// An unknown lifecycle event name falls through `_ => event` (logger.rs 22).
#[test]
fn test_s4_log_lifecycle_unknown_event_name() {
    s4_tracing_subscriber();
    log_lifecycle("custom-s4-event", "node-s4", "details");
}

/// log_rpc with no target evaluates target.unwrap_or("broadcast") only when
/// the callsite is enabled (logger.rs 46).
#[test]
fn test_s4_log_rpc_fields_without_target() {
    s4_tracing_subscriber();
    log_rpc("outgoing", "peer_chat", "s4-req", "node-a", None);
}

/// log_discovery without node_id evaluates node_id.unwrap_or("unknown")
/// (logger.rs 105).
#[test]
fn test_s4_log_discovery_fields_without_node_id() {
    s4_tracing_subscriber();
    log_discovery("found", "10.3.3.3:9000", None);
}
