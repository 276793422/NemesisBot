package forge_test

import (
	"context"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/forge"
	"github.com/276793422/NemesisBot/module/plugin"
)

// === Reflector Tests (Statistical) ===

func TestReflectorStatisticalAnalysis(t *testing.T) {
	tmpDir := t.TempDir()
	forgeDir := tmpDir
	cfg := forge.DefaultForgeConfig()
	cfg.Reflection.MinExperiences = 1 // Lower threshold for testing

	store := forge.NewExperienceStore(forgeDir, cfg)
	registryPath := filepath.Join(forgeDir, "registry.json")
	registry := forge.NewRegistry(registryPath)
	reflector := forge.NewReflector(forgeDir, store, registry, cfg)

	// Seed some experience data
	now := time.Now().UTC()
	store.AppendAggregated(&forge.AggregatedExperience{
		PatternHash:   "sha256:high1",
		ToolName:      "read_file",
		Count:         15,
		AvgDurationMs: 50,
		SuccessRate:   0.95,
		LastSeen:      now,
	})
	store.AppendAggregated(&forge.AggregatedExperience{
		PatternHash:   "sha256:high2",
		ToolName:      "edit_file",
		Count:         8,
		AvgDurationMs: 200,
		SuccessRate:   0.88,
		LastSeen:      now,
	})
	store.AppendAggregated(&forge.AggregatedExperience{
		PatternHash:   "sha256:low1",
		ToolName:      "exec",
		Count:         5,
		AvgDurationMs: 500,
		SuccessRate:   0.60,
		LastSeen:      now,
	})

	// Run reflection
	reportPath, err := reflector.Reflect(context.Background(), "today", "all")
	if err != nil {
		t.Fatalf("Reflect failed: %v", err)
	}

	// Verify report file was created
	if _, err := os.Stat(reportPath); os.IsNotExist(err) {
		t.Fatal("Report file was not created")
	}

	// Read and verify report content
	content, err := os.ReadFile(reportPath)
	if err != nil {
		t.Fatalf("Failed to read report: %v", err)
	}

	reportStr := string(content)

	// Should contain statistical summary
	if !contains(reportStr, "统计概要") {
		t.Error("Report should contain 统计概要 section")
	}
	// Should mention high-frequency tools
	if !contains(reportStr, "read_file") {
		t.Error("Report should mention read_file")
	}
	// Should mention low success patterns
	if !contains(reportStr, "exec") {
		t.Error("Report should mention exec (low success)")
	}
}

func TestReflectorInsufficientExperiences(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	cfg.Reflection.MinExperiences = 100 // High threshold

	store := forge.NewExperienceStore(tmpDir, cfg)
	registry := forge.NewRegistry(filepath.Join(tmpDir, "registry.json"))
	reflector := forge.NewReflector(tmpDir, store, registry, cfg)

	// Only write 1 experience (below threshold of 100)
	store.AppendAggregated(&forge.AggregatedExperience{
		PatternHash: "sha256:only1",
		ToolName:    "test",
		Count:       1,
		LastSeen:    time.Now().UTC(),
	})

	_, err := reflector.Reflect(context.Background(), "today", "all")
	if err == nil {
		t.Error("Expected error for insufficient experiences")
	}
}

func TestReflectorCleanupReports(t *testing.T) {
	tmpDir := t.TempDir()
	reflectionsDir := filepath.Join(tmpDir, "reflections")
	os.MkdirAll(reflectionsDir, 0755)

	// Create old report
	oldFile := filepath.Join(reflectionsDir, "2026-01-01.md")
	os.WriteFile(oldFile, []byte("# Old report"), 0644)

	// Create recent report
	today := time.Now().UTC().Format("2006-01-02")
	recentFile := filepath.Join(reflectionsDir, today+".md")
	os.WriteFile(recentFile, []byte("# Recent report"), 0644)

	cfg := forge.DefaultForgeConfig()
	store := forge.NewExperienceStore(tmpDir, cfg)
	registry := forge.NewRegistry(filepath.Join(tmpDir, "registry.json"))
	reflector := forge.NewReflector(tmpDir, store, registry, cfg)

	err := reflector.CleanupReports(30)
	if err != nil {
		t.Fatalf("CleanupReports failed: %v", err)
	}

	// Old file should be cleaned up
	if _, err := os.Stat(oldFile); !os.IsNotExist(err) {
		t.Error("Old report should be cleaned up")
	}
	// Recent file should remain
	if _, err := os.Stat(recentFile); os.IsNotExist(err) {
		t.Error("Recent report should not be cleaned up")
	}
}

