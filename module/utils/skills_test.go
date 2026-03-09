// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package utils

import (
	"testing"
)

// TestValidateSkillIdentifier tests skill identifier validation
func TestValidateSkillIdentifier(t *testing.T) {
	tests := []struct {
		name       string
		identifier string
		wantErr    bool
		errMsg     string
	}{
		{
			name:       "valid simple identifier",
			identifier: "test-skill",
			wantErr:    false,
		},
		{
			name:       "valid identifier with numbers",
			identifier: "skill123",
			wantErr:    false,
		},
		{
			name:       "valid identifier with dots",
			identifier: "org.skill-name",
			wantErr:    false,
		},
		{
			name:       "valid identifier with underscores",
			identifier: "my_skill_name",
			wantErr:    false,
		},
		{
			name:       "valid identifier with hyphens",
			identifier: "my-skill-name",
			wantErr:    false,
		},
		{
			name:       "valid complex identifier",
			identifier: "com.example.skill-name.v1",
			wantErr:    false,
		},
		{
			name:       "empty string",
			identifier: "",
			wantErr:    true,
			errMsg:     "identifier is required",
		},
		{
			name:       "whitespace only",
			identifier: "   ",
			wantErr:    true,
			errMsg:     "identifier is required",
		},
		{
			name:       "leading/trailing whitespace",
			identifier: "  test-skill  ",
			wantErr:    false,
		},
		{
			name:       "tab and newline",
			identifier: "\t\n",
			wantErr:    true,
			errMsg:     "identifier is required",
		},
		{
			name:       "contains forward slash",
			identifier: "skill/name",
			wantErr:    true,
			errMsg:     "path separators",
		},
		{
			name:       "contains backslash",
			identifier: "skill\\name",
			wantErr:    true,
			errMsg:     "path separators",
		},
		{
			name:       "contains double dot",
			identifier: "skill..name",
			wantErr:    true,
			errMsg:     "directory traversal",
		},
		{
			name:       "single double dot",
			identifier: "..",
			wantErr:    true,
			errMsg:     "directory traversal",
		},
		{
			name:       "path traversal with slash",
			identifier: "../etc/passwd",
			wantErr:    true,
			errMsg:     "directory traversal",
		},
		{
			name:       "windows path traversal",
			identifier: "..\\..\\windows",
			wantErr:    true,
			errMsg:     "directory traversal",
		},
		{
			name:       "mixed path separators",
			identifier: "../path\\to/file",
			wantErr:    true,
			errMsg:     "directory traversal",
		},
		{
			name:       "absolute unix path",
			identifier: "/usr/local/skill",
			wantErr:    true,
			errMsg:     "path separators",
		},
		{
			name:       "absolute windows path",
			identifier: "C:\\Program Files\\skill",
			wantErr:    true,
			errMsg:     "path separators",
		},
		{
			name:       "valid single character",
			identifier: "a",
			wantErr:    false,
		},
		{
			name:       "valid with version",
			identifier: "skill@v1.0.0",
			wantErr:    false,
		},
		{
			name:       "valid with colon",
			identifier: "namespace:skill",
			wantErr:    false,
		},
		{
			name:       "contains only dots",
			identifier: "...",
			wantErr:    true,
			errMsg:     "directory traversal",
		},
		{
			name:       "starts with dot",
			identifier: ".hidden",
			wantErr:    false,
		},
		{
			name:       "contains special chars",
			identifier: "skill@#$%",
			wantErr:    false,
		},
		{
			name:       "unicode characters",
			identifier: "技能名称",
			wantErr:    false,
		},
		{
			name:       "emoji in identifier",
			identifier: "skill🚀",
			wantErr:    false,
		},
		{
			name:       "mixed case",
			identifier: "MySkillName",
			wantErr:    false,
		},
		{
			name:       "double dots in middle",
			identifier: "skill..component..name",
			wantErr:    true,
			errMsg:     "directory traversal",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			err := ValidateSkillIdentifier(tt.identifier)

			if (err != nil) != tt.wantErr {
				t.Errorf("ValidateSkillIdentifier(%q) error = %v, wantErr %v", tt.identifier, err, tt.wantErr)
				return
			}

			if tt.wantErr && tt.errMsg != "" {
				if err == nil {
					t.Errorf("ValidateSkillIdentifier(%q) expected error containing %q, got nil", tt.identifier, tt.errMsg)
					return
				}
				errStr := err.Error()
				if !containsIgnoreCase(errStr, tt.errMsg) {
					t.Errorf("ValidateSkillIdentifier(%q) error = %q, want error containing %q", tt.identifier, errStr, tt.errMsg)
				}
			}
		})
	}
}

