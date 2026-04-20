package forge_test

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/276793422/NemesisBot/module/forge"
)

// === Registry Tests ===

func TestNewRegistry(t *testing.T) {
	tmpDir := t.TempDir()
	path := filepath.Join(tmpDir, "registry.json")

	r := forge.NewRegistry(path)
	if r == nil {
		t.Fatal("NewRegistry returned nil")
	}
}

func TestRegistryAddAndGet(t *testing.T) {
	tmpDir := t.TempDir()
	path := filepath.Join(tmpDir, "registry.json")
	r := forge.NewRegistry(path)

	artifact := forge.Artifact{
		ID:      "skill-test-skill",
		Type:    forge.ArtifactSkill,
		Name:    "test-skill",
		Version: "1.0",
		Status:  forge.StatusDraft,
		Path:    "/tmp/test.md",
	}

	if err := r.Add(artifact); err != nil {
		t.Fatalf("Add failed: %v", err)
	}

	// Verify retrieval
	got, found := r.Get("skill-test-skill")
	if !found {
		t.Fatal("Artifact not found after add")
	}
	if got.Name != "test-skill" {
		t.Errorf("Expected Name 'test-skill', got '%s'", got.Name)
	}
	if got.Version != "1.0" {
		t.Errorf("Expected Version '1.0', got '%s'", got.Version)
	}
	if got.Status != forge.StatusDraft {
		t.Errorf("Expected Status 'draft', got '%s'", got.Status)
	}
	if got.CreatedAt.IsZero() {
		t.Error("CreatedAt should be set automatically")
	}
}

func TestRegistryGetNonexistent(t *testing.T) {
	tmpDir := t.TempDir()
	path := filepath.Join(tmpDir, "registry.json")
	r := forge.NewRegistry(path)

	_, found := r.Get("nonexistent")
	if found {
		t.Error("Should not find nonexistent artifact")
	}
}

func TestRegistryUpdate(t *testing.T) {
	tmpDir := t.TempDir()
	path := filepath.Join(tmpDir, "registry.json")
	r := forge.NewRegistry(path)

	artifact := forge.Artifact{
		ID:      "skill-my-skill",
		Type:    forge.ArtifactSkill,
		Name:    "my-skill",
		Version: "1.0",
		Status:  forge.StatusDraft,
	}
	r.Add(artifact)

	// Update
	err := r.Update("skill-my-skill", func(a *forge.Artifact) {
		a.Version = "1.1"
		a.Status = forge.StatusActive
		a.UsageCount = 42
		a.SuccessRate = 0.95
	})
	if err != nil {
		t.Fatalf("Update failed: %v", err)
	}

	got, _ := r.Get("skill-my-skill")
	if got.Version != "1.1" {
		t.Errorf("Expected Version '1.1', got '%s'", got.Version)
	}
	if got.Status != forge.StatusActive {
		t.Errorf("Expected Status 'active', got '%s'", got.Status)
	}
	if got.UsageCount != 42 {
		t.Errorf("Expected UsageCount 42, got %d", got.UsageCount)
	}
	if got.SuccessRate != 0.95 {
		t.Errorf("Expected SuccessRate 0.95, got %f", got.SuccessRate)
	}
	if got.UpdatedAt.IsZero() {
		t.Error("UpdatedAt should be set on update")
	}
}

func TestRegistryList(t *testing.T) {
	tmpDir := t.TempDir()
	path := filepath.Join(tmpDir, "registry.json")
	r := forge.NewRegistry(path)

	// Add artifacts of different types
	r.Add(forge.Artifact{ID: "skill-a", Type: forge.ArtifactSkill, Name: "a", Status: forge.StatusDraft})
	r.Add(forge.Artifact{ID: "skill-b", Type: forge.ArtifactSkill, Name: "b", Status: forge.StatusActive})
	r.Add(forge.Artifact{ID: "script-c", Type: forge.ArtifactScript, Name: "c", Status: forge.StatusDraft})
	r.Add(forge.Artifact{ID: "mcp-d", Type: forge.ArtifactMCP, Name: "d", Status: forge.StatusActive})

	// List all
	all := r.ListAll()
	if len(all) != 4 {
		t.Errorf("Expected 4 artifacts, got %d", len(all))
	}

	// List by type
	skills := r.List(forge.ArtifactSkill, "")
	if len(skills) != 2 {
		t.Errorf("Expected 2 skills, got %d", len(skills))
	}

	scripts := r.List(forge.ArtifactScript, "")
	if len(scripts) != 1 {
		t.Errorf("Expected 1 script, got %d", len(scripts))
	}

	// List by status
	active := r.List("", forge.StatusActive)
	if len(active) != 2 {
		t.Errorf("Expected 2 active, got %d", len(active))
	}

	// List by type + status
	activeSkills := r.List(forge.ArtifactSkill, forge.StatusActive)
	if len(activeSkills) != 1 {
		t.Errorf("Expected 1 active skill, got %d", len(activeSkills))
	}
}

func TestRegistryCount(t *testing.T) {
	tmpDir := t.TempDir()
	path := filepath.Join(tmpDir, "registry.json")
	r := forge.NewRegistry(path)

	r.Add(forge.Artifact{ID: "skill-a", Type: forge.ArtifactSkill, Name: "a"})
	r.Add(forge.Artifact{ID: "skill-b", Type: forge.ArtifactSkill, Name: "b"})
	r.Add(forge.Artifact{ID: "script-c", Type: forge.ArtifactScript, Name: "c"})

	if r.Count("") != 3 {
		t.Errorf("Expected total count 3, got %d", r.Count(""))
	}
	if r.Count(forge.ArtifactSkill) != 2 {
		t.Errorf("Expected skill count 2, got %d", r.Count(forge.ArtifactSkill))
	}
	if r.Count(forge.ArtifactMCP) != 0 {
		t.Errorf("Expected MCP count 0, got %d", r.Count(forge.ArtifactMCP))
	}
}

