// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package router

import (
	"context"
	"fmt"
	"sort"
	"sync"
	"sync/atomic"
	"time"
)

// Policy defines the selection strategy for provider routing.
type Policy string

const (
	// PolicyCost selects the cheapest provider first (sorted by CostPer1K ascending).
	PolicyCost Policy = "cost"
	// PolicyQuality selects the highest quality provider first (sorted by QualityScore descending).
	PolicyQuality Policy = "quality"
	// PolicyLatency selects the fastest provider based on recorded metrics (sorted by AvgLatency ascending).
	PolicyLatency Policy = "latency"
	// PolicyRoundRobin rotates through available providers sequentially.
	PolicyRoundRobin Policy = "round_robin"
	// PolicyFallback tries providers in priority order until one succeeds.
	PolicyFallback Policy = "fallback"
)

// Config holds router configuration.
type Config struct {
	DefaultPolicy Policy            `json:"policy"`
	Aliases       map[string]string `json:"aliases"`
}

// Candidate represents a single provider/model combination eligible for selection.
type Candidate struct {
	Provider     string  `json:"provider"`
	Model        string  `json:"model"`
	CostPer1K    float64 `json:"cost_per_1k_tokens"` // estimated cost per 1K tokens
	QualityScore float64 `json:"quality_score"`      // 0-10
	Priority     int     `json:"priority"`           // lower = higher priority (used by fallback policy)
}

// Metric represents a single observation of provider performance.
type Metric struct {
	Provider   string
	Latency    time.Duration
	Success    bool
	TokensUsed int
	Cost       float64
	Timestamp  time.Time
}

// ProviderMetrics holds aggregated performance metrics for a provider.
type ProviderMetrics struct {
	Provider      string
	AvgLatency    time.Duration
	SuccessRate   float64 // 0-1
	TotalRequests int64
	TotalFailures int64
	AvgCostPer1K  float64
	LastUsed      time.Time
}

// Router intelligently selects the best provider based on configurable policies.
// It is safe for concurrent use via sync.RWMutex.
type Router struct {
	metrics       *MetricsCollector
	aliases       map[string]string
	defaultPolicy Policy
	rrCounters    map[string]*atomic.Uint64 // per-model round-robin counters
	mu            sync.RWMutex
}

// NewRouter creates a new Router with the given configuration.
func NewRouter(cfg Config) *Router {
	aliases := DefaultAliases()
	if len(cfg.Aliases) > 0 {
		aliases = MergeAliases(aliases, cfg.Aliases)
	}

	policy := cfg.DefaultPolicy
	if policy == "" {
		policy = PolicyFallback
	}

	return &Router{
		metrics:       NewMetricsCollector(),
		aliases:       aliases,
		defaultPolicy: policy,
		rrCounters:    make(map[string]*atomic.Uint64),
	}
}

// Select chooses the best candidate based on the default policy.
// Returns an error if no candidates are provided.
// The selected candidate is determined by the active policy:
//   - cost: lowest CostPer1K
//   - quality: highest QualityScore
//   - latency: lowest recorded AvgLatency (falls back to quality for unrecorded providers)
//   - round_robin: rotates sequentially through candidates
//   - fallback: highest priority (lowest Priority value)
func (r *Router) Select(ctx context.Context, model string, candidates []Candidate) (*Candidate, error) {
	if len(candidates) == 0 {
		return nil, fmt.Errorf("router: no candidates available for model %q", model)
	}

	if ctx.Err() != nil {
		return nil, ctx.Err()
	}

	// Resolve model alias
	resolvedModel := r.resolveAlias(model)

	r.mu.RLock()
	policy := r.defaultPolicy
	r.mu.RUnlock()

	return r.selectByPolicy(policy, resolvedModel, candidates)
}

// SelectWithPolicy chooses the best candidate using the specified policy override.
func (r *Router) SelectWithPolicy(ctx context.Context, policy Policy, model string, candidates []Candidate) (*Candidate, error) {
	if len(candidates) == 0 {
		return nil, fmt.Errorf("router: no candidates available for model %q", model)
	}

	if ctx.Err() != nil {
		return nil, ctx.Err()
	}

	resolvedModel := r.resolveAlias(model)
	return r.selectByPolicy(policy, resolvedModel, candidates)
}

// RecordMetric records a performance observation for a provider.
func (r *Router) RecordMetric(provider string, metric Metric) {
	r.metrics.Record(provider, metric.Latency, metric.Success, metric.TokensUsed, metric.Cost)
}

