package forge_test

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"testing"

	"github.com/276793422/NemesisBot/module/forge"
)

func newTestExporter(t *testing.T) (*forge.Exporter, string, *forge.Registry) {
	t.Helper()
	tmpDir := t.TempDir()
	registryPath := filepath.Join(tmpDir, "registry.json")
	registry := forge.NewRegistry(registryPath)
	return forge.NewExporter(tmpDir, registry), tmpDir, registry
}

func TestExporterExportArtifact(t *testing.T) {
	exporter, tmpDir, registry := newTestExporter(t)

	// Create artifact file
	artifactDir := filepath.Join(tmpDir, "forge", "mcp", "json-validator")
	os.MkdirAll(artifactDir, 0755)
	artifactPath := filepath.Join(artifactDir, "server.py")
	os.WriteFile(artifactPath, []byte("import mcp\n\ndef main(): pass\n"), 0644)

	// Add to registry
	registry.Add(forge.Artifact{
		ID:      "mcp-json-validator",
		Type:    forge.ArtifactMCP,
		Name:    "json-validator",
		Version: "1.0",
		Status:  forge.StatusActive,
		Path:    artifactPath,
	})

	targetDir := filepath.Join(tmpDir, "exports")
	err := exporter.ExportArtifact("mcp-json-validator", targetDir)
	if err != nil {
		t.Fatalf("ExportArtifact failed: %v", err)
	}

	// Verify exported files exist
	exportDir := filepath.Join(targetDir, "json-validator-1.0")
	if _, err := os.Stat(exportDir); os.IsNotExist(err) {
		t.Fatal("Export directory should exist")
	}

	// Check main file was copied
	serverContent, err := os.ReadFile(filepath.Join(exportDir, "server.py"))
	if err != nil {
		t.Fatalf("Exported server.py should exist: %v", err)
	}
	if string(serverContent) != "import mcp\n\ndef main(): pass\n" {
		t.Error("Exported content should match original")
	}

	// Check manifest exists
	manifestPath := filepath.Join(exportDir, "forge-manifest.json")
	manifestData, err := os.ReadFile(manifestPath)
	if err != nil {
		t.Fatalf("Manifest should exist: %v", err)
	}

	var manifest forge.ExportManifest
	json.Unmarshal(manifestData, &manifest)
	if manifest.ID != "mcp-json-validator" {
		t.Errorf("Manifest ID should be 'mcp-json-validator', got '%s'", manifest.ID)
	}
	if manifest.Type != "mcp" {
		t.Errorf("Manifest type should be 'mcp', got '%s'", manifest.Type)
	}
	if len(manifest.Files) == 0 {
		t.Error("Manifest should list exported files")
	}
}

func TestExporterExportArtifactWithTests(t *testing.T) {
	exporter, tmpDir, registry := newTestExporter(t)

	// Create artifact with tests/ directory
	artifactDir := filepath.Join(tmpDir, "forge", "mcp", "test-mcp")
	os.MkdirAll(artifactDir, 0755)
	artifactPath := filepath.Join(artifactDir, "server.py")
	os.WriteFile(artifactPath, []byte("import mcp\n"), 0644)

	testDir := filepath.Join(artifactDir, "tests")
	os.MkdirAll(testDir, 0755)
	os.WriteFile(filepath.Join(testDir, "test_cases.json"), []byte(`[{"input": "test"}]`), 0644)

	registry.Add(forge.Artifact{
		ID:      "mcp-test-mcp",
		Type:    forge.ArtifactMCP,
		Name:    "test-mcp",
		Version: "2.0",
		Status:  forge.StatusActive,
		Path:    artifactPath,
	})

	targetDir := filepath.Join(tmpDir, "exports")
	err := exporter.ExportArtifact("mcp-test-mcp", targetDir)
	if err != nil {
		t.Fatalf("ExportArtifact failed: %v", err)
	}

	// Check tests were copied
	exportTestFile := filepath.Join(targetDir, "test-mcp-2.0", "tests", "test_cases.json")
	if _, err := os.Stat(exportTestFile); os.IsNotExist(err) {
		t.Error("Tests should be exported")
	}
}

func TestExporterExportArtifactNotFound(t *testing.T) {
	exporter, _, _ := newTestExporter(t)

	err := exporter.ExportArtifact("nonexistent", "/tmp/exports")
	if err == nil {
		t.Error("Should error on non-existent artifact")
	}
}

