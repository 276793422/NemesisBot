// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package router_test

import (
	"context"
	"fmt"
	"sync"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/providers/router"
)

// ---------- helpers ----------

func makeCandidates() []router.Candidate {
	return []router.Candidate{
		{Provider: "groq", Model: "llama-3", CostPer1K: 0.001, QualityScore: 6.0, Priority: 3},
		{Provider: "anthropic", Model: "claude-3", CostPer1K: 0.015, QualityScore: 9.0, Priority: 1},
		{Provider: "deepseek", Model: "deepseek-chat", CostPer1K: 0.0005, QualityScore: 7.5, Priority: 2},
	}
}

// ---------- NewRouter tests ----------

func TestNewRouter_DefaultConfig(t *testing.T) {
	r := router.NewRouter(router.Config{})
	if r == nil {
		t.Fatal("NewRouter returned nil")
	}
	if r.GetPolicy() != router.PolicyFallback {
		t.Errorf("expected default policy %q, got %q", router.PolicyFallback, r.GetPolicy())
	}
}

func TestNewRouter_WithExplicitPolicy(t *testing.T) {
	cfg := router.Config{DefaultPolicy: router.PolicyCost}
	r := router.NewRouter(cfg)
	if r.GetPolicy() != router.PolicyCost {
		t.Errorf("expected policy %q, got %q", router.PolicyCost, r.GetPolicy())
	}
}

func TestNewRouter_WithCustomAliases(t *testing.T) {
	cfg := router.Config{
		DefaultPolicy: router.PolicyFallback,
		Aliases:       map[string]string{"my-custom": "openai/gpt-4o"},
	}
	r := router.NewRouter(cfg)

	// The custom alias should be merged with defaults; verify via Select resolving alias.
	// We test alias resolution indirectly through Select — "my-custom" should resolve
	// to "openai/gpt-4o". We can also check the resolved model reaches the policy.
	// For a more direct test, see aliases_test.go.

	// Select should work with the custom alias as model name
	candidates := []router.Candidate{
		{Provider: "openai", Model: "gpt-4o", CostPer1K: 0.01, QualityScore: 9.0, Priority: 1},
	}
	sel, err := r.Select(context.Background(), "my-custom", candidates)
	if err != nil {
		t.Fatalf("Select with custom alias returned error: %v", err)
	}
	if sel.Provider != "openai" {
		t.Errorf("expected provider openai, got %s", sel.Provider)
	}
}

// ---------- Select tests ----------

func TestSelect_EmptyCandidates(t *testing.T) {
	r := router.NewRouter(router.Config{})
	_, err := r.Select(context.Background(), "test-model", nil)
	if err == nil {
		t.Error("expected error for empty candidates, got nil")
	}
}

func TestSelect_CancelledContext(t *testing.T) {
	r := router.NewRouter(router.Config{})
	ctx, cancel := context.WithCancel(context.Background())
	cancel()

	candidates := makeCandidates()
	_, err := r.Select(ctx, "test", candidates)
	if err == nil {
		t.Error("expected error for cancelled context, got nil")
	}
}

func TestSelect_SingleCandidate(t *testing.T) {
	r := router.NewRouter(router.Config{})
	candidates := []router.Candidate{
		{Provider: "only", Model: "m", CostPer1K: 1.0, QualityScore: 5.0, Priority: 10},
	}
	sel, err := r.Select(context.Background(), "model", candidates)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if sel.Provider != "only" {
		t.Errorf("expected provider 'only', got %q", sel.Provider)
	}
}

func TestSelect_CostPolicy(t *testing.T) {
	cfg := router.Config{DefaultPolicy: router.PolicyCost}
	r := router.NewRouter(cfg)
	candidates := makeCandidates()

	sel, err := r.Select(context.Background(), "model", candidates)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	// deepseek has the lowest cost (0.0005)
	if sel.Provider != "deepseek" {
		t.Errorf("cost policy: expected provider deepseek, got %q", sel.Provider)
	}
}

