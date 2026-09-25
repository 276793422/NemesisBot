//! 反向桥中继服务端状态机（goal：反向桥与多设备汇聚，一期批次一）。
//!
//! 职责：桥接入鉴权（hello 帧 token 校验）、设备表（多设备并发 + 单设备
//! 最新连接顶替旧连接）、心跳判定（90s 无心跳判离线）、conn 路由（conn_id
//! → 响应字节通道）、面板访问鉴权会话（cookie → node_id，服务端只存
//! 「已授权会话」状态，重启即失效）、运行时开关（正常模式默认开，通道页
//! 可关，重启恢复）。
//!
//! 二期（批次五）：hello 携带集群身份 → 设备表存快照，上线/离线经
//! [`BridgeIdentitySink`] 抛给宿主（宿主注册进集群 registry——relay 模块
//! 零依赖集群 crate；`--relay` 不注入 sink = 只转发不注册）。离线上报
//! 三点全覆盖：断开（代际匹配移除）/ 心跳踢 / 开关踢；顶替场景旧循环
//! 代际失配不误报 offline。
//!
//! `--relay` 纯中继与正常启动内置中继共用本状态机（`full_mode` 只影响
//! 状态页是否展示自身 dashboard 入口）。

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dashmap::DashMap;
use tokio::sync::{mpsc, oneshot};

use super::identity::{BridgeClusterIdentity, IdentitySinkSlot};
use super::protocol::{
    ACCESS_CHECK_TIMEOUT_SECS, BridgeFrame, HEARTBEAT_TIMEOUT_SECS, decode_frame, encode_frame,
};

/// 桥入设备下行发送队列深度（设备消费不过来 = 连接不健康，try_send 满即拒）。
pub(crate) const DEVICE_OUTBOUND_CAPACITY: usize = 256;

/// 维护循环周期（心跳超时扫描 + 会话过期清理 + 泄漏 conn 清理）。
const MAINTENANCE_INTERVAL_SECS: u64 = 15;

/// 面板访问授权会话有效期（内存表天然重启清空；7 天对齐常见会话语义）。
const AUTH_SESSION_TTL_SECS: u64 = 7 * 24 * 3600;

/// 短 conn 生命周期上限（超过视为泄漏清理；正常 HTTP 往返秒级）。
const CONN_MAX_AGE_SECS: u64 = 600;

/// 桥入设备表项。
pub struct DeviceEntry {
    /// 设备显示名（hello 帧携带；一期仅展示，二期并入集群 registry）。
    pub name: String,
    /// 设备版本（hello 帧携带）。
    pub version: String,
    /// 接入时刻（unix 秒）。
    pub connected_at: u64,
    /// 最近一次收到任意帧的时刻（心跳判定基准）。
    pub last_seen: Instant,
    /// 接入代际（顶替旧连接判定：新代际顶掉旧代际）。
    pub generation: u64,
    /// 服务端 → 设备下行帧队列（由 /bridge handler 的写循环消费）。
    pub outbound: mpsc::Sender<BridgeFrame>,
    /// 流量计数：浏览器 → 设备方向（请求字节）。
    pub bytes_up: AtomicU64,
    /// 流量计数：设备 → 浏览器方向（响应字节）。
    pub bytes_down: AtomicU64,
    /// 集群身份快照（二期 hello 增补字段组装；None = 纯隧道设备）。
    /// 离线上报时随事件带给宿主 sink（宿主按此定位集群节点）。
    pub cluster: Option<BridgeClusterIdentity>,
}

/// 面板访问授权会话（cookie 值 → 会话）。服务端只存「已授权」状态，
/// 不存任何 token/哈希——授权凭据比对全部发生在设备本机。
pub struct AuthSession {
    pub node_id: String,
    pub created_at: u64,
}

/// 一条 conn 的服务端登记（浏览器 ↔ 设备字节流搬运）。
pub struct ConnEntry {
    /// 所属设备 node_id。
    pub device: String,
    /// 接入代际（设备重连顶替后旧 conn 一并作废）。
    pub generation: u64,
    /// 响应字节通道（设备 ConnData → /d/ handler 的响应流 / WS 泵消费）。
    pub resp_tx: mpsc::Sender<Vec<u8>>,
    /// 建立时刻（泄漏清理用）。
    pub created_at: Instant,
}

