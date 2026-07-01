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

#[cfg(test)]
mod tests;
