package forge

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"

	"github.com/276793422/NemesisBot/module/logger"
	"github.com/276793422/NemesisBot/module/providers"
)

// ActionType identifies the type of learning action.
type ActionType string

const (
	ActionCreateSkill   ActionType = "create_skill"
	ActionSuggestPrompt ActionType = "suggest_prompt"
	ActionDeprecate     ActionType = "deprecate_artifact"
)

// LearningAction represents an action to be taken based on detected patterns.
type LearningAction struct {
	ID          string      `json:"id"`
	Type        ActionType  `json:"type"`
	Priority    string      `json:"priority"` // "high"/"medium"/"low"
	Confidence  float64     `json:"confidence"`
	PatternID   string      `json:"pattern_id"`
	Status      string      `json:"status"` // "pending"/"executed"/"skipped"/"failed"
	ArtifactID  string      `json:"artifact_id,omitempty"`
	DraftName   string      `json:"draft_name,omitempty"`
	Description string      `json:"description"`
	Rationale   string      `json:"rationale"`
	CreatedAt   time.Time   `json:"created_at"`
	ExecutedAt  *time.Time  `json:"executed_at,omitempty"`
	ErrorMsg    string      `json:"error_msg,omitempty"`
}

// ActionOutcome measures the effect of a deployed artifact.
type ActionOutcome struct {
	ActionID         string    `json:"action_id"`
	ArtifactID       string    `json:"artifact_id"`
	MeasuredAt       time.Time `json:"measured_at"`
	SampleSize       int       `json:"sample_size"`
	RoundsBeforeAvg  float64   `json:"rounds_before_avg"`
	RoundsAfterAvg   float64   `json:"rounds_after_avg"`
	SuccessBefore    float64   `json:"success_before"`
	SuccessAfter     float64   `json:"success_after"`
	DurationBeforeMs int64     `json:"duration_before_ms"`
	DurationAfterMs  int64     `json:"duration_after_ms"`
	ImprovementScore float64   `json:"improvement_score"`
	Verdict          string    `json:"verdict"` // "positive"/"neutral"/"negative"/"insufficient_data"/"observing"
}

// PatternSummary is a compact representation of a detected pattern for cycle storage.
type PatternSummary struct {
	ID          string  `json:"id"`
	Type        string  `json:"type"`
	Fingerprint string  `json:"fingerprint"`
	Frequency   int     `json:"frequency"`
	Confidence  float64 `json:"confidence"`
}

// ActionSummary is a compact representation of a learning action for cycle storage.
type ActionSummary struct {
	ID         string `json:"id"`
	Type       string `json:"type"`
	Priority   string `json:"priority"`
	Status     string `json:"status"`
	ArtifactID string `json:"artifact_id,omitempty"`
}

// LearningCycle records a single learning cycle execution.
type LearningCycle struct {
	ID               string            `json:"id"`
	StartedAt        time.Time         `json:"started_at"`
	CompletedAt      *time.Time        `json:"completed_at,omitempty"`
	CycleNumber      int               `json:"cycle_number"`
	PatternsFound    int               `json:"patterns_found"`
	ActionsCreated   int               `json:"actions_created"`
	ActionsExecuted  int               `json:"actions_executed"`
	ActionsSkipped   int               `json:"actions_skipped"`
	PreviousOutcomes []*ActionOutcome  `json:"previous_outcomes,omitempty"`
	PatternSummary   []PatternSummary  `json:"pattern_summary,omitempty"`
	ActionSummary    []ActionSummary   `json:"action_summary,omitempty"`
}

// LearningEngine orchestrates the closed-loop self-learning cycle.
type LearningEngine struct {
	config     *ForgeConfig
	forgeDir   string
	registry   *Registry
	traceStore *TraceStore
	pipeline   *Pipeline
	cycleStore *CycleStore
	monitor    *DeploymentMonitor
	provider   providers.LLMProvider
	forge      *Forge // for CreateSkill shared method
}

