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
    let json = serde_json::json!({
        "name": "my-server",
        "command": "python",
        "args": ["-m", "server"],
        "env": ["API_KEY=secret"],
        "timeout": 60
    });
    let config = json_to_server_config(&json);
    assert_eq!(config.name, "my-server");
    assert_eq!(config.command, "python");
    assert_eq!(config.args, vec!["-m", "server"]);
    assert_eq!(config.env, Some(vec!["API_KEY=secret".to_string()]));
    assert_eq!(config.timeout_secs, 60);
}

#[test]
fn test_json_to_server_config_minimal() {
    let json = serde_json::json!({"name": "minimal", "command": "echo"});
    let config = json_to_server_config(&json);
    assert_eq!(config.name, "minimal");
    assert_eq!(config.command, "echo");
    assert!(config.args.is_empty());
    assert!(config.env.is_none());
    assert_eq!(config.timeout_secs, 30); // default
}

#[test]
fn test_cmd_add_new_server() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_empty_mcp_config(&tmp);

    cmd_add(&cfg, "new-server", "python", Some("-m,server"), &[], 30).unwrap();

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
    cmd_add(&cfg, "test-server", "node", None, &[], 30).unwrap();

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
    assert_eq!(servers[0]["timeout"], 60);
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

    cmd_add(&cfg, "test", "cmd", Some("arg1,arg2,arg3"), &[], 30).unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    let args = data["servers"][0]["args"].as_array().unwrap();
    assert_eq!(args.len(), 3);
    assert_eq!(args[0], "arg1");
    assert_eq!(args[2], "arg3");
}

// -------------------------------------------------------------------------
// json_to_server_config edge cases
// -------------------------------------------------------------------------

#[test]
fn test_json_to_server_config_empty_args() {
    let json = serde_json::json!({
        "name": "test",
        "command": "echo",
        "args": []
    });
    let config = json_to_server_config(&json);
    assert!(config.args.is_empty());
}

#[test]
fn test_json_to_server_config_empty_env() {
    let json = serde_json::json!({
        "name": "test",
        "command": "echo",
        "env": []
    });
    let config = json_to_server_config(&json);
    assert!(config.env.is_some());
    assert!(config.env.as_ref().unwrap().is_empty());
}

#[test]
fn test_json_to_server_config_multiple_env() {
    let json = serde_json::json!({
        "name": "test",
        "command": "python",
        "env": ["KEY1=val1", "KEY2=val2", "KEY3=val3"]
    });
    let config = json_to_server_config(&json);
    assert_eq!(config.env.unwrap().len(), 3);
}

#[test]
fn test_json_to_server_config_zero_timeout() {
    let json = serde_json::json!({
        "name": "test",
        "command": "echo",
        "timeout": 0
    });
    let config = json_to_server_config(&json);
    assert_eq!(config.timeout_secs, 0);
}

#[test]
fn test_json_to_server_config_no_name_or_command() {
    let json = serde_json::json!({});
    let config = json_to_server_config(&json);
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
    cmd_add(&cfg, "env-server", "python", None, &env, 60).unwrap();

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
    assert_eq!(server["timeout"], 60);
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
    let config = json_to_server_config(&json);
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

// ------------------------- sync_mcp_master_switch -------------------------

#[test]
fn test_s11b_sync_master_switch_no_parent_dir() {
    // 相对裸文件名 → parent() 为 None → Ok 直接返回（165-166）
    let p = std::path::Path::new("config.mcp.json");
    sync_mcp_master_switch(p, true).unwrap();
}

#[test]
fn test_s11b_sync_master_switch_config_json_absent() {
    let tmp = TempDir::new().unwrap();
    let mcp_cfg = tmp.path().join("config").join("config.mcp.json");
    std::fs::create_dir_all(mcp_cfg.parent().unwrap()).unwrap();
    // config.json 不存在 → Ok（167-169）
    sync_mcp_master_switch(&mcp_cfg, true).unwrap();
    assert!(!tmp.path().join("config").join("config.json").exists());
}

#[test]
fn test_s11b_sync_master_switch_already_in_state() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    std::fs::create_dir_all(&dir).unwrap();
    let mcp_cfg = dir.join("config.mcp.json");
    let main_cfg = dir.join("config.json");
    std::fs::write(&main_cfg, r#"{"mcp": {"enabled": true}}"#).unwrap();
    let before = std::fs::read_to_string(&main_cfg).unwrap();
    // 已是 true → Ok 早退，不重写文件（170-177）
    sync_mcp_master_switch(&mcp_cfg, true).unwrap();
    assert_eq!(std::fs::read_to_string(&main_cfg).unwrap(), before, "同态不重写");
}

#[test]
fn test_s11b_sync_master_switch_flips_both_ways() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    std::fs::create_dir_all(&dir).unwrap();
    let mcp_cfg = dir.join("config.mcp.json");
    let main_cfg = dir.join("config.json");
    // 无 mcp 段 → 翻 true 会插入 mcp.enabled=true（179-189）
    std::fs::write(&main_cfg, r#"{"agents": {}}"#).unwrap();
    sync_mcp_master_switch(&mcp_cfg, true).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&main_cfg).unwrap()).unwrap();
    assert_eq!(v["mcp"]["enabled"], true);
    assert_eq!(v["agents"], serde_json::json!({}), "原有键不丢");
    // true → false
    sync_mcp_master_switch(&mcp_cfg, false).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&main_cfg).unwrap()).unwrap();
    assert_eq!(v["mcp"]["enabled"], false);
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
struct S11bTempHomeEnv {
    _tmp: TempDir,
    home: std::path::PathBuf,
}

impl Drop for S11bTempHomeEnv {
    fn drop(&mut self) {
        unsafe { std::env::remove_var("NEMESISBOT_HOME") };
    }
}

fn s11b_temp_home_env() -> S11bTempHomeEnv {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    std::fs::create_dir_all(home.join("workspace").join("config")).unwrap();
    unsafe { std::env::set_var("NEMESISBOT_HOME", tmp.path()) };
    S11bTempHomeEnv { _tmp: tmp, home }
}

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

#[tokio::test(flavor = "multi_thread")]
async fn test_s11b_run_async_dispatch_arms() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = s11b_temp_home_env();
    let _ = &th.home;
    // block_in_place 分发 arm 用「服务器不存在」的快路径逐个覆盖：
    // Test / Tools / Resources / Prompts / Discover。
    run(McpAction::Test { name: "nope".into() }, false).unwrap();
    run(McpAction::Tools { name: "nope".into() }, false).unwrap();
    run(McpAction::Resources { name: "nope".into() }, false).unwrap();
    run(McpAction::Prompts { name: "nope".into() }, false).unwrap();
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
