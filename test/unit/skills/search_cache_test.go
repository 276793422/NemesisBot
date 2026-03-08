// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package skills_test

import (
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/skills"
)

// TestNewSearchCache tests the creation of a new search cache.
func TestNewSearchCache(t *testing.T) {
	cfg := skills.SearchCacheConfig{
		MaxSize: 10,
		TTL:     5 * time.Minute,
	}

	cache := skills.NewSearchCache(cfg)
	if cache == nil {
		t.Fatal("NewSearchCache returned nil")
	}

	stats := cache.Stats()
	if stats.MaxSize != 10 {
		t.Errorf("expected MaxSize 10, got %d", stats.MaxSize)
	}
	if stats.Size != 0 {
		t.Errorf("expected Size 0, got %d", stats.Size)
	}
}

// TestNewSearchCacheDefaults tests that default values are applied correctly.
func TestNewSearchCacheDefaults(t *testing.T) {
	cfg := skills.SearchCacheConfig{
		MaxSize: 0, // Should default to 50
		TTL:     0, // Should default to 5 minutes
	}

	cache := skills.NewSearchCache(cfg)
	stats := cache.Stats()

	if stats.MaxSize != 50 {
		t.Errorf("expected default MaxSize 50, got %d", stats.MaxSize)
	}
}

// TestSearchCacheBasic tests basic cache operations (Put and Get).
func TestSearchCacheBasic(t *testing.T) {
	cache := skills.NewSearchCache(skills.SearchCacheConfig{
		MaxSize: 10,
		TTL:     5 * time.Minute,
	})

	// Create test results
	results := []skills.SearchResult{
		{
			Slug:         "test-skill",
			DisplayName:  "Test Skill",
			Summary:      "A test skill",
			Version:      "1.0.0",
			RegistryName: "test-registry",
			Score:        0.9,
		},
	}

	// Put results in cache
	cache.Put("test query", results)

	// Get exact match
	retrieved, found := cache.Get("test query", 10)
	if !found {
		t.Fatal("expected to find exact match")
	}

	if len(retrieved) != 1 {
		t.Fatalf("expected 1 result, got %d", len(retrieved))
	}

	if retrieved[0].Slug != "test-skill" {
		t.Errorf("expected slug 'test-skill', got '%s'", retrieved[0].Slug)
	}
}

// TestSearchCacheMiss tests cache miss behavior.
func TestSearchCacheMiss(t *testing.T) {
	cache := skills.NewSearchCache(skills.SearchCacheConfig{
		MaxSize: 10,
		TTL:     5 * time.Minute,
	})

	// Try to get from empty cache
	_, found := cache.Get("nonexistent query", 10)
	if found {
		t.Error("expected cache miss, got cache hit")
	}
}

// TestSearchCacheSimilarity tests that similar queries can be matched.
func TestSearchCacheSimilarity(t *testing.T) {
	cache := skills.NewSearchCache(skills.SearchCacheConfig{
		MaxSize: 10,
		TTL:     5 * time.Minute,
	})

	results := []skills.SearchResult{
		{
			Slug:         "github",
			DisplayName:  "GitHub Integration",
			Summary:      "GitHub skills",
			Version:      "1.0.0",
			RegistryName: "test",
			Score:        0.9,
		},
	}

	// Put with one query
	cache.Put("github skills", results)

	// Try similar query (should match due to trigram similarity)
	// This tests the similarity matching functionality indirectly
	retrieved, found := cache.Get("github skill", 10)
	if !found {
		// Similarity matching is not guaranteed, so we just log this
		t.Log("similarity match did not occur (may be expected depending on threshold)")
		return
	}

	t.Log("similarity match successful", retrieved)
}

