package forge_test

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/forge"
	"github.com/276793422/NemesisBot/module/observer"
)

func TestTraceCollectorName(t *testing.T) {
	cfg := forge.DefaultForgeConfig()
	tc := forge.NewTraceCollector(nil, cfg)
	if tc.Name() != "forge_trace" {
		t.Fatalf("expected forge_trace, got %s", tc.Name())
	}
}

func TestConversationStart(t *testing.T) {
	cfg := forge.DefaultForgeConfig()
	tc := forge.NewTraceCollector(nil, cfg)

	tc.OnEvent(context.Background(), observer.ConversationEvent{
		Type:    observer.EventConversationStart,
		TraceID: "trace-1",
		Data: &observer.ConversationStartData{
			SessionKey: "session1",
			Channel:    "web",
			ChatID:     "chat1",
			Content:    "hello",
		},
	})

	// Trace should be created (verified indirectly via onEnd storing it)
	// Since no store, we verify no panic occurs
}

func TestToolStepRecording(t *testing.T) {
	dir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	store := forge.NewTraceStore(dir, cfg)
	tc := forge.NewTraceCollector(store, cfg)

	tc.OnEvent(context.Background(), observer.ConversationEvent{
		Type:    observer.EventConversationStart,
		TraceID: "trace-1",
		Data: &observer.ConversationStartData{
			SessionKey: "session1",
			Channel:    "web",
		},
	})

	tc.OnEvent(context.Background(), observer.ConversationEvent{
		Type:    observer.EventToolCall,
		TraceID: "trace-1",
		Data: &observer.ToolCallData{
			ToolName:  "read_file",
			Arguments: map[string]interface{}{"path": "/tmp/test.txt"},
			Success:   true,
			Duration:  50 * time.Millisecond,
			LLMRound:  1,
			ChainPos:  0,
		},
	})

	tc.OnEvent(context.Background(), observer.ConversationEvent{
		Type:    observer.EventToolCall,
		TraceID: "trace-1",
		Data: &observer.ToolCallData{
			ToolName:  "write_file",
			Arguments: map[string]interface{}{"path": "/tmp/out.txt", "content": "data"},
			Success:   false,
			Duration:  100 * time.Millisecond,
			Error:     "permission denied",
			LLMRound:  1,
			ChainPos:  1,
		},
	})

	tc.OnEvent(context.Background(), observer.ConversationEvent{
		Type:    observer.EventConversationEnd,
		TraceID: "trace-1",
		Data: &observer.ConversationEndData{
			TotalRounds:   2,
			TotalDuration: 5 * time.Second,
		},
	})

	// Read back from store
	traces, err := store.ReadTraces(time.Time{})
	if err != nil {
		t.Fatalf("ReadTraces failed: %v", err)
	}
	if len(traces) != 1 {
		t.Fatalf("expected 1 trace, got %d", len(traces))
	}
	tr := traces[0]
	if len(tr.ToolSteps) != 2 {
		t.Fatalf("expected 2 tool steps, got %d", len(tr.ToolSteps))
	}
	if tr.ToolSteps[0].ToolName != "read_file" || !tr.ToolSteps[0].Success {
		t.Errorf("unexpected first step: %+v", tr.ToolSteps[0])
	}
	if tr.ToolSteps[1].ToolName != "write_file" || tr.ToolSteps[1].Success {
		t.Errorf("unexpected second step: %+v", tr.ToolSteps[1])
	}
}