/// 状态页/API 的设备快照。
#[derive(Debug, Clone, serde::Serialize)]
pub struct DeviceStatus {
    pub node_id: String,
    pub name: String,
    pub version: String,
    pub connected_at: u64,
    pub bytes_up: u64,
    pub bytes_down: u64,
    /// 心跳判定在线（表里有且最近帧未超时）。
    pub online: bool,
}

/// 反向桥中继服务端。
pub struct RelayServer {
    /// 接入门 token（预共享钥匙服务端半把；hello 帧校验）。
    ws_token: String,
    /// 运行时开关（正常模式默认开；通道页可关，重启恢复默认开——不写配置）。
    enabled: AtomicBool,
    /// true = 正常启动（状态页展示自身 dashboard 入口）；false = `--relay`
    /// 纯中继（状态页即全部 UI，无 dashboard 入口）。
    full_mode: bool,
    /// 设备表：node_id → 表项。
    devices: DashMap<String, DeviceEntry>,
    /// 面板访问授权会话：cookie 值 → 会话。
    sessions: DashMap<String, AuthSession>,
    /// 活跃 conn 表：conn_id → 表项。
    conns: DashMap<u64, ConnEntry>,
    /// access_check 等待表：request_id → 回执通道（设备 AccessResult 投递）。
    access_waiters: DashMap<String, oneshot::Sender<bool>>,
    next_conn_id: AtomicU64,
    next_generation: AtomicU64,
    sweeper_started: AtomicBool,
    /// 二期：集群身份事件槽（宿主注入；None = 纯隧道语义，`--relay` 即此）。
    identity_sink: IdentitySinkSlot,
    /// hub 侧集群身份（正常模式 = hub 集群 node_id；`--relay` 空）——
    /// welcome 帧带给设备，三期跨桥寻址用。
    hub_node_id: Mutex<String>,
    /// 二期批次六：上行 `cluster_rpc` 帧出口槽（宿主注入 = hub 形态；
    /// None = `--relay` / 一期形态，上行帧维持 WARN 忽略）。
    cluster_frame_sink: super::cluster_frame::ClusterFrameSinkSlot,
    /// 三期批次八：成员表快照闭包（宿主注入 = 正常模式 hub 广播集群
    /// registry 摘要；None = `--relay` 广播自身桥设备表摘要）。广播时
    /// 拉取最新（拉模式——registry 变化无需推钩子，relay 零集群依赖）。
    member_snapshot: Mutex<Option<std::sync::Arc<dyn Fn() -> serde_json::Value + Send + Sync>>>,
}

impl RelayServer {
    /// 创建中继服务端。`ws_token` 空 = 接入门不开放（调用方负责 fail-closed
    /// 判定：正常模式不 set_relay；`--relay` 直接拒绝启动）。
    pub fn new(ws_token: String, full_mode: bool) -> Self {
        Self {
            ws_token,
            enabled: AtomicBool::new(true),
            full_mode,
            devices: DashMap::new(),
            sessions: DashMap::new(),
            conns: DashMap::new(),
            access_waiters: DashMap::new(),
            next_conn_id: AtomicU64::new(1),
            next_generation: AtomicU64::new(1),
            sweeper_started: AtomicBool::new(false),
            identity_sink: IdentitySinkSlot::new(),
            hub_node_id: Mutex::new(String::new()),
            cluster_frame_sink: super::cluster_frame::ClusterFrameSinkSlot::default(),
            member_snapshot: Mutex::new(None),
        }
    }

    /// 二期批次六：注入上行 `cluster_rpc` 帧出口（宿主装配处；`--relay`
    /// 不注入——保持一期 WARN 忽略语义）。
    pub fn set_cluster_frame_sink(
        &self,
        sink: std::sync::Arc<dyn super::cluster_frame::ClusterFrameSink>,
    ) {
        self.cluster_frame_sink.set(sink);
    }

