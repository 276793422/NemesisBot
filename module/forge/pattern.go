package forge

import (
	"crypto/sha256"
	"fmt"
	"sort"
	"strings"
	"time"
)

// PatternType identifies the category of a detected conversation pattern.
type PatternType string

const (
	PatternToolChain       PatternType = "tool_chain"
	PatternErrorRecovery   PatternType = "error_recovery"
	PatternEfficiencyIssue PatternType = "efficiency_issue"
	PatternSuccessTemplate PatternType = "success_template"
)

// ConversationPattern represents a recurring pattern detected in conversation traces.
type ConversationPattern struct {
	ID          string      `json:"id"`
	Type        PatternType `json:"type"`
	Fingerprint string      `json:"fingerprint"`
	Frequency   int         `json:"frequency"`
	Confidence  float64     `json:"confidence"`
	FirstSeen   time.Time   `json:"first_seen"`
	LastSeen    time.Time   `json:"last_seen"`

	// Type-specific fields
	ToolChain       string   `json:"tool_chain,omitempty"`
	AvgRounds       float64  `json:"avg_rounds,omitempty"`
	AvgDurationMs   int64    `json:"avg_duration_ms,omitempty"`
	SuccessRate     float64  `json:"success_rate,omitempty"`
	ErrorTool       string   `json:"error_tool,omitempty"`
	ErrorCode       string   `json:"error_code,omitempty"`
	RecoveryTool    string   `json:"recovery_tool,omitempty"`
	EfficiencyScore float64  `json:"efficiency_score,omitempty"`
	CommonArgKeys   []string `json:"common_arg_keys,omitempty"`
	Description     string   `json:"description,omitempty"`
}

// extractPatterns analyzes conversation traces and extracts patterns using pure code.
// No LLM is used — zero token cost.
func extractPatterns(traces []*ConversationTrace, minFrequency int) []*ConversationPattern {
	if len(traces) == 0 || minFrequency <= 0 {
		return nil
	}

	var allPatterns []*ConversationPattern

	// Run all four detectors
	toolChainPatterns := detectToolChainPatterns(traces, minFrequency)
	errorRecoveryPatterns := detectErrorRecoveryPatterns(traces, minFrequency)
	efficiencyPatterns := detectEfficiencyIssues(traces, minFrequency)
	successPatterns := detectSuccessTemplates(traces, minFrequency)

	allPatterns = append(allPatterns, toolChainPatterns...)
	allPatterns = append(allPatterns, errorRecoveryPatterns...)
	allPatterns = append(allPatterns, efficiencyPatterns...)
	allPatterns = append(allPatterns, successPatterns...)

	// Sort by confidence descending
	sort.Slice(allPatterns, func(i, j int) bool {
		return allPatterns[i].Confidence > allPatterns[j].Confidence
	})

	return allPatterns
}

// --- ToolChainDetector ---