// TestDerefStr tests string pointer dereferencing
func TestDerefStr(t *testing.T) {
	tests := []struct {
		name       string
		ptr        *string
		defaultVal string
		expected   string
	}{
		{
			name:       "nil pointer with default",
			ptr:        nil,
			defaultVal: "default",
			expected:   "default",
		},
		{
			name:       "nil pointer with empty default",
			ptr:        nil,
			defaultVal: "",
			expected:   "",
		},
		{
			name:       "non-nil pointer with value",
			ptr:        strPtr("value"),
			defaultVal: "default",
			expected:   "value",
		},
		{
			name:       "non-nil pointer with empty string",
			ptr:        strPtr(""),
			defaultVal: "default",
			expected:   "",
		},
		{
			name:       "non-nil pointer with spaces",
			ptr:        strPtr("   "),
			defaultVal: "default",
			expected:   "   ",
		},
		{
			name:       "non-nil pointer with special chars",
			ptr:        strPtr("特殊字符"),
			defaultVal: "default",
			expected:   "特殊字符",
		},
		{
			name:       "nil pointer without default",
			ptr:        nil,
			defaultVal: "",
			expected:   "",
		},
		{
			name:       "non-nil pointer with long string",
			ptr:        strPtr("this is a very long string that should be returned as is"),
			defaultVal: "default",
			expected:   "this is a very long string that should be returned as is",
		},
		{
			name:       "non-nil pointer with unicode",
			ptr:        strPtr("Hello 世界 🌍"),
			defaultVal: "default",
			expected:   "Hello 世界 🌍",
		},
		{
			name:       "non-nil pointer with newlines",
			ptr:        strPtr("line1\nline2\nline3"),
			defaultVal: "default",
			expected:   "line1\nline2\nline3",
		},
		{
			name:       "non-nil pointer with tabs",
			ptr:        strPtr("col1\tcol2\tcol3"),
			defaultVal: "default",
			expected:   "col1\tcol2\tcol3",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := DerefStr(tt.ptr, tt.defaultVal)
			if got != tt.expected {
				t.Errorf("DerefStr() = %q, want %q", got, tt.expected)
			}
		})
	}
}

// TestValidateSkillIdentifier_EdgeCases tests additional edge cases
func TestValidateSkillIdentifier_EdgeCases(t *testing.T) {
	tests := []struct {
		name       string
		identifier string
		wantErr    bool
	}{
		{
			name:       "starts with slash after trim",
			identifier: " /skill",
			wantErr:    true,
		},
		{
			name:       "ends with slash after trim",
			identifier: "skill/ ",
			wantErr:    true,
		},
		{
			name:       "multiple slashes",
			identifier: "skill//name",
			wantErr:    true,
		},
		{
			name:       "backslash in middle",
			identifier: "skill\\name",
			wantErr:    true,
		},
		{
			name:       "double dot at start",
			identifier: "..skill",
			wantErr:    true,
		},
		{
			name:       "double dot at end",
			identifier: "skill..",
			wantErr:    true,
		},
		{
			name:       "triple dot",
			identifier: "skill...",
			wantErr:    true,
		},
		{
			name:       "slash and double dot",
			identifier: "../",
			wantErr:    true,
		},
		{
			name:       "double dot and slash",
			identifier: "./../",
			wantErr:    true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			err := ValidateSkillIdentifier(tt.identifier)
			if (err != nil) != tt.wantErr {
				t.Errorf("ValidateSkillIdentifier(%q) error = %v, wantErr %v", tt.identifier, err, tt.wantErr)
			}
		})
	}
}

// TestDerefStr_Modification tests that modifications don't affect original
func TestDerefStr_Modification(t *testing.T) {
	original := "original"
	ptr := &original

	// Dereference
	_ = DerefStr(ptr, "default")

	// Dereference again
	result2 := DerefStr(ptr, "default")
	if result2 != "original" {
		t.Errorf("Second dereference failed: got %q, want 'original'", result2)
	}
}

// TestValidateSkillIdentifier_RealWorldExamples tests real-world skill identifiers
func TestValidateSkillIdentifier_RealWorldExamples(t *testing.T) {
	validExamples := []string{
		"structured-development",
		"build-project",
		"com.example.skill",
		"org.tools.analyzer",
		"skill@1.0.0",
		"my-skill_v2",
		"python-script",
		"typescript-compiler",
		"docker-builder",
		"kubernetes-deployer",
		"git-integration",
		"database-migrator",
		"api-client",
		"web-scraper",
		"data-processor",
		"ml-model-trainer",
		"test-runner",
		"code-generator",
		"documentation-helper",
		"security-scanner",
		"performance-monitor",
	}

	invalidExamples := []string{
		"../malicious",
		"..\\windows",
		"/etc/passwd",
		"C:\\Windows\\System32",
		"./../escape",
		"skill/../../../etc",
		"skill\\..\\..\\windows",
	}

	for _, example := range validExamples {
		t.Run("valid_"+example, func(t *testing.T) {
			err := ValidateSkillIdentifier(example)
			if err != nil {
				t.Errorf("Expected valid identifier %q to pass validation, got error: %v", example, err)
			}
		})
	}

	for _, example := range invalidExamples {
		t.Run("invalid_"+example, func(t *testing.T) {
			err := ValidateSkillIdentifier(example)
			if err == nil {
				t.Errorf("Expected invalid identifier %q to fail validation", example)
			}
		})
	}
}

// Helper functions

// strPtr returns a pointer to the given string
func strPtr(s string) *string {
	return &s
}

// containsIgnoreCase checks if a string contains a substring (case-insensitive)
func containsIgnoreCase(s, substr string) bool {
	return len(s) >= len(substr) &&
		(s == substr ||
		 len(s) > len(substr) && containsIgnoreCaseHelper(s, substr))
}

func containsIgnoreCaseHelper(s, substr string) bool {
	for i := 0; i <= len(s)-len(substr); i++ {
		match := true
		for j := 0; j < len(substr); j++ {
			if toLower(s[i+j]) != toLower(substr[j]) {
				match = false
				break
			}
		}
		if match {
			return true
		}
	}
	return false
}

func toLower(c byte) byte {
	if c >= 'A' && c <= 'Z' {
		return c + ('a' - 'A')
	}
	return c
}
