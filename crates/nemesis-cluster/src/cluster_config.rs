//! Cluster TOML configuration types.
//!
//! Defines `StaticConfig` (peers.toml) and `DynamicState` (state.toml) along
//! with their load/save functions. Uses atomic write (write-to-tmp + rename)
//! to prevent corruption.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config_loader::ConfigError;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Static cluster configuration (peers.toml).
///
/// Represents the `[node]` section of peers.toml. The `[peers.X]` subtables
/// are managed by `append_peer_to_file()` (the canonical write path) and
/// read directly by gateway.rs as raw TOML — they are NOT represented here,
/// because `Vec<PeerConfig>` serializes to `[[peers]]` (array-of-tables)
/// which is incompatible with the `[peers.X]` subtable form.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StaticConfig {
    #[serde(default)]
    pub node: NodeInfo,
}

/// Node information in the config file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub address: String,
    #[serde(default = "default_role")]
    pub role: String,
    #[serde(default = "default_category")]
    pub category: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

impl Default for NodeInfo {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            address: String::new(),
            role: default_role(),
            category: default_category(),
            tags: Vec::new(),
        }
    }
}

fn default_role() -> String {
    "worker".into()
}

fn default_category() -> String {
    "general".into()
}

/// Peer node configuration.
///
/// Note: `tags` and `capabilities` fields have been removed. The runtime
/// capabilities of remote nodes come from UDP discovery broadcasts (set by
/// each node's `cluster.set_capabilities(tool_names)`), NOT from this static
/// config. Configuring them in peers.toml had no effect.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerConfig {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub address: String,
    #[serde(default)]
    pub addresses: Vec<String>,
    #[serde(default)]
    pub rpc_port: u16,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default = "default_priority")]
    pub priority: u32,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub status: PeerStatus,
}

impl Default for PeerConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            address: String::new(),
            addresses: Vec::new(),
            rpc_port: 0,
            role: String::new(),
            category: String::new(),
            tags: Vec::new(),
            priority: default_priority(),
            enabled: default_enabled(),
            status: PeerStatus::default(),
        }
    }
}

fn default_priority() -> u32 {
    1
}

fn default_enabled() -> bool {
    true
}

/// Peer runtime status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerStatus {
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub last_seen: String,
    #[serde(default)]
    pub uptime: String,
    #[serde(default)]
    pub tasks_completed: u64,
    #[serde(default)]
    pub success_rate: f64,
    #[serde(default)]
    pub avg_response_time: u64,
    #[serde(default)]
    pub last_error: String,
}

impl Default for PeerStatus {
    fn default() -> Self {
        Self {
            state: "unknown".into(),
            last_seen: String::new(),
            uptime: String::new(),
            tasks_completed: 0,
            success_rate: 0.0,
            avg_response_time: 0,
            last_error: String::new(),
        }
    }
}

/// Dynamic cluster state (state.toml).
/// Automatically managed by the cluster module.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DynamicState {
    #[serde(default)]
    pub discovered: Vec<PeerConfig>,
    #[serde(default)]
    pub last_sync: String,
}

impl Default for DynamicState {
    fn default() -> Self {
        Self {
            discovered: Vec::new(),
            last_sync: chrono::Local::now().to_rfc3339(),
        }
    }
}

// ---------------------------------------------------------------------------
// Load / Save functions
// ---------------------------------------------------------------------------

/// Load static config from a TOML file.
pub fn load_static_config(path: &Path) -> Result<StaticConfig, ConfigError> {
    if !path.exists() {
        return Err(ConfigError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("config file not found: {}", path.display()),
        )));
    }
    let content = std::fs::read_to_string(path)?;
    let config: StaticConfig = toml::from_str(&content)?;
    Ok(config)
}

