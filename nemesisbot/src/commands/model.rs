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
    /// Set the capability tier for a model (auto/mini/normal/big).
    /// Overrides auto-detection — "user knows best".
    SetTier {
        /// Model name (alias or vendor/model)
        name: String,
        /// One of: auto, mini, normal, big
        tier: String,
    },
    /// Set the parameter size for a model (e.g. 30B, 70B, 120B).
    /// Refines auto tier detection when the alias is opaque.
    SetSize {
        name: String,
        /// Size with optional B suffix, e.g. "30B", "9b", "120"
        size: String,
    },
    /// Set the real model name (for opaque aliases like "astron-code-latest").
    /// Refines auto tier detection.
    SetRealName {
        name: String,
        /// Real model name, e.g. "Qwen3-30B-A3B"
        real_name: String,
    },
    /// Run a capability probe — sends 7 short tool-use tasks to the model and
    /// writes the detected tier to config. Costs ~7 LLM calls. Explicit only.
    Probe {
        name: String,
    },
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

            // Phase 4a (small-model-tool-robustness): tag with an auto-detect
            // tier. Resolved at runtime from the model name (and any real_name /
            // model_size_b the user adds later). Override with `model set-tier`.
            entry["model_tier"] = serde_json::Value::String("auto".to_string());

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
            // Phase 4a: print the auto-detected capability tier (advisory).
            {
                let hint = nemesis_types::capability::TierHint {
                    full_model: Some(model.clone()),
                    real_name: None,
                    size_b: None,
                };
                let resolved = nemesis_types::capability::detect_tier(&hint);
                if resolved == nemesis_types::capability::ModelTier::Big {
                    println!(
                        "  → 能力档位：big（全量工具）。若此模型实际是小模型（如 30B 左右），建议：nemesisbot model set-tier {} mini",
                        model_name_alias
                    );
                } else {
                    println!("  → 能力档位：{}（自动检测）", resolved);
                }
            }
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
        ModelAction::SetTier { name, tier } => {
            if !cfg_path.exists() {
                anyhow::bail!("Configuration not found. Run 'nemesisbot onboard default' first.");
            }
            let parsed: nemesis_types::capability::ModelTier =
                serde_json::from_value(serde_json::Value::String(tier.clone()))
                    .map_err(|_| anyhow::anyhow!(
                        "Invalid tier '{}'. Use one of: auto | mini | normal | big", tier
                    ))?;
            let data = std::fs::read_to_string(&cfg_path)?;
            let mut cfg: serde_json::Value = serde_json::from_str(&data)?;
            let updated = update_model_entry(&mut cfg, &name, |e| {
                e["model_tier"] = serde_json::Value::String(parsed.to_string());
            });
            if updated {
                std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).unwrap_or_default())?;
                println!("✓ {} → model_tier={}", name, parsed);
                println!("  (生效于下次 gateway 启动；当前运行实例需重启)");
            } else {
                println!("Model not found: {}", name);
            }
        }
        ModelAction::SetSize { name, size } => {
            if !cfg_path.exists() {
                anyhow::bail!("Configuration not found.");
            }
            // Accept "30B", "30b", or "30" — normalize to whole billions.
            let size_b = nemesis_types::capability::parse_size_marker(&size)
                .or_else(|| size.trim().parse::<u32>().ok())
                .ok_or_else(|| anyhow::anyhow!(
                    "Invalid size '{}'. Examples: 30B, 9b, 70, 120B", size
                ))?;
            let data = std::fs::read_to_string(&cfg_path)?;
            let mut cfg: serde_json::Value = serde_json::from_str(&data)?;
            let resolved = nemesis_types::capability::tier_from_size_b(size_b);
            let updated = update_model_entry(&mut cfg, &name, |e| {
                e["model_size_b"] = serde_json::Value::Number(size_b.into());
            });
            if updated {
                std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).unwrap_or_default())?;
                println!("✓ {} → model_size_b={} (auto 检测将解析为 tier={})", name, size_b, resolved);
            } else {
                println!("Model not found: {}", name);
            }
        }
        ModelAction::SetRealName { name, real_name } => {
            if !cfg_path.exists() {
                anyhow::bail!("Configuration not found.");
            }
            let data = std::fs::read_to_string(&cfg_path)?;
            let mut cfg: serde_json::Value = serde_json::from_str(&data)?;
            let updated = update_model_entry(&mut cfg, &name, |e| {
                e["real_name"] = serde_json::Value::String(real_name.clone());
            });
            // Show what the auto-detection resolves to now.
            let hint = nemesis_types::capability::TierHint {
                full_model: None,
                real_name: Some(real_name.clone()),
                size_b: None,
            };
            let resolved = nemesis_types::capability::detect_tier(&hint);
            if updated {
                std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).unwrap_or_default())?;
                println!("✓ {} → real_name=\"{}\" (auto 检测将解析为 tier={})", name, real_name, resolved);
            } else {
                println!("Model not found: {}", name);
            }
        }
        ModelAction::Probe { name } => {
            if !cfg_path.exists() {
                anyhow::bail!("Configuration not found. Run 'nemesisbot onboard default' first.");
            }
            println!("正在对 '{}' 运行能力探针（7 个任务，约 7 次 LLM 调用，请稍候）...", name);
            let report = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(run_probe(&home, &name))
            })?;
            println!("{}", format_probe_report(&name, &report));
        }
    }
    Ok(())
}

