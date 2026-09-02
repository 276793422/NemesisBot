//! Model command - manage LLM models.

use crate::common;
use anyhow::Result;
use chrono::TimeZone;
use std::path::Path;

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
    /// H4 (U16 half): set the reasoning-effort tier for a model.
    /// "off" clears it (send nothing); low/medium/high set the tier that
    /// providers translate into their wire format.
    SetEffort {
        name: String,
        /// One of: off | low | medium | high
        effort: String,
    },
    /// Run a capability probe — sends 7 short tool-use tasks to the model and
    /// writes the detected tier to config. Costs ~7 LLM calls. Explicit only.
    Probe { name: String },
    /// U16 (sixth batch): fetch the models.dev catalog (context windows /
    /// max output tokens per model) and cache it at
    /// `<home>/workspace/data/models_catalog.json`. `model add` auto-fills
    /// from the cache.
    CatalogUpdate,
    /// 价目表管理（A2 在线更新：LiteLLM 主源 + 自定义条目 + 离线导入）。
    /// 查表优先级：自定义 > 下载 > 内置（内置 36 模型表离线兜底）。
    Prices {
        #[command(subcommand)]
        action: PricesAction,
    },
}

#[derive(clap::Subcommand)]
pub enum PricesAction {
    /// 显示分层价目表概况（各层条数 + 下载元数据 + 自定义条目明细）。
    List,
    /// 在线拉取最新价目表并整体替换下载层（LiteLLM 主源，ETag 增量；
    /// 失败保留旧表 + 退码非 0）。
    Update {
        /// 镜像地址覆盖（缺省 = LiteLLM 官方 raw 地址）。
        #[arg(long)]
        url: Option<String>,
    },
    /// 从本地文件导入 LiteLLM 原始 JSON（离线环境兜底：外网机器下载后拷入）。
    Import {
        /// LiteLLM model_prices_and_context_window.json 文件路径。
        file: String,
    },
    /// 新增/更新自定义价目条目（最高查表优先级，按模型名幂等）。
    Add {
        /// 模型名（与 `model add` 一致，如 zhipu/glm-4.7；裸名亦可）。
        model: String,
        /// 输入价（USD / 百万 token）。
        #[arg(long)]
        input: f64,
        /// 输出价（USD / 百万 token）。
        #[arg(long)]
        output: f64,
        /// 缓存读价（USD / 百万 token）。
        #[arg(long, default_value_t = 0.0)]
        cache_read: f64,
        /// 缓存写价（USD / 百万 token）。
        #[arg(long, default_value_t = 0.0)]
        cache_creation: f64,
        /// 显示名（可选，缺省用模型名）。
        #[arg(long)]
        display: Option<String>,
    },
    /// 删除自定义条目（只影响自定义层；下载/内置层不动）。
    Remove {
        /// 自定义条目模型名（`prices add` 时的 model）。
        model: String,
    },
}

