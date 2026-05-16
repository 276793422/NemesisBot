//! Service adapters that bridge async implementations to sync LifecycleService traits.
//!
//! These adapters wrap concrete service instances from individual crates
//! (nemesis-health, nemesis-heartbeat, nemesis-channels) and implement
//! the sync `LifecycleService`-based traits defined in nemesis-services.
//!
//! The async `start()` methods are spawned as background tokio tasks.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use nemesis_services::{
    LifecycleService, HealthServer as HealthServerTrait,
    HeartbeatService as HeartbeatServiceTrait,
    ChannelManager as ChannelManagerTrait,
};

// ---------------------------------------------------------------------------
// HealthServer adapter
// ---------------------------------------------------------------------------

/// Adapter wrapping `nemesis_health::HealthServer` to implement the
/// `nemesis_services::HealthServer` trait.
pub struct HealthServerAdapter {
    inner: Arc<nemesis_health::server::HealthServer>,
    started: AtomicBool,
}

impl HealthServerAdapter {
    pub fn new(inner: Arc<nemesis_health::server::HealthServer>) -> Self {
        Self {
            inner,
            started: AtomicBool::new(false),
        }
    }
}

impl LifecycleService for HealthServerAdapter {
    fn start(&self) -> Result<(), String> {
        if self.started.swap(true, Ordering::SeqCst) {
            return Ok(()); // Already started
        }
        let inner = self.inner.clone();
        tokio::spawn(async move {
            if let Err(e) = inner.start().await {
                tracing::error!("Health server error: {}", e);
            }
        });
        Ok(())
    }

    fn stop(&self) -> Result<(), String> {
        // Health server stops when the process exits
        self.started.store(false, Ordering::SeqCst);
        Ok(())
    }
}

impl HealthServerTrait for HealthServerAdapter {}

// ---------------------------------------------------------------------------
// HeartbeatService adapter
// ---------------------------------------------------------------------------

/// Adapter wrapping `nemesis_heartbeat::HeartbeatService` to implement the
/// `nemesis_services::HeartbeatService` trait.
pub struct HeartbeatServiceAdapter {
    inner: Arc<nemesis_heartbeat::service::HeartbeatService>,
    started: AtomicBool,
}

impl HeartbeatServiceAdapter {
    pub fn new(inner: Arc<nemesis_heartbeat::service::HeartbeatService>) -> Self {
        Self {
            inner,
            started: AtomicBool::new(false),
        }
    }
}

impl LifecycleService for HeartbeatServiceAdapter {
    fn start(&self) -> Result<(), String> {
        if self.started.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let inner = self.inner.clone();
        tokio::spawn(async move {
            if let Err(e) = inner.start().await {
                tracing::error!("Heartbeat service error: {}", e);
            }
        });
        Ok(())
    }

    fn stop(&self) -> Result<(), String> {
        self.inner.stop();
        self.started.store(false, Ordering::SeqCst);
        Ok(())
    }
}

impl HeartbeatServiceTrait for HeartbeatServiceAdapter {}

// ---------------------------------------------------------------------------
// ChannelManager adapter
// ---------------------------------------------------------------------------

/// Adapter wrapping `nemesis_channels::ChannelManager` to implement the
/// `nemesis_services::ChannelManager` trait.
#[allow(dead_code)]
pub struct ChannelManagerAdapter {
    inner: Arc<nemesis_channels::manager::ChannelManager>,
    enabled_channels: Vec<String>,
    started: AtomicBool,
}

impl ChannelManagerAdapter {
    #[allow(dead_code)]
    pub fn new(
        inner: Arc<nemesis_channels::manager::ChannelManager>,
        enabled_channels: Vec<String>,
    ) -> Self {
        Self {
            inner,
            enabled_channels,
            started: AtomicBool::new(false),
        }
    }
}

impl LifecycleService for ChannelManagerAdapter {
    fn start(&self) -> Result<(), String> {
        if self.started.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let inner = self.inner.clone();
        tokio::spawn(async move {
            if let Err(e) = inner.start_all().await {
                tracing::error!("Channel manager start error: {}", e);
            }
        });
        Ok(())
    }

    fn stop(&self) -> Result<(), String> {
        let inner = self.inner.clone();
        tokio::spawn(async move {
            if let Err(e) = inner.stop_all().await {
                tracing::error!("Channel manager stop error: {}", e);
            }
        });
        self.started.store(false, Ordering::SeqCst);
        Ok(())
    }
}

