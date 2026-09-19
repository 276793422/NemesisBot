//! 桥帧 RPC 通道（goal 二期批次六）。
//!
//! 职责：把 `cluster_rpc` 桥帧变成 [`nemesis_cluster::rpc::client::RpcClient`]
//! 的可用传输路径——hub 与设备两侧对称，共用 [`BridgeRpcChannel`] pending 表
//! （请求 id → oneshot 唤醒）：
//!
//! - **hub 侧 [`HubBridgeRpc`]**：实现 [`BridgeSend`]（RpcClient 仲裁的桥
//!   出口——下行经 [`RelayServer::send_to_device`] 按桥↔集群映射投递）+
//!   [`ClusterFrameSink`]（上行帧分流：response/error → 唤醒 pending；
//!   request → 喂本地 RPC handler 链 → 响应封帧回上行）。
//! - **设备侧 [`DeviceBridgeRpc`]**：实现 [`BridgeSend`]（出口 = 桥上行）+
//!   下行分流入口 [`DeviceBridgeRpc::handle_downstream`]（bridge_client
//!   下行泵调用，分流语义与 hub 侧对称）。
//!
//! **零语义分叉红线**：帧载荷就是 TCP 路径同构的 `WireMessage`——请求帧
//! 由 RpcClient 统一组帧（[`wire_from_request`]），响应帧统一走
//! [`rpc_response_from_wire`]（对齐 `Frame::decode_response`）；request 方向
//! 喂 [`RpcServer::handle_wire_message`]（与 TCP 服务端共用同一 handler 链）。
//!
//! **路由语义（三期批次九定稿）**：`to` 为空或等于本机集群 node_id →
//! 本地处理；`to` 指向其它节点 → hub 侧查桥↔集群映射：目标有桥链路且
//! 在线（非来源链路自环）→ 原帧转发目标桥（A→hub→B），B 的响应 `to`
//! 即发起方集群 id，按同款映射回程（无中转表——无状态、无泄漏）；映射
//! miss / 目标离线 → 诚实 error 回帧。设备侧不转发（转发只经 hub），
//! `to` 非本机的下行帧维持诚实 error（防御）。
//!
//! **安全边界不动**：能桥入 = 已过 ws token 鉴权；帧内 `from` 按 TCP 路径
//! 同款信任（server 端 handler 不验 from 归属）——桥不新增信任边界。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nemesis_cluster::rpc::client::{BridgeSend, RpcClientError};
use nemesis_cluster::rpc::server::RpcServer;
use nemesis_cluster::transport::conn::WireMessage;
use nemesis_web::relay::protocol::BridgeFrame;
use nemesis_web::relay::{ClusterFrameSink, RelayServer};

use crate::bridge_cluster::BridgeClusterSink;

/// pending 响应表 + 下行发送出口。
///
/// 两侧共用：`request` 登记并发帧、`deliver` 按请求 id 唤醒。发送出口是
/// 同步闭包（hub 侧 `send_to_device` 与设备侧上行 channel `try_send` 都是
/// 同步操作），返回 false = 链路不可达（出口缺失/队列满/设备不在线）。
struct BridgeRpcChannel {
    pending: Mutex<HashMap<String, tokio::sync::oneshot::Sender<WireMessage>>>,
    /// 下行发送出口：`(目标锚点, 请求帧) -> 链路可达`。目标锚点 hub 侧 =
    /// 桥链路 node_id（`bridge-*`），设备侧忽略（单链路，唯一对端 = hub）。
    send_down: Box<dyn Fn(&str, WireMessage) -> bool + Send + Sync>,
}

