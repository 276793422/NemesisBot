package forge

import (
	"context"
	"fmt"
	"os"
	"time"

	"github.com/276793422/NemesisBot/module/providers"
)

// Pipeline orchestrates the three-stage validation process.
type Pipeline struct {
	validator  *StaticValidator
	testRunner *TestRunner
	evaluator  *QualityEvaluator
	registry   *Registry
	config     *ForgeConfig
}

// NewPipeline creates a new validation Pipeline.
func NewPipeline(registry *Registry, config *ForgeConfig) *Pipeline {
	return &Pipeline{
		validator:  NewStaticValidator(registry),
		testRunner: NewTestRunner(registry),
		evaluator:  NewQualityEvaluator(nil, config),
		registry:   registry,
		config:     config,
	}
}

// SetProvider sets the LLM provider for quality evaluation.
func (p *Pipeline) SetProvider(provider providers.LLMProvider) {
	p.evaluator.SetProvider(provider)
}

// Run executes the full validation pipeline for an artifact by ID.
func (p *Pipeline) Run(ctx context.Context, artifactID string) (*ArtifactValidation, error) {
	artifact, found := p.registry.Get(artifactID)
	if !found {
		return nil, fmt.Errorf("产物 %s 不存在", artifactID)
	}

	content, err := os.ReadFile(artifact.Path)
	if err != nil {
		return nil, fmt.Errorf("读取产物文件失败: %w", err)
	}

	return p.RunFromContent(ctx, &artifact, string(content)), nil
}

// RunFromContent executes the full validation pipeline with provided content.
func (p *Pipeline) RunFromContent(ctx context.Context, artifact *Artifact, content string) *ArtifactValidation {
	validation := &ArtifactValidation{
		LastValidated: time.Now().UTC(),
	}

	// Stage 1: Static validation
	stage1 := p.validator.Validate(artifact.Type, artifact.Name, content)
	validation.Stage1Static = stage1
	if !stage1.Passed {
		return validation
	}

	// Stage 2: Functional validation
	stage2 := p.testRunner.RunTests(ctx, artifact)
	validation.Stage2Functional = stage2
	if !stage2.Passed {
		return validation
	}

	// Stage 3: Quality evaluation (LLM-as-Judge)
	stage3 := p.evaluator.Evaluate(ctx, artifact, content)
	validation.Stage3Quality = stage3

	return validation
}

// DetermineStatus determines the artifact status based on validation results.
func (p *Pipeline) DetermineStatus(validation *ArtifactValidation) ArtifactStatus {
	if validation == nil {
		return StatusDraft
	}

	// If any stage failed, keep as draft
	if validation.Stage1Static != nil && !validation.Stage1Static.Passed {
		return StatusDraft
	}
	if validation.Stage2Functional != nil && !validation.Stage2Functional.Passed {
		return StatusDraft
	}

	// All stages passed - check quality score
	if validation.Stage3Quality != nil {
		if validation.Stage3Quality.Score >= 80 {
			return StatusActive
		}
		if validation.Stage3Quality.Score >= 60 {
			return StatusActive // Active but needs improvement
		}
		return StatusDraft
	}

	// Stages 1+2 passed, no stage 3 - default to testing
	return StatusTesting
}
