use super::*;

#[test]
fn test_save_and_load_config() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cluster.json");

    let config = ClusterConfig {
        node_id: "test-node-001".into(),
        bind_address: "0.0.0.0:9100".into(),
        peers: vec!["10.0.0.1:9100".into(), "10.0.0.2:9100".into()],
    };

    save_config(&path, &config).unwrap();
    assert!(path.exists());

    let loaded = load_config(&path).unwrap();
    assert_eq!(loaded.node_id, "test-node-001");
    assert_eq!(loaded.bind_address, "0.0.0.0:9100");
    assert_eq!(loaded.peers.len(), 2);
}

#[test]
fn test_load_or_default_missing_file() {
    let config = load_or_default(Some(Path::new("/nonexistent/cluster.json")));
    assert!(config.node_id.is_empty()); // default
}

#[test]
fn test_load_or_default_none() {
    let config = load_or_default(None);
    assert_eq!(config.bind_address, "0.0.0.0:9000");
}

#[test]
fn test_app_config_default() {
    let config = AppConfig::default();
    assert!(!config.enabled);
    assert_eq!(config.port, 11949);
    assert_eq!(config.rpc_port, 21949);
    assert_eq!(config.broadcast_interval, 30);
}

#[test]
fn test_app_config_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path();

    let config = AppConfig {
        enabled: true,
        port: 12345,
        rpc_port: 22345,
        broadcast_interval: 60,
        llm_timeout_secs: 7200,
    };

    save_app_config(workspace, &config).unwrap();
    let loaded = load_app_config(workspace);
    assert!(loaded.enabled);
    assert_eq!(loaded.port, 12345);
    assert_eq!(loaded.rpc_port, 22345);
}

// ============================================================
// Additional config_loader tests for missing coverage
// ============================================================

#[test]
fn test_config_error_io() {
    let err = ConfigError::Io(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "not found",
    ));
    assert!(err.to_string().contains("not found"));
}

#[test]
fn test_config_error_json() {
    let err = ConfigError::Json(serde_json::from_str::<ClusterConfig>("bad json").unwrap_err());
    assert!(err.to_string().contains("JSON"));
}

#[test]
fn test_load_config_invalid_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.json");
    std::fs::write(&path, "not valid json").unwrap();
    let result = load_config(&path);
    assert!(result.is_err());
}

#[test]
fn test_save_config_creates_parent_dir() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested").join("dir").join("cluster.json");

    let config = ClusterConfig::default();
    save_config(&path, &config).unwrap();
    assert!(path.exists());

    let loaded = load_config(&path).unwrap();
    assert_eq!(loaded.bind_address, "0.0.0.0:9000");
}

#[test]
fn test_load_or_default_with_valid_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cluster.json");

    let config = ClusterConfig {
        node_id: "test-123".into(),
        bind_address: "0.0.0.0:9200".into(),
        peers: vec![],
    };
    save_config(&path, &config).unwrap();

    let loaded = load_or_default(Some(&path));
    assert_eq!(loaded.node_id, "test-123");
    assert_eq!(loaded.bind_address, "0.0.0.0:9200");
}

#[test]
fn test_app_config_serialization_roundtrip() {
    let config = AppConfig {
        enabled: true,
        port: 9999,
        rpc_port: 19999,
        broadcast_interval: 45,
        llm_timeout_secs: 3600,
    };
    let json = serde_json::to_string_pretty(&config).unwrap();
    let parsed: AppConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.enabled, true);
    assert_eq!(parsed.port, 9999);
    assert_eq!(parsed.rpc_port, 19999);
    assert_eq!(parsed.broadcast_interval, 45);
}

#[test]
fn test_app_config_deserialization_defaults() {
    let json = r#"{}"#;
    let config: AppConfig = serde_json::from_str(json).unwrap();
    assert!(!config.enabled);
    assert_eq!(config.port, 11949);
    assert_eq!(config.rpc_port, 21949);
    assert_eq!(config.broadcast_interval, 30);
}

#[test]
fn test_load_app_config_nonexistent_dir() {
    let config = load_app_config(Path::new("/nonexistent/workspace"));
    assert!(!config.enabled);
    assert_eq!(config.port, 11949);
}

#[test]
fn test_load_app_config_invalid_json() {
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(config_dir.join("config.cluster.json"), "not valid json").unwrap();

    let config = load_app_config(dir.path());
    assert_eq!(config.port, 11949); // Falls back to default
}

#[test]
fn test_cluster_config_serialization() {
    let config = ClusterConfig {
        node_id: "node-test".into(),
        bind_address: "0.0.0.0:9999".into(),
        peers: vec!["10.0.0.1:9000".into()],
    };
    let json = serde_json::to_string(&config).unwrap();
    let parsed: ClusterConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.node_id, "node-test");
    assert_eq!(parsed.peers.len(), 1);
}
