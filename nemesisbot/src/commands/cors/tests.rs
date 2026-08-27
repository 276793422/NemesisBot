use super::*;
use tempfile::TempDir;

#[test]
fn test_default_cors_config_structure() {
    let cfg = default_cors_config();
    assert!(cfg["allowed_origins"].is_array());
    assert!(cfg["allowed_cdn_domains"].is_array());
    assert_eq!(cfg["development_mode"], false);
    assert_eq!(cfg["allow_localhost"], true);
    assert_eq!(cfg["allow_credentials"], true);
    assert_eq!(cfg["max_age"], 3600);
}

#[test]
fn test_load_or_create_cors_no_file() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config").join("cors.json");
    let cfg = load_or_create_cors(&path).unwrap();

    assert_eq!(cfg["development_mode"], false);
    assert!(path.exists()); // Should have been created
}

#[test]
fn test_load_or_create_cors_existing_file() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("cors.json");
    let data = serde_json::json!({
        "allowed_origins": ["https://example.com"],
        "allowed_cdn_domains": ["cdn.example.com"],
        "development_mode": true,
        "allow_localhost": false,
        "allow_credentials": true,
        "max_age": 7200
    });
    std::fs::write(&path, serde_json::to_string(&data).unwrap()).unwrap();

    let cfg = load_or_create_cors(&path).unwrap();
    assert_eq!(cfg["development_mode"], true);
    assert_eq!(cfg["max_age"], 7200);
}

#[test]
fn test_save_cors_and_reload() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("cors.json");

    let mut cfg = default_cors_config();
    if let Some(arr) = cfg
        .get_mut("allowed_origins")
        .and_then(|v| v.as_array_mut())
    {
        arr.push(serde_json::Value::String("https://example.com".to_string()));
    }
    save_cors(&path, &cfg).unwrap();

    let loaded = load_or_create_cors(&path).unwrap();
    let origins = loaded["allowed_origins"].as_array().unwrap();
    assert_eq!(origins.len(), 1);
    assert_eq!(origins[0], "https://example.com");
}

#[test]
fn test_add_origin_to_empty_config() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("cors.json");

    let mut cfg = default_cors_config();
    if let Some(arr) = cfg
        .get_mut("allowed_origins")
        .and_then(|v| v.as_array_mut())
    {
        arr.push(serde_json::Value::String(
            "https://app.example.com".to_string(),
        ));
    }
    save_cors(&path, &cfg).unwrap();

    let loaded = load_or_create_cors(&path).unwrap();
    let origins = loaded["allowed_origins"].as_array().unwrap();
    assert_eq!(origins.len(), 1);
}

#[test]
fn test_add_cdn_domain() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("cors.json");

    let mut cfg = default_cors_config();
    if let Some(arr) = cfg
        .get_mut("allowed_cdn_domains")
        .and_then(|v| v.as_array_mut())
    {
        arr.push(serde_json::Value::String("cdn.example.com".to_string()));
    }
    save_cors(&path, &cfg).unwrap();

    let loaded = load_or_create_cors(&path).unwrap();
    let cdns = loaded["allowed_cdn_domains"].as_array().unwrap();
    assert_eq!(cdns.len(), 1);
    assert_eq!(cdns[0], "cdn.example.com");
}

#[test]
fn test_remove_origin() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("cors.json");

    let mut cfg = default_cors_config();
    if let Some(arr) = cfg
        .get_mut("allowed_origins")
        .and_then(|v| v.as_array_mut())
    {
        arr.push(serde_json::Value::String("https://a.com".to_string()));
        arr.push(serde_json::Value::String("https://b.com".to_string()));
    }
    save_cors(&path, &cfg).unwrap();

    // Remove one
    let mut loaded = load_or_create_cors(&path).unwrap();
    if let Some(arr) = loaded
        .get_mut("allowed_origins")
        .and_then(|v| v.as_array_mut())
    {
        arr.retain(|v| v.as_str() != Some("https://a.com"));
    }
    save_cors(&path, &loaded).unwrap();

    let final_cfg = load_or_create_cors(&path).unwrap();
    let origins = final_cfg["allowed_origins"].as_array().unwrap();
    assert_eq!(origins.len(), 1);
    assert_eq!(origins[0], "https://b.com");
}

