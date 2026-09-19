//! 桥集群身份注册（goal 二期批次五，hub 侧）。
//!
//! 职责：实现 [`BridgeIdentitySink`]——把桥 hello 携带的集群身份事件落进
//! 本机集群 registry：上线 → [`Cluster::handle_discovered_node`]（同权注册，
//! 与局域网 UDP 发现节点一致，不降级）；离线 → 映射表移除 +
//! [`Cluster::mark_peer_offline`]。
//!
//! **桥↔集群两空间映射**：身份事件的锚点是桥链路 node_id（`bridge-*`），
//! registry 锚点是集群 node_id（`node-*`）——映射表把前者翻译成后者；
//! 同一桥链路顶替重连时映射关系不变（集群身份稳定），重复 online 幂等
//! upsert。
//!
//! **rpc_port==0 防御**：registry 对 rpc_port==0 的发现直接丢弃——本 sink
//! 前置同样的闸并 WARN（诚实语义：没开 RPC server 的桥设备没有集群派发
//! 价值，只作隧道设备）。
//!
//! `--relay` 纯中继不注入本 sink（gateway 装配侧决定）——relay 模块保持
//! 零集群依赖，「只转发不注册」边界不动。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use nemesis_cluster::cluster::Cluster;
use nemesis_web::relay::{BridgeIdentityEvent, BridgeIdentitySink};

/// 桥身份事件 → 集群 registry 注册器。
pub struct BridgeClusterSink {
    cluster: Arc<Cluster>,
    /// 桥链路 node_id → 集群 node_id（离线时翻译锚点用）。
    mapping: Mutex<HashMap<String, String>>,
}

impl BridgeClusterSink {
    pub fn new(cluster: Arc<Cluster>) -> Self {
        Self {
            cluster,
            mapping: Mutex::new(HashMap::new()),
        }
    }

    /// 二期批次六：集群 node_id → 桥链路 node_id 反查（hub 侧 BridgeSend
    /// 出口寻址用——RpcClient 仲裁按 registry 集群 id 说话，relay 下行按
    /// 桥链路 id 投递）。设备数小（桥拓扑单 hub 直连），线性扫即可。
    pub fn bridge_of(&self, cluster_id: &str) -> Option<String> {
        self.mapping
            .lock()
            .expect("桥↔集群映射表锁中毒")
            .iter()
            .find(|(_, c)| c.as_str() == cluster_id)
            .map(|(b, _)| b.clone())
    }
}

impl BridgeIdentitySink for BridgeClusterSink {
    fn on_bridge_identity(&self, event: BridgeIdentityEvent) {
        let Some(identity) = &event.cluster else {
            // 纯隧道设备（hello 未带集群身份）——relay 侧已只作隧道处理，
            // 这里不动 registry。
            return;
        };
        if event.online {
            if identity.rpc_port == 0 {
                tracing::warn!(
                    bridge = %event.bridge_node_id,
                    cluster_id = %identity.node_id,
                    "[BridgeCluster] 桥入设备未开 RPC server（rpc_port=0），不注册集群节点（仅作隧道设备）"
                );
                return;
            }
            self.mapping
                .lock()
                .expect("桥↔集群映射表锁中毒")
                .insert(event.bridge_node_id.clone(), identity.node_id.clone());
            // via-bridge 哨兵（hub 侧对称标记，2026-09-20）：设备侧经
            // member_sync 得知的成员已带同款哨兵（bridge_rpc
            // handle_member_sync），hub 侧补齐 = 两侧 registry 都能回答
            // 「这个 peer 是怎么接入的」。追加而非覆盖——设备自报 tags 保留。
            // 同权语义不受影响：tags 是纯元数据，RPC 路径仲裁看网段判定
            // （rpc/client.rs）不看 tags。
            let mut tags = identity.tags.clone();
            if !tags.iter().any(|t| t == "via-bridge") {
                tags.push("via-bridge".to_string());
            }
            let registered = self.cluster.handle_discovered_node(
                &identity.node_id,
                &identity.name,
                identity.addresses.clone(),
                identity.rpc_port,
                &identity.role,
                &identity.category,
                tags,
                identity.capabilities.clone(),
                &identity.node_type,
            );
            tracing::info!(
                bridge = %event.bridge_node_id,
                cluster_id = %identity.node_id,
                name = %identity.name,
                rpc_port = identity.rpc_port,
                registered,
                "[BridgeCluster] 桥入设备集群身份已注册（同权，经 handle_discovered_node）"
            );
        } else {
            // 离线：映射表移除 + registry 标 Offline（health 语义复用；
            // 未注册过（rpc_port=0 闸拦下的）映射 miss → 无动作）。
            let cluster_id = self
                .mapping
                .lock()
                .expect("桥↔集群映射表锁中毒")
                .remove(&event.bridge_node_id);
            if let Some(cluster_id) = cluster_id {
                self.cluster
                    .mark_peer_offline(&cluster_id, "bridge disconnected（桥链路断开）");
                tracing::info!(
                    bridge = %event.bridge_node_id,
                    cluster_id = %cluster_id,
                    "[BridgeCluster] 桥链路断开，集群节点已标 Offline"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests;
