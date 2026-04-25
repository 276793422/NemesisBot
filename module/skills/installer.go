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
	"sync"
	"time"

	"github.com/276793422/NemesisBot/module/utils"
)

// SkillOrigin represents the origin metadata of an installed skill.
type SkillOrigin struct {
	Version          int    `json:"version"`           // format version
	Registry         string `json:"registry"`          // registry name (e.g., "github", "clawhub")
	Slug             string `json:"slug"`              // skill slug/identifier
	InstalledVersion string `json:"installed_version"` // installed version
	InstalledAt      int64  `json:"installed_at"`      // unix timestamp
}

type SkillInstaller struct {
	workspace           string
	registryManager     *RegistryManager
	githubBaseURL       string // Base URL for GitHub raw content, defaults to https://raw.githubusercontent.com
	lastSecurityCheck   *SecurityCheckResult
	mu                  sync.Mutex
}

type AvailableSkill struct {
	Name        string   `json:"name"`
	Repository  string   `json:"repository"`
	Description string   `json:"description"`
	Author      string   `json:"author"`
	Tags        []string `json:"tags"`
}

func NewSkillInstaller(workspace string) *SkillInstaller {
	return &SkillInstaller{
		workspace:       workspace,
		registryManager: NewRegistryManager(), // Empty by default
	}
}

// SetRegistryManager sets the registry manager for advanced installation features.
func (si *SkillInstaller) SetRegistryManager(rm *RegistryManager) {
	si.registryManager = rm
}

// SetGitHubBaseURL sets the base URL for GitHub raw content (for testing).
func (si *SkillInstaller) SetGitHubBaseURL(url string) {
	si.githubBaseURL = url
}

// HasRegistryManager checks if a registry manager is configured.
func (si *SkillInstaller) HasRegistryManager() bool {
	return si.registryManager != nil
}

// HasRegistry checks if a registry with the given name exists.
func (si *SkillInstaller) HasRegistry(name string) bool {
	if si.registryManager == nil {
		return false
	}
	return si.registryManager.GetRegistry(name) != nil
}

// GetRegistryManager returns the configured registry manager.
func (si *SkillInstaller) GetRegistryManager() *RegistryManager {
	return si.registryManager
}

// SearchAll searches all configured registries for skills matching the query.
// This uses the search cache if enabled.
// Results are returned grouped by registry source.
func (si *SkillInstaller) SearchAll(ctx context.Context, query string, limit int) ([]RegistrySearchResult, error) {
	if si.registryManager == nil {
		return nil, fmt.Errorf("registry manager not configured")
	}
	return si.registryManager.SearchAll(ctx, query, limit)
}

// FlattenSearchResults flattens a slice of RegistrySearchResult into a single
// slice of SearchResult, for callers that don't need per-registry grouping.
func FlattenSearchResults(grouped []RegistrySearchResult) []SearchResult {
	var flat []SearchResult
	for _, g := range grouped {
		flat = append(flat, g.Results...)
	}
	return flat
}

func (si *SkillInstaller) InstallFromGitHub(ctx context.Context, repo string) error {
	skillDir := filepath.Join(si.workspace, "skills", filepath.Base(repo))

	if _, err := os.Stat(skillDir); err == nil {
		return fmt.Errorf("skill '%s' already exists", filepath.Base(repo))
	}

	baseURL := si.githubBaseURL
	if baseURL == "" {
		baseURL = "https://raw.githubusercontent.com"
	}
	url := fmt.Sprintf("%s/%s/main/SKILL.md", baseURL, repo)

	client := &http.Client{Timeout: 15 * time.Second}
	req, err := http.NewRequestWithContext(ctx, "GET", url, nil)
	if err != nil {
		return fmt.Errorf("failed to create request: %w", err)
	}

	resp, err := utils.DoRequestWithRetry(client, req)
	if err != nil {
		return fmt.Errorf("failed to fetch skill: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != 200 {
		return fmt.Errorf("failed to fetch skill: HTTP %d", resp.StatusCode)
	}

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return fmt.Errorf("failed to read response: %w", err)
	}

	if err := os.MkdirAll(skillDir, 0o755); err != nil {
		return fmt.Errorf("failed to create skill directory: %w", err)
	}

	skillPath := filepath.Join(skillDir, "SKILL.md")
	if err := utils.WriteFileAtomic(skillPath, body, 0o644); err != nil {
		return fmt.Errorf("failed to write skill file: %w", err)
	}

	// Security check on installed content
	skillName := filepath.Base(repo)
	checkResult := SecurityCheck(string(body), skillName, nil)
	si.setLastSecurityCheck(checkResult)

	if checkResult.Blocked {
		os.RemoveAll(skillDir)
		return fmt.Errorf("skill '%s' blocked by security check: %s", skillName, checkResult.BlockReason)
	}

	if !checkResult.LintResult.Passed {
		slog.Warn("skill has security warnings",
			"skill", skillName,
			"lint_score", checkResult.LintResult.Score,
			"issues", len(checkResult.LintResult.Issues))
	}

	if checkResult.QualityScore != nil && checkResult.QualityScore.Overall < 40 {
		slog.Warn("skill has low quality score",
			"skill", skillName,
			"quality_score", checkResult.QualityScore.Overall)
	}

	return nil
}

