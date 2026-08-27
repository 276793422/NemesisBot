use std::fs;

#[test]
fn test_model_add_and_list() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    fs::create_dir_all(&home).unwrap();
    let cfg_path = home.join("config.json");
    let cfg = serde_json::json!({"model_list": [], "default_model": ""});
    fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();

    // Simulate add
    let data = fs::read_to_string(&cfg_path).unwrap();
    let mut config: serde_json::Value = serde_json::from_str(&data).unwrap();
    let entry = serde_json::json!({"model": "test/model-1", "api_key": "test-key", "proxy": "http://proxy:8080", "auth_method": "token"});
    if let Some(obj) = config.as_object_mut() {
        if let Some(models) = obj.get_mut("model_list") {
            if let Some(arr) = models.as_array_mut() {
                arr.push(entry);
            }
        }
        obj.insert(
            "default_model".to_string(),
            serde_json::Value::String("test/model-1".to_string()),
        );
    }
    fs::write(&cfg_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();

    // Verify
    let loaded: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(
        loaded.get("default_model").and_then(|v| v.as_str()),
        Some("test/model-1")
    );
    let models = loaded.get("model_list").unwrap().as_array().unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(
        models[0].get("proxy").and_then(|v| v.as_str()),
        Some("http://proxy:8080")
    );
    assert_eq!(
        models[0].get("auth_method").and_then(|v| v.as_str()),
        Some("token")
    );
}

#[test]
fn test_model_remove_default_protection() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cfg = serde_json::json!({"model_list": [{"model": "test/model-1"}], "default_model": "test/model-1"});
    let cfg_path = tmp.path().join("config.json");
    fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();

    let loaded: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&cfg_path).unwrap()).unwrap();
    let default = loaded
        .get("default_model")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_eq!(default, "test/model-1");
    // Default model should be protected (not removed without --force)
}

// -------------------------------------------------------------------------
// Model identifier format validation tests
// -------------------------------------------------------------------------

#[test]
fn test_model_format_vendor_model() {
    // Simulate the validation logic from the run() function
    let model = "openai/gpt-4o";
    assert!(model.contains('/'));
    let parts: Vec<&str> = model.splitn(2, '/').collect();
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0], "openai");
    assert_eq!(parts[1], "gpt-4o");
}

#[test]
fn test_model_format_no_slash() {
    let model = "noslashmodel";
    assert!(!model.contains('/'));
}

#[test]
fn test_model_format_with_multiple_slashes() {
    let model = "org/sub/model";
    assert!(model.contains('/'));
    let parts: Vec<&str> = model.splitn(2, '/').collect();
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0], "org");
    assert_eq!(parts[1], "sub/model");
}

#[test]
fn test_model_alias_extraction() {
    // Matches logic in run(): let alias = model.split('/').next_back().unwrap_or(&model)
    let model = "openai/gpt-4o";
    let alias = model.split('/').next_back().unwrap_or(model);
    assert_eq!(alias, "gpt-4o");
}

#[test]
fn test_model_alias_extraction_no_slash() {
    let model = "localmodel";
    let alias = model.split('/').next_back().unwrap_or(model);
    assert_eq!(alias, "localmodel");
}

// -------------------------------------------------------------------------
// Model list JSON parsing tests
// -------------------------------------------------------------------------

#[test]
fn test_model_list_default_from_agents() {
    let cfg = serde_json::json!({
        "agents": {
            "defaults": {
                "llm": "gpt-4o"
            }
        },
        "default_model": "legacy-model"
    });
    // agents.defaults.llm takes priority
    let default = cfg
        .get("agents")
        .and_then(|a| a.get("defaults"))
        .and_then(|d| d.get("llm"))
        .and_then(|v| v.as_str())
        .or_else(|| cfg.get("default_model").and_then(|v| v.as_str()))
        .unwrap_or("(none)");
    assert_eq!(default, "gpt-4o");
}

#[test]
fn test_model_list_default_fallback() {
    let cfg = serde_json::json!({
        "default_model": "legacy-model"
    });
    // Falls back to default_model when agents.defaults.llm is absent
    let default = cfg
        .get("agents")
        .and_then(|a| a.get("defaults"))
        .and_then(|d| d.get("llm"))
        .and_then(|v| v.as_str())
        .or_else(|| cfg.get("default_model").and_then(|v| v.as_str()))
        .unwrap_or("(none)");
    assert_eq!(default, "legacy-model");
}

#[test]
fn test_model_list_no_default() {
    let cfg = serde_json::json!({});
    let default = cfg
        .get("agents")
        .and_then(|a| a.get("defaults"))
        .and_then(|d| d.get("llm"))
        .and_then(|v| v.as_str())
        .or_else(|| cfg.get("default_model").and_then(|v| v.as_str()))
        .unwrap_or("(none)");
    assert_eq!(default, "(none)");
}

// -------------------------------------------------------------------------
// Model entry manipulation tests
// -------------------------------------------------------------------------

#[test]
fn test_model_entry_duplicate_detection() {
    let arr: Vec<serde_json::Value> =
        vec![serde_json::json!({"model": "openai/gpt-4o", "model_name": "gpt-4o"})];
    let model = "openai/gpt-4o";
    let existing = arr
        .iter()
        .find(|m| m.get("model").and_then(|v| v.as_str()) == Some(model));
    assert!(existing.is_some());
}

#[test]
fn test_model_entry_no_duplicate() {
    let arr: Vec<serde_json::Value> =
        vec![serde_json::json!({"model": "openai/gpt-4o", "model_name": "gpt-4o"})];
    let model = "anthropic/claude";
    let existing = arr
        .iter()
        .find(|m| m.get("model").and_then(|v| v.as_str()) == Some(model));
    assert!(existing.is_none());
}

#[test]
fn test_model_entry_removal_by_model() {
    let mut arr: Vec<serde_json::Value> = vec![
        serde_json::json!({"model": "openai/gpt-4o"}),
        serde_json::json!({"model": "anthropic/claude"}),
    ];
    let name = "openai/gpt-4o";
    arr.retain(|m| m.get("model").and_then(|v| v.as_str()) != Some(name));
    assert_eq!(arr.len(), 1);
    assert_eq!(
        arr[0].get("model").and_then(|v| v.as_str()),
        Some("anthropic/claude")
    );
}

#[test]
fn test_model_entry_removal_by_suffix() {
    let mut arr: Vec<serde_json::Value> = vec![
        serde_json::json!({"model": "openai/gpt-4o"}),
        serde_json::json!({"model": "anthropic/claude"}),
    ];
    let name = "gpt-4o";
    arr.retain(|m| {
        let model = m.get("model").and_then(|v| v.as_str()).unwrap_or("");
        model != name && !model.ends_with(&format!("/{}", name))
    });
    assert_eq!(arr.len(), 1);
    assert_eq!(
        arr[0].get("model").and_then(|v| v.as_str()),
        Some("anthropic/claude")
    );
}

// -------------------------------------------------------------------------
// Default model setting tests (agents.defaults.llm)
// -------------------------------------------------------------------------

#[test]
fn test_set_default_model_in_config() {
    let mut cfg = serde_json::json!({});
    if let Some(obj) = cfg.as_object_mut() {
        let agents = obj.entry("agents").or_insert_with(|| serde_json::json!({}));
        if let Some(agents_obj) = agents.as_object_mut() {
            let defaults = agents_obj
                .entry("defaults")
                .or_insert_with(|| serde_json::json!({}));
            if let Some(defaults_obj) = defaults.as_object_mut() {
                defaults_obj.insert(
                    "llm".to_string(),
                    serde_json::Value::String("gpt-4o".to_string()),
                );
            }
        }
    }
    assert_eq!(
        cfg.get("agents")
            .and_then(|a| a.get("defaults"))
            .and_then(|d| d.get("llm"))
            .and_then(|v| v.as_str()),
        Some("gpt-4o")
    );
}

