//! Cluster handler — runtime status, nodes, tasks, topology, snapshots, config.
//!
//! Phase 2: 8 new backend commands + pagination + config validation.
//! Phase 3: Placeholder data filled from cluster log reader.
//! Phase 4: SSE bridge is wired in gateway.rs via `set_cluster_log_hook`.

use crate::handlers::{require_home, require_workspace};
use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_cluster::cluster::Cluster;
use nemesis_cluster::cluster_log_reader;
use nemesis_types::cluster::{NodeRole, TaskStatus};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct ClusterHandler {
    _priv: (),
}

impl ClusterHandler {
    pub fn new() -> Self {
        Self { _priv: () }
    }

    async fn diagnostics_run(
        &self,
        data: &serde_json::Value,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let cluster = require_cluster(ctx)?;
        let node_id = data["node_id"]
            .as_str()
            .ok_or("missing node_id")?;
        let action = data["action"]
            .as_str()
            .ok_or("missing action")?;

        // Use RPC client directly for async call with 10s timeout
        let rpc_client = cluster.rpc_client_arc()
            .ok_or("RPC client not available")?;

        let request = nemesis_cluster::rpc_types::RPCRequest {
            id: uuid::Uuid::new_v4().to_string(),
            action: nemesis_cluster::rpc_types::ActionType::Custom(action.to_string()),
            payload: serde_json::json!({}),
            source: cluster.node_id().to_string(),
            target: Some(node_id.to_string()),
        };

        let response = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            rpc_client.call_with_timeout(node_id, request, std::time::Duration::from_secs(10)),
        )
        .await
        .map_err(|_| "diagnostics timeout (10s)".to_string())?
        .map_err(|e| format!("RPC call failed: {}", e))?;

        if let Some(err) = response.error {
            return Err(err);
        }

        Ok(response.result)
    }
}

