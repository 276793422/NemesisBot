// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Package command provides dangerous command detection and blocking
package command

import (
	"context"
	"fmt"
	"regexp"
	"strings"
	"sync"
)

// Config configures the command guard behavior.
type Config struct {
	Enabled       bool     // Enable command guard
	StrictMode    bool     // Block on partial match
	CustomBlocked []string // Additional regex patterns to block
	Allowed       []string // Override: allow specific patterns (regex)
}

// BlockEntry describes a single dangerous command pattern.
type BlockEntry struct {
	Pattern  string // regex pattern
	Category string // "destructive", "network", "privilege", "recon"
	Severity string // "critical", "high", "medium"
	Platform string // "linux", "windows", "any"
	Reason   string // human-readable explanation

	compiled *regexp.Regexp // cached compiled regex
}

// Guard checks commands against a blocklist of dangerous patterns.
// It is thread-safe for concurrent use.
type Guard struct {
	mu        sync.RWMutex
	entries   []BlockEntry
	config    Config
	allowRe   []*regexp.Regexp // compiled allow-list patterns
}

// NewGuard creates a new command guard with the default blocklist applied.
// Custom blocked patterns from cfg are appended, and allowed patterns are
// compiled for override matching.
func NewGuard(cfg Config) (*Guard, error) {
	g := &Guard{
		config: cfg,
	}

	// Load default blocklist
	g.entries = DefaultBlocklist()

	// Append custom blocked patterns
	for _, pat := range cfg.CustomBlocked {
		re, err := regexp.Compile("(?i)" + pat)
		if err != nil {
			return nil, fmt.Errorf("invalid custom blocklist pattern %q: %w", pat, err)
		}
		g.entries = append(g.entries, BlockEntry{
			Pattern:  pat,
			Category: "custom",
			Severity: "high",
			Platform: "any",
			Reason:   "custom blocklist pattern",
			compiled: re,
		})
	}

	// Compile all default patterns
	for i := range g.entries {
		if g.entries[i].compiled == nil {
			re, err := regexp.Compile("(?i)" + g.entries[i].Pattern)
			if err != nil {
				return nil, fmt.Errorf("invalid blocklist pattern %q: %w", g.entries[i].Pattern, err)
			}
			g.entries[i].compiled = re
		}
	}

	// Compile allowed patterns
	for _, pat := range cfg.Allowed {
		re, err := regexp.Compile("(?i)" + pat)
		if err != nil {
			return nil, fmt.Errorf("invalid allowlist pattern %q: %w", pat, err)
		}
		g.allowRe = append(g.allowRe, re)
	}

	return g, nil
}

// Check inspects a command and returns an error describing why it is blocked.
// Returns nil if the command is safe.  The context is accepted for future
// extensibility (timeouts, tracing, etc.) but is not used at present.
func (g *Guard) Check(_ context.Context, command string) error {
	if !g.config.Enabled {
		return nil
	}

	g.mu.RLock()
	defer g.mu.RUnlock()

	// Normalize for matching
	normalized := strings.TrimSpace(command)
	if normalized == "" {
		return nil
	}

	// Check allow-list first (overrides blocklist)
	for _, re := range g.allowRe {
		if re.MatchString(normalized) {
			return nil
		}
	}

	// Walk blocklist entries
	for _, entry := range g.entries {
		if entry.compiled.MatchString(normalized) {
			return &BlockedError{
				Command:  command,
				Category: entry.Category,
				Severity: entry.Severity,
				Platform: entry.Platform,
				Reason:   entry.Reason,
			}
		}

		// In strict mode, also check if any blocked pattern is a substring
		// of the command after removing common separators.
		if g.config.StrictMode {
			simplified := simplifyCommand(normalized)
			patSimplified := strings.ReplaceAll(entry.Pattern, `\b`, "")
			patSimplified = strings.ReplaceAll(patSimplified, `\s+`, " ")
			patSimplified = strings.ReplaceAll(patSimplified, `.*`, "")
			patSimplified = strings.ReplaceAll(patSimplified, `.`, "")
			if patSimplified != "" && len(patSimplified) >= 4 {
				if strings.Contains(simplified, patSimplified) {
					return &BlockedError{
						Command:  command,
						Category: entry.Category,
						Severity: entry.Severity,
						Platform: entry.Platform,
						Reason:   fmt.Sprintf("%s (strict mode partial match)", entry.Reason),
					}
				}
			}
		}
	}

	return nil
}