// NewLearningEngine creates a new LearningEngine.
func NewLearningEngine(forgeDir string, registry *Registry, traceStore *TraceStore, pipeline *Pipeline, cycleStore *CycleStore, monitor *DeploymentMonitor, config *ForgeConfig) *LearningEngine {
	return &LearningEngine{
		config:     config,
		forgeDir:   forgeDir,
		registry:   registry,
		traceStore: traceStore,
		pipeline:   pipeline,
		cycleStore: cycleStore,
		monitor:    monitor,
	}
}

// SetProvider injects the LLM provider for Skill draft generation.
func (le *LearningEngine) SetProvider(provider providers.LLMProvider) {
	le.provider = provider
}

// SetForge injects the parent Forge instance for CreateSkill shared method.
func (le *LearningEngine) SetForge(f *Forge) {
	le.forge = f
}

// RunCycle executes one full learning cycle. It runs with an internal timeout
// and does not block the caller on failure.
func (le *LearningEngine) RunCycle(ctx context.Context, traces []*ConversationTrace, traceStats *TraceStats, stats *ReflectionStats) *LearningCycle {
	cycle := &LearningCycle{
		ID:         fmt.Sprintf("lc-%d", time.Now().UnixNano()),
		StartedAt:  time.Now().UTC(),
	}

	// 1. Evaluate previous outcomes
	previousOutcomes := le.monitor.EvaluateOutcomes()
	cycle.PreviousOutcomes = previousOutcomes

	// 2. Adjust confidence based on feedback
	le.adjustConfidence(previousOutcomes)

	// 3. Extract patterns
	minFreq := le.config.Learning.MinPatternFrequency
	if minFreq <= 0 {
		minFreq = 5
	}
	patterns := extractPatterns(traces, minFreq)
	cycle.PatternsFound = len(patterns)

	// Build pattern summaries
	for _, p := range patterns {
		cycle.PatternSummary = append(cycle.PatternSummary, PatternSummary{
			ID:          p.ID,
			Type:        string(p.Type),
			Fingerprint: p.Fingerprint,
			Frequency:   p.Frequency,
			Confidence:  p.Confidence,
		})
	}

	// 3.5 Check suggestion adoption status
	le.checkSuggestionAdoption(patterns)

	// 4. Generate actions from patterns
	actions := le.generateActions(patterns)
	cycle.ActionsCreated = len(actions)

	// 5. Execute actions (limit auto-creates)
	autoCreateCount := 0
	maxAutoCreates := le.config.Learning.MaxAutoCreates
	if maxAutoCreates <= 0 {
		maxAutoCreates = 3
	}

	for _, action := range actions {
		if action.Type == ActionCreateSkill {
			if autoCreateCount >= maxAutoCreates {
				action.Status = "skipped"
				cycle.ActionsSkipped++
				cycle.ActionSummary = append(cycle.ActionSummary, actionToSummary(action))
				continue
			}
			autoCreateCount++ // Count attempts, not just successes
			le.executeCreateSkill(ctx, action)
			if action.Status == "executed" {
				cycle.ActionsExecuted++
			}
		} else if action.Type == ActionSuggestPrompt {
			le.executeSuggestPrompt(action)
			if action.Status == "executed" {
				cycle.ActionsExecuted++
			}
		}
		cycle.ActionSummary = append(cycle.ActionSummary, actionToSummary(action))
	}

	// 6. Save cycle
	now := time.Now().UTC()
	cycle.CompletedAt = &now
	if le.cycleStore != nil {
		if err := le.cycleStore.Append(cycle); err != nil {
			logger.WarnCF("forge", "Failed to save learning cycle", map[string]interface{}{
				"error": err.Error(),
			})
		}
	}

	return cycle
}

// adjustConfidence modifies pattern confidence based on deployment feedback.
func (le *LearningEngine) adjustConfidence(outcomes []*ActionOutcome) {
	for _, o := range outcomes {
		if o.ArtifactID == "" {
			continue
		}
		artifact, found := le.registry.Get(o.ArtifactID)
		if !found {
			continue
		}
		// Adjust the artifact's success rate based on outcome
		delta := 0.0
		switch o.Verdict {
		case "positive":
			delta = 0.1
		case "negative":
			delta = -0.2
		}
		if delta != 0 {
			newRate := artifact.SuccessRate + delta
			if newRate < 0 {
				newRate = 0
			}
			if newRate > 1 {
				newRate = 1
			}
			le.registry.Update(o.ArtifactID, func(a *Artifact) {
				a.SuccessRate = newRate
			})
		}
	}
}

