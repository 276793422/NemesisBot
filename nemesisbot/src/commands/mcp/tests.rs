use super::*;
use tempfile::TempDir;

fn make_mcp_config(tmp: &TempDir) -> std::path::PathBuf {
    let dir = tmp.path().join("config");
    std::fs::create_dir_all(&dir).unwrap();
    let cfg_path = dir.join("config.mcp.json");
    let config = serde_json::json!({
        "enabled": true,
        "servers": [
            {
                "name": "test-server",
                "command": "node",
                "args": ["server.js"],
                "env": ["KEY=value"],
                "timeout": 30
            }
        ]
    });
    std::fs::write(&cfg_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
    cfg_path
}

fn make_empty_mcp_config(tmp: &TempDir) -> std::path::PathBuf {
    let dir = tmp.path().join("config");
    std::fs::create_dir_all(&dir).unwrap();
    let cfg_path = dir.join("config.mcp.json");
    let config = serde_json::json!({"enabled": true, "servers": []});
    std::fs::write(&cfg_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
    cfg_path
}

#[test]
fn test_find_server_found() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_mcp_config(&tmp);
    let server = find_server(&cfg, "test-server").unwrap();
    assert!(server.is_some());
    assert_eq!(server.unwrap()["command"], "node");
}

#[test]
fn test_find_server_not_found() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_mcp_config(&tmp);
    let server = find_server(&cfg, "nonexistent").unwrap();
    assert!(server.is_none());
}

#[test]
fn test_find_server_no_file() {
    let tmp = TempDir::new().unwrap();
    let cfg = tmp.path().join("nonexistent.json");
    let server = find_server(&cfg, "test").unwrap();
    assert!(server.is_none());
}

#[test]
fn test_json_to_server_config_full() {
    // 2026-08-31 单一真相源收敛：手搓 json_to_server_config 已删除，
    // CLI 与 Dashboard/MCP runtime 同用 serde 解析 McpServerConfig。
    let json = serde_json::json!({
        "name": "my-server",
        "command": "python",
        "args": ["-m", "server"],
        "env": ["API_KEY=secret"],
        "timeout": 60
    });
    let config: nemesis_config::McpServerConfig = serde_json::from_value(json).unwrap();
    assert_eq!(config.name, "my-server");
    assert_eq!(config.command, "python");
    assert_eq!(config.args, vec!["-m", "server"]);
    assert_eq!(config.env, vec!["API_KEY=secret".to_string()]);
    assert_eq!(config.timeout_secs, 60);
}

#[test]
fn test_json_to_server_config_minimal() {
    let json = serde_json::json!({"name": "minimal", "command": "echo"});
    let config: nemesis_config::McpServerConfig = serde_json::from_value(json).unwrap();
    assert_eq!(config.name, "minimal");
    assert_eq!(config.command, "echo");
    assert!(config.args.is_empty());
    assert!(config.env.is_empty());
    assert_eq!(config.timeout_secs, 30); // default
}

#[test]
fn test_cmd_add_new_server() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_empty_mcp_config(&tmp);

    cmd_add(
        &cfg,
        &main_cfg_of(&tmp),
        "new-server",
        "python",
        Some("-m,server"),
        &[],
        30,
    )
    .unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    let servers = data["servers"].as_array().unwrap();
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0]["name"], "new-server");
    assert_eq!(servers[0]["command"], "python");
}

#[test]
fn test_cmd_add_duplicate_server() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_mcp_config(&tmp);

    // Should succeed but not add duplicate
    cmd_add(
        &cfg,
        &main_cfg_of(&tmp),
        "test-server",
        "node",
        None,
        &[],
        30,
    )
    .unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    let servers = data["servers"].as_array().unwrap();
    assert_eq!(servers.len(), 1); // still just one
}

#[test]
fn test_cmd_add_creates_new_config() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = dir.join("config.mcp.json");

    cmd_add(
        &cfg,
        &main_cfg_of(&tmp),
        "fresh-server",
        "npx",
        Some("some,mcp"),
        &["KEY=val".to_string()],
        60,
    )
    .unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(data["enabled"], true);
    let servers = data["servers"].as_array().unwrap();
    assert_eq!(servers[0]["name"], "fresh-server");
    assert_eq!(servers[0]["timeout_secs"], 60);
}

#[test]
fn test_cmd_remove_existing_server() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_mcp_config(&tmp);

    cmd_remove(&cfg, "test-server").unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    let servers = data["servers"].as_array().unwrap();
    assert!(servers.is_empty());
}

#[test]
fn test_cmd_remove_nonexistent_server() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_mcp_config(&tmp);

    cmd_remove(&cfg, "nonexistent").unwrap();
    // Should succeed without error, server count unchanged
    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(data["servers"].as_array().unwrap().len(), 1);
}

#[test]
fn test_cmd_remove_no_file() {
    let tmp = TempDir::new().unwrap();
    let cfg = tmp.path().join("nonexistent.json");
    cmd_remove(&cfg, "test").unwrap();
}

#[test]
fn test_cmd_list_no_file() {
    let tmp = TempDir::new().unwrap();
    let cfg = tmp.path().join("nonexistent.json");
    cmd_list(&cfg).unwrap();
}

#[test]
fn test_cmd_list_disabled() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = dir.join("config.mcp.json");
    std::fs::write(&cfg, r#"{"enabled": false, "servers": []}"#).unwrap();
    cmd_list(&cfg).unwrap();
}

#[test]
fn test_cmd_list_with_servers() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_mcp_config(&tmp);
    cmd_list(&cfg).unwrap();
}

#[test]
fn test_cmd_inspect_found() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_mcp_config(&tmp);
    cmd_inspect(&cfg, "test-server").unwrap();
}

#[test]
fn test_cmd_inspect_not_found() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_mcp_config(&tmp);
    cmd_inspect(&cfg, "nonexistent").unwrap();
}

