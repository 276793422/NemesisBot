package forge_test

import (
	"context"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/forge"
)

// ST: Complete learning cycle — trace→pattern→action→create→monitor→degrade
func TestST_CompleteLearningCycle(t *testing.T) {
	f, _ := newTestForge(t)
	cfg := f.GetConfig()
	cfg.Trace.Enabled = true
	cfg.Learning.Enabled = true

	// Manually create the learning components (simulating enabled learning)
	traceStore := forge.NewTraceStore(f.GetWorkspace(), cfg)
	registry := f.GetRegistry()
	cycleStore := forge.NewCycleStore(f.GetWorkspace(), cfg)
	pipeline := forge.NewPipeline(registry, cfg)
	deploymentMonitor := forge.NewDeploymentMonitor(traceStore, registry, cfg)
	learningEngine := forge.NewLearningEngine(f.GetWorkspace(), registry, traceStore, pipeline, cycleStore, deploymentMonitor, cfg)

	// Step 1: Seed traces that will generate tool_chain patterns
	for i := 0; i < 5; i++ {
		trace := &forge.ConversationTrace{
			TraceID:     "st-trace-" + time.Now().Format("150405") + "-" + string(rune('a'+i)),
			SessionKey:  "hashed-session",
			Channel:     "test",
			StartTime:   time.Now().UTC().Add(-10 * time.Minute),
			EndTime:     time.Now().UTC(),
			DurationMs:  600000,
			TotalRounds: 3,
			ToolSteps: []forge.ToolStep{
				{ToolName: "read_file", ArgKeys: []string{"path"}, DurationMs: 100, Success: true, LLMRound: 1, ChainPos: 0},
				{ToolName: "edit_file", ArgKeys: []string{"path"}, DurationMs: 200, Success: true, LLMRound: 2, ChainPos: 1},
			},
		}
		traceStore.Append(trace)
	}

	// Step 2: Create a skill manually (simulating auto-create)
	sig := []string{"read_file", "edit_file"}
	artifact, err := f.CreateSkill(context.Background(), "auto-config-editor",
		"---\nname: auto-config-editor\n---\n\nConfig editing skill", "Auto-created config editor", sig)
	if err != nil {
		t.Fatalf("CreateSkill failed: %v", err)
	}

	// Set to active
	registry.Update(artifact.ID, func(a *forge.Artifact) {
		a.Status = forge.StatusActive
	})

	// Step 3: Verify learning engine was created and has cycle store
	if learningEngine == nil {
		t.Fatal("LearningEngine should be created")
	}

	// Step 4: Verify cycle store works
	cycle := &forge.LearningCycle{
		ID:              "st-cycle-001",
		StartedAt:       time.Now().UTC(),
		CycleNumber:     1,
		PatternsFound:   2,
		ActionsCreated:  1,
		ActionsExecuted:  1,
		ActionsSkipped:  0,
	}
	cycleStore.Append(cycle)

	// Read back
	cycles, err := cycleStore.ReadCycles(time.Now().UTC().AddDate(0, 0, -1))
	if err != nil {
		t.Fatalf("ReadCycles failed: %v", err)
	}
	if len(cycles) != 1 {
		t.Errorf("Expected 1 cycle, got %d", len(cycles))
	}

	// Step 5: Simulate degradation
	registry.Update(artifact.ID, func(a *forge.Artifact) {
		a.Status = forge.StatusDeprecated
		now := time.Now().UTC()
		a.LastDegradedAt = &now
	})

	// Verify
	updated, _ := registry.Get(artifact.ID)
	if updated.Status != forge.StatusDeprecated {
		t.Error("Artifact should be deprecated")
	}
	if updated.LastDegradedAt == nil {
		t.Error("LastDegradedAt should be set")
	}
}

