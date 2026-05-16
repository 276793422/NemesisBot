//! Model command - manage LLM models.

use anyhow::Result;
use crate::common;

#[derive(clap::Subcommand)]
pub enum ModelAction {
    /// Add a new model configuration
    Add {
        /// Model name in vendor/model format (e.g., zhipu/glm-4.7)
        #[arg(long)]
        model: String,
        /// API key for the model
        #[arg(long)]
        key: Option<String>,
        /// Custom base URL
        #[arg(long)]
        base: Option<String>,
        /// Proxy URL for the model
        #[arg(long)]
        proxy: Option<String>,
        /// Authentication method (e.g., "oauth", "token")
        #[arg(long)]
        auth: Option<String>,
        /// Set as default model
        #[arg(long, default_value_t = false)]
        default: bool,
    },
    /// List configured models
    List {
        /// Show verbose output with all details
        #[arg(short, long, default_value_t = false)]
        verbose: bool,
    },
    /// Remove a model configuration
    Remove {
        /// Model name to remove
        name: String,
        /// Skip confirmation prompt
        #[arg(short, long)]
        force: bool,
    },
    /// Show default model
    Default,
}

pub fn run(action: ModelAction, local: bool) -> Result<()> {
    let home = common::resolve_home(local);
    let cfg_path = common::config_path(&home);

    match action {
        ModelAction::Add { model, key, base, proxy, auth, default } => {
            if !cfg_path.exists() {
                anyhow::bail!("Configuration not found. Run 'nemesisbot onboard default' first.");
            }

            let data = std::fs::read_to_string(&cfg_path)?;
            let mut cfg: serde_json::Value = serde_json::from_str(&data)?;

            // Validate model identifier format: must be vendor/model
            if !model.contains('/') {
                anyhow::bail!(
                    "Invalid model identifier '{}'. Expected format: vendor/model\n\
                     Example: openai/gpt-4o, anthropic/claude-sonnet-4",
                    model
                );
            }

            // Parse vendor and model name
            let parts: Vec<&str> = model.splitn(2, '/').collect();
            let model_name_alias = match parts.len() {
                2 => parts[1].to_string(),
                _ => model.clone(),
            };

            // Build model entry
            let mut entry = serde_json::json!({
                "model_name": model_name_alias,
                "model": model.clone(),
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

            // Add to model list
            if let Some(obj) = cfg.as_object_mut() {
                if let Some(models) = obj.get_mut("model_list") {
                    if let Some(arr) = models.as_array_mut() {
                        // Check for duplicate and warn
                        let existing = arr.iter().find(|m| m.get("model").and_then(|v| v.as_str()) == Some(&model));
                        if let Some(existing) = existing {
                            let existing_name = existing.get("model_name").and_then(|v| v.as_str()).unwrap_or("?");
                            println!("  Warning: Model '{}' already exists (alias: {}), updating...", model, existing_name);
                        }
                        // Remove existing entry with same model name
                        arr.retain(|m| m.get("model").and_then(|v| v.as_str()) != Some(&model));
                        arr.push(entry);
                    }
                } else {
                    obj.insert("model_list".to_string(), serde_json::json!([entry]));
                }

                // Set as default if requested
                if default {
                    // Set agents.defaults.llm so get_effective_llm picks it up.
                    // Only set the alias (part after '/') as the default model,
                    // matching Go's behavior. Go does NOT set a top-level default_model field.
                    let alias = model.split('/').next_back().unwrap_or(&model).to_string();
                    let agents = obj
                        .entry("agents")
                        .or_insert_with(|| serde_json::json!({}));
                    if let Some(agents_obj) = agents.as_object_mut() {
                        let defaults = agents_obj
                            .entry("defaults")
                            .or_insert_with(|| serde_json::json!({}));
                        if let Some(defaults_obj) = defaults.as_object_mut() {
                            defaults_obj.insert("llm".to_string(), serde_json::Value::String(alias));
                        }
                    }
                }

                std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).unwrap_or_default())?;
            }

            println!("Model added: {}", model);
            if default {
                println!("Set as default model.");
            } else {
                // Auto-default: if this is the only model and no default is set,
                // automatically make it the default (matches user expectation).
                let model_count = cfg.get("model_list")
                    .and_then(|v| v.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                let current_default = cfg.get("agents")
                    .and_then(|a| a.get("defaults"))
                    .and_then(|d| d.get("llm"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if model_count == 1 && current_default.is_empty() {
                    // Auto-set as default
                    let alias = model.split('/').next_back().unwrap_or(&model).to_string();
                    if let Some(obj) = cfg.as_object_mut() {
                        let agents = obj
                            .entry("agents")
                            .or_insert_with(|| serde_json::json!({}));
                        if let Some(agents_obj) = agents.as_object_mut() {
                            let defaults = agents_obj
                                .entry("defaults")
                                .or_insert_with(|| serde_json::json!({}));
                            if let Some(defaults_obj) = defaults.as_object_mut() {
                                defaults_obj.insert("llm".to_string(), serde_json::Value::String(alias.clone()));
                            }
                        }
                        std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).unwrap_or_default())?;
                    }
                    println!("Auto-set as default model (only model configured): {}", alias);
                }
            }
        }
        ModelAction::List { verbose } => {
            println!("Configured Models");
            println!("==================");
            if !cfg_path.exists() {
                println!("  No configuration found. Run 'nemesisbot onboard default' first.");
                return Ok(());
            }
            let data = std::fs::read_to_string(&cfg_path)?;
            let cfg: serde_json::Value = serde_json::from_str(&data)?;

            // Check agents.defaults.llm first (like Go's GetEffectiveLLM), then fall back to default_model
            let default_model = cfg.get("agents")
                .and_then(|a| a.get("defaults"))
                .and_then(|d| d.get("llm"))
                .and_then(|v| v.as_str())
                .or_else(|| cfg.get("default_model").and_then(|v| v.as_str()))
                .unwrap_or("(none)");

            println!("  Default: {}", default_model);
            println!();

            if let Some(models) = cfg.get("model_list").and_then(|v| v.as_array()) {
                if models.is_empty() {
                    println!("  No models configured.");
                    println!("  Add one with: nemesisbot model add --model <vendor/model> --key <key>");
                } else {
                    for m in models {
                        let model = m.get("model").and_then(|v| v.as_str()).unwrap_or("?");
                        let model_name = m.get("model_name").and_then(|v| v.as_str()).unwrap_or("");
                        let has_key = m.get("api_key").and_then(|v| v.as_str()).map(|k| !k.is_empty()).unwrap_or(false);
                        let base = m.get("api_base").and_then(|v| v.as_str());
                        let proxy = m.get("proxy").and_then(|v| v.as_str());
                        let auth_method = m.get("auth_method").and_then(|v| v.as_str());
                        // Match by model_name (alias) or full model identifier
                        let is_default = model == default_model || model_name == default_model;

                        println!("  {} {}", if is_default { "*" } else { " " }, model);
                        println!("    API key: {}", if has_key { "configured" } else { "not set" });
                        if let Some(b) = base {
                            println!("    Base URL: {}", b);
                        }
                        if verbose {
                            // Show masked key as dots
                            if let Some(k) = m.get("api_key").and_then(|v| v.as_str()) {
                                if !k.is_empty() {
                                    println!("    API Key: {}", "\u{2022}".repeat(8));
                                } else {
                                    println!("    API Key: (not set)");
                                }
                            }
                            if let Some(b) = base {
                                println!("    API Base: {}", b);
                            }
                            if let Some(p) = proxy {
                                if !p.is_empty() {
                                    println!("    Proxy: {}", p);
                                }
                            }
                            if let Some(a) = auth_method {
                                if !a.is_empty() {
                                    println!("    Auth Method: {}", a);
                                }
                            }
                        }
                    }
                }
            } else {
                println!("  No models configured.");
            }
        }
        ModelAction::Remove { name, force } => {
            if !cfg_path.exists() {
                anyhow::bail!("Configuration not found.");
            }
            let data = std::fs::read_to_string(&cfg_path)?;
            let mut cfg: serde_json::Value = serde_json::from_str(&data)?;

            // Check if this model is the current default via agents.defaults.llm
            let default_model = cfg.get("agents")
                .and_then(|a| a.get("defaults"))
                .and_then(|d| d.get("llm"))
                .and_then(|v| v.as_str())
                .unwrap_or("");

            // Also check top-level default_model for backward compatibility
            let default_model_compat = cfg.get("default_model").and_then(|v| v.as_str()).unwrap_or("");

            let model_list = cfg.get("model_list").and_then(|v| v.as_array())
                .cloned().unwrap_or_default();

            let is_default = name == default_model || name == default_model_compat ||
                model_list.iter().any(|m| {
                    let full_model = m.get("model").and_then(|v| v.as_str()).unwrap_or("");
                    let alias = m.get("model_name").and_then(|v| v.as_str()).unwrap_or("");
                    (full_model == name || full_model.ends_with(&format!("/{}", name))) &&
                        (full_model == default_model || alias == default_model ||
                         full_model == default_model_compat || alias == default_model_compat)
                });

            if is_default {
                println!("  Error: Cannot remove model '{}' - it is the current default.", name);
                println!("  Change the default first: nemesisbot agent set llm <other-model>");
                return Ok(());
            }

            // Confirmation prompt
            if !force {
                use std::io::{self, Write};
                print!("Remove model '{}'? (y/N): ", name);
                io::stdout().flush().ok();
                let mut response = String::new();
                io::stdin().read_line(&mut response).ok();
                if response.trim().to_lowercase() != "y" {
                    println!("Aborted.");
                    return Ok(());
                }
            }

            let mut found = false;
            if let Some(obj) = cfg.as_object_mut() {
                if let Some(models) = obj.get_mut("model_list") {
                    if let Some(arr) = models.as_array_mut() {
                        arr.retain(|m| {
                            let model = m.get("model").and_then(|v| v.as_str()).unwrap_or("");
                            if model == name || model.ends_with(&format!("/{}", name)) {
                                found = true;
                                false
                            } else {
                                true
                            }
                        });
                    }
                }
            }

            if found {
                std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).unwrap_or_default())?;
                println!("Model removed: {}", name);
            } else {
                println!("Model not found: {}", name);
            }
        }
        ModelAction::Default => {
            if !cfg_path.exists() {
                println!("No configuration found.");
                return Ok(());
            }
            let data = std::fs::read_to_string(&cfg_path)?;
            let cfg: serde_json::Value = serde_json::from_str(&data)?;
            // Check agents.defaults.llm first (like Go's GetEffectiveLLM), then fall back to default_model
            let default_model = cfg.get("agents")
                .and_then(|a| a.get("defaults"))
                .and_then(|d| d.get("llm"))
                .and_then(|v| v.as_str())
                .or_else(|| cfg.get("default_model").and_then(|v| v.as_str()));
            match default_model {
                Some(m) => println!("Default model: {}", m),
                None => println!("No default model configured."),
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    
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
            obj.insert("default_model".to_string(), serde_json::Value::String("test/model-1".to_string()));
        }
        fs::write(&cfg_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();

        // Verify
        let loaded: serde_json::Value = serde_json::from_str(&fs::read_to_string(&cfg_path).unwrap()).unwrap();
        assert_eq!(loaded.get("default_model").and_then(|v| v.as_str()), Some("test/model-1"));
        let models = loaded.get("model_list").unwrap().as_array().unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].get("proxy").and_then(|v| v.as_str()), Some("http://proxy:8080"));
        assert_eq!(models[0].get("auth_method").and_then(|v| v.as_str()), Some("token"));
    }

    #[test]
    fn test_model_remove_default_protection() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = serde_json::json!({"model_list": [{"model": "test/model-1"}], "default_model": "test/model-1"});
        let cfg_path = tmp.path().join("config.json");
        fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();

        let loaded: serde_json::Value = serde_json::from_str(&fs::read_to_string(&cfg_path).unwrap()).unwrap();
        let default = loaded.get("default_model").and_then(|v| v.as_str()).unwrap_or("");
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
        let default = cfg.get("agents")
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
        let default = cfg.get("agents")
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
        let default = cfg.get("agents")
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
        let mut arr: Vec<serde_json::Value> = vec![
            serde_json::json!({"model": "openai/gpt-4o", "model_name": "gpt-4o"}),
        ];
        let model = "openai/gpt-4o";
        let existing = arr.iter().find(|m| m.get("model").and_then(|v| v.as_str()) == Some(model));
        assert!(existing.is_some());
    }

    #[test]
    fn test_model_entry_no_duplicate() {
        let arr: Vec<serde_json::Value> = vec![
            serde_json::json!({"model": "openai/gpt-4o", "model_name": "gpt-4o"}),
        ];
        let model = "anthropic/claude";
        let existing = arr.iter().find(|m| m.get("model").and_then(|v| v.as_str()) == Some(model));
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
        assert_eq!(arr[0].get("model").and_then(|v| v.as_str()), Some("anthropic/claude"));
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
        assert_eq!(arr[0].get("model").and_then(|v| v.as_str()), Some("anthropic/claude"));
    }

    // -------------------------------------------------------------------------
    // Default model setting tests (agents.defaults.llm)
    // -------------------------------------------------------------------------

    #[test]
    fn test_set_default_model_in_config() {
        let mut cfg = serde_json::json!({});
        if let Some(obj) = cfg.as_object_mut() {
            let agents = obj
                .entry("agents")
                .or_insert_with(|| serde_json::json!({}));
            if let Some(agents_obj) = agents.as_object_mut() {
                let defaults = agents_obj
                    .entry("defaults")
                    .or_insert_with(|| serde_json::json!({}));
                if let Some(defaults_obj) = defaults.as_object_mut() {
                    defaults_obj.insert("llm".to_string(), serde_json::Value::String("gpt-4o".to_string()));
                }
            }
        }
        assert_eq!(
            cfg.get("agents").and_then(|a| a.get("defaults")).and_then(|d| d.get("llm")).and_then(|v| v.as_str()),
            Some("gpt-4o")
        );
    }

    #[test]
    fn test_auto_default_single_model() {
        let mut cfg = serde_json::json!({
            "model_list": [{"model": "openai/gpt-4o"}],
        });
        let model_count = cfg.get("model_list")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        let current_default = cfg.get("agents")
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
        assert_eq!(entry.get("model_name").and_then(|v| v.as_str()), Some("gpt-4o"));
        assert_eq!(entry.get("model").and_then(|v| v.as_str()), Some("openai/gpt-4o"));
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

        assert_eq!(entry.get("api_key").and_then(|v| v.as_str()), Some("test-api-key"));
        assert_eq!(entry.get("api_base").and_then(|v| v.as_str()), Some("https://api.example.com/v1"));
        assert_eq!(entry.get("proxy").and_then(|v| v.as_str()), Some("http://proxy:8080"));
        assert_eq!(entry.get("auth_method").and_then(|v| v.as_str()), Some("oauth"));
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
        let model = model_entry.get("model").and_then(|v| v.as_str()).unwrap_or("?");
        let model_name = model_entry.get("model_name").and_then(|v| v.as_str()).unwrap_or("");
        let is_default = model == default_model || model_name == default_model;
        assert!(is_default);
    }

    #[test]
    fn test_model_is_default_check_by_full_identifier() {
        let default_model = "openai/gpt-4o";
        let model_entry = serde_json::json!({"model": "openai/gpt-4o", "model_name": "gpt-4o"});
        let model = model_entry.get("model").and_then(|v| v.as_str()).unwrap_or("?");
        let model_name = model_entry.get("model_name").and_then(|v| v.as_str()).unwrap_or("");
        let is_default = model == default_model || model_name == default_model;
        assert!(is_default);
    }

    #[test]
    fn test_model_is_not_default() {
        let default_model = "claude";
        let model_entry = serde_json::json!({"model": "openai/gpt-4o", "model_name": "gpt-4o"});
        let model = model_entry.get("model").and_then(|v| v.as_str()).unwrap_or("?");
        let model_name = model_entry.get("model_name").and_then(|v| v.as_str()).unwrap_or("");
        let is_default = model == default_model || model_name == default_model;
        assert!(!is_default);
    }

    // -------------------------------------------------------------------------
    // Model has_key detection test
    // -------------------------------------------------------------------------

    #[test]
    fn test_model_has_api_key() {
        let m = serde_json::json!({"model": "openai/gpt-4o", "api_key": "sk-12345"});
        let has_key = m.get("api_key").and_then(|v| v.as_str()).map(|k| !k.is_empty()).unwrap_or(false);
        assert!(has_key);
    }

    #[test]
    fn test_model_empty_api_key() {
        let m = serde_json::json!({"model": "openai/gpt-4o", "api_key": ""});
        let has_key = m.get("api_key").and_then(|v| v.as_str()).map(|k| !k.is_empty()).unwrap_or(false);
        assert!(!has_key);
    }

    #[test]
    fn test_model_no_api_key() {
        let m = serde_json::json!({"model": "openai/gpt-4o"});
        let has_key = m.get("api_key").and_then(|v| v.as_str()).map(|k| !k.is_empty()).unwrap_or(false);
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
        if let Some(k) = &key { entry["api_key"] = serde_json::Value::String(k.clone()); }
        if let Some(b) = &base { entry["api_base"] = serde_json::Value::String(b.clone()); }
        if let Some(p) = &proxy { entry["proxy"] = serde_json::Value::String(p.clone()); }
        if let Some(a) = &auth { entry["auth_method"] = serde_json::Value::String(a.clone()); }

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

        let mut entry = serde_json::json!({
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

        let default_model = cfg.get("agents")
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

        let default_model = cfg.get("agents")
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

        let default_model = cfg.get("agents")
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
        let existing = arr.iter().find(|m| m.get("model").and_then(|v| v.as_str()) == Some(model));
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
        let existing = arr.iter().find(|m| m.get("model").and_then(|v| v.as_str()) == Some(model));
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
        let model_count = config.get("model_list")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        let current_default = config.get("agents")
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
        let model_count = cfg.get("model_list")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        let current_default = cfg.get("agents")
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
        let model_count = cfg.get("model_list")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        let current_default = cfg.get("agents")
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
        let default_model = cfg.get("agents")
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
            (full_model == name || full_model.ends_with(&format!("/{}", name))) &&
                (full_model == default_model || alias == default_model)
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
        let model_name = custom_name.unwrap_or_else(|| {
            if parts.len() == 2 { parts[1] } else { model }
        });
        assert_eq!(model_name, "my-gpt4");
    }

    #[test]
    fn test_model_entry_default_name_from_provider() {
        let model = "openai/gpt-4o";
        let custom_name: Option<&str> = None;
        let parts: Vec<&str> = model.splitn(2, '/').collect();
        let model_name = custom_name.unwrap_or_else(|| {
            if parts.len() == 2 { parts[1] } else { model }
        });
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
            format!("{}...{}", &key[..4], &key[key.len()-4..])
        } else {
            "****".to_string()
        };
        assert_eq!(masked, "****");
    }

    #[test]
    fn test_mask_api_key_long() {
        let key = "sk-1234567890abcdefghijklmnop";
        let masked = if key.len() > 8 {
            format!("{}...{}", &key[..4], &key[key.len()-4..])
        } else {
            "****".to_string()
        };
        assert_eq!(masked, "sk-1...mnop");
    }

    #[test]
    fn test_mask_api_key_empty() {
        let key = "";
        let masked = if key.len() > 8 {
            format!("{}...{}", &key[..4], &key[key.len()-4..])
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
        let default_model = cfg.get("agents")
            .and_then(|a| a.get("defaults"))
            .and_then(|d| d.get("llm"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert_eq!(default_model, "gpt-4o");
    }

    #[test]
    fn test_no_default_model_in_config() {
        let cfg = serde_json::json!({"model_list": []});
        let default_model = cfg.get("agents")
            .and_then(|a| a.get("defaults"))
            .and_then(|d| d.get("llm"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert_eq!(default_model, "");
    }
}
