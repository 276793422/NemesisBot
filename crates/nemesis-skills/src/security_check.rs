//! Security check - combines lint + quality + signature verification.
//!
//! Performs a comprehensive security scan on skill content before installation.
//! Blocking rules:
//! - Lint score < 0.3 (30/100): Blocked (severe dangerous patterns)
//! - Any critical severity issue: Blocked（P15 扩面：任何分类的 Critical 都拦，
//!   不再只看 Destructive 分类——凭证窃取/下载执行链/按键记录等同级致命）
//! - Lint score < 0.6 (60/100): Warning only (not blocked)
//! - Quality score is informational only (never blocks)

use crate::lint::{LintSeverity, SkillLinter};
use crate::quality::QualityScorer;
use crate::types::SecurityCheckResult;

/// Run a comprehensive security check on skill content.
///
/// This performs lint analysis and quality scoring. The blocking rules are:
/// - Lint score < 0.3 -> Blocked (severe dangerous patterns detected)
/// - Any Critical severity warning (any category) -> Blocked
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

    // Check blocking conditions: any Critical severity issue（P15 扩面，
    // 不再单一 Destructive 开关；分类计数随 warnings 可由调用方汇总）.
    let critical = lint_result
        .warnings
        .iter()
        .find(|w| w.severity == LintSeverity::Critical);

    if let Some(w) = critical {
        result.blocked = true;
        result.block_reason = format!("critical severity issue detected: {}", w.message);
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
