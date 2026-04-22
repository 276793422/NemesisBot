package forge_test

import (
	"context"
	"errors"
	"strings"
	"testing"

	"github.com/276793422/NemesisBot/module/forge"
	"github.com/276793422/NemesisBot/module/providers"
)

func TestParseLLMInsights_BulletPoints(t *testing.T) {
	response := `Key insights:
- Consider creating a Skill for config editing
- Tool exec has low success rate
* Pattern read_file→edit_file is efficient
• Another insight here`

	insights := forge.ParseLLMInsights(response)

	if len(insights) != 4 {
		t.Fatalf("Expected 4 insights, got %d", len(insights))
	}
	if !contains(insights[0], "Consider creating") {
		t.Errorf("First insight mismatch: %s", insights[0])
	}
	if !contains(insights[2], "Pattern read_file") {
		t.Errorf("Third insight (asterisk) mismatch: %s", insights[2])
	}
}

func TestParseLLMInsights_NumberedList(t *testing.T) {
	response := `1. First suggestion
2. Second suggestion
3. Third suggestion`
	// Numbered lists are NOT parsed by ParseLLMInsights (only bullet points)
	insights := forge.ParseLLMInsights(response)
	if len(insights) != 0 {
		t.Errorf("Numbered list should not be parsed as bullet insights, got %d", len(insights))
	}
}

func TestParseLLMInsights_Empty(t *testing.T) {
	insights := forge.ParseLLMInsights("")
	if len(insights) != 0 {
		t.Errorf("Empty input should return empty slice, got %d", len(insights))
	}

	insights = forge.ParseLLMInsights("No bullet points here.\nJust text.")
	if len(insights) != 0 {
		t.Errorf("Text without bullets should return empty, got %d", len(insights))
	}
}

func TestParseLLMInsights_Mixed(t *testing.T) {
	response := `Analysis:
- Insight one
Some text without bullet
* Insight two
More text
- Insight three`

	insights := forge.ParseLLMInsights(response)
	if len(insights) != 3 {
		t.Fatalf("Expected 3 insights from mixed text, got %d", len(insights))
	}
}

func TestExtractJSONFromResponse_ValidJSON(t *testing.T) {
	response := `Here is the analysis:
{"patterns": 3, "confidence": 0.85, "suggestion": "create skill"}
End of response.`

	result, err := forge.ExtractJSONFromResponse(response)
	if err != nil {
		t.Fatalf("Should extract valid JSON: %v", err)
	}
	if result["patterns"].(float64) != 3 {
		t.Errorf("Expected patterns=3, got %v", result["patterns"])
	}
	if result["confidence"].(float64) != 0.85 {
		t.Errorf("Expected confidence=0.85, got %v", result["confidence"])
	}
}

func TestExtractJSONFromResponse_CodeBlock(t *testing.T) {
	response := "```json\n{\"key\": \"value\", \"count\": 42}\n```"

	result, err := forge.ExtractJSONFromResponse(response)
	if err != nil {
		t.Fatalf("Should extract JSON from code block: %v", err)
	}
	if result["key"] != "value" {
		t.Errorf("Expected key=value, got %v", result["key"])
	}
}

func TestExtractJSONFromResponse_NoJSON(t *testing.T) {
	response := "This response has no JSON at all. Just plain text."

	_, err := forge.ExtractJSONFromResponse(response)
	if err == nil {
		t.Error("Should error when no JSON found")
	}
	if !contains(err.Error(), "no JSON") {
		t.Errorf("Error should mention 'no JSON', got: %v", err)
	}
}

func TestExtractJSONFromResponse_Empty(t *testing.T) {
	_, err := forge.ExtractJSONFromResponse("")
	if err == nil {
		t.Error("Should error on empty input")
	}
}