func detectToolChainPatterns(traces []*ConversationTrace, minFreq int) []*ConversationPattern {
	type chainAgg struct {
		chain      string
		count      int
		successes  int
		totalRounds float64
		totalDur   int64
		firstSeen  time.Time
		lastSeen   time.Time
		argKeys    map[string]int
	}

	agg := make(map[string]*chainAgg) // keyed by fingerprint

	for _, t := range traces {
		chain := extractToolChain(t.ToolSteps)
		if chain == "" {
			continue
		}

		fp := patternFingerprint("tool_chain", chain)
		if a, ok := agg[fp]; ok {
			a.count++
			a.totalRounds += float64(t.TotalRounds)
			a.totalDur += t.DurationMs
			if len(t.Signals) == 0 {
				a.successes++
			}
			if t.StartTime.Before(a.firstSeen) {
				a.firstSeen = t.StartTime
			}
			if t.StartTime.After(a.lastSeen) {
				a.lastSeen = t.StartTime
			}
			for _, s := range t.ToolSteps {
				for _, k := range s.ArgKeys {
					a.argKeys[k]++
				}
			}
		} else {
			a := &chainAgg{
				chain:      chain,
				count:      1,
				totalRounds: float64(t.TotalRounds),
				totalDur:   t.DurationMs,
				firstSeen:  t.StartTime,
				lastSeen:   t.StartTime,
				argKeys:    make(map[string]int),
			}
			if len(t.Signals) == 0 {
				a.successes = 1
			}
			for _, s := range t.ToolSteps {
				for _, k := range s.ArgKeys {
					a.argKeys[k]++
				}
			}
			agg[fp] = a
		}
	}

	var patterns []*ConversationPattern
	for fp, a := range agg {
		if a.count < minFreq {
			continue
		}
		successRate := float64(a.successes) / float64(a.count)
		confidence := minFloat(1.0, float64(a.count)/float64(minFreq)) * successRate

		// Common arg keys: appear in >50% of traces
		var commonKeys []string
		for k, c := range a.argKeys {
			if float64(c)/float64(a.count) > 0.5 {
				commonKeys = append(commonKeys, k)
			}
		}
		sort.Strings(commonKeys)

		patterns = append(patterns, &ConversationPattern{
			ID:            fmt.Sprintf("tc-%s", fp[:12]),
			Type:          PatternToolChain,
			Fingerprint:   fp,
			Frequency:     a.count,
			Confidence:    confidence,
			FirstSeen:     a.firstSeen,
			LastSeen:      a.lastSeen,
			ToolChain:     a.chain,
			AvgRounds:     a.totalRounds / float64(a.count),
			AvgDurationMs: a.totalDur / int64(a.count),
			SuccessRate:   successRate,
			CommonArgKeys: commonKeys,
			Description:   fmt.Sprintf("Tool chain: %s (seen %d times, %.0f%% success)", a.chain, a.count, successRate*100),
		})
	}
	return patterns
}

// --- ErrorRecoveryDetector ---

func detectErrorRecoveryPatterns(traces []*ConversationTrace, minFreq int) []*ConversationPattern {
	type recoveryAgg struct {
		errorTool    string
		recoveryTool string
		count        int
		successes    int
		errorCodes   map[string]int
		firstSeen    time.Time
		lastSeen     time.Time
	}

	agg := make(map[string]*recoveryAgg) // keyed by fingerprint

	for _, t := range traces {
		for i := 0; i < len(t.ToolSteps)-1; i++ {
			step := t.ToolSteps[i]
			next := t.ToolSteps[i+1]

			// Condition: step[i] failed && step[i+1] is a different tool && step[i+1] succeeded
			if step.Success || !next.Success || step.ToolName == next.ToolName {
				continue
			}

			fp := patternFingerprint("error_recovery", step.ToolName+":"+next.ToolName)
			if a, ok := agg[fp]; ok {
				a.count++
				if next.Success {
					a.successes++
				}
				if step.ErrorCode != "" {
					a.errorCodes[step.ErrorCode]++
				}
				if t.StartTime.Before(a.firstSeen) {
					a.firstSeen = t.StartTime
				}
				if t.StartTime.After(a.lastSeen) {
					a.lastSeen = t.StartTime
				}
			} else {
				a := &recoveryAgg{
					errorTool:    step.ToolName,
					recoveryTool: next.ToolName,
					count:        1,
					successes:    1,
					errorCodes:   make(map[string]int),
					firstSeen:    t.StartTime,
					lastSeen:     t.StartTime,
				}
				if step.ErrorCode != "" {
					a.errorCodes[step.ErrorCode] = 1
				}
				agg[fp] = a
			}
		}
	}

	var patterns []*ConversationPattern
	for fp, a := range agg {
		if a.count < minFreq {
			continue
		}
		recoveryRate := float64(a.successes) / float64(a.count)
		confidence := recoveryRate

		// Most common error code
		var topErrorCode string
		topCount := 0
		for code, c := range a.errorCodes {
			if c > topCount {
				topCount = c
				topErrorCode = code
			}
		}

		patterns = append(patterns, &ConversationPattern{
			ID:          fmt.Sprintf("er-%s", fp[:12]),
			Type:        PatternErrorRecovery,
			Fingerprint: fp,
			Frequency:   a.count,
			Confidence:  confidence,
			FirstSeen:   a.firstSeen,
			LastSeen:    a.lastSeen,
			ErrorTool:   a.errorTool,
			ErrorCode:   topErrorCode,
			RecoveryTool: a.recoveryTool,
			SuccessRate: recoveryRate,
			Description: fmt.Sprintf("Error recovery: %s → %s (recovered %d/%d times)",
				a.errorTool, a.recoveryTool, a.successes, a.count),
		})
	}
	return patterns
}

