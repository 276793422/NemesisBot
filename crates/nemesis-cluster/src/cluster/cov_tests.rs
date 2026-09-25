// cluster.rs 覆盖率补充测试（start 配置臂 / state.toml 回读 / token 应用 /
// UDP discovery 双形态 / 占位升级与 peers.toml 写回 / canonical 解析 /
// register_rpc_peer 继承 / probe / handler 注册 / 安全网超时）。
//
// 豁免（另见文件尾注）：
// - 860 / 889-915（健康探针循环体：需真实对端应答探针 RPC 的常驻循环，
//   集成级）；722 / 655-656（discovery stop/new 失败臂，构造不出）；
// - 982-983 / 2512（sync 循环首 tick 的过期日志与落盘失败——时序竞态面，
//   单测不可 determinism 驱动）；
// - 1584 / 1617-1618（TOML 文档恒为表根 / to_string_pretty 对表恒成功
//   ——死防御，与 cluster_config 同判）；
// - 1501-1503（merge 里 placeholder==real_id 的 else：get(info.id) 命中
//   在先，find_by_address 不可能再返回同 id——不可达）；
// - 3778（hostname_display_name 的 None 臂依赖 env 缺失，edition 2024
//   下 set_var unsafe，不动进程环境）；
// - RPC 全栈往返臂（1795-1807 / 1883-1907 / 3407-3416）由同目录
//   cov_net_tests.rs 的假对端（Frame 协议）覆盖。

use super::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

// ---------------------------------------------------------------------------
// 脚手架
// ---------------------------------------------------------------------------

static WS_SEQ: AtomicU64 = AtomicU64::new(0);

struct Ws {
    root: PathBuf,
}