func TestRegistryDelete(t *testing.T) {
	tmpDir := t.TempDir()
	path := filepath.Join(tmpDir, "registry.json")
	r := forge.NewRegistry(path)

	r.Add(forge.Artifact{ID: "skill-a", Type: forge.ArtifactSkill, Name: "a"})
	r.Add(forge.Artifact{ID: "skill-b", Type: forge.ArtifactSkill, Name: "b"})

	if err := r.Delete("skill-a"); err != nil {
		t.Fatalf("Delete failed: %v", err)
	}

	if _, found := r.Get("skill-a"); found {
		t.Error("Artifact should be deleted")
	}
	if r.Count("") != 1 {
		t.Errorf("Expected 1 artifact after delete, got %d", r.Count(""))
	}
}

func TestRegistryPersistence(t *testing.T) {
	tmpDir := t.TempDir()
	path := filepath.Join(tmpDir, "registry.json")

	// Create and add artifacts
	r := forge.NewRegistry(path)
	r.Add(forge.Artifact{ID: "skill-persist", Type: forge.ArtifactSkill, Name: "persist", Version: "1.0"})

	// Create new registry instance loading from same file
	r2 := forge.NewRegistry(path)
	got, found := r2.Get("skill-persist")
	if !found {
		t.Fatal("Artifact not found after reload from disk")
	}
	if got.Name != "persist" {
		t.Errorf("Expected Name 'persist', got '%s'", got.Name)
	}
}

func TestRegistryEvolution(t *testing.T) {
	tmpDir := t.TempDir()
	path := filepath.Join(tmpDir, "registry.json")
	r := forge.NewRegistry(path)

	r.Add(forge.Artifact{
		ID:      "skill-evo",
		Type:    forge.ArtifactSkill,
		Name:    "evo",
		Version: "1.0",
		Evolution: []forge.Evolution{
			{Version: "1.0", Change: "初始创建"},
		},
	})

	r.Update("skill-evo", func(a *forge.Artifact) {
		a.Version = "1.1"
		a.Evolution = append(a.Evolution, forge.Evolution{
			Version: "1.1",
			Change:  "支持 YAML",
		})
	})

	got, _ := r.Get("skill-evo")
	if len(got.Evolution) != 2 {
		t.Fatalf("Expected 2 evolution entries, got %d", len(got.Evolution))
	}
	if got.Evolution[1].Change != "支持 YAML" {
		t.Errorf("Expected evolution change '支持 YAML', got '%s'", got.Evolution[1].Change)
	}
}

func TestRegistryEmptyList(t *testing.T) {
	tmpDir := t.TempDir()
	path := filepath.Join(tmpDir, "registry.json")
	r := forge.NewRegistry(path)

	all := r.ListAll()
	if all == nil {
		t.Error("ListAll should return empty slice, not nil")
	}
	if len(all) != 0 {
		t.Errorf("Expected 0 artifacts, got %d", len(all))
	}
}

// === Registry handles missing file gracefully ===

func TestRegistryMissingFile(t *testing.T) {
	r := forge.NewRegistry("/nonexistent/path/registry.json")
	if r.Count("") != 0 {
		t.Error("Missing file should result in empty registry")
	}
}

// === Validation Field Tests ===

func TestRegistryValidationPersistence(t *testing.T) {
	tmpDir := t.TempDir()
	path := filepath.Join(tmpDir, "registry.json")
	r := forge.NewRegistry(path)

	artifact := forge.Artifact{
		ID:      "skill-val-test",
		Type:    forge.ArtifactSkill,
		Name:    "val-test",
		Version: "1.0",
		Status:  forge.StatusDraft,
		Validation: &forge.ArtifactValidation{
			Stage1Static: &forge.StaticValidationResult{
				ValidationStage: forge.ValidationStage{Passed: true},
			},
			Stage2Functional: &forge.FunctionalValidationResult{
				ValidationStage: forge.ValidationStage{Passed: true},
				TestsRun:        3,
				TestsPassed:     3,
			},
		},
	}
	r.Add(artifact)

	// Reload from disk
	r2 := forge.NewRegistry(path)
	got, found := r2.Get("skill-val-test")
	if !found {
		t.Fatal("Artifact not found after reload")
	}
	if got.Validation == nil {
		t.Fatal("Validation should be persisted")
	}
	if got.Validation.Stage1Static == nil || !got.Validation.Stage1Static.Passed {
		t.Error("Stage1Static should be persisted as passed")
	}
	if got.Validation.Stage2Functional.TestsRun != 3 {
		t.Errorf("Expected TestsRun 3, got %d", got.Validation.Stage2Functional.TestsRun)
	}
}

func TestRegistryBackwardCompatibleNoValidation(t *testing.T) {
	tmpDir := t.TempDir()
	path := filepath.Join(tmpDir, "registry.json")

	// Write old-format JSON without validation field
	oldJSON := `{"version":"1.0","artifacts":[{"id":"skill-old","type":"skill","name":"old","version":"1.0","status":"draft","path":"/tmp/old.md"}]}`
	os.WriteFile(path, []byte(oldJSON), 0644)

	r := forge.NewRegistry(path)
	got, found := r.Get("skill-old")
	if !found {
		t.Fatal("Old-format artifact should load")
	}
	if got.Validation != nil {
		t.Error("Old artifacts should have nil Validation")
	}
}
