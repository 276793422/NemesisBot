use super::*;

#[tokio::test]
async fn test_install_and_is_installed() {
    let dir = tempfile::tempdir().unwrap();
    let installer = MCPInstaller::new(dir.path());

    assert!(!installer.is_installed("test-server"));

    installer
        .install("test-server", "uv", vec!["run".into(), "server.py".into()])
        .unwrap();

    assert!(installer.is_installed("test-server"));
}

#[tokio::test]
async fn test_uninstall() {
    let dir = tempfile::tempdir().unwrap();
    let installer = MCPInstaller::new(dir.path());

    installer
        .install("to-remove", "python", vec!["server.py".into()])
        .unwrap();

    assert!(installer.is_installed("to-remove"));

    installer.uninstall("to-remove").unwrap();
    assert!(!installer.is_installed("to-remove"));
}

#[tokio::test]
async fn test_config_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let installer = MCPInstaller::new(dir.path());

    installer
        .install("persist-test", "go", vec!["run".into()])
        .unwrap();

    // Create new installer instance
    let installer2 = MCPInstaller::new(dir.path());
    assert!(installer2.is_installed("persist-test"));
}

// --- Additional mcp_installer tests ---

#[tokio::test]
async fn test_load_config_empty() {
    let dir = tempfile::tempdir().unwrap();
    let installer = MCPInstaller::new(dir.path());
    let config = installer.load_config().unwrap();
    assert!(config.servers.is_empty());
}

#[tokio::test]
async fn test_save_and_load_config() {
    let dir = tempfile::tempdir().unwrap();
    let installer = MCPInstaller::new(dir.path());
    let mut config = McpConfig::default();
    config.servers.push(McpServerConfig {
        name: "test-server".into(),
        command: "python".into(),
        transport_type: "stdio".into(),
        args: vec!["server.py".into()],
        ..Default::default()
    });
    installer.save_config(&config).unwrap();
    let loaded = installer.load_config().unwrap();
    assert_eq!(loaded.servers.len(), 1);
    assert_eq!(loaded.servers[0].name, "test-server");
}

#[tokio::test]
async fn test_install_multiple_servers() {
    let dir = tempfile::tempdir().unwrap();
    let installer = MCPInstaller::new(dir.path());
    installer
        .install("server-a", "go", vec!["run".into()])
        .unwrap();
    installer
        .install("server-b", "python", vec!["main.py".into()])
        .unwrap();
    assert!(installer.is_installed("server-a"));
    assert!(installer.is_installed("server-b"));
}

#[tokio::test]
async fn test_uninstall_nonexistent() {
    let dir = tempfile::tempdir().unwrap();
    let installer = MCPInstaller::new(dir.path());
    // Should not panic
    installer.uninstall("nonexistent").unwrap();
}

#[tokio::test]
async fn test_reinstall_overwrites() {
    let dir = tempfile::tempdir().unwrap();
    let installer = MCPInstaller::new(dir.path());
    installer
        .install("server", "go", vec!["v1".into()])
        .unwrap();
    installer
        .install("server", "python", vec!["v2".into()])
        .unwrap();
    let config = installer.load_config().unwrap();
    assert_eq!(config.servers[0].command, "python");
}

#[tokio::test]
async fn test_config_path_in_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let installer = MCPInstaller::new(dir.path());
    let path = installer.config_path();
    assert!(path.to_string_lossy().contains("config"));
}

#[tokio::test]
async fn test_invalid_json_config_is_installed_false() {
    let dir = tempfile::tempdir().unwrap();
    let installer = MCPInstaller::new(dir.path());

    // Write garbage where config.mcp.json should be.
    let config_dir = dir.path().join("config");
    tokio::fs::create_dir_all(&config_dir).await.unwrap();
    tokio::fs::write(config_dir.join("config.mcp.json"), "{not valid json")
        .await
        .unwrap();

    // load_config fails with InvalidData (parse-class errors honest-mapped)...
    let loaded = installer.load_config();
    assert!(loaded.is_err());
    assert_eq!(loaded.unwrap_err().kind(), std::io::ErrorKind::InvalidData);

    // ...and is_installed treats a load failure as "not installed".
    assert!(!installer.is_installed("anything"));
}

// --- S8 coverage additions (quality-hardening goal 冲刺 S8) ---

#[tokio::test]
async fn test_s8_install_into_brand_new_workspace() {
    // config/ does not exist yet in a fresh workspace; install must create it.
    let dir = tempfile::tempdir().unwrap();
    let installer = MCPInstaller::new(dir.path().join("ws"));
    assert!(!installer.config_path().exists());
    installer
        .install("fresh", "python", vec!["-m".into()])
        .unwrap();
    assert!(installer.config_path().exists());
    assert!(installer.is_installed("fresh"));
}

// --- P1 单一真相源对齐回归钉（2026-10-16）---

