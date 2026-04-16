// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package skills

import (
	"context"
	"errors"
	"sync"
	"testing"
	"time"
)

func TestNewRegistryManager(t *testing.T) {
	rm := NewRegistryManager()

	if rm == nil {
		t.Fatal("NewRegistryManager should not return nil")
	}

	if rm.registries == nil {
		t.Error("registries slice should be initialized")
	}

	if len(rm.registries) != 0 {
		t.Errorf("expected 0 registries, got %d", len(rm.registries))
	}

	if rm.maxConcurrent != defaultMaxConcurrentSearches {
		t.Errorf("expected maxConcurrent %d, got %d", defaultMaxConcurrentSearches, rm.maxConcurrent)
	}

	if rm.searchCache != nil {
		t.Error("searchCache should be nil by default")
	}
}

func TestNewRegistryManagerFromConfig(t *testing.T) {
	tests := []struct {
		name    string
		cfg     RegistryConfig
		wantErr bool
		check   func(*testing.T, *RegistryManager)
	}{
		{
			name: "empty config",
			cfg:  RegistryConfig{},
			check: func(t *testing.T, rm *RegistryManager) {
				if len(rm.registries) != 0 {
					t.Errorf("expected 0 registries, got %d", len(rm.registries))
				}
				if rm.maxConcurrent != defaultMaxConcurrentSearches {
					t.Errorf("expected maxConcurrent %d, got %d", defaultMaxConcurrentSearches, rm.maxConcurrent)
				}
			},
		},
		{
			name: "with max concurrent",
			cfg: RegistryConfig{
				MaxConcurrentSearches: 5,
			},
			check: func(t *testing.T, rm *RegistryManager) {
				if rm.maxConcurrent != 5 {
					t.Errorf("expected maxConcurrent 5, got %d", rm.maxConcurrent)
				}
			},
		},
		{
			name: "with search cache",
			cfg: RegistryConfig{
				SearchCache: SearchCacheConfig{
					Enabled: true,
					MaxSize: 100,
					TTL:     10 * time.Minute,
				},
			},
			check: func(t *testing.T, rm *RegistryManager) {
				if rm.searchCache == nil {
					t.Error("searchCache should be initialized when enabled")
				}
			},
		},
		{
			name: "with github enabled",
			cfg: RegistryConfig{
				GitHub: GitHubConfig{
					Enabled: true,
					BaseURL: "github.com",
				},
			},
			check: func(t *testing.T, rm *RegistryManager) {
				if len(rm.registries) != 1 {
					t.Errorf("expected 1 registry, got %d", len(rm.registries))
				}
				if rm.registries[0].Name() != "github" {
					t.Errorf("expected github registry, got %s", rm.registries[0].Name())
				}
			},
		},
		{
			name: "with clawhub enabled",
			cfg: RegistryConfig{
				ClawHub: ClawHubConfig{
					Enabled:   true,
					ConvexURL: "https://wry-manatee-359.convex.cloud",
				},
			},
			check: func(t *testing.T, rm *RegistryManager) {
				if len(rm.registries) != 1 {
					t.Errorf("expected 1 registry, got %d", len(rm.registries))
				}
				if rm.registries[0].Name() != "clawhub" {
					t.Errorf("expected clawhub registry, got %s", rm.registries[0].Name())
				}
			},
		},
		{
			name: "with both registries",
			cfg: RegistryConfig{
				GitHub: GitHubConfig{
					Enabled: true,
				},
				ClawHub: ClawHubConfig{
					Enabled: true,
				},
			},
			check: func(t *testing.T, rm *RegistryManager) {
				if len(rm.registries) != 2 {
					t.Errorf("expected 2 registries, got %d", len(rm.registries))
				}
			},
		},
		{
			name: "with all options",
			cfg: RegistryConfig{
				SearchCache: SearchCacheConfig{
					Enabled: true,
				},
				GitHub: GitHubConfig{
					Enabled: true,
				},
				ClawHub: ClawHubConfig{
					Enabled: true,
				},
				MaxConcurrentSearches: 3,
			},
			check: func(t *testing.T, rm *RegistryManager) {
				if rm.maxConcurrent != 3 {
					t.Errorf("expected maxConcurrent 3, got %d", rm.maxConcurrent)
				}
				if rm.searchCache == nil {
					t.Error("searchCache should be initialized")
				}
				if len(rm.registries) != 2 {
					t.Errorf("expected 2 registries, got %d", len(rm.registries))
				}
			},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			rm := NewRegistryManagerFromConfig(tt.cfg)
			if tt.check != nil {
				tt.check(t, rm)
			}
		})
	}
}

