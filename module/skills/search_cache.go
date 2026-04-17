// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package skills

import (
	"sort"
	"sync"
	"time"
)

// SearchCache provides intelligent caching for skill search results with similarity matching.
// Uses trigram-based similarity and Jaccard coefficient to match similar queries.
type SearchCache struct {
	mu        sync.RWMutex
	cache     map[string]*CacheEntry
	lruList   []string // least recently used list
	maxSize   int
	ttl       time.Duration
	hitCount  int
	missCount int
}

// CacheEntry represents a single cache entry with search results and metadata.
type CacheEntry struct {
	Results      []RegistrySearchResult // grouped by registry
	Trigrams     []uint32               // trigram signature for similarity matching
	CreatedAt    time.Time
	LastAccessAt time.Time
	AccessCount  int
}

// NewSearchCache creates a new search cache with the given configuration.
func NewSearchCache(cfg SearchCacheConfig) *SearchCache {
	if cfg.MaxSize <= 0 {
		cfg.MaxSize = 50
	}
	if cfg.TTL <= 0 {
		cfg.TTL = 5 * time.Minute
	}

	return &SearchCache{
		cache:   make(map[string]*CacheEntry),
		lruList: make([]string, 0, cfg.MaxSize),
		maxSize: cfg.MaxSize,
		ttl:     cfg.TTL,
	}
}

// Get retrieves search results from cache, checking for similar queries if exact match not found.
// Returns (results, true) if cache hit (exact or similar), (nil, false) if cache miss.
// Results are clamped per-registry to the given limit.
func (sc *SearchCache) Get(query string, limit int) ([]RegistrySearchResult, bool) {
	sc.mu.Lock()
	defer sc.mu.Unlock()

	// 1. Try exact match first
	if entry, ok := sc.cache[query]; ok {
		if time.Since(entry.CreatedAt) > sc.ttl {
			// Entry expired
			delete(sc.cache, query)
			sc.missCount++
			return nil, false
		}

		// Update LRU
		sc.updateLRU(query)
		entry.LastAccessAt = time.Now()
		entry.AccessCount++
		sc.hitCount++

		// Clamp results per-registry to limit
		return clampRegistryResults(entry.Results, limit), true
	}

	// 2. Try similarity match (trigram + Jaccard)
	queryTrigrams := buildTrigrams(query)
	bestMatch := ""
	bestScore := 0.0

	for key, entry := range sc.cache {
		if time.Since(entry.CreatedAt) > sc.ttl {
			continue
		}

		// Calculate Jaccard similarity
		similarity := jaccardSimilarity(queryTrigrams, entry.Trigrams)
		if similarity > 0.7 && similarity > bestScore { // 70% threshold
			bestMatch = key
			bestScore = similarity
		}
	}

	if bestMatch != "" {
		entry := sc.cache[bestMatch]
		sc.updateLRU(bestMatch)
		entry.LastAccessAt = time.Now()
		entry.AccessCount++
		sc.hitCount++

		// Clamp results per-registry to limit
		return clampRegistryResults(entry.Results, limit), true
	}

	sc.missCount++
	return nil, false
}

// clampRegistryResults clamps each registry's results to the given limit.
func clampRegistryResults(grouped []RegistrySearchResult, limit int) []RegistrySearchResult {
	if limit <= 0 {
		return grouped
	}
	clamped := make([]RegistrySearchResult, len(grouped))
	for i, g := range grouped {
		if len(g.Results) > limit {
			clamped[i] = RegistrySearchResult{
				RegistryName: g.RegistryName,
				Results:      g.Results[:limit],
				Truncated:    true, // clamped implies truncation
			}
		} else {
			clamped[i] = g
		}
	}
	return clamped
}

// Put stores search results in the cache with the query as key.
// If the key already exists, it updates the entry and moves it to the front of LRU.
func (sc *SearchCache) Put(query string, results []RegistrySearchResult) {
	sc.mu.Lock()
	defer sc.mu.Unlock()

	// Build trigram signature for similarity matching
	trigrams := buildTrigrams(query)

	entry := &CacheEntry{
		Results:      results,
		Trigrams:     trigrams,
		CreatedAt:    time.Now(),
		LastAccessAt: time.Now(),
		AccessCount:  1,
	}

	// Check if key already exists
	if _, exists := sc.cache[query]; exists {
		// Update existing entry
		sc.cache[query] = entry
		sc.updateLRU(query)
		return
	}

	// Check if we need to evict
	if len(sc.cache) >= sc.maxSize {
		sc.evictLRU()
	}

	sc.cache[query] = entry
	sc.lruList = append(sc.lruList, query)
}

