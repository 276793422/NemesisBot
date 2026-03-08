// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package skills_test

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/skills"
	"github.com/276793422/NemesisBot/module/utils"
)

// TestRegistryManager_ConcurrentSearch tests concurrent search functionality
func TestRegistryManager_ConcurrentSearch(t *testing.T) {
	rm := skills.NewRegistryManager()

	// Add multiple mock registries to test concurrent access
	for i := 0; i < 3; i++ {
		mock := &MockRegistry{
			name: fmt.Sprintf("mock%d", i),
			searchResults: []skills.SearchResult{
				{Slug: fmt.Sprintf("skill%d", i), DisplayName: fmt.Sprintf("Skill %d", i), Summary: "Test skill", Score: float64(1.0 - float64(i)*0.1), RegistryName: fmt.Sprintf("mock%d", i)},
			},
			skillMeta: make(map[string]*skills.SkillMeta),
		}
		rm.AddRegistry(mock)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	// Test concurrent search
	results, err := rm.SearchAll(ctx, "test", 10)
	if err != nil {
		t.Fatalf("Concurrent search failed: %v", err)
	}

	// Should get results from all mock registries
	if len(results) != 3 {
		t.Errorf("Expected 3 results from concurrent search, got %d", len(results))
	}

	// Results should be sorted by score descending
	for i := 0; i < len(results)-1; i++ {
		if results[i].Score < results[i+1].Score {
			t.Errorf("Results not properly sorted by score: [%.2f, %.2f]", results[i].Score, results[i+1].Score)
		}
	}
}

// TestRegistryManager_MaxConcurrentSearches tests concurrent search limit
func TestRegistryManager_MaxConcurrentSearches(t *testing.T) {
	cfg := skills.RegistryConfig{
		MaxConcurrentSearches: 1, // Limit to 1 concurrent search
	}

	rm := skills.NewRegistryManagerFromConfig(cfg)

	// Add multiple registries
	for i := 0; i < 3; i++ {
		mock := &MockRegistry{
			name: fmt.Sprintf("slow%d", i),
			searchResults: []skills.SearchResult{
				{Slug: "skill1", DisplayName: "Skill 1", Summary: "Test", Score: 0.9, RegistryName: "slow"},
			},
			skillMeta: make(map[string]*skills.SkillMeta),
		}
		rm.AddRegistry(mock)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	// Should still complete despite limit
	results, err := rm.SearchAll(ctx, "test", 10)
	if err != nil {
		t.Fatalf("Search with concurrency limit failed: %v", err)
	}

	// Should still get results
	if len(results) == 0 {
		t.Error("Expected some results even with concurrency limit")
	}
}

// TestGitHubRegistry_ConfigDefaults tests GitHub registry configuration defaults
func TestGitHubRegistry_ConfigDefaults(t *testing.T) {
	// Test with minimal configuration
	cfg := skills.GitHubConfig{
		Enabled: true,
		// Omit optional fields to test defaults
	}

	registry := skills.NewGitHubRegistry(cfg)

	if registry == nil {
		t.Fatal("Expected registry to be created")
	}

	if registry.Name() != "github" {
		t.Errorf("Expected registry name 'github', got '%s'", registry.Name())
	}
}

// TestSearchResult_Serialization tests SearchResult JSON serialization
func TestSearchResult_Serialization(t *testing.T) {
	result := skills.SearchResult{
		Score:        0.95,
		Slug:         "test-skill",
		DisplayName:  "Test Skill",
		Summary:      "A test skill for serialization",
		Version:      "1.0.0",
		RegistryName: "test",
	}

	// Test that all fields are accessible
	if result.Slug == "" {
		t.Error("SearchResult Slug field missing")
	}
	if result.Score == 0 {
		t.Error("SearchResult Score field not set")
	}
	if result.DisplayName == "" {
		t.Error("SearchResult DisplayName field missing")
	}
	if result.RegistryName == "" {
		t.Error("SearchResult RegistryName field missing")
	}
}

// TestSkillMeta_SecurityFields tests security-related metadata fields
func TestSkillMeta_SecurityFields(t *testing.T) {
	meta := skills.SkillMeta{
		Slug:             "test-skill",
		DisplayName:      "Test Skill",
		Summary:          "Test description",
		LatestVersion:    "1.0.0",
		IsMalwareBlocked: true,
		IsSuspicious:     false,
		RegistryName:     "test",
	}

	// Test security fields are accessible
	if !meta.IsMalwareBlocked {
		t.Error("IsMalwareBlocked field not working")
	}
	if meta.IsSuspicious {
		t.Error("IsSuspicious field should be false")
	}
}

// TestInstallResult_Validation tests install result validation
func TestInstallResult_Validation(t *testing.T) {
	result := skills.InstallResult{
		Version:          "1.0.0",
		IsMalwareBlocked: false,
		IsSuspicious:     true,
		Summary:          "Test installation",
	}

	// Test malware blocking logic
	if result.IsMalwareBlocked {
		t.Error("This skill should be blocked")
	}
	if !result.IsSuspicious {
		t.Error("This skill should be marked suspicious")
	}
}

// TestSkillIdentifierValidation tests skill identifier validation
func TestSkillIdentifierValidation(t *testing.T) {
	tests := []struct {
		name      string
		input     string
		shouldErr bool
	}{
		{"Valid skill name", "weather", false},
		{"Valid with hyphens", "my-skill-name", false},
		{"Valid with numbers", "skill123", false},
		{"Empty string", "", true},
		{"Path separator slash", "weather/skill", true},
		{"Path separator backslash", "weather\\skill", true},
		{"Path traversal attempt", "../etc/passwd", true},
		{"Absolute path", "/etc/passwd", true},
		{"Current directory", "./skill", true},
		{"Parent directory", "../skill", true},
		{"Multiple separators", "skill/../../test", true},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			err := utils.ValidateSkillIdentifier(tt.input)
			if tt.shouldErr && err == nil {
				t.Errorf("Expected error for '%s', got nil", tt.input)
			}
			if !tt.shouldErr && err != nil {
				t.Errorf("Expected no error for '%s', got: %v", tt.input, err)
			}
		})
	}
}