#[test]
fn test_cmd_add_args_parsing() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_empty_mcp_config(&tmp);

    cmd_add(
        &cfg,
        &main_cfg_of(&tmp),
        "test",
        "cmd",
        Some("arg1,arg2,arg3"),
        &[],
        30,
    )
    .unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    let args = data["servers"][0]["args"].as_array().unwrap();
    assert_eq!(args.len(), 3);
    assert_eq!(args[0], "arg1");
    assert_eq!(args[2], "arg3");
}

// -------------------------------------------------------------------------
// (ex-)json_to_server_config edge cases —— 2026-08-31 起直接测 serde 解析
// McpServerConfig（CLI 第三条手搓解析路径已删除，与全仓同真相源）。
// -------------------------------------------------------------------------

#[test]
fn test_json_to_server_config_empty_args() {
    let json = serde_json::json!({
        "name": "test",
        "command": "echo",
        "args": []
    });
    let config: nemesis_config::McpServerConfig = serde_json::from_value(json).unwrap();
    assert!(config.args.is_empty());
}

#[test]
fn test_json_to_server_config_empty_env() {
    let json = serde_json::json!({
        "name": "test",
        "command": "echo",
        "env": []
    });
    let config: nemesis_config::McpServerConfig = serde_json::from_value(json).unwrap();
    // 收敛后 env 是 Vec<String>：空数组保持为空表（null 也归一为空表）
    assert!(config.env.is_empty());
}

#[test]
fn test_json_to_server_config_multiple_env() {
    let json = serde_json::json!({
        "name": "test",
        "command": "python",
        "env": ["KEY1=val1", "KEY2=val2", "KEY3=val3"]
    });
    let config: nemesis_config::McpServerConfig = serde_json::from_value(json).unwrap();
    assert_eq!(config.env.len(), 3);
}

#[test]
fn test_json_to_server_config_zero_timeout() {
    let json = serde_json::json!({
        "name": "test",
        "command": "echo",
        "timeout": 0
    });
    let config: nemesis_config::McpServerConfig = serde_json::from_value(json).unwrap();
    assert_eq!(config.timeout_secs, 0);
}

#[test]
fn test_json_to_server_config_no_name_or_command() {
    let json = serde_json::json!({});
    let config: nemesis_config::McpServerConfig = serde_json::from_value(json).unwrap();
    assert!(config.name.is_empty());
    assert!(config.command.is_empty());
}

// -------------------------------------------------------------------------
// find_server edge cases
// -------------------------------------------------------------------------

#[test]
fn test_find_server_empty_servers() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    std::fs::create_dir_all(&dir).unwrap();
    let cfg_path = dir.join("config.mcp.json");
    std::fs::write(&cfg_path, r#"{"enabled": true, "servers": []}"#).unwrap();

    let result = find_server(&cfg_path, "anything").unwrap();
    assert!(result.is_none());
}

#[test]
fn test_find_server_invalid_json() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = tmp.path().join("config.json");
    std::fs::write(&cfg_path, "not valid json").unwrap();

    let result = find_server(&cfg_path, "test");
    assert!(result.is_err());
}

// -------------------------------------------------------------------------
// cmd_add with environment variables
// -------------------------------------------------------------------------

#[test]
fn test_cmd_add_with_env_vars() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_empty_mcp_config(&tmp);

    let env = vec!["API_KEY=secret123".to_string(), "DEBUG=true".to_string()];
    cmd_add(
        &cfg,
        &main_cfg_of(&tmp),
        "env-server",
        "python",
        None,
        &env,
        60,
    )
    .unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    let server = &data["servers"][0];
    assert_eq!(server["name"], "env-server");
    let env_arr = server["env"].as_array().unwrap();
    assert_eq!(env_arr.len(), 2);
    assert_eq!(env_arr[0], "API_KEY=secret123");
}

// -------------------------------------------------------------------------
// cmd_list with empty config
// -------------------------------------------------------------------------

#[test]
fn test_cmd_list_empty_servers() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_empty_mcp_config(&tmp);
    cmd_list(&cfg).unwrap();
}

#[test]
fn test_cmd_list_with_timeout_and_env() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    std::fs::create_dir_all(&dir).unwrap();
    let cfg_path = dir.join("config.mcp.json");
    let config = serde_json::json!({
        "enabled": true,
        "servers": [
            {
                "name": "full-server",
                "command": "python",
                "args": ["-m", "server"],
                "env": ["KEY=val"],
                "timeout": 60
            }
        ]
    });
    std::fs::write(&cfg_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
    cmd_list(&cfg_path).unwrap();
}

// -------------------------------------------------------------------------
// cmd_inspect edge cases
// -------------------------------------------------------------------------

#[test]
fn test_cmd_inspect_no_file() {
    let tmp = TempDir::new().unwrap();
    let cfg = tmp.path().join("nonexistent.json");
    cmd_inspect(&cfg, "anything").unwrap();
}

// -------------------------------------------------------------------------
// cmd_remove edge cases
// -------------------------------------------------------------------------

#[test]
fn test_cmd_remove_preserves_other_servers() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    std::fs::create_dir_all(&dir).unwrap();
    let cfg_path = dir.join("config.mcp.json");
    let config = serde_json::json!({
        "enabled": true,
        "servers": [
            {"name": "keep-me", "command": "echo"},
            {"name": "remove-me", "command": "rm"}
        ]
    });
    std::fs::write(&cfg_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();

    cmd_remove(&cfg_path, "remove-me").unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    let servers = data["servers"].as_array().unwrap();
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0]["name"], "keep-me");
}

// -------------------------------------------------------------------------
// Additional coverage tests for mcp
// -------------------------------------------------------------------------