    /// 上行 `cluster_rpc` 帧出口快照（handlers 上行分发用；None = 一期形态）。
    pub(crate) fn cluster_frame_sink(
        &self,
    ) -> Option<std::sync::Arc<dyn super::cluster_frame::ClusterFrameSink>> {
        self.cluster_frame_sink.get()
    }

    /// 三期批次八：注入成员表快照闭包（宿主装配处；正常模式 = 集群
    /// registry 摘要。`--relay` 不注入——广播回落为自身桥设备表摘要）。
    pub fn set_member_snapshot(
        &self,
        snapshot: std::sync::Arc<dyn Fn() -> serde_json::Value + Send + Sync>,
    ) {
        *self.member_snapshot.lock().expect("member_snapshot 锁中毒") = Some(snapshot);
    }

    /// 三期批次八：向全部在线设备广播成员表（`member_sync` 帧）。
    ///
    /// 摘要来源：注入了快照闭包（正常模式 hub）→ 调用拉取 registry 摘要；
    /// 未注入（`--relay`）→ 自身桥设备表摘要（成员感知的纯中继形态）。
    /// 尽力而为：单设备发送失败忽略（不健康连接由心跳超时路径收口）。
    /// 接入门关闭时静默跳过（`set_enabled(false)` 清表后无需广播）。
    pub fn broadcast_member_sync(&self) {
        if !self.enabled.load(Ordering::SeqCst) {
            return;
        }
        let payload = match self
            .member_snapshot
            .lock()
            .expect("member_snapshot 锁中毒")
            .as_ref()
        {
            Some(snapshot) => snapshot(),
            None => {
                // `--relay`：桥设备表摘要（DeviceStatus → goal 定稿字段集）。
                let members: Vec<serde_json::Value> = self
                    .list_devices()
                    .iter()
                    .map(|d| {
                        serde_json::json!({
                            "node_id": d.node_id,
                            "name": d.name,
                            "online": d.online,
                            "via_bridge": false,
                            "addresses": [],
                            "rpc_port": 0,
                            "role": "device",
                            "category": "general",
                            "capabilities": [],
                            "node_type": "device",
                        })
                    })
                    .collect();
                serde_json::json!({ "members": members })
            }
        };
        let frame = BridgeFrame::MemberSync { payload };
        for entry in self.devices.iter() {
            let _ = entry.outbound.try_send(frame.clone());
        }
    }

    /// 二期：注入集群身份事件 sink（宿主装配处；`--relay` 不注入）。
    pub fn set_identity_sink(&self, sink: std::sync::Arc<dyn super::identity::BridgeIdentitySink>) {
        self.identity_sink.set(sink);
    }

    /// 二期：设置 hub 侧集群身份（welcome 帧携带；`--relay` 保持空）。
    pub fn set_hub_node_id(&self, node_id: String) {
        *self.hub_node_id.lock().expect("hub_node_id 锁中毒") = node_id;
    }

    /// hub 侧集群身份快照（welcome 帧构造用）。
    pub fn hub_node_id(&self) -> String {
        self.hub_node_id.lock().expect("hub_node_id 锁中毒").clone()
    }

    /// 上报设备离线（身份槽为空 = 静默；纯隧道设备 cluster=None 宿主跳过）。
    fn notify_identity_offline(
        &self,
        bridge_node_id: &str,
        cluster: Option<BridgeClusterIdentity>,
    ) {
        self.identity_sink
            .notify(super::identity::BridgeIdentityEvent {
                bridge_node_id: bridge_node_id.to_string(),
                online: false,
                cluster,
            });
        // 三期批次八：设备离线 = 摘要变化 → 立即广播（断开/心跳踢/开关踢
        // 三点全覆盖；开关踢时 enabled 已关，广播内部 gate 静默跳过）。
        self.broadcast_member_sync();
    }

    /// 接入门是否开放（token 已配且运行时开关为开）。
    pub fn is_gate_open(&self) -> bool {
        !self.ws_token.is_empty() && self.enabled.load(Ordering::SeqCst)
    }

    /// 是否正常启动模式（状态页 dashboard 入口显隐）。
    pub fn is_full_mode(&self) -> bool {
        self.full_mode
    }

