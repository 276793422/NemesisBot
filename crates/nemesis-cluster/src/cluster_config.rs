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
pub fn save_static_config(path: &Path, config: &StaticConfig) -> Result<(), ConfigError> {
    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Serialize to TOML
    let toml_str = toml::to_string_pretty(config).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
    })?;

    // Atomic write: write to tmp file, then rename
    atomic_write(path, toml_str.as_bytes())?;
    Ok(())
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
    let toml_str = toml::to_string_pretty(state).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
    })?;

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
/// TOML v1.0.0 explicitly allows `-` and `_` in bare keys (`A-Za-z0-9_-`), so
/// both are preserved as-is. This makes the mapping user-input → key → peer_id
/// an identity function for those characters (no round-trip loss).
pub fn sanitize_peer_key(peer_id: &str) -> String {
    peer_id.replace('.', "_").replace(':', "_")
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
pub fn append_peer_to_file(
    path: &Path,
    peer_id: &str,
    address: &str,
    role: &str,
    category: &str,
) -> Result<(), ConfigError> {
    append_peer_to_file_with_name(path, peer_id, address, role, category, None)
}

/// Like [`append_peer_to_file`] but also persists a `name` field. Used when
/// upgrading a placeholder peer to its real node_id — the human-readable name
/// (e.g. "Node-A") must be written so that after a reload the static loader
/// recovers it (otherwise `name` falls back to the real_id key and lookups by
/// the human name fail).
pub fn append_peer_to_file_with_name(
    path: &Path,
    peer_id: &str,
    address: &str,
    role: &str,
    category: &str,
    name: Option<&str>,
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
    if !table.get("peers").map_or(false, |v| v.is_table()) {
        table.insert("peers".to_string(), toml::Value::Table(toml::value::Table::new()));
    }
    let peers_table = table
        .get_mut("peers")
        .and_then(|v| v.as_table_mut())
        .expect("peers entry just ensured to be a table");

    // Build the new peer subtable
    let key = sanitize_peer_key(peer_id);

    // Detect duplicate and warn (do not block — overwrite is intentional)
    if peers_table.contains_key(&key) {
        tracing::warn!(
            peer_id = peer_id,
            key = %key,
            "[ClusterConfig] Peer already exists in peers.toml, overwriting"
        );
    }

    let mut peer_entry = toml::value::Table::new();
    peer_entry.insert("address".to_string(), toml::Value::String(address.to_string()));
    if let Some(n) = name {
        peer_entry.insert("name".to_string(), toml::Value::String(n.to_string()));
    }
    peer_entry.insert("role".to_string(), toml::Value::String(role.to_string()));
    peer_entry.insert("category".to_string(), toml::Value::String(category.to_string()));
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

    let key = sanitize_peer_key(peer_id);
    if peers_table.remove(&key).is_none() {
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
    if !table.get("node").map_or(false, |v| v.is_table()) {
        table.insert("node".to_string(), toml::Value::Table(toml::value::Table::new()));
    }
    let node_table = table
        .get_mut("node")
        .and_then(|v| v.as_table_mut())
        .expect("node entry just ensured to be a table");

    // Check if id is already set to the same value (no-op)
    let current_id = node_table
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
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

/// Write data to a file atomically: write to a `.tmp` file first, then rename.
fn atomic_write(path: &Path, data: &[u8]) -> Result<(), ConfigError> {
    let tmp_path = path.with_extension("toml.tmp");

    std::fs::write(&tmp_path, data)?;

    // Atomic rename (on Windows, this replaces if destination exists)
    match std::fs::rename(&tmp_path, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            // Clean up temp file
            let _ = std::fs::remove_file(&tmp_path);
            Err(ConfigError::Io(e))
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
