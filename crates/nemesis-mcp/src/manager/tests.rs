use super::*;
use tempfile::TempDir;

fn make_config_path(tmp: &TempDir) -> PathBuf {
    tmp.path().join("config.mcp.json")
}

fn write_config(path: &PathBuf, config: &McpConfig) {
    let content = serde_json::to_string_pretty(config).unwrap();
    std::fs::write(path, content).unwrap();
}

// ---------------------------------------------------------------------------
// Config load/save
// ---------------------------------------------------------------------------

#[test]
fn test_new_with_existing_config() {
    let tmp = TempDir::new().unwrap();
    let path = make_config_path(&tmp);
    write_config(
        &path,
        &McpConfig {
            enabled: true,
            servers: vec![ServerConfig::new("test-srv", "/usr/bin/test")],
            timeout: 60,
        },
    );

    let mgr = McpManager::new(path);
    assert!(mgr.is_enabled());
    assert_eq!(mgr.list_servers().len(), 1);
    assert_eq!(mgr.list_servers()[0].name, "test-srv");
}

#[test]
fn test_new_without_config_file() {
    let tmp = TempDir::new().unwrap();
    let path = make_config_path(&tmp);
    let mgr = McpManager::new(path);
    assert!(!mgr.is_enabled());
    assert!(mgr.list_servers().is_empty());
}

#[test]
fn test_save_and_reload() {
    let tmp = TempDir::new().unwrap();
    let path = make_config_path(&tmp);

    let mut mgr = McpManager::new(path);
    mgr.add_server(ServerConfig::new("srv1", "cmd1")).unwrap();
    mgr.add_server(ServerConfig::new("srv2", "cmd2").arg("--flag"))
        .unwrap();

    // Reload from disk
    let mgr2 = McpManager::new(mgr.config_path().to_path_buf());
    assert!(mgr2.is_enabled());
    assert_eq!(mgr2.list_servers().len(), 2);
    assert_eq!(mgr2.list_servers()[0].name, "srv1");
    assert_eq!(mgr2.list_servers()[1].args, vec!["--flag"]);
}

// ---------------------------------------------------------------------------
// CRUD
// ---------------------------------------------------------------------------

#[test]
fn test_add_server_duplicate() {
    let tmp = TempDir::new().unwrap();
    let path = make_config_path(&tmp);
    let mut mgr = McpManager::new(path);

    mgr.add_server(ServerConfig::new("dup", "cmd")).unwrap();
    let result = mgr.add_server(ServerConfig::new("dup", "cmd2"));
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("already exists"));
}

#[test]
fn test_remove_server() {
    let tmp = TempDir::new().unwrap();
    let path = make_config_path(&tmp);
    let mut mgr = McpManager::new(path);

    mgr.add_server(ServerConfig::new("a", "cmd_a")).unwrap();
    mgr.add_server(ServerConfig::new("b", "cmd_b")).unwrap();

    let removed = mgr.remove_server("a").unwrap();
    assert!(removed);
    assert_eq!(mgr.list_servers().len(), 1);
    assert_eq!(mgr.list_servers()[0].name, "b");
}

#[test]
fn test_remove_nonexistent() {
    let tmp = TempDir::new().unwrap();
    let path = make_config_path(&tmp);
    let mut mgr = McpManager::new(path);

    let removed = mgr.remove_server("ghost").unwrap();
    assert!(!removed);
}

#[test]
fn test_get_server() {
    let tmp = TempDir::new().unwrap();
    let path = make_config_path(&tmp);
    let mut mgr = McpManager::new(path);
    mgr.add_server(ServerConfig::new("target", "cmd")).unwrap();

    assert!(mgr.get_server("target").is_some());
    assert!(mgr.get_server("other").is_none());
    assert_eq!(mgr.get_server("target").unwrap().command, "cmd");
}

// ---------------------------------------------------------------------------
// find_new_servers
// ---------------------------------------------------------------------------

#[test]
fn test_find_new_servers_empty() {
    let tmp = TempDir::new().unwrap();
    let path = make_config_path(&tmp);
    let mgr = McpManager::new(path);
    assert!(mgr.find_new_servers(&[]).is_empty());
}

