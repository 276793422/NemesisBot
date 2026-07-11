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
    LifecycleService,
    ChannelManager as ChannelManagerTrait,
    AgentLoopService as AgentLoopServiceTrait,
};
#[cfg(feature = "health")]
use nemesis_services::HealthServer as HealthServerTrait;
#[cfg(feature = "heartbeat")]
use nemesis_services::HeartbeatService as HeartbeatServiceTrait;

// ---------------------------------------------------------------------------
// HealthServer adapter
// ---------------------------------------------------------------------------

/// Adapter wrapping `nemesis_health::HealthServer` to implement the
/// `nemesis_services::HealthServer` trait.
#[cfg(feature = "health")]
pub struct HealthServerAdapter {
    inner: Arc<nemesis_health::server::HealthServer>,
    started: AtomicBool,
}

#[cfg(feature = "health")]
impl HealthServerAdapter {
    pub fn new(inner: Arc<nemesis_health::server::HealthServer>) -> Self {
        Self {
            inner,
            started: AtomicBool::new(false),
        }
    }
}

#[cfg(feature = "health")]
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

#[cfg(feature = "health")]
impl HealthServerTrait for HealthServerAdapter {}

// ---------------------------------------------------------------------------
// HeartbeatService adapter
// ---------------------------------------------------------------------------

/// Adapter wrapping `nemesis_heartbeat::HeartbeatService` to implement the
/// `nemesis_services::HeartbeatService` trait.
#[cfg(feature = "heartbeat")]
pub struct HeartbeatServiceAdapter {
    inner: Arc<nemesis_heartbeat::service::HeartbeatService>,
    started: AtomicBool,
}

#[cfg(feature = "heartbeat")]
impl HeartbeatServiceAdapter {
    pub fn new(inner: Arc<nemesis_heartbeat::service::HeartbeatService>) -> Self {
        Self {
            inner,
            started: AtomicBool::new(false),
        }
    }
}

#[cfg(feature = "heartbeat")]
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

#[cfg(feature = "heartbeat")]
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

/// All mutable state protected by a single Mutex.
/// This ensures start()/stop() are inherently atomic — no intermediate
/// state is ever visible to concurrent callers.
struct AgentLoopState {
    /// Current AgentLoop instance. `Some` = running, `None` = stopped.
    agent_loop: Option<Arc<nemesis_agent::r#loop::AgentLoop>>,
    /// Handle to the inbound bridge task (bus broadcast → mpsc).
    bridge_handle: Option<tokio::task::JoinHandle<()>>,
    /// Handle to the agent loop task.
    agent_handle: Option<tokio::task::JoinHandle<()>>,
}

/// Adapter wrapping `nemesis_agent::AgentLoop` to implement the
/// `nemesis_services::AgentLoopService` trait.
///
/// On `start()`: calls `build_agent_loop()` to create a fresh AgentLoop from
/// disk config, subscribes to the message bus inbound broadcast, creates an
/// mpsc bridge, and spawns the agent loop's `run_bus_arc()`.
/// On `stop()`: aborts the inbound bridge, drops the old AgentLoop entirely.
/// The outbound bridge (agent → bus) is persistent and created separately
/// in gateway.rs; it survives stop/start cycles.
///
/// **Thread safety**: All mutable state lives in `Mutex<AgentLoopState>`.
/// `start()` and `stop()` hold this lock for the entire operation, making
/// them inherently serial. No intermediate state is possible.
pub struct AgentLoopServiceAdapter {
    /// Single Mutex protecting all mutable state.
    state: std::sync::Mutex<AgentLoopState>,
    /// Shared resources for factory function.
    shared: Arc<crate::agent_factory::SharedResources>,
    /// Shared reference with AppState — updated on each start/stop.
    agent_loop_ref: Arc<parking_lot::RwLock<Option<Arc<nemesis_agent::r#loop::AgentLoop>>>>,
    bus: Arc<nemesis_bus::MessageBus>,
    /// Tokio runtime handle captured at construction time.
    /// Needed because tray callbacks run on the winit thread (no tokio context),
    /// but `start()` needs to spawn async tasks on the tokio runtime.
    rt: tokio::runtime::Handle,
}

impl AgentLoopServiceAdapter {
    /// Create a new adapter with an initial AgentLoop (from first factory call in gateway.rs).
    /// The adapter starts in "stopped" state — `start()` must be called to begin processing.
    pub fn new(
        initial_agent_loop: Arc<nemesis_agent::r#loop::AgentLoop>,
        shared: Arc<crate::agent_factory::SharedResources>,
        bus: Arc<nemesis_bus::MessageBus>,
        agent_loop_ref: Arc<parking_lot::RwLock<Option<Arc<nemesis_agent::r#loop::AgentLoop>>>>,
    ) -> Self {
        Self {
            state: std::sync::Mutex::new(AgentLoopState {
                agent_loop: Some(initial_agent_loop),
                bridge_handle: None,
                agent_handle: None,
            }),
            shared,
            agent_loop_ref,
            bus,
            rt: tokio::runtime::Handle::current(),
        }
    }

