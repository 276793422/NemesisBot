// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package skills

import (
	"fmt"
	"math"
	"regexp"
	"strings"
	"unicode"
)

// DimensionScore represents a single scoring dimension with score, max, and details.
type DimensionScore struct {
	Score   float64 `json:"score"`   // 0-100
	Max     float64 `json:"max"`     // always 100
	Details string  `json:"details"` // explanation
}

// QualityScore holds the overall and per-dimension quality scores for a skill.
type QualityScore struct {
	Overall      float64       `json:"overall"`      // 0-100, weighted average
	Security     DimensionScore `json:"security"`     // safety analysis
	Completeness DimensionScore `json:"completeness"` // completeness of definition
	Clarity      DimensionScore `json:"clarity"`      // clarity of instructions
	Testing      DimensionScore `json:"testing"`      // test coverage hints
}

// QualityScorer evaluates skill content quality across four dimensions.
type QualityScorer struct{}

// NewQualityScorer creates a new QualityScorer.
func NewQualityScorer() *QualityScorer {
	return &QualityScorer{}
}

// Score evaluates the given skill content and returns a QualityScore.
// The metadata map may contain keys like "name", "description", "source" that
// supplement the content analysis.
func (qs *QualityScorer) Score(content string, metadata map[string]string) *QualityScore {
	if metadata == nil {
		metadata = make(map[string]string)
	}

	security := qs.scoreSecurity(content)
	completeness := qs.scoreCompleteness(content, metadata)
	clarity := qs.scoreClarity(content)
	testing := qs.scoreTesting(content)

	overall := 0.25*security.Score + 0.25*completeness.Score + 0.25*clarity.Score + 0.25*testing.Score
	// Round to 2 decimal places
	overall = math.Round(overall*100) / 100

	return &QualityScore{
		Overall:      overall,
		Security:     security,
		Completeness: completeness,
		Clarity:      clarity,
		Testing:      testing,
	}
}

// scoreSecurity uses the Linter to assess security. No issues = 100.
// Penalties: critical -40, high -25, medium -15, low -5 each.
func (qs *QualityScorer) scoreSecurity(content string) DimensionScore {
	linter := NewLinter()
	result := linter.Lint(content, "")

	if len(result.Issues) == 0 {
		return DimensionScore{
			Score:   100,
			Max:     100,
			Details: "No dangerous patterns detected",
		}
	}

	var penalty float64
	criticalCount, highCount, mediumCount, lowCount := 0, 0, 0, 0
	for _, issue := range result.Issues {
		switch issue.Severity {
		case "critical":
			penalty += 40
			criticalCount++
		case "high":
			penalty += 25
			highCount++
		case "medium":
			penalty += 15
			mediumCount++
		case "low":
			penalty += 5
			lowCount++
		}
	}

	score := 100 - penalty
	if score < 0 {
		score = 0
	}

	var parts []string
	if criticalCount > 0 {
		parts = append(parts, fmt.Sprintf("%d critical", criticalCount))
	}
	if highCount > 0 {
		parts = append(parts, fmt.Sprintf("%d high", highCount))
	}
	if mediumCount > 0 {
		parts = append(parts, fmt.Sprintf("%d medium", mediumCount))
	}
	if lowCount > 0 {
		parts = append(parts, fmt.Sprintf("%d low", lowCount))
	}
	details := fmt.Sprintf("Found %s severity issues", strings.Join(parts, ", "))

	return DimensionScore{
		Score:   math.Round(score*100) / 100,
		Max:     100,
		Details: details,
	}
}

