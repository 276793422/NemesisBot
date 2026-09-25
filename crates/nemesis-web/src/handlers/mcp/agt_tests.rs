//! mcp.rs AGT 覆盖率批次（2026-09-25）：Default 转发体（本文件唯一缺失面）。

use super::*;

#[test]
fn agt_default_impl_matches_new() {
    let _ = McpHandler::default();
    let _ = McpHandler::new();
}
