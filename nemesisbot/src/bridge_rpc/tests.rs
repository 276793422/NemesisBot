//! 桥帧 RPC 通道测试（goal 二期批次六 + 三期批次八/九）。
//!
//! 覆盖：pending 表 round-trip / 发送失败快速收口 / 超时 / hub 侧上行分流
//! （request 喂本地链、response 唤醒 pending、跨节点诚实 error）/ 设备侧
//! 对称路径 / BridgeSend 出口端到端（relay 设备表 + 映射反查）/ member_sync
//! 成员合并 / 跨桥转发（请求转发 + 响应回程 + 不可达诚实 error）。

use super::*;
use nemesis_cluster::rpc::server::RpcServerConfig;
use nemesis_cluster::types::ClusterConfig;
use nemesis_web::relay::{
    BridgeClusterIdentity, BridgeIdentityEvent, BridgeIdentitySink, RelayServer,
};

/// 构造测试 Cluster（同 bridge_cluster::tests 模式；registry 操作全同步）。
fn test_cluster() -> (Arc<nemesis_cluster::cluster::Cluster>, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config = ClusterConfig {
        node_id: String::new(),
        bind_address: "0.0.0.0:0".to_string(),
        peers: vec![],
        node_name: "TestHub".to_string(),
    };
    let cluster = Arc::new(nemesis_cluster::cluster::Cluster::with_workspace(
        config,
        tmp.path().to_path_buf(),
    ));
    (cluster, tmp)
}

fn test_server() -> Arc<RpcServer> {
    let server = Arc::new(RpcServer::new(RpcServerConfig {
        bind_address: "0.0.0.0:0".into(),
        ..Default::default()
    }));
    server.register_handler(
        "ping",
        Box::new(|payload| Ok(serde_json::json!({"pong": payload, "via": "handler"}))),
    );
    server
}

fn request_wire(id: &str, to: &str) -> WireMessage {
    WireMessage {
        version: "1.0".into(),
        id: id.into(),
        msg_type: "request".into(),
        from: "node-caller".into(),
        to: to.into(),
        action: "ping".into(),
        payload: serde_json::json!({"q": 1}),
        timestamp: 1,
        error: String::new(),
    }
}

/// 跨桥转发测试用帧：from/to 可控（请求方向 B 端响应的 `to` = 请求的
/// `from`——回程映射命中的前提）。
fn bridge_wire(id: &str, msg_type: &str, from: &str, to: &str) -> WireMessage {
    WireMessage {
        version: "1.0".into(),
        id: id.into(),
        msg_type: msg_type.into(),
        from: from.into(),
        to: to.into(),
        action: "ping".into(),
        payload: serde_json::json!({"q": 1}),
        timestamp: 1,
        error: String::new(),
    }
}

fn response_wire(id: &str, to: &str) -> WireMessage {
    WireMessage {
        version: "1.0".into(),
        id: id.into(),
        msg_type: "response".into(),
        from: "node-peer".into(),
        to: to.into(),
        action: "ping".into(),
        payload: serde_json::json!({"pong": true}),
        timestamp: 1,
        error: String::new(),
    }
}