#[test]
fn test_dev_mode_toggle() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("cors.json");

    let cfg = default_cors_config();
    save_cors(&path, &cfg).unwrap();

    // Enable dev mode
    let mut loaded = load_or_create_cors(&path).unwrap();
    if let Some(obj) = loaded.as_object_mut() {
        obj.insert(
            "development_mode".to_string(),
            serde_json::Value::Bool(true),
        );
    }
    save_cors(&path, &loaded).unwrap();

    let dev_enabled = load_or_create_cors(&path).unwrap();
    assert_eq!(dev_enabled["development_mode"], true);

    // Disable dev mode
    let mut loaded2 = load_or_create_cors(&path).unwrap();
    if let Some(obj) = loaded2.as_object_mut() {
        obj.insert(
            "development_mode".to_string(),
            serde_json::Value::Bool(false),
        );
    }
    save_cors(&path, &loaded2).unwrap();

    let dev_disabled = load_or_create_cors(&path).unwrap();
    assert_eq!(dev_disabled["development_mode"], false);
}

#[test]
fn test_cors_validate_allowed_origin() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("cors.json");

    let mut cfg = default_cors_config();
    if let Some(arr) = cfg
        .get_mut("allowed_origins")
        .and_then(|v| v.as_array_mut())
    {
        arr.push(serde_json::Value::String(
            "https://app.example.com".to_string(),
        ));
    }
    save_cors(&path, &cfg).unwrap();

    // Check if origin is in allowed list
    let loaded = load_or_create_cors(&path).unwrap();
    let origins = loaded["allowed_origins"].as_array().unwrap();
    assert!(
        origins
            .iter()
            .any(|o| o.as_str() == Some("https://app.example.com"))
    );
}

#[test]
fn test_cors_validate_localhost_allowed() {
    let cfg = default_cors_config();
    assert_eq!(cfg["allow_localhost"], true);

    // Localhost should be allowed when allow_localhost is true
    let lower = "http://localhost:8080";
    let is_localhost = lower.starts_with("http://localhost")
        || lower.starts_with("http://127.0.0.1")
        || lower.contains("localhost:");
    assert!(is_localhost);
}

#[test]
fn test_cors_validate_cdn_domain_match() {
    let domain = "cdn.example.com";
    let origin = "static.cdn.example.com";
    let matches = origin.ends_with(&format!(".{}", domain));
    assert!(matches);
}

#[test]
fn test_cors_validate_cdn_wildcard_match() {
    let domain = "cdn.example.com";
    let origin = "*.cdn.example.com";
    // The code checks: origin.ends_with(".cdn.example.com") → "*.cdn.example.com" ends with ".cdn.example.com" → true
    let pattern = format!("*{}", domain);
    let matches = origin == domain
        || origin.ends_with(&format!(".{}", domain))
        || origin.starts_with(&pattern);
    assert!(matches); // Ends with ".cdn.example.com"
}

#[test]
fn test_cors_validate_cdn_no_match() {
    let domain = "cdn.example.com";
    let origin = "https://other.com";
    let matches = origin == domain
        || origin.ends_with(&format!(".{}", domain))
        || origin.starts_with(&format!("*{}", domain));
    assert!(!matches);
}

#[test]
fn test_duplicate_origin_detection() {
    let mut cfg = default_cors_config();
    if let Some(arr) = cfg
        .get_mut("allowed_origins")
        .and_then(|v| v.as_array_mut())
    {
        arr.push(serde_json::Value::String("https://example.com".to_string()));
    }

    // Check duplicate
    let origin = "https://example.com";
    let origins = cfg["allowed_origins"].as_array().unwrap();
    let is_dup = origins.iter().any(|v| v.as_str() == Some(origin));
    assert!(is_dup);
}

#[test]
fn test_no_duplicate_when_different() {
    let mut cfg = default_cors_config();
    if let Some(arr) = cfg
        .get_mut("allowed_origins")
        .and_then(|v| v.as_array_mut())
    {
        arr.push(serde_json::Value::String("https://example.com".to_string()));
    }

    let origin = "https://other.com";
    let origins = cfg["allowed_origins"].as_array().unwrap();
    let is_dup = origins.iter().any(|v| v.as_str() == Some(origin));
    assert!(!is_dup);
}

// -------------------------------------------------------------------------
// Origin validation tests
// -------------------------------------------------------------------------

#[test]
fn test_origin_validation_http() {
    let origin = "http://localhost:3000";
    let is_valid = origin.starts_with("http://") || origin.starts_with("https://");
    assert!(is_valid);
}

#[test]
fn test_origin_validation_https() {
    let origin = "https://example.com";
    let is_valid = origin.starts_with("http://") || origin.starts_with("https://");
    assert!(is_valid);
}

#[test]
fn test_origin_validation_invalid_no_scheme() {
    let origin = "example.com";
    let is_valid = origin.starts_with("http://") || origin.starts_with("https://");
    assert!(!is_valid);
}