impl BridgeRpcChannel {
    fn new(send_down: Box<dyn Fn(&str, WireMessage) -> bool + Send + Sync>) -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            send_down,
        }
    }

    /// 发请求帧并等响应（RpcClient 桥出口的执行体；超时与 dispatch 外层
    /// deadline 同值——桥内不再叠加额外时限）。
    async fn request(
        &self,
        target: &str,
        wire: WireMessage,
        timeout: Duration,
    ) -> Result<WireMessage, RpcClientError> {
        let request_id = wire.id.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pending
            .lock()
            .expect("桥 pending 表锁中毒")
            .insert(request_id.clone(), tx);
        if !(self.send_down)(target, wire) {
            // 发不出去（设备不在线 / 上行未挂载）：立刻撤销登记，诚实失败
            // ——让 RpcClient 仲裁按「桥路径失败」走兜底/报错，不等超时。
            self.pending
                .lock()
                .expect("桥 pending 表锁中毒")
                .remove(&request_id);
            return Err(RpcClientError::Connection(format!(
                "bridge link to {target} unreachable"
            )));
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(resp)) => Ok(resp),
            // 响应端 drop（会话结束 detach 时清表）= 桥链路死亡。
            Ok(Err(_)) => Err(RpcClientError::Connection(
                "bridge responder dropped（桥会话结束）".into(),
            )),
            Err(_) => {
                // 超时撤销登记；迟到响应将因 miss 被 deliver 丢弃。
                self.pending
                    .lock()
                    .expect("桥 pending 表锁中毒")
                    .remove(&request_id);
                Err(RpcClientError::Timeout)
            }
        }
    }

    /// 下行响应帧按 id 唤醒 pending。false = 无匹配（迟到/未知响应——
    /// hub 侧随后按 `to` 判定是否跨桥转发回发起方，三期批次九）。
    fn deliver(&self, resp: &WireMessage) -> bool {
        match self
            .pending
            .lock()
            .expect("桥 pending 表锁中毒")
            .remove(&resp.id)
        {
            Some(tx) => {
                let _ = tx.send(resp.clone());
                true
            }
            None => false,
        }
    }

    /// 会话收尾：清空 pending（各 oneshot 随 drop 唤醒为 Err → 调用方按
    /// 「桥链路死亡」处理）。
    fn clear(&self) {
        self.pending.lock().expect("桥 pending 表锁中毒").clear();
    }
}

/// 判定上行/下行帧是否为响应方向（response / error 都承载「对某请求的
/// 回执」语义——`WireMessage::new_error` 的 msg_type 是 "error"）。
fn is_response(wire: &WireMessage) -> bool {
    wire.msg_type == "response" || wire.msg_type == "error"
}

// ---------------------------------------------------------------------------
// hub 侧：RpcClient 桥出口 + 上行 cluster_rpc 分流
// ---------------------------------------------------------------------------

/// hub 侧桥 RPC 枢纽：一个结构同时充当 RpcClient 的 [`BridgeSend`] 出口
/// （主动发往设备）与 relay 的 [`ClusterFrameSink`]（设备的上行帧分流）。
pub struct HubBridgeRpc {
    /// hub 主动发往设备的 pending 表。
    channel: BridgeRpcChannel,
    /// 本地 RPC handler 链（设备发来的 request 喂这里；与 TCP 服务端共用）。
    rpc_server: Arc<RpcServer>,
    /// 本机集群 node_id（`to` 路由判定锚点）。
    self_node_id: String,
    /// 桥↔集群映射反查（send_over_bridge 的集群 id → 桥链路 id）。
    sink_registry: Arc<BridgeClusterSink>,
    /// 下行投递 + 设备在线判定。
    relay: Arc<RelayServer>,
}

impl HubBridgeRpc {
    /// 装配（gateway 集群块内；`--relay` 纯中继不装配——relay 不注入 sink，
    /// RpcClient 不注入出口，一期语义原样）。
    pub fn new(
        rpc_server: Arc<RpcServer>,
        self_node_id: String,
        sink_registry: Arc<BridgeClusterSink>,
        relay: Arc<RelayServer>,
    ) -> Self {
        let relay_for_send = relay.clone();
        let channel = BridgeRpcChannel::new(Box::new(move |bridge_node_id, wire| {
            let Ok(payload) = serde_json::to_value(&wire) else {
                tracing::error!(
                    request_id = %wire.id,
                    "[HubBridge] 请求帧序列化失败（不可能路径）"
                );
                return false;
            };
            relay_for_send.send_to_device(bridge_node_id, BridgeFrame::ClusterRpc { payload }, 0)
        }));
        Self {
            channel,
            rpc_server,
            self_node_id,
            sink_registry,
            relay,
        }
    }

