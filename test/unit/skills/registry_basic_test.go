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

// MockRegistry is a test implementation of SkillRegistry
type MockRegistry struct {
	name        string
	searchResults []skills.SearchResult
	skillMeta    map[string]*skills.SkillMeta
}

func (m *MockRegistry) Name() string {
	return m.name
}

func (m *MockRegistry) Search(ctx context.Context, query string, limit int) ([]skills.SearchResult, error) {
	// Simple filter implementation
	var results []skills.SearchResult
	for i, result := range m.searchResults {
		if len(results) >= limit {
			break
		}
		// Simple matching
		if contains(result.Slug, query) || contains(result.Summary, query) || query == "" {
			results = append(results, result)
		}
		_ = i // unused
	}
	return results, nil
}

func (m *MockRegistry) GetSkillMeta(ctx context.Context, slug string) (*skills.SkillMeta, error) {
	if meta, ok := m.skillMeta[slug]; ok {
		return meta, nil
	}
	return &skills.SkillMeta{Slug: slug, DisplayName: slug, Summary: "Mock skill"}, nil
}

func (m *MockRegistry) DownloadAndInstall(ctx context.Context, slug, version, targetDir string) (*skills.InstallResult, error) {
	return &skills.InstallResult{
		Version:          version,
		IsMalwareBlocked: false,
		IsSuspicious:     false,
		Summary:          "Mock installation",
	}, nil
}

func contains(s, substr string) bool {
	return len(s) >= len(substr) && (s == substr || len(s) > len(substr))
}

func TestRegistryManagerBasic(t *testing.T) {
	rm := skills.NewRegistryManager()

	// Add mock registries
	mock1 := &MockRegistry{
		name: "mock1",
		searchResults: []skills.SearchResult{
			{Slug: "weather", DisplayName: "Weather Skill", Summary: "Get weather info", Score: 0.9, RegistryName: "mock1"},
			{Slug: "github", DisplayName: "GitHub Skill", Summary: "GitHub integration", Score: 0.8, RegistryName: "mock1"},
		},
		skillMeta: make(map[string]*skills.SkillMeta),
	}

	mock2 := &MockRegistry{
		name: "mock2",
		searchResults: []skills.SearchResult{
			{Slug: "calculator", DisplayName: "Calculator", Summary: "Math operations", Score: 0.85, RegistryName: "mock2"},
		},
		skillMeta: make(map[string]*skills.SkillMeta),
	}

	rm.AddRegistry(mock1)
	rm.AddRegistry(mock2)

	t.Run("Get registries", func(t *testing.T) {
		if rm.GetRegistry("mock1") == nil {
			t.Error("Expected to find mock1 registry")
		}
		if rm.GetRegistry("unknown") != nil {
			t.Error("Expected nil for unknown registry")
		}
	})

	t.Run("Search all registries", func(t *testing.T) {
		ctx, cancel := context.WithTimeout(context.Background(), 1*time.Second)
		defer cancel()

		results, err := rm.SearchAll(ctx, "", 10) // Empty query to get all
		if err != nil {
			t.Fatalf("SearchAll failed: %v", err)
		}

		if len(results) != 3 {
			t.Errorf("Expected 3 results, got %d", len(results))
		}

		// Check that results are sorted by score
		for i := 0; i < len(results)-1; i++ {
			if results[i].Score < results[i+1].Score {
				t.Errorf("Results not sorted by score: %.2f < %.2f", results[i].Score, results[i+1].Score)
			}
		}
	})

	t.Run("Search with limit", func(t *testing.T) {
		ctx, cancel := context.WithTimeout(context.Background(), 1*time.Second)
		defer cancel()

		results, err := rm.SearchAll(ctx, "", 2)
		if err != nil {
			t.Fatalf("SearchAll failed: %v", err)
		}

		if len(results) != 2 {
			t.Errorf("Expected 2 results (limited), got %d", len(results))
		}
	})
}

func TestGitHubRegistryBasic(t *testing.T) {
	cfg := skills.GitHubConfig{
		Enabled: true,
		Timeout: 30,
	}
	registry := skills.NewGitHubRegistry(cfg)

	if registry.Name() != "github" {
		t.Errorf("Expected registry name 'github', got '%s'", registry.Name())
	}
}

func TestRegistryManagerFromConfig(t *testing.T) {
	cfg := skills.RegistryConfig{
		GitHub: skills.GitHubConfig{
			Enabled: true,
			Timeout: 30,
		},
		MaxConcurrentSearches: 3,
	}

	rm := skills.NewRegistryManagerFromConfig(cfg)
	if rm == nil {
		t.Fatal("Expected non-nil RegistryManager")
	}

	if rm.GetRegistry("github") == nil {
		t.Error("Expected GitHub registry to be configured")
	}
}

func TestSkillInstallerBasic(t *testing.T) {
	tempDir := t.TempDir()
	installer := skills.NewSkillInstaller(tempDir)

	// Test with registry manager
	mockRM := skills.NewRegistryManager()
	installer.SetRegistryManager(mockRM)

	// We can't directly access registryManager as it's unexported
	// but we can test that the installer was created successfully
	if installer == nil {
		t.Error("Expected installer to be created")
	}
}