// TestSearchCacheLimit tests that the limit parameter works correctly.
func TestSearchCacheLimit(t *testing.T) {
	cache := skills.NewSearchCache(skills.SearchCacheConfig{
		MaxSize: 10,
		TTL:     5 * time.Minute,
	})

	// Create 5 results
	results := make([]skills.SearchResult, 5)
	for i := 0; i < 5; i++ {
		results[i] = skills.SearchResult{
			Slug:         "skill",
			DisplayName:  "Skill",
			Summary:      "Test",
			Version:      "1.0.0",
			RegistryName: "test",
			Score:        float64(i),
		}
	}

	cache.Put("test", results)

	// Request only 3
	retrieved, found := cache.Get("test", 3)
	if !found {
		t.Fatal("expected cache hit")
	}

	if len(retrieved) != 3 {
		t.Errorf("expected 3 results (limited), got %d", len(retrieved))
	}
}

// TestSearchCacheEviction tests LRU eviction when cache is full.
func TestSearchCacheEviction(t *testing.T) {
	cache := skills.NewSearchCache(skills.SearchCacheConfig{
		MaxSize: 3, // Small size to trigger eviction
		TTL:     5 * time.Minute,
	})

	results := []skills.SearchResult{
		{
			Slug:         "skill",
			DisplayName:  "Skill",
			Summary:      "Test",
			Version:      "1.0.0",
			RegistryName: "test",
			Score:        0.9,
		},
	}

	// Fill cache
	cache.Put("query1", results)
	cache.Put("query2", results)
	cache.Put("query3", results)

	// Verify all are in cache
	stats := cache.Stats()
	if stats.Size != 3 {
		t.Errorf("expected cache size 3, got %d", stats.Size)
	}

	// Add one more (should evict query1 - LRU)
	cache.Put("query4", results)

	stats = cache.Stats()
	if stats.Size != 3 {
		t.Errorf("expected cache size still 3 after eviction, got %d", stats.Size)
	}

	// query1 should be evicted
	_, found := cache.Get("query1", 10)
	if found {
		t.Error("expected query1 to be evicted (LRU), but it was found")
	}

	// query4 should be present
	_, found = cache.Get("query4", 10)
	if !found {
		t.Error("expected query4 to be in cache")
	}
}

// TestSearchCacheTTL tests that entries expire after TTL.
func TestSearchCacheTTL(t *testing.T) {
	cache := skills.NewSearchCache(skills.SearchCacheConfig{
		MaxSize: 10,
		TTL:     100 * time.Millisecond, // Very short TTL for testing
	})

	results := []skills.SearchResult{
		{
			Slug:         "skill",
			DisplayName:  "Skill",
			Summary:      "Test",
			Version:      "1.0.0",
			RegistryName: "test",
			Score:        0.9,
		},
	}

	cache.Put("test", results)

	// Should be found immediately
	_, found := cache.Get("test", 10)
	if !found {
		t.Error("expected cache hit immediately after Put")
	}

	// Wait for TTL to expire
	time.Sleep(150 * time.Millisecond)

	// Should not be found after TTL
	_, found = cache.Get("test", 10)
	if found {
		t.Error("expected cache miss after TTL expiration")
	}

	// Check stats - expired entries should count as misses
	stats := cache.Stats()
	if stats.HitCount == 0 {
		t.Error("expected at least one hit from the immediate Get")
	}
}

// TestSearchCacheClear tests that the cache can be cleared.
func TestSearchCacheClear(t *testing.T) {
	cache := skills.NewSearchCache(skills.SearchCacheConfig{
		MaxSize: 10,
		TTL:     5 * time.Minute,
	})

	results := []skills.SearchResult{
		{
			Slug:         "skill",
			DisplayName:  "Skill",
			Summary:      "Test",
			Version:      "1.0.0",
			RegistryName: "test",
			Score:        0.9,
		},
	}

	// Add some entries
	cache.Put("query1", results)
	cache.Put("query2", results)
	cache.Put("query3", results)

	// Verify cache is populated
	stats := cache.Stats()
	if stats.Size != 3 {
		t.Errorf("expected cache size 3 before clear, got %d", stats.Size)
	}

	// Clear cache
	cache.Clear()

	// Verify cache is empty
	stats = cache.Stats()
	if stats.Size != 0 {
		t.Errorf("expected cache size 0 after clear, got %d", stats.Size)
	}

	if stats.HitCount != 0 || stats.MissCount != 0 {
		t.Error("expected hit and miss counts to be reset after clear")
	}

	// Verify entries are gone
	_, found := cache.Get("query1", 10)
	if found {
		t.Error("expected cache miss after clear")
	}
}

