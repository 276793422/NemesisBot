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
}

// NewGitHubRegistry creates a new GitHub registry client from config.
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
	}
}

func (g *GitHubRegistry) Name() string {
	return "github"
}

// Search searches for available GitHub skills.
// Since GitHub doesn't have a built-in search API for skills, this returns
// results from a curated skills repository if configured.
func (g *GitHubRegistry) Search(ctx context.Context, query string, limit int) ([]SearchResult, error) {
	// Try to fetch from a curated skills repository
	url := fmt.Sprintf("%s/276793422/nemesisbot-skills/main/skills.json", g.baseURL)

	body, err := g.doGet(ctx, url)
	if err != nil {
		return nil, fmt.Errorf("failed to fetch skills list: %w", err)
	}

	var skills []githubSkill
	if err := json.Unmarshal(body, &skills); err != nil {
		return nil, fmt.Errorf("failed to parse skills list: %w", err)
	}

	// Filter by query and limit results
	results := make([]SearchResult, 0)
	for _, skill := range skills {
		if len(results) >= limit {
			break
		}

		// Simple matching (can be improved)
		if contains(skill.Name, query) || contains(skill.Description, query) {
			results = append(results, SearchResult{
				Score:        1.0, // Simple scoring
				Slug:         skill.Name,
				DisplayName:  skill.Name,
				Summary:      skill.Description,
				Version:      "latest",
				RegistryName: g.Name(),
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

	// Fetch from skills repository to get metadata
	url := fmt.Sprintf("%s/276793422/nemesisbot-skills/main/skills.json", g.baseURL)

	body, err := g.doGet(ctx, url)
	if err != nil {
		return nil, fmt.Errorf("failed to fetch skills list: %w", err)
	}

	var skills []githubSkill
	if err := json.Unmarshal(body, &skills); err != nil {
		return nil, fmt.Errorf("failed to parse skills list: %w", err)
	}

	// Find the skill by slug
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

	// Download the skill file
	// Try both direct repository and nemesisbot-skills repository
	possibleURLs := []string{
		fmt.Sprintf("%s/%s/main/SKILL.md", g.baseURL, slug),
		fmt.Sprintf("%s/276793422/nemesisbot-skills/main/skills/%s/SKILL.md", g.baseURL, slug),
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

// contains checks if a string contains a substring (case-insensitive).
func contains(s, substr string) bool {
	return len(s) >= len(substr) && (s == substr || len(s) > len(substr) && containsMiddle(s, substr))
}

func containsMiddle(s, substr string) bool {
	for i := 0; i <= len(s)-len(substr); i++ {
		if s[i:i+len(substr)] == substr {
			return true
		}
	}
	return false
}
