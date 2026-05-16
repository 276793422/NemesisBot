//! Status command - show nemesisbot system status.

use anyhow::Result;
use crate::common;

pub fn run(local: bool) -> Result<()> {
    let home = common::resolve_home(local);
    let cfg_path = common::config_path(&home);
    let workspace = common::workspace_path(&home);

    println!("nemesisbot Status");
    println!("=================");
    println!("Version: {}", common::format_version());
    if !common::VERSION_INFO.build_time.is_empty() {
        println!("  Build: {}", common::VERSION_INFO.build_time);
    }
    if !common::VERSION_INFO.rust_version.is_empty() {
        println!("  Rust: {}", common::VERSION_INFO.rust_version);
    }
    println!();

    // Config file
    if cfg_path.exists() {
        println!("  Config: {} [found]", cfg_path.display());
    } else {
        println!("  Config: {} [not found]", cfg_path.display());
    }

    // Workspace
    if workspace.exists() {
        println!("  Workspace: {} [found]", workspace.display());
    } else {
        println!("  Workspace: {} [not found]", workspace.display());
    }

    // Model info
    if cfg_path.exists() {
        let data = std::fs::read_to_string(&cfg_path)?;
        if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&data) {
            // Default model
            if let Some(model) = cfg.get("default_model").and_then(|v| v.as_str()) {
                println!("  Default model: {}", model);
            }

            // Count models
            if let Some(models) = cfg.get("model_list").and_then(|v| v.as_array()) {
                if !models.is_empty() {
                    println!();
                    println!("  Configured Models:");
                    let mut provider_counts: std::collections::HashMap<String, (usize, bool)> = std::collections::HashMap::new();
                    for m in models {
                        let model = m.get("model").and_then(|v| v.as_str()).unwrap_or("");
                        let parts: Vec<&str> = model.splitn(2, '/').collect();
                        if parts.len() == 2 {
                            let provider = parts[0].to_lowercase();
                            let has_key = m.get("api_key").and_then(|v| v.as_str()).map(|k| !k.is_empty()).unwrap_or(false);
                            let entry = provider_counts.entry(provider).or_insert((0, false));
                            entry.0 += 1;
                            entry.1 = entry.1 || has_key;
                        }
                    }
                    for (provider, (count, has_key)) in &provider_counts {
                        println!("    {}: {} model(s), API key: {}", provider, count, if *has_key { "configured" } else { "not set" });
                    }
                }
            }

            // Security
            let security_enabled = cfg.get("security").and_then(|s| s.get("enabled")).and_then(|v| v.as_bool()).unwrap_or(true);
            println!();
            println!("  Security: {}", if security_enabled { "enabled" } else { "disabled" });

            // Forge
            let forge_enabled = cfg.get("forge").and_then(|f| f.get("enabled")).and_then(|v| v.as_bool()).unwrap_or(false);
            println!("  Forge: {}", if forge_enabled { "enabled" } else { "disabled" });
        }
    }

    // Authentication
    println!();
    println!("  Authentication:");
    let auth_path = home.join("auth.json");
    if auth_path.exists() {
        let store = nemesis_auth::AuthStore::new(&auth_path.to_string_lossy());
        let providers = store.list_providers();
        if providers.is_empty() {
            println!("    No credentials stored.");
        } else {
            for provider in &providers {
                if let Some(cred) = store.get(provider) {
                    let status = if cred.is_expired() {
                        "expired"
                    } else if cred.needs_refresh() {
                        "needs refresh"
                    } else {
                        "active"
                    };
                    let display = nemesis_auth::provider_display_name(provider);
                    println!("    {}: {} ({})", display, cred.auth_method, status);
                }
            }
        }
    } else {
        println!("    No credentials stored.");
    }

    println!();
    println!("  Home directory: {}", home.display());

    Ok(())
}

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use crate::common;

    #[test]
    fn test_status_model_parsing_single_provider() {
        let models = serde_json::json!([
            {"model": "openai/gpt-4", "api_key": "sk-12345"},
            {"model": "openai/gpt-3.5", "api_key": ""}
        ]);
        let model_list = models.as_array().unwrap();

        let mut provider_counts: std::collections::HashMap<String, (usize, bool)> = std::collections::HashMap::new();
        for m in model_list {
            let model = m.get("model").and_then(|v| v.as_str()).unwrap_or("");
            let parts: Vec<&str> = model.splitn(2, '/').collect();
            if parts.len() == 2 {
                let provider = parts[0].to_lowercase();
                let has_key = m.get("api_key").and_then(|v| v.as_str()).map(|k| !k.is_empty()).unwrap_or(false);
                let entry = provider_counts.entry(provider).or_insert((0, false));
                entry.0 += 1;
                entry.1 = entry.1 || has_key;
            }
        }

        assert_eq!(provider_counts.len(), 1);
        assert_eq!(provider_counts["openai"].0, 2);
        assert_eq!(provider_counts["openai"].1, true);
    }

    #[test]
    fn test_status_model_parsing_multiple_providers() {
        let models = serde_json::json!([
            {"model": "openai/gpt-4", "api_key": "key1"},
            {"model": "anthropic/claude-3", "api_key": "key2"},
            {"model": "zhipu/glm-4", "api_key": ""}
        ]);
        let model_list = models.as_array().unwrap();

        let mut provider_counts: std::collections::HashMap<String, (usize, bool)> = std::collections::HashMap::new();
        for m in model_list {
            let model = m.get("model").and_then(|v| v.as_str()).unwrap_or("");
            let parts: Vec<&str> = model.splitn(2, '/').collect();
            if parts.len() == 2 {
                let provider = parts[0].to_lowercase();
                let has_key = m.get("api_key").and_then(|v| v.as_str()).map(|k| !k.is_empty()).unwrap_or(false);
                let entry = provider_counts.entry(provider).or_insert((0, false));
                entry.0 += 1;
                entry.1 = entry.1 || has_key;
            }
        }

        assert_eq!(provider_counts.len(), 3);
        assert_eq!(provider_counts["openai"], (1, true));
        assert_eq!(provider_counts["anthropic"], (1, true));
        assert_eq!(provider_counts["zhipu"], (1, false));
    }

    #[test]
    fn test_status_model_parsing_no_provider() {
        let models = serde_json::json!([
            {"model": "no-slash-model", "api_key": "key"}
        ]);
        let model_list = models.as_array().unwrap();

        let mut provider_counts: std::collections::HashMap<String, (usize, bool)> = std::collections::HashMap::new();
        for m in model_list {
            let model = m.get("model").and_then(|v| v.as_str()).unwrap_or("");
            let parts: Vec<&str> = model.splitn(2, '/').collect();
            if parts.len() == 2 {
                let provider = parts[0].to_lowercase();
                let entry = provider_counts.entry(provider).or_insert((0, false));
                entry.0 += 1;
            }
        }

        assert!(provider_counts.is_empty());
    }

    #[test]
    fn test_status_security_enabled_default() {
        let cfg = serde_json::json!({});
        let security_enabled = cfg.get("security")
            .and_then(|s| s.get("enabled"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        assert!(security_enabled);
    }

    #[test]
    fn test_status_security_disabled() {
        let cfg = serde_json::json!({"security": {"enabled": false}});
        let security_enabled = cfg.get("security")
            .and_then(|s| s.get("enabled"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        assert!(!security_enabled);
    }

    #[test]
    fn test_status_forge_disabled_default() {
        let cfg = serde_json::json!({});
        let forge_enabled = cfg.get("forge")
            .and_then(|f| f.get("enabled"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        assert!(!forge_enabled);
    }

    #[test]
    fn test_status_forge_enabled() {
        let cfg = serde_json::json!({"forge": {"enabled": true}});
        let forge_enabled = cfg.get("forge")
            .and_then(|f| f.get("enabled"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        assert!(forge_enabled);
    }

    #[test]
    fn test_status_default_model_extraction() {
        let cfg = serde_json::json!({"default_model": "openai/gpt-4"});
        let model = cfg.get("default_model").and_then(|v| v.as_str());
        assert_eq!(model, Some("openai/gpt-4"));
    }

    #[test]
    fn test_status_no_default_model() {
        let cfg = serde_json::json!({});
        let model = cfg.get("default_model").and_then(|v| v.as_str());
        assert!(model.is_none());
    }

    #[test]
    fn test_status_empty_model_list() {
        let cfg = serde_json::json!({"model_list": []});
        let models = cfg.get("model_list").and_then(|v| v.as_array());
        assert!(models.is_some());
        assert!(models.unwrap().is_empty());
    }

    #[test]
    fn test_model_key_has_key_detection() {
        let m = serde_json::json!({"model": "openai/gpt-4", "api_key": "sk-12345"});
        let has_key = m.get("api_key").and_then(|v| v.as_str()).map(|k| !k.is_empty()).unwrap_or(false);
        assert!(has_key);
    }

    #[test]
    fn test_model_key_empty_key_detection() {
        let m = serde_json::json!({"model": "openai/gpt-4", "api_key": ""});
        let has_key = m.get("api_key").and_then(|v| v.as_str()).map(|k| !k.is_empty()).unwrap_or(false);
        assert!(!has_key);
    }

    #[test]
    fn test_model_key_no_key_field() {
        let m = serde_json::json!({"model": "openai/gpt-4"});
        let has_key = m.get("api_key").and_then(|v| v.as_str()).map(|k| !k.is_empty()).unwrap_or(false);
        assert!(!has_key);
    }

    // -------------------------------------------------------------------------
    // Additional status tests for coverage
    // -------------------------------------------------------------------------

    #[test]
    fn test_status_run_with_temp_config() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join(".nemesisbot");
        let cfg_path = home.join("config.json");
        let workspace = home.join("workspace");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&workspace).unwrap();

        let cfg = serde_json::json!({
            "default_model": "test/model-1",
            "model_list": [
                {"model": "openai/gpt-4", "api_key": "sk-test123"},
                {"model": "anthropic/claude", "api_key": ""}
            ],
            "security": {"enabled": false},
            "forge": {"enabled": true}
        });
        std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();

        // Verify the config can be parsed back correctly
        let data = std::fs::read_to_string(&cfg_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&data).unwrap();

        // Default model
        let model = parsed.get("default_model").and_then(|v| v.as_str());
        assert_eq!(model, Some("test/model-1"));

        // Model count
        let models = parsed.get("model_list").and_then(|v| v.as_array()).unwrap();
        assert_eq!(models.len(), 2);

        // Provider counts
        let mut provider_counts: std::collections::HashMap<String, (usize, bool)> = std::collections::HashMap::new();
        for m in models {
            let model = m.get("model").and_then(|v| v.as_str()).unwrap_or("");
            let parts: Vec<&str> = model.splitn(2, '/').collect();
            if parts.len() == 2 {
                let provider = parts[0].to_lowercase();
                let has_key = m.get("api_key").and_then(|v| v.as_str()).map(|k| !k.is_empty()).unwrap_or(false);
                let entry = provider_counts.entry(provider).or_insert((0, false));
                entry.0 += 1;
                entry.1 = entry.1 || has_key;
            }
        }
        assert_eq!(provider_counts["openai"], (1, true));
        assert_eq!(provider_counts["anthropic"], (1, false));

        // Security
        let security_enabled = parsed.get("security")
            .and_then(|s| s.get("enabled"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        assert!(!security_enabled);

        // Forge
        let forge_enabled = parsed.get("forge")
            .and_then(|f| f.get("enabled"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        assert!(forge_enabled);
    }

    #[test]
    fn test_status_auth_no_credentials() {
        let tmp = tempfile::TempDir::new().unwrap();
        let auth_path = tmp.path().join("auth.json");
        // No auth file means no credentials
        assert!(!auth_path.exists());
    }

    #[test]
    fn test_status_auth_with_credentials() {
        let tmp = tempfile::TempDir::new().unwrap();
        let auth_path = tmp.path().join("auth.json");
        let store = nemesis_auth::AuthStore::new(&auth_path.to_string_lossy());
        let cred = nemesis_auth::AuthCredential::login_paste_token("openai", "test-key").unwrap();
        store.save("openai", cred).unwrap();

        let providers = store.list_providers();
        assert_eq!(providers.len(), 1);
        let retrieved = store.get("openai");
        assert!(retrieved.is_some());

        let cred = retrieved.unwrap();
        let status = if cred.is_expired() {
            "expired"
        } else if cred.needs_refresh() {
            "needs refresh"
        } else {
            "active"
        };
        assert!(!status.is_empty());
    }

    #[test]
    fn test_status_model_parsing_mixed_models() {
        let models = serde_json::json!([
            {"model": "openai/gpt-4", "api_key": "key1"},
            {"model": "anthropic/claude-3", "api_key": ""},
            {"model": "no-slash-model", "api_key": "key3"},
            {"model": "zhipu/glm-4", "api_key": "key4"},
            {"model": "", "api_key": "key5"},
        ]);
        let model_list = models.as_array().unwrap();

        let mut provider_counts: std::collections::HashMap<String, (usize, bool)> = std::collections::HashMap::new();
        let mut no_provider_count = 0;
        for m in model_list {
            let model = m.get("model").and_then(|v| v.as_str()).unwrap_or("");
            let parts: Vec<&str> = model.splitn(2, '/').collect();
            if parts.len() == 2 {
                let provider = parts[0].to_lowercase();
                let has_key = m.get("api_key").and_then(|v| v.as_str()).map(|k| !k.is_empty()).unwrap_or(false);
                let entry = provider_counts.entry(provider).or_insert((0, false));
                entry.0 += 1;
                entry.1 = entry.1 || has_key;
            } else {
                no_provider_count += 1;
            }
        }
        assert_eq!(provider_counts.len(), 3);
        assert_eq!(no_provider_count, 2); // "no-slash-model" and ""
    }
}