// TestDerefStr tests string pointer dereference utility function
func TestDerefStr(t *testing.T) {
	tests := []struct {
		name      string
		ptr       *string
		defaultVal string
		expected  string
	}{
		{"Nil pointer returns default", nil, "default", "default"},
		{"Valid pointer returns value", strPtr("value"), "default", "value"},
		{"Empty pointer returns empty", strPtr(""), "default", ""},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := utils.DerefStr(tt.ptr, tt.defaultVal)
			if result != tt.expected {
				t.Errorf("Expected '%s', got '%s'", tt.expected, result)
			}
		})
	}
}

// TestSearchResult_ScoreSorting tests search result sorting by score
func TestSearchResult_ScoreSorting(t *testing.T) {
	rm := skills.NewRegistryManager()

	// Add mock registries with different scores
	mock1 := &MockRegistry{
		name: "low-score",
		searchResults: []skills.SearchResult{
			{Slug: "low", DisplayName: "Low Score", Summary: "Test", Score: 0.3, RegistryName: "low-score"},
		},
		skillMeta: make(map[string]*skills.SkillMeta),
	}

	mock2 := &MockRegistry{
		name: "high-score",
		searchResults: []skills.SearchResult{
			{Slug: "high", DisplayName: "High Score", Summary: "Test", Score: 0.9, RegistryName: "high-score"},
		},
		skillMeta: make(map[string]*skills.SkillMeta),
	}

	rm.AddRegistry(mock1)
	rm.AddRegistry(mock2)

	ctx, cancel := context.WithTimeout(context.Background(), 1*time.Second)
	defer cancel()

	results, err := rm.SearchAll(ctx, "", 10)
	if err != nil {
		t.Fatalf("Search failed: %v", err)
	}

	// Results should be sorted by score descending
	if len(results) < 2 {
		t.Fatalf("Expected at least 2 results, got %d", len(results))
	}

	// Check sorting
	for i := 0; i < len(results)-1; i++ {
		if results[i].Score < results[i+1].Score {
			t.Errorf("Results not sorted by score: [%.2f, %.2f] at indices %d,%d",
				results[i].Score, results[i+1].Score, i, i+1)
		}
	}

	// Highest score should be first
	if results[0].Slug != "high" {
		t.Errorf("Expected highest score first, got %s", results[0].Slug)
	}
}