    /// 三期批次九：跨桥转发（请求方向与响应回程共用）。
    ///
    /// 判定链：目标集群 id 有桥链路（集群↔桥映射命中）→ 目标桥链路在线
    /// → `from_bridge` 防自环闸（请求方向传来源链路，目标==来源 = 病态
    /// 自环，拒绝）→ 原 wire 重序列化封 `ClusterRpc` 帧发目标桥。
    /// false = 任一判定不过（调用方回落诚实 error / 丢弃）。
    ///
    /// **无中转表**：请求转发不登记任何状态——B 的响应 `to` 即发起方
    /// 集群 id（`A.from`），经本方法同款映射转发回 bridge-a。无状态 =
    /// 无泄漏、无清理、幂等。
    fn forward_to_bridge(
        &self,
        cluster_id: &str,
        from_bridge: Option<&str>,
        wire: &WireMessage,
    ) -> bool {
        let Some(bridge_node_id) = self.sink_registry.bridge_of(cluster_id) else {
            return false;
        };
        if let Some(src) = from_bridge
            && bridge_node_id == src
        {
            return false;
        }
        if !self.relay.device_online(&bridge_node_id) {
            return false;
        }
        let Ok(payload) = serde_json::to_value(wire) else {
            return false;
        };
        tracing::info!(
            to = %cluster_id,
            via = %bridge_node_id,
            request_id = %wire.id,
            "[HubBridge] 跨桥转发帧"
        );
        self.relay
            .send_to_device(&bridge_node_id, BridgeFrame::ClusterRpc { payload }, 0)
    }
}

impl BridgeSend for HubBridgeRpc {
    fn bridge_online(&self, peer_id: &str) -> bool {
        // peer（集群 node_id）有桥链路且该链路在线（设备表有登记）。
        self.sink_registry
            .bridge_of(peer_id)
            .map(|bridge_node_id| self.relay.device_online(&bridge_node_id))
            .unwrap_or(false)
    }

