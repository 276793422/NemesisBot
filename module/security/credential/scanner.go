// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Package credential provides credential leak scanning for tool output and content
package credential

import (
	"context"
	"fmt"
	"regexp"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/276793422/NemesisBot/module/logger"
)

// Config holds credential scanner configuration
type Config struct {
	Enabled      bool     // master switch
	EnabledTypes []string // which credential types to scan for (empty = all)
	Action       string   // "block", "redact", "warn"
}

// DefaultConfig returns a secure-by-default configuration
func DefaultConfig() *Config {
	return &Config{
		Enabled:      true,
		EnabledTypes: []string{}, // empty means all types
		Action:       "redact",
	}
}

// CredentialMatch represents a single credential found in scanned content
type CredentialMatch struct {
	Type        string // e.g., "aws_access_key", "github_token"
	Severity    string // "critical", "high", "medium"
	MaskedValue string // masked value for safe display
	Position    int    // byte offset in original content
	Description string // human-readable description
}

// ScanResult holds the results of a credential scan
type ScanResult struct {
	HasMatches bool
	Matches    []CredentialMatch
	Action     string // action to take
	Summary    string // human-readable summary
}

// Scanner scans content for leaked credentials
type Scanner struct {
	config   *Config
	patterns []credentialPattern
	mu       sync.RWMutex
}

// credentialPattern represents a compiled credential detection pattern
type credentialPattern struct {
	Name        string
	Regex       *regexp.Regexp
	Severity    string
	Description string
	MaskFunc    func(match string) string
}

// NewScanner creates a new credential scanner with compiled patterns
func NewScanner(cfg *Config) (*Scanner, error) {
	if cfg == nil {
		cfg = DefaultConfig()
	}

	s := &Scanner{
		config: cfg,
	}

	// Build enabled type set for filtering
	enabledSet := make(map[string]bool)
	for _, t := range cfg.EnabledTypes {
		enabledSet[strings.ToLower(t)] = true
	}
	allEnabled := len(enabledSet) == 0

	// Compile patterns
	for _, p := range defaultPatterns() {
		if allEnabled || enabledSet[strings.ToLower(p.Name)] {
			compiled, err := regexp.Compile(p.RegexStr)
			if err != nil {
				return nil, fmt.Errorf("failed to compile pattern %q: %w", p.Name, err)
			}
			s.patterns = append(s.patterns, credentialPattern{
				Name:        p.Name,
				Regex:       compiled,
				Severity:    p.Severity,
				Description: p.Description,
				MaskFunc:    p.MaskFunc,
			})
		}
	}

	logger.InfoCF("credential", fmt.Sprintf("Scanner initialized: %d patterns loaded", len(s.patterns)), map[string]interface{}{
		"patterns": len(s.patterns),
		"action":   cfg.Action,
	})
	return s, nil
}

// ScanContent scans arbitrary content for leaked credentials.
// Thread-safe.
func (s *Scanner) ScanContent(ctx context.Context, content string) (*ScanResult, error) {
	if !s.config.Enabled {
		return &ScanResult{
			HasMatches: false,
			Action:     s.config.Action,
		}, nil
	}

	s.mu.RLock()
	defer s.mu.RUnlock()

	result := &ScanResult{
		Matches: []CredentialMatch{},
		Action:  s.config.Action,
	}

	for _, pattern := range s.patterns {
		// Check context cancellation
		select {
		case <-ctx.Done():
			return nil, ctx.Err()
		default:
		}

		matches := pattern.Regex.FindAllStringIndex(content, -1)
		for _, loc := range matches {
			start := loc[0]
			end := loc[1]
			matchedValue := content[start:end]

			masked := matchedValue
			if pattern.MaskFunc != nil {
				masked = pattern.MaskFunc(matchedValue)
			} else {
				masked = maskDefault(matchedValue)
			}

			result.Matches = append(result.Matches, CredentialMatch{
				Type:        pattern.Name,
				Severity:    pattern.Severity,
				MaskedValue: masked,
				Position:    start,
				Description: pattern.Description,
			})
		}
	}

	result.HasMatches = len(result.Matches) > 0
	if result.HasMatches {
		result.Summary = formatSummary(result.Matches)
		logger.WarnCF("credential", fmt.Sprintf("Found %d potential credential(s) in content", len(result.Matches)), map[string]interface{}{
			"count": len(result.Matches),
		})
	}

	return result, nil
}