#[tokio::test]
async fn channel_round_trip_and_fail_fast_and_timeout() {
    // 发送成功 + deliver 唤醒 → Ok。
    let (tx_slot, rx_slot): (
        tokio::sync::oneshot::Sender<WireMessage>,
        tokio::sync::oneshot::Receiver<WireMessage>,
    ) = tokio::sync::oneshot::channel();
    // 闭包把请求 id 转成回执信号（模拟远端收到请求）。
    let notify = Arc::new(tokio::sync::Notify::new());
    let notify2 = notify.clone();
    let channel = BridgeRpcChannel::new(Box::new(move |_t, _w| {
        notify2.notify_one();
        true
    }));
    let ch = Arc::new(channel);
    let ch2 = ch.clone();
    let req = tokio::spawn(async move {
        ch2.request(
            "bridge-x",
            request_wire("r-1", "node-x"),
            Duration::from_secs(5),
        )
        .await
    });
    notify.notified().await;
    // 模拟远端回帧。
    let delivered = ch.deliver(&response_wire("r-1", "node-caller"));
    assert!(delivered, "deliver 应命中 pending");
    let resp = req.await.unwrap().expect("round-trip 应成功");
    assert_eq!(resp.id, "r-1");
    assert_eq!(resp.payload, serde_json::json!({"pong": true}));
    drop((tx_slot, rx_slot));

    // 发送失败（出口不可达）→ 立刻 Err，不等超时。
    let dead = BridgeRpcChannel::new(Box::new(|_t, _w| false));
    let t0 = std::time::Instant::now();
    let err = dead
        .request(
            "bridge-x",
            request_wire("r-2", "node-x"),
            Duration::from_secs(30),
        )
        .await
        .expect_err("发送失败应报错");
    assert!(matches!(err, RpcClientError::Connection(_)));
    assert!(
        t0.elapsed() < Duration::from_secs(5),
        "应快速失败而非等超时"
    );

    // 无响应 → Timeout。
    let silent = BridgeRpcChannel::new(Box::new(|_t, _w| true));
    let err = silent
        .request(
            "bridge-x",
            request_wire("r-3", "node-x"),
            Duration::from_millis(50),
        )
        .await
        .expect_err("无响应应超时");
    assert!(matches!(err, RpcClientError::Timeout));
}

#[tokio::test]
async fn hub_sink_request_feeds_local_handler() {
    let server = test_server();
    let (cluster, _tmp) = test_cluster();
    let sink_registry = Arc::new(BridgeClusterSink::new(cluster.clone()));
    let relay = Arc::new(RelayServer::new("tok".into(), true));
    let hub = HubBridgeRpc::new(server, "node-hub".into(), sink_registry, relay);

    // to==自己 → 喂本地 handler 链 → 响应 payload 回传。
    let out = hub
        .on_cluster_frame(
            "bridge-x",
            serde_json::to_value(request_wire("q-1", "node-hub")).unwrap(),
        )
        .await
        .expect("request 应有响应");
    let resp: WireMessage = serde_json::from_value(out).unwrap();
    assert_eq!(resp.msg_type, "response");
    assert_eq!(resp.id, "q-1");
    assert_eq!(resp.error, "");
    assert_eq!(resp.payload["via"], "handler");

    // 广播形态（to 空）同样本地处理。
    let out = hub
        .on_cluster_frame(
            "bridge-x",
            serde_json::to_value(request_wire("q-2", "")).unwrap(),
        )
        .await
        .expect("广播 request 应有响应");
    let resp: WireMessage = serde_json::from_value(out).unwrap();
    assert_eq!(resp.id, "q-2");

    // to==其它节点 → 诚实 error（跨桥转发三期开放）。
    let out = hub
        .on_cluster_frame(
            "bridge-x",
            serde_json::to_value(request_wire("q-3", "node-other")).unwrap(),
        )
        .await
        .expect("跨节点帧应回 error 帧");
    let resp: WireMessage = serde_json::from_value(out).unwrap();
    assert_eq!(resp.msg_type, "error");
    assert!(resp.error.contains("node-other"));
    assert!(!resp.error.is_empty());
}

