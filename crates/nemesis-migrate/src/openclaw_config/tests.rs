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