#[test]
fn test_auto_default_single_model() {
    let cfg = serde_json::json!({
        "model_list": [{"model": "openai/gpt-4o"}],
    });
    let model_count = cfg
        .get("model_list")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    let current_default = cfg
        .get("agents")
        .and_then(|a| a.get("defaults"))
        .and_then(|d| d.get("llm"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    assert_eq!(model_count, 1);
    assert!(current_default.is_empty());
    // Should auto-set as default
}

// -------------------------------------------------------------------------
// Model entry building tests
// -------------------------------------------------------------------------

#[test]
fn test_build_model_entry_basic() {
    let model = "openai/gpt-4o";
    let parts: Vec<&str> = model.splitn(2, '/').collect();
    let model_name_alias = match parts.len() {
        2 => parts[1].to_string(),
        _ => model.to_string(),
    };
    let entry = serde_json::json!({
        "model_name": model_name_alias,
        "model": model,
    });
    assert_eq!(
        entry.get("model_name").and_then(|v| v.as_str()),
        Some("gpt-4o")
    );
    assert_eq!(
        entry.get("model").and_then(|v| v.as_str()),
        Some("openai/gpt-4o")
    );
}

#[test]
fn test_build_model_entry_with_all_fields() {
    let model = "zhipu/glm-4.7";
    let key = Some("test-api-key");
    let base = Some("https://api.example.com/v1");
    let proxy = Some("http://proxy:8080");
    let auth = Some("oauth");

    let mut entry = serde_json::json!({
        "model_name": "glm-4.7",
        "model": model,
    });
    if let Some(k) = &key {
        entry["api_key"] = serde_json::Value::String(k.to_string());
    }
    if let Some(b) = &base {
        entry["api_base"] = serde_json::Value::String(b.to_string());
    }
    if let Some(p) = &proxy {
        entry["proxy"] = serde_json::Value::String(p.to_string());
    }
    if let Some(a) = &auth {
        entry["auth_method"] = serde_json::Value::String(a.to_string());
    }

    assert_eq!(
        entry.get("api_key").and_then(|v| v.as_str()),
        Some("test-api-key")
    );
    assert_eq!(
        entry.get("api_base").and_then(|v| v.as_str()),
        Some("https://api.example.com/v1")
    );
    assert_eq!(
        entry.get("proxy").and_then(|v| v.as_str()),
        Some("http://proxy:8080")
    );
    assert_eq!(
        entry.get("auth_method").and_then(|v| v.as_str()),
        Some("oauth")
    );
}

#[test]
fn test_build_model_entry_optional_fields_absent() {
    let mut entry = serde_json::json!({
        "model_name": "glm-4.7",
        "model": "zhipu/glm-4.7",
    });
    let key: Option<&str> = None;
    let base: Option<&str> = None;
    if let Some(k) = &key {
        entry["api_key"] = serde_json::Value::String(k.to_string());
    }
    if let Some(b) = &base {
        entry["api_base"] = serde_json::Value::String(b.to_string());
    }
    assert!(entry.get("api_key").is_none());
    assert!(entry.get("api_base").is_none());
}

// -------------------------------------------------------------------------
// Model is_default check tests
// -------------------------------------------------------------------------

#[test]
fn test_model_is_default_check_by_model_name() {
    let default_model = "gpt-4o";
    let model_entry = serde_json::json!({"model": "openai/gpt-4o", "model_name": "gpt-4o"});
    let model = model_entry
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    let model_name = model_entry
        .get("model_name")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let is_default = model == default_model || model_name == default_model;
    assert!(is_default);
}

#[test]
fn test_model_is_default_check_by_full_identifier() {
    let default_model = "openai/gpt-4o";
    let model_entry = serde_json::json!({"model": "openai/gpt-4o", "model_name": "gpt-4o"});
    let model = model_entry
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    let model_name = model_entry
        .get("model_name")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let is_default = model == default_model || model_name == default_model;
    assert!(is_default);
}

#[test]
fn test_model_is_not_default() {
    let default_model = "claude";
    let model_entry = serde_json::json!({"model": "openai/gpt-4o", "model_name": "gpt-4o"});
    let model = model_entry
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    let model_name = model_entry
        .get("model_name")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let is_default = model == default_model || model_name == default_model;
    assert!(!is_default);
}

// -------------------------------------------------------------------------
// Model has_key detection test
// -------------------------------------------------------------------------

#[test]
fn test_model_has_api_key() {
    let m = serde_json::json!({"model": "openai/gpt-4o", "api_key": "sk-12345"});
    let has_key = m
        .get("api_key")
        .and_then(|v| v.as_str())
        .map(|k| !k.is_empty())
        .unwrap_or(false);
    assert!(has_key);
}

#[test]
fn test_model_empty_api_key() {
    let m = serde_json::json!({"model": "openai/gpt-4o", "api_key": ""});
    let has_key = m
        .get("api_key")
        .and_then(|v| v.as_str())
        .map(|k| !k.is_empty())
        .unwrap_or(false);
    assert!(!has_key);
}

#[test]
fn test_model_no_api_key() {
    let m = serde_json::json!({"model": "openai/gpt-4o"});
    let has_key = m
        .get("api_key")
        .and_then(|v| v.as_str())
        .map(|k| !k.is_empty())
        .unwrap_or(false);
    assert!(!has_key);
}

// -------------------------------------------------------------------------
// Model entry construction tests
// -------------------------------------------------------------------------

#[test]
fn test_model_entry_construction_full() {
    let model = "openai/gpt-4o";
    let key = Some("sk-12345".to_string());
    let base = Some("https://api.openai.com/v1".to_string());
    let proxy = Some("http://proxy:8080".to_string());
    let auth = Some("token".to_string());

    let mut entry = serde_json::json!({
        "model_name": model.splitn(2, '/').nth(1).unwrap_or(model),
        "model": model,
    });
    if let Some(k) = &key {
        entry["api_key"] = serde_json::Value::String(k.clone());
    }
    if let Some(b) = &base {
        entry["api_base"] = serde_json::Value::String(b.clone());
    }
    if let Some(p) = &proxy {
        entry["proxy"] = serde_json::Value::String(p.clone());
    }
    if let Some(a) = &auth {
        entry["auth_method"] = serde_json::Value::String(a.clone());
    }

    assert_eq!(entry["model_name"], "gpt-4o");
    assert_eq!(entry["model"], "openai/gpt-4o");
    assert_eq!(entry["api_key"], "sk-12345");
    assert_eq!(entry["api_base"], "https://api.openai.com/v1");
    assert_eq!(entry["proxy"], "http://proxy:8080");
    assert_eq!(entry["auth_method"], "token");
}

#[test]
fn test_model_entry_construction_minimal() {
    let model = "zhipu/glm-4.7";

    let entry = serde_json::json!({
        "model_name": model.splitn(2, '/').nth(1).unwrap_or(model),
        "model": model,
    });

    assert_eq!(entry["model_name"], "glm-4.7");
    assert_eq!(entry["model"], "zhipu/glm-4.7");
    assert!(entry.get("api_key").is_none());
    assert!(entry.get("api_base").is_none());
}

// -------------------------------------------------------------------------
// Default model detection via agents.defaults.llm
// -------------------------------------------------------------------------

#[test]
fn test_default_model_from_agents_defaults() {
    let cfg = serde_json::json!({
        "agents": {
            "defaults": {
                "llm": "gpt-4o"
            }
        },
        "default_model": "old-model"
    });

    let default_model = cfg
        .get("agents")
        .and_then(|a| a.get("defaults"))
        .and_then(|d| d.get("llm"))
        .and_then(|v| v.as_str())
        .or_else(|| cfg.get("default_model").and_then(|v| v.as_str()))
        .unwrap_or("(none)");

    assert_eq!(default_model, "gpt-4o");
}

#[test]
fn test_default_model_fallback_to_top_level() {
    let cfg = serde_json::json!({
        "default_model": "fallback-model"
    });

    let default_model = cfg
        .get("agents")
        .and_then(|a| a.get("defaults"))
        .and_then(|d| d.get("llm"))
        .and_then(|v| v.as_str())
        .or_else(|| cfg.get("default_model").and_then(|v| v.as_str()))
        .unwrap_or("(none)");

    assert_eq!(default_model, "fallback-model");
}

#[test]
fn test_default_model_none() {
    let cfg = serde_json::json!({});

    let default_model = cfg
        .get("agents")
        .and_then(|a| a.get("defaults"))
        .and_then(|d| d.get("llm"))
        .and_then(|v| v.as_str())
        .or_else(|| cfg.get("default_model").and_then(|v| v.as_str()));

    assert!(default_model.is_none());
}

// -------------------------------------------------------------------------
// Model name alias extraction
// -------------------------------------------------------------------------

#[test]
fn test_model_alias_extraction_v2() {
    let model = "openai/gpt-4o";
    let alias = model.split('/').next_back().unwrap_or(model).to_string();
    assert_eq!(alias, "gpt-4o");
}

#[test]
fn test_model_alias_no_slash() {
    let model = "gpt-4o";
    let alias = model.split('/').next_back().unwrap_or(model).to_string();
    assert_eq!(alias, "gpt-4o");
}

#[test]
fn test_model_alias_multiple_slashes() {
    let model = "org/sub/model-v1";
    let alias = model.split('/').next_back().unwrap_or(model).to_string();
    assert_eq!(alias, "model-v1");
}

// -------------------------------------------------------------------------
// Model removal matching logic
// -------------------------------------------------------------------------

#[test]
fn test_model_removal_match_by_full_name() {
    let name = "openai/gpt-4o";
    let model = "openai/gpt-4o";
    let matches = model == name || model.ends_with(&format!("/{}", name));
    assert!(matches);
}

#[test]
fn test_model_removal_match_by_short_name() {
    let name = "gpt-4o";
    let model = "openai/gpt-4o";
    let matches = model == name || model.ends_with(&format!("/{}", name));
    assert!(matches);
}

#[test]
fn test_model_removal_no_match() {
    let name = "claude";
    let model = "openai/gpt-4o";
    let matches = model == name || model.ends_with(&format!("/{}", name));
    assert!(!matches);
}

// -------------------------------------------------------------------------
// Model duplicate detection
// -------------------------------------------------------------------------

#[test]
fn test_model_duplicate_detection_found() {
    let model = "openai/gpt-4o";
    let models = serde_json::json!([
        {"model": "openai/gpt-4o"},
        {"model": "anthropic/claude"}
    ]);
    let arr = models.as_array().unwrap();
    let existing = arr
        .iter()
        .find(|m| m.get("model").and_then(|v| v.as_str()) == Some(model));
    assert!(existing.is_some());
}

#[test]
fn test_model_duplicate_detection_not_found() {
    let model = "google/gemini";
    let models = serde_json::json!([
        {"model": "openai/gpt-4o"},
        {"model": "anthropic/claude"}
    ]);
    let arr = models.as_array().unwrap();
    let existing = arr
        .iter()
        .find(|m| m.get("model").and_then(|v| v.as_str()) == Some(model));
    assert!(existing.is_none());
}

// -------------------------------------------------------------------------
// Auto-default logic
// -------------------------------------------------------------------------

#[test]
fn test_auto_default_single_model_v2() {
    let config = serde_json::json!({
        "model_list": [{"model": "openai/gpt-4o"}],
        "agents": {"defaults": {}}
    });
    let model_count = config
        .get("model_list")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    let current_default = config
        .get("agents")
        .and_then(|a| a.get("defaults"))
        .and_then(|d| d.get("llm"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let should_auto_default = model_count == 1 && current_default.is_empty();
    assert!(should_auto_default);
}

#[test]
fn test_no_auto_default_multiple_models() {
    let cfg = serde_json::json!({
        "model_list": [{"model": "openai/gpt-4o"}, {"model": "anthropic/claude"}],
        "agents": {"defaults": {}}
    });
    let model_count = cfg
        .get("model_list")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    let current_default = cfg
        .get("agents")
        .and_then(|a| a.get("defaults"))
        .and_then(|d| d.get("llm"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let should_auto_default = model_count == 1 && current_default.is_empty();
    assert!(!should_auto_default);
}

#[test]
fn test_no_auto_default_already_set() {
    let cfg = serde_json::json!({
        "model_list": [{"model": "openai/gpt-4o"}],
        "agents": {"defaults": {"llm": "gpt-4o"}}
    });
    let model_count = cfg
        .get("model_list")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    let current_default = cfg
        .get("agents")
        .and_then(|a| a.get("defaults"))
        .and_then(|d| d.get("llm"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let should_auto_default = model_count == 1 && current_default.is_empty();
    assert!(!should_auto_default);
}

// -------------------------------------------------------------------------
// Default model removal protection
// -------------------------------------------------------------------------

#[test]
fn test_is_default_by_agents_llm() {
    let cfg = serde_json::json!({
        "model_list": [{"model": "openai/gpt-4o", "model_name": "gpt-4o"}],
        "agents": {"defaults": {"llm": "gpt-4o"}}
    });
    let default_model = cfg
        .get("agents")
        .and_then(|a| a.get("defaults"))
        .and_then(|d| d.get("llm"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_eq!(default_model, "gpt-4o");

    let name = "openai/gpt-4o";
    let model_list = cfg["model_list"].as_array().unwrap();
    let is_default = model_list.iter().any(|m| {
        let full_model = m.get("model").and_then(|v| v.as_str()).unwrap_or("");
        let alias = m.get("model_name").and_then(|v| v.as_str()).unwrap_or("");
        (full_model == name || full_model.ends_with(&format!("/{}", name)))
            && (full_model == default_model || alias == default_model)
    });
    assert!(is_default);
}

// -------------------------------------------------------------------------
// Additional coverage tests for model
// -------------------------------------------------------------------------

#[test]
fn test_model_entry_with_custom_name() {
    let model = "openai/gpt-4o";
    let custom_name = Some("my-gpt4");
    let parts: Vec<&str> = model.splitn(2, '/').collect();
    let model_name = custom_name.unwrap_or_else(|| if parts.len() == 2 { parts[1] } else { model });
    assert_eq!(model_name, "my-gpt4");
}

#[test]
fn test_model_entry_default_name_from_provider() {
    let model = "openai/gpt-4o";
    let custom_name: Option<&str> = None;
    let parts: Vec<&str> = model.splitn(2, '/').collect();
    let model_name = custom_name.unwrap_or_else(|| if parts.len() == 2 { parts[1] } else { model });
    assert_eq!(model_name, "gpt-4o");
}

#[test]
fn test_model_entry_no_provider() {
    let model = "local-model";
    let parts: Vec<&str> = model.splitn(2, '/').collect();
    let has_provider = parts.len() == 2;
    assert!(!has_provider);
    let model_name = if has_provider { parts[1] } else { model };
    assert_eq!(model_name, "local-model");
}

#[test]
fn test_model_config_read_no_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("config.json");
    // Test the read logic inline
    if path.exists() {
        let data = std::fs::read_to_string(&path).unwrap();
        let cfg: serde_json::Value = serde_json::from_str(&data).unwrap();
        assert!(cfg.is_object());
    } else {
        // No file -> create default
        let cfg = serde_json::json!({"model_list": [], "agents": {}});
        assert!(cfg["model_list"].is_array());
    }
}

#[test]
fn test_model_config_read_with_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("config.json");
    let data = serde_json::json!({
        "model_list": [{"model": "test/model-1"}],
        "agents": {"defaults": {"llm": "model-1"}}
    });
    std::fs::write(&path, serde_json::to_string(&data).unwrap()).unwrap();

    let raw = std::fs::read_to_string(&path).unwrap();
    let cfg: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(cfg["model_list"].as_array().unwrap().len(), 1);
}

#[test]
fn test_model_config_save_creates_parent_dirs() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("nested").join("dir").join("config.json");
    let cfg = serde_json::json!({"model_list": []});
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, serde_json::to_string_pretty(&cfg).unwrap()).unwrap();
    assert!(path.exists());
}

#[test]
fn test_model_name_parsing_slash() {
    let full = "openai/gpt-4o";
    let parts: Vec<&str> = full.splitn(2, '/').collect();
    assert_eq!(parts[0], "openai");
    assert_eq!(parts[1], "gpt-4o");
}

#[test]
fn test_model_name_parsing_multiple_slashes() {
    let full = "provider/sub/model";
    let parts: Vec<&str> = full.splitn(2, '/').collect();
    assert_eq!(parts[0], "provider");
    assert_eq!(parts[1], "sub/model");
}

#[test]
fn test_model_name_parsing_no_slash() {
    let full = "local-model";
    let has_provider = full.contains('/');
    assert!(!has_provider);
}

#[test]
fn test_mask_api_key_short() {
    let key = "abc";
    let masked = if key.len() > 8 {
        format!("{}...{}", &key[..4], &key[key.len() - 4..])
    } else {
        "****".to_string()
    };
    assert_eq!(masked, "****");
}

#[test]
fn test_mask_api_key_long() {
    let key = "sk-1234567890abcdefghijklmnop";
    let masked = if key.len() > 8 {
        format!("{}...{}", &key[..4], &key[key.len() - 4..])
    } else {
        "****".to_string()
    };
    assert_eq!(masked, "sk-1...mnop");
}

#[test]
fn test_mask_api_key_empty() {
    let key = "";
    let masked = if key.len() > 8 {
        format!("{}...{}", &key[..4], &key[key.len() - 4..])
    } else {
        "****".to_string()
    };
    assert_eq!(masked, "****");
}

#[test]
fn test_model_list_add_and_find() {
    let mut cfg = serde_json::json!({"model_list": [], "agents": {}});
    let list = cfg["model_list"].as_array_mut().unwrap();

    let model1 = "test/model-1";
    let parts1: Vec<&str> = model1.splitn(2, '/').collect();
    list.push(serde_json::json!({
        "model_name": parts1.get(1).unwrap_or(&model1),
        "model": model1,
    }));

    let model2 = "test/model-2";
    let parts2: Vec<&str> = model2.splitn(2, '/').collect();
    list.push(serde_json::json!({
        "model_name": parts2.get(1).unwrap_or(&model2),
        "model": model2,
    }));

    assert_eq!(list.len(), 2);
    assert_eq!(list[0]["model"], "test/model-1");
    assert_eq!(list[1]["model"], "test/model-2");
}

#[test]
fn test_model_list_remove_by_index() {
    let mut cfg = serde_json::json!({"model_list": [
        {"model": "a/1"},
        {"model": "b/2"},
        {"model": "c/3"}
    ]});
    let list = cfg["model_list"].as_array_mut().unwrap();
    list.remove(1);
    assert_eq!(list.len(), 2);
    assert_eq!(list[0]["model"], "a/1");
    assert_eq!(list[1]["model"], "c/3");
}

#[test]
fn test_default_model_in_config() {
    let cfg = serde_json::json!({
        "model_list": [{"model": "openai/gpt-4o", "model_name": "gpt-4o"}],
        "agents": {"defaults": {"llm": "gpt-4o"}}
    });
    let default_model = cfg
        .get("agents")
        .and_then(|a| a.get("defaults"))
        .and_then(|d| d.get("llm"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_eq!(default_model, "gpt-4o");
}

#[test]
fn test_no_default_model_in_config() {
    let cfg = serde_json::json!({"model_list": []});
    let default_model = cfg
        .get("agents")
        .and_then(|a| a.get("defaults"))
        .and_then(|d| d.get("llm"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_eq!(default_model, "");
}

// H4 (U16 half): model set-effort writes/clears reasoning_effort in config
#[test]
fn test_model_set_effort_cli() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    fs::create_dir_all(&home).unwrap();
    let cfg_path = home.join("config.json");
    let cfg = serde_json::json!({"model_list": [
        {"model_name": "m1", "model": "test/m1"}
    ]});
    fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();

    // Simulate set-effort high (same config surgery the CLI handler does).
    let data = fs::read_to_string(&cfg_path).unwrap();
    let mut config: serde_json::Value = serde_json::from_str(&data).unwrap();
    let updated = crate::commands::model::update_model_entry_for_test(&mut config, "m1", |e| {
        e["reasoning_effort"] = serde_json::Value::String("high".to_string());
    });
    assert!(updated);
    fs::write(&cfg_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();

    let loaded: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(
        loaded["model_list"][0]["reasoning_effort"],
        serde_json::json!("high")
    );

    // Simulate set-effort off (clear).
    let mut config: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&cfg_path).unwrap()).unwrap();
    crate::commands::model::update_model_entry_for_test(&mut config, "m1", |e| {
        e["reasoning_effort"] = serde_json::Value::String(String::new());
    });
    fs::write(&cfg_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
    let loaded: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(loaded["model_list"][0]["reasoning_effort"], serde_json::json!(""));
}

// =========================================================================
// S11b 覆盖率冲刺：run() 全 arm 端到端 + format_probe_report 直测
//
// 策略：NEMESISBOT_HOME 指向临时目录（resolve_home 优先级 2），run(action,
// false) 全程只读写临时 home。env set_var 是进程级 → 持 crate::
// GLOBAL_STATE_LOCK 串行（与 cluster/tests.rs 同款 TempHomeEnv 模式）。
// Probe/CatalogUpdate 只测 cfg-missing bail——真 LLM 调用/真网络属结构性豁免。
// =========================================================================

/// RAII 守卫：设置 NEMESISBOT_HOME 指向临时根，drop 时移除。
struct S11bTempHomeEnv {
    _tmp: tempfile::TempDir,
    home: std::path::PathBuf,
}

impl Drop for S11bTempHomeEnv {
    fn drop(&mut self) {
        unsafe { std::env::remove_var("NEMESISBOT_HOME") };
    }
}

fn s11b_temp_home_env() -> S11bTempHomeEnv {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    fs::create_dir_all(&home).unwrap();
    unsafe { std::env::set_var("NEMESISBOT_HOME", tmp.path()) };
    S11bTempHomeEnv { _tmp: tmp, home }
}

fn s11b_write_cfg(home: &std::path::Path, cfg: serde_json::Value) {
    fs::write(home.join("config.json"), serde_json::to_string(&cfg).unwrap()).unwrap();
}

fn s11b_read_cfg(home: &std::path::Path) -> serde_json::Value {
    serde_json::from_str(&fs::read_to_string(home.join("config.json")).unwrap()).unwrap()
}

#[tokio::test]
async fn test_s11b_run_add_no_config_bails() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let _th = s11b_temp_home_env();
    let err = super::run(
        super::ModelAction::Add {
            model: "zhipu/glm-4.7".into(),
            key: None,
            base: None,
            proxy: None,
            auth: None,
            default: false,
        },
        false,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("Configuration not found"));
}

#[tokio::test]
async fn test_s11b_run_add_invalid_format_bails() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = s11b_temp_home_env();
    s11b_write_cfg(&th.home, serde_json::json!({"model_list": []}));
    let err = super::run(
        super::ModelAction::Add {
            model: "glm-4.7".into(), // 无 vendor 前缀
            key: None,
            base: None,
            proxy: None,
            auth: None,
            default: false,
        },
        false,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("Invalid model identifier"));
    // 失败路径不落盘任何条目
    assert!(s11b_read_cfg(&th.home)["model_list"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_s11b_run_add_basic_writes_entry() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = s11b_temp_home_env();
    s11b_write_cfg(&th.home, serde_json::json!({"model_list": []}));
    super::run(
        super::ModelAction::Add {
            model: "zhipu/glm-4.7".into(),
            key: Some("sk-test".into()),
            base: None,
            proxy: None,
            auth: None,
            default: false,
        },
        false,
    )
    .await
    .unwrap();
    let cfg = s11b_read_cfg(&th.home);
    let arr = cfg["model_list"].as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["model"], "zhipu/glm-4.7");
    assert_eq!(arr[0]["model_name"], "glm-4.7", "alias 是斜杠后半段");
    assert_eq!(arr[0]["api_key"], "sk-test");
    assert_eq!(arr[0]["model_tier"], "auto", "Phase 4a 自动打 auto 档");
    // 唯一模型 + 无默认 → 自动设默认
    assert_eq!(cfg["agents"]["defaults"]["llm"], "glm-4.7");
}

#[tokio::test]
async fn test_s11b_run_add_full_fields() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = s11b_temp_home_env();
    s11b_write_cfg(&th.home, serde_json::json!({"model_list": []}));
    super::run(
        super::ModelAction::Add {
            model: "openai/gpt-4o".into(),
            key: Some("k1".into()),
            base: Some("http://127.0.0.1:1/v1".into()),
            proxy: Some("http://127.0.0.1:1".into()),
            auth: Some("token".into()),
            default: false,
        },
        false,
    )
    .await
    .unwrap();
    let cfg = s11b_read_cfg(&th.home);
    let e = &cfg["model_list"][0];
    assert_eq!(e["api_base"], "http://127.0.0.1:1/v1");
    assert_eq!(e["proxy"], "http://127.0.0.1:1");
    assert_eq!(e["auth_method"], "token");
}

#[tokio::test]
async fn test_s11b_run_add_default_flag_sets_llm() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = s11b_temp_home_env();
    // 已有一个别的模型 + 已有默认 → --default 显式覆盖默认
    s11b_write_cfg(
        &th.home,
        serde_json::json!({
            "model_list": [{"model_name": "old", "model": "zhipu/old"}],
            "agents": {"defaults": {"llm": "old"}}
        }),
    );
    super::run(
        super::ModelAction::Add {
            model: "openai/gpt-4o".into(),
            key: None,
            base: None,
            proxy: None,
            auth: None,
            default: true,
        },
        false,
    )
    .await
    .unwrap();
    let cfg = s11b_read_cfg(&th.home);
    assert_eq!(cfg["model_list"].as_array().unwrap().len(), 2);
    assert_eq!(cfg["agents"]["defaults"]["llm"], "gpt-4o", "--default 写 alias");
}

#[tokio::test]
async fn test_s11b_run_add_auto_default_skipped_when_default_exists() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = s11b_temp_home_env();
    s11b_write_cfg(
        &th.home,
        serde_json::json!({
            "model_list": [],
            "agents": {"defaults": {"llm": "old"}}
        }),
    );
    super::run(
        super::ModelAction::Add {
            model: "openai/gpt-4o".into(),
            key: None,
            base: None,
            proxy: None,
            auth: None,
            default: false,
        },
        false,
    )
    .await
    .unwrap();
    // 已有默认 → 不自动改默认
    assert_eq!(s11b_read_cfg(&th.home)["agents"]["defaults"]["llm"], "old");
}

#[tokio::test]
async fn test_s11b_run_add_duplicate_replaces() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = s11b_temp_home_env();
    s11b_write_cfg(&th.home, serde_json::json!({"model_list": []}));
    for key in ["sk-one", "sk-two"] {
        super::run(
            super::ModelAction::Add {
                model: "zhipu/glm-4.7".into(),
                key: Some(key.into()),
                base: None,
                proxy: None,
                auth: None,
                default: false,
            },
            false,
        )
        .await
        .unwrap();
    }
    let cfg = s11b_read_cfg(&th.home);
    let arr = cfg["model_list"].as_array().unwrap();
    assert_eq!(arr.len(), 1, "同 model 重复 add 是替换不是追加");
    assert_eq!(arr[0]["api_key"], "sk-two");
}

#[tokio::test]
async fn test_s11b_run_add_catalog_hit_fills_context_window() {
    use crate::commands::model::catalog::{self, CatalogEntry};
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = s11b_temp_home_env();
    s11b_write_cfg(&th.home, serde_json::json!({"model_list": []}));
    catalog::save_cache(
        &th.home,
        vec![CatalogEntry {
            key: "testorg/coolmodel".into(),
            context_window: 123456,
            max_output_tokens: Some(8192),
            family: Some("cool".into()),
        }],
    )
    .unwrap();
    super::run(
        super::ModelAction::Add {
            model: "testorg/coolmodel".into(),
            key: None,
            base: None,
            proxy: None,
            auth: None,
            default: false,
        },
        false,
    )
    .await
    .unwrap();
    let e = &s11b_read_cfg(&th.home)["model_list"][0];
    assert_eq!(e["context_window"], 123456, "目录命中自动填 context_window");
    assert_eq!(e["max_output_tokens"], 8192);
    // 目录字段不透传到 config 条目
    assert!(e.get("family").is_none());
}

#[tokio::test]
async fn test_s11b_run_list_variants() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    // 无配置 → 打印提示后 Ok
    {
        let _th = s11b_temp_home_env();
        super::run(super::ModelAction::List { verbose: false }, false).await.unwrap();
    }
    // 空 model_list
    {
        let th = s11b_temp_home_env();
        s11b_write_cfg(&th.home, serde_json::json!({"model_list": []}));
        super::run(super::ModelAction::List { verbose: false }, false).await.unwrap();
    }
    // 有模型 + verbose（脱敏 bullet 输出）+ default 标记
    {
        let th = s11b_temp_home_env();
        s11b_write_cfg(
            &th.home,
            serde_json::json!({
                "model_list": [
                    {"model": "zhipu/glm-4.7", "model_name": "glm-4.7", "api_key": "sk-x"},
                    {"model": "openai/gpt-4o", "model_name": "gpt-4o"}
                ],
                "agents": {"defaults": {"llm": "glm-4.7"}}
            }),
        );
        super::run(super::ModelAction::List { verbose: true }, false).await.unwrap();
        super::run(super::ModelAction::List { verbose: false }, false).await.unwrap();
    }
    // model_list 缺失（else 分支）
    {
        let th = s11b_temp_home_env();
        s11b_write_cfg(&th.home, serde_json::json!({}));
        super::run(super::ModelAction::List { verbose: false }, false).await.unwrap();
    }
}

#[tokio::test]
async fn test_s11b_run_remove_no_config_bails() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let _th = s11b_temp_home_env();
    let err = super::run(
        super::ModelAction::Remove { name: "x".into(), force: true },
        false,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("Configuration not found"));
}

#[tokio::test]
async fn test_s11b_run_remove_default_protected() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = s11b_temp_home_env();
    s11b_write_cfg(
        &th.home,
        serde_json::json!({
            "model_list": [{"model": "zhipu/glm-4.7", "model_name": "glm-4.7"}],
            "agents": {"defaults": {"llm": "glm-4.7"}}
        }),
    );
    // 默认模型（alias 命中）→ 拒绝删除，Ok 返回不 bails
    super::run(
        super::ModelAction::Remove { name: "glm-4.7".into(), force: true },
        false,
    )
    .await
    .unwrap();
    // 默认模型（全名命中）
    super::run(
        super::ModelAction::Remove { name: "zhipu/glm-4.7".into(), force: true },
        false,
    )
    .await
    .unwrap();
    assert_eq!(s11b_read_cfg(&th.home)["model_list"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn test_s11b_run_remove_force_full_name_and_alias() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = s11b_temp_home_env();
    s11b_write_cfg(
        &th.home,
        serde_json::json!({
            "model_list": [
                {"model": "zhipu/glm-4.7", "model_name": "glm-4.7"},
                {"model": "openai/gpt-4o", "model_name": "gpt-4o"}
            ]
        }),
    );
    // alias（suffix 匹配 vendor 斜杠 name）
    super::run(
        super::ModelAction::Remove { name: "glm-4.7".into(), force: true },
        false,
    )
    .await
    .unwrap();
    // 全名
    super::run(
        super::ModelAction::Remove { name: "openai/gpt-4o".into(), force: true },
        false,
    )
    .await
    .unwrap();
    assert!(s11b_read_cfg(&th.home)["model_list"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_s11b_run_remove_not_found_bails() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = s11b_temp_home_env();
    s11b_write_cfg(&th.home, serde_json::json!({"model_list": []}));
    let err = super::run(
        super::ModelAction::Remove { name: "nope".into(), force: true },
        false,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("Model not found"));
}

#[tokio::test]
async fn test_s11b_run_default_arm() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    // 无配置 → Ok（打印提示）
    {
        let _th = s11b_temp_home_env();
        super::run(super::ModelAction::Default, false).await.unwrap();
    }
    // 无默认
    {
        let th = s11b_temp_home_env();
        s11b_write_cfg(&th.home, serde_json::json!({}));
        super::run(super::ModelAction::Default, false).await.unwrap();
    }
    // agents.defaults.llm 命中
    {
        let th = s11b_temp_home_env();
        s11b_write_cfg(
            &th.home,
            serde_json::json!({"agents": {"defaults": {"llm": "glm-4.7"}}}),
        );
        super::run(super::ModelAction::Default, false).await.unwrap();
    }
    // 兼容字段 default_model 回退
    {
        let th = s11b_temp_home_env();
        s11b_write_cfg(&th.home, serde_json::json!({"default_model": "legacy"}));
        super::run(super::ModelAction::Default, false).await.unwrap();
    }
}

#[tokio::test]
async fn test_s11b_run_settier_matrix() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = s11b_temp_home_env();
    s11b_write_cfg(
        &th.home,
        serde_json::json!({"model_list": [{"model": "zhipu/glm-4.7", "model_name": "glm-4.7"}]}),
    );
    // 非法 tier
    let err = super::run(
        super::ModelAction::SetTier { name: "glm-4.7".into(), tier: "huge".into() },
        false,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("Invalid tier"));
    // 合法 mini
    super::run(
        super::ModelAction::SetTier { name: "glm-4.7".into(), tier: "mini".into() },
        false,
    )
    .await
    .unwrap();
    assert_eq!(s11b_read_cfg(&th.home)["model_list"][0]["model_tier"], "mini");
    // 找不到模型
    let err = super::run(
        super::ModelAction::SetTier { name: "nope".into(), tier: "big".into() },
        false,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("Model not found"));
    // 无配置
    {
        let _th2 = s11b_temp_home_env();
        let err = super::run(
            super::ModelAction::SetTier { name: "x".into(), tier: "big".into() },
            false,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Configuration not found"));
    }
}

#[tokio::test]
async fn test_s11b_run_seteffort_matrix() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = s11b_temp_home_env();
    s11b_write_cfg(
        &th.home,
        serde_json::json!({"model_list": [{"model": "zhipu/glm-4.7", "model_name": "glm-4.7"}]}),
    );
    // 非法
    let err = super::run(
        super::ModelAction::SetEffort { name: "glm-4.7".into(), effort: "extreme".into() },
        false,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("Invalid effort"));
    // off → 清空
    super::run(
        super::ModelAction::SetEffort { name: "glm-4.7".into(), effort: "off".into() },
        false,
    )
    .await
    .unwrap();
    assert_eq!(s11b_read_cfg(&th.home)["model_list"][0]["reasoning_effort"], "");
    // high → 写入
    super::run(
        super::ModelAction::SetEffort { name: "glm-4.7".into(), effort: "high".into() },
        false,
    )
    .await
    .unwrap();
    assert_eq!(s11b_read_cfg(&th.home)["model_list"][0]["reasoning_effort"], "high");
    // 找不到
    let err = super::run(
        super::ModelAction::SetEffort { name: "nope".into(), effort: "low".into() },
        false,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("Model not found"));
}

#[tokio::test]
async fn test_s11b_run_setsize_matrix() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = s11b_temp_home_env();
    s11b_write_cfg(
        &th.home,
        serde_json::json!({"model_list": [{"model": "x/lite", "model_name": "lite"}]}),
    );
    // 非法
    let err = super::run(
        super::ModelAction::SetSize { name: "lite".into(), size: "huge".into() },
        false,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("Invalid size"));
    // 30B
    super::run(
        super::ModelAction::SetSize { name: "lite".into(), size: "30B".into() },
        false,
    )
    .await
    .unwrap();
    assert_eq!(s11b_read_cfg(&th.home)["model_list"][0]["model_size_b"], 30);
    // 裸数字
    super::run(
        super::ModelAction::SetSize { name: "lite".into(), size: "70".into() },
        false,
    )
    .await
    .unwrap();
    assert_eq!(s11b_read_cfg(&th.home)["model_list"][0]["model_size_b"], 70);
    // 找不到
    let err = super::run(
        super::ModelAction::SetSize { name: "nope".into(), size: "9b".into() },
        false,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("Model not found"));
}

#[tokio::test]
async fn test_s11b_run_setrealname_matrix() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = s11b_temp_home_env();
    s11b_write_cfg(
        &th.home,
        serde_json::json!({"model_list": [{"model": "x/opaque", "model_name": "opaque"}]}),
    );
    super::run(
        super::ModelAction::SetRealName {
            name: "opaque".into(),
            real_name: "Qwen3-30B-A3B".into(),
        },
        false,
    )
    .await
    .unwrap();
    assert_eq!(s11b_read_cfg(&th.home)["model_list"][0]["real_name"], "Qwen3-30B-A3B");
    let err = super::run(
        super::ModelAction::SetRealName { name: "nope".into(), real_name: "X".into() },
        false,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("Model not found"));
}

#[tokio::test]
async fn test_s11b_run_probe_no_config_bails() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let _th = s11b_temp_home_env();
    // cfg-missing bail 在 block_in_place 之前 → current_thread runtime 可跑。
    // 真 LLM 探针属结构性豁免（7 次真实模型调用）。
    let err = super::run(super::ModelAction::Probe { name: "glm-4.7".into() }, false)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("Configuration not found"));
}