impl Ws {
    fn new(tag: &str) -> Self {
        let n = WS_SEQ.fetch_add(1, Ordering::SeqCst);
        let root =
            std::env::temp_dir().join(format!("nmb-cluster-cov-{}-{tag}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Self { root }
    }

    fn cluster(&self, id: &str, name: &str) -> Cluster {
        let config = ClusterConfig {
            node_id: id.into(),
            bind_address: "127.0.0.1:0".into(),
            peers: Vec::new(),
            node_name: name.into(),
        };
        Cluster::with_workspace(config, self.root.clone())
    }

    /// 写 config.cluster.json（AppConfig 读取点）。
    fn write_app_config(&self, json: &str) {
        let path = nemesis_path::resolve_cluster_config_path_in_workspace(&self.root);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, json).unwrap();
    }
}

impl Drop for Ws {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn node(id: &str, name: &str, addr: &str, status: NodeStatus) -> ExtendedNodeInfo {
    ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: id.into(),
            name: name.into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: addr.into(),
            category: "development".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status,
        capabilities: Vec::new(),
        tags: Vec::new(),
        addresses: Vec::new(),
        node_type: "gateway".into(),
    }
}

/// 找一个当前空闲的 UDP 端口（探针绑定后释放，小窗口竞态可接受）。
fn free_udp_port() -> u16 {
    let s = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    s.local_addr().unwrap().port()
}

// ---------------------------------------------------------------------------
// start() 配置臂 + state.toml 回读 + token 应用
// ---------------------------------------------------------------------------

/// llm_timeout_secs=0 → 按 peer_chat 协议默认（2h）推阶梯（401 臂）。
#[test]
fn start_zero_llm_timeout_uses_protocol_default() {
    let ws = Ws::new("zero-timeout");
    ws.write_app_config(r#"{"llm_timeout_secs": 0, "health_check_interval_secs": 0}"#);
    let cluster = ws.cluster("cov-zero", "CovZero");
    cluster.start();
    assert!(cluster.is_running());
    cluster.stop();
}

/// B 端 LLM 超时 ≥ 24h → 安全网大于结果 TTL 的 WARN 臂（407-413）。
#[test]
fn start_warns_when_llm_timeout_exceeds_day() {
    let ws = Ws::new("big-timeout");
    ws.write_app_config(r#"{"llm_timeout_secs": 86401, "health_check_interval_secs": 0}"#);
    let cluster = ws.cluster("cov-big", "CovBig");
    cluster.start();
    assert!(cluster.is_running());
    cluster.stop();
}

/// state.toml 损坏 → WARN + 按空注册表启动（453-461）。
#[test]
fn start_with_corrupt_state_toml_starts_clean() {
    let ws = Ws::new("corrupt-state");
    let state_path = nemesis_path::resolve_cluster_state_path_in_workspace(&ws.root);
    std::fs::create_dir_all(state_path.parent().unwrap()).unwrap();
    std::fs::write(&state_path, "not [valid toml").unwrap();

    let cluster = ws.cluster("cov-state", "CovState");
    cluster.start();
    assert!(cluster.is_running(), "损坏 state.toml 不阻断启动");
    assert_eq!(cluster.list_nodes().len(), 1, "只种入本节点");
    cluster.stop();
}

/// state.toml 合法条目回读：空名回退 id、空 last_seen 回退 now、非空
/// last_seen 原样（504-516），全部以 Offline 落注册表。
#[test]
fn start_restores_discovered_entries_with_fallbacks() {
    use crate::cluster_config::{DynamicState, PeerConfig, PeerStatus};

    let ws = Ws::new("restore");
    let mk_pc = |id: &str, name: &str, addr: &str, last_seen: &str| PeerConfig {
        id: id.into(),
        name: name.into(),
        address: addr.into(),
        addresses: Vec::new(),
        rpc_port: 0,
        role: "worker".into(),
        category: "development".into(),
        tags: Vec::new(),
        priority: 0,
        enabled: true,
        status: PeerStatus {
            state: String::new(),
            last_seen: last_seen.into(),
            ..PeerStatus::default()
        },
    };
    let state = DynamicState {
        discovered: vec![
            mk_pc("peer-a", "", "10.0.0.5:11950", ""), // 空名 + 空 last_seen
            mk_pc(
                "peer-b",
                "PeerB",
                "10.0.0.6:11950",
                "2026-09-01T10:00:00+08:00",
            ), // 全量原样
        ],
        last_sync: chrono::Local::now().to_rfc3339(),
    };
    let state_path = nemesis_path::resolve_cluster_state_path_in_workspace(&ws.root);
    std::fs::create_dir_all(state_path.parent().unwrap()).unwrap();
    crate::cluster_config::save_dynamic_state(&state_path, &state).unwrap();

    let cluster = ws.cluster("cov-restore", "CovRestore");
    cluster.start();
    let peers = cluster.list_nodes();
    let a = peers
        .iter()
        .find(|p| p.base.id == "peer-a")
        .expect("peer-a 回读");
    assert_eq!(a.base.name, "peer-a", "空名回退 id");
    let b = peers
        .iter()
        .find(|p| p.base.id == "peer-b")
        .expect("peer-b 回读");
    assert_eq!(b.base.name, "PeerB");
    assert_eq!(
        b.base.last_seen, "2026-09-01T10:00:00+08:00",
        "非空 last_seen 原样"
    );
    assert_eq!(a.status, NodeStatus::Offline, "回读绝不直接 Online");
    cluster.stop();
}

/// token 在位：RPC server / client 双侧应用（590-599；server 先于 start
/// 注入才走 591-592 臂）。
#[test]
fn start_applies_auth_token_to_client_and_server() {
    use crate::rpc::server::{RpcServer, RpcServerConfig};

    let ws = Ws::new("token");
    ws.write_app_config(r#"{"token": "cov-secret"}"#);
    let mut cluster = ws.cluster("cov-token", "CovToken");
    let server = Arc::new(RpcServer::new(RpcServerConfig {
        bind_address: "127.0.0.1:0".into(),
        ..Default::default()
    }));
    cluster.set_rpc_server(server);

    cluster.start();
    assert!(cluster.is_running());
    cluster.stop();
}

// ---------------------------------------------------------------------------
// UDP discovery：成功 + 端口冲突（641-647）
// ---------------------------------------------------------------------------

/// discovery 启动成功（info 臂）后 stop 干净；端口被占 → start() 失败
/// error 臂（645-647）。
#[test]
fn discovery_start_success_then_conflict_error() {
    // ① 成功路径。
    let ws = Ws::new("discovery-ok");
    let mut cluster = ws.cluster("cov-disc", "CovDisc");
    let udp = free_udp_port();
    cluster.set_ports(udp, udp.saturating_add(10000));
    let arc: Arc<Cluster> = Arc::new(cluster);
    arc.start_discovery(arc.clone());
    arc.stop();

    // ② 冲突路径：握住端口 → discovery.start() bind 失败。
    let ws2 = Ws::new("discovery-conflict");
    let held = std::net::UdpSocket::bind(("127.0.0.1", 0)).unwrap();
    let port = held.local_addr().unwrap().port();
    let mut cluster2 = ws2.cluster("cov-disc2", "CovDisc2");
    cluster2.set_ports(port, port.saturating_add(10000));
    let arc2: Arc<Cluster> = Arc::new(cluster2);
    arc2.start_discovery(arc2.clone()); // 必须走 error 臂而非 panic
    arc2.stop();
    drop(held);
}

// ---------------------------------------------------------------------------
// 占位升级 / peers.toml 写回（1206 / 1269-1277 / 1315-1327 / 1459 /
// 1591-1593 / 1603-1611）
// ---------------------------------------------------------------------------

/// 无端口自报地址归一（1202-1211）+ 占位按名归并 + 地址池继承 + peers.toml
/// 升级（无 [peers] 段 → append 落盘，1591-1593）。
#[test]
fn handle_discovered_normalizes_portless_and_upgrades_placeholder() {
    let ws = Ws::new("upgrade");
    let cluster = ws.cluster("cov-up", "CovUp");
    let mut ph = node("Node-A", "Node-A", "10.0.0.9:32949", NodeStatus::Online);
    ph.addresses = vec!["127.0.0.1".to_string()]; // 占位独有候选（回环）
    cluster.register_node(ph);

    let changed = cluster.handle_discovered_node(
        "real-a",
        "Node-A",
        vec!["10.0.0.9".to_string()], // 无端口 → 归一 "10.0.0.9:32949"
        32949,
        "worker",
        "general",
        Vec::new(),
        Vec::new(),
        "gateway",
    );
    assert!(changed);

    let peers = cluster.list_nodes();
    assert!(peers.iter().all(|p| p.base.id != "Node-A"), "占位已移除");
    let real = peers
        .iter()
        .find(|p| p.base.id == "real-a")
        .expect("real-a 登记");
    assert_eq!(real.base.name, "Node-A", "人读名继承");
    assert!(
        real.addresses.contains(&"127.0.0.1".to_string())
            && real.addresses.contains(&"10.0.0.9".to_string()),
        "占位侧地址（含回环）并入候选池：{:?}",
        real.addresses
    );
    let toml = std::fs::read_to_string(nemesis_path::resolve_cluster_peers_path_in_workspace(
        &ws.root,
    ))
    .unwrap_or_default();
    assert!(toml.contains("real-a"), "peers.toml 落盘升级条目：{toml}");
}

/// merge_real_node_info：全量自报地址（带端口形态）逐个匹配占位（1455-1472
/// 的 1459 臂）。
#[test]
fn merge_real_node_matches_full_address_candidates() {
    let ws = Ws::new("merge-addr");
    let cluster = ws.cluster("cov-merge", "CovMerge");
    cluster.register_node(node(
        "Node-B",
        "Node-B",
        "10.0.0.9:32949",
        NodeStatus::Online,
    ));

    let got = cluster.merge_real_node_info(&RealNodeInfo {
        id: "real-b".into(),
        name: "RealB".into(),
        address: "10.9.9.9:11111".into(), // primary 不命中
        rpc_port: 32949,
        addresses: vec!["10.0.0.9:32949".into()], // 全量候选命中占位
        role: nemesis_types::cluster::NodeRole::Worker,
        category: "development".into(),
        capabilities: Vec::new(),
        tags: Vec::new(),
        node_type: "gateway".into(),
    });
    assert_eq!(got, "real-b");
    let peers = cluster.list_nodes();
    assert!(peers.iter().all(|p| p.base.id != "Node-B"), "占位升级移除");
    assert!(peers.iter().any(|p| p.base.id == "real-b"));
}

/// upgrade_peer_in_peers_toml：字面键删空后回退 legacy sanitize 键
/// （1598-1611），"Node.B" → "Node_B"。
#[test]
fn upgrade_removes_legacy_sanitized_key() {
    use crate::cluster_config::append_peer_to_file_with_name;

    let ws = Ws::new("legacy-key");
    let peers_path = nemesis_path::resolve_cluster_peers_path_in_workspace(&ws.root);
    std::fs::create_dir_all(peers_path.parent().unwrap()).unwrap();
    append_peer_to_file_with_name(
        &peers_path,
        "Node_B", // legacy sanitize 键（旧版写盘形态）
        "10.0.0.7:11949",
        "worker",
        "general",
        None,
        0,
    )
    .unwrap();

    let cluster = ws.cluster("cov-legacy", "CovLegacy");
    cluster.register_node(node(
        "Node.B",
        "Node.B",
        "10.0.0.7:11949",
        NodeStatus::Online,
    ));
    cluster.mark_peer_static("Node.B");

    let got = cluster.merge_real_node_info(&RealNodeInfo {
        id: "real-dot".into(),
        name: "RealDot".into(),
        address: "10.0.0.7:11949".into(),
        rpc_port: 11949,
        addresses: Vec::new(),
        role: nemesis_types::cluster::NodeRole::Worker,
        category: "general".into(),
        capabilities: Vec::new(),
        tags: Vec::new(),
        node_type: "gateway".into(),
    });
    assert_eq!(got, "real-dot");

    let content = std::fs::read_to_string(&peers_path).unwrap();
    assert!(
        !content.contains("Node_B"),
        "legacy 键必须被清掉：{content}"
    );
    assert!(content.contains("real-dot"), "真实条目落盘：{content}");
}

// ---------------------------------------------------------------------------
// canonical 解析 / register_rpc_peer 继承（2096-2102 / 2212-2251）
// ---------------------------------------------------------------------------

/// id==name 自键占位遮蔽时按 name 扫出真实运行时 id（2095-2103）。
#[test]
fn canonical_peer_id_resolves_placeholder_to_real() {
    let ws = Ws::new("canonical");
    let cluster = ws.cluster("cov-canonical", "CovCanonical");
    cluster.register_node(node("Node-B", "Node-B", "10.0.0.1:1", NodeStatus::Offline));
    cluster.register_node(node(
        "node-real-b",
        "Node-B",
        "10.0.0.2:2",
        NodeStatus::Online,
    ));

    assert_eq!(
        cluster.canonical_peer_id("Node-B").as_deref(),
        Some("node-real-b"),
        "占位遮蔽时返回真实 id"
    );
    assert_eq!(
        cluster.canonical_peer_id("node-real-b").as_deref(),
        Some("node-real-b"),
        "真实 id 直通"
    );
    assert_eq!(cluster.canonical_peer_id("ghost"), None);
}

/// register_rpc_peer：静态占位身份继承（udp 形态端口 +10000 推导 2212-2213、
/// host 并入 2222、空名回退 real_id 2225、changed info 2251）。
#[test]
fn register_rpc_peer_inherits_static_placeholder() {
    let ws = Ws::new("rpc-peer");
    let cluster = ws.cluster("cov-rpcpeer", "CovRpcPeer");
    let mut ph = node("Node-P", "Node-P", "127.0.0.1:5000", NodeStatus::Offline);
    ph.addresses = vec!["10.1.1.1".to_string()];
    cluster.register_node(ph);
    cluster.mark_peer_static("Node-P");

    assert!(
        cluster.register_rpc_peer("real-p", 0),
        "单占位无 hint 亦继承"
    );
    let peers = cluster.list_nodes();
    let real = peers
        .iter()
        .find(|p| p.base.id == "real-p")
        .expect("real-p 登记");
    assert_eq!(real.base.name, "Node-P", "占位人读名继承");
    assert_eq!(
        real.base.address, "10.1.1.1:15000",
        "udp+10000 推导 rpc 端口"
    );
    assert!(
        peers.iter().all(|p| p.base.id != "Node-P"),
        "占位已升级移除"
    );
}

/// 占位地址剥不出端口 → rpc_port=0（2215），G14 拒登记 → false。
#[test]
fn register_rpc_peer_unparseable_port_falls_back_zero() {
    let ws = Ws::new("rpc-peer-zero");
    let cluster = ws.cluster("cov-rpcpeer0", "CovRpcPeer0");
    cluster.register_node(node("Node-Z", "Node-Z", "", NodeStatus::Offline));
    cluster.mark_peer_static("Node-Z");

    assert!(
        !cluster.register_rpc_peer("real-z", 0),
        "端口不可推导 → 不登记不可达条目"
    );
}

// ---------------------------------------------------------------------------
// probe / 面板 / handler 注册（2314-2335 / 2422-2427 / 2665-2678 /
// 2719-2836 / 2890 / 2939 / 3206-3211 / 3271-3296）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn probe_peer_none_client_and_ghost() {
    let ws = Ws::new("probe");
    let cluster = ws.cluster("cov-probe", "CovProbe");

    // start 前：无 RPC client → false（2315-2317）。
    assert!(!cluster.probe_peer("ghost-node").await);

    // start 后：client 在但 ghost 不在注册表 → resolver 落空 → Err → false。
    cluster.start();
    assert!(!cluster.probe_peer("ghost-node").await);
    cluster.stop();
}

#[test]
fn local_capabilities_snapshot() {
    let ws = Ws::new("caps");
    let cluster = ws.cluster("cov-caps", "CovCaps");
    cluster.set_capabilities(vec!["llm".into(), "cluster".into()]);
    assert_eq!(
        cluster.local_capabilities(),
        vec!["llm".to_string(), "cluster".to_string()]
    );
}

/// 未 running + 无 server → query/confirm 两个注册都走 error 臂（2665-2678）。
#[test]
fn register_recovery_handlers_without_server_errors() {
    let ws = Ws::new("recovery-reg");
    let cluster = ws.cluster("cov-reg", "CovReg");
    cluster.register_task_recovery_handlers(); // 只需不 panic（error 臂留痕）
}

/// running + 真实 server → register_basic_handlers 全部 `?` 成功臂 +
/// forge 双 handler 注册（2890 / 2939）。
#[test]
fn register_basic_and_forge_handlers_with_server() {
    use crate::rpc::server::{RpcServer, RpcServerConfig};

    let ws = Ws::new("basic-reg");
    let mut cluster = ws.cluster("cov-basic", "CovBasic");
    let server = Arc::new(RpcServer::new(RpcServerConfig {
        bind_address: "127.0.0.1:0".into(),
        ..Default::default()
    }));
    cluster.set_rpc_server(server);
    cluster.start();

    cluster
        .register_basic_handlers()
        .expect("全部基础 handler 注册成功");
    cluster
        .register_forge_handlers(Box::new(crate::handlers::FileForgeProvider::new(
            ws.root.join("forge"),
        )))
        .expect("forge handler 注册成功");
    cluster.stop();
}

/// 外部注入 RPC client 的日志臂（3206-3211）。
#[test]
fn set_rpc_client_external() {
    let ws = Ws::new("set-client");
    let cluster = ws.cluster("cov-client", "CovClient");
    cluster.set_rpc_client(Arc::new(crate::rpc::client::RpcClient::new()));
    assert!(cluster.rpc_client_arc().is_some());
}

/// G4 安全网阶梯 + G2 探针调度纯函数 + 漂移限频闸单例（3271-3296）。
#[test]
fn safety_net_and_probe_gate_helpers() {
    // 0 → 协议默认 2h ×2 = 4h，被 24h 地板抬起。
    assert_eq!(stale_task_safety_net(0), chrono::Duration::hours(24));
    // 86400（24h）→ ×2 = 48h > 地板。
    assert_eq!(stale_task_safety_net(86400), chrono::Duration::hours(48));

    assert!(should_probe_peer(true, 1), "Online 每 tick 都探");
    assert!(!should_probe_peer(false, 1));
    assert!(should_probe_peer(false, 5), "Offline 1/5 降频自愈");

    let _ = probe_drift_gate();
    let a = probe_drift_gate() as *const _;
    let b = probe_drift_gate() as *const _;
    assert_eq!(a, b, "进程级单例");
}

// ---------------------------------------------------------------------------
// poll_stale_pending_tasks：安全网超时（3351-3365）
// ---------------------------------------------------------------------------

struct MockBus {
    messages: StdMutex<Vec<BusInboundMessage>>,
}

impl MessageBus for MockBus {
    fn publish_inbound(&self, msg: BusInboundMessage) {
        self.messages.lock().unwrap().push(msg);
    }
}

fn stale_task(tm: &Arc<TaskManager>, id: &str, age: chrono::Duration) {
    let task = Task {
        id: id.to_string(),
        status: nemesis_types::cluster::TaskStatus::Pending,
        action: "peer_chat".to_string(),
        peer_id: "remote-1".to_string(),
        payload: serde_json::json!({}),
        result: None,
        original_channel: "rpc".to_string(),
        original_chat_id: "chat-1".to_string(),
        created_at: (chrono::Local::now() - age).to_rfc3339(),
        completed_at: None,
    };
    tm.submit(task).unwrap();
}

/// 超安全网的 pending 任务 → 判超时 + complete_callback + bus 唤醒
/// （3355-3365）。
#[tokio::test]
async fn poll_stale_times_out_past_safety_net_publishes_bus() {
    let tm = Arc::new(TaskManager::new());
    stale_task(&tm, "cov-timeout", chrono::Duration::days(25));

    let bus = Arc::new(MockBus {
        messages: StdMutex::new(Vec::new()),
    });
    poll_stale_pending_tasks(
        &tm,
        &None,
        None,
        chrono::Duration::hours(24),
        Some(bus.as_ref()),
        false,
        None,
    )
    .await;

    assert_eq!(
        tm.get_task("cov-timeout").unwrap().status,
        nemesis_types::cluster::TaskStatus::Failed,
        "超安全网必须判死"
    );
    let msgs = bus.messages.lock().unwrap();
    assert_eq!(msgs.len(), 1, "安全网超时也要发 bus 唤醒");
    assert!(
        msgs[0]
            .sender_id
            .starts_with("cluster_continuation:cov-timeout")
    );
}