#[async_trait::async_trait]
impl ModuleHandler for ClusterHandler {
    fn module_name(&self) -> &str {
        "cluster"
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        match cmd {
            // Runtime data commands — need Cluster instance
            "runtime.status" => self.runtime_status(ctx),
            "runtime.start" => self.runtime_start(ctx),
            "runtime.stop" => self.runtime_stop(ctx),
            "nodes.list" => self.nodes_list(ctx),
            "nodes.ping" => {
                let data = data.ok_or("missing data")?;
                let node_id = data["node_id"]
                    .as_str()
                    .ok_or("missing node_id")?;
                self.nodes_ping(node_id, ctx).await
            }
            "nodes.remove" => {
                let data = data.ok_or("missing data")?;
                let node_id = data["node_id"]
                    .as_str()
                    .ok_or("missing node_id")?;
                self.nodes_remove(node_id, ctx)
            }
            "nodes.add" => {
                let data = data.ok_or("missing data")?;
                self.nodes_add(&data, ctx)
            }
            "nodes.detail" => {
                let data = data.ok_or("missing data")?;
                let node_id = data["node_id"]
                    .as_str()
                    .ok_or("missing node_id")?;
                self.nodes_detail(node_id, ctx)
            }
            "tasks.list" => {
                let status_filter = data
                    .as_ref()
                    .and_then(|d| d["status_filter"].as_str().map(String::from));
                let offset = data
                    .as_ref()
                    .and_then(|d| d["offset"].as_u64())
                    .map(|v| v as usize);
                let limit = data
                    .as_ref()
                    .and_then(|d| d["limit"].as_u64())
                    .map(|v| v as usize);
                self.tasks_list(status_filter.as_deref(), offset, limit, ctx)
            }
            "tasks.cancel" => {
                let data = data.ok_or("missing data")?;
                let task_id = data["task_id"]
                    .as_str()
                    .ok_or("missing task_id")?;
                self.tasks_cancel(task_id, ctx)
            }
            "tasks.detail" => {
                let data = data.ok_or("missing data")?;
                let task_id = data["task_id"]
                    .as_str()
                    .ok_or("missing task_id")?;
                self.tasks_detail(task_id, ctx)
            }
            "tasks.submit" => {
                let data = data.ok_or("missing data")?;
                self.tasks_submit(&data, ctx)
            }
            "topology" => self.topology(ctx),
            "traces" => self.traces(ctx),
            "events.recent" => self.events_recent(data.as_ref(), ctx),
            "snapshots.list" => self.snapshots_list(ctx).await,
            "snapshots.cleanup" => self.snapshots_cleanup(ctx).await,

            // Config file operations — workspace-based, no Cluster needed
            "status" => self.legacy_status(ctx),
            "config.get" => self.config_get(ctx),
            "config.save" => {
                let data = data.ok_or("missing data")?;
                self.config_save(ctx, &data)
            }
            "config.set_master_enabled" => {
                let data = data.ok_or("missing data")?;
                self.config_set_master_enabled(ctx, &data)
            }
            "node.update_identity" => {
                let data = data.ok_or("missing data")?;
                self.node_update_identity(&data, ctx)
            }
            "identity.get_files" => self.identity_get_files(ctx),
            "identity.save_file" => {
                let data = data.ok_or("missing data")?;
                self.identity_save_file(&data, ctx)
            }
            "peers" => self.peers(ctx),
            "firewall.check" => self.firewall_check(ctx),
            "firewall.add_rules" => {
                let data = data.ok_or("missing data")?;
                self.firewall_add_rules(&data, ctx)
            }
            "diagnostics.run" => {
                let data = data.ok_or("missing data")?;
                self.diagnostics_run(&data, ctx).await
            }
            _ => Err(format!("unknown command: cluster.{}", cmd)),
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Get the Cluster instance from AppState, or return error if not injected.
fn require_cluster(ctx: &RequestContext) -> Result<Arc<Cluster>, String> {
    ctx.state
        .cluster
        .clone()
        .ok_or_else(|| "cluster not available".to_string())
}

/// Get the cluster log directory, or return error if not configured.
fn require_log_dir(ctx: &RequestContext) -> Result<String, String> {
    ctx.state
        .cluster_log_dir
        .clone()
        .ok_or_else(|| "cluster log directory not configured".to_string())
}

/// Format a duration as human-readable string (e.g. "2d 3h", "45m", "12s").
fn format_duration(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs == 0 {
        return "0s".to_string();
    }
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    let s = secs % 60;

    if days > 0 {
        format!("{}d {}h", days, hours)
    } else if hours > 0 {
        format!("{}h {}m", hours, mins)
    } else if mins > 0 {
        format!("{}m {}s", mins, s)
    } else {
        format!("{}s", s)
    }
}

fn cluster_config_path(workspace: &str) -> PathBuf {
    PathBuf::from(workspace).join("config/config.cluster.json")
}

fn peers_path(workspace: &str) -> PathBuf {
    PathBuf::from(workspace).join("cluster/peers.toml")
}

// ---------------------------------------------------------------------------
// Runtime data commands
// ---------------------------------------------------------------------------

impl ClusterHandler {
    fn runtime_status(&self, ctx: &RequestContext) -> Result<Option<serde_json::Value>, String> {
        let workspace = require_workspace(ctx)?;

        // Use adapter's running state (reflects actual start/stop from Dashboard),
        // not cluster.is_running() which is true once cluster.start() is called at init.
        let running = ctx.state.cluster_service.as_ref()
            .map(|s| nemesis_services::bot_service::LifecycleService::is_running(s.as_ref()))
            .unwrap_or(false);

        // Try Cluster runtime first
        if let Ok(cluster) = require_cluster(ctx) {
            let nodes = cluster.list_nodes();
            let tasks = cluster.list_tasks();

            let online_nodes = nodes.iter().filter(|n| n.is_online()).count();
            let total_nodes = nodes.len();
            let active_tasks = tasks
                .iter()
                .filter(|t| matches!(t.status, TaskStatus::Pending | TaskStatus::Running))
                .count();

            let completed: Vec<_> = tasks
                .iter()
                .filter(|t| t.status == TaskStatus::Completed)
                .collect();
            let failed_count = tasks
                .iter()
                .filter(|t| t.status == TaskStatus::Failed)
                .count();
            let total_tasks = tasks.len();

            // Today completed
            let today = chrono::Local::now().format("%Y-%m-%d").to_string();
            let today_completed = completed
                .iter()
                .filter(|t| {
                    t.completed_at
                        .as_ref()
                        .map(|d| d.starts_with(&today))
                        .unwrap_or(false)
                })
                .count();

            // Success rate
            let success_rate = if completed.is_empty() && failed_count == 0 {
                1.0
            } else {
                let denom = completed.len() + failed_count;
                if denom == 0 {
                    1.0
                } else {
                    completed.len() as f64 / denom as f64
                }
            };

            // Average duration
            let avg_duration = {
                let durations: Vec<f64> = completed
                    .iter()
                    .filter_map(|t| {
                        let created = chrono::DateTime::parse_from_rfc3339(&t.created_at).ok()?;
                        let completed_str = t.completed_at.as_ref()?;
                        let done = chrono::DateTime::parse_from_rfc3339(completed_str).ok()?;
                        Some((done - created).num_seconds() as f64)
                    })
                    .collect();
                if durations.is_empty() {
                    "--".to_string()
                } else {
                    let avg = durations.iter().sum::<f64>() / durations.len() as f64;
                    format_duration(std::time::Duration::from_secs_f64(avg))
                }
            };

            // Health score: online_rate*40 + success_rate*40 + activity*20
            let online_rate = if total_nodes > 0 {
                online_nodes as f64 / total_nodes as f64
            } else {
                0.0
            };
            let activity = if active_tasks > 0 { 1.0 } else { 0.5 };
            let health_score = ((online_rate * 40.0 + success_rate * 40.0 + activity * 20.0) as i32)
                .clamp(0, 100);

            // Phase 3: Fill recent_events from log reader
            let recent_events = match require_log_dir(ctx) {
                Ok(log_dir) => {
                    let events = cluster_log_reader::read_recent_events(
                        Path::new(&log_dir),
                        20,
                    );
                    serde_json::to_value(events).unwrap_or(serde_json::json!([]))
                }
                Err(_) => serde_json::json!([]),
            };

            return Ok(Some(serde_json::json!({
                "running": running,
                "health_score": health_score,
                "online_nodes": online_nodes,
                "total_nodes": total_nodes,
                "active_tasks": active_tasks,
                "today_completed": today_completed,
                "total_tasks": total_tasks,
                "success_rate": success_rate,
                "avg_duration": avg_duration,
                "recent_events": recent_events,
            })));
        }

        // Fallback: read from config files (no Cluster runtime)
        let config_path = cluster_config_path(workspace);
        let config = if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)
                .map_err(|e| format!("failed to read cluster config: {}", e))?;
            serde_json::from_str::<serde_json::Value>(&content).ok()
        } else {
            None
        };

        let peers_path = peers_path(workspace);
        let mut peers_count: usize = 0;
        let mut node_role: Option<String> = None;
        let mut node_name: Option<String> = None;
        if peers_path.exists() {
            let content = std::fs::read_to_string(&peers_path).unwrap_or_default();
            peers_count = content
                .lines()
                .filter(|l| l.starts_with("[peers.") && l.ends_with(']'))
                .count();
            let mut in_node = false;
            for line in content.lines() {
                if line.trim() == "[node]" {
                    in_node = true;
                    continue;
                }
                if in_node {
                    if line.starts_with('[') {
                        break;
                    }
                    if let Some(val) = line.strip_prefix("role") {
                        let val = val.trim().trim_start_matches('=').trim().trim_matches('"');
                        if !val.is_empty() {
                            node_role = Some(val.to_string());
                        }
                    }
                    if let Some(val) = line.strip_prefix("name") {
                        let val = val.trim().trim_start_matches('=').trim().trim_matches('"');
                        if !val.is_empty() {
                            node_name = Some(val.to_string());
                        }
                    }
                }
            }
        }

        Ok(Some(serde_json::json!({
            "running": false,
            "health_score": 0,
            "online_nodes": 0,
            "total_nodes": peers_count,
            "active_tasks": 0,
            "today_completed": 0,
            "total_tasks": 0,
            "success_rate": 0.0,
            "avg_duration": "--",
            "recent_events": [],
            // Legacy fields for backward compat
            "config": config,
            "peers_count": peers_count,
            "config_exists": config_path.exists(),
            "role": node_role,
            "node_name": node_name,
        })))
    }

    // -- Phase 2: runtime.start / runtime.stop --------------------------------

    fn runtime_start(&self, ctx: &RequestContext) -> Result<Option<serde_json::Value>, String> {
        // Persist enabled=true to config.cluster.json before starting runtime
        if let Ok(workspace) = require_workspace(ctx) {
            let _ = Self::update_cluster_config_enabled(workspace, true);
        }
        let svc = ctx.state.cluster_service.as_ref()
            .ok_or("cluster service not available")?;
        svc.start().map_err(|e| format!("start failed: {}", e))?;
        Ok(Some(serde_json::json!({ "started": true })))
    }

    fn runtime_stop(&self, ctx: &RequestContext) -> Result<Option<serde_json::Value>, String> {
        let svc = ctx.state.cluster_service.as_ref()
            .ok_or("cluster service not available")?;
        svc.stop().map_err(|e| format!("stop failed: {}", e))?;
        // Persist enabled=false to config.cluster.json after stopping
        if let Ok(workspace) = require_workspace(ctx) {
            let _ = Self::update_cluster_config_enabled(workspace, false);
        }
        Ok(Some(serde_json::json!({ "stopped": true })))
    }

    // -- Node commands --------------------------------------------------------

    fn nodes_list(&self, ctx: &RequestContext) -> Result<Option<serde_json::Value>, String> {
        let cluster = require_cluster(ctx)?;
        let nodes = cluster.list_nodes();
        let local_node_id = cluster.node_id();

        // Phase 3: Get per-node stats from log reader
        let node_stats_map = match require_log_dir(ctx) {
            Ok(log_dir) => {
                cluster_log_reader::aggregate_node_stats(Path::new(&log_dir))
            }
            Err(_) => std::collections::HashMap::new(),
        };

        let result: Vec<serde_json::Value> = nodes
            .iter()
            .map(|n| {
                let role = match n.base.role {
                    NodeRole::Master => "manager",
                    NodeRole::Worker => "worker",
                };
                let uptime = format_duration(n.get_uptime());

                // Phase 3: Fill taskCount and successRate from log reader
                let stats = node_stats_map.get(&n.base.id);
                let task_count = stats.map(|s| s.task_count);
                let success_rate = stats.map(|s| s.success_rate);

                serde_json::json!({
                    "id": n.base.id,
                    "name": n.base.name,
                    "role": role,
                    "address": n.base.address,
                    "category": n.base.category,
                    "tags": [],
                    "capabilities": n.capabilities,
                    "online": n.is_online(),
                    "lastSeen": n.base.last_seen,
                    "taskCount": task_count,
                    "successRate": success_rate,
                    "uptime": uptime,
                    "isLocal": n.base.id == local_node_id,
                })
            })
            .collect();

        Ok(Some(serde_json::json!({ "nodes": result })))
    }

    async fn nodes_ping(
        &self,
        node_id: &str,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let cluster = require_cluster(ctx)?;
        let node = cluster
            .get_node_info(node_id)
            .ok_or_else(|| format!("node not found: {}", node_id))?;

        // TCP connect to measure latency
        let addr = node.base.address.clone();
        let start = std::time::Instant::now();
        match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            tokio::net::TcpStream::connect(&addr),
        )
        .await
        {
            Ok(Ok(_)) => {
                let latency = start.elapsed().as_millis() as u64;
                Ok(Some(serde_json::json!({ "latency": latency })))
            }
            Ok(Err(e)) => Err(format!("ping failed: {}", e)),
            Err(_) => Err("ping timeout (5s)".to_string()),
        }
    }

    fn nodes_remove(
        &self,
        node_id: &str,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let cluster = require_cluster(ctx)?;
        let ok = cluster.remove_node(node_id);
        Ok(Some(serde_json::json!({ "removed": ok })))
    }

