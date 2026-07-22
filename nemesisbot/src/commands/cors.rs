//! CORS command - manage Cross-Origin Resource Sharing configuration.

use crate::common;
use anyhow::Result;

#[derive(clap::Subcommand)]
pub enum CorsAction {
    /// List all allowed origins
    List,
    /// Add an allowed origin
    Add {
        /// Origin URL to allow
        origin: String,
        /// Add as CDN domain instead of regular origin
        #[arg(long)]
        cdn: bool,
    },
    /// Remove an allowed origin
    Remove {
        /// Origin URL to remove
        origin: String,
        /// Remove from CDN domains instead of regular origins
        #[arg(long)]
        cdn: bool,
    },
    /// Manage development mode
    DevMode {
        #[command(subcommand)]
        action: CorsDevModeAction,
    },
    /// Show full CORS configuration
    Show,
    /// Validate if an origin is allowed
    Validate {
        /// Origin URL to validate
        origin: String,
    },
}

#[derive(clap::Subcommand)]
pub enum CorsDevModeAction {
    /// Enable development mode (allows all localhost origins)
    Enable,
    /// Disable development mode (strict whitelist only)
    Disable,
    /// Show current development mode status
    Status,
}

/// Default CORS configuration JSON.
fn default_cors_config() -> serde_json::Value {
    serde_json::json!({
        "allowed_origins": [],
        "allowed_cdn_domains": [],
        "development_mode": false,
        "allow_localhost": true,
        "allow_credentials": true,
        "max_age": 3600
    })
}

/// Load or create the CORS configuration.
fn load_or_create_cors(cors_path: &std::path::Path) -> Result<serde_json::Value> {
    if cors_path.exists() {
        let data = std::fs::read_to_string(cors_path)?;
        Ok(serde_json::from_str(&data)?)
    } else {
        let dir = cors_path.parent().unwrap();
        let _ = std::fs::create_dir_all(dir);
        let cfg = default_cors_config();
        std::fs::write(
            cors_path,
            serde_json::to_string_pretty(&cfg).unwrap_or_default(),
        )?;
        Ok(cfg)
    }
}

/// Save CORS configuration to disk.
fn save_cors(cors_path: &std::path::Path, cfg: &serde_json::Value) -> Result<()> {
    std::fs::write(
        cors_path,
        serde_json::to_string_pretty(cfg).unwrap_or_default(),
    )?;
    Ok(())
}

