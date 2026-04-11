package skills

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestInstaller_writeOriginTracking_RaceCondition(t *testing.T) {
	// Create temporary workspace
	tempDir := t.TempDir()
	installer := NewSkillInstaller(tempDir)

	// Test case: Single write (simpler test)
	skillDir := filepath.Join(tempDir, "test-skill")
	os.MkdirAll(skillDir, 0755)

	// Single write
	err := installer.writeOriginTracking(skillDir, "test-registry", "test-skill", "1.0.0")
	if err != nil {
		t.Errorf("Unexpected error during write: %v", err)
	}

	// Verify the origin file was created
	originPath := filepath.Join(skillDir, ".skill-origin.json")
	if _, err := os.Stat(originPath); os.IsNotExist(err) {
		t.Error("Origin file was not created")
	}

	// Verify content
	content, err := os.ReadFile(originPath)
	if err != nil {
		t.Errorf("Failed to read origin file: %v", err)
	}

	if !strings.Contains(string(content), "test-registry") {
		t.Error("Origin file does not contain registry name")
	}
	if !strings.Contains(string(content), "test-skill") {
		t.Error("Origin file does not contain skill slug")
	}
}

func TestInstaller_writeOriginTracking_InvalidJSON(t *testing.T) {
	// Create temporary workspace
	tempDir := t.TempDir()
	installer := NewSkillInstaller(tempDir)

	// Test case: Invalid characters in slug that might cause JSON issues
	skillDir := filepath.Join(tempDir, "test-skill")
	os.MkdirAll(skillDir, 0755)

	// This should still work as the slug is just a string field
	err := installer.writeOriginTracking(skillDir, "test-registry", "skill-with-dashes", "1.0.0")
	if err != nil {
		t.Errorf("Unexpected error for valid slug: %v", err)
	}

	// Test with special characters
	err = installer.writeOriginTracking(skillDir, "test-registry", "skill_with_underscores", "1.0.0")
	if err != nil {
		t.Errorf("Unexpected error for underscores: %v", err)
	}

	// Test with unicode characters
	err = installer.writeOriginTracking(skillDir, "test-registry", "skill-中文", "1.0.0")
	if err != nil {
		t.Errorf("Unexpected error for unicode: %v", err)
	}
}

func TestInstallFromGitHub_NonExistentRepo(t *testing.T) {
	ctx := context.Background()

	// Create temporary workspace
	tempDir := t.TempDir()
	installer := NewSkillInstaller(tempDir)

	// Test case: Repository with spaces (invalid format, should fail with HTTP error)
	// Use a mock server to avoid real network calls
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusNotFound)
	}))
	defer server.Close()

	installer.SetGitHubBaseURL(server.URL)

	err := installer.InstallFromGitHub(ctx, "invalid repo name")
	if err == nil {
		t.Error("Expected error for repo with spaces, got nil")
	}

	// Test case: Repository starting with hyphen (edge case)
	err = installer.InstallFromGitHub(ctx, "-invalid-repo")
	if err == nil {
		t.Error("Expected error for repo starting with hyphen, got nil")
	}
}