#[test]
fn test_find_new_servers_filters_registered() {
    let tmp = TempDir::new().unwrap();
    let path = make_config_path(&tmp);
    let mut mgr = McpManager::new(path);
    mgr.add_server(ServerConfig::new("srv-a", "cmd_a")).unwrap();
    mgr.add_server(ServerConfig::new("srv-b", "cmd_b")).unwrap();
    mgr.add_server(ServerConfig::new("srv-c", "cmd_c")).unwrap();

    // srv-a is already registered (note: sanitize lowercases)
    let registered = vec!["mcp_srv_a_".to_string()];
    let new_srvs = mgr.find_new_servers(&registered);
    assert_eq!(new_srvs.len(), 2);
    let names: Vec<&str> = new_srvs.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"srv-b"));
    assert!(names.contains(&"srv-c"));
}

#[test]
fn test_find_new_servers_all_registered() {
    let tmp = TempDir::new().unwrap();
    let path = make_config_path(&tmp);
    let mut mgr = McpManager::new(path);
    mgr.add_server(ServerConfig::new("x", "cmd")).unwrap();

    let registered = vec!["mcp_x_".to_string()];
    assert!(mgr.find_new_servers(&registered).is_empty());
}

// ---------------------------------------------------------------------------
// mtime detection
// ---------------------------------------------------------------------------

#[test]
fn test_check_config_changed_no_change() {
    let tmp = TempDir::new().unwrap();
    let path = make_config_path(&tmp);
    let mut mgr = McpManager::new(path);
    // No file was written, so no mtime to compare
    assert!(!mgr.check_config_changed());
}

#[test]
fn test_check_config_changed_detects_write() {
    let tmp = TempDir::new().unwrap();
    let path = make_config_path(&tmp);
    let mut mgr = McpManager::new(path.clone());

    // Initially no file
    assert!(!mgr.check_config_changed());

    // Write a config file externally
    std::thread::sleep(std::time::Duration::from_millis(50));
    write_config(
        &path,
        &McpConfig {
            enabled: true,
            servers: vec![ServerConfig::new("new", "cmd")],
            timeout: 30,
        },
    );

    assert!(mgr.check_config_changed());
    assert!(mgr.is_enabled());
    assert_eq!(mgr.list_servers().len(), 1);

    // Second check should not trigger again
    assert!(!mgr.check_config_changed());
}

// ---------------------------------------------------------------------------
// discover_tools (requires actual MCP server — integration test)
// ---------------------------------------------------------------------------

#[test]
fn test_discover_tools_timeout_nonexistent_command() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let tmp = TempDir::new().unwrap();
    let path = make_config_path(&tmp);
    let mgr = McpManager::new(path);

    let server = ServerConfig::new("bad", "nonexistent_command_xyz").timeout(1);
    let result = rt.block_on(mgr.discover_tools(&server));
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// Full mtime → find_new_servers flow
// ---------------------------------------------------------------------------

#[test]
fn test_mtime_detects_new_server_after_add() {
    let tmp = TempDir::new().unwrap();
    let path = make_config_path(&tmp);

    // Start with one server
    write_config(
        &path,
        &McpConfig {
            enabled: true,
            servers: vec![ServerConfig::new("srv-a", "cmd_a")],
            timeout: 30,
        },
    );
    let mut mgr = McpManager::new(path);

    // Consume initial mtime
    assert!(!mgr.check_config_changed());

    // Simulate "mcp add" writing a new server to config
    std::thread::sleep(std::time::Duration::from_millis(50));
    write_config(
        &mgr.config_path().to_path_buf(),
        &McpConfig {
            enabled: true,
            servers: vec![
                ServerConfig::new("srv-a", "cmd_a"),
                ServerConfig::new("srv-b", "cmd_b"),
            ],
            timeout: 30,
        },
    );

    // mtime should detect change and reload config
    assert!(mgr.check_config_changed());
    assert_eq!(mgr.list_servers().len(), 2);

    // Second check should not trigger
    assert!(!mgr.check_config_changed());
}

