use super::*;

#[tokio::test]
async fn test_install_and_is_installed() {
    let dir = tempfile::tempdir().unwrap();
    let installer = MCPInstaller::new(dir.path());

    assert!(!installer.is_installed("test-server").await);

    installer
        .install("test-server", "uv", vec!["run".into(), "server.py".into()])
        .await
        .unwrap();

    assert!(installer.is_installed("test-server").await);
}

#[tokio::test]
async fn test_uninstall() {
    let dir = tempfile::tempdir().unwrap();
    let installer = MCPInstaller::new(dir.path());

    installer
        .install("to-remove", "python", vec!["server.py".into()])
        .await
        .unwrap();

    assert!(installer.is_installed("to-remove").await);

    installer.uninstall("to-remove").await.unwrap();
    assert!(!installer.is_installed("to-remove").await);
}

#[tokio::test]
async fn test_config_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let installer = MCPInstaller::new(dir.path());

    installer
        .install("persist-test", "go", vec!["run".into()])
        .await
        .unwrap();

    // Create new installer instance
    let installer2 = MCPInstaller::new(dir.path());
    assert!(installer2.is_installed("persist-test").await);
}

// --- Additional mcp_installer tests ---

#[tokio::test]
async fn test_load_config_empty() {
    let dir = tempfile::tempdir().unwrap();
    let installer = MCPInstaller::new(dir.path());
    let config = installer.load_config().await.unwrap();
    assert!(config.servers.is_empty());
}

#[tokio::test]
async fn test_save_and_load_config() {
    let dir = tempfile::tempdir().unwrap();
    let installer = MCPInstaller::new(dir.path());
    let mut config = MCPConfig::default();
    config.servers.push(MCPServerConfig {
        name: "test-server".into(),
        command: "python".into(),
        args: vec!["server.py".into()],
    });
    installer.save_config(&config).await.unwrap();
    let loaded = installer.load_config().await.unwrap();
    assert_eq!(loaded.servers.len(), 1);
    assert_eq!(loaded.servers[0].name, "test-server");
}

#[tokio::test]
async fn test_install_multiple_servers() {
    let dir = tempfile::tempdir().unwrap();
    let installer = MCPInstaller::new(dir.path());
    installer.install("server-a", "go", vec!["run".into()]).await.unwrap();
    installer.install("server-b", "python", vec!["main.py".into()]).await.unwrap();
    assert!(installer.is_installed("server-a").await);
    assert!(installer.is_installed("server-b").await);
}

#[tokio::test]
async fn test_uninstall_nonexistent() {
    let dir = tempfile::tempdir().unwrap();
    let installer = MCPInstaller::new(dir.path());
    // Should not panic
    installer.uninstall("nonexistent").await.unwrap();
}

#[tokio::test]
async fn test_reinstall_overwrites() {
    let dir = tempfile::tempdir().unwrap();
    let installer = MCPInstaller::new(dir.path());
    installer.install("server", "go", vec!["v1".into()]).await.unwrap();
    installer.install("server", "python", vec!["v2".into()]).await.unwrap();
    let config = installer.load_config().await.unwrap();
    assert_eq!(config.servers[0].command, "python");
}

#[tokio::test]
async fn test_config_path_in_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let installer = MCPInstaller::new(dir.path());
    let path = installer.config_path();
    assert!(path.to_string_lossy().contains("config"));
}