/// Find a model entry in `model_list` by alias / full id / `vendor/<name>` suffix
/// and apply a mutation. Returns true if the entry was found and updated.
fn update_model_entry<F>(cfg: &mut serde_json::Value, name: &str, f: F) -> bool
where
    F: FnOnce(&mut serde_json::Value),
{
    let arr = match cfg
        .get_mut("model_list")
        .and_then(|v| v.as_array_mut())
    {
        Some(a) => a,
        None => return false,
    };
    for m in arr.iter_mut() {
        let full = m.get("model").and_then(|v| v.as_str()).unwrap_or("");
        let alias = m.get("model_name").and_then(|v| v.as_str()).unwrap_or("");
        if full == name || alias == name || full.ends_with(&format!("/{}", name)) {
            f(m);
            return true;
        }
    }
    false
}

/// Build a provider for the target model and run the capability probe. Writes
/// the detected tier to config. Must run inside a tokio multi-thread runtime
/// (caller wraps with `block_in_place` + `Handle::block_on`).
async fn run_probe(
    home: &std::path::Path,
    name: &str,
) -> anyhow::Result<nemesis_agent::probe::ProbeReport> {
    use std::collections::HashMap;
    use std::sync::Arc;

    let cfg_path = common::config_path(home);
    let cfg = nemesis_config::load_config(&cfg_path)
        .map_err(|e| anyhow::anyhow!("Failed to load config: {}", e))?;
    let llm_ref = if name.is_empty() {
        nemesis_config::get_effective_llm(Some(&cfg))
    } else {
        name.to_string()
    };
    let resolution = nemesis_config::resolve_model_config(&cfg, &llm_ref)
        .map_err(|e| anyhow::anyhow!("Failed to resolve model '{}': {}", llm_ref, e))?;
    let model_name = resolution.model_name.clone();
    let factory_cfg = nemesis_providers::factory::FactoryConfig {
        llm_ref: format!("{}/{}", resolution.provider_name, resolution.model_name),
        api_key: resolution.api_key.clone(),
        api_base: resolution.api_base.clone(),
        workspace: home.join("workspace").to_string_lossy().to_string(),
        connect_mode: resolution.connect_mode,
        account_id: String::new(),
        headers: HashMap::new(),
    };
    let provider = nemesis_providers::factory::create_provider(&factory_cfg)
        .map_err(|e| anyhow::anyhow!("Failed to create provider: {}", e))?;
    let provider_arc: Arc<dyn nemesis_providers::router::LLMProvider> = Arc::from(provider);
    let adapter = nemesis_web::ProviderAdapter::new(provider_arc, model_name.clone());

    let report = nemesis_agent::probe::run(&adapter, &model_name)
        .await
        .map_err(|e| anyhow::anyhow!("Probe failed: {}", e))?;

    // Persist the detected tier.
    let data = std::fs::read_to_string(&cfg_path)?;
    let mut cfg_val: serde_json::Value = serde_json::from_str(&data)?;
    let wrote = update_model_entry(&mut cfg_val, name, |e| {
        e["model_tier"] = serde_json::Value::String(report.tier.to_string());
    });
    if wrote {
        std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg_val).unwrap_or_default())?;
    }
    Ok(report)
}

fn format_probe_report(name: &str, r: &nemesis_agent::probe::ProbeReport) -> String {
    let mut s = format!("能力探针报告: {}\n", name);
    s.push_str(&format!(
        "  format={:.2}  selection={:.2}  schema={:.2}\n",
        r.format_score, r.selection_score, r.schema_score
    ));
    s.push_str("  每个工具得分:\n");
    for (tool, sc) in &r.per_task {
        s.push_str(&format!(
            "    {:<14} format={:.0} selection={:.0} schema={:.0}\n",
            tool, sc.format, sc.selection, sc.schema
        ));
    }
    s.push_str(&format!("  → tier={} (已写入 config.json)", r.tier));
    s
}

#[cfg(test)]
mod tests;