// ScanToolOutput scans tool output for leaked credentials.
// This is a convenience wrapper around ScanContent with tool name logging.
// Thread-safe.
func (s *Scanner) ScanToolOutput(ctx context.Context, toolName string, output string) (*ScanResult, error) {
	if !s.config.Enabled {
		return &ScanResult{
			HasMatches: false,
			Action:     s.config.Action,
		}, nil
	}

	start := time.Now()
	result, err := s.ScanContent(ctx, output)
	if err != nil {
		return nil, err
	}

	elapsed := time.Since(start)
	if result.HasMatches {
		logger.WarnCF("credential", fmt.Sprintf("Tool %q output contains potential credentials", toolName), map[string]interface{}{
			"tool":    toolName,
			"count":   len(result.Matches),
			"elapsed": elapsed.String(),
		})
	} else {
		logger.DebugCF("credential", fmt.Sprintf("Tool %q output clean", toolName), map[string]interface{}{
			"tool":    toolName,
			"elapsed": elapsed.String(),
		})
	}

	return result, nil
}

// IsEnabled returns whether the credential scanner is enabled
func (s *Scanner) IsEnabled() bool {
	return s.config.Enabled
}

// GetAction returns the configured action
func (s *Scanner) GetAction() string {
	return s.config.Action
}

// replacement tracks a position-based string replacement
type replacement struct {
	start int
	end   int
	value string
}

// RedactContent removes or masks credentials from content.
// Returns the redacted content string.
// Thread-safe.
func (s *Scanner) RedactContent(ctx context.Context, content string) (string, error) {
	if !s.config.Enabled {
		return content, nil
	}

	s.mu.RLock()
	defer s.mu.RUnlock()

	var replacements []replacement

	for _, pattern := range s.patterns {
		select {
		case <-ctx.Done():
			return content, ctx.Err()
		default:
		}

		matches := pattern.Regex.FindAllStringIndex(content, -1)
		for _, loc := range matches {
			matchedValue := content[loc[0]:loc[1]]
			masked := matchedValue
			if pattern.MaskFunc != nil {
				masked = pattern.MaskFunc(matchedValue)
			} else {
				masked = maskDefault(matchedValue)
			}
			replacements = append(replacements, replacement{loc[0], loc[1], masked})
		}
	}

	if len(replacements) == 0 {
		return content, nil
	}

	// Sort replacements by start position ascending
	sort.Slice(replacements, func(i, j int) bool {
		return replacements[i].start < replacements[j].start
	})

	// Deduplicate overlapping replacements (keep the first/longest)
	var deduped []replacement
	lastEnd := -1
	for _, r := range replacements {
		if r.start >= lastEnd {
			deduped = append(deduped, r)
			lastEnd = r.end
		}
	}

	// Build redacted string by walking forward
	var sb strings.Builder
	cursor := 0
	for _, r := range deduped {
		if r.start > cursor {
			sb.WriteString(content[cursor:r.start])
		}
		sb.WriteString(r.value)
		cursor = r.end
	}
	// Append remaining content after last replacement
	if cursor < len(content) {
		sb.WriteString(content[cursor:])
	}

	return sb.String(), nil
}

// SetAction dynamically updates the action.
// Thread-safe.
func (s *Scanner) SetAction(action string) error {
	switch action {
	case "block", "redact", "warn":
		s.mu.Lock()
		s.config.Action = action
		s.mu.Unlock()
		return nil
	default:
		return fmt.Errorf("invalid action %q: must be block, redact, or warn", action)
	}
}

// maskDefault provides default masking for a matched value
func maskDefault(value string) string {
	runes := []rune(value)
	length := len(runes)
	switch {
	case length <= 4:
		return strings.Repeat("*", length)
	case length <= 8:
		return string(runes[:2]) + strings.Repeat("*", length-2)
	default:
		return string(runes[:3]) + strings.Repeat("*", length-6) + string(runes[length-3:])
	}
}

// formatSummary creates a human-readable summary of matches
func formatSummary(matches []CredentialMatch) string {
	typeCount := make(map[string]int)
	sevCount := make(map[string]int)
	for _, m := range matches {
		typeCount[m.Type]++
		sevCount[m.Severity]++
	}

	var parts []string
	if c, ok := sevCount["critical"]; ok {
		parts = append(parts, fmt.Sprintf("%d critical", c))
	}
	if c, ok := sevCount["high"]; ok {
		parts = append(parts, fmt.Sprintf("%d high", c))
	}
	if c, ok := sevCount["medium"]; ok {
		parts = append(parts, fmt.Sprintf("%d medium", c))
	}

	types := make([]string, 0, len(typeCount))
	for t := range typeCount {
		types = append(types, t)
	}

	return fmt.Sprintf("Found %d credential(s) (%s): %s",
		len(matches), strings.Join(parts, ", "), strings.Join(types, ", "))
}
