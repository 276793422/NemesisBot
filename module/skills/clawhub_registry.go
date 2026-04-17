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

	"github.com/276793422/NemesisBot/module/utils"
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
	baseURL      string // ClawHub website URL (for search API)
	convexURL    string // Convex deployment URL (for query API)
	convexSiteURL string // Convex site URL override (for ZIP download); if empty, derived from convexURL
	timeout      time.Duration
	client       *http.Client
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

// siteURL returns the Convex site URL for ZIP downloads.
// If explicitly set, returns that. Otherwise derives from convexURL (.convex.cloud → .convex.site).
func (r *ClawHubRegistry) siteURL() string {
	if r.convexSiteURL != "" {
		return r.convexSiteURL
	}
	return strings.Replace(r.convexURL, ".convex.cloud", ".convex.site", 1)
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
	if limit <= 0 {
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

	// Mark truncation: if we got exactly `limit` results, there may be more
	if len(results) == limit {
		results[len(results)-1].Truncated = true
	}

	return results, nil
}

// searchList fetches recent skills via Convex skills:list.
func (r *ClawHubRegistry) searchList(ctx context.Context, limit int) ([]SearchResult, error) {
	if limit <= 0 {
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

	// Mark truncation: Convex skills:list has a hard limit of 200.
	// If the caller requested more than 200 and we got exactly 200, there are more.
	if limit > 200 && len(results) == 200 {
		results[len(results)-1].Truncated = true
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
// the full skill directory (all files and subdirectories).
//
// Strategy:
//  1. Try ZIP download from the Convex site URL (primary).
//  2. Fallback to GitHub Trees API for individual file downloads.
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

	owner := detail.Owner.Handle
	installVersion := detail.LatestVersion.Version
	if installVersion == "" {
		installVersion = "latest"
	}

	// Strategy 1: Try ZIP download from Convex site
	if zipErr := r.downloadSkillZip(ctx, slug, targetDir); zipErr == nil {
		slog.Debug("clawhub skill installed via ZIP", "slug", slug, "owner", owner, "path", targetDir)
		return &InstallResult{
			Version:          installVersion,
			IsMalwareBlocked: false,
			IsSuspicious:     false,
			Summary:          detail.Skill.Summary,
		}, nil
	} else {
		slog.Debug("ZIP download failed, falling back to GitHub Trees API", "slug", slug, "error", zipErr)
	}

	// Strategy 2: Fallback to GitHub Trees API
	if treeErr := DownloadSkillTreeFromGitHub(ctx, r.client,
		"https://api.github.com", "https://raw.githubusercontent.com",
		"openclaw/skills", "main",
		fmt.Sprintf("skills/%s/%s", owner, slug),
		targetDir, 0, // 0 = use default 10MB limit
	); treeErr != nil {
		return nil, fmt.Errorf("all download strategies failed: %w", treeErr)
	}

	slog.Debug("clawhub skill installed via GitHub Trees API", "slug", slug, "owner", owner, "path", targetDir)

	return &InstallResult{
		Version:          installVersion,
		IsMalwareBlocked: false,
		IsSuspicious:     false,
		Summary:          detail.Skill.Summary,
	}, nil
}

// downloadSkillZip downloads a skill as a ZIP from the Convex site and extracts it.
func (r *ClawHubRegistry) downloadSkillZip(ctx context.Context, slug, targetDir string) error {
	siteURL := r.siteURL()
	if siteURL == "" {
		return fmt.Errorf("site URL not configured")
	}
	downloadURL := fmt.Sprintf("%s/api/v1/download?slug=%s", siteURL, url.QueryEscape(slug))

	req, err := http.NewRequestWithContext(ctx, http.MethodGet, downloadURL, nil)
	if err != nil {
		return fmt.Errorf("failed to create ZIP download request: %w", err)
	}

	resp, err := r.client.Do(req)
	if err != nil {
		return fmt.Errorf("ZIP download request failed: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(io.LimitReader(resp.Body, 512))
		return fmt.Errorf("ZIP download failed with status %d: %s", resp.StatusCode, string(body))
	}

	// Check content type — should be ZIP
	contentType := resp.Header.Get("Content-Type")
	if contentType != "" && !strings.Contains(contentType, "zip") &&
		!strings.Contains(contentType, "application/octet-stream") {
		return fmt.Errorf("unexpected content type for ZIP download: %s", contentType)
	}

	// Download to temp file
	tmpFile, err := os.CreateTemp("", "clawhub-skill-*.zip")
	if err != nil {
		return fmt.Errorf("failed to create temp file: %w", err)
	}
	tmpPath := tmpFile.Name()
	defer os.Remove(tmpPath)

	if _, err := io.Copy(tmpFile, io.LimitReader(resp.Body, 50*1024*1024)); err != nil { // 50MB max
		tmpFile.Close()
		return fmt.Errorf("failed to write ZIP to temp file: %w", err)
	}
	tmpFile.Close()

	// Extract to a staging directory
	stagingDir, err := os.MkdirTemp("", "clawhub-skill-extract-*")
	if err != nil {
		return fmt.Errorf("failed to create staging directory: %w", err)
	}
	defer os.RemoveAll(stagingDir)

	if err := utils.ExtractZipFile(tmpPath, stagingDir); err != nil {
		return fmt.Errorf("failed to extract ZIP: %w", err)
	}

	// Check if ZIP contained a single top-level directory — flatten if so
	finalSrc, err := flattenSingleTopDir(stagingDir)
	if err != nil {
		return fmt.Errorf("failed to process extracted contents: %w", err)
	}

	// Move contents to targetDir
	if err := os.MkdirAll(targetDir, 0o755); err != nil {
		return fmt.Errorf("failed to create target directory: %w", err)
	}

	return moveDirContents(finalSrc, targetDir)
}

// flattenSingleTopDir checks if stagingDir contains a single subdirectory at the top level.
// If so, returns the path to that subdirectory (for flattening). Otherwise returns stagingDir.
func flattenSingleTopDir(stagingDir string) (string, error) {
	entries, err := os.ReadDir(stagingDir)
	if err != nil {
		return stagingDir, nil
	}

	// If there's exactly one entry and it's a directory, flatten into it
	if len(entries) == 1 && entries[0].IsDir() {
		return filepath.Join(stagingDir, entries[0].Name()), nil
	}

	return stagingDir, nil
}

// moveDirContents moves all files and directories from srcDir to dstDir.
func moveDirContents(srcDir, dstDir string) error {
	entries, err := os.ReadDir(srcDir)
	if err != nil {
		return fmt.Errorf("failed to read source directory: %w", err)
	}

	for _, entry := range entries {
		srcPath := filepath.Join(srcDir, entry.Name())
		dstPath := filepath.Join(dstDir, entry.Name())

		if entry.IsDir() {
			if err := os.MkdirAll(dstPath, 0o755); err != nil {
				return fmt.Errorf("failed to create directory: %w", err)
			}
			if err := moveDirContents(srcPath, dstPath); err != nil {
				return err
			}
		} else {
			data, err := os.ReadFile(srcPath)
			if err != nil {
				return fmt.Errorf("failed to read %s: %w", entry.Name(), err)
			}
			if err := os.WriteFile(dstPath, data, 0o644); err != nil {
				return fmt.Errorf("failed to write %s: %w", entry.Name(), err)
			}
		}
	}

	return nil
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