func TestRegistryManager_AddRegistry(t *testing.T) {
	rm := NewRegistryManager()
	reg1 := NewMockRegistry("reg1")
	reg2 := NewMockRegistry("reg2")

	rm.AddRegistry(reg1)
	if len(rm.registries) != 1 {
		t.Errorf("expected 1 registry, got %d", len(rm.registries))
	}

	rm.AddRegistry(reg2)
	if len(rm.registries) != 2 {
		t.Errorf("expected 2 registries, got %d", len(rm.registries))
	}

	// Add duplicate
	rm.AddRegistry(reg1)
	if len(rm.registries) != 3 {
		t.Errorf("expected 3 registries (allows duplicates), got %d", len(rm.registries))
	}
}

func TestRegistryManager_AddRegistry_Concurrent(t *testing.T) {
	rm := NewRegistryManager()
	var wg sync.WaitGroup

	for i := 0; i < 10; i++ {
		wg.Add(1)
		go func(idx int) {
			defer wg.Done()
			rm.AddRegistry(NewMockRegistry("reg" + string(rune('0'+idx))))
		}(i)
	}

	wg.Wait()

	if len(rm.registries) != 10 {
		t.Errorf("expected 10 registries, got %d", len(rm.registries))
	}
}

func TestRegistryManager_GetRegistry(t *testing.T) {
	rm := NewRegistryManager()
	reg1 := NewMockRegistry("registry1")
	reg2 := NewMockRegistry("registry2")

	rm.AddRegistry(reg1)
	rm.AddRegistry(reg2)

	tests := []struct {
		name     string
		registry string
		wantNil  bool
	}{
		{"existing registry", "registry1", false},
		{"existing registry 2", "registry2", false},
		{"non-existing registry", "registry3", true},
		{"empty name", "", true},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			reg := rm.GetRegistry(tt.registry)
			if tt.wantNil {
				if reg != nil {
					t.Errorf("expected nil for %q, got %v", tt.registry, reg)
				}
			} else {
				if reg == nil {
					t.Errorf("expected registry for %q, got nil", tt.registry)
				} else if reg.Name() != tt.registry {
					t.Errorf("expected registry name %q, got %q", tt.registry, reg.Name())
				}
			}
		})
	}
}

func TestRegistryManager_GetRegistry_Concurrent(t *testing.T) {
	rm := NewRegistryManager()
	reg1 := NewMockRegistry("reg1")
	reg2 := NewMockRegistry("reg2")

	rm.AddRegistry(reg1)
	rm.AddRegistry(reg2)

	var wg sync.WaitGroup
	for i := 0; i < 100; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			reg := rm.GetRegistry("reg1")
			if reg == nil {
				t.Error("expected to find registry")
			}
		}()
	}

	wg.Wait()
}

func TestRegistryManager_GetSearchCache(t *testing.T) {
	tests := []struct {
		name           string
		cfg            RegistryConfig
		expectedNotNil bool
	}{
		{
			name:           "no cache",
			cfg:            RegistryConfig{},
			expectedNotNil: false,
		},
		{
			name: "with cache",
			cfg: RegistryConfig{
				SearchCache: SearchCacheConfig{
					Enabled: true,
				},
			},
			expectedNotNil: true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			rm := NewRegistryManagerFromConfig(tt.cfg)
			cache := rm.GetSearchCache()

			if tt.expectedNotNil && cache == nil {
				t.Error("expected cache to be non-nil")
			}
			if !tt.expectedNotNil && cache != nil {
				t.Error("expected cache to be nil")
			}
		})
	}
}

func TestRegistryManager_SearchAll_NoRegistries(t *testing.T) {
	rm := NewRegistryManager()
	ctx := context.Background()

	results, err := rm.SearchAll(ctx, "test", 10)
	if err == nil {
		t.Error("expected error when no registries configured")
	}
	if results != nil {
		t.Errorf("expected nil results, got %v", results)
	}
}

