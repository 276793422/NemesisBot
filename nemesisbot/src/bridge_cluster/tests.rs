//! 桥集群身份注册测试（goal 二期批次五）。
//!
//! 覆盖：online 注册（同权进 registry）/ offline 标离线 / rpc_port==0
//! 闸（不注册）/ 纯隧道事件（无集群身份）不动 registry / 顶替重连幂等。

use super::*;
use nemesis_cluster::types::ClusterConfig;
use nemesis_web::relay::{BridgeClusterIdentity, BridgeIdentityEvent};

/// 构造测试 Cluster（tempdir workspace，不 start 网络——registry 操作全同步）。
fn test_cluster() -> (Arc<Cluster>, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config = ClusterConfig {
        node_id: String::new(),
        bind_address: "0.0.0.0:0".to_string(),
        peers: vec![],
        node_name: "TestHub".to_string(),
    };
    let cluster = Arc::new(Cluster::with_workspace(config, tmp.path().to_path_buf()));
    (cluster, tmp)
}

fn identity(node_id: &str, rpc_port: u16) -> BridgeClusterIdentity {
    BridgeClusterIdentity {
        node_id: node_id.to_string(),
        name: "远端机".to_string(),
        role: "worker".to_string(),
        category: "general".to_string(),
        tags: vec![],
        capabilities: vec!["chat".to_string()],
        node_type: "agent".to_string(),
        rpc_port,
        addresses: vec!["10.0.0.5".to_string()],
    }
}

fn online_event(bridge: &str, cluster_id: &str, rpc_port: u16) -> BridgeIdentityEvent {
    BridgeIdentityEvent {
        bridge_node_id: bridge.to_string(),
        online: true,
        cluster: Some(identity(cluster_id, rpc_port)),
    }
}

fn offline_event(bridge: &str, cluster_id: &str) -> BridgeIdentityEvent {
    BridgeIdentityEvent {
        bridge_node_id: bridge.to_string(),
        online: false,
        // 离线事件的身份是服务端表项快照（端口值此时无关紧要）。
        cluster: Some(identity(cluster_id, 0)),
    }
}

#[test]
fn online_registers_cluster_node_with_full_identity() {
    let (cluster, _tmp) = test_cluster();
    let sink = BridgeClusterSink::new(cluster.clone());

    sink.on_bridge_identity(online_event("bridge-x", "node-x-1", 21949));

    // 注册成功：registry 有节点且在线（同权语义——与 UDP 发现节点同表）。
    let peer = cluster
        .get_peer("node-x-1")
        .expect("桥入设备应注册进 registry");
    assert_eq!(peer.base.name, "远端机");
    // base.role 是 NodeRole 枚举（registry 归一形态），Debug 小写比对。
    assert_eq!(format!("{:?}", peer.base.role).to_lowercase(), "worker");
    assert_eq!(peer.node_type, "agent");
    assert!(peer.addresses.contains(&"10.0.0.5".to_string()));
    assert!(peer.is_online(), "注册即 Online（发现语义）");
}

#[test]
fn offline_marks_registry_node_offline() {
    let (cluster, _tmp) = test_cluster();
    let sink = BridgeClusterSink::new(cluster.clone());

    sink.on_bridge_identity(online_event("bridge-x", "node-x-1", 21949));
    sink.on_bridge_identity(offline_event("bridge-x", "node-x-1"));

    // 离线：registry 保留条目（health 语义）但状态 Offline。
    let peer = cluster.get_peer("node-x-1").expect("离线不删表项");
    assert!(!peer.is_online(), "桥断开应标 Offline");
}

#[test]
fn rpc_port_zero_is_not_registered() {
    let (cluster, _tmp) = test_cluster();
    let sink = BridgeClusterSink::new(cluster.clone());

    // 没开 RPC server 的桥设备：不注册（诚实语义——无派发价值）。
    sink.on_bridge_identity(online_event("bridge-bare", "node-bare-1", 0));
    assert!(
        cluster.get_peer("node-bare-1").is_none(),
        "rpc_port=0 不得注册进 registry"
    );

    // 它断开时映射 miss → 无动作（不 panic、不误标他人）。
    sink.on_bridge_identity(offline_event("bridge-bare", "node-bare-1"));
    assert!(cluster.get_peer("node-bare-1").is_none());
}

#[test]
fn tunnel_only_event_is_ignored() {
    let (cluster, _tmp) = test_cluster();
    let sink = BridgeClusterSink::new(cluster.clone());

    // 纯隧道设备（hello 未带集群身份 → cluster=None）：sink 无动作。
    sink.on_bridge_identity(BridgeIdentityEvent {
        bridge_node_id: "bridge-tunnel-only".to_string(),
        online: true,
        cluster: None,
    });
    assert!(cluster.get_peer("bridge-tunnel-only").is_none());
}

#[test]
fn replacement_reconnect_is_idempotent() {
    let (cluster, _tmp) = test_cluster();
    let sink = BridgeClusterSink::new(cluster.clone());

    // 顶替重连：同一桥链路重复 online（映射覆盖 + registry upsert 幂等），
    // 旧循环退出不报 offline（relay 侧代际闸保证——sink 只需幂等吃 online）。
    sink.on_bridge_identity(online_event("bridge-x", "node-x-1", 21949));
    sink.on_bridge_identity(online_event("bridge-x", "node-x-1", 21949));
    assert!(
        cluster.get_peer("node-x-1").is_some_and(|p| p.is_online()),
        "顶替重连后仍 Online"
    );
    // 正常断开（新连接的读循环退出）→ Offline。
    sink.on_bridge_identity(offline_event("bridge-x", "node-x-1"));
    assert!(
        cluster.get_peer("node-x-1").is_some_and(|p| !p.is_online()),
        "最终断开应 Offline"
    );
}
