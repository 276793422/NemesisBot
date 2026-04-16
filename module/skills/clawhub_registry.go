// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package skills

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"time"
)

const (
	defaultClawHubBaseURL = "https://clawhub.ai"
	defaultConvexURL      = "https://wry-manatee-359.convex.cloud"
)

// ClawHubRegistry implements the SkillRegistry interface for ClawHub.ai.
//
// API endpoints used:
//   - Search: GET {baseURL}/api/search?q={query}&limit={limit}
//   - List:   Convex POST /api/query  {"path":"skills:list","args":{"limit":N},"format":"json"}
//   - Meta:   Convex POST /api/query  {"path":"skills:getBySlug","args":{"slug":"..."},"format":"json"}
type ClawHubRegistry struct {
	baseURL   string // ClawHub website URL (for search API)
	convexURL string // Convex deployment URL (for query API)
	timeout   time.Duration
	client    *http.Client
}

// NewClawHubRegistry creates a new ClawHub registry client.
func NewClawHubRegistry(cfg ClawHubConfig) *ClawHubRegistry {
	timeout := 30 * time.Second
	if cfg.Timeout > 0 {
		timeout = time.Duration(cfg.Timeout) * time.Second
	}

	baseURL := cfg.BaseURL
	if baseURL == "" {
		baseURL = defaultClawHubBaseURL
	}

	convexURL := cfg.ConvexURL
	if convexURL == "" {
		convexURL = defaultConvexURL
	}

	return &ClawHubRegistry{
		baseURL:   baseURL,
		convexURL: convexURL,
		timeout:   timeout,
		client: &http.Client{
			// No client-level Timeout — use context deadlines instead
			// so callers can control per-request timeouts.
			Transport: &http.Transport{
				MaxIdleConns:        5,
				IdleConnTimeout:     30 * time.Second,
				TLSHandshakeTimeout: 10 * time.Second,
			},
		},
	}
}

// Name returns the registry name.
func (r *ClawHubRegistry) Name() string {
	return "clawhub"
}

// --- ClawHub search API ---

// clawhubSearchResponse represents the response from clawhub.ai/api/search.
type clawhubSearchResponse struct {
	Results []clawhubSearchItem `json:"results"`
}

// clawhubSearchItem represents a single result from the search API.
type clawhubSearchItem struct {
	Score       float64 `json:"score"`
	Slug        string  `json:"slug"`
	DisplayName string  `json:"displayName"`
	Summary     string  `json:"summary"`
	Version     *string `json:"version"`
	UpdatedAt   int64   `json:"updatedAt"`
}

// --- Convex API types ---

// convexResponse is the wrapper for all Convex HTTP API responses.
type convexResponse struct {
	Status       string          `json:"status"`
	Value        json.RawMessage `json:"value"`
	ErrorMessage string          `json:"errorMessage,omitempty"`
}

// convexSkillListItem is a single skill from the skills:list Convex query.
type convexSkillListItem struct {
	Slug        string `json:"slug"`
	DisplayName string `json:"displayName"`
	Summary     string `json:"summary"`
	Stats       struct {
		Downloads float64 `json:"downloads"`
	} `json:"stats"`
}

// convexSkillDetail is the full skill detail from skills:getBySlug.
type convexSkillDetail struct {
	Owner struct {
		Handle string `json:"handle"`
	} `json:"owner"`
	Skill struct {
		Slug        string `json:"slug"`
		DisplayName string `json:"displayName"`
		Summary     string `json:"summary"`
		Stats       struct {
			Downloads float64 `json:"downloads"`
		} `json:"stats"`
	} `json:"skill"`
	LatestVersion struct {
		Version string `json:"version"`
	} `json:"latestVersion"`
	ResolvedSlug string `json:"resolvedSlug"`
}