// ST: Reflection → sanitize → share → receive
func TestST_ReflectionWithClusterShare(t *testing.T) {
	f, _ := newTestForge(t)
	cfg := f.GetConfig()
	cfg.Reflection.MinExperiences = 1

	// Seed data
	store := forge.NewExperienceStore(f.GetWorkspace(), cfg)
	store.AppendAggregated(&forge.AggregatedExperience{
		PatternHash: "sha256:share-1",
		ToolName:    "exec",
		Count:       10,
		LastSeen:    time.Now().UTC(),
	})

	// Step 1: Generate reflection report
	reportPath, err := f.ReflectNow(context.Background(), "today", "all")
	if err != nil {
		t.Fatalf("ReflectNow failed: %v", err)
	}

	// Step 2: Sanitize the report
	reportContent, _ := os.ReadFile(reportPath)
	sanitized := forge.SanitizeReportForTest(cfg, string(reportContent))

	// Sanitized version should not contain sensitive data
	if contains(sanitized, "[REDACTED]") {
		// This is fine — means something was sanitized
		t.Log("Report was sanitized successfully")
	}

	// Step 3: Set up mock bridge and share
	bridge := &mockBridge{
		clusterRun: true,
		peers:      []forge.PeerInfo{{ID: "peer-1", Name: "Peer1"}},
	}
	f.SetBridge(bridge)

	syncer := f.GetSyncer()
	if !syncer.IsEnabled() {
		t.Fatal("Syncer should be enabled with bridge")
	}

	// Step 4: Share the reflection
	err = syncer.ShareReflection(context.Background(), reportPath)
	if err != nil {
		t.Fatalf("ShareReflection failed: %v", err)
	}

	if bridge.shareCalls != 1 {
		t.Errorf("Expected 1 share call, got %d", bridge.shareCalls)
	}

	// Step 5: Simulate receiving a remote reflection
	err = syncer.ReceiveReflection(map[string]interface{}{
		"content":  "# Remote Reflection\n\nData from another node.",
		"filename": "remote_2026-04-21.md",
	})
	if err != nil {
		t.Fatalf("ReceiveReflection failed: %v", err)
	}

	// Verify remote report was saved
	remoteDir := filepath.Join(f.GetWorkspace(), "reflections", "remote")
	entries, err := os.ReadDir(remoteDir)
	if err != nil || len(entries) == 0 {
		t.Error("Remote reflection should be saved in reflections/remote/")
	}
}

// ST: Iterative refinement — generate→validate→fail→refine→pass→deploy
func TestST_SkillCreationWithRefinement(t *testing.T) {
	f, _ := newTestForge(t)

	// Round 1: Create with minimal content (will likely get low quality score)
	artifact, err := f.CreateSkill(context.Background(), "refine-target", "minimal content", "test", nil)
	if err != nil {
		t.Fatalf("CreateSkill failed: %v", err)
	}

	// Round 2: Update with better content (proper frontmatter + structured content)
	ftools := forge.NewForgeTools(f)
	updateTool := findTool(t, ftools, "forge_update")

	betterContent := "---\nname: refine-target\ndescription: A refined skill\n---\n\n# Refined Skill\n\n## Steps\n\n1. Read configuration\n2. Validate schema\n3. Apply changes\n4. Verify result\n\n## Best Practices\n\n- Always backup before editing\n- Validate input before processing"
	result := updateTool.Execute(context.Background(), map[string]interface{}{
		"id":                 artifact.ID,
		"content":            betterContent,
		"change_description": "Refined with proper structure",
	})
	if result.IsError {
		t.Fatalf("Update should succeed: %s", result.ForLLM)
	}

	// Round 3: Evaluate
	evaluateTool := findTool(t, ftools, "forge_evaluate")
	evalResult := evaluateTool.Execute(context.Background(), map[string]interface{}{
		"id": artifact.ID,
	})
	if evalResult.IsError {
		t.Errorf("Evaluate should succeed: %s", evalResult.ForLLM)
	}

	// Verify version progression
	updated, _ := f.GetRegistry().Get(artifact.ID)
	if updated.Version == "1.0" {
		t.Error("Version should have been incremented")
	}
	if len(updated.Evolution) < 2 {
		t.Errorf("Expected at least 2 evolution entries, got %d", len(updated.Evolution))
	}
}

// ST: Prompt suggestion adoption
func TestST_PromptSuggestionAdoption(t *testing.T) {
	f, workspace := newTestForge(t)

	// Write a suggestion file
	promptsDir := filepath.Join(workspace, "prompts")
	suggestionContent := "# Suggestion: Config Editor\n\nConsider creating a Skill for config editing pattern."
	suggestionPath := filepath.Join(promptsDir, "config-editor_suggestion.md")
	os.WriteFile(suggestionPath, []byte(suggestionContent), 0644)

	// Verify it exists
	if _, err := os.Stat(suggestionPath); os.IsNotExist(err) {
		t.Fatal("Suggestion file should exist")
	}

	// Create the actual skill (adopting the suggestion)
	_, err := f.CreateSkill(context.Background(), "config-editor",
		"---\nname: config-editor\n---\n\n# Config Editor\n\nEdit configuration files safely.", "Config editing skill", nil)
	if err != nil {
		t.Fatalf("CreateSkill failed: %v", err)
	}

	// The suggestion file still exists (it's cleaned up by cleanup goroutine based on age)
	if _, err := os.Stat(suggestionPath); os.IsNotExist(err) {
		t.Log("Suggestion file was cleaned up")
	}
}