    /// Phase 2: Add a peer node by writing to peers.toml.
    fn nodes_add(
        &self,
        data: &serde_json::Value,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let workspace = require_workspace(ctx)?;
        let address = data["address"]
            .as_str()
            .ok_or("missing address")?
            .to_string();
        let name = data["name"].as_str().unwrap_or("").to_string();
        let role = data["role"].as_str().unwrap_or("worker").to_string();
        let category = data["category"].as_str().unwrap_or("general").to_string();

        let ppath = peers_path(workspace);
        let mut config = if ppath.exists() {
            nemesis_cluster::cluster_config::load_static_config(&ppath)
                .map_err(|e| format!("failed to load peers.toml: {}", e))?
        } else {
            // No existing peers.toml — create a minimal config with empty fields.
            // StaticConfig does not derive Default, so we construct manually.
            nemesis_cluster::cluster_config::StaticConfig {
                cluster: nemesis_cluster::cluster_config::ClusterMeta::default(),
                node: nemesis_cluster::cluster_config::NodeInfo::default(),
                peers: Vec::new(),
            }
        };

        // Build a new PeerConfig
        let peer = nemesis_cluster::cluster_config::PeerConfig {
            id: if !name.is_empty() {
                name.clone()
            } else {
                format!("peer-{}", config.peers.len() + 1)
            },
            name: name.clone(),
            address: address.clone(),
            rpc_port: 0, // will be parsed from address or auto-detected
            role: role.clone(),
            category: category.clone(),
            enabled: true,
            ..Default::default()
        };

        config.peers.push(peer);

        nemesis_cluster::cluster_config::save_static_config(&ppath, &config)
            .map_err(|e| format!("failed to save peers.toml: {}", e))?;

        // If cluster is running, try to register the node
        if let Ok(cluster) = require_cluster(ctx) {
            let node_id = if !name.is_empty() {
                name.clone()
            } else {
                address.clone()
            };
            let info = nemesis_cluster::types::ExtendedNodeInfo {
                base: nemesis_types::cluster::NodeInfo {
                    id: node_id.clone(),
                    name: name.clone(),
                    role: if role == "manager" || role == "master" {
                        NodeRole::Master
                    } else {
                        NodeRole::Worker
                    },
                    address: address.clone(),
                    category: category.clone(),
                    last_seen: String::new(),
                },
                status: nemesis_cluster::types::NodeStatus::Offline,
                capabilities: Vec::new(),
                addresses: Vec::new(),
                node_type: String::new(),
            };
            cluster.register_node(info);
        }

        Ok(Some(serde_json::json!({ "added": true })))
    }

    /// Phase 2: Get node detail enriched with log stats.
    fn nodes_detail(
        &self,
        node_id: &str,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let cluster = require_cluster(ctx)?;
        let node = cluster
            .get_node_info(node_id)
            .ok_or_else(|| format!("node not found: {}", node_id))?;

        let role = match node.base.role {
            NodeRole::Master => "manager",
            NodeRole::Worker => "worker",
        };
        let uptime = format_duration(node.get_uptime());

        // Enrich with log stats
        let stats = match require_log_dir(ctx) {
            Ok(log_dir) => {
                let map =
                    cluster_log_reader::aggregate_node_stats(Path::new(&log_dir));
                map.get(node_id).cloned()
            }
            Err(_) => None,
        };

        Ok(Some(serde_json::json!({
            "id": node.base.id,
            "name": node.base.name,
            "role": role,
            "address": node.base.address,
            "category": node.base.category,
            "capabilities": node.capabilities,
            "online": node.is_online(),
            "lastSeen": node.base.last_seen,
            "uptime": uptime,
            "taskCount": stats.as_ref().map(|s| s.task_count),
            "successCount": stats.as_ref().map(|s| s.success_count),
            "failCount": stats.as_ref().map(|s| s.fail_count),
            "successRate": stats.as_ref().map(|s| s.success_rate),
        })))
    }

    /// Update runtime identity (name, role, category, tags) and persist to peers.toml.
    fn node_update_identity(
        &self,
        data: &serde_json::Value,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let cluster = require_cluster(ctx)?;
        let mut updated = serde_json::json!({});

        if let Some(name) = data.get("name").and_then(|v| v.as_str()) {
            let name = name.trim().to_string();
            if name.is_empty() {
                return Err("name cannot be empty".to_string());
            }
            cluster.set_node_name(&name);
            updated["name"] = serde_json::json!(name);
        }
        if let Some(role) = data.get("role").and_then(|v| v.as_str()) {
            let role = role.trim().to_string();
            if role != "manager" && role != "worker" {
                return Err("role must be 'manager' or 'worker'".to_string());
            }
            cluster.set_role(&role);
            updated["role"] = serde_json::json!(role);
        }
        if let Some(category) = data.get("category").and_then(|v| v.as_str()) {
            let category = category.trim().to_string();
            if category.is_empty() {
                return Err("category cannot be empty".to_string());
            }
            cluster.set_category(&category);
            updated["category"] = serde_json::json!(category);
        }
        if let Some(tags_arr) = data.get("tags").and_then(|v| v.as_array()) {
            let tag_list: Vec<String> = tags_arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect();
            cluster.set_tags(tag_list.clone());
            updated["tags"] = serde_json::json!(tag_list);
        }

        // Persist to peers.toml [node] section
        if let Ok(workspace) = require_workspace(ctx) {
            let ppath = peers_path(workspace);
            let mut config = if ppath.exists() {
                nemesis_cluster::cluster_config::load_static_config(&ppath)
                    .map_err(|e| format!("failed to load peers.toml: {}", e))?
            } else {
                nemesis_cluster::cluster_config::StaticConfig {
                    cluster: nemesis_cluster::cluster_config::ClusterMeta::default(),
                    node: nemesis_cluster::cluster_config::NodeInfo::default(),
                    peers: Vec::new(),
                }
            };
            if let Some(name) = data.get("name").and_then(|v| v.as_str()) {
                config.node.name = name.trim().to_string();
            }
            if let Some(role) = data.get("role").and_then(|v| v.as_str()) {
                config.node.role = role.trim().to_string();
            }
            if let Some(category) = data.get("category").and_then(|v| v.as_str()) {
                config.node.category = category.trim().to_string();
            }
            if let Some(tags_arr) = data.get("tags").and_then(|v| v.as_array()) {
                config.node.tags = tags_arr
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
                    .filter(|s| !s.is_empty())
                    .collect();
            }
            if let Some(parent) = ppath.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            nemesis_cluster::cluster_config::save_static_config(&ppath, &config)
                .map_err(|e| format!("failed to save peers.toml: {}", e))?;

            // Also sync identity fields to config.cluster.json so gateway.rs
            // picks up the updated name on restart (cluster init writes name there).
            let ccfg_path = cluster_config_path(workspace);
            if ccfg_path.exists() {
                if let Ok(content) = std::fs::read_to_string(&ccfg_path) {
                    if let Ok(mut ccfg) = serde_json::from_str::<serde_json::Value>(&content) {
                        if let Some(obj) = ccfg.as_object_mut() {
                            if let Some(name) = data.get("name").and_then(|v| v.as_str()) {
                                obj.insert("name".into(), serde_json::json!(name.trim()));
                            }
                            if let Some(role) = data.get("role").and_then(|v| v.as_str()) {
                                obj.insert("role".into(), serde_json::json!(role.trim()));
                            }
                            if let Some(category) = data.get("category").and_then(|v| v.as_str()) {
                                obj.insert("category".into(), serde_json::json!(category.trim()));
                            }
                            if let Some(tags_arr) = data.get("tags").and_then(|v| v.as_array()) {
                                obj.insert("tags".into(), serde_json::json!(tags_arr));
                            }
                        }
                        if let Ok(json) = serde_json::to_string_pretty(&ccfg) {
                            let _ = std::fs::write(&ccfg_path, json);
                        }
                    }
                }
            }
        }

        // Return current values
        updated["current_name"] = serde_json::json!(cluster.node_name());
        updated["current_role"] = serde_json::json!(cluster.role());
        updated["current_category"] = serde_json::json!(cluster.category());
        updated["current_tags"] = serde_json::json!(cluster.tags());
        updated["current_node_type"] = serde_json::json!(cluster.node_type());
        let caps = cluster.get_capabilities();
        updated["current_capabilities"] = serde_json::json!(caps);

        Ok(Some(updated))
    }

    // -- Task commands --------------------------------------------------------

    fn tasks_list(
        &self,
        status_filter: Option<&str>,
        offset: Option<usize>,
        limit: Option<usize>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let cluster = require_cluster(ctx)?;
        let tasks = cluster.list_tasks();

        // Map backend status to frontend status
        let map_status = |s: TaskStatus| -> &'static str {
            match s {
                TaskStatus::Pending => "queued",
                TaskStatus::Running => "running",
                TaskStatus::Completed => "completed",
                TaskStatus::Failed => "failed",
                TaskStatus::Cancelled => "failed", // cancelled treated as failed
            }
        };

