//! cluster_service 单测：适配器裸构造 + 未运行时 stop() 的无副作用短路。
//!
//! first_start() / LifecycleService::start() 需要装配完整 cluster agent
//! （build_cluster_agent_loop + tokio spawn + 组件重启），停/起真组件属
//! 结构性（cluster-uat 的 T 系列真机链路覆盖）；这里钉生命周期状态机的
//! 确定性边界。

#![cfg(feature = "cluster")]

use std::sync::Arc;

use nemesis_cluster::types::ClusterConfig;
use nemesis_services::LifecycleService;

use super::*;

#[tokio::test]
async fn new_adapter_reports_not_running() {
    let tmp = tempfile::tempdir().unwrap();
    let cluster = Arc::new(nemesis_cluster::cluster::Cluster::new(ClusterConfig {
        node_id: "test-node".to_string(),
        bind_address: "127.0.0.1:0".to_string(),
        peers: Vec::new(),
    }));
    let shared = Arc::new(crate::agent_factory::SharedResources {
        home: tmp.path().to_path_buf(),
        ..Default::default()
    });
    let adapter = ClusterServiceAdapter::new(
        cluster,
        shared,
        tokio::runtime::Handle::current(),
        tmp.path().to_path_buf(),
        Arc::new(ClusterTaskList::new(tmp.path().join("cluster_tasks"))),
        Arc::new(ClusterWorkQueue::new(8)),
    );

    // 固有方法 + trait 实现都报未运行。
    assert!(!ClusterServiceAdapter::is_running(&adapter));
    assert!(!LifecycleService::is_running(&adapter));
}

#[tokio::test]
async fn stop_when_not_running_is_ok_without_side_effects() {
    let tmp = tempfile::tempdir().unwrap();
    let cluster = Arc::new(nemesis_cluster::cluster::Cluster::new(ClusterConfig {
        node_id: "test-node".to_string(),
        bind_address: "127.0.0.1:0".to_string(),
        peers: Vec::new(),
    }));
    let shared = Arc::new(crate::agent_factory::SharedResources {
        home: tmp.path().to_path_buf(),
        ..Default::default()
    });
    let adapter = ClusterServiceAdapter::new(
        cluster,
        shared,
        tokio::runtime::Handle::current(),
        tmp.path().to_path_buf(),
        Arc::new(ClusterTaskList::new(tmp.path().join("cluster_tasks"))),
        Arc::new(ClusterWorkQueue::new(8)),
    );

    // 未运行时 stop() 必须短路 Ok（幂等停机），不触碰 cluster 内部。
    LifecycleService::stop(&adapter).expect("stop on !running → Ok");
    assert!(!LifecycleService::is_running(&adapter));
    // 再停一遍仍然 Ok（重复停机幂等）。
    LifecycleService::stop(&adapter).expect("second stop → Ok");
}

// =========================================================================
// S11d 补测（quality-hardening goal 冲刺 S11）：first_start 三分支
// （恢复磁盘任务 + 构建 agent 成功 / 构建失败 lenient）+ LifecycleService
// start/stop 全路径（组件重启 + ClusterRpcTool 使能旗标翻转 + 幂等）。
// 全部离线：Cluster::new 不绑口；discovery 起不来也只是 warn 不致命。
// =========================================================================

use std::sync::atomic::AtomicBool;

use nemesis_cluster::cluster_task::{ClusterTask, TaskSource, TaskStatus};

/// 离线可构建的迷你模型 config（与 agent_factory/tests.rs 同源形态）。
fn write_cluster_model_config(home: &std::path::Path) {
    let cfg = serde_json::json!({
        "agents": { "defaults": { "llm": "mini-model", "max_tool_iterations": 5 } },
        "model_list": [{
            "model_name": "mini-model",
            "model": "testai/mini-model",
            "api_key": "test-key",
            "api_base": "http://127.0.0.1:9",
            "model_tier": "mini"
        }]
    });
    std::fs::create_dir_all(home).unwrap();
    std::fs::write(home.join("config.json"), cfg.to_string()).unwrap();
}

