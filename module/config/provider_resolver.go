// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package config

import (
	"fmt"
	"strings"
	"sync/atomic"
)

// rrCounter is a global counter for round-robin load balancing across models.
var rrCounter atomic.Uint64

// ProviderResolution represents the resolved provider and model configuration from ModelList
type ProviderResolution struct {
	ProviderName string // e.g., "zhipu", "openrouter"
	ModelName    string // e.g., "glm-4.7-flash"
	APIKey       string
	APIBase      string
	Proxy        string
	AuthMethod   string // "oauth" or "token"
	ConnectMode  string // for special providers like github_copilot
	Workspace    string // for CLI-based providers
	Enabled      bool
}

// ModelResolution represents the resolved model with its fallbacks
type ModelResolution struct {
	Primary   string
	Fallbacks []string
}

// ResolveModelConfig resolves a model reference from ModelList.
// modelRef can be:
//   - "model_name" (searches by ModelConfig.ModelName)
//   - "vendor/model" (searches by ModelConfig.Model)
// Returns ProviderResolution with all configuration needed to make API calls.
func ResolveModelConfig(cfg *Config, modelRef string) (*ProviderResolution, error) {
	if cfg == nil {
		return nil, fmt.Errorf("config is nil")
	}

	modelRef = strings.TrimSpace(modelRef)
	if modelRef == "" {
		return nil, fmt.Errorf("model reference is empty")
	}

	// First, try to find by model_name (exact match)
	for i := range cfg.ModelList {
		if cfg.ModelList[i].ModelName == modelRef {
			return resolveFromModelConfig(&cfg.ModelList[i]), nil
		}
	}

	// Then, try to find by model field (vendor/model format)
	if strings.Contains(modelRef, "/") {
		// Find matching ModelConfig by model field
		for i := range cfg.ModelList {
			if cfg.ModelList[i].Model == modelRef {
				return resolveFromModelConfig(&cfg.ModelList[i]), nil
			}
		}
	}

	// Not found, try to infer provider from model name
	inferredProvider := inferProviderFromModel(modelRef)
	if inferredProvider != "" {
		// Create a basic resolution with inferred provider
		return &ProviderResolution{
			ProviderName: inferredProvider,
			ModelName:    modelRef,
			APIBase:      getDefaultAPIBase(inferredProvider),
			Enabled:      true,
		}, nil
	}

	return nil, fmt.Errorf("model %q not found in model_list", modelRef)
}

// resolveFromModelConfig converts a ModelConfig to ProviderResolution
func resolveFromModelConfig(mc *ModelConfig) *ProviderResolution {
	// Parse model field to extract vendor
	model := mc.Model
	var providerName string
	var modelName string

	if strings.Contains(model, "/") {
		parts := strings.SplitN(model, "/", 2)
		providerName = strings.ToLower(strings.TrimSpace(parts[0]))
		modelName = strings.TrimSpace(parts[1])
	} else {
		// No vendor prefix, try to infer from model name
		providerName = inferProviderFromModel(model)
		modelName = model
	}

	// Determine API base
	apiBase := mc.APIBase
	if apiBase == "" {
		apiBase = getDefaultAPIBase(providerName)
	}

	return &ProviderResolution{
		ProviderName: providerName,
		ModelName:    modelName,
		APIKey:       mc.APIKey,
		APIBase:      apiBase,
		Proxy:        mc.Proxy,
		AuthMethod:   mc.AuthMethod,
		ConnectMode:  mc.ConnectMode,
		Workspace:    mc.Workspace,
		Enabled:      true,
	}
}