func TestSelect_QualityPolicy(t *testing.T) {
	cfg := router.Config{DefaultPolicy: router.PolicyQuality}
	r := router.NewRouter(cfg)
	candidates := makeCandidates()

	sel, err := r.Select(context.Background(), "model", candidates)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	// anthropic has the highest quality (9.0)
	if sel.Provider != "anthropic" {
		t.Errorf("quality policy: expected provider anthropic, got %q", sel.Provider)
	}
}

func TestSelect_FallbackPolicy(t *testing.T) {
	cfg := router.Config{DefaultPolicy: router.PolicyFallback}
	r := router.NewRouter(cfg)
	candidates := makeCandidates()

	sel, err := r.Select(context.Background(), "model", candidates)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	// anthropic has priority=1 (lowest = highest priority)
	if sel.Provider != "anthropic" {
		t.Errorf("fallback policy: expected provider anthropic, got %q", sel.Provider)
	}
}

func TestSelect_LatencyPolicy_NoMetrics(t *testing.T) {
	cfg := router.Config{DefaultPolicy: router.PolicyLatency}
	r := router.NewRouter(cfg)
	candidates := makeCandidates()

	// No metrics recorded: should fall back to quality score
	sel, err := r.Select(context.Background(), "model", candidates)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if sel.Provider != "anthropic" {
		t.Errorf("latency policy (no metrics): expected provider anthropic (highest quality), got %q", sel.Provider)
	}
}

func TestSelect_LatencyPolicy_WithMetrics(t *testing.T) {
	cfg := router.Config{DefaultPolicy: router.PolicyLatency}
	r := router.NewRouter(cfg)
	candidates := makeCandidates()

	// Record fast latency for groq
	r.RecordMetric("groq", router.Metric{
		Latency:    50 * time.Millisecond,
		Success:    true,
		TokensUsed: 100,
		Cost:       0.001,
		Timestamp:  time.Now(),
	})
	// Record slow latency for anthropic
	r.RecordMetric("anthropic", router.Metric{
		Latency:    500 * time.Millisecond,
		Success:    true,
		TokensUsed: 100,
		Cost:       0.01,
		Timestamp:  time.Now(),
	})

	sel, err := r.Select(context.Background(), "model", candidates)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if sel.Provider != "groq" {
		t.Errorf("latency policy (with metrics): expected provider groq (fastest), got %q", sel.Provider)
	}
}

func TestSelect_LatencyPolicy_OneProviderWithMetrics(t *testing.T) {
	cfg := router.Config{DefaultPolicy: router.PolicyLatency}
	r := router.NewRouter(cfg)
	candidates := makeCandidates()

	// Only record metrics for one provider
	r.RecordMetric("groq", router.Metric{
		Latency:    50 * time.Millisecond,
		Success:    true,
		TokensUsed: 100,
		Cost:       0.001,
		Timestamp:  time.Now(),
	})

	sel, err := r.Select(context.Background(), "model", candidates)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	// groq has metrics, others don't — groq should be preferred
	if sel.Provider != "groq" {
		t.Errorf("latency policy (partial metrics): expected provider groq, got %q", sel.Provider)
	}
}

func TestSelect_RoundRobinPolicy(t *testing.T) {
	cfg := router.Config{DefaultPolicy: router.PolicyRoundRobin}
	r := router.NewRouter(cfg)
	candidates := makeCandidates()

	// Round robin should cycle through all candidates
	seen := map[string]int{}
	for i := 0; i < 9; i++ {
		sel, err := r.Select(context.Background(), "model", candidates)
		if err != nil {
			t.Fatalf("unexpected error at iteration %d: %v", i, err)
		}
		seen[sel.Provider]++
	}

	// With 3 candidates and 9 selections, each should appear 3 times
	for _, p := range []string{"groq", "anthropic", "deepseek"} {
		if seen[p] != 3 {
			t.Errorf("round_robin: expected 3 selections for %s, got %d", p, seen[p])
		}
	}
}

func TestSelect_RoundRobinPolicy_SingleCandidate(t *testing.T) {
	cfg := router.Config{DefaultPolicy: router.PolicyRoundRobin}
	r := router.NewRouter(cfg)
	candidates := []router.Candidate{
		{Provider: "only", Model: "m", CostPer1K: 1.0, QualityScore: 5.0, Priority: 10},
	}

	for i := 0; i < 5; i++ {
		sel, err := r.Select(context.Background(), "model", candidates)
		if err != nil {
			t.Fatalf("unexpected error at iteration %d: %v", i, err)
		}
		if sel.Provider != "only" {
			t.Errorf("round_robin single: expected 'only', got %q", sel.Provider)
		}
	}
}

