// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package skills

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/276793422/NemesisBot/module/utils"
)

const (
	defaultGitHubTimeout = 30 * time.Second
	defaultGitHubMaxSize = 1 * 1024 * 1024 // 1 MB
)

// GitHubRegistry implements SkillRegistry for GitHub repositories.
type GitHubRegistry struct {
	baseURL string
	timeout time.Duration
	maxSize int
	client  *http.Client

	// Multi-source fields
	repo             string // e.g. "anthropics/skills"
	branch           string // default "main"
	indexType        string // "skills_json" or "github_api"
	indexPath        string // e.g. "skills.json"
	skillPathPattern string // e.g. "skills/{slug}/SKILL.md"
	registryName     string // override for Name(), e.g. "anthropics"
}

// NewGitHubRegistry creates a new GitHub registry client from config (legacy single-source).
func NewGitHubRegistry(cfg GitHubConfig) *GitHubRegistry {
	baseURL := cfg.BaseURL
	if baseURL == "" {
		baseURL = "https://raw.githubusercontent.com"
	}

	timeout := defaultGitHubTimeout
	if cfg.Timeout > 0 {
		timeout = time.Duration(cfg.Timeout) * time.Second
	}

	maxSize := defaultGitHubMaxSize
	if cfg.MaxSize > 0 {
		maxSize = cfg.MaxSize
	}

	return &GitHubRegistry{
		baseURL: baseURL,
		timeout: timeout,
		maxSize: maxSize,
		client: &http.Client{
			Timeout: timeout,
			Transport: &http.Transport{
				MaxIdleConns:        5,
				IdleConnTimeout:     30 * time.Second,
				TLSHandshakeTimeout: 10 * time.Second,
			},
		},
		// Legacy: use hardcoded repo
		repo:             "276793422/nemesisbot-skills",
		branch:           "main",
		indexType:        "skills_json",
		indexPath:        "skills.json",
		skillPathPattern: "skills/{slug}/SKILL.md",
		registryName:     "",
	}
}

// NewGitHubRegistryFromSource creates a new GitHub registry from a GitHubSourceConfig.
func NewGitHubRegistryFromSource(source GitHubSourceConfig) *GitHubRegistry {
	baseURL := "https://raw.githubusercontent.com"

	timeout := defaultGitHubTimeout
	if source.Timeout > 0 {
		timeout = time.Duration(source.Timeout) * time.Second
	}

	maxSize := defaultGitHubMaxSize
	if source.MaxSize > 0 {
		maxSize = source.MaxSize
	}

	branch := source.Branch
	if branch == "" {
		branch = "main"
	}

	return &GitHubRegistry{
		baseURL:          baseURL,
		timeout:          timeout,
		maxSize:          maxSize,
		repo:             source.Repo,
		branch:           branch,
		indexType:        source.IndexType,
		indexPath:        source.IndexPath,
		skillPathPattern: source.SkillPathPattern,
		registryName:     source.Name,
		client: &http.Client{
			Timeout: timeout,
			Transport: &http.Transport{
				MaxIdleConns:        5,
				IdleConnTimeout:     30 * time.Second,
				TLSHandshakeTimeout: 10 * time.Second,
			},
		},
	}
}

func (g *GitHubRegistry) Name() string {
	if g.registryName != "" {
		return g.registryName
	}
	return "github"
}

// Search searches for available GitHub skills.
// Dispatches to the appropriate search strategy based on indexType.
func (g *GitHubRegistry) Search(ctx context.Context, query string, limit int) ([]SearchResult, error) {
	switch g.indexType {
	case "github_api":
		return g.searchGitHubAPI(ctx, query, limit)
	case "skills_json":
		return g.searchSkillsJSON(ctx, query, limit)
	default:
		return g.searchSkillsJSON(ctx, query, limit)
	}
}

