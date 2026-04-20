package forge_test

import (
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/forge"
)

// === Experience Store Tests ===

func TestNewExperienceStore(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	store := forge.NewExperienceStore(tmpDir, cfg)
	if store == nil {
		t.Fatal("NewExperienceStore returned nil")
	}
}

func TestAppendAndReadAggregated(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	store := forge.NewExperienceStore(tmpDir, cfg)

	now := time.Now().UTC()

	// Write several records
	records := []*forge.AggregatedExperience{
		{
			PatternHash:   "sha256:aaaa0001",
			ToolName:      "read_file",
			Count:         10,
			AvgDurationMs: 50,
			SuccessRate:   0.95,
			LastSeen:      now,
		},
		{
			PatternHash:   "sha256:aaaa0002",
			ToolName:      "edit_file",
			Count:         5,
			AvgDurationMs: 200,
			SuccessRate:   0.80,
			LastSeen:      now,
		},
	}

	for _, rec := range records {
		if err := store.AppendAggregated(rec); err != nil {
			t.Fatalf("AppendAggregated failed: %v", err)
		}
	}

	// Read back
	readBack, err := store.ReadAggregated(time.Time{})
	if err != nil {
		t.Fatalf("ReadAggregated failed: %v", err)
	}
	if len(readBack) != 2 {
		t.Errorf("Expected 2 records, got %d", len(readBack))
	}

	// Verify content
	found := false
	for _, rec := range readBack {
		if rec.PatternHash == "sha256:aaaa0001" {
			found = true
			if rec.ToolName != "read_file" {
				t.Errorf("Expected ToolName 'read_file', got '%s'", rec.ToolName)
			}
			if rec.Count != 10 {
				t.Errorf("Expected Count 10, got %d", rec.Count)
			}
		}
	}
	if !found {
		t.Error("Record sha256:aaaa0001 not found in read back")
	}
}

func TestReadAggregatedByDay(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	store := forge.NewExperienceStore(tmpDir, cfg)

	now := time.Now().UTC()
	store.AppendAggregated(&forge.AggregatedExperience{
		PatternHash: "sha256:daytest",
		ToolName:    "exec",
		Count:       3,
		LastSeen:    now,
	})

	grouped, err := store.ReadAggregatedByDay(time.Time{})
	if err != nil {
		t.Fatalf("ReadAggregatedByDay failed: %v", err)
	}

	today := now.Format("2006-01-02")
	if _, ok := grouped[today]; !ok {
		t.Errorf("Expected records for today (%s), got keys: %v", today, grouped)
	}
}

func TestGetTopPatterns(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	store := forge.NewExperienceStore(tmpDir, cfg)

	now := time.Now().UTC()

	// Write records with different counts
	patterns := []*forge.AggregatedExperience{
		{PatternHash: "sha256:top3", ToolName: "c", Count: 3, LastSeen: now},
		{PatternHash: "sha256:top1", ToolName: "a", Count: 15, LastSeen: now},
		{PatternHash: "sha256:top2", ToolName: "b", Count: 8, LastSeen: now},
		{PatternHash: "sha256:top4", ToolName: "d", Count: 1, LastSeen: now},
	}
	for _, p := range patterns {
		store.AppendAggregated(p)
	}

	top, err := store.GetTopPatterns(time.Time{}, 2)
	if err != nil {
		t.Fatalf("GetTopPatterns failed: %v", err)
	}
	if len(top) != 2 {
		t.Fatalf("Expected 2 top patterns, got %d", len(top))
	}
	if top[0].ToolName != "a" {
		t.Errorf("Expected top pattern to be 'a' (count 15), got '%s'", top[0].ToolName)
	}
	if top[1].ToolName != "b" {
		t.Errorf("Expected second pattern to be 'b' (count 8), got '%s'", top[1].ToolName)
	}
}

