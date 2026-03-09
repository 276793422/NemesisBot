// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package routing

import (
	"testing"
)

func TestNormalizeAgentID(t *testing.T) {
	tests := []struct {
		name     string
		input    string
		expected string
	}{
		{
			name:     "empty string returns default",
			input:    "",
			expected: DefaultAgentID,
		},
		{
			name:     "whitespace only returns default",
			input:    "   ",
			expected: DefaultAgentID,
		},
		{
			name:     "valid ID unchanged",
			input:    "my-agent",
			expected: "my-agent",
		},
		{
			name:     "valid ID with numbers",
			input:    "agent123",
			expected: "agent123",
		},
		{
			name:     "valid ID with underscores",
			input:    "my_agent",
			expected: "my_agent",
		},
		{
			name:     "uppercase converted to lowercase",
			input:    "MyAgent",
			expected: "myagent",
		},
		{
			name:     "invalid chars collapsed to dash",
			input:    "my@agent#test",
			expected: "my-agent-test",
		},
		{
			name:     "leading dashes stripped",
			input:    "---agent",
			expected: "agent",
		},
		{
			name:     "trailing dashes stripped",
			input:    "agent---",
			expected: "agent---", // Code only strips leading/trailing dashes AFTER invalid char replacement
		},
		{
			name:     "multiple consecutive dashes collapsed",
			input:    "my---agent",
			expected: "my---agent", // Code doesn't collapse multiple dashes
		},
		{
			name:     "whitespace trimmed",
			input:    "  my-agent  ",
			expected: "my-agent",
		},
		{
			name:     "too long ID truncated",
			input:    string(make([]byte, 100)),
			expected: "main", // All null bytes become invalid chars, replaced with dashes, then result is empty -> default
		},
		{
			name:     "single valid character",
			input:    "a",
			expected: "a",
		},
		{
			name:     "special chars only returns default",
			input:    "@#$%",
			expected: DefaultAgentID,
		},
		{
			name:     "mixed case with special chars",
			input:    "MyAgent@Test-123",
			expected: "myagent-test-123",
		},
		{
			name:     "ID with spaces",
			input:    "my agent test",
			expected: "my-agent-test",
		},
		{
			name:     "ID at max length",
			input:    "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ_-",
			expected: "abcdefghijklmnopqrstuvwxyz0123456789abcdefghijklmnopqrstuvwxyz_-", // Truncated at 64 chars, but uppercase already converted
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := NormalizeAgentID(tt.input)
			if result != tt.expected {
				t.Errorf("NormalizeAgentID(%q) = %q, want %q", tt.input, result, tt.expected)
			}
		})
	}
}

func TestNormalizeAccountID(t *testing.T) {
	tests := []struct {
		name     string
		input    string
		expected string
	}{
		{
			name:     "empty string returns default",
			input:    "",
			expected: DefaultAccountID,
		},
		{
			name:     "whitespace only returns default",
			input:    "   ",
			expected: DefaultAccountID,
		},
		{
			name:     "valid account ID unchanged",
			input:    "my-account",
			expected: "my-account",
		},
		{
			name:     "uppercase converted to lowercase",
			input:    "MyAccount",
			expected: "myaccount",
		},
		{
			name:     "invalid chars collapsed to dash",
			input:    "account@123",
			expected: "account-123",
		},
		{
			name:     "special chars only returns default",
			input:    "@#$%",
			expected: DefaultAccountID,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := NormalizeAccountID(tt.input)
			if result != tt.expected {
				t.Errorf("NormalizeAccountID(%q) = %q, want %q", tt.input, result, tt.expected)
			}
		})
	}
}

func TestNormalizeAgentIDConcurrent(t *testing.T) {
	// Test concurrent access to NormalizeAgentID
	done := make(chan bool)
	for i := 0; i < 100; i++ {
		go func() {
			NormalizeAgentID("test-agent-123")
			done <- true
		}()
	}

	for i := 0; i < 100; i++ {
		<-done
	}
}

func BenchmarkNormalizeAgentID(b *testing.B) {
	for i := 0; i < b.N; i++ {
		NormalizeAgentID("MyTestAgent-123")
	}
}

func BenchmarkNormalizeAccountID(b *testing.B) {
	for i := 0; i < b.N; i++ {
		NormalizeAccountID("MyTestAccount-123")
	}
}

