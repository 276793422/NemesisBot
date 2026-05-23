//! Security check - combines lint + quality + signature verification.
//!
//! Performs a comprehensive security scan on skill content before installation.
//! Blocking rules:
//! - Lint score < 0.3 (30/100): Blocked (severe dangerous patterns)
//! - Any critical severity issue: Blocked
//! - Lint score < 0.6 (60/100): Warning only (not blocked)
//! - Quality score is informational only (never blocks)

use crate::lint::{LintCategory, SkillLinter};
use crate::quality::QualityScorer;
use crate::types::SecurityCheckResult;

/// Run a comprehensive security check on skill content.
///
/// This performs lint analysis and quality scoring. The blocking rules are:
/// - Lint score < 0.3 -> Blocked (severe dangerous patterns detected)
/// - Any destructive-category warning -> Blocked
/// - Lint score < 0.6 -> Warning (not blocked, but concerning)
/// - Quality score is informational only (never blocks)
///
/// Returns a `SecurityCheckResult` with the lint and quality details.
pub fn check_skill_security(
    content: &str,
    skill_name: &str,
    description: &str,
) -> SecurityCheckResult {
    let linter = SkillLinter::new();
    let lint_result = linter.lint(content);

    let mut result = SecurityCheckResult {
        lint_result: lint_result.clone(),
        quality_score: None,
        blocked: false,
        block_reason: String::new(),
    };

    // Check blocking conditions: score too low.
    if lint_result.score < 0.3 {
        result.blocked = true;
        result.block_reason = "security score too low".to_string();
        return result;
    }

    // Check blocking conditions: destructive category = critical severity.
    let has_critical = lint_result
        .warnings
        .iter()
        .any(|w| w.category == LintCategory::Destructive);

    if has_critical {
        result.blocked = true;
        let msg = lint_result
            .warnings
            .iter()
            .find(|w| w.category == LintCategory::Destructive)
            .map(|w| w.message.clone())
            .unwrap_or_default();
        result.block_reason = format!("critical severity issue detected: {}", msg);
        return result;
    }

    // Quality scoring (informational, never blocks).
    let mut meta = std::collections::HashMap::new();
    meta.insert("name", skill_name);
    meta.insert("description", description);
    let quality_result = QualityScorer::score(content, Some(&meta));
    result.quality_score = Some(quality_result);

    result
}

#[cfg(test)]
mod tests;