// callConvex calls a Convex HTTP query endpoint.
// Request: POST {convexURL}/api/query  {"path":"fnName","args":{...},"format":"json"}
// Response: {"status":"success","value":...} or {"status":"error","errorMessage":"..."}
func (r *ClawHubRegistry) callConvex(ctx context.Context, functionName string, args interface{}) (json.RawMessage, error) {
	reqBody := map[string]interface{}{
		"path":   functionName,
		"args":   args,
		"format": "json",
	}
	data, err := json.Marshal(reqBody)
	if err != nil {
		return nil, fmt.Errorf("failed to marshal request: %w", err)
	}

	reqURL := fmt.Sprintf("%s/api/query", r.convexURL)
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, reqURL, strings.NewReader(string(data)))
	if err != nil {
		return nil, fmt.Errorf("failed to create request: %w", err)
	}
	req.Header.Set("Content-Type", "application/json")

	resp, err := r.client.Do(req)
	if err != nil {
		return nil, fmt.Errorf("convex request failed: %w", err)
	}
	defer resp.Body.Close()

	body, err := io.ReadAll(io.LimitReader(resp.Body, 1<<20)) // 1MB max
	if err != nil {
		return nil, fmt.Errorf("failed to read response: %w", err)
	}

	var envelope convexResponse
	if err := json.Unmarshal(body, &envelope); err != nil {
		return nil, fmt.Errorf("failed to decode convex response: %w", err)
	}

	if envelope.Status == "error" {
		return nil, fmt.Errorf("convex error: %s", envelope.ErrorMessage)
	}

	return envelope.Value, nil
}

// Search searches the ClawHub registry for skills matching the query.
// Non-empty query uses the clawhub.ai search API (vector search).
// Empty query falls back to Convex skills:list (sorted by creation time).
func (r *ClawHubRegistry) Search(ctx context.Context, query string, limit int) ([]SearchResult, error) {
	if query == "" {
		return r.searchList(ctx, limit)
	}
	return r.searchQuery(ctx, query, limit)
}

// searchQuery uses the clawhub.ai search API for vector search.
func (r *ClawHubRegistry) searchQuery(ctx context.Context, query string, limit int) ([]SearchResult, error) {
	if limit <= 0 || limit > 50 {
		limit = 20
	}

	u := fmt.Sprintf("%s/api/search?q=%s&limit=%d", r.baseURL, url.QueryEscape(query), limit)
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, u, nil)
	if err != nil {
		return nil, fmt.Errorf("failed to create search request: %w", err)
	}

	resp, err := r.client.Do(req)
	if err != nil {
		return nil, fmt.Errorf("search request failed: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(io.LimitReader(resp.Body, 1024))
		return nil, fmt.Errorf("search failed with status %d: %s", resp.StatusCode, string(body))
	}

	var searchResp clawhubSearchResponse
	if err := json.NewDecoder(resp.Body).Decode(&searchResp); err != nil {
		return nil, fmt.Errorf("failed to parse search response: %w", err)
	}

	results := make([]SearchResult, 0, len(searchResp.Results))
	for _, item := range searchResp.Results {
		// Normalize score to 0-1 range (clawhub search returns scores in ~0-5 range)
		score := item.Score
		if score > 1.0 {
			score = score / 5.0
		}

		results = append(results, SearchResult{
			Score:        score,
			Slug:         item.Slug,
			DisplayName:  item.DisplayName,
			Summary:      item.Summary,
			Version:      "latest",
			RegistryName: "clawhub",
		})
	}

	return results, nil
}

// searchList fetches recent skills via Convex skills:list.
func (r *ClawHubRegistry) searchList(ctx context.Context, limit int) ([]SearchResult, error) {
	if limit <= 0 || limit > 100 {
		limit = 20
	}

	value, err := r.callConvex(ctx, "skills:list", map[string]interface{}{"limit": limit})
	if err != nil {
		return nil, fmt.Errorf("failed to list skills: %w", err)
	}

	var items []convexSkillListItem
	if err := json.Unmarshal(value, &items); err != nil {
		return nil, fmt.Errorf("failed to parse list response: %w", err)
	}

	results := make([]SearchResult, 0, len(items))
	for _, item := range items {
		results = append(results, SearchResult{
			Score:        1.0,
			Slug:         item.Slug,
			DisplayName:  item.DisplayName,
			Summary:      item.Summary,
			Version:      "latest",
			RegistryName: "clawhub",
			Downloads:    int64(item.Stats.Downloads),
		})
	}

	return results, nil
}

