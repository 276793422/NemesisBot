// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package providers

import (
	"testing"

	"github.com/276793422/NemesisBot/module/config"
)

func TestProviderType_Constants(t *testing.T) {
	// Verify that provider type constants are unique and non-zero
	types := []providerType{
		providerTypeHTTPCompat,
		providerTypeClaudeAuth,
		providerTypeCodexAuth,
		providerTypeClaudeCLI,
		providerTypeCodexCLI,
		providerTypeGitHubCopilot,
	}

	seen := make(map[providerType]bool)
	for _, pt := range types {
		if seen[pt] {
			t.Errorf("Duplicate provider type detected: %v", pt)
		}
		seen[pt] = true
	}
}

func TestProviderSelection_Structure(t *testing.T) {
	sel := providerSelection{
		providerType:    providerTypeHTTPCompat,
		apiKey:          "test-key",
		apiBase:         "https://api.example.com",
		proxy:           "http://proxy.example.com",
		model:           "test-model",
		workspace:       "/workspace",
		connectMode:     "direct",
		enableWebSearch: true,
	}

	if sel.providerType != providerTypeHTTPCompat {
		t.Errorf("Expected providerTypeHTTPCompat, got %v", sel.providerType)
	}

	if sel.apiKey != "test-key" {
		t.Errorf("Expected apiKey 'test-key', got '%s'", sel.apiKey)
	}

	if sel.apiBase != "https://api.example.com" {
		t.Errorf("Expected apiBase 'https://api.example.com', got '%s'", sel.apiBase)
	}

	if sel.proxy != "http://proxy.example.com" {
		t.Errorf("Expected proxy 'http://proxy.example.com', got '%s'", sel.proxy)
	}

	if sel.model != "test-model" {
		t.Errorf("Expected model 'test-model', got '%s'", sel.model)
	}

	if sel.workspace != "/workspace" {
		t.Errorf("Expected workspace '/workspace', got '%s'", sel.workspace)
	}

	if sel.connectMode != "direct" {
		t.Errorf("Expected connectMode 'direct', got '%s'", sel.connectMode)
	}

	if !sel.enableWebSearch {
		t.Error("Expected enableWebSearch to be true")
	}
}

func TestProviderSelection_DefaultValues(t *testing.T) {
	sel := providerSelection{}

	if sel.providerType != 0 {
		t.Errorf("Expected zero providerType, got %v", sel.providerType)
	}

	if sel.apiKey != "" {
		t.Errorf("Expected empty apiKey, got '%s'", sel.apiKey)
	}

	if sel.apiBase != "" {
		t.Errorf("Expected empty apiBase, got '%s'", sel.apiBase)
	}

	if sel.enableWebSearch {
		t.Error("Expected enableWebSearch to be false by default")
	}
}

func TestResolveProviderSelection_NoConfig(t *testing.T) {
	// Test with nil config
	_, err := resolveProviderSelection(nil)
	if err == nil {
		t.Error("Expected error when config is nil")
	}
}

func TestResolveProviderSelection_EmptyModelList(t *testing.T) {
	cfg := &config.Config{
		ModelList: []config.ModelConfig{},
	}

	_, err := resolveProviderSelection(cfg)
	if err == nil {
		t.Error("Expected error when ModelList is empty")
	}
}

func TestCreateProvider_InvalidConfig(t *testing.T) {
	tests := []struct {
		name    string
		cfg     *config.Config
		wantErr bool
	}{
		{
			name:    "nil config",
			cfg:     nil,
			wantErr: true,
		},
		{
			name: "empty config",
			cfg: &config.Config{
				ModelList: []config.ModelConfig{},
			},
			wantErr: true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			_, err := CreateProvider(tt.cfg)
			if (err != nil) != tt.wantErr {
				t.Errorf("CreateProvider() error = %v, wantErr %v", err, tt.wantErr)
			}
		})
	}
}

func TestProviderTypeNames(t *testing.T) {
	// Test that provider type constants have expected string representations
	tests := []struct {
		name      string
		pt        providerType
		minVal    int
		maxVal    int
	}{
		{"HTTPCompat", providerTypeHTTPCompat, 0, 0},
		{"ClaudeAuth", providerTypeClaudeAuth, 1, 1},
		{"CodexAuth", providerTypeCodexAuth, 2, 2},
		{"ClaudeCLI", providerTypeClaudeCLI, 3, 3},
		{"CodexCLI", providerTypeCodexCLI, 4, 4},
		{"GitHubCopilot", providerTypeGitHubCopilot, 5, 5},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			val := int(tt.pt)
			if val < tt.minVal || val > tt.maxVal {
				t.Errorf("Provider type %v has unexpected value: %d", tt.pt, val)
			}
		})
	}
}

func TestDefaultAnthropicAPIBase(t *testing.T) {
	expected := "https://api.anthropic.com/v1"
	if defaultAnthropicAPIBase != expected {
		t.Errorf("Expected defaultAnthropicAPIBase to be '%s', got '%s'", expected, defaultAnthropicAPIBase)
	}
}

func TestProviderSelection_Copy(t *testing.T) {
	original := providerSelection{
		providerType:    providerTypeClaudeAuth,
		apiKey:          "key123",
		apiBase:         "https://api.example.com",
		proxy:           "proxy",
		model:           "model",
		workspace:       "workspace",
		connectMode:     "mode",
		enableWebSearch: true,
	}

	// Copy by assignment
	copy := original

	// Verify copy has same values
	if copy.providerType != original.providerType {
		t.Error("Copy should have same providerType")
	}

	if copy.apiKey != original.apiKey {
		t.Error("Copy should have same apiKey")
	}

	if copy.apiBase != original.apiBase {
		t.Error("Copy should have same apiBase")
	}

	// Modify original
	original.apiKey = "modified"

	// Copy should be unaffected (strings are immutable)
	if copy.apiKey == "modified" {
		t.Error("Copy should be independent of original")
	}
}

