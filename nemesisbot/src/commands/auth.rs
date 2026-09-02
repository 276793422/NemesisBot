//! Auth command - manage authentication (login, logout, status).
//!
//! Integrates with nemesis-auth crate for OAuth/device-code flows,
//! credential storage, and token management.

use crate::common;
use anyhow::Result;

/// OpenAI OAuth config with an optional test seam.
///
/// `NEMESISBOT_OAUTH_ISSUER`（未设置时行为与 `OAuthProviderConfig::openai()`
/// 完全一致）覆盖 issuer endpoint，使 CLI 层 OAuth 流能指向本地 mock——
/// crate 层（nemesis-auth/src/oauth/tests.rs）已有全套 issuer 可注入的
/// mock 测试，此前只差 CLI 层这个硬编码常量没有注入口。
fn oauth_openai_config_or_override() -> nemesis_auth::OAuthProviderConfig {
    let mut cfg = nemesis_auth::OAuthProviderConfig::openai();
    if let Ok(issuer) = std::env::var("NEMESISBOT_OAUTH_ISSUER")
        && !issuer.is_empty()
    {
        cfg.issuer = issuer;
    }
    cfg
}

#[derive(clap::Subcommand)]
pub enum AuthAction {
    /// Login via OAuth or paste token
    Login {
        /// Provider name (openai, anthropic)
        #[arg(short, long)]
        provider: String,
        /// Use device code flow instead of browser
        #[arg(long)]
        device_code: bool,
    },
    /// Remove stored credentials
    Logout {
        /// Provider to logout from (omit for all)
        #[arg(short, long)]
        provider: Option<String>,
    },
    /// Show current auth status
    Status,
}

pub async fn run(action: AuthAction, local: bool) -> Result<()> {
    let home = common::resolve_home(local);
    let auth_path = home.join("auth.json");

    match action {
        AuthAction::Login {
            provider,
            device_code,
        } => {
            let valid_providers = ["openai", "anthropic"];
            if !valid_providers.contains(&provider.as_str()) {
                println!("Unsupported provider: {}", provider);
                println!("Supported providers: {}", valid_providers.join(", "));
                return Ok(());
            }

            println!("Logging in to {}...", provider);

            // Try real OAuth flow for OpenAI
            if provider == "openai" {
                let store = nemesis_auth::AuthStore::new(&auth_path.to_string_lossy());
                let oauth_config = oauth_openai_config_or_override();

                let result = if device_code {
                    println!("  Using device code flow...");
                    oauth_config.login_device_code().await
                } else {
                    println!("  Using browser-based OAuth flow...");
                    oauth_config.login_browser().await
                };

                match result {
                    Ok(cred) => {
                        store
                            .save(&provider, cred)
                            .map_err(|e| anyhow::anyhow!("{}", e))?;
                        println!("  Logged in to {} successfully.", provider);
                        println!("  Credentials saved to: {}", auth_path.display());
                        return Ok(());
                    }
                    Err(e) => {
                        println!("  OAuth flow failed: {}", e);
                        println!("  Falling back to paste-token mode.");
                    }
                }
            }

            // Fallback: paste-token mode (for all providers or when OAuth fails)
            use std::io::{self, Write};
            let display_name = nemesis_auth::provider_display_name(&provider);
            print!("Enter {} API token (from {}): ", provider, display_name);
            io::stdout().flush().ok();
            let mut token = String::new();
            io::stdin().read_line(&mut token).ok();
            let token = token.trim().to_string();

            if token.is_empty() {
                println!("No token entered. Login cancelled.");
                return Ok(());
            }

            let cred = nemesis_auth::AuthCredential::login_paste_token(&provider, &token)
                .map_err(|e| anyhow::anyhow!("{}", e))?;

            let store = nemesis_auth::AuthStore::new(&auth_path.to_string_lossy());
            store
                .save(&provider, cred)
                .map_err(|e| anyhow::anyhow!("{}", e))?;

            println!("  Token saved to: {}", auth_path.display());
            println!("  Logged in to {} successfully.", provider);
        }
        AuthAction::Logout { provider } => {
            if !auth_path.exists() {
                println!("No credentials stored.");
                return Ok(());
            }

            let store = nemesis_auth::AuthStore::new(&auth_path.to_string_lossy());
            match provider {
                Some(p) => {
                    if store.get(&p).is_some() {
                        store.remove(&p).map_err(|e| anyhow::anyhow!("{}", e))?;
                        println!("Logged out from {}", p);
                    } else {
                        println!("No credentials found for {}", p);
                    }
                }
                None => {
                    store.delete_all().map_err(|e| anyhow::anyhow!("{}", e))?;
                    println!("Logged out from all providers.");
                }
            }
        }
        AuthAction::Status => {
            println!("Authentication Status");
            println!("=====================");
            if auth_path.exists() {
                let store = nemesis_auth::AuthStore::new(&auth_path.to_string_lossy());
                let providers = store.list_providers();
                if providers.is_empty() {
                    println!("  No authenticated providers.");
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
                            println!("  {} ({})", display, cred.auth_method);
                            println!("    Status: {}", status);
                            if let Some(ref account) = cred.account_id
                                && !account.is_empty()
                            {
                                println!("    Account: {}", account);
                            }
                            if let Some(expires) = cred.expires_at {
                                println!("    Expires: {}", expires.format("%Y-%m-%d %H:%M UTC"));
                            }
                        }
                    }
                }
            } else {
                println!("  No credentials stored.");
                println!("  Login with: nemesisbot auth login --provider <name>");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
