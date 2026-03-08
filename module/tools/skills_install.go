// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package tools

import (
	"context"
	"fmt"

	"github.com/276793422/NemesisBot/module/skills"
)

// InstallSkillTool is a tool that installs a skill from a configured registry.
type InstallSkillTool struct {
	registryManager *skills.RegistryManager
	installer       *skills.SkillInstaller
}

// NewInstallSkillTool creates a new install_skill tool.
func NewInstallSkillTool(registryManager *skills.RegistryManager, installer *skills.SkillInstaller) *InstallSkillTool {
	return &InstallSkillTool{
		registryManager: registryManager,
		installer:       installer,
	}
}

// Name returns the tool name.
func (t *InstallSkillTool) Name() string {
	return "install_skill"
}

// Description returns the tool description.
func (t *InstallSkillTool) Description() string {
	return "Install a skill from a configured registry (GitHub, ClawHub, etc.). Use this tool after finding a skill with find_skills."
}

// ParameterSchema returns the tool's parameter schema.
func (t *InstallSkillTool) ParameterSchema() map[string]interface{} {
	return map[string]interface{}{
		"type": "object",
		"properties": map[string]interface{}{
			"slug": map[string]interface{}{
				"type":        "string",
				"description": "Skill identifier (e.g., 'github', 'weather', 'docker-compose')",
			},
			"registry": map[string]interface{}{
				"type":        "string",
				"description": "Registry name (e.g., 'github', 'clawhub'). If not specified, uses the first available registry.",
			},
			"version": map[string]interface{}{
				"type":        "string",
				"description": "Specific version to install (optional, defaults to latest)",
			},
			"force": map[string]interface{}{
				"type":        "boolean",
				"description": "Force reinstall if the skill already exists (default: false)",
			},
		},
		"required": []string{"slug"},
	}
}

// Execute executes the install_skill tool.
func (t *InstallSkillTool) Execute(ctx context.Context, args map[string]interface{}) *ToolResult {
	// Parse parameters
	slug, ok := args["slug"].(string)
	if !ok || slug == "" {
		return ErrorResult("slug parameter is required and must be a non-empty string")
	}

	registry := ""
	if registryVal, ok := args["registry"].(string); ok {
		registry = registryVal
	}

	version := ""
	if versionVal, ok := args["version"].(string); ok {
		version = versionVal
	}

	force := false
	if forceVal, ok := args["force"].(bool); ok {
		force = forceVal
	}

	// Validate registry manager
	if t.registryManager == nil {
		return ErrorResult("registry manager not configured")
	}

	// Validate installer
	if t.installer == nil {
		return ErrorResult("installer not configured")
	}

	// Set registry manager for installer if needed
	if t.installer != nil {
		t.installer.SetRegistryManager(t.registryManager)
	}

	// Determine which registry to use
	var targetRegistry string
	if registry != "" {
		// Use specified registry
		reg := t.registryManager.GetRegistry(registry)
		if reg == nil {
			return ErrorResult(fmt.Sprintf("registry '%s' not found or not configured", registry))
		}
		targetRegistry = registry
	} else {
		// Use first available registry
		targetRegistry = "github" // default to github
		reg := t.registryManager.GetRegistry(targetRegistry)
		if reg == nil {
			return ErrorResult("no registries configured")
		}
	}

	// Check if skill already exists
	if !force {
		if origin, err := t.installer.GetOriginTracking(slug); err == nil {
			return ErrorResult(fmt.Sprintf("skill '%s' is already installed (version: %s, registry: %s, installed at: %d). Use force=true to reinstall.",
				slug, origin.InstalledVersion, origin.Registry, origin.InstalledAt))
		}
	}

	// Install skill
	err := t.installer.InstallFromRegistry(ctx, targetRegistry, slug, version)
	if err != nil {
		return ErrorResult(fmt.Sprintf("failed to install skill '%s': %v", slug, err))
	}

	return NewToolResult(fmt.Sprintf("✓ Skill '%s' installed successfully from registry '%s'", slug, targetRegistry))
}
