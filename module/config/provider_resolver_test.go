// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package config

import (
	"testing"
)

func TestResolveModelConfig(t *testing.T) {
	cfg := &Config{
		ModelList: []ModelConfig{
			{
				ModelName: "gpt4",
				Model:     "openai/gpt-4",
				APIKey:    "key1",
				APIBase:   "https://api.openai.com/v1",
			},
			{
				ModelName: "claude-sonnet",
				Model:     "anthropic/claude-3-sonnet",
				APIKey:    "key2",
			},
			{
				ModelName: "glm-flash",
				Model:     "zhipu/glm-4.7-flash",
				APIKey:    "key3",
			},
		},
	}

	tests := []struct {
		name           string
		modelRef       string
		wantErr        bool
		wantProvider   string
		wantModel      string
		wantAPIKey     string
		wantAPIBase    string
	}{
		{
			name:         "Find by model_name",
			modelRef:     "gpt4",
			wantErr:      false,
			wantProvider: "openai",
			wantModel:    "gpt-4",
			wantAPIKey:   "key1",
			wantAPIBase:  "https://api.openai.com/v1",
		},
		{
			name:         "Find by model field",
			modelRef:     "anthropic/claude-3-sonnet",
			wantErr:      false,
			wantProvider: "anthropic",
			wantModel:    "claude-3-sonnet",
			wantAPIKey:   "key2",
		},
		{
			name:         "Find by model_name with vendor prefix",
			modelRef:     "glm-flash",
			wantErr:      false,
			wantProvider: "zhipu",
			wantModel:    "glm-4.7-flash",
			wantAPIKey:   "key3",
		},
		{
			name:     "Not found",
			modelRef: "nonexistent",
			wantErr:  true,
		},
		{
			name:     "Empty model reference",
			modelRef: "",
			wantErr:  true,
		},
		{
			name:     "Nil config",
			modelRef: "gpt4",
			wantErr:  true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			var cfgToUse *Config
			if tt.name == "Nil config" {
				cfgToUse = nil
			} else {
				cfgToUse = cfg
			}

			got, err := ResolveModelConfig(cfgToUse, tt.modelRef)
			if (err != nil) != tt.wantErr {
				t.Errorf("ResolveModelConfig() error = %v, wantErr %v", err, tt.wantErr)
				return
			}
			if !tt.wantErr {
				if got.ProviderName != tt.wantProvider {
					t.Errorf("ProviderName = %v, want %v", got.ProviderName, tt.wantProvider)
				}
				if got.ModelName != tt.wantModel {
					t.Errorf("ModelName = %v, want %v", got.ModelName, tt.wantModel)
				}
				if got.APIKey != tt.wantAPIKey {
					t.Errorf("APIKey = %v, want %v", got.APIKey, tt.wantAPIKey)
				}
				if tt.wantAPIBase != "" && got.APIBase != tt.wantAPIBase {
					t.Errorf("APIBase = %v, want %v", got.APIBase, tt.wantAPIBase)
				}
			}
		})
	}
}

func TestResolveModelConfig_InferProvider(t *testing.T) {
	cfg := &Config{
		ModelList: []ModelConfig{},
	}

	tests := []struct {
		name         string
		modelRef     string
		wantProvider string
		wantErr      bool
	}{
		{
			name:         "Infer claude provider",
			modelRef:     "claude-3-opus",
			wantProvider: "anthropic",
			wantErr:      false,
		},
		{
			name:         "Infer gpt provider",
			modelRef:     "gpt-4-turbo",
			wantProvider: "openai",
			wantErr:      false,
		},
		{
			name:         "Infer gemini provider",
			modelRef:     "gemini-pro",
			wantProvider: "gemini",
			wantErr:      false,
		},
		{
			name:         "Infer glm/zhipu provider",
			modelRef:     "glm-4",
			wantProvider: "zhipu",
			wantErr:      false,
		},
		{
			name:         "Infer groq provider",
			modelRef:     "llama-3.3-70b",
			wantProvider: "ollama",
			wantErr:      false,
		},
		{
			name:         "Infer deepseek provider",
			modelRef:     "deepseek-chat",
			wantProvider: "deepseek",
			wantErr:      false,
		},
		{
			name:         "Unknown provider",
			modelRef:     "unknown-model-x",
			wantProvider: "",
			wantErr:      true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got, err := ResolveModelConfig(cfg, tt.modelRef)
			if (err != nil) != tt.wantErr {
				t.Errorf("ResolveModelConfig() error = %v, wantErr %v", err, tt.wantErr)
				return
			}
			if !tt.wantErr && got.ProviderName != tt.wantProvider {
				t.Errorf("ProviderName = %v, want %v", got.ProviderName, tt.wantProvider)
			}
		})
	}
}

