//! OpenClaw config migration: load, convert, and merge OpenClaw configs to NemesisBot format.
//!
//! Mirrors Go migrate/config.go.

use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

/// Supported providers for OpenClaw -> NemesisBot migration.
const SUPPORTED_PROVIDERS: &[&str] = &[
    "anthropic",
    "openai",
    "openrouter",
    "groq",
    "zhipu",
    "vllm",
    "gemini",
];

/// Supported channels for migration.
const SUPPORTED_CHANNELS: &[&str] = &[
    "telegram", "discord", "whatsapp", "feishu", "qq", "dingtalk", "maixcam",
];

/// Find OpenClaw config file in the given home directory.
pub fn find_openclaw_config(openclaw_home: &Path) -> Result<String, String> {
    let candidates = [
        openclaw_home.join("openclaw.json"),
        openclaw_home.join("config.json"),
    ];
    for p in &candidates {
        if p.exists() {
            return Ok(p.to_string_lossy().to_string());
        }
    }
    Err(format!(
        "no config file found in {} (tried openclaw.json, config.json)",
        openclaw_home.display()
    ))
}

/// Load an OpenClaw config file and convert keys to snake_case.
pub fn load_openclaw_config(config_path: &Path) -> Result<HashMap<String, Value>, String> {
    let data = std::fs::read_to_string(config_path)
        .map_err(|e| format!("reading OpenClaw config: {}", e))?;
    let raw: Value =
        serde_json::from_str(&data).map_err(|e| format!("parsing OpenClaw config: {}", e))?;
    let converted = convert_keys_to_snake(&raw);
    let result = converted
        .as_object()
        .ok_or_else(|| "unexpected config format".to_string())?
        .clone()
        .into_iter()
        .collect();
    Ok(result)
}

