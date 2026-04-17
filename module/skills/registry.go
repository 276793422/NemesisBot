// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package skills

import (
	"context"
	"fmt"
	"log/slog"
	"sync"
	"time"
)

const (
	defaultMaxConcurrentSearches = 2
)

// SearchResult represents a single result from a skill registry search.
type SearchResult struct {
	Score        float64 `json:"score"`
	Slug         string  `json:"slug"`
	DisplayName  string  `json:"display_name"`
	Summary      string  `json:"summary"`
	Version      string  `json:"version"`
	RegistryName string  `json:"registry_name"`
	SourceRepo   string  `json:"source_repo,omitempty"`   // e.g. "anthropics/skills"
	DownloadPath string  `json:"download_path,omitempty"` // e.g. "skills/pdf/SKILL.md"
	Author       string  `json:"author,omitempty"`        // skill author handle
	Downloads    int64  `json:"downloads,omitempty"`      // download count
	Truncated    bool    `json:"truncated,omitempty"`     // hint: more results may exist beyond this entry
}

// RegistrySearchResult holds the search results for a single registry source.
type RegistrySearchResult struct {
	RegistryName string         // e.g. "clawhub", "anthropics", "openclaw"
	Results      []SearchResult // results from this registry (sorted by score desc)
	Truncated    bool           // true if the registry may have more results than returned
}

// SkillMeta holds metadata about a skill from a registry.
type SkillMeta struct {
	Slug             string `json:"slug"`
	DisplayName      string `json:"display_name"`
	Summary          string `json:"summary"`
	LatestVersion    string `json:"latest_version"`
	IsMalwareBlocked bool   `json:"is_malware_blocked,omitempty"` // make optional for backward compatibility
	IsSuspicious     bool   `json:"is_suspicious,omitempty"`      // make optional for backward compatibility
	RegistryName     string `json:"registry_name"`
}

// InstallResult is returned by DownloadAndInstall to carry metadata
// back to the caller for moderation and user messaging.
type InstallResult struct {
	Version          string
	IsMalwareBlocked bool
	IsSuspicious     bool
	Summary          string
}

// SkillRegistry is the interface that all skill registries must implement.
// Each registry represents a different source of skills (e.g., clawhub.ai, github)
type SkillRegistry interface {
	// Name returns the unique name of this registry (e.g., "clawhub").
	Name() string
	// Search searches the registry for skills matching the query.
	Search(ctx context.Context, query string, limit int) ([]SearchResult, error)
	// GetSkillMeta retrieves metadata for a specific skill by slug.
	GetSkillMeta(ctx context.Context, slug string) (*SkillMeta, error)
	// DownloadAndInstall fetches metadata, resolves the version, downloads and
	// installs the skill to targetDir. Returns an InstallResult with metadata
	// for the caller to use for moderation and user messaging.
	DownloadAndInstall(ctx context.Context, slug, version, targetDir string) (*InstallResult, error)
}

// RegistryConfig holds configuration for all skill registries.
type RegistryConfig struct {
	SearchCache           SearchCacheConfig
	ClawHub               ClawHubConfig
	GitHub                GitHubConfig         // Legacy single-source config (backward compat)
	GitHubSources         []GitHubSourceConfig // New multi-source config
	MaxConcurrentSearches int
}

// SearchCacheConfig configures the search cache.
type SearchCacheConfig struct {
	Enabled bool          `json:"enabled"`
	MaxSize int           `json:"max_size"` // maximum number of cache entries (default: 50)
	TTL     time.Duration `json:"ttl"`      // time-to-live (default: 5 minutes)
}

// ClawHubConfig configures the ClawHub registry.
type ClawHubConfig struct {
	Enabled   bool
	BaseURL   string // ClawHub website URL (e.g. "https://clawhub.ai"), used for search API
	ConvexURL string // Convex deployment URL (e.g. "https://wry-manatee-359.convex.cloud")
	Timeout   int    // seconds, 0 = default (30s)
}

// GitHubConfig configures the GitHub registry (legacy single-source).
type GitHubConfig struct {
	Enabled bool
	BaseURL string // defaults to github.com
	Timeout int    // seconds, 0 = default (30s)
	MaxSize int    // bytes, 0 = default (1MB)
}

// GitHubSourceConfig configures a single GitHub source for skills.
type GitHubSourceConfig struct {
	Name             string
	Repo             string // e.g. "anthropics/skills"
	Enabled          bool
	Branch           string // default "main"
	IndexType        string // "skills_json" or "github_api"
	IndexPath        string // e.g. "skills.json"
	SkillPathPattern string // e.g. "skills/{slug}/SKILL.md"
	Timeout          int
	MaxSize          int
}

// RegistryManager coordinates multiple skill registries.
// It fans out search requests and routes installs to the correct registry.
type RegistryManager struct {
	registries    []SkillRegistry
	maxConcurrent int
	mu            sync.RWMutex
	searchCache   *SearchCache
}

// NewRegistryManager creates an empty RegistryManager.
func NewRegistryManager() *RegistryManager {
	return &RegistryManager{
		registries:    make([]SkillRegistry, 0),
		maxConcurrent: defaultMaxConcurrentSearches,
	}
}

