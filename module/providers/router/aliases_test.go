// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package router_test

import (
	"context"
	"testing"

	"github.com/276793422/NemesisBot/module/providers/router"
)

// ---------- DefaultAliases tests ----------

func TestDefaultAliases(t *testing.T) {
	aliases := router.DefaultAliases()

	expected := map[string]string{
		"fast":      "groq/llama-3.3-70b-versatile",
		"smart":     "anthropic/claude-sonnet-4-20250514",
		"cheap":     "deepseek/deepseek-chat",
		"local":     "ollama/llama3.3",
		"reasoning": "openai/o3-mini",
		"code":      "anthropic/claude-sonnet-4-20250514",
	}

	if len(aliases) != len(expected) {
		t.Errorf("expected %d default aliases, got %d", len(expected), len(aliases))
	}

	for k, v := range expected {
		if aliases[k] != v {
			t.Errorf("default alias %q: expected %q, got %q", k, v, aliases[k])
		}
	}
}

func TestDefaultAliases_ReturnsNewMap(t *testing.T) {
	// Each call should return a fresh map, not a shared reference
	a1 := router.DefaultAliases()
	a2 := router.DefaultAliases()

	a1["test"] = "modified"
	if a2["test"] == "modified" {
		t.Error("DefaultAliases should return a new map each time, not a shared reference")
	}
}

// ---------- ResolveAlias tests ----------

func TestResolveAlias_KnownAliases(t *testing.T) {
	aliases := router.DefaultAliases()

	tests := []struct {
		input    string
		expected string
	}{
		{"fast", "groq/llama-3.3-70b-versatile"},
		{"smart", "anthropic/claude-sonnet-4-20250514"},
		{"cheap", "deepseek/deepseek-chat"},
		{"local", "ollama/llama3.3"},
		{"reasoning", "openai/o3-mini"},
		{"code", "anthropic/claude-sonnet-4-20250514"},
	}

	for _, tt := range tests {
		t.Run(tt.input, func(t *testing.T) {
			result := router.ResolveAlias(aliases, tt.input)
			if result != tt.expected {
				t.Errorf("ResolveAlias(%q): expected %q, got %q", tt.input, tt.expected, result)
			}
		})
	}
}

func TestResolveAlias_UnknownAlias(t *testing.T) {
	aliases := router.DefaultAliases()
	result := router.ResolveAlias(aliases, "unknown_alias")
	if result != "unknown_alias" {
		t.Errorf("ResolveAlias for unknown: expected original 'unknown_alias', got %q", result)
	}
}

func TestResolveAlias_EmptyString(t *testing.T) {
	aliases := router.DefaultAliases()
	result := router.ResolveAlias(aliases, "")
	if result != "" {
		t.Errorf("ResolveAlias for empty string: expected empty, got %q", result)
	}
}

func TestResolveAlias_NilMap(t *testing.T) {
	result := router.ResolveAlias(nil, "fast")
	if result != "fast" {
		t.Errorf("ResolveAlias with nil map: expected 'fast', got %q", result)
	}
}

func TestResolveAlias_EmptyMap(t *testing.T) {
	result := router.ResolveAlias(map[string]string{}, "anything")
	if result != "anything" {
		t.Errorf("ResolveAlias with empty map: expected 'anything', got %q", result)
	}
}

func TestResolveAlias_FullModelString(t *testing.T) {
	aliases := router.DefaultAliases()
	// A full model string like "openai/gpt-4" should pass through unchanged
	result := router.ResolveAlias(aliases, "openai/gpt-4")
	if result != "openai/gpt-4" {
		t.Errorf("expected 'openai/gpt-4' to pass through unchanged, got %q", result)
	}
}

// ---------- MergeAliases tests ----------

