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
            "[WebServer] CORS config loaded"
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

        tracing::info!(origin = %origin, "[WebServer] CORS: added allowed origin");

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

        tracing::info!(origin = %origin, "[WebServer] CORS: removed allowed origin");

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

        tracing::info!(enabled = %enabled, "[WebServer] CORS: development mode changed");

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
                "[WebServer] Atomic rename failed, fell back to direct write"
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
mod tests;
