package forge

import (
	"testing"
	"time"
)

// --- TraceStore tests ---

func TestNewTraceStore(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewTraceStore(tmpDir, cfg)
	if store == nil {
		t.Fatal("NewTraceStore returned nil")
	}
}

func TestTraceStore_AppendAndRead(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewTraceStore(tmpDir, cfg)

	now := time.Now().UTC()
	trace := &ConversationTrace{
		TraceID:     "trace-001",
		SessionKey:  "hashed-session-key",
		Channel:     "web",
		StartTime:   now,
		EndTime:     now.Add(5 * time.Minute),
		DurationMs:  300000,
		TotalRounds: 4,
		ToolSteps: []ToolStep{
			{ToolName: "read_file", Success: true, DurationMs: 100, LLMRound: 1, ChainPos: 0, ArgKeys: []string{"path"}},
			{ToolName: "edit_file", Success: true, DurationMs: 200, LLMRound: 2, ChainPos: 1, ArgKeys: []string{"path", "content"}},
		},
		Signals:    []SessionSignal{},
		TokensUsed: 500,
	}

	if err := store.Append(trace); err != nil {
		t.Fatalf("Append failed: %v", err)
	}

	// Read back
	traces, err := store.ReadTraces(time.Time{})
	if err != nil {
		t.Fatalf("ReadTraces failed: %v", err)
	}
	if len(traces) != 1 {
		t.Fatalf("Expected 1 trace, got %d", len(traces))
	}

	got := traces[0]
	if got.TraceID != "trace-001" {
		t.Errorf("Expected TraceID 'trace-001', got '%s'", got.TraceID)
	}
	if got.Channel != "web" {
		t.Errorf("Expected Channel 'web', got '%s'", got.Channel)
	}
	if got.TotalRounds != 4 {
		t.Errorf("Expected TotalRounds 4, got %d", got.TotalRounds)
	}
	if len(got.ToolSteps) != 2 {
		t.Errorf("Expected 2 tool steps, got %d", len(got.ToolSteps))
	}
	if got.ToolSteps[0].ToolName != "read_file" {
		t.Errorf("Expected first tool 'read_file', got '%s'", got.ToolSteps[0].ToolName)
	}
	if got.TokensUsed != 500 {
		t.Errorf("Expected TokensUsed 500, got %d", got.TokensUsed)
	}
}

func TestTraceStore_MultipleTraces(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewTraceStore(tmpDir, cfg)

	now := time.Now().UTC()
	for i := 0; i < 10; i++ {
		trace := &ConversationTrace{
			TraceID:     "trace-" + string(rune('0'+i)),
			StartTime:   now,
			TotalRounds: i + 1,
			ToolSteps: []ToolStep{
				{ToolName: "tool", Success: true},
			},
		}
		if err := store.Append(trace); err != nil {
			t.Fatalf("Append %d failed: %v", i, err)
		}
	}

	traces, _ := store.ReadTraces(time.Time{})
	if len(traces) != 10 {
		t.Errorf("Expected 10 traces, got %d", len(traces))
	}
}

func TestTraceStore_ReadTraces_Since(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewTraceStore(tmpDir, cfg)

	now := time.Now().UTC()
	trace := &ConversationTrace{
		TraceID:   "trace-today",
		StartTime: now,
	}
	store.Append(trace)

	// Read since today's start
	startOfToday := time.Date(now.Year(), now.Month(), now.Day(), 0, 0, 0, 0, time.UTC)
	traces, _ := store.ReadTraces(startOfToday)
	if len(traces) != 1 {
		t.Errorf("Expected 1 trace since start of today, got %d", len(traces))
	}

	// Read since tomorrow (should be 0)
	tomorrow := now.AddDate(0, 0, 1)
	traces2, _ := store.ReadTraces(tomorrow)
	if len(traces2) != 0 {
		t.Errorf("Expected 0 traces since tomorrow, got %d", len(traces2))
	}
}

func TestTraceStore_Cleanup(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewTraceStore(tmpDir, cfg)

	// Write old trace
	oldTime := time.Now().UTC().AddDate(0, 0, -60)
	oldTrace := &ConversationTrace{
		TraceID:   "trace-old",
		StartTime: oldTime,
	}
	store.Append(oldTrace)

	// Write new trace
	newTrace := &ConversationTrace{
		TraceID:   "trace-new",
		StartTime: time.Now().UTC(),
	}
	store.Append(newTrace)

	// Cleanup files older than 30 days
	if err := store.Cleanup(30); err != nil {
		t.Fatalf("Cleanup failed: %v", err)
	}

	traces, _ := store.ReadTraces(time.Time{})
	if len(traces) != 1 {
		t.Errorf("Expected 1 trace after cleanup, got %d", len(traces))
	}
	if len(traces) > 0 && traces[0].TraceID != "trace-new" {
		t.Errorf("Expected 'trace-new', got '%s'", traces[0].TraceID)
	}
}

