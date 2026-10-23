//! rendezvous 成员名册——共享类型与客户端半边（QUIC 中继自动发现，2026-10-22）。
//!
//! 中继从「纯传输」升级为「传输 + 名册」双面：设备接入中继即周期报到
//! （register），并周期拉取同 token 名册下的其他设备（members）——名册成员
//! 经 `handle_discovered_node_ex` 单一漏斗入册（`via-relay` 哨兵 tag），之后
//! 既有链路接管（健康探针启动冲刺 → iroh 拨号 → probe carried identity →
//! announce 传播 → 全联通）。零额外配置：客户端地址由 relay_url host 推导。
//!
//! **token 即命名空间**：同名册 = 同 token = 同一集群的设备才互相可见；
//! 名册只供地址——入网连接仍要过传输身份（QUIC-TLS）+ 集群 RPC token
//! （AEAD）+ trust.toml 信任簿三关，语义与既有路径完全同源。
//!
//! 分层：类型 / 客户端（sync_once / resolve_registry_url）随 `cluster-roaming`
//! 进编译（设备侧只需要这半边）；axum 服务端在 [`super::rendezvous_server`]
//! （随 `quic-relay`，中继机侧）。

use serde::{Deserialize, Serialize};

/// 注册表默认端口（真相源在 [`crate::roaming::config`]，与 relay 端口常量
/// 同居一处；此处 re-export 供客户端/观测面按名取用）。
pub use crate::roaming::config::DEFAULT_REGISTRY_PORT;

/// 经名册入册的成员携带的哨兵 tag（镜像 via-bridge 先例——来源可辨、
/// UI/日志可解释；同名册成员与静态 peer 汇合时走既有 upsert_if_changed）。
pub const VIA_RELAY_TAG: &str = "via-relay";

/// 成员 TTL：超过此时长未报到即从名册剔除（心跳 = 周期 register，客户端
/// tick 30s = 4 个心跳周期容错）。
pub const MEMBER_TTL_SECS: u64 = 120;

/// 名册容量闸（超出拒绝注册——中继是公网面，无界存储不设防）。
pub const MAX_MEMBERS: usize = 256;

/// 名册成员（注册请求体元素 / members 响应元素同构）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RendezvousMember {
    /// 集群节点 id（对端 peers.toml 段键语义）。
    pub node_id: String,
    /// 节点显示名。
    #[serde(default)]
    pub name: String,
    /// 角色（worker/coordinator/...）。
    #[serde(default)]
    pub role: String,
    /// 分类（development/...）。
    #[serde(default)]
    pub category: String,
    /// 节点类型（Cluster::node_type，默认 "agent"）。
    #[serde(default)]
    pub node_type: String,
    /// 集群 RPC 端口（入册闸要求 >0——`handle_discovered_node_impl` 的 G14
    /// 语义：无 RPC 端口的记录造不出可用邻居）。
    #[serde(default)]
    pub rpc_port: u16,
    /// 标签（派发匹配用）。
    #[serde(default)]
    pub tags: Vec<String>,
    /// 专业职能（slug 形态 `family[:spec]`）。
    #[serde(default)]
    pub professions: Vec<String>,
    /// 能力档位（mini/normal/big）。
    #[serde(default)]
    pub tier: Option<String>,
    /// iroh 漫游身份（64 位小写 hex；注册必填且校验）。
    pub iroh_id: String,
    /// 构建号（git 短 hash）。
    #[serde(default)]
    pub build: String,
    /// 该成员的对外 relay_url（对端拼 `iroh:<hex>@<relay_url>` 用；空 =
    /// 拉取方以自己的 relay_url 兜底）。
    #[serde(default)]
    pub relay_url: String,
    /// 功能宣告（严格不兼容语义，2026-10-23 拍板：消费方按需过滤——集群
    /// 同步循环只收 `features` 含 "cluster" 的成员；空 = 消费侧跳过，不猜）。
    /// 服务端登记容忍：serde default 空 vec，缺字段不拒注册。
    #[serde(default)]
    pub features: Vec<String>,
    /// 服务器侧最近报到时刻（epoch ms；注册请求里忽略，响应里由服务端填）。
    #[serde(default)]
    pub last_seen_ms: u64,
}

