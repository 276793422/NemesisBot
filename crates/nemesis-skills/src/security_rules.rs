//! Embedded lint rule table (P15 供应链扩面).
//!
//! The extended lint categories (credential theft / persistence / download
//! execute chain / sensitive path access / environment probing / dynamic
//! construction exec / supply chain trace) are data-driven: rules live in
//! `security_rules.json` which is embedded at compile time via `include_str!`.
//! Keeping the table in a data file (instead of hardcoded regexes) makes it
//! hot-updatable in a follow-up without touching linter code.
//!
//! Legacy 27 patterns (DEST/EXFL/PRIV/OBFS/RECN) stay hardcoded in
//! `lint.rs` for byte-stable behavior with the Go implementation.

use std::sync::OnceLock;

use serde::Deserialize;

use crate::lint::LintSeverity;

/// A single extended lint rule parsed from the embedded JSON table.
#[derive(Debug, Clone, Deserialize)]
pub struct RuleDef {
    /// Unique rule id (e.g. "CRED-001"); prefix mirrors the category.
    pub id: String,
    /// Category name in snake_case (parsed via `LintCategory::from_rule_name`).
    pub category: String,
    /// Regex pattern (case-insensitive via inline `(?i)` where intended).
    pub pattern: String,
    /// Severity name: "critical" | "high" | "medium" | "low".
    pub severity: String,
    /// Human-readable description surfaced as the warning message.
    pub message: String,
}

impl RuleDef {
    /// Parse the severity string; unknown values fall back to Medium.
    pub fn severity(&self) -> LintSeverity {
        match self.severity.to_lowercase().as_str() {
            "critical" => LintSeverity::Critical,
            "high" => LintSeverity::High,
            "low" => LintSeverity::Low,
            _ => LintSeverity::Medium,
        }
    }
}

/// Envelope of the embedded rule file.
#[derive(Debug, Deserialize)]
pub struct RuleFile {
    /// Format version (currently 1).
    #[allow(dead_code)]
    pub version: u32,
    /// Human description of the table.
    #[allow(dead_code)]
    pub description: Option<String>,
    /// The rules themselves.
    pub rules: Vec<RuleDef>,
}

impl RuleFile {
    /// Borrow the rule list (stable accessor for the linter).
    pub fn rules(&self) -> &[RuleDef] {
        &self.rules
    }
}

static EMBEDDED_RULES: OnceLock<RuleFile> = OnceLock::new();

/// Parse failure of the embedded table is a build-time bug: this module has
/// unit tests pinning parseability, but if it ever regresses the linter must
/// fail loudly (panic) rather than silently scanning with a shrunken table.
fn parse_embedded() -> RuleFile {
    match serde_json::from_str(include_str!("security_rules.json")) {
        Ok(file) => file,
        Err(e) => panic!("embedded security_rules.json failed to parse: {}", e),
    }
}

/// Access the embedded rule table (parsed once, shared).
pub fn embedded_rule_file() -> &'static RuleFile {
    EMBEDDED_RULES.get_or_init(parse_embedded)
}

#[cfg(test)]
mod tests;