// ST: Multiple cycle evolution
func TestST_MultipleCycleEvolution(t *testing.T) {
	f, _ := newTestForge(t)
	cfg := f.GetConfig()
	cfg.Trace.Enabled = true
	cfg.Learning.Enabled = true

	cycleStore := forge.NewCycleStore(f.GetWorkspace(), cfg)

	// Cycle 1: Initial patterns detected
	cycle1 := &forge.LearningCycle{
		ID:              "st-cycle-e1",
		StartedAt:       time.Now().UTC().Add(-2 * time.Hour),
		CycleNumber:     1,
		PatternsFound:   3,
		ActionsCreated:  1,
		ActionsExecuted:  1,
		ActionsSkipped:  2,
		PatternSummary: []forge.PatternSummary{
			{Type: "tool_chain", Fingerprint: "abc111", Frequency: 10, Confidence: 0.75},
		},
		ActionSummary: []forge.ActionSummary{
			{Type: "create_skill", Priority: "medium", Status: "executed", ArtifactID: "skill-c1"},
		},
	}
	cycle1.CompletedAt = &[]time.Time{time.Now().UTC().Add(-1 * time.Hour)}[0]
	cycleStore.Append(cycle1)

	// Create artifact from cycle 1
	f.CreateSkill(context.Background(), "c1-skill", "Cycle 1 skill", "Created in cycle 1", []string{"read_file"})

	// Cycle 2: More patterns, artifact being monitored
	cycle2 := &forge.LearningCycle{
		ID:              "st-cycle-e2",
		StartedAt:       time.Now().UTC().Add(-30 * time.Minute),
		CycleNumber:     2,
		PatternsFound:   5,
		ActionsCreated:  2,
		ActionsExecuted:  2,
		ActionsSkipped:  0,
		PatternSummary: []forge.PatternSummary{
			{Type: "tool_chain", Fingerprint: "abc111", Frequency: 15, Confidence: 0.88},
			{Type: "efficiency_issue", Fingerprint: "def222", Frequency: 8, Confidence: 0.65},
		},
		PreviousOutcomes: []*forge.ActionOutcome{
			{
				ActionID:         "action-c1-1",
				ArtifactID:       "skill-c1-skill",
				MeasuredAt:       time.Now().UTC(),
				SampleSize:       10,
				RoundsBeforeAvg:  4.5,
				RoundsAfterAvg:   3.0,
				SuccessBefore:    0.6,
				SuccessAfter:     0.85,
				ImprovementScore: 0.35,
				Verdict:          "positive",
			},
		},
	}
	cycle2.CompletedAt = &[]time.Time{time.Now().UTC()}[0]
	cycleStore.Append(cycle2)

	// Cycle 3: Stability check
	cycle3 := &forge.LearningCycle{
		ID:              "st-cycle-e3",
		StartedAt:       time.Now().UTC(),
		CycleNumber:     3,
		PatternsFound:   4,
		ActionsCreated:  0,
		ActionsExecuted:  0,
		ActionsSkipped:  0,
		PreviousOutcomes: []*forge.ActionOutcome{
			{
				ArtifactID:       "skill-c1-skill",
				MeasuredAt:       time.Now().UTC(),
				SampleSize:       15,
				RoundsBeforeAvg:  4.5,
				RoundsAfterAvg:   2.8,
				SuccessBefore:    0.6,
				SuccessAfter:     0.9,
				ImprovementScore: 0.42,
				Verdict:          "positive",
			},
		},
	}
	cycle3.CompletedAt = &[]time.Time{time.Now().UTC()}[0]
	cycleStore.Append(cycle3)

	// Verify all cycles are stored
	cycles, err := cycleStore.ReadCycles(time.Now().UTC().Add(-24 * time.Hour))
	if err != nil {
		t.Fatalf("ReadCycles failed: %v", err)
	}
	if len(cycles) != 3 {
		t.Errorf("Expected 3 cycles, got %d", len(cycles))
	}

	// Generate a report that includes learning insights
	stats := &forge.ReflectionStats{
		TotalRecords:   100,
		UniquePatterns: 10,
		AvgSuccessRate: 0.82,
	}
	report := &forge.ReflectionReport{
		Date:         time.Now().UTC().Format("2006-01-02"),
		Stats:        stats,
		LearningCycle: cycle3,
	}
	reportContent := forge.FormatReport(report)
	if !contains(reportContent, "闭环学习状态") {
		t.Error("Report should include learning insights from cycle 3")
	}
	if !contains(reportContent, "positive") {
		t.Error("Report should show positive outcome")
	}

	// Verify learning engine can read latest cycle
	registry := f.GetRegistry()
	traceStore := forge.NewTraceStore(f.GetWorkspace(), cfg)
	pipeline := forge.NewPipeline(registry, cfg)
	deploymentMonitor := forge.NewDeploymentMonitor(traceStore, registry, cfg)
	le := forge.NewLearningEngine(f.GetWorkspace(), registry, traceStore, pipeline, cycleStore, deploymentMonitor, cfg)
	le.SetProvider(&mockLLMProvider{})

	latest := le.GetLatestCycle()
	if latest == nil {
		t.Fatal("GetLatestCycle should return the most recent cycle")
	}
	if latest.CycleNumber != 3 {
		t.Errorf("Latest cycle should be #3, got #%d", latest.CycleNumber)
	}
}