    fn send_over_bridge(
        &self,
        peer_id: &str,
        request: WireMessage,
        timeout: Duration,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<WireMessage, RpcClientError>> + Send + '_>,
    > {
        // block 内只借 &self（单一 lifetime）；peer_id 拷贝进块——两个借用
        // 生命周期无法统一到 trait 的单一匿名 lifetime。
        let registry = self.sink_registry.clone();
        let peer_id = peer_id.to_string();
        Box::pin(async move {
            // 集群 id → 桥链路 id（映射 miss = 该 peer 无桥链路——仲裁的
            // bridge_online 已闸住，这里防御性再判）。
            let Some(bridge_node_id) = registry.bridge_of(&peer_id) else {
                return Err(RpcClientError::Connection(format!(
                    "peer {peer_id} has no bridge link"
                )));
            };
            self.channel
                .request(&bridge_node_id, request, timeout)
                .await
        })
    }
}

impl ClusterFrameSink for HubBridgeRpc {
    fn on_cluster_frame(
        &self,
        from_device: &str,
        payload: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<serde_json::Value>> + Send + '_>>
    {
        // block 内只借 &self（单一 lifetime，对齐 trait 返回类型的 elision）；
        // from_device 拷贝进块——引用捕获会把 '1 带进输出 lifetime。
        let from_device = from_device.to_string();
        Box::pin(async move {
            let Ok(wire) = serde_json::from_value::<WireMessage>(payload) else {
                tracing::warn!(
                    from_device = %from_device,
                    "[HubBridge] cluster_rpc 载荷无法解析为 WireMessage，忽略"
                );
                return None;
            };

            // 响应方向：hub 主动发往设备的请求回来了 → 唤醒 pending。
            if is_response(&wire) {
                if !self.channel.deliver(&wire) {
                    // 三期批次九：pending miss 且 `to` 指向其它节点 = 桥入
                    // 设备 A 发往设备 B 的请求之响应回程（B 的响应 to=A）
                    // → 按映射转发回发起方桥链路。仍 miss → 迟到/未知，丢弃。
                    if wire.to.is_empty()
                        || wire.to == self.self_node_id
                        || !self.forward_to_bridge(&wire.to, None, &wire)
                    {
                        tracing::debug!(
                            from_device = %from_device,
                            "[HubBridge] 响应无匹配 pending 且不可转发（迟到/未知），丢弃"
                        );
                    }
                }
                return None;
            }
            if wire.msg_type != "request" {
                // 对齐 TCP 服务端读循环：只处理 request，其余忽略。
                tracing::warn!(
                    from_device = %from_device,
                    msg_type = %wire.msg_type,
                    "[HubBridge] 非 request/response 帧，忽略"
                );
                return None;
            }

            // 请求方向：`to` 路由三分支（三期批次九定稿）：
            // 1. `to` 空 / 本机 → 喂本地 RPC 链（下方）；
            // 2. `to` 有桥链路且在线（且非来源链路自环）→ 原帧转发目标桥；
            // 3. 其余（未知目标 / 目标桥离线 / 自环）→ 诚实 error 回帧。
            if !wire.to.is_empty() && wire.to != self.self_node_id {
                if self.forward_to_bridge(&wire.to, Some(&from_device), &wire) {
                    return None;
                }
                tracing::info!(
                    from_device = %from_device,
                    to = %wire.to,
                    "[HubBridge] 帧目标无可用桥链路，回诚实 error"
                );
                let mut err = wire.clone();
                err.error = format!("node {} not reachable via this hub", wire.to);
                err.msg_type = "error".into();
                return serde_json::to_value(err).ok();
            }

            // 喂本地 RPC 链（与 TCP 服务端同一 handler 链——零行为分叉）。
            let resp = self.rpc_server.handle_wire_message(wire).await;
            serde_json::to_value(resp).ok()
        })
    }
}

// ---------------------------------------------------------------------------
// 设备侧：RpcClient 桥出口 + 下行 cluster_rpc 分流
// ---------------------------------------------------------------------------

/// 设备侧桥 RPC 枢纽：RpcClient 的 [`BridgeSend`] 出口（经桥上行发往 hub）
/// + 下行帧分流入口（bridge_client 下行泵调用）。
pub struct DeviceBridgeRpc {
    /// 设备发往 hub 的 pending 表。
    channel: BridgeRpcChannel,
    /// 本地 RPC handler 链（hub 发来的 request 喂这里）。
    rpc_server: Arc<RpcServer>,
    /// 本机集群 node_id（`to` 路由判定锚点）。
    self_node_id: String,
    /// 宿主集群句柄（三期批次八：member_sync 成员合并进 registry；装配时
    /// 由 gateway 注入——仅集群构建，与本模块 cfg 门一致）。
    cluster: Option<Arc<nemesis_cluster::cluster::Cluster>>,
    /// 桥成员表：hub 广播摘要里的**在线**成员 node_id（含 hub 自己；自己
    /// 除外）。经桥可达的判定面——detach（hub 离线）即清空 = 成员表失效
    /// 降级（goal 批次八；仲裁只问 [`BridgeSend::bridge_online`]）。
    bridge_members: Mutex<std::collections::HashSet<String>>,
    /// 桥上行出口（session() 启动时挂载 cmd_tx，结束 detach——装配早于
    /// 会话建立，运行期插拔）。
    uplink: Arc<Mutex<Option<tokio::sync::mpsc::UnboundedSender<BridgeFrame>>>>,
}

impl DeviceBridgeRpc {
    /// 装配（gateway spawn 桥客户端前；仅集群构建）。`cluster` = 宿主集群
    /// 句柄（member_sync 合并进 registry；测试可传 None——只测通道语义）。
    pub fn new(
        rpc_server: Arc<RpcServer>,
        self_node_id: String,
        cluster: Option<Arc<nemesis_cluster::cluster::Cluster>>,
    ) -> Self {
        let uplink = Arc::new(Mutex::new(
            None::<tokio::sync::mpsc::UnboundedSender<BridgeFrame>>,
        ));
        let uplink_for_send = uplink.clone();
        let channel = BridgeRpcChannel::new(Box::new(move |_target, wire| {
            let Ok(payload) = serde_json::to_value(&wire) else {
                tracing::error!(
                    request_id = %wire.id,
                    "[DeviceBridge] 请求帧序列化失败（不可能路径）"
                );
                return false;
            };
            match uplink_for_send.lock().expect("桥上行出口锁中毒").as_ref() {
                Some(tx) => tx.send(BridgeFrame::ClusterRpc { payload }).is_ok(),
                None => false,
            }
        }));
        Self {
            channel,
            rpc_server,
            self_node_id,
            cluster,
            bridge_members: Mutex::new(std::collections::HashSet::new()),
            uplink,
        }
    }

