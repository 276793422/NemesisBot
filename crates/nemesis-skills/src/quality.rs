//! Quality scorer - evaluates skill content across four dimensions.
//!
//! Mirrors the Go implementation exactly:
//! - Security (25%): Uses the Linter to detect dangerous patterns
//! - Completeness (25%): Checks for name, description, steps, examples, I/O, error handling
//! - Clarity (25%): Checks headers, code blocks, numbered steps, line length, script consistency
//! - Testing (25%): Checks for test examples, validation rules, edge cases, error scenarios

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::lint::SkillLinter;

/// Individual dimension score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DimensionScore {
    /// Score for this dimension (0-100).
    pub score: f64,
    /// Maximum score (always 100).
    pub max: f64,
    /// Explanation of the score.
    pub details: String,
}

/// Overall quality assessment result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityScore {
    /// Weighted average score across all dimensions (0-100).
    pub overall: f64,
    /// Security dimension score.
    pub security: DimensionScore,
    /// Completeness dimension score.
    pub completeness: DimensionScore,
    /// Clarity dimension score.
    pub clarity: DimensionScore,
    /// Testing dimension score.
    pub testing: DimensionScore,
}

/// Quality scorer evaluates skill content quality across four dimensions.
pub struct QualityScorer;

impl Default for QualityScorer {
    fn default() -> Self {
        Self::new()
    }
}

impl QualityScorer {
    /// Create a new QualityScorer instance.
    ///
    /// Mirrors the Go `NewQualityScorer()` constructor.
    pub fn new() -> Self {
        Self
    }

    /// Score the given skill content and return a QualityScore.
    ///
    /// The metadata map may contain keys like "name", "description", "source" that
    /// supplement the content analysis.
    pub fn score(
        content: &str,
        metadata: Option<&std::collections::HashMap<&str, &str>>,
    ) -> QualityScore {
        let empty_meta = std::collections::HashMap::new();
        let meta = metadata.unwrap_or(&empty_meta);

        let security = Self::score_security(content);
        let completeness = Self::score_completeness(content, meta);
        let clarity = Self::score_clarity(content);
        let testing = Self::score_testing(content);

        let overall = 0.25 * security.score
            + 0.25 * completeness.score
            + 0.25 * clarity.score
            + 0.25 * testing.score;
        // Round to 2 decimal places
        let overall = (overall * 100.0).round() / 100.0;

        QualityScore {
            overall,
            security,
            completeness,
            clarity,
            testing,
        }
    }

    /// Score the security dimension using the Linter.
    ///
    /// No issues = 100. Penalties: critical -40, high -25, medium -15, low -5 each.
    fn score_security(content: &str) -> DimensionScore {
        let linter = SkillLinter::new();
        let result = linter.lint(content);

        if result.warnings.is_empty() {
            return DimensionScore {
                score: 100.0,
                max: 100.0,
                details: "No dangerous patterns detected".to_string(),
            };
        }

        let mut penalty = 0.0_f64;
        let mut critical_count = 0usize;
        let mut high_count = 0usize;
        let mut medium_count = 0usize;
        let mut low_count = 0usize;

        for warning in &result.warnings {
            match warning.severity {
                crate::lint::LintSeverity::Critical => {
                    penalty += 40.0;
                    critical_count += 1;
                }
                crate::lint::LintSeverity::High => {
                    penalty += 25.0;
                    high_count += 1;
                }
                crate::lint::LintSeverity::Medium => {
                    penalty += 15.0;
                    medium_count += 1;
                }
                crate::lint::LintSeverity::Low => {
                    penalty += 5.0;
                    low_count += 1;
                }
            }
        }

        let score = (100.0 - penalty).max(0.0);

        let mut parts: Vec<String> = Vec::new();
        if critical_count > 0 {
            parts.push(format!("{} critical", critical_count));
        }
        if high_count > 0 {
            parts.push(format!("{} high", high_count));
        }
        if medium_count > 0 {
            parts.push(format!("{} medium", medium_count));
        }
        if low_count > 0 {
            parts.push(format!("{} low", low_count));
        }
        let details = format!("Found {} severity issues", parts.join(", "));

        DimensionScore {
            score: (score * 100.0).round() / 100.0,
            max: 100.0,
            details,
        }
    }

