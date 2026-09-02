//! P3-web3（2026-08-25）：cluster.rs 深覆盖测试。
//!
//! 与 `cluster_extra_tests.rs` / `cluster_more_tests.rs` 互补，聚焦此前零覆盖
//! 的运行时路径：
//! - runtime.status 任务指标 / runtime.start|stop 持久化（FakeClusterSvc）
//! - nodes.ping 真实 TCP 探测（本地监听器成功 + 释放端口拒绝）
//! - nodes.add 注册分支 / nodes.refresh 全臂（set_call_with_context_fn 测试钩子）
//! - tasks.list / tasks.detail 从 cluster 日志聚合 rounds/toolChain
//! - tasks.submit 普通任务 + peer_chat 分支（RpcClient::new 快速失败）
//! - topology 真实 RPC 连接 + traces（broadcast 跳过 / inbound 跳过）
//! - config.get 静态回退 / config.set_master_enabled 非对象 cluster
//! - firewall.check AddrInUse 视为 pass（端口被本测试占用）
//! - diagnostics.run 三段门（缺参 / 无 client / resolver 缺 peer）
//! - persona_generate 校验 + tier 门 + wiremock 全流程；persona_apply 全臂
//!
//! 所有测试用 tempdir 自建 ctx，不触碰 default_path_manager() 单例 home。

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_cluster::cluster::Cluster;
use nemesis_cluster::rpc::client::RpcClient;
use nemesis_cluster::types::{ClusterConfig, ExtendedNodeInfo, NodeStatus};
use nemesis_types::cluster::{NodeInfo, NodeRole};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

// -----------------------------------------------------------------------
// Test infrastructure
// -----------------------------------------------------------------------

/// 可编程假集群服务：控制 start/stop 成败与 is_running 状态。
struct FakeClusterSvc {
    running: AtomicBool,
    fail_start: bool,
    fail_stop: bool,
}

impl FakeClusterSvc {
    fn new() -> Self {
        Self {
            running: AtomicBool::new(false),
            fail_start: false,
            fail_stop: false,
        }
    }
}