#[test]
fn test_mcp_config_read_no_file() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.mcp.json");
    cmd_list(&path).unwrap();
}

#[test]
fn test_mcp_config_invalid_json_find() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.mcp.json");
    std::fs::write(&path, "bad json").unwrap();
    let result = find_server(&path, "test");
    assert!(result.is_err());
}

#[test]
fn test_mcp_config_save_and_read() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("nested").join("config");
    let path = dir.join("config.mcp.json");

    let config = serde_json::json!({
        "enabled": true,
        "servers": [
            {"name": "server1", "command": "cmd1"},
            {"name": "server2", "command": "cmd2", "args": ["--flag"]}
        ]
    });
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(&path, serde_json::to_string_pretty(&config).unwrap()).unwrap();

    let loaded: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(loaded["enabled"], true);
    assert_eq!(loaded["servers"].as_array().unwrap().len(), 2);
}

#[test]
fn test_mcp_cmd_list_empty_config() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_empty_mcp_config(&tmp);
    cmd_list(&cfg).unwrap();
}

#[test]
fn test_mcp_cmd_add_with_env_vars() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_empty_mcp_config(&tmp);

    cmd_add(
        &cfg,
        &main_cfg_of(&tmp),
        "env-server",
        "cmd",
        None,
        &["KEY=VALUE".to_string()],
        30,
    )
    .unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    let servers = data["servers"].as_array().unwrap();
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0]["name"], "env-server");
}

#[test]
fn test_mcp_cmd_add_with_args_and_env() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_empty_mcp_config(&tmp);

    cmd_add(
        &cfg,
        &main_cfg_of(&tmp),
        "full-server",
        "cmd",
        Some("a,b"),
        &["K=V".to_string()],
        60,
    )
    .unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    let server = &data["servers"][0];
    assert_eq!(server["args"].as_array().unwrap().len(), 2);
    assert_eq!(server["timeout_secs"], 60);
}

#[test]
fn test_mcp_cmd_remove_nonexistent_v2() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_mcp_config(&tmp);

    cmd_remove(&cfg, "nonexistent").unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    let servers = data["servers"].as_array().unwrap();
    assert_eq!(servers.len(), 1);
}

#[test]
fn test_mcp_cmd_remove_all() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_mcp_config(&tmp);

    cmd_remove(&cfg, "test-server").unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert!(data["servers"].as_array().unwrap().is_empty());
}

#[test]
fn test_mcp_json_to_server_config_all_fields() {
    let json = serde_json::json!({
        "name": "full",
        "command": "python",
        "args": ["-m", "server"],
        "env": ["KEY=val"],
        "timeout": 120
    });
    let config: nemesis_config::McpServerConfig = serde_json::from_value(json).unwrap();
    assert_eq!(config.name, "full");
    assert_eq!(config.command, "python");
    assert_eq!(config.args.len(), 2);
    assert_eq!(config.timeout_secs, 120);
}

#[test]
fn test_mcp_find_server_found_v2() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_mcp_config(&tmp);
    let result = find_server(&cfg, "test-server").unwrap();
    assert!(result.is_some());
}

#[test]
fn test_mcp_find_server_not_found_v2() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_mcp_config(&tmp);
    let result = find_server(&cfg, "other-server").unwrap();
    assert!(result.is_none());
}

// =========================================================================
// S11b 覆盖率冲刺：sync_mcp_master_switch 全分支 + cmd_test/tools/resources/
// prompts/discover（python 假 MCP 服务器成功路径 + 快失败路径）+ run() 分发。
//
// 假服务器沿用 crates/nemesis-mcp/src/manager/tests.rs 的 W4C_FAKE_MCP 模式
// （newline-delimited JSON-RPC over stdio）；python 缺失时优雅跳过。
// run() 的 async arm（Test/Tools/Resources/Prompts/Discover）内部 block_in_place
// → 必须 multi_thread runtime。
// =========================================================================

const S11B_FAKE_MCP: &str = r#"
import sys, json
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    req = json.loads(line)
    rid = req.get("id")
    if rid is None:
        continue
    m = req.get("method", "")
    if m == "initialize":
        result = {"protocolVersion": "2024-11-05", "capabilities": {}, "serverInfo": {"name": "fake", "version": "1.2.3"}}
    elif m == "tools/list":
        result = {"tools": [{"name": "echo", "description": "echo tool", "inputSchema": {"type": "object", "properties": {"text": {"type": ["string", "null"]}}}}]}
    elif m == "resources/list":
        result = {"resources": [{"uri": "file:///x", "name": "x"}]}
    elif m == "prompts/list":
        result = {"prompts": [{"name": "p1", "description": "pd"}]}
    else:
        result = {}
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": rid, "result": result}) + "\n")
    sys.stdout.flush()
"#;