#[tokio::test]
async fn test_s11b_run_catalog_update_no_config_bails() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let _th = s11b_temp_home_env();
    // cfg-missing bail；真网络拉取 models.dev 属结构性豁免。
    let err = super::run(super::ModelAction::CatalogUpdate, false).await.unwrap_err();
    assert!(err.to_string().contains("Configuration not found"));
}

#[test]
fn test_s11b_format_probe_report_direct() {
    use nemesis_agent::probe::{ProbeReport, ProbeScore};
    use nemesis_types::capability::ModelTier;
    let report = ProbeReport {
        format_score: 0.85,
        selection_score: 0.71,
        schema_score: 0.92,
        tier: ModelTier::Normal,
        per_task: vec![
            (
                "exec".to_string(),
                ProbeScore { format: 1.0, selection: 0.0, schema: 1.0 },
            ),
            (
                "grep".to_string(),
                ProbeScore { format: 0.5, selection: 1.0, schema: 0.5 },
            ),
        ],
    };
    let s = super::format_probe_report("glm-4.7", &report);
    assert!(s.contains("能力探针报告: glm-4.7"));
    assert!(s.contains("format=0.85"), "总分保留两位小数：{s}");
    assert!(s.contains("selection=0.71"));
    assert!(s.contains("schema=0.92"));
    assert!(s.contains("exec"), "每任务得分行：{s}");
    assert!(s.contains("grep"));
    assert!(s.contains("tier=normal"), "tier 结论行：{s}");
    // 空任务表也能格式化
    let empty = ProbeReport {
        format_score: 0.0,
        selection_score: 0.0,
        schema_score: 0.0,
        tier: ModelTier::Mini,
        per_task: vec![],
    };
    let s2 = super::format_probe_report("m", &empty);
    assert!(s2.contains("tier=mini"));
}