        // Filter
        let filtered: Vec<_> = tasks
            .iter()
            .filter(|t| {
                if let Some(filter) = status_filter {
                    map_status(t.status) == filter
                } else {
                    true
                }
            })
            .collect();

        // Build stats
        let mut queued = 0u32;
        let mut running = 0u32;
        let mut completed = 0u32;
        let mut failed = 0u32;
        for t in &tasks {
            match t.status {
                TaskStatus::Pending => queued += 1,
                TaskStatus::Running => running += 1,
                TaskStatus::Completed => completed += 1,
                TaskStatus::Failed | TaskStatus::Cancelled => failed += 1,
            }
        }

        // Phase 3: Batch-enrich all tasks with log reader data
        let task_summaries = match require_log_dir(ctx) {
            Ok(log_dir) => {
                let all_task_ids: Vec<String> = filtered.iter().map(|t| t.id.clone()).collect();
                if all_task_ids.is_empty() {
                    std::collections::HashMap::new()
                } else {
                    cluster_log_reader::aggregate_task_summaries(
                        Path::new(&log_dir),
                        &all_task_ids,
                    )
                }
            }
            Err(_) => std::collections::HashMap::new(),
        };

        let total = filtered.len();
        let result: Vec<serde_json::Value> = filtered
            .iter()
            .map(|t| {
                let duration = match (&t.created_at, &t.completed_at) {
                    (created, Some(completed_str)) => {
                        let c = chrono::DateTime::parse_from_rfc3339(created).ok();
                        let d = chrono::DateTime::parse_from_rfc3339(completed_str).ok();
                        match (c, d) {
                            (Some(c), Some(d)) => {
                                let secs = (d - c).num_seconds();
                                serde_json::json!(format!("{}s", secs))
                            }
                            _ => serde_json::Value::Null,
                        }
                    }
                    _ => serde_json::Value::Null,
                };

                let input = if t.payload.is_string() {
                    t.payload.as_str().unwrap_or("").to_string()
                } else {
                    t.payload.to_string()
                };

                // Phase 3: Fill rounds, toolCalls, toolChain from log reader
                let summary = task_summaries.get(&t.id);
                let rounds = summary.map(|s| s.rounds);
                let tool_calls = summary.map(|s| s.tool_calls);
                let tool_chain = summary.map(|s| {
                    s.tool_chain
                        .iter()
                        .map(|t| serde_json::Value::String(t.clone()))
                        .collect::<Vec<_>>()
                });

                serde_json::json!({
                    "id": t.id,
                    "status": map_status(t.status),
                    "source": t.original_channel,
                    "target": t.peer_id,
                    "input": truncate_str(&input, 200),
                    "duration": duration,
                    "rounds": rounds,
                    "toolCalls": tool_calls,
                    "toolChain": tool_chain,
                })
            })
            .collect();

        // Phase 2: Apply pagination
        let offset_val = offset.unwrap_or(0);
        let limit_val = limit.unwrap_or(result.len());
        let paginated: Vec<&serde_json::Value> = result
            .iter()
            .skip(offset_val)
            .take(limit_val)
            .collect();

        Ok(Some(serde_json::json!({
            "tasks": paginated,
            "total": total,
            "offset": offset_val,
            "limit": limit_val,
            "stats": {
                "queued": queued,
                "running": running,
                "completed": completed,
                "failed": failed,
            }
        })))
    }

    fn tasks_cancel(
        &self,
        task_id: &str,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let cluster = require_cluster(ctx)?;
        let ok = cluster.task_manager().delete_task(task_id);
        Ok(Some(serde_json::json!({ "cancelled": ok })))
    }

    /// Phase 2: Get task detail enriched with execution summary.
    fn tasks_detail(
        &self,
        task_id: &str,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let cluster = require_cluster(ctx)?;
        let task = cluster
            .get_task(task_id)
            .ok_or_else(|| format!("task not found: {}", task_id))?;

        let map_status = |s: TaskStatus| -> &'static str {
            match s {
                TaskStatus::Pending => "queued",
                TaskStatus::Running => "running",
                TaskStatus::Completed => "completed",
                TaskStatus::Failed => "failed",
                TaskStatus::Cancelled => "failed",
            }
        };

        // Enrich with log reader data
        let summary = match require_log_dir(ctx) {
            Ok(log_dir) => {
                let ids = vec![task_id.to_string()];
                let map = cluster_log_reader::aggregate_task_summaries(
                    Path::new(&log_dir),
                    &ids,
                );
                map.get(task_id).cloned()
            }
            Err(_) => None,
        };

        Ok(Some(serde_json::json!({
            "id": task.id,
            "status": map_status(task.status),
            "action": task.action,
            "source": task.original_channel,
            "target": task.peer_id,
            "payload": task.payload,
            "result": task.result,
            "createdAt": task.created_at,
            "completedAt": task.completed_at,
            "rounds": summary.as_ref().map(|s| s.rounds),
            "toolCalls": summary.as_ref().map(|s| s.tool_calls),
            "toolChain": summary.as_ref().map(|s| s.tool_chain.clone()),
        })))
    }

    /// Phase 2: Submit a new task or peer_chat.
    fn tasks_submit(
        &self,
        data: &serde_json::Value,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let cluster = require_cluster(ctx)?;
        let content = data["content"]
            .as_str()
            .ok_or("missing content")?
            .to_string();
        let target_node_id = data["target_node_id"].as_str();

        let task_id = if let Some(target) = target_node_id {
            // Submit peer_chat to a specific node
            let payload = serde_json::json!({
                "content": content,
            });
            cluster.submit_peer_chat(
                target,
                "dashboard_test",
                payload,
                "dashboard",
                "dashboard_session",
            )?
        } else {
            // Submit a regular task
            let payload = serde_json::json!({
                "content": content,
            });
            cluster.submit_task(
                "dashboard_test",
                payload,
                "dashboard",
                "dashboard_session",
            )
        };

        Ok(Some(serde_json::json!({
            "task_id": task_id,
            "submitted": true,
        })))
    }

    // -- Topology -------------------------------------------------------------

    fn topology(&self, ctx: &RequestContext) -> Result<Option<serde_json::Value>, String> {
        let cluster = require_cluster(ctx)?;
        let nodes = cluster.list_nodes();

        let node_list: Vec<serde_json::Value> = nodes
            .iter()
            .map(|n| {
                let role = match n.base.role {
                    NodeRole::Master => "manager",
                    NodeRole::Worker => "worker",
                };
                serde_json::json!({
                    "id": n.base.id,
                    "name": n.base.name,
                    "role": role,
                    "online": n.is_online(),
                })
            })
            .collect();

        // Phase 3: Replace full-mesh with real RPC connections from log reader
        let connections: Vec<serde_json::Value> = match require_log_dir(ctx) {
            Ok(log_dir) => {
                let rpc_conns =
                    cluster_log_reader::read_rpc_connections(Path::new(&log_dir));
                rpc_conns
                    .iter()
                    .map(|c| {
                        serde_json::json!({
                            "from": c.from,
                            "to": c.to,
                            "active": true,
                            "lastSeen": c.last_seen,
                        })
                    })
                    .collect()
            }
            Err(_) => {
                // Fallback: full mesh for online nodes
                let online_ids: Vec<&str> = nodes
                    .iter()
                    .filter(|n| n.is_online())
                    .map(|n| n.base.id.as_str())
                    .collect();
                let mut conns = Vec::new();
                for i in 0..online_ids.len() {
                    for j in (i + 1)..online_ids.len() {
                        conns.push(serde_json::json!({
                            "from": online_ids[i],
                            "to": online_ids[j],
                            "active": true,
                        }));
                    }
                }
                conns
            }
        };

        // Phase 3: Fill traces from log reader
        let traces = match require_log_dir(ctx) {
            Ok(log_dir) => {
                let t = cluster_log_reader::reconstruct_traces(Path::new(&log_dir));
                serde_json::to_value(t).unwrap_or(serde_json::json!([]))
            }
            Err(_) => serde_json::json!([]),
        };

        Ok(Some(serde_json::json!({
            "nodes": node_list,
            "connections": connections,
            "traces": traces,
        })))
    }

    /// Phase 2: Get RPC traces.
    fn traces(&self, ctx: &RequestContext) -> Result<Option<serde_json::Value>, String> {
        let log_dir = require_log_dir(ctx)?;
        let traces =
            cluster_log_reader::reconstruct_traces(Path::new(&log_dir));
        Ok(Some(serde_json::json!({ "traces": traces })))
    }

    /// Phase 2: Get recent events for the ActivityFeed.
    fn events_recent(
        &self,
        data: Option<&serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let log_dir = require_log_dir(ctx)?;
        let limit = data
            .and_then(|d| d["limit"].as_u64())
            .map(|v| v as usize)
            .unwrap_or(50);
        let events = cluster_log_reader::read_recent_events(
            Path::new(&log_dir),
            limit,
        );
        Ok(Some(serde_json::json!({ "events": events })))
    }

    async fn snapshots_list(&self, ctx: &RequestContext) -> Result<Option<serde_json::Value>, String> {
        let cluster = require_cluster(ctx)?;
        let cont_store = cluster.continuation_store();

        let _task_ids = cont_store.list_pending().await;
        let cache_dir = cont_store.cache_dir();

        let mut snapshots = Vec::new();
        if cache_dir.exists() {
            let mut entries = std::fs::read_dir(cache_dir)
                .map_err(|e| format!("failed to read cache dir: {}", e))?;
            while let Some(entry) = entries.next() {
                let entry = entry.map_err(|e| format!("failed to read entry: {}", e))?;
                let path = entry.path();
                if path.extension().map(|e| e == "json").unwrap_or(false) {
                    let name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("")
                        .to_string();
                    let size = entry
                        .metadata()
                        .map(|m| m.len())
                        .unwrap_or(0);
                    let created = entry
                        .metadata()
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .map(|t| {
                            let elapsed = t.elapsed().unwrap_or_default();
                            format_ago(elapsed)
                        })
                        .unwrap_or_else(|| "unknown".to_string());

                    snapshots.push(serde_json::json!({
                        "name": name,
                        "size": format_bytes(size),
                        "created": created,
                    }));
                }
            }
        }

        Ok(Some(serde_json::json!({ "snapshots": snapshots })))
    }

    async fn snapshots_cleanup(&self, ctx: &RequestContext) -> Result<Option<serde_json::Value>, String> {
        let cluster = require_cluster(ctx)?;
        let cont_store = cluster.continuation_store();

        // Cleanup all snapshots (max_age = 0 means remove everything)
        let removed = cont_store
            .cleanup_old(std::time::Duration::ZERO)
            .await
            .map_err(|e| format!("cleanup failed: {}", e))?;

        Ok(Some(serde_json::json!({ "removed": removed })))
    }
}