fn s11b_have_python() -> bool {
    std::process::Command::new("python")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// 在 tmp 下写一个指向 python 假服务器的 mcp 配置，返回配置路径。
fn s11b_fake_mcp_config(tmp: &TempDir) -> std::path::PathBuf {
    let dir = tmp.path().join("config");
    std::fs::create_dir_all(&dir).unwrap();
    let cfg_path = dir.join("config.mcp.json");
    let config = serde_json::json!({
        "enabled": true,
        "servers": [
            {
                "name": "fake",
                "command": "python",
                "args": ["-c", S11B_FAKE_MCP],
                "env": [],
                "timeout": 15
            }
        ]
    });
    std::fs::write(&cfg_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
    cfg_path
}

/// cmd_add 现直传真实主配置路径（2026-08-28 修复 master switch 推导错位）；
/// 测试里主配置默认不存在 → sync 静默跳过，与 cmd_add 测试意图一致。
fn main_cfg_of(tmp: &TempDir) -> std::path::PathBuf {
    tmp.path().join("config.json")
}

// ------------------------- sync_mcp_master_switch -------------------------

// 2026-08-28 契约更新：sync_mcp_master_switch 的参数现在**就是**主配置
// config.json 路径（由调用方直传 common::config_path(&home)），不再从
// config.mcp.json 父目录推导（旧推导指向无人读取的
// `<home>/workspace/config/config.json`，mcp.enabled 从不真正置位）。

#[test]
fn test_s11b_sync_master_switch_nonexistent_path_ok() {
    // 目标 config.json 不存在 → Ok 直接返回，不创建文件
    let tmp = TempDir::new().unwrap();
    let main_cfg = tmp.path().join("config.json");
    sync_mcp_master_switch(&main_cfg, true).unwrap();
    assert!(!main_cfg.exists());
}

#[test]
fn test_s11b_sync_master_switch_already_in_state() {
    let tmp = TempDir::new().unwrap();
    let main_cfg = tmp.path().join("config.json");
    std::fs::write(&main_cfg, r#"{"mcp": {"enabled": true}}"#).unwrap();
    let before = std::fs::read_to_string(&main_cfg).unwrap();
    // 已是 true → Ok 早退，不重写文件
    sync_mcp_master_switch(&main_cfg, true).unwrap();
    assert_eq!(
        std::fs::read_to_string(&main_cfg).unwrap(),
        before,
        "同态不重写"
    );
}

#[test]
fn test_s11b_sync_master_switch_flips_both_ways() {
    let tmp = TempDir::new().unwrap();
    let main_cfg = tmp.path().join("config.json");
    // 无 mcp 段 → 翻 true 会插入 mcp.enabled=true
    std::fs::write(&main_cfg, r#"{"agents": {}}"#).unwrap();
    sync_mcp_master_switch(&main_cfg, true).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&main_cfg).unwrap()).unwrap();
    assert_eq!(v["mcp"]["enabled"], true);
    assert_eq!(v["agents"], serde_json::json!({}), "原有键不丢");
    // true → false
    sync_mcp_master_switch(&main_cfg, false).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&main_cfg).unwrap()).unwrap();
    assert_eq!(v["mcp"]["enabled"], false);
}

#[test]
fn test_s11b_sync_master_switch_flips_real_production_layout() {
    // 生产布局回归钉：CLI 传 <home>/config.json（home 根），修复后 master
    // switch 必须落在**这个**文件，而非 <home>/workspace/config/config.json。
    let tmp = TempDir::new().unwrap();
    let home = tmp.path();
    let main_cfg = home.join("config.json");
    std::fs::write(&main_cfg, r#"{"mcp": {"enabled": false}}"#).unwrap();
    sync_mcp_master_switch(&main_cfg, true).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&main_cfg).unwrap()).unwrap();
    assert_eq!(v["mcp"]["enabled"], true);
    // 旧的错误位置不得出现
    assert!(
        !home
            .join("workspace")
            .join("config")
            .join("config.json")
            .exists()
    );
}

// ------------------------- cmd_test / tools / resources / prompts ---------

#[tokio::test]
async fn test_s11b_cmd_test_not_found_and_missing_command() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_mcp_config(&tmp);
    // 服务器不存在 → 打印后 Ok
    cmd_test(&cfg, "nope").await.unwrap();
    // 服务器存在但命令不在 PATH → 跳过连接测试
    let dir = tmp.path().join("config2");
    std::fs::create_dir_all(&dir).unwrap();
    let cfg2 = dir.join("config.mcp.json");
    std::fs::write(
        &cfg2,
        r#"{"enabled": true, "servers": [{"name": "ghost", "command": "definitely-missing-cmd-s11b", "args": [], "timeout": 5}]}"#,
    )
    .unwrap();
    cmd_test(&cfg2, "ghost").await.unwrap();
}

#[tokio::test]
async fn test_s11b_cmd_test_python_fake_full_pass() {
    if !s11b_have_python() {
        eprintln!("Skipping test: python not available");
        return;
    }
    let tmp = TempDir::new().unwrap();
    let cfg = s11b_fake_mcp_config(&tmp);
    cmd_test(&cfg, "fake").await.unwrap();
}

#[tokio::test]
async fn test_s11b_cmd_tools_not_found_and_python_fake() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_mcp_config(&tmp);
    cmd_tools(&cfg, "nope").await.unwrap();
    if !s11b_have_python() {
        eprintln!("Skipping python part: python not available");
        return;
    }
    let tmp2 = TempDir::new().unwrap();
    let cfg2 = s11b_fake_mcp_config(&tmp2);
    cmd_tools(&cfg2, "fake").await.unwrap();
}

#[tokio::test]
async fn test_s11b_cmd_resources_not_found_and_python_fake() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_mcp_config(&tmp);
    cmd_resources(&cfg, "nope").await.unwrap();
    if !s11b_have_python() {
        eprintln!("Skipping python part: python not available");
        return;
    }
    let tmp2 = TempDir::new().unwrap();
    let cfg2 = s11b_fake_mcp_config(&tmp2);
    cmd_resources(&cfg2, "fake").await.unwrap();
}

#[tokio::test]
async fn test_s11b_cmd_prompts_not_found_and_python_fake() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_mcp_config(&tmp);
    cmd_prompts(&cfg, "nope").await.unwrap();
    if !s11b_have_python() {
        eprintln!("Skipping python part: python not available");
        return;
    }
    let tmp2 = TempDir::new().unwrap();
    let cfg2 = s11b_fake_mcp_config(&tmp2);
    cmd_prompts(&cfg2, "fake").await.unwrap();
}

// ------------------------- cmd_discover -----------------------------------

#[tokio::test]
async fn test_s11b_cmd_discover_no_args_errors() {
    // (None, None) → 打印用法错误后 Ok（614-617）
    cmd_discover(None, None, None, 5).await.unwrap();
}