fn make_pending_task(task_id: &str) -> ClusterTask {
    ClusterTask {
        task_id: task_id.to_string(),
        source: TaskSource {
            node_id: "node-b".to_string(),
            rpc_address: "127.0.0.1:9".to_string(),
            session_key: String::new(),
        },
        status: TaskStatus::Pending,
        content: "hello".to_string(),
        conversation: None,
        waiting_for_task_id: None,
        waiting_tool_call_id: None,
        callback_result: None,
    }
}

/// 组一个带 ClusterRpcTool 使能旗标的 shared（可观察 start/stop 的旗标翻转）。
fn make_shared_with_flag(home: &std::path::Path) -> Arc<crate::agent_factory::SharedResources> {
    let (outbound_tx, _rx) = tokio::sync::mpsc::channel(16);
    Arc::new(crate::agent_factory::SharedResources {
        home: home.to_path_buf(),
        agent_outbound_tx: outbound_tx,
        cron_service: Arc::new(std::sync::Mutex::new(nemesis_cron::service::CronService::new(
            "",
        ))),
        mcp_config_path: home.join("nonexistent-mcp.json"),
        cluster_rpc_enabled: parking_lot::RwLock::new(Some(Arc::new(AtomicBool::new(false)))),
        ..Default::default()
    })
}

fn make_adapter(
    home: &std::path::Path,
    task_dir: &std::path::Path,
) -> (
    ClusterServiceAdapter,
    Arc<nemesis_cluster::cluster::Cluster>,
    Arc<crate::agent_factory::SharedResources>,
    Arc<ClusterTaskList>,
    Arc<ClusterWorkQueue>,
    Arc<AtomicBool>,
) {
    let cluster = Arc::new(nemesis_cluster::cluster::Cluster::new(ClusterConfig {
        node_id: "test-node".to_string(),
        bind_address: "127.0.0.1:0".to_string(),
        peers: Vec::new(),
    }));
    let shared = make_shared_with_flag(home);
    let flag = shared
        .cluster_rpc_enabled
        .read()
        .clone()
        .expect("flag set above");
    let task_list = Arc::new(ClusterTaskList::new(task_dir));
    let work_queue = Arc::new(ClusterWorkQueue::new(8));
    let adapter = ClusterServiceAdapter::new(
        cluster.clone(),
        shared.clone(),
        tokio::runtime::Handle::current(),
        home.to_path_buf(),
        task_list.clone(),
        work_queue.clone(),
    );
    (adapter, cluster, shared, task_list, work_queue, flag)
}

#[tokio::test]
async fn first_start_with_valid_config_marks_running_and_enables_rpc_tool() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    write_cluster_model_config(&home);
    let (adapter, _cluster, _shared, _tl, _wq, flag) =
        make_adapter(&home, &tmp.path().join("tasks"));

    assert!(!flag.load(std::sync::atomic::Ordering::Relaxed));
    adapter.first_start().expect("first_start with valid config");
    assert!(ClusterServiceAdapter::is_running(&adapter));
    assert!(LifecycleService::is_running(&adapter));
    // ClusterRpcTool 使能旗标被置 true。
    assert!(flag.load(std::sync::atomic::Ordering::Relaxed));
}

