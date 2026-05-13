//! Auth credential types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Stored authentication credential.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthCredential {
    pub access_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    pub provider: String,
    pub auth_method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
}

impl AuthCredential {
    /// Check if the credential is expired.
    pub fn is_expired(&self) -> bool {
        match self.expires_at {
            Some(t) => Utc::now() >= t,
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
            Some(t) => Utc::now() + chrono::Duration::minutes(5) >= t,
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
mod tests {
    use super::*;

    #[test]
    fn test_credential_not_expired() {
        let cred = AuthCredential {
            access_token: "test".to_string(),
            refresh_token: Some("refresh".to_string()),
            expires_at: Some(Utc::now() + chrono::Duration::hours(1)),
            provider: "openai".to_string(),
            auth_method: "oauth".to_string(),
            account_id: None,
        };
        assert!(!cred.is_expired());
        assert!(cred.can_refresh());
    }

    #[test]
    fn test_credential_expired() {
        let cred = AuthCredential {
            access_token: "test".to_string(),
            refresh_token: None,
            expires_at: Some(Utc::now() - chrono::Duration::hours(1)),
            provider: "openai".to_string(),
            auth_method: "api_key".to_string(),
            account_id: None,
        };
        assert!(cred.is_expired());
        assert!(!cred.can_refresh());
    }

    #[test]
    fn test_login_paste_token() {
        let cred = AuthCredential::login_paste_token("openai", "  sk-abc123  ").unwrap();
        assert_eq!(cred.access_token, "sk-abc123");
        assert_eq!(cred.provider, "openai");
        assert_eq!(cred.auth_method, "token");
        assert!(cred.refresh_token.is_none());
        assert!(cred.account_id.is_none());
    }

    #[test]
    fn test_login_paste_token_empty() {
        let result = AuthCredential::login_paste_token("openai", "   ");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty"));
    }

    #[test]
    fn test_login_paste_token_provider() {
        let cred = AuthCredential::login_paste_token("anthropic", "sk-ant-xyz").unwrap();
        assert_eq!(cred.provider, "anthropic");
    }

    #[test]
    fn test_provider_display_name() {
        assert_eq!(provider_display_name("anthropic"), "console.anthropic.com");
        assert_eq!(provider_display_name("openai"), "platform.openai.com");
        assert_eq!(provider_display_name("custom"), "custom");
    }

    #[test]
    fn test_needs_refresh_when_expiring_soon() {
        // Credential expiring in 3 minutes -> needs refresh (within 5 min window)
        let cred = AuthCredential {
            access_token: "test".to_string(),
            refresh_token: Some("refresh".to_string()),
            expires_at: Some(Utc::now() + chrono::Duration::minutes(3)),
            provider: "openai".to_string(),
            auth_method: "oauth".to_string(),
            account_id: None,
        };
        assert!(cred.needs_refresh());
    }

    #[test]
    fn test_needs_refresh_when_not_expiring_soon() {
        // Credential expiring in 1 hour -> does not need refresh
        let cred = AuthCredential {
            access_token: "test".to_string(),
            refresh_token: Some("refresh".to_string()),
            expires_at: Some(Utc::now() + chrono::Duration::hours(1)),
            provider: "openai".to_string(),
            auth_method: "oauth".to_string(),
            account_id: None,
        };
        assert!(!cred.needs_refresh());
    }

    #[test]
    fn test_needs_refresh_no_expiry() {
        // No expiry -> never needs refresh
        let cred = AuthCredential {
            access_token: "test".to_string(),
            refresh_token: Some("refresh".to_string()),
            expires_at: None,
            provider: "openai".to_string(),
            auth_method: "api_key".to_string(),
            account_id: None,
        };
        assert!(!cred.needs_refresh());
    }

    #[test]
    fn test_needs_refresh_already_expired() {
        // Already expired -> definitely needs refresh
        let cred = AuthCredential {
            access_token: "test".to_string(),
            refresh_token: Some("refresh".to_string()),
            expires_at: Some(Utc::now() - chrono::Duration::minutes(10)),
            provider: "openai".to_string(),
            auth_method: "oauth".to_string(),
            account_id: None,
        };
        assert!(cred.needs_refresh());
    }

    #[test]
    fn test_is_expired_no_expiry() {
        // No expiry -> not expired
        let cred = AuthCredential {
            access_token: "test".to_string(),
            refresh_token: None,
            expires_at: None,
            provider: "openai".to_string(),
            auth_method: "api_key".to_string(),
            account_id: None,
        };
        assert!(!cred.is_expired());
    }

    #[test]
    fn test_is_expired_exactly_now() {
        // Expiry set to exactly now (or slightly before) -> expired
        let cred = AuthCredential {
            access_token: "test".to_string(),
            refresh_token: None,
            expires_at: Some(Utc::now() - chrono::Duration::seconds(1)),
            provider: "openai".to_string(),
            auth_method: "oauth".to_string(),
            account_id: None,
        };
        assert!(cred.is_expired());
    }

    #[test]
    fn test_can_refresh_empty_string() {
        // Empty refresh token -> cannot refresh
        let cred = AuthCredential {
            access_token: "test".to_string(),
            refresh_token: Some("".to_string()),
            expires_at: None,
            provider: "openai".to_string(),
            auth_method: "oauth".to_string(),
            account_id: None,
        };
        assert!(!cred.can_refresh());
    }

    #[test]
    fn test_can_refresh_none() {
        let cred = AuthCredential {
            access_token: "test".to_string(),
            refresh_token: None,
            expires_at: None,
            provider: "openai".to_string(),
            auth_method: "api_key".to_string(),
            account_id: None,
        };
        assert!(!cred.can_refresh());
    }

    #[test]
    fn test_can_refresh_valid() {
        let cred = AuthCredential {
            access_token: "test".to_string(),
            refresh_token: Some("valid_refresh_token".to_string()),
            expires_at: None,
            provider: "openai".to_string(),
            auth_method: "oauth".to_string(),
            account_id: None,
        };
        assert!(cred.can_refresh());
    }

    #[test]
    fn test_login_paste_token_actual_empty() {
        let result = AuthCredential::login_paste_token("openai", "");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty"));
    }

    #[test]
    fn test_login_paste_token_fields() {
        let cred = AuthCredential::login_paste_token("myprovider", "  my-token-value  ").unwrap();
        assert_eq!(cred.access_token, "my-token-value");
        assert_eq!(cred.provider, "myprovider");
        assert_eq!(cred.auth_method, "token");
        assert!(cred.refresh_token.is_none());
        assert!(cred.expires_at.is_none());
        assert!(cred.account_id.is_none());
    }

    #[test]
    fn test_credential_serialization_roundtrip() {
        let cred = AuthCredential {
            access_token: "at_123".to_string(),
            refresh_token: Some("rt_456".to_string()),
            expires_at: Some(Utc::now()),
            provider: "openai".to_string(),
            auth_method: "oauth".to_string(),
            account_id: Some("acct_789".to_string()),
        };
        let json = serde_json::to_string(&cred).unwrap();
        let deserialized: AuthCredential = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.access_token, cred.access_token);
        assert_eq!(deserialized.refresh_token, cred.refresh_token);
        assert_eq!(deserialized.provider, cred.provider);
        assert_eq!(deserialized.auth_method, cred.auth_method);
        assert_eq!(deserialized.account_id, cred.account_id);
    }

    #[test]
    fn test_credential_serialization_skip_none() {
        let cred = AuthCredential {
            access_token: "at_123".to_string(),
            refresh_token: None,
            expires_at: None,
            provider: "openai".to_string(),
            auth_method: "api_key".to_string(),
            account_id: None,
        };
        let json = serde_json::to_string(&cred).unwrap();
        assert!(!json.contains("refresh_token"));
        assert!(!json.contains("expires_at"));
        assert!(!json.contains("account_id"));
    }

    #[test]
    fn test_credential_deserialization_with_optional_fields_missing() {
        let json = r#"{"access_token":"at","provider":"test","auth_method":"token"}"#;
        let cred: AuthCredential = serde_json::from_str(json).unwrap();
        assert_eq!(cred.access_token, "at");
        assert!(cred.refresh_token.is_none());
        assert!(cred.expires_at.is_none());
        assert!(cred.account_id.is_none());
    }

    #[test]
    fn test_provider_display_name_additional() {
        assert_eq!(provider_display_name("google"), "google");
        assert_eq!(provider_display_name("github"), "github");
        assert_eq!(provider_display_name(""), "");
    }
}