// NewRegistryManagerFromConfig builds a RegistryManager from config,
// instantiating only the enabled registries.
func NewRegistryManagerFromConfig(cfg RegistryConfig) *RegistryManager {
	rm := NewRegistryManager()
	if cfg.MaxConcurrentSearches > 0 {
		rm.maxConcurrent = cfg.MaxConcurrentSearches
	}

	// Initialize search cache if enabled
	if cfg.SearchCache.Enabled {
		rm.searchCache = NewSearchCache(cfg.SearchCache)
	}

	// New: iterate GitHubSources and create per-source instances
	for _, source := range cfg.GitHubSources {
		if source.Enabled {
			rm.AddRegistry(NewGitHubRegistryFromSource(source))
		}
	}

	// Backward compat: if no GitHubSources configured, fall back to legacy GitHub config
	if len(cfg.GitHubSources) == 0 && cfg.GitHub.Enabled {
		rm.AddRegistry(NewGitHubRegistry(cfg.GitHub))
	}

	// ClawHub support
	if cfg.ClawHub.Enabled {
		rm.AddRegistry(NewClawHubRegistry(cfg.ClawHub))
	}
	return rm
}

// AddRegistry adds a registry to the manager.
func (rm *RegistryManager) AddRegistry(r SkillRegistry) {
	rm.mu.Lock()
	defer rm.mu.Unlock()
	rm.registries = append(rm.registries, r)
}

// GetRegistry returns a registry by name, or nil if not found.
func (rm *RegistryManager) GetRegistry(name string) SkillRegistry {
	rm.mu.RLock()
	defer rm.mu.RUnlock()
	for _, r := range rm.registries {
		if r.Name() == name {
			return r
		}
	}
	return nil
}

// GetSearchCache returns the search cache, or nil if not enabled.
func (rm *RegistryManager) GetSearchCache() *SearchCache {
	rm.mu.RLock()
	defer rm.mu.RUnlock()
	return rm.searchCache
}

// SearchAll fans out the query to all registries concurrently.
// Each registry is searched independently with the given limit.
// Results are returned grouped by registry, not merged.
func (rm *RegistryManager) SearchAll(ctx context.Context, query string, limit int) ([]RegistrySearchResult, error) {
	// 1. Check cache first if enabled
	if rm.searchCache != nil {
		if results, ok := rm.searchCache.Get(query, limit); ok {
			slog.Debug("search cache hit", "query", query, "registries", len(results))
			return results, nil
		}
	}

	rm.mu.RLock()
	regs := make([]SkillRegistry, len(rm.registries))
	copy(regs, rm.registries)
	rm.mu.RUnlock()

	if len(regs) == 0 {
		return nil, fmt.Errorf("no registries configured")
	}

	type regResult struct {
		name     string
		results  []SearchResult
		trunc    bool
		err      error
	}

	// Semaphore: limit concurrency.
	sem := make(chan struct{}, rm.maxConcurrent)
	resultsCh := make(chan regResult, len(regs))

	var wg sync.WaitGroup
	for _, reg := range regs {
		wg.Add(1)
		go func(r SkillRegistry) {
			defer wg.Done()

			// Acquire semaphore slot.
			select {
			case sem <- struct{}{}:
				defer func() { <-sem }()
			case <-ctx.Done():
				resultsCh <- regResult{err: ctx.Err()}
				return
			}

			searchCtx, cancel := context.WithTimeout(ctx, 1*time.Minute)
			defer cancel()

			results, err := r.Search(searchCtx, query, limit)
			if err != nil {
				slog.Warn("registry search failed", "registry", r.Name(), "error", err)
				resultsCh <- regResult{err: err}
				return
			}

			// Check if the last result indicates truncation
			truncated := false
			if len(results) > 0 && results[len(results)-1].Truncated {
				// Remove the truncation marker from the last result itself
				results[len(results)-1].Truncated = false
				truncated = true
			}

			resultsCh <- regResult{name: r.Name(), results: results, trunc: truncated}
		}(reg)
	}

	// Close results channel after all goroutines complete.
	go func() {
		wg.Wait()
		close(resultsCh)
	}()

	var grouped []RegistrySearchResult
	var lastErr error
	var anyRegistrySucceeded bool

	for rr := range resultsCh {
		if rr.err != nil {
			lastErr = rr.err
			continue
		}
		anyRegistrySucceeded = true
		grouped = append(grouped, RegistrySearchResult{
			RegistryName: rr.name,
			Results:      rr.results,
			Truncated:    rr.trunc,
		})
	}

	// If all registries failed, return the last error.
	if !anyRegistrySucceeded && lastErr != nil {
		return nil, fmt.Errorf("all registries failed: %w", lastErr)
	}

	// 2. Store results in cache BEFORE clamping.
	if rm.searchCache != nil && len(grouped) > 0 {
		rm.searchCache.Put(query, grouped)
		slog.Debug("search cache stored", "query", query, "registries", len(grouped))
	}

	return grouped, nil
}

// sortByScoreDesc sorts SearchResults by Score in descending order (insertion sort — small slices).
func sortByScoreDesc(results []SearchResult) {
	for i := 1; i < len(results); i++ {
		key := results[i]
		j := i - 1
		for j >= 0 && results[j].Score < key.Score {
			results[j+1] = results[j]
			j--
		}
		results[j+1] = key
	}
}