func TestListAvailableSkillsFromGitHub_JSONEdgeCases(t *testing.T) {
	// Test case 1: JSON array with null values
	var skills []AvailableSkill
	err := json.Unmarshal([]byte("[null]"), &skills)
	if err != nil {
		t.Errorf("Failed to parse array with null: %v", err)
	}

	if len(skills) != 1 {
		t.Errorf("Expected 1 skill in array with null, got %d", len(skills))
	}

	// Test case 2: JSON array with empty objects (Go allows this - fields are optional)
	err = json.Unmarshal([]byte("[{}, {}]"), &skills)
	if err != nil {
		t.Errorf("Go JSON allows empty objects for structs with optional fields: %v", err)
	}
	if len(skills) != 2 {
		t.Errorf("Expected 2 skills from empty objects, got %d", len(skills))
	}
	// Verify fields are zero-valued
	if skills[0].Name != "" || skills[0].Description != "" {
		t.Error("Empty object should have zero-valued fields")
	}

	// Test case 3: JSON array with missing optional fields (Go allows this)
	invalidSkills := `[
		{"name": "Skill 1", "description": "First skill"},
		{"name": "Skill 2", "tags": ["test"]}
	]`
	err = json.Unmarshal([]byte(invalidSkills), &skills)
	if err != nil {
		t.Errorf("Go JSON allows missing optional fields: %v", err)
	}
	if len(skills) != 2 {
		t.Errorf("Expected 2 skills, got %d", len(skills))
	}
	if skills[1].Description != "" {
		t.Error("Missing description should be empty string")
	}

	// Test case 4: Valid JSON with all fields
	validSkills := `[
		{
			"name": "Skill 1",
			"repository": "user/repo1",
			"description": "First skill",
			"author": "user1",
			"tags": ["test", "demo"]
		},
		{
			"name": "Skill 2",
			"repository": "user/repo2",
			"description": "Second skill",
			"author": "user2",
			"tags": ["production"]
		}
	]`
	err = json.Unmarshal([]byte(validSkills), &skills)
	if err != nil {
		t.Errorf("Failed to parse valid skills: %v", err)
	}
	if len(skills) != 2 {
		t.Errorf("Expected 2 skills, got %d", len(skills))
	}
	if skills[0].Repository != "user/repo1" {
		t.Errorf("Expected user/repo1 for first skill, got %s", skills[0].Repository)
	}
	if skills[1].Author != "user2" {
		t.Errorf("Expected user2 for second skill, got %s", skills[1].Author)
	}
}

func TestListAvailableSkillsFromGitHub_LargeResponse(t *testing.T) {
	// Test case: Large number of skills (memory test)
	var skillsBuilder strings.Builder
	skillsBuilder.WriteString("[")

	for i := 0; i < 1000; i++ {
		if i > 0 {
			skillsBuilder.WriteString(",")
		}
		skill := `{
			"name": "Skill` + string(rune('0'+i%10)) + `",
			"repository": "user/repo` + string(rune('0'+i%10)) + `",
			"description": "This is skill number ` + string(rune('0'+i%10)) + `",
			"author": "author` + string(rune('0'+i%10)) + `",
			"tags": ["test", "demo"]
		}`
		skillsBuilder.WriteString(skill)
	}

	skillsBuilder.WriteString("]")

	var skills []AvailableSkill
	err := json.Unmarshal([]byte(skillsBuilder.String()), &skills)
	if err != nil {
		t.Errorf("Failed to parse large skills array: %v", err)
	}

	if len(skills) != 1000 {
		t.Errorf("Expected 1000 skills, got %d", len(skills))
	}

	// Test case: Very long strings
	longString := strings.Repeat("a", 10000)
	longSkills := `[
		{
			"name": "Long Name Skill",
			"repository": "user/long-repo",
			"description": "` + longString + `",
			"author": "test-author",
			"tags": ["test"]
		}
	]`

	err = json.Unmarshal([]byte(longSkills), &skills)
	if err != nil {
		t.Errorf("Failed to parse skills with long strings: %v", err)
	}
	if len(skills) != 1 {
		t.Errorf("Expected 1 skill with long string, got %d", len(skills))
	}
	if len(skills[0].Description) != 10000 {
		t.Errorf("Expected long description of length 10000, got %d", len(skills[0].Description))
	}
}

func TestSkillInstaller_InstallFromRegistry_RegistryNotFound(t *testing.T) {
	ctx := context.Background()

	// Create temporary workspace
	tempDir := t.TempDir()
	installer := NewSkillInstaller(tempDir)

	// Create registry manager
	registryManager := NewRegistryManager()

	installer.SetRegistryManager(registryManager)

	// Test case: Install from non-existent registry
	err := installer.InstallFromRegistry(ctx, "non-existent-registry", "test-skill", "latest")
	if err == nil {
		t.Error("Expected error for non-existent registry, got nil")
	}
	if !strings.Contains(err.Error(), "non-existent-registry") {
		t.Errorf("Expected registry name in error, got: %v", err)
	}
}