/// Save static config to a TOML file using atomic write.
///
/// **Peer-preserving**（UAT-BUG-3 根修 2026-09-15）：`StaticConfig` 结构上
/// 只含 `[node]`，整体序列化会把文件里已有的 `[peers.*]` 表静默抹掉
/// （真机实证：`node.update_identity` 改一次身份 → pair 写入的静态 peers
/// 全部消失）。因此对已存在且可解析的文件，解析旧文档后**只替换 `[node]`
/// 表、其余内容原样保留**；新文件/不可解析文件才整体序列化。全部调用方
/// （`node.update_identity` / persona 身份安装 / CLI 身份更新 /
/// `cluster init`）语义都是「写本节点身份」，无一想要清 peers。
pub fn save_static_config(path: &Path, config: &StaticConfig) -> Result<(), ConfigError> {
    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Peer-preserving write: replace only the [node] table when the file
    // already exists and parses as a TOML table. Any read/parse failure
    // (missing or corrupt file) falls back to serializing `config` fresh.
    let parsed = std::fs::read_to_string(path)
        .ok()
        .and_then(|content| content.parse::<toml::Value>().ok())
        .filter(|doc| doc.is_table());
    let toml_str = match parsed {
        Some(mut doc) => {
            let node_value = toml::Value::try_from(&config.node)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
            doc.as_table_mut()
                .expect("filtered to a table above")
                .insert("node".to_string(), node_value);
            toml::to_string_pretty(&doc)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?
        }
        None => toml::to_string_pretty(config)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?,
    };

    // Atomic write: write to tmp file, then rename
    atomic_write(path, toml_str.as_bytes())?;
    Ok(())
}

/// Collect known peer UDP endpoints (`host:port`) from the `[peers.*]`
/// tables of a static config file (peers.toml).
///
/// `[peers.X].address` is stored UDP-form (pair 与 RPC 合并 `persist_real_peer_to_toml`
/// 双路回写都落这个形态），是 peer UDP 发现端口的唯一权威来源——registry 里
/// 只存 RPC 形态地址（`host:rpc_port`），答不了「这个 peer 的 UDP 监听端口是几」。
/// 供 discovery announce 对异端口 peer 定向单播使用。
/// 文件缺失/不可解析时返回空 vec（退化为纯广播，行为同旧版）。
pub fn load_peer_udp_endpoints(path: &Path) -> Vec<String> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let Ok(doc) = content.parse::<toml::Value>() else {
        return Vec::new();
    };
    let Some(peers) = doc.get("peers").and_then(|v| v.as_table()) else {
        return Vec::new();
    };
    peers
        .values()
        .filter_map(|p| p.get("address").and_then(|v| v.as_str()))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Load dynamic state from a TOML file.
/// Returns a default empty state if the file doesn't exist.
pub fn load_dynamic_state(path: &Path) -> Result<DynamicState, ConfigError> {
    if !path.exists() {
        return Ok(DynamicState::default());
    }
    let content = std::fs::read_to_string(path)?;
    let state: DynamicState = toml::from_str(&content)?;
    Ok(state)
}

/// Save dynamic state to a TOML file using atomic write.
pub fn save_dynamic_state(path: &Path, state: &DynamicState) -> Result<(), ConfigError> {
    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Serialize to TOML
    let toml_str = toml::to_string_pretty(state)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;

    // Atomic write
    atomic_write(path, toml_str.as_bytes())?;
    Ok(())
}

/// Create a default static config.
pub fn create_static_config(node_id: &str, node_name: &str, address: &str) -> StaticConfig {
    StaticConfig {
        node: NodeInfo {
            id: node_id.into(),
            name: node_name.into(),
            address: address.into(),
            role: "worker".into(),
            category: "general".into(),
            tags: Vec::new(),
        },
    }
}

/// Sanitize a peer id into a TOML-safe key.
///
/// Replaces only characters that are illegal or ambiguous in TOML bare keys:
/// - `.` is the dotted-key separator in TOML, must be replaced
/// - `:` commonly appears in `host:port` and is reserved-style, replaced for safety
///
/// **域已收窄（发现②/B3，2026-09-15）**：写盘路径（`append_peer_to_file_with_name`
/// / `remove_peer_from_file`）已改字面 id 键（TOML 引号键保真），本函数只
/// 保留两处用途：① `upgrade_peer_in_peers_toml` 的旧键同键比对；②
/// `remove_peer_from_file` 清理旧版代码落盘的有损键残留。新写路径不得再调用。
pub fn sanitize_peer_key(peer_id: &str) -> String {
    peer_id.replace(['.', ':'], "_")
}