    /// session() 启动时挂载上行出口（conn 泵同款 unbounded channel）。
    pub fn attach_uplink(&self, tx: tokio::sync::mpsc::UnboundedSender<BridgeFrame>) {
        *self.uplink.lock().expect("桥上行出口锁中毒") = Some(tx);
    }

    /// session() 收尾：摘除上行出口 + 清 pending（在途请求按链路死亡收口）
    /// 与清成员表（goal 批次八「hub 离线后成员表失效降级」——hub 不可达时
    /// 经桥成员表一并失效，仲裁回落直连）。
    pub fn detach_uplink(&self) {
        *self.uplink.lock().expect("桥上行出口锁中毒") = None;
        self.channel.clear();
        self.bridge_members.lock().expect("桥成员表锁中毒").clear();
    }

    /// 三期批次八：下行 `member_sync` 帧合并（bridge_client 下行泵调用）。
    ///
    /// 语义（goal 定稿）：members 里 `online=true` 且非本机的成员 →
    /// `cluster.handle_discovered_node` 合并（tags 加 `via-bridge` 哨兵，
    /// 标「经 hub」路径）+ 写入本地桥成员表；`online=false` 不合并（离线
    /// 成员不入 registry，避免洗白 hub 侧 Offline 语义）；本机自己跳过。
    /// 快照闭包拉模式 + 广播全量 → 每次**全量替换**成员表（成员从摘要
    /// 消失 = 下次 detach 前的过期成员自然清除）。
    pub fn handle_member_sync(&self, payload: &serde_json::Value) {
        let Some(members) = payload.get("members").and_then(|m| m.as_array()) else {
            tracing::warn!("[DeviceBridge] member_sync 载荷无 members 数组，忽略");
            return;
        };
        let mut fresh = std::collections::HashSet::new();
        for m in members {
            let Some(node_id) = m.get("node_id").and_then(|v| v.as_str()) else {
                continue;
            };
            // 离线成员不合并（语义见方法 doc）。
            if !m.get("online").and_then(|v| v.as_bool()).unwrap_or(false) {
                continue;
            }
            // 自己的条目（hub 摘要含设备自身）跳过——本机已在本地 registry。
            if node_id == self.self_node_id {
                continue;
            }
            fresh.insert(node_id.to_string());

            // registry 合并（cluster 未装配 = 纯通道模式，只维护成员表）。
            let Some(cluster) = self.cluster.as_ref() else {
                continue;
            };
            let name = m
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let addresses: Vec<String> = m
                .get("addresses")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            let rpc_port = m.get("rpc_port").and_then(|v| v.as_u64()).unwrap_or(0) as u16;
            let role = m
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or("worker")
                .to_string();
            let category = m
                .get("category")
                .and_then(|v| v.as_str())
                .unwrap_or("general")
                .to_string();
            let capabilities: Vec<String> = m
                .get("capabilities")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            let node_type = m
                .get("node_type")
                .and_then(|v| v.as_str())
                .unwrap_or("agent")
                .to_string();
            // via-bridge 哨兵：标「经 hub 广播得知」的路径来源（goal 批次八）。
            let tags = vec!["via-bridge".to_string()];
            cluster.handle_discovered_node(
                node_id,
                &name,
                addresses,
                rpc_port,
                &role,
                &category,
                tags,
                capabilities,
                &node_type,
            );
        }
        *self.bridge_members.lock().expect("桥成员表锁中毒") = fresh;
    }