func TestExporterExportAllEmpty(t *testing.T) {
	exporter, tmpDir, _ := newTestExporter(t)

	targetDir := filepath.Join(tmpDir, "exports")
	count, err := exporter.ExportAll(targetDir)
	if err != nil {
		t.Fatalf("ExportAll should not error on empty registry: %v", err)
	}
	if count != 0 {
		t.Errorf("Expected 0 exports, got %d", count)
	}
}

func TestExporterExportAll(t *testing.T) {
	exporter, tmpDir, registry := newTestExporter(t)

	// Create two artifacts - one active, one draft
	for i, status := range []forge.ArtifactStatus{forge.StatusActive, forge.StatusDraft} {
		artifactDir := filepath.Join(tmpDir, "forge", "scripts", fmt.Sprintf("script-%d", i))
		os.MkdirAll(artifactDir, 0755)
		artifactPath := filepath.Join(artifactDir, "main.sh")
		os.WriteFile(artifactPath, []byte("#!/bin/bash\necho hi\n"), 0644)

		registry.Add(forge.Artifact{
			ID:     fmt.Sprintf("script-script-%d", i),
			Type:   forge.ArtifactScript,
			Name:   fmt.Sprintf("script-%d", i),
			Status: status,
			Path:   artifactPath,
		})
	}

	targetDir := filepath.Join(tmpDir, "exports")
	count, err := exporter.ExportAll(targetDir)
	if err != nil {
		t.Fatalf("ExportAll failed: %v", err)
	}
	if count != 1 {
		t.Errorf("Expected 1 export (only active), got %d", count)
	}
}

func TestExporterExportAllWithProjectFiles(t *testing.T) {
	exporter, tmpDir, registry := newTestExporter(t)

	// Create MCP artifact with requirements.txt
	artifactDir := filepath.Join(tmpDir, "forge", "mcp", "py-validator")
	os.MkdirAll(artifactDir, 0755)
	artifactPath := filepath.Join(artifactDir, "server.py")
	os.WriteFile(artifactPath, []byte("import mcp\n"), 0644)
	os.WriteFile(filepath.Join(artifactDir, "requirements.txt"), []byte("mcp>=1.0.0\n"), 0644)

	registry.Add(forge.Artifact{
		ID:      "mcp-py-validator",
		Type:    forge.ArtifactMCP,
		Name:    "py-validator",
		Version: "1.0",
		Status:  forge.StatusActive,
		Path:    artifactPath,
	})

	targetDir := filepath.Join(tmpDir, "exports")
	err := exporter.ExportArtifact("mcp-py-validator", targetDir)
	if err != nil {
		t.Fatalf("Export failed: %v", err)
	}

	exportDir := filepath.Join(targetDir, "py-validator-1.0")
	reqContent, err := os.ReadFile(filepath.Join(exportDir, "requirements.txt"))
	if err != nil {
		t.Fatalf("requirements.txt should be exported: %v", err)
	}
	if string(reqContent) != "mcp>=1.0.0\n" {
		t.Error("requirements.txt content mismatch")
	}
}

func TestExporterManifestFields(t *testing.T) {
	exporter, tmpDir, registry := newTestExporter(t)

	artifactDir := filepath.Join(tmpDir, "forge", "skills", "my-skill")
	os.MkdirAll(artifactDir, 0755)
	artifactPath := filepath.Join(artifactDir, "SKILL.md")
	os.WriteFile(artifactPath, []byte("---\nname: my-skill\n---\nContent"), 0644)

	registry.Add(forge.Artifact{
		ID:      "skill-my-skill",
		Type:    forge.ArtifactSkill,
		Name:    "my-skill",
		Version: "3.2",
		Status:  forge.StatusActive,
		Path:    artifactPath,
	})

	targetDir := filepath.Join(tmpDir, "exports")
	exporter.ExportArtifact("skill-my-skill", targetDir)

	manifestPath := filepath.Join(targetDir, "my-skill-3.2", "forge-manifest.json")
	data, _ := os.ReadFile(manifestPath)
	var manifest forge.ExportManifest
	json.Unmarshal(data, &manifest)

	if manifest.ExportedAt == "" {
		t.Error("Manifest should have exported_at timestamp")
	}
	if manifest.Version != "3.2" {
		t.Errorf("Manifest version should be '3.2', got '%s'", manifest.Version)
	}
}