// searchSkillsJSON fetches a skills.json index file and searches within it.
func (g *GitHubRegistry) searchSkillsJSON(ctx context.Context, query string, limit int) ([]SearchResult, error) {
	url := fmt.Sprintf("%s/%s/%s/%s", g.baseURL, g.repo, g.branch, g.indexPath)

	body, err := g.doGet(ctx, url)
	if err != nil {
		return nil, fmt.Errorf("failed to fetch skills list: %w", err)
	}

	var skills []githubSkill
	if err := json.Unmarshal(body, &skills); err != nil {
		return nil, fmt.Errorf("failed to parse skills list: %w", err)
	}

	results := make([]SearchResult, 0)
	for _, skill := range skills {
		if len(results) >= limit {
			break
		}

		if contains(skill.Name, query) || contains(skill.Description, query) {
			results = append(results, SearchResult{
				Score:        1.0,
				Slug:         skill.Name,
				DisplayName:  skill.Name,
				Summary:      skill.Description,
				Version:      "latest",
				RegistryName: g.Name(),
				SourceRepo:   g.repo,
			})
		}
	}

	return results, nil
}

// searchGitHubAPI uses the GitHub Contents API to list skill directories.
func (g *GitHubRegistry) searchGitHubAPI(ctx context.Context, query string, limit int) ([]SearchResult, error) {
	// Use GitHub API to list contents of the skills/ directory
	apiURL := fmt.Sprintf("https://api.github.com/repos/%s/contents/skills", g.repo)

	req, err := http.NewRequestWithContext(ctx, http.MethodGet, apiURL, nil)
	if err != nil {
		return nil, fmt.Errorf("failed to create API request: %w", err)
	}
	req.Header.Set("Accept", "application/vnd.github.v3+json")

	resp, err := utils.DoRequestWithRetry(g.client, req)
	if err != nil {
		return nil, fmt.Errorf("failed to list skills directory: %w", err)
	}
	defer resp.Body.Close()

	body, err := io.ReadAll(io.LimitReader(resp.Body, int64(g.maxSize)))
	if err != nil {
		return nil, fmt.Errorf("failed to read API response: %w", err)
	}

	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		return nil, fmt.Errorf("GitHub API HTTP %d: %s", resp.StatusCode, string(body))
	}

	// Parse directory listing
	var entries []githubContentEntry
	if err := json.Unmarshal(body, &entries); err != nil {
		return nil, fmt.Errorf("failed to parse GitHub directory listing: %w", err)
	}

	results := make([]SearchResult, 0)
	for _, entry := range entries {
		if len(results) >= limit {
			break
		}

		if entry.Type != "dir" {
			continue
		}

		// Match against directory name (slug)
		slug := entry.Name
		if contains(slug, query) {
			downloadPath := strings.ReplaceAll(g.skillPathPattern, "{slug}", slug)
			results = append(results, SearchResult{
				Score:        1.0,
				Slug:         slug,
				DisplayName:  slug,
				Summary:      fmt.Sprintf("Skill from %s", g.repo),
				Version:      "latest",
				RegistryName: g.Name(),
				SourceRepo:   g.repo,
				DownloadPath: downloadPath,
			})
		}
	}

	return results, nil
}

// GetSkillMeta retrieves metadata for a specific GitHub skill.
func (g *GitHubRegistry) GetSkillMeta(ctx context.Context, slug string) (*SkillMeta, error) {
	if err := utils.ValidateSkillIdentifier(slug); err != nil {
		return nil, fmt.Errorf("invalid slug %q: %w", slug, err)
	}

	// For skills_json index type, try to look up in the index
	if g.indexType == "skills_json" {
		url := fmt.Sprintf("%s/%s/%s/%s", g.baseURL, g.repo, g.branch, g.indexPath)

		body, err := g.doGet(ctx, url)
		if err != nil {
			return nil, fmt.Errorf("failed to fetch skills list: %w", err)
		}

		var skills []githubSkill
		if err := json.Unmarshal(body, &skills); err != nil {
			return nil, fmt.Errorf("failed to parse skills list: %w", err)
		}

		for _, skill := range skills {
			if skill.Name == slug {
				return &SkillMeta{
					Slug:          skill.Name,
					DisplayName:   skill.Name,
					Summary:       skill.Description,
					LatestVersion: "latest",
					RegistryName:  g.Name(),
				}, nil
			}
		}

		return nil, fmt.Errorf("skill not found: %s", slug)
	}

	// For github_api, return basic metadata (no index to look up)
	return &SkillMeta{
		Slug:          slug,
		DisplayName:   slug,
		Summary:       fmt.Sprintf("Skill from %s", g.repo),
		LatestVersion: "latest",
		RegistryName:  g.Name(),
	}, nil
}