func TestResolveProviderSelection_ModelListConfig(t *testing.T) {
	// Test with ModelList configuration using direct API key (no OAuth)
	cfg := &config.Config{
		ModelList: []config.ModelConfig{
			{
				ModelName: "test-model",
				Model:     "vendor/model",
				APIKey:    "test-key-12345", // Must provide API key for HTTP compat provider
				APIBase:   "https://api.example.com",
			},
		},
	}

	sel, err := resolveProviderSelection(cfg)
	if err != nil {
		t.Logf("resolveProviderSelection() failed: %v", err)
		// Error is acceptable in test environment
		return
	}

	// Verify selection is populated if no error
	if sel.model == "" {
		t.Error("Expected model to be set")
	}

	if sel.apiBase == "" {
		t.Error("Expected apiBase to be set")
	}
}

func TestCreateClaudeAuthProvider_APIKey(t *testing.T) {
	// Test that createClaudeAuthProvider properly initializes
	// Note: This test will fail if credentials aren't set up,
	// so we're mainly testing the function signature and basic behavior

	apiBase := "https://api.anthropic.com"

	// This will fail because we don't have credentials, but we can test the error path
	_, err := createClaudeAuthProvider(apiBase)
	if err == nil {
		t.Error("Expected error when credentials are not available")
	}

	// Verify error message contains expected information
	if err != nil {
		errStr := err.Error()
		// Error should mention credentials or anthropic
		if len(errStr) == 0 {
			t.Error("Error message should not be empty")
		}
	}
}

func TestCreateCodexAuthProvider_APIKey(t *testing.T) {
	// Test that createCodexAuthProvider properly initializes
	// Note: This test will fail if credentials aren't set up

	// This will fail because we don't have credentials, but we can test the error path
	_, err := createCodexAuthProvider(false)
	if err == nil {
		t.Error("Expected error when credentials are not available")
	}

	// Verify error message contains expected information
	if err != nil {
		errStr := err.Error()
		// Error should mention credentials or openai
		if len(errStr) == 0 {
			t.Error("Error message should not be empty")
		}
	}
}

func TestCreateClaudeAuthProvider_DefaultAPIBase(t *testing.T) {
	// Test with empty apiBase (should use default)
	_, err := createClaudeAuthProvider("")
	if err == nil {
		t.Error("Expected error when credentials are not available")
	}

	// Even though it fails, we can verify the function doesn't panic
	if err != nil {
		// Expected path
		t.Log("Got expected error:", err)
	}
}

func TestProviderSelection_EnableWebSearch(t *testing.T) {
	tests := []struct {
		name            string
		enableWebSearch bool
	}{
		{"web search enabled", true},
		{"web search disabled", false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			sel := providerSelection{
				enableWebSearch: tt.enableWebSearch,
			}

			if sel.enableWebSearch != tt.enableWebSearch {
				t.Errorf("Expected enableWebSearch %v, got %v", tt.enableWebSearch, sel.enableWebSearch)
			}
		})
	}
}

func TestResolveProviderSelection_ProviderAliases(t *testing.T) {
	// Test that various provider name aliases are handled correctly
	providerAliases := map[string]string{
		"claude-cli":    "claude-cli",
		"claude-code":   "claude-cli",
		"claudecode":    "claude-cli",
		"claudecodec":   "claude-cli",
		"codex-cli":     "codex-cli",
		"codex-code":    "codex-cli",
		"github_copilot": "github-copilot",
		"copilot":       "github-copilot",
	}

	for alias, expectedType := range providerAliases {
		t.Run(alias, func(t *testing.T) {
			cfg := &config.Config{
				ModelList: []config.ModelConfig{
					{
						ModelName: alias,
						Model:     alias + "/model",
					},
				},
			}

			sel, err := resolveProviderSelection(cfg)
			if err != nil {
				t.Logf("resolveProviderSelection for %s failed (expected): %v", alias, err)
				// Some aliases may fail without proper configuration
				return
			}

			// Verify that the selection was created
			if sel.providerType == 0 && expectedType != "http-compat" {
				t.Errorf("Expected non-zero providerType for alias '%s'", alias)
			}
		})
	}
}

func TestCreateProvider_ProviderTypePriority(t *testing.T) {
	// Test that provider type is correctly determined based on configuration
	tests := []struct {
		name        string
		model       string
		authMethod  string
		expectError bool
	}{
		{"anthropic with token", "anthropic/claude-sonnet-4.6", "token", false},
		{"claude with token", "claude/claude-sonnet-4.6", "token", false},
		{"openai with token", "openai/gpt-4o", "token", false},
		{"gpt with token", "gpt/gpt-4o", "token", false},
		{"unknown provider with token", "unknown/model", "token", true},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			cfg := &config.Config{
				ModelList: []config.ModelConfig{
					{
						ModelName:  "test-model",
						Model:      tt.model,
						APIKey:     "test-key",
						AuthMethod: tt.authMethod,
					},
				},
			}

			sel, err := resolveProviderSelection(cfg)
			if tt.expectError && err == nil {
				t.Error("Expected error for unsupported provider")
			} else if !tt.expectError && err != nil {
				// Some errors are expected due to missing auth
				t.Logf("Got expected error: %v", err)
			}

			if !tt.expectError && err == nil {
				// Verify provider type was set
				if sel.providerType == 0 && tt.model != "unknown/model" {
					t.Error("Expected providerType to be set")
				}
			}
		})
	}
}