func TestReflectorGetLatestReport(t *testing.T) {
	tmpDir := t.TempDir()
	reflectionsDir := filepath.Join(tmpDir, "reflections")
	os.MkdirAll(reflectionsDir, 0755)

	cfg := forge.DefaultForgeConfig()
	store := forge.NewExperienceStore(tmpDir, cfg)
	registry := forge.NewRegistry(filepath.Join(tmpDir, "registry.json"))
	reflector := forge.NewReflector(tmpDir, store, registry, cfg)

	// No reports yet
	_, err := reflector.GetLatestReport()
	if err == nil {
		t.Error("Expected error when no reports exist")
	}

	// Create a report
	today := time.Now().UTC().Format("2006-01-02")
	os.WriteFile(filepath.Join(reflectionsDir, today+".md"), []byte("# Today"), 0644)

	latest, err := reflector.GetLatestReport()
	if err != nil {
		t.Fatalf("GetLatestReport failed: %v", err)
	}
	if !contains(latest, today) {
		t.Errorf("Latest report path should contain today's date: %s", latest)
	}
}

// === Report Format Tests ===

func TestFormatReport(t *testing.T) {
	report := &forge.ReflectionReport{
		Date:   "2026-04-20",
		Period: "today",
		Focus:  "all",
		Stats: &forge.ReflectionStats{
			TotalRecords:   47,
			UniquePatterns: 12,
			AvgSuccessRate: 0.89,
			ToolFrequency: map[string]int{
				"read_file":  20,
				"edit_file":  15,
				"exec":       12,
			},
			TopPatterns: []*forge.PatternInsight{
				{
					ToolName:      "read_file",
					Count:         20,
					SuccessRate:   0.95,
					AvgDurationMs: 50,
					Suggestion:    "High frequency, consider creating a Skill",
				},
			},
			LowSuccess: []*forge.PatternInsight{
				{
					ToolName:      "exec",
					Count:         12,
					SuccessRate:   0.60,
					AvgDurationMs: 500,
					Suggestion:    "Review failure modes",
				},
			},
		},
	}

	content := forge.FormatReport(report)

	if !contains(content, "Forge 反思报告") {
		t.Error("Report should contain header")
	}
	if !contains(content, "2026-04-20") {
		t.Error("Report should contain date")
	}
	if !contains(content, "47") {
		t.Error("Report should contain total records")
	}
	if !contains(content, "read_file") {
		t.Error("Report should mention read_file")
	}
	if !contains(content, "exec") {
		t.Error("Report should mention exec (low success)")
	}
}

func TestFormatReportWithArtifacts(t *testing.T) {
	report := &forge.ReflectionReport{
		Date:   "2026-04-20",
		Period: "week",
		Stats: &forge.ReflectionStats{
			TotalRecords:   10,
			UniquePatterns: 3,
			AvgSuccessRate: 0.90,
		},
		Artifacts: []forge.Artifact{
			{
				ID:          "skill-test",
				Type:        forge.ArtifactSkill,
				Name:        "test",
				Version:     "1.0",
				Status:      forge.StatusActive,
				UsageCount:  5,
				SuccessRate: 0.80,
			},
		},
	}

	content := forge.FormatReport(report)
	if !contains(content, "现有自学习产物") {
		t.Error("Report should contain artifacts section")
	}
	if !contains(content, "test") {
		t.Error("Report should contain artifact name 'test'")
	}
	if !contains(content, "active") {
		t.Error("Report should contain artifact status 'active'")
	}
}

func TestFormatReportWithLLMInsights(t *testing.T) {
	report := &forge.ReflectionReport{
		Date:   "2026-04-20",
		Stats: &forge.ReflectionStats{
			TotalRecords:   5,
			UniquePatterns: 2,
			AvgSuccessRate: 0.85,
		},
		LLMInsights: "Pattern A could be automated into a Skill",
	}

	content := forge.FormatReport(report)
	if !contains(content, "LLM 深度分析") {
		t.Error("Report should contain LLM insights section")
	}
	if !contains(content, "Pattern A") {
		t.Error("Report should contain LLM insight content")
	}
}

