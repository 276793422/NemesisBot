//! OpenClaw config migration: load, convert, and merge OpenClaw configs to NemesisBot format.
//!
//! Mirrors Go migrate/config.go.

use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

/// Supported providers for OpenClaw -> NemesisBot migration.
const SUPPORTED_PROVIDERS: &[&str] = &[
    "anthropic", "openai", "openrouter", "groq", "zhipu", "vllm", "gemini",
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
    let raw: Value = serde_json::from_str(&data)
        .map_err(|e| format!("parsing OpenClaw config: {}", e))?;
    let converted = convert_keys_to_snake(&raw);
    let result = converted.as_object()
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
                "max_tool_iterations": 20,
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
            if let Some(max_tool_iter) = defaults.get("max_tool_iterations").and_then(|v| v.as_i64()) {
                config["agents"]["defaults"]["max_tool_iterations"] = Value::Number(max_tool_iter.into());
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
                    warnings.push(format!("Provider '{}' not supported in NemesisBot, skipping", name));
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
                    let exists = model_list.iter().any(|m| {
                        m.get("model_name").and_then(|v| v.as_str()) == Some(model_name)
                    });
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
                warnings.push(format!("Channel '{}' not supported in NemesisBot, skipping", name));
                continue;
            }

            let enabled = c_map.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);

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
                    config["tools"]["web"]["brave"]["max_results"] = Value::Number(max_results.into());
                    // Also set DuckDuckGo max_results, matching Go behavior
                    if config["tools"]["web"]["duck_duck_go"].is_null() {
                        config["tools"]["web"]["duck_duck_go"] = serde_json::json!({});
                    }
                    config["tools"]["web"]["duck_duck_go"]["max_results"] = Value::Number(max_results.into());
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
        existing.get_mut("model_list").and_then(|v| v.as_array_mut()),
        incoming.get("model_list").and_then(|v| v.as_array()),
    ) {
        for model in incoming_models {
            let model_name = model.get("model_name").and_then(|v| v.as_str());
            let exists = existing_models.iter().any(|m| {
                m.get("model_name").and_then(|v| v.as_str()) == model_name
            });
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
                let existing_enabled = existing_ch.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
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
    if m.contains("claude") { return "anthropic".to_string(); }
    if m.contains("gpt") { return "openai".to_string(); }
    if m.contains("llama") { return "groq".to_string(); }
    if m.contains("gemini") { return "gemini".to_string(); }
    if m.contains("glm") { return "zhipu".to_string(); }
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
        Value::Array(arr) => {
            Value::Array(arr.iter().map(convert_keys_to_snake).collect())
        }
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
mod tests {
    use super::*;

    #[test]
    fn test_camel_to_snake() {
        assert_eq!(camel_to_snake("apiKey"), "api_key");
        assert_eq!(camel_to_snake("bridgeURL"), "bridge_url");
        assert_eq!(camel_to_snake("maxToolIterations"), "max_tool_iterations");
        assert_eq!(camel_to_snake("already_snake"), "already_snake");
        assert_eq!(camel_to_snake("HTMLParser"), "html_parser");
    }

    #[test]
    fn test_convert_keys_to_snake() {
        let input = serde_json::json!({"apiKey": "test", "nested": {"innerKey": 1}});
        let output = convert_keys_to_snake(&input);
        assert_eq!(output["api_key"], "test");
        assert_eq!(output["nested"]["inner_key"], 1);
    }

    #[test]
    fn test_infer_provider() {
        assert_eq!(infer_provider_from_model("claude-3"), "anthropic");
        assert_eq!(infer_provider_from_model("gpt-4"), "openai");
        assert_eq!(infer_provider_from_model("glm-4"), "zhipu");
        assert_eq!(infer_provider_from_model("unknown"), "");
    }

    #[test]
    fn test_convert_config_basic() {
        let mut data = HashMap::new();
        data.insert("agents".to_string(), serde_json::json!({
            "defaults": {"llm": "zhipu/glm-4.7-flash", "max_tokens": 4096}
        }));
        let (config, warnings) = convert_config(&data);
        assert!(warnings.is_empty());
        assert_eq!(config["agents"]["defaults"]["max_tokens"], 4096);
    }

    #[test]
    fn test_merge_config_models() {
        let mut existing = serde_json::json!({
            "model_list": [{"model_name": "gpt-4", "model": "openai/gpt-4"}],
            "channels": {}
        });
        let incoming = serde_json::json!({
            "model_list": [{"model_name": "claude", "model": "anthropic/claude"}],
            "channels": {}
        });
        merge_config(&mut existing, &incoming);
        let models = existing["model_list"].as_array().unwrap();
        assert_eq!(models.len(), 2);
    }

    #[test]
    fn test_merge_config_no_duplicate() {
        let mut existing = serde_json::json!({
            "model_list": [{"model_name": "gpt-4"}],
            "channels": {}
        });
        let incoming = serde_json::json!({
            "model_list": [{"model_name": "gpt-4"}],
            "channels": {}
        });
        merge_config(&mut existing, &incoming);
        let models = existing["model_list"].as_array().unwrap();
        assert_eq!(models.len(), 1);
    }

    #[test]
    fn test_find_openclaw_config_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let result = find_openclaw_config(dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn test_find_openclaw_config_found() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("openclaw.json"), "{}").unwrap();
        let result = find_openclaw_config(dir.path());
        assert!(result.is_ok());
    }

    // ============================================================
    // Additional tests for missing coverage
    // ============================================================

    #[test]
    fn test_convert_config_with_providers() {
        let mut data = HashMap::new();
        data.insert("providers".to_string(), serde_json::json!({
            "anthropic": {"api_key": "sk-test", "api_base": "https://api.anthropic.com"},
            "openai": {"api_key": "sk-openai", "api_base": ""}
        }));
        let (config, warnings) = convert_config(&data);
        assert!(warnings.is_empty());
        let models = config["model_list"].as_array().unwrap();
        assert!(!models.is_empty());
        // Should have at least anthropic and openai models
        assert!(models.iter().any(|m| m["model_name"] == "claude-sonnet-4"));
        assert!(models.iter().any(|m| m["model_name"] == "gpt-4o"));
    }

    #[test]
    fn test_convert_config_unsupported_provider_warning() {
        let mut data = HashMap::new();
        data.insert("providers".to_string(), serde_json::json!({
            "unknown_provider": {"api_key": "sk-test", "api_base": "https://example.com"}
        }));
        let (_, warnings) = convert_config(&data);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("not supported"));
    }

    #[test]
    fn test_convert_config_unsupported_provider_no_creds_no_warning() {
        let mut data = HashMap::new();
        data.insert("providers".to_string(), serde_json::json!({
            "unknown_provider": {"api_key": "", "api_base": ""}
        }));
        let (_, warnings) = convert_config(&data);
        assert!(warnings.is_empty());
    }

    #[test]
    fn test_convert_config_with_channels() {
        let mut data = HashMap::new();
        data.insert("channels".to_string(), serde_json::json!({
            "telegram": {"enabled": true, "token": "12345"},
            "discord": {"enabled": false}
        }));
        let (config, warnings) = convert_config(&data);
        assert!(warnings.is_empty());
        assert_eq!(config["channels"]["telegram"]["enabled"], true);
        assert_eq!(config["channels"]["discord"]["enabled"], false);
    }

    #[test]
    fn test_convert_config_unsupported_channel_warning() {
        let mut data = HashMap::new();
        data.insert("channels".to_string(), serde_json::json!({
            "slack": {"enabled": true}
        }));
        let (_, warnings) = convert_config(&data);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("not supported"));
    }

    #[test]
    fn test_convert_config_with_gateway() {
        let mut data = HashMap::new();
        data.insert("gateway".to_string(), serde_json::json!({
            "host": "127.0.0.1",
            "port": 9090
        }));
        let (config, _) = convert_config(&data);
        assert_eq!(config["gateway"]["host"], "127.0.0.1");
        assert_eq!(config["gateway"]["port"], 9090);
    }

    #[test]
    fn test_convert_config_tools_web_search_brave_migration() {
        let mut data = HashMap::new();
        data.insert("tools".to_string(), serde_json::json!({
            "web": {
                "search": {
                    "api_key": "brave-key-123",
                    "max_results": 10
                }
            }
        }));
        let (config, _) = convert_config(&data);
        assert_eq!(config["tools"]["web"]["brave"]["api_key"], "brave-key-123");
        assert_eq!(config["tools"]["web"]["brave"]["enabled"], true);
        assert_eq!(config["tools"]["web"]["brave"]["max_results"], 10);
    }

    #[test]
    fn test_convert_config_tools_web_search_empty_key() {
        let mut data = HashMap::new();
        data.insert("tools".to_string(), serde_json::json!({
            "web": {
                "search": {
                    "api_key": "",
                    "max_results": 5
                }
            }
        }));
        let (config, _) = convert_config(&data);
        assert_eq!(config["tools"]["web"]["brave"]["api_key"], "");
        // Empty key should NOT set enabled=true
        assert!(config["tools"]["web"]["brave"].get("enabled").is_none());
    }

    #[test]
    fn test_convert_config_model_field_in_agents() {
        let mut data = HashMap::new();
        data.insert("agents".to_string(), serde_json::json!({
            "defaults": {"model": "gpt-4o-mini"}
        }));
        let (config, _) = convert_config(&data);
        let llm = config["agents"]["defaults"]["llm"].as_str().unwrap();
        assert!(llm.contains("openai/gpt-4o-mini"));
    }

    #[test]
    fn test_convert_config_model_field_unknown() {
        let mut data = HashMap::new();
        data.insert("agents".to_string(), serde_json::json!({
            "defaults": {"model": "custom-model"}
        }));
        let (config, _) = convert_config(&data);
        let llm = config["agents"]["defaults"]["llm"].as_str().unwrap();
        assert_eq!(llm, "zhipu/custom-model");
    }

    #[test]
    fn test_convert_config_workspace_rewrite() {
        let mut data = HashMap::new();
        data.insert("agents".to_string(), serde_json::json!({
            "defaults": {"workspace": "/home/user/.openclaw/workspace"}
        }));
        let (config, _) = convert_config(&data);
        let ws = config["agents"]["defaults"]["workspace"].as_str().unwrap();
        assert!(ws.contains(".nemesisbot"));
        assert!(!ws.contains(".openclaw"));
    }

    #[test]
    fn test_merge_config_channels() {
        let mut existing = serde_json::json!({
            "model_list": [],
            "channels": {
                "telegram": {"enabled": false, "token": "old"}
            }
        });
        let incoming = serde_json::json!({
            "model_list": [],
            "channels": {
                "telegram": {"enabled": true, "token": "new"}
            }
        });
        merge_config(&mut existing, &incoming);
        // Existing has enabled=false, so incoming should replace
        assert_eq!(existing["channels"]["telegram"]["enabled"], true);
    }

    #[test]
    fn test_merge_config_channels_existing_enabled() {
        let mut existing = serde_json::json!({
            "model_list": [],
            "channels": {
                "telegram": {"enabled": true, "token": "old"}
            }
        });
        let incoming = serde_json::json!({
            "model_list": [],
            "channels": {
                "telegram": {"enabled": false, "token": "new"}
            }
        });
        merge_config(&mut existing, &incoming);
        // Existing has enabled=true, should NOT be replaced
        assert_eq!(existing["channels"]["telegram"]["enabled"], true);
    }

    #[test]
    fn test_merge_config_empty_model_list() {
        let mut existing = serde_json::json!({"model_list": [], "channels": {}});
        let incoming = serde_json::json!({
            "model_list": [{"model_name": "m1"}],
            "channels": {}
        });
        merge_config(&mut existing, &incoming);
        let models = existing["model_list"].as_array().unwrap();
        assert_eq!(models.len(), 1);
    }

    #[test]
    fn test_camel_to_snake_edge_cases() {
        assert_eq!(camel_to_snake(""), "");
        assert_eq!(camel_to_snake("a"), "a");
        assert_eq!(camel_to_snake("A"), "a");
        assert_eq!(camel_to_snake("ABC"), "abc");
        assert_eq!(camel_to_snake("APIKey"), "api_key");
        assert_eq!(camel_to_snake("simpleTest"), "simple_test");
        assert_eq!(camel_to_snake("number1Field"), "number1_field");
    }

    #[test]
    fn test_find_openclaw_config_fallback_config_json() {
        let dir = tempfile::tempdir().unwrap();
        // No openclaw.json, but config.json exists
        std::fs::write(dir.path().join("config.json"), "{}").unwrap();
        let result = find_openclaw_config(dir.path());
        assert!(result.is_ok());
        assert!(result.unwrap().contains("config.json"));
    }

    #[test]
    fn test_find_openclaw_config_prefers_openclaw_json() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("openclaw.json"), "{}").unwrap();
        std::fs::write(dir.path().join("config.json"), "{}").unwrap();
        let result = find_openclaw_config(dir.path());
        assert!(result.is_ok());
        assert!(result.unwrap().contains("openclaw.json"));
    }

    #[test]
    fn test_load_openclaw_config_valid() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("test.json");
        std::fs::write(&config_path, r#"{"apiKey": "test123", "maxTokens": 100}"#).unwrap();
        let result = load_openclaw_config(&config_path);
        assert!(result.is_ok());
        let data = result.unwrap();
        // Keys should be converted to snake_case
        assert!(data.contains_key("api_key"));
        assert!(data.contains_key("max_tokens"));
    }

    #[test]
    fn test_load_openclaw_config_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("bad.json");
        std::fs::write(&config_path, "not json").unwrap();
        let result = load_openclaw_config(&config_path);
        assert!(result.is_err());
    }

    #[test]
    fn test_load_openclaw_config_missing_file() {
        let result = load_openclaw_config(Path::new("/nonexistent/config.json"));
        assert!(result.is_err());
    }

    #[test]
    fn test_infer_provider_additional_models() {
        assert_eq!(infer_provider_from_model("llama-3"), "groq");
        assert_eq!(infer_provider_from_model("gemini-pro"), "gemini");
        assert_eq!(infer_provider_from_model("Claude-3"), "anthropic");
        assert_eq!(infer_provider_from_model("GPT-4"), "openai");
    }

    #[test]
    fn test_convert_config_all_supported_providers() {
        let mut data = HashMap::new();
        data.insert("providers".to_string(), serde_json::json!({
            "anthropic": {"api_key": "k1", "api_base": ""},
            "openai": {"api_key": "k2", "api_base": ""},
            "openrouter": {"api_key": "k3", "api_base": ""},
            "groq": {"api_key": "k4", "api_base": ""},
            "zhipu": {"api_key": "k5", "api_base": ""},
            "vllm": {"api_key": "", "api_base": "http://localhost:8000"},
            "gemini": {"api_key": "k7", "api_base": ""}
        }));
        let (config, warnings) = convert_config(&data);
        assert!(warnings.is_empty());
        let models = config["model_list"].as_array().unwrap();
        assert_eq!(models.len(), 7);
    }

    #[test]
    fn test_convert_config_duplicate_provider_not_added() {
        let mut data = HashMap::new();
        data.insert("providers".to_string(), serde_json::json!({
            "anthropic": {"api_key": "k1", "api_base": ""}
        }));
        // Convert twice with same provider
        let (config1, _) = convert_config(&data);
        let (config2, _) = convert_config(&data);
        // Both should have exactly 1 model entry for anthropic
        assert_eq!(config1["model_list"].as_array().unwrap().len(), 1);
        assert_eq!(config2["model_list"].as_array().unwrap().len(), 1);
    }

    // ---- Additional coverage tests for 95%+ ----

    #[test]
    fn test_infer_provider_all_known() {
        assert_eq!(infer_provider_from_model("claude-3-opus"), "anthropic");
        assert_eq!(infer_provider_from_model("gpt-4o"), "openai");
        assert_eq!(infer_provider_from_model("glm-4-plus"), "zhipu");
        assert_eq!(infer_provider_from_model("llama-3"), "groq");
        assert_eq!(infer_provider_from_model("gemini-pro"), "gemini");
        assert_eq!(infer_provider_from_model("unknown-model"), "");
    }

    #[test]
    fn test_convert_keys_to_snake_array() {
        let input = serde_json::json!([{"myKey": 1}, {"otherKey": 2}]);
        let output = convert_keys_to_snake(&input);
        assert!(output.is_array());
        let arr = output.as_array().unwrap();
        assert_eq!(arr[0]["my_key"], 1);
        assert_eq!(arr[1]["other_key"], 2);
    }

    #[test]
    fn test_convert_config_with_channels_v2() {
        let mut data = HashMap::new();
        data.insert("channels".to_string(), serde_json::json!({
            "web": {"enabled": true, "host": "0.0.0.0", "port": 8080},
            "telegram": {"enabled": false, "token": ""}
        }));
        let (config, _warnings) = convert_config(&data);
        assert!(config["channels"].is_object());
    }

    #[test]
    fn test_convert_config_with_security() {
        let mut data = HashMap::new();
        data.insert("security".to_string(), serde_json::json!({
            "enabled": true,
            "restrict_to_workspace": true
        }));
        let (config, warnings) = convert_config(&data);
        // convert_config may or may not pass through security key
        // just verify it doesn't panic and returns valid output
        assert!(config.is_object());
    }

    #[test]
    fn test_merge_config_channels_merge() {
        let mut existing = serde_json::json!({
            "model_list": [],
            "channels": {
                "web": {"enabled": true},
                "telegram": {"enabled": false}
            }
        });
        let incoming = serde_json::json!({
            "model_list": [],
            "channels": {"telegram": {"enabled": true, "token": "123"}}
        });
        merge_config(&mut existing, &incoming);
        let channels = existing["channels"].as_object().unwrap();
        // Telegram was disabled in existing, so it gets updated from incoming
        assert_eq!(channels["telegram"]["token"], "123");
    }

    #[test]
    fn test_find_openclaw_config_with_subdir() {
        let dir = tempfile::tempdir().unwrap();
        // The function looks in openclaw_home directly, not in subdirs
        std::fs::write(dir.path().join("config.json"), "{}").unwrap();
        let result = find_openclaw_config(dir.path());
        assert!(result.is_ok());
        assert!(result.unwrap().contains("config.json"));
    }
}