func TestMergeAliases_CustomOverridesDefault(t *testing.T) {
	defaults := map[string]string{
		"fast":  "groq/llama",
		"smart": "anthropic/claude",
	}
	custom := map[string]string{
		"fast": "custom/fast-model", // override
	}

	result := router.MergeAliases(defaults, custom)

	if result["fast"] != "custom/fast-model" {
		t.Errorf("custom should override default for 'fast', got %q", result["fast"])
	}
	if result["smart"] != "anthropic/claude" {
		t.Errorf("default should be preserved for 'smart', got %q", result["smart"])
	}
}

func TestMergeAliases_AddsNewKeys(t *testing.T) {
	defaults := map[string]string{"fast": "groq/llama"}
	custom := map[string]string{"custom-alias": "my/model"}

	result := router.MergeAliases(defaults, custom)

	if result["custom-alias"] != "my/model" {
		t.Errorf("custom key should be added, got %q", result["custom-alias"])
	}
	if result["fast"] != "groq/llama" {
		t.Errorf("default key should be preserved, got %q", result["fast"])
	}
}

func TestMergeAliases_DoesNotModifyInputs(t *testing.T) {
	defaults := map[string]string{"fast": "groq/llama"}
	custom := map[string]string{"new": "model"}

	result := router.MergeAliases(defaults, custom)

	// Modify the result
	result["extra"] = "should-not-affect-inputs"

	if _, ok := defaults["extra"]; ok {
		t.Error("MergeAliases modified the defaults map")
	}
	if _, ok := custom["extra"]; ok {
		t.Error("MergeAliases modified the custom map")
	}
}

func TestMergeAliases_EmptyDefaults(t *testing.T) {
	custom := map[string]string{"my": "model"}
	result := router.MergeAliases(map[string]string{}, custom)

	if len(result) != 1 || result["my"] != "model" {
		t.Errorf("expected custom-only result, got %v", result)
	}
}

func TestMergeAliases_EmptyCustom(t *testing.T) {
	defaults := map[string]string{"fast": "groq/llama"}
	result := router.MergeAliases(defaults, map[string]string{})

	if len(result) != 1 || result["fast"] != "groq/llama" {
		t.Errorf("expected defaults-only result, got %v", result)
	}
}

func TestMergeAliases_BothEmpty(t *testing.T) {
	result := router.MergeAliases(map[string]string{}, map[string]string{})
	if len(result) != 0 {
		t.Errorf("expected empty result, got %d entries", len(result))
	}
}

func TestMergeAliases_NilInputs(t *testing.T) {
	// nil defaults should not panic
	result := router.MergeAliases(nil, map[string]string{"a": "b"})
	if result["a"] != "b" {
		t.Errorf("expected 'b' for key 'a', got %q", result["a"])
	}

	// nil custom should not panic
	result2 := router.MergeAliases(map[string]string{"c": "d"}, nil)
	if result2["c"] != "d" {
		t.Errorf("expected 'd' for key 'c', got %q", result2["c"])
	}
}

// ---------- Integration: aliases in Router ----------

func TestRouter_UsesDefaultAliases(t *testing.T) {
	r := router.NewRouter(router.Config{})

	// "fast" should resolve to "groq/llama-3.3-70b-versatile"
	// We verify this by checking that Select doesn't error and the model name is used
	candidates := []router.Candidate{
		{Provider: "groq", Model: "llama-3.3-70b-versatile", CostPer1K: 0.001, QualityScore: 6.0, Priority: 1},
	}

	sel, err := r.Select(context.Background(), "fast", candidates)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if sel == nil {
		t.Fatal("expected non-nil selection")
	}
}

func TestRouter_CustomAliasesOverrideDefaults(t *testing.T) {
	cfg := router.Config{
		Aliases: map[string]string{
			"fast": "custom/fast-model", // override the default "fast" alias
		},
	}
	r := router.NewRouter(cfg)

	candidates := []router.Candidate{
		{Provider: "custom", Model: "fast-model", CostPer1K: 0.001, QualityScore: 6.0, Priority: 1},
	}

	sel, err := r.Select(context.Background(), "fast", candidates)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if sel.Provider != "custom" {
		t.Errorf("expected custom alias to be resolved, got provider %q", sel.Provider)
	}
}