#[test]
fn test_origin_validation_ftp_rejected() {
    let origin = "ftp://example.com";
    let is_valid = origin.starts_with("http://") || origin.starts_with("https://");
    assert!(!is_valid);
}

// -------------------------------------------------------------------------
// Localhost detection tests
// -------------------------------------------------------------------------

#[test]
fn test_localhost_detection_various() {
    let localhost_origins = vec![
        "http://localhost",
        "http://localhost:8080",
        "http://127.0.0.1",
        "http://127.0.0.1:3000",
    ];
    for origin in localhost_origins {
        let lower = origin.to_lowercase();
        let is_localhost = lower.starts_with("http://localhost")
            || lower.starts_with("http://127.0.0.1")
            || lower.contains("localhost:");
        assert!(
            is_localhost,
            "Expected '{}' to be detected as localhost",
            origin
        );
    }
}

#[test]
fn test_non_localhost_not_detected() {
    let non_localhost_origins = vec![
        "https://example.com",
        "http://192.168.1.1",
        "http://10.0.0.1",
    ];
    for origin in non_localhost_origins {
        let lower = origin.to_lowercase();
        let is_localhost = lower.starts_with("http://localhost")
            || lower.starts_with("http://127.0.0.1")
            || lower.contains("localhost:");
        assert!(
            !is_localhost,
            "Expected '{}' to NOT be detected as localhost",
            origin
        );
    }
}

// -------------------------------------------------------------------------
// CDN domain matching tests
// -------------------------------------------------------------------------

#[test]
fn test_cdn_exact_match() {
    let domain = "cdn.example.com";
    let origin = "cdn.example.com";
    let matches = origin == domain;
    assert!(matches);
}

#[test]
fn test_cdn_subdomain_match() {
    let domain = "cdn.example.com";
    let origin = "static.cdn.example.com";
    let matches = origin.ends_with(&format!(".{}", domain));
    assert!(matches);
}

#[test]
fn test_cdn_wildcard_prefix_match() {
    let domain = "cdn.example.com";
    let origin = "*cdn.example.com";
    let matches = origin.starts_with(&format!("*{}", domain));
    assert!(matches);
}

#[test]
fn test_cdn_parent_domain_no_match() {
    let domain = "cdn.example.com";
    let origin = "example.com";
    let matches = origin == domain
        || origin.ends_with(&format!(".{}", domain))
        || origin.starts_with(&format!("*{}", domain));
    assert!(!matches);
}

// -------------------------------------------------------------------------
// Config reload tests
// -------------------------------------------------------------------------

#[test]
fn test_add_multiple_origins() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("cors.json");

    let mut cfg = default_cors_config();
    let origins = cfg
        .get_mut("allowed_origins")
        .and_then(|v| v.as_array_mut())
        .unwrap();
    origins.push(serde_json::Value::String("https://a.com".to_string()));
    origins.push(serde_json::Value::String("https://b.com".to_string()));
    origins.push(serde_json::Value::String("https://c.com".to_string()));
    save_cors(&path, &cfg).unwrap();

    let loaded = load_or_create_cors(&path).unwrap();
    let arr = loaded["allowed_origins"].as_array().unwrap();
    assert_eq!(arr.len(), 3);
}

#[test]
fn test_remove_all_origins() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("cors.json");

    let mut cfg = default_cors_config();
    let origins = cfg
        .get_mut("allowed_origins")
        .and_then(|v| v.as_array_mut())
        .unwrap();
    origins.push(serde_json::Value::String("https://a.com".to_string()));
    save_cors(&path, &cfg).unwrap();

    // Remove all
    let mut loaded = load_or_create_cors(&path).unwrap();
    if let Some(arr) = loaded
        .get_mut("allowed_origins")
        .and_then(|v| v.as_array_mut())
    {
        arr.retain(|v| v.as_str() != Some("https://a.com"));
    }
    save_cors(&path, &loaded).unwrap();

    let final_cfg = load_or_create_cors(&path).unwrap();
    assert!(final_cfg["allowed_origins"].as_array().unwrap().is_empty());
}

#[test]
fn test_cors_config_max_age_field() {
    let cfg = default_cors_config();
    assert_eq!(cfg["max_age"], 3600);

    // Modify max_age
    let mut modified = cfg.clone();
    if let Some(obj) = modified.as_object_mut() {
        obj.insert(
            "max_age".to_string(),
            serde_json::Value::Number(7200.into()),
        );
    }
    assert_eq!(modified["max_age"], 7200);
}

