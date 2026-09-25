//! cluster.rs AGT 覆盖率批次（2026-09-24）。
//!
//! 与 cluster_deep_tests（handlers 级、只能走公开面）互补：本文件声明在
//! cluster.rs 内部，可触达私有项（persist_peers_identity / truncate_str /
//! peers_path）。聚焦仍缺的确定性臂：
//! - 模块接口面（Default / module_name / commands 表无重复）
//! - truncate_str 截断点落在字符串中段（非末字符）的分支
//! - persist_peers_identity 无 peers.toml 时的全新 StaticConfig 臂 + 落盘
//! - tasks.submit 急停冻结门（EST-04）
//! - firewall.add_rules 非法端口拒绝（不触 netsh add）
//! - firewall.check 结构投影（netsh 只读查询，不断言 pass 值——宿主相关）
//! - config.save 在 config 目录被文件占位时的 create_dir 失败臂
//! - tasks.list 空任务 + 有 log_dir 的空聚合臂；duration 解析失败 Null 臂；
//!   tasks.detail 的 Completed / Failed / Cancelled 标签映射
//! - pair 对拒连地址快速失败（ refused 即返，无 8s 探测窗口）
//!
//! 结构性豁免（见报告）：spawn_elevated / add_platform_firewall_rules（UAC +
//! 系统防火墙写操作）、nodes_ping 5s 超时臂（黑洞地址时序依赖）、
//! check_platform_firewall 判定臂（宿主防火墙状态决定）、broadcast/loopback
//! 错误臂（环境故障注入）、nodes.refresh tags 臂（真实 RPC 回包）。

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::ModuleHandler;
use nemesis_cluster::cluster::Cluster;
use nemesis_cluster::types::{ClusterConfig, ExtendedNodeInfo};
use nemesis_types::cluster::{NodeInfo, NodeRole, Task, TaskStatus};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

fn agt_make_ctx(
    dir: &tempfile::TempDir,
    cluster: Option<Arc<Cluster>>,
    log_dir: Option<String>,
    estop: Option<Arc<nemesis_agent::estop::EstopState>>,
) -> RequestContext {
    let ws = dir.path().to_string_lossy().to_string();
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: Some(ws.clone()),
        home: Some(ws.clone()),
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new("test-model".to_string())),
        model_base: Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: Arc::new(AtomicBool::new(false)),
        event_hub: Arc::new(EventHub::new()),
        running: Arc::new(AtomicBool::new(true)),
        session_manager: Arc::new(SessionManager::with_default_timeout()),
        inbound_tx: None,
        streaming_provider: None,
        ws_router: None,
        agent_service: None,
        data_store: None,
        memory_manager: None,
        forge: None,
        agent_loop: Arc::new(parking_lot::RwLock::new(None)),
        cluster,
        cluster_service: None,
        cluster_log_dir: log_dir,
        workflow_engine: None,
        #[cfg(feature = "workflow")]
        chat_secret_store: Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::in_memory()),
        #[cfg(not(feature = "workflow"))]
        chat_secret_store: Arc::new(()),
        #[cfg(feature = "workflow")]
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        #[cfg(not(feature = "workflow"))]
        webhook_rate_limiter: Arc::new(()),
        internal_cmd_tx: None,
        estop,
        signature_verify: None,
        cron: None,
        board: None,
    });
    RequestContext {
        session_id: "agt".to_string(),
        chat_id: "agt".to_string(),
        workspace: Some(ws),
        home: None,
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

fn agt_cluster(dir: &tempfile::TempDir) -> Arc<Cluster> {
    Arc::new(Cluster::with_workspace(
        ClusterConfig::default(),
        dir.path().to_path_buf(),
    ))
}

fn agt_task(id: &str, status: TaskStatus, created: &str, completed: Option<&str>) -> Task {
    Task {
        id: id.to_string(),
        status,
        action: "peer_chat".to_string(),
        peer_id: "n1".to_string(),
        payload: serde_json::json!({}),
        result: None,
        original_channel: "dashboard".to_string(),
        original_chat_id: "c1".to_string(),
        created_at: created.to_string(),
        completed_at: completed.map(String::from),
    }
}

// -----------------------------------------------------------------------
// 模块接口面 + 纯 helper
// -----------------------------------------------------------------------

#[test]
fn agt_cluster_module_surface_and_command_table() {
    let h = ClusterHandler::default();
    assert_eq!(h.module_name(), "cluster");
    let cmds = h.commands();
    assert!(cmds.contains(&"runtime.status"));
    assert!(cmds.contains(&"firewall.check"));
    assert!(cmds.contains(&"persona_apply"));
    let mut sorted = cmds.to_vec();
    sorted.sort_unstable();
    let before = sorted.len();
    sorted.dedup();
    assert_eq!(sorted.len(), before, "duplicate command in table");
}

#[test]
fn agt_truncate_str_boundary_mid_string() {
    // 4 字节处有字符边界且 < len → 走"截断点在中段"分支（非末字符特例）
    let out = truncate_str("αααα", 5);
    assert_eq!(out, "αα...");
    // ASCII 长串：取前 max_len 个字符再加省略号
    assert_eq!(truncate_str("abcdefghij", 6), "abcdef...");
}

#[test]
fn agt_persist_peers_identity_fresh_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let pkg = cluster_persona_gen::PersonaPackage {
        node_name: "pkg-node".to_string(),
        display_name: "包装节点".to_string(),
        emoji: "🤖".to_string(),
        role: "worker".to_string(),
        category: "development".to_string(),
        tags: vec!["rust".to_string(), "edge".to_string()],
        identity_md: "# ID".to_string(),
        soul_md: "# SOUL".to_string(),
        expertise_md: String::new(),
        coverage: None,
    };
    // 无 peers.toml → 全新 StaticConfig 臂
    ClusterHandler::persist_peers_identity(&ws, &pkg).expect("fresh persist must succeed");
    let cfg = nemesis_cluster::cluster_config::load_static_config(&peers_path(&ws))
        .expect("peers.toml must exist after persist");
    assert_eq!(cfg.node.name, "包装节点");
    assert_eq!(cfg.node.role, "worker");
    assert_eq!(cfg.node.category, "development");
    assert_eq!(cfg.node.tags, vec!["rust".to_string(), "edge".to_string()]);
}

