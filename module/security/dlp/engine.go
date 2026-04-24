// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

// Package dlp provides Data Loss Prevention scanning for NemesisBot.
// It detects sensitive data such as credit card numbers, API keys,
// private keys, SSNs, and other PII in tool inputs, outputs, and
// free-form content using configurable regex-based rules.
package dlp

import (
	"context"
	"fmt"
	"regexp"
	"sort"
	"strings"
	"sync"
)

// Match represents a single rule match found during scanning.
type Match struct {
	RuleName    string // Name of the rule that matched
	Category    string // Category of the sensitive data
	Severity    string // "high", "medium", or "low"
	MaskedValue string // Masked representation, e.g. "42****1234"
	Position    int    // Byte offset in the scanned content
	FullMatch   bool   // true if the entire pattern matched completely
}

// ScanResult holds the outcome of a DLP scan operation.
type ScanResult struct {
	HasMatches bool
	Matches    []Match
	Action     string // "block", "redact", "warn", "allow"
	Summary    string
}

// Config holds configuration for the DLP engine.
type Config struct {
	Enabled          bool     // Master switch for DLP scanning
	EnabledRules     []string // Rule names to enable; empty means all rules
	ActionOnMatch    string   // Default action when a match is found: "block", "redact", "warn"
	MaxContentLength int      // Maximum content size in bytes to scan; 0 means unlimited
	CustomRules      []Rule   // Additional user-defined rules
}

// Rule defines a single DLP detection rule with a regex pattern.
type Rule struct {
	Name        string // Unique rule identifier
	Description string // Human-readable description
	Category    string // Category grouping (e.g., "financial", "credentials")
	Severity    string // "high", "medium", or "low"
	Pattern     string // Go regex pattern string
}

// compiledRule is a Rule with its pre-compiled regex.
type compiledRule struct {
	Rule
	re *regexp.Regexp
}

// DLPEngine is the main data loss prevention scanning engine.
// It is thread-safe; all public methods can be called concurrently.
type DLPEngine struct {
	mu     sync.RWMutex
	config Config

	rules    []compiledRule
	ruleMap  map[string]int // rule name → index in rules slice
	initOnce sync.Once
}

// NewDLPEngine creates a new DLP engine with the given configuration.
// Rules are compiled lazily on first scan (via sync.Once).
func NewDLPEngine(cfg Config) *DLPEngine {
	return &DLPEngine{
		config:  cfg,
		ruleMap: make(map[string]int),
	}
}

// init compiles all built-in and custom rules exactly once.
func (e *DLPEngine) init() {
	e.initOnce.Do(func() {
		e.mu.Lock()
		defer e.mu.Unlock()

		allRules := make([]Rule, 0, len(builtinRules)+len(e.config.CustomRules))
		allRules = append(allRules, builtinRules...)
		allRules = append(allRules, e.config.CustomRules...)

		e.rules = make([]compiledRule, 0, len(allRules))
		e.ruleMap = make(map[string]int, len(allRules))

		enabledSet := make(map[string]struct{}, len(e.config.EnabledRules))
		for _, name := range e.config.EnabledRules {
			enabledSet[name] = struct{}{}
		}

		for _, r := range allRules {
			// If EnabledRules is specified, only include listed rules.
			if len(enabledSet) > 0 {
				if _, ok := enabledSet[r.Name]; !ok {
					continue
				}
			}

			re, err := regexp.Compile(r.Pattern)
			if err != nil {
				// Skip rules with invalid patterns rather than failing entirely.
				continue
			}

			idx := len(e.rules)
			e.rules = append(e.rules, compiledRule{Rule: r, re: re})
			e.ruleMap[r.Name] = idx
		}
	})
}

// ScanContent scans arbitrary content for sensitive data.
// Returns a ScanResult describing all matches and the recommended action.
func (e *DLPEngine) ScanContent(ctx context.Context, content string) (*ScanResult, error) {
	e.init()

	e.mu.RLock()
	defer e.mu.RUnlock()

	if !e.config.Enabled {
		return &ScanResult{Action: "allow", Summary: "DLP scanning is disabled"}, nil
	}

	// Respect context cancellation.
	if err := ctx.Err(); err != nil {
		return nil, fmt.Errorf("dlp scan cancelled: %w", err)
	}

	// Check content length limit.
	if e.config.MaxContentLength > 0 && len(content) > e.config.MaxContentLength {
		content = content[:e.config.MaxContentLength]
	}

	matches := scanString(content, e.rules)
	return buildResult(matches, e.config.ActionOnMatch), nil
}