// --- EfficiencyAnalyzer ---

func detectEfficiencyIssues(traces []*ConversationTrace, minFreq int) []*ConversationPattern {
	if len(traces) == 0 {
		return nil
	}

	// Calculate global average rounds
	var totalRounds float64
	for _, t := range traces {
		totalRounds += float64(t.TotalRounds)
	}
	globalAvgRounds := totalRounds / float64(len(traces))

	if globalAvgRounds == 0 {
		return nil
	}

	type effAgg struct {
		chain       string
		count       int
		totalRounds float64
		totalDur    int64
		firstSeen   time.Time
		lastSeen    time.Time
	}

	agg := make(map[string]*effAgg)

	for _, t := range traces {
		// Only consider traces with rounds > 2x average and tool chain <= 3 tools
		chain := extractToolChain(t.ToolSteps)
		if chain == "" {
			continue
		}
		toolCount := len(t.ToolSteps)
		if toolCount > 3 {
			continue
		}
		if float64(t.TotalRounds) <= 2*globalAvgRounds {
			continue
		}

		fp := patternFingerprint("efficiency", chain)
		if a, ok := agg[fp]; ok {
			a.count++
			a.totalRounds += float64(t.TotalRounds)
			a.totalDur += t.DurationMs
			if t.StartTime.Before(a.firstSeen) {
				a.firstSeen = t.StartTime
			}
			if t.StartTime.After(a.lastSeen) {
				a.lastSeen = t.StartTime
			}
		} else {
			agg[fp] = &effAgg{
				chain:       chain,
				count:       1,
				totalRounds: float64(t.TotalRounds),
				totalDur:    t.DurationMs,
				firstSeen:   t.StartTime,
				lastSeen:    t.StartTime,
			}
		}
	}

	var patterns []*ConversationPattern
	for fp, a := range agg {
		if a.count < minFreq {
			continue
		}
		actualRounds := a.totalRounds / float64(a.count)
		effScore := 1.0 - actualRounds/(2*globalAvgRounds)
		if effScore < 0 {
			effScore = 0
		}

		patterns = append(patterns, &ConversationPattern{
			ID:              fmt.Sprintf("ef-%s", fp[:12]),
			Type:            PatternEfficiencyIssue,
			Fingerprint:     fp,
			Frequency:       a.count,
			Confidence:      effScore,
			FirstSeen:       a.firstSeen,
			LastSeen:        a.lastSeen,
			ToolChain:       a.chain,
			AvgRounds:       actualRounds,
			AvgDurationMs:   a.totalDur / int64(a.count),
			EfficiencyScore: effScore,
			Description:     fmt.Sprintf("Efficiency issue: %s (%.1f avg rounds vs %.1f global avg)", a.chain, actualRounds, globalAvgRounds),
		})
	}
	return patterns
}

// --- SuccessTemplateFinder ---

