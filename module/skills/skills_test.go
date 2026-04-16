// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package skills

import (
	"context"
	"testing"
	"time"
)

func TestSearchResult_Structure(t *testing.T) {
	result := SearchResult{
		Score:        0.95,
		Slug:         "test-skill",
		DisplayName:  "Test Skill",
		Summary:      "A test skill",
		Version:      "1.0.0",
		RegistryName: "test-registry",
	}

	if result.Score != 0.95 {
		t.Errorf("Expected score 0.95, got %f", result.Score)
	}

	if result.Slug != "test-skill" {
		t.Errorf("Expected slug 'test-skill', got '%s'", result.Slug)
	}

	if result.DisplayName != "Test Skill" {
		t.Errorf("Expected display name 'Test Skill', got '%s'", result.DisplayName)
	}

	if result.Summary != "A test skill" {
		t.Errorf("Expected summary 'A test skill', got '%s'", result.Summary)
	}

	if result.Version != "1.0.0" {
		t.Errorf("Expected version '1.0.0', got '%s'", result.Version)
	}

	if result.RegistryName != "test-registry" {
		t.Errorf("Expected registry name 'test-registry', got '%s'", result.RegistryName)
	}
}

func TestSkillMeta_Structure(t *testing.T) {
	meta := SkillMeta{
		Slug:             "test-skill",
		DisplayName:      "Test Skill",
		Summary:          "A test skill",
		LatestVersion:    "1.0.0",
		IsMalwareBlocked: false,
		IsSuspicious:     false,
		RegistryName:     "test-registry",
	}

	if meta.Slug != "test-skill" {
		t.Errorf("Expected slug 'test-skill', got '%s'", meta.Slug)
	}

	if meta.DisplayName != "Test Skill" {
		t.Errorf("Expected display name 'Test Skill', got '%s'", meta.DisplayName)
	}

	if meta.Summary != "A test skill" {
		t.Errorf("Expected summary 'A test skill', got '%s'", meta.Summary)
	}

	if meta.LatestVersion != "1.0.0" {
		t.Errorf("Expected latest version '1.0.0', got '%s'", meta.LatestVersion)
	}

	if meta.IsMalwareBlocked {
		t.Error("Expected IsMalwareBlocked to be false")
	}

	if meta.IsSuspicious {
		t.Error("Expected IsSuspicious to be false")
	}

	if meta.RegistryName != "test-registry" {
		t.Errorf("Expected registry name 'test-registry', got '%s'", meta.RegistryName)
	}
}

func TestInstallResult_Structure(t *testing.T) {
	result := InstallResult{
		Version:          "1.0.0",
		IsMalwareBlocked: false,
		IsSuspicious:     true,
		Summary:          "Test summary",
	}

	if result.Version != "1.0.0" {
		t.Errorf("Expected version '1.0.0', got '%s'", result.Version)
	}

	if result.IsMalwareBlocked {
		t.Error("Expected IsMalwareBlocked to be false")
	}

	if !result.IsSuspicious {
		t.Error("Expected IsSuspicious to be true")
	}

	if result.Summary != "Test summary" {
		t.Errorf("Expected summary 'Test summary', got '%s'", result.Summary)
	}
}

func TestRegistryConfig_DefaultValues(t *testing.T) {
	config := RegistryConfig{}

	if config.SearchCache.MaxSize != 0 {
		t.Errorf("Expected default MaxSize 0, got %d", config.SearchCache.MaxSize)
	}

	if config.SearchCache.TTL != 0 {
		t.Errorf("Expected default TTL 0, got %v", config.SearchCache.TTL)
	}

	if config.MaxConcurrentSearches != 0 {
		t.Errorf("Expected default MaxConcurrentSearches 0, got %d", config.MaxConcurrentSearches)
	}
}

func TestRegistryConfig_WithValues(t *testing.T) {
	config := RegistryConfig{
		SearchCache: SearchCacheConfig{
			Enabled: true,
			MaxSize: 100,
			TTL:     5 * time.Minute,
		},
		ClawHub: ClawHubConfig{
			Enabled:   true,
			BaseURL:   "https://clawhub.ai",
			ConvexURL: "https://wry-manatee-359.convex.cloud",
			Timeout:   30,
		},
		GitHub: GitHubConfig{
			Enabled: true,
			BaseURL: "github.com",
			Timeout: 30,
			MaxSize: 1024 * 1024,
		},
		MaxConcurrentSearches: 5,
	}

	if !config.SearchCache.Enabled {
		t.Error("Expected SearchCache.Enabled to be true")
	}

	if config.SearchCache.MaxSize != 100 {
		t.Errorf("Expected SearchCache.MaxSize 100, got %d", config.SearchCache.MaxSize)
	}

	if config.SearchCache.TTL != 5*time.Minute {
		t.Errorf("Expected SearchCache.TTL 5m, got %v", config.SearchCache.TTL)
	}

	if !config.ClawHub.Enabled {
		t.Error("Expected ClawHub.Enabled to be true")
	}

	if config.ClawHub.ConvexURL != "https://wry-manatee-359.convex.cloud" {
		t.Errorf("Expected ClawHub.ConvexURL 'https://wry-manatee-359.convex.cloud', got '%s'", config.ClawHub.ConvexURL)
	}

	if config.MaxConcurrentSearches != 5 {
		t.Errorf("Expected MaxConcurrentSearches 5, got %d", config.MaxConcurrentSearches)
	}
}