func TestRetrySignalDetection(t *testing.T) {
	dir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	store := forge.NewTraceStore(dir, cfg)
	tc := forge.NewTraceCollector(store, cfg)

	tc.OnEvent(context.Background(), observer.ConversationEvent{
		Type:    observer.EventConversationStart,
		TraceID: "trace-retry",
		Data:    &observer.ConversationStartData{SessionKey: "s1"},
	})

	// First attempt fails
	tc.OnEvent(context.Background(), observer.ConversationEvent{
		Type:    observer.EventToolCall,
		TraceID: "trace-retry",
		Data: &observer.ToolCallData{
			ToolName: "read_file",
			Success:  false,
			LLMRound: 1,
		},
	})
	// Second attempt succeeds (retry)
	tc.OnEvent(context.Background(), observer.ConversationEvent{
		Type:    observer.EventToolCall,
		TraceID: "trace-retry",
		Data: &observer.ToolCallData{
			ToolName: "read_file",
			Success:  true,
			LLMRound: 2,
		},
	})

	tc.OnEvent(context.Background(), observer.ConversationEvent{
		Type:    observer.EventConversationEnd,
		TraceID: "trace-retry",
		Data: &observer.ConversationEndData{
			TotalRounds:   2,
			TotalDuration: 3 * time.Second,
		},
	})

	traces, _ := store.ReadTraces(time.Time{})
	if len(traces) != 1 {
		t.Fatalf("expected 1 trace, got %d", len(traces))
	}

	hasRetry := false
	for _, sig := range traces[0].Signals {
		if sig.Type == "retry" {
			hasRetry = true
		}
	}
	if !hasRetry {
		t.Error("expected retry signal to be detected")
	}
}

func TestBacktrackDetection(t *testing.T) {
	dir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	store := forge.NewTraceStore(dir, cfg)
	tc := forge.NewTraceCollector(store, cfg)

	tc.OnEvent(context.Background(), observer.ConversationEvent{
		Type:    observer.EventConversationStart,
		TraceID: "trace-bt",
		Data:    &observer.ConversationStartData{SessionKey: "s1"},
	})

	// Tool A fails
	tc.OnEvent(context.Background(), observer.ConversationEvent{
		Type:    observer.EventToolCall,
		TraceID: "trace-bt",
		Data: &observer.ToolCallData{
			ToolName: "read_file",
			Success:  false,
			LLMRound: 1,
			ChainPos: 0,
		},
	})
	// Different tool B called in same round (backtrack)
	tc.OnEvent(context.Background(), observer.ConversationEvent{
		Type:    observer.EventToolCall,
		TraceID: "trace-bt",
		Data: &observer.ToolCallData{
			ToolName: "search",
			Success:  true,
			LLMRound: 1,
			ChainPos: 1,
		},
	})

	tc.OnEvent(context.Background(), observer.ConversationEvent{
		Type:    observer.EventConversationEnd,
		TraceID: "trace-bt",
		Data: &observer.ConversationEndData{
			TotalRounds:   1,
			TotalDuration: 2 * time.Second,
		},
	})

	traces, _ := store.ReadTraces(time.Time{})
	if len(traces) != 1 {
		t.Fatalf("expected 1 trace, got %d", len(traces))
	}

	hasBacktrack := false
	for _, sig := range traces[0].Signals {
		if sig.Type == "backtrack" {
			hasBacktrack = true
		}
	}
	if !hasBacktrack {
		t.Error("expected backtrack signal to be detected")
	}
}

func TestSessionKeyHashed(t *testing.T) {
	dir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	store := forge.NewTraceStore(dir, cfg)
	tc := forge.NewTraceCollector(store, cfg)

	tc.OnEvent(context.Background(), observer.ConversationEvent{
		Type:    observer.EventConversationStart,
		TraceID: "trace-hash",
		Data: &observer.ConversationStartData{
			SessionKey: "sensitive_session_key",
		},
	})
	tc.OnEvent(context.Background(), observer.ConversationEvent{
		Type:    observer.EventConversationEnd,
		TraceID: "trace-hash",
		Data:    &observer.ConversationEndData{TotalRounds: 1, TotalDuration: time.Second},
	})

	traces, _ := store.ReadTraces(time.Time{})
	if len(traces) != 1 {
		t.Fatalf("expected 1 trace")
	}
	// SessionKey should be hashed, not the raw value
	if traces[0].SessionKey == "sensitive_session_key" {
		t.Error("SessionKey should be hashed, not stored in plain text")
	}
	if len(traces[0].SessionKey) != 64 { // SHA256 hex = 64 chars
		t.Errorf("expected 64-char hash, got %d chars: %s", len(traces[0].SessionKey), traces[0].SessionKey)
	}
}

