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
