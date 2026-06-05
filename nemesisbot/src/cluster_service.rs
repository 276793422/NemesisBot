//! Cluster service adapter for dynamic start/stop lifecycle management.
//!
//! Mirrors `AgentLoopServiceAdapter` — wraps the cluster module's components
//! (discovery, RPC, cluster agent) into a `LifecycleService` that can be
//! started and stopped at runtime without restarting the gateway.
//!
//! **Current mode**: Simplified — tool stays registered, enabled flag toggles.
//! **Future mode** [ClusterService-Full]: Tool dynamically removed/added.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use nemesis_services::LifecycleService;

use crate::agent_factory::SharedResources;

// ---------------------------------------------------------------------------
// ClusterServiceState — mutable state protected by Mutex
// ---------------------------------------------------------------------------

struct ClusterServiceState {
    /// Whether the cluster service has been started.
    running: bool,
    /// Handle to the cluster agent event loop task.
    agent_handle: Option<tokio::task::JoinHandle<()>>,
}

// ---------------------------------------------------------------------------
// ClusterServiceAdapter
// ---------------------------------------------------------------------------

/// Adapter that manages the cluster module's lifecycle.
///
/// On `start()`: sets the enabled flag on ClusterRpcTool, allowing RPC calls.
/// On `stop()`: clears the enabled flag, preventing RPC calls.
///
/// The actual cluster initialization (discovery, RPC server, cluster agent)
/// still happens in `gateway.rs` at startup. This adapter controls whether
/// the cluster is **active** (tool enabled) or **inactive** (tool disabled).
///
/// **Why not fully start/stop cluster components here?**
/// The gateway.rs initialization block (~397 lines) creates deeply interconnected
/// state (Arc references to cluster, work queue, task list, RPC client, peer chat
/// handler, continuation manager, etc.). Fully extracting this into the adapter
/// is a larger refactoring effort. This adapter provides the most impactful part:
/// toggling the cluster on/off from the user's perspective.
pub struct ClusterServiceAdapter {
    state: std::sync::Mutex<ClusterServiceState>,
    shared: Arc<SharedResources>,
    rt: tokio::runtime::Handle,
}

impl ClusterServiceAdapter {
    /// Create a new adapter with references to shared resources.
    pub fn new(shared: Arc<SharedResources>, rt: tokio::runtime::Handle) -> Self {
        Self {
            state: std::sync::Mutex::new(ClusterServiceState {
                running: false,
                agent_handle: None,
            }),
            shared,
            rt,
        }
    }

    /// Check if the cluster service is running.
    pub fn is_running(&self) -> bool {
        self.state.lock().unwrap().running
    }

    /// Set the cluster agent task handle (called from gateway.rs after spawning).
    pub fn set_agent_handle(&self, handle: tokio::task::JoinHandle<()>) {
        self.state.lock().unwrap().agent_handle = Some(handle);
    }
}

impl LifecycleService for ClusterServiceAdapter {
    fn start(&self) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();

        if state.running {
            tracing::info!("[ClusterAdapter] start: already running, skipping");
            return Ok(());
        }

        tracing::info!("[ClusterAdapter] start: enabling cluster...");

        // Enable the ClusterRpcTool via shared enabled flag.
        if let Some(ref enabled) = *self.shared.cluster_rpc_enabled.read() {
            enabled.store(true, Ordering::Relaxed);
            tracing::info!("[ClusterAdapter] ClusterRpcTool enabled=true");
        } else {
            tracing::warn!("[ClusterAdapter] No cluster_rpc_enabled flag set — tool not registered?");
        }

        state.running = true;
        tracing::info!("[ClusterAdapter] start: cluster enabled");
        Ok(())
    }

    fn stop(&self) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();

        if !state.running {
            tracing::info!("[ClusterAdapter] stop: already stopped, skipping");
            return Ok(());
        }

        tracing::info!("[ClusterAdapter] stop: disabling cluster...");

        // Disable the ClusterRpcTool via shared enabled flag.
        // The tool definition stays in the prompt (preserving LLM cache),
        // but execute() will return "集群功能未启用" immediately.
        if let Some(ref enabled) = *self.shared.cluster_rpc_enabled.read() {
            enabled.store(false, Ordering::Relaxed);
            tracing::info!("[ClusterAdapter] ClusterRpcTool enabled=false");
        }

        // Abort the cluster agent event loop if running.
        if let Some(handle) = state.agent_handle.take() {
            handle.abort();
            tracing::info!("[ClusterAdapter] Cluster agent task aborted");
        }

        state.running = false;
        tracing::info!("[ClusterAdapter] stop: cluster disabled");
        Ok(())
    }
}
