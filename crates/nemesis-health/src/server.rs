//! Health check server with /health, /ready, /live endpoints and uptime tracking.

use axum::{Json, Router, extract::State, routing::get};
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;
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
        "timestamp": chrono::Local::now().to_rfc3339(),
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
        check_results.insert(
            name.clone(),
            json!({
                "healthy": healthy,
                "message": message,
            }),
        );
    }
    drop(checks);

    Json(json!({
        "ready": all_healthy,
        "beat_count": state.beat_count.load(Ordering::SeqCst),
        "checks": check_results,
        "timestamp": chrono::Local::now().to_rfc3339(),
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
        let addr: SocketAddr = self
            .config
            .listen_addr
            .parse()
            .map_err(|e| format!("invalid address: {}", e))?;
        let app = self.build_router();
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| format!("bind failed: {}", e))?;
        tracing::info!("[Health] Server listening on {}", addr);
        axum::serve(listener, app)
            .await
            .map_err(|e| format!("server error: {}", e))
    }

    /// Start the health check server with graceful shutdown support.
    /// Mirrors Go's `Server.StartContext(ctx)`.
    pub async fn start_with_shutdown(&self) -> Result<(), String> {
        let addr: SocketAddr = self
            .config
            .listen_addr
            .parse()
            .map_err(|e| format!("invalid address: {}", e))?;
        let app = self.build_router();
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| format!("bind failed: {}", e))?;
        tracing::info!("[Health] Server listening on {}", addr);

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
mod tests;
