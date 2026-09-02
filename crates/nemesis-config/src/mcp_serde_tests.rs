//! MCP 配置宽容反序列化测试（2026-08-31 单一真相源收敛配套）。
//!
//! 根因背景：用户实盘 config.mcp.json 出现 `"env": null`（LLM 代写）时，
//! Dashboard 侧严格解析直接崩溃（"invalid type: null, expected a
//! sequence"），而 MCP runtime（当时的 nemesis-mcp 私有结构）却一切正常
//! ——双真相源让同一份文件呈现两种命运。收敛后本文件钉住宽容契约：
//! `null`/map/标量形状的列表字段、`timeout`/`timeout_secs` 双键、Claude
//! Desktop `mcpServers` 外部形状、空/残缺配置，全部可解析。

use super::*;

// ---------------------------------------------------------------------------
// 用户实盘文件形状（回归锚点）
// ---------------------------------------------------------------------------

/// 用户实盘崩溃文件的精确形状：全局 `timeout` + per-server `timeout_secs`
/// 双键并存 + `"env": null` + `"args": []`。收敛前 Dashboard 在此崩溃。
#[test]
fn user_live_file_with_null_env_parses() {
    let json = r#"{
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
    let cfg: McpConfig = serde_json::from_str(json).unwrap();
    assert!(cfg.enabled);
    assert_eq!(cfg.timeout, 30);
    assert_eq!(cfg.servers.len(), 1);
    let s = &cfg.servers[0];
    assert_eq!(s.name, "desktop-pet2");
    assert_eq!(s.transport_type, "http");
    assert_eq!(s.url, "http://127.0.0.1:8808/mcp");
    assert!(s.env.is_empty(), "env:null must become empty, not error");
    assert_eq!(s.timeout_secs, 30, "timeout_secs alias must be accepted");
}

// ---------------------------------------------------------------------------
// 列表字段宽容（env/args/headers/tags 同一 helper）
// ---------------------------------------------------------------------------

#[test]
fn env_map_shape_converts_to_kv_list() {
    let json = r#"{"name":"n","command":"c","env":{"DEBUG":"1","LOG_LEVEL":"info"}}"#;
    let cfg: McpServerConfig = serde_json::from_str(json).unwrap();
    assert_eq!(
        cfg.env,
        vec!["DEBUG=1".to_string(), "LOG_LEVEL=info".to_string()]
    );
}

#[test]
fn env_null_and_args_null_become_empty() {
    let json = r#"{"name":"n","command":"c","env":null,"args":null,"headers":null,"tags":null}"#;
    let cfg: McpServerConfig = serde_json::from_str(json).unwrap();
    assert!(cfg.env.is_empty());
    assert!(cfg.args.is_empty());
    assert!(cfg.headers.is_empty());
    assert!(cfg.tags.is_empty());
}

#[test]
fn env_array_unchanged_and_null_elements_skipped() {
    let json = r#"{"name":"n","command":"c","env":["A=1",null,"B"]}"#;
    let cfg: McpServerConfig = serde_json::from_str(json).unwrap();
    assert_eq!(cfg.env, vec!["A=1".to_string(), "B".to_string()]);
}

#[test]
fn env_scalar_becomes_single_element() {
    let json = r#"{"name":"n","command":"c","env":"A=1"}"#;
    let cfg: McpServerConfig = serde_json::from_str(json).unwrap();
    assert_eq!(cfg.env, vec!["A=1".to_string()]);
}

// ---------------------------------------------------------------------------
// timeout 双键 + 宽容
// ---------------------------------------------------------------------------