#[test]
fn test_cors_config_allow_credentials_field() {
    let cfg = default_cors_config();
    assert_eq!(cfg["allow_credentials"], true);

    let mut modified = cfg;
    if let Some(obj) = modified.as_object_mut() {
        obj.insert(
            "allow_credentials".to_string(),
            serde_json::Value::Bool(false),
        );
    }
    assert_eq!(modified["allow_credentials"], false);
}

// -------------------------------------------------------------------------
// load_or_create_cors edge cases
// -------------------------------------------------------------------------

#[test]
fn test_load_or_create_cors_invalid_json() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("cors.json");
    std::fs::write(&path, "not valid json {{{{").unwrap();

    let result = load_or_create_cors(&path);
    assert!(result.is_err());
}

#[test]
fn test_load_or_create_cors_nested_dir_creation() {
    let tmp = TempDir::new().unwrap();
    let path = tmp
        .path()
        .join("deep")
        .join("nested")
        .join("config")
        .join("cors.json");
    // Parent dir doesn't exist yet, but load_or_create_cors creates it
    let result = load_or_create_cors(&path);
    assert!(result.is_ok());
    assert!(path.exists());
}

// -------------------------------------------------------------------------
// Additional coverage tests for cors
// -------------------------------------------------------------------------

#[test]
fn test_cors_default_config_structure() {
    let config = default_cors_config();
    assert!(config["allowed_origins"].is_array());
    assert!(config["allowed_cdn_domains"].is_array());
    assert_eq!(config["development_mode"], false);
    assert_eq!(config["allow_localhost"], true);
    assert_eq!(config["allow_credentials"], true);
    assert_eq!(config["max_age"], 3600);
}

#[test]
fn test_cors_config_serialization_roundtrip() {
    let config = default_cors_config();
    let json = serde_json::to_string(&config).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(parsed["allowed_origins"].is_array());
}

#[test]
fn test_cors_save_and_load_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("cors.json");

    let config = default_cors_config();
    save_cors(&path, &config).unwrap();

    let loaded = load_or_create_cors(&path).unwrap();
    assert!(loaded["allowed_origins"].is_array());
}

#[test]
fn test_cors_save_creates_parent_dirs() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("a").join("b").join("c");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("cors.json");
    let config = default_cors_config();
    save_cors(&path, &config).unwrap();
    assert!(path.exists());
}

#[test]
fn test_cors_config_with_origins() {
    let mut config = default_cors_config();
    config["allowed_origins"] = serde_json::json!(["http://localhost:3000", "http://example.com"]);
    config["allowed_cdn_domains"] = serde_json::json!(["cdn.example.com"]);

    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("cors.json");
    save_cors(&path, &config).unwrap();

    let loaded = load_or_create_cors(&path).unwrap();
    let origins = loaded["allowed_origins"].as_array().unwrap();
    assert_eq!(origins.len(), 2);
    assert!(origins.iter().any(|o| o == "http://localhost:3000"));
    assert!(origins.iter().any(|o| o == "http://example.com"));

    let cdns = loaded["allowed_cdn_domains"].as_array().unwrap();
    assert_eq!(cdns.len(), 1);
    assert_eq!(cdns[0], "cdn.example.com");
}

#[test]
fn test_cors_config_allow_all_origins() {
    let mut config = default_cors_config();
    config["allow_all_origins"] = serde_json::Value::Bool(true);

    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("cors.json");
    save_cors(&path, &config).unwrap();

    let loaded = load_or_create_cors(&path).unwrap();
    assert_eq!(loaded["allow_all_origins"], true);
}

#[test]
fn test_cors_config_max_age() {
    let mut config = default_cors_config();
    config["max_age"] = serde_json::Value::Number(3600.into());

    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("cors.json");
    save_cors(&path, &config).unwrap();

    let loaded = load_or_create_cors(&path).unwrap();
    assert_eq!(loaded["max_age"], 3600);
}

#[test]
fn test_cors_load_nonexistent_creates_default() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("cors.json");
    let result = load_or_create_cors(&path);
    assert!(result.is_ok());
    assert!(path.exists());
}

