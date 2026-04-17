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
	gitHubAPIURL     string // GitHub API base URL, default "https://api.github.com"
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
			// No client-level Timeout — use context deadlines instead
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
			// No client-level Timeout — use context deadlines instead
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

// apiBaseURL returns the GitHub API base URL, defaulting to https://api.github.com.
func (g *GitHubRegistry) apiBaseURL() string {
	if g.gitHubAPIURL != "" {
		return g.gitHubAPIURL
	}
	return "https://api.github.com"
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

// isThreeLayerPattern returns true if the skill path pattern contains {author},
// indicating a three-layer directory structure: skills/{author}/{slug}/SKILL.md
func (g *GitHubRegistry) isThreeLayerPattern() bool {
	return strings.Contains(g.skillPathPattern, "{author}")
}

// searchGitHubAPI uses the GitHub Contents API to list skill directories.
// For two-layer repos (skills/{slug}/SKILL.md), it lists the skills/ directory directly.
// For three-layer repos (skills/{author}/{slug}/SKILL.md), it uses the Trees API
// to get the full directory tree and matches against skill slugs.
func (g *GitHubRegistry) searchGitHubAPI(ctx context.Context, query string, limit int) ([]SearchResult, error) {
	if g.isThreeLayerPattern() {
		return g.searchThreeLayer(ctx, query, limit)
	}
	return g.searchTwoLayer(ctx, query, limit)
}

// searchTwoLayer searches two-layer repos (skills/{slug}/SKILL.md)
// by listing the skills/ directory and matching against directory names.
func (g *GitHubRegistry) searchTwoLayer(ctx context.Context, query string, limit int) ([]SearchResult, error) {
	apiURL := fmt.Sprintf("%s/repos/%s/contents/skills", g.apiBaseURL(), g.repo)

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

// searchThreeLayer searches three-layer repos (skills/{author}/{slug}/SKILL.md)
// using the GitHub Trees API with a streaming JSON decoder to handle large repos
// (openclaw/skills has 65K+ entries, ~20MB response) efficiently.
func (g *GitHubRegistry) searchThreeLayer(ctx context.Context, query string, limit int) ([]SearchResult, error) {
	apiURL := fmt.Sprintf("%s/repos/%s/git/trees/%s?recursive=1", g.apiBaseURL(), g.repo, g.branch)

	req, err := http.NewRequestWithContext(ctx, http.MethodGet, apiURL, nil)
	if err != nil {
		return nil, fmt.Errorf("failed to create API request: %w", err)
	}
	req.Header.Set("Accept", "application/vnd.github.v3+json")

	resp, err := utils.DoRequestWithRetry(g.client, req)
	if err != nil {
		return nil, fmt.Errorf("failed to fetch tree: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		body, _ := io.ReadAll(io.LimitReader(resp.Body, 4096))
		return nil, fmt.Errorf("GitHub API HTTP %d: %s", resp.StatusCode, string(body))
	}

	// Use streaming JSON decoder — avoids buffering the entire ~20MB response.
	decoder := json.NewDecoder(resp.Body)

	// Read opening {
	if t, err := decoder.Token(); err != nil {
		return nil, fmt.Errorf("failed to read tree response: %w", err)
	} else if t != json.Delim('{') {
		return nil, fmt.Errorf("expected JSON object, got %v", t)
	}

	var truncated bool
	results := make([]SearchResult, 0)
	skillsSet := make(map[string]bool)

	for decoder.More() {
		keyToken, err := decoder.Token()
		if err != nil {
			break
		}
		key, _ := keyToken.(string)

		switch key {
		case "truncated":
			decoder.Decode(&truncated)

		case "tree":
			// Decode the tree array entry by entry (streaming)
			if t, err := decoder.Token(); err != nil {
				break
			} else if t != json.Delim('[') {
				break
			}

			for decoder.More() {
				if len(results) >= limit {
					break
				}

				// Check context cancellation
				select {
				case <-ctx.Done():
					return nil, fmt.Errorf("tree search cancelled: %w", ctx.Err())
				default:
				}

				var entry struct {
					Path string `json:"path"`
					Type string `json:"type"`
				}
				if err := decoder.Decode(&entry); err != nil {
					continue // skip malformed entries
				}

				if entry.Type != "blob" {
					continue
				}
				path := entry.Path
				if !strings.HasPrefix(path, "skills/") || !strings.HasSuffix(path, "/SKILL.md") {
					continue
				}

				inner := strings.TrimPrefix(path, "skills/")
				inner = strings.TrimSuffix(inner, "/SKILL.md")
				parts := strings.SplitN(inner, "/", 2)
				if len(parts) != 2 {
					continue
				}
				author, slug := parts[0], parts[1]
				if author == "" || slug == "" {
					continue
				}
				if skillsSet[slug] {
					continue
				}

				if query != "" && !contains(slug, query) {
					continue
				}

				skillsSet[slug] = true
				downloadPath := strings.ReplaceAll(
					strings.ReplaceAll(g.skillPathPattern, "{author}", author),
					"{slug}", slug,
				)
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
			// Consume closing ]
			decoder.Token()

		default:
			// Skip other top-level fields (sha, url, etc.)
			var discard interface{}
			decoder.Decode(&discard)
		}
	}

	if truncated {
		slog.Debug("github tree truncated, results may be incomplete", "repo", g.repo)
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
// Downloads all files and subdirectories within the skill directory, not just SKILL.md.
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

	// Use Trees API to download the full skill directory
	if g.repo != "" && g.skillPathPattern != "" {
		// Calculate the skill directory prefix from the pattern
		dirPrefix := g.skillDirPrefix(slug)
		if dirPrefix != "" {
			if err := g.downloadSkillTree(ctx, dirPrefix, targetDir); err != nil {
				return nil, fmt.Errorf("failed to download skill directory: %w", err)
			}

			return &InstallResult{
				Version:          installVersion,
				IsMalwareBlocked: false,
				IsSuspicious:     false,
				Summary:          meta.Summary,
			}, nil
		}
	}

	// Legacy fallback: download only SKILL.md (for repos without skillPathPattern)
	var possibleURLs []string
	if g.repo != "" && g.skillPathPattern != "" {
		possibleURLs = []string{g.buildSkillURL(slug)}
	} else {
		possibleURLs = []string{
			fmt.Sprintf("%s/%s/main/SKILL.md", g.baseURL, slug),
			fmt.Sprintf("%s/276793422/nemesisbot-skills/main/skills/%s/SKILL.md", g.baseURL, slug),
		}
	}

	var lastErr error
	for _, url := range possibleURLs {
		body, err := g.doGet(ctx, url)
		if err == nil {
			if err := os.MkdirAll(targetDir, 0o755); err != nil {
				return nil, fmt.Errorf("failed to create directory: %w", err)
			}

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

// skillDirPrefix calculates the directory prefix for a skill from the skillPathPattern.
// For example, "skills/{slug}/SKILL.md" with slug "pdf" → "skills/pdf"
// For example, "skills/{author}/{slug}/SKILL.md" with slug "clawcv/pdf" → "skills/clawcv/pdf"
func (g *GitHubRegistry) skillDirPrefix(slug string) string {
	path := g.skillPathPattern
	if strings.Contains(path, "{author}") {
		parts := strings.SplitN(slug, "/", 2)
		if len(parts) == 2 {
			path = strings.ReplaceAll(path, "{author}", parts[0])
			path = strings.ReplaceAll(path, "{slug}", parts[1])
		} else {
			return "" // Can't determine author from slug alone
		}
	} else {
		path = strings.ReplaceAll(path, "{slug}", slug)
	}

	// Remove the trailing filename (e.g., "/SKILL.md") to get the directory prefix
	// The last "/" separates the directory from the filename
	lastSlash := strings.LastIndex(path, "/")
	if lastSlash < 0 {
		return ""
	}
	return path[:lastSlash]
}

// downloadSkillTree downloads all files under the given directory prefix from GitHub
// using the shared Trees API download logic.
func (g *GitHubRegistry) downloadSkillTree(ctx context.Context, dirPrefix, targetDir string) error {
	return DownloadSkillTreeFromGitHub(ctx, g.client, g.apiBaseURL(), g.baseURL,
		g.repo, g.branch, dirPrefix, targetDir, int64(g.maxSize))
}

// buildSkillURL constructs the download URL for a skill using the configured pattern.
func (g *GitHubRegistry) buildSkillURL(slug string) string {
	path := g.skillPathPattern
	if strings.Contains(path, "{author}") {
		// Three-level pattern: skills/{author}/{slug}/SKILL.md
		// slug may be "author/skill-name" or just "skill-name"
		parts := strings.SplitN(slug, "/", 2)
		if len(parts) == 2 {
			path = strings.ReplaceAll(path, "{author}", parts[0])
			path = strings.ReplaceAll(path, "{slug}", parts[1])
		} else {
			// Only slug provided, author unknown — cannot resolve
			path = strings.ReplaceAll(path, "{slug}", slug)
		}
	} else {
		path = strings.ReplaceAll(path, "{slug}", slug)
	}
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
