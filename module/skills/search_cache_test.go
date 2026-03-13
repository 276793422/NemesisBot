// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package skills

import (
	"testing"
	"time"
)

func TestNewSearchCache(t *testing.T) {
	tests := []struct {
		name   string
		cfg    SearchCacheConfig
		expect func(*testing.T, *SearchCache)
	}{
		{
			name: "default config",
			cfg:  SearchCacheConfig{},
			expect: func(t *testing.T, sc *SearchCache) {
				if sc.maxSize != 50 {
					t.Errorf("expected default maxSize 50, got %d", sc.maxSize)
				}
				if sc.ttl != 5*time.Minute {
					t.Errorf("expected default TTL 5m, got %v", sc.ttl)
				}
				if sc.cache == nil {
					t.Error("cache map should be initialized")
				}
				if sc.lruList == nil {
					t.Error("lruList should be initialized")
				}
			},
		},
		{
			name: "custom config",
			cfg: SearchCacheConfig{
				MaxSize: 100,
				TTL:     10 * time.Minute,
			},
			expect: func(t *testing.T, sc *SearchCache) {
				if sc.maxSize != 100 {
					t.Errorf("expected maxSize 100, got %d", sc.maxSize)
				}
				if sc.ttl != 10*time.Minute {
					t.Errorf("expected TTL 10m, got %v", sc.ttl)
				}
			},
		},
		{
			name: "zero maxSize",
			cfg: SearchCacheConfig{
				MaxSize: 0,
			},
			expect: func(t *testing.T, sc *SearchCache) {
				if sc.maxSize != 50 {
					t.Errorf("expected default maxSize 50, got %d", sc.maxSize)
				}
			},
		},
		{
			name: "zero TTL",
			cfg: SearchCacheConfig{
				TTL: 0,
			},
			expect: func(t *testing.T, sc *SearchCache) {
				if sc.ttl != 5*time.Minute {
					t.Errorf("expected default TTL 5m, got %v", sc.ttl)
				}
			},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			sc := NewSearchCache(tt.cfg)
			if tt.expect != nil {
				tt.expect(t, sc)
			}
		})
	}
}

func TestSearchCache_PutAndGet(t *testing.T) {
	sc := NewSearchCache(SearchCacheConfig{
		MaxSize: 10,
		TTL:     5 * time.Minute,
	})

	results := []SearchResult{
		{
			Score:        0.9,
			Slug:         "skill1",
			DisplayName:  "Skill 1",
			Summary:      "Test skill",
			Version:      "1.0.0",
			RegistryName: "test",
		},
	}

	// Put results
	sc.Put("test query", results)

	// Get exact match
	retrieved, ok := sc.Get("test query", 10)
	if !ok {
		t.Fatal("expected cache hit")
	}

	if len(retrieved) != 1 {
		t.Errorf("expected 1 result, got %d", len(retrieved))
	}

	if retrieved[0].Slug != "skill1" {
		t.Errorf("expected slug 'skill1', got '%s'", retrieved[0].Slug)
	}
}

func TestSearchCache_GetMiss(t *testing.T) {
	sc := NewSearchCache(SearchCacheConfig{
		MaxSize: 10,
		TTL:     5 * time.Minute,
	})

	// Get non-existent key
	_, ok := sc.Get("non-existent", 10)
	if ok {
		t.Error("expected cache miss")
	}

	// Check stats
	stats := sc.Stats()
	if stats.MissCount != 1 {
		t.Errorf("expected 1 miss, got %d", stats.MissCount)
	}
}

func TestSearchCache_TTLExpiry(t *testing.T) {
	sc := NewSearchCache(SearchCacheConfig{
		MaxSize: 10,
		TTL:     10 * time.Millisecond,
	})

	results := []SearchResult{
		{Score: 0.9, Slug: "skill1"},
	}

	sc.Put("test", results)

	// Wait for expiry
	time.Sleep(15 * time.Millisecond)

	// Should miss due to expiry
	_, ok := sc.Get("test", 10)
	if ok {
		t.Error("expected cache miss due to TTL expiry")
	}

	stats := sc.Stats()
	if stats.MissCount != 1 {
		t.Errorf("expected 1 miss, got %d", stats.MissCount)
	}

	if stats.Size != 0 {
		t.Errorf("expected cache size 0 after expiry, got %d", stats.Size)
	}
}

