//! DLP Engine - Layer 5
//! Data Loss Prevention with 30+ configurable rules for PII and sensitive data.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use parking_lot::RwLock;

use nemesis_types::utils;

/// DLP scan result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DlpResult {
    pub has_matches: bool,
    pub matches: Vec<DlpMatch>,
    pub action: String,
    pub summary: String,
}

/// Severity of a DLP match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DlpSeverity {
    #[default]
    Low,
    Medium,
    High,
    Critical,
}

/// A single DLP match.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DlpMatch {
    pub rule_name: String,
    pub category: String,
    pub count: usize,
    /// Severity of the match.
    #[serde(default)]
    pub severity: DlpSeverity,
    /// Masked/redacted representation of the match.
    #[serde(default)]
    pub masked_value: String,
    /// Byte offset of the first match within the input.
    #[serde(default)]
    pub start_position: usize,
}

/// A DLP rule definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DlpRule {
    pub name: String,
    pub category: String,
    pub pattern: String,
    pub enabled: bool,
    pub action: String,
}

/// DLP engine configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DlpConfig {
    pub enabled: bool,
    pub action: String,
    pub custom_rules: Vec<DlpRule>,
    /// Only run rules whose names are in this list (empty = all rules).
    #[serde(default)]
    pub enabled_rules: Vec<String>,
    /// Maximum content length to scan (0 = no limit).
    #[serde(default)]
    pub max_content_length: usize,
}

impl Default for DlpConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            action: "block".to_string(),
            custom_rules: vec![],
            enabled_rules: vec![],
            max_content_length: 0,
        }
    }
}

/// DLP engine with dynamic rules and redaction.
pub struct DlpEngine {
    config: DlpConfig,
    dynamic_rules: RwLock<Vec<DlpRule>>,
}

impl DlpEngine {
    pub fn new(enabled: bool, action: &str) -> Self {
        Self {
            config: DlpConfig {
                enabled,
                action: action.to_string(),
                custom_rules: vec![],
                enabled_rules: vec![],
                max_content_length: 0,
            },
            dynamic_rules: RwLock::new(Vec::new()),
        }
    }

    pub fn with_config(config: DlpConfig) -> Self {
        Self {
            config,
            dynamic_rules: RwLock::new(Vec::new()),
        }
    }

    /// Scan tool input for sensitive data.
    pub fn scan_tool_input(&self, _tool_name: &str, args: &serde_json::Value) -> DlpResult {
        let text = extract_text(args);
        self.scan_text(&text)
    }

    /// Scan text for DLP patterns.
    pub fn scan_text(&self, text: &str) -> DlpResult {
        if !self.config.enabled || text.is_empty() {
            return DlpResult {
                has_matches: false,
                matches: vec![],
                action: self.config.action.clone(),
                summary: String::new(),
            };
        }

        // M15: Truncate oversized content instead of skipping
        let scan_text: std::borrow::Cow<str> =
            if self.config.max_content_length > 0 && text.len() > self.config.max_content_length {
                std::borrow::Cow::Borrowed(&text[..self.config.max_content_length])
            } else {
                std::borrow::Cow::Borrowed(text)
            };

        let rules = get_dlp_rules();
        let mut matches = Vec::new();

        for (category, name, re) in rules {
            if !self.is_rule_enabled(name) {
                continue;
            }
            for mat in re.find_iter(&*scan_text) {
                let matched_text = mat.as_str();
                matches.push(DlpMatch {
                    rule_name: name.to_string(),
                    category: category.to_string(),
                    count: 1,
                    severity: category_to_severity(category),
                    masked_value: partial_mask(matched_text),
                    start_position: mat.start(),
                });
            }
        }

        // Check dynamic rules
        let dynamic = self.dynamic_rules.read();
        for rule in dynamic.iter() {
            if !rule.enabled { continue; }
            if !self.is_rule_enabled(&rule.name) {
                continue;
            }
            if let Ok(re) = Regex::new(&rule.pattern) {
                for mat in re.find_iter(&*scan_text) {
                    let matched_text = mat.as_str();
                    matches.push(DlpMatch {
                        rule_name: rule.name.clone(),
                        category: rule.category.clone(),
                        count: 1,
                        severity: DlpSeverity::Medium,
                        masked_value: partial_mask(matched_text),
                        start_position: mat.start(),
                    });
                }
            }
        }

        // M16: Deduplicate by (rule_name, start_position) — same pattern at
        // different positions produces separate entries.
        let mut seen = std::collections::HashSet::new();
        matches.retain(|m| seen.insert((m.rule_name.clone(), m.start_position)));

        // Aggregate count per (rule_name, start_position) is 1 by construction,
        // but we may have multiple positions for the same rule.  Sum total.
        let has_matches = !matches.is_empty();
        let summary = if has_matches {
            let total: usize = matches.iter().map(|m| m.count).sum();
            format!("{} sensitive data pattern(s) detected across {} rule(s)", total, matches.len())
        } else {
            String::new()
        };

        DlpResult {
            has_matches,
            matches,
            action: self.config.action.clone(),
            summary,
        }
    }

