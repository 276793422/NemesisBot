use super::*;

fn sample_config() -> CORSConfig {
    CORSConfig {
        allowed_origins: vec![
            "https://example.com".into(),
            "https://app.example.com".into(),
        ],
        allowed_cdn_domains: vec!["cdn.example.com".into()],
        ..CORSConfig::default()
    }
}

// -- CORSConfig defaults ------------------------------------------------

#[test]
fn default_config_values() {
    let cfg = CORSConfig::default();
    assert!(cfg.allowed_origins.is_empty());
    assert_eq!(cfg.allowed_methods, vec!["GET", "POST"]);
    assert_eq!(cfg.allowed_headers, vec!["Content-Type", "Authorization"]);
    assert!(cfg.allow_credentials);
    assert_eq!(cfg.max_age, 3600);
    assert!(cfg.allow_localhost);
    assert!(!cfg.development_mode);
    assert!(cfg.allowed_cdn_domains.is_empty());
    assert!(cfg.allow_no_origin);
}

#[test]
fn config_serialization_roundtrip() {
    let cfg = sample_config();
    let json = serde_json::to_string_pretty(&cfg).unwrap();
    let parsed: CORSConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(cfg.allowed_origins, parsed.allowed_origins);
    assert_eq!(cfg.allowed_cdn_domains, parsed.allowed_cdn_domains);
    assert_eq!(cfg.allow_localhost, parsed.allow_localhost);
}

// -- Origin checking ----------------------------------------------------

#[test]
fn check_exact_match() {
    let mgr = CORSManager {
        config: RwLock::new(sample_config()),
        config_path: PathBuf::from("/tmp/test_cors.json"),
    };
    assert!(mgr.check_origin("https://example.com"));
    assert!(mgr.check_origin("https://app.example.com"));
}

#[test]
fn check_localhost_allowed() {
    let mgr = CORSManager {
        config: RwLock::new(sample_config()),
        config_path: PathBuf::from("/tmp/test_cors.json"),
    };
    assert!(mgr.check_origin("http://localhost:3000"));
    assert!(mgr.check_origin("http://127.0.0.1:8080"));
}

#[test]
fn check_localhost_denied_when_disabled() {
    let mut cfg = sample_config();
    cfg.allow_localhost = false;
    let mgr = CORSManager {
        config: RwLock::new(cfg),
        config_path: PathBuf::from("/tmp/test_cors.json"),
    };
    assert!(!mgr.check_origin("http://localhost:3000"));
    assert!(!mgr.check_origin("http://127.0.0.1:8080"));
}

#[test]
fn check_cdn_subdomain() {
    let mgr = CORSManager {
        config: RwLock::new(sample_config()),
        config_path: PathBuf::from("/tmp/test_cors.json"),
    };
    // Exact CDN match.
    assert!(mgr.check_origin("https://cdn.example.com"));
    // Subdomain of CDN.
    assert!(mgr.check_origin("https://abc.cdn.example.com"));
    // Must NOT match a suffix that is not a proper subdomain.
    assert!(!mgr.check_origin("https://fake-cdn.example.com.evil.com"));
}

#[test]
fn check_unknown_origin_denied() {
    let mgr = CORSManager {
        config: RwLock::new(sample_config()),
        config_path: PathBuf::from("/tmp/test_cors.json"),
    };
    assert!(!mgr.check_origin("https://evil.com"));
    assert!(!mgr.check_origin("http://192.168.1.1:3000"));
}

#[test]
fn check_no_origin() {
    let mgr = CORSManager {
        config: RwLock::new(sample_config()),
        config_path: PathBuf::from("/tmp/test_cors.json"),
    };
    // Default: allow_no_origin = true.
    assert!(mgr.check_origin(""));

    let mut cfg = sample_config();
    cfg.allow_no_origin = false;
    let mgr2 = CORSManager {
        config: RwLock::new(cfg),
        config_path: PathBuf::from("/tmp/test_cors.json"),
    };
    assert!(!mgr2.check_origin(""));
}