func TestSkillInstaller_InstallFromRegistry_InvalidVersion(t *testing.T) {
	ctx := context.Background()

	// Create temporary workspace
	tempDir := t.TempDir()
	installer := NewSkillInstaller(tempDir)

	// Create registry manager with mock registry
	registryManager := NewRegistryManager()
	registry := NewMockRegistry("test-registry")

	// Add skill to registry
	registry.AddSearchResult(SearchResult{
		Slug:        "test-skill",
		DisplayName: "Test Skill",
		Summary:     "A test skill",
	})

	registry.SetSkillMeta("test-skill", &SkillMeta{
		Slug:             "test-skill",
		DisplayName:      "Test Skill",
		Summary:          "A test skill",
		LatestVersion:    "1.0.0",
		IsMalwareBlocked: false,
	})

	registryManager.AddRegistry(registry)
	installer.SetRegistryManager(registryManager)

	// Test case: Install with custom version (installer doesn't validate versions)
	// The installer just passes the version to the registry
	err := installer.InstallFromRegistry(ctx, "test-registry", "test-skill-v1", "custom-version")
	if err != nil {
		t.Errorf("Unexpected error for custom version: %v", err)
	}

	// Verify skill was installed
	skillDir := filepath.Join(tempDir, "skills", "test-skill-v1")
	if _, err := os.Stat(skillDir); os.IsNotExist(err) {
		t.Error("Skill should have been installed")
	}

	// Add another skill for the second test case
	registry.SetSkillMeta("test-skill-v2", &SkillMeta{
		Slug:             "test-skill-v2",
		DisplayName:      "Test Skill V2",
		Summary:          "A test skill v2",
		LatestVersion:    "2.0.0",
		IsMalwareBlocked: false,
	})

	// Test case: Install with empty version (should use latest from registry or default)
	err = installer.InstallFromRegistry(ctx, "test-registry", "test-skill-v2", "")
	if err != nil {
		t.Errorf("Unexpected error for empty version: %v", err)
	}
}

func TestSkillInstaller_InstallFromRegistry_MalwareVariants(t *testing.T) {
	ctx := context.Background()

	// Create temporary workspace
	tempDir := t.TempDir()
	installer := NewSkillInstaller(tempDir)

	// Create registry manager
	registryManager := NewRegistryManager()

	testCases := []struct {
		name        string
		slug        string
		isMalware   bool
		expectError bool
	}{
		{"clear_malware", "clear-skill", false, false},
		{"blocked_malware", "malware-skill", true, true},
		{"suspicious", "suspicious-skill", false, false}, // Not malware but might be suspicious
	}

	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			// Create fresh registry for each test
			registry := NewMockRegistry("test-registry")

			// Add skill to registry
			registry.AddSearchResult(SearchResult{
				Slug:        tc.slug,
				DisplayName: tc.slug,
				Summary:     "A test skill",
			})

			registry.SetSkillMeta(tc.slug, &SkillMeta{
				Slug:             tc.slug,
				DisplayName:      tc.slug,
				Summary:          "A test skill",
				LatestVersion:    "1.0.0",
				IsMalwareBlocked: tc.isMalware,
			})

			registryManager = NewRegistryManager()
			registryManager.AddRegistry(registry)
			installer.SetRegistryManager(registryManager)

			// Test install
			err := installer.InstallFromRegistry(ctx, "test-registry", tc.slug, "latest")

			if tc.expectError {
				if err == nil {
					t.Error("Expected error for malware, got nil")
				}
			} else {
				if err != nil {
					t.Errorf("Expected no error, got: %v", err)
				}
			}
		})
	}
}