// DownloadAndInstall downloads a skill from GitHub and installs it to the target directory.
func (g *GitHubRegistry) DownloadAndInstall(ctx context.Context, slug, version, targetDir string) (*InstallResult, error) {
	if err := utils.ValidateSkillIdentifier(slug); err != nil {
		return nil, fmt.Errorf("invalid slug %q: %w", slug, err)
	}

	// Fetch metadata first
	meta, err := g.GetSkillMeta(ctx, slug)
	if err != nil {
		// If metadata fetch fails, try to install anyway
		meta = &SkillMeta{
			Slug:          slug,
			DisplayName:   slug,
			LatestVersion: version,
			RegistryName:  g.Name(),
		}
	}

	// Determine the actual version to install
	installVersion := version
	if installVersion == "" {
		installVersion = meta.LatestVersion
	}
	if installVersion == "" {
		installVersion = "main"
	}

	// Build the download URL(s)
	var possibleURLs []string
	if g.repo != "" && g.skillPathPattern != "" {
		// Multi-source: use buildSkillURL to construct URL from pattern
		possibleURLs = []string{g.buildSkillURL(slug)}
	} else {
		// Legacy fallback: try both direct repository and nemesisbot-skills repository
		possibleURLs = []string{
			fmt.Sprintf("%s/%s/main/SKILL.md", g.baseURL, slug),
			fmt.Sprintf("%s/276793422/nemesisbot-skills/main/skills/%s/SKILL.md", g.baseURL, slug),
		}
	}

	var lastErr error
	for _, url := range possibleURLs {
		body, err := g.doGet(ctx, url)
		if err == nil {
			// Create target directory
			if err := os.MkdirAll(targetDir, 0o755); err != nil {
				return nil, fmt.Errorf("failed to create directory: %w", err)
			}

			// Write the skill file
			skillPath := filepath.Join(targetDir, "SKILL.md")
			if err := os.WriteFile(skillPath, body, 0o644); err != nil {
				return nil, fmt.Errorf("failed to write skill file: %w", err)
			}

			return &InstallResult{
				Version:          installVersion,
				IsMalwareBlocked: false,
				IsSuspicious:     false,
				Summary:          meta.Summary,
			}, nil
		}
		lastErr = err
	}

	return nil, fmt.Errorf("failed to download skill from GitHub: %w", lastErr)
}

// buildSkillURL constructs the download URL for a skill using the configured pattern.
func (g *GitHubRegistry) buildSkillURL(slug string) string {
	path := strings.ReplaceAll(g.skillPathPattern, "{slug}", slug)
	return fmt.Sprintf("%s/%s/%s/%s", g.baseURL, g.repo, g.branch, path)
}

// doGet performs an HTTP GET request with retry logic.
func (g *GitHubRegistry) doGet(ctx context.Context, url string) ([]byte, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return nil, fmt.Errorf("failed to create request: %w", err)
	}

	resp, err := utils.DoRequestWithRetry(g.client, req)
	if err != nil {
		return nil, fmt.Errorf("request failed: %w", err)
	}
	defer resp.Body.Close()

	// Limit response size to prevent memory issues
	body, err := io.ReadAll(io.LimitReader(resp.Body, int64(g.maxSize)))
	if err != nil {
		return nil, fmt.Errorf("failed to read response: %w", err)
	}

	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		return nil, fmt.Errorf("HTTP %d: %s", resp.StatusCode, string(body))
	}

	return body, nil
}

// githubSkill represents a skill from the GitHub skills repository.
type githubSkill struct {
	Name        string   `json:"name"`
	Description string   `json:"description"`
	Repository  string   `json:"repository"`
	Author      string   `json:"author"`
	Tags        []string `json:"tags"`
}

// githubContentEntry represents a directory entry from GitHub Contents API.
type githubContentEntry struct {
	Name string `json:"name"`
	Type string `json:"type"` // "dir" or "file"
	Path string `json:"path"`
}

// contains checks if a string contains a substring (case-insensitive for search).
func contains(s, substr string) bool {
	// For search purposes, use case-insensitive matching
	sLower := toLower(s)
	substrLower := toLower(substr)
	return len(sLower) >= len(substrLower) && (sLower == substrLower || len(sLower) > len(substrLower) && containsMiddle(sLower, substrLower))
}

func containsMiddle(s, substr string) bool {
	for i := 0; i <= len(s)-len(substr); i++ {
		if s[i:i+len(substr)] == substr {
			return true
		}
	}
	return false
}
