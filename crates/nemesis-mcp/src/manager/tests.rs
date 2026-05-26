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