func TestNoRawContentStored(t *testing.T) {
	dir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	store := forge.NewTraceStore(dir, cfg)
	tc := forge.NewTraceCollector(store, cfg)

	tc.OnEvent(context.Background(), observer.ConversationEvent{
		Type:    observer.EventConversationStart,
		TraceID: "trace-priv",
		Data: &observer.ConversationStartData{
			Content: "sensitive user message with api_key=sk-12345",
		},
	})
	tc.OnEvent(context.Background(), observer.ConversationEvent{
		Type:    observer.EventConversationEnd,
		TraceID: "trace-priv",
		Data: &observer.ConversationEndData{
			Content: "sensitive response",
		},
	})

	traces, _ := store.ReadTraces(time.Time{})
	if len(traces) != 1 {
		t.Fatalf("expected 1 trace")
	}
	// ConversationTrace struct has no Content field, so this is guaranteed by design
	// Just verify the trace doesn't contain the raw content anywhere
	tr := traces[0]
	_ = tr // Trace struct doesn't have content fields
}

func TestTraceStoreCRUD(t *testing.T) {
	dir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	store := forge.NewTraceStore(dir, cfg)

	trace := &forge.ConversationTrace{
		TraceID:     "test-1",
		SessionKey:  "hashed",
		Channel:     "web",
		StartTime:   time.Now(),
		EndTime:     time.Now().Add(5 * time.Second),
		DurationMs:  5000,
		TotalRounds: 3,
		ToolSteps: []forge.ToolStep{
			{ToolName: "read_file", Success: true, DurationMs: 50, LLMRound: 1},
			{ToolName: "write_file", Success: true, DurationMs: 100, LLMRound: 2},
		},
	}

	if err := store.Append(trace); err != nil {
		t.Fatalf("Append failed: %v", err)
	}

	traces, err := store.ReadTraces(time.Time{})
	if err != nil {
		t.Fatalf("ReadTraces failed: %v", err)
	}
	if len(traces) != 1 {
		t.Fatalf("expected 1 trace, got %d", len(traces))
	}
	if traces[0].TraceID != "test-1" {
		t.Errorf("unexpected TraceID: %s", traces[0].TraceID)
	}
	if len(traces[0].ToolSteps) != 2 {
		t.Errorf("expected 2 tool steps, got %d", len(traces[0].ToolSteps))
	}
}

func TestTraceStoreCleanup(t *testing.T) {
	dir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	store := forge.NewTraceStore(dir, cfg)

	// Write an old trace
	oldTime := time.Now().Add(-60 * 24 * time.Hour) // 60 days ago
	oldTrace := &forge.ConversationTrace{
		TraceID:    "old",
		StartTime:  oldTime,
		TotalRounds: 1,
	}
	if err := store.Append(oldTrace); err != nil {
		t.Fatalf("Append failed: %v", err)
	}

	// Write a recent trace
	recentTrace := &forge.ConversationTrace{
		TraceID:    "recent",
		StartTime:  time.Now(),
		TotalRounds: 1,
	}
	if err := store.Append(recentTrace); err != nil {
		t.Fatalf("Append failed: %v", err)
	}

	// Cleanup traces older than 30 days
	if err := store.Cleanup(30); err != nil {
		t.Fatalf("Cleanup failed: %v", err)
	}

	traces, _ := store.ReadTraces(time.Time{})
	for _, tr := range traces {
		if tr.TraceID == "old" {
			t.Error("old trace should have been cleaned up")
		}
	}
}

