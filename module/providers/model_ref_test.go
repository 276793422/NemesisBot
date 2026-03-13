// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package providers

import (
	"strings"
	"testing"
)

// TestParseModelRef_ValidFormats tests valid model reference formats
func TestParseModelRef_ValidFormats(t *testing.T) {
	tests := []struct {
		name             string
		input            string
		defaultProvider  string
		expectedProvider string
		expectedModel    string
	}{
		{
			name:             "full reference with slash",
			input:            "anthropic/claude-3-5",
			defaultProvider:  "openai",
			expectedProvider: "anthropic",
			expectedModel:    "claude-3-5",
		},
		{
			name:             "model only - uses default provider",
			input:            "gpt-4",
			defaultProvider:  "openai",
			expectedProvider: "openai",
			expectedModel:    "gpt-4",
		},
		{
			name:             "with spaces",
			input:            "  anthropic/claude-3-5  ",
			defaultProvider:  "openai",
			expectedProvider: "anthropic",
			expectedModel:    "claude-3-5",
		},
		{
			name:             "model with version",
			input:            "google/gemini-2.0-flash",
			defaultProvider:  "openai",
			expectedProvider: "gemini",
			expectedModel:    "gemini-2.0-flash",
		},
		{
			name:             "z.ai provider",
			input:            "z.ai/model",
			defaultProvider:  "openai",
			expectedProvider: "zai",
			expectedModel:    "model",
		},
		{
			name:             "z-ai provider",
			input:            "z-ai/model",
			defaultProvider:  "openai",
			expectedProvider: "zai",
			expectedModel:    "model",
		},
		{
			name:             "opencode-zen provider",
			input:            "opencode-zen/model",
			defaultProvider:  "openai",
			expectedProvider: "opencode",
			expectedModel:    "model",
		},
		{
			name:             "qwen provider",
			input:            "qwen/model",
			defaultProvider:  "openai",
			expectedProvider: "qwen-portal",
			expectedModel:    "model",
		},
		{
			name:             "kimi-code provider",
			input:            "kimi-code/model",
			defaultProvider:  "openai",
			expectedProvider: "kimi-coding",
			expectedModel:    "model",
		},
		{
			name:             "gpt provider",
			input:            "gpt/model",
			defaultProvider:  "openai",
			expectedProvider: "openai",
			expectedModel:    "model",
		},
		{
			name:             "claude provider",
			input:            "claude/model",
			defaultProvider:  "openai",
			expectedProvider: "anthropic",
			expectedModel:    "model",
		},
		{
			name:             "glm provider",
			input:            "glm/model",
			defaultProvider:  "openai",
			expectedProvider: "zhipu",
			expectedModel:    "model",
		},
		{
			name:             "google provider",
			input:            "google/model",
			defaultProvider:  "openai",
			expectedProvider: "gemini",
			expectedModel:    "model",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := ParseModelRef(tt.input, tt.defaultProvider)
			if result == nil {
				t.Fatal("expected non-nil result")
			}

			if result.Provider != tt.expectedProvider {
				t.Errorf("expected provider '%s', got '%s'", tt.expectedProvider, result.Provider)
			}

			if result.Model != tt.expectedModel {
				t.Errorf("expected model '%s', got '%s'", tt.expectedModel, result.Model)
			}
		})
	}
}

// TestParseModelRef_InvalidFormats tests invalid model reference formats
func TestParseModelRef_InvalidFormats(t *testing.T) {
	tests := []struct {
		name            string
		input           string
		defaultProvider string
	}{
		{
			name:            "empty string",
			input:           "",
			defaultProvider: "openai",
		},
		{
			name:            "whitespace only",
			input:           "   ",
			defaultProvider: "openai",
		},
		{
			name:            "slash but no model",
			input:           "anthropic/",
			defaultProvider: "openai",
		},
		{
			name:            "slash with spaces but no model",
			input:           "anthropic/   ",
			defaultProvider: "openai",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := ParseModelRef(tt.input, tt.defaultProvider)
			if result != nil {
				t.Error("expected nil result for invalid input")
			}
		})
	}
}