// GetModelByName finds a model configuration by model_name or model field.
// Returns the first matching ModelConfig. For multiple matches, uses round-robin.
func GetModelByName(cfg *Config, modelRef string) (*ModelConfig, error) {
	var matches []ModelConfig

	// Search by model_name
	for i := range cfg.ModelList {
		if cfg.ModelList[i].ModelName == modelRef {
			matches = append(matches, cfg.ModelList[i])
		}
	}

	// If no exact match, try model field
	if len(matches) == 0 {
		for i := range cfg.ModelList {
			if cfg.ModelList[i].Model == modelRef {
				matches = append(matches, cfg.ModelList[i])
			}
		}
	}

	if len(matches) == 0 {
		return nil, fmt.Errorf("model %q not found in model_list", modelRef)
	}

	// Round-robin for load balancing
	if len(matches) == 1 {
		return &matches[0], nil
	}

	idx := rrCounter.Add(1) % uint64(len(matches))
	return &matches[idx], nil
}

// GetEffectiveLLM returns the effective LLM reference for the default agent.
// After migration, only the LLM field is used (old Provider/Model fields are removed).
func GetEffectiveLLM(cfg *Config) string {
	if cfg == nil {
		return "zhipu/glm-4.7-flash" // Ultimate default for nil config
	}

	// Use the new LLM format
	if cfg.Agents.Defaults.LLM != "" {
		return cfg.Agents.Defaults.LLM
	}

	return "zhipu/glm-4.7-flash" // Ultimate default
}

// inferProviderFromModel infers the provider name from the model name
func inferProviderFromModel(model string) string {
	modelLower := strings.ToLower(model)
	switch {
	case strings.Contains(modelLower, "claude"):
		return "anthropic"
	case strings.Contains(modelLower, "gpt"):
		return "openai"
	case strings.Contains(modelLower, "gemini"):
		return "gemini"
	case strings.Contains(modelLower, "glm") || strings.Contains(modelLower, "zhipu"):
		return "zhipu"
	case strings.Contains(modelLower, "groq"):
		return "groq"
	case strings.Contains(modelLower, "llama"):
		return "ollama"
	case strings.Contains(modelLower, "moonshot") || strings.Contains(modelLower, "kimi"):
		return "moonshot"
	case strings.Contains(modelLower, "nvidia"):
		return "nvidia"
	case strings.Contains(modelLower, "deepseek"):
		return "deepseek"
	default:
		return ""
	}
}

// inferDefaultModel returns the default model for a provider
func inferDefaultModel(provider string) string {
	switch provider {
	case "anthropic", "claude":
		return "claude-sonnet-4-20250514"
	case "openai", "gpt":
		return "gpt-4o"
	case "zhipu", "glm":
		return "glm-4.7-flash"
	case "groq":
		return "llama-3.3-70b-versatile"
	case "ollama":
		return "llama3.3"
	case "gemini", "google":
		return "gemini-2.0-flash-exp"
	case "nvidia":
		return "nvidia/llama-3.1-nemotron-70b-instruct"
	case "moonshot", "kimi":
		return "moonshot-v1-8k"
	case "deepseek":
		return "deepseek-chat"
	default:
		return ""
	}
}

// getDefaultAPIBase returns the default API base for a provider
func getDefaultAPIBase(providerName string) string {
	switch providerName {
	case "anthropic", "claude":
		return "https://api.anthropic.com/v1"
	case "openai", "gpt":
		return "https://api.openai.com/v1"
	case "openrouter":
		return "https://openrouter.ai/api/v1"
	case "groq":
		return "https://api.groq.com/openai/v1"
	case "zhipu", "glm":
		return "https://open.bigmodel.cn/api/paas/v4"
	case "gemini", "google":
		return "https://generativelanguage.googleapis.com/v1beta"
	case "nvidia":
		return "https://integrate.api.nvidia.com/v1"
	case "ollama":
		return "http://localhost:11434/v1"
	case "moonshot", "kimi":
		return "https://api.moonshot.cn/v1"
	case "shengsuanyun":
		return "https://router.shengsuanyun.com/api/v1"
	case "deepseek":
		return "https://api.deepseek.com/v1"
	case "github_copilot":
		return "localhost:4321"
	default:
		return ""
	}
}