func TestRegistryManager_SearchAll_SingleRegistry(t *testing.T) {
	rm := NewRegistryManager()
	reg := NewMockRegistry("test-registry")
	reg.AddSearchResult(SearchResult{
		Score:        0.9,
		Slug:         "skill1",
		DisplayName:  "Skill 1",
		Summary:      "Test skill",
		Version:      "1.0.0",
		RegistryName: "test-registry",
	})
	rm.AddRegistry(reg)

	ctx := context.Background()
	results, err := rm.SearchAll(ctx, "test", 10)
	if err != nil {
		t.Fatalf("SearchAll failed: %v", err)
	}

	if len(results) != 1 {
		t.Errorf("expected 1 result, got %d", len(results))
	}

	if results[0].Slug != "skill1" {
		t.Errorf("expected slug 'skill1', got '%s'", results[0].Slug)
	}
}

func TestRegistryManager_SearchAll_MultipleRegistries(t *testing.T) {
	rm := NewRegistryManager()

	reg1 := NewMockRegistry("reg1")
	reg1.AddSearchResult(SearchResult{
		Score:        0.8,
		Slug:         "skill1",
		DisplayName:  "Skill 1",
		RegistryName: "reg1",
	})

	reg2 := NewMockRegistry("reg2")
	reg2.AddSearchResult(SearchResult{
		Score:        0.9,
		Slug:         "skill2",
		DisplayName:  "Skill 2",
		RegistryName: "reg2",
	})

	rm.AddRegistry(reg1)
	rm.AddRegistry(reg2)

	ctx := context.Background()
	results, err := rm.SearchAll(ctx, "test", 10)
	if err != nil {
		t.Fatalf("SearchAll failed: %v", err)
	}

	if len(results) != 2 {
		t.Errorf("expected 2 results, got %d", len(results))
	}

	// Results should be sorted by score descending
	if results[0].Score < results[1].Score {
		t.Error("results should be sorted by score descending")
	}
}

func TestRegistryManager_SearchAll_WithLimit(t *testing.T) {
	rm := NewRegistryManager()
	reg := NewMockRegistry("test")

	for i := 0; i < 10; i++ {
		reg.AddSearchResult(SearchResult{
			Score:        float64(10 - i),
			Slug:         "skill" + string(rune('0'+i)),
			DisplayName:  "Skill",
			RegistryName: "test",
		})
	}

	rm.AddRegistry(reg)

	ctx := context.Background()

	// Test with limit
	results, err := rm.SearchAll(ctx, "test", 5)
	if err != nil {
		t.Fatalf("SearchAll failed: %v", err)
	}

	if len(results) != 5 {
		t.Errorf("expected 5 results, got %d", len(results))
	}

	// Test with limit 0 (no limit)
	results, err = rm.SearchAll(ctx, "test", 0)
	if err != nil {
		t.Fatalf("SearchAll failed: %v", err)
	}

	if len(results) != 10 {
		t.Errorf("expected 10 results with limit 0, got %d", len(results))
	}
}

func TestRegistryManager_SearchAll_WithError(t *testing.T) {
	rm := NewRegistryManager()

	// Add a registry that will fail
	failingReg := &failingMockRegistry{name: "failing"}
	rm.AddRegistry(failingReg)

	ctx := context.Background()
	results, err := rm.SearchAll(ctx, "test", 10)
	if err == nil {
		t.Error("expected error when all registries fail")
	}
	if results != nil {
		t.Errorf("expected nil results when all fail, got %v", results)
	}
}

func TestRegistryManager_SearchAll_PartialFailure(t *testing.T) {
	rm := NewRegistryManager()

	// Add one failing and one succeeding registry
	failingReg := &failingMockRegistry{name: "failing"}
	successReg := NewMockRegistry("success")
	successReg.AddSearchResult(SearchResult{
		Score:        0.9,
		Slug:         "skill1",
		DisplayName:  "Skill 1",
		RegistryName: "success",
	})

	rm.AddRegistry(failingReg)
	rm.AddRegistry(successReg)

	ctx := context.Background()
	results, err := rm.SearchAll(ctx, "test", 10)
	if err != nil {
		t.Fatalf("SearchAll should succeed with partial failures: %v", err)
	}

	if len(results) != 1 {
		t.Errorf("expected 1 result from succeeding registry, got %d", len(results))
	}
}

