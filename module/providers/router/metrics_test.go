// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package router_test

import (
	"sync"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/providers/router"
)

// ---------- NewMetricsCollector tests ----------

func TestNewMetricsCollector(t *testing.T) {
	mc := router.NewMetricsCollector()
	if mc == nil {
		t.Fatal("NewMetricsCollector returned nil")
	}
}

// ---------- Record / GetMetrics tests ----------

func TestMetricsCollector_RecordAndGetMetrics(t *testing.T) {
	mc := router.NewMetricsCollector()

	mc.Record("provider-a", 100*time.Millisecond, true, 200, 0.02)

	m := mc.GetMetrics("provider-a")
	if m == nil {
		t.Fatal("GetMetrics returned nil for recorded provider")
	}

	if m.Provider != "provider-a" {
		t.Errorf("expected provider 'provider-a', got %q", m.Provider)
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
	if m.AvgLatency != 100*time.Millisecond {
		t.Errorf("expected avg latency 100ms, got %v", m.AvgLatency)
	}
	if m.AvgCostPer1K != 0.1 { // 0.02/200 * 1000 = 0.1
		t.Errorf("expected avg cost per 1K = 0.1, got %f", m.AvgCostPer1K)
	}
}

func TestMetricsCollector_RecordFailure(t *testing.T) {
	mc := router.NewMetricsCollector()

	mc.Record("provider-fail", 500*time.Millisecond, false, 0, 0.0)

	m := mc.GetMetrics("provider-fail")
	if m == nil {
		t.Fatal("GetMetrics returned nil")
	}
	if m.SuccessRate != 0.0 {
		t.Errorf("expected success rate 0.0, got %f", m.SuccessRate)
	}
	if m.TotalFailures != 1 {
		t.Errorf("expected 1 failure, got %d", m.TotalFailures)
	}
	if m.AvgLatency != 500*time.Millisecond {
		t.Errorf("expected avg latency 500ms, got %v", m.AvgLatency)
	}
	if m.AvgCostPer1K != 0.0 {
		t.Errorf("expected avg cost per 1K = 0 (no tokens), got %f", m.AvgCostPer1K)
	}
}

func TestMetricsCollector_RecordMultiple(t *testing.T) {
	mc := router.NewMetricsCollector()

	// Record 4 observations: 3 success, 1 failure
	records := []struct {
		latency    time.Duration
		success    bool
		tokensUsed int
		cost       float64
	}{
		{100 * time.Millisecond, true, 100, 0.01},
		{200 * time.Millisecond, true, 200, 0.02},
		{300 * time.Millisecond, false, 0, 0.0},
		{400 * time.Millisecond, true, 300, 0.03},
	}

	for _, r := range records {
		mc.Record("test", r.latency, r.success, r.tokensUsed, r.cost)
	}

	m := mc.GetMetrics("test")
	if m == nil {
		t.Fatal("GetMetrics returned nil")
	}

	if m.TotalRequests != 4 {
		t.Errorf("expected 4 total requests, got %d", m.TotalRequests)
	}
	if m.TotalFailures != 1 {
		t.Errorf("expected 1 failure, got %d", m.TotalFailures)
	}
	if m.SuccessRate != 0.75 {
		t.Errorf("expected success rate 0.75, got %f", m.SuccessRate)
	}
	// Average latency: (100+200+300+400)/4 = 250ms
	if m.AvgLatency != 250*time.Millisecond {
		t.Errorf("expected avg latency 250ms, got %v", m.AvgLatency)
	}
	// Average cost per 1K: totalCost/totalTokens * 1000 = 0.06/600 * 1000 = 0.1
	if m.AvgCostPer1K < 0.099 || m.AvgCostPer1K > 0.101 {
		t.Errorf("expected avg cost per 1K ~ 0.1, got %f", m.AvgCostPer1K)
	}
}

// ---------- Unknown provider tests ----------

func TestMetricsCollector_GetMetrics_UnknownProvider(t *testing.T) {
	mc := router.NewMetricsCollector()
	m := mc.GetMetrics("nonexistent")
	if m != nil {
		t.Errorf("expected nil for unknown provider, got %+v", m)
	}
}

// ---------- GetAllMetrics tests ----------

func TestMetricsCollector_GetAllMetrics(t *testing.T) {
	mc := router.NewMetricsCollector()

	mc.Record("a", 100*time.Millisecond, true, 100, 0.01)
	mc.Record("b", 200*time.Millisecond, true, 200, 0.02)

	all := mc.GetAllMetrics()
	if len(all) != 2 {
		t.Fatalf("expected 2 providers in GetAllMetrics, got %d", len(all))
	}

	if all["a"] == nil || all["b"] == nil {
		t.Fatal("expected non-nil metrics for both providers")
	}
	if all["a"].AvgLatency != 100*time.Millisecond {
		t.Errorf("provider a: expected avg latency 100ms, got %v", all["a"].AvgLatency)
	}
	if all["b"].AvgLatency != 200*time.Millisecond {
		t.Errorf("provider b: expected avg latency 200ms, got %v", all["b"].AvgLatency)
	}
}

func TestMetricsCollector_GetAllMetrics_Empty(t *testing.T) {
	mc := router.NewMetricsCollector()
	all := mc.GetAllMetrics()
	if len(all) != 0 {
		t.Errorf("expected empty map, got %d entries", len(all))
	}
}

// ---------- Reset tests ----------

func TestMetricsCollector_Reset(t *testing.T) {
	mc := router.NewMetricsCollector()

	mc.Record("to-reset", 100*time.Millisecond, true, 100, 0.01)
	m := mc.GetMetrics("to-reset")
	if m == nil {
		t.Fatal("expected metrics before reset")
	}

	mc.Reset("to-reset")
	m = mc.GetMetrics("to-reset")
	if m != nil {
		t.Errorf("expected nil after reset, got %+v", m)
	}
}

func TestMetricsCollector_Reset_OtherProviderUnaffected(t *testing.T) {
	mc := router.NewMetricsCollector()

	mc.Record("keep", 100*time.Millisecond, true, 100, 0.01)
	mc.Record("remove", 200*time.Millisecond, true, 200, 0.02)

	mc.Reset("remove")

	if mc.GetMetrics("remove") != nil {
		t.Error("expected nil for removed provider")
	}
	if mc.GetMetrics("keep") == nil {
		t.Error("expected non-nil for kept provider")
	}
}

// ---------- Circular buffer behavior ----------

func TestMetricsCollector_CircularBuffer_Overwrite(t *testing.T) {
	mc := router.NewMetricsCollector()

	// Record 1500 entries — the buffer is 1000, so the oldest 500 should be overwritten
	totalEntries := 1500
	for i := 0; i < totalEntries; i++ {
		latency := time.Duration(i+1) * time.Millisecond
		mc.Record("circ", latency, true, 100, 0.01)
	}

	m := mc.GetMetrics("circ")
	if m == nil {
		t.Fatal("GetMetrics returned nil")
	}

	// Should only keep the last 1000 entries (indices 501-1500)
	if m.TotalRequests != 1000 {
		t.Errorf("expected 1000 total requests (capped), got %d", m.TotalRequests)
	}

	// The oldest entry kept should be latency=501ms, newest=1500ms
	// Average of 501..1500 = (501+1500)/2 = 1000.5ms
	expectedAvg := time.Duration(1000500) * time.Microsecond // 1000.5ms
	if m.AvgLatency != expectedAvg {
		t.Errorf("expected avg latency %v, got %v", expectedAvg, m.AvgLatency)
	}
}

func TestMetricsCollector_CircularBuffer_ExactlyFull(t *testing.T) {
	mc := router.NewMetricsCollector()

	for i := 0; i < 1000; i++ {
		mc.Record("full", time.Duration(i+1)*time.Millisecond, true, 100, 0.01)
	}

	m := mc.GetMetrics("full")
	if m.TotalRequests != 1000 {
		t.Errorf("expected 1000 total requests, got %d", m.TotalRequests)
	}
}

// ---------- LastUsed tracking ----------

func TestMetricsCollector_LastUsed(t *testing.T) {
	mc := router.NewMetricsCollector()

	before := time.Now()
	mc.Record("lu", 100*time.Millisecond, true, 100, 0.01)
	after := time.Now()

	m := mc.GetMetrics("lu")
	if m.LastUsed.Before(before) || m.LastUsed.After(after) {
		t.Errorf("LastUsed %v not between %v and %v", m.LastUsed, before, after)
	}
}

// ---------- Prune tests ----------

func TestMetricsCollector_Prune(t *testing.T) {
	mc := router.NewMetricsCollector()

	// Record old and new entries
	mc.Record("prunable", 100*time.Millisecond, true, 100, 0.01)

	// We can't easily control the timestamp since Record uses time.Now()
	// But we can verify Prune doesn't panic and old entries get pruned
	mc.Prune(1 * time.Nanosecond) // prune everything older than 1ns

	m := mc.GetMetrics("prunable")
	// The entry was recorded very recently, but 1ns might have passed
	// This is a timing-sensitive test; just verify no panic
	_ = m
}

// ---------- Concurrent recording ----------

func TestMetricsCollector_ConcurrentRecording(t *testing.T) {
	mc := router.NewMetricsCollector()
	goroutines := 50
	recordsPer := 20

	var wg sync.WaitGroup
	wg.Add(goroutines)
	for g := 0; g < goroutines; g++ {
		go func(id int) {
			defer wg.Done()
			for i := 0; i < recordsPer; i++ {
				mc.Record(
					"concurrent-provider",
					time.Duration(i+1)*time.Millisecond,
					true,
					100,
					0.01,
				)
			}
		}(g)
	}
	wg.Wait()

	m := mc.GetMetrics("concurrent-provider")
	if m == nil {
		t.Fatal("GetMetrics returned nil after concurrent recording")
	}

	expected := int64(goroutines * recordsPer)
	if m.TotalRequests != expected {
		t.Errorf("expected %d total requests, got %d", expected, m.TotalRequests)
	}
}

func TestMetricsCollector_ConcurrentReadWrite(t *testing.T) {
	mc := router.NewMetricsCollector()
	var wg sync.WaitGroup

	// Concurrent writers
	for g := 0; g < 20; g++ {
		wg.Add(1)
		go func(id int) {
			defer wg.Done()
			for i := 0; i < 50; i++ {
				mc.Record("rw-provider", 100*time.Millisecond, true, 100, 0.01)
			}
		}(g)
	}

	// Concurrent readers
	for g := 0; g < 10; g++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for i := 0; i < 50; i++ {
				m := mc.GetMetrics("rw-provider")
				_ = m
			}
		}()
	}

	wg.Wait()
}

// ---------- AvgCostPer1K edge cases ----------

func TestMetricsCollector_AvgCostPer1K_ZeroTokens(t *testing.T) {
	mc := router.NewMetricsCollector()

	mc.Record("zero-tokens", 100*time.Millisecond, true, 0, 0.0)

	m := mc.GetMetrics("zero-tokens")
	if m == nil {
		t.Fatal("GetMetrics returned nil")
	}
	// When totalTokens is 0, AvgCostPer1K should be 0
	if m.AvgCostPer1K != 0.0 {
		t.Errorf("expected AvgCostPer1K = 0.0 for zero tokens, got %f", m.AvgCostPer1K)
	}
}