    /// 下行 `cluster_rpc` 帧分流（bridge_client 下行泵调用）。
    ///
    /// 返回 `Some(frame)` = 需回上行（request 的响应帧）；`None` = 无需回帧
    /// （response 已唤醒本地 pending / 帧被忽略）。
    pub async fn handle_downstream(&self, payload: serde_json::Value) -> Option<BridgeFrame> {
        let Ok(wire) = serde_json::from_value::<WireMessage>(payload) else {
            tracing::warn!("[DeviceBridge] cluster_rpc 载荷无法解析为 WireMessage，忽略");
            return None;
        };

        // 响应方向：设备发往 hub 的请求回来了 → 唤醒 pending。
        if is_response(&wire) {
            if !self.channel.deliver(&wire) {
                tracing::debug!("[DeviceBridge] 响应无匹配 pending（迟到/未知），丢弃");
            }
            return None;
        }
        if wire.msg_type != "request" {
            // 对齐 TCP 服务端读循环：只处理 request，其余忽略。
            tracing::warn!(
                msg_type = %wire.msg_type,
                "[DeviceBridge] 非 request/response 帧，忽略"
            );
            return None;
        }

        // 请求方向：`to` 路由。设备只处理发给自己（或广播）的帧；hub 不会
        // 把别人的帧转给设备（三期转发也不经设备），防御性诚实 error。
        if !wire.to.is_empty() && wire.to != self.self_node_id {
            tracing::info!(
                to = %wire.to,
                "[DeviceBridge] 帧目标非本机，回诚实 error"
            );
            let mut err = wire.clone();
            err.error = format!("node {} not on this device", wire.to);
            err.msg_type = "error".into();
            return Some(BridgeFrame::ClusterRpc {
                payload: serde_json::to_value(err).ok()?,
            });
        }

        // 喂本地 RPC 链（与 TCP 服务端同一 handler 链——零行为分叉）。
        let resp = self.rpc_server.handle_wire_message(wire).await;
        Some(BridgeFrame::ClusterRpc {
            payload: serde_json::to_value(resp).ok()?,
        })
    }
}

impl BridgeSend for DeviceBridgeRpc {
    fn bridge_online(&self, peer_id: &str) -> bool {
        // 设备视角：经桥可达 = 上行出口挂载（桥会话活着）**且** peer 在
        // hub 广播的在线成员表里（三期批次八；hub 自己也在摘要里）。表空
        // = 尚未收到 sync / 已 detach——按不可达处理，仲裁回落直连。
        self.uplink.lock().expect("桥上行出口锁中毒").is_some()
            && self
                .bridge_members
                .lock()
                .expect("桥成员表锁中毒")
                .contains(peer_id)
    }

    fn send_over_bridge(
        &self,
        _peer_id: &str,
        request: WireMessage,
        timeout: Duration,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<WireMessage, RpcClientError>> + Send + '_>,
    > {
        // 桥出口只负责搬帧：响应转换（WireMessage → RPCResponse）由
        // RpcClient dispatch 统一做（rpc_response_from_wire）——出口不做。
        Box::pin(async move { self.channel.request("", request, timeout).await })
    }
}

#[cfg(test)]
mod tests;
