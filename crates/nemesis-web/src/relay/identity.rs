//! 桥设备集群身份交换（goal 二期批次五）。
//!
//! 设计要点：relay 模块（nemesis-web）**零依赖** nemesis-cluster——身份
//! 事件经无类型回调槽 [`BridgeIdentitySink`] 抛给宿主进程，注册进
//! registry / 标 Offline 的动作由宿主完成（nemesisbot 正常模式注入
//! sink；`--relay` 纯中继不注入 = 只转发不注册，边界不动）。
//!
//! 身份两空间：桥链路身份 `bridge-{hostname}`（conn 路由 / 顶替锚点，
//! hello `node_id` 字段）与集群身份 `node-...`（registry 锚点，集群身份
//! 字段）并存，hello 同时携带两者。

use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

/// 桥设备携带的集群身份快照（hello 增补字段组装产物；`--relay` 场景仅
/// 存表不消费）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BridgeClusterIdentity {
    /// 集群 registry 锚点 node_id（`node-...`）。
    pub node_id: String,
    /// 集群显示名（config.cluster.node_name 或 hostname 解析链产物）。
    pub name: String,
    /// 集群角色（worker/master/...）。
    pub role: String,
    /// 集群类别。
    pub category: String,
    /// 集群标签。
    pub tags: Vec<String>,
    /// RPC 能力（工具名清单）。
    pub capabilities: Vec<String>,
    /// 节点类型（agent/node）。
    pub node_type: String,
    /// 本机 RPC server 端口（0 = 未启动；宿主 sink 侧不注册集群节点）。
    pub rpc_port: u16,
    /// 本机网卡地址清单（同网段直连仲裁用）。
    pub addresses: Vec<String>,
}

/// 身份事件：设备上线（登记成功）/ 离线（断开、心跳踢、开关踢）。
/// 顶替场景：新连接先报 online，旧循环退出时因代际失配**不**报 offline。
#[derive(Debug, Clone)]
pub struct BridgeIdentityEvent {
    /// 桥链路 node_id（`bridge-{hostname}`）。
    pub bridge_node_id: String,
    /// true = 上线登记；false = 离线。
    pub online: bool,
    /// 集群身份（hello 未携带 = None——纯隧道设备，宿主 sink 跳过注册）。
    pub cluster: Option<BridgeClusterIdentity>,
}

/// 身份事件回调槽（宿主注入）。同步 trait：registry 注册/标离线均为
/// 同步操作，事件泵不需要异步。
pub trait BridgeIdentitySink: Send + Sync {
    fn on_bridge_identity(&self, event: BridgeIdentityEvent);
}

/// RelayServer 内部槽位形态（Mutex 单槽，运行时注入一次）。
#[derive(Default)]
pub(crate) struct IdentitySinkSlot {
    inner: Mutex<Option<Arc<dyn BridgeIdentitySink>>>,
}

impl IdentitySinkSlot {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 注入 sink（宿主装配处调用；`--relay` 不调用 = 保持 None）。
    pub(crate) fn set(&self, sink: Arc<dyn BridgeIdentitySink>) {
        *self.inner.lock().expect("identity sink 槽锁中毒") = Some(sink);
    }

    /// 分发事件（无 sink = 静默丢弃——纯隧道语义）。
    pub(crate) fn notify(&self, event: BridgeIdentityEvent) {
        let guard = self.inner.lock().expect("identity sink 槽锁中毒");
        if let Some(sink) = guard.as_ref() {
            sink.on_bridge_identity(event);
        }
    }
}
