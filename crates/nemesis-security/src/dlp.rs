//! DLP Engine - Layer 5
//! Data Loss Prevention with 30+ configurable rules for PII and sensitive data.

use parking_lot::RwLock;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

use nemesis_types::utils;

/// DLP scan result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DlpResult {
    pub has_matches: bool,
    pub matches: Vec<DlpMatch>,
    pub action: String,
    pub summary: String,
    /// Whether the pipeline should *block* the operation.
    ///
    /// True iff at least one match resolved to a "block" effective action
    /// (i.e. a High/Medium-confidence match under the default policy).
    /// Low-confidence matches (phone/ip/email on arbitrary web content) set
    /// this to false — they are recorded for audit but do not break the flow.
    #[serde(default)]
    pub should_block: bool,
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

/// Confidence that a match is a *genuine* secret/PII, not a false positive.
///
/// Orthogonal to [`DlpSeverity`] (which measures *how sensitive* the data is).
/// Confidence drives the effective action: `High`→block, `Medium`→block,
/// `Low`→log-only by default (see `DlpConfig::low_confidence_action`).
///
/// Low-confidence rules are pattern-only and prone to false positives on
/// arbitrary web content (e.g. `phone_international` matching a Chinese ICP
/// filing number, `ip_address_public` matching a software version like `2.4.1.8`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DlpConfidence {
    /// High certainty: fixed prefix + checksum/structure (private keys, AWS keys,
    /// JWT, GitHub/Slack tokens). Almost never a false positive.
    High,
    /// Medium certainty: keyed assignment patterns (`password=`, `token=`) or
    /// structured identifiers (credit cards w/o Luhn yet, IBAN).
    #[default]
    Medium,
    /// Low certainty: bare pattern, easily matches ordinary numbers/text
    /// (phone, public IP, email, national IDs without checksum).
    Low,
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
    /// Confidence that this is a genuine match (not a false positive).
    #[serde(default)]
    pub confidence: DlpConfidence,
    /// Effective action decided for this match ("block" or "log").
    /// Driven by confidence + config policy; see [`DlpConfig::action_for`].
    #[serde(default)]
    pub effective_action: String,
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
    /// Confidence of this rule's matches (default Medium).
    #[serde(default)]
    pub confidence: DlpConfidence,
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
    /// Action taken for **low-confidence** matches (default "log" — detect but
    /// don't block). High/Medium-confidence matches always use `action`.
    /// Set to "block" to restore the old hard-block-everything behavior.
    #[serde(default = "default_low_confidence_action")]
    pub low_confidence_action: String,
}

fn default_low_confidence_action() -> String {
    "log".to_string()
}

impl Default for DlpConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            action: "block".to_string(),
            custom_rules: vec![],
            enabled_rules: vec![],
            max_content_length: 0,
            low_confidence_action: default_low_confidence_action(),
        }
    }
}

impl DlpConfig {
    /// Decide the effective action for a match of the given confidence.
    ///
    /// - High/Medium → `action` (block by default)
    /// - Low → `low_confidence_action` (log by default)
    pub fn action_for(&self, confidence: DlpConfidence) -> String {
        match confidence {
            DlpConfidence::High | DlpConfidence::Medium => self.action.clone(),
            DlpConfidence::Low => self.low_confidence_action.clone(),
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
                low_confidence_action: default_low_confidence_action(),
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
                should_block: false,
            };
        }

        // M15: Truncate oversized content instead of skipping.
        // floor_char_boundary avoids slicing mid-codepoint (which panics on
        // multibyte text like Chinese) — same approach as scan_tool_output.
        let scan_text: std::borrow::Cow<str> =
            if self.config.max_content_length > 0 && text.len() > self.config.max_content_length {
                let end = utils::floor_char_boundary(text, self.config.max_content_length);
                std::borrow::Cow::Borrowed(&text[..end])
            } else {
                std::borrow::Cow::Borrowed(text)
            };

        let rules = get_dlp_rules();
        let mut matches = Vec::new();

        for rule in rules {
            if !self.is_rule_enabled(rule.name) {
                continue;
            }
            for mat in rule.re.find_iter(&*scan_text) {
                let matched_text = mat.as_str();
                // Post-match validator (Luhn / ID checksum): demote to Low if it fails,
                // killing "right format, not a real number" false positives.
                let confidence = match rule.validator {
                    Some(v) if !v(&scan_text, mat.start(), mat.end()) => DlpConfidence::Low,
                    _ => rule.confidence,
                };
                let effective_action = self.config.action_for(confidence);
                matches.push(DlpMatch {
                    rule_name: rule.name.to_string(),
                    category: rule.category.to_string(),
                    count: 1,
                    severity: category_to_severity(rule.category),
                    confidence,
                    effective_action: effective_action.clone(),
                    masked_value: partial_mask(matched_text),
                    start_position: mat.start(),
                });
            }
        }

