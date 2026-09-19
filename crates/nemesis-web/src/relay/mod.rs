//! 反向桥中继模块（goal：节点显示名 + 反向桥与多设备汇聚，一期）。
//!
//! - [`protocol`]：桥帧协议（JSON 文本帧；二三期预留 `cluster_rpc`/
//!   `member_sync` 类型一次定对）
//! - [`server`]：中继服务端状态机（设备表 / 授权会话 / conn 路由 / 心跳
//!   判定 / 运行时开关）
//! - [`handlers`]：HTTP/WS handlers（`/bridge` 接入、`/d/<node_id>/` 隧道、
//!   `__auth` 输入页、`/relay` 状态页、`/api/relay/status`）
//! - [`ws_codec`]：WS 长泵的帧格式转换层（axum Message ↔ 原始 ws 帧字节）
//! - [`subpath`]：设备侧子路径支持（`/d/<自身 node_id>/` 前缀剥离 +
//!   HTML `<base href>` 注入；批次二）
//! - [`client_status`]：桥客户端状态槽（批次三：通道页【中继通道】
//!   客户端连接状态 + 手动重连踢信号；nemesisbot 桥客户端经此回传）
//!
//! **位置注记（对 goal 的偏离，已裁决记录）**：goal 原文把协议/服务端放
//! `nemesisbot/src/relay/`，但 nemesis-web 不能依赖 bin crate，而路由/
//! handler/状态页归属 nemesis-web——模块整体放这里，桥客户端（批次二，
//! nemesisbot）复用 [`protocol`]。

pub mod client_status;
pub mod handlers;
pub mod protocol;
pub mod server;
pub mod subpath;
pub mod ws_codec;

pub use client_status::{
    BridgeClientState, BridgeClientStatus, client_status, kick_reconnect, reconnect_notify,
    report_client_state, report_client_status,
};

pub use handlers::{
    ADMIN_COOKIE, AUTH_COOKIE, handle_auth_page, handle_auth_submit, handle_bridge_ws,
    handle_device_request, handle_relay_api_client_reconnect, handle_relay_api_enabled,
    handle_relay_api_overview, handle_relay_api_status, handle_relay_login,
    handle_relay_status_page,
};
pub use protocol::BridgeFrame;
pub use server::{DeviceStatus, RelayServer};
pub use subpath::BridgeSubpathService;

#[cfg(test)]
mod client_status_tests;
#[cfg(test)]
mod http_tests;
#[cfg(test)]
mod subpath_tests;
#[cfg(test)]
mod tests;