pub fn run(action: CorsAction, local: bool) -> Result<()> {
    let home = common::resolve_home(local);
    let cors_path = common::cors_config_path(&home);

    match action {
        CorsAction::List => {
            println!("CORS Configuration");
            println!("===================");

            if !cors_path.exists() {
                println!("  No CORS configuration found.");
                println!("  Initialize with: nemesisbot cors add <origin>");
                return Ok(());
            }

            let cfg = load_or_create_cors(&cors_path)?;

            println!("  Allowed origins:");
            if let Some(origins) = cfg.get("allowed_origins").and_then(|v| v.as_array()) {
                if origins.is_empty() {
                    println!("    (none)");
                } else {
                    for o in origins {
                        println!("    - {}", o);
                    }
                }
            }

            println!("  CDN domains:");
            if let Some(cdns) = cfg.get("allowed_cdn_domains").and_then(|v| v.as_array()) {
                if cdns.is_empty() {
                    println!("    (none)");
                } else {
                    for c in cdns {
                        println!("    - {}", c);
                    }
                }
            }

            let dev_mode = cfg
                .get("development_mode")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let allow_localhost = cfg
                .get("allow_localhost")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let allow_credentials = cfg
                .get("allow_credentials")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let max_age = cfg.get("max_age").and_then(|v| v.as_i64()).unwrap_or(3600);

            println!(
                "  Development mode: {}",
                if dev_mode { "ENABLED" } else { "disabled" }
            );
            println!(
                "  Allow localhost:  {}",
                if allow_localhost { "yes" } else { "no" }
            );
            println!(
                "  Allow credentials: {}",
                if allow_credentials { "yes" } else { "no" }
            );
            println!("  Max age:          {}s", max_age);
        }

        CorsAction::Add { origin, cdn } => {
            // Validate URL format for non-CDN origins (matching Go behavior)
            if !cdn && !origin.starts_with("http://") && !origin.starts_with("https://") {
                anyhow::bail!(
                    "Invalid origin URL '{}'. Must start with http:// or https://",
                    origin
                );
            }

            let mut cfg = load_or_create_cors(&cors_path)?;

            let key = if cdn {
                "allowed_cdn_domains"
            } else {
                "allowed_origins"
            };

            if let Some(arr) = cfg.get_mut(key).and_then(|v| v.as_array_mut()) {
                if arr.iter().any(|v| v.as_str() == Some(&origin)) {
                    println!(
                        "Origin '{}' already exists in {}.",
                        origin,
                        if cdn {
                            "CDN domains"
                        } else {
                            "allowed origins"
                        }
                    );
                    return Ok(());
                }
                arr.push(serde_json::Value::String(origin.clone()));
            }

            save_cors(&cors_path, &cfg)?;
            println!(
                "Added '{}' to {}.",
                origin,
                if cdn {
                    "CDN domains"
                } else {
                    "allowed origins"
                }
            );
            println!("Changes will be automatically reloaded within 30 seconds.");
        }

        CorsAction::Remove { origin, cdn } => {
            if !cors_path.exists() {
                println!("No CORS configuration found.");
                return Ok(());
            }

            let mut cfg: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&cors_path)?)?;

            let key = if cdn {
                "allowed_cdn_domains"
            } else {
                "allowed_origins"
            };

            let mut found = false;
            if let Some(arr) = cfg.get_mut(key).and_then(|v| v.as_array_mut()) {
                let before = arr.len();
                arr.retain(|v| v.as_str() != Some(&origin));
                found = arr.len() < before;
            }

            save_cors(&cors_path, &cfg)?;

            if found {
                println!(
                    "Removed '{}' from {}.",
                    origin,
                    if cdn {
                        "CDN domains"
                    } else {
                        "allowed origins"
                    }
                );
            } else {
                println!(
                    "'{}' not found in {}.",
                    origin,
                    if cdn {
                        "CDN domains"
                    } else {
                        "allowed origins"
                    }
                );
            }
        }

        CorsAction::DevMode { action } => match action {
            CorsDevModeAction::Enable => {
                let mut cfg = load_or_create_cors(&cors_path)?;
                if let Some(obj) = cfg.as_object_mut() {
                    obj.insert(
                        "development_mode".to_string(),
                        serde_json::Value::Bool(true),
                    );
                }
                save_cors(&cors_path, &cfg)?;
                println!("Development mode ENABLED.");
                println!("WARNING: Development mode allows all localhost origins.");
                println!("         Should NOT be used in production!");
            }
            CorsDevModeAction::Disable => {
                if !cors_path.exists() {
                    println!("No CORS configuration found. Development mode is already disabled.");
                    return Ok(());
                }
                let mut cfg: serde_json::Value =
                    serde_json::from_str(&std::fs::read_to_string(&cors_path)?)?;
                if let Some(obj) = cfg.as_object_mut() {
                    obj.insert(
                        "development_mode".to_string(),
                        serde_json::Value::Bool(false),
                    );
                }
                save_cors(&cors_path, &cfg)?;
                println!("Development mode DISABLED. Using strict whitelist.");
            }
            CorsDevModeAction::Status => {
                let enabled = if cors_path.exists() {
                    let cfg = load_or_create_cors(&cors_path)?;
                    cfg.get("development_mode")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
                } else {
                    false
                };
                println!(
                    "Development mode: {}",
                    if enabled { "ENABLED" } else { "DISABLED" }
                );
                if enabled {
                    println!("  All localhost origins are accepted.");
                } else {
                    println!("  Only explicitly whitelisted origins are accepted.");
                }
            }
        },

        CorsAction::Show => {
            if !cors_path.exists() {
                println!("No CORS configuration found.");
                println!("Initialize with: nemesisbot cors add <origin>");
                return Ok(());
            }
            let contents = std::fs::read_to_string(&cors_path)?;
            println!("CORS Configuration ({})", cors_path.display());
            println!("{}", contents);
        }

        CorsAction::Validate { origin } => {
            println!("Validating origin: {}", origin);

            if !cors_path.exists() {
                println!("  Result: DENIED (no CORS configuration)");
                return Ok(());
            }

            let cfg = load_or_create_cors(&cors_path)?;

            // Check explicit allowed origins
            let mut allowed = false;
            let mut match_source = String::new();

            if let Some(origins) = cfg.get("allowed_origins").and_then(|v| v.as_array()) {
                for o in origins {
                    if o.as_str() == Some(&origin) {
                        allowed = true;
                        match_source = "allowed_origins".to_string();
                        break;
                    }
                }
            }

            // Check CDN domains
            if !allowed {
                if let Some(cdns) = cfg.get("allowed_cdn_domains").and_then(|v| v.as_array()) {
                    for c in cdns {
                        if let Some(domain) = c.as_str() {
                            if origin == domain
                                || origin.ends_with(&format!(".{}", domain))
                                || origin.starts_with(&format!("*{}", domain))
                            {
                                allowed = true;
                                match_source = "allowed_cdn_domains".to_string();
                                break;
                            }
                        }
                    }
                }
            }

            // Check dev mode / localhost
            if !allowed {
                let dev_mode = cfg
                    .get("development_mode")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let allow_localhost = cfg
                    .get("allow_localhost")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);

                if dev_mode || allow_localhost {
                    let lower = origin.to_lowercase();
                    if lower.starts_with("http://localhost")
                        || lower.starts_with("http://127.0.0.1")
                        || lower.contains("localhost:")
                    {
                        allowed = true;
                        match_source = if dev_mode {
                            "development_mode + localhost".to_string()
                        } else {
                            "allow_localhost".to_string()
                        };
                    }
                }
            }

            if allowed {
                println!("  Result: ALLOWED (matched: {})", match_source);
            } else {
                println!("  Result: DENIED (not in whitelist)");
                println!("  Add with: nemesisbot cors add '{}'", origin);
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests;
