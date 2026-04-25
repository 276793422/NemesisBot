package forge

import (
	"os"
	"testing"
	"time"
)

// --- ExperienceStore tests ---

func TestExperienceStore_AppendAndRead(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewExperienceStore(tmpDir, cfg)

	now := time.Now().UTC()
	rec := &AggregatedExperience{
		PatternHash:   "sha256:abc123",
		ToolName:      "read_file",
		Count:         10,
		AvgDurationMs: 150,
		SuccessRate:   0.9,
		LastSeen:      now,
	}

	if err := store.AppendAggregated(rec); err != nil {
		t.Fatalf("AppendAggregated failed: %v", err)
	}

	records, err := store.ReadAggregated(time.Time{})
	if err != nil {
		t.Fatalf("ReadAggregated failed: %v", err)
	}
	if len(records) != 1 {
		t.Fatalf("Expected 1 record, got %d", len(records))
	}
	if records[0].PatternHash != "sha256:abc123" {
		t.Errorf("Expected hash 'sha256:abc123', got '%s'", records[0].PatternHash)
	}
	if records[0].Count != 10 {
		t.Errorf("Expected count 10, got %d", records[0].Count)
	}
	if records[0].ToolName != "read_file" {
		t.Errorf("Expected tool 'read_file', got '%s'", records[0].ToolName)
	}
}

func TestExperienceStore_ReadAggregatedByDay(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewExperienceStore(tmpDir, cfg)

	now := time.Now().UTC()
	store.AppendAggregated(&AggregatedExperience{PatternHash: "h1", ToolName: "tool1", Count: 5, LastSeen: now})
	store.AppendAggregated(&AggregatedExperience{PatternHash: "h2", ToolName: "tool2", Count: 3, LastSeen: now})

	grouped, err := store.ReadAggregatedByDay(time.Time{})
	if err != nil {
		t.Fatalf("ReadAggregatedByDay failed: %v", err)
	}
	if len(grouped) != 1 {
		t.Fatalf("Expected 1 day group, got %d", len(grouped))
	}

	day := now.Format("2006-01-02")
	if len(grouped[day]) != 2 {
		t.Errorf("Expected 2 records for day %s, got %d", day, len(grouped[day]))
	}
}

func TestExperienceStore_GetTopPatterns(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewExperienceStore(tmpDir, cfg)

	now := time.Now().UTC()
	patterns := []*AggregatedExperience{
		{PatternHash: "h1", ToolName: "tool1", Count: 50, LastSeen: now},
		{PatternHash: "h2", ToolName: "tool2", Count: 30, LastSeen: now},
		{PatternHash: "h3", ToolName: "tool3", Count: 10, LastSeen: now},
	}
	for _, p := range patterns {
		store.AppendAggregated(p)
	}

	top2, err := store.GetTopPatterns(time.Time{}, 2)
	if err != nil {
		t.Fatalf("GetTopPatterns failed: %v", err)
	}
	if len(top2) != 2 {
		t.Fatalf("Expected 2 top patterns, got %d", len(top2))
	}
	if top2[0].ToolName != "tool1" {
		t.Errorf("Expected top pattern 'tool1', got '%s'", top2[0].ToolName)
	}
	if top2[1].ToolName != "tool2" {
		t.Errorf("Expected second pattern 'tool2', got '%s'", top2[1].ToolName)
	}
}

func TestExperienceStore_GetTopPatterns_All(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewExperienceStore(tmpDir, cfg)

	now := time.Now().UTC()
	store.AppendAggregated(&AggregatedExperience{PatternHash: "h1", ToolName: "tool1", Count: 50, LastSeen: now})
	store.AppendAggregated(&AggregatedExperience{PatternHash: "h2", ToolName: "tool2", Count: 30, LastSeen: now})

	// topN=0 should return all
	all, _ := store.GetTopPatterns(time.Time{}, 0)
	if len(all) != 2 {
		t.Errorf("Expected 2 patterns with topN=0, got %d", len(all))
	}
}