impl nemesis_services::bot_service::LifecycleService for FakeClusterSvc {
    fn start(&self) -> Result<(), String> {
        if self.fail_start {
            return Err("start boom".to_string());
        }
        self.running
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
    fn stop(&self) -> Result<(), String> {
        if self.fail_stop {
            return Err("stop boom".to_string());
        }
        self.running
            .store(false, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
    fn is_running(&self) -> bool {
        self.running.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[allow(clippy::too_many_arguments)]
fn make_deep_ctx(
    ws: Option<String>,
    cluster: Option<Arc<Cluster>>,
    log_dir: Option<String>,
    service: Option<Arc<FakeClusterSvc>>,
) -> RequestContext {
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: ws.clone(),
        home: ws.clone(),
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
        cluster_service: service
            .map(|s| s as Arc<dyn nemesis_services::bot_service::LifecycleService>),
        cluster_log_dir: log_dir,
        workflow_engine: None,
        #[cfg(feature = "workflow")]
        chat_secret_store: std::sync::Arc::new(
            nemesis_workflow::chat_secrets::ChatSecretStore::in_memory(),
        ),
        #[cfg(not(feature = "workflow"))]
        chat_secret_store: std::sync::Arc::new(()),
        #[cfg(feature = "workflow")]
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        #[cfg(not(feature = "workflow"))]
        webhook_rate_limiter: Arc::new(()),
        internal_cmd_tx: None,
        estop: None,
        cron: None,
        board: None,
    });
    RequestContext {
        session_id: "test-session".to_string(),
        chat_id: "test-chat".to_string(),
        workspace: ws,
        home: None,
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

/// workspace=home=ws（标准形态；大多数命令按 home==workspace 查 config）。
fn ctx_ws(
    dir: &tempfile::TempDir,
    cluster: Option<Arc<Cluster>>,
    log_dir: Option<String>,
    service: Option<Arc<FakeClusterSvc>>,
) -> RequestContext {
    let ws = dir.path().to_string_lossy().to_string();
    let mut ctx = make_deep_ctx(Some(ws.clone()), cluster, log_dir, service);
    ctx.home = Some(ws);
    ctx
}

fn test_cluster(dir: &tempfile::TempDir) -> Arc<Cluster> {
    Arc::new(Cluster::with_workspace(
        ClusterConfig::default(),
        dir.path().to_path_buf(),
    ))
}

fn node(id: &str, name: &str, role: NodeRole, online: bool, address: &str) -> ExtendedNodeInfo {
    ExtendedNodeInfo {
        base: NodeInfo {
            id: id.to_string(),
            name: name.to_string(),
            role,
            address: address.to_string(),
            category: "edge".to_string(),
            last_seen: String::new(),
        },
        status: if online {
            NodeStatus::Online
        } else {
            NodeStatus::Offline
        },
        capabilities: vec!["cluster".to_string()],
        addresses: vec![address.to_string()],
        node_type: "agent".to_string(),
    }
}

/// 写今天的 cluster log（JSONL），文件名必须用当天日期（reader 只扫近 N 天）。
fn write_today_cluster_log(log_dir: &Path, lines: &[serde_json::Value]) {
    std::fs::create_dir_all(log_dir).unwrap();
    let today = chrono::Local::now().format("%Y-%m-%d");
    let path = log_dir.join(format!("cluster_{today}.log"));
    let content = lines
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(path, content + "\n").unwrap();
}

fn log_event(event: &str, data: serde_json::Value) -> serde_json::Value {
    let mut v = serde_json::json!({ "event": event, "ts": chrono::Local::now().to_rfc3339() });
    if let Some(obj) = v.as_object_mut() {
        for (k, val) in data.as_object().into_iter().flatten() {
            obj.insert(k.clone(), val.clone());
        }
    }
    v
}

// -----------------------------------------------------------------------
// runtime.status / runtime.start / runtime.stop
// -----------------------------------------------------------------------

#[tokio::test]
async fn rt_status_task_state_metrics() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let cluster = test_cluster(&dir);
    cluster.register_node(node(
        "n1",
        "alpha",
        NodeRole::Worker,
        true,
        "10.0.0.1:12000",
    ));
    let svc = Arc::new(FakeClusterSvc::new());
    svc.running.store(true, std::sync::atomic::Ordering::SeqCst);
    let ctx = ctx_ws(&dir, Some(cluster.clone()), None, Some(svc));

    let c = ctx.state.cluster.as_ref().unwrap();
    let t1 = c.submit_task("a", serde_json::json!({"content":"hi"}), "dashboard", "s1");
    c.complete_task(&t1, serde_json::json!({"ok":true}));
    let t2 = c.submit_task("a", serde_json::json!({}), "dashboard", "s2");
    c.fail_task(&t2, "boom");
    let t3 = c.submit_task("a", serde_json::json!({}), "dashboard", "s3");
    assert!(c.task_manager().assign_task(&t3, "n1"));
    let _t4 = c.submit_task("a", serde_json::json!({}), "dashboard", "s4"); // Pending

    let result = handler
        .handle_cmd("runtime.status", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["running"], true); // service.is_running()
    assert_eq!(result["total_nodes"], 1);
    assert_eq!(result["online_nodes"], 1);
    assert_eq!(result["active_tasks"], 2); // t3 Running + t4 Pending
    assert_eq!(result["today_completed"], 1);
    assert_eq!(result["total_tasks"], 4);
    assert_eq!(result["success_rate"], 0.5); // 1 completed / (1+1)
    assert_ne!(result["avg_duration"], "--"); // t1 has parseable timestamps
}

#[tokio::test]
async fn rt_status_recent_events_with_log_dir() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let log_dir = dir.path().join("cluster_logs");
    write_today_cluster_log(
        &log_dir,
        &[
            log_event("cluster_start", serde_json::json!({"node_id":"local-1"})),
            log_event(
                "node_discovered",
                serde_json::json!({"peer_addr":"10.0.0.9:12000"}),
            ),
        ],
    );
    let ctx = ctx_ws(
        &dir,
        Some(test_cluster(&dir)),
        Some(log_dir.to_string_lossy().to_string()),
        None,
    );

    let result = handler
        .handle_cmd("runtime.status", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    let events = result["recent_events"].as_array().unwrap();
    assert!(!events.is_empty());
    assert!(events.iter().any(|e| e["type"] == "system"));

    // events.recent 命令独立走同一条读取路径（带 limit）。
    let result = handler
        .handle_cmd("events.recent", Some(serde_json::json!({"limit": 1})), &ctx)
        .await
        .unwrap()
        .unwrap();
    let events = result["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
}

#[tokio::test]
async fn rt_start_stop_persist_enabled_flag_existing_config() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("config")).unwrap();
    let cfg_path = dir.path().join("config/config.cluster.json");
    std::fs::write(&cfg_path, r#"{"port": 11949, "rpc_port": 21949}"#).unwrap();

    let ctx = ctx_ws(&dir, None, None, Some(Arc::new(FakeClusterSvc::new())));
    handler
        .handle_cmd("runtime.start", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    let cfg: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(cfg["enabled"], true); // 新增 enabled
    assert_eq!(cfg["port"], 11949); // 既有键保留（update in place）

    handler
        .handle_cmd("runtime.stop", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    let cfg: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(cfg["enabled"], false);
    assert_eq!(cfg["port"], 11949);
}

#[tokio::test]
async fn rt_start_stop_creates_config_when_absent() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx_ws(&dir, None, None, Some(Arc::new(FakeClusterSvc::new())));

    let result = handler
        .handle_cmd("runtime.start", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["started"], true);
    let cfg_path = dir.path().join("config/config.cluster.json");
    let cfg: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(cfg["enabled"], true);
}

#[tokio::test]
async fn rt_start_stop_error_arms_and_no_workspace() {
    let handler = cluster::ClusterHandler::new();

    // 无 service → "cluster service not available"
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx_ws(&dir, None, None, None);
    let err = handler
        .handle_cmd("runtime.start", None, &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "cluster service not available");

    // start 失败 → "start failed: ..."
    let svc = Arc::new(FakeClusterSvc {
        running: AtomicBool::new(false),
        fail_start: true,
        fail_stop: false,
    });
    let ctx = ctx_ws(&dir, None, None, Some(svc));
    let err = handler
        .handle_cmd("runtime.start", None, &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("start failed"), "{err}");
    assert!(err.contains("start boom"), "{err}");

    // stop 失败 → "stop failed: ..."
    let svc = Arc::new(FakeClusterSvc {
        running: AtomicBool::new(false),
        fail_start: false,
        fail_stop: true,
    });
    let ctx = ctx_ws(&dir, None, None, Some(svc));
    let err = handler
        .handle_cmd("runtime.stop", None, &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("stop failed"), "{err}");

    // 无 workspace：跳过持久化但 service 操作照常成功，不写任何文件。
    let dir2 = tempfile::tempdir().unwrap();
    let ctx = make_deep_ctx(None, None, None, Some(Arc::new(FakeClusterSvc::new())));
    let result = handler
        .handle_cmd("runtime.start", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["started"], true);
    assert!(!dir2.path().join("config/config.cluster.json").exists());
}

// -----------------------------------------------------------------------
// nodes.ping（真实 TCP 探测）
// -----------------------------------------------------------------------

#[tokio::test]
async fn nodes_ping_unknown_node() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx_ws(&dir, Some(test_cluster(&dir)), None, None);

    let err = handler
        .handle_cmd(
            "nodes.ping",
            Some(serde_json::json!({"node_id":"ghost"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("node not found"), "{err}");
}

#[tokio::test]
async fn nodes_ping_success_marks_peer_healthy() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    // 本地监听器保持存活 → TCP connect 成功。
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let cluster = test_cluster(&dir);
    cluster.register_node(node("n1", "alpha", NodeRole::Worker, false, &addr));
    let ctx = ctx_ws(&dir, Some(cluster.clone()), None, None);

    let result = handler
        .handle_cmd(
            "nodes.ping",
            Some(serde_json::json!({"node_id":"n1"})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert!(result["latency"].is_u64());
    // 探测成功 → registry 标记 Online
    assert!(cluster.get_peer("n1").unwrap().is_online());
}

#[tokio::test]
async fn nodes_ping_refused_marks_peer_offline() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    // 先占后放 → 端口空闲，connect 被拒绝。
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    drop(listener);
    let cluster = test_cluster(&dir);
    cluster.register_node(node("n1", "alpha", NodeRole::Worker, true, &addr));
    let ctx = ctx_ws(&dir, Some(cluster.clone()), None, None);

    let err = handler
        .handle_cmd(
            "nodes.ping",
            Some(serde_json::json!({"node_id":"n1"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("ping failed"), "{err}");
    // 显式探测失败 → 立即翻 Offline
    assert!(!cluster.get_peer("n1").unwrap().is_online());
}

// -----------------------------------------------------------------------
// nodes.add / nodes.refresh
// -----------------------------------------------------------------------

#[tokio::test]
async fn nodes_add_registers_into_running_cluster() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let cluster = test_cluster(&dir);
    let ctx = ctx_ws(&dir, Some(cluster.clone()), None, None);

    let result = handler
        .handle_cmd(
            "nodes.add",
            Some(serde_json::json!({
                "address": "10.0.0.5:12000",
                "name": "Node-Five",
                "id": "n5",
                "role": "worker",
                "category": "edge",
            })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["added"], true);

    // peers.toml 写入
    let peers = std::fs::read_to_string(dir.path().join("cluster/peers.toml")).unwrap();
    assert!(peers.contains("[peers.n5]"), "{peers}");
    // 运行时注册（Offline）
    let registered = cluster.get_peer("n5").unwrap();
    assert!(!registered.is_online());
    assert_eq!(registered.base.address, "10.0.0.5:12000");
}

#[tokio::test]
async fn nodes_refresh_success_upgrades_placeholder() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let cluster = test_cluster(&dir);
    cluster.register_node(node(
        "ph",
        "placeholder",
        NodeRole::Worker,
        false,
        "10.1.2.3:9000",
    ));
    // 测试钩子：拦截 get_info，返回远端真实身份。
    cluster.set_call_with_context_fn(Box::new(|_peer, action, _payload| {
        assert_eq!(action, "get_info");
        Ok(serde_json::to_vec(&serde_json::json!({
            "node_id": "real-1",
            "name": "RealOne",
            "addresses": ["10.1.2.3"],
            "rpc_port": 9000,
            "role": "manager",
            "category": "dev",
            "capabilities": ["tools"],
            "node_type": "agent",
        }))
        .unwrap())
    }));
    let ctx = ctx_ws(&dir, Some(cluster.clone()), None, None);

    let result = handler
        .handle_cmd(
            "nodes.refresh",
            Some(serde_json::json!({"node_id":"ph"})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["refreshed"], true);
    assert_eq!(result["canonical_id"], "real-1");
    assert_eq!(result["upgraded_from_placeholder"], true);
    assert_eq!(result["node"]["id"], "real-1");
    assert_eq!(result["node"]["address"], "10.1.2.3:9000");
    assert_eq!(result["node"]["role"], "manager");
    // 占位符被移除，真实 ID 顶上且在线。
    assert!(cluster.get_peer("ph").is_none());
    let real = cluster.get_peer("real-1").unwrap();
    assert!(real.is_online());
    assert_eq!(real.base.name, "RealOne");
}

#[tokio::test]
async fn nodes_refresh_rpc_error_restores_status() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let cluster = test_cluster(&dir);
    cluster.register_node(node(
        "ph",
        "placeholder",
        NodeRole::Worker,
        false,
        "10.2.3.4:9000",
    ));
    cluster.set_call_with_context_fn(Box::new(|_p, _a, _payload| Err("boom".to_string())));
    let ctx = ctx_ws(&dir, Some(cluster.clone()), None, None);

    // 已注册（Offline）peer：失败后状态恢复为 Offline。
    let err = handler
        .handle_cmd(
            "nodes.refresh",
            Some(serde_json::json!({"node_id":"ph"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert_eq!(err, "RPC get_info failed: boom");
    assert!(matches!(
        cluster.get_peer("ph").unwrap().status,
        NodeStatus::Offline
    ));

    // 未注册 peer（original_status=None 分支）：同样报错，不 panic。
    let err = handler
        .handle_cmd(
            "nodes.refresh",
            Some(serde_json::json!({"node_id":"ghost"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert_eq!(err, "RPC get_info failed: boom");
}

#[tokio::test]
async fn nodes_refresh_timeout_clamped_to_one_sec() {
    use nemesis_cluster::rpc::client::{LocalNetworkInterface, PeerResolver, RpcClient};
    use nemesis_cluster::rpc_types::{ActionType, RPCRequest};

    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let cluster = test_cluster(&dir);
    cluster.register_node(node(
        "slow",
        "slow-peer",
        NodeRole::Worker,
        false,
        "127.0.0.1:1",
    ));

    // 前置原理：tokio::time::Timeout 只在「内层 future Pending 且 deadline 到」
    // 时返回 Elapsed；正常 RPC 路径内层自己的 timer 先臂先响（同一次 poll 级联
    // 里内层先臂、时长相同）→ 外层永远输。唯一能让外层赢的路径：内层卡在限流
    // 器 acquire_async 的 100ms 睡眠重试（Pending）。为此先用 30 次快速失败
    // 调用（resolver 返回 offline → 连接前即 Err，毫秒级 acquire+release）填满
    // 滑动窗口（30 次/10s/peer），随后 refresh 的 acquire 持续被拒 → 外层 1s 到。
    struct OfflineResolver;
    impl PeerResolver for OfflineResolver {
        fn get_peer_info(&self, _peer_id: &str) -> Option<(Vec<String>, u16, bool)> {
            Some((vec!["127.0.0.1".to_string()], 1, false))
        }
        fn get_local_interfaces(&self) -> Vec<LocalNetworkInterface> {
            Vec::new()
        }
        fn get_node_id(&self) -> String {
            "local-test".to_string()
        }
    }

    let client = Arc::new(RpcClient::with_resolver(Arc::new(OfflineResolver)));
    // 预热：30 次快速失败调用 → 窗口时间戳累积到上限（release 只还 token，
    // 不清窗口时间戳）
    for _ in 0..30 {
        let req = RPCRequest {
            id: uuid::Uuid::new_v4().to_string(),
            action: ActionType::Custom("get_info".to_string()),
            payload: serde_json::json!({}),
            source: "local-test".to_string(),
            target: Some("slow".to_string()),
        };
        let _ = client
            .call_with_timeout("slow", req, std::time::Duration::from_millis(500))
            .await;
    }

    cluster.set_rpc_client(client);
    let ctx = ctx_ws(&dir, Some(cluster.clone()), None, None);

    let started = std::time::Instant::now();
    let err = handler
        .handle_cmd(
            "nodes.refresh",
            Some(serde_json::json!({"node_id":"slow", "timeout_secs": 1})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert_eq!(err, "RPC get_info timed out (1s)");
    // 确实等了约 1s（clamp 到下限 1，而非默认 15）
    assert!(started.elapsed() >= std::time::Duration::from_secs(1));
    // 失败后恢复原状态（Offline）。
    assert!(matches!(
        cluster.get_peer("slow").unwrap().status,
        NodeStatus::Offline
    ));
}

#[tokio::test]
async fn nodes_refresh_parse_errors_and_address_fallbacks() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let cluster = test_cluster(&dir);
    let ctx = ctx_ws(&dir, Some(cluster.clone()), None, None);

    // 非 JSON 响应
    cluster.set_call_with_context_fn(Box::new(|_p, _a, _pl| Ok(b"not-json".to_vec())));
    let err = handler
        .handle_cmd(
            "nodes.refresh",
            Some(serde_json::json!({"node_id":"ph"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("parse get_info response"), "{err}");

    // 缺 node_id
    cluster.set_call_with_context_fn(Box::new(|_p, _a, _pl| {
        Ok(serde_json::to_vec(&serde_json::json!({"name":"x"})).unwrap())
    }));
    let err = handler
        .handle_cmd(
            "nodes.refresh",
            Some(serde_json::json!({"node_id":"ph"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert_eq!(err, "get_info missing node_id");

    // addresses 有值但无 rpc_port → primary = addresses[0] 原样
    cluster.set_call_with_context_fn(Box::new(|_p, _a, _pl| {
        Ok(serde_json::to_vec(&serde_json::json!({
            "node_id": "r-noport",
            "name": "NoPort",
            "addresses": ["10.9.9.9"],
        }))
        .unwrap())
    }));
    let result = handler
        .handle_cmd(
            "nodes.refresh",
            Some(serde_json::json!({"node_id":"ph"})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["node"]["address"], "10.9.9.9");

    // addresses 为空 → primary = 空串
    cluster.set_call_with_context_fn(Box::new(|_p, _a, _pl| {
        Ok(serde_json::to_vec(&serde_json::json!({
            "node_id": "r-empty",
            "name": "Empty",
        }))
        .unwrap())
    }));
    let result = handler
        .handle_cmd(
            "nodes.refresh",
            Some(serde_json::json!({"node_id":"ph"})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["node"]["address"], "");
}

// -----------------------------------------------------------------------
// nodes.detail / node.update_identity
// -----------------------------------------------------------------------

#[tokio::test]
async fn nodes_detail_master_worker_with_log_stats() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let cluster = test_cluster(&dir);
    cluster.register_node(node(
        "mA",
        "master-a",
        NodeRole::Coordinator,
        true,
        "10.0.0.1:12000",
    ));
    cluster.register_node(node(
        "nB",
        "worker-b",
        NodeRole::Worker,
        true,
        "10.0.0.2:12000",
    ));
    let log_dir = dir.path().join("cluster_logs");
    // task_assigned 的 data.action 是承接节点 id（reader 的映射约定）。
    write_today_cluster_log(
        &log_dir,
        &[
            log_event(
                "task_assigned",
                serde_json::json!({"task_id":"tA", "action":"mA"}),
            ),
            log_event("task_completed", serde_json::json!({"task_id":"tA"})),
            log_event("task_failed", serde_json::json!({"task_id":"tA"})),
        ],
    );
    let ctx = ctx_ws(
        &dir,
        Some(cluster),
        Some(log_dir.to_string_lossy().to_string()),
        None,
    );

    let master = handler
        .handle_cmd(
            "nodes.detail",
            Some(serde_json::json!({"node_id":"mA"})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(master["role"], "manager");
    assert_eq!(master["taskCount"], 1);
    assert_eq!(master["successCount"], 1);
    assert_eq!(master["failCount"], 1);
    assert_eq!(master["successRate"], 0.5);

    let worker = handler
        .handle_cmd(
            "nodes.detail",
            Some(serde_json::json!({"node_id":"nB"})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(worker["role"], "worker");
    assert!(worker["taskCount"].is_null()); // 无日志 → null

    let err = handler
        .handle_cmd(
            "nodes.detail",
            Some(serde_json::json!({"node_id":"ghost"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("node not found"), "{err}");
}

#[tokio::test]
async fn node_update_identity_creates_missing_peers_file() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx_ws(&dir, Some(test_cluster(&dir)), None, None);

    let result = handler
        .handle_cmd(
            "node.update_identity",
            Some(serde_json::json!({
                "name": "NewName",
                "role": "manager",
                "category": "dev",
                "tags": ["rust", " cluster ", ""],
            })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["name"], "NewName");
    assert_eq!(result["current_name"], "NewName");
    assert_eq!(result["current_role"], "manager");

    // peers.toml 不存在 → 走默认 StaticConfig 分支并新建文件
    let ppath = dir.path().join("cluster/peers.toml");
    assert!(ppath.exists());
    let content = std::fs::read_to_string(&ppath).unwrap();
    assert!(content.contains("NewName"), "{content}");
    assert!(content.contains("manager"), "{content}");
    assert!(content.contains("dev"), "{content}");
    // tags trim + 空串过滤
    let parsed = nemesis_cluster::cluster_config::load_static_config(&ppath).unwrap();
    assert_eq!(
        parsed.node.tags,
        vec!["rust".to_string(), "cluster".to_string()]
    );
}

#[tokio::test]
async fn node_update_identity_save_failure() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx_ws(&dir, Some(test_cluster(&dir)), None, None);
    // Cluster::with_workspace 会预建 ws/cluster 目录；先删掉再放同名文件，
    // 使 peers.toml 的父目录不可进入 → save_static_config 失败。
    std::fs::remove_dir_all(dir.path().join("cluster")).unwrap();
    std::fs::write(dir.path().join("cluster"), "not a dir").unwrap();

    let err = handler
        .handle_cmd(
            "node.update_identity",
            Some(serde_json::json!({"name": "X"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("failed to save peers.toml"), "{err}");
}

// -----------------------------------------------------------------------
// tasks.list / tasks.detail / tasks.submit
// -----------------------------------------------------------------------

#[tokio::test]
async fn tasks_list_states_string_payload_and_summaries() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let cluster = test_cluster(&dir);
    let log_dir = dir.path().join("cluster_logs");
    let ctx = ctx_ws(
        &dir,
        Some(cluster.clone()),
        Some(log_dir.to_string_lossy().to_string()),
        None,
    );

    let c = ctx.state.cluster.as_ref().unwrap();
    let t1 = c.submit_task("a", serde_json::json!({"content":"hi"}), "dashboard", "s1");
    c.complete_task(&t1, serde_json::json!({"ok":true}));
    let t2 = c.submit_task("a", serde_json::json!({}), "dashboard", "s2");
    c.fail_task(&t2, "boom");
    let t3 = c.submit_task("a", serde_json::json!({}), "dashboard", "s3");
    assert!(c.task_manager().assign_task(&t3, "n1"));
    let long_input = "x".repeat(250);
    let t4 = c.submit_task(
        "a",
        serde_json::json!(long_input.clone()),
        "dashboard",
        "s4",
    );

    // 日志聚合 fixture（在拿到 task id 后写，键到 t1）
    write_today_cluster_log(
        &log_dir,
        &[
            log_event("task_llm_start", serde_json::json!({"task_id": t1})),
            log_event("task_llm_start", serde_json::json!({"task_id": t1})),
            log_event(
                "task_tool_call",
                serde_json::json!({"task_id": t1, "tool": "read_file"}),
            ),
            log_event(
                "task_tool_call",
                serde_json::json!({"task_id": t1, "tool": "write_file"}),
            ),
        ],
    );

    let result = handler
        .handle_cmd("tasks.list", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["stats"]["queued"], 1); // t4
    assert_eq!(result["stats"]["running"], 1); // t3
    assert_eq!(result["stats"]["completed"], 1); // t1
    assert_eq!(result["stats"]["failed"], 1); // t2
    assert_eq!(result["total"], 4);

    let tasks = result["tasks"].as_array().unwrap();
    let find = |id: &str| {
        tasks
            .iter()
            .find(|t| t["id"] == serde_json::json!(id))
            .unwrap_or_else(|| panic!("task {id} missing"))
            .clone()
    };
    let e1 = find(&t1);
    assert_eq!(e1["status"], "completed");
    assert_eq!(e1["duration"], "0s"); // 两个时间戳都可解析
    assert_eq!(e1["rounds"], 2);
    assert_eq!(e1["toolCalls"], 2);
    let chain = e1["toolChain"].as_array().unwrap();
    assert_eq!(chain.len(), 2);
    assert_eq!(chain[0], "read_file");
    assert_eq!(chain[1], "write_file");

    let e4 = find(&t4);
    assert_eq!(e4["status"], "queued");
    // 250 字符输入截到 200 + "..."（truncate_str 回退臂）
    let input = e4["input"].as_str().unwrap();
    assert!(input.ends_with("..."), "{input}");
    assert_eq!(input.chars().count(), 203);
    assert_eq!(&input[..200], &long_input[..200]);
}

#[tokio::test]
async fn tasks_detail_enriched_from_log() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let cluster = test_cluster(&dir);
    let ctx = ctx_ws(&dir, Some(cluster.clone()), None, None);
    let c = ctx.state.cluster.as_ref().unwrap();
    let t1 = c.submit_task(
        "peer_chat",
        serde_json::json!({"content":"hi"}),
        "dashboard",
        "s1",
    );

    let result = handler
        .handle_cmd(
            "tasks.detail",
            Some(serde_json::json!({"task_id": t1})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["status"], "queued");
    assert_eq!(result["action"], "peer_chat");
    assert_eq!(result["source"], "dashboard");

    // 带日志目录 → rounds/toolCalls/toolChain 填充
    let log_dir = dir.path().join("cluster_logs");
    write_today_cluster_log(
        &log_dir,
        &[
            log_event("task_llm_start", serde_json::json!({"task_id": t1})),
            log_event(
                "task_tool_call",
                serde_json::json!({"task_id": t1, "tool": "grep"}),
            ),
        ],
    );
    let ctx2 = ctx_ws(
        &dir,
        Some(cluster.clone()),
        Some(log_dir.to_string_lossy().to_string()),
        None,
    );
    let result = handler
        .handle_cmd(
            "tasks.detail",
            Some(serde_json::json!({"task_id": t1})),
            &ctx2,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["rounds"], 1);
    assert_eq!(result["toolCalls"], 1);
    assert_eq!(result["toolChain"], serde_json::json!(["grep"]));

    let err = handler
        .handle_cmd(
            "tasks.detail",
            Some(serde_json::json!({"task_id":"ghost"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("task not found"), "{err}");
}

#[tokio::test]
async fn tasks_submit_regular_creates_dashboard_task() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let cluster = test_cluster(&dir);
    let ctx = ctx_ws(&dir, Some(cluster.clone()), None, None);

    let result = handler
        .handle_cmd(
            "tasks.submit",
            Some(serde_json::json!({"content": "hello task"})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["submitted"], true);
    let task_id = result["task_id"].as_str().unwrap().to_string();

    let task = cluster.get_task(&task_id).unwrap();
    assert_eq!(task.action, "dashboard_test");
    assert_eq!(task.payload, serde_json::json!({"content": "hello task"}));
    assert_eq!(task.original_channel, "dashboard");

    // 缺 content → Err
    let err = handler
        .handle_cmd("tasks.submit", Some(serde_json::json!({})), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing content");
}

#[tokio::test]
async fn tasks_submit_peer_chat_paths() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let cluster = test_cluster(&dir);
    let ctx = ctx_ws(&dir, Some(cluster.clone()), None, None);

    // 无 RPC client → Err（本地任务已建，但 RPC 无法发出）
    let err = handler
        .handle_cmd(
            "tasks.submit",
            Some(serde_json::json!({"content": "hi", "target_node_id": "n1"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert_eq!(err, "RPC client not available");

    // 注入无 resolver 的 RpcClient → Ok（后台发送任务失败仅 warn）
    cluster.set_rpc_client(Arc::new(RpcClient::new()));
    let result = handler
        .handle_cmd(
            "tasks.submit",
            Some(serde_json::json!({"content": "hi", "target_node_id": "n1"})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["submitted"], true);
    let task_id = result["task_id"].as_str().unwrap().to_string();
    let task = cluster.get_task(&task_id).unwrap();
    assert_eq!(task.action, "peer_chat");
    assert_eq!(task.peer_id, "n1");
}

// -----------------------------------------------------------------------
// topology
// -----------------------------------------------------------------------

#[tokio::test]
async fn topology_connections_traces_and_roles() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let cluster = test_cluster(&dir);
    cluster.register_node(node(
        "mA",
        "master-a",
        NodeRole::Coordinator,
        true,
        "10.0.0.1:12000",
    ));
    cluster.register_node(node(
        "nB",
        "worker-b",
        NodeRole::Worker,
        true,
        "10.0.0.2:12000",
    ));
    let log_dir = dir.path().join("cluster_logs");
    write_today_cluster_log(
        &log_dir,
        &[
            // outbound mA→nB：进 connections + traces
            log_event(
                "rpc_call",
                serde_json::json!({"direction":"outbound", "request_id":"req-1", "source":"mA", "target":"nB"}),
            ),
            // broadcast 目标：connections 跳过，但 traces 不筛 → 只进 traces
            log_event(
                "rpc_call",
                serde_json::json!({"direction":"outbound", "request_id":"req-2", "source":"local", "target":"broadcast"}),
            ),
            // inbound（无 source）：connections 跳过（空 source），traces 跳过（非 outbound）
            log_event(
                "rpc_call",
                serde_json::json!({"direction":"inbound", "request_id":"req-3", "target":"nB"}),
            ),
        ],
    );
    let ctx = ctx_ws(
        &dir,
        Some(cluster),
        Some(log_dir.to_string_lossy().to_string()),
        None,
    );

    let result = handler
        .handle_cmd("topology", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    let nodes = result["nodes"].as_array().unwrap();
    let roles: Vec<&str> = nodes.iter().map(|n| n["role"].as_str().unwrap()).collect();
    assert!(roles.contains(&"manager"));
    assert!(roles.contains(&"worker"));

    let conns = result["connections"].as_array().unwrap();
    assert_eq!(conns.len(), 1); // broadcast + 空 source 均被跳过
    assert_eq!(conns[0]["from"], "mA");
    assert_eq!(conns[0]["to"], "nB");

    let traces = result["traces"].as_array().unwrap();
    assert_eq!(traces.len(), 2); // req-1 + req-2（traces 不筛 broadcast）
    let ids: Vec<&str> = traces.iter().map(|t| t["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&"req-1"));
    assert!(ids.contains(&"req-2"));
}

// -----------------------------------------------------------------------
// config.get / config.save / config.set_master_enabled
// -----------------------------------------------------------------------

#[tokio::test]
async fn config_get_falls_back_to_static_peers() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("config")).unwrap();
    std::fs::create_dir_all(dir.path().join("cluster")).unwrap();
    std::fs::write(
        dir.path().join("config/config.cluster.json"),
        r#"{"port": 11950}"#,
    )
    .unwrap();
    std::fs::write(
        dir.path().join("cluster/peers.toml"),
        "[node]\nid = \"local-1\"\nname = \"LocalNode\"\nrole = \"worker\"\ncategory = \"dev\"\ntags = [\"a\"]\n",
    )
    .unwrap();
    // 无 Cluster 运行时 → 走 peers.toml 静态回退
    let ctx = ctx_ws(&dir, None, None, None);

    let result = handler
        .handle_cmd("config.get", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["port"], 11950);
    assert_eq!(result["node_id"], "local-1");
    assert_eq!(result["name"], "LocalNode");
    assert_eq!(result["role"], "worker");
    assert_eq!(result["category"], "dev");
    assert_eq!(result["master_enabled"], false); // home 无 config.json
}

#[tokio::test]
async fn config_save_fails_when_config_dir_is_file() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config"), "not a dir").unwrap();
    let ctx = ctx_ws(&dir, None, None, None);

    let err = handler
        .handle_cmd(
            "config.save",
            Some(serde_json::json!({"port": 11949})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("failed to create config dir"), "{err}");
}

#[tokio::test]
async fn config_set_master_enabled_non_object_cluster_and_missing_file() {
    let handler = cluster::ClusterHandler::new();

    // home 无 config.json → Err
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx_ws(&dir, None, None, None);
    let err = handler
        .handle_cmd(
            "config.set_master_enabled",
            Some(serde_json::json!({"enabled": true})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert_eq!(err, "config.json not found");

    // cluster 是非对象（数字）→ enabled 插不进去但整体 Ok
    std::fs::write(dir.path().join("config.json"), r#"{"cluster": 5}"#).unwrap();
    let result = handler
        .handle_cmd(
            "config.set_master_enabled",
            Some(serde_json::json!({"enabled": true})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["updated"], true);
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("config.json")).unwrap())
            .unwrap();
    assert_eq!(written["cluster"], 5); // 原样保留，未强插 enabled
}

// -----------------------------------------------------------------------
// firewall.check（AddrInUse 视为 pass）
// -----------------------------------------------------------------------

#[tokio::test]
async fn firewall_check_reports_addr_in_use_as_pass() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    // 占住一个 UDP 端口 + 一个 TCP 端口（保持存活），写入 cluster 配置。
    let udp = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
    let udp_port = udp.local_addr().unwrap().port();
    let tcp = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
    let tcp_port = tcp.local_addr().unwrap().port();
    std::fs::create_dir_all(dir.path().join("config")).unwrap();
    std::fs::write(
        dir.path().join("config/config.cluster.json"),
        serde_json::to_string(&serde_json::json!({"port": udp_port, "rpc_port": tcp_port}))
            .unwrap(),
    )
    .unwrap();
    let ctx = ctx_ws(&dir, None, None, None);

    let result = handler
        .handle_cmd("firewall.check", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["udp_port"], u64::from(udp_port));
    assert_eq!(result["tcp_port"], u64::from(tcp_port));
    let tests = result["tests"].as_array().unwrap();
    let find = |name: &str| {
        tests
            .iter()
            .find(|t| t["name"] == serde_json::json!(name))
            .unwrap_or_else(|| panic!("test {name} missing"))
            .clone()
    };
    // 端口被本测试占用 → AddrInUse → 视为“已被集群占用” pass
    let udp_bind = find("udp_bind");
    assert_eq!(udp_bind["pass"], true);
    assert!(
        udp_bind["detail"]
            .as_str()
            .unwrap()
            .contains("已被集群占用")
    );
    let tcp_bind = find("tcp_bind");
    assert_eq!(tcp_bind["pass"], true);
    assert!(
        tcp_bind["detail"]
            .as_str()
            .unwrap()
            .contains("已被集群占用")
    );
}

// -----------------------------------------------------------------------
// diagnostics.run
// -----------------------------------------------------------------------

#[tokio::test]
async fn diagnostics_run_gates_and_fails_fast() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let cluster = test_cluster(&dir);
    let ctx = ctx_ws(&dir, Some(cluster.clone()), None, None);

    // 缺 node_id / 缺 action
    let err = handler
        .handle_cmd("diagnostics.run", Some(serde_json::json!({})), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing node_id");
    let err = handler
        .handle_cmd(
            "diagnostics.run",
            Some(serde_json::json!({"node_id": "n1"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert_eq!(err, "missing action");

    // 有参但 cluster 无 rpc client
    let err = handler
        .handle_cmd(
            "diagnostics.run",
            Some(serde_json::json!({"node_id": "n1", "action": "ping"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert_eq!(err, "RPC client not available");

    // 注入无 resolver 的 RpcClient → 对未知 peer 快速失败
    cluster.set_rpc_client(Arc::new(RpcClient::new()));
    let err = handler
        .handle_cmd(
            "diagnostics.run",
            Some(serde_json::json!({"node_id": "n1", "action": "ping"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("RPC call failed"), "{err}");
}

// -----------------------------------------------------------------------
// persona_generate / persona_apply
// ---------------------------------------------------------------------------

use nemesis_providers::http_provider::{HttpProvider, HttpProviderConfig};

fn dummy_provider() -> Arc<HttpProvider> {
    Arc::new(HttpProvider::new(HttpProviderConfig {
        name: "dummy".into(),
        base_url: "http://127.0.0.1:1".into(),
        api_key: "k".into(),
        default_model: "test-model".into(),
        timeout_secs: 5,
        headers: std::collections::HashMap::new(),
        proxy: None,
        preserve_prefix: false,
    }))
}

/// 构造带 streaming_provider 的 ctx（state 一次性建好，避免事后改 Arc）。
fn ctx_ws_with_provider(
    dir: &tempfile::TempDir,
    cluster: Option<Arc<Cluster>>,
    service: Option<Arc<FakeClusterSvc>>,
    provider: Option<Arc<HttpProvider>>,
    home_config: Option<&str>,
) -> RequestContext {
    let ws = dir.path().to_string_lossy().to_string();
    if let Some(cfg) = home_config {
        std::fs::write(dir.path().join("config.json"), cfg).unwrap();
    }
    let state = AppState {
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
        streaming_provider: provider,
        ws_router: None,
        agent_service: None,
        data_store: None,
        memory_manager: None,
        forge: None,
        agent_loop: Arc::new(parking_lot::RwLock::new(None)),
        cluster,
        cluster_service: service
            .map(|s| s as Arc<dyn nemesis_services::bot_service::LifecycleService>),
        cluster_log_dir: None,
        workflow_engine: None,
        #[cfg(feature = "workflow")]
        chat_secret_store: std::sync::Arc::new(
            nemesis_workflow::chat_secrets::ChatSecretStore::in_memory(),
        ),
        #[cfg(not(feature = "workflow"))]
        chat_secret_store: std::sync::Arc::new(()),
        #[cfg(feature = "workflow")]
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        #[cfg(not(feature = "workflow"))]
        webhook_rate_limiter: Arc::new(()),
        internal_cmd_tx: None,
        estop: None,
        cron: None,
        board: None,
    };
    RequestContext {
        session_id: "test-session".to_string(),
        chat_id: "test-chat".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state: Arc::new(state),
        auth_method: crate::session::AuthMethod::default(),
    }
}

const GEN_TEXT: &str = "这是一段用于测试的足够长的岗位描述文本，描述一个基于消息队列的后端架构师岗位，要求熟悉 RocketMQ 事务消息与分布式事务一致性方案，超过四十个字符。";

#[tokio::test]
async fn persona_generate_validation_and_tier_gates() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();

    // 缺 kind / 非法 kind / 缺 text
    let ctx = ctx_ws(&dir, None, None, None);
    let err = handler
        .handle_cmd("persona_generate", Some(serde_json::json!({})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("missing 'kind'"), "{err}");
    let err = handler
        .handle_cmd(
            "persona_generate",
            Some(serde_json::json!({"kind": "foo", "text": GEN_TEXT})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert_eq!(err, "kind must be 'jd' or 'resume'");
    let err = handler
        .handle_cmd(
            "persona_generate",
            Some(serde_json::json!({"kind": "jd"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert_eq!(err, "missing 'text'");

    // 无 provider
    let err = handler
        .handle_cmd(
            "persona_generate",
            Some(serde_json::json!({"kind": "jd", "text": GEN_TEXT})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("未配置 LLM 模型"), "{err}");

    // 有 provider + mini tier → 拒绝
    let ctx = ctx_ws_with_provider(
        &dir,
        None,
        None,
        Some(dummy_provider()),
        Some(r#"{"model_list":[{"model_name":"test-model","model_tier":"mini"}]}"#),
    );
    let err = handler
        .handle_cmd(
            "persona_generate",
            Some(serde_json::json!({"kind": "jd", "text": GEN_TEXT})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("mini"), "{err}");

    // config.json 缺失 → 读取失败（新 tempdir：避免上个子例已写入的文件干扰）
    let dir2 = tempfile::tempdir().unwrap();
    let ctx = ctx_ws_with_provider(&dir2, None, None, Some(dummy_provider()), None);
    let err = handler
        .handle_cmd(
            "persona_generate",
            Some(serde_json::json!({"kind": "jd", "text": GEN_TEXT})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("读取 config.json 失败"), "{err}");

    // config.json 非法 JSON → 解析失败
    std::fs::write(dir2.path().join("config.json"), "not json").unwrap();
    let err = handler
        .handle_cmd(
            "persona_generate",
            Some(serde_json::json!({"kind": "jd", "text": GEN_TEXT})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("解析 config.json 失败"), "{err}");
}

#[tokio::test]
async fn persona_generate_success_via_mock_llm() {
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();

    let server = MockServer::start().await;
    fn llm_args(args: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [{
                        "id": "call-1",
                        "type": "function",
                        "function": { "name": "stage_tool", "arguments": args.to_string() }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        })
    }
    async fn mount(server: &MockServer, marker: &str, args: serde_json::Value) {
        let template = ResponseTemplate::new(200).set_body_json(llm_args(args));
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(body_string_contains(marker.to_string()))
            .respond_with(template)
            .expect(1)
            .mount(server)
            .await;
    }
    // 三阶段：分析师（提取）→ 人格设计师（创作）→ 完整性审计员（审计）
    mount(
        &server,
        "分析师",
        serde_json::json!({
            "units": [{
                "id": "u1", "content": "RocketMQ 方案", "unit_type": "tech_decision",
                "relevance": "high", "disposition": "identity", "key_entities": ["RocketMQ"]
            }],
            "segments": [{ "id": "s1", "label": "项目", "unit_count": 1 }],
        }),
    )
    .await;
    mount(
        &server,
        "人格设计师",
        serde_json::json!({
            "node_name": "mq-architect", "display_name": "MQ 架构师", "emoji": "🚀",
            "role": "worker", "category": "development", "tags": ["RocketMQ"],
            "identity_md": "# 定位\n熟悉 RocketMQ 的事端架构师",
            "soul_md": "# 工作哲学\n以消息可靠性为锚点",
            "expertise_md": "",
        }),
    )
    .await;
    mount(
        &server,
        "完整性审计员",
        serde_json::json!({"entries": [{"unit_id": "u1", "status": "covered"}]}),
    )
    .await;

    let provider = Arc::new(HttpProvider::new(HttpProviderConfig {
        name: "mock".into(),
        base_url: server.uri(),
        api_key: "test-key".into(),
        default_model: "test-model".into(),
        timeout_secs: 15,
        headers: std::collections::HashMap::new(),
        proxy: None,
        preserve_prefix: false,
    }));
    let ctx = ctx_ws_with_provider(
        &dir,
        None,
        None,
        Some(provider),
        Some(r#"{"model_list":[{"model_name":"test-model","model_tier":"normal"}]}"#),
    );

    let result = handler
        .handle_cmd(
            "persona_generate",
            Some(serde_json::json!({"kind": "jd", "text": GEN_TEXT})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["node_name"], "mq-architect");
    assert_eq!(result["display_name"], "MQ 架构师");
    // 完整流程产出 coverage 报告（u1 covered、无 missing、无硬缺口）
    assert_eq!(result["coverage"]["total"], 1);
    assert_eq!(result["coverage"]["covered"], 1);
    assert_eq!(result["coverage"]["missing"], 0);
    assert!(result["coverage"].get("segment_gaps").is_none()); // 空 → skip 序列化
}

// persona_apply 夹具：完整合法人格包。
fn apply_pkg() -> serde_json::Value {
    serde_json::json!({
        "node_name": "mq-architect",
        "display_name": "MQ 架构师",
        "emoji": "🚀",
        "role": "worker",
        "category": "development",
        "tags": ["RocketMQ"],
        "identity_md": "# 定位\n消息架构师",
        "soul_md": "# 工作哲学\n可靠优先",
        "expertise_md": "",
    })
}

#[tokio::test]
async fn persona_apply_invalid_pkg_and_validation() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx_ws(&dir, Some(test_cluster(&dir)), None, None);

    // 非法 JSON 结构 → 无法反序列化
    let err = handler
        .handle_cmd("persona_apply", Some(serde_json::json!({"foo": 1})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("无效的人格包"), "{err}");

    // 反序列化成功但 role 非法 → validate 失败
    let mut pkg = apply_pkg();
    pkg["role"] = serde_json::json!("admin");
    let err = handler
        .handle_cmd("persona_apply", Some(pkg), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("role"), "{err}");
}

#[tokio::test]
async fn persona_apply_requires_workspace_and_cluster() {
    let handler = cluster::ClusterHandler::new();

    // 无 workspace
    let ctx = make_deep_ctx(None, None, None, None);
    let err = handler
        .handle_cmd("persona_apply", Some(apply_pkg()), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("workspace not configured"), "{err}");

    // 有 workspace、无 Cluster：人格文件先写，随后报 cluster 不可用
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx_ws(&dir, None, None, None);
    let err = handler
        .handle_cmd("persona_apply", Some(apply_pkg()), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "cluster not available");
    assert!(dir.path().join("cluster/IDENTITY.md").exists());
    assert!(dir.path().join("cluster/SOUL.md").exists());
}

#[tokio::test]
async fn persona_apply_full_ok_without_service() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let cluster = test_cluster(&dir);
    let ctx = ctx_ws(&dir, Some(cluster.clone()), None, None);

    let result = handler
        .handle_cmd("persona_apply", Some(apply_pkg()), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["applied"], true);
    assert_eq!(result["reloaded"], false);
    assert!(result["note"].as_str().unwrap().contains("集群服务不可用"));

    // 人格文件落盘
    let identity = std::fs::read_to_string(dir.path().join("cluster/IDENTITY.md")).unwrap();
    assert!(identity.contains("消息架构师"));
    let soul = std::fs::read_to_string(dir.path().join("cluster/SOUL.md")).unwrap();
    assert!(soul.contains("可靠优先"));
    // expertise_md 为空 → 不产出 EXPERTISE.md
    assert!(!dir.path().join("cluster/EXPERTISE.md").exists());
    // peers.toml [node] 持久化
    let peers = std::fs::read_to_string(dir.path().join("cluster/peers.toml")).unwrap();
    assert!(peers.contains("MQ 架构师"), "{peers}");
    assert!(peers.contains("worker"), "{peers}");
    // 运行时身份同步
    assert_eq!(cluster.node_name(), "MQ 架构师");
}

#[tokio::test]
async fn persona_apply_service_not_running_note() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx_ws(
        &dir,
        Some(test_cluster(&dir)),
        None,
        Some(Arc::new(FakeClusterSvc::new())),
    );

    let result = handler
        .handle_cmd("persona_apply", Some(apply_pkg()), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["reloaded"], false);
    assert!(result["note"].as_str().unwrap().contains("集群当前未运行"));
}

#[tokio::test]
async fn persona_apply_service_restart_success_and_failure() {
    let handler = cluster::ClusterHandler::new();

    // service 运行中 + 重启成功 → reloaded=true
    let dir = tempfile::tempdir().unwrap();
    let svc = Arc::new(FakeClusterSvc::new());
    svc.running.store(true, std::sync::atomic::Ordering::SeqCst);
    let ctx = ctx_ws(&dir, Some(test_cluster(&dir)), None, Some(svc));
    let result = handler
        .handle_cmd("persona_apply", Some(apply_pkg()), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["reloaded"], true);
    assert_eq!(result["note"], "");

    // service 运行中 + stop 成功但 start 失败 → note 带失败说明
    let dir = tempfile::tempdir().unwrap();
    let svc = Arc::new(FakeClusterSvc {
        running: AtomicBool::new(true),
        fail_start: true,
        fail_stop: false,
    });
    let ctx = ctx_ws(&dir, Some(test_cluster(&dir)), None, Some(svc));
    let result = handler
        .handle_cmd("persona_apply", Some(apply_pkg()), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["reloaded"], false);
    assert!(
        result["note"].as_str().unwrap().contains("失败"),
        "{}",
        result["note"]
    );
}

#[tokio::test]
async fn persona_apply_expertise_written_and_corrupt_peers_fails() {
    let handler = cluster::ClusterHandler::new();

    // expertise_md 非空 → EXPERTISE.md 产出
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx_ws(&dir, Some(test_cluster(&dir)), None, None);
    let mut pkg = apply_pkg();
    pkg["expertise_md"] = serde_json::json!("# RocketMQ 方案\n半消息机制");
    let result = handler
        .handle_cmd("persona_apply", Some(pkg), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["applied"], true);
    let expertise = std::fs::read_to_string(dir.path().join("cluster/EXPERTISE.md")).unwrap();
    assert!(expertise.contains("半消息机制"));

    // peers.toml 损坏 → 加载失败
    // （必须在 test_cluster 之后写：Cluster::with_workspace 加载失败时会用
    //   默认值覆写 peers.toml，先写会被冲掉）
    let dir = tempfile::tempdir().unwrap();
    let cluster = test_cluster(&dir);
    std::fs::write(dir.path().join("cluster/peers.toml"), "invalid [[[ toml").unwrap();
    let ctx = ctx_ws(&dir, Some(cluster), None, None);
    let err = handler
        .handle_cmd("persona_apply", Some(apply_pkg()), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("加载 peers.toml 失败"), "{err}");
}

// -----------------------------------------------------------------------
// R4 覆盖率（2026-08-27）：tasks_list/tasks_detail 的 Cancelled / Running
// 状态映射臂（此前 map_status 的 Cancelled→"failed" 与 detail 的 Running
// 分支未走到）。Cancelled 任务没有公开 API 能自然产生（tasks.cancel 直接
// delete），用 TaskManager::submit 全量 Task 构造钉住映射语义。
// -----------------------------------------------------------------------

#[tokio::test]
async fn tasks_list_maps_cancelled_status_to_failed() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let cluster = test_cluster(&dir);
    let ctx = ctx_ws(&dir, Some(cluster.clone()), Some(ws.clone()), None);

    use nemesis_types::cluster::{Task, TaskStatus};
    let cancelled = Task {
        id: "t-cancelled".to_string(),
        status: TaskStatus::Cancelled,
        action: "peer_chat".to_string(),
        peer_id: "n1".to_string(),
        payload: serde_json::json!({}),
        result: None,
        original_channel: "dashboard".to_string(),
        original_chat_id: "c1".to_string(),
        created_at: chrono::Local::now().to_rfc3339(),
        completed_at: Some(chrono::Local::now().to_rfc3339()),
    };
    cluster.task_manager().submit(cancelled).unwrap();

    let out = handler
        .handle_cmd("tasks.list", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    let tasks = out["tasks"].as_array().unwrap();
    let hit = tasks.iter().find(|t| t["id"] == "t-cancelled").unwrap();
    assert_eq!(hit["status"], "failed", "cancelled must map to failed");
}

#[tokio::test]
async fn tasks_detail_reports_running_status() {
    let handler = cluster::ClusterHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let cluster = test_cluster(&dir);
    cluster.register_node(node(
        "n1",
        "alpha",
        NodeRole::Worker,
        true,
        "10.0.0.1:12000",
    ));
    let ctx = ctx_ws(&dir, Some(cluster.clone()), Some(ws.clone()), None);

    let id = cluster.submit_task(
        "peer_chat",
        serde_json::json!({"content":"hi"}),
        "dashboard",
        "s1",
    );
    assert!(cluster.task_manager().assign_task(&id, "n1"));

    let out = handler
        .handle_cmd(
            "tasks.detail",
            Some(serde_json::json!({ "task_id": id })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["status"], "running");
}
