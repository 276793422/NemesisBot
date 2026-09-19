//! 二期（桥集群）：上行 `cluster_rpc` 帧出口槽（批次六）。
//!
//! 职责：relay 收到设备的上行 [`BridgeFrame::ClusterRpc`] 帧后，交由宿主
//! 注入的 [`ClusterFrameSink`] 处理——hub 形态下喂本地 RPC handler 链
//! （[`crate::...`] 由宿主决定），响应 payload 返回后由 relay 封帧回上行。
//!
//! **relay 零集群依赖边界不动**：本模块只有无类型 trait + 槽位（与
//! `identity.rs` 同款装配模式），不 import nemesis-cluster；帧内的
//! `WireMessage` 解析与分发全在宿主侧。
//!
//! **一期兼容**：未装配 sink（`--relay` 纯中继 / 一期形态）→ 上行
//! `cluster_rpc` 帧维持「WARN 忽略」语义。
//!
//! 三期扩展点：`to` 路由判定（自己 / 转发其它成员 / 诚实 error）在宿主
//! sink 内实现——relay 侧只做无差别搬运，届时无需改协议。

use serde_json::Value;
use std::sync::{Arc, Mutex};

/// 上行 `cluster_rpc` 帧处理器（宿主注入）。
///
/// * `from_device`：桥链路 node_id（`bridge-*` 空间——帧 `from` 的可信来源
///   校验锚点；payload 里的 `from` 字段由宿主按需覆盖/校验）。
/// * `payload`：ClusterRpc 帧载荷（宿主自行解析——hub 侧为 `WireMessage`
///   JSON）。
/// * 返回 `Some(resp_payload)`：relay 封 `ClusterRpc` 帧回上行（设备侧按
///   帧内 id 匹配 pending）；`None`：无需回帧（宿主已另行处理或决定静默）。
///
/// 返回 boxed future（dyn 兼容）——喂本地 RPC 链是 async 调用。
pub trait ClusterFrameSink: Send + Sync {
    fn on_cluster_frame(
        &self,
        from_device: &str,
        payload: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<Value>> + Send + '_>>;
}

/// sink 槽（运行期装配一次；`Mutex<Option<Arc<dyn _>>>` 与 identity 槽同款）。
#[derive(Default)]
pub(crate) struct ClusterFrameSinkSlot {
    inner: Mutex<Option<Arc<dyn ClusterFrameSink>>>,
}

impl ClusterFrameSinkSlot {
    pub(crate) fn set(&self, sink: Arc<dyn ClusterFrameSink>) {
        *self.inner.lock().expect("cluster_frame sink 槽锁中毒") = Some(sink);
    }

    pub(crate) fn get(&self) -> Option<Arc<dyn ClusterFrameSink>> {
        self.inner
            .lock()
            .expect("cluster_frame sink 槽锁中毒")
            .clone()
    }
}

#[cfg(test)]
mod tests;
