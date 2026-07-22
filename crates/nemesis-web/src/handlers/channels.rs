//! Channels handler — list/get/update/test channel configurations.

use crate::handlers::{mask_sensitive, require_home};
use crate::ws_router::{ModuleHandler, RequestContext};
use std::path::PathBuf;

pub struct ChannelsHandler {
    _priv: (),
}

impl ChannelsHandler {
    pub fn new() -> Self {
        Self { _priv: () }
    }
}

#[async_trait::async_trait]
impl ModuleHandler for ChannelsHandler {
    fn module_name(&self) -> &str {
        "channels"
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let home = require_home(ctx)?;
        match cmd {
            "list" => self.list(home),
            "get" => {
                let data = data.ok_or("missing data")?;
                let name = crate::handlers::get_str(&data, "name")?;
                self.get(home, &name)
            }
            "update" => {
                let data = data.ok_or("missing data")?;
                let name = crate::handlers::get_str(&data, "name")?;
                self.update(home, &name, &data)
            }
            "test" => {
                let data = data.ok_or("missing data")?;
                let name = crate::handlers::get_str(&data, "name")?;
                self.test(&name)
            }
            _ => Err(format!("unknown command: channels.{}", cmd)),
        }
    }
}

fn load_config(home: &str) -> Result<nemesis_config::Config, String> {
    // Live store first (single source of truth); fall back to disk in CLI mode.
    if let Some(cfg) = nemesis_config::load_live() {
        return Ok(cfg);
    }
    let path = PathBuf::from(home).join("config.json");
    nemesis_config::load_config(&path).map_err(|e| format!("failed to load config: {}", e))
}

fn save_config(home: &str, config: &mut nemesis_config::Config) -> Result<(), String> {
    // Live store first: update lands in-memory AND on disk, so every consumer
    // (executor sandbox probe, tier, …) sees the change without a restart.
    if let Some(r) = nemesis_config::save_live(config.clone()) {
        return r.map_err(|e| format!("failed to save config: {}", e));
    }
    let path = PathBuf::from(home).join("config.json");
    nemesis_config::save_config(&path, config).map_err(|e| format!("failed to save config: {}", e))
}

impl ChannelsHandler {
    fn list(&self, home: &str) -> Result<Option<serde_json::Value>, String> {
        let config = load_config(home)?;
        let json = serde_json::to_value(&config.channels)
            .map_err(|e| format!("failed to serialize channels: {}", e))?;
        // Return the channels object with enabled status summary
        let empty_map = serde_json::Map::new();
        let channels_map = json.as_object().unwrap_or(&empty_map);
        let summary: Vec<serde_json::Value> = channels_map
            .iter()
            .map(|(name, value)| {
                let enabled = value
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                serde_json::json!({
                    "name": name,
                    "enabled": enabled,
                })
            })
            .collect();
        Ok(Some(serde_json::json!({ "channels": summary })))
    }

    fn get(&self, home: &str, name: &str) -> Result<Option<serde_json::Value>, String> {
        let config = load_config(home)?;
        let json = serde_json::to_value(&config.channels)
            .map_err(|e| format!("failed to serialize channels: {}", e))?;
        let channel = json
            .get(name)
            .ok_or_else(|| format!("channel '{}' not found", name))?
            .clone();
        // Mask sensitive fields
        let masked = mask_sensitive_fields(channel);
        Ok(Some(serde_json::json!({ "name": name, "config": masked })))
    }

    fn update(
        &self,
        home: &str,
        name: &str,
        data: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        let channel_config = data.get("config").ok_or("missing config field")?.clone();
        let mut config = load_config(home)?;

        // Serialize channels to a mutable JSON object, update the channel, then re-parse
        let mut channels_json = serde_json::to_value(&config.channels)
            .map_err(|e| format!("failed to serialize channels: {}", e))?;
        if channels_json.get(name).is_none() {
            return Err(format!("channel '{}' not found", name));
        }
        channels_json[name] = channel_config;
        config.channels = serde_json::from_value(channels_json)
            .map_err(|e| format!("failed to parse updated channels: {}", e))?;

        save_config(home, &mut config)?;
        Ok(Some(serde_json::json!({ "updated": true, "name": name })))
    }

    fn test(&self, name: &str) -> Result<Option<serde_json::Value>, String> {
        Ok(Some(serde_json::json!({
            "name": name,
            "status": "not_implemented",
            "message": "Channel test not yet supported"
        })))
    }
}

/// Recursively mask known sensitive field names in a JSON value.
fn mask_sensitive_fields(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let new_map: serde_json::Map<String, serde_json::Value> = map
                .into_iter()
                .map(|(k, v)| {
                    if crate::handlers::is_sensitive_field(&k) {
                        if let Some(s) = v.as_str() {
                            if !s.is_empty() {
                                return (k, serde_json::Value::String(mask_sensitive(s)));
                            }
                        }
                    }
                    (k, mask_sensitive_fields(v))
                })
                .collect();
            serde_json::Value::Object(new_map)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.into_iter().map(mask_sensitive_fields).collect())
        }
        other => other,
    }
}