#[test]
fn timeout_key_and_alias_both_accepted() {
    // 线上键名（存盘/Dashboard/UI 历史形状）
    let a: McpServerConfig = serde_json::from_str(r#"{"name":"n","timeout":15}"#).unwrap();
    assert_eq!(a.timeout_secs, 15);
    // 别名（nemesis-mcp 旧结构形状 / e2e fixture）
    let b: McpServerConfig = serde_json::from_str(r#"{"name":"n","timeout_secs":45}"#).unwrap();
    assert_eq!(b.timeout_secs, 45);
    // 缺省 → 30
    let c: McpServerConfig = serde_json::from_str(r#"{"name":"n"}"#).unwrap();
    assert_eq!(c.timeout_secs, 30);
    // null / 字符串数字 → 宽容
    let d: McpServerConfig = serde_json::from_str(r#"{"name":"n","timeout":null}"#).unwrap();
    assert_eq!(d.timeout_secs, 30);
    let e: McpServerConfig = serde_json::from_str(r#"{"name":"n","timeout":"60"}"#).unwrap();
    assert_eq!(e.timeout_secs, 60);
}

#[test]
fn global_timeout_tolerant() {
    let a: McpConfig =
        serde_json::from_str(r#"{"enabled":true,"servers":[],"timeout":120}"#).unwrap();
    assert_eq!(a.timeout, 120);
    let b: McpConfig =
        serde_json::from_str(r#"{"enabled":true,"servers":[],"timeout":null}"#).unwrap();
    assert_eq!(b.timeout, 30);
}

// ---------------------------------------------------------------------------
// Claude Desktop 外部形状
// ---------------------------------------------------------------------------

#[test]
fn claude_desktop_mcpservers_map_shape() {
    let json = r#"{
        "mcpServers": {
            "filesystem": {
                "command": "npx",
                "args": ["-y", "@modelcontextprotocol/server-filesystem"],
                "env": {"HOME": "/Users/x"}
            },
            "pet": { "url": "http://127.0.0.1:8808/mcp", "transport_type": "http" }
        }
    }"#;
    let cfg: McpConfig = serde_json::from_str(json).unwrap();
    // 配置存在即启用
    assert!(cfg.enabled);
    assert_eq!(cfg.servers.len(), 2);
    // name 从 map key 取
    assert_eq!(cfg.servers[0].name, "filesystem");
    assert_eq!(cfg.servers[0].command, "npx");
    assert_eq!(cfg.servers[0].env, vec!["HOME=/Users/x".to_string()]);
    assert_eq!(cfg.servers[1].name, "pet");
    assert_eq!(cfg.servers[1].transport_type, "http");
}

#[test]
fn mcpservers_array_shape_accepted() {
    let json = r#"{"enabled":false,"mcpServers":[{"name":"a","command":"x"}]}"#;
    let cfg: McpConfig = serde_json::from_str(json).unwrap();
    assert!(!cfg.enabled, "explicit top-level enabled must win");
    assert_eq!(cfg.servers.len(), 1);
    assert_eq!(cfg.servers[0].name, "a");
}

// ---------------------------------------------------------------------------
// 空/残缺配置容忍
// ---------------------------------------------------------------------------

#[test]
fn empty_and_partial_configs_tolerated() {
    let a: McpConfig = serde_json::from_str("{}").unwrap();
    assert!(!a.enabled);
    assert!(a.servers.is_empty());
    assert_eq!(a.timeout, 30);

    let b: McpConfig = serde_json::from_str(r#"{"enabled":true}"#).unwrap();
    assert!(b.enabled);
    assert!(b.servers.is_empty());
}

// ---------------------------------------------------------------------------
// 序列化契约（线上键名稳定）
// ---------------------------------------------------------------------------

#[test]
fn serialize_emits_timeout_wire_key_and_roundtrips() {
    let mut s = McpServerConfig::new("srv", "node").arg("a.js").env("K=V");
    s.normalize();
    let json = serde_json::to_string(&s).unwrap();
    // UI / CLI / 存量工具读的都是 "timeout" 键——序列化不得改名
    assert!(
        json.contains(r#""timeout":"#),
        "wire key must stay 'timeout': {json}"
    );
    assert!(!json.contains("timeout_secs"), "no stray alias key: {json}");

    let back: McpServerConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(back, s, "roundtrip must be lossless (PartialEq)");
}

#[test]
fn mcp_config_roundtrip_lossless() {
    let cfg = McpConfig {
        enabled: true,
        servers: vec![McpServerConfig::new("s1", "cmd").arg("--x").env("A=1")],
        timeout: 60,
    };
    let json = serde_json::to_string_pretty(&cfg).unwrap();
    let parsed: McpConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, cfg);
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

#[test]
fn builder_methods_match_legacy_nemesis_mcp_api() {
    let cfg = McpServerConfig::new("test", "node")
        .arg("server.js")
        .env("FOO=bar")
        .timeout(60);
    assert_eq!(cfg.name, "test");
    assert_eq!(cfg.command, "node");
    assert_eq!(cfg.args, vec!["server.js"]);
    assert_eq!(cfg.env, vec!["FOO=bar".to_string()]);
    assert_eq!(cfg.timeout_secs, 60);
}

// ---------------------------------------------------------------------------
// 真实 loader 端到端（Dashboard 同路径）
// ---------------------------------------------------------------------------

#[test]
fn load_mcp_config_end_to_end_with_null_env() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.mcp.json");
    std::fs::write(
        &path,
        r#"{"enabled":true,"servers":[{"name":"x","url":"http://h/mcp","transport_type":"http","env":null,"timeout_secs":30}]}"#,
    )
    .unwrap();
    let cfg = load_mcp_config(&path).unwrap();
    assert!(cfg.enabled);
    assert_eq!(cfg.servers.len(), 1);
    assert!(cfg.servers[0].env.is_empty());
    assert_eq!(cfg.servers[0].timeout_secs, 30);
}

#[test]
fn load_mcp_config_claude_desktop_shape_via_loader() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.mcp.json");
    std::fs::write(
        &path,
        r#"{"mcpServers":{"fs":{"command":"npx","args":["-y","fs-server"]}}}"#,
    )
    .unwrap();
    let cfg = load_mcp_config(&path).unwrap();
    assert!(cfg.enabled);
    assert_eq!(cfg.servers.len(), 1);
    assert_eq!(cfg.servers[0].name, "fs");
}
