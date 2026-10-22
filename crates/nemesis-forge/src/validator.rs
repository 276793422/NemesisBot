//! Static validator - Stage 1 content validation.
//!
//! Validates artifact content based on type-specific rules including
//! structure checks, security patterns, and duplicate detection.
//!
//! Stage 3 quality evaluation lives in `pipeline.rs` (LLM-as-Judge, inlined
//! there); the legacy `QualityEvaluator` copy was removed as dead code
//! (2026-10-22 复查：零生产调用方，仅 tests 消费).

use regex::Regex;
use std::sync::LazyLock;

use nemesis_types::forge::ArtifactKind;

use super::pipeline::{StaticValidationResult, ValidationStage};

// ---------------------------------------------------------------------------
// Stage 1: Static Validator
// ---------------------------------------------------------------------------

/// Static content validator.
pub struct StaticValidator;

impl StaticValidator {
    /// Create a new static validator.
    pub fn new() -> Self {
        Self
    }

    /// Validate artifact content.
    pub fn validate(
        &self,
        kind: ArtifactKind,
        _name: &str,
        content: &str,
    ) -> StaticValidationResult {
        let mut result = StaticValidationResult {
            stage: ValidationStage {
                passed: false,
                timestamp: chrono::Local::now().to_rfc3339(),
                errors: Vec::new(),
            },
            warnings: Vec::new(),
        };

        match kind {
            ArtifactKind::Skill => self.validate_skill(content, &mut result),
            ArtifactKind::Script => self.validate_script(content, &mut result),
            ArtifactKind::Mcp => self.validate_mcp(content, &mut result),
        }

        // Common security checks — apply to ALL artifact kinds (F-S1): dangerous
        // commands were previously only checked for Scripts, so a Skill with
        // `rm -rf /` / `curl|bash` passed and got deployed as an agent instruction.
        self.check_security(content, &mut result);
        self.check_dangerous_commands(content, &mut result);

        result.stage.passed = result.stage.errors.is_empty();
        result
    }

    fn validate_skill(&self, content: &str, result: &mut StaticValidationResult) {
        if content.len() < 50 {
            result
                .stage
                .errors
                .push("Skill content too short (less than 50 chars)".into());
        } else if content.len() > 5000 {
            result
                .warnings
                .push("Skill content is long (over 5000 chars)".into());
        }

        if !content.contains("---") {
            result
                .warnings
                .push("Skill missing frontmatter separator".into());
        }
    }

    fn validate_script(&self, content: &str, result: &mut StaticValidationResult) {
        if content.trim().is_empty() {
            result.stage.errors.push("Script content is empty".into());
        }
        // Dangerous-command patterns are now checked for ALL kinds in
        // check_dangerous_commands (called from validate()).
    }

    fn validate_mcp(&self, content: &str, result: &mut StaticValidationResult) {
        if content.trim().is_empty() {
            result.stage.errors.push("MCP content is empty".into());
            return;
        }

        let has_python =
            content.contains("import ") && (content.contains("def ") || content.contains("class "));
        let has_go = content.contains("package ") && content.contains("func ");

        if !has_python && !has_go {
            result
                .warnings
                .push("MCP content lacks basic code structure".into());
        }
    }

    fn check_security(&self, content: &str, result: &mut StaticValidationResult) {
        // Check for hardcoded secrets（表单一真相源 = hardcoded_secret_errors）
        for desc in self.hardcoded_secret_errors(content) {
            result.stage.errors.push(desc);
        }
    }

    /// Check for dangerous shell patterns. Applied to ALL artifact kinds (not
    /// just Scripts) so a learned/generated Skill can't deploy `rm -rf /` /
    /// `curl|bash` as an agent instruction. (F-S1)
    fn check_dangerous_commands(&self, content: &str, result: &mut StaticValidationResult) {
        // 表单一真相源 = dangerous_command_errors
        for desc in self.dangerous_command_errors(content) {
            result.stage.errors.push(desc);
        }
    }

    /// Return ONLY security errors (dangerous commands + hardcoded secrets),
    /// ignoring quality checks (length, frontmatter). Used as the write gate
    /// for skill/script/mcp creation so a short-but-safe skill isn't rejected,
    /// while dangerous / secret-laden content is blocked before it is written
    /// to disk and becomes an agent instruction. (F-S1)
    pub fn security_errors(&self, content: &str) -> Vec<String> {
        let mut errors = self.hardcoded_secret_errors(content);
        errors.extend(self.dangerous_command_errors(content));
        errors
    }

    /// Hardcoded-secret regex errors (api_key/secret_key assignments).
    fn hardcoded_secret_errors(&self, content: &str) -> Vec<String> {
        let patterns: [(Regex, &str); 2] = [
            (
                Regex::new("(?i)api[_-]?key\\s*[:=]\\s*[\"'][^\"']{8,}").unwrap(),
                "Contains potential API key",
            ),
            (
                Regex::new("(?i)secret[_-]?key\\s*[:=]\\s*[\"'][^\"']{8,}").unwrap(),
                "Contains potential secret key",
            ),
        ];
        patterns
            .iter()
            .filter(|(re, _)| re.is_match(content))
            .map(|(_, desc)| desc.to_string())
            .collect()
    }

    /// Dangerous shell pattern errors (`rm -rf /` / `curl|bash`). pub(crate)
    /// so `pipeline::validate_static`（learning_engine refine 循环的唯一静态
    /// 闸）能只取危险命令面、不与既有密钥子串检查重复计错。(F-S1)
    pub(crate) fn dangerous_command_errors(&self, content: &str) -> Vec<String> {
        static DANGEROUS: LazyLock<Vec<(Regex, &str)>> = LazyLock::new(|| {
            vec![
                (
                    Regex::new(r"rm\s+-rf\s+/").unwrap(),
                    "Dangerous command: rm -rf /",
                ),
                (
                    Regex::new(r"curl.*\|.*bash").unwrap(),
                    "Dangerous pattern: curl | bash",
                ),
            ]
        });
        DANGEROUS
            .iter()
            .filter(|(pattern, _)| pattern.is_match(content))
            .map(|(_, desc)| desc.to_string())
            .collect()
    }
}

impl Default for StaticValidator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
