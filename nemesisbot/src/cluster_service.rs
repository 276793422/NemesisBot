//! Cluster service adapter for dynamic start/stop lifecycle management.
//!
//! Mirrors `AgentLoopServiceAdapter` — wraps the cluster module's components
//! (discovery, RPC, cluster agent) into a `LifecycleService` that can be
//! started and stopped at runtime without restarting the gateway.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use nemesis_cluster::cluster::Cluster;
use nemesis_cluster::cluster_task::{ClusterTaskList, ClusterWorkQueue};
use nemesis_services::LifecycleService;

use crate::agent_factory::SharedResources;

// ---------------------------------------------------------------------------
// ClusterServiceState — mutable state protected by Mutex
// ---------------------------------------------------------------------------

struct ClusterServiceState {
    running: bool,
    agent_handle: Option<tokio::task::JoinHandle<()>>,
}

// ---------------------------------------------------------------------------
// ClusterServiceAdapter
// ---------------------------------------------------------------------------

/// Adapter that manages the cluster module's lifecycle.
///
/// On `start()`: starts cluster internals (RPC server, discovery), spawns
/// cluster agent loop, enables ClusterRpcTool.
///
/// On `stop()`: disables ClusterRpcTool, sends shutdown signal to agent loop,
/// stops cluster internals (RPC server, discovery, recovery/sync loops).
pub struct ClusterServiceAdapter {
    state: std::sync::Mutex<ClusterServiceState>,
    cluster: Arc<Cluster>,
    shared: Arc<SharedResources>,
    rt: tokio::runtime::Handle,
    #[allow(dead_code)]
    home: std::path::PathBuf,
    cluster_task_list: Arc<ClusterTaskList>,
    cluster_work_queue: Arc<ClusterWorkQueue>,
    shutdown_tx: tokio::sync::broadcast::Sender<()>,
}

impl ClusterServiceAdapter {
    /// Create a new adapter with references to shared resources.
    pub fn new(
        cluster: Arc<Cluster>,
        shared: Arc<SharedResources>,
        rt: tokio::runtime::Handle,
        home: std::path::PathBuf,
        cluster_task_list: Arc<ClusterTaskList>,
        cluster_work_queue: Arc<ClusterWorkQueue>,
    ) -> Self {
        let (shutdown_tx, _) = tokio::sync::broadcast::channel(1);
        Self {
            state: std::sync::Mutex::new(ClusterServiceState {
                running: false,
                agent_handle: None,
            }),
            cluster,
            shared,
            rt,
            home,
            cluster_task_list,
            cluster_work_queue,
            shutdown_tx,
        }
    }

    /// Check if the cluster service is running.
    #[allow(dead_code)]
    pub fn is_running(&self) -> bool {
        self.state.lock().unwrap().running
    }

    /// Get a reference to the cluster.
    #[allow(dead_code)]
    pub fn cluster(&self) -> &Arc<Cluster> {
        &self.cluster
    }

    /// Perform first-time cluster agent startup (called once from gateway.rs).
    ///
    /// Gateway already handles cluster.start(), rpc_server.start(), start_discovery().
    /// This method only: restores tasks from disk, spawns agent loop, sets enabled flag.
    /// Returns the agent handle for the caller to track.
    pub fn first_start(&self) -> Result<(), String> {
        // Task recovery from disk (crash recovery)
        if let Err(e) = self.cluster_task_list.restore_from_disk() {
            tracing::warn!("[ClusterAdapter] Failed to restore tasks from disk: {}", e);
        }
        let recovered = self.cluster_task_list.recover_task_ids();
        for task_id in &recovered {
            if let Err(e) = self.cluster_work_queue.submit(task_id.clone()) {
                tracing::warn!(task_id = %task_id, "[ClusterAdapter] Failed to re-submit recovered task: {}", e);
            }
        }
        if !recovered.is_empty() {
            tracing::info!(count = recovered.len(), "[ClusterAdapter] Recovered {} tasks", recovered.len());
        }

        // Build and spawn cluster agent loop
        let rpc_client = self.cluster.rpc_client_arc();
        let cluster_arc = self.cluster.clone();
        let handle = match crate::agent_factory::build_cluster_agent_loop(&self.shared, cluster_arc) {
            Ok((cluster_agent, cluster_config)) => {
                let shutdown_rx = self.shutdown_tx.subscribe();
                let work_queue = self.cluster_work_queue.clone();
                let task_list = self.cluster_task_list.clone();
                let handle = tokio::spawn(async move {
                    crate::cluster_agent::cluster_agent_loop(
                        cluster_agent,
                        cluster_config,
                        work_queue,
                        task_list,
                        rpc_client,
                        shutdown_rx,
                    )
                    .await;
                });
                tracing::info!("[ClusterAdapter] Agent event loop spawned");
                Some(handle)
            }
            Err(e) => {
                tracing::warn!("[ClusterAdapter] Failed to build cluster agent: {}", e);
                None
            }
        };

        self.state.lock().unwrap().agent_handle = handle;
        self.state.lock().unwrap().running = true;

        if let Some(ref enabled) = *self.shared.cluster_rpc_enabled.read() {
            enabled.store(true, Ordering::Relaxed);
        }

        tracing::info!("[ClusterAdapter] First start completed");
        Ok(())
    }
}