#[tokio::test]
async fn test_s11b_cmd_discover_bad_command_err_arm() {
    // 命令不存在 → spawn 失败 → Err 分支打印 "Discovery failed"（702-704）
    cmd_discover(Some("/definitely/missing/cmd/s11b"), None, None, 5)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_s11b_cmd_discover_dead_url_err_arm() {
    // URL 连不上（本机拒绝端口）→ Err 分支；不经外网（602-604 入口行）
    cmd_discover(None, Some("http://127.0.0.1:1/mcp"), None, 5)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_s11b_cmd_discover_stdio_python_fake_ok_arm() {
    if !s11b_have_python() {
        eprintln!("Skipping test: python not available");
        return;
    }
    // args 按逗号切分 → 不能内联脚本；写临时 .py 文件（路径无逗号）。
    let tmp = TempDir::new().unwrap();
    let script = tmp.path().join("fake_mcp.py");
    std::fs::write(&script, S11B_FAKE_MCP).unwrap();
    let args_str = script.to_str().unwrap();
    assert!(!args_str.contains(','), "路径含逗号会被 args 切分破坏");
    // Ok 分支全量格式化：server_info + tools + resources + prompts 区段
    cmd_discover(Some("python"), None, Some(args_str), 15)
        .await
        .unwrap();
}

// ------------------------- run() 分发 -------------------------------------

/// RAII 守卫：NEMESISBOT_HOME 指向临时根，drop 时移除（同 cluster/tests.rs 模式）。
#[cfg(windows)] // Windows-form helper (Linux nightly: excluded, 2026-09-02 sweep)
struct S11bTempHomeEnv {
    _tmp: TempDir,
    home: std::path::PathBuf,
}

#[cfg(windows)] // Windows-form helper (Linux nightly: excluded, 2026-09-02 sweep)
impl Drop for S11bTempHomeEnv {
    fn drop(&mut self) {
        unsafe { std::env::remove_var("NEMESISBOT_HOME") };
    }
}

#[cfg(windows)] // Windows-form helper (Linux nightly: excluded, 2026-09-02 sweep)
fn s11b_temp_home_env() -> S11bTempHomeEnv {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    std::fs::create_dir_all(home.join("workspace").join("config")).unwrap();
    unsafe { std::env::set_var("NEMESISBOT_HOME", tmp.path()) };
    S11bTempHomeEnv { _tmp: tmp, home }
}

#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[test]
fn test_s11b_run_sync_arms() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = s11b_temp_home_env();
    let mcp_cfg = crate::common::mcp_config_path(&th.home);

    // List：无配置文件
    run(McpAction::List, false).unwrap();
    // Add：新配置文件创建
    run(
        McpAction::Add {
            name: "srv".into(),
            command: "python".into(),
            args: Some("-c,print(1)".into()),
            env: vec![],
            timeout: 5,
        },
        false,
    )
    .unwrap();
    assert!(mcp_cfg.exists());
    // Inspect：命中
    run(McpAction::Inspect { name: "srv".into() }, false).unwrap();
    // Remove：命中
    run(McpAction::Remove { name: "srv".into() }, false).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&mcp_cfg).unwrap()).unwrap();
    assert!(v["servers"].as_array().unwrap().is_empty());
}

#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_s11b_run_async_dispatch_arms() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = s11b_temp_home_env();
    let _ = &th.home;
    // block_in_place 分发 arm 用「服务器不存在」的快路径逐个覆盖：
    // Test / Tools / Resources / Prompts / Discover。
    run(
        McpAction::Test {
            name: "nope".into(),
        },
        false,
    )
    .unwrap();
    run(
        McpAction::Tools {
            name: "nope".into(),
        },
        false,
    )
    .unwrap();
    run(
        McpAction::Resources {
            name: "nope".into(),
        },
        false,
    )
    .unwrap();
    run(
        McpAction::Prompts {
            name: "nope".into(),
        },
        false,
    )
    .unwrap();
    run(
        McpAction::Discover {
            command: None,
            url: None,
            args: None,
            timeout: 5,
        },
        false,
    )
    .unwrap();
}

// =========================================================================
// wave_b（覆盖率补测 B 波）：
//
// 纯文件臂：find_server 无 servers 键 / sync_mcp_master_switch 真 None-parent /
// cmd_list 无 servers 键与配置路径不可读 / cmd_add 无 servers 数组时静默丢
// server（可疑点 S1，按现状钉住）/ cmd_remove 无 servers 键。
//
// 活服务器臂（沿用 s11b_fake_mcp 的 newline-delimited JSON-RPC stdio 假服
// 务器模式；脚本落盘为 .py 文件、以 args 数组直传 cmd_* 函数，绕开 CLI 逗号
// 切分）：连接失败 / tools/list 报错响应 / 空列表臂 / required 星标与参数
// 渲染 / resource description+mimeType 打印 / prompt arguments 全矩阵 /
// discover 空 list 区段与富渲染区段。全部持 python 探测守卫。
// （discover 的「Server: (unknown)」臂不可达：InitializeResult.serverInfo
// 为必填字段，健康握手后 server_info() 恒为 Some —— 见报告 EXEMPT 表。）
// =========================================================================

mod wave_b {
    use super::*;
    use tempfile::TempDir;

    // ------------------------- 纯文件/解析臂 ------------------------------

    /// find_server：config 存在但没有 "servers" 键 → if-let None 落穿返回
    /// Ok(None)（95 关联区域）。
    #[test]
    fn wave_b_find_server_without_servers_key_returns_none() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("config");
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("config.mcp.json");
        std::fs::write(&cfg, r#"{"enabled": true}"#).unwrap();
        assert!(find_server(&cfg, "any").unwrap().is_none());
    }