func TestTraceStore_fileNewerThan(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewTraceStore(tmpDir, cfg)

	// Today's file should be newer than yesterday
	yesterday := time.Now().UTC().AddDate(0, 0, -1)
	filename := time.Now().UTC().Format("20060102") + ".jsonl"

	if !store.fileNewerThan(filename, yesterday) {
		t.Error("Today's file should be newer than yesterday")
	}
	if store.fileNewerThan("20200101.jsonl", time.Now().UTC()) {
		t.Error("Old file should not be newer than now")
	}
}

// --- TraceCollector tests ---

func TestTraceCollector_Name(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewTraceStore(tmpDir, cfg)
	collector := NewTraceCollector(store, cfg)

	if collector.Name() != "forge_trace" {
		t.Errorf("Expected name 'forge_trace', got '%s'", collector.Name())
	}
}

// --- hashSessionKey tests ---

func TestHashSessionKey(t *testing.T) {
	key1 := hashSessionKey("session-abc")
	key2 := hashSessionKey("session-abc")
	key3 := hashSessionKey("session-def")

	if key1 != key2 {
		t.Error("Same input should produce same hash")
	}
	if key1 == key3 {
		t.Error("Different input should produce different hash")
	}
	if len(key1) != 64 {
		t.Errorf("SHA256 hex should be 64 chars, got %d", len(key1))
	}
}

// --- truncateError tests ---

func TestTruncateError_Short(t *testing.T) {
	input := "short error"
	result := truncateError(input)
	if result != input {
		t.Errorf("Short error should not be truncated: %s", result)
	}
}

func TestTruncateError_Long(t *testing.T) {
	longError := ""
	for i := 0; i < 200; i++ {
		longError += "x"
	}
	result := truncateError(longError)
	if len(result) > 100 {
		t.Errorf("Long error should be truncated to <= 100 chars, got %d", len(result))
	}
}

// --- detectSignals tests ---

func TestDetectSignals_NoRetry(t *testing.T) {
	steps := []ToolStep{
		{ToolName: "read_file", Success: true, LLMRound: 1},
		{ToolName: "edit_file", Success: true, LLMRound: 2},
	}
	signals := detectSignals(steps, time.Now().UTC())
	if len(signals) != 0 {
		t.Errorf("Expected no signals for clean execution, got %d", len(signals))
	}
}

func TestDetectSignals_Retry(t *testing.T) {
	steps := []ToolStep{
		{ToolName: "exec", Success: false, LLMRound: 1},
		{ToolName: "exec", Success: true, LLMRound: 2},
	}
	signals := detectSignals(steps, time.Now().UTC())
	foundRetry := false
	for _, s := range signals {
		if s.Type == "retry" {
			foundRetry = true
		}
	}
	if !foundRetry {
		t.Error("Expected retry signal when same tool fails then succeeds")
	}
}

func TestDetectSignals_Backtrack(t *testing.T) {
	steps := []ToolStep{
		{ToolName: "exec", Success: false, LLMRound: 1},
		{ToolName: "read_file", Success: true, LLMRound: 1}, // different tool in same round after failure
	}
	signals := detectSignals(steps, time.Now().UTC())
	foundBacktrack := false
	for _, s := range signals {
		if s.Type == "backtrack" {
			foundBacktrack = true
		}
	}
	if !foundBacktrack {
		t.Error("Expected backtrack signal when different tool used after failure")
	}
}

func TestDetectSignals_NoBacktrack_SameTool(t *testing.T) {
	steps := []ToolStep{
		{ToolName: "exec", Success: false, LLMRound: 1},
		{ToolName: "exec", Success: true, LLMRound: 1}, // same tool, not a backtrack
	}
	signals := detectSignals(steps, time.Now().UTC())
	for _, s := range signals {
		if s.Type == "backtrack" {
			t.Error("Same tool after failure should not produce backtrack signal")
		}
	}
}

// --- maxRound tests ---

func TestMaxRound(t *testing.T) {
	byRound := map[int][]ToolStep{
		1: {{}},
		3: {{}},
		2: {{}},
	}
	result := maxRound(byRound)
	if result != 3 {
		t.Errorf("Expected max round 3, got %d", result)
	}
}

func TestMaxRound_Empty(t *testing.T) {
	result := maxRound(map[int][]ToolStep{})
	if result != 0 {
		t.Errorf("Expected 0 for empty map, got %d", result)
	}
}
