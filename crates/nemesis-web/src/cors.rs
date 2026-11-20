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

// CORSConfig

/// Persistent CORS configuration, serialised as JSON.
///
/// **接线状态注记（BUG-20261121-004）**：九键中只有
/// `allowed_origins` / `allow_localhost` / `development_mode` /
/// `allowed_cdn_domains` 参与真实 CORS 层判定（`origin_allowed`）；
/// `allow_no_origin` 只作用于咨询面（`check_origin`——装配层对无
/// `Origin` 头的请求本不经 CORS 判定，语义自然成立）。
/// `allowed_methods` / `allowed_headers` / `allow_credentials` /
/// `max_age` 四键**当前未被任何 CorsLayer 消费**——接线它们会把
/// methods/headers 从现硬编码形态（GET/POST/PUT/DELETE/OPTIONS +
/// Any headers）收窄到配置值（默认 GET,POST + Content-Type,
/// Authorization），属破坏性行为变更，独立决策项；四键序列化照常
/// 持盘（不丢用户配置），消费是后续增强。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CORSConfig {
    /// Origins that are always allowed (exact string match).
    #[serde(default)]
    pub allowed_origins: Vec<String>,

    /// HTTP methods permitted in CORS preflight responses.
    /// **未接线**：真实层硬编码 GET/POST/PUT/DELETE/OPTIONS（见结构体
    /// 注记；接线 = 行为收窄，独立决策项）。
    #[serde(default = "default_methods")]
    pub allowed_methods: Vec<String>,

    /// HTTP headers permitted in CORS preflight responses.
    /// **未接线**：真实层为 Any（同上）。
    #[serde(default = "default_headers")]
    pub allowed_headers: Vec<String>,

    /// Whether to send `Access-Control-Allow-Credentials: true`.
    /// **未接线**：真实层不发该头（接线属行为放宽，独立决策项）。
    #[serde(default = "default_true")]
    pub allow_credentials: bool,

    /// `Access-Control-Max-Age` value in seconds. **未接线**：真实层不发
    /// 该头。
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

// CORSManager

/// Manages CORS configuration with file persistence and runtime mutation.
///
/// Thread-safe: all mutable operations acquire a write lock on the internal
/// `RwLock`; read operations acquire a read lock.
pub struct CORSManager {
    config: RwLock<CORSConfig>,
    config_path: PathBuf,
}

impl CORSManager {
    // Construction

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

    // Origin validation

    /// Check whether the given `origin` string is permitted.
    ///
    /// The empty string is treated as "no origin" and is subject to the
    /// `allow_no_origin` flag.
    pub fn check_origin(&self, origin: &str) -> bool {
        let cfg = self.config.read();
        origin_allowed(&cfg, origin)
    }

    // Runtime mutation

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

    // File helpers (private)

    fn load_from_file(path: &Path) -> std::io::Result<CORSConfig> {
        // read_config_string 剥 BOM（裸读时记事本 BOM → parse 失败 →
        // CORSManager::new Err 上浮 gateway 启动失败，BUG-20261103-334 同族）。
        let data = nemesis_utils::fsutil::read_config_string(path)?;
        let cfg: CORSConfig = serde_json::from_str(&data)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        Ok(cfg)
    }

    /// Write config to `path` atomically — 委托统一
    /// helper `nemesis_utils::write_file_atomic`（唯一临时名 + sync_all +
    /// 失败清理），替换旧的固定 `.json.tmp` + 失败回退裸写实现（回退会
    /// 把原子性悄悄降级为 truncate 写，已废弃）。
    fn save_to_file(cfg: &CORSConfig, path: &Path) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(cfg)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        nemesis_utils::write_file_atomic(&path.to_string_lossy(), json.as_bytes(), 0o600)
            .map_err(std::io::Error::other)
    }
}

// Static CORS layer helpers (preserved from original implementation)

