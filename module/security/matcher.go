// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package security

import (
	"regexp"
	"strings"
)

// MatchPattern checks if a target matches a pattern with wildcard support
// Supported wildcards:
//   - - matches any sequence in a single directory level (e.g., *.key, D:/123/*.key)
//     ** - matches any sequence across multiple directory levels (e.g., D:/123/**.key)
//     No wildcard - exact match (e.g., /etc/passwd)
//
// Special case: patterns without directory separator (e.g., *.key) match globally
// across all directories (e.g., *.key matches /home/user/test.key)
//
// Examples:
//
//	*.key              matches all .key files in all directories (global pattern)
//	D:/123/*.key        matches .key files directly in D:/123/
//	D:/123/**.key       matches .key files in D:/123/ and all subdirectories
//	/etc/passwd        matches exactly /etc/passwd
func MatchPattern(pattern, target string) bool {
	// Normalize path separators to /
	pattern = normalizePath(pattern)
	target = normalizePath(target)

	// If no wildcards, do exact match
	if !strings.Contains(pattern, "*") {
		return pattern == target
	}

	// Special case: if pattern has no directory separator and has wildcards,
	// it's a global pattern - prepend ** to match across all directories
	// Example: *.key becomes **.key to match test.key, /home/test.key, etc.
	if !strings.Contains(pattern, "/") {
		pattern = "**" + pattern
	}

	// Convert wildcard pattern to regex
	regexPattern := wildcardToRegex(pattern)

	// Compile and match
	matched, _ := regexp.MatchString(regexPattern, target)
	return matched
}

// normalizePath converts all path separators to /
func normalizePath(path string) string {
	return strings.ReplaceAll(path, "\\", "/")
}

// wildcardToRegex converts a wildcard pattern to a regex pattern
// Supports:
//   - - matches any sequence except / (single directory level)
//     ** - matches any sequence including / (multiple directory levels)
func wildcardToRegex(pattern string) string {
	var regex strings.Builder
	regex.WriteString("^")

	i := 0
	for i < len(pattern) {
		switch {
		case i+1 < len(pattern) && pattern[i:i+2] == "**":
			// ** - matches any sequence including /
			regex.WriteString(".*")
			i += 2
		case pattern[i] == '*':
			// * - matches any sequence except /
			regex.WriteString("[^/]*")
			i++
		case pattern[i] == '.' || pattern[i] == '^' || pattern[i] == '$' || pattern[i] == '+' || pattern[i] == '(' || pattern[i] == ')' || pattern[i] == '[' || pattern[i] == ']' || pattern[i] == '{' || pattern[i] == '}' || pattern[i] == '|' || pattern[i] == '\\':
			// Escape special regex characters
			regex.WriteString("\\")
			regex.WriteByte(pattern[i])
			i++
		default:
			regex.WriteByte(pattern[i])
			i++
		}
	}

	regex.WriteString("$")
	return regex.String()
}

// MatchCommandPattern checks if a command matches a pattern
// Supports * wildcard for command arguments
// Examples:
//
//	"git *"         matches "git status", "git commit -m 'msg'"
//	"rm -rf *"      matches "rm -rf /tmp/test"
//	"*sudo*"        matches "sudo apt-get install", "sudo vim"
func MatchCommandPattern(pattern, command string) bool {
	// For commands, * matches any characters including spaces
	// We need to replace * with .* BEFORE quoting the rest of the pattern
	// Use a placeholder to preserve wildcards through QuoteMeta
	placeholder := "\x00WILDCARD\x00"
	pattern = strings.ReplaceAll(pattern, "*", placeholder)
	pattern = regexp.QuoteMeta(pattern)
	pattern = strings.ReplaceAll(pattern, placeholder, ".*")
	regexPattern := "^" + pattern + "$"
	matched, _ := regexp.MatchString(regexPattern, command)
	return matched
}

// MatchDomainPattern checks if a domain matches a pattern
// Examples:
//
//	"*.github.com"  matches "api.github.com", "raw.githubusercontent.com"
//	"*.openai.com"  matches "api.openai.com"
//	"github.com"    matches exactly "github.com"
func MatchDomainPattern(pattern, domain string) bool {
	domain = strings.ToLower(domain)
	pattern = strings.ToLower(pattern)

	// No wildcard - exact match
	if !strings.Contains(pattern, "*") {
		return domain == pattern
	}

	// Convert *.example.com to regex
	// Use placeholders to preserve wildcards through transformations
	wildcardPlaceholder := "\x00WILDCARD\x00"
	dotPlaceholder := "\x00LITERALESCAPEDDOT\x00"

	// Step 1: Replace wildcards with placeholder
	pattern = strings.ReplaceAll(pattern, "*", wildcardPlaceholder)

	// Step 2: Replace literal dots with placeholder (to be escaped later)
	pattern = strings.ReplaceAll(pattern, ".", dotPlaceholder)

	// Step 3: Escape the remaining special characters
	pattern = regexp.QuoteMeta(pattern)

	// Step 4: Replace placeholders with actual regex patterns
	// For domains, * should match only a single subdomain level (anything except dot)
	pattern = strings.ReplaceAll(pattern, wildcardPlaceholder, "[^.]*")
	pattern = strings.ReplaceAll(pattern, dotPlaceholder, "\\.")

	regexPattern := "^" + pattern + "$"
	matched, _ := regexp.MatchString(regexPattern, domain)
	return matched
}
