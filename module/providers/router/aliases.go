// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package router

// DefaultAliases returns the built-in alias mappings from short names to provider/model strings.
// These aliases allow users to reference models by simple names like "fast" or "smart"
// instead of full provider/model identifiers.
func DefaultAliases() map[string]string {
	return map[string]string{
		"fast":      "groq/llama-3.3-70b-versatile",
		"smart":     "anthropic/claude-sonnet-4-20250514",
		"cheap":     "deepseek/deepseek-chat",
		"local":     "ollama/llama3.3",
		"reasoning": "openai/o3-mini",
		"code":      "anthropic/claude-sonnet-4-20250514",
	}
}

// ResolveAlias looks up the name in the aliases map and returns the mapped value.
// If the name is not found in the map, the original name is returned unchanged.
func ResolveAlias(aliases map[string]string, name string) string {
	if aliases == nil {
		return name
	}
	if resolved, ok := aliases[name]; ok {
		return resolved
	}
	return name
}

// MergeAliases combines custom aliases with the defaults.
// Custom aliases take precedence over defaults when keys collide.
// Neither input map is modified; a new map is returned.
func MergeAliases(defaults, custom map[string]string) map[string]string {
	result := make(map[string]string, len(defaults)+len(custom))

	// Apply defaults first
	for k, v := range defaults {
		result[k] = v
	}

	// Custom overrides defaults
	for k, v := range custom {
		result[k] = v
	}

	return result
}