// ---------------------------------------------------------------------------
// Config file operations (no Cluster needed)
// ---------------------------------------------------------------------------

impl ClusterHandler {
    fn legacy_status(&self, ctx: &RequestContext) -> Result<Option<serde_json::Value>, String> {
        // Redirect to runtime.status which handles both cases
        self.runtime_status(ctx)
    }

    fn config_get(&self, ctx: &RequestContext) -> Result<Option<serde_json::Value>, String> {
        let workspace = require_workspace(ctx)?;
        let path = cluster_config_path(workspace);
        let mut config: serde_json::Value = if path.exists() {
            let content = std::fs::read_to_string(&path)
                .map_err(|e| format!("failed to read cluster config: {}", e))?;
            serde_json::from_str(&content)
                .map_err(|e| format!("invalid cluster config: {}", e))?
        } else {
            serde_json::json!({})
        };
        // Also return the master switch status from config.json
        if let Ok(home) = require_home(ctx) {
            let main_cfg_path = PathBuf::from(home).join("config.json");
            let master_enabled = if main_cfg_path.exists() {
                std::fs::read_to_string(&main_cfg_path).ok()
                    .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
                    .and_then(|v| v.get("cluster").and_then(|c| c.get("enabled")).and_then(|e| e.as_bool()))
                    .unwrap_or(false)
            } else {
                false
            };
            if let Some(obj) = config.as_object_mut() {
                obj.insert("master_enabled".to_string(), serde_json::json!(master_enabled));
            }
        }
        // Return node identity from Cluster runtime or peers.toml
        if let Ok(cluster) = require_cluster(ctx) {
            if let Some(obj) = config.as_object_mut() {
                obj.insert("node_id".to_string(), serde_json::json!(cluster.node_id()));
                obj.insert("name".to_string(), serde_json::json!(cluster.node_name()));
                obj.insert("role".to_string(), serde_json::json!(cluster.role()));
                obj.insert("category".to_string(), serde_json::json!(cluster.category()));
                obj.insert("node_type".to_string(), serde_json::json!(cluster.node_type()));
                obj.insert("tags".to_string(), serde_json::json!(cluster.tags()));
                let caps = cluster.get_capabilities();
                obj.insert("capabilities".to_string(), serde_json::json!(caps));
            }
        } else if let Ok(workspace) = require_workspace(ctx) {
            let ppath = peers_path(workspace);
            if ppath.exists() {
                if let Ok(static_cfg) = nemesis_cluster::cluster_config::load_static_config(&ppath) {
                    if let Some(obj) = config.as_object_mut() {
                        obj.insert("node_id".to_string(), serde_json::json!(static_cfg.node.id));
                        obj.insert("name".to_string(), serde_json::json!(static_cfg.node.name));
                        obj.insert("role".to_string(), serde_json::json!(static_cfg.node.role));
                        obj.insert("category".to_string(), serde_json::json!(static_cfg.node.category));
                        obj.insert("tags".to_string(), serde_json::json!(static_cfg.node.tags));
                        obj.insert("capabilities".to_string(), serde_json::json!(static_cfg.node.capabilities));
                    }
                }
            }
        }
        Ok(Some(config))
    }

    fn config_save(
        &self,
        ctx: &RequestContext,
        data: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        let workspace = require_workspace(ctx)?;

        // Phase 2: Config validation
        if let Some(cluster_cfg) = data.get("cluster") {
            let discovery_port = cluster_cfg
                .get("discovery_port")
                .and_then(|v| v.as_u64());
            let rpc_port = cluster_cfg.get("rpc_port").and_then(|v| v.as_u64());

            // Check ports in valid range (1-65535)
            if let Some(port) = discovery_port {
                if port == 0 || port > 65535 {
                    return Err("discovery_port must be between 1 and 65535".to_string());
                }
            }
            if let Some(port) = rpc_port {
                if port == 0 || port > 65535 {
                    return Err("rpc_port must be between 1 and 65535".to_string());
                }
            }

            // Check discovery_port != rpc_port
            if let (Some(dp), Some(rp)) = (discovery_port, rpc_port) {
                if dp == rp {
                    return Err(
                        "discovery_port and rpc_port must be different".to_string()
                    );
                }
            }
        }

        let path = cluster_config_path(workspace);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create config dir: {}", e))?;
        }
        let json = serde_json::to_string_pretty(data)
            .map_err(|e| format!("failed to serialize: {}", e))?;
        std::fs::write(&path, &json)
            .map_err(|e| format!("failed to write cluster config: {}", e))?;