    /// sync_mcp_master_switch：mcp_cfg_path 的 parent() 真为 None（根路径 "/",
    /// 以及空路径）→ Ok 早退（160-163）。纯词法操作，不触盘。
    /// 注：既有 test_s11b_sync_master_switch_no_parent_dir 传入裸文件名，
    /// 其 parent() 实际是 Some("")——真正的 None 臂由本测试补上。
    #[test]
    // 2026-08-28 契约更新：参数即主配置路径，parent() 死分支已删除。
    // 不存在的路径 → Ok 无操作（生产对应 home 根 config.json 缺失时静默跳过）。
    fn wave_b_sync_master_switch_nonexistent_paths_ok() {
        sync_mcp_master_switch(std::path::Path::new("/definitely-missing-cfg"), true).unwrap();
        sync_mcp_master_switch(std::path::Path::new(""), false).unwrap();
    }

    /// cmd_list：enabled 配置但无 "servers" 键 → else 臂打印 0 服务（253-256）。
    #[test]
    fn wave_b_cmd_list_enabled_config_without_servers_key() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("config");
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("config.mcp.json");
        std::fs::write(&cfg, r#"{"enabled": true}"#).unwrap();
        cmd_list(&cfg).unwrap();
    }

    /// cmd_list：配置路径存在但不可读（目录）→ 首个 read_to_string Err
    /// 跳过 disabled 检查（192-199 家族），随后 exists()+read 触发 Err 传播。
    #[test]
    fn wave_b_cmd_list_unreadable_directory_config_propagates_err() {
        let tmp = TempDir::new().unwrap();
        let cfg = tmp.path().join("not-a-file");
        std::fs::create_dir_all(&cfg).unwrap();
        assert!(cmd_list(&cfg).is_err(), "目录当配置文件必须 Err");
    }

    /// （历史注记）cmd_add 对无 servers 数组的旧配置曾静默丢弃服务器，
    /// BUG 台账 #38 修复为自动补建数组并持久化 —— 行为由文件尾部两条
    /// test_cmd_add_missing_servers_key_still_persists / non_array 回归钉住。
    ///
    /// cmd_remove：config 存在但无 "servers" 键 → if-let None（336 区），
    /// found=false 走 not-found 提示且不回写。
    #[test]
    fn wave_b_cmd_remove_on_config_without_servers_key_reports_not_found() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("config");
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("config.mcp.json");
        std::fs::write(&cfg, r#"{"enabled": true}"#).unwrap();
        cmd_remove(&cfg, "ghost").unwrap();
        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert!(data.get("servers").is_none(), "未找到时不新增键");
    }

    /// cmd_add 对既有 servers 数组的两条路径：
    /// 相位1 无重名 → for 完整扫描掉落（289/291）、push 追加成功、
    /// 主开关 false→true 翻转并写盘 —— 这是有数组时的正常路径，
    /// 与下方 S1 可疑点（无数组时静默丢弃）互为对照。
    /// 相位2 重名 → 扫描命中直接早退（285-288）：不追加、不覆盖旧条目、
    /// 主开关保持原值不翻转。
    #[test]
    fn wave_b_cmd_add_append_then_duplicate_short_circuit() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("config");
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("config.mcp.json");
        std::fs::write(
            &cfg,
            r#"{"enabled": false, "servers": [{"name": "first", "command": "original-cmd", "args": [], "env": [], "timeout": 5}]}"#,
        )
        .unwrap();

        // 相位1：追加新名
        cmd_add(&cfg, &main_cfg_of(&tmp), "second", "python", None, &[], 9).unwrap();
        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        let servers = data["servers"].as_array().unwrap();
        assert_eq!(servers.len(), 2, "无重名时追加一条");
        assert_eq!(servers[0]["name"], "first", "旧条目不动");
        assert_eq!(servers[1]["name"], "second");
        assert_eq!(servers[1]["timeout_secs"], 9);
        assert_eq!(data["enabled"], true, "主开关翻转落盘");

        // 相位2：重名短路（快照对比整份文件不变）
        let before = std::fs::read_to_string(&cfg).unwrap();
        cmd_add(
            &cfg,
            &main_cfg_of(&tmp),
            "second",
            "replacement-cmd",
            None,
            &[],
            77,
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(&cfg).unwrap(),
            before,
            "重名时早退，文件零改动"
        );
        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(
            data["servers"].as_array().unwrap()[1]["command"],
            "python",
            "旧条目不被同名替换"
        );
    }

    // ------------------------- 假服务器臂 ---------------------------------

    const WB_DEAD_SERVER: &str = "import sys\nsys.exit(0)\n";

    const WB_ERR_TOOLS_SERVER: &str = r#"
import sys, json
def send(o):
    sys.stdout.write(json.dumps(o) + "\n")
    sys.stdout.flush()
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    req = json.loads(line)
    rid = req.get("id")
    if rid is None:
        continue
    m = req.get("method", "")
    if m == "initialize":
        send({"jsonrpc": "2.0", "id": rid, "result": {"protocolVersion": "2024-11-05", "capabilities": {}, "serverInfo": {"name": "wberr", "version": "0.0.1"}}})
    elif m == "tools/list":
        send({"jsonrpc": "2.0", "id": rid, "error": {"code": -32601, "message": "wb intentional tools error"}})
    else:
        send({"jsonrpc": "2.0", "id": rid, "result": {"tools": [], "resources": [], "prompts": []}})
"#;

    const WB_MIN_SERVER: &str = r#"
import sys, json
def send(o):
    sys.stdout.write(json.dumps(o) + "\n")
    sys.stdout.flush()
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    req = json.loads(line)
    rid = req.get("id")
    if rid is None:
        continue
    m = req.get("method", "")
    if m == "initialize":
        send({"jsonrpc": "2.0", "id": rid, "result": {"protocolVersion": "2024-11-05", "capabilities": {}, "serverInfo": {"name": "wbmin", "version": "0.0.2"}}})
    else:
        send({"jsonrpc": "2.0", "id": rid, "result": {"tools": [], "resources": [], "prompts": []}})