// ===========================================================================
// wave_b（coverage 补测，2026-08-27）：miss 行清零补洞。
//
// 目标行（model.rs）与本模块的对应关系：
//  - 195（config 无 model_list 键时的首次插入臂）+ 236-238（tinyllama 命中
//    small_markers → 非 big 提示 else 臂）→
//    wave_b_run_add_inserts_model_list_key_and_mini_hint；
//  - 333（非 verbose 也打印的 "Base URL:" 行）+ 337-355 verbose 全臂
//    （key 非空圆点遮蔽 / key 空串 "(not set)" / API Base / Proxy 非空 /
//    Auth Method 非空）→ wave_b_run_list_verbose_and_base_url_rows；
//  - 500 / 537 / 566（SetEffort / SetSize / SetRealName 在 config.json 缺席时
//    的三个 bail 臂）→ wave_b_run_setaffort_setsize_setrealname_bail_no_config；
//  - 676（update_model_entry 对未知模型返 false 的收尾臂；用闭包 panic-on-call
//    反证未误中条目）→ wave_b_update_model_entry_for_test_miss_and_hit。
//
// ARTIFACT：117 —— splitn(2,'/') 在已通过 `contains('/')` 校验后 parts.len()
// 恒为 2，`_ => model.clone()` 是编译器要求的穷尽臂，生产语义不可达。
//
// ALREADY（既有测试名证据，不重复覆盖）：168（catalog 命中回填 context_window）
// = test_s11b_run_add_catalog_hit_fills_context_window；199-220/243-282（default
// 落盘 + 基础写路径）= test_s11b_run_add_basic_writes* 系列；491/524/555/584
// （四个 mutate 成功后的落盘 ?行）= test_model_set_effort_cli +
// test_s11b_run_seteffort_matrix@1432 + test_s11b_run_setsize_matrix@1474 +
// test_s11b_run_setrealname_matrix@1516；594-596 = probe 缺配置 bail（S11b）。
//
// EXEMPT：
//  - 413-422 Remove 非 force 确认分支 —— 进程内直读真实 stdin（io::stdin），
//    cargo test 交互终端下会阻塞等待回车、CI 下读 EOF；同进程无法重定向 stdin，
//    而 spawn 子进程被测试纪律禁止 ⇒ 不存在确定性注入手段，宁缺勿挂死全量套件。
//  - 597-604 Probe 主体 + 609-646 CatalogUpdate 主体 —— run_probe 逐题调真实
//    LLM 端点、catalog::fetch_http 走真网络（reqwest），均属「真外部交互」纪律
//    豁免面；离线快失败/缓存保留语义由 catalog 单元级测试与历史 S11b 钉住。
// ===========================================================================

