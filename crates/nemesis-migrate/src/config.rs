//! Migration configuration and shared utility functions.
//!
//! Mirrors Go `module/migrate/config.go` for config conversion helpers.
//! The main OpenClaw config conversion is in `openclaw_config.rs`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Migration configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrateConfig {
    pub workspace_path: String,
    pub target_version: u32,
}

impl Default for MigrateConfig {
    fn default() -> Self {
        Self {
            workspace_path: String::new(),
            target_version: 1,
        }
    }
}

// ---------------------------------------------------------------------------
// Config conversion helper functions (mirrors Go config.go)
// ---------------------------------------------------------------------------

/// Extract a nested object (map) from a JSON object by key.
pub fn get_map<'a>(
    data: &'a serde_json::Value,
    key: &str,
) -> Option<&'a serde_json::Map<String, serde_json::Value>> {
    data.get(key)?.as_object()
}

/// Extract a string value from a JSON object by key.
pub fn get_string<'a>(data: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    data.get(key)?.as_str()
}

/// Extract a float value from a JSON object by key.
/// In Go, JSON numbers are parsed as float64 by default.
pub fn get_float(data: &serde_json::Value, key: &str) -> Option<f64> {
    data.get(key)?.as_f64()
}

/// Extract an integer value from a JSON object by key.
pub fn get_int(data: &serde_json::Value, key: &str) -> Option<i64> {
    data.get(key)?.as_i64()
}

/// Extract a boolean value from a JSON object by key.
pub fn get_bool(data: &serde_json::Value, key: &str) -> Option<bool> {
    data.get(key)?.as_bool()
}

/// Extract a boolean value from a JSON object, returning a default if missing or wrong type.
pub fn get_bool_or_default(data: &serde_json::Value, key: &str, default: bool) -> bool {
    get_bool(data, key).unwrap_or(default)
}

/// Extract a string slice from a JSON object by key.
/// Handles both `["a", "b"]` arrays and `null`/missing values.
pub fn get_string_slice(data: &serde_json::Value, key: &str) -> Vec<String> {
    match data.get(key) {
        Some(serde_json::Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect(),
        _ => Vec::new(),
    }
}

/// Rewrite a workspace path from OpenClaw format to NemesisBot format.
/// Replaces `.openclaw` with `.nemesisbot` in the path.
pub fn rewrite_workspace_path(path: &str) -> String {
    path.replace(".openclaw", ".nemesisbot")
}

/// Convert a HashMap<String, Value> into a serde_json::Value::Object.
pub fn hashmap_to_value(map: HashMap<String, serde_json::Value>) -> serde_json::Value {
    serde_json::Value::Object(map.into_iter().collect())
}

#[cfg(test)]
mod tests;