// -----------------------------------------------------------------------
// tasks.submit 急停冻结门
// -----------------------------------------------------------------------

#[tokio::test]
async fn agt_tasks_submit_frozen_under_estop() {
    let dir = tempfile::tempdir().unwrap();
    let cluster = agt_cluster(&dir);
    let estop = Arc::new(nemesis_agent::estop::EstopState::new());
    estop.trigger();
    let ctx = agt_make_ctx(&dir, Some(cluster), None, Some(estop));

    let err = ClusterHandler::new()
        .handle_cmd(
            "tasks.submit",
            Some(serde_json::json!({ "content": "hi" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("急停"), "err: {err}");
}

// -----------------------------------------------------------------------
// firewall.add_rules 非法端口 / firewall.check 结构投影
// -----------------------------------------------------------------------

#[tokio::test]
async fn agt_firewall_add_rules_rejects_invalid_ports_before_netsh() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = agt_make_ctx(&dir, None, None, None);
    let h = ClusterHandler::new();

    for ports in [
        serde_json::json!({ "udp_port": 0 }),
        serde_json::json!({ "udp_port": 11949, "tcp_port": 0 }),
    ] {
        let err = h
            .handle_cmd("firewall.add_rules", Some(ports), &ctx)
            .await
            .unwrap_err();
        assert!(err.contains("端口范围无效"), "err: {err}");
    }
}

#[tokio::test]
async fn agt_firewall_check_reports_structure_from_config_ports() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_dir = dir.path().join("config");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(
        cfg_dir.join("config.cluster.json"),
        serde_json::json!({ "port": 12000, "rpc_port": 22000 }).to_string(),
    )
    .unwrap();
    let ctx = agt_make_ctx(&dir, None, None, None);
    let out = ClusterHandler::new()
        .handle_cmd("firewall.check", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["udp_port"], 12000);
    assert_eq!(out["tcp_port"], 22000);
    assert_eq!(out["platform"], "windows");
    let tests = out["tests"].as_array().unwrap();
    let names: Vec<&str> = tests.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        vec![
            "udp_bind",
            "broadcast_flag",
            "broadcast_loopback",
            "tcp_bind",
            "firewall_status"
        ]
    );
    assert_eq!(out["all_pass"], tests.iter().all(|t| t["pass"] == true));

    // 无配置文件 → 默认端口回退
    let dir2 = tempfile::tempdir().unwrap();
    let ctx2 = agt_make_ctx(&dir2, None, None, None);
    let out2 = ClusterHandler::new()
        .handle_cmd("firewall.check", None, &ctx2)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out2["udp_port"], 11949);
    assert_eq!(out2["tcp_port"], 21949);
}