#[tokio::test]
async fn hub_sink_response_wakes_pending_via_bridge_send() {
    let server = test_server();
    let (cluster, _tmp) = test_cluster();
    let sink_registry = Arc::new(BridgeClusterSink::new(cluster.clone()));
    let relay = Arc::new(RelayServer::new("tok".into(), true));

    // 设备表插 bridge-x（hub BridgeSend 下行投递的锚点）。
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    relay
        .authenticate_device("tok", "bridge-x", "远端机", "v1", None, out_tx)
        .expect("插设备表");
    // 映射：bridge-x ↔ node-x（喂 online 身份事件）。
    sink_registry.on_bridge_identity(BridgeIdentityEvent {
        bridge_node_id: "bridge-x".into(),
        online: true,
        cluster: Some(BridgeClusterIdentity {
            node_id: "node-x".into(),
            name: "远端机".into(),
            role: "worker".into(),
            category: "general".into(),
            tags: vec![],
            capabilities: vec![],
            node_type: "agent".into(),
            rpc_port: 12345,
            addresses: vec![],
        }),
    });

    let hub = Arc::new(HubBridgeRpc::new(
        server,
        "node-hub".into(),
        sink_registry.clone(),
        relay.clone(),
    ));

    // bridge_online：映射命中 + 设备表在线。
    assert!(BridgeSend::bridge_online(hub.as_ref(), "node-x"));
    // 映射 miss（无桥链路的 peer）→ false。
    assert!(!BridgeSend::bridge_online(hub.as_ref(), "node-nowhere"));

    // 端到端：send_over_bridge → 下行帧出现在设备 out_rx → hub sink 收到
    // 设备回的 response → 唤醒 pending。
    let hub2 = hub.clone();
    let req_task = tokio::spawn(async move {
        BridgeSend::send_over_bridge(
            hub2.as_ref(),
            "node-x",
            request_wire("e-1", "node-x"),
            Duration::from_secs(5),
        )
        .await
    });
    // 三期批次八起 authenticate_device 触发 member_sync 广播——跳过之，
    // 等真正的 ClusterRpc 下行帧。
    let frame = loop {
        let f = tokio::time::timeout(Duration::from_secs(2), out_rx.recv())
            .await
            .expect("下行帧应到达设备队列")
            .expect("队列不应关闭");
        if matches!(f, BridgeFrame::ClusterRpc { .. }) {
            break f;
        }
    };
    let BridgeFrame::ClusterRpc { payload } = frame else {
        panic!("应收到 ClusterRpc 帧，得到 {frame:?}");
    };
    let sent: WireMessage = serde_json::from_value(payload).unwrap();
    assert_eq!(sent.id, "e-1");
    assert_eq!(sent.to, "node-x");

    // 模拟设备回响应（经 hub 的上行分流 → deliver）。
    let wake = hub
        .on_cluster_frame(
            "bridge-x",
            serde_json::to_value(response_wire("e-1", "node-x")).unwrap(),
        )
        .await;
    assert!(wake.is_none(), "response 分流不回帧");
    let resp = req_task.await.unwrap().expect("端到端 round-trip 应成功");
    assert_eq!(resp.id, "e-1");
    assert_eq!(resp.payload, serde_json::json!({"pong": true}));

    // send_over_bridge 映射 miss → 防御性 Err（仲裁的 bridge_online 已闸）。
    let err = BridgeSend::send_over_bridge(
        hub.as_ref(),
        "node-nowhere",
        request_wire("e-2", "node-nowhere"),
        Duration::from_secs(1),
    )
    .await
    .expect_err("无桥链路 peer 应失败");
    assert!(matches!(err, RpcClientError::Connection(_)));
}

