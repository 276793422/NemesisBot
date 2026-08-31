//! Config handler — get/save/set_field config.json + CORS commands.

use crate::handlers::require_home;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::path::PathBuf;

pub struct ConfigHandler {
    _priv: (),
}

impl Default for ConfigHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfigHandler {
    pub fn new() -> Self {
        Self { _priv: () }
    }
}

#[async_trait::async_trait]
impl ModuleHandler for ConfigHandler {
    fn module_name(&self) -> &str {
        "config"
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let home = require_home(ctx)?;
        match cmd {
            "get" => self.get(home),
            "save" => {
                let data = data.ok_or("missing data")?;
                self.save(home, &data)
            }
            "set_field" => {
                let data = data.ok_or("missing data")?;
                let path = crate::handlers::get_str(&data, "path")?;
                let value = data
                    .get("value")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                self.set_field(home, &path, &value)
            }
            "cors.list" => self.cors_list(),
            "cors.add" => {
                let data = data.ok_or("missing data")?;
                let origin = crate::handlers::get_str(&data, "origin")?;
                self.cors_add(&origin)
            }
            "cors.remove" => {
                let data = data.ok_or("missing data")?;
                let origin = crate::handlers::get_str(&data, "origin")?;
                self.cors_remove(&origin)
            }
            "cors.toggle" => {
                let data = data.ok_or("missing data")?;
                let enabled = data
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .ok_or("missing or invalid 'enabled' field")?;
                self.cors_toggle(enabled)
            }
            _ => Err(format!("unknown command: config.{}", cmd)),
        }
    }
}

fn config_path(home: &str) -> PathBuf {
    PathBuf::from(home).join("config.json")
}

fn load_config(home: &str) -> Result<nemesis_config::Config, String> {
    if let Some(cfg) = nemesis_config::load_live() {
        return Ok(cfg);
    }
    nemesis_config::load_config(&config_path(home))
        .map_err(|e| format!("failed to load config: {}", e))
}

fn save_config_to_disk(home: &str, config: &mut nemesis_config::Config) -> Result<(), String> {
    if let Some(r) = nemesis_config::save_live(config.clone()) {
        return r.map_err(|e| format!("failed to save config: {}", e));
    }
    nemesis_config::save_config(&config_path(home), config)
        .map_err(|e| format!("failed to save config: {}", e))
}

impl ConfigHandler {
    fn get(&self, home: &str) -> Result<Option<serde_json::Value>, String> {
        let config = load_config(home)?;
        let mut json = serde_json::to_value(&config)
            .map_err(|e| format!("failed to serialize config: {}", e))?;
        // Mask sensitive fields
        sanitize_config(&mut json);
        Ok(Some(json))
    }

    fn save(
        &self,
        home: &str,
        data: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        let mut config: nemesis_config::Config = serde_json::from_value(data.clone())
            .map_err(|e| format!("invalid config data: {}", e))?;
        save_config_to_disk(home, &mut config)?;
        Ok(Some(serde_json::json!({ "saved": true })))
    }

    fn set_field(
        &self,
        home: &str,
        path: &str,
        value: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        let mut config = load_config(home)?;
        let mut json = serde_json::to_value(&config)
            .map_err(|e| format!("failed to serialize config: {}", e))?;

        set_json_path(&mut json, path, value.clone())?;

        config = serde_json::from_value(json)
            .map_err(|e| format!("invalid config after field update: {}", e))?;

        // Typed reparse silently drops unknown keys (serde ignores unrecognized
        // fields). Verify the field survived the round-trip before claiming
        // success — otherwise an unknown path would answer `updated:true` while
        // persisting nothing (R1 真机验收发现的诚实性缺口，G6 loud 拒绝要求).
        let reserialized = serde_json::to_value(&config)
            .map_err(|e| format!("failed to serialize config: {}", e))?;
        match json_path_get(&reserialized, path) {
            None => return Err(format!("unknown config field: {}", path)),
            Some(actual) if actual != value => {
                return Err(format!(
                    "config field {} did not round-trip (value rejected or normalized)",
                    path
                ));
            }
            _ => {}
        }

        save_config_to_disk(home, &mut config)?;
        Ok(Some(serde_json::json!({ "updated": true, "path": path })))
    }

    fn cors_list(&self) -> Result<Option<serde_json::Value>, String> {
        // CORSManager access would need to go through AppState
        // For now, read from cors.json directly
        Ok(Some(
            serde_json::json!({ "origins": [], "message": "CORS manager not connected" }),
        ))
    }

    fn cors_add(&self, _origin: &str) -> Result<Option<serde_json::Value>, String> {
        Ok(Some(
            serde_json::json!({ "added": false, "message": "CORS manager not connected" }),
        ))
    }

    fn cors_remove(&self, _origin: &str) -> Result<Option<serde_json::Value>, String> {
        Ok(Some(
            serde_json::json!({ "removed": false, "message": "CORS manager not connected" }),
        ))
    }

    fn cors_toggle(&self, _enabled: bool) -> Result<Option<serde_json::Value>, String> {
        Ok(Some(
            serde_json::json!({ "toggled": false, "message": "CORS manager not connected" }),
        ))
    }
}

/// Mask sensitive fields in a config JSON object.
fn sanitize_config(json: &mut serde_json::Value) {
    if let Some(obj) = json.as_object_mut() {
        for (key, value) in obj.iter_mut() {
            if crate::handlers::is_sensitive_field(key) {
                if let Some(s) = value.as_str()
                    && !s.is_empty() {
                        *value = serde_json::Value::String(crate::handlers::mask_sensitive(s));
                    }
            } else {
                sanitize_config(value);
            }
        }
    } else if let Some(arr) = json.as_array_mut() {
        for item in arr.iter_mut() {
            sanitize_config(item);
        }
    }
}

/// Set a value at a dot-separated path in a JSON object. Array segments are
/// addressed by numeric index (e.g. `model_list.0.api_key`); out-of-bounds
/// indices are a loud error (no append semantics).
fn set_json_path(
    json: &mut serde_json::Value,
    path: &str,
    value: serde_json::Value,
) -> Result<(), String> {
    if path.is_empty() {
        return Err("empty path".to_string());
    }
    let parts: Vec<&str> = path.split('.').collect();
    let last = parts.len() - 1;
    let mut current = json;
    for (i, part) in parts.iter().enumerate() {
        // 数组节点：段必须解析为 usize 下标，越界显式报错。
        if current.is_array() {
            let idx: usize = part.parse().map_err(|_| {
                format!("path segment `{part}` is not a valid array index ({path})")
            })?;
            if current.get(idx).is_none() {
                return Err(format!("array index out of bounds: {path}"));
            }
            if i == last {
                current[idx] = value;
                return Ok(());
            }
            current = &mut current[idx];
            continue;
        }
        if i == last {
            current[part] = value;
            return Ok(());
        }
        if current[part].is_null() {
            current[part] = serde_json::json!({});
        }
        current = &mut current[part];
    }
    Ok(())
}

/// Resolve a dot path on a JSON value; None if any segment is missing.
/// Array nodes take numeric-index segments (mirror of [`set_json_path`]).
fn json_path_get<'a>(
    json: &'a serde_json::Value,
    path: &str,
) -> Option<&'a serde_json::Value> {
    let mut current = json;
    for part in path.split('.') {
        if current.is_array() {
            current = current.get(part.parse::<usize>().ok()?)?;
        } else {
            current = current.get(part)?;
        }
    }
    Some(current)
}