#[tokio::test]
async fn test_install_preserves_existing_server_fields() {
    // 旧影子 schema（{name, command, args} 三字段）的 RMW 会把盘上既有
    // 条目的 url/headers/env 等字段整体剥掉——本钉锁死「更新臂只覆盖
    // transport_type/command/args，其余字段原样保留」的契约。
    let dir = tempfile::tempdir().unwrap();
    let installer = MCPInstaller::new(dir.path());

    let mut seed = McpConfig::default();
    seed.servers.push(McpServerConfig {
        name: "multi-tool".into(),
        transport_type: "http".into(),
        url: "http://127.0.0.1:9999/mcp".into(),
        description: "user-authored entry".into(),
        headers: vec!["Authorization=Bearer tok".into()],
        env: vec!["K=V".into()],
        timeout_secs: 77,
        provider_name: "prov".into(),
        provider_url: "http://prov".into(),
        tags: vec!["tag-a".into()],
        command: "old-command".into(),
        args: vec!["old-arg".into()],
        extra: [("custom_key".to_string(), serde_json::json!(42))]
            .into_iter()
            .collect(),
    });
    installer.save_config(&seed).unwrap();

    installer
        .install("multi-tool", "uv", vec!["run".into(), "server.py".into()])
        .unwrap();

    // 经 nemesis-config 同源解析回读（不是 installer 自己的视图）。
    let cfg = nemesis_config::load_mcp_config(&installer.config_path()).unwrap();
    assert!(cfg.enabled, "install must flip enabled on");
    assert_eq!(cfg.servers.len(), 1);
    let s = &cfg.servers[0];
    // forge 拥有的字段被覆盖
    assert_eq!(s.transport_type, "stdio");
    assert_eq!(s.command, "uv");
    assert_eq!(s.args, vec!["run".to_string(), "server.py".to_string()]);
    // 其余字段原样保留
    assert_eq!(s.url, "http://127.0.0.1:9999/mcp");
    assert_eq!(s.description, "user-authored entry");
    assert_eq!(s.headers, vec!["Authorization=Bearer tok".to_string()]);
    assert_eq!(s.env, vec!["K=V".to_string()]);
    assert_eq!(s.timeout_secs, 77);
    assert_eq!(s.provider_name, "prov");
    assert_eq!(s.provider_url, "http://prov");
    assert_eq!(s.tags, vec!["tag-a".to_string()]);
    assert_eq!(s.extra.get("custom_key"), Some(&serde_json::json!(42)));
}

#[tokio::test]
async fn test_install_uses_standard_servers_shape() {
    // 旧 forge_build_mcp 裸写 `mcpServers` map 形状——文件若已是标准
    // `servers` 数组形态，宽容解析只认 `servers`，注册整段隐形。本钉
    // 锁死安装产物是标准形状。
    let dir = tempfile::tempdir().unwrap();
    let installer = MCPInstaller::new(dir.path());
    installer
        .install("shape-pin", "go", vec!["run".into(), "main.go".into()])
        .unwrap();

    let raw = std::fs::read_to_string(installer.config_path()).unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert!(
        v.get("servers").is_some_and(|s| s.is_array()),
        "install must produce the standard `servers` array shape, got: {raw}"
    );
    let entry = &v["servers"][0];
    assert_eq!(entry["name"], "shape-pin");
    assert_eq!(entry["transport_type"], "stdio");
    assert_eq!(entry["command"], "go");
}

#[tokio::test]
async fn test_uninstall_preserves_other_servers_and_fields() {
    // 卸载只摘目标条目：同文件里用户手工维护的另一个条目连同其字段
    // 必须原样存活（旧影子 schema 全量重写同族风险面）。
    let dir = tempfile::tempdir().unwrap();
    let installer = MCPInstaller::new(dir.path());

    let mut seed = McpConfig::default();
    seed.servers.push(McpServerConfig {
        name: "user-entry".into(),
        transport_type: "http".into(),
        url: "http://keep-me".into(),
        headers: vec!["X-Keep=1".into()],
        ..Default::default()
    });
    installer.save_config(&seed).unwrap();

    installer
        .install("forge-skill", "python", vec!["server.py".into()])
        .unwrap();
    installer.uninstall("forge-skill").unwrap();

    let cfg = nemesis_config::load_mcp_config(&installer.config_path()).unwrap();
    assert_eq!(cfg.servers.len(), 1);
    assert_eq!(cfg.servers[0].name, "user-entry");
    assert_eq!(cfg.servers[0].url, "http://keep-me");
    assert_eq!(cfg.servers[0].headers, vec!["X-Keep=1".to_string()]);
}

#[tokio::test]
async fn test_uninstall_absent_entry_does_not_rewrite_file() {
    // P1-b 回归钉（2026-10-16 diff 复查）：卸载不存在的条目 = no-op，不得
    // 触碰文件——McpConfig 顶层无 flatten extra，no-op 重写会把宽容解析不
    // 认识的外来顶层键整体剥掉（旧影子 schema 之前的实现只在确实移除时才
    // 写盘）。回归钉咬合：把 uninstall 的 no-op 短路摘掉（无条件 save），
    // 本测试翻红（文件被归一化为 {enabled,servers,timeout}，foo 丢失）。
    let dir = tempfile::tempdir().unwrap();
    let installer = MCPInstaller::new(dir.path());
    let path = installer.config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, r#"{"foo": 1}"#).unwrap();
    let before = std::fs::read(&path).unwrap();

    installer.uninstall("never-existed").unwrap();

    let after = std::fs::read(&path).unwrap();
    assert_eq!(
        before, after,
        "no-op 卸载不得重写 config.mcp.json（外来顶层键会被剥掉）"
    );
}