#[tokio::test]
async fn first_start_with_bad_config_is_lenient_running_without_agent() {
    let tmp = tempfile::tempdir().unwrap();
    // 不写 config.json → build_cluster_agent_loop 失败（默认模型无 key）→
    // 现行为：handle=None 但 running=true（lenient，不回滚状态）。
    let (adapter, _cluster, _shared, _tl, _wq, flag) =
        make_adapter(&tmp.path().join("home"), &tmp.path().join("tasks"));

    adapter.first_start().expect("first_start never errors today");
    assert!(LifecycleService::is_running(&adapter), "lenient: still running");
    assert!(flag.load(std::sync::atomic::Ordering::Relaxed));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_start_recovers_persisted_tasks_and_resubmits_them() {
    let tmp = tempfile::tempdir().unwrap();
    let task_dir = tmp.path().join("tasks");
    std::fs::create_dir_all(&task_dir).unwrap();

    // 种子：前一个进程留下 Pending 任务 + WaitingRemote 任务（磁盘持久化）。
    let seeder = ClusterTaskList::new(&task_dir);
    seeder.create_task(make_pending_task("recover-pending"));
    let mut waiting = make_pending_task("recover-waiting");
    waiting.status = TaskStatus::WaitingRemote;
    seeder.create_task(waiting);
    seeder
        .persist_to_disk()
        .expect("seed persist must succeed");
    drop(seeder);

    // 用坏 config（无 config.json）让 agent 构建失败 → 没有 agent 消费队列，
    // 恢复重提的任务会留在 work queue 里可被直接观察。
    let (adapter, _cluster, _shared, task_list, work_queue, _flag) =
        make_adapter(&tmp.path().join("home"), &task_dir);
    adapter.first_start().expect("first_start");

    // WaitingRemote → Pending 归一化（恢复语义）。
    let restored = task_list.get_task("recover-waiting").expect("restored");
    assert_eq!(restored.status, TaskStatus::Pending);
    assert_eq!(task_list.get_task("recover-pending").unwrap().status, TaskStatus::Pending);

    // 两个恢复任务都被重新提交进 work queue（顺序按提交先后）。
    let mut seen = vec![work_queue.next().await, work_queue.next().await];
    seen.sort();
    assert_eq!(
        seen,
        vec![Some("recover-pending".to_string()), Some("recover-waiting".to_string())]
    );
    // 注：ClusterWorkQueue 自持 sender，next() 不会返回 None —— 不能断言队列排空。
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lifecycle_start_stop_flips_flag_and_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    write_cluster_model_config(&home);
    let (adapter, _cluster, _shared, _tl, _wq, flag) =
        make_adapter(&home, &tmp.path().join("tasks"));

    // start：组件重启 + agent spawn + 旗标 true。
    adapter
        .start()
        .expect("start must succeed (offline cluster, no rpc server set)");
    assert!(LifecycleService::is_running(&adapter));
    assert!(flag.load(std::sync::atomic::Ordering::Relaxed));

    // 幂等：already-running 分支。
    adapter.start().expect("second start → Ok skip");

    // stop：旗标 false + running false。
    adapter.stop().expect("stop must succeed");
    assert!(!LifecycleService::is_running(&adapter));
    assert!(!flag.load(std::sync::atomic::Ordering::Relaxed));

    // 幂等：already-stopped 分支。
    adapter.stop().expect("second stop → Ok skip");

    // 停后再 start（重启路径，block_in_place + rt.block_on 全链再来一遍）。
    adapter.start().expect("restart after stop must succeed");
    assert!(LifecycleService::is_running(&adapter));
    assert!(flag.load(std::sync::atomic::Ordering::Relaxed));
    adapter.stop().expect("final stop");
}

// =========================================================================
// wave_c 补测（coverage 补测批次 C）：first_start / start / stop 的错误与
// 降级臂。
//
// 目标 miss：restore_from_disk 失败 warn（lenient 不致命）、恢复重提撞满
// 队列容量的 submit-Err 分支、start_cluster_components 里 RPC server
// 绑定失败向上传播、shared 无 ClusterRpcTool 旗标（None）时 start/stop
// 全程容忍、agent 构建失败后 stop() 拿不到 handle 的 None 臂。
// 全部离线：LLM 指向 127.0.0.1:9（立即拒绝），UDP/RPC 绑定口冲突用
// 本进程先占端口的方式确定性构造。
// =========================================================================

/// 种子 N 个 Pending 任务并持久化到磁盘（供 restore_from_disk 恢复）。
fn seed_pending_tasks(task_dir: &std::path::Path, ids: &[&str]) {
    let seeder = ClusterTaskList::new(task_dir);
    for id in ids {
        seeder.create_task(make_pending_task(id));
    }
    seeder.persist_to_disk().expect("seed persist must succeed");
}

/// restore_from_disk 读损坏的 tasks.json 索引失败 → first_start 只 warn
/// 不失败（lenient），running 照常置位。
#[tokio::test]
async fn wave_c_first_start_lenient_when_tasks_index_is_corrupt() {
    let tmp = tempfile::tempdir().unwrap();
    let task_dir = tmp.path().join("tasks");
    let index_dir = task_dir.join("cluster");
    std::fs::create_dir_all(&index_dir).unwrap();
    std::fs::write(index_dir.join("tasks.json"), "{{{ corrupt").unwrap();

    // 坏 config（无 config.json）→ agent 构建走 lenient 分支；两处 lenient
    // 叠加也不影响 Ok。
    let (adapter, _cluster, _shared, _tl, _wq, _flag) =
        make_adapter(&tmp.path().join("home"), &task_dir);
    adapter.first_start().expect("restore failure must be lenient");
    assert!(LifecycleService::is_running(&adapter));
}

/// 恢复重提撞满队列容量：容量 8、磁盘上 10 个 Pending 任务、无消费者
/// （坏 config → agent 构建失败）→ 前 8 个入队成功、其余 submit Err
/// （内部只 warn）。可观察结果：队列里恰好能抽到 8 个 id。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wave_c_first_start_resubmit_overflow_is_bounded_by_queue_capacity() {
    const CAPACITY: usize = 8;
    let tmp = tempfile::tempdir().unwrap();
    let task_dir = tmp.path().join("tasks");
    std::fs::create_dir_all(&task_dir).unwrap();
    let ids: Vec<String> = (0..CAPACITY + 2).map(|i| format!("wc-task-{i}")).collect();
    let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    seed_pending_tasks(&task_dir, &id_refs);

    let (adapter, _cluster, _shared, task_list, work_queue, _flag) =
        make_adapter(&tmp.path().join("home"), &task_dir);
    adapter.first_start().expect("first_start");

    // 全部恢复成 Pending。
    for id in &ids {
        let t = task_list.get_task(id).expect("restored task present");
        assert_eq!(t.status, TaskStatus::Pending);
    }
    // 队列有界：恰好 CAPACITY 个被接受，多出的两个被 try_send 拒绝（warn 内吞）。
    // 注意：ClusterWorkQueue 自持 sender，next() 永不返回 None（见 S11d 注），
    // 排空判定必须用超时而非等 None。
    let mut drained = Vec::new();
    for _ in 0..ids.len() + 4 {
        match tokio::time::timeout(
            std::time::Duration::from_millis(500),
            work_queue.next(),
        )
        .await
        {
            Ok(Some(id)) => drained.push(id),
            _ => break,
        }
    }
    assert_eq!(drained.len(), CAPACITY, "queue must accept exactly capacity items");
    // 抽到的都是合法任务 id 且无重复。
    drained.sort();
    drained.dedup();
    assert_eq!(drained.len(), CAPACITY);

    // 清理：停掉可能存在的 agent 任务（本例 agent 未构建，但保持对称）。
    LifecycleService::stop(&adapter).ok();
}

