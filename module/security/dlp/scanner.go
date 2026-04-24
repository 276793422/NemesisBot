// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package dlp

// scanString scans content against all given compiled rules and returns matches.
// The returned matches are sorted by position (ascending).
func scanString(content string, rules []compiledRule) []Match {
	if len(rules) == 0 || len(content) == 0 {
		return nil
	}

	seen := make(map[string]struct{})
	var matches []Match

	for _, cr := range rules {
		locs := cr.re.FindAllStringIndex(content, -1)
		for _, loc := range locs {
			start := loc[0]
			end := loc[1]
			matchedText := content[start:end]

			// Deduplicate by (ruleName, position) to avoid duplicate matches
			// when multiple regex passes produce the same result.
			dedupeKey := cr.Name + "|" + string(rune(start))
			if _, ok := seen[dedupeKey]; ok {
				continue
			}
			seen[dedupeKey] = struct{}{}

			matches = append(matches, Match{
				RuleName:    cr.Name,
				Category:    cr.Category,
				Severity:    cr.Severity,
				MaskedValue: maskValue(matchedText),
				Position:    start,
				FullMatch:   isFullMatch(content, start, end),
			})
		}
	}

	// Sort by position.
	sortMatchesByPosition(matches)
	return matches
}

// maskValue masks a matched string, showing only the first 2 and last 2 characters.
// If the string is 4 characters or fewer, it is fully masked.
// If the string is 5-6 characters, show first 2 and last 1.
func maskValue(s string) string {
	length := len(s)
	if length <= 4 {
		return "****"
	}
	if length <= 6 {
		return s[:2] + "****" + s[length-1:]
	}
	return s[:2] + "****" + s[length-2:]
}

// isFullMatch returns true if the matched region is bounded by word boundaries
// or string edges, indicating a complete pattern match rather than a partial one.
func isFullMatch(content string, start, end int) bool {
	// Check if the character before the match is a word boundary.
	if start > 0 {
		ch := content[start-1]
		if isWordChar(ch) {
			return false
		}
	}
	// Check if the character after the match is a word boundary.
	if end < len(content) {
		ch := content[end]
		if isWordChar(ch) {
			return false
		}
	}
	return true
}

// isWordChar returns true if the byte is an alphanumeric or underscore character.
func isWordChar(b byte) bool {
	return (b >= 'a' && b <= 'z') ||
		(b >= 'A' && b <= 'Z') ||
		(b >= '0' && b <= '9') ||
		b == '_'
}

// sortMatchesByPosition sorts matches by byte position in ascending order.
// Uses a simple insertion sort since match counts are typically small.
func sortMatchesByPosition(matches []Match) {
	for i := 1; i < len(matches); i++ {
		for j := i; j > 0 && matches[j].Position < matches[j-1].Position; j-- {
			matches[j], matches[j-1] = matches[j-1], matches[j]
		}
	}
}
