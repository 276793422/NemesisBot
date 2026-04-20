package forge

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"

	"github.com/276793422/NemesisBot/module/providers"
)

// semanticAnalysis uses LLM to generate deeper insights from statistical data.
func semanticAnalysis(ctx context.Context, provider providers.LLMProvider, stats *ReflectionStats, artifacts []Artifact, config *ForgeConfig) (string, error) {
	// Build context for LLM
	var sb strings.Builder
	sb.WriteString("Analyze the following tool usage data from an AI agent system and provide insights:\n\n")

	sb.WriteString("## Statistical Summary\n")
	sb.WriteString(fmt.Sprintf("- Total tool invocations: %d\n", stats.TotalRecords))
	sb.WriteString(fmt.Sprintf("- Unique patterns: %d\n", stats.UniquePatterns))
	sb.WriteString(fmt.Sprintf("- Average success rate: %.1f%%\n\n", stats.AvgSuccessRate*100))

	sb.WriteString("## Tool Frequency\n")
	for tool, count := range stats.ToolFrequency {
		sb.WriteString(fmt.Sprintf("- %s: %d uses\n", tool, count))
	}

	sb.WriteString("\n## High-Frequency Patterns\n")
	for i, p := range stats.TopPatterns {
		if i >= 5 {
			break
		}
		sb.WriteString(fmt.Sprintf("- %s: %d uses, %.0f%% success, avg %dms\n",
			p.ToolName, p.Count, p.SuccessRate*100, p.AvgDurationMs))
	}

	if len(stats.LowSuccess) > 0 {
		sb.WriteString("\n## Low Success Patterns\n")
		for _, p := range stats.LowSuccess {
			sb.WriteString(fmt.Sprintf("- %s: %d uses, %.0f%% success\n",
				p.ToolName, p.Count, p.SuccessRate*100))
		}
	}

	sb.WriteString("\n## Existing Forge Artifacts\n")
	for _, a := range artifacts {
		sb.WriteString(fmt.Sprintf("- [%s] %s v%s (%s, %d uses, %.0f%% success)\n",
			a.Type, a.Name, a.Version, a.Status, a.UsageCount, a.SuccessRate*100))
	}

	sb.WriteString("\nPlease provide:\n")
	sb.WriteString("1. Key patterns that could become reusable Skills or scripts\n")
	sb.WriteString("2. Areas for improvement in the agent's tool usage\n")
	sb.WriteString("3. Suggestions for optimizing high-frequency operations\n")

	// Build messages
	messages := []providers.Message{
		{
			Role:    "system",
			Content: "You are an AI system analyst. Analyze tool usage data and provide concise, actionable insights. Focus on identifying patterns that could be automated, improved, or turned into reusable components. Keep your response under 500 words.",
		},
		{
			Role:    "user",
			Content: sb.String(),
		},
	}

	// Call LLM
	model := provider.GetDefaultModel()
	resp, err := provider.Chat(ctx, messages, nil, model, map[string]interface{}{
		"max_tokens": config.Reflection.LLMBudgetTokens,
	})
	if err != nil {
		return "", fmt.Errorf("LLM call failed: %w", err)
	}

	return resp.Content, nil
}

// ParseLLMInsights extracts structured suggestions from LLM response.
func ParseLLMInsights(response string) []string {
	var insights []string
	lines := strings.Split(response, "\n")
	for _, line := range lines {
		line = strings.TrimSpace(line)
		if strings.HasPrefix(line, "- ") || strings.HasPrefix(line, "* ") || strings.HasPrefix(line, "• ") {
			insights = append(insights, strings.TrimPrefix(strings.TrimPrefix(strings.TrimPrefix(line, "- "), "* "), "• "))
		}
	}
	return insights
}

// ExtractJSONFromResponse attempts to extract JSON from an LLM response.
func ExtractJSONFromResponse(response string) (map[string]interface{}, error) {
	start := strings.Index(response, "{")
	end := strings.LastIndex(response, "}")
	if start >= 0 && end > start {
		var result map[string]interface{}
		if err := json.Unmarshal([]byte(response[start:end+1]), &result); err != nil {
			return nil, err
		}
		return result, nil
	}
	return nil, fmt.Errorf("no JSON found in response")
}
