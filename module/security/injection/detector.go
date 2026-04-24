// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Package injection provides prompt injection detection and classification
package injection

import (
	"context"
	"fmt"
	"regexp"
	"strings"
	"sync"
	"time"
)

// Config controls the behavior of the injection detector.
type Config struct {
	Enabled       bool    // master switch
	Threshold     float64 // 0.0-1.0, default 0.7; score above this is classified malicious
	MaxInputLength int    // max bytes to analyze, 0 = unlimited
	StrictMode    bool    // if true, lower threshold for high-risk tools
}

// DefaultConfig returns sensible defaults.
func DefaultConfig() Config {
	return Config{
		Enabled:        true,
		Threshold:      0.7,
		MaxInputLength: 65536,
		StrictMode:     false,
	}
}

// PatternMatch records a single pattern that fired during analysis.
type PatternMatch struct {
	PatternName string
	Category    string
	MatchedText string
	Weight      float64
	Position    int // byte offset of the match within the input
}

// AnalysisResult is the full output of an injection analysis pass.
type AnalysisResult struct {
	Score           float64        // 0.0-1.0 aggregate threat score
	Level           string         // "clean", "suspicious", "malicious"
	IsInjection     bool           // true when Level == "malicious"
	MatchedPatterns []PatternMatch // every pattern that fired
	Recommendation  string         // "allow", "review", "block"
	Summary         string         // human-readable one-line summary
	AnalyzedAt      time.Time
	InputLength     int
}

// Detector performs prompt-injection analysis against a compiled pattern
// database. It is safe for concurrent use.
type Detector struct {
	mu       sync.RWMutex
	config   Config
	patterns []compiledPattern
	classifier *Classifier
}

type compiledPattern struct {
	Name        string
	Category    string // "jailbreak", "role_escape", "data_extraction", "command_injection"
	Weight      float64
	Description string
	regex       *regexp.Regexp
}

// NewDetector builds a Detector with the default pattern library.
func NewDetector(cfg Config) *Detector {
	raw := DefaultPatterns()
	return newDetectorFromPatterns(cfg, raw)
}

// NewDetectorWithPatterns builds a Detector from a custom pattern list.
func NewDetectorWithPatterns(cfg Config, raw []Pattern) *Detector {
	return newDetectorFromPatterns(cfg, raw)
}

func newDetectorFromPatterns(cfg Config, raw []Pattern) *Detector {
	compiled := make([]compiledPattern, 0, len(raw))
	for _, p := range raw {
		cp, err := compilePattern(p)
		if err != nil {
			continue // skip uncompilable patterns
		}
		compiled = append(compiled, cp)
	}
	d := &Detector{
		config:    cfg,
		patterns:  compiled,
		classifier: NewClassifier(),
	}
	return d
}

// UpdateConfig replaces the detector configuration in a thread-safe manner.
func (d *Detector) UpdateConfig(cfg Config) {
	d.mu.Lock()
	defer d.mu.Unlock()
	d.config = cfg
}

// Analyze inspects a free-text input for injection attempts.
// The provided ctx is checked for cancellation but does not otherwise affect
// the analysis. A non-nil error is returned only when ctx is cancelled before
// analysis completes.
func (d *Detector) Analyze(ctx context.Context, input string) (*AnalysisResult, error) {
	if ctx.Err() != nil {
		return nil, ctx.Err()
	}

	d.mu.RLock()
	cfg := d.config
	patterns := d.patterns
	d.mu.RUnlock()

	if !cfg.Enabled {
		return &AnalysisResult{
			Score:          0,
			Level:          "clean",
			IsInjection:    false,
			Recommendation: "allow",
			Summary:        "injection detection disabled",
			AnalyzedAt:     time.Now(),
			InputLength:    len(input),
		}, nil
	}

	// Truncate input if configured.
	analyzed := input
	if cfg.MaxInputLength > 0 && len(analyzed) > cfg.MaxInputLength {
		analyzed = analyzed[:cfg.MaxInputLength]
	}

	// 1. Pattern matching phase.
	matches := runPatterns(patterns, analyzed)

	// 2. Classifier phase (entropy, keyword density, etc.).
	classResult := d.classifier.Classify(analyzed)

	// 3. Combine scores.
	finalScore := combineScores(matches, classResult, cfg)

	// 4. Determine level and recommendation.
	level, recommendation := scoreToLevel(finalScore, cfg.Threshold)

	summary := buildSummary(level, matches, finalScore)

	return &AnalysisResult{
		Score:           roundScore(finalScore),
		Level:           level,
		IsInjection:     level == "malicious",
		MatchedPatterns: matches,
		Recommendation:  recommendation,
		Summary:         summary,
		AnalyzedAt:      time.Now(),
		InputLength:     len(analyzed),
	}, nil
}