// GetMetrics returns aggregated metrics for a specific provider.
// Returns nil if no metrics have been recorded for the provider.
func (r *Router) GetMetrics(provider string) *ProviderMetrics {
	return r.metrics.GetMetrics(provider)
}

// SetPolicy changes the default routing policy.
func (r *Router) SetPolicy(policy Policy) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.defaultPolicy = policy
}

// GetPolicy returns the current default routing policy.
func (r *Router) GetPolicy() Policy {
	r.mu.RLock()
	defer r.mu.RUnlock()
	return r.defaultPolicy
}

// SetAliases updates the alias mappings.
func (r *Router) SetAliases(aliases map[string]string) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.aliases = aliases
}

// ResolveAlias resolves a short name to a provider/model string, or returns the original if no alias matches.
func (r *Router) resolveAlias(name string) string {
	r.mu.RLock()
	defer r.mu.RUnlock()
	return ResolveAlias(r.aliases, name)
}

// selectByPolicy applies the given policy to select a candidate.
func (r *Router) selectByPolicy(policy Policy, model string, candidates []Candidate) (*Candidate, error) {
	switch policy {
	case PolicyCost:
		return r.selectByCost(candidates), nil
	case PolicyQuality:
		return r.selectByQuality(candidates), nil
	case PolicyLatency:
		return r.selectByLatency(candidates), nil
	case PolicyRoundRobin:
		return r.selectByRoundRobin(model, candidates), nil
	case PolicyFallback:
		return r.selectByFallback(candidates), nil
	default:
		// Unknown policy: fall back to fallback policy
		return r.selectByFallback(candidates), nil
	}
}

// selectByCost selects the candidate with the lowest cost per 1K tokens.
func (r *Router) selectByCost(candidates []Candidate) *Candidate {
	if len(candidates) == 0 {
		return nil
	}

	sorted := make([]Candidate, len(candidates))
	copy(sorted, candidates)
	sort.Slice(sorted, func(i, j int) bool {
		return sorted[i].CostPer1K < sorted[j].CostPer1K
	})
	return &sorted[0]
}

// selectByQuality selects the candidate with the highest quality score.
func (r *Router) selectByQuality(candidates []Candidate) *Candidate {
	if len(candidates) == 0 {
		return nil
	}

	sorted := make([]Candidate, len(candidates))
	copy(sorted, candidates)
	sort.Slice(sorted, func(i, j int) bool {
		return sorted[i].QualityScore > sorted[j].QualityScore
	})
	return &sorted[0]
}

// selectByLatency selects the candidate with the lowest average latency from recorded metrics.
// Candidates without recorded metrics are ranked by quality score as a tiebreaker.
func (r *Router) selectByLatency(candidates []Candidate) *Candidate {
	if len(candidates) == 0 {
		return nil
	}

	sorted := make([]Candidate, len(candidates))
	copy(sorted, candidates)
	sort.Slice(sorted, func(i, j int) bool {
		mi := r.metrics.GetMetrics(sorted[i].Provider)
		mj := r.metrics.GetMetrics(sorted[j].Provider)

		// Both have metrics: compare latency
		if mi != nil && mi.TotalRequests > 0 && mj != nil && mj.TotalRequests > 0 {
			return mi.AvgLatency < mj.AvgLatency
		}

		// Only one has metrics: prefer the one with metrics
		if mi != nil && mi.TotalRequests > 0 {
			return true
		}
		if mj != nil && mj.TotalRequests > 0 {
			return false
		}

		// Neither has metrics: fall back to quality score
		return sorted[i].QualityScore > sorted[j].QualityScore
	})
	return &sorted[0]
}

// selectByRoundRobin rotates through candidates sequentially, maintaining a per-model counter.
func (r *Router) selectByRoundRobin(model string, candidates []Candidate) *Candidate {
	if len(candidates) == 0 {
		return nil
	}

	if len(candidates) == 1 {
		return &candidates[0]
	}

	r.mu.Lock()
	counter, exists := r.rrCounters[model]
	if !exists {
		counter = &atomic.Uint64{}
		r.rrCounters[model] = counter
	}
	r.mu.Unlock()

	idx := counter.Add(1) % uint64(len(candidates))
	return &candidates[idx]
}

// selectByFallback selects the candidate with the highest priority (lowest Priority value).
func (r *Router) selectByFallback(candidates []Candidate) *Candidate {
	if len(candidates) == 0 {
		return nil
	}

	sorted := make([]Candidate, len(candidates))
	copy(sorted, candidates)
	sort.Slice(sorted, func(i, j int) bool {
		return sorted[i].Priority < sorted[j].Priority
	})
	return &sorted[0]
}
