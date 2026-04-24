// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package router

import (
	"sync"
	"time"
)

const (
	// maxObservations is the size of the circular buffer per provider.
	maxObservations = 1000
)

// observation records a single metric event for a provider.
type observation struct {
	latency    time.Duration
	success    bool
	tokensUsed int
	cost       float64
	timestamp  time.Time
}

// providerStats holds a circular buffer of observations for a single provider.
type providerStats struct {
	observations []observation
	writeIdx     int
	count        int
}

// add inserts an observation into the circular buffer, overwriting the oldest entry when full.
func (ps *providerStats) add(obs observation) {
	if ps.observations == nil {
		ps.observations = make([]observation, maxObservations)
	}
	ps.observations[ps.writeIdx] = obs
	ps.writeIdx = (ps.writeIdx + 1) % maxObservations
	if ps.count < maxObservations {
		ps.count++
	}
}

// compute calculates aggregated metrics from the circular buffer.
func (ps *providerStats) compute(provider string) *ProviderMetrics {
	if ps.count == 0 {
		return nil
	}

	metrics := &ProviderMetrics{
		Provider: provider,
	}

	var totalLatency time.Duration
	var totalCost float64
	var totalTokens int
	var failures int64
	var lastUsed time.Time

	for i := 0; i < ps.count; i++ {
		obs := ps.observations[i]
		totalLatency += obs.latency
		totalCost += obs.cost
		totalTokens += obs.tokensUsed
		if !obs.success {
			failures++
		}
		if obs.timestamp.After(lastUsed) {
			lastUsed = obs.timestamp
		}
	}

	metrics.TotalRequests = int64(ps.count)
	metrics.TotalFailures = failures
	metrics.AvgLatency = totalLatency / time.Duration(ps.count)
	metrics.SuccessRate = float64(ps.count-int(failures)) / float64(ps.count)
	metrics.LastUsed = lastUsed

	if totalTokens > 0 {
		metrics.AvgCostPer1K = totalCost / float64(totalTokens) * 1000
	}

	return metrics
}

// prune removes observations older than the given duration from the current time.
// After pruning, remaining observations are compacted to the start of the buffer.
func (ps *providerStats) prune(olderThan time.Duration, now time.Time) {
	if ps.count == 0 {
		return
	}

	cutoff := now.Add(-olderThan)
	var kept []observation

	for i := 0; i < ps.count; i++ {
		if !ps.observations[i].timestamp.Before(cutoff) {
			kept = append(kept, ps.observations[i])
		}
	}

	// Reset buffer and re-insert kept observations
	ps.observations = make([]observation, maxObservations)
	ps.writeIdx = 0
	ps.count = 0
	for _, obs := range kept {
		ps.add(obs)
	}
}

// MetricsCollector records and aggregates provider performance metrics.
// Each provider maintains a circular buffer of the last 1000 observations.
// Thread-safe via sync.RWMutex.
type MetricsCollector struct {
	mu       sync.RWMutex
	providers map[string]*providerStats
}

// NewMetricsCollector creates a new MetricsCollector.
func NewMetricsCollector() *MetricsCollector {
	return &MetricsCollector{
		providers: make(map[string]*providerStats),
	}
}

// Record adds a metric observation for the given provider.
func (mc *MetricsCollector) Record(provider string, latency time.Duration, success bool, tokens int, cost float64) {
	mc.mu.Lock()
	defer mc.mu.Unlock()

	stats, exists := mc.providers[provider]
	if !exists {
		stats = &providerStats{}
		mc.providers[provider] = stats
	}

	stats.add(observation{
		latency:    latency,
		success:    success,
		tokensUsed: tokens,
		cost:       cost,
		timestamp:  time.Now(),
	})
}

// GetMetrics returns aggregated metrics for a specific provider.
// Returns nil if no observations have been recorded for the provider.
func (mc *MetricsCollector) GetMetrics(provider string) *ProviderMetrics {
	mc.mu.RLock()
	defer mc.mu.RUnlock()

	stats, exists := mc.providers[provider]
	if !exists {
		return nil
	}
	return stats.compute(provider)
}

// GetAllMetrics returns aggregated metrics for all providers.
func (mc *MetricsCollector) GetAllMetrics() map[string]*ProviderMetrics {
	mc.mu.RLock()
	defer mc.mu.RUnlock()

	result := make(map[string]*ProviderMetrics, len(mc.providers))
	for provider, stats := range mc.providers {
		result[provider] = stats.compute(provider)
	}
	return result
}

// Reset clears all recorded metrics for a specific provider.
func (mc *MetricsCollector) Reset(provider string) {
	mc.mu.Lock()
	defer mc.mu.Unlock()
	delete(mc.providers, provider)
}

// Prune removes observations older than the given duration from all providers.
func (mc *MetricsCollector) Prune(olderThan time.Duration) {
	mc.mu.Lock()
	defer mc.mu.Unlock()

	now := time.Now()
	for _, stats := range mc.providers {
		stats.prune(olderThan, now)
	}
}
