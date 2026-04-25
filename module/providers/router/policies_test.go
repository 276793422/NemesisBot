// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package router_test

import (
	"context"
	"testing"

	"github.com/276793422/NemesisBot/module/providers/router"
)

// ---------- PolicyConfig default tests ----------

func TestPolicyConfig_Fields(t *testing.T) {
	cfg := router.PolicyConfig{
		Name:        "test",
		Description: "a test policy",
		Policy:      router.PolicyCost,
		Weights:     router.PolicyWeights{Cost: 1.0},
	}
	if cfg.Name != "test" {
		t.Errorf("expected name 'test', got %q", cfg.Name)
	}
	if cfg.Policy != router.PolicyCost {
		t.Errorf("expected policy cost, got %q", cfg.Policy)
	}
}

// ---------- PolicyWeights presets tests ----------

func TestPolicyWeights_FastPreset(t *testing.T) {
	cfg := router.GetPolicy("fast")
	if cfg.Policy != router.PolicyLatency {
		t.Errorf("fast policy: expected PolicyLatency, got %q", cfg.Policy)
	}
	if cfg.Weights.Latency != 1.0 {
		t.Errorf("fast weights: expected Latency=1.0, got %f", cfg.Weights.Latency)
	}
	if cfg.Weights.Cost != 0.0 {
		t.Errorf("fast weights: expected Cost=0.0, got %f", cfg.Weights.Cost)
	}
	if cfg.Weights.Quality != 0.0 {
		t.Errorf("fast weights: expected Quality=0.0, got %f", cfg.Weights.Quality)
	}
}

func TestPolicyWeights_BalancedPreset(t *testing.T) {
	cfg := router.GetPolicy("balanced")
	if cfg.Policy != router.PolicyQuality {
		t.Errorf("balanced policy: expected PolicyQuality, got %q", cfg.Policy)
	}
	if cfg.Weights.Cost != 0.33 {
		t.Errorf("balanced weights: expected Cost=0.33, got %f", cfg.Weights.Cost)
	}
	if cfg.Weights.Quality != 0.34 {
		t.Errorf("balanced weights: expected Quality=0.34, got %f", cfg.Weights.Quality)
	}
	if cfg.Weights.Latency != 0.33 {
		t.Errorf("balanced weights: expected Latency=0.33, got %f", cfg.Weights.Latency)
	}
}

func TestPolicyWeights_CheapPreset(t *testing.T) {
	cfg := router.GetPolicy("cheap")
	if cfg.Policy != router.PolicyCost {
		t.Errorf("cheap policy: expected PolicyCost, got %q", cfg.Policy)
	}
	if cfg.Weights.Cost != 1.0 {
		t.Errorf("cheap weights: expected Cost=1.0, got %f", cfg.Weights.Cost)
	}
}

func TestPolicyWeights_BestPreset(t *testing.T) {
	cfg := router.GetPolicy("best")
	if cfg.Policy != router.PolicyQuality {
		t.Errorf("best policy: expected PolicyQuality, got %q", cfg.Policy)
	}
	if cfg.Weights.Quality != 1.0 {
		t.Errorf("best weights: expected Quality=1.0, got %f", cfg.Weights.Quality)
	}
}

// ---------- GetPolicy tests ----------

func TestGetPolicy_KnownPolicies(t *testing.T) {
	tests := []struct {
		name         string
		expectedType router.Policy
	}{
		{"fast", router.PolicyLatency},
		{"balanced", router.PolicyQuality},
		{"cheap", router.PolicyCost},
		{"best", router.PolicyQuality},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			cfg := router.GetPolicy(tt.name)
			if cfg.Policy != tt.expectedType {
				t.Errorf("GetPolicy(%q): expected %q, got %q", tt.name, tt.expectedType, cfg.Policy)
			}
			if cfg.Name != tt.name {
				t.Errorf("GetPolicy(%q): expected name %q, got %q", tt.name, tt.name, cfg.Name)
			}
		})
	}
}

func TestGetPolicy_UnknownReturnsBalanced(t *testing.T) {
	cfg := router.GetPolicy("nonexistent_policy")
	balanced := router.GetPolicy("balanced")

	if cfg.Policy != balanced.Policy {
		t.Errorf("unknown policy: expected balanced policy %q, got %q", balanced.Policy, cfg.Policy)
	}
	if cfg.Name != "balanced" {
		t.Errorf("unknown policy: expected name 'balanced', got %q", cfg.Name)
	}
}

// ---------- AllPolicies tests ----------

func TestAllPolicies(t *testing.T) {
	all := router.AllPolicies()
	expected := []string{"fast", "balanced", "cheap", "best"}

	if len(all) != len(expected) {
		t.Errorf("expected %d policies, got %d", len(expected), len(all))
	}

	for _, name := range expected {
		if _, ok := all[name]; !ok {
			t.Errorf("missing policy %q in AllPolicies result", name)
		}
	}
}

// ---------- PolicyNames tests ----------

func TestPolicyNames(t *testing.T) {
	names := router.PolicyNames()
	expected := map[string]bool{
		"fast":     true,
		"balanced": true,
		"cheap":    true,
		"best":     true,
	}

	if len(names) != len(expected) {
		t.Errorf("expected %d policy names, got %d", len(expected), len(names))
	}

	for _, name := range names {
		if !expected[name] {
			t.Errorf("unexpected policy name %q", name)
		}
	}
}

// ---------- Policy string constants ----------

func TestPolicyConstants(t *testing.T) {
	tests := []struct {
		policy   router.Policy
		expected string
	}{
		{router.PolicyCost, "cost"},
		{router.PolicyQuality, "quality"},
		{router.PolicyLatency, "latency"},
		{router.PolicyRoundRobin, "round_robin"},
		{router.PolicyFallback, "fallback"},
	}

	for _, tt := range tests {
		if string(tt.policy) != tt.expected {
			t.Errorf("expected policy string %q, got %q", tt.expected, string(tt.policy))
		}
	}
}

// ---------- PolicyWeights zero-value ----------

func TestPolicyWeights_ZeroValue(t *testing.T) {
	var w router.PolicyWeights
	if w.Cost != 0.0 || w.Quality != 0.0 || w.Latency != 0.0 {
		t.Errorf("zero-value PolicyWeights should have all fields 0.0, got Cost=%f, Quality=%f, Latency=%f",
			w.Cost, w.Quality, w.Latency)
	}
}

// ---------- Integration: use preset policies with Router ----------

func TestPresetPolicy_IntegrationWithRouter(t *testing.T) {
	tests := []struct {
		name           string
		presetName     string
		expectedTarget string
	}{
		{"cheap_selects_cheapest", "cheap", "deepseek"},
		{"best_selects_highest_quality", "best", "anthropic"},
	}

	candidates := []router.Candidate{
		{Provider: "groq", Model: "llama", CostPer1K: 0.001, QualityScore: 6.0, Priority: 3},
		{Provider: "anthropic", Model: "claude", CostPer1K: 0.015, QualityScore: 9.0, Priority: 1},
		{Provider: "deepseek", Model: "chat", CostPer1K: 0.0005, QualityScore: 7.5, Priority: 2},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			policyCfg := router.GetPolicy(tt.presetName)
			r := router.NewRouter(router.Config{DefaultPolicy: policyCfg.Policy})
			sel, err := r.Select(context.Background(), "model", candidates)
			if err != nil {
				t.Fatalf("unexpected error: %v", err)
			}
			if sel.Provider != tt.expectedTarget {
				t.Errorf("preset %q: expected provider %q, got %q", tt.presetName, tt.expectedTarget, sel.Provider)
			}
		})
	}
}