    /// Redact content by replacing all DLP matches with overlap detection.
    ///
    /// Sorts all match positions, resolves overlaps by preferring the longest
    /// match, and replaces only in non-overlapping regions.
    pub fn redact_content(&self, text: &str) -> String {
        if !self.config.enabled || text.is_empty() {
            return text.to_string();
        }

        // Collect all match spans (start, end) from all rules
        let mut spans: Vec<(usize, usize)> = Vec::new();
        let rules = get_dlp_rules();
        let _byte_text = text.as_bytes();

        for (_category, _name, re) in rules.iter() {
            for m in re.find_iter(text) {
                spans.push((m.start(), m.end()));
            }
        }

        // Dynamic rules
        let dynamic = self.dynamic_rules.read();
        for rule in dynamic.iter() {
            if !rule.enabled { continue; }
            if let Ok(re) = Regex::new(&rule.pattern) {
                for m in re.find_iter(text) {
                    spans.push((m.start(), m.end()));
                }
            }
        }

        if spans.is_empty() {
            return text.to_string();
        }

        // Sort by start position, then by length descending (prefer longer matches)
        spans.sort_by(|a, b| {
            a.0.cmp(&b.0).then_with(|| b.1.cmp(&a.1))
        });

        // Resolve overlaps: keep non-overlapping spans
        let mut resolved: Vec<(usize, usize)> = Vec::new();
        for span in spans {
            if let Some(last) = resolved.last() {
                if span.0 < last.1 {
                    // Overlapping - skip (already have a longer match at this position)
                    continue;
                }
            }
            resolved.push(span);
        }

        // Build the result by replacing matched regions
        let mut result = String::with_capacity(text.len());
        let mut last_end = 0;
        for (start, end) in &resolved {
            if *start > last_end {
                result.push_str(&text[last_end..*start]);
            }
            result.push_str("[REDACTED]");
            last_end = *end;
        }
        if last_end < text.len() {
            result.push_str(&text[last_end..]);
        }

        result
    }

    /// Add a dynamic rule.
    pub fn add_rule(&self, rule: DlpRule) -> Result<(), String> {
        // Validate pattern
        Regex::new(&rule.pattern).map_err(|e| format!("invalid pattern: {}", e))?;
        self.dynamic_rules.write().push(rule);
        Ok(())
    }

    /// Remove a dynamic rule by name.
    pub fn remove_rule(&self, name: &str) -> bool {
        let mut rules = self.dynamic_rules.write();
        let before = rules.len();
        rules.retain(|r| r.name != name);
        rules.len() < before
    }

    /// Get all rule names (static + dynamic).
    pub fn get_rule_names(&self) -> Vec<String> {
        let static_rules = get_dlp_rules();
        let mut names: Vec<String> = static_rules.iter().map(|(_, name, _)| name.to_string()).collect();
        let dynamic = self.dynamic_rules.read();
        for rule in dynamic.iter() {
            names.push(rule.name.clone());
        }
        names
    }

    /// Update configuration dynamically.
    pub fn update_config(&mut self, enabled: Option<bool>, action: Option<String>) {
        if let Some(e) = enabled {
            self.config.enabled = e;
        }
        if let Some(a) = action {
            self.config.action = a;
        }
    }

    /// Scan tool output for sensitive data leaks.
    pub fn scan_tool_output(&self, tool_name: &str, output: &str) -> DlpResult {
        let text = if output.len() > 5000 {
            let end = utils::floor_char_boundary(output, 5000);
            &output[..end]
        } else {
            output
        };
        let mut result = self.scan_text(text);
        // Mark output scans as lower severity
        for m in &mut result.matches {
            if m.severity == DlpSeverity::Critical {
                m.severity = DlpSeverity::High;
            }
        }
        let _ = tool_name; // used for logging in full impl
        result
    }

    /// Check if a rule is enabled based on the enabled_rules filter.
    fn is_rule_enabled(&self, rule_name: &str) -> bool {
        if self.config.enabled_rules.is_empty() {
            return true;
        }
        self.config.enabled_rules.iter().any(|r| r == rule_name)
    }