        Ok(Some(serde_json::json!({ "saved": true })))
    }

    /// Update only the master switch in config.json (cluster.enabled).
    /// Does NOT touch config.cluster.json or the runtime.
    fn config_set_master_enabled(
        &self,
        ctx: &RequestContext,
        data: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        let enabled = data.get("enabled")
            .and_then(|v| v.as_bool())
            .ok_or("missing or invalid 'enabled' field")?;
        let home = require_home(ctx)?;
        let main_cfg_path = PathBuf::from(home).join("config.json");
        if !main_cfg_path.exists() {
            return Err("config.json not found".to_string());
        }
        let content = std::fs::read_to_string(&main_cfg_path)
            .map_err(|e| format!("failed to read config.json: {}", e))?;
        let mut main_cfg: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| format!("invalid config.json: {}", e))?;
        // Ensure cluster object exists
        if main_cfg.get("cluster").is_none() {
            main_cfg["cluster"] = serde_json::json!({});
        }
        if let Some(cluster_obj) = main_cfg.get_mut("cluster") {
            if let Some(obj) = cluster_obj.as_object_mut() {
                obj.insert("enabled".to_string(), serde_json::json!(enabled));
            }
        }
        let updated = serde_json::to_string_pretty(&main_cfg)
            .map_err(|e| format!("failed to serialize config.json: {}", e))?;
        std::fs::write(&main_cfg_path, updated)
            .map_err(|e| format!("failed to write config.json: {}", e))?;
        Ok(Some(serde_json::json!({ "updated": true, "enabled": enabled })))
    }

    /// Update the `enabled` field in config.cluster.json.
    fn update_cluster_config_enabled(workspace: &str, enabled: bool) -> Result<(), String> {
        let path = cluster_config_path(workspace);
        let mut cfg: serde_json::Value = if path.exists() {
            let content = std::fs::read_to_string(&path)
                .map_err(|e| format!("failed to read cluster config: {}", e))?;
            serde_json::from_str(&content).unwrap_or(serde_json::json!({}))
        } else {
            serde_json::json!({})
        };
        if let Some(obj) = cfg.as_object_mut() {
            obj.insert("enabled".to_string(), serde_json::json!(enabled));
        }
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let json = serde_json::to_string_pretty(&cfg)
            .map_err(|e| format!("failed to serialize: {}", e))?;
        std::fs::write(&path, json)
            .map_err(|e| format!("failed to write cluster config: {}", e))?;
        Ok(())
    }

    fn peers(&self, ctx: &RequestContext) -> Result<Option<serde_json::Value>, String> {
        let workspace = require_workspace(ctx)?;
        let path = peers_path(workspace);
        if !path.exists() {
            return Ok(Some(serde_json::json!({ "peers": [] })));
        }
        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("failed to read peers.toml: {}", e))?;

        Ok(Some(serde_json::json!({
            "peers": content,
            "format": "toml",
        })))
    }

    /// Read cluster persona files from workspace.
    fn identity_get_files(&self, ctx: &RequestContext) -> Result<Option<serde_json::Value>, String> {
        let workspace = require_workspace(ctx)?;
        let identity = crate::handlers::read_workspace_file(workspace, "cluster/IDENTITY.md")
            .unwrap_or_default();
        let soul = crate::handlers::read_workspace_file(workspace, "cluster/SOUL.md")
            .unwrap_or_default();
        Ok(Some(serde_json::json!({
            "identity": identity,
            "soul": soul,
        })))
    }

    /// Save a cluster persona file.
    fn identity_save_file(
        &self,
        data: &serde_json::Value,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let workspace = require_workspace(ctx)?;
        let file = data.get("file").and_then(|v| v.as_str())
            .ok_or("missing 'file' field")?;
        let allowed = ["IDENTITY.md", "SOUL.md"];
        if !allowed.contains(&file) {
            return Err(format!("file '{}' not allowed, must be one of: {}", file, allowed.join(", ")));
        }
        let content = data.get("content").and_then(|v| v.as_str())
            .ok_or("missing 'content' field")?;
        let path = crate::handlers::resolve_path(workspace, &format!("cluster/{}", file))?;
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(&path, content)
            .map_err(|e| format!("failed to write {}: {}", file, e))?;
        tracing::info!(file = %file, "[Cluster] Persona file saved");
        Ok(Some(serde_json::json!({"saved": true, "file": file})))
    }
}

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        return s.to_string();
    }
    // Find the largest byte position <= max_len that lands on a char boundary.
    let boundary = s
        .char_indices()
        .take_while(|(i, _)| *i <= max_len)
        .last()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    if boundary == 0 || boundary > max_len {
        // Fallback: take only chars that fully fit
        let end = s
            .char_indices()
            .take_while(|(i, c)| *i + c.len_utf8() <= max_len)
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        format!("{}...", &s[..end])
    } else {
        format!("{}...", &s[..boundary])
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{}B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.0}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn format_ago(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{}s ago", secs)
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86400)
    }
}

// ---------------------------------------------------------------------------
// Firewall diagnostics
// ---------------------------------------------------------------------------

impl ClusterHandler {
    /// Check network readiness for cluster discovery and RPC.
    fn firewall_check(&self, ctx: &RequestContext) -> Result<Option<serde_json::Value>, String> {
        let (udp_port, tcp_port) = Self::read_cluster_ports(ctx);
        let mut tests = Vec::new();

        // Test 1: UDP bind
        let udp_bind_result = test_udp_bind(udp_port);
        tests.push(udp_bind_result.clone());

        // Test 2: SO_BROADCAST flag
        let broadcast_result = if udp_bind_result["pass"].as_bool().unwrap_or(false) {
            test_broadcast_flag()
        } else {
            serde_json::json!({ "name": "broadcast_flag", "pass": false, "detail": "跳过（UDP 绑定失败）" })
        };
        tests.push(broadcast_result.clone());

        // Test 3: Broadcast loopback (send to 255.255.255.255, receive back)
        let loopback_result = if broadcast_result["pass"].as_bool().unwrap_or(false) {
            test_broadcast_loopback()
        } else {
            serde_json::json!({ "name": "broadcast_loopback", "pass": false, "detail": "跳过（广播标志不可用）" })
        };
        tests.push(loopback_result);

        // Test 4: TCP bind
        let tcp_result = test_tcp_bind(tcp_port);
        tests.push(tcp_result);

        // Test 5: Platform firewall status
        let fw_result = check_platform_firewall(udp_port, tcp_port);
        tests.push(fw_result);

        let all_pass = tests.iter().all(|t| t["pass"].as_bool().unwrap_or(false));

        let platform = if cfg!(target_os = "windows") {
            "windows"
        } else if cfg!(target_os = "linux") {
            "linux"
        } else if cfg!(target_os = "macos") {
            "macos"
        } else {
            "other"
        };

        Ok(Some(serde_json::json!({
            "udp_port": udp_port,
            "tcp_port": tcp_port,
            "platform": platform,
            "tests": tests,
            "all_pass": all_pass,
        })))
    }

    /// Add firewall rules for cluster ports.
    fn firewall_add_rules(
        &self,
        data: &serde_json::Value,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let (default_udp, default_tcp) = Self::read_cluster_ports(ctx);
        let udp_port = data.get("udp_port")
            .and_then(|v| v.as_u64())
            .unwrap_or(default_udp as u64) as u16;
        let tcp_port = data.get("tcp_port")
            .and_then(|v| v.as_u64())
            .unwrap_or(default_tcp as u64) as u16;

        if udp_port == 0 || tcp_port == 0 {
            return Err("端口范围无效 (1-65535)".to_string());
        }

        add_platform_firewall_rules(udp_port, tcp_port)
    }

    /// Read cluster ports from config file, fallback to defaults.
    fn read_cluster_ports(ctx: &RequestContext) -> (u16, u16) {
        let default_udp = 11949u16;
        let default_tcp = 21949u16;

        let Ok(workspace) = require_workspace(ctx) else {
            return (default_udp, default_tcp);
        };
        let path = cluster_config_path(workspace);
        let Ok(content) = std::fs::read_to_string(&path) else {
            return (default_udp, default_tcp);
        };
        let cfg: serde_json::Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(_) => return (default_udp, default_tcp),
        };

        let udp = cfg.get("port")
            .and_then(|v| v.as_u64())
            .unwrap_or(default_udp as u64) as u16;
        let tcp = cfg.get("rpc_port")
            .and_then(|v| v.as_u64())
            .unwrap_or(default_tcp as u64) as u16;
        (udp, tcp)
    }
}

// ---------------------------------------------------------------------------
// Individual test functions
// ---------------------------------------------------------------------------

fn test_udp_bind(port: u16) -> serde_json::Value {
    match std::net::UdpSocket::bind(format!("0.0.0.0:{}", port)) {
        Ok(_) => serde_json::json!({
            "name": "udp_bind",
            "pass": true,
            "detail": format!("UDP {} 端口绑定成功", port)
        }),
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            serde_json::json!({
                "name": "udp_bind",
                "pass": true,
                "detail": format!("UDP {} 端口已被集群占用", port)
            })
        }
        Err(e) => serde_json::json!({
            "name": "udp_bind",
            "pass": false,
            "detail": format!("UDP {} 绑定失败: {}", port, e)
        }),
    }
}

fn test_broadcast_flag() -> serde_json::Value {
    match std::net::UdpSocket::bind("0.0.0.0:0") {
        Ok(socket) => match socket.set_broadcast(true) {
            Ok(_) => serde_json::json!({
                "name": "broadcast_flag",
                "pass": true,
                "detail": "SO_BROADCAST 设置成功"
            }),
            Err(e) => serde_json::json!({
                "name": "broadcast_flag",
                "pass": false,
                "detail": format!("设置广播标志失败: {}", e)
            }),
        },
        Err(e) => serde_json::json!({
            "name": "broadcast_flag",
            "pass": false,
            "detail": format!("创建测试套接字失败: {}", e)
        }),
    }
}