/// start() 里 RPC server 绑定到已被占用的端口 → 错误向上传播为
/// "RPC server start: ..."，且 running 保持 false（无半开状态泄漏）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wave_c_start_fails_when_rpc_port_is_already_bound() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    write_cluster_model_config(&home);

    // 先在本进程占住一个端口（最可靠的确定性口冲突构造）。
    let blocker = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let busy_port = blocker.local_addr().unwrap().port();

    let mut cluster = nemesis_cluster::cluster::Cluster::new(ClusterConfig {
        node_id: "wc-busy-port".to_string(),
        bind_address: "127.0.0.1:0".to_string(),
        peers: Vec::new(),
    });
    cluster.set_rpc_server(Arc::new(
        nemesis_cluster::rpc::server::RpcServer::new(
            nemesis_cluster::rpc::server::RpcServerConfig {
                bind_address: format!("127.0.0.1:{busy_port}"),
                ..Default::default()
            },
        ),
    ));
    let cluster = Arc::new(cluster);

    let shared = make_shared_with_flag(&home);
    let task_list = Arc::new(ClusterTaskList::new(tmp.path().join("tasks")));
    let work_queue = Arc::new(ClusterWorkQueue::new(8));
    let adapter = ClusterServiceAdapter::new(
        cluster,
        shared,
        tokio::runtime::Handle::current(),
        home,
        task_list,
        work_queue,
    );

    let err = adapter.start().expect_err("busy rpc port must fail start");
    assert!(
        err.contains("RPC server start"),
        "error must identify the failing component, got: {err}"
    );
    assert!(!LifecycleService::is_running(&adapter));
    drop(blocker);
}

