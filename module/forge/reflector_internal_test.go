package forge

import (
	"testing"
	"time"
)

// --- resolvePeriod tests ---

func TestReflector_ResolvePeriod_Today(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewExperienceStore(tmpDir, cfg)
	registry := NewRegistry(tmpDir + "/registry.json")
	r := NewReflector(tmpDir, store, registry, cfg)

	result := r.resolvePeriod("today")
	now := time.Now().UTC()
	expected := time.Date(now.Year(), now.Month(), now.Day(), 0, 0, 0, 0, time.UTC)
	if !result.Equal(expected) {
		t.Errorf("resolvePeriod('today') = %v, expected %v", result, expected)
	}
}

func TestReflector_ResolvePeriod_Week(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewExperienceStore(tmpDir, cfg)
	registry := NewRegistry(tmpDir + "/registry.json")
	r := NewReflector(tmpDir, store, registry, cfg)

	result := r.resolvePeriod("week")
	now := time.Now().UTC()
	expected := now.AddDate(0, 0, -7)
	// Allow 1 second tolerance
	diff := result.Sub(expected)
	if diff < -time.Second || diff > time.Second {
		t.Errorf("resolvePeriod('week') = %v, expected approx %v", result, expected)
	}
}

func TestReflector_ResolvePeriod_All(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewExperienceStore(tmpDir, cfg)
	registry := NewRegistry(tmpDir + "/registry.json")
	r := NewReflector(tmpDir, store, registry, cfg)

	result := r.resolvePeriod("all")
	if !result.IsZero() {
		t.Errorf("resolvePeriod('all') should be zero time, got %v", result)
	}
}

func TestReflector_ResolvePeriod_Default(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewExperienceStore(tmpDir, cfg)
	registry := NewRegistry(tmpDir + "/registry.json")
	r := NewReflector(tmpDir, store, registry, cfg)

	result := r.resolvePeriod("unknown")
	today := r.resolvePeriod("today")
	if !result.Equal(today) {
		t.Errorf("resolvePeriod('unknown') should default to today, got %v", result)
	}
}

// --- generateSuggestion tests ---

func TestReflector_GenerateSuggestion_HighFrequency(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewExperienceStore(tmpDir, cfg)
	registry := NewRegistry(tmpDir + "/registry.json")
	r := NewReflector(tmpDir, store, registry, cfg)

	rec := &AggregatedExperience{
		ToolName:    "read_file",
		Count:       10,
		SuccessRate: 0.95,
	}
	suggestion := r.generateSuggestion(rec)
	if suggestion == "" {
		t.Error("Suggestion should not be empty")
	}
}

func TestReflector_GenerateSuggestion_StablePattern(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewExperienceStore(tmpDir, cfg)
	registry := NewRegistry(tmpDir + "/registry.json")
	r := NewReflector(tmpDir, store, registry, cfg)

	rec := &AggregatedExperience{
		ToolName:    "edit_file",
		Count:       4,
		SuccessRate: 0.8,
	}
	suggestion := r.generateSuggestion(rec)
	if suggestion == "" {
		t.Error("Suggestion should not be empty")
	}
}

func TestReflector_GenerateSuggestion_LowSuccess(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewExperienceStore(tmpDir, cfg)
	registry := NewRegistry(tmpDir + "/registry.json")
	r := NewReflector(tmpDir, store, registry, cfg)

	rec := &AggregatedExperience{
		ToolName:    "exec",
		Count:       5,
		SuccessRate: 0.5,
	}
	suggestion := r.generateSuggestion(rec)
	if suggestion == "" {
		t.Error("Suggestion should not be empty")
	}
}

func TestReflector_GenerateSuggestion_Normal(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewExperienceStore(tmpDir, cfg)
	registry := NewRegistry(tmpDir + "/registry.json")
	r := NewReflector(tmpDir, store, registry, cfg)

	rec := &AggregatedExperience{
		ToolName:    "tool",
		Count:       2,
		SuccessRate: 0.85,
	}
	suggestion := r.generateSuggestion(rec)
	if suggestion == "" {
		t.Error("Suggestion should not be empty")
	}
}