// TestParseModelRef_ModelOnlyTests tests model-only references
func TestParseModelRef_ModelOnlyTests(t *testing.T) {
	tests := []struct {
		name            string
		input           string
		defaultProvider string
		expectedModel   string
	}{
		{
			name:            "simple model name",
			input:           "claude-3-5",
			defaultProvider: "anthropic",
			expectedModel:   "claude-3-5",
		},
		{
			name:            "model with version",
			input:           "gpt-4-turbo",
			defaultProvider: "openai",
			expectedModel:   "gpt-4-turbo",
		},
		{
			name:            "model with dots",
			input:           "  gemini.pro  ",
			defaultProvider: "google",
			expectedModel:   "gemini.pro",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := ParseModelRef(tt.input, tt.defaultProvider)
			if result == nil {
				t.Fatal("expected non-nil result")
			}

			if result.Model != tt.expectedModel {
				t.Errorf("expected model '%s', got '%s'", tt.expectedModel, result.Model)
			}

			// Provider should be normalized version of defaultProvider
			expectedProvider := NormalizeProvider(tt.defaultProvider)
			if result.Provider != expectedProvider {
				t.Errorf("expected provider '%s', got '%s'", expectedProvider, result.Provider)
			}
		})
	}
}

// TestNormalizeProvider tests provider name normalization
func TestNormalizeProvider(t *testing.T) {
	tests := []struct {
		input    string
		expected string
	}{
		// Aliases
		{"z.ai", "zai"},
		{"z-ai", "zai"},
		{"opencode-zen", "opencode"},
		{"qwen", "qwen-portal"},
		{"kimi-code", "kimi-coding"},
		{"gpt", "openai"},
		{"claude", "anthropic"},
		{"glm", "zhipu"},
		{"google", "gemini"},
		// Case normalization
		{"Anthropic", "anthropic"},
		{"OPENAI", "openai"},
		{"Google", "gemini"},
		// Whitespace trimming
		{"  anthropic  ", "anthropic"},
		{"		openai		", "openai"},
		// Already normalized
		{"anthropic", "anthropic"},
		{"openai", "openai"},
		{"gemini", "gemini"},
		{"zai", "zai"},
		{"opencode", "opencode"},
		{"qwen-portal", "qwen-portal"},
		{"kimi-coding", "kimi-coding"},
		{"zhipu", "zhipu"},
	}

	for _, tt := range tests {
		t.Run(tt.input, func(t *testing.T) {
			result := NormalizeProvider(tt.input)
			if result != tt.expected {
				t.Errorf("expected '%s', got '%s'", tt.expected, result)
			}
		})
	}
}

// TestModelKey tests model key generation
func TestModelKey(t *testing.T) {
	tests := []struct {
		name        string
		provider    string
		model       string
		expectedKey string
	}{
		{
			name:        "standard case",
			provider:    "anthropic",
			model:       "claude-3-5",
			expectedKey: "anthropic/claude-3-5",
		},
		{
			name:        "provider normalization",
			provider:    "Claude",
			model:       "claude-3-5",
			expectedKey: "anthropic/claude-3-5",
		},
		{
			name:        "model lowercase",
			provider:    "openai",
			model:       "GPT-4-Turbo",
			expectedKey: "openai/gpt-4-turbo",
		},
		{
			name:        "both normalization",
			provider:    "  GLM  ",
			model:       "  MODEL-4  ",
			expectedKey: "zhipu/model-4",
		},
		{
			name:        "z.ai alias",
			provider:    "z.ai",
			model:       "model",
			expectedKey: "zai/model",
		},
		{
			name:        "gpt alias",
			provider:    "gpt",
			model:       "model",
			expectedKey: "openai/model",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := ModelKey(tt.provider, tt.model)
			if result != tt.expectedKey {
				t.Errorf("expected key '%s', got '%s'", tt.expectedKey, result)
			}
		})
	}
}