// -----------------------------------------------------------------------
// config.save：config 目录被文件占位 → create_dir 失败
// -----------------------------------------------------------------------

#[tokio::test]
async fn agt_config_save_fails_when_config_dir_is_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config"), b"not a dir").unwrap();
    let ctx = agt_make_ctx(&dir, None, None, None);
    let err = ClusterHandler::new()
        .handle_cmd(
            "config.save",
            Some(serde_json::json!({ "enabled": true })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("failed to create config dir"), "err: {err}");
}

// -----------------------------------------------------------------------
// tasks.list / tasks.detail：空聚合 / 终态标签 / duration Null
// -----------------------------------------------------------------------

#[tokio::test]
async fn agt_tasks_list_empty_with_log_dir_takes_empty_summaries_arm() {
    let dir = tempfile::tempdir().unwrap();
    let log_dir = dir.path().join("cluster_logs");
    let ctx = agt_make_ctx(
        &dir,
        Some(agt_cluster(&dir)),
        Some(log_dir.to_string_lossy().to_string()),
        None,
    );
    let out = ClusterHandler::new()
        .handle_cmd("tasks.list", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["total"], 0);
    assert_eq!(out["tasks"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn agt_tasks_detail_terminal_labels_and_list_duration_null() {
    let dir = tempfile::tempdir().unwrap();
    let cluster = agt_cluster(&dir);
    cluster.register_node(ExtendedNodeInfo {
        base: NodeInfo {
            id: "n1".to_string(),
            name: "alpha".to_string(),
            role: NodeRole::Worker,
            address: "10.0.0.1:12000".to_string(),
            category: "edge".to_string(),
            last_seen: String::new(),
        },
        status: nemesis_cluster::types::NodeStatus::Online,
        capabilities: Vec::new(),
        tags: Vec::new(),
        addresses: vec!["10.0.0.1:12000".to_string()],
        node_type: "agent".to_string(),
    });
    let ctx = agt_make_ctx(&dir, Some(cluster.clone()), None, None);
    let h = ClusterHandler::new();

    // 终态三标签：completed / failed / failed(cancelled)
    cluster
        .task_manager()
        .submit(agt_task(
            "t-done",
            TaskStatus::Completed,
            &chrono::Local::now().to_rfc3339(),
            Some(&chrono::Local::now().to_rfc3339()),
        ))
        .unwrap();
    cluster
        .task_manager()
        .submit(agt_task(
            "t-fail",
            TaskStatus::Failed,
            &chrono::Local::now().to_rfc3339(),
            Some(&chrono::Local::now().to_rfc3339()),
        ))
        .unwrap();
    cluster
        .task_manager()
        .submit(agt_task(
            "t-cancel",
            TaskStatus::Cancelled,
            &chrono::Local::now().to_rfc3339(),
            Some(&chrono::Local::now().to_rfc3339()),
        ))
        .unwrap();

    for (id, label) in [
        ("t-done", "completed"),
        ("t-fail", "failed"),
        ("t-cancel", "failed"),
    ] {
        let out = h
            .handle_cmd(
                "tasks.detail",
                Some(serde_json::json!({ "task_id": id })),
                &ctx,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(out["status"], label, "task {id}");
    }

    // duration 解析失败（created_at 非法）→ Null
    cluster
        .task_manager()
        .submit(agt_task(
            "t-badts",
            TaskStatus::Completed,
            "not-a-timestamp",
            Some(&chrono::Local::now().to_rfc3339()),
        ))
        .unwrap();
    let out = h
        .handle_cmd("tasks.list", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    let row = out["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == "t-badts")
        .expect("t-badts must be listed");
    assert!(row["duration"].is_null(), "row: {row}");
}

// -----------------------------------------------------------------------
// pair：拒连地址快速失败（probe → ECONNREFUSED 即 Err，不进 8s 超时窗）
// -----------------------------------------------------------------------

#[tokio::test]
async fn agt_pair_rejects_refused_address() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = agt_make_ctx(&dir, None, None, None);
    let started = Instant::now();
    let result = ClusterHandler::new()
        .handle_cmd(
            "pair",
            Some(serde_json::json!({ "address": "127.0.0.1:1" })),
            &ctx,
        )
        .await;
    assert!(result.is_err(), "refused address must fail pairing");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(8),
        "refused must fail fast, took {:?}",
        started.elapsed()
    );
}