// GetSkillMeta retrieves metadata for a specific skill by slug.
func (r *ClawHubRegistry) GetSkillMeta(ctx context.Context, slug string) (*SkillMeta, error) {
	if err := ValidateSkillIdentifier(slug); err != nil {
		return nil, fmt.Errorf("invalid skill slug: %w", err)
	}

	value, err := r.callConvex(ctx, "skills:getBySlug", map[string]interface{}{"slug": slug})
	if err != nil {
		return nil, fmt.Errorf("failed to get skill metadata: %w", err)
	}

	var detail convexSkillDetail
	if err := json.Unmarshal(value, &detail); err != nil {
		return nil, fmt.Errorf("failed to parse metadata response: %w", err)
	}

	if detail.Skill.Slug == "" && detail.ResolvedSlug == "" {
		return nil, fmt.Errorf("skill '%s' not found", slug)
	}

	metaSlug := detail.Skill.Slug
	if metaSlug == "" {
		metaSlug = detail.ResolvedSlug
	}

	version := detail.LatestVersion.Version
	if version == "" {
		version = "latest"
	}

	return &SkillMeta{
		Slug:          metaSlug,
		DisplayName:   detail.Skill.DisplayName,
		Summary:       detail.Skill.Summary,
		LatestVersion: version,
		RegistryName:  "clawhub",
	}, nil
}

// DownloadAndInstall fetches metadata to find the owner handle, then downloads
// the SKILL.md from the openclaw/skills GitHub repository.
func (r *ClawHubRegistry) DownloadAndInstall(ctx context.Context, slug, version, targetDir string) (*InstallResult, error) {
	if err := ValidateSkillIdentifier(slug); err != nil {
		return nil, fmt.Errorf("invalid skill slug: %w", err)
	}

	// Get full skill detail including owner handle
	value, err := r.callConvex(ctx, "skills:getBySlug", map[string]interface{}{"slug": slug})
	if err != nil {
		return nil, fmt.Errorf("failed to get skill info: %w", err)
	}

	var detail convexSkillDetail
	if err := json.Unmarshal(value, &detail); err != nil {
		return nil, fmt.Errorf("failed to parse skill detail: %w", err)
	}

	if detail.Owner.Handle == "" {
		return nil, fmt.Errorf("owner handle not found for skill '%s'", slug)
	}

	// Download SKILL.md from openclaw/skills GitHub repo
	downloadURL := fmt.Sprintf(
		"https://raw.githubusercontent.com/openclaw/skills/main/skills/%s/%s/SKILL.md",
		detail.Owner.Handle, slug,
	)

	req, err := http.NewRequestWithContext(ctx, http.MethodGet, downloadURL, nil)
	if err != nil {
		return nil, fmt.Errorf("failed to create download request: %w", err)
	}

	resp, err := r.client.Do(req)
	if err != nil {
		return nil, fmt.Errorf("failed to download skill: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(io.LimitReader(resp.Body, 512))
		return nil, fmt.Errorf("download failed with status %d: %s", resp.StatusCode, string(body))
	}

	body, err := io.ReadAll(io.LimitReader(resp.Body, 512*1024)) // 512KB max
	if err != nil {
		return nil, fmt.Errorf("failed to read download body: %w", err)
	}

	// Create target directory
	if err := os.MkdirAll(targetDir, 0o755); err != nil {
		return nil, fmt.Errorf("failed to create target directory: %w", err)
	}

	// Write SKILL.md
	skillPath := filepath.Join(targetDir, "SKILL.md")
	if err := os.WriteFile(skillPath, body, 0o644); err != nil {
		return nil, fmt.Errorf("failed to write skill file: %w", err)
	}

	slog.Debug("clawhub skill installed", "slug", slug, "owner", detail.Owner.Handle, "path", targetDir)

	return &InstallResult{
		Version:          detail.LatestVersion.Version,
		IsMalwareBlocked: false,
		IsSuspicious:     false,
		Summary:          detail.Skill.Summary,
	}, nil
}

// ValidateSkillIdentifier validates a skill identifier to prevent path traversal attacks.
func ValidateSkillIdentifier(slug string) error {
	trimmed := strings.TrimSpace(slug)
	if trimmed == "" {
		return fmt.Errorf("skill identifier cannot be empty")
	}

	// Check for path traversal
	if strings.ContainsAny(trimmed, "/\\") {
		return fmt.Errorf("skill identifier cannot contain path separators")
	}

	if strings.Contains(trimmed, "..") {
		return fmt.Errorf("skill identifier cannot contain '..'")
	}

	// Check length
	if len(trimmed) > 64 {
		return fmt.Errorf("skill identifier too long (max 64 characters)")
	}

	return nil
}
