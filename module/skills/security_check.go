// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package skills

// SecurityCheckResult holds the combined results of lint + quality + signature checks.
type SecurityCheckResult struct {
	LintResult   *LintResult   `json:"lint_result"`
	QualityScore *QualityScore `json:"quality_score"`
	Blocked      bool          `json:"blocked"`
	BlockReason  string        `json:"block_reason"`
}

// SecurityCheck performs lint + quality scoring on skill content.
//
// Blocking rules:
//   - LintResult.Score < 30 → Blocked (severe dangerous patterns)
//   - Any critical severity issue → Blocked
//   - LintResult.Score < 60 → Warning only (not blocked)
//   - QualityScore is informational only (never blocks)
func SecurityCheck(content string, skillName string, metadata map[string]string) *SecurityCheckResult {
	linter := NewLinter()
	lintResult := linter.Lint(content, skillName)

	result := &SecurityCheckResult{
		LintResult: lintResult,
	}

	// Check blocking conditions
	if lintResult.Score < 30 {
		result.Blocked = true
		result.BlockReason = "security score too low"
		return result
	}

	for _, issue := range lintResult.Issues {
		if issue.Severity == "critical" {
			result.Blocked = true
			result.BlockReason = "critical severity issue detected: " + issue.Message
			return result
		}
	}

	// Quality scoring (informational, never blocks)
	scorer := NewQualityScorer()
	result.QualityScore = scorer.Score(content, metadata)

	return result
}