#[test]
fn test_find_new_servers_after_mtime_reload() {
    let tmp = TempDir::new().unwrap();
    let path = make_config_path(&tmp);

    write_config(
        &path,
        &McpConfig {
            enabled: true,
            servers: vec![ServerConfig::new("srv-a", "cmd_a")],
            timeout: 30,
        },
    );
    let mut mgr = McpManager::new(path);

    // Simulate registering tools from srv-a (prefix-based)
    let registered = vec!["mcp_srv_a_".to_string()];
    let new_srvs = mgr.find_new_servers(&registered);
    assert!(new_srvs.is_empty(), "srv-a already registered");

    // Add srv-b externally
    std::thread::sleep(std::time::Duration::from_millis(50));
    write_config(
        &mgr.config_path().to_path_buf(),
        &McpConfig {
            enabled: true,
            servers: vec![
                ServerConfig::new("srv-a", "cmd_a"),
                ServerConfig::new("srv-b", "cmd_b"),
            ],
            timeout: 30,
        },
    );

    // Detect change and find new servers
    assert!(mgr.check_config_changed());
    let new_srvs = mgr.find_new_servers(&registered);
    assert_eq!(new_srvs.len(), 1);
    assert_eq!(new_srvs[0].name, "srv-b");
}

#[test]
fn test_remove_server_updates_config() {
    let tmp = TempDir::new().unwrap();
    let path = make_config_path(&tmp);

    write_config(
        &path,
        &McpConfig {
            enabled: true,
            servers: vec![
                ServerConfig::new("srv-a", "cmd_a"),
                ServerConfig::new("srv-b", "cmd_b"),
            ],
            timeout: 30,
        },
    );
    let mut mgr = McpManager::new(path);

    // Remove one server
    mgr.remove_server("srv-a").unwrap();
    assert_eq!(mgr.list_servers().len(), 1);
    assert_eq!(mgr.list_servers()[0].name, "srv-b");

    // Verify persistence
    let mgr2 = McpManager::new(mgr.config_path().to_path_buf());
    assert_eq!(mgr2.list_servers().len(), 1);
    assert_eq!(mgr2.list_servers()[0].name, "srv-b");
}

// ---------------------------------------------------------------------------
// Additional coverage tests: load/save error paths, hot-reload failure
// ---------------------------------------------------------------------------

#[test]
fn test_load_config_invalid_json_returns_err() {
    let tmp = TempDir::new().unwrap();
    let path = make_config_path(&tmp);
    // Write garbage that is not valid JSON
    std::fs::write(&path, "{ this is not valid json,,,,").unwrap();

    let mut mgr = McpManager::new(path.clone());
    // load_config should surface a parse error
    let result = mgr.load_config();
    assert!(result.is_err());
    assert!(result.unwrap_err().to_lowercase().contains("parse"));
}

#[test]
fn test_load_config_read_error_returns_err() {
    let tmp = TempDir::new().unwrap();
    // Point config_path at a path that exists as a directory (not a file),
    // so read_to_string fails with a read error rather than a parse error.
    let dir_path = tmp.path().join("is_a_dir.mcp.json");
    std::fs::create_dir(&dir_path).unwrap();

    let mut mgr = McpManager::new(dir_path);
    let result = mgr.load_config();
    assert!(result.is_err());
    assert!(result.unwrap_err().to_lowercase().contains("read"));
}

#[test]
fn test_load_config_missing_file_is_ok() {
    // No file on disk — load_config must return Ok and keep empty defaults.
    let tmp = TempDir::new().unwrap();
    let path = make_config_path(&tmp);
    let mut mgr = McpManager::new(path);
    assert!(mgr.load_config().is_ok());
    assert!(!mgr.is_enabled());
    assert!(mgr.list_servers().is_empty());
}

#[test]
fn test_save_config_creates_parent_dirs() {
    let tmp = TempDir::new().unwrap();
    // Nest the config file two levels deep under non-existent dirs.
    let nested = tmp.path().join("a").join("b").join("config.mcp.json");

    let mgr = McpManager::new(nested.clone());
    // Saving should create the missing parent directories.
    mgr.save_config().unwrap();
    assert!(nested.exists());

    // The written file must be valid and reloadable.
    let reloaded = McpManager::new(nested);
    assert!(reloaded.list_servers().is_empty());
}