func TestSearchCache_LRU_Eviction(t *testing.T) {
	sc := NewSearchCache(SearchCacheConfig{
		MaxSize: 3,
		TTL:     5 * time.Minute,
	})

	results := []SearchResult{{Score: 0.9, Slug: "skill"}}

	// Fill cache to max
	sc.Put("query1", results)
	sc.Put("query2", results)
	sc.Put("query3", results)

	stats := sc.Stats()
	if stats.Size != 3 {
		t.Errorf("expected size 3, got %d", stats.Size)
	}

	// Add one more - should evict query1
	sc.Put("query4", results)

	stats = sc.Stats()
	if stats.Size != 3 {
		t.Errorf("expected size 3 after eviction, got %d", stats.Size)
	}

	// query1 should be evicted
	_, ok := sc.Get("query1", 10)
	if ok {
		t.Error("expected query1 to be evicted")
	}

	// query4 should be present
	_, ok = sc.Get("query4", 10)
	if !ok {
		t.Error("expected query4 to be present")
	}
}

func TestSearchCache_UpdateLRU(t *testing.T) {
	sc := NewSearchCache(SearchCacheConfig{
		MaxSize: 3,
		TTL:     5 * time.Minute,
	})

	results := []SearchResult{{Score: 0.9, Slug: "skill"}}

	// Add entries
	sc.Put("query1", results)
	sc.Put("query2", results)
	sc.Put("query3", results)

	// Access query1 to make it recently used
	sc.Get("query1", 10)

	// Add query4 - should evict query2 (least recently used after query1 access)
	sc.Put("query4", results)

	// query1 should still be present
	_, ok := sc.Get("query1", 10)
	if !ok {
		t.Error("expected query1 to be present")
	}

	// query2 should be evicted
	_, ok = sc.Get("query2", 10)
	if ok {
		t.Error("expected query2 to be evicted")
	}
}

func TestSearchCache_UpdateExisting(t *testing.T) {
	sc := NewSearchCache(SearchCacheConfig{
		MaxSize: 10,
		TTL:     5 * time.Minute,
	})

	results1 := []SearchResult{{Score: 0.9, Slug: "skill1"}}
	results2 := []SearchResult{{Score: 0.8, Slug: "skill2"}}

	// Put initial results
	sc.Put("query", results1)

	stats := sc.Stats()
	if stats.Size != 1 {
		t.Errorf("expected size 1, got %d", stats.Size)
	}

	// Update with new results
	sc.Put("query", results2)

	stats = sc.Stats()
	if stats.Size != 1 {
		t.Errorf("expected size 1 after update, got %d", stats.Size)
	}

	// Get updated results
	retrieved, _ := sc.Get("query", 10)
	if len(retrieved) != 1 {
		t.Errorf("expected 1 result, got %d", len(retrieved))
	}

	if retrieved[0].Slug != "skill2" {
		t.Errorf("expected slug 'skill2', got '%s'", retrieved[0].Slug)
	}
}

func TestSearchCache_GetWithLimit(t *testing.T) {
	sc := NewSearchCache(SearchCacheConfig{
		MaxSize: 10,
		TTL:     5 * time.Minute,
	})

	results := []SearchResult{
		{Score: 0.9, Slug: "skill1"},
		{Score: 0.8, Slug: "skill2"},
		{Score: 0.7, Slug: "skill3"},
	}

	sc.Put("query", results)

	// Get with limit
	retrieved, ok := sc.Get("query", 2)
	if !ok {
		t.Fatal("expected cache hit")
	}

	if len(retrieved) != 2 {
		t.Errorf("expected 2 results with limit, got %d", len(retrieved))
	}

	// Get without limit
	retrieved, ok = sc.Get("query", 0)
	if !ok {
		t.Fatal("expected cache hit")
	}

	if len(retrieved) != 3 {
		t.Errorf("expected 3 results without limit, got %d", len(retrieved))
	}
}

func TestSearchCache_Clear(t *testing.T) {
	sc := NewSearchCache(SearchCacheConfig{
		MaxSize: 10,
		TTL:     5 * time.Minute,
	})

	results := []SearchResult{{Score: 0.9, Slug: "skill"}}

	// Add some entries
	sc.Put("query1", results)
	sc.Put("query2", results)
	sc.Put("query3", results)

	stats := sc.Stats()
	if stats.Size != 3 {
		t.Errorf("expected size 3, got %d", stats.Size)
	}

	// Clear cache
	sc.Clear()

	stats = sc.Stats()
	if stats.Size != 0 {
		t.Errorf("expected size 0 after clear, got %d", stats.Size)
	}

	if stats.HitCount != 0 {
		t.Errorf("expected hit count 0 after clear, got %d", stats.HitCount)
	}

	if stats.MissCount != 0 {
		t.Errorf("expected miss count 0 after clear, got %d", stats.MissCount)
	}
}