impl ChannelManagerTrait for ChannelManagerAdapter {
    fn enabled_channels(&self) -> Vec<String> {
        self.enabled_channels.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_health_config(port: u16) -> nemesis_health::server::HealthServerConfig {
        nemesis_health::server::HealthServerConfig {
            listen_addr: format!("127.0.0.1:{}", port),
            version: Some("test".to_string()),
        }
    }

    fn make_heartbeat_config() -> nemesis_heartbeat::HeartbeatConfig {
        nemesis_heartbeat::HeartbeatConfig::new(30, true, std::env::temp_dir().to_string_lossy().to_string())
    }

    // -------------------------------------------------------------------------
    // HealthServerAdapter construction
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn test_health_server_adapter_initial_state() {
        let health_server = Arc::new(nemesis_health::server::HealthServer::new(make_health_config(18790)));
        let adapter = HealthServerAdapter::new(health_server);
        assert!(adapter.start().is_ok());
    }

    #[test]
    fn test_health_server_adapter_stop() {
        let health_server = Arc::new(nemesis_health::server::HealthServer::new(make_health_config(18791)));
        let adapter = HealthServerAdapter::new(health_server);
        assert!(adapter.stop().is_ok());
    }

    #[tokio::test]
    async fn test_health_server_adapter_start_idempotent() {
        let health_server = Arc::new(nemesis_health::server::HealthServer::new(make_health_config(18792)));
        let adapter = HealthServerAdapter::new(health_server);
        assert!(adapter.start().is_ok());
        assert!(adapter.start().is_ok());
        assert!(adapter.stop().is_ok());
    }

    // -------------------------------------------------------------------------
    // HeartbeatServiceAdapter construction
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn test_heartbeat_adapter_initial_state() {
        let heartbeat = Arc::new(nemesis_heartbeat::service::HeartbeatService::new(make_heartbeat_config()));
        let adapter = HeartbeatServiceAdapter::new(heartbeat);
        assert!(adapter.start().is_ok());
    }

    #[test]
    fn test_heartbeat_adapter_stop() {
        let heartbeat = Arc::new(nemesis_heartbeat::service::HeartbeatService::new(make_heartbeat_config()));
        let adapter = HeartbeatServiceAdapter::new(heartbeat);
        assert!(adapter.stop().is_ok());
    }

    #[tokio::test]
    async fn test_heartbeat_adapter_start_idempotent() {
        let heartbeat = Arc::new(nemesis_heartbeat::service::HeartbeatService::new(make_heartbeat_config()));
        let adapter = HeartbeatServiceAdapter::new(heartbeat);
        assert!(adapter.start().is_ok());
        assert!(adapter.start().is_ok());
        assert!(adapter.stop().is_ok());
    }

    // -------------------------------------------------------------------------
    // ChannelManagerAdapter construction
    // -------------------------------------------------------------------------

    #[test]
    fn test_channel_manager_adapter_enabled_channels() {
        let manager = Arc::new(nemesis_channels::manager::ChannelManager::new());
        let channels = vec!["web".to_string(), "websocket".to_string()];
        let adapter = ChannelManagerAdapter::new(manager, channels.clone());
        assert_eq!(adapter.enabled_channels(), channels);
    }

    #[test]
    fn test_channel_manager_adapter_empty_channels() {
        let manager = Arc::new(nemesis_channels::manager::ChannelManager::new());
        let adapter = ChannelManagerAdapter::new(manager, vec![]);
        assert!(adapter.enabled_channels().is_empty());
    }

    #[tokio::test]
    async fn test_channel_manager_adapter_start() {
        let manager = Arc::new(nemesis_channels::manager::ChannelManager::new());
        let adapter = ChannelManagerAdapter::new(manager, vec!["web".to_string()]);
        assert!(adapter.start().is_ok());
    }

    #[tokio::test]
    async fn test_channel_manager_adapter_stop() {
        let manager = Arc::new(nemesis_channels::manager::ChannelManager::new());
        let adapter = ChannelManagerAdapter::new(manager, vec![]);
        assert!(adapter.stop().is_ok());
    }

    #[tokio::test]
    async fn test_channel_manager_adapter_start_idempotent() {
        let manager = Arc::new(nemesis_channels::manager::ChannelManager::new());
        let adapter = ChannelManagerAdapter::new(manager, vec![]);
        assert!(adapter.start().is_ok());
        assert!(adapter.start().is_ok());
    }

    // -------------------------------------------------------------------------
    // AtomicBool ordering test
    // -------------------------------------------------------------------------

    #[test]
    fn test_atomic_bool_swap_behavior() {
        let flag = AtomicBool::new(false);
        assert!(!flag.swap(true, Ordering::SeqCst));
        assert!(flag.swap(true, Ordering::SeqCst));
        assert!(flag.swap(false, Ordering::SeqCst));
        assert!(!flag.swap(false, Ordering::SeqCst));
    }

    // -------------------------------------------------------------------------
    // LifecycleService trait tests
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn test_health_server_adapter_trait_object() {
        let health_server = Arc::new(nemesis_health::server::HealthServer::new(make_health_config(18793)));
        let adapter = HealthServerAdapter::new(health_server);
        let _trait_obj: &dyn LifecycleService = &adapter;
        assert!(adapter.start().is_ok());
    }

    #[tokio::test]
    async fn test_heartbeat_adapter_trait_object() {
        let heartbeat = Arc::new(nemesis_heartbeat::service::HeartbeatService::new(make_heartbeat_config()));
        let adapter = HeartbeatServiceAdapter::new(heartbeat);
        let _trait_obj: &dyn LifecycleService = &adapter;
        assert!(adapter.start().is_ok());
    }

    #[tokio::test]
    async fn test_channel_manager_adapter_trait_object() {
        let manager = Arc::new(nemesis_channels::manager::ChannelManager::new());
        let adapter = ChannelManagerAdapter::new(manager, vec!["web".to_string()]);
        let _trait_obj: &dyn LifecycleService = &adapter;
        assert!(adapter.start().is_ok());
    }
}
