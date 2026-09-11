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
    // G1 收口（2026-09-08）：work-queue 路径的回调结果持久化（回调失败 →
    // set_result 落盘真结果；成功 → delete 清占位）。与 gateway 传给
    // peer_chat_handler 的是同一份 adapter（同一 result_store 真相源）。
    result_persister: Arc<dyn nemesis_cluster::rpc::peer_chat_handler::TaskResultPersister>,
    // Swarm M3（G4）：看板讨论事件入站箱（worker 侧 Some；master/board
    // 裁剪时 None）。每次 start 换入新管道（stop/start 周期安全）。
    discussion_inbox: Option<Arc<crate::cluster_agent::DiscussionInbox>>,
    shutdown_tx: tokio::sync::broadcast::Sender<()>,
}

impl ClusterServiceAdapter {
    /// Create a new adapter with references to shared resources.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        cluster: Arc<Cluster>,
        shared: Arc<SharedResources>,
        rt: tokio::runtime::Handle,
        home: std::path::PathBuf,
        cluster_task_list: Arc<ClusterTaskList>,
        cluster_work_queue: Arc<ClusterWorkQueue>,
        result_persister: Arc<dyn nemesis_cluster::rpc::peer_chat_handler::TaskResultPersister>,
        discussion_inbox: Option<Arc<crate::cluster_agent::DiscussionInbox>>,
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
            result_persister,
            discussion_inbox,
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
            tracing::info!(
                count = recovered.len(),
                "[ClusterAdapter] Recovered {} tasks",
                recovered.len()
            );
        }

        // G1（2026-09-01 集群韧性 goal）：rpc_cache/results 7 天 TTL 清扫 +
        // 磁盘结果回载（A 崩溃前本地完成的任务结果，轮询恢复时可直接命中）。
        // 返回值 (清扫数, 回载数) 由 helper 内部日志承载，此处无需消费。
        let _ = sweep_and_reload_stale_results(self.cluster.result_store());

        // G5（A 侧重启恢复链路）：把磁盘上未删除的续行快照重新登记进
        // TaskManager（Pending + peer_id），让恢复轮询（poll_stale_pending_tasks）
        // 能向 B 查询结果并回灌 bus → AgentLoop 拦截 cluster_continuation 完成
        // 回复。continuation 数据本体已由 agent_factory 的
        // ContinuationManager::with_disk_store 回载进内存，这里只补任务登记。
        {
            let workspace = self.cluster.workspace().clone();
            let cont_store = nemesis_agent::ContinuationStore::new(&workspace);
            let mut restored = 0usize;
            for task_id in cont_store.list_pending() {
                if self.cluster.task_manager().get_task(&task_id).is_some() {
                    continue; // 已登记，防重复提交
                }
                let snapshot = match cont_store.load(&task_id) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!(
                            task_id = %task_id,
                            "[ClusterAdapter] Failed to load continuation snapshot: {}",
                            e
                        );
                        continue;
                    }
                };
                if snapshot.peer_id.is_empty() {
                    // 旧格式快照无 peer_id，无法向 B 发起查询；留给 TTL 清扫。
                    continue;
                }
                let task = nemesis_types::cluster::Task {
                    id: task_id.clone(),
                    status: nemesis_types::cluster::TaskStatus::Pending,
                    action: "peer_chat".to_string(),
                    peer_id: snapshot.peer_id,
                    payload: serde_json::json!({}),
                    result: None,
                    original_channel: snapshot.channel,
                    original_chat_id: snapshot.chat_id,
                    created_at: snapshot.created_at,
                    completed_at: None,
                };
                if let Err(e) = self.cluster.task_manager().submit(task) {
                    tracing::warn!(
                        task_id = %task_id,
                        "[ClusterAdapter] Failed to re-register pending continuation: {}",
                        e
                    );
                    continue;
                }
                restored += 1;
            }
            if restored > 0 {
                tracing::info!(
                    count = restored,
                    "[ClusterAdapter] Re-registered {} pending cluster tasks from continuation snapshots",
                    restored
                );
            }
        }

        // Build and spawn cluster agent loop
        let rpc_client = self.cluster.rpc_client_arc();
        let cluster_arc = self.cluster.clone();
        let result_persister = self.result_persister.clone();
        // 讨论通道：每次 start 造新管道换入 inbox（stop/start 周期安全）。
        let discussion_rx = match &self.discussion_inbox {
            Some(inbox) => {
                let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
                inbox.set_sender(tx);
                Some(rx)
            }
            None => None,
        };
        let self_node_id = self.cluster.node_id().to_string();
        let handle = match crate::agent_factory::build_cluster_agent_loop(&self.shared, cluster_arc)
        {
            Ok((cluster_agent, cluster_config, cluster_observer)) => {
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
                        cluster_observer,
                        Some(result_persister),
                        discussion_rx,
                        self_node_id,
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
                &self.result_persister,
                self.discussion_inbox.as_ref(),
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
// G1: rpc_cache/results TTL sweep + disk reload
// ---------------------------------------------------------------------------

/// G1（2026-09-01 集群韧性 goal；2026-09-11 提为自由函数以便链路测试）：
/// rpc_cache/results 7 天 TTL 清扫 + 磁盘结果回载（A 崩溃前本地完成的任务
/// 结果，轮询恢复时可直接命中）。返回 (清扫数, 回载数)。
///
/// 清扫判据是文件内 JSON 的 `stored_at`（RFC3339）；解析失败 fail-open 保留。
fn sweep_and_reload_stale_results(
    result_store: &nemesis_cluster::task_result_store::TaskResultStore,
) -> (usize, usize) {
    let swept = result_store.sweep_older_than(chrono::Duration::days(7));
    if swept > 0 {
        tracing::info!(
            count = swept,
            "[ClusterAdapter] Swept {} stale rpc_cache result files",
            swept
        );
    }
    let loaded = result_store.load_from_disk();
    if loaded > 0 {
        tracing::info!(
            count = loaded,
            "[ClusterAdapter] Loaded {} task results from disk",
            loaded
        );
    }
    (swept, loaded)
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
    result_persister: &Arc<dyn nemesis_cluster::rpc::peer_chat_handler::TaskResultPersister>,
    discussion_inbox: Option<&Arc<crate::cluster_agent::DiscussionInbox>>,
    shutdown_tx: tokio::sync::broadcast::Sender<()>,
) -> Result<Option<tokio::task::JoinHandle<()>>, String> {
    // 1. Start cluster (registers local node, creates RPC client, starts sync/recovery loops)
    cluster.start();
    tracing::info!("[ClusterComponents] cluster.start() done");

    // 2. Start RPC server (bind TCP listener + accept loop)
    if let Some(server) = cluster.rpc_server() {
        let server = server.clone();
        server
            .start()
            .await
            .map_err(|e| format!("RPC server start: {}", e))?;
        tracing::info!("[ClusterComponents] RPC server started");
    }

    // 3. Start UDP discovery
    cluster.start_discovery(cluster.clone());
    tracing::info!("[ClusterComponents] Discovery started");

    // 4. Build cluster agent loop and spawn
    let rpc_client = cluster.rpc_client_arc();
    let cluster_arc = cluster.clone();
    // 讨论通道：每次 start 造新管道换入 inbox（stop/start 周期安全）。
    let discussion_rx = match discussion_inbox {
        Some(inbox) => {
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            inbox.set_sender(tx);
            Some(rx)
        }
        None => None,
    };
    let self_node_id = cluster.node_id().to_string();
    match crate::agent_factory::build_cluster_agent_loop(shared, cluster_arc) {
        Ok((cluster_agent, cluster_config, cluster_observer)) => {
            let shutdown_rx = shutdown_tx.subscribe();
            let work_queue = cluster_work_queue.clone();
            let task_list = cluster_task_list.clone();
            let result_persister = result_persister.clone();
            let handle = tokio::spawn(async move {
                crate::cluster_agent::cluster_agent_loop(
                    cluster_agent,
                    cluster_config,
                    work_queue,
                    task_list,
                    rpc_client,
                    cluster_observer,
                    Some(result_persister),
                    discussion_rx,
                    self_node_id,
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

#[cfg(test)]
mod tests;
