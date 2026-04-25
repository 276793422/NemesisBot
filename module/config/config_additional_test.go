// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package config

import (
	"os"
	"testing"
)

// TestInferProviderFromModel_AllCases tests all provider inference paths.
func TestInferProviderFromModel_AllCases(t *testing.T) {
	tests := []struct {
		model    string
		expected string
	}{
		{"claude-3-opus", "anthropic"},
		{"claude-sonnet-4", "anthropic"},
		{"gpt-4o", "openai"},
		{"gpt-3.5-turbo", "openai"},
		{"gemini-pro", "gemini"},
		{"glm-4", "zhipu"},
		{"zhipu-model", "zhipu"},
		{"groq-llama", "groq"},
		{"llama-3.3", "ollama"},
		{"moonshot-v1", "moonshot"},
		{"kimi-latest", "moonshot"},
		{"nvidia-nemotron", "nvidia"},
		{"deepseek-chat", "deepseek"},
		{"mistral-large", "mistral"},
		{"mixtral-8x7b", "mistral"},
		{"codestral-latest", "mistral"},
		{"command-r-plus", "cohere"},
		{"cohere-embed", "cohere"},
		{"sonar-large", "perplexity"},
		{"perplexity-model", "perplexity"},
		{"unknown-model", ""},
		{"", ""},
	}

	for _, tt := range tests {
		result := inferProviderFromModel(tt.model)
		if result != tt.expected {
			t.Errorf("inferProviderFromModel(%q) = %q, want %q", tt.model, result, tt.expected)
		}
	}
}

// TestInferDefaultModel_AllCases tests all default model inference paths.
func TestInferDefaultModel_AllCases(t *testing.T) {
	tests := []struct {
		provider string
		expected string
	}{
		{"anthropic", "claude-sonnet-4-20250514"},
		{"claude", "claude-sonnet-4-20250514"},
		{"openai", "gpt-4o"},
		{"gpt", "gpt-4o"},
		{"zhipu", "glm-4.7-flash"},
		{"glm", "glm-4.7-flash"},
		{"groq", "llama-3.3-70b-versatile"},
		{"ollama", "llama3.3"},
		{"gemini", "gemini-2.0-flash-exp"},
		{"google", "gemini-2.0-flash-exp"},
		{"nvidia", "nvidia/llama-3.1-nemotron-70b-instruct"},
		{"moonshot", "moonshot-v1-8k"},
		{"kimi", "moonshot-v1-8k"},
		{"deepseek", "deepseek-chat"},
		{"mistral", "mistral-large-latest"},
		{"cohere", "command-r-plus"},
		{"perplexity", "sonar"},
		{"together", "meta-llama/Llama-3.3-70B-Instruct-Turbo"},
		{"fireworks", "accounts/fireworks/models/llama-v3p3-70b-instruct"},
		{"cerebras", "llama-3.3-70b"},
		{"sambanova", "Meta-Llama-3.3-70B-Instruct"},
		{"unknown", ""},
	}

	for _, tt := range tests {
		result := inferDefaultModel(tt.provider)
		if result != tt.expected {
			t.Errorf("inferDefaultModel(%q) = %q, want %q", tt.provider, result, tt.expected)
		}
	}
}

// TestGetDefaultAPIBase_AllCases tests all API base resolution paths.
func TestGetDefaultAPIBase_AllCases(t *testing.T) {
	tests := []struct {
		provider string
		expected string
	}{
		{"anthropic", "https://api.anthropic.com/v1"},
		{"claude", "https://api.anthropic.com/v1"},
		{"openai", "https://api.openai.com/v1"},
		{"gpt", "https://api.openai.com/v1"},
		{"openrouter", "https://openrouter.ai/api/v1"},
		{"groq", "https://api.groq.com/openai/v1"},
		{"zhipu", "https://open.bigmodel.cn/api/paas/v4"},
		{"glm", "https://open.bigmodel.cn/api/paas/v4"},
		{"gemini", "https://generativelanguage.googleapis.com/v1beta"},
		{"google", "https://generativelanguage.googleapis.com/v1beta"},
		{"nvidia", "https://integrate.api.nvidia.com/v1"},
		{"ollama", "http://localhost:11434/v1"},
		{"moonshot", "https://api.moonshot.cn/v1"},
		{"kimi", "https://api.moonshot.cn/v1"},
		{"deepseek", "https://api.deepseek.com/v1"},
		{"mistral", "https://api.mistral.ai/v1"},
		{"cohere", "https://api.cohere.ai/v2"},
		{"perplexity", "https://api.perplexity.ai/v1"},
		{"together", "https://api.together.xyz/v1"},
		{"fireworks", "https://api.fireworks.ai/inference/v1"},
		{"cerebras", "https://api.cerebras.ai/v1"},
		{"sambanova", "https://api.sambanova.ai/v1"},
		{"github_copilot", "localhost:4321"},
		{"unknown", ""},
	}

	for _, tt := range tests {
		result := getDefaultAPIBase(tt.provider)
		if result != tt.expected {
			t.Errorf("getDefaultAPIBase(%q) = %q, want %q", tt.provider, result, tt.expected)
		}
	}
}

// TestGetPlatformSecurityConfigFilename_Explicit tests platform filename function.
func TestGetPlatformSecurityConfigFilename_Explicit(t *testing.T) {
	result := GetPlatformSecurityConfigFilename()
	if result == "" {
		t.Error("expected non-empty platform security config filename")
	}
}

// TestGetPlatformDisplayName_Explicit tests platform display name function.
func TestGetPlatformDisplayName_Explicit(t *testing.T) {
	result := GetPlatformDisplayName()
	if result == "" {
		t.Error("expected non-empty platform display name")
	}
}

// TestSaveConfig_Success tests SaveConfig creates a file.
func TestSaveConfig_Success(t *testing.T) {
	cfg := DefaultConfig()
	dir := t.TempDir()
	configPath := dir + "/config.json"
	err := SaveConfig(configPath, cfg)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if _, statErr := os.Stat(configPath); statErr != nil {
		t.Errorf("expected config file to exist: %v", statErr)
	}
}