#[test]
fn check_development_mode_allows_localhost() {
    let mut cfg = sample_config();
    cfg.development_mode = true;
    // Even with allow_localhost=false, development_mode enables it.
    cfg.allow_localhost = false;
    let mgr = CORSManager {
        config: RwLock::new(cfg),
        config_path: PathBuf::from("/tmp/test_cors.json"),
    };
    assert!(mgr.check_origin("http://localhost:3000"));
    assert!(mgr.check_origin("http://127.0.0.1:9999"));
    // Non-localhost origins are still subject to the allow-list.
    assert!(!mgr.check_origin("https://evil.com"));
}

// -- Runtime mutation ---------------------------------------------------

#[test]
fn add_and_remove_origin() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cors.json");
    let mgr = CORSManager::new(&path).unwrap();

    mgr.add_origin("https://foo.com").unwrap();
    assert!(mgr.list_origins().contains(&"https://foo.com".to_string()));

    // Duplicate add is a no-op.
    mgr.add_origin("https://foo.com").unwrap();
    assert_eq!(
        mgr.list_origins()
            .iter()
            .filter(|o| **o == "https://foo.com")
            .count(),
        1
    );

    mgr.remove_origin("https://foo.com").unwrap();
    assert!(!mgr.list_origins().contains(&"https://foo.com".to_string()));
}

#[test]
fn set_development_mode_persists() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cors.json");
    let mgr = CORSManager::new(&path).unwrap();

    assert!(!mgr.config().development_mode);
    mgr.set_development_mode(true).unwrap();
    assert!(mgr.config().development_mode);

    // Reload from file to verify persistence.
    let mgr2 = CORSManager::new(&path).unwrap();
    assert!(mgr2.config().development_mode);
}

// -- Static layer helpers -----------------------------------------------

#[test]
fn test_dev_cors_layer_creation() {
    let _layer = dev_cors_layer();
}

#[test]
fn test_production_cors_layer_creation() {
    let origins = vec!["https://example.com".to_string()];
    let _layer = production_cors_layer(&origins);
}

#[test]
fn test_cors_layer_from_manager_dev_mode() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cors.json");
    let mgr = CORSManager::new(&path).unwrap();
    mgr.set_development_mode(true).unwrap();
    let _layer = cors_layer_from_manager(&mgr);
}

#[test]
fn test_cors_layer_from_manager_prod() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cors.json");
    let mgr = CORSManager::new(&path).unwrap();
    let _layer = cors_layer_from_manager(&mgr);
}

#[test]
fn test_config_deserialization_missing_optional_fields() {
    let json = r#"{}"#;
    let cfg: CORSConfig = serde_json::from_str(json).unwrap();
    assert!(cfg.allowed_origins.is_empty());
    assert_eq!(cfg.allowed_methods, vec!["GET", "POST"]);
    assert_eq!(cfg.allowed_headers, vec!["Content-Type", "Authorization"]);
    assert!(cfg.allow_credentials);
    assert_eq!(cfg.max_age, 3600);
}

#[test]
fn test_config_serialization_includes_all_fields() {
    let cfg = CORSConfig::default();
    let json = serde_json::to_string(&cfg).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(parsed.get("allowed_origins").is_some());
    assert!(parsed.get("allowed_methods").is_some());
    assert!(parsed.get("allow_credentials").is_some());
    assert!(parsed.get("max_age").is_some());
    assert!(parsed.get("allow_localhost").is_some());
}

#[test]
fn test_check_exact_origin_match_multiple() {
    let mgr = CORSManager {
        config: RwLock::new(CORSConfig {
            allowed_origins: vec![
                "https://a.com".into(),
                "https://b.com".into(),
                "https://c.com".into(),
            ],
            allow_localhost: false,
            ..CORSConfig::default()
        }),
        config_path: PathBuf::from("/tmp/test_cors.json"),
    };
    assert!(mgr.check_origin("https://a.com"));
    assert!(mgr.check_origin("https://b.com"));
    assert!(mgr.check_origin("https://c.com"));
    assert!(!mgr.check_origin("https://d.com"));
}

#[test]
fn test_check_origin_with_port() {
    let mgr = CORSManager {
        config: RwLock::new(CORSConfig {
            allowed_origins: vec!["https://example.com:8443".into()],
            allow_localhost: false,
            ..CORSConfig::default()
        }),
        config_path: PathBuf::from("/tmp/test_cors.json"),
    };
    assert!(mgr.check_origin("https://example.com:8443"));
    assert!(!mgr.check_origin("https://example.com"));
}

