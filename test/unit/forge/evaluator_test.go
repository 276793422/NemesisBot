package forge_test

import (
	"context"
	"testing"

	"github.com/276793422/NemesisBot/module/forge"
	"github.com/276793422/NemesisBot/module/providers"
)

// === QualityEvaluator Tests (Stage 3) ===

func TestEvaluatorNoProvider(t *testing.T) {
	cfg := forge.DefaultForgeConfig()
	evaluator := forge.NewQualityEvaluator(nil, cfg)

	artifact := &forge.Artifact{
		Type:    forge.ArtifactSkill,
		Name:    "test-skill",
		Version: "1.0",
	}

	result := evaluator.Evaluate(context.Background(), artifact, "some content")

	if !result.Passed {
		t.Error("Expected default pass when no provider")
	}
	if result.Score != 70 {
		t.Errorf("Expected default score 70, got %d", result.Score)
	}
	if len(result.Dimensions) == 0 {
		t.Error("Expected default dimensions")
	}
}

func TestEvaluatorWithMockProvider(t *testing.T) {
	cfg := forge.DefaultForgeConfig()
	mockProvider := &mockLLMProvider{
		chatFunc: func(ctx context.Context, messages []providers.Message, tools []providers.ToolDefinition, model string, options map[string]interface{}) (*providers.LLMResponse, error) {
			return &providers.LLMResponse{
				Content:      `{"correctness": 85, "quality": 80, "security": 90, "reusability": 75, "notes": "Good quality"}`,
				FinishReason: "stop",
			}, nil
		},
	}

	evaluator := forge.NewQualityEvaluator(mockProvider, cfg)
	artifact := &forge.Artifact{
		Type:    forge.ArtifactSkill,
		Name:    "test-skill",
		Version: "1.0",
	}

	result := evaluator.Evaluate(context.Background(), artifact, "---\nname: test\n---\nGood content here")

	if !result.Passed {
		t.Errorf("Expected pass with good scores, got errors: %v", result.Errors)
	}
	if result.Score <= 0 {
		t.Errorf("Expected positive score, got %d", result.Score)
	}
	if result.Notes != "Good quality" {
		t.Errorf("Expected notes 'Good quality', got '%s'", result.Notes)
	}
	if len(result.Dimensions) != 4 {
		t.Errorf("Expected 4 dimensions, got %d", len(result.Dimensions))
	}
	if result.Dimensions["correctness"] != 85 {
		t.Errorf("Expected correctness 85, got %d", result.Dimensions["correctness"])
	}
}

func TestEvaluatorLLMError(t *testing.T) {
	cfg := forge.DefaultForgeConfig()
	mockProvider := &mockLLMProvider{
		chatFunc: func(ctx context.Context, messages []providers.Message, tools []providers.ToolDefinition, model string, options map[string]interface{}) (*providers.LLMResponse, error) {
			return nil, context.DeadlineExceeded
		},
	}

	evaluator := forge.NewQualityEvaluator(mockProvider, cfg)
	artifact := &forge.Artifact{
		Type:    forge.ArtifactSkill,
		Name:    "test-skill",
		Version: "1.0",
	}

	result := evaluator.Evaluate(context.Background(), artifact, "content")

	if result.Passed {
		t.Error("Expected fail when LLM errors")
	}
	if result.Score != 0 {
		t.Errorf("Expected score 0 on error, got %d", result.Score)
	}
}

func TestEvaluatorInvalidJSON(t *testing.T) {
	cfg := forge.DefaultForgeConfig()
	mockProvider := &mockLLMProvider{
		chatFunc: func(ctx context.Context, messages []providers.Message, tools []providers.ToolDefinition, model string, options map[string]interface{}) (*providers.LLMResponse, error) {
			return &providers.LLMResponse{
				Content:      "This is not JSON at all",
				FinishReason: "stop",
			}, nil
		},
	}

	evaluator := forge.NewQualityEvaluator(mockProvider, cfg)
	artifact := &forge.Artifact{
		Type:    forge.ArtifactSkill,
		Name:    "test-skill",
		Version: "1.0",
	}

	result := evaluator.Evaluate(context.Background(), artifact, "content")

	if result.Passed {
		t.Error("Expected fail with invalid JSON response")
	}
}

func TestEvaluatorLowScore(t *testing.T) {
	cfg := forge.DefaultForgeConfig()
	mockProvider := &mockLLMProvider{
		chatFunc: func(ctx context.Context, messages []providers.Message, tools []providers.ToolDefinition, model string, options map[string]interface{}) (*providers.LLMResponse, error) {
			return &providers.LLMResponse{
				Content:      `{"correctness": 30, "quality": 20, "security": 40, "reusability": 10, "notes": "Poor quality"}`,
				FinishReason: "stop",
			}, nil
		},
	}

	evaluator := forge.NewQualityEvaluator(mockProvider, cfg)
	artifact := &forge.Artifact{
		Type:    forge.ArtifactSkill,
		Name:    "bad-skill",
		Version: "1.0",
	}

	result := evaluator.Evaluate(context.Background(), artifact, "bad content")

	if result.Passed {
		t.Error("Expected fail with low scores")
	}
	if result.Score >= 60 {
		t.Errorf("Expected score < 60, got %d", result.Score)
	}
}

func TestEvaluatorSetProvider(t *testing.T) {
	cfg := forge.DefaultForgeConfig()
	evaluator := forge.NewQualityEvaluator(nil, cfg)

	// Initially no provider → default score
	artifact := &forge.Artifact{Type: forge.ArtifactSkill, Name: "test", Version: "1.0"}
	result1 := evaluator.Evaluate(context.Background(), artifact, "content")
	if result1.Score != 70 {
		t.Errorf("Expected default score 70, got %d", result1.Score)
	}

	// Set provider
	mockProvider := &mockLLMProvider{
		chatFunc: func(ctx context.Context, messages []providers.Message, tools []providers.ToolDefinition, model string, options map[string]interface{}) (*providers.LLMResponse, error) {
			return &providers.LLMResponse{
				Content:      `{"correctness": 90, "quality": 90, "security": 90, "reusability": 90, "notes": "Excellent"}`,
				FinishReason: "stop",
			}, nil
		},
	}
	evaluator.SetProvider(mockProvider)

	result2 := evaluator.Evaluate(context.Background(), artifact, "content")
	if result2.Score == 70 {
		t.Error("Expected non-default score after setting provider")
	}
}

func TestEvaluatorCustomMinScore(t *testing.T) {
	cfg := forge.DefaultForgeConfig()
	cfg.Validation.MinQualityScore = 80 // Higher threshold

	mockProvider := &mockLLMProvider{
		chatFunc: func(ctx context.Context, messages []providers.Message, tools []providers.ToolDefinition, model string, options map[string]interface{}) (*providers.LLMResponse, error) {
			return &providers.LLMResponse{
				Content:      `{"correctness": 70, "quality": 70, "security": 70, "reusability": 70, "notes": "Mediocre"}`,
				FinishReason: "stop",
			}, nil
		},
	}

	evaluator := forge.NewQualityEvaluator(mockProvider, cfg)
	artifact := &forge.Artifact{Type: forge.ArtifactSkill, Name: "test", Version: "1.0"}

	result := evaluator.Evaluate(context.Background(), artifact, "content")
	if result.Passed {
		t.Errorf("Expected fail with score %d < threshold 80", result.Score)
	}
}
