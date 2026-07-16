//! Static validator - Stage 1 content validation.
//!
//! Validates artifact content based on type-specific rules including
//! structure checks, security patterns, and duplicate detection.
//!
//! Also includes the Stage 3 QualityEvaluator (LLM-as-Judge).

use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

use nemesis_types::forge::ArtifactKind;

use super::pipeline::{QualityValidationResult, StaticValidationResult, ValidationStage};
use super::reflector_llm::LLMCaller;

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
    pub fn validate(&self, kind: ArtifactKind, _name: &str, content: &str) -> StaticValidationResult {
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
            result.stage.errors.push("Skill content too short (less than 50 chars)".into());
        } else if content.len() > 5000 {
            result.warnings.push("Skill content is long (over 5000 chars)".into());
        }

        if !content.contains("---") {
            result.warnings.push("Skill missing frontmatter separator".into());
        }
    }

    fn validate_script(&self, content: &str, result: &mut StaticValidationResult) {
        if content.trim().is_empty() {
            result.stage.errors.push("Script content is empty".into());
            return;
        }
        // Dangerous-command patterns are now checked for ALL kinds in
        // check_dangerous_commands (called from validate()).
    }

    fn validate_mcp(&self, content: &str, result: &mut StaticValidationResult) {
        if content.trim().is_empty() {
            result.stage.errors.push("MCP content is empty".into());
            return;
        }

        let has_python = content.contains("import ") && (content.contains("def ") || content.contains("class "));
        let has_go = content.contains("package ") && content.contains("func ");

        if !has_python && !has_go {
            result.warnings.push("MCP content lacks basic code structure".into());
        }
    }

    fn check_security(&self, content: &str, result: &mut StaticValidationResult) {
        // Check for hardcoded secrets
        let patterns: [(Regex, &str); 2] = [
            (Regex::new("(?i)api[_-]?key\\s*[:=]\\s*[\"'][^\"']{8,}").unwrap(), "Contains potential API key"),
            (Regex::new("(?i)secret[_-]?key\\s*[:=]\\s*[\"'][^\"']{8,}").unwrap(), "Contains potential secret key"),
        ];

        for (re, desc) in &patterns {
            if re.is_match(content) {
                result.stage.errors.push(desc.to_string());
            }
        }
    }

    /// Check for dangerous shell patterns. Applied to ALL artifact kinds (not
    /// just Scripts) so a learned/generated Skill can't deploy `rm -rf /` /
    /// `curl|bash` as an agent instruction. (F-S1)
    fn check_dangerous_commands(&self, content: &str, result: &mut StaticValidationResult) {
        static DANGEROUS: LazyLock<Vec<(Regex, &str)>> = LazyLock::new(|| {
            vec![
                (Regex::new(r"rm\s+-rf\s+/").unwrap(), "Dangerous command: rm -rf /"),
                (Regex::new(r"curl.*\|.*bash").unwrap(), "Dangerous pattern: curl | bash"),
            ]
        });
        for (pattern, desc) in DANGEROUS.iter() {
            if pattern.is_match(content) {
                result.stage.errors.push(desc.to_string());
            }
        }
    }

    /// Return ONLY security errors (dangerous commands + hardcoded secrets),
    /// ignoring quality checks (length, frontmatter). Used as the write gate
    /// for skill/script/mcp creation so a short-but-safe skill isn't rejected,
    /// while dangerous / secret-laden content is blocked before it is written
    /// to disk and becomes an agent instruction. (F-S1)
    pub fn security_errors(&self, content: &str) -> Vec<String> {
        let mut result = StaticValidationResult {
            stage: ValidationStage {
                passed: false,
                timestamp: chrono::Local::now().to_rfc3339(),
                errors: Vec::new(),
            },
            warnings: Vec::new(),
        };
        self.check_security(content, &mut result);
        self.check_dangerous_commands(content, &mut result);
        result.stage.errors
    }
}

impl Default for StaticValidator {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Stage 3: Quality Evaluator (LLM-as-Judge)
// ---------------------------------------------------------------------------

/// Quality evaluator that uses LLM-as-Judge for Stage 3 evaluation.
///
/// Evaluates artifacts across 4 dimensions with weighted scoring:
/// - correctness (40%): Does the content correctly implement its purpose?
/// - quality (20%): Code/text quality, clarity, documentation
/// - security (20%): Security considerations, no dangerous patterns
/// - reusability (20%): Can this be reused in other contexts?
pub struct QualityEvaluator {
    caller: Option<Box<dyn LLMCaller>>,
    max_tokens: i64,
    min_score: u32,
}

impl QualityEvaluator {
    /// Create a new quality evaluator without an LLM caller.
    ///
    /// Uses heuristic-based scoring when no LLM is available.
    pub fn new() -> Self {
        Self {
            caller: None,
            max_tokens: 2000,
            min_score: 60,
        }
    }

    /// Create a new quality evaluator with an LLM caller.
    pub fn with_caller(caller: Box<dyn LLMCaller>) -> Self {
        Self {
            caller: Some(caller),
            max_tokens: 2000,
            min_score: 60,
        }
    }

    /// Set the LLM caller.
    pub fn set_caller(&mut self, caller: Box<dyn LLMCaller>) {
        self.caller = Some(caller);
    }

    /// Set the maximum tokens for LLM evaluation.
    pub fn set_max_tokens(&mut self, tokens: i64) {
        self.max_tokens = tokens;
    }

    /// Set the minimum score to pass.
    pub fn set_min_score(&mut self, score: u32) {
        self.min_score = score;
    }