/// Append a peer as a `[peers.{sanitized_id}]` subtable to peers.toml.
///
/// This is the **canonical write path** for peers — used by both CLI
/// `cluster peers add` and web handler `nodes.add`. It parses the existing
/// file as a `toml::Value`, inserts a new subtable under `peers`, and writes
/// back atomically. This preserves any existing `[node]` section and other
/// `[peers.X]` entries without rewriting the whole file.
///
/// If the file does not exist, a minimal skeleton with `[node]` defaults
/// is created. If the file exists but `peers` is currently an array (legacy
/// `[[peers]]` format from `save_static_config`), it is replaced with an
/// empty table — this is considered safe because gateway.rs only reads the
/// `[peers.X]` table form anyway.
///
/// If a peer with the same sanitized key already exists, a `tracing::warn!`
/// is logged and the existing entry is overwritten. This is intentional —
/// "add the same name twice" is the canonical update flow.
/// `rpc_port > 0` 时写入显式 `rpc_port` 字段（pair 实测值），0 = 不写
/// （调用方不知道真实 RPC 端口，如 Dashboard 手工加节点），装载端回落
/// `udp+10000` 约定推导。
pub fn append_peer_to_file(
    path: &Path,
    peer_id: &str,
    address: &str,
    role: &str,
    category: &str,
    rpc_port: u16,
) -> Result<(), ConfigError> {
    append_peer_to_file_with_name(path, peer_id, address, role, category, None, rpc_port)
}

/// Resolve a static peer entry's RPC port at load time (单一真相源，gateway 与
/// CLI node 装载器同源消费）。
///
/// 显式 `rpc_port` 字段（pair / 占位升级写盘的探测实测值，>0 且 ≤65535）
/// 优先；缺字段时回落 `udp+10000` 约定推导（兼容手写/旧版条目）。
pub fn resolve_peer_rpc_port(peer_entry: &toml::Value, udp_port: u16) -> u16 {
    peer_entry
        .get("rpc_port")
        .and_then(|v| v.as_integer())
        .filter(|v| *v > 0 && *v <= u16::MAX as i64)
        .map(|v| v as u16)
        .unwrap_or_else(|| if udp_port > 0 { udp_port + 10000 } else { 0 })
}