func TestSemanticAnalysis_WithMockLLM(t *testing.T) {
	cfg := forge.DefaultForgeConfig()
	stats := &forge.ReflectionStats{
		TotalRecords:   50,
		UniquePatterns: 10,
		AvgSuccessRate: 0.8,
		ToolFrequency:  map[string]int{"read_file": 30, "exec": 20},
	}
	artifacts := []forge.Artifact{
		{ID: "skill-1", Type: forge.ArtifactSkill, Name: "test", Version: "1.0", Status: forge.StatusActive, UsageCount: 5, SuccessRate: 0.9},
	}

	mockProvider := &mockLLMProvider{
		chatFunc: func(ctx context.Context, messages []providers.Message, tools []providers.ToolDefinition, model string, options map[string]interface{}) (*providers.LLMResponse, error) {
			// Verify context is passed correctly
			if len(messages) < 2 {
				t.Error("Expected at least 2 messages")
			}
			if !strings.Contains(messages[1].Content, "50") {
				t.Error("User prompt should contain total records")
			}
			return &providers.LLMResponse{
				Content:      "Analysis: read_file pattern is dominant. Suggest creating a Skill.",
				FinishReason: "stop",
			}, nil
		},
	}

	result, err := forge.SemanticAnalysisForTest(context.Background(), mockProvider, stats, artifacts, nil, nil, cfg)
	if err != nil {
		t.Fatalf("SemanticAnalysis should succeed: %v", err)
	}
	if !contains(result, "read_file") {
		t.Errorf("Result should contain tool name, got: %s", result)
	}
}

func TestSemanticAnalysis_LLMError(t *testing.T) {
	cfg := forge.DefaultForgeConfig()
	stats := &forge.ReflectionStats{
		TotalRecords:   10,
		UniquePatterns: 2,
		AvgSuccessRate: 0.5,
	}

	mockProvider := &mockLLMProvider{
		chatFunc: func(ctx context.Context, messages []providers.Message, tools []providers.ToolDefinition, model string, options map[string]interface{}) (*providers.LLMResponse, error) {
			return nil, errors.New("LLM service unavailable")
		},
	}

	_, err := forge.SemanticAnalysisForTest(context.Background(), mockProvider, stats, nil, nil, nil, cfg)
	if err == nil {
		t.Error("Should return error when LLM fails")
	}
	if !contains(err.Error(), "LLM call failed") {
		t.Errorf("Error should mention LLM failure, got: %v", err)
	}
}

func TestSemanticAnalysis_WithCycle(t *testing.T) {
	cfg := forge.DefaultForgeConfig()
	stats := &forge.ReflectionStats{
		TotalRecords:   30,
		UniquePatterns: 5,
		AvgSuccessRate: 0.75,
	}
	cycle := &forge.LearningCycle{
		PatternsFound:   2,
		ActionsCreated:  1,
		ActionsExecuted: 1,
		ActionsSkipped:  0,
		PatternSummary: []forge.PatternSummary{
			{Type: "tool_chain", Fingerprint: "abc123", Frequency: 8, Confidence: 0.9},
		},
		PreviousOutcomes: []*forge.ActionOutcome{
			{ArtifactID: "skill-1", Verdict: "positive", ImprovementScore: 0.35, SampleSize: 10},
		},
	}

	var capturedPrompt string
	mockProvider := &mockLLMProvider{
		chatFunc: func(ctx context.Context, messages []providers.Message, tools []providers.ToolDefinition, model string, options map[string]interface{}) (*providers.LLMResponse, error) {
			capturedPrompt = messages[1].Content
			return &providers.LLMResponse{Content: "Learning cycle analysis", FinishReason: "stop"}, nil
		},
	}

	_, err := forge.SemanticAnalysisForTest(context.Background(), mockProvider, stats, nil, nil, cycle, cfg)
	if err != nil {
		t.Fatalf("SemanticAnalysis with cycle should succeed: %v", err)
	}

	// Verify cycle data was included in the prompt
	if !contains(capturedPrompt, "Closed-Loop Learning State") {
		t.Error("Prompt should include learning cycle state section")
	}
	if !contains(capturedPrompt, "tool_chain") {
		t.Error("Prompt should include pattern type from cycle")
	}
	if !contains(capturedPrompt, "positive") {
		t.Error("Prompt should include previous outcome verdict")
	}
}