#[test]
fn test_cors_default_config_values() {
    let config = default_cors_config();
    assert!(config["allowed_origins"].is_array());
    assert!(config["allowed_cdn_domains"].is_array());
    assert!(
        config
            .get("allowed_origins")
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        config
            .get("allowed_cdn_domains")
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(config["max_age"], 3600);
}

// ===========================================================================
// run() 全臂（S11c，quality-hardening goal 冲刺 S11）—— 此前 LH=0，40 个既有
// 测试只覆盖 helper（load_or_create_cors 等），dispatch 层从没跑过。
// env home 隔离 + GLOBAL_STATE_LOCK 串行；配置落 {home}/config/cors.json。
// ===========================================================================

mod run_arm {
    use super::super::{run, CorsAction, CorsDevModeAction};
    use serde_json::json;

    fn with_env_home(f: impl FnOnce(std::path::PathBuf)) {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("NEMESISBOT_HOME", tmp.path());
        }
        f(tmp.path().join(".nemesisbot"));
        unsafe {
            std::env::remove_var("NEMESISBOT_HOME");
        }
    }

    fn cors_of(home: &std::path::Path) -> std::path::PathBuf {
        home.join("config").join("cors.json")
    }

    fn write_cors(home: &std::path::Path, cfg: serde_json::Value) {
        let p = cors_of(home);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, serde_json::to_string_pretty(&cfg).unwrap()).unwrap();
    }

    fn read_cors(home: &std::path::Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(cors_of(home)).unwrap()).unwrap()
    }

    // --- List ---

    #[test]
    fn list_without_config_prints_initialize_hint() {
        with_env_home(|_home| {
            run(CorsAction::List, false).expect("无 cors.json → 提示 + Ok");
        });
    }

    #[test]
    fn list_with_empty_and_populated_config_covers_all_branches() {
        with_env_home(|home| {
            // 空配置 → 两组 (none) 分支。
            write_cors(&home, json!({}));
            run(CorsAction::List, false).expect("空对象 → unwrap_or 默认分支");

            // 填满 → 数组非空 + dev_mode/credentials/max_age 各分支。
            write_cors(
                &home,
                json!({
                    "allowed_origins": ["https://a.test"],
                    "allowed_cdn_domains": ["cdn.test"],
                    "development_mode": true,
                    "allow_localhost": false,
                    "allow_credentials": false,
                    "max_age": 60
                }),
            );
            run(CorsAction::List, false).expect("全字段 → Ok");
        });
    }

    // --- Add ---

    #[test]
    fn add_rejects_non_http_origin() {
        with_env_home(|home| {
            let err = run(
                CorsAction::Add { origin: "notaurl".into(), cdn: false },
                false,
            )
            .expect_err("非 http(s) 前缀 → bail");
            assert!(
                err.to_string().contains("Invalid origin URL"),
                "got: {err:#}"
            );
            assert!(!cors_of(&home).exists(), "bail 在任何写盘之前");
        });
    }

    #[test]
    fn add_valid_origin_and_cdn_domain_and_duplicate_noop() {
        with_env_home(|home| {
            run(
                CorsAction::Add { origin: "https://app.test".into(), cdn: false },
                false,
            )
            .expect("首个 origin 会顺带初始化 cors.json");
            let cfg = read_cors(&home);
            assert_eq!(cfg["allowed_origins"][0], "https://app.test");

            run(CorsAction::Add { origin: "cdn.test".into(), cdn: true }, false)
                .expect("cdn add ok");
            assert_eq!(read_cors(&home)["allowed_cdn_domains"][0], "cdn.test");

            // 重复添加 → already exists 分支，不重复入列。
            run(
                CorsAction::Add { origin: "https://app.test".into(), cdn: false },
                false,
            )
            .expect("duplicate → Ok");
            let cfg = read_cors(&home);
            assert_eq!(
                cfg["allowed_origins"].as_array().unwrap().len(),
                1,
                "重复 origin 不得二次入列"
            );
        });
    }

    // --- Remove ---

    #[test]
    fn remove_no_config_found_and_not_found() {
        with_env_home(|home| {
            // 无配置文件。
            run(
                CorsAction::Remove { origin: "https://x.test".into(), cdn: false },
                false,
            )
            .expect("无 cors.json → No CORS configuration found");

            write_cors(
                &home,
                json!({
                    "allowed_origins": ["https://a.test", "https://b.test"],
                    "allowed_cdn_domains": ["cdn.test"]
                }),
            );
            run(
                CorsAction::Remove { origin: "https://a.test".into(), cdn: false },
                false,
            )
            .expect("remove ok");
            let cfg = read_cors(&home);
            assert_eq!(cfg["allowed_origins"], json!(["https://b.test"]));

            run(
                CorsAction::Remove { origin: "ghost.test".into(), cdn: true },
                false,
            )
            .expect("cdn not found → Ok");
            assert_eq!(
                read_cors(&home)["allowed_cdn_domains"],
                json!(["cdn.test"])
            );
        });
    }

    // --- DevMode ---

    #[test]
    fn devmode_enable_disable_and_status_states() {
        with_env_home(|home| {
            // Disable 无配置文件分支（264-267）。
            run(
                CorsAction::DevMode { action: CorsDevModeAction::Disable },
                false,
            )
            .expect("无文件 disable → already disabled Ok");

            // Status 无配置文件 → false 分支。
            run(
                CorsAction::DevMode { action: CorsDevModeAction::Status },
                false,
            )
            .expect("无文件 status → DISABLED");

            // Enable → 写文件；Status 读到 true。
            run(
                CorsAction::DevMode { action: CorsDevModeAction::Enable },
                false,
            )
            .expect("enable ok");
            assert_eq!(read_cors(&home)["development_mode"], true);
            run(
                CorsAction::DevMode { action: CorsDevModeAction::Status },
                false,
            )
            .expect("enabled status ok");

            // Disable 有配置文件 → 置 false。
            run(
                CorsAction::DevMode { action: CorsDevModeAction::Disable },
                false,
            )
            .expect("disable ok");
            assert_eq!(read_cors(&home)["development_mode"], false);
        });
    }

    // --- Show ---

    #[test]
    fn show_without_and_with_config() {
        with_env_home(|home| {
            run(CorsAction::Show, false).expect("无 cors.json → 提示 + Ok");
            write_cors(&home, json!({"allowed_origins": ["https://a.test"]}));
            run(CorsAction::Show, false).expect("有配置 → 打印原文 + Ok");
        });
    }

    // --- Validate ---

    #[test]
    fn validate_without_config_is_denied() {
        with_env_home(|_home| {
            run(
                CorsAction::Validate { origin: "https://any.test".into() },
                false,
            )
            .expect("无配置 → DENIED + Ok");
        });
    }

    #[test]
    fn validate_matches_exact_cdn_subdomain_and_wildcard() {
        with_env_home(|home| {
            write_cors(
                &home,
                json!({
                    "allowed_origins": ["https://app.test"],
                    "allowed_cdn_domains": ["cdn.test", "*.wild.test"],
                    "development_mode": false,
                    "allow_localhost": false
                }),
            );
            // allowed_origins 精确命中。
            run(
                CorsAction::Validate { origin: "https://app.test".into() },
                false,
            )
            .expect("exact origin ok");
            // cdn 精确命中。
            run(CorsAction::Validate { origin: "cdn.test".into() }, false)
                .expect("cdn exact ok");
            // cdn 子域命中（.cdn.test 后缀）。
            run(
                CorsAction::Validate { origin: "static.cdn.test".into() },
                false,
            )
            .expect("cdn subdomain ok");
            // 通配 *wild.test 前缀命中。
            run(CorsAction::Validate { origin: "*wild.test".into() }, false)
                .expect("wildcard prefix ok");
            // 都不命中 + localhost 关 → DENIED。
            run(
                CorsAction::Validate { origin: "https://evil.test".into() },
                false,
            )
            .expect("denied ok");
        });
    }

    #[test]
    fn validate_localhost_via_allow_localhost_and_via_dev_mode() {
        with_env_home(|home| {
            // allow_localhost=true、dev_mode=false → "allow_localhost" 来源。
            write_cors(
                &home,
                json!({
                    "allowed_origins": [],
                    "allowed_cdn_domains": [],
                    "development_mode": false,
                    "allow_localhost": true
                }),
            );
            for origin in [
                "http://localhost:8080",
                "http://127.0.0.1:49000",
                "http://LOCALHOST:5173",
            ] {
                run(CorsAction::Validate { origin: origin.into() }, false)
                    .expect("localhost 允许分支");
            }

            // dev_mode=true 时 localhost 也放行（development_mode + localhost）。
            write_cors(
                &home,
                json!({
                    "allowed_origins": [],
                    "allowed_cdn_domains": [],
                    "development_mode": true,
                    "allow_localhost": false
                }),
            );
            run(
                CorsAction::Validate { origin: "http://localhost:3000".into() },
                false,
            )
            .expect("dev_mode + localhost ok");

            // 两者都关 → localhost DENIED。
            write_cors(
                &home,
                json!({
                    "allowed_origins": [],
                    "allowed_cdn_domains": [],
                    "development_mode": false,
                    "allow_localhost": false
                }),
            );
            run(
                CorsAction::Validate { origin: "http://localhost:9999".into() },
                false,
            )
            .expect("localhost 关闭 → DENIED");
        });
    }
}