mod wave_b {
    use super::{s11b_read_cfg, s11b_temp_home_env, s11b_write_cfg};
    use super::super::{run, update_model_entry_for_test, ModelAction};

    #[tokio::test]
    async fn wave_b_run_add_inserts_model_list_key_and_mini_hint() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = s11b_temp_home_env();
        // 配置存在但没有 model_list 键 ⇒ 走 obj.insert 首插臂（194-196）。
        s11b_write_cfg(&th.home, serde_json::json!({}));

        run(
            ModelAction::Add {
                // tinyllama 在 capability small_markers 里 ⇒ detect_tier=Mini
                // ⇒ 提示打印走 else 臂（236-238），而非 big 建议。
                model: "tinyllama/local-1".into(),
                key: Some("k".into()),
                base: None,
                proxy: None,
                auth: None,
                default: false,
            },
            false,
        )
        .await
        .expect("add into keyless config must succeed");

        let cfg = s11b_read_cfg(&th.home);
        let list = cfg["model_list"].as_array().expect("model_list inserted");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["model"], "tinyllama/local-1");
        assert_eq!(list[0]["model_tier"], "auto");
    }

    #[tokio::test]
    async fn wave_b_run_list_verbose_and_base_url_rows() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = s11b_temp_home_env();
        s11b_write_cfg(
            &th.home,
            serde_json::json!({
                "agents": {"defaults": {"llm": "beta"}},
                "model_list": [
                    {"model_name": "alpha", "model": "prov/alpha",
                     "api_key": "secret", "api_base": "http://alpha.local/v1",
                     "proxy": "http://proxy:8080", "auth_method": "api_key"},
                    {"model_name": "beta", "model": "prov/beta",
                     "api_key": "", "api_base": "http://beta.local/v1"},
                    {"model_name": "gamma", "model": "prov/gamma"}
                ]
            }),
        );

        // verbose=true：alpha 打全部 verbose 行（遮蔽 key/Base/Proxy/Auth），
        // beta 走空 api_key 的 "(not set)" 分支并打 Base，gamma 三者皆缺省。
        run(ModelAction::List { verbose: true }, false)
            .await
            .expect("verbose list must succeed");
        // verbose=false：Base URL 行在本分支也照打（333 目标行的另一入口）。
        run(ModelAction::List { verbose: false }, false)
            .await
            .expect("plain list must succeed");
    }

    #[tokio::test]
    async fn wave_b_run_setaffort_setsize_setrealname_bail_no_config() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = s11b_temp_home_env(); // 故意不写 config.json

        let err = run(
            ModelAction::SetEffort { name: "m".into(), effort: "low".into() },
            false,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Configuration not found"));

        let err = run(ModelAction::SetSize { name: "m".into(), size: "30B".into() }, false)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Configuration not found"));

        let err = run(
            ModelAction::SetRealName { name: "m".into(), real_name: "Qwen3-30B".into() },
            false,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Configuration not found"));

        // 收尾一致性：三条路径都不应凭空创建配置文件。
        assert!(!th.home.join("config.json").exists());
    }

    #[test]
    fn wave_b_update_model_entry_for_test_miss_and_hit() {
        let mut cfg = serde_json::json!({
            "model_list": [{"model_name": "b", "model": "prov/b"}]
        });

        // 未知名：返回 false 且绝不触碰任何条目（闭包一旦被调用即 fail）。
        let mut closure_ran = false;
        let updated = update_model_entry_for_test(&mut cfg, "zzz", |_e| {
            closure_ran = true;
        });
        assert!(!updated, "unknown alias/full-id must yield false");
        assert!(!closure_ran, "closure must never run on miss");

        // 对照组：别名命中则突变生效。
        let updated = update_model_entry_for_test(&mut cfg, "b", |e| {
            e["model_tier"] = serde_json::Value::String("mini".into());
        });
        assert!(updated);
        assert_eq!(cfg["model_list"][0]["model_tier"], "mini");
    }
}