// AnalyzeToolInput inspects tool arguments for injection attempts.
// In strict mode the effective threshold is lowered for tools that are
// considered high-risk (file_write, process_exec, etc.).
func (d *Detector) AnalyzeToolInput(ctx context.Context, toolName string, args map[string]interface{}) (*AnalysisResult, error) {
	if ctx.Err() != nil {
		return nil, ctx.Err()
	}

	d.mu.RLock()
	cfg := d.config
	d.mu.RUnlock()

	if !cfg.Enabled {
		return &AnalysisResult{
			Score:          0,
			Level:          "clean",
			IsInjection:    false,
			Recommendation: "allow",
			Summary:        "injection detection disabled",
			AnalyzedAt:     time.Now(),
		}, nil
	}

	// Concatenate all argument values into a single string for analysis.
	var parts []string
	for _, v := range args {
		parts = append(parts, fmt.Sprintf("%v", v))
	}
	combined := strings.Join(parts, " ")

	// Determine effective threshold.
	effectiveCfg := cfg
	if cfg.StrictMode && isHighRiskTool(toolName) {
		effectiveCfg.Threshold = cfg.Threshold * 0.7 // lower by 30%
		if effectiveCfg.Threshold < 0.3 {
			effectiveCfg.Threshold = 0.3
		}
	}

	// Reuse Analyze with the adjusted config (temporarily swap).
	d.mu.Lock()
	origConfig := d.config
	d.config = effectiveCfg
	d.mu.Unlock()

	result, err := d.Analyze(ctx, combined)

	d.mu.Lock()
	d.config = origConfig
	d.mu.Unlock()

	if err != nil {
		return nil, err
	}

	// Append tool name to summary for context.
	if result.Summary != "" {
		result.Summary = fmt.Sprintf("[tool:%s] %s", toolName, result.Summary)
	}
	return result, nil
}

// ---------------------------------------------------------------------------
// internal helpers
// ---------------------------------------------------------------------------

// highRiskTools lists tools whose arguments are treated with extra caution.
var highRiskTools = map[string]bool{
	"file_write":    true,
	"file_edit":     true,
	"file_append":   true,
	"process_exec":  true,
	"process_spawn": true,
	"shell_exec":    true,
	"command_run":   true,
}

func isHighRiskTool(name string) bool {
	return highRiskTools[name]
}

// runPatterns applies every compiled pattern against the input and returns
// all matches.
func runPatterns(patterns []compiledPattern, input string) []PatternMatch {
	lower := strings.ToLower(input)
	var matches []PatternMatch
	for _, p := range patterns {
		if p.regex.MatchString(lower) {
			matched := p.regex.FindString(lower)
			loc := p.regex.FindStringIndex(lower)
			pos := 0
			if len(loc) > 0 {
				pos = loc[0]
			}
			matches = append(matches, PatternMatch{
				PatternName: p.Name,
				Category:    p.Category,
				MatchedText: truncate(matched, 120),
				Weight:      p.Weight,
				Position:    pos,
			})
		}
	}
	return matches
}

// combineScores merges pattern-match scores with classifier output.
func combineScores(matches []PatternMatch, classResult *ClassificationResult, cfg Config) float64 {
	if len(matches) == 0 && classResult == nil {
		return 0
	}

	// Weighted sum of pattern matches (capped at 1.0).
	var patternScore float64
	var totalWeight float64
	for _, m := range matches {
		patternScore += m.Weight
		totalWeight += m.Weight
	}
	// Normalize: if many patterns fire we want a strong signal, but we also
	// want diminishing returns so a single heavy pattern can dominate.
	if totalWeight > 0 {
		patternScore = patternScore / (patternScore + 1.0) // sigmoid-like
	}

	// Classifier score.
	classifierScore := 0.0
	if classResult != nil {
		classifierScore = classResult.Score
	}

	// Final blend: 65% pattern, 35% classifier.
	final := 0.65*patternScore + 0.35*classifierScore
	if final > 1.0 {
		final = 1.0
	}
	return final
}

func scoreToLevel(score, threshold float64) (level, recommendation string) {
	switch {
	case score >= threshold:
		return "malicious", "block"
	case score >= threshold*0.6:
		return "suspicious", "review"
	default:
		return "clean", "allow"
	}
}

func buildSummary(level string, matches []PatternMatch, score float64) string {
	if level == "clean" {
		return "no injection indicators detected"
	}
	if len(matches) == 0 {
		return fmt.Sprintf("%s (score %.2f) via heuristic analysis", level, score)
	}
	categories := make(map[string]int)
	for _, m := range matches {
		categories[m.Category]++
	}
	var parts []string
	for cat, cnt := range categories {
		parts = append(parts, fmt.Sprintf("%s(%d)", cat, cnt))
	}
	return fmt.Sprintf("%s (score %.2f): %s", level, score, strings.Join(parts, ", "))
}

func truncate(s string, maxLen int) string {
	if len(s) <= maxLen {
		return s
	}
	return s[:maxLen] + "..."
}

func roundScore(s float64) float64 {
	// Round to 4 decimal places.
	return float64(int(s*10000+0.5)) / 10000
}