"#;

    const WB_RICH_SERVER: &str = r#"
import sys, json
def send(o):
    sys.stdout.write(json.dumps(o) + "\n")
    sys.stdout.flush()
RICH_TOOLS = [
    {"name": "starred", "description": "tool with required param",
     "inputSchema": {"type": "object",
                     "properties": {"path": {"type": "string"}, "force": {"type": "boolean"}},
                     "required": ["path"]}},
    {"name": "schemaless", "description": "schema without properties",
     "inputSchema": {"type": "object"}}
]
RICH_RESOURCES = [
    {"uri": "file:///rich", "name": "rich", "description": "rich description text", "mimeType": "text/plain"},
    {"uri": "file:///blank", "name": "blank", "description": "", "mimeType": ""}
]
RICH_PROMPTS = [
    {"name": "prich", "description": "prich desc",
     "arguments": [{"name": "reqd", "description": "give me a value", "required": True},
                   {"name": "optz", "description": "", "required": False},
                   {"name": "ghost"}]},
    {"name": "nodesc"},
    {"name": "chatty", "description": "talks but takes no arguments"}
]
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    req = json.loads(line)
    rid = req.get("id")
    if rid is None:
        continue
    m = req.get("method", "")
    if m == "initialize":
        send({"jsonrpc": "2.0", "id": rid, "result": {"protocolVersion": "2024-11-05", "capabilities": {}, "serverInfo": {"name": "wbrich", "version": "0.0.3"}}})
    elif m == "tools/list":
        send({"jsonrpc": "2.0", "id": rid, "result": {"tools": RICH_TOOLS}})
    elif m == "resources/list":
        send({"jsonrpc": "2.0", "id": rid, "result": {"resources": RICH_RESOURCES}})
    elif m == "prompts/list":
        send({"jsonrpc": "2.0", "id": rid, "result": {"prompts": RICH_PROMPTS}})
    else:
        send({"jsonrpc": "2.0", "id": rid, "result": {}})
"#;

    /// 写一份指向 python 假服务器的 mcp 配置；脚本作为 .py 文件落盘，
    /// 经 config args 数组直传（不经 CLI 逗号切分）。返回 (配置路径, 脚本路径)。
    fn wb_fake_server(tmp: &TempDir, tag: &str, script: &str, timeout: u64) -> std::path::PathBuf {
        let dir = tmp.path().join(tag).join("config");
        std::fs::create_dir_all(&dir).unwrap();
        let script_path = tmp.path().join(tag).join("fake_server.py");
        std::fs::write(&script_path, script).unwrap();
        let cfg_path = dir.join("config.mcp.json");
        let config = serde_json::json!({
            "enabled": true,
            "servers": [{
                "name": format!("wb-{tag}"),
                "command": "python",
                "args": [script_path.to_string_lossy()],
                "env": [],
                "timeout": timeout,
            }]
        });
        std::fs::write(&cfg_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
        cfg_path
    }

    /// cmd_test：命令在 PATH 但进程秒退 → initialize 失败 → 连接 FAILED
    /// 错误臂（402-405）。
    #[tokio::test]
    async fn wave_b_cmd_test_dead_process_prints_connection_failed_arm() {
        if !s11b_have_python() {
            eprintln!("Skipping test: python not available");
            return;
        }
        let tmp = TempDir::new().unwrap();
        let cfg = wb_fake_server(&tmp, "dead", WB_DEAD_SERVER, 15);
        cmd_test(&cfg, "wb-dead").await.unwrap();
    }

    /// cmd_test：tools/list 返回 JSON-RPC error 对象 → list_tools Err →
    /// 「Tools: error - ...」臂（391）；随后 close 正常。
    #[tokio::test]
    async fn wave_b_cmd_test_tools_error_response_prints_error_line() {
        if !s11b_have_python() {
            eprintln!("Skipping test: python not available");
            return;
        }
        let tmp = TempDir::new().unwrap();
        let cfg = wb_fake_server(&tmp, "err", WB_ERR_TOOLS_SERVER, 15);
        cmd_test(&cfg, "wb-err").await.unwrap();
    }

    /// cmd_tools：空列表 → 「No tools available.」（428-429）；
    /// 富列表 → required 星标分支（455）+ inputSchema 无 properties 时跳过
    /// 参数转储（465 关联区域）+ 双工具渲染。
    #[tokio::test]
    async fn wave_b_cmd_tools_empty_then_rich_rendering() {
        if !s11b_have_python() {
            eprintln!("Skipping test: python not available");
            return;
        }
        let tmp = TempDir::new().unwrap();
        let min_cfg = wb_fake_server(&tmp, "mintools", WB_MIN_SERVER, 15);
        cmd_tools(&min_cfg, "wb-mintools").await.unwrap();

        let rich_cfg = wb_fake_server(&tmp, "richtools", WB_RICH_SERVER, 15);
        cmd_tools(&rich_cfg, "wb-richtools").await.unwrap();
    }

    /// cmd_resources：空 → 「No resources available.」（494-495）；富 →
    /// 非空 description 打印（503-505）、非空 mimeType 打印（508-510）、
    /// 以及同循环内空串跳过臂。
    #[tokio::test]
    async fn wave_b_cmd_resources_empty_then_rich_rendering() {
        if !s11b_have_python() {
            eprintln!("Skipping test: python not available");
            return;
        }
        let tmp = TempDir::new().unwrap();
        let min_cfg = wb_fake_server(&tmp, "minres", WB_MIN_SERVER, 15);
        cmd_resources(&min_cfg, "wb-minres").await.unwrap();

        let rich_cfg = wb_fake_server(&tmp, "richres", WB_RICH_SERVER, 15);
        cmd_resources(&rich_cfg, "wb-richres").await.unwrap();
    }

    /// cmd_prompts：空 → 「No prompts available.」（540-541）；富 →
    /// arguments 全矩阵：required=true 星标 / required=false 素名 / 无 required
    /// 字段 / 有描述 / 空描述 / 缺描述字段 / prompt 无 description（551 区）/
    /// 无 arguments 的 prompt（552 假臂→570 区）。
    #[tokio::test]
    async fn wave_b_cmd_prompts_empty_then_rich_argument_matrix() {
        if !s11b_have_python() {
            eprintln!("Skipping test: python not available");
            return;
        }
        let tmp = TempDir::new().unwrap();
        let min_cfg = wb_fake_server(&tmp, "minprompts", WB_MIN_SERVER, 15);
        cmd_prompts(&min_cfg, "wb-minprompts").await.unwrap();

        let rich_cfg = wb_fake_server(&tmp, "richprompts", WB_RICH_SERVER, 15);
        cmd_prompts(&rich_cfg, "wb-richprompts").await.unwrap();
    }

    /// discover（stdio）：握手成功 + 四类列表全空 → Tools/Resources/Prompts
    /// 三个 "(none)" 臂（631-632 / 670-671 / 686-687）与 Discovery complete。
    #[tokio::test]
    async fn wave_b_cmd_discover_min_renders_all_none_sections() {
        if !s11b_have_python() {
            eprintln!("Skipping test: python not available");
            return;
        }
        let tmp = TempDir::new().unwrap();
        let script_path = tmp.path().join("min_disc.py");
        std::fs::write(&script_path, WB_MIN_SERVER).unwrap();
        let args_str = script_path.to_str().unwrap();
        assert!(!args_str.contains(','));
        cmd_discover(Some("python"), None, Some(args_str), 15)
            .await
            .unwrap();
    }

    /// discover（stdio）：富渲染 → 工具 required 星标（655）+
    /// schemaless 工具 properties-None 跳过（664 区）、resource description
    /// 打印（677-679）、prompt description 打印（696）。
    #[tokio::test]
    async fn wave_b_cmd_discover_rich_renders_required_marks_and_descriptions() {
        if !s11b_have_python() {
            eprintln!("Skipping test: python not available");
            return;
        }
        let tmp = TempDir::new().unwrap();
        let script_path = tmp.path().join("rich_disc.py");
        std::fs::write(&script_path, WB_RICH_SERVER).unwrap();
        let args_str = script_path.to_str().unwrap();
        assert!(!args_str.contains(','));
        cmd_discover(Some("python"), None, Some(args_str), 15)
            .await
            .unwrap();
    }
}

