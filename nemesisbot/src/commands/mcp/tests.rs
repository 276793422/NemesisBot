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

/// stdio 快捷构造（W1 HTTP 化前 `cmd_add` 七参形态的测试等价物——
/// cmd_add 收敛为 AddRequest 后，stdio 用例经此走同一校验入口）。
fn add_stdio(
    cfg: &std::path::Path,
    main_cfg: &std::path::Path,
    name: &str,
    command: &str,
    args: Option<&str>,
    env: &[String],
    timeout: u64,
) -> anyhow::Result<()> {
    let req = super::AddRequest::build(name, Some(command), None, None, &[], args, env, timeout)?;
    super::cmd_add(cfg, main_cfg, &req)
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

    add_stdio(
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
    add_stdio(
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

    add_stdio(
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

    add_stdio(
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
    add_stdio(
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

    add_stdio(
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

    add_stdio(
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
    cmd_discover(None, None, &[], None, 5).await.unwrap();
}

#[tokio::test]
async fn test_s11b_cmd_discover_bad_command_err_arm() {
    // 命令不存在 → spawn 失败 → Err 分支打印 "Discovery failed"（702-704）
    cmd_discover(Some("/definitely/missing/cmd/s11b"), None, &[], None, 5)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_s11b_cmd_discover_dead_url_err_arm() {
    // URL 连不上（本机拒绝端口）→ Err 分支；不经外网（602-604 入口行）
    cmd_discover(None, Some("http://127.0.0.1:1/mcp"), &[], None, 5)
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
    cmd_discover(Some("python"), None, &[], Some(args_str), 15)
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
    let _guard = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let th = s11b_temp_home_env();
    let mcp_cfg = crate::common::mcp_config_path(&th.home);

    // List：无配置文件
    run(McpAction::List, false).unwrap();
    // Add：新配置文件创建
    run(
        McpAction::Add {
            name: "srv".into(),
            command: Some("python".into()),
            url: None,
            transport: None,
            header: vec![],
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
    let _guard = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
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
            header: vec![],
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
        add_stdio(&cfg, &main_cfg_of(&tmp), "second", "python", None, &[], 9).unwrap();
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
        add_stdio(
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
        cmd_discover(Some("python"), None, &[], Some(args_str), 15)
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
        cmd_discover(Some("python"), None, &[], Some(args_str), 15)
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

    add_stdio(
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

    add_stdio(
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

// =========================================================================
// W1（2026-10-15）mcp add HTTP 化：
// - 互斥校验（command/url 恰给其一、形态专属参数、transport 一致性）
// - header 行语法闸（缺冒号 / 空名 / 保留头 / 空引用名；内嵌引用是字面量）
// - http 写盘形态 + serde round-trip；stdio 旧形态键集不变（红槽）
// - run() 分发 http arm；header_vault_aliases / embedded_ref_headers
// - wiremock e2e：cmd_test HTTP 直连（header 真注入）+ 401 诊断分支
// =========================================================================

mod w1_http_add {
    use super::*;

    fn build(
        command: Option<&str>,
        url: Option<&str>,
        transport: Option<&str>,
        headers: &[&str],
        args: Option<&str>,
        env: &[&str],
    ) -> anyhow::Result<super::AddRequest> {
        let h: Vec<String> = headers.iter().map(|s| s.to_string()).collect();
        let e: Vec<String> = env.iter().map(|s| s.to_string()).collect();
        super::AddRequest::build("pet", command, url, transport, &h, args, &e, 30)
    }

    // ------------------------- 互斥与推断 -------------------------

    #[test]
    fn w1_command_url_mutually_exclusive() {
        let err = build(Some("python"), Some("http://x/mcp"), None, &[], None, &[]).unwrap_err();
        assert!(err.to_string().contains("只能给一个"), "实际: {err}");
    }

    #[test]
    fn w1_command_url_both_missing() {
        let err = build(None, None, None, &[], None, &[]).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("--command") && msg.contains("--url"),
            "实际: {msg}"
        );
        assert!(
            msg.contains("mcp add -n pet --url"),
            "错误文案自带 HTTP 示例: {msg}"
        );
    }

    #[test]
    fn w1_empty_string_flags_treated_as_absent() {
        // 空串旗标 = 未给（trim + filter 语义）
        assert!(build(Some("  "), None, None, &[], None, &[]).is_err());
        assert!(build(None, Some(" "), None, &[], None, &[]).is_err());
    }

    #[test]
    fn w1_http_rejects_args_and_env() {
        let e1 = build(None, Some("http://x/mcp"), None, &[], Some("a,b"), &[]).unwrap_err();
        assert!(e1.to_string().contains("--args"), "实际: {e1}");
        let e2 = build(None, Some("http://x/mcp"), None, &[], None, &["K=V"]).unwrap_err();
        assert!(e2.to_string().contains("--env"), "实际: {e2}");
    }

    #[test]
    fn w1_header_rejected_for_stdio() {
        let err = build(Some("python"), None, None, &["A: b"], None, &[]).unwrap_err();
        assert!(err.to_string().contains("--header"), "实际: {err}");
    }

    #[test]
    fn w1_transport_conflicts_and_redundant_consistency() {
        assert!(build(Some("python"), None, Some("http"), &[], None, &[]).is_err());
        assert!(build(None, Some("http://x/mcp"), Some("stdio"), &[], None, &[]).is_err());
        // 显式值与形态一致（冗余）→ OK
        assert!(build(None, Some("http://x/mcp"), Some("http"), &[], None, &[]).is_ok());
        assert!(build(Some("python"), None, Some("stdio"), &[], None, &[]).is_ok());
        // 大小写不敏感
        assert!(build(None, Some("http://x/mcp"), Some("HTTP"), &[], None, &[]).is_ok());
        assert!(build(Some("python"), None, Some("Stdio"), &[], None, &[]).is_ok());
    }

    #[test]
    fn w1_transport_unknown_value_rejected() {
        let err = build(None, Some("http://x/mcp"), Some("sse"), &[], None, &[]).unwrap_err();
        // v1 拍板：sse 不放，错误文案列出支持值
        assert!(err.to_string().contains("sse"), "实际: {err}");
        assert!(err.to_string().contains("http"), "实际: {err}");
    }

    #[test]
    fn w1_transport_inferred_from_form() {
        assert_eq!(
            build(None, Some("http://x/mcp"), None, &[], None, &[])
                .unwrap()
                .transport,
            "http"
        );
        assert_eq!(
            build(Some("python"), None, None, &[], None, &[])
                .unwrap()
                .transport,
            "stdio"
        );
    }

    // ------------------------- header 行语法闸 -------------------------

    #[test]
    fn w1_header_line_validation() {
        // 合法：字面量值
        assert!(build(None, Some("http://x/mcp"), None, &["X-A: v1"], None, &[]).is_ok());
        // 合法：整值引用（语法放行；绝不解引用真值）
        assert!(
            build(
                None,
                Some("http://x/mcp"),
                None,
                &["Authorization: vault:pet-token"],
                None,
                &[]
            )
            .is_ok()
        );
        assert!(
            build(
                None,
                Some("http://x/mcp"),
                None,
                &["Authorization: env:PET_TOKEN"],
                None,
                &[]
            )
            .is_ok()
        );
        assert!(
            build(
                None,
                Some("http://x/mcp"),
                None,
                &["Authorization: yaml:creds.pet"],
                None,
                &[]
            )
            .is_ok()
        );
        // 缺冒号
        assert!(build(None, Some("http://x/mcp"), None, &["no-colon"], None, &[]).is_err());
        // 空头名
        assert!(build(None, Some("http://x/mcp"), None, &[" : v"], None, &[]).is_err());
        // 头名含空白
        assert!(build(None, Some("http://x/mcp"), None, &["Ba d: v"], None, &[]).is_err());
        // 协议保留头
        assert!(
            build(
                None,
                Some("http://x/mcp"),
                None,
                &["Mcp-Session-Id: forged"],
                None,
                &[]
            )
            .is_err()
        );
        // 引用名为空
        assert!(
            build(
                None,
                Some("http://x/mcp"),
                None,
                &["Authorization: vault:"],
                None,
                &[]
            )
            .is_err()
        );
        // 2026-10-15 第五轮复查：引用名带首尾空格——运行期按原样解析
        // （resolve_vault_reference / std::env::var 不 trim），必然失败 →
        // WARN 跳过头 → 401。add 时 loud 拒绝，三前缀同闸。
        assert!(
            build(
                None,
                Some("http://x/mcp"),
                None,
                &["Authorization: vault: pet-token"],
                None,
                &[]
            )
            .is_err()
        );
        assert!(
            build(
                None,
                Some("http://x/mcp"),
                None,
                &["Authorization: env: PET_TOKEN"],
                None,
                &[]
            )
            .is_err()
        );
        assert!(
            build(
                None,
                Some("http://x/mcp"),
                None,
                &["Authorization: yaml: creds.pet"],
                None,
                &[]
            )
            .is_err()
        );
        // 内嵌引用（如 'Bearer vault:x'）：语法上合法（is_secret_reference 只认
        // 整值前缀，内嵌按字面量发送）——由 embedded_ref_headers 警告提示。
        assert!(
            build(
                None,
                Some("http://x/mcp"),
                None,
                &["Authorization: Bearer vault:x"],
                None,
                &[]
            )
            .is_ok()
        );
    }

    #[test]
    fn w1_header_error_message_redacts_value() {
        // 非法头行的错误文案只接头名，不回显值段（值段可能夹带真凭据）
        // 形态一：名含空白（非法），值段带疑似凭据 → 名可见、值被 <redacted>
        let err = build(
            None,
            Some("http://x/mcp"),
            None,
            &["Auth Orization: Bearer sk-live-abc123def456"],
            None,
            &[],
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            !msg.contains("sk-live-abc123def456"),
            "值不得进错误文案: {msg}"
        );
        assert!(msg.contains("Auth Orization"), "实际: {msg}");
        // 形态二：缺冒号 → 整行 <redacted>（无值段可截，名字也不回显）
        let err2 = build(
            None,
            Some("http://x/mcp"),
            None,
            &["Authorization sk-live-abc123def456"],
            None,
            &[],
        )
        .unwrap_err();
        let msg2 = err2.to_string();
        assert!(
            !msg2.contains("sk-live-abc123def456"),
            "值不得进错误文案: {msg2}"
        );
        assert!(msg2.contains("<redacted>"), "实际: {msg2}");
    }

    // ------------------------- 写盘形态 -------------------------

    #[test]
    fn w1_cmd_add_http_writes_http_shape() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_empty_mcp_config(&tmp);
        let req = build(
            None,
            Some("http://127.0.0.1:8808/mcp"),
            None,
            &["Authorization: vault:pet-token"],
            None,
            &[],
        )
        .unwrap();
        super::cmd_add(&cfg, &main_cfg_of(&tmp), &req).unwrap();

        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(data["enabled"], true, "自动 enabled");
        let s = &data["servers"][0];
        assert_eq!(s["name"], "pet");
        assert_eq!(s["transport_type"], "http");
        assert_eq!(s["url"], "http://127.0.0.1:8808/mcp");
        assert_eq!(
            s["headers"][0], "Authorization: vault:pet-token",
            "header 原样落盘"
        );
        assert_eq!(s["timeout_secs"], 30);
        // HTTP 形态不携带 stdio 专属键
        assert!(s.get("command").is_none(), "http 形态不写 command");
        assert!(s.get("args").is_none());
        assert!(s.get("env").is_none());
    }

    #[test]
    fn w1_cmd_add_stdio_shape_key_set_unchanged() {
        // 红槽：stdio 旧写盘形态键集逐字节不变（存量用户零迁移）
        let tmp = TempDir::new().unwrap();
        let cfg = make_empty_mcp_config(&tmp);
        add_stdio(
            &cfg,
            &main_cfg_of(&tmp),
            "legacy",
            "python",
            Some("-m,srv"),
            &[],
            30,
        )
        .unwrap();

        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        let s = &data["servers"][0];
        let mut keys: Vec<&str> = s.as_object().unwrap().keys().map(|k| k.as_str()).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec!["args", "command", "env", "name", "timeout_secs"],
            "stdio 键集不变（无 transport_type/url/headers 新键）"
        );
        assert_eq!(s["command"], "python");
        assert_eq!(s["args"][0], "-m");
        assert_eq!(s["timeout_secs"], 30);
    }

    #[test]
    fn w1_cmd_add_http_roundtrip_serde() {
        // 写盘条目 → McpServerConfig serde 反序列化：与运行时（manager 注册
        // /CLI 连接路径）同一真相源消费
        let tmp = TempDir::new().unwrap();
        let cfg = make_empty_mcp_config(&tmp);
        let req = build(
            None,
            Some("http://127.0.0.1:8808/mcp"),
            Some("http"),
            &["Authorization: vault:pet-token", "X-Trace: t1"],
            None,
            &[],
        )
        .unwrap();
        super::cmd_add(&cfg, &main_cfg_of(&tmp), &req).unwrap();

        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        let config: nemesis_config::McpServerConfig =
            serde_json::from_value(data["servers"][0].clone()).unwrap();
        assert_eq!(config.transport_type, "http");
        assert_eq!(config.url, "http://127.0.0.1:8808/mcp");
        assert_eq!(
            config.headers,
            vec![
                "Authorization: vault:pet-token".to_string(),
                "X-Trace: t1".to_string()
            ]
        );
        assert_eq!(config.timeout_secs, 30);
    }

    // ------------------------- 提示辅助 -------------------------

    #[test]
    fn w1_header_vault_aliases_extraction() {
        let headers = vec![
            serde_json::json!("Authorization: vault:pet-token"),
            serde_json::json!("X-A: literal"),
            serde_json::json!("X-B: Bearer vault:embedded"),
            serde_json::json!("X-C: vault:"),
        ];
        let aliases = super::header_vault_aliases(&headers);
        // 只认整值前缀；内嵌与空别名不算
        assert_eq!(aliases, vec!["pet-token".to_string()]);
    }

    #[test]
    fn w1_embedded_ref_headers_detection() {
        let headers = vec![
            "Authorization: Bearer vault:x".to_string(), // 内嵌 → 命中
            "X-Env: prefix env:VAR".to_string(),         // 内嵌 → 命中
            "Authorization: vault:ok".to_string(),       // 整值 → 不算
            "X-Literal: plain value".to_string(),        // 无引用 → 不算
        ];
        let flagged = super::embedded_ref_headers(&headers);
        assert_eq!(flagged.len(), 2);
        assert!(flagged[0].starts_with("Authorization: Bearer"));
        assert!(flagged[1].starts_with("X-Env:"));
    }

    // ------------------------- run() 分发 -------------------------

    /// RAII 守卫复用 s11b 模式（NEMESISBOT_HOME → 临时根）。
    #[cfg(windows)] // Windows-form helper (Linux nightly: excluded, 2026-09-02 sweep)
    struct W1TempHomeEnv {
        _tmp: TempDir,
    }

    #[cfg(windows)] // Windows-form helper (Linux nightly: excluded, 2026-09-02 sweep)
    impl Drop for W1TempHomeEnv {
        fn drop(&mut self) {
            unsafe { std::env::remove_var("NEMESISBOT_HOME") };
        }
    }

    #[cfg(windows)] // Windows-form helper (Linux nightly: excluded, 2026-09-02 sweep)
    fn w1_temp_home_env() -> (W1TempHomeEnv, std::path::PathBuf) {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join(".nemesisbot");
        std::fs::create_dir_all(home.join("workspace").join("config")).unwrap();
        unsafe { std::env::set_var("NEMESISBOT_HOME", tmp.path()) };
        (W1TempHomeEnv { _tmp: tmp }, home)
    }

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[test]
    fn w1_run_add_http_dispatch_arm() {
        let _guard = crate::GLOBAL_STATE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let (_th, home) = w1_temp_home_env();
        let mcp_cfg = crate::common::mcp_config_path(&home);
        // master switch 同步跳过不存在的 config.json（静默），先种最小主配置
        std::fs::write(crate::common::config_path(&home), "{}").unwrap();

        run(
            McpAction::Add {
                name: "pet".into(),
                command: None,
                url: Some("http://127.0.0.1:8808/mcp".into()),
                transport: None,
                header: vec!["Authorization: vault:pet-token".into()],
                args: None,
                env: vec![],
                timeout: 30,
            },
            false,
        )
        .unwrap();

        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&mcp_cfg).unwrap()).unwrap();
        let s = &data["servers"][0];
        assert_eq!(s["transport_type"], "http");
        assert_eq!(s["url"], "http://127.0.0.1:8808/mcp");
        assert_eq!(s["headers"][0], "Authorization: vault:pet-token");
        // master switch 同步（home 根 config.json，非 workspace/config）
        let main: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(crate::common::config_path(&home)).unwrap(),
        )
        .unwrap();
        assert_eq!(main["mcp"]["enabled"], true);
    }

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[test]
    fn w1_run_add_validation_error_propagates() {
        let _guard = crate::GLOBAL_STATE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let (_th, _home) = w1_temp_home_env();
        // command/url 双给 → run() Err 传播（互斥校验在分发层生效）
        let err = run(
            McpAction::Add {
                name: "bad".into(),
                command: Some("python".into()),
                url: Some("http://x/mcp".into()),
                transport: None,
                header: vec![],
                args: None,
                env: vec![],
                timeout: 30,
            },
            false,
        )
        .unwrap_err();
        assert!(err.to_string().contains("只能给一个"), "实际: {err}");
    }

    // ------------------------- wiremock e2e（真实连通） -------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn w1_cmd_test_http_e2e_header_injected_ok() {
        let server = wiremock::MockServer::start().await;

        // 带 Bearer 头的请求 → 200（header 匹配失败落到 catch-all 401）
        let init_ok = wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "result": {"protocolVersion": "2024-11-05", "capabilities": {},
                        "serverInfo": {"name": "w1fake", "version": "0.1.0"}}
        }));
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            // 精确到 "method":"initialize"：裸 "initialize" 子串会把
            // notifications/initialized 也匹配进来（expect(1) 撞 2 假红）
            .and(wiremock::matchers::body_string_contains(
                "\"method\":\"initialize\"",
            ))
            .and(wiremock::matchers::header("authorization", "Bearer tok123"))
            .respond_with(init_ok)
            .expect(1)
            .mount(&server)
            .await;
        let tools_ok = wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "jsonrpc": "2.0", "id": 2,
            "result": {"tools": [{"name": "echo", "description": "e",
                                    "inputSchema": {"type": "object", "properties": {}}}]}
        }));
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::body_string_contains("tools/list"))
            .and(wiremock::matchers::header("authorization", "Bearer tok123"))
            .respond_with(tools_ok)
            .expect(1)
            .mount(&server)
            .await;
        // catch-all：缺/错鉴权头 → 401
        wiremock::Mock::given(wiremock::matchers::any())
            .respond_with(wiremock::ResponseTemplate::new(401).set_body_string("unauthorized"))
            .mount(&server)
            .await;

        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("config");
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("config.mcp.json");
        let config = serde_json::json!({
            "enabled": true,
            "servers": [{
                "name": "w1pet",
                "transport_type": "http",
                "url": server.uri(),
                "headers": ["Authorization: Bearer tok123"],
                "timeout_secs": 10,
            }]
        });
        std::fs::write(&cfg, serde_json::to_string_pretty(&config).unwrap()).unwrap();

        // http 直连 + header 注入：全绿（401 catch-all 未命中）
        super::cmd_test(&cfg, "w1pet").await.unwrap();
        server.verify().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn w1_cmd_test_http_e2e_vault_ref_unresolved_401_hint_branch() {
        // vault 引用（本测试进程无解析器 → WARN 跳过 → 头未注入）→ 401：
        // 覆盖案例设备的真实失败形态与诊断分支（cmd_test 打印提示后 Ok）
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::any())
            .respond_with(wiremock::ResponseTemplate::new(401).set_body_string("no auth"))
            .mount(&server)
            .await;

        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("config");
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("config.mcp.json");
        let config = serde_json::json!({
            "enabled": true,
            "servers": [{
                "name": "w1vault",
                "transport_type": "http",
                "url": server.uri(),
                "headers": ["Authorization: vault:w1-missing-alias"],
                "timeout_secs": 10,
            }]
        });
        std::fs::write(&cfg, serde_json::to_string_pretty(&config).unwrap()).unwrap();

        super::cmd_test(&cfg, "w1vault").await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn w1_cmd_discover_http_with_header_e2e() {
        // discover --url --header 带认证探测：头匹配 mock 命中 = 注入成立
        let server = wiremock::MockServer::start().await;
        let init_ok = wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "result": {"protocolVersion": "2024-11-05", "capabilities": {},
                        "serverInfo": {"name": "w1disc", "version": "0.2.0"}}
        }));
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            // 同 header_injected_ok：裸子串会误吞 notifications/initialized
            .and(wiremock::matchers::body_string_contains(
                "\"method\":\"initialize\"",
            ))
            .and(wiremock::matchers::header(
                "authorization",
                "Bearer disc-tok",
            ))
            .respond_with(init_ok)
            .expect(1)
            .mount(&server)
            .await;
        for method_marker in ["tools/list", "resources/list", "prompts/list"] {
            wiremock::Mock::given(wiremock::matchers::method("POST"))
                .and(wiremock::matchers::body_string_contains(method_marker))
                .and(wiremock::matchers::header(
                    "authorization",
                    "Bearer disc-tok",
                ))
                .respond_with(
                    wiremock::ResponseTemplate::new(200).set_body_json(
                        serde_json::json!({"jsonrpc": "2.0", "id": 0, "result": {}}),
                    ),
                )
                .mount(&server)
                .await;
        }

        super::cmd_discover(
            None,
            Some(server.uri().as_str()),
            &["Authorization: Bearer disc-tok".to_string()],
            None,
            10,
        )
        .await
        .unwrap();
        server.verify().await;
    }

    // ------------------------- test 形态判定（2026-10-15 复查修复） ---------

    #[test]
    fn w1_server_entry_is_http_matches_dispatch_aliases() {
        // 别名集与 McpClient::from_server_config 同源（大小写敏感语义一致）
        let mk = |v: serde_json::Value| v;
        assert!(super::server_entry_is_http(&mk(serde_json::json!({
            "name": "a", "transport_type": "http", "url": "http://x/mcp"
        }))));
        // 修复目标：streamable_http / streamable-http 别名此前被误判 stdio
        assert!(super::server_entry_is_http(&mk(serde_json::json!({
            "name": "b", "transport_type": "streamable_http", "url": "http://x/mcp"
        }))));
        assert!(super::server_entry_is_http(&mk(serde_json::json!({
            "name": "c", "transport_type": "streamable-http", "url": "http://x/mcp"
        }))));
        // 旧手写兼容：裸 url 无 command 无 transport_type
        assert!(super::server_entry_is_http(&mk(serde_json::json!({
            "name": "d", "url": "http://x/mcp"
        }))));
        // 大写 "HTTP"：运行时分派（大小写敏感）落 stdio 分支——test 同口径，
        // 诚实走 PATH 探测分支，不做两套判定
        assert!(!super::server_entry_is_http(&mk(serde_json::json!({
            "name": "e", "transport_type": "HTTP", "url": "http://x/mcp"
        }))));
        // stdio 形态
        assert!(!super::server_entry_is_http(&mk(serde_json::json!({
            "name": "f", "command": "python", "args": ["-m", "srv"]
        }))));
        // 显式 stdio + url（畸形混写）：分派按 stdio，test 同口径
        assert!(!super::server_entry_is_http(&mk(serde_json::json!({
            "name": "g", "transport_type": "stdio", "url": "http://x/mcp"
        }))));
        // command + url 混写、无 transport_type：分派按 stdio
        assert!(!super::server_entry_is_http(&mk(serde_json::json!({
            "name": "h", "command": "python", "url": "http://x/mcp"
        }))));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn w1_cmd_test_streamable_http_alias_takes_http_branch() {
        // 咬合测试：streamable_http 条目必须走 HTTP 直连（mock 收到请求）；
        // 修复前掉 stdio 分支被 which::which("?") 跳过 → verify() 零请求红。
        let server = wiremock::MockServer::start().await;
        let init_ok = wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "result": {"protocolVersion": "2024-11-05", "capabilities": {},
                        "serverInfo": {"name": "w1alias", "version": "0.1.0"}}
        }));
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::body_string_contains(
                "\"method\":\"initialize\"",
            ))
            .respond_with(init_ok)
            .expect(1)
            .mount(&server)
            .await;

        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("config");
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("config.mcp.json");
        let config = serde_json::json!({
            "enabled": true,
            "servers": [{
                "name": "w1alias",
                "transport_type": "streamable_http",
                "url": server.uri(),
                "timeout_secs": 10,
            }]
        });
        std::fs::write(&cfg, serde_json::to_string_pretty(&config).unwrap()).unwrap();

        super::cmd_test(&cfg, "w1alias").await.unwrap();
        server.verify().await;
    }
}

