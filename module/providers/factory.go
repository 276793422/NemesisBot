// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package providers

import (
	"fmt"

	"github.com/276793422/NemesisBot/module/auth"
	"github.com/276793422/NemesisBot/module/config"
)

const defaultAnthropicAPIBase = "https://api.anthropic.com/v1"

var getCredential = auth.GetCredential

type providerType int

const (
	providerTypeHTTPCompat providerType = iota
	providerTypeClaudeAuth
	providerTypeCodexAuth
	providerTypeClaudeCLI
	providerTypeCodexCLI
	providerTypeGitHubCopilot
)

type providerSelection struct {
	providerType    providerType
	apiKey          string
	apiBase         string
	proxy           string
	model           string
	workspace       string
	connectMode     string
	enableWebSearch bool
}

// resolveProviderSelection resolves the provider selection using the new ModelList config structure
func resolveProviderSelection(cfg *config.Config) (providerSelection, error) {
	// Get effective LLM reference
	llmRef := config.GetEffectiveLLM(cfg)

	// Resolve model configuration using new ModelList system
	modelRes, err := config.ResolveModelConfig(cfg, llmRef)
	if err != nil {
		return providerSelection{}, err
	}

	// Extract provider name from model field (format: vendor/model)
	providerName := modelRes.ProviderName
	modelName := modelRes.ModelName

	sel := providerSelection{
		providerType: providerTypeHTTPCompat,
		model:        modelName,
		apiBase:      modelRes.APIBase,
		proxy:        modelRes.Proxy,
		connectMode:  modelRes.ConnectMode,
		workspace:    modelRes.Workspace,
	}

	// Handle special providers first (before checking API key)
	if providerName == "claude-cli" || providerName == "claude-code" || providerName == "claudecode" || providerName == "claudecodec" {
		sel.providerType = providerTypeClaudeCLI
		if sel.workspace == "" {
			sel.workspace = cfg.WorkspacePath()
		}
		if sel.workspace == "" {
			sel.workspace = "."
		}
		return sel, nil
	}

	if providerName == "codex-cli" || providerName == "codex-code" {
		sel.providerType = providerTypeCodexCLI
		if sel.workspace == "" {
			sel.workspace = cfg.WorkspacePath()
		}
		if sel.workspace == "" {
			sel.workspace = "."
		}
		return sel, nil
	}

	if providerName == "github_copilot" || providerName == "copilot" {
		sel.providerType = providerTypeGitHubCopilot
		return sel, nil
	}

	// Handle authentication method
	if modelRes.AuthMethod == "oauth" || modelRes.AuthMethod == "token" {
		// OAuth/token authentication
		switch providerName {
		case "anthropic", "claude":
			sel.providerType = providerTypeClaudeAuth
		case "openai", "gpt":
			sel.providerType = providerTypeCodexAuth
		default:
			return providerSelection{}, fmt.Errorf("OAuth authentication not supported for provider: %s", providerName)
		}
	} else if modelRes.APIKey != "" {
		// Direct API key authentication
		sel.apiKey = modelRes.APIKey
	} else {
		// No authentication configured
		return providerSelection{}, fmt.Errorf("no API key configured for provider: %s (please set api_key in model_list or use: nemesisbot auth login --provider %s)", providerName, providerName)
	}

	return sel, nil
}

func createClaudeAuthProvider(apiBase string) (LLMProvider, error) {
	if apiBase == "" {
		apiBase = defaultAnthropicAPIBase
	}
	cred, err := getCredential("anthropic")
	if err != nil {
		return nil, fmt.Errorf("loading auth credentials: %w", err)
	}
	if cred == nil {
		return nil, fmt.Errorf("no credentials for anthropic. Run: nemesisbot auth login --provider anthropic")
	}
	return NewClaudeProviderWithTokenSourceAndBaseURL(cred.AccessToken, createClaudeTokenSource(), apiBase), nil
}

func createCodexAuthProvider(enableWebSearch bool) (LLMProvider, error) {
	cred, err := getCredential("openai")
	if err != nil {
		return nil, fmt.Errorf("loading auth credentials: %w", err)
	}
	if cred == nil {
		return nil, fmt.Errorf("no credentials for openai. Run: nemesisbot auth login --provider openai")
	}
	p := NewCodexProviderWithTokenSource(cred.AccessToken, cred.AccountID, createCodexTokenSource())
	p.enableWebSearch = enableWebSearch
	return p, nil
}

func CreateProvider(cfg *config.Config) (LLMProvider, error) {
	sel, err := resolveProviderSelection(cfg)
	if err != nil {
		return nil, err
	}

	switch sel.providerType {
	case providerTypeClaudeAuth:
		return createClaudeAuthProvider(sel.apiBase)
	case providerTypeCodexAuth:
		return createCodexAuthProvider(sel.enableWebSearch)
	case providerTypeClaudeCLI:
		return NewClaudeCliProvider(sel.workspace), nil
	case providerTypeCodexCLI:
		return NewCodexCliProvider(sel.workspace), nil
	case providerTypeGitHubCopilot:
		prov, err := NewGitHubCopilotProvider(sel.apiBase, sel.connectMode, sel.model)
		if err != nil {
			return nil, fmt.Errorf("creating github copilot provider: %w", err)
		}
		return prov, nil
	default:
		return NewHTTPProvider(sel.apiKey, sel.apiBase, sel.proxy), nil
	}
}
