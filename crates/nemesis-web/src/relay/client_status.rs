//! 桥客户端状态槽（goal 批次三：通道页【中继通道】数据源）。
//!
//! 桥客户端跑在 nemesisbot（bin crate），而通道页的数据出口在 nemesis-web
//! ——nemesisbot 依赖 nemesis-web，所以状态用「模块级全局槽」回传：客户端
//! 主循环在状态迁移点调 [`report_client_status`]，HTTP 端点
//! `GET /api/relay/overview` 调 [`client_status`] 读取。手动重连按钮调
//! [`kick_reconnect`]，客户端主循环在退避等待与会话主 select 两处监听
//! [`reconnect_notify`]，被踢后跳过退避立即重连。
//!
//! **运行时态，不落盘**：槽内全部是进程内状态，重启即随进程消失（与
//! config.json 的 bridge.client 持久配置严格分离）。未 spawn 客户端时
//! 槽为 None，端点诚实回 null（前端显示「未启用」）。

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Notify;

/// 客户端连接状态（`as_str` 直接作为 JSON 值下发给前端）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeClientState {
    /// 客户端已启用但尚未发起连接（spawn 后的第一态，极短暂）。
    Connecting,
    /// welcome 握手完成，桥服务中。
    Connected,
    /// 服务端拒绝（ws token 不匹配）——退避重试中但每次都会再被拒。
    Rejected,
    /// 掉线 / 握手未完成即断开，退避重连中。
    Disconnected,
}

impl BridgeClientState {
    pub fn as_str(&self) -> &'static str {
        match self {
            BridgeClientState::Connecting => "connecting",
            BridgeClientState::Connected => "connected",
            BridgeClientState::Rejected => "rejected",
            BridgeClientState::Disconnected => "disconnected",
        }
    }
}

/// 客户端状态快照（一次整体替换写入，读侧拿整份克隆）。
#[derive(Debug, Clone)]
pub struct BridgeClientStatus {
    /// 客户端总开关（gateway 装配时的 cfg.bridge.client.enabled）。
    pub enabled: bool,
    pub state: BridgeClientState,
    /// 中继地址（配置原样回显，前端展示用）。
    pub relay_url: String,
    /// 本机对外暴露的桥 node_id。
    pub node_id: String,
    /// 最近一次错误（连接失败 / 被拒 / 掉线原因；None = 无错误）。
    pub last_error: Option<String>,
    /// 状态更新时刻（unix 秒；前端展示「最后更新」用）。
    pub updated_at: u64,
}

/// 模块级状态槽（`pub(crate)` 供测试清理/预置）。`Mutex::new(None)` 是
/// const——零 OnceLock 样板。
pub(crate) static CLIENT_STATUS: Mutex<Option<BridgeClientStatus>> = Mutex::new(None);

/// 手动重连踢信号（此 tokio 版本 `Notify::new()` 非 const，用 OnceLock）。
/// 客户端主循环与会话主 select 各挂一个 `notified()` 分支。
static RECONNECT_KICK: std::sync::OnceLock<Notify> = std::sync::OnceLock::new();

/// 上报一次状态快照（整体替换）。bridge_client 主循环的状态迁移点调用。
pub fn report_client_status(status: BridgeClientStatus) {
    if let Ok(mut slot) = CLIENT_STATUS.lock() {
        *slot = Some(status);
    }
}

/// 便捷上报：以最近一次快照为底，只改 state / last_error（保留
/// relay_url / node_id / enabled 展示连续性）。槽为空时忽略——
/// 说明客户端尚未做过首次上报，无底可改。
pub fn report_client_state(state: BridgeClientState, last_error: Option<String>) {
    let Ok(mut slot) = CLIENT_STATUS.lock() else {
        return;
    };
    let Some(cur) = slot.as_mut() else {
        return;
    };
    cur.state = state;
    cur.last_error = last_error;
    cur.updated_at = unix_now();
}

/// 读取当前快照（无客户端运行时 None）。
pub fn client_status() -> Option<BridgeClientStatus> {
    CLIENT_STATUS.lock().ok().and_then(|s| s.clone())
}

/// 手动重连：唤醒客户端（退避等待被跳过 / 当前会话被断开立即重连）。
/// 客户端未运行时调用无害——permit 留在 Notify 里，下次 spawn 后的
/// 第一个 `notified()` 立即消费（语义仍正确：尽快连接）。
pub fn kick_reconnect() {
    reconnect_notify().notify_one();
}

/// 客户端侧监听句柄（主循环 / 会话 select 挂 `.notified()`）。
pub fn reconnect_notify() -> &'static Notify {
    RECONNECT_KICK.get_or_init(Notify::new)
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