pub async fn run(action: ModelAction, local: bool) -> Result<()> {
    let home = common::resolve_home(local);
    let cfg_path = common::config_path(&home);

    match action {
        ModelAction::Add {
            model,
            key,
            base,
            proxy,
            auth,
            default,
        } => {
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

            // U16 (sixth batch): auto-fill context_window / max_output_tokens
            // from the models.dev catalog cache when the model is a catalog
            // hit and the user didn't set them otherwise (there is no CLI
            // flag for these today — catalog is the only writer besides a
            // manual config edit). Falls back silently when no/empty cache:
            // `model catalog update` populates it.
            {
                let cfg_dir = cfg_path
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_default();
                if let Ok(Some(cat)) = catalog::load_cache(&cfg_dir)
                    && let Some(hit) = catalog::lookup(&cat, &model)
                {
                    entry["context_window"] = serde_json::Value::Number(hit.context_window.into());
                    if let Some(mot) = hit.max_output_tokens {
                        entry["max_output_tokens"] = serde_json::Value::Number(mot.into());
                    }
                    println!(
                        "  Catalog hit (models.dev): context_window={}, max_output_tokens={}",
                        hit.context_window,
                        hit.max_output_tokens
                            .map(|n| n.to_string())
                            .unwrap_or_else(|| "(not declared)".to_string())
                    );
                }
            }

            // Add to model list
            if let Some(obj) = cfg.as_object_mut() {
                if let Some(models) = obj.get_mut("model_list") {
                    if let Some(arr) = models.as_array_mut() {
                        // Check for duplicate and warn
                        let existing = arr
                            .iter()
                            .find(|m| m.get("model").and_then(|v| v.as_str()) == Some(&model));
                        if let Some(existing) = existing {
                            let existing_name = existing
                                .get("model_name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("?");
                            println!(
                                "  Warning: Model '{}' already exists (alias: {}), updating...",
                                model, existing_name
                            );
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
                    let agents = obj.entry("agents").or_insert_with(|| serde_json::json!({}));
                    if let Some(agents_obj) = agents.as_object_mut() {
                        let defaults = agents_obj
                            .entry("defaults")
                            .or_insert_with(|| serde_json::json!({}));
                        if let Some(defaults_obj) = defaults.as_object_mut() {
                            defaults_obj
                                .insert("llm".to_string(), serde_json::Value::String(alias));
                        }
                    }
                }

                std::fs::write(
                    &cfg_path,
                    serde_json::to_string_pretty(&cfg).unwrap_or_default(),
                )?;
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
                if model_count == 1 && current_default.is_empty() {
                    // Auto-set as default
                    let alias = model.split('/').next_back().unwrap_or(&model).to_string();
                    if let Some(obj) = cfg.as_object_mut() {
                        let agents = obj.entry("agents").or_insert_with(|| serde_json::json!({}));
                        if let Some(agents_obj) = agents.as_object_mut() {
                            let defaults = agents_obj
                                .entry("defaults")
                                .or_insert_with(|| serde_json::json!({}));
                            if let Some(defaults_obj) = defaults.as_object_mut() {
                                defaults_obj.insert(
                                    "llm".to_string(),
                                    serde_json::Value::String(alias.clone()),
                                );
                            }
                        }
                        std::fs::write(
                            &cfg_path,
                            serde_json::to_string_pretty(&cfg).unwrap_or_default(),
                        )?;
                    }
                    println!(
                        "Auto-set as default model (only model configured): {}",
                        alias
                    );
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
            let default_model = cfg
                .get("agents")
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
                    println!(
                        "  Add one with: nemesisbot model add --model <vendor/model> --key <key>"
                    );
                } else {
                    for m in models {
                        let model = m.get("model").and_then(|v| v.as_str()).unwrap_or("?");
                        let model_name = m.get("model_name").and_then(|v| v.as_str()).unwrap_or("");
                        let has_key = m
                            .get("api_key")
                            .and_then(|v| v.as_str())
                            .map(|k| !k.is_empty())
                            .unwrap_or(false);
                        let base = m.get("api_base").and_then(|v| v.as_str());
                        let proxy = m.get("proxy").and_then(|v| v.as_str());
                        let auth_method = m.get("auth_method").and_then(|v| v.as_str());
                        // Match by model_name (alias) or full model identifier
                        let is_default = model == default_model || model_name == default_model;

                        println!("  {} {}", if is_default { "*" } else { " " }, model);
                        println!(
                            "    API key: {}",
                            if has_key { "configured" } else { "not set" }
                        );
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
                            if let Some(p) = proxy
                                && !p.is_empty()
                            {
                                println!("    Proxy: {}", p);
                            }
                            if let Some(a) = auth_method
                                && !a.is_empty()
                            {
                                println!("    Auth Method: {}", a);
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
            let default_model = cfg
                .get("agents")
                .and_then(|a| a.get("defaults"))
                .and_then(|d| d.get("llm"))
                .and_then(|v| v.as_str())
                .unwrap_or("");

            // Also check top-level default_model for backward compatibility
            let default_model_compat = cfg
                .get("default_model")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let model_list = cfg
                .get("model_list")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            let is_default = name == default_model
                || name == default_model_compat
                || model_list.iter().any(|m| {
                    let full_model = m.get("model").and_then(|v| v.as_str()).unwrap_or("");
                    let alias = m.get("model_name").and_then(|v| v.as_str()).unwrap_or("");
                    (full_model == name || full_model.ends_with(&format!("/{}", name)))
                        && (full_model == default_model
                            || alias == default_model
                            || full_model == default_model_compat
                            || alias == default_model_compat)
                });

            if is_default {
                println!(
                    "  Error: Cannot remove model '{}' - it is the current default.",
                    name
                );
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
            if let Some(obj) = cfg.as_object_mut()
                && let Some(models) = obj.get_mut("model_list")
                && let Some(arr) = models.as_array_mut()
            {
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

            if found {
                std::fs::write(
                    &cfg_path,
                    serde_json::to_string_pretty(&cfg).unwrap_or_default(),
                )?;
                println!("Model removed: {}", name);
            } else {
                anyhow::bail!("Model not found: {}", name);
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
            let default_model = cfg
                .get("agents")
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
                serde_json::from_value(serde_json::Value::String(tier.clone())).map_err(|_| {
                    anyhow::anyhow!(
                        "Invalid tier '{}'. Use one of: auto | mini | normal | big",
                        tier
                    )
                })?;
            let data = std::fs::read_to_string(&cfg_path)?;
            let mut cfg: serde_json::Value = serde_json::from_str(&data)?;
            let updated = update_model_entry(&mut cfg, &name, |e| {
                e["model_tier"] = serde_json::Value::String(parsed.to_string());
            });
            if updated {
                std::fs::write(
                    &cfg_path,
                    serde_json::to_string_pretty(&cfg).unwrap_or_default(),
                )?;
                println!("✓ {} → model_tier={}", name, parsed);
                println!("  (生效于下次 gateway 启动；当前运行实例需重启)");
            } else {
                anyhow::bail!("Model not found: {}", name);
            }
        }
        ModelAction::SetEffort { name, effort } => {
            if !cfg_path.exists() {
                anyhow::bail!("Configuration not found. Run 'nemesisbot onboard default' first.");
            }
            let e = effort.to_lowercase();
            if !matches!(e.as_str(), "off" | "low" | "medium" | "high") {
                anyhow::bail!(
                    "Invalid effort '{}'. Use one of: off | low | medium | high",
                    effort
                );
            }
            let data = std::fs::read_to_string(&cfg_path)?;
            let mut cfg: serde_json::Value = serde_json::from_str(&data)?;
            // "off" clears the field (absent = send nothing); a tier writes it.
            let value = if e == "off" {
                serde_json::Value::String(String::new())
            } else {
                serde_json::Value::String(e.clone())
            };
            let updated = update_model_entry(&mut cfg, &name, |en| {
                en["reasoning_effort"] = value.clone();
            });
            if updated {
                std::fs::write(
                    &cfg_path,
                    serde_json::to_string_pretty(&cfg).unwrap_or_default(),
                )?;
                if e == "off" {
                    println!("✓ {} → reasoning_effort cleared", name);
                } else {
                    println!("✓ {} → reasoning_effort={}", name, e);
                }
                println!("  (生效于下次 LLM 调用前的 config 重读)");
            } else {
                anyhow::bail!("Model not found: {}", name);
            }
        }
        ModelAction::SetSize { name, size } => {
            if !cfg_path.exists() {
                anyhow::bail!("Configuration not found.");
            }
            // Accept "30B", "30b", or "30" — normalize to whole billions.
            let size_b = nemesis_types::capability::parse_size_marker(&size)
                .or_else(|| size.trim().parse::<u32>().ok())
                .ok_or_else(|| {
                    anyhow::anyhow!("Invalid size '{}'. Examples: 30B, 9b, 70, 120B", size)
                })?;
            let data = std::fs::read_to_string(&cfg_path)?;
            let mut cfg: serde_json::Value = serde_json::from_str(&data)?;
            let resolved = nemesis_types::capability::tier_from_size_b(size_b);
            let updated = update_model_entry(&mut cfg, &name, |e| {
                e["model_size_b"] = serde_json::Value::Number(size_b.into());
            });
            if updated {
                std::fs::write(
                    &cfg_path,
                    serde_json::to_string_pretty(&cfg).unwrap_or_default(),
                )?;
                println!(
                    "✓ {} → model_size_b={} (auto 检测将解析为 tier={})",
                    name, size_b, resolved
                );
            } else {
                anyhow::bail!("Model not found: {}", name);
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
                std::fs::write(
                    &cfg_path,
                    serde_json::to_string_pretty(&cfg).unwrap_or_default(),
                )?;
                println!(
                    "✓ {} → real_name=\"{}\" (auto 检测将解析为 tier={})",
                    name, real_name, resolved
                );
            } else {
                anyhow::bail!("Model not found: {}", name);
            }
        }
        ModelAction::Probe { name } => {
            if !cfg_path.exists() {
                anyhow::bail!("Configuration not found. Run 'nemesisbot onboard default' first.");
            }
            println!(
                "正在对 '{}' 运行能力探针（7 个任务，约 7 次 LLM 调用，请稍候）...",
                name
            );
            let report = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(run_probe(&home, &name))
            })?;
            println!("{}", format_probe_report(&name, &report));
        }
        ModelAction::CatalogUpdate => {
            if !cfg_path.exists() {
                anyhow::bail!("Configuration not found. Run 'nemesisbot onboard default' first.");
            }
            println!(
                "正在拉取 models.dev 模型目录（{}，失败自动走 jsDelivr 镜像）...",
                catalog::API_URL
            );
            let cfg_dir = cfg_path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_default();
            match catalog::fetch_http().await {
                Ok(entries) => {
                    let n = entries.len();
                    catalog::save_cache(&cfg_dir, entries).map_err(|e| {
                        anyhow::anyhow!(
                            "目录写入失败（{}）：{e}",
                            nemesis_path::models_catalog_cache_path(&cfg_dir).display()
                        )
                    })?;
                    println!(
                        "目录已更新：{} 个模型 → {}",
                        n,
                        nemesis_path::models_catalog_cache_path(&cfg_dir).display()
                    );
                    println!(
                        "之后 `model add` 命中的模型会自动填充 context_window / max_output_tokens。"
                    );
                }
                Err(e) => {
                    // Offline/intranet semantics: keep the existing cache,
                    // report loudly, exit non-zero (CLI contract).
                    let cached = catalog::load_cache(&cfg_dir).ok().flatten();
                    match cached {
                        Some(cat) => {
                            println!(
                                "拉取失败（{e}），保留现有缓存：{} 个模型（fetched_at={}）",
                                cat.entries.len(),
                                cat.fetched_at
                            );
                        }
                        None => {
                            anyhow::bail!(
                                "拉取失败且无本地缓存：{e}\n\
                                 内网部署请在外网机器上运行 `model catalog update` 后拷贝 {} 过来。",
                                nemesis_path::models_catalog_cache_path(&cfg_dir).display()
                            );
                        }
                    }
                }
            }
        }
        ModelAction::Prices { action } => {
            run_prices(action, &home).await?;
        }
    }
    Ok(())
}

/// A2（2026-08-31）：`model prices` 子命令实现。CLI 直开 DataStore（与
/// gateway 同一路径真相源 `workspace_data_dir`），计价/价目管理复用
/// nemesis-data + nemesis-web 的同步实现，不重复造轮子。
async fn run_prices(action: PricesAction, home: &Path) -> Result<()> {
    let db_path = nemesis_path::workspace_data_dir(home).join("nemesisbot_data.db");
    let ds = nemesis_data::DataStore::open(&db_path)
        .map_err(|e| anyhow::anyhow!("DataStore 打开失败（{}）：{e}", db_path.display()))?;
    let pricing = ds.pricing();

    match action {
        PricesAction::List => {
            let custom = pricing.list_custom();
            let downloaded = pricing.list_downloaded();
            let meta = pricing.meta();
            println!("价目表分层概况（查表优先级：自定义 > 下载 > 内置）：");
            println!("  自定义层: {} 条（最高优先）", custom.len());
            match &downloaded {
                Some(dl) => println!(
                    "  下载层:   {} 条（fetched_at={}，source={}，etag={}）",
                    dl.len(),
                    meta.fetched_at
                        .map(|t| chrono::Local
                            .timestamp_opt(t, 0)
                            .single()
                            .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
                            .unwrap_or_else(|| t.to_string()))
                        .unwrap_or_else(|| "从未".to_string()),
                    meta.source_url.as_deref().unwrap_or("-"),
                    meta.etag.as_deref().unwrap_or("-"),
                ),
                None => println!(
                    "  下载层:   未下载（`model prices update` 在线拉取，或 `import` 离线导入）"
                ),
            }
            println!(
                "  内置层:   {} 条（离线兜底）",
                nemesis_data::all_pricing().len()
            );
            if !custom.is_empty() {
                println!("自定义条目：");
                for p in &custom {
                    println!(
                        "  {:<32} in={:.4} out={:.4} cache_read={:.4} cache_creation={:.4} $/Mtok",
                        p.model_id,
                        p.input_cost_per_million,
                        p.output_cost_per_million,
                        p.cache_read_cost_per_million,
                        p.cache_creation_cost_per_million
                    );
                }
            }
        }
        PricesAction::Update { url } => {
            let shown = url
                .clone()
                .unwrap_or_else(|| nemesis_data::LITELLM_PRICE_URL.to_string());
            println!("正在拉取价目表（{shown}）...");
            match nemesis_web::pricing_sync::fetch_and_replace(pricing, url.as_deref()).await {
                Ok(r) if r.updated => {
                    println!(
                        "价目表已更新：{} 个模型 → {}",
                        r.entry_count,
                        db_path.parent().unwrap_or(Path::new(".")).display()
                    );
                }
                Ok(r) => println!(
                    "表已是最新（304 NotModified，{} 条，etag={}）",
                    r.entry_count,
                    r.etag.as_deref().unwrap_or("-")
                ),
                Err(e) => anyhow::bail!("{e}"),
            }
        }
        PricesAction::Import { file } => {
            let raw = std::fs::read_to_string(&file)
                .map_err(|e| anyhow::anyhow!("读取 {} 失败：{e}", file))?;
            let entries = nemesis_data::parse_litellm_json(&raw)
                .map_err(|e| anyhow::anyhow!("解析失败：{e}"))?;
            let n = entries.len();
            pricing
                .replace_downloaded(
                    entries,
                    nemesis_data::PricingMeta {
                        etag: None,
                        fetched_at: Some(chrono::Local::now().timestamp()),
                        source_url: Some(format!("manual-import:{}", file)),
                        entry_count: n,
                    },
                )
                .map_err(|e| anyhow::anyhow!("落盘失败：{e}"))?;
            println!("导入完成：{n} 个模型 → 下载层（内置层继续兜底）");
        }
        PricesAction::Add {
            model,
            input,
            output,
            cache_read,
            cache_creation,
            display,
        } => {
            let name = model.trim().to_string();
            if name.is_empty() {
                anyhow::bail!("模型名不能为空");
            }
            pricing
                .upsert_custom(nemesis_data::ModelPricing {
                    model_id: name.clone(),
                    display_name: display.unwrap_or_else(|| name.clone()),
                    input_cost_per_million: input,
                    output_cost_per_million: output,
                    cache_read_cost_per_million: cache_read,
                    cache_creation_cost_per_million: cache_creation,
                    max_input_tokens: None,
                    max_output_tokens: None,
                    aliases: Vec::new(),
                })
                .map_err(|e| anyhow::anyhow!("写入自定义条目失败：{e}"))?;
            println!("自定义条目已保存：{name}（in={input} out={output} $/Mtok，最高查表优先级）");
        }
        PricesAction::Remove { model } => {
            let removed = pricing
                .remove_custom(&model)
                .map_err(|e| anyhow::anyhow!("删除失败：{e}"))?;
            if removed {
                println!("已删除自定义条目：{model}");
            } else {
                anyhow::bail!("自定义条目不存在：{model}（`model prices list` 查看）");
            }
        }
    }
    Ok(())
}

/// Find a model entry in `model_list` by alias / full id / `vendor/<name>` suffix
/// and apply a mutation. Returns true if the entry was found and updated.
/// Test-visible delegate for `update_model_entry` (same crate, tests module).
#[cfg(test)]
pub(crate) fn update_model_entry_for_test<F>(cfg: &mut serde_json::Value, name: &str, f: F) -> bool
where
    F: FnOnce(&mut serde_json::Value),
{
    update_model_entry(cfg, name, f)
}

fn update_model_entry<F>(cfg: &mut serde_json::Value, name: &str, f: F) -> bool
where
    F: FnOnce(&mut serde_json::Value),
{
    let arr = match cfg.get_mut("model_list").and_then(|v| v.as_array_mut()) {
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
    let provider_arc: Arc<dyn nemesis_providers::router::LLMProvider> = provider;
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
        std::fs::write(
            &cfg_path,
            serde_json::to_string_pretty(&cfg_val).unwrap_or_default(),
        )?;
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

/// U16 (sixth batch): models.dev catalog fetch / cache / lookup.
pub mod catalog;

#[cfg(test)]
mod tests;