func (si *SkillInstaller) Uninstall(skillName string) error {
	skillDir := filepath.Join(si.workspace, "skills", skillName)

	if _, err := os.Stat(skillDir); os.IsNotExist(err) {
		return fmt.Errorf("skill '%s' not found", skillName)
	}

	if err := os.RemoveAll(skillDir); err != nil {
		return fmt.Errorf("failed to remove skill: %w", err)
	}

	return nil
}

// InstallFromRegistry installs a skill using a registry from the RegistryManager.
func (si *SkillInstaller) InstallFromRegistry(ctx context.Context, registryName, slug, version string) error {
	if si.registryManager == nil {
		return fmt.Errorf("registry manager not configured")
	}

	registry := si.registryManager.GetRegistry(registryName)
	if registry == nil {
		return fmt.Errorf("registry '%s' not found", registryName)
	}

	skillDir := filepath.Join(si.workspace, "skills", filepath.Base(slug))
	if _, err := os.Stat(skillDir); err == nil {
		return fmt.Errorf("skill '%s' already exists", slug)
	}

	result, err := registry.DownloadAndInstall(ctx, slug, version, skillDir)
	if err != nil {
		return fmt.Errorf("failed to download and install skill: %w", err)
	}

	// Log installation metadata
	if result.IsMalwareBlocked {
		return fmt.Errorf("skill '%s' was blocked as malware", slug)
	}

	if result.IsSuspicious {
		fmt.Printf("⚠️  Warning: Skill '%s' is marked as suspicious\n", slug)
	}

	// Security check on installed content
	skillFilePath := filepath.Join(skillDir, "SKILL.md")
	if content, err := os.ReadFile(skillFilePath); err == nil {
		checkResult := SecurityCheck(string(content), slug, nil)
		si.setLastSecurityCheck(checkResult)

		if checkResult.Blocked {
			os.RemoveAll(skillDir)
			return fmt.Errorf("skill '%s' blocked by security check: %s", slug, checkResult.BlockReason)
		}
		if !checkResult.LintResult.Passed {
			fmt.Printf("⚠️  Security warnings (score: %.0f/100, %d issues)\n",
				checkResult.LintResult.Score, len(checkResult.LintResult.Issues))
		}
		if checkResult.QualityScore != nil {
			fmt.Printf("   Quality score: %.0f/100\n", checkResult.QualityScore.Overall)
		}
	}

	fmt.Printf("✓ Skill '%s' (version %s) installed successfully\n", slug, result.Version)
	if result.Summary != "" {
		fmt.Printf("  %s\n", result.Summary)
	}

	// Write origin tracking metadata
	if err := si.writeOriginTracking(skillDir, registryName, slug, result.Version); err != nil {
		slog.Warn("failed to write origin tracking", "skill", slug, "error", err)
		// Don't fail the installation if origin tracking fails
	}

	return nil
}

// SearchRegistries searches across all configured registries.
// Results are returned grouped by registry source.
func (si *SkillInstaller) SearchRegistries(ctx context.Context, query string, limit int) ([]RegistrySearchResult, error) {
	if si.registryManager == nil {
		return nil, fmt.Errorf("registry manager not configured")
	}

	return si.registryManager.SearchAll(ctx, query, limit)
}