// ===========================================================================
// wave_c（coverage 补测，2026-08-27）：update_model_entry 对畸形形状的安全面。
// 目标行 676（`None => return false`）——cfg 缺 model_list 键 / model_list 非
// 数组两种形状都汇入该 None 臂，既有 miss_and_hit 只走了「有数组但查无此名」
// 的循环耗尽 false（686），676 从未被执行。顺带钉死 `{}` 空对象条目的匹配
// 语义：缺 model/model_name 字段时按空串参与比对 → 必须被安全跳过且原样
// 保留（不误命中、不加杂物字段），突变只落在真正命中的邻居上、原字段全保。
// ===========================================================================

mod wave_c {
    use super::super::update_model_entry_for_test;

    #[test]
    fn wave_c_no_usable_model_list_returns_false_without_mutation() {
        let mut called = false;
        // ① 整个 config 是 {} —— 没有 model_list 键。
        let mut empty_cfg = serde_json::json!({});
        assert!(!update_model_entry_for_test(&mut empty_cfg, "any", |_e| {
            called = true;
        }));
        assert_eq!(
            empty_cfg,
            serde_json::json!({}),
            "miss 路径不得给 config 凭空加键"
        );

        // ② model_list 不是数组（对象形状同样路由到 676 的 None 臂）。
        let mut non_array_cfg = serde_json::json!({"model_list": {"model": "prov/x"}});
        assert!(!update_model_entry_for_test(&mut non_array_cfg, "x", |_e| {
            called = true;
        }));

        assert!(!called, "两种畸形形状都必须在任何闭包调用之前返 false");
    }