func TestSearchCacheConfig_DefaultValues(t *testing.T) {
	config := SearchCacheConfig{}

	if config.Enabled {
		t.Error("Expected default Enabled to be false")
	}

	if config.MaxSize != 0 {
		t.Errorf("Expected default MaxSize 0, got %d", config.MaxSize)
	}

	if config.TTL != 0 {
		t.Errorf("Expected default TTL 0, got %v", config.TTL)
	}
}

func TestClawHubConfig_DefaultValues(t *testing.T) {
	config := ClawHubConfig{}

	if config.Enabled {
		t.Error("Expected default Enabled to be false")
	}

	if config.ConvexURL != "" {
		t.Errorf("Expected default ConvexURL empty, got '%s'", config.ConvexURL)
	}

	if config.Timeout != 0 {
		t.Errorf("Expected default Timeout 0, got %d", config.Timeout)
	}
}

func TestGitHubConfig_DefaultValues(t *testing.T) {
	config := GitHubConfig{}

	if config.Enabled {
		t.Error("Expected default Enabled to be false")
	}

	if config.BaseURL != "" {
		t.Errorf("Expected default BaseURL empty, got '%s'", config.BaseURL)
	}

	if config.Timeout != 0 {
		t.Errorf("Expected default Timeout 0, got %d", config.Timeout)
	}

	if config.MaxSize != 0 {
		t.Errorf("Expected default MaxSize 0, got %d", config.MaxSize)
	}
}

func TestSkillRegistry_Interface(t *testing.T) {
	// Verify that mock registry implements the interface
	var _ SkillRegistry = NewMockRegistry("test")
}

func TestConstants(t *testing.T) {
	if defaultMaxConcurrentSearches != 2 {
		t.Errorf("Expected defaultMaxConcurrentSearches to be 2, got %d", defaultMaxConcurrentSearches)
	}
}

func TestMockRegistry_Name(t *testing.T) {
	reg := NewMockRegistry("test-registry")
	if reg.Name() != "test-registry" {
		t.Errorf("Expected name 'test-registry', got '%s'", reg.Name())
	}
}

func TestMockRegistry_Search(t *testing.T) {
	reg := NewMockRegistry("test")
	reg.AddSearchResult(SearchResult{
		Score:        1.0,
		Slug:         "test-skill",
		DisplayName:  "Test Skill",
		Summary:      "A test skill",
		Version:      "1.0.0",
		RegistryName: "test",
	})

	ctx := context.Background()
	results, err := reg.Search(ctx, "test", 10)
	if err != nil {
		t.Fatalf("Search() failed: %v", err)
	}

	if len(results) != 1 {
		t.Errorf("Expected 1 result, got %d", len(results))
	}

	if results[0].Slug != "test-skill" {
		t.Errorf("Expected slug 'test-skill', got '%s'", results[0].Slug)
	}
}

func TestMockRegistry_GetSkillMeta(t *testing.T) {
	reg := NewMockRegistry("test")
	ctx := context.Background()

	meta, err := reg.GetSkillMeta(ctx, "test-skill")
	if err != nil {
		t.Fatalf("GetSkillMeta() failed: %v", err)
	}

	if meta.Slug != "test-skill" {
		t.Errorf("Expected slug 'test-skill', got '%s'", meta.Slug)
	}
}

func TestMockRegistry_DownloadAndInstall(t *testing.T) {
	reg := NewMockRegistry("test")
	ctx := context.Background()

	result, err := reg.DownloadAndInstall(ctx, "test-skill", "1.0.0", "/tmp")
	if err != nil {
		t.Fatalf("DownloadAndInstall() failed: %v", err)
	}

	if result.Version != "1.0.0" {
		t.Errorf("Expected version '1.0.0', got '%s'", result.Version)
	}

	if result.Summary != "Mock installation" {
		t.Errorf("Expected summary 'Mock installation', got '%s'", result.Summary)
	}
}

func TestSearchResult_ScoreRanges(t *testing.T) {
	tests := []struct {
		name  string
		score float64
		valid bool
	}{
		{"perfect score", 1.0, true},
		{"zero score", 0.0, true},
		{"negative score", -0.5, false},
		{"above one score", 1.5, false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := SearchResult{Score: tt.score}
			isValid := result.Score >= 0.0 && result.Score <= 1.0

			if isValid != tt.valid {
				t.Errorf("Score %f validity: expected %v, got %v", tt.score, tt.valid, isValid)
			}
		})
	}
}

func TestSkillMeta_MalwareFlags(t *testing.T) {
	tests := []struct {
		name             string
		isMalwareBlocked bool
		isSuspicious     bool
	}{
		{"clean skill", false, false},
		{"malware blocked", true, false},
		{"suspicious skill", false, true},
		{"both flags", true, true},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			meta := SkillMeta{
				IsMalwareBlocked: tt.isMalwareBlocked,
				IsSuspicious:     tt.isSuspicious,
			}

			if meta.IsMalwareBlocked != tt.isMalwareBlocked {
				t.Errorf("IsMalwareBlocked: expected %v, got %v", tt.isMalwareBlocked, meta.IsMalwareBlocked)
			}

			if meta.IsSuspicious != tt.isSuspicious {
				t.Errorf("IsSuspicious: expected %v, got %v", tt.isSuspicious, meta.IsSuspicious)
			}
		})
	}
}

func TestRegistryConfig_Copy(t *testing.T) {
	original := RegistryConfig{
		MaxConcurrentSearches: 3,
		SearchCache: SearchCacheConfig{
			Enabled: true,
			MaxSize: 50,
		},
	}

	copy := original
	copy.MaxConcurrentSearches = 5

	if original.MaxConcurrentSearches != 3 {
		t.Error("Copy should be independent of original")
	}
}