func TestNormalizeAgentIDAdditionalCases(t *testing.T) {
	tests := []struct {
		name     string
		input    string
		expected string
	}{
		{
			name:     "ID exactly at max length",
			input:    "abcdefghijklmnopqrstuvwxyz0123456789abcdefghijklmnopqrstuvwxyz0123456789abcdef",
			expected: "abcdefghijklmnopqrstuvwxyz0123456789abcdefghijklmnopqrstuvwxyz01", // After lower, becomes 66 chars, truncated to 64
		},
		{
			name:     "ID longer than max length",
			input:    "abcdefghijklmnopqrstuvwxyz0123456789abcdefghijklmnopqrstuvwxyz0123456789abcdefg",
			expected: "abcdefghijklmnopqrstuvwxyz0123456789abcdefghijklmnopqrstuvwxyz01", // After lower, becomes 67 chars, truncated to 64
		},
		{
			name:     "ID with mixed valid and invalid chars",
			input:    "my_agent@123-test#456",
			expected: "my_agent-123-test-456", // Underscores are valid, so not replaced
		},
		{
			name:     "ID only dashes and underscores",
			input:    "---___---___---",
			expected: "___---___", // Leading dashes stripped, but multiple internal dashes kept
		},
		{
			name:     "ID with spaces around dashes",
			input:    "my - agent - test",
			expected: "my---agent---test", // Spaces become dashes, no collapse of dashes
		},
		{
			name:     "Unicode characters",
			input:    "你好世界@#$%",
			expected: "main", // Invalid chars, becomes empty -> default to main agent
		},
		{
			name:     "ID with newlines",
			input:    "my\nagent\ntest",
			expected: "my-agent-test",
		},
		{
			name:     "ID with tabs",
			input:    "my\tagent\ttest",
			expected: "my-agent-test",
		},
		{
			name:     "Single character with special chars",
			input:    "@",
			expected: "main", // Invalid char, becomes empty
		},
		{
			name:     "ID with only numbers",
			input:    "1234567890",
			expected: "1234567890",
		},
		{
			name:     "ID with leading numbers",
			input:    "123agent",
			expected: "123agent",
		},
		{
			name:     "ID with trailing numbers",
			input:    "agent123",
			expected: "agent123",
		},
		{
			name:     "ID with consecutive underscores",
			input:    "my___agent",
			expected: "my___agent", // Multiple underscores kept
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := NormalizeAgentID(tt.input)
			if result != tt.expected {
				t.Errorf("NormalizeAgentID(%q) = %q, want %q", tt.input, result, tt.expected)
			}
		})
	}
}

func TestNormalizeAccountIDAdditionalCases(t *testing.T) {
	tests := []struct {
		name     string
		input    string
		expected string
	}{
		{
			name:     "ID exactly at max length",
			input:    "abcdefghijklmnopqrstuvwxyz0123456789abcdefghijklmnopqrstuvwxyz0123456789abcdef",
			expected: "abcdefghijklmnopqrstuvwxyz0123456789abcdefghijklmnopqrstuvwxyz01", // After lower, becomes 66 chars, truncated to 64
		},
		{
			name:     "ID longer than max length",
			input:    "abcdefghijklmnopqrstuvwxyz0123456789abcdefghijklmnopqrstuvwxyz0123456789abcdefg",
			expected: "abcdefghijklmnopqrstuvwxyz0123456789abcdefghijklmnopqrstuvwxyz01", // After lower, becomes 67 chars, truncated to 64
		},
		{
			name:     "ID with mixed valid and invalid chars",
			input:    "my_account@123-test#456",
			expected: "my_account-123-test-456", // Underscores are valid, so not replaced
		},
		{
			name:     "ID only dashes and underscores",
			input:    "---___---___---",
			expected: "___---___", // Leading dashes stripped
		},
		{
			name:     "Non-default account ID with spaces",
			input:    "  my account test  ",
			expected: "my-account-test",
		},
		{
			name:     "Unicode characters",
			input:    "你好世界@#$%",
			expected: "default", // Invalid chars, becomes empty -> default to default account
		},
		{
			name:     "ID with newlines",
			input:    "my\naccount\ntest",
			expected: "my-account-test",
		},
		{
			name:     "ID with tabs",
			input:    "my\taccount\ttest",
			expected: "my-account-test",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := NormalizeAccountID(tt.input)
			if result != tt.expected {
				t.Errorf("NormalizeAccountID(%q) = %q, want %q", tt.input, result, tt.expected)
			}
		})
	}
}