        // Check dynamic rules
        let dynamic = self.dynamic_rules.read();
        for rule in dynamic.iter() {
            if !rule.enabled {
                continue;
            }
            if !self.is_rule_enabled(&rule.name) {
                continue;
            }
            if let Ok(re) = Regex::new(&rule.pattern) {
                for mat in re.find_iter(&*scan_text) {
                    let matched_text = mat.as_str();
                    let confidence = rule.confidence;
                    let effective_action = self.config.action_for(confidence);
                    matches.push(DlpMatch {
                        rule_name: rule.name.clone(),
                        category: rule.category.clone(),
                        count: 1,
                        severity: DlpSeverity::Medium,
                        confidence,
                        effective_action: effective_action.clone(),
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
        // should_block: any match whose effective action is "block"
        // (i.e. High/Medium-confidence under the default policy). Low-confidence
        // matches (phone/ip/email on arbitrary content) do not block.
        let should_block = matches.iter().any(|m| m.effective_action == "block");
        let summary = if has_matches {
            let total: usize = matches.iter().map(|m| m.count).sum();
            let blocking = matches
                .iter()
                .filter(|m| m.effective_action == "block")
                .count();
            format!(
                "{} sensitive data pattern(s) detected across {} rule(s) ({} blocking)",
                total,
                matches.len(),
                blocking
            )
        } else {
            String::new()
        };

        DlpResult {
            has_matches,
            matches,
            action: self.config.action.clone(),
            summary,
            should_block,
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

        for rule in rules.iter() {
            for m in rule.re.find_iter(text) {
                spans.push((m.start(), m.end()));
            }
        }

        // Dynamic rules
        let dynamic = self.dynamic_rules.read();
        for rule in dynamic.iter() {
            if !rule.enabled {
                continue;
            }
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
        spans.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(&a.1)));

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
        let mut names: Vec<String> = static_rules.iter().map(|r| r.name.to_string()).collect();
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
        serde_json::Value::Array(arr) => arr.iter().map(extract_text).collect::<Vec<_>>().join(" "),
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
    // Char-based (not byte-based) slicing so multibyte matched text — e.g. a
    // Chinese password caught by `secret_password_assignment`'s `[^\s'"]{8,}` —
    // doesn't slice mid-codepoint and panic. Shows first 2 + last 2 chars,
    // e.g. "中文密码符" -> "中文****码符".
    let chars: Vec<char> = s.chars().collect();
    if chars.len() < 5 {
        // Too short: prefix(2)+suffix(2) would cover the whole string, so
        // masking would leak everything — return fully redacted instead.
        return "[REDACTED]".to_string();
    }
    let n = chars.len();
    let prefix: String = chars[..2].iter().collect();
    let suffix: String = chars[n - 2..].iter().collect();
    format!("{}****{}", prefix, suffix)
}

type DlpRules = Vec<StaticRule>;

/// Post-match validator: given the full text and the match's byte span,
/// returns true if the match is genuine (passes checksum). false demotes the
/// match to Low confidence. Used to kill "right format, not a real number"
/// false positives (e.g. any 16-digit string masquerading as a credit card).
type Validator = fn(text: &str, start: usize, end: usize) -> bool;

/// A compiled static DLP rule with confidence and optional post-match validator.
struct StaticRule {
    category: &'static str,
    name: &'static str,
    re: Regex,
    confidence: DlpConfidence,
    validator: Option<Validator>,
}

/// Luhn checksum validation for credit-card-shaped matches.
fn luhn_valid(text: &str, start: usize, end: usize) -> bool {
    let s = match text.get(start..end) {
        Some(s) => s,
        None => return false,
    };
    let digits: Vec<u8> = s
        .chars()
        .filter_map(|c| c.to_digit(10))
        .map(|d| d as u8)
        .collect();
    if digits.len() < 2 {
        return false;
    }
    let mut sum = 0u32;
    let mut double = false;
    for &d in digits.iter().rev() {
        let mut x = d as u32;
        if double {
            x *= 2;
            if x > 9 {
                x -= 9;
            }
        }
        sum += x;
        double = !double;
    }
    sum % 10 == 0
}

/// Checksum validation for China resident-ID-shaped matches (18 digits).
/// Last digit is a weighted checksum (GB 11643-1999).
fn china_id_valid(text: &str, start: usize, end: usize) -> bool {
    let s = match text.get(start..end) {
        Some(s) => s,
        None => return false,
    };
    let chars: Vec<char> = s.chars().collect();
    if chars.len() != 18 {
        return false;
    }
    const WEIGHTS: [u32; 17] = [7, 9, 10, 5, 8, 4, 2, 1, 6, 3, 7, 9, 10, 5, 8, 4, 2];
    const CHECK: [char; 11] = ['1', '0', 'X', '9', '8', '7', '6', '5', '4', '3', '2'];
    let mut sum = 0u32;
    for i in 0..17 {
        let d = match chars[i].to_digit(10) {
            Some(d) => d,
            None => return false,
        };
        sum += d * WEIGHTS[i];
    }
    let expected = CHECK[(sum % 11) as usize];
    chars[17].to_ascii_uppercase() == expected
}

fn get_dlp_rules() -> &'static DlpRules {
    static RULES: OnceLock<DlpRules> = OnceLock::new();
    RULES.get_or_init(|| {
        // (category, name, pattern, confidence, validator)
        let raw: Vec<(&'static str, &'static str, &'static str, DlpConfidence, Option<Validator>)> = vec![
            // Credit cards (6) — Medium, Luhn-validated (kills "any 16 digits" false positives)
            ("credit_card", "visa", r"\b4[0-9]{12}(?:[0-9]{3})?\b", DlpConfidence::Medium, Some(luhn_valid)),
            ("credit_card", "mastercard", r"\b(?:5[1-5][0-9]{2}|222[1-9]|22[3-9][0-9]|2[3-6][0-9]{2}|27[01][0-9]|2720)[0-9]{12}\b", DlpConfidence::Medium, Some(luhn_valid)),
            ("credit_card", "amex", r"\b3[47][0-9]{13}\b", DlpConfidence::Medium, Some(luhn_valid)),
            ("credit_card", "discover", r"\b(?:6011|65[0-9]{2}|64[4-9][0-9]|622(?:12[6-9]|1[3-9][0-9]|[2-8][0-9]{2}|9[01][0-9]|92[0-5]))[0-9]{12}\b", DlpConfidence::Medium, Some(luhn_valid)),
            ("credit_card", "jcb", r"\b(?:352[89]|35[3-8][0-9])[0-9]{12}\b", DlpConfidence::Medium, Some(luhn_valid)),
            ("credit_card", "diners", r"\b(?:3(?:0[0-5]|[68][0-9]))[0-9]{11,13}\b", DlpConfidence::Medium, Some(luhn_valid)),

            // API keys and tokens (7) — High (fixed prefix + length, near-zero FP)
            ("credential", "aws_access_key", r"(?:A3T[A-Z0-9]|AKIA|AGPA|AIDA|AROA|AIPA|ANPA|ANVA|ASIA)[A-Z0-9]{16}", DlpConfidence::High, None),
            ("credential", "aws_secret_key", r"(?i)aws[_\-]?secret[_\-]?access[_\-]?key\s*[=:]\s*[A-Za-z0-9/+=]{40}", DlpConfidence::High, None),
            ("credential", "google_api_key", r"AIza[0-9A-Za-z\-_]{35}", DlpConfidence::High, None),
            ("credential", "google_oauth_token", r"ya29\.[0-9A-Za-z\-_]+", DlpConfidence::High, None),
            ("credential", "azure_api_key", r"(?i)azure[_\-]?(?:api|subscription)[_\-]?key\s*[=:]\s*[A-Za-z0-9\-_]{32,}", DlpConfidence::High, None),
            ("credential", "generic_hex_key", r"(?i)(?:api[_\-]?key|apikey|secret|token|password|auth[_\-]?key)\s*[=:]\s*[0-9a-f]{32,}", DlpConfidence::High, None),
            ("credential", "generic_base64_key", r"(?i)(?:api[_\-]?key|apikey|secret|token|password|auth[_\-]?key)\s*[=:]\s*[A-Za-z0-9+/=]{40,}", DlpConfidence::High, None),

            // Private keys (4) — High (fixed PEM header)
            ("credential", "private_key_rsa", r"-----BEGIN RSA PRIVATE KEY-----", DlpConfidence::High, None),
            ("credential", "private_key_generic", r"-----BEGIN PRIVATE KEY-----", DlpConfidence::High, None),
            ("credential", "private_key_openssh", r"-----BEGIN OPENSSH PRIVATE KEY-----", DlpConfidence::High, None),
            ("credential", "private_key_pkcs8", r"-----BEGIN ENCRYPTED PRIVATE KEY-----", DlpConfidence::High, None),

            // Personal IDs (4)
            ("pii", "us_ssn", r"\b[0-9]{3}-[0-9]{2}-[0-9]{4}\b", DlpConfidence::Medium, None),
            ("pii", "china_id", r"\b[1-9][0-9]{5}(?:19|20)[0-9]{2}(?:0[1-9]|1[0-2])(?:0[1-9]|[12][0-9]|3[01])[0-9]{3}[0-9Xx]\b", DlpConfidence::Medium, Some(china_id_valid)),
            ("pii", "email", r"\b[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}\b", DlpConfidence::Low, None),
            ("pii", "phone_international", r"(?:\+?\d{1,3}[\s\-.]?)?\(?\d{2,4}\)?[\s\-.]?\d{3,4}[\s\-.]?\d{3,4}", DlpConfidence::Low, None),

            // Network identifiers (3) — Low (false positives on version numbers, ICP numbers, etc.)
            ("network", "ip_address_private", r"\b(?:10\.\d{1,3}\.\d{1,3}\.\d{1,3}|172\.(?:1[6-9]|2\d|3[01])\.\d{1,3}\.\d{1,3}|192\.168\.\d{1,3}\.\d{1,3})\b", DlpConfidence::Low, None),
            ("network", "ip_address_public", r"\b(?:[1-9]\d?|1\d\d|2[01]\d|22[0-3])(?:\.\d{1,3}){3}\b", DlpConfidence::Low, None),
            ("network", "ip_address_ipv6", r"(?:[0-9a-fA-F]{1,4}:){7}[0-9a-fA-F]{1,4}", DlpConfidence::Low, None),

            // Financial: Bank accounts (2) — Medium (keyed)
            ("financial", "bank_account_number", r"\b(?:account[_\s\-]?number|acct|iban|swift|bic)\s*[=:]\s*[A-Z0-9]{8,17}\b", DlpConfidence::Medium, None),
            ("financial", "iban", r"\b[A-Z]{2}[0-9]{2}[A-Z0-9]{4}[0-9]{7}(?:[A-Z0-9]?){0,16}\b", DlpConfidence::Medium, None),

            // Tokens and connection strings (5) — High (fixed prefix/structure)
            ("credential", "jwt_token", r"\beyJ[A-Za-z0-9\-_]+\.eyJ[A-Za-z0-9\-_]+\.[A-Za-z0-9\-_]+\b", DlpConfidence::High, None),
            ("credential", "database_connection_string", r#"(?i)(?:mysql|postgres|postgresql|mongodb|redis|mssql|sqlserver|oracle)://[^\s'"]+:[^\s'"]+@[^\s'"]+"#, DlpConfidence::High, None),
            ("credential", "github_token", r"gh[ps]_[A-Za-z0-9_]{36,}", DlpConfidence::High, None),
            ("credential", "slack_token", r"xox[bopsa]-[0-9]{10,13}-[0-9]{10,13}-[a-zA-Z0-9]{24,34}", DlpConfidence::High, None),
            ("credential", "stripe_key", r"(?:sk|pk)_(?:test_|live_)[A-Za-z0-9]{24,}", DlpConfidence::High, None),

            // Generic secrets patterns (4) — Medium (keyed assignment)
            ("credential", "secret_password_assignment", r#"(?i)(?:password|passwd|pwd)\s*[=:]\s*['"]?[^\s'"]{8,}['"]?"#, DlpConfidence::Medium, None),
            ("credential", "secret_token_assignment", r#"(?i)(?:token|bearer|access[_\-]?token|auth[_\-]?token|refresh[_\-]?token)\s*[=:]\s*['"]?[^\s'"]{8,}['"]?"#, DlpConfidence::Medium, None),
            ("credential", "secret_key_assignment", r#"(?i)(?:secret[_\-]?key|client[_\-]?secret|shared[_\-]?secret|encryption[_\-]?key)\s*[=:]\s*['"]?[^\s'"]{8,}['"]?"#, DlpConfidence::Medium, None),
            ("credential", "authorization_header", r"(?i)authorization\s*:\s*(?:bearer|basic)\s+[A-Za-z0-9\-_.~+/]+=*", DlpConfidence::Medium, None),
        ];

        raw.into_iter()
            .filter_map(|(cat, name, pattern, confidence, validator)| {
                Regex::new(pattern).ok().map(|re| StaticRule {
                    category: cat,
                    name,
                    re,
                    confidence,
                    validator,
                })
            })
            .collect()
    })
}

#[cfg(test)]
mod tests;