func TestExperienceStore_Cleanup(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewExperienceStore(tmpDir, cfg)

	// Write old record by creating the file manually in an old month dir
	oldDir := tmpDir + "/experiences/202601"
	if err := os.MkdirAll(oldDir, 0755); err != nil {
		t.Fatal(err)
	}

	// Write current record
	now := time.Now().UTC()
	store.AppendAggregated(&AggregatedExperience{
		PatternHash: "current",
		ToolName:    "tool",
		Count:       1,
		LastSeen:    now,
	})

	// Cleanup files older than 30 days
	if err := store.Cleanup(30); err != nil {
		t.Fatalf("Cleanup failed: %v", err)
	}

	records, _ := store.ReadAggregated(time.Time{})
	for _, r := range records {
		if r.PatternHash == "old" {
			t.Error("Old record should have been cleaned up")
		}
	}
}

func TestExperienceStore_DailyLimit(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	cfg.Collection.MaxExperiencesPerDay = 3
	store := NewExperienceStore(tmpDir, cfg)

	for i := 0; i < 5; i++ {
		store.AppendAggregated(&AggregatedExperience{
			PatternHash: "h",
			ToolName:    "tool",
			Count:       1,
			LastSeen:    time.Now().UTC(),
		})
	}

	records, _ := store.ReadAggregated(time.Time{})
	if len(records) > 3 {
		t.Errorf("Should respect daily limit of 3, got %d records", len(records))
	}
}

func TestExperienceStore_GetStats(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewExperienceStore(tmpDir, cfg)

	now := time.Now().UTC()
	store.AppendAggregated(&AggregatedExperience{PatternHash: "h1", ToolName: "tool1", Count: 10, LastSeen: now})
	store.AppendAggregated(&AggregatedExperience{PatternHash: "h2", ToolName: "tool2", Count: 5, LastSeen: now})

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

func TestExperienceStore_fileNewerThan(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewExperienceStore(tmpDir, cfg)

	// Today's file should be newer than yesterday
	yesterday := time.Now().UTC().AddDate(0, 0, -1)
	todayFilename := time.Now().UTC().Format("20060102") + ".jsonl"
	if !store.fileNewerThan(todayFilename, yesterday) {
		t.Error("Today's file should be newer than yesterday")
	}

	// Old file
	now := time.Now().UTC()
	if store.fileNewerThan("20200101.jsonl", now) {
		t.Error("2020 file should not be newer than now")
	}

	// Invalid filename
	if !store.fileNewerThan("garbage.jsonl", now) {
		t.Error("Invalid filename should default to true")
	}
}

func TestExperienceStore_ReadAggregated_Since(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewExperienceStore(tmpDir, cfg)

	now := time.Now().UTC()
	store.AppendAggregated(&AggregatedExperience{
		PatternHash: "current",
		ToolName:    "tool",
		Count:       1,
		LastSeen:    now,
	})

	// Read since start of today - should find the record
	startOfToday := time.Date(now.Year(), now.Month(), now.Day(), 0, 0, 0, 0, time.UTC)
	records, _ := store.ReadAggregated(startOfToday)
	if len(records) != 1 {
		t.Errorf("Expected 1 record since today, got %d", len(records))
	}

	// Read since tomorrow - should find nothing
	tomorrow := now.AddDate(0, 0, 1)
	records2, _ := store.ReadAggregated(tomorrow)
	if len(records2) != 0 {
		t.Errorf("Expected 0 records since tomorrow, got %d", len(records2))
	}
}

func TestExperienceStore_GetTopPatterns_MergeByHash(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewExperienceStore(tmpDir, cfg)

	now := time.Now().UTC()
	store.AppendAggregated(&AggregatedExperience{PatternHash: "h1", ToolName: "tool1", Count: 50, SuccessRate: 0.9, AvgDurationMs: 100, LastSeen: now})
	store.AppendAggregated(&AggregatedExperience{PatternHash: "h1", ToolName: "tool1", Count: 30, SuccessRate: 0.8, AvgDurationMs: 200, LastSeen: now.Add(time.Hour)})

	top, _ := store.GetTopPatterns(time.Time{}, 0)
	if len(top) != 1 {
		t.Fatalf("Expected 1 merged pattern, got %d", len(top))
	}
	if top[0].Count != 80 {
		t.Errorf("Expected merged count 80, got %d", top[0].Count)
	}
}