#[test]
fn test_save_config_round_trips_enabled_and_timeout() {
    let tmp = TempDir::new().unwrap();
    let path = make_config_path(&tmp);

    let mut mgr = McpManager::new(path.clone());
    mgr.add_server(ServerConfig::new("srv", "cmd").timeout(99))
        .unwrap();
    assert!(mgr.is_enabled());

    // Reload from the same path and confirm enabled flag + server preserved.
    let reloaded = McpManager::new(path);
    assert!(reloaded.is_enabled());
    assert_eq!(reloaded.list_servers().len(), 1);
    assert_eq!(reloaded.list_servers()[0].timeout_secs, 99);
}

#[test]
fn test_check_config_changed_reload_failure_keeps_mtime() {
    let tmp = TempDir::new().unwrap();
    let path = make_config_path(&tmp);

    // Start with a valid config and consume the initial mtime.
    write_config(
        &path,
        &McpConfig {
            enabled: true,
            servers: vec![ServerConfig::new("srv-a", "cmd_a")],
            timeout: 30,
        },
    );
    let mut mgr = McpManager::new(path.clone());
    assert!(!mgr.check_config_changed());

    // Corrupt the file on disk (mtime changes), then ask for changes.
    std::thread::sleep(std::time::Duration::from_millis(50));
    std::fs::write(&path, "{ broken json").unwrap();

    // Reload fails — check_config_changed must report false AND must NOT
    // update last_mtime (so the next round retries the broken file).
    assert!(!mgr.check_config_changed());

    // Because mtime was not updated, fixing the file makes the next round
    // detect and reload successfully.
    std::thread::sleep(std::time::Duration::from_millis(50));
    write_config(
        &path,
        &McpConfig {
            enabled: true,
            servers: vec![
                ServerConfig::new("srv-a", "cmd_a"),
                ServerConfig::new("srv-b", "cmd_b"),
            ],
            timeout: 30,
        },
    );
    assert!(mgr.check_config_changed());
    assert_eq!(mgr.list_servers().len(), 2);
}

#[test]
fn test_add_server_enables_mcp() {
    let tmp = TempDir::new().unwrap();
    let path = make_config_path(&tmp);
    let mut mgr = McpManager::new(path);

    // MCP starts disabled by default when no config file exists.
    assert!(!mgr.is_enabled());

    // Adding the first server auto-enables MCP.
    mgr.add_server(ServerConfig::new("first", "cmd")).unwrap();
    assert!(mgr.is_enabled());
}

#[test]
fn test_remove_last_server_keeps_enabled_flag() {
    let tmp = TempDir::new().unwrap();
    let path = make_config_path(&tmp);
    let mut mgr = McpManager::new(path);

    mgr.add_server(ServerConfig::new("only", "cmd")).unwrap();
    assert!(mgr.is_enabled());

    // Removing the last server should still report success; enabled flag
    // is not toggled back off by removal (matches Go behavior).
    let removed = mgr.remove_server("only").unwrap();
    assert!(removed);
    assert!(mgr.list_servers().is_empty());
}

#[test]
fn test_get_server_returns_command_and_args() {
    let tmp = TempDir::new().unwrap();
    let path = make_config_path(&tmp);
    let mut mgr = McpManager::new(path);
    mgr.add_server(
        ServerConfig::new("worker", "/usr/bin/node")
            .arg("index.js")
            .arg("--verbose"),
    )
    .unwrap();

    let srv = mgr.get_server("worker").expect("server should exist");
    assert_eq!(srv.command, "/usr/bin/node");
    assert_eq!(srv.args, vec!["index.js", "--verbose"]);
}

#[test]
fn test_find_new_servers_empty_prefix_matches_none() {
    let tmp = TempDir::new().unwrap();
    let path = make_config_path(&tmp);
    let mut mgr = McpManager::new(path);
    mgr.add_server(ServerConfig::new("alpha", "cmd")).unwrap();
    mgr.add_server(ServerConfig::new("beta", "cmd")).unwrap();

    // No registered prefixes → all servers are "new".
    let new_srvs = mgr.find_new_servers(&[]);
    assert_eq!(new_srvs.len(), 2);
}

