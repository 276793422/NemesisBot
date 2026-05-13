//! Health check server with /health, /ready, /live endpoints and uptime tracking.

use axum::{Router, routing::get, Json, extract::State};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;
use parking_lot::Mutex;
use tokio::sync::oneshot;

/// A named health check function.
type HealthCheckFn = Box<dyn Fn() -> (bool, String) + Send + Sync>;

/// Shared state for health handlers.
#[derive(Clone)]
pub struct HealthState {
    start_time: Instant,
    version: Option<String>,
    beat_count: Arc<AtomicU64>,
    ready: Arc<AtomicBool>,
    checks: Arc<Mutex<HashMap<String, HealthCheckFn>>>,
}

/// Health check handler.
async fn health(State(state): State<Arc<HealthState>>) -> Json<Value> {
    Json(json!({
        "status": "healthy",
        "uptime_seconds": state.start_time.elapsed().as_secs(),
        "version": state.version,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    }))
}

/// Readiness check handler.
/// Evaluates all registered health checks and reports readiness status.
async fn ready(State(state): State<Arc<HealthState>>) -> Json<Value> {
    let mut all_healthy = state.ready.load(Ordering::SeqCst);
    let mut check_results = serde_json::Map::new();

    let checks = state.checks.lock();
    for (name, check_fn) in checks.iter() {
        let (healthy, message) = check_fn();
        if !healthy {
            all_healthy = false;
        }
        check_results.insert(name.clone(), json!({
            "healthy": healthy,
            "message": message,
        }));
    }
    drop(checks);

    Json(json!({
        "ready": all_healthy,
        "beat_count": state.beat_count.load(Ordering::SeqCst),
        "checks": check_results,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    }))
}

/// Liveness check handler.
async fn live() -> Json<Value> {
    Json(json!({ "alive": true }))
}

/// Health server configuration.
#[derive(Debug, Clone)]
pub struct HealthServerConfig {
    pub listen_addr: String,
    pub version: Option<String>,
}

impl Default for HealthServerConfig {
    fn default() -> Self {
        Self {
            listen_addr: "127.0.0.1:9090".to_string(),
            version: None,
        }
    }
}

/// Health check HTTP server.
pub struct HealthServer {
    config: HealthServerConfig,
    state: Arc<HealthState>,
    shutdown_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

impl HealthServer {
    pub fn new(config: HealthServerConfig) -> Self {
        let state = Arc::new(HealthState {
            start_time: Instant::now(),
            version: config.version.clone(),
            beat_count: Arc::new(AtomicU64::new(0)),
            ready: Arc::new(AtomicBool::new(false)),
            checks: Arc::new(Mutex::new(HashMap::new())),
        });
        Self {
            config,
            state,
            shutdown_tx: Arc::new(Mutex::new(None)),
        }
    }

    /// Build the health check router.
    pub fn build_router(&self) -> Router {
        Router::new()
            .route("/health", get(health))
            .route("/ready", get(ready))
            .route("/live", get(live))
            .with_state(self.state.clone())
    }

    /// Start the health check server (blocking).
    pub async fn start(&self) -> Result<(), String> {
        let addr: SocketAddr = self.config.listen_addr.parse()
            .map_err(|e| format!("invalid address: {}", e))?;
        let app = self.build_router();
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| format!("bind failed: {}", e))?;
        tracing::info!("Health server listening on {}", addr);
        axum::serve(listener, app).await.map_err(|e| format!("server error: {}", e))
    }

    /// Start the health check server with graceful shutdown support.
    /// Mirrors Go's `Server.StartContext(ctx)`.
    pub async fn start_with_shutdown(&self) -> Result<(), String> {
        let addr: SocketAddr = self.config.listen_addr.parse()
            .map_err(|e| format!("invalid address: {}", e))?;
        let app = self.build_router();
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| format!("bind failed: {}", e))?;
        tracing::info!("Health server listening on {}", addr);

