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
pub fn get_map<'a>(data: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Map<String, serde_json::Value>> {
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
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_default_config() {
        let config = MigrateConfig::default();
        assert!(config.workspace_path.is_empty());
        assert_eq!(config.target_version, 1);
    }

    #[test]
    fn test_get_map() {
        let data = json!({"agents": {"defaults": {"llm": "test"}}});
        let agents = get_map(&data, "agents").unwrap();
        assert!(agents.contains_key("defaults"));

        assert!(get_map(&data, "missing").is_none());
        assert!(get_map(&json!({"key": "string"}), "key").is_none());
    }

    #[test]
    fn test_get_string() {
        let data = json!({"name": "test", "count": 42});
        assert_eq!(get_string(&data, "name"), Some("test"));
        assert!(get_string(&data, "count").is_none());
        assert!(get_string(&data, "missing").is_none());
    }

    #[test]
    fn test_get_float() {
        let data = json!({"temp": 0.7, "count": 42});
        assert_eq!(get_float(&data, "temp"), Some(0.7));
        assert_eq!(get_float(&data, "count"), Some(42.0));
        assert!(get_float(&data, "missing").is_none());
    }

    #[test]
    fn test_get_int() {
        let data = json!({"count": 42, "temp": 0.7});
        assert_eq!(get_int(&data, "count"), Some(42));
        assert!(get_int(&data, "temp").is_none());
    }

    #[test]
    fn test_get_bool() {
        let data = json!({"enabled": true, "disabled": false, "name": "test"});
        assert_eq!(get_bool(&data, "enabled"), Some(true));
        assert_eq!(get_bool(&data, "disabled"), Some(false));
        assert!(get_bool(&data, "name").is_none());
    }

    #[test]
    fn test_get_bool_or_default() {
        let data = json!({"enabled": true});
        assert!(get_bool_or_default(&data, "enabled", false));
        assert!(!get_bool_or_default(&data, "missing", false));
        assert!(get_bool_or_default(&data, "missing", true));
    }

    #[test]
    fn test_get_string_slice() {
        let data = json!({"allow_from": ["user1", "user2", 123]});
        let slice = get_string_slice(&data, "allow_from");
        assert_eq!(slice, vec!["user1", "user2"]);

        let empty = get_string_slice(&data, "missing");
        assert!(empty.is_empty());
    }

    #[test]
    fn test_rewrite_workspace_path() {
        assert_eq!(
            rewrite_workspace_path("/home/user/.openclaw/workspace"),
            "/home/user/.nemesisbot/workspace"
        );
        assert_eq!(
            rewrite_workspace_path("/home/user/.nemesisbot/workspace"),
            "/home/user/.nemesisbot/workspace"
        );
        assert_eq!(
            rewrite_workspace_path("no_openclaw_here"),
            "no_openclaw_here"
        );
    }

    #[test]
    fn test_hashmap_to_value() {
        let mut map = HashMap::new();
        map.insert("key".to_string(), json!("value"));
        let val = hashmap_to_value(map);
        assert!(val.is_object());
        assert_eq!(val["key"], "value");
    }

    #[test]
    fn test_migrate_config_serialization() {
        let config = MigrateConfig {
            workspace_path: "/test/path".to_string(),
            target_version: 2,
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: MigrateConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.workspace_path, "/test/path");
        assert_eq!(parsed.target_version, 2);
    }

    #[test]
    fn test_get_string_array() {
        let data = json!({"list": ["a", "b", "c"]});
        let arr = get_string_slice(&data, "list");
        assert_eq!(arr, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_get_map_nested() {
        let data = json!({"a": {"b": {"c": 1}}});
        let inner = get_map(&data, "a").unwrap();
        assert_eq!(inner["b"]["c"], 1);
    }

    #[test]
    fn test_rewrite_workspace_path_edge_cases() {
        // Path with .openclaw at end
        assert_eq!(
            rewrite_workspace_path("/home/.openclaw"),
            "/home/.nemesisbot"
        );
        // Path with .openclaw in middle - also gets rewritten since it contains .openclaw
        assert_eq!(
            rewrite_workspace_path("/home/.openclaw_backup/workspace"),
            "/home/.nemesisbot_backup/workspace"
        );
    }

    #[test]
    fn test_hashmap_to_value_empty() {
        let map: HashMap<String, serde_json::Value> = HashMap::new();
        let val = hashmap_to_value(map);
        assert!(val.is_object());
        assert!(val.as_object().unwrap().is_empty());
    }
}