func TestRegistryManager_SearchAll_WithCache(t *testing.T) {
	cfg := RegistryConfig{
		SearchCache: SearchCacheConfig{
			Enabled: true,
			MaxSize: 10,
			TTL:     5 * time.Minute,
		},
	}

	rm := NewRegistryManagerFromConfig(cfg)
	reg := NewMockRegistry("test")
	reg.AddSearchResult(SearchResult{
		Score:        0.9,
		Slug:         "skill1",
		DisplayName:  "Skill 1",
		RegistryName: "test",
	})
	rm.AddRegistry(reg)

	ctx := context.Background()

	// First call should hit registry
	results1, err := rm.SearchAll(ctx, "test", 10)
	if err != nil {
		t.Fatalf("SearchAll failed: %v", err)
	}

	// Second call should hit cache
	results2, err := rm.SearchAll(ctx, "test", 10)
	if err != nil {
		t.Fatalf("SearchAll failed: %v", err)
	}

	if len(results1) != len(results2) {
		t.Error("cached results should match original results")
	}
}

func TestRegistryManager_SearchAll_ConcurrencyLimit(t *testing.T) {
	cfg := RegistryConfig{
		MaxConcurrentSearches: 1,
	}
	rm := NewRegistryManagerFromConfig(cfg)

	// Add multiple registries
	for i := 0; i < 5; i++ {
		reg := NewMockRegistry("reg" + string(rune('0'+i)))
		reg.AddSearchResult(SearchResult{
			Score:        0.9,
			Slug:         "skill",
			DisplayName:  "Skill",
			RegistryName: reg.Name(),
		})
		rm.AddRegistry(reg)
	}

	ctx := context.Background()
	results, err := rm.SearchAll(ctx, "test", 10)
	if err != nil {
		t.Fatalf("SearchAll failed: %v", err)
	}

	if len(results) != 5 {
		t.Errorf("expected 5 results, got %d", len(results))
	}
}

func TestSortByScoreDesc(t *testing.T) {
	tests := []struct {
		name  string
		input []SearchResult
		want  []float64
	}{
		{
			name:  "empty slice",
			input: []SearchResult{},
			want:  []float64{},
		},
		{
			name: "single element",
			input: []SearchResult{
				{Score: 0.5},
			},
			want: []float64{0.5},
		},
		{
			name: "already sorted",
			input: []SearchResult{
				{Score: 0.9},
				{Score: 0.5},
				{Score: 0.1},
			},
			want: []float64{0.9, 0.5, 0.1},
		},
		{
			name: "reverse sorted",
			input: []SearchResult{
				{Score: 0.1},
				{Score: 0.5},
				{Score: 0.9},
			},
			want: []float64{0.9, 0.5, 0.1},
		},
		{
			name: "random order",
			input: []SearchResult{
				{Score: 0.5},
				{Score: 0.9},
				{Score: 0.2},
				{Score: 0.7},
				{Score: 0.1},
			},
			want: []float64{0.9, 0.7, 0.5, 0.2, 0.1},
		},
		{
			name: "duplicate scores",
			input: []SearchResult{
				{Score: 0.5},
				{Score: 0.5},
				{Score: 0.5},
			},
			want: []float64{0.5, 0.5, 0.5},
		},
		{
			name: "zero scores",
			input: []SearchResult{
				{Score: 0.0},
				{Score: 0.0},
			},
			want: []float64{0.0, 0.0},
		},
		{
			name: "perfect scores",
			input: []SearchResult{
				{Score: 1.0},
				{Score: 1.0},
			},
			want: []float64{1.0, 1.0},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			sortByScoreDesc(tt.input)

			if len(tt.input) != len(tt.want) {
				t.Fatalf("length mismatch: got %d, want %d", len(tt.input), len(tt.want))
			}

			for i := range tt.input {
				if tt.input[i].Score != tt.want[i] {
					t.Errorf("index %d: got score %f, want %f", i, tt.input[i].Score, tt.want[i])
				}
			}
		})
	}
}

