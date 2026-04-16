// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package utils

import (
	"fmt"
	"strings"
)

// ValidateSkillIdentifier validates that the given skill identifier (slug or registry name) is non-empty
// and does not contain path traversal sequences ("..", "\") for security.
// A single "/" is allowed to support author/slug format (e.g. "clawcv/pdf-export" for three-layer repos).
func ValidateSkillIdentifier(identifier string) error {
	trimmed := strings.TrimSpace(identifier)
	if trimmed == "" {
		return fmt.Errorf("identifier is required and must be a non-empty string")
	}
	// Backslash is never allowed (Windows path separator)
	if strings.Contains(trimmed, "\\") {
		return fmt.Errorf("identifier must not contain backslashes to prevent directory traversal")
	}
	// ".." is never allowed (path traversal)
	if strings.Contains(trimmed, "..") {
		return fmt.Errorf("identifier must not contain '..' to prevent directory traversal")
	}
	// Allow at most one "/" for author/slug format (e.g. "clawcv/pdf-export")
	if strings.Count(trimmed, "/") > 1 {
		return fmt.Errorf("identifier must not contain multiple slashes")
	}
	return nil
}

// DerefStr safely dereferences a string pointer, returning the value or a default if nil.
func DerefStr(ptr *string, defaultVal string) string {
	if ptr != nil {
		return *ptr
	}
	return defaultVal
}