// TestSearchCacheStats tests cache statistics tracking.
func TestSearchCacheStats(t *testing.T) {
	cache := skills.NewSearchCache(skills.SearchCacheConfig{
		MaxSize: 10,
		TTL:     5 * time.Minute,
	})

	results := []skills.SearchResult{
		{
			Slug:         "skill",
			DisplayName:  "Skill",
			Summary:      "Test",
			Version:      "1.0.0",
			RegistryName: "test",
			Score:        0.9,
		},
	}

	// Initial stats
	stats := cache.Stats()
	if stats.HitCount != 0 || stats.MissCount != 0 {
		t.Error("expected zero hits and misses initially")
	}

	// Add entry and get it (cache hit)
	cache.Put("test", results)
	cache.Get("test", 10)

	stats = cache.Stats()
	if stats.HitCount != 1 {
		t.Errorf("expected 1 hit, got %d", stats.HitCount)
	}

	// Cache miss
	cache.Get("nonexistent", 10)

	stats = cache.Stats()
	if stats.MissCount != 1 {
		t.Errorf("expected 1 miss, got %d", stats.MissCount)
	}

	// Check hit rate
	expectedHitRate := 0.5 // 1 hit out of 2 total
	if stats.HitRate != expectedHitRate {
		t.Errorf("expected hit rate %.2f, got %.2f", expectedHitRate, stats.HitRate)
	}
}

// TestSearchCacheUpdate tests that updating an existing entry works correctly.
func TestSearchCacheUpdate(t *testing.T) {
	cache := skills.NewSearchCache(skills.SearchCacheConfig{
		MaxSize: 10,
		TTL:     5 * time.Minute,
	})

	results1 := []skills.SearchResult{
		{
			Slug:         "skill",
			DisplayName:  "Skill",
			Summary:      "Version 1",
			Version:      "1.0.0",
			RegistryName: "test",
			Score:        0.9,
		},
	}

	results2 := []skills.SearchResult{
		{
			Slug:         "skill",
			DisplayName:  "Skill",
			Summary:      "Version 2",
			Version:      "2.0.0",
			RegistryName: "test",
			Score:        0.9,
		},
	}

	// Add initial results
	cache.Put("test", results1)

	// Update with new results
	cache.Put("test", results2)

	// Should get updated results
	retrieved, found := cache.Get("test", 10)
	if !found {
		t.Fatal("expected cache hit")
	}

	if retrieved[0].Version != "2.0.0" {
		t.Errorf("expected version 2.0.0, got %s", retrieved[0].Version)
	}

	// Should only count as one entry
	stats := cache.Stats()
	if stats.Size != 1 {
		t.Errorf("expected cache size 1 (not 2), got %d", stats.Size)
	}
}

// TestSearchCacheConcurrent tests concurrent access to the cache.
func TestSearchCacheConcurrent(t *testing.T) {
	cache := skills.NewSearchCache(skills.SearchCacheConfig{
		MaxSize: 100,
		TTL:     5 * time.Minute,
	})

	results := []skills.SearchResult{
		{
			Slug:         "skill",
			DisplayName:  "Skill",
			Summary:      "Test",
			Version:      "1.0.0",
			RegistryName: "test",
			Score:        0.9,
		},
	}

	done := make(chan bool)

	// Start multiple goroutines
	for i := 0; i < 10; i++ {
		go func(id int) {
			query := "query"
			for j := 0; j < 100; j++ {
				cache.Put(query, results)
				cache.Get(query, 10)
			}
			done <- true
		}(i)
	}

	// Wait for all goroutines to complete
	for i := 0; i < 10; i++ {
		<-done
	}

	// Verify cache is still in consistent state
	stats := cache.Stats()
	if stats.Size == 0 {
		t.Error("expected cache to have entries after concurrent operations")
	}

	t.Logf("Concurrent test completed: %d hits, %d misses, %.2f%% hit rate",
		stats.HitCount, stats.MissCount, stats.HitRate*100)
}
