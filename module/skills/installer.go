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

type SkillInstaller struct {
	workspace        string
	registryManager  *RegistryManager
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

func (si *SkillInstaller) InstallFromGitHub(ctx context.Context, repo string) error {
	skillDir := filepath.Join(si.workspace, "skills", filepath.Base(repo))

	if _, err := os.Stat(skillDir); err == nil {
		return fmt.Errorf("skill '%s' already exists", filepath.Base(repo))
	}

	url := fmt.Sprintf("https://raw.githubusercontent.com/%s/main/SKILL.md", repo)

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
	if err := os.WriteFile(skillPath, body, 0o644); err != nil {
		return fmt.Errorf("failed to write skill file: %w", err)
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

	skillDir := filepath.Join(si.workspace, "skills", slug)
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

	fmt.Printf("✓ Skill '%s' (version %s) installed successfully\n", slug, result.Version)
	if result.Summary != "" {
		fmt.Printf("  %s\n", result.Summary)
	}

	return nil
}

// SearchRegistries searches across all configured registries.
func (si *SkillInstaller) SearchRegistries(ctx context.Context, query string, limit int) ([]SearchResult, error) {
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
	results, err := si.registryManager.SearchAll(ctx, "", 100) // Empty query to get all skills
	if err != nil {
		return nil, fmt.Errorf("failed to search registries: %w", err)
	}

	skills := make([]AvailableSkill, len(results))
	for i, result := range results {
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