func TestSelect_RoundRobinPolicy_DifferentModels(t *testing.T) {
	cfg := router.Config{DefaultPolicy: router.PolicyRoundRobin}
	r := router.NewRouter(cfg)
	candidates := []router.Candidate{
		{Provider: "a", Model: "m1"},
		{Provider: "b", Model: "m1"},
	}

	// Each model name should have its own independent counter.
	// Use a different model name to get a different counter.
	candidates2 := []router.Candidate{
		{Provider: "x", Model: "m2"},
		{Provider: "y", Model: "m2"},
	}

	sel1, _ := r.Select(context.Background(), "m1", candidates)
	sel2, _ := r.Select(context.Background(), "m2", candidates2)

	// First selection for each model should be candidates[1] (counter starts at 0, Add(1) → idx=1%2=1)
	if sel1.Provider != "b" {
		t.Errorf("round_robin m1 first call: expected 'b', got %q", sel1.Provider)
	}
	if sel2.Provider != "y" {
		t.Errorf("round_robin m2 first call: expected 'y', got %q", sel2.Provider)
	}
}

// ---------- SelectWithPolicy tests ----------

func TestSelectWithPolicy_EmptyCandidates(t *testing.T) {
	r := router.NewRouter(router.Config{})
	_, err := r.SelectWithPolicy(context.Background(), router.PolicyCost, "model", nil)
	if err == nil {
		t.Error("expected error for empty candidates, got nil")
	}
}

func TestSelectWithPolicy_CancelledContext(t *testing.T) {
	r := router.NewRouter(router.Config{})
	ctx, cancel := context.WithCancel(context.Background())
	cancel()

	_, err := r.SelectWithPolicy(ctx, router.PolicyCost, "model", makeCandidates())
	if err == nil {
		t.Error("expected error for cancelled context, got nil")
	}
}

func TestSelectWithPolicy_OverridesDefault(t *testing.T) {
	// Router default is fallback, but we override to cost
	cfg := router.Config{DefaultPolicy: router.PolicyFallback}
	r := router.NewRouter(cfg)
	candidates := makeCandidates()

	sel, err := r.SelectWithPolicy(context.Background(), router.PolicyCost, "model", candidates)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	// Cost policy should select deepseek (cheapest)
	if sel.Provider != "deepseek" {
		t.Errorf("SelectWithPolicy cost: expected deepseek, got %q", sel.Provider)
	}

	// Verify default policy hasn't changed
	if r.GetPolicy() != router.PolicyFallback {
		t.Errorf("default policy should still be fallback, got %q", r.GetPolicy())
	}
}

func TestSelectWithPolicy_AllPolicies(t *testing.T) {
	candidates := makeCandidates()
	r := router.NewRouter(router.Config{})

	// Record metrics for latency testing
	r.RecordMetric("groq", router.Metric{
		Latency:    50 * time.Millisecond,
		Success:    true,
		TokensUsed: 100,
		Cost:       0.001,
		Timestamp:  time.Now(),
	})

	tests := []struct {
		name           string
		policy         router.Policy
		expectedTarget string
	}{
		{"cost", router.PolicyCost, "deepseek"},
		{"quality", router.PolicyQuality, "anthropic"},
		{"latency", router.PolicyLatency, "groq"},
		{"fallback", router.PolicyFallback, "anthropic"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			sel, err := r.SelectWithPolicy(context.Background(), tt.policy, "model", candidates)
			if err != nil {
				t.Fatalf("unexpected error: %v", err)
			}
			if sel.Provider != tt.expectedTarget {
				t.Errorf("policy %s: expected provider %q, got %q", tt.name, tt.expectedTarget, sel.Provider)
			}
		})
	}
}

