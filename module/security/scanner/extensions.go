// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package scanner

import (
	"path/filepath"
	"strings"
)

// ExtensionRules controls which files should be scanned based on extension.
type ExtensionRules struct {
	// ScanExtensions is a whitelist: only scan files with these extensions.
	// Empty means scan all (subject to SkipExtensions).
	ScanExtensions []string

	// SkipExtensions is a blacklist: skip files with these extensions.
	SkipExtensions []string
}

// ShouldScanFile decides whether a file should be scanned based on extension rules.
//
// Logic:
//  1. If ScanExtensions is non-empty, only scan files whose extension is in the list.
//  2. Otherwise, skip files whose extension is in SkipExtensions.
//  3. If both lists are empty, scan everything.
func ShouldScanFile(filePath string, rules ExtensionRules) bool {
	ext := strings.ToLower(filepath.Ext(filePath))

	// Whitelist mode
	if len(rules.ScanExtensions) > 0 {
		for _, allowed := range rules.ScanExtensions {
			if strings.ToLower(allowed) == ext {
				return true
			}
		}
		return false
	}

	// Blacklist mode
	if len(rules.SkipExtensions) > 0 {
		for _, skip := range rules.SkipExtensions {
			if strings.ToLower(skip) == ext {
				return false
			}
		}
	}

	return true
}
