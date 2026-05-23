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

    cmd_add(&cfg, "fresh-server", "npx", Some("some,mcp"), &["KEY=val".to_string()], 60).unwrap();

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

    cmd_add(&cfg, "env-server", "cmd", None, &["KEY=VALUE".to_string()], 30).unwrap();

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

    cmd_add(&cfg, "full-server", "cmd", Some("a,b"), &["K=V".to_string()], 60).unwrap();

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
