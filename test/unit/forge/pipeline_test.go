package forge_test

import (
	"context"
	"os"
	"path/filepath"
	"testing"

	"github.com/276793422/NemesisBot/module/forge"
	"github.com/276793422/NemesisBot/module/providers"
)

// === Pipeline Tests ===

func newTestPipeline(t *testing.T) (*forge.Pipeline, string) {
	t.Helper()
	tmpDir := t.TempDir()
	path := filepath.Join(tmpDir, "registry.json")
	registry := forge.NewRegistry(path)
	cfg := forge.DefaultForgeConfig()
	return forge.NewPipeline(registry, cfg), tmpDir
}

func TestPipelineAllStagesPass(t *testing.T) {
	pipeline, tmpDir := newTestPipeline(t)

	// Create a well-structured skill
	skillDir := filepath.Join(tmpDir, "skills", "good-skill")
	os.MkdirAll(skillDir, 0755)
	skillContent := "---\nname: good-skill\ndescription: A good skill\n---\n\n## 步骤\n\n1. Do something\n2. Verify\n\n## 触发条件\n\nWhen needed.\n\n- Item 1\n- Item 2\n"
	skillPath := filepath.Join(skillDir, "SKILL.md")
	os.WriteFile(skillPath, []byte(skillContent), 0644)

	artifact := &forge.Artifact{
		Type:    forge.ArtifactSkill,
		Name:    "good-skill",
		Version: "1.0",
		Path:    skillPath,
	}

	// Set up mock provider for Stage 3
	mockProvider := &mockLLMProvider{
		chatFunc: func(ctx context.Context, messages []providers.Message, tools []providers.ToolDefinition, model string, options map[string]interface{}) (*providers.LLMResponse, error) {
			return &providers.LLMResponse{
				Content:      `{"correctness": 90, "quality": 85, "security": 95, "reusability": 80, "notes": "Excellent"}`,
				FinishReason: "stop",
			}, nil
		},
	}
	pipeline.SetProvider(mockProvider)

	validation := pipeline.RunFromContent(context.Background(), artifact, skillContent)

	if validation.Stage1Static == nil || !validation.Stage1Static.Passed {
		t.Error("Stage 1 should pass")
	}
	if validation.Stage2Functional == nil || !validation.Stage2Functional.Passed {
		t.Error("Stage 2 should pass")
	}
	if validation.Stage3Quality == nil || !validation.Stage3Quality.Passed {
		t.Error("Stage 3 should pass")
	}
}

func TestPipelineStage1FailStops(t *testing.T) {
	pipeline, _ := newTestPipeline(t)

	artifact := &forge.Artifact{
		Type:    forge.ArtifactSkill,
		Name:    "bad-skill",
		Version: "1.0",
		Path:    "/nonexistent/path",
	}

	// Content too short
	validation := pipeline.RunFromContent(context.Background(), artifact, "hi")

	if validation.Stage1Static == nil || validation.Stage1Static.Passed {
		t.Error("Stage 1 should fail with short content")
	}
	if validation.Stage2Functional != nil {
		t.Error("Stage 2 should not run when Stage 1 fails")
	}
	if validation.Stage3Quality != nil {
		t.Error("Stage 3 should not run when Stage 1 fails")
	}
}

func TestPipelineDetermineStatusAllPass(t *testing.T) {
	pipeline, _ := newTestPipeline(t)

	validation := &forge.ArtifactValidation{
		Stage1Static:     &forge.StaticValidationResult{ValidationStage: forge.ValidationStage{Passed: true}},
		Stage2Functional: &forge.FunctionalValidationResult{ValidationStage: forge.ValidationStage{Passed: true}},
		Stage3Quality:    &forge.QualityValidationResult{ValidationStage: forge.ValidationStage{Passed: true}, Score: 85},
	}

	status := pipeline.DetermineStatus(validation)
	if status != forge.StatusActive {
		t.Errorf("Expected active status for score 85, got %s", status)
	}
}

func TestPipelineDetermineStatusNeedsImprovement(t *testing.T) {
	pipeline, _ := newTestPipeline(t)

	validation := &forge.ArtifactValidation{
		Stage1Static:     &forge.StaticValidationResult{ValidationStage: forge.ValidationStage{Passed: true}},
		Stage2Functional: &forge.FunctionalValidationResult{ValidationStage: forge.ValidationStage{Passed: true}},
		Stage3Quality:    &forge.QualityValidationResult{ValidationStage: forge.ValidationStage{Passed: true}, Score: 65},
	}

	status := pipeline.DetermineStatus(validation)
	if status != forge.StatusActive {
		t.Errorf("Expected active status for score 65, got %s", status)
	}
}

func TestPipelineDetermineStatusLowScore(t *testing.T) {
	pipeline, _ := newTestPipeline(t)

	validation := &forge.ArtifactValidation{
		Stage1Static:     &forge.StaticValidationResult{ValidationStage: forge.ValidationStage{Passed: true}},
		Stage2Functional: &forge.FunctionalValidationResult{ValidationStage: forge.ValidationStage{Passed: true}},
		Stage3Quality:    &forge.QualityValidationResult{ValidationStage: forge.ValidationStage{Passed: false}, Score: 40},
	}

	status := pipeline.DetermineStatus(validation)
	if status != forge.StatusDraft {
		t.Errorf("Expected draft status for score 40, got %s", status)
	}
}

func TestPipelineDetermineStatusNilValidation(t *testing.T) {
	pipeline, _ := newTestPipeline(t)

	status := pipeline.DetermineStatus(nil)
	if status != forge.StatusDraft {
		t.Errorf("Expected draft status for nil validation, got %s", status)
	}
}

func TestPipelineRunWithRegisteredArtifact(t *testing.T) {
	tmpDir := t.TempDir()
	path := filepath.Join(tmpDir, "registry.json")
	registry := forge.NewRegistry(path)
	cfg := forge.DefaultForgeConfig()
	pipeline := forge.NewPipeline(registry, cfg)

	// Create skill file
	skillDir := filepath.Join(tmpDir, "skills", "reg-skill")
	os.MkdirAll(skillDir, 0755)
	skillPath := filepath.Join(skillDir, "SKILL.md")
	skillContent := "---\nname: reg-skill\ndescription: Test\n---\n\n## 步骤\n\n1. Do stuff\n\n## 触发条件\n\nWhen needed.\n"
	os.WriteFile(skillPath, []byte(skillContent), 0644)

	// Register artifact
	registry.Add(forge.Artifact{
		ID:   "skill-reg-skill",
		Type: forge.ArtifactSkill,
		Name: "reg-skill",
		Path: skillPath,
	})

	validation, err := pipeline.Run(context.Background(), "skill-reg-skill")
	if err != nil {
		t.Fatalf("Run failed: %v", err)
	}
	if validation == nil {
		t.Fatal("Expected non-nil validation")
	}
	if validation.Stage1Static == nil || !validation.Stage1Static.Passed {
		t.Error("Stage 1 should pass")
	}
}

func TestPipelineRunNonexistentArtifact(t *testing.T) {
	tmpDir := t.TempDir()
	path := filepath.Join(tmpDir, "registry.json")
	registry := forge.NewRegistry(path)
	cfg := forge.DefaultForgeConfig()
	pipeline := forge.NewPipeline(registry, cfg)

	_, err := pipeline.Run(context.Background(), "nonexistent")
	if err == nil {
		t.Error("Expected error for nonexistent artifact")
	}
}