#[tokio::test]
async fn device_bridge_round_trip_and_downstream_request() {
    let server = test_server();
    // cluster=None：本测试只覆盖通道语义（成员合并有专项测试）。
    let dev = Arc::new(DeviceBridgeRpc::new(server, "node-dev".into(), None));

    // 未挂载上行出口：bridge_online false + 发送快速失败。
    assert!(!BridgeSend::bridge_online(dev.as_ref(), "node-hub"));
    let err = BridgeSend::send_over_bridge(
        dev.as_ref(),
        "node-hub",
        request_wire("d-0", "node-hub"),
        Duration::from_secs(1),
    )
    .await
    .expect_err("未挂载上行应失败");
    assert!(matches!(err, RpcClientError::Connection(_)));

    // 挂载上行 → 端到端：send_over_bridge → 上行帧进入 rx → 模拟 hub 回
    // response（经 handle_downstream）→ 完成。三期批次八起 bridge_online
    // = 出口活 + 成员在表——先收一帧 sync 塞表（模拟 hub 广播先行）。
    let (up_tx, mut up_rx) = tokio::sync::mpsc::unbounded_channel::<BridgeFrame>();
    dev.attach_uplink(up_tx);
    dev.handle_member_sync(&serde_json::json!({
        "members": [
            {"node_id": "node-hub", "name": "H", "online": true,
             "via_bridge": false, "addresses": [], "rpc_port": 1,
             "role": "coordinator", "category": "general",
             "capabilities": [], "node_type": "agent"},
        ]
    }));
    assert!(BridgeSend::bridge_online(dev.as_ref(), "node-hub"));

    let dev2 = dev.clone();
    let req_task = tokio::spawn(async move {
        BridgeSend::send_over_bridge(
            dev2.as_ref(),
            "node-hub",
            request_wire("d-1", "node-hub"),
            Duration::from_secs(5),
        )
        .await
    });
    let frame = tokio::time::timeout(Duration::from_secs(2), up_rx.recv())
        .await
        .expect("上行帧应到达")
        .expect("通道不应关闭");
    let BridgeFrame::ClusterRpc { payload } = frame else {
        panic!("应收到 ClusterRpc 帧，得到 {frame:?}");
    };
    let sent: WireMessage = serde_json::from_value(payload).unwrap();
    assert_eq!(sent.id, "d-1");

    // 模拟 hub 回响应（设备下行分流 → deliver）。
    let wake = dev
        .handle_downstream(serde_json::to_value(response_wire("d-1", "node-dev")).unwrap())
        .await;
    assert!(wake.is_none(), "response 分流不回帧");
    let resp = req_task.await.unwrap().expect("设备侧 round-trip 应成功");
    assert_eq!(resp.id, "d-1");
    assert_eq!(resp.payload, serde_json::json!({"pong": true}));

    // 下行 request（to==本机）→ 喂本地链 → Some 回帧。
    let out = dev
        .handle_downstream(serde_json::to_value(request_wire("d-2", "node-dev")).unwrap())
        .await
        .expect("request 应有回帧");
    let BridgeFrame::ClusterRpc { payload } = out else {
        panic!("应回 ClusterRpc 帧");
    };
    let resp: WireMessage = serde_json::from_value(payload).unwrap();
    assert_eq!(resp.msg_type, "response");
    assert_eq!(resp.id, "d-2");
    assert_eq!(resp.payload["via"], "handler");

    // detach：出口摘除 + pending 清空 → online false。
    dev.detach_uplink();
    assert!(!BridgeSend::bridge_online(dev.as_ref(), "node-hub"));
}

