//! Auth credential types.

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

/// Stored authentication credential.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthCredential {
    pub access_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Local>>,
    pub provider: String,
    pub auth_method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
}

impl AuthCredential {
    /// Check if the credential is expired.
    pub fn is_expired(&self) -> bool {
        match self.expires_at {
            Some(t) => Local::now() >= t,
            None => false,
        }
    }

    /// Check if the credential can be refreshed.
    pub fn can_refresh(&self) -> bool {
        self.refresh_token.as_ref().map_or(false, |t| !t.is_empty())
    }

    /// Check if the credential needs refresh (expires within 5 minutes).
    /// Mirrors Go AuthCredential.NeedsRefresh.
    pub fn needs_refresh(&self) -> bool {
        match self.expires_at {
            Some(t) => Local::now() + chrono::Duration::minutes(5) >= t,
            None => false,
        }
    }

    /// Login by pasting a token from stdin.
    ///
    /// Reads a token from the provided reader (e.g., stdin),
    /// trims whitespace, and returns an AuthCredential.
    pub fn login_paste_token(provider: &str, input: &str) -> Result<Self, String> {
        let token = input.trim();
        if token.is_empty() {
            return Err("token cannot be empty".to_string());
        }

        Ok(Self {
            access_token: token.to_string(),
            provider: provider.to_string(),
            auth_method: "token".to_string(),
            refresh_token: None,
            expires_at: None,
            account_id: None,
        })
    }
}

/// Get a display name for a provider.
pub fn provider_display_name(provider: &str) -> String {
    match provider {
        "anthropic" => "console.anthropic.com".to_string(),
        "openai" => "platform.openai.com".to_string(),
        _ => provider.to_string(),
    }
}

#[cfg(test)]
mod tests;