#[test]
fn test_check_cdn_exact_domain_match() {
    let mgr = CORSManager {
        config: RwLock::new(CORSConfig {
            allowed_cdn_domains: vec!["cdn.example.com".into()],
            allow_localhost: false,
            ..CORSConfig::default()
        }),
        config_path: PathBuf::from("/tmp/test_cors.json"),
    };
    assert!(mgr.check_origin("https://cdn.example.com"));
}

#[test]
fn test_check_cdn_deep_subdomain() {
    let mgr = CORSManager {
        config: RwLock::new(CORSConfig {
            allowed_cdn_domains: vec!["cdn.example.com".into()],
            allow_localhost: false,
            ..CORSConfig::default()
        }),
        config_path: PathBuf::from("/tmp/test_cors.json"),
    };
    assert!(mgr.check_origin("https://a.b.cdn.example.com"));
}

#[test]
fn test_check_localhost_various_ports() {
    let mgr = CORSManager {
        config: RwLock::new(CORSConfig {
            allow_localhost: true,
            ..CORSConfig::default()
        }),
        config_path: PathBuf::from("/tmp/test_cors.json"),
    };
    assert!(mgr.check_origin("http://localhost:3000"));
    assert!(mgr.check_origin("http://localhost:8080"));
    assert!(mgr.check_origin("http://localhost:5173"));
    assert!(mgr.check_origin("http://127.0.0.1:3000"));
    assert!(mgr.check_origin("http://127.0.0.1:9999"));
}

#[test]
fn test_check_development_mode_does_not_allow_arbitrary() {
    let mgr = CORSManager {
        config: RwLock::new(CORSConfig {
            development_mode: true,
            allowed_origins: vec![],
            ..CORSConfig::default()
        }),
        config_path: PathBuf::from("/tmp/test_cors.json"),
    };
    // Dev mode allows localhost but not arbitrary origins
    assert!(!mgr.check_origin("https://evil.com"));
}

#[test]
fn test_add_origin_creates_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("new_cors.json");
    let mgr = CORSManager::new(&path).unwrap();
    mgr.add_origin("https://new.com").unwrap();
    assert!(path.exists());
    let loaded = CORSManager::new(&path).unwrap();
    assert!(
        loaded
            .list_origins()
            .contains(&"https://new.com".to_string())
    );
}

#[test]
fn test_remove_origin_nonexistent_is_noop() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cors.json");
    let mgr = CORSManager::new(&path).unwrap();
    // Should not error
    mgr.remove_origin("https://nonexistent.com").unwrap();
}

#[test]
fn test_add_multiple_origins() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cors.json");
    let mgr = CORSManager::new(&path).unwrap();
    mgr.add_origin("https://a.com").unwrap();
    mgr.add_origin("https://b.com").unwrap();
    mgr.add_origin("https://c.com").unwrap();
    let origins = mgr.list_origins();
    assert_eq!(origins.len(), 3);
}

#[test]
fn test_config_cloning() {
    let cfg = CORSConfig {
        allowed_origins: vec!["https://example.com".into()],
        ..CORSConfig::default()
    };
    let cloned = cfg.clone();
    assert_eq!(cloned.allowed_origins, cfg.allowed_origins);
    assert_eq!(cloned.max_age, cfg.max_age);
}

#[test]
fn test_production_cors_layer_with_empty_origins() {
    let _layer = production_cors_layer(&[]);
}

#[test]
fn test_production_cors_layer_with_invalid_origin() {
    let origins = vec!["not-a-valid-url".to_string()];
    let _layer = production_cors_layer(&origins);
}

#[test]
fn test_manager_config_returns_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cors.json");
    let mgr = CORSManager::new(&path).unwrap();
    let cfg1 = mgr.config();
    mgr.add_origin("https://test.com").unwrap();
    let cfg2 = mgr.config();
    assert!(cfg1.allowed_origins.is_empty());
    assert_eq!(cfg2.allowed_origins.len(), 1);
}