// generateActions creates LearningActions from detected patterns.
func (le *LearningEngine) generateActions(patterns []*ConversationPattern) []*LearningAction {
	var actions []*LearningAction
	highConf := le.config.Learning.HighConfThreshold
	if highConf <= 0 {
		highConf = 0.8
	}

	for _, p := range patterns {
		switch p.Type {
		case PatternToolChain:
			if p.Confidence >= highConf && p.Frequency >= 10 {
				actions = append(actions, &LearningAction{
					ID:          fmt.Sprintf("la-%s", p.ID),
					Type:        ActionCreateSkill,
					Priority:    "high",
					Confidence:  p.Confidence,
					PatternID:   p.ID,
					Status:      "pending",
					DraftName:   generateSkillName(p.ToolChain),
					Description: p.Description,
					Rationale:   fmt.Sprintf("High-confidence tool chain (%.2f) with frequency %d", p.Confidence, p.Frequency),
					CreatedAt:   time.Now().UTC(),
				})
			} else {
				actions = append(actions, &LearningAction{
					ID:          fmt.Sprintf("la-%s", p.ID),
					Type:        ActionSuggestPrompt,
					Priority:    "medium",
					Confidence:  p.Confidence,
					PatternID:   p.ID,
					Status:      "pending",
					DraftName:   generateSkillName(p.ToolChain),
					Description: p.Description,
					Rationale:   fmt.Sprintf("Tool chain below threshold (%.2f < %.2f), suggest prompt", p.Confidence, highConf),
					CreatedAt:   time.Now().UTC(),
				})
			}

		case PatternErrorRecovery:
			if p.Confidence >= highConf {
				actions = append(actions, &LearningAction{
					ID:          fmt.Sprintf("la-%s", p.ID),
					Type:        ActionCreateSkill,
					Priority:    "high",
					Confidence:  p.Confidence,
					PatternID:   p.ID,
					Status:      "pending",
					DraftName:   fmt.Sprintf("%s-error-handler", p.RecoveryTool),
					Description: p.Description,
					Rationale:   fmt.Sprintf("High-confidence error recovery (%.2f): %s → %s", p.Confidence, p.ErrorTool, p.RecoveryTool),
					CreatedAt:   time.Now().UTC(),
				})
			}

		case PatternEfficiencyIssue:
			actions = append(actions, &LearningAction{
				ID:          fmt.Sprintf("la-%s", p.ID),
				Type:        ActionSuggestPrompt,
				Priority:    "medium",
				Confidence:  p.Confidence,
				PatternID:   p.ID,
				Status:      "pending",
				DraftName:   generateSkillName(p.ToolChain),
				Description: p.Description,
				Rationale:   fmt.Sprintf("Efficiency issue (%.2f score), suggest optimization", p.EfficiencyScore),
				CreatedAt:   time.Now().UTC(),
			})

		case PatternSuccessTemplate:
			if p.Confidence >= highConf {
				actions = append(actions, &LearningAction{
					ID:          fmt.Sprintf("la-%s", p.ID),
					Type:        ActionCreateSkill,
					Priority:    "high",
					Confidence:  p.Confidence,
					PatternID:   p.ID,
					Status:      "pending",
					DraftName:   generateSkillName(p.ToolChain),
					Description: p.Description,
					Rationale:   fmt.Sprintf("Success template (%.2f confidence), automate as Skill", p.Confidence),
					CreatedAt:   time.Now().UTC(),
				})
			}
		}
	}

	// Sort by priority (high first) then confidence descending
	sortActions(actions)
	return actions
}