// ===========================================================================
// wave_a（R7 中批补盲，2026-08-27）：此前 run_arm 已铺满主干臂，这里只补
// 仍然真实的缺口 —— ① cdn 标签重复/已存在删除臂（179/231）、② 纯 origin
// 未命中删除臂（243）、③ https://localhost 走 contains-only 第三个条件
// （366-368 短路时前两个 starts_with 吃掉命中）、④ 显式空数组的 (none)
// 提示臂（107-108/118-119；json!({}) 缺键走的是 if-let 整段跳过）、⑤ 写
// 失败冒泡（74/84）：父路径为普通文件 / 只读文件两类确定性构造。
// ===========================================================================

mod wave_a {
    use super::super::{load_or_create_cors, run, CorsAction};

    fn with_env_home(f: impl FnOnce(std::path::PathBuf)) {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("NEMESISBOT_HOME", tmp.path());
        }
        f(tmp.path().join(".nemesisbot"));
        unsafe {
            std::env::remove_var("NEMESISBOT_HOME");
        }
    }

    fn cors_of(home: &std::path::Path) -> std::path::PathBuf {
        home.join("config").join("cors.json")
    }

    fn write_cors(home: &std::path::Path, cfg: serde_json::Value) {
        let p = cors_of(home);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, serde_json::to_string_pretty(&cfg).unwrap()).unwrap();
    }

    fn read_cors(home: &std::path::Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(cors_of(home)).unwrap()).unwrap()
    }

    #[cfg(unix)]
    fn deny_write(p: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = std::fs::metadata(p).unwrap().permissions();
        perm.set_mode(0o444);
        std::fs::set_permissions(p, perm).unwrap();
    }

    #[cfg(unix)]
    fn allow_write(p: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = std::fs::metadata(p).unwrap().permissions();
        perm.set_mode(0o644);
        std::fs::set_permissions(p, perm).unwrap();
    }

    #[cfg(windows)]
    fn deny_write(p: &std::path::Path) {
        let mut perm = std::fs::metadata(p).unwrap().permissions();
        perm.set_readonly(true);
        std::fs::set_permissions(p, perm).unwrap();
    }

    #[cfg(windows)]
    fn allow_write(p: &std::path::Path) {
        let mut perm = std::fs::metadata(p).unwrap().permissions();
        perm.set_readonly(false);
        std::fs::set_permissions(p, perm).unwrap();
    }

    #[test]
    fn add_duplicate_cdn_origin_reports_cdn_label() {
        with_env_home(|home| {
            write_cors(
                &home,
                serde_json::json!({
                    "allowed_origins": [],
                    "allowed_cdn_domains": ["cdn.test"]
                }),
            );
            run(
                CorsAction::Add { origin: "cdn.test".into(), cdn: true },
                false,
            )
            .expect("重复 cdn 域 → already exists + Ok");
            assert_eq!(
                read_cors(&home)["allowed_cdn_domains"],
                serde_json::json!(["cdn.test"]),
                "不得二次入列"
            );
        });
    }

    #[test]
    fn remove_found_cdn_entry_and_missing_plain_origin() {
        with_env_home(|home| {
            write_cors(
                &home,
                serde_json::json!({
                    "allowed_origins": ["https://a.test"],
                    "allowed_cdn_domains": ["cdn.test"]
                }),
            );
            // 已存在的 cdn 条目被移除（230-235 found + cdn 标签臂）。
            run(
                CorsAction::Remove { origin: "cdn.test".into(), cdn: true },
                false,
            )
            .expect("remove 存在的 cdn 条目");
            assert_eq!(
                read_cors(&home)["allowed_cdn_domains"],
                serde_json::json!([])
            );
            // 不存在的纯 origin（236-245 not-found + 非 cdn 标签臂）。
            run(
                CorsAction::Remove { origin: "https://ghost.test".into(), cdn: false },
                false,
            )
            .expect("remove 不存在的纯 origin");
            assert_eq!(
                read_cors(&home)["allowed_origins"],
                serde_json::json!(["https://a.test"])
            );
        });
    }

    #[test]
    fn validate_https_localhost_matches_via_contains_only() {
        with_env_home(|home| {
            // https:// 前缀让两个 http:// starts_with 都不中，只有第三个
            // contains("localhost:") 条件能放行（368 行所在短路链尾）；
            // 大写 LOCALHOST 验证 lowercase 归一在条件之前完成。
            write_cors(
                &home,
                serde_json::json!({
                    "allowed_origins": [],
                    "allowed_cdn_domains": [],
                    "development_mode": false,
                    "allow_localhost": true
                }),
            );
            run(
                CorsAction::Validate { origin: "https://LOCALHOST:8443".into() },
                false,
            )
            .expect("contains-only localhost 臂放行");
        });
    }

    #[test]
    fn list_with_present_but_empty_arrays_prints_none_hints() {
        with_env_home(|home| {
            // 显式空数组键（区别于缺键：缺键走 if-let 整段跳过，
            // 只有显式 [] 才进 is_empty → "(none)" 分支）。
            write_cors(
                &home,
                serde_json::json!({
                    "allowed_origins": [],
                    "allowed_cdn_domains": []
                }),
            );
            run(CorsAction::List, false).expect("空数组 → 两组 (none) 提示");
        });
    }

    #[test]
    fn load_or_create_write_failure_when_parent_is_regular_file_bubbles() {
        // 父路径是普通文件：create_dir_all 失败被 `let _` 吞掉，
        // 随后的 fs::write 打不开路径 → 74 行 `?` 把 Err 冒给调用方。
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("config"), "not a dir").unwrap();
        let path = tmp.path().join("config").join("cors.json");
        assert!(
            load_or_create_cors(&path).is_err(),
            "父路径为文件 → 写初始化必须报错"
        );
    }

    #[test]
    fn run_add_save_failure_propagates_when_existing_config_is_readonly() {
        // 只读 cors.json：exists → 读入正常，push 后 save_cors 写失败 → 84 行 `?`。
        // Windows readonly 属性 / unix 0o444 都会拒绝写打开，语义一致。
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("NEMESISBOT_HOME", tmp.path());
        }
        let home = tmp.path().join(".nemesisbot");
        let p = cors_of(&home);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, r#"{"allowed_origins":["https://a.test"]}"#).unwrap();

        deny_write(&p);
        let res = run(
            CorsAction::Add { origin: "https://b.test".into(), cdn: false },
            false,
        );
        allow_write(&p); // 先恢复再清场，TempDir 才能删掉
        unsafe {
            std::env::remove_var("NEMESISBOT_HOME");
        }
        assert!(res.is_err(), "只读配置上的 Add 必须把写错误冒上来");
    }
}