    /// Score the completeness dimension.
    ///
    /// Checks for: name, description, steps/instructions, examples, inputs/outputs,
    /// error handling hints. Each present = +15, max 100.
    fn score_completeness(
        content: &str,
        metadata: &std::collections::HashMap<&str, &str>,
    ) -> DimensionScore {
        let mut score = 0.0_f64;
        let mut found: Vec<&str> = Vec::new();

        // 1. Name: check metadata or content heading
        if metadata.contains_key("name") && !metadata["name"].is_empty()
            || has_heading_pattern(content, r"(?i)^#+\s*(?:name|skill\s*name)")
        {
            score += 15.0;
            found.push("name");
        }

        // 2. Description: check metadata or content
        if metadata.contains_key("description") && !metadata["description"].is_empty()
            || has_heading_pattern(content, r"(?i)^#+\s*(?:description|overview|summary)")
        {
            score += 15.0;
            found.push("description");
        }

        // 3. Steps/instructions
        if has_heading_pattern(
            content,
            r"(?i)^#+\s*(?:steps|instructions|procedure|workflow|process)",
        ) || content.to_lowercase().contains("step 1")
            || content.to_lowercase().contains("step 1:")
            || Regex::new(r"(?i)^\d+\.\s")
                .map(|re| re.is_match(content))
                .unwrap_or(false)
        {
            score += 15.0;
            found.push("steps");
        }

        // 4. Examples
        if has_heading_pattern(content, r"(?i)^#+\s*(?:examples?|usage|demo)")
            || content.contains("```")
            || content.to_lowercase().contains("example")
        {
            score += 15.0;
            found.push("examples");
        }

        // 5. Inputs/outputs
        if has_heading_pattern(
            content,
            r"(?i)^#+\s*(?:inputs?|outputs?|parameters?|arguments?|io\b)",
        ) || content.to_lowercase().contains("input:")
            || content.to_lowercase().contains("output:")
            || content.to_lowercase().contains("parameter")
        {
            score += 15.0;
            found.push("inputs/outputs");
        }

        // 6. Error handling hints
        if has_heading_pattern(
            content,
            r"(?i)^#+\s*(?:errors?|error\s*handling|troubleshooting|caveats?|warnings?)",
        ) || content.to_lowercase().contains("error")
            || content.to_lowercase().contains("fail")
            || content.to_lowercase().contains("exception")
        {
            score += 15.0;
            found.push("error handling");
        }

        score = score.min(100.0);

        let details = if found.is_empty() {
            "No completeness indicators found".to_string()
        } else {
            format!(
                "Found {} of 6 completeness indicators: {}",
                found.len(),
                found.join(", ")
            )
        };

        DimensionScore {
            score: (score * 100.0).round() / 100.0,
            max: 100.0,
            details,
        }
    }

    /// Score the clarity dimension.
    ///
    /// Checks: line length consistency, section headers, code blocks,
    /// step numbering, language consistency.
    fn score_clarity(content: &str) -> DimensionScore {
        if content.is_empty() {
            return DimensionScore {
                score: 0.0,
                max: 100.0,
                details: "Empty content".to_string(),
            };
        }

        let mut score = 0.0_f64;
        let lines: Vec<&str> = content.lines().collect();
        let non_empty_lines = filter_non_empty(&lines);

        // 1. Section headers (markdown headings)
        let header_count = count_matches(content, r"(?m)^#{1,6}\s+\S");
        if header_count >= 3 {
            score += 20.0;
        } else if header_count >= 1 {
            score += 10.0;
        }

        // 2. Code blocks present
        let code_block_count = count_matches(content, "```");
        if code_block_count >= 2 {
            score += 20.0;
        } else if code_block_count >= 1 {
            score += 10.0;
        }

        // 3. Step numbering (e.g., "1. ", "Step 1", numbered lists)
        let numbered_step_count = count_matches(content, r"(?m)(?:^|\n)\s*\d+\.\s+\S");
        if numbered_step_count >= 3 {
            score += 20.0;
        } else if numbered_step_count >= 1 {
            score += 10.0;
        }

        // 4. Line length consistency (low variance = good)
        if !non_empty_lines.is_empty() {
            let avg_len = average_line_length(&non_empty_lines);
            let variance = line_length_variance(&non_empty_lines, avg_len);
            let stddev = variance.sqrt();
            if avg_len > 0.0 && stddev / avg_len < 0.5 {
                score += 20.0;
            } else if avg_len > 0.0 && stddev / avg_len < 1.0 {
                score += 10.0;
            }
        }

        // 5. Language consistency: check that the content is predominantly one script
        if is_consistent_script(content) {
            score += 20.0;
        } else {
            score += 10.0; // mixed but present
        }

        score = score.min(100.0);

        let details = format!(
            "Headers: {}, Code blocks: {}, Numbered steps: {}",
            header_count,
            code_block_count / 2,
            numbered_step_count
        );

        DimensionScore {
            score: (score * 100.0).round() / 100.0,
            max: 100.0,
            details,
        }
    }

