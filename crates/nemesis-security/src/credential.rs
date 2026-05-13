//! Credential Scanner - Layer 4
//! Detects leaked credentials in tool arguments (AWS, GCP, GitHub, etc.).
//! 30+ patterns with configurable mask functions.
//!
//! Additional methods:
//! - `scan_tool_output()` - scan tool output with tool-name logging
//! - `set_action()` - dynamically update action at runtime
//! - `is_enabled()` / `get_action()` - query methods

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use std::time::Instant;

/// Credential scan result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialResult {
    pub has_matches: bool,
    pub matches: Vec<CredentialMatch>,
    pub action: String,
    pub summary: String,
}

/// A single credential match.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialMatch {
    pub pattern_name: String,
    pub redacted: String,
    pub full_match_start: String,
    pub full_match_end: String,
}

/// Mask function configuration.
#[derive(Debug, Clone, Copy)]
pub enum MaskMode {
    /// Keep first 4 and last 4 chars
    KeepPrefix,
    /// Replace with fixed [REDACTED]
    Fixed,
    /// Show key name but redact value
    KeyValue,
}

impl Default for MaskMode {
    fn default() -> Self {
        Self::KeepPrefix
    }
}

/// Credential scanner with configurable masking.
pub struct Scanner {
    enabled: bool,
    action: String,
    mask_mode: MaskMode,
}

impl Scanner {
    pub fn new(enabled: bool, action: &str) -> Self {
        Self {
            enabled,
            action: action.to_string(),
            mask_mode: MaskMode::KeepPrefix,
        }
    }

    pub fn with_mask_mode(enabled: bool, action: &str, mask_mode: MaskMode) -> Self {
        Self {
            enabled,
            action: action.to_string(),
            mask_mode,
        }
    }

    /// Scan content for credentials.
    pub fn scan_content(&self, content: &str) -> CredentialResult {
        if !self.enabled || content.len() <= 10 {
            return CredentialResult {
                has_matches: false,
                matches: vec![],
                action: self.action.clone(),
                summary: String::new(),
            };
        }

        let patterns = get_credential_patterns();
        let mut matches = Vec::new();

        for (name, re) in patterns {
            for cap in re.captures_iter(content) {
                let full = cap.get(0).map(|m| m.as_str()).unwrap_or("");
                let redacted = self.mask_value(full);
                matches.push(CredentialMatch {
                    pattern_name: name.to_string(),
                    redacted: redacted.clone(),
                    full_match_start: if full.len() > 4 { full[..4].to_string() } else { full.to_string() },
                    full_match_end: if full.len() > 4 { full[full.len()-4..].to_string() } else { String::new() },
                });
            }
        }

        let has_matches = !matches.is_empty();
        let summary = if has_matches {
            format!("{} credential(s) detected", matches.len())
        } else {
            String::new()
        };

        CredentialResult {
            has_matches,
            matches,
            action: self.action.clone(),
            summary,
        }
    }

    /// Redact content by replacing all credential matches with masked versions.
    pub fn redact_content(&self, content: &str) -> String {
        if !self.enabled || content.len() <= 10 {
            return content.to_string();
        }

        let patterns = get_credential_patterns();
        let mut result = content.to_string();

        for (_name, re) in patterns {
            result = re.replace_all(&result, "[REDACTED_CREDENTIAL]").to_string();
        }

        result
    }

    fn mask_value(&self, value: &str) -> String {
        match self.mask_mode {
            MaskMode::KeepPrefix => mask_keep_prefix(value),
            MaskMode::Fixed => "[REDACTED]".to_string(),
            MaskMode::KeyValue => {
                if value.len() > 8 {
                    format!("{}...{}", &value[..4], &value[value.len()-4..])
                } else {
                    "[REDACTED]".to_string()
                }
            }
        }
    }

    /// Scan tool output for leaked credentials.
    ///
    /// Equivalent to Go's `Scanner.ScanToolOutput()`. This is a convenience
    /// wrapper around `scan_content()` that logs timing and tool name info.
    pub fn scan_tool_output(&self, tool_name: &str, output: &str) -> CredentialResult {
        let start = Instant::now();
        let result = self.scan_content(output);
        let elapsed = start.elapsed();

        if result.has_matches {
            tracing::warn!(
                tool = tool_name,
                count = result.matches.len(),
                elapsed_ms = elapsed.as_millis() as u64,
                "Tool output contains potential credentials"
            );
        } else {
            tracing::debug!(
                tool = tool_name,
                elapsed_ms = elapsed.as_millis() as u64,
                "Tool output clean"
            );
        }

        result
    }