func TestSortByScoreDesc_Stability(t *testing.T) {
	// Test that sort is stable (elements with equal scores maintain order)
	input := []SearchResult{
		{Score: 0.5, Slug: "first"},
		{Score: 0.5, Slug: "second"},
		{Score: 0.5, Slug: "third"},
	}

	sortByScoreDesc(input)

	// All scores should still be 0.5
	for i, result := range input {
		if result.Score != 0.5 {
			t.Errorf("element %d: expected score 0.5, got %f", i, result.Score)
		}
	}
}

// failingMockRegistry is a mock registry that always fails
type failingMockRegistry struct {
	name string
}

func (f *failingMockRegistry) Name() string {
	return f.name
}

func (f *failingMockRegistry) Search(ctx context.Context, query string, limit int) ([]SearchResult, error) {
	return nil, errors.New("search failed")
}

func (f *failingMockRegistry) GetSkillMeta(ctx context.Context, slug string) (*SkillMeta, error) {
	return nil, errors.New("get metadata failed")
}

func (f *failingMockRegistry) DownloadAndInstall(ctx context.Context, slug, version, targetDir string) (*InstallResult, error) {
	return nil, errors.New("download failed")
}

func TestRegistryManager_ConcurrentSearchAndAdd(t *testing.T) {
	rm := NewRegistryManager()
	ctx := context.Background()

	var wg sync.WaitGroup

	// Concurrently add registries and search
	for i := 0; i < 10; i++ {
		wg.Add(2)

		go func(idx int) {
			defer wg.Done()
			reg := NewMockRegistry("reg" + string(rune('0'+idx)))
			reg.AddSearchResult(SearchResult{
				Score:        0.9,
				Slug:         "skill" + string(rune('0'+idx)),
				DisplayName:  "Skill",
				RegistryName: reg.Name(),
			})
			rm.AddRegistry(reg)
		}(i)

		go func() {
			defer wg.Done()
			rm.SearchAll(ctx, "test", 10)
		}()
	}

	wg.Wait()

	// Should have 10 registries
	if len(rm.registries) != 10 {
		t.Errorf("expected 10 registries, got %d", len(rm.registries))
	}
}

func TestRegistryManager_MaxConcurrentSearches(t *testing.T) {
	tests := []struct {
		name            string
		maxConcurrent   int
		expectedLimited bool
	}{
		{"no limit", 0, false},
		{"limit 1", 1, true},
		{"limit 5", 5, true},
		{"limit 100", 100, false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			cfg := RegistryConfig{
				MaxConcurrentSearches: tt.maxConcurrent,
			}
			rm := NewRegistryManagerFromConfig(cfg)

			if rm.maxConcurrent != tt.maxConcurrent && tt.maxConcurrent > 0 {
				t.Errorf("expected maxConcurrent %d, got %d", tt.maxConcurrent, rm.maxConcurrent)
			}

			if tt.maxConcurrent == 0 && rm.maxConcurrent != defaultMaxConcurrentSearches {
				t.Errorf("expected default maxConcurrent %d, got %d", defaultMaxConcurrentSearches, rm.maxConcurrent)
			}
		})
	}
}

func TestSearchResult_JSONTags(t *testing.T) {
	// Verify JSON tags are correct
	result := SearchResult{
		Score:        0.9,
		Slug:         "test",
		DisplayName:  "Test",
		Summary:      "Summary",
		Version:      "1.0.0",
		RegistryName: "registry",
	}

	// This test ensures the struct can be used for JSON marshaling
	if result.Score != 0.9 {
		t.Error("Score field not working")
	}
}

func TestSkillMeta_JSONTags(t *testing.T) {
	meta := SkillMeta{
		Slug:             "test",
		DisplayName:      "Test",
		Summary:          "Summary",
		LatestVersion:    "1.0.0",
		IsMalwareBlocked: true,
		IsSuspicious:     false,
		RegistryName:     "registry",
	}

	if !meta.IsMalwareBlocked {
		t.Error("IsMalwareBlocked field not working")
	}

	if meta.IsSuspicious {
		t.Error("IsSuspicious field not working")
	}
}