    /// Score the testing dimension.
    ///
    /// Checks for: test examples, validation rules, edge case mentions,
    /// error scenarios. Each = +20, max 100.
    fn score_testing(content: &str) -> DimensionScore {
        let mut score = 0.0_f64;
        let mut found: Vec<&str> = Vec::new();
        let lower = content.to_lowercase();

        // 1. Test examples
        if has_heading_pattern(content, r"(?i)^#+\s*(?:tests?|test\s*cases?|testing)")
            || lower.contains("test case")
            || lower.contains("test example")
            || lower.contains("unit test")
            || lower.contains("integration test")
        {
            score += 20.0;
            found.push("test examples");
        }

        // 2. Validation rules
        if has_heading_pattern(
            content,
            r"(?i)^#+\s*(?:validation|rules|constraints?|requirements?)",
        ) || lower.contains("validate")
            || lower.contains("must be")
            || lower.contains("required")
            || lower.contains("constraint")
        {
            score += 20.0;
            found.push("validation rules");
        }

        // 3. Edge case mentions
        if has_heading_pattern(
            content,
            r"(?i)^#+\s*(?:edge\s*cases?|corner\s*cases?|boundary)",
        ) || lower.contains("edge case")
            || lower.contains("corner case")
            || lower.contains("boundary")
            || lower.contains("limit")
        {
            score += 20.0;
            found.push("edge cases");
        }

        // 4. Error scenarios
        if has_heading_pattern(
            content,
            r"(?i)^#+\s*(?:error\s*scenarios?|failure\s*modes?|error\s*cases?)",
        ) || lower.contains("error scenario")
            || lower.contains("failure mode")
            || lower.contains("when.*fails")
            || lower.contains("error condition")
        {
            score += 20.0;
            found.push("error scenarios");
        }

        score = score.min(100.0);

        let details = if found.is_empty() {
            "No testing indicators found".to_string()
        } else {
            format!(
                "Found {} of 4 testing indicators: {}",
                found.len(),
                found.join(", ")
            )
        };

        DimensionScore {
            score: (score * 100.0).round() / 100.0,
            max: 100.0,
            details,
        }
    }
}

// --- Helper functions ---

/// Checks whether the content has a markdown heading matching the pattern.
/// Ensures (?m) multiline flag is present so ^ matches the beginning of each line.
fn has_heading_pattern(content: &str, pattern: &str) -> bool {
    let full_pattern = if pattern.contains("(?m)") {
        pattern.to_string()
    } else if pattern.starts_with("(?") {
        // Has flags like (?i) but not (?m) — inject (?m) after the existing flags
        if let Some(close_pos) = pattern.find(')') {
            format!("{}m{}", &pattern[..close_pos], &pattern[close_pos..])
        } else {
            format!("(?m){}", pattern)
        }
    } else {
        format!("(?m){}", pattern)
    };
    Regex::new(&full_pattern)
        .map(|re| re.is_match(content))
        .unwrap_or(false)
}

/// Returns the number of non-overlapping matches of pattern in content.
fn count_matches(content: &str, pattern: &str) -> usize {
    Regex::new(pattern)
        .map(|re| re.find_iter(content).count())
        .unwrap_or(0)
}

/// Returns non-empty lines (after trimming).
fn filter_non_empty<'a>(lines: &[&'a str]) -> Vec<&'a str> {
    lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .copied()
        .collect()
}

/// Computes the average length of lines.
fn average_line_length(lines: &[&str]) -> f64 {
    if lines.is_empty() {
        return 0.0;
    }
    let total: usize = lines.iter().map(|l| l.len()).sum();
    total as f64 / lines.len() as f64
}

/// Computes the variance of line lengths.
fn line_length_variance(lines: &[&str], avg: f64) -> f64 {
    if lines.is_empty() {
        return 0.0;
    }
    let sum: f64 = lines
        .iter()
        .map(|line| {
            let diff = line.len() as f64 - avg;
            diff * diff
        })
        .sum();
    sum / lines.len() as f64
}

/// Checks whether the content is predominantly in one script (Latin, CJK, etc.)
/// rather than a suspicious mix.
fn is_consistent_script(content: &str) -> bool {
    let mut latin = 0usize;
    let mut cjk = 0usize;
    let mut other = 0usize;

    for r in content.chars() {
        if r.is_ascii() || is_unicode_common(r) {
            latin += 1;
        } else if is_cjk_char(r) {
            cjk += 1;
        } else if r.is_alphabetic() {
            other += 1;
        }
    }

    // If one script dominates (>=80% of non-common characters), it's consistent
    let total = latin + cjk + other;
    if total == 0 {
        return true;
    }
    if (latin as f64 / total as f64) >= 0.8 || (cjk as f64 / total as f64) >= 0.8 {
        return true;
    }
    // Equal mix is also acceptable (e.g., bilingual docs)
    if latin > 0 && cjk > 0 && other == 0 {
        return true;
    }
    false
}

/// Check if a character is a "common" Unicode category (punctuation, digits, etc.).
fn is_unicode_common(r: char) -> bool {
    // Matches Go's unicode.Common category roughly:
    // whitespace, digits, punctuation, symbols
    r.is_whitespace()
        || r.is_numeric()
        || r.is_ascii_punctuation()
        || r == '\n'
        || r == '\r'
        || r == '\t'
}

/// Check if a character is CJK (Han, Hiragana, Katakana).
fn is_cjk_char(r: char) -> bool {
    // CJK Unified Ideographs
    (r >= '\u{4E00}' && r <= '\u{9FFF}')
    // CJK Extension A
    || (r >= '\u{3400}' && r <= '\u{4DBF}')
    // Hiragana
    || (r >= '\u{3040}' && r <= '\u{309F}')
    // Katakana
    || (r >= '\u{30A0}' && r <= '\u{30FF}')
}

#[cfg(test)]
mod tests;
