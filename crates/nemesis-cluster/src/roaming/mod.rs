//! A1（A级差距修正计划 2026-10-18）— iroh 跨网 P2P 叠加层。
//!
//! 传输身份自证（ed25519 密钥对，QUIC-TLS 握手即验身）+ 中继打洞（NAT 后
//! 零端口转发互连）。LAN 场景现有 TCP 路径字节不变——iroh 是叠加层不是替换：
//! 地址语法新增 `iroh:<node_id_hex>` 前缀形态，dial 按前缀分派；AEAD token
//! 语义保留（传输加密之外的应用授权层，字节兼容）。
//!
//! - [`endpoint`]：节点密钥（roaming.key）+ iroh Endpoint 装配 + ping/echo 通路；
//! - [`config`]：config.cluster.json `roaming` 段解析（单一真相源纯函数）；
//! - [`quic_config`]：config.quic.json 解析（QUIC 自身全部配置：服务端
//!   `relay` 段 + 客户端 `client` 段；旧位置首读迁移，2026-10-23 解耦）；
//! - [`trust`]：TrustBook 本地 allowlist（trust.toml，可选第三层信任）；
//! - [`bridge`]：iroh bi-stream ↔ 现有帧协议（framing + AEAD）桥接。
//! - [`relay_server`]：自建 iroh 中继回落服务器（QUIC 内网穿透；quic-relay）。
//! - [`rendezvous`] / [`rendezvous_server`]：成员名册（中继自动发现，
//!   2026-10-22）——设备接入中继即报到 + 周期拉名册，零手工配置互见；
//!   客户端半边随 `cluster-roaming`，axum 服务端随 `quic-relay`。
//!
//! feature：`cluster-roaming`（默认关——LAN 用户零新依赖）；`quic-relay`
//! （隐含 cluster-roaming，中继服务器面：iroh-relay/server + rcgen + rustls + axum）。

pub mod bridge;
pub mod config;
pub mod endpoint;
pub mod quic_config;
/// 中继生命周期管理（2026-10-23 与集群解耦——宿主直接持有，集群关闭时
/// 中继照起）。依赖 relay_server/quic_config，随 `quic-relay` 编译。
#[cfg(feature = "quic-relay")]
pub mod relay_manager;
#[cfg(feature = "quic-relay")]
pub mod relay_server;
#[cfg(feature = "cluster-roaming")]
pub mod rendezvous;
#[cfg(feature = "quic-relay")]
pub mod rendezvous_server;
pub mod trust;