// ScanToolInput scans a tool invocation's arguments for sensitive data.
// It concatenates all string argument values and scans the result.
func (e *DLPEngine) ScanToolInput(ctx context.Context, toolName string, args map[string]interface{}) (*ScanResult, error) {
	e.init()

	e.mu.RLock()
	defer e.mu.RUnlock()

	if !e.config.Enabled {
		return &ScanResult{Action: "allow", Summary: "DLP scanning is disabled"}, nil
	}

	if err := ctx.Err(); err != nil {
		return nil, fmt.Errorf("dlp scan cancelled: %w", err)
	}

	// Concatenate all argument values into a single string for scanning.
	var sb strings.Builder
	sb.WriteString(toolName)
	sb.WriteString(" ")

	// Use deterministic key ordering for reproducible results.
	keys := sortedKeys(args)
	for _, k := range keys {
		v := args[k]
		sb.WriteString(k)
		sb.WriteString("=")
		sb.WriteString(fmt.Sprintf("%v", v))
		sb.WriteString(" ")
	}

	content := sb.String()
	if e.config.MaxContentLength > 0 && len(content) > e.config.MaxContentLength {
		content = content[:e.config.MaxContentLength]
	}

	matches := scanString(content, e.rules)

	result := buildResult(matches, e.config.ActionOnMatch)
	if result.HasMatches {
		result.Summary = fmt.Sprintf("DLP scan of tool %q input: %s", toolName, result.Summary)
	}
	return result, nil
}

// ScanToolOutput scans a tool's output string for sensitive data.
func (e *DLPEngine) ScanToolOutput(ctx context.Context, toolName string, output string) (*ScanResult, error) {
	e.init()

	e.mu.RLock()
	defer e.mu.RUnlock()

	if !e.config.Enabled {
		return &ScanResult{Action: "allow", Summary: "DLP scanning is disabled"}, nil
	}

	if err := ctx.Err(); err != nil {
		return nil, fmt.Errorf("dlp scan cancelled: %w", err)
	}

	content := output
	if e.config.MaxContentLength > 0 && len(content) > e.config.MaxContentLength {
		content = content[:e.config.MaxContentLength]
	}

	matches := scanString(content, e.rules)

	result := buildResult(matches, e.config.ActionOnMatch)
	if result.HasMatches {
		result.Summary = fmt.Sprintf("DLP scan of tool %q output: %s", toolName, result.Summary)
	}
	return result, nil
}

// AddRule adds a custom rule to the engine at runtime.
// The rule is compiled immediately; returns an error if the pattern is invalid.
// This method must be called before the first scan (before lazy initialization),
// or the rule will not be included. For post-init additions use AddCustomRule.
func (e *DLPEngine) AddRule(r Rule) error {
	re, err := regexp.Compile(r.Pattern)
	if err != nil {
		return fmt.Errorf("invalid regex pattern for rule %q: %w", r.Name, err)
	}

	e.mu.Lock()
	defer e.mu.Unlock()

	idx := len(e.rules)
	e.rules = append(e.rules, compiledRule{Rule: r, re: re})
	e.ruleMap[r.Name] = idx
	return nil
}

// RemoveRule removes a rule by name. Returns false if the rule was not found.
func (e *DLPEngine) RemoveRule(name string) bool {
	e.mu.Lock()
	defer e.mu.Unlock()

	idx, ok := e.ruleMap[name]
	if !ok {
		return false
	}

	// Swap-remove to keep the slice compact.
	last := len(e.rules) - 1
	if idx != last {
		e.rules[idx] = e.rules[last]
		e.ruleMap[e.rules[idx].Name] = idx
	}
	e.rules = e.rules[:last]
	delete(e.ruleMap, name)
	return true
}

// GetRuleNames returns the names of all currently loaded rules.
func (e *DLPEngine) GetRuleNames() []string {
	e.init()

	e.mu.RLock()
	defer e.mu.RUnlock()

	names := make([]string, 0, len(e.rules))
	for _, r := range e.rules {
		names = append(names, r.Name)
	}
	return names
}

// UpdateConfig updates the engine configuration.
// Note: EnabledRules changes will not take effect after the first scan
// because rules are compiled once via sync.Once. Use AddRule/RemoveRule
// for dynamic rule management after initialization.
func (e *DLPEngine) UpdateConfig(cfg Config) {
	e.mu.Lock()
	defer e.mu.Unlock()
	e.config = cfg
}

// IsEnabled returns whether DLP scanning is currently enabled.
func (e *DLPEngine) IsEnabled() bool {
	e.mu.RLock()
	defer e.mu.RUnlock()
	return e.config.Enabled
}

