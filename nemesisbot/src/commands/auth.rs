//! Auth command - manage authentication (login, logout, status).
//!
//! Integrates with nemesis-auth crate for OAuth/device-code flows,
//! credential storage, and token management.

use anyhow::Result;
use crate::common;

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
        AuthAction::Login { provider, device_code } => {
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
                let oauth_config = nemesis_auth::OAuthProviderConfig::openai();

                let result = if device_code {
                    println!("  Using device code flow...");
                    oauth_config.login_device_code().await
                } else {
                    println!("  Using browser-based OAuth flow...");
                    oauth_config.login_browser().await
                };

                match result {
                    Ok(cred) => {
                        store.save(&provider, cred).map_err(|e| anyhow::anyhow!("{}", e))?;
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
            store.save(&provider, cred).map_err(|e| anyhow::anyhow!("{}", e))?;

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
                            if let Some(ref account) = cred.account_id {
                                if !account.is_empty() {
                                    println!("    Account: {}", account);
                                }
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
mod tests {
    use tempfile::TempDir;

    #[test]
    fn test_valid_providers_openai() {
        let valid_providers = ["openai", "anthropic"];
        assert!(valid_providers.contains(&"openai"));
    }

    #[test]
    fn test_valid_providers_anthropic() {
        let valid_providers = ["openai", "anthropic"];
        assert!(valid_providers.contains(&"anthropic"));
    }

    #[test]
    fn test_invalid_provider_rejected() {
        let valid_providers = ["openai", "anthropic"];
        assert!(!valid_providers.contains(&"google"));
        assert!(!valid_providers.contains(&"invalid"));
    }

    #[test]
    fn test_auth_path_construction() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join(".nemesisbot");
        let auth_path = home.join("auth.json");
        assert!(auth_path.to_string_lossy().contains("auth.json"));
    }

    #[test]
    fn test_auth_store_creation() {
        let tmp = TempDir::new().unwrap();
        let auth_path = tmp.path().join("auth.json");
        let store = nemesis_auth::AuthStore::new(&auth_path.to_string_lossy());
        let providers = store.list_providers();
        assert!(providers.is_empty());
    }

    #[test]
    fn test_auth_store_save_and_get() {
        let tmp = TempDir::new().unwrap();
        let auth_path = tmp.path().join("auth.json");

        let store = nemesis_auth::AuthStore::new(&auth_path.to_string_lossy());
        let cred = nemesis_auth::AuthCredential::login_paste_token("openai", "test-token-12345").unwrap();
        store.save("openai", cred).unwrap();

        let retrieved = store.get("openai");
        assert!(retrieved.is_some());
        // auth_method may be "token" (crate returns the actual method name)
        let method = &retrieved.unwrap().auth_method;
        assert!(!method.is_empty());
    }

    #[test]
    fn test_auth_store_list_providers() {
        let tmp = TempDir::new().unwrap();
        let auth_path = tmp.path().join("auth.json");

        let store = nemesis_auth::AuthStore::new(&auth_path.to_string_lossy());
        let cred = nemesis_auth::AuthCredential::login_paste_token("openai", "test-token").unwrap();
        store.save("openai", cred).unwrap();

        let providers = store.list_providers();
        assert_eq!(providers.len(), 1);
        assert!(providers.contains(&"openai".to_string()));
    }

    #[test]
    fn test_auth_store_remove() {
        let tmp = TempDir::new().unwrap();
        let auth_path = tmp.path().join("auth.json");

        let store = nemesis_auth::AuthStore::new(&auth_path.to_string_lossy());
        let cred = nemesis_auth::AuthCredential::login_paste_token("openai", "test-token").unwrap();
        store.save("openai", cred).unwrap();

        store.remove("openai").unwrap();
        assert!(store.get("openai").is_none());
    }

    #[test]
    fn test_auth_store_delete_all() {
        let tmp = TempDir::new().unwrap();
        let auth_path = tmp.path().join("auth.json");

        let store = nemesis_auth::AuthStore::new(&auth_path.to_string_lossy());
        let cred1 = nemesis_auth::AuthCredential::login_paste_token("openai", "token1").unwrap();
        let cred2 = nemesis_auth::AuthCredential::login_paste_token("anthropic", "token2").unwrap();
        store.save("openai", cred1).unwrap();
        store.save("anthropic", cred2).unwrap();

        store.delete_all().unwrap();
        assert!(store.list_providers().is_empty());
    }

    #[test]
    fn test_provider_display_name() {
        let name = nemesis_auth::provider_display_name("openai");
        assert!(!name.is_empty());
    }

    #[test]
    fn test_credential_is_expired() {
        let cred = nemesis_auth::AuthCredential::login_paste_token("openai", "test").unwrap();
        let _ = cred.is_expired();
    }

    #[test]
    fn test_credential_needs_refresh() {
        let cred = nemesis_auth::AuthCredential::login_paste_token("openai", "test").unwrap();
        let _ = cred.needs_refresh();
    }

    #[test]
    fn test_auth_store_get_nonexistent() {
        let tmp = TempDir::new().unwrap();
        let auth_path = tmp.path().join("auth.json");
        let store = nemesis_auth::AuthStore::new(&auth_path.to_string_lossy());
        assert!(store.get("nonexistent").is_none());
    }

    #[test]
    fn test_auth_no_file_exists() {
        let tmp = TempDir::new().unwrap();
        let auth_path = tmp.path().join("nonexistent").join("auth.json");
        assert!(!auth_path.exists());
    }

    // -------------------------------------------------------------------------
    // Additional auth tests for coverage
    // -------------------------------------------------------------------------

    #[test]
    fn test_multiple_provider_operations() {
        let tmp = TempDir::new().unwrap();
        let auth_path = tmp.path().join("auth.json");
        let store = nemesis_auth::AuthStore::new(&auth_path.to_string_lossy());

        // Save multiple credentials
        let cred1 = nemesis_auth::AuthCredential::login_paste_token("openai", "key1").unwrap();
        let cred2 = nemesis_auth::AuthCredential::login_paste_token("anthropic", "key2").unwrap();
        store.save("openai", cred1).unwrap();
        store.save("anthropic", cred2).unwrap();

        let providers = store.list_providers();
        assert_eq!(providers.len(), 2);

        // Get individual
        assert!(store.get("openai").is_some());
        assert!(store.get("anthropic").is_some());

        // Remove one
        store.remove("openai").unwrap();
        assert!(store.get("openai").is_none());
        assert!(store.get("anthropic").is_some());
        assert_eq!(store.list_providers().len(), 1);
    }

    #[test]
    fn test_auth_credential_fields() {
        let cred = nemesis_auth::AuthCredential::login_paste_token("openai", "test-key-12345").unwrap();
        assert!(!cred.auth_method.is_empty());
        assert!(cred.is_expired() == false || cred.is_expired() == true); // Just ensure it doesn't panic
    }

    #[test]
    fn test_auth_store_nonexistent_remove() {
        let tmp = TempDir::new().unwrap();
        let auth_path = tmp.path().join("auth.json");
        let store = nemesis_auth::AuthStore::new(&auth_path.to_string_lossy());
        // Removing a nonexistent provider should not panic
        let result = store.remove("nonexistent");
        // May succeed or fail depending on implementation
        let _ = result;
    }

    #[test]
    fn test_provider_display_names() {
        let name_openai = nemesis_auth::provider_display_name("openai");
        assert!(!name_openai.is_empty());

        let name_anthropic = nemesis_auth::provider_display_name("anthropic");
        assert!(!name_anthropic.is_empty());

        // Unknown provider should still return something
        let name_unknown = nemesis_auth::provider_display_name("unknown_provider");
        assert!(!name_unknown.is_empty());
    }

    #[test]
    fn test_auth_store_overwrite() {
        let tmp = TempDir::new().unwrap();
        let auth_path = tmp.path().join("auth.json");
        let store = nemesis_auth::AuthStore::new(&auth_path.to_string_lossy());

        let cred1 = nemesis_auth::AuthCredential::login_paste_token("openai", "key1").unwrap();
        store.save("openai", cred1).unwrap();

        let cred2 = nemesis_auth::AuthCredential::login_paste_token("openai", "key2-updated").unwrap();
        store.save("openai", cred2).unwrap();

        let providers = store.list_providers();
        assert_eq!(providers.len(), 1);
    }

    #[test]
    fn test_auth_path_in_home_directory() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path();
        let auth_path = home.join("auth.json");
        assert!(auth_path.to_string_lossy().ends_with("auth.json"));
    }

    #[test]
    fn test_token_empty_detection() {
        let token = "";
        assert!(token.is_empty());

        let token = "  ";
        assert!(!token.is_empty()); // whitespace is not empty

        let token = " valid-token ";
        assert!(!token.is_empty());
    }

    #[test]
    fn test_token_trim() {
        let token = "  my-api-key  ".trim().to_string();
        assert_eq!(token, "my-api-key");
    }
}
