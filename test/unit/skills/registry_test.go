// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package skills_test

import (
	"context"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/skills"
)

func TestGitHubRegistry(t *testing.T) {
	cfg := skills.GitHubConfig{
		Enabled: true,
		Timeout: 30,
	}
	registry := skills.NewGitHubRegistry(cfg)

	if registry.Name() != "github" {
		t.Errorf("Expected registry name 'github', got '%s'", registry.Name())
	}

	t.Run("Search skills", func(t *testing.T) {
		ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
		defer cancel()

		results, err := registry.Search(ctx, "weather", 10)
		if err != nil {
			t.Logf("Search failed (might be expected if no network): %v", err)
			return
		}

		if len(results) == 0 {
			t.Log("No weather skills found (might be expected)")
			return
		}

		for _, result := range results {
			if result.RegistryName != "github" {
				t.Errorf("Expected registry name 'github', got '%s'", result.RegistryName)
			}
			if result.Slug == "" {
				t.Error("Expected non-empty slug")
			}
			t.Logf("Found skill: %s - %s", result.DisplayName, result.Summary)
		}
	})

	t.Run("Get metadata", func(t *testing.T) {
		ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
		defer cancel()

		// Try to get metadata for a known skill
		meta, err := registry.GetSkillMeta(ctx, "weather")
		if err != nil {
			t.Logf("GetSkillMeta failed: %v", err)
			return
		}

		if meta.Slug == "" {
			t.Error("Expected non-empty slug in metadata")
		}
		t.Logf("Metadata: %+v", meta)
	})
}

func TestRegistryManager(t *testing.T) {
	cfg := skills.RegistryConfig{
		GitHub: skills.GitHubConfig{
			Enabled: true,
			Timeout: 30,
		},
		MaxConcurrentSearches: 2,
	}

	rm := skills.NewRegistryManagerFromConfig(cfg)
	if rm == nil {
		t.Fatal("Expected non-nil RegistryManager")
	}

	t.Run("Get GitHub registry", func(t *testing.T) {
		github := rm.GetRegistry("github")
		if github == nil {
			t.Error("Expected GitHub registry to be found")
		}
	})

	t.Run("Get unknown registry", func(t *testing.T) {
		unknown := rm.GetRegistry("unknown")
		if unknown != nil {
			t.Error("Expected nil for unknown registry")
		}
	})

	t.Run("Search all registries", func(t *testing.T) {
		ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
		defer cancel()

		results, err := rm.SearchAll(ctx, "weather", 5)
		if err != nil {
			t.Logf("Search failed: %v", err)
			return
		}

		t.Logf("Found %d skills matching 'weather'", len(results))
		for i, result := range results {
			if i >= 3 { // Only log first 3
				break
			}
			t.Logf("  %d. %s (%s) - %.2f", i+1, result.DisplayName, result.RegistryName, result.Score)
		}
	})
}

func TestSkillInstaller(t *testing.T) {
	// This test requires a temporary workspace
	tempDir := t.TempDir()

	installer := skills.NewSkillInstaller(tempDir)

	// Test with registry manager
	cfg := skills.RegistryConfig{
		GitHub: skills.GitHubConfig{
			Enabled: true,
			Timeout: 30,
		},
	}

	rm := skills.NewRegistryManagerFromConfig(cfg)
	installer.SetRegistryManager(rm)

	// We can't directly test registryManager as it's unexported
	// but we can test that the installer was created and configured
	if installer == nil {
		t.Error("Expected installer to be created")
	}
}