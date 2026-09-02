//! MCP 配置单一真相源契约测试（2026-08-31 收敛配套）。
//!
//! 钉住的契约：**同一份 config.mcp.json，MCP runtime（McpManager）与
//! Dashboard/CLI（nemesis_config::load_mcp_config）必须解析出完全一致的
//! 结果**。收敛前两个消费方各持一份结构定义（`env: Option<Vec<_>>` vs
//! `Vec<_>`、`timeout_secs` vs `timeout`），用户实盘文件里 LLM 代写的
//! `"env": null` 让 Dashboard 解析崩溃而 runtime 一切正常——本测试保证
//! 这类分叉永不回归。
//!
//! 关键样本来自真实事故：`"env": null` + 全局 `timeout` 与 per-server
//! `timeout_secs` 双键并存（`bin_windows/.nemesisbot` 实盘 config.mcp.json
//! 2026-08-30 形状）。

use nemesis_config::load_mcp_config;
use nemesis_mcp::manager::McpManager;

/// 用户实盘崩溃文件（2026-08-30）的精确形状。
const USER_LIVE_FILE: &str = r#"{
  "enabled": true,
  "timeout": 30,
  "servers": [
    {
      "name": "desktop-pet2",
      "command": "",
      "transport_type": "http",
      "url": "http://127.0.0.1:8808/mcp",
      "args": [],
      "env": null,
      "timeout_secs": 30
    }
  ]
}"#;

fn write_file(dir: &std::path::Path, content: &str) -> std::path::PathBuf {
    let path = dir.join("config.mcp.json");
    std::fs::write(&path, content).unwrap();
    path
}

#[test]
fn manager_and_dashboard_parse_live_file_identically() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_file(dir.path(), USER_LIVE_FILE);

    // MCP runtime 路径（agent loop 热重载同款）
    let mgr = McpManager::new(path.clone());
    // Dashboard/CLI 路径（handlers::mcp 同款）
    let cfg = load_mcp_config(&path).unwrap();

    assert!(mgr.is_enabled());
    assert!(cfg.enabled);
    assert_eq!(mgr.list_servers().len(), 1);
    assert_eq!(cfg.servers.len(), 1);

    // 单一真相源的核心断言：两条路径产出逐字段一致的类型
    assert_eq!(mgr.list_servers(), cfg.servers.as_slice());

    let s = &cfg.servers[0];
    assert_eq!(s.name, "desktop-pet2");
    assert_eq!(s.transport_type, "http");
    assert!(s.env.is_empty(), "env:null → 空表，不再解析崩溃");
    assert_eq!(s.timeout_secs, 30);
}

#[test]
fn manager_accepts_claude_desktop_shape() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_file(
        dir.path(),
        r#"{"mcpServers":{"fs":{"command":"npx","args":["-y","fs-server"],"env":{"K":"V"}}}}"#,
    );

    let mgr = McpManager::new(path.clone());
    let cfg = load_mcp_config(&path).unwrap();

    // 外部生态形状两条路径一致接受（LLM 抄配置进盘不再炸）
    assert_eq!(mgr.list_servers(), cfg.servers.as_slice());
    assert!(mgr.is_enabled());
    assert_eq!(cfg.servers[0].name, "fs");
    assert_eq!(cfg.servers[0].env, vec!["K=V".to_string()]);
}

#[test]
fn manager_roundtrip_preserves_server_entries() {
    // add_server → save → 重新 new：存盘-回读闭环（manager 现在写 canonical
    // 全字段形状，Dashboard 读写同形）。
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.mcp.json");

    let mut mgr = McpManager::new(path.clone());
    mgr.add_server(
        nemesis_config::McpServerConfig::new("srv", "node")
            .arg("a.js")
            .env("K=V")
            .timeout(45),
    )
    .unwrap();

    let mgr2 = McpManager::new(path.clone());
    assert_eq!(mgr2.list_servers(), mgr.list_servers());
    assert_eq!(mgr2.list_servers()[0].timeout_secs, 45);

    // 存盘文件 Dashboard 侧同样可读且一致
    let cfg = load_mcp_config(&path).unwrap();
    assert_eq!(mgr2.list_servers(), cfg.servers.as_slice());
}

#[test]
fn manager_tolerates_partial_and_null_fields() {
    let dir = tempfile::tempdir().unwrap();
    // 老式最小条目 + null 字段 + 字符串数字 timeout
    let path = write_file(
        dir.path(),
        r#"{"enabled":true,"servers":[{"name":"min","command":"x","env":null,"headers":null,"tags":null,"timeout":"15"}]}"#,
    );

    let mgr = McpManager::new(path);
    assert!(mgr.is_enabled());
    let s = &mgr.list_servers()[0];
    assert!(s.env.is_empty());
    assert!(s.headers.is_empty());
    assert!(s.tags.is_empty());
    assert_eq!(s.timeout_secs, 15, "字符串数字 timeout 宽容解析");
}
