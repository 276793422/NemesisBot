//! CORS middleware configuration and management.
//!
//! Provides both simple static CORS layer constructors for quick setup and a
//! full [`CORSManager`] that mirrors the Go implementation with:
//! - JSON config file persistence (load/save with atomic write)
//! - Origin validation: exact match, localhost detection, CDN subdomain matching
//! - Runtime mutation: add/remove origins, toggle development mode
//! - Thread safety via `parking_lot::RwLock`

use std::path::{Path, PathBuf};

use http::Method;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tower_http::cors::{Any, CorsLayer};
use tracing;

// ---------------------------------------------------------------------------
// CORSConfig
// ---------------------------------------------------------------------------

/// Persistent CORS configuration, serialised as JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CORSConfig {
    /// Origins that are always allowed (exact string match).
    #[serde(default)]
    pub allowed_origins: Vec<String>,

    /// HTTP methods permitted in CORS preflight responses.
    #[serde(default = "default_methods")]
    pub allowed_methods: Vec<String>,

    /// HTTP headers permitted in CORS preflight responses.
    #[serde(default = "default_headers")]
    pub allowed_headers: Vec<String>,

    /// Whether to send `Access-Control-Allow-Credentials: true`.
    #[serde(default = "default_true")]
    pub allow_credentials: bool,

    /// `Access-Control-Max-Age` value in seconds.
    #[serde(default = "default_max_age")]
    pub max_age: u64,

    /// Automatically allow `http://localhost:*` and `http://127.0.0.1:*`.
    #[serde(default = "default_true")]
    pub allow_localhost: bool,

    /// When `true` all origins are accepted (equivalent to `dev_cors_layer`).
    #[serde(default)]
    pub development_mode: bool,

    /// CDN domains whose subdomains are also accepted.
    ///
    /// For example `"cdn.example.com"` matches `"abc.cdn.example.com"` but
    /// **not** `"fake-cdn.example.com.evil.com"`.
    #[serde(default)]
    pub allowed_cdn_domains: Vec<String>,

    /// Allow requests that carry no `Origin` header (curl, mobile apps, etc.).
    #[serde(default = "default_true")]
    pub allow_no_origin: bool,
}

fn default_methods() -> Vec<String> {
    vec!["GET".into(), "POST".into()]
}

fn default_headers() -> Vec<String> {
    vec!["Content-Type".into(), "Authorization".into()]
}

fn default_max_age() -> u64 {
    3600
}

fn default_true() -> bool {
    true
}

impl Default for CORSConfig {
    fn default() -> Self {
        Self {
            allowed_origins: Vec::new(),
            allowed_methods: default_methods(),
            allowed_headers: default_headers(),
            allow_credentials: true,
            max_age: 3600,
            allow_localhost: true,
            development_mode: false,
            allowed_cdn_domains: Vec::new(),
            allow_no_origin: true,
        }
    }
}

// ---------------------------------------------------------------------------
// CORSManager
// ---------------------------------------------------------------------------

/// Manages CORS configuration with file persistence and runtime mutation.
///
/// Thread-safe: all mutable operations acquire a write lock on the internal
/// `RwLock`; read operations acquire a read lock.
pub struct CORSManager {
    config: RwLock<CORSConfig>,
    config_path: PathBuf,
}

impl CORSManager {
    // -----------------------------------------------------------------------
    // Construction
    // -----------------------------------------------------------------------

    /// Create a new `CORSManager`.
    ///
    /// If the config file at `config_path` already exists it is loaded;
    /// otherwise a default config is written to that path.
    pub fn new(config_path: impl Into<PathBuf>) -> std::io::Result<Self> {
        let config_path = config_path.into();
        let config = if config_path.exists() {
            Self::load_from_file(&config_path)?
        } else {
            let cfg = CORSConfig::default();
            Self::save_to_file(&cfg, &config_path)?;
            cfg
        };

        tracing::info!(
            path = %config_path.display(),
            "CORS config loaded"
        );

        Ok(Self {
            config: RwLock::new(config),
            config_path,
        })
    }

    // -----------------------------------------------------------------------
    // Origin validation
    // -----------------------------------------------------------------------

    /// Check whether the given `origin` string is permitted.
    ///
    /// The empty string is treated as "no origin" and is subject to the
    /// `allow_no_origin` flag.
    pub fn check_origin(&self, origin: &str) -> bool {
        let cfg = self.config.read();

        // No Origin header ------------------------------------------------
        if origin.is_empty() {
            return cfg.allow_no_origin;
        }

        // Development mode / localhost ------------------------------------
        if cfg.development_mode || cfg.allow_localhost {
            if origin.starts_with("http://localhost:")
                || origin.starts_with("http://127.0.0.1:")
            {
                return true;
            }
        }

        // Exact match in allowed_origins ----------------------------------
        if cfg.allowed_origins.iter().any(|o| o == origin) {
            return true;
        }

        // CDN subdomain matching ------------------------------------------
        for cdn in &cfg.allowed_cdn_domains {
            if let Ok(parsed) = url::Url::parse(origin) {
                if let Some(host) = parsed.host_str() {
                    // Exact CDN match or proper subdomain (".cdn" prefix).
                    if host == cdn || host.ends_with(&format!(".{cdn}")) {
                        return true;
                    }
                }
            }
        }

        false
    }