// executeCreateSkill generates a Skill via LLM, validates it, and deploys if it passes.
func (le *LearningEngine) executeCreateSkill(ctx context.Context, action *LearningAction) {
	// Check if already exists in Registry (dedup by fingerprint)
	existing := le.findArtifactByFingerprint(action.DraftName)
	if existing != nil {
		action.Status = "skipped"
		action.ErrorMsg = fmt.Sprintf("Artifact %s already exists", existing.ID)
		return
	}

	// Generate Skill draft using LLM
	if le.provider == nil {
		action.Status = "failed"
		action.ErrorMsg = "No LLM provider available"
		return
	}

	content, err := le.generateSkillDraft(ctx, action)
	if err != nil {
		action.Status = "failed"
		action.ErrorMsg = fmt.Sprintf("LLM generation failed: %v", err)
		return
	}

	// Iterative refinement loop
	maxRefine := le.config.Learning.MaxRefineRounds
	if maxRefine <= 0 {
		maxRefine = 3
	}

	for attempt := 0; attempt <= maxRefine; attempt++ {
		// Validate using Pipeline (in-memory, don't write file yet)
		artifact := &Artifact{
			Type: ArtifactSkill,
			Name: action.DraftName,
		}
		validation := le.pipeline.RunFromContent(ctx, artifact, content)
		newStatus := le.pipeline.DetermineStatus(validation)

		if newStatus == StatusActive || newStatus == StatusTesting {
			// Passed — deploy
			toolSig := extractToolSignatureFromChain(action.Description)
			deployedArtifact, err := le.forge.CreateSkill(ctx, action.DraftName, content, action.Description, toolSig)
			if err != nil {
				action.Status = "failed"
				action.ErrorMsg = fmt.Sprintf("Deploy failed: %v", err)
				return
			}
			action.Status = "executed"
			action.ArtifactID = deployedArtifact.ID
			now := time.Now().UTC()
			action.ExecutedAt = &now
			return
		}

		// Failed — try to refine
		if attempt < maxRefine {
			diagnosis := buildDiagnosis(validation)
			refinedContent, refineErr := le.refineSkillDraft(ctx, action, content, diagnosis)
			if refineErr != nil {
				logger.WarnCF("forge", "Skill refinement failed", map[string]interface{}{
					"attempt": attempt + 1,
					"error":   refineErr.Error(),
				})
				break // can't refine further
			}
			content = refinedContent
		}
	}

	action.Status = "failed"
	action.ErrorMsg = fmt.Sprintf("Skill validation failed after %d refinement rounds", maxRefine)
}

// executeSuggestPrompt writes a prompt suggestion to workspace/prompts/.
func (le *LearningEngine) executeSuggestPrompt(action *LearningAction) {
	promptsDir := filepath.Join(filepath.Dir(le.forgeDir), "prompts")
	if err := os.MkdirAll(promptsDir, 0755); err != nil {
		action.Status = "failed"
		action.ErrorMsg = fmt.Sprintf("Failed to create prompts dir: %v", err)
		return
	}

	// Generate a safe filename from the draft name
	filename := strings.ReplaceAll(action.DraftName, "→", "-")
	filename = strings.ReplaceAll(filename, " ", "-")
	filename = strings.ToLower(filename)
	if len(filename) > 60 {
		filename = filename[:60]
	}

	content := fmt.Sprintf("# Prompt Suggestion: %s\n\n", action.DraftName)
	content += fmt.Sprintf("## Rationale\n%s\n\n", action.Rationale)
	content += fmt.Sprintf("## Pattern Description\n%s\n\n", action.Description)
	content += fmt.Sprintf("## Confidence\n%.2f\n\n", action.Confidence)
	content += fmt.Sprintf("## Suggested Action\nConsider creating a Skill or improving the workflow for this pattern.\n")
	content += fmt.Sprintf("Generated: %s\n", time.Now().UTC().Format(time.RFC3339))

	path := filepath.Join(promptsDir, filename+"_suggestion.md")
	if err := os.WriteFile(path, []byte(content), 0644); err != nil {
		action.Status = "failed"
		action.ErrorMsg = fmt.Sprintf("Failed to write suggestion: %v", err)
		return
	}

	action.Status = "executed"
	now := time.Now().UTC()
	action.ExecutedAt = &now
	action.ArtifactID = path // store path for tracking
}