    /// Get the current AgentLoop (if running). Used by heartbeat and external callers.
    #[allow(dead_code)] // used by the heartbeat handler (heartbeat feature) + external callers.
    pub fn current(&self) -> Option<Arc<nemesis_agent::r#loop::AgentLoop>> {
        self.state.lock().unwrap().agent_loop.clone()
    }
}

impl LifecycleService for AgentLoopServiceAdapter {
    fn start(&self) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();

        if state.agent_loop.is_some() && state.bridge_handle.is_some() {
            tracing::info!("[AgentAdapter] start: already started, skipping");
            return Ok(());
        }

        // If we have a pre-built AgentLoop (from new()) but no tasks yet, use it.
        // Otherwise (after a stop), build a fresh one from disk.
        let agent_loop = if let Some(al) = state.agent_loop.take() {
            tracing::info!("[AgentAdapter] start: using pre-built AgentLoop");
            al
        } else {
            tracing::info!("[AgentAdapter] start: building fresh AgentLoop via factory...");
            match crate::agent_factory::build_agent_loop(&self.shared) {
                Ok(al) => al,
                Err(e) => {
                    // No need to roll back any flag — state is unchanged
                    return Err(format!("Failed to build agent loop: {}", e));
                }
            }
        };

        // Update shared reference for WebServer/AppState.
        *self.agent_loop_ref.write() = Some(agent_loop.clone());

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
        let agent_loop_clone = agent_loop.clone();
        let agent_task = self.rt.spawn(async move {
            agent_loop_clone.run_bus_arc(agent_inbound_rx).await;
        });

        // Store everything in state (still holding the lock).
        state.agent_loop = Some(agent_loop);
        state.bridge_handle = Some(bridge);
        state.agent_handle = Some(agent_task);

        tracing::info!("[AgentAdapter] start: agent loop started, listening on bus");
        Ok(())
    }

    fn stop(&self) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();

        if state.agent_loop.is_none() {
            tracing::info!("[AgentAdapter] stop: already stopped, skipping");
            return Ok(());
        }

        tracing::info!("[AgentAdapter] stop: shutting down agent...");

        // Set running=false and clear session busy states.
        if let Some(ref al) = state.agent_loop {
            al.stop();
            al.clear_session_busy();
        }

        // Abort the inbound bridge. This drops `agent_inbound_tx` (the mpsc
        // sender), which causes `run_bus_arc`'s `inbound_rx.recv()` to return
        // `None`, breaking the loop promptly — even when idle.
        if let Some(handle) = state.bridge_handle.take() {
            handle.abort();
        }

        // Abort the agent loop task. Known behavior: if the agent was
        // processing a message, the response for that message is lost.
        // This is expected — the user explicitly stopped the agent, so
        // incomplete replies are acceptable.
        if let Some(handle) = state.agent_handle.take() {
            handle.abort();
        }

        // Drop the old AgentLoop entirely.
        state.agent_loop.take();
        *self.agent_loop_ref.write() = None;

        tracing::info!("[AgentAdapter] stop: agent loop stopped and destroyed");
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.state.lock().unwrap().bridge_handle.is_some()
    }
}

impl AgentLoopServiceTrait for AgentLoopServiceAdapter {
    fn cancel_session(&self, session_key: &str) -> bool {
        let state = self.state.lock().unwrap();
        if let Some(ref al) = state.agent_loop {
            al.cancel_session(session_key)
        } else {
            false
        }
    }

    fn cancel_all_sessions(&self) -> usize {
        let state = self.state.lock().unwrap();
        if let Some(ref al) = state.agent_loop {
            al.cancel_all_sessions()
        } else {
            0
        }
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
    fn send_to_session(&self, session_id: &str, role: &str, content: &str, model: Option<&str>) -> std::result::Result<(), String> {
        let sm = self.session_manager.clone();
        let sid = session_id.to_string();
        let content = content.to_string();
        let model = model.map(|s| s.to_string());
        tokio::task::block_in_place(|| {
            self.rt.block_on(nemesis_web::server::send_to_session(
                &sm, &sid, role, &content, model.as_deref(),
            ))
        })
    }

    fn send_history_to_session(&self, session_id: &str, content: &str) -> std::result::Result<(), String> {
        let sm = self.session_manager.clone();
        let sid = session_id.to_string();
        let content = content.to_string();
        tokio::task::block_in_place(|| {
            self.rt.block_on(nemesis_web::server::send_history_to_session(
                &sm, &sid, &content,
            ))
        })
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
        let sm = self.session_manager.clone();
        for sid in self.active_session_ids() {
            let data_clone = data.clone();
            tokio::task::block_in_place(|| {
                self.rt.block_on(sm.broadcast(&sid, &data_clone))
            })?;
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
