//! A1（A级差距修正计划 2026-10-18）— iroh 跨网 P2P 叠加层（QUIC 插件化
//! M3 后 = 桥消费面 + 数据形态）。
//!
//! 传输身份自证（ed25519 密钥对，QUIC-TLS 握手即验身）+ 中继打洞（NAT 后
//! 零端口转发互连）。LAN 场景现有 TCP 路径字节不变——iroh 是叠加层不是替换：
//! 地址语法新增 `iroh:<node_id_hex>` 前缀形态，dial 按前缀分派；AEAD token
//! 语义保留（传输加密之外的应用授权层，字节兼容）。
//!
//! **QUIC 插件化 M3（2026-10-24）**：iroh 端点本体（QUIC 栈/身份密钥读写/
//! accept trust 闸/拨号路径观测）整体在 plugin_roaming C-ABI 插件内——宿主
//! 二进制零 iroh 依赖。本模块只剩：
//!
//! - [`plugin_bridge`]：C-ABI 桥消费面——身份/名册控制面（M2）+ 连接面
//!   （M3：dial/accept loopback 交接/端点启停/路径观测快照）。dlopen 细节
//!   在 nemesisbot loader；
//! - [`bridge`]：帧协议组件 [`bridge::RoamingConn`]（宿主侧帧收发复用，
//!   载体已换 loopback TCP）+ `iroh:` 地址形态判定 re-export；
//! - [`config`]：config.cluster.json `roaming` 段解析（单一真相源纯函数）；
//! - [`quic_config`]：config.quic.json 解析（QUIC 自身全部配置：服务端
//!   `relay` 段 + 客户端 `client` 段；旧位置首读迁移，2026-10-23 解耦）；
//! - [`trust`]：TrustBook 本地 allowlist（trust.toml——**裁决在插件 accept
//!   闸**，宿主只留数据形态供 CLI/WSAPI 只读显示）；
//! - [`relay_info`]：中继显示面（证书路径/对外 URL 纯函数）；
//! - [`rendezvous`]：成员名册 wire 类型 + registry_url 解析/拨号地址拼装
//!   （同步循环在 plugin_roaming——宿主只留数据形态）。
//!
//! feature：M4（2026-10-24）起 cluster-roaming/quic-relay feature 整体删除
//!（拍板②——少一个 feature 少一分组合矩阵）：本模块恒编译，iroh 家族重
//! 依赖随迁 plugin_roaming C-ABI 插件（宿主二进制零 iroh-relay/rcgen/axum
//! 增量），插件在场与否是唯一功能开关（缺席 = 诚实降级）。

pub mod bridge;
pub mod config;
/// C-ABI 桥（QUIC 插件化 M2 控制面 + M3 连接面）：插件句柄槽 +
/// spawn_blocking 调用 + 三态裁决 + 成员回流漏斗 + dial/accept 交接与
/// 端点启停。依赖 `nemesis-plugin`（optional，随本 feature）。
pub mod plugin_bridge;
pub mod quic_config;
/// 中继显示面（证书路径/对外 URL 纯函数；零 iroh 依赖——QUIC 插件化
/// M1 后宿主侧仅存的 relay 相关代码，CLI/WSAPI 显示消费）。
pub mod relay_info;
pub mod rendezvous;
pub mod trust;

/// 节点密钥文件路径（`<workspace>/cluster/roaming.key`）。
///
/// QUIC 插件化 M3：key 的读写在 plugin_roaming（插件是唯一读写者，
/// seed 恒不出插件），宿主只剩路径展示面（CLI/WSAPI 显示消费）。
pub fn roaming_key_path(workspace: &std::path::Path) -> std::path::PathBuf {
    workspace.join("cluster").join("roaming.key")
}

/// 64 字符 hex → 32 字节 node_id（大小写不敏感；长度/字符/on-curve 非法
/// loud 报错）。
///
/// QUIC 插件化 M3 宿主侧重实现：iroh `EndpointId` 已随端点迁入插件，
/// announce 入池前置校验换 ed25519-dalek 同语义底座——
/// `VerifyingKey::from_bytes` 的 on-curve 校验与 `EndpointId::from_bytes`
/// 等价（拒绝非曲线点/非规范编码）。
pub fn node_id_from_hex(s: &str) -> Result<[u8; 32], String> {
    let bytes = hex::decode(s.trim())
        .map_err(|e| format!("node_id hex 非法（应为 64 位十六进制）: {e}"))?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|v: Vec<u8>| format!("node_id 长度 {} 字节 ≠ 32", v.len()))?;
    ed25519_dalek::VerifyingKey::from_bytes(&arr)
        .map(|_| arr)
        .map_err(|e| format!("node_id 非法: {e}"))
}

/// 测试辅助：真实 ed25519 密钥派生 node_id hex（`valid_iroh_id` 走
/// on-curve 公钥校验，任意 hex 会被诚实拒绝——测试身份必须真派生）。
/// 递增计数器 seed：多次调用互异（同测试双身份不碰撞）。
#[cfg(test)]
pub(crate) fn random_valid_iroh_hex() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(1);
    let mut seed = [0u8; 32];
    seed[..8].copy_from_slice(&SEQ.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    let key = ed25519_dalek::SigningKey::from_bytes(&seed);
    hex::encode(key.verifying_key().as_bytes())
}

#[cfg(test)]
mod tests;