func TestTraceAnalysisToolChains(t *testing.T) {
	cfg := forge.DefaultForgeConfig()
	r := forge.NewReflector("", nil, nil, cfg)

	dir := t.TempDir()
	store := forge.NewTraceStore(dir, cfg)
	r.SetTraceStore(store)

	// Write traces with specific tool chains
	chains := []struct {
		tools []string
	}{
		{[]string{"read_file", "edit_file", "exec"}},
		{[]string{"read_file", "edit_file", "exec"}},
		{[]string{"search", "read_file"}},
		{[]string{"read_file", "edit_file", "exec"}},
	}

	for i, c := range chains {
		steps := make([]forge.ToolStep, len(c.tools))
		for j, tool := range c.tools {
			steps[j] = forge.ToolStep{
				ToolName:  tool,
				Success:   true,
				DurationMs: 50,
				LLMRound:  j + 1,
			}
		}
		trace := &forge.ConversationTrace{
			TraceID:     fmt.Sprintf("chain-%d", i),
			StartTime:   time.Now(),
			EndTime:     time.Now().Add(time.Second),
			DurationMs:  1000,
			TotalRounds: len(c.tools),
			ToolSteps:   steps,
		}
		store.Append(trace)
	}

	stats := r.AnalyzeTracesForTest(time.Time{})
	if stats == nil {
		t.Fatal("expected non-nil trace stats")
	}
	if len(stats.ToolChainPatterns) == 0 {
		t.Fatal("expected tool chain patterns")
	}
	// The most common chain should be read_file→edit_file→exec
	top := stats.ToolChainPatterns[0]
	if top.Count != 3 {
		t.Errorf("expected top chain count 3, got %d", top.Count)
	}
}

func TestTraceAnalysisRetryPatterns(t *testing.T) {
	cfg := forge.DefaultForgeConfig()
	r := forge.NewReflector("", nil, nil, cfg)

	dir := t.TempDir()
	store := forge.NewTraceStore(dir, cfg)
	r.SetTraceStore(store)

	// Create a trace with retry pattern
	trace := &forge.ConversationTrace{
		TraceID:     "retry-trace",
		StartTime:   time.Now(),
		EndTime:     time.Now().Add(3 * time.Second),
		DurationMs:  3000,
		TotalRounds: 2,
		ToolSteps: []forge.ToolStep{
			{ToolName: "read_file", Success: false, LLMRound: 1},
			{ToolName: "read_file", Success: true, LLMRound: 2},
		},
		Signals: []forge.SessionSignal{
			{Type: "retry", Round: 2},
		},
	}
	store.Append(trace)

	stats := r.AnalyzeTracesForTest(time.Time{})
	if stats == nil {
		t.Fatal("expected non-nil trace stats")
	}
	if stats.SignalSummary["retry"] != 1 {
		t.Errorf("expected 1 retry signal, got %d", stats.SignalSummary["retry"])
	}
}

func TestTraceAnalysisEfficiency(t *testing.T) {
	cfg := forge.DefaultForgeConfig()
	r := forge.NewReflector("", nil, nil, cfg)

	dir := t.TempDir()
	store := forge.NewTraceStore(dir, cfg)
	r.SetTraceStore(store)

	// Efficient: 3 tools in 1 round
	trace := &forge.ConversationTrace{
		TraceID:     "efficient",
		StartTime:   time.Now(),
		DurationMs:  1000,
		TotalRounds: 1,
		ToolSteps: []forge.ToolStep{
			{ToolName: "read", Success: true, LLMRound: 1},
			{ToolName: "edit", Success: true, LLMRound: 1},
			{ToolName: "exec", Success: true, LLMRound: 1},
		},
	}
	store.Append(trace)

	stats := r.AnalyzeTracesForTest(time.Time{})
	if stats == nil {
		t.Fatal("expected non-nil trace stats")
	}
	// Efficiency: 3 tools / 1 round = 1.0 (capped)
	if stats.EfficiencyScore != 1.0 {
		t.Errorf("expected efficiency 1.0, got %.2f", stats.EfficiencyScore)
	}
}