func TestGetModelByName(t *testing.T) {
	cfg := &Config{
		ModelList: []ModelConfig{
			{
				ModelName: "gpt4",
				Model:     "openai/gpt-4",
				APIKey:    "key1",
			},
			{
				ModelName: "gpt4-duplicate",
				Model:     "openai/gpt-4",
				APIKey:    "key2",
			},
			{
				ModelName: "claude",
				Model:     "anthropic/claude-3",
				APIKey:    "key3",
			},
		},
	}

	tests := []struct {
		name      string
		modelRef  string
		wantErr   bool
		modelName string
	}{
		{
			name:      "Single match",
			modelRef:  "claude",
			wantErr:   false,
			modelName: "claude",
		},
		{
			name:      "Multiple matches - round robin",
			modelRef:  "gpt4",
			wantErr:   false,
			modelName: "gpt4",
		},
		{
			name:     "Not found",
			modelRef: "nonexistent",
			wantErr:  true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got, err := GetModelByName(cfg, tt.modelRef)
			if (err != nil) != tt.wantErr {
				t.Errorf("GetModelByName() error = %v, wantErr %v", err, tt.wantErr)
				return
			}
			if !tt.wantErr && got.ModelName != tt.modelName {
				t.Errorf("ModelName = %v, want %v", got.ModelName, tt.modelName)
			}
		})
	}
}

func TestGetModelByName_RoundRobin(t *testing.T) {
	cfg := &Config{
		ModelList: []ModelConfig{
			{
				ModelName: "model1",
				Model:     "openai/gpt-4",
				APIKey:    "key1",
			},
			{
				ModelName: "model2",
				Model:     "openai/gpt-4",
				APIKey:    "key2",
			},
			{
				ModelName: "model3",
				Model:     "openai/gpt-4",
				APIKey:    "key3",
			},
		},
	}

	// Test round-robin distribution
	results := make(map[string]int)
	iterations := 30

	for i := 0; i < iterations; i++ {
		model, err := GetModelByName(cfg, "openai/gpt-4")
		if err != nil {
			t.Fatalf("GetModelByName() error = %v", err)
		}
		results[model.ModelName]++
	}

	// Check that all models were used
	if len(results) != 3 {
		t.Errorf("Round-robin didn't distribute evenly, got %d unique models", len(results))
	}

	// Check distribution is roughly even (at least 5 times each for 30 iterations)
	for modelName, count := range results {
		if count < 5 {
			t.Errorf("Model %s was only selected %d times, expected at least 5", modelName, count)
		}
	}
}

func TestGetEffectiveLLM(t *testing.T) {
	tests := []struct {
		name     string
		config   *Config
		wantLLM  string
	}{
		{
			name:    "Nil config",
			config:  nil,
			wantLLM: "zhipu/glm-4.7-flash",
		},
		{
			name: "Config with LLM set",
			config: &Config{
				Agents: AgentsConfig{
					Defaults: AgentDefaults{
						LLM: "openai/gpt-4",
					},
				},
			},
			wantLLM: "openai/gpt-4",
		},
		{
			name: "Config with empty LLM",
			config: &Config{
				Agents: AgentsConfig{
					Defaults: AgentDefaults{
						LLM: "",
					},
				},
			},
			wantLLM: "zhipu/glm-4.7-flash",
		},
		{
			name: "Config with zero agents",
			config: &Config{},
			wantLLM: "zhipu/glm-4.7-flash",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := GetEffectiveLLM(tt.config)
			if got != tt.wantLLM {
				t.Errorf("GetEffectiveLLM() = %v, want %v", got, tt.wantLLM)
			}
		})
	}
}