func TestSearchCache_Stats(t *testing.T) {
	sc := NewSearchCache(SearchCacheConfig{
		MaxSize: 10,
		TTL:     5 * time.Minute,
	})

	results := []SearchResult{{Score: 0.9, Slug: "skill"}}

	// Initial stats
	stats := sc.Stats()
	if stats.Size != 0 {
		t.Errorf("expected initial size 0, got %d", stats.Size)
	}
	if stats.HitCount != 0 {
		t.Errorf("expected initial hit count 0, got %d", stats.HitCount)
	}
	if stats.MissCount != 0 {
		t.Errorf("expected initial miss count 0, got %d", stats.MissCount)
	}
	if stats.HitRate != 0.0 {
		t.Errorf("expected initial hit rate 0.0, got %f", stats.HitRate)
	}

	// Add entry
	sc.Put("query", results)

	// Hit
	sc.Get("query", 10)

	stats = sc.Stats()
	if stats.HitCount != 1 {
		t.Errorf("expected hit count 1, got %d", stats.HitCount)
	}
	if stats.HitRate != 1.0 {
		t.Errorf("expected hit rate 1.0, got %f", stats.HitRate)
	}

	// Miss
	sc.Get("non-existent", 10)

	stats = sc.Stats()
	if stats.MissCount != 1 {
		t.Errorf("expected miss count 1, got %d", stats.MissCount)
	}

	expectedRate := 0.5
	if stats.HitRate != expectedRate {
		t.Errorf("expected hit rate %f, got %f", expectedRate, stats.HitRate)
	}
}

func TestBuildTrigrams(t *testing.T) {
	tests := []struct {
		name        string
		input       string
		expectCount int
		expectEmpty bool
	}{
		{
			name:        "empty string",
			input:       "",
			expectEmpty: true,
		},
		{
			name:        "single char",
			input:       "a",
			expectEmpty: true,
		},
		{
			name:        "two chars",
			input:       "ab",
			expectEmpty: true,
		},
		{
			name:        "three chars",
			input:       "abc",
			expectCount: 1,
		},
		{
			name:        "four chars",
			input:       "abcd",
			expectCount: 2,
		},
		{
			name:        "word",
			input:       "hello",
			expectCount: 3,
		},
		{
			name:        "with spaces",
			input:       "hello world",
			expectCount: 9, // "hel", "ell", "llo", "lo ", "o w", " wo", "wor", "orl", "rld"
		},
		{
			name:        "uppercase",
			input:       "HELLO",
			expectCount: 3,
		},
		{
			name:        "mixed case",
			input:       "Hello",
			expectCount: 3,
		},
		{
			name:        "with duplicates",
			input:       "aaaa",
			expectCount: 1, // Should deduplicate
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			trigrams := buildTrigrams(tt.input)

			if tt.expectEmpty {
				if len(trigrams) != 0 {
					t.Errorf("expected empty trigrams, got %d", len(trigrams))
				}
			} else {
				if len(trigrams) != tt.expectCount {
					t.Errorf("expected %d trigrams, got %d", tt.expectCount, len(trigrams))
				}
			}
		})
	}
}

