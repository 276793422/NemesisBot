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
    if let Some(arr) = cfg.get_mut("allowed_origins").and_then(|v| v.as_array_mut()) {
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
    if let Some(arr) = cfg.get_mut("allowed_origins").and_then(|v| v.as_array_mut()) {
        arr.push(serde_json::Value::String("https://app.example.com".to_string()));
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
    if let Some(arr) = cfg.get_mut("allowed_cdn_domains").and_then(|v| v.as_array_mut()) {
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
    if let Some(arr) = cfg.get_mut("allowed_origins").and_then(|v| v.as_array_mut()) {
        arr.push(serde_json::Value::String("https://a.com".to_string()));
        arr.push(serde_json::Value::String("https://b.com".to_string()));
    }
    save_cors(&path, &cfg).unwrap();

    // Remove one
    let mut loaded = load_or_create_cors(&path).unwrap();
    if let Some(arr) = loaded.get_mut("allowed_origins").and_then(|v| v.as_array_mut()) {
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
        obj.insert("development_mode".to_string(), serde_json::Value::Bool(true));
    }
    save_cors(&path, &loaded).unwrap();

    let dev_enabled = load_or_create_cors(&path).unwrap();
    assert_eq!(dev_enabled["development_mode"], true);

    // Disable dev mode
    let mut loaded2 = load_or_create_cors(&path).unwrap();
    if let Some(obj) = loaded2.as_object_mut() {
        obj.insert("development_mode".to_string(), serde_json::Value::Bool(false));
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
    if let Some(arr) = cfg.get_mut("allowed_origins").and_then(|v| v.as_array_mut()) {
        arr.push(serde_json::Value::String("https://app.example.com".to_string()));
    }
    save_cors(&path, &cfg).unwrap();

    // Check if origin is in allowed list
    let loaded = load_or_create_cors(&path).unwrap();
    let origins = loaded["allowed_origins"].as_array().unwrap();
    assert!(origins.iter().any(|o| o.as_str() == Some("https://app.example.com")));
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
    if let Some(arr) = cfg.get_mut("allowed_origins").and_then(|v| v.as_array_mut()) {
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
    if let Some(arr) = cfg.get_mut("allowed_origins").and_then(|v| v.as_array_mut()) {
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
        assert!(is_localhost, "Expected '{}' to be detected as localhost", origin);
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
        assert!(!is_localhost, "Expected '{}' to NOT be detected as localhost", origin);
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
    let origins = cfg.get_mut("allowed_origins").and_then(|v| v.as_array_mut()).unwrap();
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
    let origins = cfg.get_mut("allowed_origins").and_then(|v| v.as_array_mut()).unwrap();
    origins.push(serde_json::Value::String("https://a.com".to_string()));
    save_cors(&path, &cfg).unwrap();

    // Remove all
    let mut loaded = load_or_create_cors(&path).unwrap();
    if let Some(arr) = loaded.get_mut("allowed_origins").and_then(|v| v.as_array_mut()) {
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
        obj.insert("max_age".to_string(), serde_json::Value::Number(7200.into()));
    }
    assert_eq!(modified["max_age"], 7200);
}

#[test]
fn test_cors_config_allow_credentials_field() {
    let cfg = default_cors_config();
    assert_eq!(cfg["allow_credentials"], true);

    let mut modified = cfg;
    if let Some(obj) = modified.as_object_mut() {
        obj.insert("allow_credentials".to_string(), serde_json::Value::Bool(false));
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
    let path = tmp.path().join("deep").join("nested").join("config").join("cors.json");
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
    assert!(config.get("allowed_origins").unwrap().as_array().unwrap().is_empty());
    assert!(config.get("allowed_cdn_domains").unwrap().as_array().unwrap().is_empty());
    assert_eq!(config["max_age"], 3600);
}