#[test]
fn r8_display_command_mirrors_stdio_url_fallback() {
    // mcp list 命令显示与分派真相源 stdio 回落同源（Dashboard stdio 约定
    // 命令存 url 字段，无 command 键）。
    // web 形态：command 键缺省，url 即命令
    assert_eq!(
        super::display_command_for(&serde_json::json!({
            "name": "web", "transport_type": "stdio", "url": "python -m srv"
        })),
        "python -m srv"
    );
    // 传统形态：command 键在
    assert_eq!(
        super::display_command_for(&serde_json::json!({
            "name": "cli", "command": "python"
        })),
        "python"
    );
    // command 与 url 并存：command 优先（与分派 command-wins 一致）
    assert_eq!(
        super::display_command_for(&serde_json::json!({
            "name": "both", "command": "python", "url": "python"
        })),
        "python"
    );
    // 显式空 command + 有 url（畸形）：回落 url（与真相源 is_empty 口径一致）
    assert_eq!(
        super::display_command_for(&serde_json::json!({
            "name": "empty-cmd", "command": "", "url": "python"
        })),
        "python"
    );
    // 双空：占位
    assert_eq!(
        super::display_command_for(&serde_json::json!({ "name": "blank" })),
        "?"
    );
}

#[test]
fn r8_server_entry_is_http_empty_command_key_matches_dispatch() {
    // 第八轮对齐：显式 "command": ""（键在但空串）+ 裸 url 无 transport_type
    // ——真相源按 command.is_empty() 判 HTTP 意图，CLI 镜像不得用 is_none()
    // 造成诊断/行为分裂。
    assert!(super::server_entry_is_http(&serde_json::json!({
        "name": "e", "command": "", "url": "http://x/mcp"
    })));
}