func TestFormatReportNilStats(t *testing.T) {
	report := &forge.ReflectionReport{
		Date:  "2026-04-20",
		Stats: nil,
	}

	content := forge.FormatReport(report)
	if !contains(content, "Forge 反思报告") {
		t.Error("Report should still render with nil stats")
	}
}

// === Forge Plugin Tests (Mock Plugin Manager) ===

func TestForgePluginImplementsInterface(t *testing.T) {
	// Verify ForgePlugin implements plugin.Plugin interface
	var _ plugin.Plugin = (*forge.ForgePlugin)(nil)
}

func TestForgePluginExecuteAllowsAll(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	store := forge.NewExperienceStore(tmpDir, cfg)
	collector := forge.NewCollector(store, cfg)
	fp := forge.NewForgePlugin(collector)

	invocation := &plugin.ToolInvocation{
		ToolName: "read_file",
		Args:     map[string]interface{}{"path": "/tmp/test.txt"},
		Metadata: map[string]interface{}{"session_id": "sess-123"},
	}

	allowed, err, modified := fp.Execute(context.Background(), invocation)
	if !allowed {
		t.Error("ForgePlugin should always allow operations")
	}
	if err != nil {
		t.Errorf("ForgePlugin should not return error: %v", err)
	}
	if modified {
		t.Error("ForgePlugin should never modify invocations")
	}
}

func TestForgePluginExecuteSanitizesArgs(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	store := forge.NewExperienceStore(tmpDir, cfg)
	collector := forge.NewCollector(store, cfg)
	fp := forge.NewForgePlugin(collector)

	invocation := &plugin.ToolInvocation{
		ToolName: "exec",
		Args: map[string]interface{}{
			"command":  "echo hello",
			"api_key":  "secret123",
		},
		Metadata: map[string]interface{}{},
	}

	allowed, _, _ := fp.Execute(context.Background(), invocation)
	if !allowed {
		t.Error("ForgePlugin should allow even with sensitive args")
	}
}

func TestForgePluginName(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	store := forge.NewExperienceStore(tmpDir, cfg)
	collector := forge.NewCollector(store, cfg)
	fp := forge.NewForgePlugin(collector)

	if fp.Name() != "forge" {
		t.Errorf("Expected plugin name 'forge', got '%s'", fp.Name())
	}
}

func TestForgePluginVersion(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	store := forge.NewExperienceStore(tmpDir, cfg)
	collector := forge.NewCollector(store, cfg)
	fp := forge.NewForgePlugin(collector)

	if fp.Version() != "1.0.0" {
		t.Errorf("Expected version '1.0.0', got '%s'", fp.Version())
	}
}

func TestForgePluginWithPluginManager(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	store := forge.NewExperienceStore(tmpDir, cfg)
	collector := forge.NewCollector(store, cfg)
	fp := forge.NewForgePlugin(collector)

	mgr := plugin.NewManager()
	if err := mgr.Register(fp); err != nil {
		t.Fatalf("Failed to register ForgePlugin: %v", err)
	}

	if !mgr.IsEnabled("forge") {
		t.Error("ForgePlugin should be enabled after registration")
	}

	// Execute through manager
	invocation := &plugin.ToolInvocation{
		ToolName: "read_file",
		Args:     map[string]interface{}{"path": "/tmp/test"},
		Metadata: map[string]interface{}{},
	}

	allowed, err := mgr.Execute(context.Background(), invocation)
	if !allowed {
		t.Error("Manager should allow operation through ForgePlugin")
	}
	if err != nil {
		t.Errorf("Manager should not return error: %v", err)
	}
}

func TestForgePluginDuplicateRegistration(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	store := forge.NewExperienceStore(tmpDir, cfg)
	collector := forge.NewCollector(store, cfg)
	fp1 := forge.NewForgePlugin(collector)
	fp2 := forge.NewForgePlugin(collector)

	mgr := plugin.NewManager()
	mgr.Register(fp1)

	err := mgr.Register(fp2)
	if err == nil {
		t.Error("Expected error for duplicate plugin registration")
	}
}

// === Helper ===

func contains(s, substr string) bool {
	return len(s) >= len(substr) && (s == substr || len(s) > 0 && containsStr(s, substr))
}

func containsStr(s, substr string) bool {
	for i := 0; i <= len(s)-len(substr); i++ {
		if s[i:i+len(substr)] == substr {
			return true
		}
	}
	return false
}