    #[test]
    fn wave_c_empty_object_entry_skipped_and_preserved() {
        let mut cfg = serde_json::json!({
            "model_list": [
                {},
                {"model_name": "real", "model": "prov/real"}
            ]
        });

        let updated = update_model_entry_for_test(&mut cfg, "real", |e| {
            e["model_tier"] = serde_json::Value::String("mini".into());
        });
        assert!(updated);

        let list = cfg["model_list"].as_array().unwrap();
        assert_eq!(list.len(), 2, "不得增删任何条目");
        assert_eq!(
            list[0],
            serde_json::json!({}),
            "`{{}}` 空条目必须原样保留（字段缺失按空串参与匹配 → 不误命中）"
        );
        assert_eq!(list[1]["model_tier"], "mini", "突变落在正确邻居上");
        assert_eq!(list[1]["model"], "prov/real", "命中条目原有字段保留");
        assert_eq!(list[1]["model_name"], "real", "命中条目原有别名保留");
    }
}

// ===========================================================================
// r9_subprocess（R9 补测批零头组，2026-08-27）：子进程级真链路。
//
// 1) `model probe` 全链（run() Probe 臂 + run_probe + agent::probe::run）：
//    MockAiServer 按 probe_tasks() 的 FIFO 顺序回 7 个 schema 全对的
//    tool_call → 三轴满分 → tier=Big；断言 stdout 报告行 + config.json 的
//    model_tier 落盘。此前 692-758 行从未被执行过。
// 2) `model remove` y/N 双臂（415-455）：管道 stdin 喂 "y\n"（删）/ "\n"
//    （Aborted.）。run_cli_with_stdin 写完即 EOF，正是该 read_line 所需。
//
// 环境约束：--local 由 harness 前置（cwd 隔离），无需动 NEMESISBOT_HOME；
// 不碰 GLOBAL_STATE_LOCK（无 env/port 依赖——MockAiServer 用随机端口）。
// ===========================================================================

mod r9_subprocess {
    use test_harness::mock_ai::{MockAiReply, MockAiServer};
    use test_harness::{resolve_nemesisbot_bin, TestWorkspace};

    /// 按探针任务顺序给全对 tool_call：exec/read_file/create_dir/grep/
    /// write_file/edit_file/cluster_rpc，每个的 arguments 满足其 required。
    fn perfect_probe_script() -> Vec<MockAiReply> {
        vec![
            MockAiReply::ToolCall {
                name: "exec".into(),
                arguments: r#"{"command":"date"}"#.into(),
            },
            MockAiReply::ToolCall {
                name: "read_file".into(),
                arguments: r#"{"path":"README.md"}"#.into(),
            },
            MockAiReply::ToolCall {
                name: "create_dir".into(),
                arguments: r#"{"path":"test"}"#.into(),
            },
            MockAiReply::ToolCall {
                name: "grep".into(),
                arguments: r#"{"pattern":"TODO"}"#.into(),
            },
            MockAiReply::ToolCall {
                name: "write_file".into(),
                arguments: r#"{"path":"note.md","content":"hi"}"#.into(),
            },
            MockAiReply::ToolCall {
                name: "edit_file".into(),
                arguments: r#"{"path":"note.md","old_text":"hi","new_text":"yo"}"#.into(),
            },
            MockAiReply::ToolCall {
                name: "cluster_rpc".into(),
                arguments: r#"{"target_node":"node-b","message":"你好"}"#.into(),
            },
        ]
    }

