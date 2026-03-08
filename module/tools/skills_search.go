// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package tools

import (
	"context"
	"fmt"
	"sort"
	"strings"

	"github.com/276793422/NemesisBot/module/skills"
)

// FindSkillsTool is a tool that searches for available skills from configured registries.
type FindSkillsTool struct {
	registryManager *skills.RegistryManager
	searchCache     *skills.SearchCache
}

// NewFindSkillsTool creates a new find_skills tool.
func NewFindSkillsTool(registryManager *skills.RegistryManager, searchCache *skills.SearchCache) *FindSkillsTool {
	return &FindSkillsTool{
		registryManager: registryManager,
		searchCache:     searchCache,
	}
}

// Name returns the tool name.
func (t *FindSkillsTool) Name() string {
	return "find_skills"
}

// Description returns the tool description.
func (t *FindSkillsTool) Description() string {
	return "Search for available skills from configured registries (GitHub, ClawHub, etc.). Use this tool when you need to find or discover new skills to install."
}

// ParameterSchema returns the tool's parameter schema.
func (t *FindSkillsTool) ParameterSchema() map[string]interface{} {
	return map[string]interface{}{
		"type": "object",
		"properties": map[string]interface{}{
			"query": map[string]interface{}{
				"type":        "string",
				"description": "Search query (e.g., 'weather', 'github', 'docker'). Leave empty to list all available skills.",
			},
			"limit": map[string]interface{}{
				"type":        "integer",
				"description": "Maximum number of results to return (1-50, default: 5)",
				"minimum":     1,
				"maximum":     50,
			},
		},
		"required": []string{"query"},
	}
}

// Execute executes the find_skills tool.
func (t *FindSkillsTool) Execute(ctx context.Context, args map[string]interface{}) *ToolResult {
	// Parse parameters
	query, _ := args["query"].(string)
	limit := 5 // default limit

	if limitVal, ok := args["limit"].(float64); ok {
		limit = int(limitVal)
		if limit < 1 {
			limit = 1
		} else if limit > 50 {
			limit = 50
		}
	}

	// Validate registry manager
	if t.registryManager == nil {
		return ErrorResult("registry manager not configured")
	}

	// Search registries
	results, err := t.registryManager.SearchAll(ctx, query, limit)
	if err != nil {
		return ErrorResult(fmt.Sprintf("failed to search registries: %v", err))
	}

	// Format output
	if len(results) == 0 {
		return NewToolResult(fmt.Sprintf("No skills found for query '%s'", query))
	}

	// Sort by score descending
	sort.Slice(results, func(i, j int) bool {
		return results[i].Score > results[j].Score
	})

	// Build formatted output
	var output strings.Builder
	output.WriteString(fmt.Sprintf("Found %d skill(s) for \"%s\":\n", len(results), query))

	for i, result := range results {
		output.WriteString(fmt.Sprintf("\n%d. **%s**", i+1, result.Slug))
		if result.Version != "" {
			output.WriteString(fmt.Sprintf(" v%s", result.Version))
		}
		output.WriteString(fmt.Sprintf(" (score: %.2f, registry: %s)\n", result.Score, result.RegistryName))

		if result.DisplayName != "" {
			output.WriteString(fmt.Sprintf("   Display Name: %s\n", result.DisplayName))
		}

		if result.Summary != "" {
			output.WriteString(fmt.Sprintf("   Description: %s\n", result.Summary))
		}
	}

	// Add cache statistics if available
	if t.searchCache != nil {
		stats := t.searchCache.Stats()
		if stats.Size > 0 {
			output.WriteString(fmt.Sprintf("\n[Cache Stats: %d entries, %.1f%% hit rate]",
				stats.Size, stats.HitRate*100))
		}
	}

	return NewToolResult(output.String())
}