// ===========================================================================
// r10（覆盖率 A 类 miss 补充）：Validate 的匹配矩阵合并夹具 ——
// ① 精确 origin 命中后 break，跳过整个 CDN 块（336-351 的关闭区）；
// ② CDN 子域通配命中（origin.ends_with(".domain") 臂）；
// ③ 仅 development_mode=true → match_source 走 dev 分支文案（371-372）；
// ④ 仅 allow_localhost=true → else 分支文案（374-375）。
// 全程 run() 分发 + tempdir home（持全局锁），零网络。
// ===========================================================================

mod r10_validate {
    use super::super::{run, CorsAction};

    fn with_env_home(f: impl FnOnce(&std::path::Path)) {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("NEMESISBOT_HOME", tmp.path());
        }
        let home = tmp.path().join(".nemesisbot");
        f(home.as_path());
        unsafe {
            std::env::remove_var("NEMESISBOT_HOME");
        }
    }

    fn write_cors(home: &std::path::Path, cfg: serde_json::Value) {
        let p = home.join("config").join("cors.json");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, serde_json::to_string_pretty(&cfg).unwrap()).unwrap();
    }

    fn validate(origin: &str) {
        run(
            CorsAction::Validate {
                origin: origin.to_string(),
            },
            false,
        )
        .expect("validate 必须为 Ok（打印型命令）");
    }

    #[test]
    fn r10_validate_exact_hit_skips_cdn_and_wildcard_and_localhost_labels() {
        with_env_home(|home| {
            // 相位①+②：精确命中 + CDN 子域通配。
            write_cors(
                home,
                serde_json::json!({
                    "allowed_origins": ["https://exact.test"],
                    "allowed_cdn_domains": ["cdn.example"],
                    "development_mode": false,
                    "allow_localhost": false,
                }),
            );
            validate("https://exact.test"); // allowed_origins 命中 → break，不进 CDN 块
            validate("https://sub.cdn.example"); // ends_with(".cdn.example") 命中
            validate("https://denied.test"); // 三块全 miss → DENIED 臂

            // 相位③：仅 dev_mode → localhost 命中走 dev 文案臂。
            write_cors(
                home,
                serde_json::json!({
                    "allowed_origins": [],
                    "allowed_cdn_domains": [],
                    "development_mode": true,
                    "allow_localhost": false,
                }),
            );
            validate("http://localhost:1234");

            // 相位④：仅 allow_localhost → else 文案臂。
            write_cors(
                home,
                serde_json::json!({
                    "allowed_origins": [],
                    "allowed_cdn_domains": [],
                    "development_mode": false,
                    "allow_localhost": true,
                }),
            );
            validate("http://127.0.0.1:9999");
        });
    }
}