/// 三期批次八：`member_sync` 合并（registry 注册 + via-bridge 哨兵 + 成员
/// 表全量替换 + 自己跳过 + 离线过滤 + detach 失效降级 + bridge_online 判定）。
#[tokio::test]
async fn member_sync_merges_registry_and_member_table() {
    let server = test_server();
    let (cluster, _tmp) = test_cluster();
    let self_id = cluster.node_id().to_string();
    let dev = Arc::new(DeviceBridgeRpc::new(
        server,
        self_id.clone(),
        Some(cluster.clone()),
    ));

    // 挂载上行（会话活）但成员表空 → hub 节点不在表 → bridge_online false
    // （三期批次八收紧后的语义：经桥可达 = 出口活 + 成员在表）。
    let (up_tx, _up_rx) = tokio::sync::mpsc::unbounded_channel::<BridgeFrame>();
    dev.attach_uplink(up_tx);
    assert!(!BridgeSend::bridge_online(dev.as_ref(), "node-hub"));

    // hub 广播摘要：hub 自身 + 另一桥入成员 + 离线成员 + 本机自己。
    let payload = serde_json::json!({
        "members": [
            {"node_id": "node-hub", "name": "VpsHub", "online": true,
             "via_bridge": false, "addresses": [], "rpc_port": 21952,
             "role": "coordinator", "category": "general",
             "capabilities": [], "node_type": "agent"},
            {"node_id": "node-peer", "name": "PeerDev", "online": true,
             "via_bridge": true, "addresses": ["10.0.0.9"], "rpc_port": 21961,
             "role": "worker", "category": "general",
             "capabilities": [], "node_type": "agent"},
            {"node_id": "node-dead", "name": "DeadPeer", "online": false,
             "via_bridge": true, "addresses": ["10.0.0.9"], "rpc_port": 21962,
             "role": "worker", "category": "general",
             "capabilities": [], "node_type": "agent"},
            {"node_id": self_id, "name": "Self", "online": true,
             "via_bridge": true, "addresses": [], "rpc_port": 21951,
             "role": "worker", "category": "general",
             "capabilities": [], "node_type": "agent"},
        ]
    });
    dev.handle_member_sync(&payload);

    // registry：hub 与 peer 注册（rpc_port>0），离线成员被过滤，自己不注册
    // （自己由本机 discovery 自管）。
    let known: Vec<String> = cluster
        .list_nodes()
        .iter()
        .map(|n| n.base.id.clone())
        .collect();
    assert!(known.iter().any(|id| id == "node-hub"), "hub 应注册");
    assert!(known.iter().any(|id| id == "node-peer"), "peer 应注册");
    assert!(
        !known.iter().any(|id| id == "node-dead"),
        "离线成员不应注册"
    );
    assert!(
        !known.iter().any(|id| id == &self_id),
        "自己不应经 sync 注册"
    );

    // via-bridge 哨兵 + 地址/rpc_port 重组。
    let peer = cluster
        .list_nodes()
        .into_iter()
        .find(|n| n.base.id == "node-peer")
        .expect("peer 条目应存在");
    assert!(
        peer.tags.iter().any(|t| t == "via-bridge"),
        "合并成员应带 via-bridge 哨兵"
    );
    assert_eq!(peer.addresses, vec!["10.0.0.9".to_string()]);
    assert_eq!(peer.base.address, "10.0.0.9:21961");

    // 成员表：hub + peer 在表（自己/离线成员不在）→ bridge_online 判定。
    assert!(BridgeSend::bridge_online(dev.as_ref(), "node-hub"));
    assert!(BridgeSend::bridge_online(dev.as_ref(), "node-peer"));
    assert!(!BridgeSend::bridge_online(dev.as_ref(), "node-dead"));

    // 全量替换：第二轮 sync 中 peer 消失 → 出表（过期成员清除）。
    let payload2 = serde_json::json!({
        "members": [
            {"node_id": "node-hub", "name": "VpsHub", "online": true,
             "via_bridge": false, "addresses": [], "rpc_port": 21952,
             "role": "coordinator", "category": "general",
             "capabilities": [], "node_type": "agent"},
        ]
    });
    dev.handle_member_sync(&payload2);
    assert!(BridgeSend::bridge_online(dev.as_ref(), "node-hub"));
    assert!(
        !BridgeSend::bridge_online(dev.as_ref(), "node-peer"),
        "从摘要消失的成员应出表"
    );

    // detach（hub 离线）：成员表清空 → 全部经桥成员失效（降级直连判定）。
    dev.detach_uplink();
    assert!(!BridgeSend::bridge_online(dev.as_ref(), "node-hub"));
}

/// 三期批次八：畸形 member_sync 载荷（无 members 数组）不 panic、不清表。
#[tokio::test]
async fn member_sync_malformed_payload_is_tolerated() {
    let server = test_server();
    let dev = Arc::new(DeviceBridgeRpc::new(server, "node-dev".into(), None));
    let (up_tx, _up_rx) = tokio::sync::mpsc::unbounded_channel::<BridgeFrame>();
    dev.attach_uplink(up_tx);
    dev.handle_member_sync(&serde_json::json!({
        "members": [
            {"node_id": "node-hub", "online": true, "addresses": [],
             "rpc_port": 1, "role": "coordinator", "category": "general",
             "capabilities": [], "node_type": "agent"},
        ]
    }));
    assert!(BridgeSend::bridge_online(dev.as_ref(), "node-hub"));

    // 无 members 键 → WARN 忽略，成员表保持原样（不清空）。
    dev.handle_member_sync(&serde_json::json!({"other": 1}));
    assert!(
        BridgeSend::bridge_online(dev.as_ref(), "node-hub"),
        "畸形载荷不应清空既有成员表"
    );

    // cluster=None（纯通道模式）：只维护成员表，不碰 registry（不 panic）。
    dev.handle_member_sync(&serde_json::json!({
        "members": [
            {"node_id": "node-x", "online": true, "addresses": [],
             "rpc_port": 1, "role": "worker", "category": "general",
             "capabilities": [], "node_type": "agent"},
        ]
    }));
    assert!(BridgeSend::bridge_online(dev.as_ref(), "node-x"));
}