// IsBlocked returns true if the command matches a blocked pattern.
// This is a convenience wrapper around Check for boolean results.
func (g *Guard) IsBlocked(command string) bool {
	return g.Check(context.Background(), command) != nil
}

// GetCategory returns the blocklist category for a command, or an empty
// string if the command is not blocked.
func (g *Guard) GetCategory(command string) string {
	if !g.config.Enabled {
		return ""
	}

	g.mu.RLock()
	defer g.mu.RUnlock()

	normalized := strings.TrimSpace(command)
	if normalized == "" {
		return ""
	}

	// Check allow-list first
	for _, re := range g.allowRe {
		if re.MatchString(normalized) {
			return ""
		}
	}

	for _, entry := range g.entries {
		if entry.compiled.MatchString(normalized) {
			return entry.Category
		}
	}
	return ""
}

// GetBlockedEntry returns the first BlockEntry that matches the command,
// or nil if the command is not blocked.
func (g *Guard) GetBlockedEntry(command string) *BlockEntry {
	if !g.config.Enabled {
		return nil
	}

	g.mu.RLock()
	defer g.mu.RUnlock()

	normalized := strings.TrimSpace(command)
	if normalized == "" {
		return nil
	}

	for _, re := range g.allowRe {
		if re.MatchString(normalized) {
			return nil
		}
	}

	for i := range g.entries {
		if g.entries[i].compiled.MatchString(normalized) {
			return &g.entries[i]
		}
	}
	return nil
}

// Entries returns a copy of the current blocklist entries.
func (g *Guard) Entries() []BlockEntry {
	g.mu.RLock()
	defer g.mu.RUnlock()

	out := make([]BlockEntry, len(g.entries))
	copy(out, g.entries)
	return out
}

// AddEntry adds a new blocklist entry at runtime.  The pattern is compiled
// with case-insensitive matching.
func (g *Guard) AddEntry(entry BlockEntry) error {
	re, err := regexp.Compile("(?i)" + entry.Pattern)
	if err != nil {
		return fmt.Errorf("invalid pattern %q: %w", entry.Pattern, err)
	}

	g.mu.Lock()
	defer g.mu.Unlock()

	entry.compiled = re
	g.entries = append(g.entries, entry)
	return nil
}

// RemoveEntry removes all entries whose Pattern exactly matches the given
// pattern string.
func (g *Guard) RemoveEntry(pattern string) {
	g.mu.Lock()
	defer g.mu.Unlock()

	filtered := g.entries[:0]
	for _, e := range g.entries {
		if e.Pattern != pattern {
			filtered = append(filtered, e)
		}
	}
	g.entries = filtered
}

// SetConfig updates the guard configuration.
func (g *Guard) SetConfig(cfg Config) error {
	g.mu.Lock()
	defer g.mu.Unlock()

	// Recompile allow patterns
	allowRe := make([]*regexp.Regexp, 0, len(cfg.Allowed))
	for _, pat := range cfg.Allowed {
		re, err := regexp.Compile("(?i)" + pat)
		if err != nil {
			return fmt.Errorf("invalid allowlist pattern %q: %w", pat, err)
		}
		allowRe = append(allowRe, re)
	}

	g.config = cfg
	g.allowRe = allowRe
	return nil
}

// BlockedError is returned when a command is blocked by the guard.
type BlockedError struct {
	Command  string
	Category string
	Severity string
	Platform string
	Reason   string
}

func (e *BlockedError) Error() string {
	return fmt.Sprintf("command blocked [%s/%s]: %s (command: %s)",
		e.Category, e.Severity, e.Reason, e.Command)
}

// simplifyCommand removes extra whitespace and common separators for
// strict-mode substring matching.
func simplifyCommand(cmd string) string {
	// Lowercase and collapse whitespace
	s := strings.ToLower(cmd)
	s = strings.Join(strings.Fields(s), " ")
	return s
}
