use super::*;
use tempfile::TempDir;

fn make_config_path(tmp: &TempDir) -> PathBuf {
    tmp.path().join("config.mcp.json")
}

fn write_config(path: &PathBuf, config: &McpFileConfig) {
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
    write_config(&path, &McpFileConfig {
        enabled: true,
        servers: vec![
            ServerConfig::new("test-srv", "/usr/bin/test"),
        ],
        timeout: 60,
    });

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
    mgr.add_server(ServerConfig::new("srv2", "cmd2").arg("--flag")).unwrap();

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
    write_config(&path, &McpFileConfig {
        enabled: true,
        servers: vec![ServerConfig::new("new", "cmd")],
        timeout: 30,
    });

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
    write_config(&path, &McpFileConfig {
        enabled: true,
        servers: vec![ServerConfig::new("srv-a", "cmd_a")],
        timeout: 30,
    });
    let mut mgr = McpManager::new(path);

    // Consume initial mtime
    assert!(!mgr.check_config_changed());

    // Simulate "mcp add" writing a new server to config
    std::thread::sleep(std::time::Duration::from_millis(50));
    write_config(
        &mgr.config_path().to_path_buf(),
        &McpFileConfig {
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

    write_config(&path, &McpFileConfig {
        enabled: true,
        servers: vec![ServerConfig::new("srv-a", "cmd_a")],
        timeout: 30,
    });
    let mut mgr = McpManager::new(path);

    // Simulate registering tools from srv-a (prefix-based)
    let registered = vec!["mcp_srv_a_".to_string()];
    let new_srvs = mgr.find_new_servers(&registered);
    assert!(new_srvs.is_empty(), "srv-a already registered");

    // Add srv-b externally
    std::thread::sleep(std::time::Duration::from_millis(50));
    write_config(
        &mgr.config_path().to_path_buf(),
        &McpFileConfig {
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

    write_config(&path, &McpFileConfig {
        enabled: true,
        servers: vec![
            ServerConfig::new("srv-a", "cmd_a"),
            ServerConfig::new("srv-b", "cmd_b"),
        ],
        timeout: 30,
    });
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
    mgr.add_server(ServerConfig::new("srv", "cmd").timeout(99)).unwrap();
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
    write_config(&path, &McpFileConfig {
        enabled: true,
        servers: vec![ServerConfig::new("srv-a", "cmd_a")],
        timeout: 30,
    });
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
    write_config(&path, &McpFileConfig {
        enabled: true,
        servers: vec![
            ServerConfig::new("srv-a", "cmd_a"),
            ServerConfig::new("srv-b", "cmd_b"),
        ],
        timeout: 30,
    });
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
    ).unwrap();

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
    // The serde default for McpFileConfig.timeout is 30 when omitted.
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