impl LifecycleService for ClusterServiceAdapter {
    fn is_running(&self) -> bool {
        self.state.lock().unwrap().running
    }

    fn start(&self) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        if state.running {
            tracing::info!("[ClusterAdapter] start: already running, skipping");
            return Ok(());
        }

        let handle = tokio::task::block_in_place(|| {
            self.rt.block_on(start_cluster_components(
                &self.cluster,
                &self.shared,
                &self.cluster_task_list,
                &self.cluster_work_queue,
                self.shutdown_tx.clone(),
            ))
        })?;

        state.agent_handle = handle;
        state.running = true;

        if let Some(ref enabled) = *self.shared.cluster_rpc_enabled.read() {
            enabled.store(true, Ordering::Relaxed);
            tracing::info!("[ClusterAdapter] ClusterRpcTool enabled=true");
        }

        tracing::info!("[ClusterAdapter] Cluster started");
        Ok(())
    }

    fn stop(&self) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        if !state.running {
            tracing::info!("[ClusterAdapter] stop: already stopped, skipping");
            return Ok(());
        }

        // 1. Disable ClusterRpcTool FIRST — prevent new RPC calls during shutdown window.
        if let Some(ref enabled) = *self.shared.cluster_rpc_enabled.read() {
            enabled.store(false, Ordering::Relaxed);
            tracing::info!("[ClusterAdapter] ClusterRpcTool enabled=false");
        }

        // 2. Send shutdown signal to cluster agent loop (graceful: finish current task then exit)
        let _ = self.shutdown_tx.send(());

        // 3. Stop cluster internals: RPC server → discovery → recovery/sync loops
        self.cluster.stop();

        // 4. Abort agent handle (safety net — agent should have exited from shutdown signal)
        if let Some(handle) = state.agent_handle.take() {
            handle.abort();
            tracing::info!("[ClusterAdapter] Cluster agent task aborted");
        }

        state.running = false;
        tracing::info!("[ClusterAdapter] Cluster stopped");
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// start_cluster_components — reusable startup function
// ---------------------------------------------------------------------------

/// Restart cluster runtime components after a stop().
///
/// This is called by `LifecycleService::start()` to restart all components
/// that were stopped by a previous `stop()` call:
/// - cluster.start() (registers local node, RPC client, recovery/sync loops)
/// - RPC server.start() (re-bind TCP listener)
/// - start_discovery() (re-start UDP broadcast/listen)
/// - spawn cluster agent loop
async fn start_cluster_components(
    cluster: &Arc<Cluster>,
    shared: &Arc<SharedResources>,
    cluster_task_list: &Arc<ClusterTaskList>,
    cluster_work_queue: &Arc<ClusterWorkQueue>,
    shutdown_tx: tokio::sync::broadcast::Sender<()>,
) -> Result<Option<tokio::task::JoinHandle<()>>, String> {
    // 1. Start cluster (registers local node, creates RPC client, starts sync/recovery loops)
    cluster.start();
    tracing::info!("[ClusterComponents] cluster.start() done");

    // 2. Start RPC server (bind TCP listener + accept loop)
    if let Some(server) = cluster.rpc_server() {
        let server = server.clone();
        server.start().await.map_err(|e| format!("RPC server start: {}", e))?;
        tracing::info!("[ClusterComponents] RPC server started");
    }

    // 3. Start UDP discovery
    cluster.start_discovery(cluster.clone());
    tracing::info!("[ClusterComponents] Discovery started");

    // 4. Build cluster agent loop and spawn
    let rpc_client = cluster.rpc_client_arc();
    let cluster_arc = cluster.clone();
    match crate::agent_factory::build_cluster_agent_loop(shared, cluster_arc) {
        Ok((cluster_agent, cluster_config)) => {
            let shutdown_rx = shutdown_tx.subscribe();
            let work_queue = cluster_work_queue.clone();
            let task_list = cluster_task_list.clone();
            let handle = tokio::spawn(async move {
                crate::cluster_agent::cluster_agent_loop(
                    cluster_agent,
                    cluster_config,
                    work_queue,
                    task_list,
                    rpc_client,
                    shutdown_rx,
                )
                .await;
            });
            tracing::info!("[ClusterComponents] Agent event loop spawned");
            Ok(Some(handle))
        }
        Err(e) => {
            tracing::warn!("[ClusterComponents] Failed to build cluster agent: {}", e);
            Ok(None)
        }
    }
}
