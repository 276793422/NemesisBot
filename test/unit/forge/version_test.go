package forge_test

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/276793422/NemesisBot/module/forge"
)

// === Version Snapshot Tests ===

func TestSaveAndLoadVersionSnapshot(t *testing.T) {
	tmpDir := t.TempDir()

	// Create an artifact file
	artifactDir := filepath.Join(tmpDir, "skills", "versioned-skill")
	os.MkdirAll(artifactDir, 0755)
	artifactPath := filepath.Join(artifactDir, "SKILL.md")
	originalContent := "---\nname: test\n---\n\nOriginal content v1.0"
	os.WriteFile(artifactPath, []byte(originalContent), 0644)

	// Save snapshot
	err := forge.SaveVersionSnapshot(artifactPath, "1.0")
	if err != nil {
		t.Fatalf("SaveVersionSnapshot failed: %v", err)
	}

	// Verify .versions directory was created
	versionsDir := filepath.Join(artifactDir, ".versions")
	if _, err := os.Stat(versionsDir); os.IsNotExist(err) {
		t.Error(".versions directory should be created")
	}

	// Verify snapshot file exists
	snapshotPath := filepath.Join(versionsDir, "1.0.bak")
	if _, err := os.Stat(snapshotPath); os.IsNotExist(err) {
		t.Error("Snapshot file should exist")
	}

	// Load snapshot
	loaded, err := forge.LoadVersionSnapshot(artifactPath, "1.0")
	if err != nil {
		t.Fatalf("LoadVersionSnapshot failed: %v", err)
	}
	if loaded != originalContent {
		t.Errorf("Loaded content mismatch: got '%s', want '%s'", loaded, originalContent)
	}
}

func TestLoadNonexistentVersionSnapshot(t *testing.T) {
	tmpDir := t.TempDir()

	artifactDir := filepath.Join(tmpDir, "skills", "no-snap")
	os.MkdirAll(artifactDir, 0755)
	artifactPath := filepath.Join(artifactDir, "SKILL.md")
	os.WriteFile(artifactPath, []byte("content"), 0644)

	_, err := forge.LoadVersionSnapshot(artifactPath, "99.0")
	if err == nil {
		t.Error("Expected error for nonexistent snapshot")
	}
}

func TestVersionSnapshotOverwrite(t *testing.T) {
	tmpDir := t.TempDir()

	artifactDir := filepath.Join(tmpDir, "skills", "ow-skill")
	os.MkdirAll(artifactDir, 0755)
	artifactPath := filepath.Join(artifactDir, "SKILL.md")

	// Save v1
	os.WriteFile(artifactPath, []byte("v1 content"), 0644)
	forge.SaveVersionSnapshot(artifactPath, "1.0")

	// Save v2
	os.WriteFile(artifactPath, []byte("v2 content"), 0644)
	forge.SaveVersionSnapshot(artifactPath, "2.0")

	// Both should be loadable
	v1, err := forge.LoadVersionSnapshot(artifactPath, "1.0")
	if err != nil || v1 != "v1 content" {
		t.Errorf("v1 snapshot mismatch: got '%s', err=%v", v1, err)
	}

	v2, err := forge.LoadVersionSnapshot(artifactPath, "2.0")
	if err != nil || v2 != "v2 content" {
		t.Errorf("v2 snapshot mismatch: got '%s', err=%v", v2, err)
	}
}

func TestVersionSnapshotNoArtifactFile(t *testing.T) {
	tmpDir := t.TempDir()

	artifactDir := filepath.Join(tmpDir, "skills", "missing")
	os.MkdirAll(artifactDir, 0755)
	artifactPath := filepath.Join(artifactDir, "SKILL.md")

	// Don't create the file - save should fail
	err := forge.SaveVersionSnapshot(artifactPath, "1.0")
	if err == nil {
		t.Error("Expected error when artifact file doesn't exist")
	}
}