    // -----------------------------------------------------------------------
    // Runtime mutation
    // -----------------------------------------------------------------------

    /// Add an origin to the allow-list. No-op if it already exists.
    /// Persists the updated config to disk.
    pub fn add_origin(&self, origin: &str) -> std::io::Result<()> {
        let mut cfg = self.config.write();

        if cfg.allowed_origins.iter().any(|o| o == origin) {
            return Ok(());
        }

        cfg.allowed_origins.push(origin.to_owned());

        tracing::info!(origin = %origin, "CORS: added allowed origin");

        let cloned = cfg.clone();
        drop(cfg); // release write lock before I/O
        Self::save_to_file(&cloned, &self.config_path)
    }

    /// Remove an origin from the allow-list. No-op if it does not exist.
    /// Persists the updated config to disk.
    pub fn remove_origin(&self, origin: &str) -> std::io::Result<()> {
        let mut cfg = self.config.write();

        let before = cfg.allowed_origins.len();
        cfg.allowed_origins.retain(|o| o != origin);

        if cfg.allowed_origins.len() == before {
            return Ok(()); // nothing removed
        }

        tracing::info!(origin = %origin, "CORS: removed allowed origin");

        let cloned = cfg.clone();
        drop(cfg);
        Self::save_to_file(&cloned, &self.config_path)
    }

    /// Return a snapshot of the current allowed origins list.
    pub fn list_origins(&self) -> Vec<String> {
        self.config.read().allowed_origins.clone()
    }

    /// Enable or disable development mode and persist the change.
    pub fn set_development_mode(&self, enabled: bool) -> std::io::Result<()> {
        let mut cfg = self.config.write();
        cfg.development_mode = enabled;

        tracing::info!(enabled = %enabled, "CORS: development mode changed");

        let cloned = cfg.clone();
        drop(cfg);
        Self::save_to_file(&cloned, &self.config_path)
    }

    /// Return a clone of the current configuration.
    pub fn config(&self) -> CORSConfig {
        self.config.read().clone()
    }

    // -----------------------------------------------------------------------
    // File helpers (private)
    // -----------------------------------------------------------------------

    fn load_from_file(path: &Path) -> std::io::Result<CORSConfig> {
        let data = std::fs::read_to_string(path)?;
        let cfg: CORSConfig = serde_json::from_str(&data).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e)
        })?;
        Ok(cfg)
    }

    /// Write config to `path` atomically: write to a `.tmp` file then rename.
    fn save_to_file(cfg: &CORSConfig, path: &Path) -> std::io::Result<()> {
        // Ensure parent directory exists.
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let json = serde_json::to_string_pretty(cfg).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e)
        })?;

        let tmp_path = path.with_extension("json.tmp");
        std::fs::write(&tmp_path, &json)?;

        // Atomic rename (best-effort on Windows; may fail across drives).
        if let Err(e) = std::fs::rename(&tmp_path, path) {
            // Fallback: just remove the temp file and try a direct write.
            let _ = std::fs::remove_file(&tmp_path);
            std::fs::write(path, &json)?;
            tracing::warn!(
                error = %e,
                "Atomic rename failed, fell back to direct write"
            );
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Static CORS layer helpers (preserved from original implementation)
// ---------------------------------------------------------------------------

/// Build a CORS layer for development (allows all origins).
pub fn dev_cors_layer() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers(Any)
}

/// Build a CORS layer for production with specific allowed origins.
pub fn production_cors_layer(allowed_origins: &[String]) -> CorsLayer {
    let origins: Vec<_> = allowed_origins
        .iter()
        .filter_map(|o| o.parse().ok())
        .collect();

    CorsLayer::new()
        .allow_origin(origins)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers(Any)
}

/// Build a CORS layer from a [`CORSManager`], respecting its configuration.
///
/// When `development_mode` is enabled, this is equivalent to [`dev_cors_layer`].
/// Otherwise only the explicitly allowed origins (plus localhost origins when
/// `allow_localhost` is set) are permitted.
pub fn cors_layer_from_manager(mgr: &CORSManager) -> CorsLayer {
    let cfg = mgr.config();

    if cfg.development_mode {
        return dev_cors_layer();
    }

    // Collect all allowed origins.
    let mut origins: Vec<String> = cfg.allowed_origins.clone();

    // When localhost is allowed, add common dev ports so the CorsLayer can
    // respond correctly to preflight requests.
    if cfg.allow_localhost {
        for port in ["3000", "5173", "8080", "8000", "4173"] {
            origins.push(format!("http://localhost:{port}"));
            origins.push(format!("http://127.0.0.1:{port}"));
        }
    }

    production_cors_layer(&origins)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
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
        assert_eq!(mgr.list_origins().iter().filter(|o| **o == "https://foo.com").count(), 1);

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
        assert!(loaded.list_origins().contains(&"https://new.com".to_string()));
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
}