// ---------------------------------------------------------------------------
// 三期批次九：跨桥转发（hub 三分支泛化）
// ---------------------------------------------------------------------------

/// 转发测试环境：双桥入设备（bridge-a ↔ node-a、bridge-b ↔ node-b）注册
/// 进设备表与集群↔桥映射，hub 装配完成。
struct ForwardEnv {
    hub: Arc<HubBridgeRpc>,
    rx_a: tokio::sync::mpsc::Receiver<BridgeFrame>,
    rx_b: tokio::sync::mpsc::Receiver<BridgeFrame>,
}

fn forward_env() -> ForwardEnv {
    let server = test_server();
    let (cluster, _tmp) = test_cluster();
    let sink_registry = Arc::new(BridgeClusterSink::new(cluster.clone()));
    let relay = Arc::new(RelayServer::new("tok".into(), true));
    let (tx_a, rx_a) = tokio::sync::mpsc::channel(16);
    let (tx_b, rx_b) = tokio::sync::mpsc::channel(16);

    // 设备表登记（authenticate_device）+ 集群↔桥映射（身份事件），两台。
    relay
        .authenticate_device("tok", "bridge-a", "桥机A", "v1", None, tx_a)
        .expect("插 bridge-a");
    sink_registry.on_bridge_identity(BridgeIdentityEvent {
        bridge_node_id: "bridge-a".into(),
        online: true,
        cluster: Some(BridgeClusterIdentity {
            node_id: "node-a".into(),
            name: "桥机A".into(),
            role: "worker".into(),
            category: "general".into(),
            tags: vec![],
            capabilities: vec![],
            node_type: "agent".into(),
            // rpc_port 非 0（BridgeClusterSink 对 0 有防预闸——不写映射）。
            rpc_port: 21961,
            addresses: vec![],
        }),
    });
    relay
        .authenticate_device("tok", "bridge-b", "桥机B", "v1", None, tx_b)
        .expect("插 bridge-b");
    sink_registry.on_bridge_identity(BridgeIdentityEvent {
        bridge_node_id: "bridge-b".into(),
        online: true,
        cluster: Some(BridgeClusterIdentity {
            node_id: "node-b".into(),
            name: "桥机B".into(),
            role: "worker".into(),
            category: "general".into(),
            tags: vec![],
            capabilities: vec![],
            node_type: "agent".into(),
            rpc_port: 21962,
            addresses: vec![],
        }),
    });

    let hub = Arc::new(HubBridgeRpc::new(
        server,
        "node-hub".into(),
        sink_registry,
        relay,
    ));
    ForwardEnv { hub, rx_a, rx_b }
}