fn test_broadcast_loopback() -> serde_json::Value {
    use std::net::UdpSocket;
    use std::time::Duration;

    // Bind receiver on a random port
    let receiver = match UdpSocket::bind("0.0.0.0:0") {
        Ok(s) => s,
        Err(e) => {
            return serde_json::json!({
                "name": "broadcast_loopback",
                "pass": false,
                "detail": format!("绑定接收套接字失败: {}", e)
            })
        }
    };
    receiver.set_broadcast(true).ok();
    let recv_port = match receiver.local_addr() {
        Ok(a) => a.port(),
        Err(e) => {
            return serde_json::json!({
                "name": "broadcast_loopback",
                "pass": false,
                "detail": format!("获取接收端口失败: {}", e)
            })
        }
    };
    receiver
        .set_read_timeout(Some(Duration::from_secs(2)))
        .ok();

    // Bind sender on a different random port
    let sender = match UdpSocket::bind("0.0.0.0:0") {
        Ok(s) => s,
        Err(e) => {
            return serde_json::json!({
                "name": "broadcast_loopback",
                "pass": false,
                "detail": format!("绑定发送套接字失败: {}", e)
            })
        }
    };
    sender.set_broadcast(true).ok();

    let test_payload = b"NEMESIS_FIREWALL_TEST";
    let broadcast_addr = format!("255.255.255.255:{}", recv_port);

    if let Err(e) = sender.send_to(test_payload, &broadcast_addr) {
        return serde_json::json!({
            "name": "broadcast_loopback",
            "pass": false,
            "detail": format!("广播发送失败: {}", e)
        });
    }

    let mut buf = [0u8; 64];
    match receiver.recv_from(&mut buf) {
        Ok((n, _)) if &buf[..n] == test_payload => serde_json::json!({
            "name": "broadcast_loopback",
            "pass": true,
            "detail": "广播回环成功"
        }),
        Ok(_) => serde_json::json!({
            "name": "broadcast_loopback",
            "pass": false,
            "detail": "收到数据但不匹配"
        }),
        Err(e) if e.kind() == std::io::ErrorKind::TimedOut
            || e.kind() == std::io::ErrorKind::WouldBlock =>
        {
            serde_json::json!({
                "name": "broadcast_loopback",
                "pass": false,
                "detail": "广播回环超时（可能被防火墙阻止）"
            })
        }
        Err(e) => serde_json::json!({
            "name": "broadcast_loopback",
            "pass": false,
            "detail": format!("接收失败: {}", e)
        }),
    }
}

fn test_tcp_bind(port: u16) -> serde_json::Value {
    match std::net::TcpListener::bind(format!("0.0.0.0:{}", port)) {
        Ok(_) => serde_json::json!({
            "name": "tcp_bind",
            "pass": true,
            "detail": format!("TCP {} 端口绑定成功", port)
        }),
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            serde_json::json!({
                "name": "tcp_bind",
                "pass": true,
                "detail": format!("TCP {} 端口已被集群占用", port)
            })
        }
        Err(e) => serde_json::json!({
            "name": "tcp_bind",
            "pass": false,
            "detail": format!("TCP {} 绑定失败: {}", port, e)
        }),
    }
}

// ---------------------------------------------------------------------------
// Platform-specific firewall check
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
fn check_platform_firewall(udp_port: u16, tcp_port: u16) -> serde_json::Value {
    // Check if Windows Firewall is enabled
    let fw_enabled = std::process::Command::new("netsh")
        .args(&["advfirewall", "show", "currentprofile", "state"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .map(|s| s.contains("ON"))
        .unwrap_or(false);

    // Check if rules already exist
    let udp_rule_exists = std::process::Command::new("netsh")
        .args(&["advfirewall", "firewall", "show", "rule", "name=NemesisBot Discovery"])
        .output()
        .ok()
        .map(|o| o.status.success())
        .unwrap_or(false);

    let tcp_rule_exists = std::process::Command::new("netsh")
        .args(&["advfirewall", "firewall", "show", "rule", "name=NemesisBot RPC"])
        .output()
        .ok()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !fw_enabled {
        return serde_json::json!({
            "name": "firewall_status",
            "pass": true,
            "detail": "Windows 防火墙未启用，不阻止流量"
        });
    }

    if udp_rule_exists && tcp_rule_exists {
        serde_json::json!({
            "name": "firewall_status",
            "pass": true,
            "detail": format!("Windows 防火墙已启用，NemesisBot 规则已存在 (UDP {} + TCP {})", udp_port, tcp_port)
        })
    } else {
        let missing = match (udp_rule_exists, tcp_rule_exists) {
            (false, false) => "UDP 和 TCP 规则均缺失",
            (false, true) => "UDP 规则缺失",
            (true, false) => "TCP 规则缺失",
            _ => unreachable!(),
        };
        serde_json::json!({
            "name": "firewall_status",
            "pass": false,
            "detail": format!("Windows 防火墙已启用，{}", missing)
        })
    }
}

#[cfg(target_os = "linux")]
fn check_platform_firewall(udp_port: u16, tcp_port: u16) -> serde_json::Value {
    // Try ufw first
    let ufw_output = std::process::Command::new("ufw")
        .arg("status")
        .output()
        .ok();

    if let Some(output) = ufw_output {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.contains("inactive") {
                return serde_json::json!({
                    "name": "firewall_status",
                    "pass": true,
                    "detail": "UFW 防火墙未启用，不阻止流量"
                });
            }
            // UFW is active — check if ports are allowed
            let udp_ok = stdout.contains(&format!("{}/udp", udp_port))
                || stdout.contains("Anywhere");
            let tcp_ok = stdout.contains(&format!("{}/tcp", tcp_port))
                || stdout.contains("Anywhere");
            if udp_ok && tcp_ok {
                return serde_json::json!({
                    "name": "firewall_status",
                    "pass": true,
                    "detail": format!("UFW 已启用，端口 UDP {} 和 TCP {} 已放行", udp_port, tcp_port)
                });
            } else {
                return serde_json::json!({
                    "name": "firewall_status",
                    "pass": false,
                    "detail": format!("UFW 已启用，但端口 UDP {} 或 TCP {} 未放行", udp_port, tcp_port)
                });
            }
        }
    }

    // Fallback: check iptables
    let ipt_output = std::process::Command::new("iptables")
        .args(&["-L", "INPUT", "-n"])
        .output()
        .ok();

    if let Some(output) = ipt_output {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let policy_accept = stdout.contains("Chain INPUT (policy ACCEPT)");
            let udp_ok = stdout.contains(&format!("dpt:{}", udp_port));
            let tcp_ok = stdout.contains(&format!("dpt:{}", tcp_port));
            if policy_accept && !stdout.contains("REJECT") && !stdout.contains("DROP") {
                return serde_json::json!({
                    "name": "firewall_status",
                    "pass": true,
                    "detail": "iptables 默认策略 ACCEPT，不阻止流量"
                });
            }
            if udp_ok && tcp_ok {
                return serde_json::json!({
                    "name": "firewall_status",
                    "pass": true,
                    "detail": format!("iptables 已放行端口 UDP {} 和 TCP {}", udp_port, tcp_port)
                });
            }
            return serde_json::json!({
                "name": "firewall_status",
                "pass": false,
                "detail": format!("iptables 可能阻止端口 UDP {} 或 TCP {}", udp_port, tcp_port)
            });
        }
    }

    serde_json::json!({
        "name": "firewall_status",
        "pass": true,
        "detail": "未检测到防火墙（ufw/iptables 不可用）"
    })
}