// checkSuggestionAdoption checks if previously suggested prompts have been adopted.
func (le *LearningEngine) checkSuggestionAdoption(patterns []*ConversationPattern) {
	promptsDir := filepath.Join(filepath.Dir(le.forgeDir), "prompts")
	entries, err := os.ReadDir(promptsDir)
	if err != nil {
		return
	}

	for _, entry := range entries {
		if entry.IsDir() || !strings.HasSuffix(entry.Name(), "_suggestion.md") {
			continue
		}
		// Check if any pattern matches this suggestion with improved confidence
		for _, p := range patterns {
			if strings.Contains(entry.Name(), p.ToolChain) ||
				strings.Contains(p.ToolChain, strings.ReplaceAll(entry.Name(), "_suggestion.md", "")) {
				// Pattern still exists — if confidence is higher than threshold, mark adopted
				if p.Confidence >= le.config.Learning.HighConfThreshold {
					os.Remove(filepath.Join(promptsDir, entry.Name()))
				}
			}
		}
	}
}

// generateSkillDraft uses LLM to generate a SKILL.md content.
func (le *LearningEngine) generateSkillDraft(ctx context.Context, action *LearningAction) (string, error) {
	prompt := fmt.Sprintf(`Generate a complete SKILL.md for a Forge self-learning Skill with the following specification:

Name: %s
Description: %s
Rationale: %s

The SKILL.md must have YAML frontmatter between --- markers with these fields:
- name: skill name
- description: what the skill does
- version: "1.0"

Then provide the skill instructions in Markdown. The skill should define clear steps that an AI agent can follow.
Focus on the tool usage pattern identified. Keep it concise and actionable.`, action.DraftName, action.Description, action.Rationale)

	messages := []providers.Message{
		{Role: "system", Content: "You are a Skill definition generator. Generate valid SKILL.md content with YAML frontmatter."},
		{Role: "user", Content: prompt},
	}

	model := le.provider.GetDefaultModel()
	resp, err := le.provider.Chat(ctx, messages, nil, model, map[string]interface{}{
		"max_tokens": le.config.Learning.LLMBudgetTokens,
	})
	if err != nil {
		return "", err
	}
	return resp.Content, nil
}

// refineSkillDraft uses LLM to refine a failed Skill draft based on validation diagnosis.
func (le *LearningEngine) refineSkillDraft(ctx context.Context, action *LearningAction, previousContent string, diagnosis string) (string, error) {
	prompt := fmt.Sprintf(`The following Skill draft failed validation. Please fix it based on the diagnosis.

Skill Name: %s
Original Description: %s

Previous Content:
%s

Validation Diagnosis:
%s

Please generate a corrected, complete SKILL.md with YAML frontmatter (--- markers). Fix ALL issues identified in the diagnosis.`,
		action.DraftName, action.Description, previousContent, diagnosis)

	messages := []providers.Message{
		{Role: "system", Content: "You are a Skill definition generator. Fix the failing Skill and return a complete corrected SKILL.md."},
		{Role: "user", Content: prompt},
	}

	model := le.provider.GetDefaultModel()
	resp, err := le.provider.Chat(ctx, messages, nil, model, map[string]interface{}{
		"max_tokens": le.config.Learning.LLMBudgetTokens,
	})
	if err != nil {
		return "", err
	}
	return resp.Content, nil
}

// findArtifactByFingerprint checks if an artifact with a matching name already exists.
func (le *LearningEngine) findArtifactByFingerprint(name string) *Artifact {
	artifacts := le.registry.ListAll()
	for i := range artifacts {
		if artifacts[i].Name == name && artifacts[i].Status != StatusDeprecated {
			return &artifacts[i]
		}
	}
	return nil
}

// GetLatestCycle returns the most recent learning cycle.
func (le *LearningEngine) GetLatestCycle() *LearningCycle {
	if le.cycleStore == nil {
		return nil
	}
	cycles, err := le.cycleStore.ReadCycles(time.Now().UTC().AddDate(0, 0, -30))
	if err != nil || len(cycles) == 0 {
		return nil
	}
	return cycles[len(cycles)-1]
}

// --- Helpers ---

