package forge

import (
	"encoding/json"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"time"
)

// ExportManifest describes an exported artifact's metadata.
type ExportManifest struct {
	ID         string   `json:"id"`
	Type       string   `json:"type"`
	Name       string   `json:"name"`
	Version    string   `json:"version"`
	ExportedAt string   `json:"exported_at"`
	Files      []string `json:"files"`
}

// Exporter handles exporting Forge artifacts to shareable formats.
type Exporter struct {
	workspace string
	registry  *Registry
}

// NewExporter creates a new Exporter for the given workspace.
func NewExporter(workspace string, registry *Registry) *Exporter {
	return &Exporter{
		workspace: workspace,
		registry:  registry,
	}
}

// ExportArtifact exports a single artifact to a target directory.
func (e *Exporter) ExportArtifact(artifactID string, targetDir string) error {
	artifact, found := e.registry.Get(artifactID)
	if !found {
		return fmt.Errorf("产物 %s 不存在", artifactID)
	}

	// Create target subdirectory: {targetDir}/{name}-{version}/
	exportDir := filepath.Join(targetDir, fmt.Sprintf("%s-%s", artifact.Name, artifact.Version))
	if err := os.MkdirAll(exportDir, 0755); err != nil {
		return fmt.Errorf("创建导出目录失败: %w", err)
	}

	var files []string
	artifactDir := filepath.Dir(artifact.Path)

	// Copy artifact main file
	mainFile := filepath.Base(artifact.Path)
	if err := copyFile(artifact.Path, filepath.Join(exportDir, mainFile)); err != nil {
		return fmt.Errorf("复制主文件失败: %w", err)
	}
	files = append(files, mainFile)

	// Copy project structure files from artifact directory
	projectFiles := []string{"requirements.txt", "go.mod", "README.md"}
	for _, f := range projectFiles {
		src := filepath.Join(artifactDir, f)
		if _, err := os.Stat(src); err == nil {
			if err := copyFile(src, filepath.Join(exportDir, f)); err == nil {
				files = append(files, f)
			}
		}
	}

	// Copy tests/ subdirectory if it exists
	testsDir := filepath.Join(artifactDir, "tests")
	if info, err := os.Stat(testsDir); err == nil && info.IsDir() {
		testTarget := filepath.Join(exportDir, "tests")
		if err := os.MkdirAll(testTarget, 0755); err == nil {
			if copied := copyDir(testsDir, testTarget); len(copied) > 0 {
				for _, f := range copied {
					files = append(files, "tests/"+f)
				}
			}
		}
	}

	// Generate forge-manifest.json
	manifest := ExportManifest{
		ID:         artifact.ID,
		Type:       string(artifact.Type),
		Name:       artifact.Name,
		Version:    artifact.Version,
		ExportedAt: time.Now().UTC().Format(time.RFC3339),
		Files:      files,
	}

	manifestData, err := json.MarshalIndent(manifest, "", "  ")
	if err != nil {
		return fmt.Errorf("生成 manifest 失败: %w", err)
	}

	manifestPath := filepath.Join(exportDir, "forge-manifest.json")
	if err := os.WriteFile(manifestPath, manifestData, 0644); err != nil {
		return fmt.Errorf("写入 manifest 失败: %w", err)
	}

	return nil
}

// ExportAll exports all active artifacts to a target directory.
func (e *Exporter) ExportAll(targetDir string) (int, error) {
	artifacts := e.registry.ListAll()
	if len(artifacts) == 0 {
		return 0, nil
	}

	if err := os.MkdirAll(targetDir, 0755); err != nil {
		return 0, fmt.Errorf("创建导出目录失败: %w", err)
	}

	count := 0
	for _, a := range artifacts {
		if a.Status != StatusActive {
			continue
		}
		if err := e.ExportArtifact(a.ID, targetDir); err != nil {
			continue // Skip artifacts that fail to export
		}
		count++
	}

	return count, nil
}

// copyFile copies a single file from src to dst.
func copyFile(src, dst string) error {
	data, err := os.ReadFile(src)
	if err != nil {
		return err
	}
	return os.WriteFile(dst, data, 0644)
}

// copyDir copies all files from srcDir to dstDir, returning relative file paths.
func copyDir(srcDir, dstDir string) []string {
	var files []string
	filepath.WalkDir(srcDir, func(path string, d fs.DirEntry, err error) error {
		if err != nil || d.IsDir() {
			return nil
		}
		rel, err := filepath.Rel(srcDir, path)
		if err != nil {
			return nil
		}
		targetPath := filepath.Join(dstDir, rel)
		os.MkdirAll(filepath.Dir(targetPath), 0755)
		if err := copyFile(path, targetPath); err == nil {
			files = append(files, rel)
		}
		return nil
	})
	return files
}