func TestJaccardSimilarity(t *testing.T) {
	tests := []struct {
		name     string
		a        []uint32
		b        []uint32
		expected float64
	}{
		{
			name:     "both empty",
			a:        []uint32{},
			b:        []uint32{},
			expected: 1.0,
		},
		{
			name:     "one empty",
			a:        []uint32{},
			b:        []uint32{1, 2, 3},
			expected: 0.0,
		},
		{
			name:     "identical",
			a:        []uint32{1, 2, 3},
			b:        []uint32{1, 2, 3},
			expected: 1.0,
		},
		{
			name:     "no overlap",
			a:        []uint32{1, 2, 3},
			b:        []uint32{4, 5, 6},
			expected: 0.0,
		},
		{
			name:     "partial overlap",
			a:        []uint32{1, 2, 3},
			b:        []uint32{2, 3, 4},
			expected: 2.0 / 4.0, // intersection=2, union=4
		},
		{
			name:     "one is subset",
			a:        []uint32{1, 2},
			b:        []uint32{1, 2, 3},
			expected: 2.0 / 3.0, // intersection=2, union=3
		},
		{
			name:     "large overlap",
			a:        []uint32{1, 2, 3, 4, 5},
			b:        []uint32{1, 2, 3, 6, 7},
			expected: 3.0 / 7.0, // intersection=3, union=7
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			similarity := jaccardSimilarity(tt.a, tt.b)
			if similarity != tt.expected {
				t.Errorf("expected similarity %f, got %f", tt.expected, similarity)
			}
		})
	}
}

func TestToLower(t *testing.T) {
	tests := []struct {
		name     string
		input    string
		expected string
	}{
		{"empty", "", ""},
		{"lowercase", "hello", "hello"},
		{"uppercase", "HELLO", "hello"},
		{"mixed", "HeLLo", "hello"},
		{"with spaces", "Hello World", "hello world"},
		{"with numbers", "Hello123", "hello123"},
		{"special chars", "Hello@World!", "hello@world!"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := toLower(tt.input)
			if result != tt.expected {
				t.Errorf("toLower(%q) = %q, want %q", tt.input, result, tt.expected)
			}
		})
	}
}

func TestSearchCache_AccessCount(t *testing.T) {
	sc := NewSearchCache(SearchCacheConfig{
		MaxSize: 10,
		TTL:     5 * time.Minute,
	})

	results := []SearchResult{{Score: 0.9, Slug: "skill"}}
	sc.Put("query", results)

	// Access multiple times
	for i := 0; i < 5; i++ {
		sc.Get("query", 10)
	}

	// We can't directly access the entry, but we can verify it still works
	retrieved, ok := sc.Get("query", 10)
	if !ok {
		t.Error("expected cache hit")
	}
	if len(retrieved) != 1 {
		t.Errorf("expected 1 result, got %d", len(retrieved))
	}
}

func TestCacheEntry_Structure(t *testing.T) {
	now := time.Now()
	entry := &CacheEntry{
		Results:      []SearchResult{{Score: 0.9, Slug: "skill"}},
		Trigrams:     []uint32{1, 2, 3},
		CreatedAt:    now,
		LastAccessAt: now,
		AccessCount:  5,
	}

	if len(entry.Results) != 1 {
		t.Error("Results field not working")
	}
	if len(entry.Trigrams) != 3 {
		t.Error("Trigrams field not working")
	}
	if entry.CreatedAt.IsZero() {
		t.Error("CreatedAt field not working")
	}
	if entry.LastAccessAt.IsZero() {
		t.Error("LastAccessAt field not working")
	}
	if entry.AccessCount != 5 {
		t.Error("AccessCount field not working")
	}
}

func TestCacheStats_Structure(t *testing.T) {
	stats := CacheStats{
		Size:      5,
		MaxSize:   10,
		HitCount:  7,
		MissCount: 3,
		HitRate:   0.7,
	}

	if stats.Size != 5 {
		t.Error("Size field not working")
	}
	if stats.MaxSize != 10 {
		t.Error("MaxSize field not working")
	}
	if stats.HitCount != 7 {
		t.Error("HitCount field not working")
	}
	if stats.MissCount != 3 {
		t.Error("MissCount field not working")
	}
	if stats.HitRate != 0.7 {
		t.Error("HitRate field not working")
	}
}

func TestSearchCache_ConcurrentAccess(t *testing.T) {
	sc := NewSearchCache(SearchCacheConfig{
		MaxSize: 100,
		TTL:     5 * time.Minute,
	})

	results := []SearchResult{{Score: 0.9, Slug: "skill"}}

	// Concurrent puts and gets
	done := make(chan bool)

	for i := 0; i < 10; i++ {
		go func(idx int) {
			query := "query" + string(rune('0'+idx))
			sc.Put(query, results)
			sc.Get(query, 10)
			done <- true
		}(i)
	}

	// Wait for all goroutines
	for i := 0; i < 10; i++ {
		<-done
	}

	// Verify cache is still functional
	stats := sc.Stats()
	if stats.Size != 10 {
		t.Errorf("expected size 10, got %d", stats.Size)
	}
}