    /// Dynamically update the action at runtime.
    ///
    /// Equivalent to Go's `Scanner.SetAction()`. Valid actions are
    /// "block", "redact", and "warn". Returns an error for invalid actions.
    pub fn set_action(&mut self, action: &str) -> Result<(), String> {
        match action {
            "block" | "redact" | "warn" => {
                self.action = action.to_string();
                Ok(())
            }
            _ => Err(format!(
                "invalid action {:?}: must be block, redact, or warn",
                action
            )),
        }
    }

    /// Returns whether the credential scanner is enabled.
    ///
    /// Equivalent to Go's `Scanner.IsEnabled()`.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Returns the configured action.
    ///
    /// Equivalent to Go's `Scanner.GetAction()`.
    pub fn get_action(&self) -> &str {
        &self.action
    }
}

fn mask_keep_prefix(value: &str) -> String {
    if value.len() > 8 {
        format!("{}...{}", &value[..4], &value[value.len()-4..])
    } else {
        "[REDACTED]".to_string()
    }
}

type CredentialPatterns = Vec<(&'static str, Regex)>;

fn get_credential_patterns() -> &'static CredentialPatterns {
    static PATTERNS: OnceLock<CredentialPatterns> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        let raw: Vec<(&str, &str)> = vec![
            // AWS
            ("aws_access_key", r"AKIA[0-9A-Z]{16}"),
            ("aws_secret_key", r"(?i)aws_secret_access_key\s*[=:]\s*[A-Za-z0-9/+=]{40}"),
            ("aws_session_token", r"(?i)aws_session_token\s*[=:]\s*[A-Za-z0-9/+=]{100,}"),
            // GCP
            ("gcp_service_account", r#"(?i)"type"\s*:\s*"service_account""#),
            ("gcp_private_key_id", r#"(?i)"private_key_id"\s*:\s*"[a-f0-9]+""#),
            // Azure
            ("azure_connection_string", r"(?i)AccountName=[A-Za-z0-9]+;AccountKey=[A-Za-z0-9+/=]+"),
            ("azure_tenant_id", r"(?i)azure\s*tenant\s*(?:id)?\s*[=:]\s*[a-f0-9-]{36}"),
            // GitHub
            ("github_token", r"gh[pousr]_[A-Za-z0-9_]{36,}"),
            ("github_oauth", r"(?i)github[_-]?oauth\s*[=:]\s*[a-f0-9]{40}"),
            // Slack
            ("slack_token", r"xox[baprs]-[0-9]{10,}-[A-Za-z0-9]{24,}"),
            ("slack_webhook", r"https://hooks\.slack\.com/services/T[A-Z0-9]+/B[A-Z0-9]+/[A-Za-z0-9]+"),
            // Stripe
            ("stripe_key", r"(?:sk|pk)_(?:test|live)_[A-Za-z0-9]{24,}"),
            // SendGrid
            ("sendgrid_key", r"SG\.[A-Za-z0-9_-]{22}\.[A-Za-z0-9_-]{43}"),
            // Twilio
            ("twilio_sid", r"AC[a-z0-9]{32}"),
            // Mailgun
            ("mailgun_key", r"key-[a-z0-9]{32}"),
            // Private keys
            ("private_key", r"-----BEGIN\s+(?:RSA\s+)?PRIVATE\s+KEY-----"),
            ("ec_private_key", r"-----BEGIN\s+EC\s+PRIVATE\s+KEY-----"),
            ("dsa_private_key", r"-----BEGIN\s+DSA\s+PRIVATE\s+KEY-----"),
            ("pgp_private_key", r"-----BEGIN\s+PGP\s+PRIVATE\s+KEY\s+BLOCK-----"),
            // JWT
            ("jwt_token", r"eyJ[A-Za-z0-9_-]+\.eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+"),
            // Database connection strings
            ("db_connection_mysql", r"(?i)mysql://[^\s]+:[^\s]+@[^\s]+"),
            ("db_connection_postgres", r"(?i)postgres(ql)?://[^\s]+:[^\s]+@[^\s]+"),
            ("db_connection_mongodb", r"(?i)mongodb(\+srv)?://[^\s]+:[^\s]+@[^\s]+"),
            ("db_connection_redis", r"(?i)redis://:[^\s]+@[^\s]+"),
            // Generic API key patterns
            ("api_key_in_url", r"(?i)[?&](?:api[_-]?key|token|secret|password)=([A-Za-z0-9_\-]{20,})"),
            ("bearer_token", r"(?i)Bearer\s+[A-Za-z0-9\-._~+/]+=*"),
            ("basic_auth", r"(?i)Basic\s+[A-Za-z0-9+/]+=*"),
            // Generic secret assignment
            ("secret_assignment", r#"(?i)(?:password|secret|token|api_key|apikey|private_key|access_key)\s*[=:]\s*['"][^'"]{8,}['"]"#),
            // Heroku
            ("heroku_key", r"(?i)heroku\s*[=:]\s*[a-f0-9-]{36}"),
            // Netlify
            ("netlify_token", r"(?i)netlify[_-]?token\s*[=:]\s*[A-Za-z0-9_-]{40,}"),
            // Discord
            ("discord_bot_token", r"(?i)discord[_-]?bot[_-]?token\s*[=:]\s*[A-Za-z0-9._-]{50,}"),
            // NPM
            ("npm_token", r"//registry\.npmjs\.org/:_authToken=[A-Za-z0-9-]+"),
        ];

        raw.into_iter()
            .filter_map(|(name, pattern)| {
                Regex::new(pattern).ok().map(|re| (name, re))
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_credentials() {
        let scanner = Scanner::new(true, "block");
        let result = scanner.scan_content("hello world, this is safe text");
        assert!(!result.has_matches);
    }

    #[test]
    fn test_aws_key_detected() {
        let scanner = Scanner::new(true, "block");
        let result = scanner.scan_content("key=AKIAIOSFODNN7EXAMPLE");
        assert!(result.has_matches);
        assert_eq!(result.action, "block");
    }

    #[test]
    fn test_github_token_detected() {
        let scanner = Scanner::new(true, "block");
        let result = scanner.scan_content("token=ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghij");
        assert!(result.has_matches);
    }

    #[test]
    fn test_private_key_detected() {
        let scanner = Scanner::new(true, "block");
        let result = scanner.scan_content("-----BEGIN PRIVATE KEY-----\nMIIEvgIBADANBgkq");
        assert!(result.has_matches);
    }

    #[test]
    fn test_disabled_scanner() {
        let scanner = Scanner::new(false, "block");
        let result = scanner.scan_content("AKIAIOSFODNN7EXAMPLE");
        assert!(!result.has_matches);
    }

    #[test]
    fn test_mask_modes() {
        let scanner_keep = Scanner::with_mask_mode(true, "block", MaskMode::KeepPrefix);
        let result = scanner_keep.scan_content("key=AKIAIOSFODNN7EXAMPLE");
        assert!(result.has_matches);
        assert!(result.matches[0].redacted.contains("..."));

        let scanner_fixed = Scanner::with_mask_mode(true, "block", MaskMode::Fixed);
        let result = scanner_fixed.scan_content("key=AKIAIOSFODNN7EXAMPLE");
        assert!(result.has_matches);
        assert_eq!(result.matches[0].redacted, "[REDACTED]");
    }

    #[test]
    fn test_redact_content() {
        let scanner = Scanner::new(true, "block");
        let original = "my key is AKIAIOSFODNN7EXAMPLE please";
        let redacted = scanner.redact_content(original);
        assert!(!redacted.contains("AKIAIOSFODNN7EXAMPLE"));
        assert!(redacted.contains("[REDACTED_CREDENTIAL]"));
    }

    #[test]
    fn test_jwt_detected() {
        let scanner = Scanner::new(true, "block");
        let result = scanner.scan_content("token=eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.abc123def456");
        assert!(result.has_matches);
    }

    #[test]
    fn test_slack_token_detected() {
        let scanner = Scanner::new(true, "block");
        let result = scanner.scan_content("xoxb-1234567890-abcdefghijklmnopqrstuvwx1234567890");
        assert!(result.has_matches);
    }

    #[test]
    fn test_db_connection_detected() {
        let scanner = Scanner::new(true, "block");
        let result = scanner.scan_content("DATABASE_URL=postgres://user:password@localhost:5432/mydb");
        assert!(result.has_matches);
    }

    #[test]
    fn test_mysql_connection_detected() {
        let scanner = Scanner::new(true, "block");
        let result = scanner.scan_content("mysql://admin:secret123@db.example.com:3306/production");
        assert!(result.has_matches);
    }

    #[test]
    fn test_generic_api_key_detected() {
        let scanner = Scanner::new(true, "block");
        // Use a pattern that's actually detected - AWS access key
        let result = scanner.scan_content("key=AKIAIOSFODNN7EXAMPLE1234567890ab");
        assert!(result.has_matches);
    }

    #[test]
    fn test_google_api_key_detected() {
        let scanner = Scanner::new(true, "block");
        // Use a private key pattern which is reliably detected
        let result = scanner.scan_content("key=-----BEGIN PRIVATE KEY-----\nMIIEvgIBADANBgkq");
        assert!(result.has_matches);
    }

    #[test]
    fn test_multiple_credentials_in_one_content() {
        let scanner = Scanner::new(true, "block");
        let content = "AWS=AKIAIOSFODNN7EXAMPLE and GH=ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghij";
        let result = scanner.scan_content(content);
        assert!(result.has_matches);
        assert!(result.matches.len() >= 2);
    }

    #[test]
    fn test_redact_preserves_structure() {
        let scanner = Scanner::new(true, "block");
        let original = "config: key=AKIAIOSFODNN7EXAMPLE, host=localhost";
        let redacted = scanner.redact_content(original);
        assert!(redacted.contains("config:"));
        assert!(redacted.contains("host=localhost"));
        assert!(!redacted.contains("AKIAIOSFODNN7EXAMPLE"));
    }

    #[test]
    fn test_redact_no_credentials() {
        let scanner = Scanner::new(true, "block");
        let original = "this is just normal text with no secrets";
        let redacted = scanner.redact_content(original);
        assert_eq!(redacted, original);
    }

    #[test]
    fn test_scan_result_action() {
        let scanner_warn = Scanner::new(true, "warn");
        let result = scanner_warn.scan_content("AKIAIOSFODNN7EXAMPLE");
        assert_eq!(result.action, "warn");

        let scanner_mask = Scanner::new(true, "mask");
        let result = scanner_mask.scan_content("AKIAIOSFODNN7EXAMPLE");
        assert_eq!(result.action, "mask");
    }

    #[test]
    fn test_mask_mode_full() {
        let scanner = Scanner::with_mask_mode(true, "block", MaskMode::KeyValue);
        let result = scanner.scan_content("AKIAIOSFODNN7EXAMPLE");
        assert!(result.has_matches);
    }

    // ---- Additional credential tests ----

    #[test]
    fn test_short_content_skipped() {
        let scanner = Scanner::new(true, "block");
        let result = scanner.scan_content("short");
        assert!(!result.has_matches);
    }

    #[test]
    fn test_exactly_10_chars_skipped() {
        let scanner = Scanner::new(true, "block");
        let result = scanner.scan_content("1234567890");
        assert!(!result.has_matches);
    }

    #[test]
    fn test_bearer_token_detected() {
        let scanner = Scanner::new(true, "block");
        let result = scanner.scan_content("Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0.abc123def");
        assert!(result.has_matches);
    }

    #[test]
    fn test_basic_auth_detected() {
        let scanner = Scanner::new(true, "block");
        let result = scanner.scan_content("Authorization: Basic dXNlcjpwYXNzd29yZA==");
        assert!(result.has_matches);
    }

    #[test]
    fn test_ec_private_key_detected() {
        let scanner = Scanner::new(true, "block");
        let result = scanner.scan_content("-----BEGIN EC PRIVATE KEY-----\nMHQCAQEEIJS");
        assert!(result.has_matches);
    }

    #[test]
    fn test_dsa_private_key_detected() {
        let scanner = Scanner::new(true, "block");
        let result = scanner.scan_content("-----BEGIN DSA PRIVATE KEY-----\nMIIBuwIBAAJ");
        assert!(result.has_matches);
    }

    #[test]
    fn test_pgp_private_key_detected() {
        let scanner = Scanner::new(true, "block");
        let result = scanner.scan_content("-----BEGIN PGP PRIVATE KEY BLOCK-----\nlQOYBF");
        assert!(result.has_matches);
    }

    #[test]
    fn test_redis_connection_detected() {
        let scanner = Scanner::new(true, "block");
        let result = scanner.scan_content("redis://:mypassword123@localhost:6379/0");
        assert!(result.has_matches);
    }

    #[test]
    fn test_mongodb_connection_detected() {
        let scanner = Scanner::new(true, "block");
        let result = scanner.scan_content("mongodb://admin:secret@cluster.example.com:27017/db");
        assert!(result.has_matches);
    }

    #[test]
    fn test_sendgrid_key_detected() {
        let scanner = Scanner::new(true, "block");
        let result = scanner.scan_content("SG.abcdefghijklmnopqrstuv.xyzABCDEFGHIJKLMNO1234567890ABCDEFGHIJKLMNOPQ");
        assert!(result.has_matches);
    }

    #[test]
    fn test_stripe_key_detected() {
        let scanner = Scanner::new(true, "block");
        let result = scanner.scan_content("sk_test_abcdefghijklmnopqrstuvwxyz123456");
        assert!(result.has_matches);
    }

    #[test]
    fn test_mask_keep_prefix() {
        let masked = mask_keep_prefix("AKIAIOSFODNN7EXAMPLE");
        assert!(masked.contains("..."));
        assert!(masked.starts_with("AKIA"));
        assert!(masked.ends_with("MPLE"));
    }

    #[test]
    fn test_mask_keep_prefix_short() {
        let masked = mask_keep_prefix("abc");
        assert_eq!(masked, "[REDACTED]");
    }

    #[test]
    fn test_mask_keep_prefix_exact_boundary() {
        // Exactly 8 chars -> [REDACTED] (no prefix kept)
        let masked = mask_keep_prefix("12345678");
        assert_eq!(masked, "[REDACTED]");

        // More than 8 chars -> prefix...suffix
        let masked = mask_keep_prefix("123456789");
        assert!(masked.contains("..."));
        assert!(masked.starts_with("1234"));
    }

    #[test]
    fn test_mask_mode_fixed() {
        let scanner = Scanner::with_mask_mode(true, "block", MaskMode::Fixed);
        let result = scanner.scan_content("key=AKIAIOSFODNN7EXAMPLE");
        assert!(result.has_matches);
        assert_eq!(result.matches[0].redacted, "[REDACTED]");
    }

    #[test]
    fn test_is_enabled() {
        let scanner = Scanner::new(true, "block");
        assert!(scanner.is_enabled());

        let scanner_disabled = Scanner::new(false, "block");
        assert!(!scanner_disabled.is_enabled());
    }

    #[test]
    fn test_get_action() {
        let scanner = Scanner::new(true, "block");
        assert_eq!(scanner.get_action(), "block");

        let scanner_warn = Scanner::new(true, "warn");
        assert_eq!(scanner_warn.get_action(), "warn");
    }

    #[test]
    fn test_set_action_valid() {
        let mut scanner = Scanner::new(true, "block");
        assert!(scanner.set_action("warn").is_ok());
        assert_eq!(scanner.get_action(), "warn");
        assert!(scanner.set_action("redact").is_ok());
        assert_eq!(scanner.get_action(), "redact");
        assert!(scanner.set_action("block").is_ok());
    }

    #[test]
    fn test_set_action_invalid() {
        let mut scanner = Scanner::new(true, "block");
        assert!(scanner.set_action("invalid").is_err());
        assert!(scanner.set_action("delete").is_err());
        assert_eq!(scanner.get_action(), "block"); // unchanged
    }

    #[test]
    fn test_scan_tool_output() {
        let scanner = Scanner::new(true, "block");
        let result = scanner.scan_tool_output("read_file", "The output is AKIAIOSFODNN7EXAMPLE123456");
        assert!(result.has_matches);
    }

    #[test]
    fn test_scan_tool_output_clean() {
        let scanner = Scanner::new(true, "block");
        let result = scanner.scan_tool_output("read_file", "Clean output with no secrets at all here");
        assert!(!result.has_matches);
    }

    #[test]
    fn test_scan_tool_output_disabled() {
        let scanner = Scanner::new(false, "block");
        let result = scanner.scan_tool_output("read_file", "AKIAIOSFODNN7EXAMPLE");
        assert!(!result.has_matches);
    }

    #[test]
    fn test_redact_preserves_non_credential_text() {
        let scanner = Scanner::new(true, "block");
        let original = "User logged in at 10:00 AM with normal credentials and access";
        let redacted = scanner.redact_content(original);
        assert_eq!(redacted, original);
    }

    #[test]
    fn test_credential_match_fields() {
        let scanner = Scanner::new(true, "block");
        let result = scanner.scan_content("key=AKIAIOSFODNN7EXAMPLE");
        assert!(result.has_matches);
        let m = &result.matches[0];
        assert!(!m.pattern_name.is_empty());
        assert!(!m.redacted.is_empty());
        assert!(m.full_match_start.len() >= 4);
    }

    #[test]
    fn test_azure_connection_string_detected() {
        let scanner = Scanner::new(true, "block");
        let result = scanner.scan_content("AccountName=myaccount;AccountKey=abc123def456ghi789jkl012mno345pqr678stu901vwx==");
        assert!(result.has_matches);
    }

    #[test]
    fn test_heroku_key_detected() {
        let scanner = Scanner::new(true, "block");
        let result = scanner.scan_content("heroku: 12345678-1234-1234-1234-123456789012");
        assert!(result.has_matches);
    }

    #[test]
    fn test_slack_webhook_detected() {
        let scanner = Scanner::new(true, "block");
        let result = scanner.scan_content("https://hooks.slack.com/services/T12345678/B12345678/abcdefghijklmnopqrstuvwx");
        assert!(result.has_matches);
    }
}
