package forge

import (
	"context"
	"encoding/json"
	"fmt"
	"time"

	"github.com/276793422/NemesisBot/module/providers"
)

// QualityEvaluator performs Stage 3 quality evaluation using LLM-as-Judge.
type QualityEvaluator struct {
	provider providers.LLMProvider
	config   *ForgeConfig
}

// NewQualityEvaluator creates a new QualityEvaluator.
func NewQualityEvaluator(provider providers.LLMProvider, config *ForgeConfig) *QualityEvaluator {
	return &QualityEvaluator{
		provider: provider,
		config:   config,
	}
}

// SetProvider updates the LLM provider.
func (e *QualityEvaluator) SetProvider(provider providers.LLMProvider) {
	e.provider = provider
}

// Evaluate performs quality evaluation on an artifact using LLM-as-Judge.
func (e *QualityEvaluator) Evaluate(ctx context.Context, artifact *Artifact, content string) *QualityValidationResult {
	result := &QualityValidationResult{
		ValidationStage: ValidationStage{
			Timestamp: time.Now().UTC(),
		},
	}

	if e.provider == nil {
		// Default score when no provider is available
		result.Passed = true
		result.Score = 70
		result.Notes = "无 LLM Provider 可用，使用默认评分"
		result.Dimensions = map[string]int{
			"correctness": 70,
			"quality":     70,
			"security":    75,
			"reusability": 65,
		}
		return result
	}

	maxTokens := 2000
	if e.config != nil && e.config.Validation.LLMMaxTokens > 0 {
		maxTokens = e.config.Validation.LLMMaxTokens
	}

	prompt := fmt.Sprintf(`Evaluate the following Forge artifact for quality.

Type: %s
Name: %s
Version: %s

Content:
%s

Score each dimension from 0-100:
- correctness: Does the content correctly implement its stated purpose? (weight 40%%)
- quality: Code/text quality, clarity, documentation (weight 20%%)
- security: Security considerations, no dangerous patterns (weight 20%%)
- reusability: Can this be reused in other contexts? (weight 20%%)

Respond with ONLY a JSON object:
{"correctness": N, "quality": N, "security": N, "reusability": N, "notes": "brief explanation"}`,
		artifact.Type, artifact.Name, artifact.Version, content)

	messages := []providers.Message{
		{
			Role:    "system",
			Content: "You are a code quality evaluator. Respond only with valid JSON.",
		},
		{
			Role:    "user",
			Content: prompt,
		},
	}

	model := e.provider.GetDefaultModel()
	resp, err := e.provider.Chat(ctx, messages, nil, model, map[string]interface{}{
		"max_tokens": maxTokens,
	})
	if err != nil {
		result.Errors = append(result.Errors, "LLM 调用失败: "+err.Error())
		result.Passed = false
		result.Score = 0
		return result
	}

	// Parse LLM response
	parsed, err := ExtractJSONFromResponse(resp.Content)
	if err != nil {
		result.Errors = append(result.Errors, "无法解析 LLM 响应: "+err.Error())
		result.Passed = false
		result.Score = 0
		return result
	}

	// Extract dimensions
	dimensions := make(map[string]int)
	for _, key := range []string{"correctness", "quality", "security", "reusability"} {
		if val, ok := parsed[key]; ok {
			switch v := val.(type) {
			case float64:
				dimensions[key] = int(v)
			case json.Number:
				if n, err := v.Int64(); err == nil {
					dimensions[key] = int(n)
				}
			}
		}
	}

	// Extract notes
	if notes, ok := parsed["notes"]; ok {
		result.Notes = fmt.Sprintf("%v", notes)
	}

	result.Dimensions = dimensions

	// Calculate weighted score: correctness(40%) + quality(20%) + security(20%) + reusability(20%)
	score := 0
	if c, ok := dimensions["correctness"]; ok {
		score += c * 40 / 100
	}
	if q, ok := dimensions["quality"]; ok {
		score += q * 20 / 100
	}
	if s, ok := dimensions["security"]; ok {
		score += s * 20 / 100
	}
	if r, ok := dimensions["reusability"]; ok {
		score += r * 20 / 100
	}

	result.Score = score

	// Determine pass based on config threshold
	minScore := 60
	if e.config != nil && e.config.Validation.MinQualityScore > 0 {
		minScore = e.config.Validation.MinQualityScore
	}
	result.Passed = score >= minScore

	return result
}
