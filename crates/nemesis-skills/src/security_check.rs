//! Security check - combines lint + quality + signature verification.
//!
//! Performs a comprehensive security scan on skill content before installation.
//! Blocking rules:
//! - Lint score < 0.3 (30/100): Blocked (severe dangerous patterns)
//! - Any critical severity issue: Blocked（P15 扩面：任何分类的 Critical 都拦，
//!   不再只看 Destructive 分类——凭证窃取/下载执行链/按键记录等同级致命）
//! - Lint score < 0.6 (60/100): Warning only (not blocked)
//! - Quality score is informational only (never blocks)

use std::path::Path;

use crate::lint::{LintResult, LintSeverity, SkillLinter};
use crate::quality::QualityScorer;
use crate::types::SecurityCheckResult;

/// 阻断规则单一真相源（M5 重构：两个检查入口共用，规则不再双写）。
fn apply_block_rules(result: &mut SecurityCheckResult, lint_result: &LintResult) {
    if lint_result.score < 0.3 {
        result.blocked = true;
        result.block_reason = "security score too low".to_string();
        return;
    }
    // 任何 Critical severity issue（P15 扩面，不再单一 Destructive 开关；
    // 分类计数随 warnings 可由调用方汇总）。
    if let Some(w) = lint_result
        .warnings
        .iter()
        .find(|w| w.severity == LintSeverity::Critical)
    {
        result.blocked = true;
        result.block_reason = format!("critical severity issue detected: {}", w.message);
    }
}

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

    apply_block_rules(&mut result, &lint_result);
    if result.blocked {
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

/// 对技能目录做装前安全检查（M5 目录形态入口）。
///
/// lint 面扩展到整个技能目录的可执行面（[`crate::lint::SURFACE_FILE_EXTENSIONS`]：
/// SKILL.md + 全部 .md + 脚本/解释型语言形态）——恶意载荷藏在辅助脚本里
/// 无法再绕过检查；quality 评分仍只吃 SKILL.md 内容（元数据载体）。阻断
/// 规则与 [`check_skill_security`] 同源（`apply_block_rules`）。
pub fn check_skill_security_dir(
    dir: &Path,
    skill_md_content: &str,
    skill_name: &str,
    description: &str,
) -> SecurityCheckResult {
    let linter = SkillLinter::new();
    let lint_result = linter.lint_dir(dir, skill_name);

    let mut result = SecurityCheckResult {
        lint_result: lint_result.clone(),
        quality_score: None,
        blocked: false,
        block_reason: String::new(),
    };

    apply_block_rules(&mut result, &lint_result);
    if result.blocked {
        return result;
    }

    // Quality scoring (informational, never blocks) — SKILL.md only.
    let mut meta = std::collections::HashMap::new();
    meta.insert("name", skill_name);
    meta.insert("description", description);
    let quality_result = QualityScorer::score(skill_md_content, Some(&meta));
    result.quality_score = Some(quality_result);

    result
}

#[cfg(test)]
mod tests;