func generateSkillName(toolChain string) string {
	// Convert "read→edit→exec" to "read-edit-exec-workflow"
	name := strings.ReplaceAll(toolChain, "→", "-")
	name = strings.ReplaceAll(name, "_", "-")
	name = strings.ToLower(name)
	if len(name) > 50 {
		name = name[:50]
	}
	return name + "-workflow"
}

func extractToolSignatureFromChain(description string) []string {
	// Extract tool names from chain description like "read→edit→exec"
	// Look for arrow-separated names
	parts := strings.Split(description, "→")
	var tools []string
	for _, p := range parts {
		p = strings.TrimSpace(p)
		// Remove leading text like "Tool chain: "
		if idx := strings.LastIndex(p, " "); idx >= 0 {
			p = p[idx+1:]
		}
		if p != "" {
			tools = append(tools, p)
		}
	}
	return tools
}

func buildDiagnosis(validation *ArtifactValidation) string {
	var sb strings.Builder
	if validation.Stage1Static != nil && !validation.Stage1Static.Passed {
		sb.WriteString("Stage 1 (Static) FAILED:\n")
		for _, e := range validation.Stage1Static.Errors {
			sb.WriteString(fmt.Sprintf("  - %s\n", e))
		}
	}
	if validation.Stage2Functional != nil && !validation.Stage2Functional.Passed {
		sb.WriteString("Stage 2 (Functional) FAILED:\n")
		for _, e := range validation.Stage2Functional.Errors {
			sb.WriteString(fmt.Sprintf("  - %s\n", e))
		}
	}
	if validation.Stage3Quality != nil {
		sb.WriteString(fmt.Sprintf("Stage 3 (Quality) Score: %d/100\n", validation.Stage3Quality.Score))
		if validation.Stage3Quality.Notes != "" {
			sb.WriteString(fmt.Sprintf("  Notes: %s\n", validation.Stage3Quality.Notes))
		}
		for dim, score := range validation.Stage3Quality.Dimensions {
			sb.WriteString(fmt.Sprintf("  %s: %d\n", dim, score))
		}
	}
	return sb.String()
}

func actionToSummary(a *LearningAction) ActionSummary {
	return ActionSummary{
		ID:         a.ID,
		Type:       string(a.Type),
		Priority:   a.Priority,
		Status:     a.Status,
		ArtifactID: a.ArtifactID,
	}
}

func sortActions(actions []*LearningAction) {
	priorityOrder := map[string]int{"high": 0, "medium": 1, "low": 2}
	sort.Slice(actions, func(i, j int) bool {
		pi := priorityOrder[actions[i].Priority]
		pj := priorityOrder[actions[j].Priority]
		if pi != pj {
			return pi < pj
		}
		return actions[i].Confidence > actions[j].Confidence
	})
}

// --- Test exports ---

// GenerateActionsForTest exposes generateActions for testing.
func (le *LearningEngine) GenerateActionsForTest(patterns []*ConversationPattern) []*LearningAction {
	return le.generateActions(patterns)
}

// ExecuteCreateSkillForTest exposes executeCreateSkill for testing.
func (le *LearningEngine) ExecuteCreateSkillForTest(ctx context.Context, action *LearningAction) {
	le.executeCreateSkill(ctx, action)
}

// AdjustConfidenceForTest exposes adjustConfidence for testing.
func (le *LearningEngine) AdjustConfidenceForTest(outcomes []*ActionOutcome) {
	le.adjustConfidence(outcomes)
}

// GenerateSkillNameForTest exposes generateSkillName for testing.
func GenerateSkillNameForTest(toolChain string) string {
	return generateSkillName(toolChain)
}

// BuildDiagnosisForTest exposes buildDiagnosis for testing.
func BuildDiagnosisForTest(validation *ArtifactValidation) string {
	return buildDiagnosis(validation)
}

// ExtractToolSignatureFromChainForTest exposes extractToolSignatureFromChain for testing.
func ExtractToolSignatureFromChainForTest(description string) []string {
	return extractToolSignatureFromChain(description)
}

// ExecuteSuggestPromptForTest exposes executeSuggestPrompt for testing.
func (le *LearningEngine) ExecuteSuggestPromptForTest(action *LearningAction) {
	le.executeSuggestPrompt(action)
}