    /// 状态页管理凭据：ws token 的 SHA-256 hex。cookie 值与此比对即通过
    /// 状态页门（服务端自己持有 token；cookie 被拿 = 同级会话泄露，不引入
    /// 额外面）。
    pub fn admin_hash_hex(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(self.ws_token.as_bytes());
        let out = hasher.finalize();
        out.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// 运行时开关（通道页调用；关闭即踢掉全部桥入设备）。
    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::SeqCst);
        if !on {
            // 先收集（node_id, 集群身份）再清表——离线上报需要身份快照。
            let kicked: Vec<(String, Option<BridgeClusterIdentity>)> = self
                .devices
                .iter()
                .map(|e| (e.key().clone(), e.cluster.clone()))
                .collect();
            let kicked_count = kicked.len();
            // drop 全部下行 sender → 各 /bridge 写循环 recv None 退出 →
            // ws 关闭 → 读循环退出 → mark_device_gone（表已被清，幂等）。
            self.devices.clear();
            for (node_id, cluster) in kicked {
                self.notify_identity_offline(&node_id, cluster);
            }
            tracing::warn!("[Relay] 中继开关已关闭，踢出 {kicked_count} 台桥入设备");
        }
    }

    /// 当前开关态（状态页/API 展示）。
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    /// 桥接入鉴权 + 设备登记。成功返回接入代际；失败返回诚实原因
    /// （`BridgeWelcome{ok:false}` 由 /bridge handler 回给设备）。
    /// 单设备重复接入 = 最新连接顶替旧连接（本地重启重连场景）。
    ///
    /// 二期：`identity` = hello 集群身份快照（None = 老版本/纯隧道设备）。
    /// 登记成功后上报 online 事件（顶替重连重复上报——宿主侧注册幂等）。
    pub fn authenticate_device(
        &self,
        token: &str,
        node_id: &str,
        name: &str,
        version: &str,
        identity: Option<BridgeClusterIdentity>,
        outbound: mpsc::Sender<BridgeFrame>,
    ) -> Result<u64, String> {
        if self.ws_token.is_empty() {
            return Err("接入门未配置（服务端未开放）".to_string());
        }
        // 常数时间比较不必要（token 非密码学密钥，网络侧无法计时探测
        // JSON 帧处理路径）；直接比对即可。
        if token != self.ws_token {
            tracing::warn!(
                node_id = %node_id,
                "[Relay] 桥接入被拒：接入门 token 不匹配（配对失败）"
            );
            return Err("接入门 token 不匹配（配对失败）".to_string());
        }
        if !self.enabled.load(Ordering::SeqCst) {
            return Err("中继开关已关闭".to_string());
        }
        if node_id.is_empty() {
            return Err("node_id 为空".to_string());
        }
        let generation = self.next_generation.fetch_add(1, Ordering::SeqCst);
        if let Some(old) = self.devices.insert(
            node_id.to_string(),
            DeviceEntry {
                name: if name.is_empty() {
                    node_id.to_string()
                } else {
                    name.to_string()
                },
                version: version.to_string(),
                connected_at: unix_now(),
                last_seen: Instant::now(),
                generation,
                outbound,
                bytes_up: AtomicU64::new(0),
                bytes_down: AtomicU64::new(0),
                cluster: identity.clone(),
            },
        ) {
            // 顶替旧连接：drop 旧下行 sender → 旧写循环退出 → 旧 ws 关闭。
            // 旧读循环发现自己代际失效后同样退出（幂等；离线不误报——
            // mark_device_gone 代际失配不通知）。
            tracing::info!(
                node_id = %node_id,
                old_generation = old.generation,
                "[Relay] 设备重连：新连接顶替旧连接"
            );
        } else {
            tracing::info!(
                node_id = %node_id,
                name = %name,
                version = %version,
                "[Relay] 桥入设备接入"
            );
        }
        // 二期：登记成功 → 上报 online（身份 None 宿主侧自然跳过注册）。
        self.identity_sink
            .notify(super::identity::BridgeIdentityEvent {
                bridge_node_id: node_id.to_string(),
                online: true,
                cluster: identity,
            });
        // 三期批次八：设备上线 = 摘要变化 → 立即广播（不等 15s 兜底）。
        self.broadcast_member_sync();
        Ok(generation)
    }

    /// 设备读循环退出时调用：代际仍匹配才移除（被顶替的旧循环不动新表项，
    /// 也不误报 offline——二期身份事件随移除一并上报）。
    pub fn mark_device_gone(&self, node_id: &str, generation: u64) {
        let mut removed: Option<Option<BridgeClusterIdentity>> = None;
        self.devices.remove_if(node_id, |_, entry| {
            if entry.generation == generation {
                removed = Some(entry.cluster.clone());
                true
            } else {
                false
            }
        });
        if let Some(cluster) = removed {
            tracing::info!(node_id = %node_id, "[Relay] 桥入设备连接断开");
            self.notify_identity_offline(node_id, cluster);
        }
    }

    /// 设备代际是否仍然有效（读循环每帧校验；不有效 = 已被顶替，应退出）。
    pub fn generation_valid(&self, node_id: &str, generation: u64) -> bool {
        self.devices
            .get(node_id)
            .map(|e| e.generation == generation)
            .unwrap_or(false)
    }

    /// 收到设备任意帧：刷新心跳并计入流量（data_len = 帧承载的字节数，
    /// 控制帧传 0）。
    pub fn touch_device(&self, node_id: &str, data_len: u64) {
        if let Some(mut entry) = self.devices.get_mut(node_id) {
            entry.last_seen = Instant::now();
            entry.bytes_down.fetch_add(data_len, Ordering::Relaxed);
        }
    }

    /// 服务端向设备发一帧（下行流量计数在此处）。设备不在表 / 队列满 =
    /// false。
    pub fn send_to_device(&self, node_id: &str, frame: BridgeFrame, wire_len: u64) -> bool {
        let Some(entry) = self.devices.get_mut(node_id) else {
            return false;
        };
        match entry.outbound.try_send(frame) {
            Ok(()) => {
                entry.bytes_up.fetch_add(wire_len, Ordering::Relaxed);
                true
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                tracing::warn!(node_id = %node_id, "[Relay] 设备下行队列满，丢弃该帧");
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        }
    }

    /// 分配 conn_id（服务端全局原子递增）。
    pub fn alloc_conn_id(&self) -> u64 {
        self.next_conn_id.fetch_add(1, Ordering::SeqCst)
    }

    /// 登记 conn（/d/ handler 建立隧道时）。返回响应字节接收端。
    pub fn register_conn(
        &self,
        conn_id: u64,
        device: &str,
        generation: u64,
    ) -> mpsc::Receiver<Vec<u8>> {
        let (tx, rx) = mpsc::channel::<Vec<u8>>(64);
        self.conns.insert(
            conn_id,
            ConnEntry {
                device: device.to_string(),
                generation,
                resp_tx: tx,
                created_at: Instant::now(),
            },
        );
        rx
    }

    /// 设备 ConnData 路由：按 conn_id 找登记表转发响应字节。
    /// conn 不存在（已关闭/未注册）→ false（设备侧应收到 conn_close）。
    pub fn route_conn_data(&self, conn_id: u64, data: Vec<u8>) -> bool {
        let Some(entry) = self.conns.get(&conn_id) else {
            return false;
        };
        entry.resp_tx.try_send(data).is_ok()
    }

    /// 关闭并移除 conn（响应通道关闭 → 消费流终止）。
    pub fn close_conn(&self, conn_id: u64, reason: &str) {
        if let Some((_, entry)) = self.conns.remove(&conn_id) {
            tracing::debug!(conn_id, reason = %reason, "[Relay] conn 关闭");
            drop(entry);
        }
    }

    /// close_conn 便捷形态（默认 reason）。
    pub fn close_conn_id(&self, conn_id: &u64) {
        self.close_conn(*conn_id, "closed by handler");
    }

    /// 设备是否在表（在线判定快路径；心跳超时由维护循环踢表）。
    pub fn device_online(&self, node_id: &str) -> bool {
        self.devices.contains_key(node_id)
    }

    /// 发起面板访问 token 校验（服务端只转发哈希，零存储原文）。
    /// 设备离线 → Err；设备回执 → Ok(ok)；超时/断开 → Ok(false)。
    pub async fn access_check(&self, node_id: &str, hash_hex: &str) -> Result<bool, String> {
        if !self.devices.contains_key(node_id) {
            return Err("设备不在线".to_string());
        }
        let request_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        self.access_waiters.insert(request_id.clone(), tx);
        let sent = self.send_to_device(
            node_id,
            BridgeFrame::AccessCheck {
                request_id: request_id.clone(),
                node_id: node_id.to_string(),
                hash_hex: hash_hex.to_string(),
            },
            0,
        );
        if !sent {
            self.access_waiters.remove(&request_id);
            return Err("设备不在线".to_string());
        }
        match tokio::time::timeout(Duration::from_secs(ACCESS_CHECK_TIMEOUT_SECS), rx).await {
            Ok(Ok(ok)) => Ok(ok),
            Ok(Err(_)) => Ok(false), // 设备断开导致通道关闭
            Err(_) => {
                self.access_waiters.remove(&request_id);
                tracing::warn!(
                    node_id = %node_id,
                    "[Relay] access_check 超时（{ACCESS_CHECK_TIMEOUT_SECS}s 无回执）"
                );
                Ok(false)
            }
        }
    }

    /// 设备回执投递（AccessResult）。
    pub fn deliver_access_result(&self, request_id: &str, ok: bool) {
        if let Some((_, tx)) = self.access_waiters.remove(request_id) {
            let _ = tx.send(ok);
        }
    }

    /// 创建授权会话（access_check 通过后调用）。返回 cookie 值。
    pub fn create_auth_session(&self, node_id: &str) -> String {
        let cookie = uuid::Uuid::new_v4().to_string() + &uuid::Uuid::new_v4().to_string();
        self.sessions.insert(
            cookie.clone(),
            AuthSession {
                node_id: node_id.to_string(),
                created_at: unix_now(),
            },
        );
        cookie
    }

    /// 授权会话校验（cookie 属于该设备且未过期）。
    pub fn session_valid(&self, cookie: &str, node_id: &str) -> bool {
        match self.sessions.get(cookie) {
            Some(s) => {
                s.node_id == node_id
                    && unix_now().saturating_sub(s.created_at) < AUTH_SESSION_TTL_SECS
            }
            None => false,
        }
    }

    /// 列出设备快照（状态页 / relay.status API 数据源）。
    pub fn list_devices(&self) -> Vec<DeviceStatus> {
        let now = Instant::now();
        self.devices
            .iter()
            .map(|e| DeviceStatus {
                node_id: e.key().clone(),
                name: e.name.clone(),
                version: e.version.clone(),
                connected_at: e.connected_at,
                bytes_up: e.bytes_up.load(Ordering::Relaxed),
                bytes_down: e.bytes_down.load(Ordering::Relaxed),
                online: now.duration_since(e.last_seen).as_secs() < HEARTBEAT_TIMEOUT_SECS,
            })
            .collect()
    }

    /// 启动维护循环（幂等；首次 /bridge 接入时由 handler 惰性拉起）。
    /// 每 15s：90s 无心跳设备判离线踢连接 + 过期授权会话清理 + 泄漏 conn 清理。
    pub fn ensure_maintenance(self: &std::sync::Arc<Self>) {
        if self.sweeper_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let relay = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(MAINTENANCE_INTERVAL_SECS));
            loop {
                tick.tick().await;
                relay.maintenance_tick();
            }
        });
    }

    /// 维护循环的单次扫描（抽离以便测试直接驱动，不必等 15s 周期）。
    /// 关闭状态下静默跳过（开关关闭时设备表已清，无需动作）。
    pub fn maintenance_tick(&self) {
        if !self.enabled.load(Ordering::SeqCst) {
            return;
        }
        // 心跳超时设备：判离线踢连接（drop 下行 sender → 连接关闭）。
        let mut stale = Vec::new();
        for e in self.devices.iter() {
            if now_instant().duration_since(e.last_seen).as_secs() > HEARTBEAT_TIMEOUT_SECS {
                stale.push((e.key().clone(), e.generation));
            }
        }
        for (node_id, generation) in stale {
            tracing::warn!(
                node_id = %node_id,
                "[Relay] {HEARTBEAT_TIMEOUT_SECS}s 无心跳，判定设备离线，关闭桥连接"
            );
            if let Some((_, entry)) = self.devices.remove(&node_id) {
                // 发送 bridge_close 尽力而为（大概率发不动）。
                let _ = entry.outbound.try_send(BridgeFrame::BridgeClose {
                    reason: "heartbeat timeout".to_string(),
                });
                // 二期：心跳踢 = 设备离线 → 身份事件上报（与断开同语义）。
                let cluster = entry.cluster.clone();
                drop(entry);
                self.notify_identity_offline(&node_id, cluster);
            }
            let _ = generation;
        }
        // 过期授权会话。
        let now = unix_now();
        self.sessions
            .retain(|_, s| now.saturating_sub(s.created_at) < AUTH_SESSION_TTL_SECS);
        // 泄漏 conn（短 conn 正常秒级收口；超龄 = 消费方已死）。
        self.conns.retain(|_, c| {
            now_instant().duration_since(c.created_at).as_secs() < CONN_MAX_AGE_SECS
        });
        // 三期批次八：成员表周期兜底广播（goal：变化触发 + 15s 兜底全量）。
        self.broadcast_member_sync();
    }

    /// 【仅测试用】把设备 last_seen 拨旧指定秒数，以便不等待真实时间
    /// 直接驱动 maintenance_tick 验证心跳超时踢出路径。
    pub fn backdate_device_last_seen_for_test(&self, node_id: &str, by_secs: u64) {
        if let Some(mut entry) = self.devices.get_mut(node_id) {
            entry.last_seen = Instant::now()
                .checked_sub(Duration::from_secs(by_secs))
                .unwrap_or_else(Instant::now);
        }
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn now_instant() -> Instant {
    Instant::now()
}

/// /bridge handler 的帧分发辅助：控制帧统一处理（心跳回 pong、access 回执
/// 投递、预留帧忽略 + WARN）。返回是否为数据面帧（由 handler 自行路由）。
pub fn handle_control_frame(frame: BridgeFrame, relay: &RelayServer, node_id: &str) {
    match frame {
        BridgeFrame::Heartbeat => {
            relay.touch_device(node_id, 0);
            let _ = relay.send_to_device(node_id, BridgeFrame::Pong, 0);
        }
        BridgeFrame::AccessResult { request_id, ok } => {
            relay.deliver_access_result(&request_id, ok);
        }
        BridgeFrame::ClusterRpc { .. } | BridgeFrame::MemberSync { .. } => {
            tracing::warn!(
                node_id = %node_id,
                "[Relay] 收到二三期预留帧（一期不支持），已忽略"
            );
        }
        // 数据面帧（ConnOpen/ConnData/ConnClose）与握手帧不进控制分发，
        // 由 /bridge handler 的读循环先行路由；此处统一吞掉（防御）。
        _ => {}
    }
}

/// 解码一帧文本（供 handler 使用；解码失败记 WARN）。
pub fn decode_or_warn(raw: &str) -> Option<BridgeFrame> {
    match decode_frame(raw) {
        Ok(f) => Some(f),
        Err(e) => {
            tracing::warn!("[Relay] 无法解码桥帧：{e}（raw 前 120 字节截断）");
            None
        }
    }
}

/// 编码帧为文本（供 handler 使用；序列化失败记 ERROR 返回 None）。
pub fn encode_or_none(frame: &BridgeFrame) -> Option<String> {
    match encode_frame(frame) {
        Ok(s) => Some(s),
        Err(e) => {
            tracing::error!("[Relay] 无法编码桥帧：{e}");
            None
        }
    }
}

// AGT 覆盖率批次（2026-09-25）：cluster_frame_sink 槽、authenticate 三前置
// 臂（门未配置 / 开关关闭 / 空 name 回落 node_id）、send_to_device 队列满/
// 通道关闭、access_check 投递失败 + 5s 超时、maintenance 超龄 conn 清理、
// 控制面数据帧防御吞掉 + decode 失败臂。声明为 server 子模块以便直读私有
// conns 表做时间拨旧（同 backdate 家族语义）。豁免见 server_agt_tests 头注。
#[cfg(test)]
mod server_agt_tests;
