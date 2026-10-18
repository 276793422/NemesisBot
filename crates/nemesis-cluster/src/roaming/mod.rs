//! A1（A级差距修正计划 2026-10-18）— iroh 跨网 P2P 叠加层。
//!
//! 传输身份自证（ed25519 密钥对，QUIC-TLS 握手即验身）+ 中继打洞（NAT 后
//! 零端口转发互连）。LAN 场景现有 TCP 路径字节不变——iroh 是叠加层不是替换：
//! 地址语法新增 `iroh:<node_id_hex>` 前缀形态，dial 按前缀分派；AEAD token
//! 语义保留（传输加密之外的应用授权层，字节兼容）。
//!
//! - [`endpoint`]：节点密钥（roaming.key）+ iroh Endpoint 装配 + ping/echo 通路；
//! - [`trust`]：TrustBook 本地 allowlist（trust.toml，可选第三层信任）；
//! - [`bridge`]：iroh bi-stream ↔ 现有帧协议（framing + AEAD）桥接。
//!
//! feature：`cluster-roaming`（默认关——LAN 用户零新依赖）。

pub mod bridge;
pub mod endpoint;
pub mod trust;