    /// Check if the engine is enabled.
    /// Mirrors Go DLP `IsEnabled()`.
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Enable or disable the engine.
    /// Mirrors Go DLP `SetEnabled(bool)`.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.config.enabled = enabled;
    }

    /// Get total rule count (static + dynamic), regardless of enabled_rules filter.
    /// Mirrors Go DLP `GetRuleCount()`.
    pub fn total_rule_count(&self) -> usize {
        get_dlp_rules().len() + self.dynamic_rules.read().len()
    }

    /// Get enabled rules count.
    pub fn enabled_rule_count(&self) -> usize {
        if self.config.enabled_rules.is_empty() {
            get_dlp_rules().len() + self.dynamic_rules.read().len()
        } else {
            self.config.enabled_rules.len()
        }
    }

    /// Scan content — convenience wrapper for scan_text.
    /// Mirrors Go DLP `ScanContent(ctx, content)`.
    pub fn scan_content(&self, content: &str) -> DlpResult {
        self.scan_text(content)
    }
}

fn extract_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(map) => {
            map.values().map(extract_text).collect::<Vec<_>>().join(" ")
        }
        serde_json::Value::Array(arr) => {
            arr.iter().map(extract_text).collect::<Vec<_>>().join(" ")
        }
        _ => String::new(),
    }
}

/// Map a category string to severity.
fn category_to_severity(category: &str) -> DlpSeverity {
    match category {
        "credential" | "secret_key" | "api_key" => DlpSeverity::Critical,
        "pii" | "financial" | "credit_card" | "bank" => DlpSeverity::High,
        "network" | "contact" | "personal" | "location" | "medical" => DlpSeverity::Medium,
        "ssn" | "national_id" | "email" | "phone" | "ip" => DlpSeverity::Medium,
        _ => DlpSeverity::Low,
    }
}

/// Partially mask a matched string, showing the first 2 and last 2 characters.
///
/// For strings shorter than 4 characters, returns `[REDACTED]`.
/// Mirrors Go's partial masking behavior (e.g., `"42****1234"`).
fn partial_mask(s: &str) -> String {
    if s.len() < 4 {
        return "[REDACTED]".to_string();
    }
    let prefix_end = 2.min(s.len());
    let suffix_start = s.len().saturating_sub(2);
    if suffix_start <= prefix_end {
        return "[REDACTED]".to_string();
    }
    format!("{}****{}", &s[..prefix_end], &s[suffix_start..])
}

type DlpRules = Vec<(&'static str, &'static str, Regex)>;