/// Origin 判定单一真相源（BUG-20261121-004）：`CORSManager::check_origin`
/// 与真实 CorsLayer 装配（`cors_layer_from_manager` 的 predicate）共用，
/// 消除「咨询面说 ALLOWED、浏览器实际被拒」的判定分裂。
///
/// 判定顺序：空 origin（`allow_no_origin`）→ development_mode/allow_localhost
/// 的 localhost 任意端口 → 显式 origins 精确匹配 → CDN 域精确/子域
/// （`Url` 提取 host，杜绝 `evil.com` 伪装后缀）。
pub fn origin_allowed(cfg: &CORSConfig, origin: &str) -> bool {
    // No Origin header ------------------------------------------------
    if origin.is_empty() {
        return cfg.allow_no_origin;
    }

    // Development mode / localhost ------------------------------------
    if (cfg.development_mode || cfg.allow_localhost)
        && (origin.starts_with("http://localhost:") || origin.starts_with("http://127.0.0.1:"))
    {
        return true;
    }

    // Exact match in allowed_origins ----------------------------------
    if cfg.allowed_origins.iter().any(|o| o == origin) {
        return true;
    }

    // CDN subdomain matching ------------------------------------------
    for cdn in &cfg.allowed_cdn_domains {
        if let Ok(parsed) = url::Url::parse(origin)
            && let Some(host) = parsed.host_str()
        {
            // Exact CDN match or proper subdomain (".cdn" prefix).
            if host == cdn || host.ends_with(&format!(".{cdn}")) {
                return true;
            }
        }
    }

    false
}

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

/// Build a restrictive CORS layer: NO cross-origin access at all.
///
/// A bare [`CorsLayer`] emits no `Access-Control-Allow-Origin` header, so
/// browsers deny every cross-origin read; same-origin requests (the served
/// dashboard, standalone pages, share view) are unaffected — CORS is a
/// browser-enforced contract they never enter.
///
/// This is the default when no CORS config exists and `cors_allow_all` is
/// off (hardening): the previous mapping "empty origin list →
/// allow-all" silently turned a missing config file into `*`.
pub fn restrictive_cors_layer() -> CorsLayer {
    CorsLayer::new()
}

/// Build a CORS layer from a [`CORSManager`], respecting its configuration.
///
/// When `development_mode` is enabled, this is equivalent to [`dev_cors_layer`].
/// Otherwise origin judgment goes through [`origin_allowed`] — the same
/// single source of truth as [`CORSManager::check_origin`] — so explicit
/// origins, `allow_localhost` (any port) and CDN subdomains all apply to the
/// real wire behavior (BUG-20261121-004: the previous implementation folded
/// localhost into a fixed five-port enumeration and silently dropped CDN
/// domains entirely, leaving those two config keys dead at the assembly
/// layer).
///
/// `allowed_methods` / `allowed_headers` / `allow_credentials` / `max_age`
/// stay hard-wired (see [`CORSConfig`] wiring notes) — wiring them would
/// narrow the current header surface and is a separate decision.
pub fn cors_layer_from_manager(mgr: &CORSManager) -> CorsLayer {
    let cfg = mgr.config();

    if cfg.development_mode {
        return dev_cors_layer();
    }

    CorsLayer::new()
        .allow_origin(tower_http::cors::AllowOrigin::predicate(
            move |origin, _parts| {
                origin
                    .to_str()
                    .map(|o| origin_allowed(&cfg, o))
                    .unwrap_or(false)
            },
        ))
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers(Any)
}

// Tests

#[cfg(test)]
mod tests;

// (, quality-hardening goal 冲刺 web): error arms —
// invalid JSON load, create_dir_all failure, read-only rename fallback,
// CDN Url::parse failure.
#[cfg(test)]
mod s10b_tests;

// AGT 覆盖率批次：CORSManager::new 的 tracing::info! 字段
// 求值（无 subscriber 时宏短路）。豁免（收括号 lcov 伪零）见 agt_tests
// 头注。
#[cfg(test)]
mod agt_tests;