func TestProviderResolution(t *testing.T) {
	resolution := &ProviderResolution{
		ProviderName: "openai",
		ModelName:    "gpt-4",
		APIKey:       "test-key",
		APIBase:      "https://api.openai.com/v1",
		Proxy:        "http://proxy.example.com",
		AuthMethod:   "token",
		ConnectMode:  "stdio",
		Workspace:    "/tmp/workspace",
		Enabled:      true,
	}

	if resolution.ProviderName == "" {
		t.Error("ProviderName should not be empty")
	}
	if resolution.ModelName == "" {
		t.Error("ModelName should not be empty")
	}
	if resolution.APIKey == "" {
		t.Error("APIKey should not be empty")
	}
	if !resolution.Enabled {
		t.Error("Enabled should be true")
	}
}

func TestModelResolution(t *testing.T) {
	resolution := &ModelResolution{
		Primary:   "gpt-4",
		Fallbacks: []string{"claude-haiku", "gemini-flash"},
	}

	if resolution.Primary == "" {
		t.Error("Primary should not be empty")
	}
	if len(resolution.Fallbacks) == 0 {
		t.Error("Fallbacks should not be empty")
	}
}

func TestResolveModelConfig_WithWhitespace(t *testing.T) {
	cfg := &Config{
		ModelList: []ModelConfig{
			{
				ModelName: "gpt4",
				Model:     "openai/gpt-4",
				APIKey:    "key1",
			},
		},
	}

	tests := []struct {
		name     string
		modelRef string
		wantErr  bool
	}{
		{
			name:     "Leading whitespace",
			modelRef: "  gpt4",
			wantErr:  false,
		},
		{
			name:     "Trailing whitespace",
			modelRef: "gpt4  ",
			wantErr:  false,
		},
		{
			name:     "Both leading and trailing whitespace",
			modelRef: "  gpt4  ",
			wantErr:  false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got, err := ResolveModelConfig(cfg, tt.modelRef)
			if (err != nil) != tt.wantErr {
				t.Errorf("ResolveModelConfig() error = %v, wantErr %v", err, tt.wantErr)
				return
			}
			if !tt.wantErr && got.ModelName != "gpt-4" {
				t.Errorf("ModelName = %v, want gpt-4", got.ModelName)
			}
		})
	}
}

func TestResolveModelConfig_WithoutVendorPrefix(t *testing.T) {
	cfg := &Config{
		ModelList: []ModelConfig{
			{
				ModelName: "claude",
				Model:     "claude-3-sonnet", // No vendor prefix
				APIKey:    "key1",
			},
		},
	}

	got, err := ResolveModelConfig(cfg, "claude")
	if err != nil {
		t.Fatalf("ResolveModelConfig() error = %v", err)
	}

	// Should infer provider from model name
	if got.ProviderName != "anthropic" {
		t.Errorf("ProviderName = %v, want anthropic", got.ProviderName)
	}
}

func TestConcurrentModelResolution(t *testing.T) {
	cfg := &Config{
		ModelList: []ModelConfig{
			{
				ModelName: "model1",
				Model:     "openai/gpt-4",
				APIKey:    "key1",
			},
			{
				ModelName: "model2",
				Model:     "openai/gpt-4",
				APIKey:    "key2",
			},
		},
	}

	done := make(chan bool)
	for i := 0; i < 50; i++ {
		go func() {
			_, _ = GetModelByName(cfg, "openai/gpt-4")
			done <- true
		}()
	}

	for i := 0; i < 50; i++ {
		<-done
	}
}