// scoreCompleteness checks for: name, description, steps/instructions,
// examples, inputs/outputs, error handling hints. Each present = +15, max 100.
func (qs *QualityScorer) scoreCompleteness(content string, metadata map[string]string) DimensionScore {
	var score float64
	var found []string

	// 1. Name: check metadata or content heading
	if metadata["name"] != "" || hasHeadingPattern(content, `(?i)^#+\s*(?:name|skill\s*name)`) {
		score += 15
		found = append(found, "name")
	}

	// 2. Description: check metadata or content
	if metadata["description"] != "" || hasHeadingPattern(content, `(?i)^#+\s*(?:description|overview|summary)`) {
		score += 15
		found = append(found, "description")
	}

	// 3. Steps/instructions
	if hasHeadingPattern(content, `(?i)^#+\s*(?:steps|instructions|procedure|workflow|process)`) ||
		strings.Contains(strings.ToLower(content), "step 1") ||
		strings.Contains(strings.ToLower(content), "step 1:") ||
		regexp.MustCompile(`(?i)^\d+\.\s`).MatchString(content) {
		score += 15
		found = append(found, "steps")
	}

	// 4. Examples
	if hasHeadingPattern(content, `(?i)^#+\s*(?:examples?|usage|demo)`) ||
		strings.Contains(content, "```") ||
		strings.Contains(strings.ToLower(content), "example") {
		score += 15
		found = append(found, "examples")
	}

	// 5. Inputs/outputs
	if hasHeadingPattern(content, `(?i)^#+\s*(?:inputs?|outputs?|parameters?|arguments?|io\b)`) ||
		strings.Contains(strings.ToLower(content), "input:") ||
		strings.Contains(strings.ToLower(content), "output:") ||
		strings.Contains(strings.ToLower(content), "parameter") {
		score += 15
		found = append(found, "inputs/outputs")
	}

	// 6. Error handling hints
	if hasHeadingPattern(content, `(?i)^#+\s*(?:errors?|error\s*handling|troubleshooting|caveats?|warnings?)`) ||
		strings.Contains(strings.ToLower(content), "error") ||
		strings.Contains(strings.ToLower(content), "fail") ||
		strings.Contains(strings.ToLower(content), "exception") {
		score += 15
		found = append(found, "error handling")
	}

	if score > 100 {
		score = 100
	}

	details := fmt.Sprintf("Found %d of 6 completeness indicators: %s",
		len(found), strings.Join(found, ", "))
	if len(found) == 0 {
		details = "No completeness indicators found"
	}

	return DimensionScore{
		Score:   math.Round(score*100) / 100,
		Max:     100,
		Details: details,
	}
}

// scoreClarity checks: line length consistency, section headers, code blocks,
// step numbering, language consistency.
func (qs *QualityScorer) scoreClarity(content string) DimensionScore {
	if content == "" {
		return DimensionScore{
			Score:   0,
			Max:     100,
			Details: "Empty content",
		}
	}

	var score float64
	lines := strings.Split(content, "\n")
	nonEmptyLines := filterNonEmpty(lines)

	// 1. Section headers (markdown headings)
	headerCount := countMatches(content, `(?m)^#{1,6}\s+\S`)
	if headerCount >= 3 {
		score += 20
	} else if headerCount >= 1 {
		score += 10
	}

	// 2. Code blocks present
	codeBlockCount := countMatches(content, "```")
	if codeBlockCount >= 2 {
		score += 20
	} else if codeBlockCount >= 1 {
		score += 10
	}

	// 3. Step numbering (e.g., "1. ", "Step 1", numbered lists)
	numberedStepCount := countMatches(content, `(?m)(?:^|\n)\s*\d+\.\s+\S`)
	if numberedStepCount >= 3 {
		score += 20
	} else if numberedStepCount >= 1 {
		score += 10
	}

	// 4. Line length consistency (low variance = good)
	if len(nonEmptyLines) > 0 {
		avgLen := averageLineLength(nonEmptyLines)
		variance := lineLengthVariance(nonEmptyLines, avgLen)
		// Standard deviation; low stddev relative to avg is good
		stddev := math.Sqrt(variance)
		if avgLen > 0 && stddev/avgLen < 0.5 {
			score += 20
		} else if avgLen > 0 && stddev/avgLen < 1.0 {
			score += 10
		}
	}

	// 5. Language consistency: check that the content is predominantly one script
	if isConsistentScript(content) {
		score += 20
	} else {
		score += 10 // mixed but present
	}

	if score > 100 {
		score = 100
	}

	details := fmt.Sprintf("Headers: %d, Code blocks: %d, Numbered steps: %d",
		headerCount, codeBlockCount/2, numberedStepCount)

	return DimensionScore{
		Score:   math.Round(score*100) / 100,
		Max:     100,
		Details: details,
	}
}

