//! Discovery message types.
//!
//! Defines the Announce/Bye message format used for UDP multicast discovery.
//! Matches the Go discovery.MessageType / DiscoveryMessage wire format exactly.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

/// Discovery protocol version. Must match the Go constant `ProtocolVersion`.
pub const PROTOCOL_VERSION: &str = "1.0";

/// Default expiry threshold in seconds. Messages older than this are considered stale.
///
/// The 120-second threshold is intentionally chosen for LAN scenarios
/// (RTT <1ms, clock skew <1s). It can be overridden per deployment via
/// `config.cluster.json` 的 `announce_expiry_secs`（G3，≤0 = 关闭过期丢弃）；
/// 此常量是该配置的默认值与协议层 fallback。
pub const DEFAULT_EXPIRY_THRESHOLD_SECS: i64 = 120;

// ---------------------------------------------------------------------------
// MessageType
// ---------------------------------------------------------------------------

/// Discovery message type, mirroring Go's `MessageType` string enum.
///
/// Serialized as lowercase strings (`"announce"`, `"bye"`) to remain
/// wire-compatible with the Go implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DiscoveryMessageType {
    /// A node announcing its presence on the network.
    #[serde(rename = "announce")]
    Announce,
    /// A node announcing its departure from the network.
    #[serde(rename = "bye")]
    Bye,
}

impl DiscoveryMessageType {
    /// Returns the wire string for this variant.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Announce => "announce",
            Self::Bye => "bye",
        }
    }
}

impl fmt::Display for DiscoveryMessageType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// DiscoveryMessage
// ---------------------------------------------------------------------------

/// A discovery message broadcast over UDP multicast.
///
/// Wire format (JSON) matches Go's `DiscoveryMessage` struct exactly so that
/// Rust and Go nodes can coexist on the same multicast group.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiscoveryMessage {
    /// Protocol version (must be `"1.0"`).
    pub version: String,
    /// Message type.
    #[serde(rename = "type")]
    pub msg_type: DiscoveryMessageType,
    /// Unique node identifier.
    pub node_id: String,
    /// Human-readable node name.
    #[serde(default)]
    pub name: String,
    /// List of IP addresses (multiple NICs support).
    #[serde(default)]
    pub addresses: Vec<String>,
    /// RPC port number.
    #[serde(default)]
    pub rpc_port: u16,
    /// Cluster role: manager, coordinator, worker, observer, standby.
    #[serde(default)]
    pub role: String,
    /// Business category: design, development, testing, etc.
    #[serde(default)]
    pub category: String,
    /// Custom tags.
    #[serde(default)]
    pub tags: Vec<String>,
    /// List of capabilities (e.g. "llm", "tools").
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Node type: "agent" (full with LLM) or "node" (lightweight, no LLM).
    #[serde(default)]
    pub node_type: String,
    /// 自报职能清单（集群专业职能框架 M2；`family[:spec]` slug）。旧节点 /
    /// Go 节点的 announce 无此字段 → serde default 空 = 未宣告（wire 兼容）。
    #[serde(default)]
    pub professions: Vec<String>,
    /// 节点自报档位（role_tier 口径：mini/normal/big；None = 未宣告）。
    #[serde(default)]
    pub tier: Option<String>,
    /// 自报 agent 实体清单（P2 2026-10-16）。旧节点 / Go 节点的 announce
    /// 无此字段 → serde default 空 = 未宣告（wire 兼容，同 professions 先例）。
    #[serde(default)]
    pub agents: Vec<crate::types::AgentAnnouncement>,
    /// 自报 iroh 漫游身份 hex（A1 跨网 P2P 叠加层；对端据此拼 `iroh:<hex>`
    /// 拨号候选入池）。旧节点 / Go 节点的 announce 无此字段 → serde
    /// default `None` = 未宣告（wire 兼容，同 professions 先例）。
    #[serde(default)]
    pub iroh_id: Option<String>,
    /// 自报构建号（git 短 hash；运行时硬化批 A1 2026-10-19）。旧节点 /
    /// Go 节点的 announce 无此字段 → serde default 空 = 未宣告（wire
    /// 兼容，同 professions 先例）。三机换装偏斜可见性：注册表透传后
    /// Dashboard 节点面直接展示对端构建号，人工 sha 比对退役。
    #[serde(default)]
    pub build: String,
    /// Unix timestamp (seconds since epoch).
    pub timestamp: i64,
}

// ---------------------------------------------------------------------------
// Constructors
// ---------------------------------------------------------------------------

impl DiscoveryMessage {
    /// Create a new announce message, mirroring Go's `NewAnnounceMessage`.
    pub fn new_announce(
        node_id: impl Into<String>,
        name: impl Into<String>,
        addresses: Vec<String>,
        rpc_port: u16,
        role: impl Into<String>,
        category: impl Into<String>,
        tags: Vec<String>,
        capabilities: Vec<String>,
        node_type: impl Into<String>,
    ) -> Self {
        Self {
            version: PROTOCOL_VERSION.to_string(),
            msg_type: DiscoveryMessageType::Announce,
            node_id: node_id.into(),
            name: name.into(),
            addresses,
            rpc_port,
            role: role.into(),
            category: category.into(),
            tags,
            capabilities,
            node_type: node_type.into(),
            professions: Vec::new(),
            tier: None,
            agents: Vec::new(),
            iroh_id: None,
            build: String::new(),
            timestamp: now_unix(),
        }
    }