// TestModelKey_Consistency tests that ModelKey produces consistent results
func TestModelKey_Consistency(t *testing.T) {
	// Different inputs that should produce the same key
	inputs := []struct {
		provider string
		model    string
	}{
		{"anthropic", "claude-3-5"},
		{"Anthropic", "claude-3-5"},
		{"  anthropic  ", "  claude-3-5  "},
		{"claude", "claude-3-5"},
	}

	var firstKey string
	for i, input := range inputs {
		key := ModelKey(input.provider, input.model)
		if i == 0 {
			firstKey = key
		} else if key != firstKey {
			t.Errorf("inconsistent keys: '%s' vs '%s'", firstKey, key)
		}
	}
}

// TestModelKey_Deduplication tests that ModelKey enables deduplication
func TestModelKey_Deduplication(t *testing.T) {
	// These should all produce unique keys after normalization
	refs := []struct {
		provider string
		model    string
	}{
		{"anthropic", "claude-3-5"},
		{"openai", "gpt-4"},
		{"gemini", "gemini-pro"},
		{"claude", "sonnet-4"}, // This normalizes to anthropic but different model
		{"gpt", "gpt-4-turbo"}, // This normalizes to openai but different model
	}

	seen := make(map[string]bool)
	for _, ref := range refs {
		key := ModelKey(ref.provider, ref.model)
		if seen[key] {
			t.Errorf("duplicate key detected: %s", key)
		}
		seen[key] = true
	}

	// Should have 5 unique keys
	expectedCount := 5
	if len(seen) != expectedCount {
		t.Errorf("expected %d unique keys, got %d", expectedCount, len(seen))
	}
}

// TestModelRef_Structure tests ModelRef structure
func TestModelRef_Structure(t *testing.T) {
	ref := &ModelRef{
		Provider: "anthropic",
		Model:    "claude-3-5",
	}

	if ref.Provider != "anthropic" {
		t.Errorf("expected provider 'anthropic', got '%s'", ref.Provider)
	}

	if ref.Model != "claude-3-5" {
		t.Errorf("expected model 'claude-3-5', got '%s'", ref.Model)
	}
}

// TestParseModelRef_PreservesModelCase tests that model name case is preserved
func TestParseModelRef_PreservesModelCase(t *testing.T) {
	tests := []struct {
		input         string
		expectedModel string
	}{
		{"anthropic/Claude-3.5-Sonnet", "Claude-3.5-Sonnet"},
		{"openai/GPT-4-Turbo", "GPT-4-Turbo"},
		{"google/GEMINI-2.0-Flash", "GEMINI-2.0-Flash"},
	}

	for _, tt := range tests {
		t.Run(tt.input, func(t *testing.T) {
			result := ParseModelRef(tt.input, "openai")
			if result == nil {
				t.Fatal("expected non-nil result")
			}

			if result.Model != tt.expectedModel {
				t.Errorf("expected model '%s', got '%s'", tt.expectedModel, result.Model)
			}
		})
	}
}

// TestParseModelRef_SpecialCharacters tests model names with special characters
func TestParseModelRef_SpecialCharacters(t *testing.T) {
	tests := []struct {
		name          string
		input         string
		expectedModel string
	}{
		{
			name:          "dots in model name",
			input:         "anthropic/claude-3.5.sonnet",
			expectedModel: "claude-3.5.sonnet",
		},
		{
			name:          "underscores",
			input:         "openai/gpt_4_turbo",
			expectedModel: "gpt_4_turbo",
		},
		{
			name:          "mixed separators",
			input:         "google/gemini-2.0.flash-exp",
			expectedModel: "gemini-2.0.flash-exp",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := ParseModelRef(tt.input, "openai")
			if result == nil {
				t.Fatal("expected non-nil result")
			}

			if result.Model != tt.expectedModel {
				t.Errorf("expected model '%s', got '%s'", tt.expectedModel, result.Model)
			}
		})
	}
}