/// shared 没有 ClusterRpcTool 使能旗标（None）+ agent 构建失败（无
/// config.json）→ start/stop 都要容忍：running 正常翻转，stop 在拿不到
/// agent handle 时走 None 臂不 panic。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wave_c_lifecycle_tolerates_flagless_shared_and_absent_agent_handle() {
    let tmp = tempfile::tempdir().unwrap();
    let cluster = Arc::new(nemesis_cluster::cluster::Cluster::new(ClusterConfig {
        node_id: "wc-flagless".to_string(),
        bind_address: "127.0.0.1:0".to_string(),
        peers: Vec::new(),
    }));
    // Default shared：cluster_rpc_enabled 为 None。
    let shared = Arc::new(crate::agent_factory::SharedResources {
        home: tmp.path().to_path_buf(),
        ..Default::default()
    });
    assert!(shared.cluster_rpc_enabled.read().is_none());
    let adapter = ClusterServiceAdapter::new(
        cluster,
        shared,
        tokio::runtime::Handle::current(),
        tmp.path().to_path_buf(),
        Arc::new(ClusterTaskList::new(tmp.path().join("tasks"))),
        Arc::new(ClusterWorkQueue::new(8)),
    );

    // agent 构建 lenient-Ok(None) → running=true 但 handle 缺席。
    adapter.start().expect("start with bad config is lenient");
    assert!(LifecycleService::is_running(&adapter));

    // stop：handle.take()==None 的臂 + 旗标缺席的臂都要无害通过。
    adapter.stop().expect("stop tolerates absent handle");
    assert!(!LifecycleService::is_running(&adapter));
}

// =========================================================================
// r10（覆盖率 A 类 miss 补充）：恢复统计的 tracing::info! 参数行。
// tracing 的字段表达式（count = recovered.len() 等）编译进惰性闭包——只有
// 存在已启用 subscriber 时才求值。默认测试进程无 subscriber → info! 外层
// 行被执行而参数行 span 永不执行。本测试先 ensure_default_logger() 安装
// INFO 级 fmt subscriber，再驱动带存量任务的 first_start()，让
// 「Recovered N tasks」事件的字段闭包真实执行。
// =========================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r10_first_start_recovery_info_fields_execute_with_subscriber() {
    crate::common::ensure_default_logger();

    let tmp = tempfile::tempdir().unwrap();
    let task_dir = tmp.path().join("tasks");
    std::fs::create_dir_all(&task_dir).unwrap();

    let seeder = ClusterTaskList::new(&task_dir);
    seeder.create_task(make_pending_task("r10-recover-a"));
    let mut waiting = make_pending_task("r10-recover-b");
    waiting.status = TaskStatus::WaitingRemote;
    seeder.create_task(waiting);
    seeder.persist_to_disk().expect("seed persist must succeed");
    drop(seeder);

    // 坏 config（无 config.json）→ agent 构建失败 lenient，但恢复分支先行。
    let (adapter, _cluster, _shared, _tl, _wq, _flag) =
        make_adapter(&tmp.path().join("home"), &task_dir);
    adapter.first_start().expect("first_start");

    assert!(
        LifecycleService::is_running(&adapter),
        "first_start 完成后必须置 running"
    );
}