// SetEnabled enables or disables DLP scanning.
func (e *DLPEngine) SetEnabled(enabled bool) {
	e.mu.Lock()
	defer e.mu.Unlock()
	e.config.Enabled = enabled
}

// GetRuleCount returns the number of currently loaded rules.
func (e *DLPEngine) GetRuleCount() int {
	e.init()

	e.mu.RLock()
	defer e.mu.RUnlock()
	return len(e.rules)
}

// RedactContent returns a copy of content with all matched sensitive values
// replaced by their masked representations. Returns the redacted content and
// the scan result describing what was found.
func (e *DLPEngine) RedactContent(ctx context.Context, content string) (string, *ScanResult, error) {
	e.init()

	e.mu.RLock()
	rulesCopy := make([]compiledRule, len(e.rules))
	copy(rulesCopy, e.rules)
	e.mu.RUnlock()

	if !e.config.Enabled {
		return content, &ScanResult{Action: "allow", Summary: "DLP scanning is disabled"}, nil
	}

	if err := ctx.Err(); err != nil {
		return "", nil, fmt.Errorf("dlp scan cancelled: %w", err)
	}

	toScan := content
	if e.config.MaxContentLength > 0 && len(toScan) > e.config.MaxContentLength {
		toScan = toScan[:e.config.MaxContentLength]
	}

	// Collect all match locations across all rules.
	type replacement struct {
		start int
		end   int
		mask  string
	}
	var replacements []replacement

	for _, cr := range rulesCopy {
		locs := cr.re.FindAllStringIndex(toScan, -1)
		for _, loc := range locs {
			matched := toScan[loc[0]:loc[1]]
			replacements = append(replacements, replacement{
				start: loc[0],
				end:   loc[1],
				mask:  maskValue(matched),
			})
		}
	}

	if len(replacements) == 0 {
		return content, &ScanResult{Action: "allow", Summary: "No sensitive data detected"}, nil
	}

	// Sort replacements by start position, longest first for overlapping.
	sort.Slice(replacements, func(i, j int) bool {
		if replacements[i].start != replacements[j].start {
			return replacements[i].start < replacements[j].start
		}
		return replacements[i].end > replacements[j].end
	})

	// Build redacted string, applying non-overlapping replacements.
	var sb strings.Builder
	prevEnd := 0
	for _, r := range replacements {
		if r.start < prevEnd {
			// Overlapping match; skip.
			continue
		}
		sb.WriteString(content[prevEnd:r.start])
		sb.WriteString(r.mask)
		prevEnd = r.end
	}
	if prevEnd < len(content) {
		sb.WriteString(content[prevEnd:])
	}

	matches := scanString(toScan, rulesCopy)
	result := buildResult(matches, e.config.ActionOnMatch)
	return sb.String(), result, nil
}

// buildResult constructs a ScanResult from a slice of matches.
func buildResult(matches []Match, actionOnMatch string) *ScanResult {
	if len(matches) == 0 {
		return &ScanResult{
			HasMatches: false,
			Matches:    nil,
			Action:     "allow",
			Summary:    "No sensitive data detected",
		}
	}

	action := actionOnMatch
	if action == "" {
		action = defaultActionForMatches(matches)
	}

	high, medium, low := countSeverities(matches)
	var summaryParts []string
	if high > 0 {
		summaryParts = append(summaryParts, fmt.Sprintf("%d high", high))
	}
	if medium > 0 {
		summaryParts = append(summaryParts, fmt.Sprintf("%d medium", medium))
	}
	if low > 0 {
		summaryParts = append(summaryParts, fmt.Sprintf("%d low", low))
	}

	summary := fmt.Sprintf("Detected %d potential sensitive data match(es): %s",
		len(matches), strings.Join(summaryParts, ", "))

	return &ScanResult{
		HasMatches: true,
		Matches:    matches,
		Action:     action,
		Summary:    summary,
	}
}

// defaultActionForMatches determines the default action based on the
// highest severity match found.
func defaultActionForMatches(matches []Match) string {
	for _, m := range matches {
		if m.Severity == "high" {
			return "block"
		}
	}
	for _, m := range matches {
		if m.Severity == "medium" {
			return "redact"
		}
	}
	return "warn"
}

// countSeverities counts matches by severity level.
func countSeverities(matches []Match) (high, medium, low int) {
	for _, m := range matches {
		switch m.Severity {
		case "high":
			high++
		case "medium":
			medium++
		case "low":
			low++
		}
	}
	return
}

// sortedKeys returns the keys of a map in sorted order.
func sortedKeys(m map[string]interface{}) []string {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	return keys
}