func detectSuccessTemplates(traces []*ConversationTrace, minFreq int) []*ConversationPattern {
	if len(traces) == 0 {
		return nil
	}

	// Calculate global average rounds for "baseline"
	var totalRounds float64
	var successCount int
	for _, t := range traces {
		totalRounds += float64(t.TotalRounds)
		if len(t.Signals) == 0 {
			successCount++
		}
	}
	globalAvgRounds := totalRounds / float64(len(traces))

	type succAgg struct {
		chain       string
		count       int
		successes   int
		totalRounds float64
		totalDur    int64
		firstSeen   time.Time
		lastSeen    time.Time
		argKeys     map[string]int
	}

	agg := make(map[string]*succAgg)

	for _, t := range traces {
		chain := extractToolChain(t.ToolSteps)
		if chain == "" {
			continue
		}

		isSuccess := len(t.Signals) == 0
		if !isSuccess {
			continue // Only look at successful conversations for templates
		}

		fp := patternFingerprint("success", chain)
		if a, ok := agg[fp]; ok {
			a.count++
			a.successes++
			a.totalRounds += float64(t.TotalRounds)
			a.totalDur += t.DurationMs
			if t.StartTime.Before(a.firstSeen) {
				a.firstSeen = t.StartTime
			}
			if t.StartTime.After(a.lastSeen) {
				a.lastSeen = t.StartTime
			}
			for _, s := range t.ToolSteps {
				for _, k := range s.ArgKeys {
					a.argKeys[k]++
				}
			}
		} else {
			a := &succAgg{
				chain:       chain,
				count:       1,
				successes:   1,
				totalRounds: float64(t.TotalRounds),
				totalDur:    t.DurationMs,
				firstSeen:   t.StartTime,
				lastSeen:    t.StartTime,
				argKeys:     make(map[string]int),
			}
			for _, s := range t.ToolSteps {
				for _, k := range s.ArgKeys {
					a.argKeys[k]++
				}
			}
			agg[fp] = a
		}
	}

	var patterns []*ConversationPattern
	for fp, a := range agg {
		if a.count < minFreq {
			continue
		}
		successRate := float64(a.successes) / float64(a.count)
		actualRounds := a.totalRounds / float64(a.count)

		// baselineRounds / actualRounds > 1.3 means this is more efficient than average
		effRatio := globalAvgRounds / actualRounds
		if effRatio <= 1.3 || successRate <= 0.9 {
			continue
		}

		confidence := minFloat(1.0, effRatio) * successRate

		var commonKeys []string
		for k, c := range a.argKeys {
			if float64(c)/float64(a.count) > 0.5 {
				commonKeys = append(commonKeys, k)
			}
		}
		sort.Strings(commonKeys)

		patterns = append(patterns, &ConversationPattern{
			ID:            fmt.Sprintf("st-%s", fp[:12]),
			Type:          PatternSuccessTemplate,
			Fingerprint:   fp,
			Frequency:     a.count,
			Confidence:    confidence,
			FirstSeen:     a.firstSeen,
			LastSeen:      a.lastSeen,
			ToolChain:     a.chain,
			AvgRounds:     actualRounds,
			AvgDurationMs: a.totalDur / int64(a.count),
			SuccessRate:   successRate,
			CommonArgKeys: commonKeys,
			Description:   fmt.Sprintf("Success template: %s (%.1f rounds, %.0f%% success, %.1fx faster)", a.chain, actualRounds, successRate*100, effRatio),
		})
	}
	return patterns
}

// --- Helpers ---

// patternFingerprint generates a SHA256 fingerprint for deduplication.
func patternFingerprint(prefix, data string) string {
	h := sha256.Sum256([]byte(prefix + ":" + data))
	return fmt.Sprintf("%x", h[:])
}

// deduplicateChainString normalizes a tool chain for fingerprinting.
// The chain is order-sensitive (read→edit→exec ≠ edit→read→exec).
func deduplicateChainString(tools []string) string {
	return strings.Join(tools, "→")
}

func minFloat(a, b float64) float64 {
	if a < b {
		return a
	}
	return b
}

// ExtractPatternsForTest exposes extractPatterns for testing.
func ExtractPatternsForTest(traces []*ConversationTrace, minFrequency int) []*ConversationPattern {
	return extractPatterns(traces, minFrequency)
}

// PatternFingerprintForTest exposes patternFingerprint for testing.
func PatternFingerprintForTest(prefix, data string) string {
	return patternFingerprint(prefix, data)
}