/// Like [`append_peer_to_file`] but also persists a `name` field. Used when
/// upgrading a placeholder peer to its real node_id — the human-readable name
/// (e.g. "Node-A") must be written so that after a reload the static loader
/// recovers it (otherwise `name` falls back to the real_id key and lookups by
/// the human name fail).
///
/// `rpc_port`：pair 探测到的对端真实 RPC 端口（>0 时显式落盘）。此前只写
/// UDP `address`，装载端按 `udp+10000` 猜 RPC——非约定端口布局（如
/// udp=19411/rpc=29412）会永久猜错，且 B4 占位升级的地址比对同样含端口
/// 永不命中（2026-09-15 R1-7 真机实证：占位与真实 ID 双条目并存）。
pub fn append_peer_to_file_with_name(
    path: &Path,
    peer_id: &str,
    address: &str,
    role: &str,
    category: &str,
    name: Option<&str>,
    rpc_port: u16,
) -> Result<(), ConfigError> {
    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Load existing content or start with an empty table
    let mut doc: toml::Value = if path.exists() {
        let content = std::fs::read_to_string(path)?;
        match content.parse::<toml::Value>() {
            Ok(v) => v,
            Err(_) => {
                // File is corrupt — fall back to a fresh table. Better than
                // blocking the user from adding peers; they can investigate
                // the original file from backups if needed.
                toml::Value::Table(toml::value::Table::new())
            }
        }
    } else {
        toml::Value::Table(toml::value::Table::new())
    };

    let table = doc.as_table_mut().ok_or_else(|| {
        ConfigError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "peers.toml root is not a table",
        ))
    })?;

    // Ensure `peers` is a table (replace if it was a legacy array)
    if !table.get("peers").is_some_and(|v| v.is_table()) {
        table.insert(
            "peers".to_string(),
            toml::Value::Table(toml::value::Table::new()),
        );
    }
    let peers_table = table
        .get_mut("peers")
        .and_then(|v| v.as_table_mut())
        .expect("peers entry just ensured to be a table");

    // Build the new peer subtable
    // 发现②/B3（2026-09-15）：表键改**字面 peer_id** 写入——sanitize 把
    // `.`/`:` 有损替换 `_`，若对端自定义 id 含这两字符，系统写出的表键
    // ≠真实 id（静态装载「表键即 peer_id」→ 永远连不上），系统代写也会
    // 出错。TOML 序列化器对非 bare key 自动加引号（`[peers."peer.a"]`），
    // 字面键天然保真；gateway 装载器按表键原样读作 peer_id，全链一致。
    let key = peer_id.to_string();

    // Detect duplicate and warn (do not block — overwrite is intentional)
    if peers_table.contains_key(&key) {
        tracing::warn!(
            peer_id = peer_id,
            key = %key,
            "[ClusterConfig] Peer already exists in peers.toml, overwriting"
        );
    }

    let mut peer_entry = toml::value::Table::new();
    peer_entry.insert(
        "address".to_string(),
        toml::Value::String(address.to_string()),
    );
    if let Some(n) = name {
        peer_entry.insert("name".to_string(), toml::Value::String(n.to_string()));
    }
    peer_entry.insert("role".to_string(), toml::Value::String(role.to_string()));
    peer_entry.insert(
        "category".to_string(),
        toml::Value::String(category.to_string()),
    );
    if rpc_port > 0 {
        peer_entry.insert(
            "rpc_port".to_string(),
            toml::Value::Integer(rpc_port as i64),
        );
    }
    peers_table.insert(key, toml::Value::Table(peer_entry));

    // Serialize and atomic write
    let toml_str = toml::to_string_pretty(&doc).map_err(|e| {
        ConfigError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            e.to_string(),
        ))
    })?;
    atomic_write(path, toml_str.as_bytes())?;
    Ok(())
}

/// Remove a peer's `[peers.{sanitized_id}]` subtable from peers.toml.
///
/// Symmetric counterpart to `append_peer_to_file`, used by `nodes.remove`
/// (web) and `cluster peers remove` (CLI) to persist node deletion.
///
/// Idempotent: returns `Ok(())` if the file does not exist, has no `peers`
/// table, or the key is not present — caller should not need to check
/// existence first. Preserves the `[node]` section and any other `[peers.X]`
/// entries. If the file is corrupt, the deletion is skipped with a warn log
/// (same fallback strategy as `append_peer_to_file`).
pub fn remove_peer_from_file(path: &Path, peer_id: &str) -> Result<(), ConfigError> {
    // No file → nothing to remove.
    if !path.exists() {
        return Ok(());
    }

    let content = std::fs::read_to_string(path)?;
    let mut doc: toml::Value = match content.parse::<toml::Value>() {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "[ClusterConfig] Skipping peers.toml removal (parse failed)"
            );
            return Ok(());
        }
    };

    let table = doc.as_table_mut().ok_or_else(|| {
        ConfigError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "peers.toml root is not a table",
        ))
    })?;

    let peers_table = match table.get_mut("peers").and_then(|v| v.as_table_mut()) {
        Some(t) => t,
        None => return Ok(()), // no peers table → nothing to remove
    };

    // 发现②/B3：字面键删除为主；若与旧版 sanitize 键不同且存在，一并
    // 清理——那是旧代码给同一 peer 留下的有损键残留（写盘已字面化，
    // 这里只清历史遗留，不留双条目）。
    let key = peer_id.to_string();
    let legacy = sanitize_peer_key(peer_id);
    let removed = peers_table.remove(&key).is_some();
    let removed_legacy = if legacy != key {
        peers_table.remove(&legacy).is_some()
    } else {
        false
    };
    if !removed && !removed_legacy {
        // Key not present — nothing was removed. Avoid the atomic rewrite.
        return Ok(());
    }

    let toml_str = toml::to_string_pretty(&doc).map_err(|e| {
        ConfigError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            e.to_string(),
        ))
    })?;
    atomic_write(path, toml_str.as_bytes())?;
    Ok(())
}