// --- statisticalAnalysis tests ---

func TestReflector_StatisticalAnalysis(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewExperienceStore(tmpDir, cfg)
	registry := NewRegistry(tmpDir + "/registry.json")
	r := NewReflector(tmpDir, store, registry, cfg)

	records := []*AggregatedExperience{
		{PatternHash: "h1", ToolName: "read_file", Count: 20, SuccessRate: 0.95, AvgDurationMs: 100},
		{PatternHash: "h2", ToolName: "edit_file", Count: 10, SuccessRate: 0.80, AvgDurationMs: 200},
		{PatternHash: "h3", ToolName: "exec", Count: 5, SuccessRate: 0.50, AvgDurationMs: 500},
	}

	stats := r.statisticalAnalysis(records)

	if stats.TotalRecords != 35 {
		t.Errorf("Expected TotalRecords 35, got %d", stats.TotalRecords)
	}
	if stats.UniquePatterns != 3 {
		t.Errorf("Expected UniquePatterns 3, got %d", stats.UniquePatterns)
	}
	// Weighted average: (0.95*20 + 0.80*10 + 0.50*5) / 35
	expectedRate := (0.95*20 + 0.80*10 + 0.50*5) / 35
	if stats.AvgSuccessRate < expectedRate-0.01 || stats.AvgSuccessRate > expectedRate+0.01 {
		t.Errorf("Expected AvgSuccessRate ~%.3f, got %.3f", expectedRate, stats.AvgSuccessRate)
	}
	if len(stats.TopPatterns) != 3 {
		t.Errorf("Expected 3 top patterns, got %d", len(stats.TopPatterns))
	}
	if stats.TopPatterns[0].ToolName != "read_file" {
		t.Errorf("Top pattern should be read_file (highest count), got %s", stats.TopPatterns[0].ToolName)
	}
}

func TestReflector_StatisticalAnalysis_LowSuccess(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewExperienceStore(tmpDir, cfg)
	registry := NewRegistry(tmpDir + "/registry.json")
	r := NewReflector(tmpDir, store, registry, cfg)

	records := []*AggregatedExperience{
		{PatternHash: "h1", ToolName: "exec", Count: 5, SuccessRate: 0.5, AvgDurationMs: 500},
	}

	stats := r.statisticalAnalysis(records)

	if len(stats.LowSuccess) != 1 {
		t.Fatalf("Expected 1 low success pattern, got %d", len(stats.LowSuccess))
	}
	if stats.LowSuccess[0].ToolName != "exec" {
		t.Errorf("Low success tool should be exec, got %s", stats.LowSuccess[0].ToolName)
	}
}

func TestReflector_StatisticalAnalysis_Empty(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewExperienceStore(tmpDir, cfg)
	registry := NewRegistry(tmpDir + "/registry.json")
	r := NewReflector(tmpDir, store, registry, cfg)

	stats := r.statisticalAnalysis(nil)

	if stats.TotalRecords != 0 {
		t.Errorf("Expected TotalRecords 0, got %d", stats.TotalRecords)
	}
	if stats.AvgSuccessRate != 0 {
		t.Errorf("Expected AvgSuccessRate 0, got %f", stats.AvgSuccessRate)
	}
}

// --- extractToolChain tests ---

func TestExtractToolChain(t *testing.T) {
	steps := []ToolStep{
		{ToolName: "read_file"},
		{ToolName: "edit_file"},
		{ToolName: "exec"},
	}
	chain := extractToolChain(steps)
	if chain != "read_file→edit_file→exec" {
		t.Errorf("Expected 'read_file→edit_file→exec', got '%s'", chain)
	}
}

func TestExtractToolChain_Empty(t *testing.T) {
	chain := extractToolChain(nil)
	if chain != "" {
		t.Errorf("Expected empty chain, got '%s'", chain)
	}
}

func TestExtractToolChain_Single(t *testing.T) {
	steps := []ToolStep{{ToolName: "read_file"}}
	chain := extractToolChain(steps)
	if chain != "read_file" {
		t.Errorf("Expected 'read_file', got '%s'", chain)
	}
}