    /// Evaluate an artifact's quality.
    ///
    /// When an LLM caller is available, uses LLM-as-Judge with 4-dimension
    /// scoring. Falls back to heuristic-based scoring otherwise.
    pub async fn evaluate(
        &self,
        kind: ArtifactKind,
        name: &str,
        version: &str,
        content: &str,
    ) -> QualityValidationResult {
        match self.caller {
            Some(ref caller) => self.evaluate_with_llm(caller.as_ref(), kind, name, version, content).await,
            None => self.evaluate_heuristic(kind, name, content),
        }
    }

    /// LLM-as-Judge evaluation (Stage 3).
    async fn evaluate_with_llm(
        &self,
        caller: &dyn LLMCaller,
        kind: ArtifactKind,
        name: &str,
        version: &str,
        content: &str,
    ) -> QualityValidationResult {
        let prompt = format!(
            r#"Evaluate the following Forge artifact for quality.

Type: {:?}
Name: {}
Version: {}

Content:
{}

Score each dimension from 0-100:
- correctness: Does the content correctly implement its stated purpose? (weight 40%%)
- quality: Code/text quality, clarity, documentation (weight 20%%)
- security: Security considerations, no dangerous patterns (weight 20%%)
- reusability: Can this be reused in other contexts? (weight 20%%)

Respond with ONLY a JSON object:
{{"correctness": N, "quality": N, "security": N, "reusability": N, "notes": "brief explanation"}}"#,
            kind, name, version, content
        );

        let system_prompt = "You are a code quality evaluator. Respond only with valid JSON.";

        let mut result = QualityValidationResult {
            stage: ValidationStage {
                passed: false,
                timestamp: chrono::Local::now().to_rfc3339(),
                errors: Vec::new(),
            },
            score: 0,
            notes: String::new(),
            dimensions: HashMap::new(),
        };

        match caller.chat(system_prompt, &prompt, Some(self.max_tokens)).await {
            Ok(response) => {
                // Parse JSON from LLM response
                if let Some(json) = super::reflector_llm::extract_json(&response) {
                    // Extract dimension scores
                    for key in &["correctness", "quality", "security", "reusability"] {
                        if let Some(val) = json.get(key).and_then(|v| v.as_u64()) {
                            result.dimensions.insert(key.to_string(), val as u32);
                        }
                    }

                    // Extract notes
                    if let Some(notes) = json.get("notes").and_then(|v| v.as_str()) {
                        result.notes = notes.to_string();
                    }

                    // Calculate weighted score
                    let score = self.calculate_weighted_score(&result.dimensions);
                    result.score = score;
                    result.stage.passed = score >= self.min_score;
                } else {
                    result
                        .stage
                        .errors
                        .push("Failed to parse JSON from LLM response".to_string());
                    result.stage.passed = false;
                }
            }
            Err(e) => {
                result
                    .stage
                    .errors
                    .push(format!("LLM call failed: {}", e));
                result.stage.passed = false;
            }
        }

        result
    }

    /// Heuristic-based evaluation fallback (no LLM available).
    fn evaluate_heuristic(
        &self,
        kind: ArtifactKind,
        name: &str,
        content: &str,
    ) -> QualityValidationResult {
        let mut dimensions = HashMap::new();
        let notes = "Heuristic evaluation (no LLM provider available)".to_string();
        let content_len = content.len();

        // Heuristic correctness score
        let correctness = match kind {
            ArtifactKind::Skill => {
                if content.contains("---") && content_len >= 100 {
                    75
                } else {
                    50
                }
            }
            ArtifactKind::Script => {
                if content_len >= 50 && !content.contains("rm -rf") {
                    70
                } else {
                    40
                }
            }
            ArtifactKind::Mcp => {
                if content.contains("import ") || content.contains("package ") {
                    75
                } else {
                    50
                }
            }
        };

        // Heuristic quality score
        let quality = if content_len > 200 {
            70
        } else if content_len > 100 {
            60
        } else {
            40
        };

        // Heuristic security score
        let security = if content.contains("api_key") || content.contains("secret") {
            40
        } else {
            80
        };

        // Heuristic reusability score
        let reusability = if !name.is_empty() && content_len > 100 {
            65
        } else {
            50
        };

        dimensions.insert("correctness".to_string(), correctness);
        dimensions.insert("quality".to_string(), quality);
        dimensions.insert("security".to_string(), security);
        dimensions.insert("reusability".to_string(), reusability);

        let score = self.calculate_weighted_score(&dimensions);

        QualityValidationResult {
            stage: ValidationStage {
                passed: score >= self.min_score,
                timestamp: chrono::Local::now().to_rfc3339(),
                errors: Vec::new(),
            },
            score,
            notes,
            dimensions,
        }
    }

    /// Calculate weighted score from dimensions.
    ///
    /// Weighting: correctness(40%) + quality(20%) + security(20%) + reusability(20%)
    fn calculate_weighted_score(&self, dimensions: &HashMap<String, u32>) -> u32 {
        let mut score: u32 = 0;

        if let Some(&c) = dimensions.get("correctness") {
            score += c * 40 / 100;
        }
        if let Some(&q) = dimensions.get("quality") {
            score += q * 20 / 100;
        }
        if let Some(&s) = dimensions.get("security") {
            score += s * 20 / 100;
        }
        if let Some(&r) = dimensions.get("reusability") {
            score += r * 20 / 100;
        }

        score
    }
}

impl Default for QualityEvaluator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