// updateLRU updates the LRU list by moving the key to the end.
func (sc *SearchCache) updateLRU(key string) {
	// Remove key from current position
	for i, k := range sc.lruList {
		if k == key {
			sc.lruList = append(sc.lruList[:i], sc.lruList[i+1:]...)
			break
		}
	}
	// Add to end
	sc.lruList = append(sc.lruList, key)
}

// evictLRU evicts the least recently used entry.
func (sc *SearchCache) evictLRU() {
	if len(sc.lruList) == 0 {
		return
	}

	// Remove first entry (least recently used)
	lru := sc.lruList[0]
	delete(sc.cache, lru)
	sc.lruList = sc.lruList[1:]
}

// Clear clears all entries from the cache.
func (sc *SearchCache) Clear() {
	sc.mu.Lock()
	defer sc.mu.Unlock()

	sc.cache = make(map[string]*CacheEntry)
	sc.lruList = make([]string, 0, sc.maxSize)
	sc.hitCount = 0
	sc.missCount = 0
}

// Stats returns cache statistics.
func (sc *SearchCache) Stats() CacheStats {
	sc.mu.RLock()
	defer sc.mu.RUnlock()

	total := sc.hitCount + sc.missCount
	hitRate := 0.0
	if total > 0 {
		hitRate = float64(sc.hitCount) / float64(total)
	}

	return CacheStats{
		Size:      len(sc.cache),
		MaxSize:   sc.maxSize,
		HitCount:  sc.hitCount,
		MissCount: sc.missCount,
		HitRate:   hitRate,
	}
}

// CacheStats holds cache statistics.
type CacheStats struct {
	Size      int     // current number of entries
	MaxSize   int     // maximum number of entries
	HitCount  int     // number of cache hits
	MissCount int     // number of cache misses
	HitRate   float64 // cache hit rate (0.0 - 1.0)
}

// buildTrigrams generates trigram signatures for similarity matching.
// A trigram is a sequence of 3 consecutive characters.
// Uses hashing to reduce memory footprint.
func buildTrigrams(s string) []uint32 {
	if len(s) < 3 {
		return []uint32{}
	}

	// Convert to lowercase for case-insensitive matching
	s = toLower(s)

	trigrams := make([]uint32, 0, len(s)-2)

	// Build hash for each trigram
	for i := 0; i <= len(s)-3; i++ {
		// Create a 3-byte hash: s[i] << 16 | s[i+1] << 8 | s[i+2]
		// This is a simple rolling hash suitable for ASCII
		hash := uint32(s[i])<<16 | uint32(s[i+1])<<8 | uint32(s[i+2])
		trigrams = append(trigrams, hash)
	}

	// Sort and deduplicate
	sort.Slice(trigrams, func(i, j int) bool {
		return trigrams[i] < trigrams[j]
	})

	uniqueTrigrams := make([]uint32, 0, len(trigrams))
	for i, t := range trigrams {
		if i == 0 || t != trigrams[i-1] {
			uniqueTrigrams = append(uniqueTrigrams, t)
		}
	}

	return uniqueTrigrams
}

// jaccardSimilarity calculates the Jaccard similarity coefficient between two trigram sets.
// J(A, B) = |A ∩ B| / |A ∪ B|
// Returns a value between 0.0 (no similarity) and 1.0 (identical).
func jaccardSimilarity(a, b []uint32) float64 {
	if len(a) == 0 && len(b) == 0 {
		return 1.0
	}
	if len(a) == 0 || len(b) == 0 {
		return 0.0
	}

	// Calculate intersection size
	intersection := 0
	i, j := 0, 0
	for i < len(a) && j < len(b) {
		if a[i] == b[j] {
			intersection++
			i++
			j++
		} else if a[i] < b[j] {
			i++
		} else {
			j++
		}
	}

	// Calculate union size
	union := len(a) + len(b) - intersection

	if union == 0 {
		return 0.0
	}

	return float64(intersection) / float64(union)
}

// toLower converts a string to lowercase for case-insensitive matching.
// More efficient than strings.ToLower for our use case.
func toLower(s string) string {
	result := make([]byte, len(s))
	for i := 0; i < len(s); i++ {
		c := s[i]
		if c >= 'A' && c <= 'Z' {
			result[i] = c + 32
		} else {
			result[i] = c
		}
	}
	return string(result)
}