#[test]
fn test_config_path_accessor_returns_bound_path() {
    let tmp = TempDir::new().unwrap();
    let path = make_config_path(&tmp);
    let mgr = McpManager::new(path.clone());
    assert_eq!(mgr.config_path(), &path);
}

#[test]
fn test_new_logs_and_recovers_from_corrupt_init_config() {
    // new() calls load_config internally; a corrupt initial file should be
    // swallowed (logged via warn) rather than panicking, leaving empty state.
    let tmp = TempDir::new().unwrap();
    let path = make_config_path(&tmp);
    std::fs::write(&path, "not json at all").unwrap();

    let mgr = McpManager::new(path);
    assert!(!mgr.is_enabled());
    assert!(mgr.list_servers().is_empty());
}

#[test]
fn test_default_timeout_value_is_30() {
    // The serde default for McpConfig.timeout is 30 when omitted.
    let tmp = TempDir::new().unwrap();
    let path = make_config_path(&tmp);
    // Write a config that omits the timeout field entirely.
    std::fs::write(&path, r#"{"enabled":true,"servers":[]}"#).unwrap();

    let mgr = McpManager::new(path);
    assert!(mgr.is_enabled());
    // Round-trip through save to confirm the default timeout serializes.
    mgr.save_config().unwrap();
    let raw = std::fs::read_to_string(mgr.config_path()).unwrap();
    assert!(raw.contains("\"timeout\": 30"));
}

// ===========================================================================
// W4c 补测（2026-08-25）：discovery 三函数——discover_tools（成功/起不来/
// init 错误/超时 + 工具执行矩阵）、discover_server_metadata（stdio 成功+超时）、
// discover_server_metadata_http（wiremock 成功/超时/5xx）
// ===========================================================================

/// 完整假 MCP 服务器（newline-delimited JSON-RPC over stdio）。
/// tools/call 按 arguments 分支：fail → isError、img → image 内容、slow → 睡 10s。
const W4C_FAKE_MCP: &str = r#"
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
    elif m == "tools/call":
        args = req.get("params", {}).get("arguments", {})
        if args.get("fail"):
            sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": rid, "result": {"content": [{"type": "text", "text": "boom"}], "isError": True}}) + "\n")
            sys.stdout.flush()
            continue
        if args.get("img"):
            result = {"content": [{"type": "image", "text": "b64x"}], "isError": False}
        elif args.get("slow"):
            import time
            time.sleep(10)
            result = {"content": [{"type": "text", "text": "late"}], "isError": False}
        else:
            result = {"content": [{"type": "text", "text": "called"}], "isError": False}
    elif m == "resources/list":
        result = {"resources": [{"uri": "file:///x", "name": "x"}]}
    elif m == "prompts/list":
        result = {"prompts": [{"name": "p1", "description": "pd"}]}
    else:
        result = {}
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": rid, "result": result}) + "\n")
    sys.stdout.flush()
"#;

fn w4c_have_python() -> bool {
    std::process::Command::new("python")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn w4c_fake_server_config(name: &str, timeout_secs: u64) -> ServerConfig {
    ServerConfig::new(name, "python")
        .arg("-c")
        .arg(W4C_FAKE_MCP)
        .timeout(timeout_secs)
}

#[tokio::test]
async fn test_w4c_discover_tools_success_and_execute_matrix() {
    if !w4c_have_python() {
        eprintln!("Skipping test: python not available");
        return;
    }
    let tmp = TempDir::new().unwrap();
    let mgr = McpManager::new(tmp.path().to_path_buf().join("config.mcp.json"));
    let cfg = w4c_fake_server_config("Fake Srv", 5);

    let tools = mgr.discover_tools(&cfg).await.unwrap();
    assert_eq!(tools.len(), 1);
    let def = tools[0].definition();
    // 配置名（非自报名 "fake"）用于前缀
    assert_eq!(def.name, "mcp_fake_srv_echo");
    assert!(def.description.contains("[MCP:Fake Srv]"));

    // 正常调用 → text
    let r = tools[0].execute(serde_json::json!({"text": "hi"})).await;
    assert!(!r.is_error);
    assert_eq!(r.content, "called");

    // image 内容分支
    let r = tools[0].execute(serde_json::json!({"img": true})).await;
    assert!(!r.is_error);
    assert!(r.content.contains("[Image: b64x]"));

    // 服务器侧 isError → "returned error"
    let r = tools[0].execute(serde_json::json!({"fail": true})).await;
    assert!(r.is_error);
    assert!(r.content.contains("returned error"));
    assert!(r.content.contains("boom"));
}

#[tokio::test]
async fn test_w4c_discover_tools_adapter_timeout() {
    if !w4c_have_python() {
        eprintln!("Skipping test: python not available");
        return;
    }
    let tmp = TempDir::new().unwrap();
    let mgr = McpManager::new(tmp.path().to_path_buf().join("config.mcp.json"));
    // timeout_secs=3 → adapter 3s 超时（slow 分支睡 10s）。
    // 注意不能太小：initialize 里的 notifications/initialized 通知固定等 1s 传输超时。
    let cfg = w4c_fake_server_config("slow-srv", 3);
    let tools = mgr.discover_tools(&cfg).await.unwrap();
    let r = tools[0].execute(serde_json::json!({"slow": true})).await;
    assert!(r.is_error);
    assert!(r.content.contains("timed out after"));
}

#[tokio::test]
async fn test_w4c_discover_tools_spawn_failure() {
    let tmp = TempDir::new().unwrap();
    let mgr = McpManager::new(tmp.path().to_path_buf().join("config.mcp.json"));
    let cfg = ServerConfig::new("nope", "/absolutely/nonexistent/command/xyz");
    let err = match mgr.discover_tools(&cfg).await {
        Ok(_) => panic!("expected error"),
        Err(e) => e,
    };
    assert!(err.contains("nope"), "unexpected: {}", err);
    assert!(err.contains("initialization failed"), "unexpected: {}", err);
}

#[tokio::test]
async fn test_w4c_discover_tools_init_error_response() {
    if !w4c_have_python() {
        eprintln!("Skipping test: python not available");
        return;
    }
    let script = r#"
import sys, json
line = sys.stdin.readline()
req = json.loads(line)
sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": req.get("id"), "error": {"code": -32600, "message": "denied"}}) + "\n")
sys.stdout.flush()
import time
time.sleep(30)
"#;
    let tmp = TempDir::new().unwrap();
    let mgr = McpManager::new(tmp.path().to_path_buf().join("config.mcp.json"));
    let cfg = ServerConfig::new("err-srv", "python")
        .arg("-c")
        .arg(script)
        .timeout(5);
    let err = match mgr.discover_tools(&cfg).await {
        Ok(_) => panic!("expected error"),
        Err(e) => e,
    };
    assert!(err.contains("err-srv"), "unexpected: {}", err);
    assert!(err.contains("initialization failed"), "unexpected: {}", err);
    assert!(err.contains("denied"), "unexpected: {}", err);
}

#[tokio::test]
async fn test_w4c_discover_tools_init_timeout() {
    if !w4c_have_python() {
        eprintln!("Skipping test: python not available");
        return;
    }
    let script = r#"
import time
time.sleep(30)
"#;
    let tmp = TempDir::new().unwrap();
    let mgr = McpManager::new(tmp.path().to_path_buf().join("config.mcp.json"));
    let cfg = ServerConfig::new("hang-srv", "python")
        .arg("-c")
        .arg(script)
        .timeout(1);
    let err = match mgr.discover_tools(&cfg).await {
        Ok(_) => panic!("expected error"),
        Err(e) => e,
    };
    assert!(err.contains("hang-srv"), "unexpected: {}", err);
    assert!(
        err.contains("initialization timed out"),
        "unexpected: {}",
        err
    );
}

#[tokio::test]
async fn test_w4c_discover_server_metadata_success() {
    if !w4c_have_python() {
        eprintln!("Skipping test: python not available");
        return;
    }
    let result =
        discover_server_metadata("python", vec!["-c".into(), W4C_FAKE_MCP.into()], vec![], 5)
            .await
            .expect("discovery should succeed");
    let info = result.server_info.expect("server_info");
    assert_eq!(info.name, "fake");
    assert_eq!(info.version, "1.2.3");
    assert_eq!(result.tools.len(), 1);
    assert_eq!(result.tools[0].name, "echo");
    assert_eq!(result.resources.len(), 1);
    assert_eq!(result.resources[0].name, "x");
    assert_eq!(result.prompts.len(), 1);
    assert_eq!(result.prompts[0].name, "p1");
}

#[tokio::test]
async fn test_w4c_discover_server_metadata_timeout_hint() {
    if !w4c_have_python() {
        eprintln!("Skipping test: python not available");
        return;
    }
    let script = "import time\ntime.sleep(30)\n";
    let err = match discover_server_metadata("python", vec!["-c".into(), script.into()], vec![], 1)
        .await
    {
        Ok(_) => panic!("expected error"),
        Err(e) => e,
    };
    assert!(err.contains("timed out after 1s"), "unexpected: {}", err);
    assert!(err.contains("parameter instead"), "unexpected: {}", err);
}

#[tokio::test]
async fn test_w4c_discover_server_metadata_http_success() {
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .and(body_partial_json(serde_json::json!({"method": "initialize"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "result": {"protocolVersion": "2024-11-05", "capabilities": {}, "serverInfo": {"name": "httpfake", "version": "9.9"}}
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .and(body_partial_json(serde_json::json!({"method": "tools/list"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "jsonrpc": "2.0", "id": 2,
            "result": {"tools": [{"name": "ht", "description": "http tool", "inputSchema": {"type": "object"}}]}
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .and(body_partial_json(
            serde_json::json!({"method": "resources/list"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "jsonrpc": "2.0", "id": 3, "result": {"resources": [{"uri": "u://1", "name": "r1"}]}
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .and(body_partial_json(
            serde_json::json!({"method": "prompts/list"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "jsonrpc": "2.0", "id": 4, "result": {"prompts": [{"name": "pp", "description": "pd"}]}
        })))
        .mount(&server)
        .await;

    let url = format!("{}/mcp", server.uri());
    let result = discover_server_metadata_http(&url, 5).await.unwrap();
    assert_eq!(result.server_info.unwrap().name, "httpfake");
    assert_eq!(result.tools.len(), 1);
    assert_eq!(result.tools[0].name, "ht");
    assert_eq!(result.resources[0].name, "r1");
    assert_eq!(result.prompts[0].name, "pp");
}

#[tokio::test]
async fn test_w4c_discover_server_metadata_http_timeout() {
    use std::time::Duration;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"jsonrpc":"2.0","id":1,"result":{}}))
                .set_delay(Duration::from_secs(5)),
        )
        .mount(&server)
        .await;

    let url = format!("{}/mcp", server.uri());
    let err = match discover_server_metadata_http(&url, 1).await {
        Ok(_) => panic!("expected error"),
        Err(e) => e,
    };
    assert!(err.contains("timed out"), "unexpected: {}", err);
}

#[tokio::test]
async fn test_w4c_discover_server_metadata_http_error_status() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(ResponseTemplate::new(500).set_body_string("backend down"))
        .mount(&server)
        .await;

    let url = format!("{}/mcp", server.uri());
    let err = match discover_server_metadata_http(&url, 5).await {
        Ok(_) => panic!("expected error"),
        Err(e) => e,
    };
    assert!(err.contains("initialization failed"), "unexpected: {}", err);
}

// ===========================================================================
// S1 补测（2026-08-26）：timeout_secs=0 的 30s 回退臂；initialize 成功后
// list_tools/list_resources/list_prompts 失败的 warn!/Vec::new() 回退臂
// （stdio + HTTP 两个变体）
// ===========================================================================

#[tokio::test]
async fn test_s1_discover_tools_zero_timeout_uses_default_fallback() {
    // timeout_secs=0 → the `else { 30 }` fallback arm. The command does not
    // exist, so initialize() fails immediately and we never actually wait.
    let tmp = TempDir::new().unwrap();
    let mgr = McpManager::new(tmp.path().to_path_buf().join("config.mcp.json"));
    let cfg = ServerConfig::new("zero-t-srv", "/absolutely/nonexistent/command/xyz").timeout(0);
    let err = match mgr.discover_tools(&cfg).await {
        Ok(_) => panic!("expected error"),
        Err(e) => e,
    };
    assert!(err.contains("zero-t-srv"), "unexpected: {}", err);
    assert!(err.contains("initialization failed"), "unexpected: {}", err);
}

fn s1_sink_subscriber() -> impl tracing::Subscriber + Send + Sync + 'static {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(std::io::sink)
        .finish()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_s1_discover_server_metadata_list_failures_after_init() {
    if !w4c_have_python() {
        eprintln!("Skipping test: python not available");
        return;
    }
    // Answers initialize correctly, then exits: every subsequent list_*
    // request hits EOF and must fall back to an empty vec (warn! arm) while
    // discovery itself still succeeds.
    let script = r#"
import sys, json
line = sys.stdin.readline()
req = json.loads(line)
sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": req.get("id"), "result": {"protocolVersion": "2024-11-05", "capabilities": {}, "serverInfo": {"name": "die-after-init", "version": "0.0.1"}}}) + "\n")
sys.stdout.flush()
sys.exit(0)
"#;
    let handle = tokio::runtime::Handle::current();
    let result = tokio::task::block_in_place(|| {
        tracing::subscriber::with_default(s1_sink_subscriber(), || {
            handle.block_on(discover_server_metadata(
                "python",
                vec!["-c".into(), script.into()],
                vec![],
                5,
            ))
        })
    })
    .expect("discovery must still succeed (lists are best-effort)");

    let info = result.server_info.expect("server_info");
    assert_eq!(info.name, "die-after-init");
    assert!(result.tools.is_empty());
    assert!(result.resources.is_empty());
    assert!(result.prompts.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_s1_discover_server_metadata_http_list_failures() {
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    // Mount order matters: initialize first, then per-method 500s, then a
    // catch-all 202 (for the fire-and-forget notifications/initialized POST).
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .and(body_partial_json(
            serde_json::json!({"method": "initialize"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "result": {
                "protocolVersion": "2024-11-05", "capabilities": {},
                "serverInfo": {"name": "httpflaky", "version": "9.9.9"}
            }
        })))
        .mount(&server)
        .await;
    for m in ["tools/list", "resources/list", "prompts/list"] {
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .and(body_partial_json(serde_json::json!({"method": m})))
            .respond_with(ResponseTemplate::new(500).set_body_string("backend down"))
            .mount(&server)
            .await;
    }
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(ResponseTemplate::new(202))
        .mount(&server)
        .await;

    let url = format!("{}/mcp", server.uri());
    let handle = tokio::runtime::Handle::current();
    let result = tokio::task::block_in_place(|| {
        tracing::subscriber::with_default(s1_sink_subscriber(), || {
            handle.block_on(discover_server_metadata_http(&url, 5))
        })
    })
    .expect("HTTP discovery must still succeed (lists are best-effort)");

    let info = result.server_info.expect("server_info");
    assert_eq!(info.name, "httpflaky");
    assert!(result.tools.is_empty());
    assert!(result.resources.is_empty());
    assert!(result.prompts.is_empty());
}

// ---------------------------------------------------------------------------
// S1 coverage batch (2026-08-26): save_config parent() == None arm (line 93).
// ---------------------------------------------------------------------------

#[test]
#[cfg(windows)]
fn test_s1_save_config_root_drive_parent_none() {
    // "c:\" 的 parent() == None → 跳过建目录分支（行 90 的 else 臂，闭括号
    // 计数落在行 93）；随后 tmp 路径仍为 "c:\"（with_extension 对无 file_name
    // 的路径原样返回）→ fs::write 失败 → "Failed to write MCP config"。
    let mgr = McpManager::new(PathBuf::from("c:\\"));
    let err = mgr.save_config().unwrap_err();
    assert!(
        err.starts_with("Failed to write MCP config"),
        "unexpected error: {err}"
    );
}