func TestSelectWithPolicy_UnknownPolicy(t *testing.T) {
	r := router.NewRouter(router.Config{})
	candidates := makeCandidates()

	sel, err := r.SelectWithPolicy(context.Background(), router.Policy("unknown"), "model", candidates)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	// Unknown policy should fall back to fallback policy (anthropic, priority=1)
	if sel.Provider != "anthropic" {
		t.Errorf("unknown policy: expected fallback to anthropic, got %q", sel.Provider)
	}
}

// ---------- RecordMetric & GetMetrics tests ----------

func TestRecordMetricAndGetMetrics(t *testing.T) {
	r := router.NewRouter(router.Config{})

	ts := time.Now()
	r.RecordMetric("test-provider", router.Metric{
		Latency:    200 * time.Millisecond,
		Success:    true,
		TokensUsed: 500,
		Cost:       0.05,
		Timestamp:  ts,
	})

	m := r.GetMetrics("test-provider")
	if m == nil {
		t.Fatal("GetMetrics returned nil")
	}
	if m.TotalRequests != 1 {
		t.Errorf("expected 1 total request, got %d", m.TotalRequests)
	}
	if m.TotalFailures != 0 {
		t.Errorf("expected 0 failures, got %d", m.TotalFailures)
	}
	if m.SuccessRate != 1.0 {
		t.Errorf("expected success rate 1.0, got %f", m.SuccessRate)
	}
	if m.AvgLatency != 200*time.Millisecond {
		t.Errorf("expected avg latency 200ms, got %v", m.AvgLatency)
	}
}

func TestGetMetrics_UnknownProvider(t *testing.T) {
	r := router.NewRouter(router.Config{})
	m := r.GetMetrics("nonexistent")
	if m != nil {
		t.Errorf("expected nil for unknown provider, got %+v", m)
	}
}

func TestRecordMetric_MultipleProviders(t *testing.T) {
	r := router.NewRouter(router.Config{})

	r.RecordMetric("a", router.Metric{
		Latency: 100 * time.Millisecond, Success: true, TokensUsed: 100, Cost: 0.01,
	})
	r.RecordMetric("b", router.Metric{
		Latency: 200 * time.Millisecond, Success: false, TokensUsed: 200, Cost: 0.02,
	})

	ma := r.GetMetrics("a")
	mb := r.GetMetrics("b")

	if ma == nil || mb == nil {
		t.Fatal("expected non-nil metrics for both providers")
	}
	if ma.AvgLatency != 100*time.Millisecond {
		t.Errorf("provider a: expected avg latency 100ms, got %v", ma.AvgLatency)
	}
	if mb.SuccessRate != 0.0 {
		t.Errorf("provider b: expected success rate 0.0, got %f", mb.SuccessRate)
	}
}

func TestRecordMetric_MultipleRecords(t *testing.T) {
	r := router.NewRouter(router.Config{})

	for i := 0; i < 10; i++ {
		success := i%2 == 0 // 5 successes, 5 failures
		r.RecordMetric("provider", router.Metric{
			Latency:    time.Duration(100+i*10) * time.Millisecond,
			Success:    success,
			TokensUsed: 100,
			Cost:       0.01,
		})
	}

	m := r.GetMetrics("provider")
	if m.TotalRequests != 10 {
		t.Errorf("expected 10 requests, got %d", m.TotalRequests)
	}
	if m.TotalFailures != 5 {
		t.Errorf("expected 5 failures, got %d", m.TotalFailures)
	}
	if m.SuccessRate != 0.5 {
		t.Errorf("expected success rate 0.5, got %f", m.SuccessRate)
	}
}

// ---------- SetPolicy / GetPolicy tests ----------

func TestSetPolicy(t *testing.T) {
	r := router.NewRouter(router.Config{})
	if r.GetPolicy() != router.PolicyFallback {
		t.Errorf("initial: expected fallback, got %q", r.GetPolicy())
	}

	r.SetPolicy(router.PolicyCost)
	if r.GetPolicy() != router.PolicyCost {
		t.Errorf("after SetPolicy: expected cost, got %q", r.GetPolicy())
	}
}

