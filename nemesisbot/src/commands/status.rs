//! Status command - show nemesisbot system status.

use crate::common;
use anyhow::Result;

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
        let data = nemesis_utils::fsutil::read_config_string(&cfg_path)?;
        if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&data) {
            // Default model
            if let Some(model) = cfg.get("default_model").and_then(|v| v.as_str()) {
                println!("  Default model: {}", model);
            }

            // Count models
            if let Some(models) = cfg.get("model_list").and_then(|v| v.as_array())
                && !models.is_empty()
            {
                println!();
                println!("  Configured Models:");
                // BTreeMap（非 HashMap）：provider 分组聚合按字典序输出——
                // user-visible listings must be deterministic（BUG-20261114-050，
                // 与 AuthStore list_providers 排序约定同源；HashMap 迭代序
                // 逐进程漂移，同机两次 status 输出顺序不同）。聚合逻辑
                // 单一真相源 = aggregate_provider_stats。
                for (provider, (count, has_key)) in &aggregate_provider_stats(models) {
                    println!(
                        "    {}: {} model(s), API key: {}",
                        provider,
                        count,
                        if *has_key { "configured" } else { "not set" }
                    );
                }
            }

            // Security
            let security_enabled = cfg
                .get("security")
                .and_then(|s| s.get("enabled"))
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            println!();
            println!(
                "  Security: {}",
                if security_enabled {
                    "enabled"
                } else {
                    "disabled"
                }
            );

            // Forge
            let forge_enabled = cfg
                .get("forge")
                .and_then(|f| f.get("enabled"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            println!(
                "  Forge: {}",
                if forge_enabled { "enabled" } else { "disabled" }
            );
        }
    }

    // Authentication
    println!();
    println!("  Authentication:");
    #[cfg(feature = "auth")]
    {
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
    }
    #[cfg(not(feature = "auth"))]
    {
        println!("    (auth feature disabled in this build)");
    }

    println!();
    println!("  Home directory: {}", home.display());

    Ok(())
}

/// model_list → provider 分组聚合（模型数 + 是否有任一 API key）。
/// BTreeMap 保证输出序确定性（BUG-20261114-050：HashMap 迭代序逐进程
/// 漂移，违反 user-visible listings 确定性约定）；provider 段小写归一
/// （大小写形态归并同组）；无 slash 前缀的模型名不计入任何 provider
/// （与既有行为一致）。单一真相源：run() 与测试直测同一函数。
fn aggregate_provider_stats(
    models: &[serde_json::Value],
) -> std::collections::BTreeMap<String, (usize, bool)> {
    let mut provider_counts: std::collections::BTreeMap<String, (usize, bool)> =
        std::collections::BTreeMap::new();
    for m in models {
        let model = m.get("model").and_then(|v| v.as_str()).unwrap_or("");
        let parts: Vec<&str> = model.splitn(2, '/').collect();
        if parts.len() == 2 {
            let provider = parts[0].to_lowercase();
            let has_key = m
                .get("api_key")
                .and_then(|v| v.as_str())
                .map(|k| !k.is_empty())
                .unwrap_or(false);
            let entry = provider_counts.entry(provider).or_insert((0, false));
            entry.0 += 1;
            entry.1 = entry.1 || has_key;
        }
    }
    provider_counts
}

#[cfg(test)]
mod tests;
