//! Read Codex CLI credentials from ~/.codex/auth.json.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::SystemTime;

/// The auth.json structure from the Codex CLI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexCliAuth {
    pub tokens: CodexCliTokens,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexCliTokens {
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
    #[serde(default)]
    pub account_id: String,
}

/// Credentials read from the Codex CLI auth file.
#[derive(Debug, Clone)]
pub struct CodexCredentials {
    pub access_token: String,
    pub account_id: String,
    pub expires_at: SystemTime,
}

/// Resolve the path to the Codex CLI auth.json file.
/// Checks `CODEX_HOME` env var first, then falls back to `~/.codex/auth.json`.
pub fn resolve_codex_auth_path() -> Result<PathBuf, String> {
    if let Ok(codex_home) = std::env::var("CODEX_HOME") {
        return Ok(PathBuf::from(codex_home).join("auth.json"));
    }

    let home = dirs::home_dir().ok_or("cannot determine home directory")?;
    Ok(home.join(".codex").join("auth.json"))
}

/// Read OAuth tokens from the Codex CLI's auth.json file.
/// Expiry is estimated as file modification time + 1 hour (same approach as moltbot).
pub fn read_codex_cli_credentials() -> Result<CodexCredentials, String> {
    let auth_path = resolve_codex_auth_path()?;

    let data = std::fs::read_to_string(&auth_path)
        .map_err(|e| format!("reading {}: {}", auth_path.display(), e))?;

    let auth: CodexCliAuth =
        serde_json::from_str(&data).map_err(|e| format!("parsing {}: {}", auth_path.display(), e))?;

    if auth.tokens.access_token.is_empty() {
        return Err(format!("no access_token in {}", auth_path.display()));
    }

    let metadata = std::fs::metadata(&auth_path);
    let expires_at = match metadata {
        Ok(meta) => meta
            .modified()
            .unwrap_or_else(|_| SystemTime::now())
            + std::time::Duration::from_secs(3600),
        Err(_) => SystemTime::now() + std::time::Duration::from_secs(3600),
    };

    Ok(CodexCredentials {
        access_token: auth.tokens.access_token,
        account_id: auth.tokens.account_id,
        expires_at,
    })
}

/// Create a token source function that reads from ~/.codex/auth.json.
/// This allows the CodexProvider to reuse Codex CLI credentials.
pub fn create_codex_cli_token_source(
) -> Box<dyn Fn() -> Result<(String, String), String> + Send + Sync> {
    Box::new(|| {
        let creds = read_codex_cli_credentials()?;
        if SystemTime::now() > creds.expires_at {
            return Err(
                "codex cli credentials expired (auth.json last modified > 1h ago). Run: codex login"
                    .to_string(),
            );
        }
        Ok((creds.access_token, creds.account_id))
    })
}

#[cfg(test)]
mod tests;
