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
    AgentLoopService as AgentLoopServiceTrait,
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
                tracing::error!("[Main] Health server error: {}", e);
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
                tracing::error!("[Main] Heartbeat service error: {}", e);
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
                tracing::error!("[Main] Channel manager start error: {}", e);
            }
        });
        Ok(())
    }

    fn stop(&self) -> Result<(), String> {
        let inner = self.inner.clone();
        tokio::spawn(async move {
            if let Err(e) = inner.stop_all().await {
                tracing::error!("[Main] Channel manager stop error: {}", e);
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

// ---------------------------------------------------------------------------
// AgentLoop adapter
// ---------------------------------------------------------------------------

/// Adapter wrapping `nemesis_agent::AgentLoop` to implement the
/// `nemesis_services::AgentLoopService` trait.
///
/// On `start()`: subscribes to the message bus inbound broadcast, creates
/// an mpsc bridge, and spawns the agent loop's `run_bus_arc()`.
/// On `stop()`: aborts the inbound bridge (dropping the mpsc sender) so
/// `run_bus_arc` receives `None` from its `recv()` and exits cleanly.
/// The outbound bridge (agent → bus) is persistent and created separately
/// in gateway.rs; it survives stop/start cycles.
pub struct AgentLoopServiceAdapter {
    inner: Arc<nemesis_agent::r#loop::AgentLoop>,
    bus: Arc<nemesis_bus::MessageBus>,
    /// Tokio runtime handle captured at construction time.
    /// Needed because tray callbacks run on the winit thread (no tokio context),
    /// but `start()` needs to spawn async tasks on the tokio runtime.
    rt: tokio::runtime::Handle,
    started: AtomicBool,
    /// Handle to the inbound bridge task (bus broadcast → mpsc).
    /// Aborting it drops the mpsc sender, causing the agent loop's
    /// `recv()` to return `None` so it exits promptly.
    bridge_handle: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Handle to the agent loop task.
    agent_handle: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl AgentLoopServiceAdapter {
    pub fn new(
        inner: Arc<nemesis_agent::r#loop::AgentLoop>,
        bus: Arc<nemesis_bus::MessageBus>,
    ) -> Self {
        Self {
            inner,
            bus,
            rt: tokio::runtime::Handle::current(),
            started: AtomicBool::new(false),
            bridge_handle: std::sync::Mutex::new(None),
            agent_handle: std::sync::Mutex::new(None),
        }
    }
}

impl LifecycleService for AgentLoopServiceAdapter {
    fn start(&self) -> Result<(), String> {
        if self.started.swap(true, Ordering::SeqCst) {
            tracing::info!("[AgentAdapter] start: already started, skipping");
            return Ok(()); // Already started
        }

        tracing::info!("[AgentAdapter] start: initializing...");

        // Create a new mpsc channel for inbound messages
        let (agent_inbound_tx, agent_inbound_rx) =
            tokio::sync::mpsc::channel::<nemesis_types::channel::InboundMessage>(1024);

        // Bridge: bus inbound broadcast → agent inbound mpsc
        let bus_inbound = self.bus.subscribe_inbound();
        let rt = self.rt.clone();
        let bridge = rt.spawn(async move {
            let mut rx = bus_inbound;
            let mut total_dropped: u64 = 0;
            loop {
                match rx.recv().await {
                    Ok(msg) => {
                        if agent_inbound_tx.send(msg).await.is_err() {
                            break; // Agent receiver dropped
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        total_dropped += n as u64;
                        tracing::warn!(
                            "[Main] Agent inbound bridge lagged by {} messages (total dropped: {})",
                            n, total_dropped
                        );
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        if total_dropped > 0 {
                            tracing::warn!(
                                "[Main] Agent inbound bridge closing with {} total dropped messages",
                                total_dropped
                            );
                        }
                        break;
                    }
                }
            }
        });

        // Spawn the agent loop
        let agent_loop = self.inner.clone();
        let agent_task = self.rt.spawn(async move {
            agent_loop.run_bus_arc(agent_inbound_rx).await;
        });

        // Store handles so stop() can abort them
        *self.bridge_handle.lock().unwrap() = Some(bridge);
        *self.agent_handle.lock().unwrap() = Some(agent_task);

        tracing::info!("[AgentAdapter] start: agent loop started, listening on bus");
        Ok(())
    }

    fn stop(&self) -> Result<(), String> {
        if !self.started.swap(false, Ordering::SeqCst) {
            tracing::info!("[AgentAdapter] stop: already stopped, skipping");
            return Ok(()); // Already stopped
        }

        tracing::info!("[AgentAdapter] stop: shutting down agent...");

        // Set running=false so the agent loop won't process more messages
        // after the current recv() returns.
        self.inner.stop();

        // Abort the inbound bridge. This drops `agent_inbound_tx` (the mpsc
        // sender), which causes `run_bus_arc`'s `inbound_rx.recv()` to return
        // `None`, breaking the loop promptly — even when idle.
        if let Some(handle) = self.bridge_handle.lock().unwrap().take() {
            handle.abort();
        }

        // Abort the agent loop task. Known behavior: if the agent was
        // processing a message, the response for that message is lost.
        // This is expected — the user explicitly stopped the agent, so
        // incomplete replies are acceptable.
        if let Some(handle) = self.agent_handle.lock().unwrap().take() {
            handle.abort();
        }

        // Clear session busy states. If the agent was aborted mid-processing,
        // sessions remain locked as "busy" and would reject all future
        // messages after restart. Clearing unlocks them.
        self.inner.clear_session_busy();

        tracing::info!("[AgentAdapter] stop: agent loop stopped");
        Ok(())
    }
}

impl AgentLoopServiceTrait for AgentLoopServiceAdapter {
    fn is_running(&self) -> bool {
        self.inner.is_running()
    }
}

#[cfg(test)]
mod tests;

// ---------------------------------------------------------------------------
// WebServerOps adapter
// ---------------------------------------------------------------------------

use nemesis_channels::web::WebServerOps;

/// Adapter that bridges `nemesis_web::SessionManager` to the `WebServerOps` trait
/// used by `WebChannel` for outbound message delivery.
pub struct WebServerOpsAdapter {
    session_manager: Arc<nemesis_web::session::SessionManager>,
    rt: tokio::runtime::Handle,
}

impl WebServerOpsAdapter {
    pub fn new(session_manager: Arc<nemesis_web::session::SessionManager>) -> Self {
        Self {
            session_manager,
            rt: tokio::runtime::Handle::current(),
        }
    }
}

impl WebServerOps for WebServerOpsAdapter {
    fn send_to_session(&self, session_id: &str, role: &str, content: &str) -> std::result::Result<(), String> {
        self.rt.block_on(nemesis_web::server::send_to_session(
            &self.session_manager, session_id, role, content,
        ))
    }

    fn send_history_to_session(&self, session_id: &str, content: &str) -> std::result::Result<(), String> {
        self.rt.block_on(nemesis_web::server::send_history_to_session(
            &self.session_manager, session_id, content,
        ))
    }

    fn broadcast(&self, content: &str) -> std::result::Result<(), String> {
        let msg = nemesis_web::protocol::ProtocolMessage::new(
            "message", "chat", "receive",
            Some(serde_json::json!({
                "role": "assistant",
                "content": content,
            })),
        );
        let data = serde_json::to_vec(&msg).map_err(|e| format!("marshal: {}", e))?;
        for sid in self.active_session_ids() {
            self.rt.block_on(self.session_manager.broadcast(&sid, &data))?;
        }
        Ok(())
    }

    fn active_session_ids(&self) -> Vec<String> {
        self.session_manager.all_sessions().into_iter().map(|s| s.id).collect()
    }

    fn start_server(&self) -> std::result::Result<(), String> {
        Ok(())
    }

    fn stop_server(&self) {}
}
