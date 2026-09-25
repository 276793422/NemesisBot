// loop/tools_trait.rs 覆盖率补充测试（enable_mcp_reload 启用/停用两臂 +
// 幽灵 server 发现失败诚实面 / check_mcp_reload 热重载新 server 发现 /
// record_tool_validation_stats 落账）。
//
// 幽灵 server（command 指向不存在的可执行文件）让 discover_tools 在
// spawn 阶段确定性失败——不依赖真实 MCP server。block_in_place 需要
// multi_thread runtime（current_thread 下 panic）。

use super::*;
use std::sync::Arc;
use std::time::Duration;

struct CovMcpProvider;

#[async_trait]
impl crate::r#loop::LlmProvider for CovMcpProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<crate::r#loop::LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<crate::r#loop::LlmResponse, String> {
        Ok(crate::r#loop::LlmResponse {
            content: "ok".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

fn cov_config() -> crate::types::AgentConfig {
    crate::types::AgentConfig {
        model: "test-model".to_string(),
        system_prompt: None,
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
    }
}

fn mcp_config(path: &std::path::Path, server_name: &str) {
    std::fs::write(
        path,
        serde_json::json!({
            "enabled": true,
            "servers": [{
                "name": server_name,
                "command": "definitely-not-a-real-cov-cmd",
                "args": []
            }]
        })
        .to_string(),
    )
    .unwrap();
}

/// 停用形态：manager 照存（未来启用探测），快照刷新为空。
#[tokio::test]
async fn enable_mcp_reload_disabled_config_keeps_manager() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.mcp.json");
    std::fs::write(&cfg, r#"{"enabled": false, "servers": []}"#).unwrap();

    let mut al = AgentLoop::new(Box::new(CovMcpProvider), cov_config());
    al.enable_mcp_reload(cfg);

    // 无 mcp_ 前缀工具 → 快照空。
    assert!(al.mcp_tool_snapshot().read().is_empty());
    // check_mcp_reload 无变化 → no-op 不 panic。
    al.check_mcp_reload();
}

/// 启用 + 幽灵 server：发现确定性失败 → warn，不影响 loop 状态。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enable_mcp_reload_ghost_server_reports_discovery_failure() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.mcp.json");
    mcp_config(&cfg, "ghost");

    let mut al = AgentLoop::new(Box::new(CovMcpProvider), cov_config());
    al.enable_mcp_reload(cfg);
    al.check_mcp_reload();

    // 发现失败 → 无工具注册，快照空。
    assert!(al.mcp_tool_snapshot().read().is_empty());
}

/// 热重载：配置变更（换 ghost server 名）→ check_config_changed 命中 →
/// find_new_servers 判新（旧注册前缀为空）→ 再发现、再失败、诚实 warn。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn check_mcp_reload_rediscovers_after_config_change() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.mcp.json");
    mcp_config(&cfg, "ghost_a");

    let mut al = AgentLoop::new(Box::new(CovMcpProvider), cov_config());
    al.enable_mcp_reload(cfg.clone());

    // NTFS mtime 粒度防抖。
    tokio::time::sleep(Duration::from_millis(60)).await;
    mcp_config(&cfg, "ghost_b");

    al.check_mcp_reload();
    assert!(al.mcp_tool_snapshot().read().is_empty());
}

/// record_tool_validation_stats：DataStore 在位 → 天×模型 upsert 落账。
#[test]
fn record_tool_validation_stats_persists_rows() {
    let dir = tempfile::tempdir().unwrap();
    let ds = Arc::new(nemesis_data::DataStore::open(&dir.path().join("data.db")).unwrap());

    let mut al = AgentLoop::new(Box::new(CovMcpProvider), cov_config());
    al.data_store = Some(ds.clone());
    al.record_tool_validation_stats(true);
    al.record_tool_validation_stats(false);
    al.record_tool_validation_stats(false);

    let health = ds.query_model_tool_health(1).unwrap();
    assert_eq!(health.len(), 1, "one model row: {health:?}");
    assert_eq!(health[0].model, "test-model");
    assert_eq!(health[0].tool_calls, 3);
    assert_eq!(health[0].validation_failures, 1);
}

/// 无 DataStore → 静默跳过（不 panic）。
#[test]
fn record_tool_validation_stats_without_store_is_noop() {
    let al = AgentLoop::new(Box::new(CovMcpProvider), cov_config());
    al.record_tool_validation_stats(true);
}
