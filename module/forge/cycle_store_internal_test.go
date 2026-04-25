package forge

import (
	"fmt"
	"testing"
	"time"
)

// --- CycleStore tests ---

func TestNewCycleStore(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewCycleStore(tmpDir, cfg)
	if store == nil {
		t.Fatal("NewCycleStore returned nil")
	}
}

func TestCycleStore_AppendAndRead(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewCycleStore(tmpDir, cfg)

	now := time.Now().UTC()
	cycle := &LearningCycle{
		ID:              "lc-test-1",
		StartedAt:       now,
		CycleNumber:     1,
		PatternsFound:   3,
		ActionsCreated:  2,
		ActionsExecuted: 1,
	}

	if err := store.Append(cycle); err != nil {
		t.Fatalf("Append failed: %v", err)
	}

	// Read back
	cycles, err := store.ReadCycles(time.Time{})
	if err != nil {
		t.Fatalf("ReadCycles failed: %v", err)
	}
	if len(cycles) != 1 {
		t.Fatalf("Expected 1 cycle, got %d", len(cycles))
	}
	if cycles[0].ID != "lc-test-1" {
		t.Errorf("Expected ID 'lc-test-1', got '%s'", cycles[0].ID)
	}
	if cycles[0].PatternsFound != 3 {
		t.Errorf("Expected PatternsFound 3, got %d", cycles[0].PatternsFound)
	}
}

func TestCycleStore_MultipleAppends(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewCycleStore(tmpDir, cfg)

	now := time.Now().UTC()
	for i := 0; i < 5; i++ {
		cycle := &LearningCycle{
			ID:            fmt.Sprintf("lc-%d", i),
			StartedAt:     now,
			CycleNumber:   i + 1,
			PatternsFound: i,
		}
		if err := store.Append(cycle); err != nil {
			t.Fatalf("Append %d failed: %v", i, err)
		}
	}

	cycles, err := store.ReadCycles(time.Time{})
	if err != nil {
		t.Fatalf("ReadCycles failed: %v", err)
	}
	if len(cycles) != 5 {
		t.Errorf("Expected 5 cycles, got %d", len(cycles))
	}
}

func TestCycleStore_ReadCycles_Since(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewCycleStore(tmpDir, cfg)

	// Write a cycle with today's date
	now := time.Now().UTC()
	cycle := &LearningCycle{
		ID:        "lc-today",
		StartedAt: now,
	}
	store.Append(cycle)

	// Read with since=today
	sinceToday := time.Date(now.Year(), now.Month(), now.Day(), 0, 0, 0, 0, time.UTC)
	cycles, err := store.ReadCycles(sinceToday)
	if err != nil {
		t.Fatalf("ReadCycles failed: %v", err)
	}
	if len(cycles) != 1 {
		t.Errorf("Expected 1 cycle since today, got %d", len(cycles))
	}

	// Read with since=tomorrow (should return 0)
	tomorrow := now.AddDate(0, 0, 1)
	cycles2, _ := store.ReadCycles(tomorrow)
	if len(cycles2) != 0 {
		t.Errorf("Expected 0 cycles since tomorrow, got %d", len(cycles2))
	}
}

func TestCycleStore_LoadLatestCycle(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewCycleStore(tmpDir, cfg)

	// No cycles yet
	_, err := store.LoadLatestCycle()
	if err == nil {
		t.Error("Expected error when no cycles exist")
	}

	now := time.Now().UTC()
	cycle1 := &LearningCycle{ID: "lc-1", StartedAt: now, CycleNumber: 1}
	cycle2 := &LearningCycle{ID: "lc-2", StartedAt: now, CycleNumber: 2}
	store.Append(cycle1)
	store.Append(cycle2)

	latest, err := store.LoadLatestCycle()
	if err != nil {
		t.Fatalf("LoadLatestCycle failed: %v", err)
	}
	if latest.ID != "lc-2" {
		t.Errorf("Expected latest cycle 'lc-2', got '%s'", latest.ID)
	}
}

func TestCycleStore_Cleanup(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewCycleStore(tmpDir, cfg)

	// Write a cycle with old date
	oldDate := time.Now().UTC().AddDate(0, 0, -60)
	oldCycle := &LearningCycle{
		ID:        "lc-old",
		StartedAt: oldDate,
	}
	store.Append(oldCycle)

	// Write a cycle with today's date
	newCycle := &LearningCycle{
		ID:        "lc-new",
		StartedAt: time.Now().UTC(),
	}
	store.Append(newCycle)

	// Cleanup files older than 30 days
	if err := store.Cleanup(30); err != nil {
		t.Fatalf("Cleanup failed: %v", err)
	}

	// Only the new cycle should remain
	cycles, _ := store.ReadCycles(time.Time{})
	if len(cycles) != 1 {
		t.Errorf("Expected 1 cycle after cleanup, got %d", len(cycles))
	}
	if len(cycles) > 0 && cycles[0].ID != "lc-new" {
		t.Errorf("Expected 'lc-new', got '%s'", cycles[0].ID)
	}
}

func TestCycleStore_fileNewerThan(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewCycleStore(tmpDir, cfg)

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

	// Unparseable filename
	if !store.fileNewerThan("garbage.jsonl", time.Time{}) {
		t.Error("Unparseable filename should default to true")
	}
}

func TestCycleStore_CycleWithPatternSummary(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewCycleStore(tmpDir, cfg)

	now := time.Now().UTC()
	completedAt := now.Add(time.Minute)
	cycle := &LearningCycle{
		ID:              "lc-summary",
		StartedAt:       now,
		CompletedAt:     &completedAt,
		CycleNumber:     1,
		PatternsFound:   2,
		ActionsCreated:  2,
		ActionsExecuted: 1,
		ActionsSkipped:  1,
		PatternSummary: []PatternSummary{
			{ID: "p1", Type: "tool_chain", Frequency: 10, Confidence: 0.9},
			{ID: "p2", Type: "error_recovery", Frequency: 5, Confidence: 0.8},
		},
		ActionSummary: []ActionSummary{
			{ID: "a1", Type: "create_skill", Priority: "high", Status: "executed"},
			{ID: "a2", Type: "suggest_prompt", Priority: "medium", Status: "skipped"},
		},
	}

	store.Append(cycle)

	cycles, _ := store.ReadCycles(time.Time{})
	if len(cycles) != 1 {
		t.Fatalf("Expected 1 cycle, got %d", len(cycles))
	}

	c := cycles[0]
	if len(c.PatternSummary) != 2 {
		t.Errorf("Expected 2 pattern summaries, got %d", len(c.PatternSummary))
	}
	if len(c.ActionSummary) != 2 {
		t.Errorf("Expected 2 action summaries, got %d", len(c.ActionSummary))
	}
	if c.CompletedAt == nil {
		t.Error("CompletedAt should not be nil")
	}
}