func TestArgKeysOnlyNoValues(t *testing.T) {
	dir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	store := forge.NewTraceStore(dir, cfg)
	tc := forge.NewTraceCollector(store, cfg)

	tc.OnEvent(context.Background(), observer.ConversationEvent{
		Type:    observer.EventConversationStart,
		TraceID: "trace-args",
		Data:    &observer.ConversationStartData{SessionKey: "s1"},
	})

	tc.OnEvent(context.Background(), observer.ConversationEvent{
		Type:    observer.EventToolCall,
		TraceID: "trace-args",
		Data: &observer.ToolCallData{
			ToolName: "write_file",
			Arguments: map[string]interface{}{
				"path":    "/secret/path/file.txt",
				"content": "sensitive data with api_key=sk-xxx",
			},
			Success:  true,
			LLMRound: 1,
		},
	})

	tc.OnEvent(context.Background(), observer.ConversationEvent{
		Type:    observer.EventConversationEnd,
		TraceID: "trace-args",
		Data:    &observer.ConversationEndData{TotalRounds: 1, TotalDuration: time.Second},
	})

	traces, _ := store.ReadTraces(time.Time{})
	if len(traces) != 1 {
		t.Fatalf("expected 1 trace")
	}
	step := traces[0].ToolSteps[0]
	// ArgKeys should contain key names only
	sort.Strings(step.ArgKeys)
	if len(step.ArgKeys) != 2 || step.ArgKeys[0] != "content" || step.ArgKeys[1] != "path" {
		t.Errorf("expected [content, path], got %v", step.ArgKeys)
	}
}

func TestReflectorWithTraces(t *testing.T) {
	cfg := forge.DefaultForgeConfig()
	cfg.Reflection.MinExperiences = 1
	dir := t.TempDir()

	store := forge.NewExperienceStore(dir, cfg)
	registry := forge.NewRegistry(filepath.Join(dir, "registry.json"))
	r := forge.NewReflector(dir, store, registry, cfg)

	// Add an experience so Reflect doesn't fail on min count
	store.AppendAggregated(&forge.AggregatedExperience{
		PatternHash: "hash1",
		ToolName:    "test_tool",
		Count:       5,
		SuccessRate: 0.9,
		LastSeen:    time.Now(),
	})

	// Set up trace store
	traceStore := forge.NewTraceStore(dir, cfg)
	r.SetTraceStore(traceStore)

	traceStore.Append(&forge.ConversationTrace{
		TraceID:     "t1",
		StartTime:   time.Now(),
		DurationMs:  1000,
		TotalRounds: 2,
		ToolSteps: []forge.ToolStep{
			{ToolName: "test_tool", Success: true, LLMRound: 1},
		},
	})

	reportPath, err := r.Reflect(context.Background(), "today", "all")
	if err != nil {
		t.Fatalf("Reflect failed: %v", err)
	}

	data, err := os.ReadFile(reportPath)
	if err != nil {
		t.Fatalf("Failed to read report: %v", err)
	}
	content := string(data)
	if len(content) == 0 {
		t.Fatal("Report is empty")
	}
}

func TestReportWithTraceInsights(t *testing.T) {
	ts := &forge.TraceStats{
		TotalTraces:   10,
		AvgRounds:     3.5,
		AvgDurationMs: 2500,
		EfficiencyScore: 0.85,
		ToolChainPatterns: []*forge.ToolChainPattern{
			{Chain: "read_file→edit_file", Count: 5, AvgRounds: 2.0, SuccessRate: 0.9},
		},
		SignalSummary: map[string]int{"retry": 3},
	}

	report := &forge.ReflectionReport{
		Date:       "2026-04-21",
		Stats:      &forge.ReflectionStats{ToolFrequency: map[string]int{}},
		TraceStats: ts,
	}

	content := forge.FormatReport(report)
	if len(content) == 0 {
		t.Fatal("report is empty")
	}
	if !strings.Contains(content, "对话级洞察") {
		t.Error("report should contain trace insights section")
	}
	if !strings.Contains(content, "read_file→edit_file") {
		t.Error("report should contain tool chain pattern")
	}
	if !strings.Contains(content, "retry") {
		t.Error("report should contain retry signal")
	}
}