func TestSetPolicy_ConcurrentSafe(t *testing.T) {
	r := router.NewRouter(router.Config{})
	policies := []router.Policy{
		router.PolicyCost,
		router.PolicyQuality,
		router.PolicyLatency,
		router.PolicyRoundRobin,
		router.PolicyFallback,
	}

	var wg sync.WaitGroup
	for i := 0; i < 50; i++ {
		wg.Add(1)
		go func(idx int) {
			defer wg.Done()
			r.SetPolicy(policies[idx%len(policies)])
			_ = r.GetPolicy()
		}(i)
	}
	wg.Wait()
}

// ---------- SetAliases tests ----------

func TestSetAliases(t *testing.T) {
	r := router.NewRouter(router.Config{})

	newAliases := map[string]string{
		"custom": "custom/model",
	}
	r.SetAliases(newAliases)

	candidates := []router.Candidate{
		{Provider: "custom", Model: "model", CostPer1K: 1.0, QualityScore: 5.0, Priority: 1},
	}
	sel, err := r.Select(context.Background(), "custom", candidates)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if sel.Provider != "custom" {
		t.Errorf("expected provider custom, got %q", sel.Provider)
	}
}

// ---------- Select with alias resolution ----------

func TestSelect_WithDefaultAlias(t *testing.T) {
	r := router.NewRouter(router.Config{})
	candidates := []router.Candidate{
		{Provider: "groq", Model: "llama-3.3-70b-versatile", CostPer1K: 0.001, QualityScore: 6.0, Priority: 1},
		{Provider: "anthropic", Model: "claude-3", CostPer1K: 0.015, QualityScore: 9.0, Priority: 2},
	}

	// "fast" alias should resolve to "groq/llama-3.3-70b-versatile"
	sel, err := r.Select(context.Background(), "fast", candidates)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	// Should work without error — the alias is resolved, but the candidates are still
	// matched by the policy, not by model name matching
	_ = sel
}

// ---------- Concurrent Select test ----------

func TestSelect_Concurrent(t *testing.T) {
	cfg := router.Config{DefaultPolicy: router.PolicyRoundRobin}
	r := router.NewRouter(cfg)
	candidates := makeCandidates()

	var wg sync.WaitGroup
	errors := make(chan error, 100)
	for i := 0; i < 100; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			sel, err := r.Select(context.Background(), "model", candidates)
			if err != nil {
				errors <- err
				return
			}
			if sel == nil {
				errors <- fmt.Errorf("nil candidate returned")
			}
		}()
	}
	wg.Wait()
	close(errors)

	for err := range errors {
		t.Errorf("concurrent select error: %v", err)
	}
}

// ---------- Latency policy edge cases ----------

func TestSelect_LatencyPolicy_MixedMetrics(t *testing.T) {
	cfg := router.Config{DefaultPolicy: router.PolicyLatency}
	r := router.NewRouter(cfg)
	candidates := []router.Candidate{
		{Provider: "groq", Model: "llama", QualityScore: 6.0},
		{Provider: "deepseek", Model: "chat", QualityScore: 7.5},
		{Provider: "anthropic", Model: "claude", QualityScore: 9.0},
	}

	// Record metrics for deepseek only (slow)
	r.RecordMetric("deepseek", router.Metric{
		Latency:    1 * time.Second,
		Success:    true,
		TokensUsed: 100,
	})

	sel, err := r.Select(context.Background(), "model", candidates)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	// groq and anthropic have no metrics; deepseek has slow metrics.
	// Only one provider has metrics → it should be ranked after those without metrics
	// Actually, per the code: "Only one has metrics: prefer the one with metrics" is false here.
	// The code says: if only mi has metrics, prefer mi (return true for i).
	// Let me re-read: for i=groq (no metrics) vs j=deepseek (has metrics):
	//   mi=nil → condition mi!=nil && mi.TotalRequests > 0 is false
	//   mj!=nil && mj.TotalRequests > 0 is true → return false
	// So groq is NOT preferred over deepseek. deepseek wins.
	// Wait, actually the code prefers the one WITH metrics:
	//   if mi has metrics → true (prefer i)
	//   if mj has metrics → false (prefer j)
	// So deepseek (with metrics) should be preferred over groq/anthropic (without metrics).
	if sel.Provider != "deepseek" {
		t.Errorf("latency with mixed metrics: expected deepseek (only one with metrics), got %q", sel.Provider)
	}
}