// ---------------------------------------------------------------------------
// BUG 台账 #38 回归：已存在的 config.mcp.json 缺失 "servers" 键（旧 schema /
// 手工编辑）时，原实现的 get_mut(servers) 静默 no-op 仍打印成功 —— 服务器
// 根本没写进文件。新契约：缺键/非数组 ⇒ 自动补建空数组后插入，必定持久化。
// ---------------------------------------------------------------------------

#[test]
fn test_cmd_add_missing_servers_key_still_persists() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = dir.join("config.mcp.json");
    // 有文件但没有 servers 数组 —— 修复前这里静默丢服务器。
    std::fs::write(&cfg, r#"{"enabled": false}"#).unwrap();

    cmd_add(
        &cfg,
        &main_cfg_of(&tmp),
        "late-server",
        "python",
        None,
        &[],
        30,
    )
    .unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(data["enabled"], true);
    let servers = data["servers"]
        .as_array()
        .expect("servers 必须被补建成数组");
    assert_eq!(servers.len(), 1, "服务器必须真实落盘（恰好一条）");
    assert_eq!(servers[0]["name"], "late-server");
    assert_eq!(servers[0]["command"], "python");
}

#[test]
fn test_cmd_add_non_array_servers_is_repaired_not_dropped() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = dir.join("config.mcp.json");
    std::fs::write(&cfg, r#"{"enabled": true, "servers": {}}"#).unwrap();

    cmd_add(
        &cfg,
        &main_cfg_of(&tmp),
        "healed-server",
        "node",
        None,
        &[],
        30,
    )
    .unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    let servers = data["servers"]
        .as_array()
        .expect("坏类型 servers 必须重置为数组");
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0]["name"], "healed-server");
}

// ===========================================================================
// r10（覆盖率 A 类 miss 补充）：
// - cmd_list 的「配置存在但无 enabled 键」通路：显式 disabled 早退判否
//   （cfg.get("enabled") 为 None ≠ Some(false)）后落穿收口括号区，servers
//   渲染走满 args join / timeout>0 / env>0 展示臂。wave_b 已有无 servers
//   键变体、旧批次有显式 false 变体；本夹具补齐缺失键 + 带 servers 组合。
// - 诚实边界：discover "(unknown)" 臂按 wave_b 头注结构性豁免
//  （InitializeResult.serverInfo 必填 → 健康握手后 server_info 恒 Some）；
//   run() Add 尾臂由 test_s11b_run_sync_arms 覆盖。
// ===========================================================================

#[test]
fn r10_cmd_list_without_enabled_key_renders_servers_fully() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = dir.join("config.mcp.json");
    std::fs::write(
        &cfg,
        serde_json::json!({
            "servers": [
                {
                    "name": "r10-srv",
                    "command": "python",
                    "args": ["-m", "server"],
                    "env": ["K=V", "K2=V2"],
                    "timeout": 60
                }
            ]
        })
        .to_string(),
    )
    .unwrap();

    // 无 enabled 键：不得被当作 disabled 早退，必须完整渲染 servers 列表。
    cmd_list(&cfg).unwrap();
}