    /// 附自报职能与档位（profession framework M2；生产 announce 构造点
    /// 链式调用。保持 [`Self::new_announce`] 既有签名不变——避免波及
    /// 数十个既有测试构造点）。
    pub fn with_professions(mut self, professions: Vec<String>, tier: Option<String>) -> Self {
        self.professions = professions;
        self.tier = tier;
        self
    }

    /// 附自报 agent 实体清单（P2 2026-10-16；生产 announce 构造点
    /// `build_announce_from_callbacks` 链式调用——UDP 广播与 ping-probe
    /// TCP 回退共用该单一真相源，两通道字段集必须一致）。
    pub fn with_agents(mut self, agents: Vec<crate::types::AgentAnnouncement>) -> Self {
        self.agents = agents;
        self
    }

    /// 附自报 iroh 漫游身份 hex（A1；`None` = 未启用漫游叠加层，不带字段
    /// 语义。生产 announce 构造点 `build_announce_from_callbacks` 链式
    /// 调用——与 agents 同一单一真相源纪律）。
    pub fn with_iroh_id(mut self, iroh_id: Option<String>) -> Self {
        self.iroh_id = iroh_id;
        self
    }

    /// 附自报构建号（A1 运行时硬化批；生产 announce 构造点
    /// `build_announce_from_callbacks` 链式调用——与 agents 同一单一真相
    /// 源纪律）。空串 = 未宣告，wire 上省略语义。
    pub fn with_build(mut self, build: String) -> Self {
        self.build = build;
        self
    }

    /// Create a new bye message, mirroring Go's `NewByeMessage`.
    pub fn new_bye(node_id: impl Into<String>) -> Self {
        Self {
            version: PROTOCOL_VERSION.to_string(),
            msg_type: DiscoveryMessageType::Bye,
            node_id: node_id.into(),
            name: String::new(),
            addresses: Vec::new(),
            rpc_port: 0,
            role: String::new(),
            category: String::new(),
            tags: Vec::new(),
            capabilities: Vec::new(),
            node_type: String::new(),
            professions: Vec::new(),
            tier: None,
            agents: Vec::new(),
            iroh_id: None,
            build: String::new(),
            timestamp: now_unix(),
        }
    }
}

// ---------------------------------------------------------------------------
// Validation / expiry / serialization
// ---------------------------------------------------------------------------

impl DiscoveryMessage {
    /// Validate the message, mirroring Go's `DiscoveryMessage.Validate()`.
    ///
    /// Checks:
    /// - `version` must be `"1.0"`
    /// - `node_id` must not be empty
    /// - For `Announce` messages: `name`, `addresses` (non-empty), and `rpc_port` (>0)
    ///   are required.
    pub fn validate(&self) -> Result<(), MessageValidationError> {
        if self.version != PROTOCOL_VERSION {
            return Err(MessageValidationError::UnsupportedVersion {
                version: self.version.clone(),
            });
        }

        if self.node_id.is_empty() {
            return Err(MessageValidationError::MissingNodeId);
        }

        if self.msg_type == DiscoveryMessageType::Announce {
            if self.name.is_empty() {
                return Err(MessageValidationError::MissingName);
            }
            if self.addresses.is_empty() {
                return Err(MessageValidationError::MissingAddresses);
            }
            if self.rpc_port == 0 {
                return Err(MessageValidationError::MissingRpcPort);
            }
        }

        Ok(())
    }

    /// Check whether the message has expired (timestamp older than 120 seconds).
    ///
    /// Mirrors Go's `DiscoveryMessage.IsExpired()`.
    pub fn is_expired(&self) -> bool {
        self.is_expired_with_threshold(DEFAULT_EXPIRY_THRESHOLD_SECS)
    }

    /// G3: 用可配置阈值判断消息是否过期。
    ///
    /// `threshold <= 0` 视为关闭过期丢弃（恒返回 false），与
    /// `announce_expiry_secs` 配置语义一致。
    pub fn is_expired_with_threshold(&self, threshold: i64) -> bool {
        if threshold <= 0 {
            return false;
        }
        self.age_secs() > threshold
    }

    /// G3: 消息年龄（当前 Unix 时间 − 消息时间戳，秒）。时钟回拨时可为负。
    pub fn age_secs(&self) -> i64 {
        now_unix() - self.timestamp
    }

    /// Serialize the message to JSON bytes, mirroring Go's `DiscoveryMessage.Bytes()`.
    pub fn to_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    /// Deserialize a discovery message from JSON bytes.
    pub fn from_bytes(data: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(data)
    }
}

// ---------------------------------------------------------------------------
// Display
// ---------------------------------------------------------------------------

impl fmt::Display for DiscoveryMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "DiscoveryMessage{{type={}, node_id={}, name={}, addresses={:?}, rpc_port={}, \
             role={}, category={}, tags={:?}, caps={:?}}}",
            self.msg_type,
            self.node_id,
            self.name,
            self.addresses,
            self.rpc_port,
            self.role,
            self.category,
            self.tags,
            self.capabilities,
        )
    }
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors returned by [`DiscoveryMessage::validate()`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MessageValidationError {
    #[error("unsupported protocol version: {version}")]
    UnsupportedVersion { version: String },
    #[error("node_id is required")]
    MissingNodeId,
    #[error("name is required for announce")]
    MissingName,
    #[error("addresses is required for announce")]
    MissingAddresses,
    #[error("rpc_port is required for announce")]
    MissingRpcPort,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Return the current Unix timestamp in seconds.
fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests;