fn get_dlp_rules() -> &'static DlpRules {
    static RULES: OnceLock<DlpRules> = OnceLock::new();
    RULES.get_or_init(|| {
        let raw: Vec<(&str, &str, &str)> = vec![
            // Credit cards (6)
            ("credit_card", "visa", r"\b4[0-9]{12}(?:[0-9]{3})?\b"),
            ("credit_card", "mastercard", r"\b(?:5[1-5][0-9]{2}|222[1-9]|22[3-9][0-9]|2[3-6][0-9]{2}|27[01][0-9]|2720)[0-9]{12}\b"),
            ("credit_card", "amex", r"\b3[47][0-9]{13}\b"),
            ("credit_card", "discover", r"\b(?:6011|65[0-9]{2}|64[4-9][0-9]|622(?:12[6-9]|1[3-9][0-9]|[2-8][0-9]{2}|9[01][0-9]|92[0-5]))[0-9]{12}\b"),
            ("credit_card", "jcb", r"\b(?:352[89]|35[3-8][0-9])[0-9]{12}\b"),
            ("credit_card", "diners", r"\b(?:3(?:0[0-5]|[68][0-9]))[0-9]{11,13}\b"),

            // API keys and tokens (7)
            ("credential", "aws_access_key", r"(?:A3T[A-Z0-9]|AKIA|AGPA|AIDA|AROA|AIPA|ANPA|ANVA|ASIA)[A-Z0-9]{16}"),
            ("credential", "aws_secret_key", r"(?i)aws[_\-]?secret[_\-]?access[_\-]?key\s*[=:]\s*[A-Za-z0-9/+=]{40}"),
            ("credential", "google_api_key", r"AIza[0-9A-Za-z\-_]{35}"),
            ("credential", "google_oauth_token", r"ya29\.[0-9A-Za-z\-_]+"),
            ("credential", "azure_api_key", r"(?i)azure[_\-]?(?:api|subscription)[_\-]?key\s*[=:]\s*[A-Za-z0-9\-_]{32,}"),
            ("credential", "generic_hex_key", r"(?i)(?:api[_\-]?key|apikey|secret|token|password|auth[_\-]?key)\s*[=:]\s*[0-9a-f]{32,}"),
            ("credential", "generic_base64_key", r"(?i)(?:api[_\-]?key|apikey|secret|token|password|auth[_\-]?key)\s*[=:]\s*[A-Za-z0-9+/=]{40,}"),

            // Private keys (4)
            ("credential", "private_key_rsa", r"-----BEGIN RSA PRIVATE KEY-----"),
            ("credential", "private_key_generic", r"-----BEGIN PRIVATE KEY-----"),
            ("credential", "private_key_openssh", r"-----BEGIN OPENSSH PRIVATE KEY-----"),
            ("credential", "private_key_pkcs8", r"-----BEGIN ENCRYPTED PRIVATE KEY-----"),

            // Personal IDs (4)
            ("pii", "us_ssn", r"\b[0-9]{3}-[0-9]{2}-[0-9]{4}\b"),
            ("pii", "china_id", r"\b[1-9][0-9]{5}(?:19|20)[0-9]{2}(?:0[1-9]|1[0-2])(?:0[1-9]|[12][0-9]|3[01])[0-9]{3}[0-9Xx]\b"),
            ("pii", "email", r"\b[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}\b"),
            ("pii", "phone_international", r"(?:\+?\d{1,3}[\s\-.]?)?\(?\d{2,4}\)?[\s\-.]?\d{3,4}[\s\-.]?\d{3,4}"),

            // Network identifiers (3)
            ("network", "ip_address_private", r"\b(?:10\.\d{1,3}\.\d{1,3}\.\d{1,3}|172\.(?:1[6-9]|2\d|3[01])\.\d{1,3}\.\d{1,3}|192\.168\.\d{1,3}\.\d{1,3})\b"),
            ("network", "ip_address_public", r"\b(?:[1-9]\d?|1\d\d|2[01]\d|22[0-3])(?:\.\d{1,3}){3}\b"),
            ("network", "ip_address_ipv6", r"(?:[0-9a-fA-F]{1,4}:){7}[0-9a-fA-F]{1,4}"),

            // Financial: Bank accounts (2)
            ("financial", "bank_account_number", r"\b(?:account[_\s\-]?number|acct|iban|swift|bic)\s*[=:]\s*[A-Z0-9]{8,17}\b"),
            ("financial", "iban", r"\b[A-Z]{2}[0-9]{2}[A-Z0-9]{4}[0-9]{7}(?:[A-Z0-9]?){0,16}\b"),

            // Tokens and connection strings (6)
            ("credential", "jwt_token", r"\beyJ[A-Za-z0-9\-_]+\.eyJ[A-Za-z0-9\-_]+\.[A-Za-z0-9\-_]+\b"),
            ("credential", "database_connection_string", r#"(?i)(?:mysql|postgres|postgresql|mongodb|redis|mssql|sqlserver|oracle)://[^\s'"]+:[^\s'"]+@[^\s'"]+"#),
            ("credential", "github_token", r"gh[ps]_[A-Za-z0-9_]{36,}"),
            ("credential", "slack_token", r"xox[bopsa]-[0-9]{10,13}-[0-9]{10,13}-[a-zA-Z0-9]{24,34}"),
            ("credential", "stripe_key", r"(?:sk|pk)_(?:test_|live_)[A-Za-z0-9]{24,}"),

            // Generic secrets patterns (4)
            ("credential", "secret_password_assignment", r#"(?i)(?:password|passwd|pwd)\s*[=:]\s*['"]?[^\s'"]{8,}['"]?"#),
            ("credential", "secret_token_assignment", r#"(?i)(?:token|bearer|access[_\-]?token|auth[_\-]?token|refresh[_\-]?token)\s*[=:]\s*['"]?[^\s'"]{8,}['"]?"#),
            ("credential", "secret_key_assignment", r#"(?i)(?:secret[_\-]?key|client[_\-]?secret|shared[_\-]?secret|encryption[_\-]?key)\s*[=:]\s*['"]?[^\s'"]{8,}['"]?"#),
            ("credential", "authorization_header", r"(?i)authorization\s*:\s*(?:bearer|basic)\s+[A-Za-z0-9\-_.~+/]+=*"),
        ];

        raw.into_iter()
            .filter_map(|(cat, name, pattern)| {
                Regex::new(pattern).ok().map(|re| (cat, name, re))
            })
            .collect()
    })
}

#[cfg(test)]
mod tests;