#[cfg(target_os = "macos")]
fn check_platform_firewall(_udp_port: u16, _tcp_port: u16) -> serde_json::Value {
    let output = std::process::Command::new("pfctl")
        .args(&["-s", "info"])
        .output()
        .ok();

    match output {
        Some(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            if stdout.contains("Enabled") {
                serde_json::json!({
                    "name": "firewall_status",
                    "pass": true,
                    "detail": "macOS pf 已启用，但通常不阻止局域网流量"
                })
            } else {
                serde_json::json!({
                    "name": "firewall_status",
                    "pass": true,
                    "detail": "macOS pf 未启用"
                })
            }
        }
        _ => serde_json::json!({
            "name": "firewall_status",
            "pass": true,
            "detail": "macOS pf 状态未知（可能需要 sudo）"
        }),
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
fn check_platform_firewall(_udp_port: u16, _tcp_port: u16) -> serde_json::Value {
    serde_json::json!({
        "name": "firewall_status",
        "pass": true,
        "detail": "当前平台不支持防火墙检测"
    })
}

// ---------------------------------------------------------------------------
// Platform-specific firewall rule addition
// ---------------------------------------------------------------------------

/// Windows: UAC elevation via ShellExecuteW with "runas" verb.
/// Fire-and-forget — returns immediately after triggering the UAC prompt.
#[cfg(target_os = "windows")]
fn spawn_elevated(exe: &str, args: &str) -> Result<(), String> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    let verb: Vec<u16> = OsStr::new("runas").encode_wide().chain(std::iter::once(0)).collect();
    let file: Vec<u16> = OsStr::new(exe).encode_wide().chain(std::iter::once(0)).collect();
    let params: Vec<u16> = OsStr::new(args).encode_wide().chain(std::iter::once(0)).collect();

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn ShellExecuteW(
            hwnd: isize,
            lpverb: *const u16,
            lpfile: *const u16,
            lpparameters: *const u16,
            lpdirectory: *const u16,
            nshowcmd: i32,
        ) -> isize;
    }
    const SW_SHOWNORMAL: i32 = 1;

    unsafe {
        let ret = ShellExecuteW(
            0,
            verb.as_ptr(),
            file.as_ptr(),
            params.as_ptr(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        );
        // ShellExecuteW returns > 32 on success
        if ret <= 32 {
            return Err(format!("ShellExecuteW runas 失败 (code: {})", ret));
        }
    }
    Ok(())
}


#[cfg(target_os = "windows")]
fn add_platform_firewall_rules(udp_port: u16, tcp_port: u16) -> Result<Option<serde_json::Value>, String> {
    let manual_udp = format!(
        "netsh advfirewall firewall add rule name=\"NemesisBot Discovery\" dir=in action=allow protocol=UDP localport={} profile=any",
        udp_port
    );
    let manual_tcp = format!(
        "netsh advfirewall firewall add rule name=\"NemesisBot RPC\" dir=in action=allow protocol=TCP localport={} profile=any",
        tcp_port
    );

    // Step 1: Try direct execution (succeeds if already running as admin)
    let udp_ok = std::process::Command::new("netsh")
        .args(&[
            "advfirewall", "firewall", "add", "rule",
            "name=NemesisBot Discovery",
            "dir=in", "action=allow", "protocol=UDP",
            &format!("localport={}", udp_port),
            "profile=any",
        ])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    let tcp_ok = std::process::Command::new("netsh")
        .args(&[
            "advfirewall", "firewall", "add", "rule",
            "name=NemesisBot RPC",
            "dir=in", "action=allow", "protocol=TCP",
            &format!("localport={}", tcp_port),
            "profile=any",
        ])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if udp_ok && tcp_ok {
        return Ok(Some(serde_json::json!({
            "success": true,
            "udp_rule_added": true,
            "tcp_rule_added": true,
            "message": format!("防火墙规则已添加：UDP {} + TCP {}", udp_port, tcp_port),
            "permission_denied": false,
        })));
    }

    // Step 2: Direct execution failed — elevate via ShellExecuteW "runas".
    // netsh outputs errors to stdout, not stderr, so keyword detection is unreliable.
    // Always try UAC elevation as fallback.
    let exe_path = std::env::current_exe().map_err(|e| format!("无法获取当前程序路径: {}", e))?;
    let exe_str = exe_path.to_str().ok_or("程序路径包含非法字符")?;
    let args = format!("cluster firewall add --udp-port {} --tcp-port {}", udp_port, tcp_port);

    match spawn_elevated(exe_str, &args) {
        Ok(()) => {
            Ok(Some(serde_json::json!({
                "success": false,
                "udp_rule_added": false,
                "tcp_rule_added": false,
                "message": "已弹出 UAC 提权请求，请在弹窗中确认。添加完成后请重新检测网络。".to_string(),
                "permission_denied": false,
                "uac_triggered": true,
                "manual_commands": [manual_udp, manual_tcp],
                "platform_hint": "如果未看到 UAC 弹窗，请手动执行以下命令",
            })))
        }
        Err(e) => {
            Ok(Some(serde_json::json!({
                "success": false,
                "udp_rule_added": false,
                "tcp_rule_added": false,
                "message": format!("无法启动 UAC 提权: {}", e),
                "permission_denied": true,
                "manual_commands": [manual_udp, manual_tcp],
                "platform_hint": "以管理员身份手动执行以下命令",
            })))
        }
    }
}

#[cfg(target_os = "linux")]
fn add_platform_firewall_rules(udp_port: u16, tcp_port: u16) -> Result<Option<serde_json::Value>, String> {
    // Try ufw first
    let ufw_exists = std::process::Command::new("which")
        .arg("ufw")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if ufw_exists {
        let udp_result = std::process::Command::new("ufw")
            .arg("allow")
            .arg(format!("{}/udp", udp_port))
            .output();
        let tcp_result = std::process::Command::new("ufw")
            .arg("allow")
            .arg(format!("{}/tcp", tcp_port))
            .output();

        let udp_ok = udp_result.as_ref().map(|o| o.status.success()).unwrap_or(false);
        let tcp_ok = tcp_result.as_ref().map(|o| o.status.success()).unwrap_or(false);

        if udp_ok && tcp_ok {
            return Ok(Some(serde_json::json!({
                "success": true,
                "udp_rule_added": true,
                "tcp_rule_added": true,
                "message": format!("UFW 规则已添加：UDP {} + TCP {}", udp_port, tcp_port),
                "permission_denied": false,
            })));
        }
    }

    // Fallback: iptables
    let udp_result = std::process::Command::new("iptables")
        .args(&["-I", "INPUT", "-p", "udp", "--dport", &udp_port.to_string(), "-j", "ACCEPT"])
        .output();
    let tcp_result = std::process::Command::new("iptables")
        .args(&["-I", "INPUT", "-p", "tcp", "--dport", &tcp_port.to_string(), "-j", "ACCEPT"])
        .output();

    let udp_ok = udp_result.as_ref().map(|o| o.status.success()).unwrap_or(false);
    let tcp_ok = tcp_result.as_ref().map(|o| o.status.success()).unwrap_or(false);

    if udp_ok && tcp_ok {
        return Ok(Some(serde_json::json!({
            "success": true,
            "udp_rule_added": true,
            "tcp_rule_added": true,
            "message": format!("iptables 规则已添加：UDP {} + TCP {}", udp_port, tcp_port),
            "permission_denied": false,
        })));
    }

    // Both failed — return manual commands
    let manual_udp = if ufw_exists {
        format!("sudo ufw allow {}/udp", udp_port)
    } else {
        format!("sudo iptables -I INPUT -p udp --dport {} -j ACCEPT", udp_port)
    };
    let manual_tcp = if ufw_exists {
        format!("sudo ufw allow {}/tcp", tcp_port)
    } else {
        format!("sudo iptables -I INPUT -p tcp --dport {} -j ACCEPT", tcp_port)
    };

    Ok(Some(serde_json::json!({
        "success": false,
        "udp_rule_added": false,
        "tcp_rule_added": false,
        "message": "权限不足，无法添加防火墙规则".to_string(),
        "permission_denied": true,
        "manual_commands": [manual_udp, manual_tcp],
        "platform_hint": "使用 sudo 运行 NemesisBot",
    })))
}

#[cfg(target_os = "macos")]
fn add_platform_firewall_rules(_udp_port: u16, _tcp_port: u16) -> Result<Option<serde_json::Value>, String> {
    Ok(Some(serde_json::json!({
        "success": false,
        "udp_rule_added": false,
        "tcp_rule_added": false,
        "message": "macOS pf 通常不阻止局域网流量，无需手动添加规则",
        "permission_denied": false,
        "manual_commands": [] as Vec<&str>,
        "platform_hint": "如需配置，请编辑 /etc/pf.conf",
    })))
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
fn add_platform_firewall_rules(_udp_port: u16, _tcp_port: u16) -> Result<Option<serde_json::Value>, String> {
    Err("当前平台不支持防火墙规则管理".to_string())
}
