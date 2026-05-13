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
}