/// 三期批次九：跨桥转发端到端——A 上行请求 `to=node-b` → hub 原帧转发
/// bridge-b（不回帧）；B 的响应 `to=node-a` → hub 按同款映射回程转发
/// bridge-a。全程无中转表（响应 `to` 即发起方集群 id）。
#[tokio::test]
async fn hub_forwards_request_and_response_between_bridges() {
    let ForwardEnv {
        hub,
        mut rx_a,
        mut rx_b,
    } = forward_env();

    // A → hub：请求 to=node-b → 转发 bridge-b，对 A 不回帧。
    let out = hub
        .on_cluster_frame(
            "bridge-a",
            serde_json::to_value(bridge_wire("f-1", "request", "node-a", "node-b")).unwrap(),
        )
        .await;
    assert!(out.is_none(), "转发不回帧");
    let frame = loop {
        let f = tokio::time::timeout(Duration::from_secs(2), rx_b.recv())
            .await
            .expect("转发帧应到达 bridge-b")
            .expect("队列不应关闭");
        // authenticate_device 触发的 member_sync 广播跳过。
        if matches!(f, BridgeFrame::ClusterRpc { .. }) {
            break f;
        }
    };
    let BridgeFrame::ClusterRpc { payload } = frame else {
        panic!("应收到 ClusterRpc 帧");
    };
    let sent: WireMessage = serde_json::from_value(payload).unwrap();
    assert_eq!(sent.id, "f-1");
    assert_eq!(sent.from, "node-a");
    assert_eq!(sent.to, "node-b");
    assert_eq!(sent.msg_type, "request", "转发原帧（不改写）");

    // B → hub：响应 to=node-a（B 端 handler 回程语义）→ 转发 bridge-a。
    let out = hub
        .on_cluster_frame(
            "bridge-b",
            serde_json::to_value(bridge_wire("f-1", "response", "node-b", "node-a")).unwrap(),
        )
        .await;
    assert!(out.is_none(), "响应转发不回帧");
    let frame = loop {
        let f = tokio::time::timeout(Duration::from_secs(2), rx_a.recv())
            .await
            .expect("响应回程应到达 bridge-a")
            .expect("队列不应关闭");
        if matches!(f, BridgeFrame::ClusterRpc { .. }) {
            break f;
        }
    };
    let BridgeFrame::ClusterRpc { payload } = frame else {
        panic!("应收到 ClusterRpc 帧");
    };
    let back: WireMessage = serde_json::from_value(payload).unwrap();
    assert_eq!(back.id, "f-1");
    assert_eq!(back.from, "node-b");
    assert_eq!(back.to, "node-a");
    assert_eq!(back.msg_type, "response");
}

/// 三期批次九：转发不可达面——目标无映射 / 来源自环 → 请求方向诚实
/// error 回来源链路；响应回程不可转发 → 静默丢弃（发起方 pending 超时收口）。
#[tokio::test]
async fn hub_forward_unreachable_falls_to_honest_error() {
    let env = forward_env();

    // 目标无桥链路映射（直连节点 / 未知节点）→ 诚实 error 回 bridge-a。
    let out = env
        .hub
        .on_cluster_frame(
            "bridge-a",
            serde_json::to_value(bridge_wire("g-1", "request", "node-a", "node-ghost")).unwrap(),
        )
        .await
        .expect("不可达请求应回 error 帧");
    let resp: WireMessage = serde_json::from_value(out).unwrap();
    assert_eq!(resp.msg_type, "error");
    assert!(resp.error.contains("node-ghost"));
    assert!(resp.error.contains("not reachable"));

    // 自环（to == 来源设备自身集群 id）→ 不转发，诚实 error。
    let out = env
        .hub
        .on_cluster_frame(
            "bridge-a",
            serde_json::to_value(bridge_wire("g-2", "request", "node-a", "node-a")).unwrap(),
        )
        .await
        .expect("自环请求应回 error 帧");
    let resp: WireMessage = serde_json::from_value(out).unwrap();
    assert_eq!(resp.msg_type, "error");

    // 响应 to 无映射（node-ghost）→ deliver miss + 转发 miss → 静默丢弃。
    let out = env
        .hub
        .on_cluster_frame(
            "bridge-a",
            serde_json::to_value(bridge_wire("g-3", "response", "node-x", "node-ghost")).unwrap(),
        )
        .await;
    assert!(out.is_none(), "不可转发响应静默丢弃");

    // 响应 to == hub 本机（本机 pending 的迟到响应）→ 丢弃不转发。
    let out = env
        .hub
        .on_cluster_frame(
            "bridge-a",
            serde_json::to_value(bridge_wire("g-4", "response", "node-x", "node-hub")).unwrap(),
        )
        .await;
    assert!(out.is_none(), "本机迟到响应丢弃");
}