func TestGetStats(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	store := forge.NewExperienceStore(tmpDir, cfg)

	now := time.Now().UTC()
	store.AppendAggregated(&forge.AggregatedExperience{
		PatternHash: "sha256:stat1", ToolName: "a", Count: 10, LastSeen: now,
	})
	store.AppendAggregated(&forge.AggregatedExperience{
		PatternHash: "sha256:stat2", ToolName: "b", Count: 5, LastSeen: now,
	})

	totalRecords, uniquePatterns, err := store.GetStats()
	if err != nil {
		t.Fatalf("GetStats failed: %v", err)
	}
	if totalRecords != 15 {
		t.Errorf("Expected totalRecords 15, got %d", totalRecords)
	}
	if uniquePatterns != 2 {
		t.Errorf("Expected uniquePatterns 2, got %d", uniquePatterns)
	}
}

func TestCleanup(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	store := forge.NewExperienceStore(tmpDir, cfg)

	// The store's baseDir is tmpDir/experiences/, so create files there
	experiencesDir := filepath.Join(tmpDir, "experiences")

	// Create an old file (date 2025-01-15, clearly older than any 30-day window)
	oldMonthDir := filepath.Join(experiencesDir, "202501")
	os.MkdirAll(oldMonthDir, 0755)
	oldFile := filepath.Join(oldMonthDir, "20250115.jsonl")
	os.WriteFile(oldFile, []byte("{\"pattern_hash\":\"sha256:old\"}\n"), 0644)

	// Create today's file
	now := time.Now().UTC()
	todayMonthDir := filepath.Join(experiencesDir, now.Format("200601"))
	os.MkdirAll(todayMonthDir, 0755)
	todayFile := filepath.Join(todayMonthDir, now.Format("20060102")+".jsonl")
	os.WriteFile(todayFile, []byte("{\"pattern_hash\":\"sha256:today\"}\n"), 0644)

	// Verify both files exist before cleanup
	if _, err := os.Stat(oldFile); os.IsNotExist(err) {
		t.Fatal("Old file should exist before cleanup")
	}
	if _, err := os.Stat(todayFile); os.IsNotExist(err) {
		t.Fatal("Today's file should exist before cleanup")
	}

	// Cleanup files older than 30 days
	err := store.Cleanup(30)
	if err != nil {
		t.Fatalf("Cleanup failed: %v", err)
	}

	// Old file should be removed
	if _, err := os.Stat(oldFile); !os.IsNotExist(err) {
		t.Errorf("Old file should be cleaned up, err=%v", err)
	}
	// Today's file should remain
	if _, err := os.Stat(todayFile); os.IsNotExist(err) {
		t.Error("Today's file should not be cleaned up")
	}
}

func TestMaxExperiencesPerDay(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	cfg.Collection.MaxExperiencesPerDay = 3
	store := forge.NewExperienceStore(tmpDir, cfg)

	now := time.Now().UTC()
	for i := 0; i < 10; i++ {
		store.AppendAggregated(&forge.AggregatedExperience{
			PatternHash: "sha256:limit",
			ToolName:    "test",
			Count:       1,
			LastSeen:    now,
		})
	}

	// Should have been capped at 3
	readBack, _ := store.ReadAggregated(time.Time{})
	if len(readBack) > 3 {
		t.Errorf("Expected at most 3 records (daily limit), got %d", len(readBack))
	}
}

func TestReadAggregatedEmptyDir(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	store := forge.NewExperienceStore(tmpDir, cfg)

	records, err := store.ReadAggregated(time.Time{})
	if err != nil {
		t.Fatalf("ReadAggregated on empty dir should not error: %v", err)
	}
	if len(records) != 0 {
		t.Errorf("Expected 0 records, got %d", len(records))
	}
}

func TestGetTopPatternsMergeSameHash(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	store := forge.NewExperienceStore(tmpDir, cfg)

	now := time.Now().UTC()
	// Same pattern hash written twice
	store.AppendAggregated(&forge.AggregatedExperience{
		PatternHash: "sha256:merge", ToolName: "test", Count: 5, LastSeen: now,
	})
	store.AppendAggregated(&forge.AggregatedExperience{
		PatternHash: "sha256:merge", ToolName: "test", Count: 3, LastSeen: now,
	})

	top, err := store.GetTopPatterns(time.Time{}, 10)
	if err != nil {
		t.Fatalf("GetTopPatterns failed: %v", err)
	}
	if len(top) != 1 {
		t.Fatalf("Expected 1 merged pattern, got %d", len(top))
	}
	if top[0].Count != 8 {
		t.Errorf("Expected merged count 8, got %d", top[0].Count)
	}
}