// TestNormalizeProvider_AllAliases tests all provider aliases
func TestNormalizeProvider_AllAliases(t *testing.T) {
	aliasTests := []struct {
		alias      string
		normalized string
	}{
		// z.ai aliases
		{"z.ai", "zai"},
		{"z-ai", "zai"},
		{"Z.AI", "zai"},
		{"Z-AI", "zai"},
		// opencode-zen aliases
		{"opencode-zen", "opencode"},
		{"OpenCode-Zen", "opencode"},
		// qwen aliases
		{"qwen", "qwen-portal"},
		{"Qwen", "qwen-portal"},
		// kimi-code aliases
		{"kimi-code", "kimi-coding"},
		{"Kimi-Code", "kimi-coding"},
		// gpt aliases
		{"gpt", "openai"},
		{"GPT", "openai"},
		// claude aliases
		{"claude", "anthropic"},
		{"Claude", "anthropic"},
		// glm aliases
		{"glm", "zhipu"},
		{"GLM", "zhipu"},
		// google aliases
		{"google", "gemini"},
		{"Google", "gemini"},
	}

	for _, tt := range aliasTests {
		t.Run(tt.alias, func(t *testing.T) {
			result := NormalizeProvider(tt.alias)
			if result != tt.normalized {
				t.Errorf("alias '%s': expected '%s', got '%s'", tt.alias, tt.normalized, result)
			}
		})
	}
}

// TestModelKey_EmptyStrings tests edge cases with empty/whitespace strings
func TestModelKey_EmptyStrings(t *testing.T) {
	tests := []struct {
		name        string
		provider    string
		model       string
		expectedKey string
	}{
		{
			name:        "empty model",
			provider:    "anthropic",
			model:       "",
			expectedKey: "anthropic/",
		},
		{
			name:        "whitespace model",
			provider:    "openai",
			model:       "   ",
			expectedKey: "openai/",
		},
		{
			name:        "whitespace provider",
			provider:    "   ",
			model:       "model",
			expectedKey: "/model",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := ModelKey(tt.provider, tt.model)
			if result != tt.expectedKey {
				t.Errorf("expected key '%s', got '%s'", tt.expectedKey, result)
			}
		})
	}
}

// TestParseModelRef_LongModelNames tests parsing long model names
func TestParseModelRef_LongModelNames(t *testing.T) {
	longModelName := "very-long-model-name-with-lots-of-dashes-and-numbers-12345"
	input := "anthropic/" + longModelName

	result := ParseModelRef(input, "openai")
	if result == nil {
		t.Fatal("expected non-nil result")
	}

	if result.Model != longModelName {
		t.Errorf("expected model '%s', got '%s'", longModelName, result.Model)
	}

	if result.Provider != "anthropic" {
		t.Errorf("expected provider 'anthropic', got '%s'", result.Provider)
	}
}

// TestNormalizeProvider_UnknownProvider tests unknown provider names
func TestNormalizeProvider_UnknownProvider(t *testing.T) {
	unknownProviders := []string{
		"unknown-provider",
		"custom-provider",
		"test-provider",
		"some-random-name",
	}

	for _, provider := range unknownProviders {
		t.Run(provider, func(t *testing.T) {
			result := NormalizeProvider(provider)
			// Unknown providers should be lowercased but not otherwise changed
			expected := strings.ToLower(strings.TrimSpace(provider))
			if result != expected {
				t.Errorf("expected '%s', got '%s'", expected, result)
			}
		})
	}
}

// TestModelRef_LowercaseProvider tests that provider is normalized to lowercase
func TestModelRef_LowercaseProvider(t *testing.T) {
	tests := []struct {
		input            string
		expectedProvider string
	}{
		{"Anthropic/Model", "anthropic"},
		{"OPENAI/Model", "openai"},
		{"Google/Model", "gemini"},
		{"GPT/Model", "openai"},
	}

	for _, tt := range tests {
		t.Run(tt.input, func(t *testing.T) {
			result := ParseModelRef(tt.input, "openai")
			if result == nil {
				t.Fatal("expected non-nil result")
			}

			if result.Provider != tt.expectedProvider {
				t.Errorf("expected provider '%s', got '%s'", tt.expectedProvider, result.Provider)
			}
		})
	}
}