/// Convert an OpenClaw config (as HashMap<String, Value>) into a NemesisBot Config JSON value.
/// Returns (config_json, warnings).
pub fn convert_config(data: &HashMap<String, Value>) -> (Value, Vec<String>) {
    let mut warnings = Vec::new();

    // Start with default NemesisBot config
    let mut config = serde_json::json!({
        "agents": {
            "defaults": {
                "workspace": "",
                "restrict_to_workspace": true,
                "llm": "zhipu/glm-4.7-flash",
                "max_tokens": 8192,
                "temperature": 0.7,
                "max_tool_iterations": 100,
                "concurrent_request_mode": "reject",
                "queue_size": 8
            }
        },
        "channels": {
            "whatsapp": {"enabled": false, "bridge_url": "ws://localhost:3001"},
            "telegram": {"enabled": false},
            "feishu": {"enabled": false},
            "discord": {"enabled": false},
            "maixcam": {"enabled": false, "host": "0.0.0.0", "port": 18790},
            "qq": {"enabled": false},
            "dingtalk": {"enabled": false},
            "web": {"enabled": true, "host": "0.0.0.0", "port": 8080, "path": "/ws", "heartbeat_interval": 30, "session_timeout": 3600}
        },
        "model_list": [],
        "gateway": {"host": "0.0.0.0", "port": 18790}
    });

    // Process agents.defaults
    if let Some(agents) = data.get("agents").and_then(|v| v.as_object()) {
        if let Some(defaults) = agents.get("defaults").and_then(|v| v.as_object()) {
            if let Some(llm) = defaults.get("llm").and_then(|v| v.as_str()) {
                config["agents"]["defaults"]["llm"] = Value::String(llm.to_string());
            } else if let Some(model) = defaults.get("model").and_then(|v| v.as_str()) {
                let provider = infer_provider_from_model(model);
                let llm = if provider.is_empty() {
                    format!("zhipu/{}", model)
                } else {
                    format!("{}/{}", provider, model)
                };
                config["agents"]["defaults"]["llm"] = Value::String(llm);
            }
            if let Some(max_tokens) = defaults.get("max_tokens").and_then(|v| v.as_i64()) {
                config["agents"]["defaults"]["max_tokens"] = Value::Number(max_tokens.into());
            }
            if let Some(temperature) = defaults.get("temperature").and_then(|v| v.as_f64()) {
                config["agents"]["defaults"]["temperature"] = serde_json::json!(temperature);
            }
            if let Some(max_tool_iter) =
                defaults.get("max_tool_iterations").and_then(|v| v.as_i64())
            {
                config["agents"]["defaults"]["max_tool_iterations"] =
                    Value::Number(max_tool_iter.into());
            }
            if let Some(workspace) = defaults.get("workspace").and_then(|v| v.as_str()) {
                let rewritten = workspace.replace(".openclaw", ".nemesisbot");
                config["agents"]["defaults"]["workspace"] = Value::String(rewritten);
            }
        }
    }

    // Process providers
    if let Some(providers) = data.get("providers").and_then(|v| v.as_object()) {
        for (name, val) in providers {
            let p_map = match val.as_object() {
                Some(m) => m,
                None => continue,
            };
            let api_key = p_map.get("api_key").and_then(|v| v.as_str()).unwrap_or("");
            let api_base = p_map.get("api_base").and_then(|v| v.as_str()).unwrap_or("");

            if !SUPPORTED_PROVIDERS.contains(&name.as_str()) {
                if !api_key.is_empty() || !api_base.is_empty() {
                    warnings.push(format!(
                        "Provider '{}' not supported in NemesisBot, skipping",
                        name
                    ));
                }
                continue;
            }

            let (model_identifier, model_name) = match name.as_str() {
                "anthropic" => ("anthropic/claude-sonnet-4-20250514", "claude-sonnet-4"),
                "openai" => ("openai/gpt-4o", "gpt-4o"),
                "openrouter" => ("openrouter/gpt-4", "openrouter-model"),
                "groq" => ("groq/llama-3.3-70b-versatile", "groq-model"),
                "zhipu" => ("zhipu/glm-4.7-flash", "glm-4.7"),
                "vllm" => ("vllm/local", "vllm-local"),
                "gemini" => ("gemini/gemini-2.0-flash-exp", "gemini-2.0-flash"),
                _ => continue,
            };

            if !api_key.is_empty() || !api_base.is_empty() {
                let mc = serde_json::json!({
                    "model_name": model_name,
                    "model": model_identifier,
                    "api_key": api_key,
                    "api_base": api_base
                });

                if let Some(model_list) = config["model_list"].as_array_mut() {
                    let exists = model_list
                        .iter()
                        .any(|m| m.get("model_name").and_then(|v| v.as_str()) == Some(model_name));
                    if !exists {
                        model_list.push(mc);
                    }
                }
            }
        }
    }

    // Process channels
    if let Some(channels) = data.get("channels").and_then(|v| v.as_object()) {
        for (name, val) in channels {
            let c_map = match val.as_object() {
                Some(m) => m,
                None => continue,
            };
            if !SUPPORTED_CHANNELS.contains(&name.as_str()) {
                warnings.push(format!(
                    "Channel '{}' not supported in NemesisBot, skipping",
                    name
                ));
                continue;
            }

            let enabled = c_map
                .get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            match name.as_str() {
                "telegram" | "discord" | "whatsapp" | "feishu" | "qq" | "dingtalk" | "maixcam" => {
                    if let Some(channel) = config["channels"].get_mut(name) {
                        channel["enabled"] = Value::Bool(enabled);
                        // Copy known fields
                        for (key, value) in c_map {
                            if key != "enabled" {
                                channel[key.clone()] = value.clone();
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // Process gateway
    if let Some(gateway) = data.get("gateway").and_then(|v| v.as_object()) {
        if let Some(host) = gateway.get("host").and_then(|v| v.as_str()) {
            config["gateway"]["host"] = Value::String(host.to_string());
        }
        if let Some(port) = gateway.get("port").and_then(|v| v.as_i64()) {
            config["gateway"]["port"] = Value::Number(port.into());
        }
    }

    // Migrate old "tools.web.search" config to "tools.web.brave" (mirrors Go lines 256-272)
    if let Some(tools) = data.get("tools").and_then(|v| v.as_object()) {
        if let Some(web) = tools.get("web").and_then(|v| v.as_object()) {
            if let Some(search) = web.get("search").and_then(|v| v.as_object()) {
                // Ensure tools.web.brave exists in config
                if config["tools"].is_null() {
                    config["tools"] = serde_json::json!({});
                }
                if config["tools"]["web"].is_null() {
                    config["tools"]["web"] = serde_json::json!({});
                }
                if config["tools"]["web"]["brave"].is_null() {
                    config["tools"]["web"]["brave"] = serde_json::json!({});
                }

                if let Some(api_key) = search.get("api_key").and_then(|v| v.as_str()) {
                    config["tools"]["web"]["brave"]["api_key"] = Value::String(api_key.to_string());
                    if !api_key.is_empty() {
                        config["tools"]["web"]["brave"]["enabled"] = Value::Bool(true);
                    }
                }
                if let Some(max_results) = search.get("max_results").and_then(|v| v.as_i64()) {
                    config["tools"]["web"]["brave"]["max_results"] =
                        Value::Number(max_results.into());
                    // Also set DuckDuckGo max_results, matching Go behavior
                    if config["tools"]["web"]["duck_duck_go"].is_null() {
                        config["tools"]["web"]["duck_duck_go"] = serde_json::json!({});
                    }
                    config["tools"]["web"]["duck_duck_go"]["max_results"] =
                        Value::Number(max_results.into());
                }
            }
        }
    }

    (config, warnings)
}

/// Merge an incoming NemesisBot config into an existing one.
/// Models are added only if not present. Channels are merged (existing takes priority).
pub fn merge_config(existing: &mut Value, incoming: &Value) {
    // Merge model_list
    if let (Some(existing_models), Some(incoming_models)) = (
        existing
            .get_mut("model_list")
            .and_then(|v| v.as_array_mut()),
        incoming.get("model_list").and_then(|v| v.as_array()),
    ) {
        for model in incoming_models {
            let model_name = model.get("model_name").and_then(|v| v.as_str());
            let exists = existing_models
                .iter()
                .any(|m| m.get("model_name").and_then(|v| v.as_str()) == model_name);
            if !exists {
                existing_models.push(model.clone());
            }
        }
    }

    // Merge channels - incoming only fills in disabled channels
    if let (Some(existing_channels), Some(incoming_channels)) = (
        existing.get_mut("channels").and_then(|v| v.as_object_mut()),
        incoming.get("channels").and_then(|v| v.as_object()),
    ) {
        for (name, incoming_ch) in incoming_channels {
            if let Some(existing_ch) = existing_channels.get(name) {
                let existing_enabled = existing_ch
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if !existing_enabled {
                    existing_channels.insert(name.clone(), incoming_ch.clone());
                }
            }
        }
    }
}

/// Infer provider name from model name.
fn infer_provider_from_model(model: &str) -> String {
    let m = model.to_lowercase();
    if m.contains("claude") {
        return "anthropic".to_string();
    }
    if m.contains("gpt") {
        return "openai".to_string();
    }
    if m.contains("llama") {
        return "groq".to_string();
    }
    if m.contains("gemini") {
        return "gemini".to_string();
    }
    if m.contains("glm") {
        return "zhipu".to_string();
    }
    String::new()
}

/// Convert all JSON object keys from camelCase to snake_case.
fn convert_keys_to_snake(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let converted: serde_json::Map<String, Value> = map
                .iter()
                .map(|(k, v)| (camel_to_snake(k), convert_keys_to_snake(v)))
                .collect();
            Value::Object(converted)
        }
        Value::Array(arr) => Value::Array(arr.iter().map(convert_keys_to_snake).collect()),
        other => other.clone(),
    }
}

/// Convert a camelCase string to snake_case.
fn camel_to_snake(s: &str) -> String {
    let mut result = String::with_capacity(s.len() + 4);
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                let prev = s.chars().nth(i - 1).unwrap();
                if prev.is_lowercase() || prev.is_ascii_digit() {
                    result.push('_');
                } else if prev.is_uppercase() && i + 1 < s.len() {
                    let next = s.chars().nth(i + 1).unwrap();
                    if next.is_lowercase() {
                        result.push('_');
                    }
                }
            }
            result.extend(c.to_lowercase());
        } else {
            result.push(c);
        }
    }
    result
}

#[cfg(test)]
mod tests;