/// Convert an RPC address (`host:rpc_port`) to the UDP address (`host:udp_port`)
/// for peers.toml write-back, reversing the static loader's
/// `rpc_port = udp_port + 10000` convention (gateway.rs). Falls back to the
/// input unchanged if the port can't be parsed or is ≤ 10000 (no convention to
/// reverse — e.g. a non-standard port or an address without a port).
///
/// 单一真相源：`Cluster::persist_real_peer_to_toml`（升级/merge 写盘）与
/// `pair` 配对写盘共用本函数，保证 RPC↔UDP 换算只此一份。
pub fn rpc_to_udp_address(rpc_addr: &str) -> String {
    if let Some((host, port_str)) = rpc_addr.rsplit_once(':')
        && let Ok(rpc_port) = port_str.parse::<u32>()
        && rpc_port > 10000
    {
        return format!("{}:{}", host, rpc_port - 10000);
    }
    rpc_addr.to_string()
}

/// Load existing config or create a default one.
pub fn load_or_create_config(path: &Path, node_id: &str) -> StaticConfig {
    match load_static_config(path) {
        Ok(config) => config,
        Err(_) => create_static_config(node_id, &format!("Bot {}", node_id), ""),
    }
}

/// Ensure `[node].id` is set in peers.toml. If the file doesn't exist or
/// `[node].id` is empty, write the provided `node_id` and persist. Otherwise
/// leave the file untouched (preserves user-edited id).
///
/// Used by `Cluster::with_workspace` to persist runtime-generated IDs so they
/// remain stable across restarts. Operates on raw TOML to preserve any
/// existing `[peers.X]` subtables (which `StaticConfig` doesn't represent).
///
/// Returns true if the file was modified (id was missing and got written).
pub fn ensure_node_id(path: &Path, node_id: &str) -> Result<bool, ConfigError> {
    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Load existing content or start with an empty table
    let mut doc: toml::Value = if path.exists() {
        let content = std::fs::read_to_string(path)?;
        match content.parse::<toml::Value>() {
            Ok(v) => v,
            Err(_) => toml::Value::Table(toml::value::Table::new()),
        }
    } else {
        toml::Value::Table(toml::value::Table::new())
    };

    let table = doc.as_table_mut().ok_or_else(|| {
        ConfigError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "peers.toml root is not a table",
        ))
    })?;

    // Ensure [node] is a table
    if !table.get("node").is_some_and(|v| v.is_table()) {
        table.insert(
            "node".to_string(),
            toml::Value::Table(toml::value::Table::new()),
        );
    }
    let node_table = table
        .get_mut("node")
        .and_then(|v| v.as_table_mut())
        .expect("node entry just ensured to be a table");

    // Check if id is already set to the same value (no-op)
    let current_id = node_table.get("id").and_then(|v| v.as_str()).unwrap_or("");
    if !current_id.is_empty() {
        // User has set an id; respect it
        return Ok(false);
    }

    // Set the id
    node_table.insert("id".to_string(), toml::Value::String(node_id.to_string()));

    // Serialize and atomic write
    let toml_str = toml::to_string_pretty(&doc).map_err(|e| {
        ConfigError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            e.to_string(),
        ))
    })?;
    atomic_write(path, toml_str.as_bytes())?;
    Ok(true)
}

// ---------------------------------------------------------------------------
// Atomic write helper
// ---------------------------------------------------------------------------

/// Write data to a file atomically — REL-002（2026-09-23）起委托统一 helper
/// `nemesis_utils::write_file_atomic`（唯一临时名 + sync_all + 失败清理 +
/// unix 0600 创建即挂），本函数只保留签名兼容（peers.toml 家族 5 个调用点）。
fn atomic_write(path: &Path, data: &[u8]) -> Result<(), ConfigError> {
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), data, 0o600)
        .map_err(|e| ConfigError::Io(std::io::Error::other(e)))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