func (si *SkillInstaller) ListAvailableSkills(ctx context.Context) ([]AvailableSkill, error) {
	// If registry manager is configured, use it for better results
	if si.registryManager != nil {
		return si.listAvailableSkillsFromRegistry(ctx)
	}

	// Fallback to original implementation
	return si.listAvailableSkillsFromGitHub(ctx)
}

// listAvailableSkillsFromRegistry uses the registry manager to list available skills.
func (si *SkillInstaller) listAvailableSkillsFromRegistry(ctx context.Context) ([]AvailableSkill, error) {
	grouped, err := si.registryManager.SearchAll(ctx, "", 100) // Empty query to get all skills
	if err != nil {
		return nil, fmt.Errorf("failed to search registries: %w", err)
	}

	// Flatten grouped results
	var allResults []SearchResult
	for _, g := range grouped {
		allResults = append(allResults, g.Results...)
	}

	skills := make([]AvailableSkill, len(allResults))
	for i, result := range allResults {
		skills[i] = AvailableSkill{
			Name:        result.Slug,
			Description: result.Summary,
			Tags:        []string{result.RegistryName},
		}
	}

	return skills, nil
}

// listAvailableSkillsFromGitHub is the original implementation.
func (si *SkillInstaller) listAvailableSkillsFromGitHub(ctx context.Context) ([]AvailableSkill, error) {
	url := "https://raw.githubusercontent.com/276793422/nemesisbot-skills/main/skills.json"

	client := &http.Client{Timeout: 15 * time.Second}
	req, err := http.NewRequestWithContext(ctx, "GET", url, nil)
	if err != nil {
		return nil, fmt.Errorf("failed to create request: %w", err)
	}

	resp, err := utils.DoRequestWithRetry(client, req)
	if err != nil {
		return nil, fmt.Errorf("failed to fetch skills list: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != 200 {
		return nil, fmt.Errorf("failed to fetch skills list: HTTP %d", resp.StatusCode)
	}

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, fmt.Errorf("failed to read response: %w", err)
	}

	var skills []AvailableSkill
	if err := json.Unmarshal(body, &skills); err != nil {
		return nil, fmt.Errorf("failed to parse skills list: %w", err)
	}

	return skills, nil
}

// writeOriginTracking writes the .skill-origin.json file with installation metadata.
func (si *SkillInstaller) writeOriginTracking(skillDir, registryName, slug, version string) error {
	origin := SkillOrigin{
		Version:          1,
		Registry:         registryName,
		Slug:             slug,
		InstalledVersion: version,
		InstalledAt:      time.Now().Unix(),
	}

	data, err := json.MarshalIndent(origin, "", "  ")
	if err != nil {
		return fmt.Errorf("failed to marshal origin metadata: %w", err)
	}

	originPath := filepath.Join(skillDir, ".skill-origin.json")
	if err := utils.WriteFileAtomic(originPath, data, 0o644); err != nil {
		return fmt.Errorf("failed to write origin file: %w", err)
	}

	slog.Debug("wrote origin tracking", "skill", slug, "registry", registryName, "version", version)
	return nil
}

// GetOriginTracking reads the .skill-origin.json file for a skill.
func (si *SkillInstaller) GetOriginTracking(skillName string) (*SkillOrigin, error) {
	skillDir := filepath.Join(si.workspace, "skills", skillName)
	originPath := filepath.Join(skillDir, ".skill-origin.json")

	data, err := os.ReadFile(originPath)
	if err != nil {
		return nil, fmt.Errorf("failed to read origin file: %w", err)
	}

	var origin SkillOrigin
	if err := json.Unmarshal(data, &origin); err != nil {
		return nil, fmt.Errorf("failed to parse origin file: %w", err)
	}

	return &origin, nil
}

// LastSecurityCheck returns the security check result from the most recent install.
func (si *SkillInstaller) LastSecurityCheck() *SecurityCheckResult {
	si.mu.Lock()
	defer si.mu.Unlock()
	return si.lastSecurityCheck
}

// setLastSecurityCheck stores the security check result from the most recent install.
func (si *SkillInstaller) setLastSecurityCheck(result *SecurityCheckResult) {
	si.mu.Lock()
	defer si.mu.Unlock()
	si.lastSecurityCheck = result
}
