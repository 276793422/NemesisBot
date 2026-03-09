// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package skills

import (
	"context"
)

// MockRegistry is a test implementation of SkillRegistry
type MockRegistry struct {
	name          string
	searchResults []SearchResult
	skillMeta     map[string]*SkillMeta
}

// NewMockRegistry creates a new MockRegistry with the given name
func NewMockRegistry(name string) *MockRegistry {
	return &MockRegistry{
		name:      name,
		skillMeta: make(map[string]*SkillMeta),
	}
}

// AddSearchResult adds a search result to the mock registry
func (m *MockRegistry) AddSearchResult(result SearchResult) {
	m.searchResults = append(m.searchResults, result)
}

// SetSkillMeta sets the metadata for a skill
func (m *MockRegistry) SetSkillMeta(slug string, meta *SkillMeta) {
	m.skillMeta[slug] = meta
}

func (m *MockRegistry) Name() string {
	return m.name
}

func (m *MockRegistry) Search(ctx context.Context, query string, limit int) ([]SearchResult, error) {
	// Simple filter implementation
	var results []SearchResult
	for i, result := range m.searchResults {
		// Only respect limit if it's positive
		if limit > 0 && len(results) >= limit {
			break
		}
		// Simple matching
		if mockContains(result.Slug, query) || mockContains(result.Summary, query) || query == "" {
			results = append(results, result)
		}
		_ = i // unused
	}
	return results, nil
}

func (m *MockRegistry) GetSkillMeta(ctx context.Context, slug string) (*SkillMeta, error) {
	if meta, ok := m.skillMeta[slug]; ok {
		return meta, nil
	}
	return &SkillMeta{Slug: slug, DisplayName: slug, Summary: "Mock skill"}, nil
}

func (m *MockRegistry) DownloadAndInstall(ctx context.Context, slug, version, targetDir string) (*InstallResult, error) {
	// Check if skill metadata exists and if it's marked as malware
	if meta, ok := m.skillMeta[slug]; ok {
		// Return result with malware flag if set
		if meta.IsMalwareBlocked {
			return &InstallResult{
				Version:          version,
				IsMalwareBlocked: true,
				IsSuspicious:     meta.IsSuspicious,
				Summary:          meta.Summary,
			}, nil
		}
	}

	return &InstallResult{
		Version:          version,
		IsMalwareBlocked: false,
		IsSuspicious:     false,
		Summary:          "Mock installation",
	}, nil
}

func mockContains(s, substr string) bool {
	return len(s) >= len(substr) && (s == substr || len(s) > len(substr))
}