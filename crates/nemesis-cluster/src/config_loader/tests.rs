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
    // G2: 主动健康探针默认 60s 间隔、3 次失败判离线。
    assert_eq!(config.health_check_interval_secs, 60);
    assert_eq!(config.health_check_failure_threshold, 3);
    // G3: announce 过期阈值默认与协议常量一致。
    assert_eq!(
        config.announce_expiry_secs,
        crate::discovery::DEFAULT_EXPIRY_THRESHOLD_SECS
    );
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
        health_check_interval_secs: 30,
        health_check_failure_threshold: 2,
        announce_expiry_secs: 180,
        token: "cfg09-secret".to_string(),
    };

    save_app_config(workspace, &config).unwrap();
    let loaded = load_app_config(workspace);
    assert!(loaded.enabled);
    assert_eq!(loaded.port, 12345);
    assert_eq!(loaded.rpc_port, 22345);
    assert_eq!(loaded.health_check_interval_secs, 30);
    assert_eq!(loaded.health_check_failure_threshold, 2);
    assert_eq!(loaded.announce_expiry_secs, 180);
    // CFG-09：save 全量覆盖写不得抹掉 token（此前 typed 无此字段，
    // 一次 save 即把用户鉴权 token 静默清空）。
    assert_eq!(loaded.token, "cfg09-secret");
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
        health_check_interval_secs: 15,
        health_check_failure_threshold: 5,
        announce_expiry_secs: 0,
        token: "tok-xyz".to_string(),
    };
    let json = serde_json::to_string_pretty(&config).unwrap();
    let parsed: AppConfig = serde_json::from_str(&json).unwrap();
    assert!(parsed.enabled);
    assert_eq!(parsed.port, 9999);
    assert_eq!(parsed.rpc_port, 19999);
    assert_eq!(parsed.broadcast_interval, 45);
    assert_eq!(parsed.health_check_interval_secs, 15);
    assert_eq!(parsed.health_check_failure_threshold, 5);
    assert_eq!(parsed.announce_expiry_secs, 0);
    assert_eq!(parsed.token, "tok-xyz");
}

#[test]
fn test_app_config_deserialization_defaults() {
    let json = r#"{}"#;
    let config: AppConfig = serde_json::from_str(json).unwrap();
    assert!(!config.enabled);
    assert_eq!(config.port, 11949);
    assert_eq!(config.rpc_port, 21949);
    assert_eq!(config.broadcast_interval, 30);
    assert_eq!(config.health_check_interval_secs, 60);
    assert_eq!(config.health_check_failure_threshold, 3);
    assert_eq!(config.announce_expiry_secs, 120);
    // CFG-09：旧文件缺 token → 空串（= 无鉴权，语义不变）。
    assert!(config.token.is_empty());
}

/// CFG-09（2026-09-16）：token 的生产读取方（`Cluster::load_rpc_auth_token` /
/// discovery）是裸 JSON 读，不走 typed。此测试钉「裸写的 config.cluster.json
/// 带 token → typed load 拿得到 → save 回去不丢」的契约——save_app_config
/// 此前会整行抹掉裸写的 token。
#[test]
fn cfg09_save_preserves_hand_written_token() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path();
    let cfg_path = nemesis_path::resolve_cluster_config_path_in_workspace(workspace);
    std::fs::create_dir_all(cfg_path.parent().unwrap()).unwrap();
    // 模拟真实部署：文件是用户/老版本手写的裸 JSON（键序任意、含 token）。
    std::fs::write(
        &cfg_path,
        r#"{"enabled": true, "token": "hand-written-secret", "port": 12000}"#,
    )
    .unwrap();

    // 读-改-写（save_app_config 唯一合法用法）
    let mut cfg = load_app_config(workspace);
    assert_eq!(cfg.token, "hand-written-secret");
    cfg.broadcast_interval = 45;
    save_app_config(workspace, &cfg).unwrap();

    // 回读：token 保真 + 改动生效
    let reloaded = load_app_config(workspace);
    assert_eq!(reloaded.token, "hand-written-secret");
    assert_eq!(reloaded.broadcast_interval, 45);
    assert_eq!(reloaded.port, 12000);
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

// ============================================================
// S4 coverage: nested-parent save, unreadable app config,
// config dir creation on save.
// ============================================================

/// save_config creates missing parent directories (config_loader.rs 33-36).
#[test]
fn test_s4_save_config_creates_nested_parent_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .join("nested")
        .join("deeper")
        .join("cluster.json");
    save_config(&path, &ClusterConfig::default()).unwrap();
    assert!(path.exists());
}

/// config.cluster.json existing as a directory: exists() is true but the read
/// fails → default config (config_loader.rs 98-102).
#[test]
fn test_s4_load_app_config_unreadable_file_falls_back() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config").join("config.cluster.json");
    std::fs::create_dir_all(&cfg).unwrap(); // directory: exists() but read fails
    let app = load_app_config(dir.path());
    assert_eq!(app.port, 11949, "default port after read failure");
}

/// save_app_config creates the missing config directory
/// (config_loader.rs 112-115).
#[test]
fn test_s4_save_app_config_creates_config_dir() {
    let dir = tempfile::tempdir().unwrap();
    let app = AppConfig {
        enabled: true,
        ..Default::default()
    };
    save_app_config(dir.path(), &app).unwrap();
    let path = dir.path().join("config").join("config.cluster.json");
    assert!(path.exists());
    let loaded = load_app_config(dir.path());
    assert!(loaded.enabled);
}

// ---------------------------------------------------------------------
// 集群完备性加固 2026-09-11：坏 JSON 解析失败留 WARN 且按默认配置启动
// （旧实现 unwrap_or_default 双双静默——「配置写了却不生效」无日志可查）。
// ---------------------------------------------------------------------

#[test]
fn test_load_app_config_bad_json_falls_back_to_default() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("config").join("config.cluster.json");
    std::fs::create_dir_all(cfg_path.parent().unwrap()).unwrap();
    std::fs::write(&cfg_path, "{ not valid json !!!").unwrap();
    let app = load_app_config(dir.path());
    assert!(
        !app.enabled,
        "坏 JSON → 全默认（lenient 启动），enabled=false 绝不静默开集群"
    );
    assert_eq!(app.port, 11949);
    assert_eq!(app.rpc_port, 21949);
    assert_eq!(app.llm_timeout_secs, 7200);
}

#[test]
fn test_load_app_config_valid_json_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("config").join("config.cluster.json");
    std::fs::create_dir_all(cfg_path.parent().unwrap()).unwrap();
    std::fs::write(
        &cfg_path,
        r#"{"enabled":true,"port":13000,"rpc_port":23000,"broadcast_interval":15,"llm_timeout_secs":0}"#,
    )
    .unwrap();
    let app = load_app_config(dir.path());
    assert!(app.enabled);
    assert_eq!(app.port, 13000);
    assert_eq!(app.rpc_port, 23000);
    assert_eq!(app.broadcast_interval, 15);
    assert_eq!(app.llm_timeout_secs, 0, "0=不限语义透传到读取层");
}
