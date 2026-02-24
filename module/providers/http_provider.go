// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package providers

import (
	"context"

	"github.com/276793422/NemesisBot/module/providers/openai_compat"
)

type HTTPProvider struct {
	delegate *openai_compat.Provider
}

func NewHTTPProvider(apiKey, apiBase, proxy string) *HTTPProvider {
	return &HTTPProvider{
		delegate: openai_compat.NewProvider(apiKey, apiBase, proxy),
	}
}

func (p *HTTPProvider) Chat(ctx context.Context, messages []Message, tools []ToolDefinition, model string, options map[string]interface{}) (*LLMResponse, error) {
	return p.delegate.Chat(ctx, messages, tools, model, options)
}

func (p *HTTPProvider) GetDefaultModel() string {
	return ""
}