// scoreTesting checks for: test examples, validation rules, edge case mentions,
// error scenarios. Each = +20, max 100.
func (qs *QualityScorer) scoreTesting(content string) DimensionScore {
	var score float64
	var found []string
	lower := strings.ToLower(content)

	// 1. Test examples
	if hasHeadingPattern(content, `(?i)^#+\s*(?:tests?|test\s*cases?|testing)`) ||
		strings.Contains(lower, "test case") ||
		strings.Contains(lower, "test example") ||
		strings.Contains(lower, "unit test") ||
		strings.Contains(lower, "integration test") {
		score += 20
		found = append(found, "test examples")
	}

	// 2. Validation rules
	if hasHeadingPattern(content, `(?i)^#+\s*(?:validation|rules|constraints?|requirements?)`) ||
		strings.Contains(lower, "validate") ||
		strings.Contains(lower, "must be") ||
		strings.Contains(lower, "required") ||
		strings.Contains(lower, "constraint") {
		score += 20
		found = append(found, "validation rules")
	}

	// 3. Edge case mentions
	if hasHeadingPattern(content, `(?i)^#+\s*(?:edge\s*cases?|corner\s*cases?|boundary)`) ||
		strings.Contains(lower, "edge case") ||
		strings.Contains(lower, "corner case") ||
		strings.Contains(lower, "boundary") ||
		strings.Contains(lower, "limit") {
		score += 20
		found = append(found, "edge cases")
	}

	// 4. Error scenarios
	if hasHeadingPattern(content, `(?i)^#+\s*(?:error\s*scenarios?|failure\s*modes?|error\s*cases?)`) ||
		strings.Contains(lower, "error scenario") ||
		strings.Contains(lower, "failure mode") ||
		strings.Contains(lower, "when.*fails") ||
		strings.Contains(lower, "error condition") {
		score += 20
		found = append(found, "error scenarios")
	}

	if score > 100 {
		score = 100
	}

	details := fmt.Sprintf("Found %d of 4 testing indicators: %s",
		len(found), strings.Join(found, ", "))
	if len(found) == 0 {
		details = "No testing indicators found"
	}

	return DimensionScore{
		Score:   math.Round(score*100) / 100,
		Max:     100,
		Details: details,
	}
}

// --- helper functions ---

// hasHeadingPattern checks whether the content has a markdown heading matching the pattern.
func hasHeadingPattern(content string, pattern string) bool {
	re := regexp.MustCompile(pattern)
	return re.MatchString(content)
}

// countMatches returns the number of non-overlapping matches of pattern in content.
func countMatches(content string, pattern string) int {
	re := regexp.MustCompile(pattern)
	return len(re.FindAllStringIndex(content, -1))
}

// filterNonEmpty returns non-empty lines after trimming.
func filterNonEmpty(lines []string) []string {
	var result []string
	for _, line := range lines {
		trimmed := strings.TrimSpace(line)
		if trimmed != "" {
			result = append(result, line)
		}
	}
	return result
}

// averageLineLength computes the average length of lines.
func averageLineLength(lines []string) float64 {
	if len(lines) == 0 {
		return 0
	}
	var total float64
	for _, line := range lines {
		total += float64(len(line))
	}
	return total / float64(len(lines))
}

// lineLengthVariance computes the variance of line lengths.
func lineLengthVariance(lines []string, avg float64) float64 {
	if len(lines) == 0 {
		return 0
	}
	var sum float64
	for _, line := range lines {
		diff := float64(len(line)) - avg
		sum += diff * diff
	}
	return sum / float64(len(lines))
}

// isConsistentScript checks whether the content is predominantly in one script
// (Latin, CJK, etc.) rather than a suspicious mix.
func isConsistentScript(content string) bool {
	var latin, cjk, other int
	for _, r := range content {
		if unicode.Is(unicode.Latin, r) || unicode.Is(unicode.Common, r) {
			latin++
		} else if unicode.Is(unicode.Han, r) || unicode.Is(unicode.Hiragana, r) || unicode.Is(unicode.Katakana, r) {
			cjk++
		} else if unicode.IsLetter(r) {
			other++
		}
	}

	// If one script dominates (>=80% of non-common characters), it's consistent
	total := latin + cjk + other
	if total == 0 {
		return true
	}
	if float64(latin)/float64(total) >= 0.8 || float64(cjk)/float64(total) >= 0.8 {
		return true
	}
	// Equal mix is also acceptable (e.g., bilingual docs)
	if latin > 0 && cjk > 0 && other == 0 {
		return true
	}
	return false
}