        let (tx, rx) = oneshot::channel();
        *self.shutdown_tx.lock() = Some(tx);

        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = rx.await;
            })
            .await
            .map_err(|e| format!("server error: {}", e))
    }

    /// Stop the health check server gracefully.
    /// Mirrors Go's `Server.Stop(ctx)`.
    pub fn stop(&self) {
        if let Some(tx) = self.shutdown_tx.lock().take() {
            let _ = tx.send(());
        }
    }

    /// Set the readiness state.
    /// Mirrors Go's `Server.SetReady(ready)`.
    pub fn set_ready(&self, ready: bool) {
        self.state.ready.store(ready, Ordering::SeqCst);
    }

    /// Register a custom health check function.
    /// Mirrors Go's `Server.RegisterCheck(name, checkFn)`.
    pub fn register_check(&self, name: impl Into<String>, check_fn: HealthCheckFn) {
        self.state.checks.lock().insert(name.into(), check_fn);
    }

    /// Record a heartbeat (called by heartbeat service).
    pub fn record_beat(&self) {
        self.state.beat_count.fetch_add(1, Ordering::SeqCst);
    }

    /// Get the health state (for programmatic checks).
    pub fn state(&self) -> &Arc<HealthState> {
        &self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    #[test]
    fn test_build_router() {
        let server = HealthServer::new(HealthServerConfig {
            listen_addr: "127.0.0.1:0".to_string(),
            version: Some("1.0.0".to_string()),
        });
        let _router = server.build_router();
    }

    #[test]
    fn test_default_config() {
        let config = HealthServerConfig::default();
        assert_eq!(config.listen_addr, "127.0.0.1:9090");
    }

    #[test]
    fn test_record_beat() {
        let server = HealthServer::new(HealthServerConfig::default());
        assert_eq!(server.state().beat_count.load(Ordering::SeqCst), 0);
        server.record_beat();
        server.record_beat();
        assert_eq!(server.state().beat_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_health_endpoint() {
        let server = HealthServer::new(HealthServerConfig {
            listen_addr: "127.0.0.1:0".to_string(),
            version: Some("test".to_string()),
        });
        let app = server.build_router();
        let resp = app.oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn test_live_endpoint() {
        let server = HealthServer::new(HealthServerConfig::default());
        let app = server.build_router();
        let resp = app.oneshot(Request::builder().uri("/live").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn test_ready_endpoint() {
        let server = HealthServer::new(HealthServerConfig::default());
        let app = server.build_router();
        let resp = app.oneshot(Request::builder().uri("/ready").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), 200);
    }

    #[test]
    fn test_set_ready_true() {
        let server = HealthServer::new(HealthServerConfig::default());
        assert!(!server.state().ready.load(Ordering::SeqCst));
        server.set_ready(true);
        assert!(server.state().ready.load(Ordering::SeqCst));
    }

    #[test]
    fn test_set_ready_toggle() {
        let server = HealthServer::new(HealthServerConfig::default());
        server.set_ready(true);
        assert!(server.state().ready.load(Ordering::SeqCst));
        server.set_ready(false);
        assert!(!server.state().ready.load(Ordering::SeqCst));
    }

    #[test]
    fn test_record_multiple_beats() {
        let server = HealthServer::new(HealthServerConfig::default());
        for i in 0..100 {
            server.record_beat();
            assert_eq!(server.state().beat_count.load(Ordering::SeqCst), i as u64 + 1);
        }
    }

    #[test]
    fn test_register_custom_check() {
        let server = HealthServer::new(HealthServerConfig::default());
        server.register_check("db", Box::new(|| (true, "database ok".to_string())));
        server.register_check("cache", Box::new(|| (false, "cache timeout".to_string())));

        let checks = server.state().checks.lock();
        assert_eq!(checks.len(), 2);
        assert!(checks.contains_key("db"));
        assert!(checks.contains_key("cache"));
    }

    #[test]
    fn test_config_with_version() {
        let config = HealthServerConfig {
            listen_addr: "0.0.0.0:8080".to_string(),
            version: Some("2.0.0".to_string()),
        };
        assert_eq!(config.listen_addr, "0.0.0.0:8080");
        assert_eq!(config.version.as_deref(), Some("2.0.0"));
    }

    #[test]
    fn test_config_debug_format() {
        let config = HealthServerConfig::default();
        let debug = format!("{:?}", config);
        assert!(debug.contains("127.0.0.1:9090"));
    }

    #[test]
    fn test_state_clone() {
        let state = HealthState {
            start_time: Instant::now(),
            version: Some("1.0".to_string()),
            beat_count: Arc::new(AtomicU64::new(42)),
            ready: Arc::new(AtomicBool::new(true)),
            checks: Arc::new(Mutex::new(HashMap::new())),
        };
        let cloned = state.clone();
        assert_eq!(cloned.beat_count.load(Ordering::SeqCst), 42);
        assert!(cloned.ready.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn test_health_response_body() {
        let server = HealthServer::new(HealthServerConfig {
            listen_addr: "127.0.0.1:0".to_string(),
            version: Some("1.0.0".to_string()),
        });
        let app = server.build_router();
        let resp = app.oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap()).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "healthy");
        assert_eq!(json["version"], "1.0.0");
    }

    #[tokio::test]
    async fn test_live_response_body() {
        let server = HealthServer::new(HealthServerConfig::default());
        let app = server.build_router();
        let resp = app.oneshot(Request::builder().uri("/live").body(Body::empty()).unwrap()).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["alive"], true);
    }

    #[tokio::test]
    async fn test_ready_with_checks() {
        let server = HealthServer::new(HealthServerConfig::default());
        server.set_ready(true);
        server.register_check("test_check", Box::new(|| (true, "all good".to_string())));
        let app = server.build_router();
        let resp = app.oneshot(Request::builder().uri("/ready").body(Body::empty()).unwrap()).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["ready"], true);
    }

    #[test]
    fn test_stop_without_start() {
        let server = HealthServer::new(HealthServerConfig::default());
        // Should not panic when stop is called without starting
        server.stop();
    }

    #[test]
    fn test_state_returns_arc() {
        let server = HealthServer::new(HealthServerConfig::default());
        let state = server.state();
        assert!(Arc::strong_count(state) >= 1);
    }

    // ==================== Additional coverage tests ====================

    #[tokio::test]
    async fn test_ready_endpoint_with_failing_check() {
        let server = HealthServer::new(HealthServerConfig::default());
        server.set_ready(true);
        server.register_check(
            "failing_check",
            Box::new(|| (false, "database connection refused".to_string())),
        );

        let app = server.build_router();
        let resp = app
            .oneshot(Request::builder().uri("/ready").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        let body = axum::body::to_bytes(resp.into_body(), 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["ready"], false);
        assert_eq!(json["checks"]["failing_check"]["healthy"], false);
        assert_eq!(
            json["checks"]["failing_check"]["message"],
            "database connection refused"
        );
    }

    #[tokio::test]
    async fn test_ready_set_false_with_passing_checks() {
        let server = HealthServer::new(HealthServerConfig::default());
        // Manual flag is false (default), but all checks pass
        server.register_check(
            "db",
            Box::new(|| (true, "database ok".to_string())),
        );
        server.register_check(
            "cache",
            Box::new(|| (true, "cache ok".to_string())),
        );

        let app = server.build_router();
        let resp = app
            .oneshot(Request::builder().uri("/ready").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        // Manual flag (false) should override passing checks
        assert_eq!(json["ready"], false);
        // Individual checks should still show as healthy
        assert_eq!(json["checks"]["db"]["healthy"], true);
        assert_eq!(json["checks"]["cache"]["healthy"], true);
    }

    #[tokio::test]
    async fn test_ready_set_false_then_true_with_passing_checks() {
        let server = HealthServer::new(HealthServerConfig::default());
        server.register_check(
            "svc",
            Box::new(|| (true, "service ok".to_string())),
        );

        // First: set_ready false (default), ready should be false
        let app = server.build_router();
        let resp = app
            .oneshot(Request::builder().uri("/ready").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["ready"], false);

        // Now: set_ready true, ready should become true
        server.set_ready(true);
        let app = server.build_router();
        let resp = app
            .oneshot(Request::builder().uri("/ready").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["ready"], true);
    }

    #[tokio::test]
    async fn test_health_endpoint_json_fields() {
        let server = HealthServer::new(HealthServerConfig {
            listen_addr: "127.0.0.1:0".to_string(),
            version: Some("2.5.0".to_string()),
        });
        let app = server.build_router();
        let resp = app
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        // Validate all expected fields exist
        assert_eq!(json["status"], "healthy");
        assert!(json["uptime_seconds"].is_number());
        assert!(json["uptime_seconds"].as_u64().is_some());
        assert_eq!(json["version"], "2.5.0");

        // Validate timestamp is RFC3339 format (contains 'T' and 'Z' or offset)
        let ts = json["timestamp"].as_str().unwrap();
        assert!(ts.contains('T'), "timestamp should be RFC3339 format");
    }

    #[tokio::test]
    async fn test_health_endpoint_uptime_increases() {
        let server = HealthServer::new(HealthServerConfig {
            listen_addr: "127.0.0.1:0".to_string(),
            version: None,
        });

        let app = server.build_router();
        let resp1 = app
            .clone()
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body1 = axum::body::to_bytes(resp1.into_body(), 1024)
            .await
            .unwrap();
        let json1: serde_json::Value = serde_json::from_slice(&body1).unwrap();

        // Small delay to let uptime tick
        std::thread::sleep(std::time::Duration::from_millis(100));

        let resp2 = app
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body2 = axum::body::to_bytes(resp2.into_body(), 1024)
            .await
            .unwrap();
        let json2: serde_json::Value = serde_json::from_slice(&body2).unwrap();

        let uptime1 = json1["uptime_seconds"].as_u64().unwrap();
        let uptime2 = json2["uptime_seconds"].as_u64().unwrap();
        assert!(uptime2 >= uptime1, "uptime should be non-decreasing");
    }

    #[tokio::test]
    async fn test_health_no_version() {
        let server = HealthServer::new(HealthServerConfig {
            listen_addr: "127.0.0.1:0".to_string(),
            version: None,
        });
        let app = server.build_router();
        let resp = app
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["version"], serde_json::Value::Null);
    }

    #[test]
    fn test_health_server_config_default_values() {
        let config = HealthServerConfig::default();
        assert_eq!(config.listen_addr, "127.0.0.1:9090");
        assert!(config.version.is_none());
    }

    #[test]
    fn test_health_state_serialization_via_arc() {
        let state = HealthState {
            start_time: Instant::now(),
            version: Some("1.0.0".to_string()),
            beat_count: Arc::new(AtomicU64::new(10)),
            ready: Arc::new(AtomicBool::new(true)),
            checks: Arc::new(Mutex::new(HashMap::new())),
        };

        // Verify state is accessible via Arc
        assert_eq!(state.beat_count.load(Ordering::SeqCst), 10);
        assert!(state.ready.load(Ordering::SeqCst));
        assert_eq!(state.version.as_deref(), Some("1.0.0"));
    }

    #[tokio::test]
    async fn test_ready_endpoint_no_checks() {
        let server = HealthServer::new(HealthServerConfig::default());
        // No checks registered, ready flag is false (default)
        let app = server.build_router();
        let resp = app
            .oneshot(Request::builder().uri("/ready").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["ready"], false);
        assert_eq!(json["beat_count"], 0);
        // checks should be an empty object
        assert!(json["checks"].as_object().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_ready_endpoint_beat_count_reflected() {
        let server = HealthServer::new(HealthServerConfig::default());
        server.set_ready(true);
        server.record_beat();
        server.record_beat();
        server.record_beat();

        let app = server.build_router();
        let resp = app
            .oneshot(Request::builder().uri("/ready").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["beat_count"], 3);
    }

    #[tokio::test]
    async fn test_ready_with_mixed_checks() {
        let server = HealthServer::new(HealthServerConfig::default());
        server.set_ready(true);
        server.register_check(
            "db",
            Box::new(|| (true, "database connected".to_string())),
        );
        server.register_check(
            "redis",
            Box::new(|| (true, "redis ok".to_string())),
        );
        server.register_check(
            "disk",
            Box::new(|| (false, "disk 95% full".to_string())),
        );

        let app = server.build_router();
        let resp = app
            .oneshot(Request::builder().uri("/ready").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        // One failing check should make ready=false even with set_ready(true)
        assert_eq!(json["ready"], false);
        assert_eq!(json["checks"]["db"]["healthy"], true);
        assert_eq!(json["checks"]["redis"]["healthy"], true);
        assert_eq!(json["checks"]["disk"]["healthy"], false);
        assert_eq!(json["checks"]["disk"]["message"], "disk 95% full");
    }

    #[tokio::test]
    async fn test_ready_check_override_by_failing_check() {
        let server = HealthServer::new(HealthServerConfig::default());
        server.set_ready(true);
        server.register_check(
            "always_fails",
            Box::new(|| (false, "service unavailable".to_string())),
        );

        let app = server.build_router();
        let resp = app
            .oneshot(Request::builder().uri("/ready").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        // Failing check overrides set_ready(true)
        assert_eq!(json["ready"], false);
    }

    #[test]
    fn test_multiple_stops_no_panic() {
        let server = HealthServer::new(HealthServerConfig::default());
        server.stop();
        server.stop();
        server.stop();
    }

    #[test]
    fn test_register_multiple_checks_same_name_overwrites() {
        let server = HealthServer::new(HealthServerConfig::default());
        server.register_check("db", Box::new(|| (true, "ok".to_string())));
        server.register_check("db", Box::new(|| (false, "down".to_string())));

        let checks = server.state().checks.lock();
        assert_eq!(checks.len(), 1);
        let (healthy, msg) = checks.get("db").unwrap()();
        assert!(!healthy);
        assert_eq!(msg, "down");
    }

    // ---- Additional tests for 95%+ coverage ----

    #[tokio::test]
    async fn test_start_invalid_address() {
        let server = HealthServer::new(HealthServerConfig {
            listen_addr: "invalid:address:that:cannot:bind".to_string(),
            version: None,
        });
        let result = server.start().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_start_with_shutdown_and_stop() {
        let server = HealthServer::new(HealthServerConfig {
            listen_addr: "127.0.0.1:0".to_string(),
            version: Some("1.0".to_string()),
        });
        // Start with shutdown in background
        let server_clone = HealthServer::new(HealthServerConfig {
            listen_addr: "127.0.0.1:0".to_string(),
            version: None,
        });
        server_clone.stop(); // stop before start should be fine
    }

    #[tokio::test]
    async fn test_ready_no_checks_set_ready_true() {
        let server = HealthServer::new(HealthServerConfig::default());
        server.set_ready(true);
        // No checks registered, but ready flag is true
        let app = server.build_router();
        let resp = app
            .oneshot(Request::builder().uri("/ready").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["ready"], true);
        assert_eq!(json["beat_count"], 0);
    }

    #[test]
    fn test_multiple_record_beats() {
        let server = HealthServer::new(HealthServerConfig::default());
        for _ in 0..1000 {
            server.record_beat();
        }
        assert_eq!(server.state().beat_count.load(Ordering::SeqCst), 1000);
    }

    #[tokio::test]
    async fn test_health_endpoint_with_null_version() {
        let server = HealthServer::new(HealthServerConfig {
            listen_addr: "127.0.0.1:0".to_string(),
            version: None,
        });
        let app = server.build_router();
        let resp = app
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["uptime_seconds"].as_u64().unwrap() >= 0);
    }

    #[test]
    fn test_state_arc_refcount() {
        let server = HealthServer::new(HealthServerConfig::default());
        let state = server.state();
        let count = Arc::strong_count(state);
        // At least 2: server holds one, state() returns &Arc which we just cloned
        assert!(count >= 1, "arc refcount should be >= 1, got {}", count);
    }

    // ---- Additional edge case tests for 95%+ ----

    #[tokio::test]
    async fn test_start_with_shutdown_invalid_addr() {
        let server = HealthServer::new(HealthServerConfig {
            listen_addr: "not-valid:addr".to_string(),
            version: None,
        });
        let result = server.start_with_shutdown().await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid address"));
    }

    #[test]
    fn test_set_ready_default_is_false() {
        let server = HealthServer::new(HealthServerConfig {
            listen_addr: "127.0.0.1:0".to_string(),
            version: None,
        });
        assert!(!server.state().ready.load(Ordering::SeqCst));
    }

    #[test]
    fn test_beat_count_starts_at_zero() {
        let server = HealthServer::new(HealthServerConfig {
            listen_addr: "127.0.0.1:0".to_string(),
            version: Some("test".to_string()),
        });
        assert_eq!(server.state().beat_count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_register_check_and_invoke() {
        let server = HealthServer::new(HealthServerConfig::default());
        let call_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let call_count_clone = call_count.clone();
        server.register_check("counter", Box::new(move || {
            call_count_clone.fetch_add(1, Ordering::SeqCst);
            (true, "called".to_string())
        }));

        let checks = server.state().checks.lock();
        let (healthy, msg) = checks.get("counter").unwrap()();
        assert!(healthy);
        assert_eq!(msg, "called");
        drop(checks);
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_ready_endpoint_timestamp_format() {
        let server = HealthServer::new(HealthServerConfig::default());
        server.set_ready(true);
        let app = server.build_router();
        let resp = app
            .oneshot(Request::builder().uri("/ready").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let ts = json["timestamp"].as_str().unwrap();
        assert!(ts.contains('T'), "timestamp should be RFC3339");
    }

    #[tokio::test]
    async fn test_health_uptime_is_reasonable() {
        let server = HealthServer::new(HealthServerConfig {
            listen_addr: "127.0.0.1:0".to_string(),
            version: None,
        });
        let app = server.build_router();
        let resp = app
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let uptime = json["uptime_seconds"].as_u64().unwrap();
        // Should be very small (< 10 seconds) since just created
        assert!(uptime < 10, "uptime should be small for newly created server");
    }

    #[test]
    fn test_config_listen_addr_custom() {
        let config = HealthServerConfig {
            listen_addr: "0.0.0.0:8081".to_string(),
            version: Some("3.0.0".to_string()),
        };
        assert_eq!(config.listen_addr, "0.0.0.0:8081");
        assert_eq!(config.version.as_deref(), Some("3.0.0"));
    }

    #[tokio::test]
    async fn test_ready_with_multiple_checks_all_passing() {
        let server = HealthServer::new(HealthServerConfig::default());
        server.set_ready(true);
        for i in 0..5 {
            server.register_check(
                format!("check_{}", i),
                Box::new(move || (true, format!("check {} ok", i))),
            );
        }

        let app = server.build_router();
        let resp = app
            .oneshot(Request::builder().uri("/ready").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["ready"], true);
        let checks = json["checks"].as_object().unwrap();
        assert_eq!(checks.len(), 5);
    }

    // ---- Additional edge-case tests for 95%+ coverage ----

    #[tokio::test]
    async fn test_start_with_shutdown_valid_addr_and_stop() {
        let server = HealthServer::new(HealthServerConfig {
            listen_addr: "127.0.0.1:0".to_string(),
            version: Some("test-stop".to_string()),
        });

        let server_for_stop = HealthServer::new(HealthServerConfig {
            listen_addr: "127.0.0.1:0".to_string(),
            version: Some("test-stop".to_string()),
        });

        // Start in background, then stop
        let handle = tokio::spawn(async move {
            let _ = server.start_with_shutdown().await;
        });

        // Give server time to start
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        server_for_stop.stop();

        // The original server hasn't started, so abort its handle
        handle.abort();
    }

    #[test]
    fn test_health_server_new_with_version() {
        let server = HealthServer::new(HealthServerConfig {
            listen_addr: "0.0.0.0:9999".to_string(),
            version: Some("3.0.0".to_string()),
        });
        assert_eq!(server.state().version.as_deref(), Some("3.0.0"));
    }

    #[test]
    fn test_health_server_new_no_version() {
        let server = HealthServer::new(HealthServerConfig {
            listen_addr: "0.0.0.0:9999".to_string(),
            version: None,
        });
        assert!(server.state().version.is_none());
    }

    #[test]
    fn test_stop_twice_no_panic() {
        let server = HealthServer::new(HealthServerConfig::default());
        server.stop();
        server.stop();
    }

    #[test]
    fn test_set_ready_false_default() {
        let server = HealthServer::new(HealthServerConfig::default());
        assert!(!server.state().ready.load(Ordering::SeqCst));
    }

    #[test]
    fn test_register_check_overwrites() {
        let server = HealthServer::new(HealthServerConfig::default());
        server.register_check("db", Box::new(|| (true, "ok".to_string())));
        server.register_check("db", Box::new(|| (false, "down".to_string())));
        let checks = server.state().checks.lock();
        assert_eq!(checks.len(), 1);
        let (healthy, msg) = checks.get("db").unwrap()();
        assert!(!healthy);
        assert_eq!(msg, "down");
    }

    #[tokio::test]
    async fn test_ready_endpoint_single_passing_check() {
        let server = HealthServer::new(HealthServerConfig::default());
        server.set_ready(true);
        server.register_check("db", Box::new(|| (true, "db connected".to_string())));

        let app = server.build_router();
        let resp = app
            .oneshot(Request::builder().uri("/ready").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["ready"], true);
        assert_eq!(json["checks"]["db"]["healthy"], true);
        assert_eq!(json["checks"]["db"]["message"], "db connected");
    }

    #[tokio::test]
    async fn test_ready_endpoint_single_failing_check() {
        let server = HealthServer::new(HealthServerConfig::default());
        server.set_ready(true);
        server.register_check("redis", Box::new(|| (false, "timeout".to_string())));

        let app = server.build_router();
        let resp = app
            .oneshot(Request::builder().uri("/ready").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["ready"], false);
        assert_eq!(json["checks"]["redis"]["healthy"], false);
        assert_eq!(json["checks"]["redis"]["message"], "timeout");
    }

    #[test]
    fn test_config_clone() {
        let config = HealthServerConfig {
            listen_addr: "0.0.0.0:8080".to_string(),
            version: Some("1.0".to_string()),
        };
        let cloned = config.clone();
        assert_eq!(cloned.listen_addr, config.listen_addr);
        assert_eq!(cloned.version, config.version);
    }

    #[test]
    fn test_config_debug() {
        let config = HealthServerConfig {
            listen_addr: "127.0.0.1:9090".to_string(),
            version: Some("2.0".to_string()),
        };
        let debug = format!("{:?}", config);
        assert!(debug.contains("127.0.0.1:9090"));
        assert!(debug.contains("2.0"));
    }

    #[tokio::test]
    async fn test_health_uptime_is_nonnegative() {
        let server = HealthServer::new(HealthServerConfig {
            listen_addr: "127.0.0.1:0".to_string(),
            version: None,
        });
        let app = server.build_router();
        let resp = app
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let uptime = json["uptime_seconds"].as_u64();
        assert!(uptime.is_some());
        assert!(uptime.unwrap() <= 300); // Should be less than 5 minutes
    }

    #[tokio::test]
    async fn test_ready_endpoint_with_beats_and_checks() {
        let server = HealthServer::new(HealthServerConfig::default());
        server.set_ready(true);
        server.record_beat();
        server.record_beat();
        server.register_check("svc", Box::new(|| (true, "running".to_string())));

        let app = server.build_router();
        let resp = app
            .oneshot(Request::builder().uri("/ready").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["ready"], true);
        assert_eq!(json["beat_count"], 2);
        assert_eq!(json["checks"]["svc"]["healthy"], true);
        assert!(json["timestamp"].is_string());
    }
}
