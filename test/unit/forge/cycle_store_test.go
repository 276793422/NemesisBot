package forge_test

import (
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/forge"
)

func TestCycleStoreAppendAndRead(t *testing.T) {
	tmpDir := t.TempDir()
	forgeDir := filepath.Join(tmpDir, "forge")
	os.MkdirAll(forgeDir, 0755)

	store := forge.NewCycleStore(forgeDir, forge.DefaultForgeConfig())

	now := time.Now().UTC()
	cycle := &forge.LearningCycle{
		ID:              "lc-test-1",
		StartedAt:       now,
		CompletedAt:     &now,
		CycleNumber:     1,
		PatternsFound:   5,
		ActionsCreated:  3,
		ActionsExecuted: 2,
		ActionsSkipped:  1,
		PatternSummary: []forge.PatternSummary{
			{ID: "p1", Type: "tool_chain", Fingerprint: "abc123", Frequency: 10, Confidence: 0.95},
		},
		ActionSummary: []forge.ActionSummary{
			{ID: "a1", Type: "create_skill", Priority: "high", Status: "executed", ArtifactID: "skill-test"},
		},
	}

	if err := store.Append(cycle); err != nil {
		t.Fatalf("Append failed: %v", err)
	}

	cycles, err := store.ReadCycles(now.Add(-24 * time.Hour))
	if err != nil {
		t.Fatalf("ReadCycles failed: %v", err)
	}
	if len(cycles) != 1 {
		t.Fatalf("Expected 1 cycle, got %d", len(cycles))
	}
	if cycles[0].ID != "lc-test-1" {
		t.Errorf("Expected ID lc-test-1, got %s", cycles[0].ID)
	}
	if cycles[0].PatternsFound != 5 {
		t.Errorf("Expected PatternsFound=5, got %d", cycles[0].PatternsFound)
	}
	if len(cycles[0].PatternSummary) != 1 {
		t.Errorf("Expected 1 pattern summary, got %d", len(cycles[0].PatternSummary))
	}
	if len(cycles[0].ActionSummary) != 1 {
		t.Errorf("Expected 1 action summary, got %d", len(cycles[0].ActionSummary))
	}
}

func TestCycleStoreReadSince(t *testing.T) {
	tmpDir := t.TempDir()
	forgeDir := filepath.Join(tmpDir, "forge")
	os.MkdirAll(forgeDir, 0755)

	store := forge.NewCycleStore(forgeDir, forge.DefaultForgeConfig())

	oldTime := time.Now().UTC().AddDate(0, 0, -2)
	oldCycle := &forge.LearningCycle{
		ID:        "lc-old",
		StartedAt: oldTime,
	}
	store.Append(oldCycle)

	newTime := time.Now().UTC()
	newCycle := &forge.LearningCycle{
		ID:        "lc-new",
		StartedAt: newTime,
	}
	store.Append(newCycle)

	cycles, err := store.ReadCycles(time.Now().UTC().AddDate(0, 0, -1))
	if err != nil {
		t.Fatalf("ReadCycles failed: %v", err)
	}
	if len(cycles) != 1 {
		t.Fatalf("Expected 1 recent cycle, got %d", len(cycles))
	}
	if cycles[0].ID != "lc-new" {
		t.Errorf("Expected lc-new, got %s", cycles[0].ID)
	}
}

func TestCycleStoreCleanup(t *testing.T) {
	tmpDir := t.TempDir()
	forgeDir := filepath.Join(tmpDir, "forge")
	os.MkdirAll(forgeDir, 0755)

	store := forge.NewCycleStore(forgeDir, forge.DefaultForgeConfig())

	oldTime := time.Now().UTC().AddDate(0, 0, -10)
	oldCycle := &forge.LearningCycle{
		ID:        "lc-old",
		StartedAt: oldTime,
	}
	store.Append(oldCycle)

	newCycle := &forge.LearningCycle{
		ID:        "lc-new",
		StartedAt: time.Now().UTC(),
	}
	store.Append(newCycle)

	store.Cleanup(7)

	cycles, _ := store.ReadCycles(time.Time{})
	if len(cycles) != 1 {
		t.Fatalf("Expected 1 cycle after cleanup, got %d", len(cycles))
	}
	if cycles[0].ID != "lc-new" {
		t.Errorf("Expected lc-new after cleanup, got %s", cycles[0].ID)
	}
}

func TestCycleStoreLoadLatestCycle(t *testing.T) {
	tmpDir := t.TempDir()
	forgeDir := filepath.Join(tmpDir, "forge")
	os.MkdirAll(forgeDir, 0755)

	store := forge.NewCycleStore(forgeDir, forge.DefaultForgeConfig())

	_, err := store.LoadLatestCycle()
	if err == nil {
		t.Error("Expected error when no cycles exist")
	}

	for i := 0; i < 3; i++ {
		cycle := &forge.LearningCycle{
			ID:        "lc-" + string(rune('0'+i)),
			StartedAt: time.Now().UTC().Add(time.Duration(i) * time.Hour),
		}
		store.Append(cycle)
	}

	latest, err := store.LoadLatestCycle()
	if err != nil {
		t.Fatalf("LoadLatestCycle failed: %v", err)
	}
	if latest.ID != "lc-2" {
		t.Errorf("Expected latest cycle lc-2, got %s", latest.ID)
	}
}

func TestCycleStoreEmptyDirectory(t *testing.T) {
	tmpDir := t.TempDir()
	forgeDir := filepath.Join(tmpDir, "forge")
	os.MkdirAll(forgeDir, 0755)

	store := forge.NewCycleStore(forgeDir, forge.DefaultForgeConfig())

	cycles, err := store.ReadCycles(time.Time{})
	if err != nil {
		t.Fatalf("ReadCycles on empty store failed: %v", err)
	}
	if len(cycles) != 0 {
		t.Errorf("Expected 0 cycles from empty store, got %d", len(cycles))
	}
}