/// 客户端同步状态快照（观测面：`cluster.roaming.status` 的 rendezvous 段）。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RendezvousClientState {
    pub registry_url: String,
    pub last_sync_ok: bool,
    /// 最近一次同步尝试时刻（epoch ms；0 = 尚未尝试）。
    pub last_sync_ms: u64,
    /// 上次成功同步学到的成员数（含自己）。
    pub member_count: usize,
    pub last_error: String,
}

// ---------------------------------------------------------------------------
// 客户端地址推导
// ---------------------------------------------------------------------------

/// 注册表地址推导（单一真相源）：显式 `rendezvous_url` 优先；空 = relay_url
/// 同 scheme 同 host、端口 [`DEFAULT_REGISTRY_PORT`]。
pub fn resolve_registry_url(rendezvous_url: &str, relay_url: &str) -> Result<String, String> {
    let explicit = rendezvous_url.trim();
    if !explicit.is_empty() {
        return Ok(explicit.to_string());
    }
    let relay = relay_url.trim();
    let (scheme, rest) = relay
        .split_once("://")
        .ok_or_else(|| format!("relay_url 缺 scheme（应为 http(s)://host:port）: {relay:?}"))?;
    if scheme != "http" && scheme != "https" {
        return Err(format!("relay_url scheme 非法（仅 http/https）: {relay:?}"));
    }
    let host_end = rest.find('/').unwrap_or(rest.len());
    let hostport = &rest[..host_end];
    let host = split_host(hostport);
    Ok(format!("{scheme}://{host}:{DEFAULT_REGISTRY_PORT}"))
}

/// host:port → host（支持 IPv6 字面量 `[::1]:3340` 形态）。
fn split_host(hostport: &str) -> &str {
    if let Some(rest) = hostport.strip_prefix('[')
        && let Some(end) = rest.find(']')
    {
        // end 是 ']' 在 rest（去头 '[' 后）中的下标；hostport 含头 '['，
        // 闭区间收在 ']' 处 = end + 2。
        return &hostport[..end + 2];
    }
    match hostport.rsplit_once(':') {
        Some((h, _)) if !h.is_empty() => h,
        _ => hostport, // 无端口形态
    }
}

// ---------------------------------------------------------------------------
// 客户端同步
// ---------------------------------------------------------------------------

/// 单次同步：register 自己 + 拉全量成员（两请求一次往返语义）。
///
/// 返回**含自己**的成员列表（调用方自过滤——保留全量便于观测 member_count）。
pub async fn sync_once(
    client: &reqwest::Client,
    registry_url: &str,
    token: &str,
    self_member: &RendezvousMember,
) -> Result<Vec<RendezvousMember>, String> {
    let base = registry_url.trim().trim_end_matches('/');
    let url = format!("{base}/v1/roaming/register");
    let resp = client
        .post(url)
        .bearer_auth(token)
        .json(self_member)
        .send()
        .await
        .map_err(|e| format!("register 失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("register 被拒（HTTP {}）", resp.status().as_u16()));
    }
    let url = format!("{base}/v1/roaming/members");
    let resp = client
        .get(url)
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| format!("拉取成员失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("拉取成员被拒（HTTP {}）", resp.status().as_u16()));
    }
    #[derive(Deserialize)]
    struct MembersResp {
        members: Vec<RendezvousMember>,
    }
    let body: MembersResp = resp
        .json()
        .await
        .map_err(|e| format!("成员响应解析失败: {e}"))?;
    Ok(body.members)
}

/// 名册成员 → `handle_discovered_node_ex` 入参的地址形态
/// （`iroh:<hex>@<relay_url>`；成员未自报 relay_url 时以本方 relay_url 兜底
/// ——同一中继部署下两者一致）。
pub fn member_dial_address(m: &RendezvousMember, fallback_relay_url: &str) -> String {
    let relay = if m.relay_url.is_empty() {
        fallback_relay_url
    } else {
        &m.relay_url
    };
    format!("iroh:{}@{}", m.iroh_id, relay)
}

#[cfg(test)]
mod tests;