    fn read_cfg(ws: &TestWorkspace) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(ws.config_path()).unwrap()).unwrap()
    }

    #[tokio::test]
    async fn probe_full_chain_writes_tier_big_via_mock_script() {
        let ws = TestWorkspace::new().expect("workspace");
        std::fs::create_dir_all(ws.home()).unwrap();
        // model add 要求 config.json 已存在（101 行 read_to_string）。
        std::fs::write(&ws.config_path(), "{}").unwrap();

        let mock = MockAiServer::start(perfect_probe_script()).expect("mock ai server");

        let bin = resolve_nemesisbot_bin().expect("release binary");
        let base = format!("{}/v1", mock.base_url());
        let add = ws
            .run_cli(
                &bin,
                &[
                    "model",
                    "add",
                    "--model",
                    "r9p/probe-x",
                    "--key",
                    "sk-r9",
                    "--base",
                    base.as_str(),
                ],
            )
            .await;
        assert!(
            add.success(),
            "model add 失败：stdout={} stderr={}",
            add.stdout,
            add.stderr
        );

        let probe = ws.run_cli_with_timeout(&bin, &["model", "probe", "r9p/probe-x"], 60).await;
        assert!(
            probe.success(),
            "probe 应全对满分：stdout={} stderr={}",
            probe.stdout,
            probe.stderr
        );
        assert!(probe.stdout_contains("能力探针报告: r9p/probe-x"));
        for task in [
            "exec",
            "read_file",
            "create_dir",
            "grep",
            "write_file",
            "edit_file",
            "cluster_rpc",
        ] {
            assert!(
                probe.stdout_contains(task),
                "每工具得分行缺失 {task}"
);
        }
        assert!(
            probe.stdout_contains("→ tier=big"),
            "7/7 全对应评 Big，实际 probe 输出：\n{}",
            probe.stdout
        );
        // 确实消费完整脚本：chat 次数 ≥7（若有 models-list GET 只多不少），
        // 剩余 0 证明恰好 7 次调度（GET 不出队，FIFO 正好被 7 题耗尽）。
        assert!(mock.hits() >= 7, "至少 7 次 HTTP 往返");
        assert_eq!(mock.remaining(), 0, "脚本必须正好耗尽（一题一条）");

        // tier 落盘：config.json 条目 model_tier == "big"。
        let cfg = read_cfg(&ws);
        let entry = cfg["model_list"]
            .as_array()
            .and_then(|a| a.first())
            .expect("add 必须留下一条模型记录");
        assert_eq!(entry["model"], "r9p/probe-x");
        assert_eq!(
            entry["model_tier"], "big",
            "探针结果必须写回 config.json"
        );
    }

    #[tokio::test]
    async fn remove_confirm_prompt_both_arms() {
        let bin = resolve_nemesisbot_bin().expect("release binary");

        // --- y 臂：确认删除 ---
        let ws = TestWorkspace::new().unwrap();
        std::fs::create_dir_all(ws.home()).unwrap();
        std::fs::write(
            &ws.config_path(),
            serde_json::json!({
                "model_list": [{"model_name": "rm-y", "model": "r9/rm-y", "api_key": "k"}]
            })
            .to_string(),
        )
        .unwrap();
        let out = ws
            .run_cli_with_stdin(&bin, &["model", "remove", "r9/rm-y"], "y\n", 30)
            .await;
        assert!(
            out.success(),
            "y 臂失败：stdout={} stderr={}",
            out.stdout,
            out.stderr
        );
        assert!(out.stdout_contains("Remove model 'r9/rm-y'"), "先出 y/N 提示");
        assert!(out.stdout_contains("Model removed: r9/rm-y"));
        let cfg = read_cfg(&ws);
        let models = cfg["model_list"].as_array().unwrap();
        assert!(
            !models.iter().any(|m| m["model"] == "r9/rm-y"),
            "y 臂必须真的把条目删掉"
        );

        // --- N 臂（空回车）：Aborted 且条目原样保留 ---
        let ws2 = TestWorkspace::new().unwrap();
        std::fs::create_dir_all(ws2.home()).unwrap();
        std::fs::write(
            &ws2.config_path(),
            serde_json::json!({
                "model_list": [{"model_name": "rm-n", "model": "r9/rm-n", "api_key": "k"}]
            })
            .to_string(),
        )
        .unwrap();
        let out = ws2
            .run_cli_with_stdin(&bin, &["model", "remove", "r9/rm-n"], "\n", 30)
            .await;
        assert!(out.success(), "N 臂也应是正常退出（非 Err）");
        assert!(out.stdout_contains("Aborted."));
        let cfg = read_cfg(&ws2);
        let models = cfg["model_list"].as_array().unwrap();
        assert_eq!(models.len(), 1, "取消删除后条目必须还在");
        assert_eq!(models[0]["model"], "r9/rm-n");
    }

    /// 进程内回归：MockAiServer ToolCall → factory::create_provider("r9p/x")
    /// → provider.chat，arguments 必须单层转义原样到达（曾 double-encode 导致
    /// probe schema 轴 7/7 全 Invalid → tier 误判 mini）。
    #[tokio::test]
    async fn r9_repro_toolcall_arguments_roundtrip() {
        let mock = MockAiServer::start(vec![MockAiReply::ToolCall {
            name: "exec".into(),
            arguments: r#"{"command":"date"}"#.into(),
        }])
        .expect("mock");

        let cfg = nemesis_providers::factory::FactoryConfig {
            llm_ref: "r9p/repro-x".into(),
            api_key: "sk-r9".into(),
            api_base: format!("{}/v1", mock.base_url()),
            workspace: ".".into(),
            connect_mode: Default::default(),
            account_id: String::new(),
            headers: Default::default(),
        };
        let provider = nemesis_providers::factory::create_provider(&cfg).expect("provider");

        let messages = vec![nemesis_providers::types::Message {
            role: "user".into(),
            content: "run date".into(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
            reasoning_content: None,
            extra: Default::default(),
        }];
        let resp = provider
            .chat(&messages, &[], "repro-x", &Default::default())
            .await
            .expect("chat ok");
        assert_eq!(resp.tool_calls.len(), 1);
        let tc = &resp.tool_calls[0];
        assert_eq!(tc.function.as_ref().expect("function").name, "exec");
        let args = &tc.function.as_ref().expect("function").arguments;
        assert_eq!(
            args, r#"{"command":"date"}"#,
            "arguments 必须是裸 JSON 文本（double-encode 回归）"
        );
        // args_validator 必须判 Valid（probe schema 轴的真实判定路径）。
        let outcome = nemesis_agent::args_validator::check(
            &serde_json::json!({
                "type": "object",
                "properties": {"command": {"type": "string"}},
                "required": ["command"]
            }),
            args,
        );
        assert!(
            matches!(outcome, nemesis_agent::args_validator::Outcome::Valid),
            "Valid 才能支撑 probe tier=big，实际 {outcome:?}"
        );
    }
}

// ===========================================================================
// r10（覆盖率 A 类 miss 补充）：probe 的空名臂（model.rs 702-704）——
// `name.is_empty()` 时改走 get_effective_llm(Some(&cfg)) 取 agents.defaults.llm
// 作为探测目标。唯一入口是 CLI 传空字符串参数；子进程 + MockAiServer 全链
// 驱动（七题脚本沿用 r9_subprocess 形态，本模块内自带一份避免跨界借用）。
// 空名同时意味着 update_model_entry 匹配不到任何条目 → wrote=false →
// tier 不落盘（734-739 的写回短路边），一并断言。
// ===========================================================================

mod r10_subprocess {
    use test_harness::mock_ai::{MockAiReply, MockAiServer};
    use test_harness::{resolve_nemesisbot_bin, TestWorkspace};

    fn perfect_probe_script() -> Vec<MockAiReply> {
        vec![
            MockAiReply::ToolCall {
                name: "exec".into(),
                arguments: r#"{"command":"date"}"#.into(),
            },
            MockAiReply::ToolCall {
                name: "read_file".into(),
                arguments: r#"{"path":"README.md"}"#.into(),
            },
            MockAiReply::ToolCall {
                name: "create_dir".into(),
                arguments: r#"{"path":"test"}"#.into(),
            },
            MockAiReply::ToolCall {
                name: "grep".into(),
                arguments: r#"{"pattern":"TODO"}"#.into(),
            },
            MockAiReply::ToolCall {
                name: "write_file".into(),
                arguments: r#"{"path":"note.md","content":"hi"}"#.into(),
            },
            MockAiReply::ToolCall {
                name: "edit_file".into(),
                arguments: r#"{"path":"note.md","old_text":"hi","new_text":"yo"}"#.into(),
            },
            MockAiReply::ToolCall {
                name: "cluster_rpc".into(),
                arguments: r#"{"target_node":"node-b","message":"你好"}"#.into(),
            },
        ]
    }

    #[tokio::test]
    async fn r10_probe_with_empty_name_resolves_effective_llm_and_skips_tier_persist() {
        let ws = TestWorkspace::new().expect("workspace");
        std::fs::create_dir_all(ws.home()).unwrap();
        std::fs::write(&ws.config_path(), "{}").unwrap();

        let mock = MockAiServer::start(perfect_probe_script()).expect("mock ai server");
        let bin = resolve_nemesisbot_bin().expect("release binary");
        let base = format!("{}/v1", mock.base_url());

        let add = ws
            .run_cli(
                &bin,
                &[
                    "model",
                    "add",
                    "--model",
                    "r10p/empty-name",
                    "--key",
                    "sk-r10",
                    "--base",
                    base.as_str(),
                ],
            )
            .await;
        assert!(
            add.success(),
            "model add 失败：stdout={} stderr={}",
            add.stdout,
            add.stderr
        );

        // 把 effective LLM 指到该模型：agents.defaults.llm = 模型全名。
        let cfg_path = ws.config_path();
        let mut cfg: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
        cfg["agents"]["defaults"]["llm"] =
            serde_json::Value::String("r10p/empty-name".to_string());
        std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).unwrap()).unwrap();

        // 关键动作：探测目标传空串 → 走 get_effective_llm 回退臂。
        let probe = ws.run_cli_with_timeout(&bin, &["model", "probe", ""], 60).await;
        assert!(
            probe.success(),
            "空名 probe 必须解析 effective LLM 后成功：stdout={} stderr={}",
            probe.stdout,
            probe.stderr
        );
        assert!(probe.stdout_contains("能力探针报告"));
        assert_eq!(mock.remaining(), 0, "七题脚本必须恰好耗尽");

        // wrote=false：空名匹配不到任何 model_list 条目 → tier 不落盘。
        let after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
        let entry = after["model_list"]
            .as_array()
            .and_then(|a| a.first())
            .expect("add 留下的条目必须在");
        assert_ne!(
            entry["model_tier"].as_str(),
            Some("big"),
            "空名 probe 不得把 tier 写回（update_model_entry 匹配不到空名条目），实际：{entry}"
        );
    }
}