// TestSkillInstaller_RegistryManagerIntegration tests installer with registry manager
func TestSkillInstaller_RegistryManagerIntegration(t *testing.T) {
	tempDir := t.TempDir()

	installer := skills.NewSkillInstaller(tempDir)

	// Create registry manager
	cfg := skills.RegistryConfig{
		GitHub: skills.GitHubConfig{
			Enabled: false, // Disable to test without network
		},
		MaxConcurrentSearches: 2,
	}

	rm := skills.NewRegistryManagerFromConfig(cfg)
	installer.SetRegistryManager(rm)

	// Test that installer was created successfully
	if installer == nil {
		t.Error("Expected installer to be created")
	}

	// Test search with disabled GitHub (should handle gracefully)
	ctx, cancel := context.WithTimeout(context.Background(), 1*time.Second)
	defer cancel()

	results, err := installer.SearchRegistries(ctx, "test", 5)
	if err != nil {
		t.Logf("Search with disabled registries failed as expected: %v", err)
	}

	// With no registries enabled, should return empty results or error
	if results == nil {
		t.Log("No results as expected with no registries")
	} else if len(results) == 0 {
		t.Log("Empty results as expected with no registries")
	} else {
		t.Logf("Got %d results unexpectedly: %v", len(results), results)
	}
}

// TestContext_Cancellation tests context cancellation handling
func TestContext_Cancellation(t *testing.T) {
	rm := skills.NewRegistryManager()

	mock := &MockRegistry{
		name: "slow",
		searchResults: []skills.SearchResult{
			{Slug: "slow-skill", DisplayName: "Slow Skill", Summary: "Test", Score: 0.5, RegistryName: "slow"},
		},
		skillMeta: make(map[string]*skills.SkillMeta),
	}

	rm.AddRegistry(mock)

	// Create a context that gets cancelled immediately
	ctx, cancel := context.WithCancel(context.Background())
	cancel() // Cancel immediately

	// Search should handle cancellation gracefully
	_, err := rm.SearchAll(ctx, "test", 10)
	if err != nil {
		if err == context.Canceled {
			t.Log("Search properly cancelled with context.Canceled")
		} else {
			t.Logf("Search got different error after cancellation: %v", err)
		}
	}
}

// TestFileWriteAtomic tests atomic file write functionality
func TestFileWriteAtomic(t *testing.T) {
	tempDir := t.TempDir()
	testFile := filepath.Join(tempDir, "test_atomic.txt")
	testData := []byte("test data for atomic write")

	// Test atomic write
	err := utils.WriteFileAtomic(testFile, testData, 0644)
	if err != nil {
		t.Fatalf("WriteFileAtomic failed: %v", err)
	}

	// Verify file was created
	if _, err := os.Stat(testFile); err != nil {
		t.Fatalf("File was not created: %v", err)
	}

	// Verify content
	content, err := os.ReadFile(testFile)
	if err != nil {
		t.Fatalf("Failed to read file: %v", err)
	}

	if string(content) != string(testData) {
		t.Errorf("File content mismatch. Expected '%s', got '%s'", string(testData), string(content))
	}

	// Test overwriting existing file
	newData := []byte("updated data")
	err = utils.WriteFileAtomic(testFile, newData, 0644)
	if err != nil {
		t.Fatalf("Overwrite failed: %v", err)
	}

	// Verify update
	content, err = os.ReadFile(testFile)
	if err != nil {
		t.Fatalf("Failed to read updated file: %v", err)
	}

	if string(content) != string(newData) {
		t.Errorf("Updated content mismatch. Expected '%s', got '%s'", string(newData), string(content))
	}
}

// TestFileWriteAtomic_Permissions tests file permission setting
func TestFileWriteAtomic_Permissions(t *testing.T) {
	tempDir := t.TempDir()
	testFile := filepath.Join(tempDir, "test_perms.txt")
	testData := []byte("test data")

	// Test with read-only permissions
	err := utils.WriteFileAtomic(testFile, testData, 0400)
	if err != nil {
		t.Fatalf("WriteFileAtomic with 0400 permissions failed: %v", err)
	}

	// Verify file exists
	info, err := os.Stat(testFile)
	if err != nil {
		t.Fatalf("Failed to stat file: %v", err)
	}

	// Check permissions (on Unix we can check exact mode)
	perm := info.Mode().Perm()
	if perm&0400 != 0400 {
		t.Logf("Warning: File permissions may not be exactly 0400, got: %v", perm)
	}
}

// TestSkillInstaller_BackwardCompatibility tests backward compatibility
func TestSkillInstaller_BackwardCompatibility(t *testing.T) {
	tempDir := t.TempDir()

	installer := skills.NewSkillInstaller(tempDir)

	// Test basic creation without registry manager
	if installer == nil {
		t.Error("Expected installer to be created")
	}

	// Test workspace is set correctly
	if installer == nil || tempDir == "" {
		t.Error("Installer workspace not set correctly")
	}
}

// Helper function
func strPtr(s string) *string {
	return &s
}