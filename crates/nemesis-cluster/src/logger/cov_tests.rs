// logger.rs 覆盖率补充测试（log_rpc debug 臂 / log_discovery info 臂——
// 带 target/node_id 与 None 兜底两种实参形态）。
//
// 豁免：无。

use super::*;

/// log_rpc：定向 target 与广播 None 两种形态都执行（46）。
#[test]
fn log_rpc_direct_and_broadcast_forms() {
    log_rpc("outbound", "cov.action", "req-1", "node-a", Some("node-b"));
    log_rpc("inbound", "cov.action", "req-2", "node-c", None);
}

/// log_discovery：带 node_id 与 None 兜底两种形态（105）。
#[test]
fn log_discovery_with_and_without_node_id() {
    log_discovery("found", "127.0.0.1:21950", Some("node-x"));
    log_discovery("lost", "127.0.0.1:21951", None);
}
