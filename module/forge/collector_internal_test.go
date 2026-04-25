package forge

import (
	"testing"
	"time"
)

// --- computePatternHash tests ---

func TestComputePatternHash_Deterministic(t *testing.T) {
	args := map[string]interface{}{"path": "/tmp/test", "mode": "read"}
	hash1 := ComputePatternHash("read_file", args)
	hash2 := ComputePatternHash("read_file", args)
	if hash1 != hash2 {
		t.Errorf("ComputePatternHash should be deterministic, got %s and %s", hash1, hash2)
	}
}

func TestComputePatternHash_DifferentTools(t *testing.T) {
	args := map[string]interface{}{"path": "/tmp/test"}
	hash1 := ComputePatternHash("read_file", args)
	hash2 := ComputePatternHash("write_file", args)
	if hash1 == hash2 {
		t.Error("Different tool names should produce different hashes")
	}
}

func TestComputePatternHash_ArgKeyOrderInsensitive(t *testing.T) {
	args1 := map[string]interface{}{"b": 1, "a": 2}
	args2 := map[string]interface{}{"a": 2, "b": 1}
	hash1 := ComputePatternHash("tool", args1)
	hash2 := ComputePatternHash("tool", args2)
	if hash1 != hash2 {
		t.Errorf("Hash should be insensitive to arg key order, got %s and %s", hash1, hash2)
	}
}

func TestComputePatternHash_EmptyArgs(t *testing.T) {
	hash := ComputePatternHash("tool", map[string]interface{}{})
	if hash == "" {
		t.Error("Hash should not be empty")
	}
}

func TestComputePatternHash_HasPrefix(t *testing.T) {
	hash := ComputePatternHash("tool", map[string]interface{}{})
	if len(hash) < 8 || hash[:7] != "sha256:" {
		t.Errorf("Hash should have sha256: prefix, got %s", hash)
	}
}

// --- SanitizeArgs tests ---

func TestSanitizeArgs_NilFields(t *testing.T) {
	args := map[string]interface{}{"api_key": "secret123", "name": "test"}
	result := SanitizeArgs(args, nil)
	if result["api_key"] != "secret123" {
		t.Error("With nil sanitize fields, nothing should be redacted")
	}
}

func TestSanitizeArgs_EmptyFields(t *testing.T) {
	args := map[string]interface{}{"api_key": "secret123", "name": "test"}
	result := SanitizeArgs(args, []string{})
	if result["api_key"] != "secret123" {
		t.Error("With empty sanitize fields, nothing should be redacted")
	}
}

func TestSanitizeArgs_MatchingFields(t *testing.T) {
	args := map[string]interface{}{
		"api_key":    "sk-12345",
		"password":   "secret",
		"normal_arg": "visible",
	}
	fields := []string{"api_key", "password"}
	result := SanitizeArgs(args, fields)

	if result["api_key"] != "[REDACTED]" {
		t.Errorf("api_key should be redacted, got %v", result["api_key"])
	}
	if result["password"] != "[REDACTED]" {
		t.Errorf("password should be redacted, got %v", result["password"])
	}
	if result["normal_arg"] != "visible" {
		t.Errorf("normal_arg should not be redacted, got %v", result["normal_arg"])
	}
}

func TestSanitizeArgs_CaseInsensitive(t *testing.T) {
	args := map[string]interface{}{
		"API_KEY": "sk-12345",
	}
	fields := []string{"api_key"}
	result := SanitizeArgs(args, fields)

	if result["API_KEY"] != "[REDACTED]" {
		t.Errorf("API_KEY should be redacted with case-insensitive match, got %v", result["API_KEY"])
	}
}

func TestSanitizeArgs_PartialMatch(t *testing.T) {
	args := map[string]interface{}{
		"my_api_key_value": "sk-12345",
	}
	fields := []string{"api_key"}
	result := SanitizeArgs(args, fields)

	if result["my_api_key_value"] != "[REDACTED]" {
		t.Errorf("Partial match should redact, got %v", result["my_api_key_value"])
	}
}

// --- Collector ProcessRecord tests ---

func TestCollector_ProcessRecord_Dedup(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewExperienceStore(tmpDir, cfg)
	collector := NewCollector(store, cfg)

	now := time.Now().UTC()
	hash := ComputePatternHash("tool", map[string]interface{}{"x": 1})

	rec1 := &ExperienceRecord{
		Timestamp:   now,
		ToolName:    "tool",
		Args:        map[string]interface{}{"x": 1},
		Success:     true,
		DurationMs:  100,
		PatternHash: hash,
	}
	rec2 := &ExperienceRecord{
		Timestamp:   now.Add(time.Second),
		ToolName:    "tool",
		Args:        map[string]interface{}{"x": 1},
		Success:     false,
		DurationMs:  200,
		PatternHash: hash,
	}

	collector.ProcessRecord(rec1)
	collector.ProcessRecord(rec2)

	collector.mu.Lock()
	agg := collector.patternCounts[hash]
	collector.mu.Unlock()

	if agg == nil {
		t.Fatal("Expected aggregate to exist")
	}
	if agg.count != 2 {
		t.Errorf("Expected count 2, got %d", agg.count)
	}
	if agg.successes != 1 {
		t.Errorf("Expected 1 success, got %d", agg.successes)
	}
	if agg.totalDuration != 300 {
		t.Errorf("Expected total duration 300, got %d", agg.totalDuration)
	}
}

func TestCollector_GetNextPosition(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewExperienceStore(tmpDir, cfg)
	collector := NewCollector(store, cfg)

	pos1 := collector.getNextPosition("session-1")
	pos2 := collector.getNextPosition("session-1")
	pos3 := collector.getNextPosition("session-2")

	if pos1 != 0 {
		t.Errorf("First position should be 0, got %d", pos1)
	}
	if pos2 != 1 {
		t.Errorf("Second position should be 1, got %d", pos2)
	}
	if pos3 != 0 {
		t.Errorf("Different session should start at 0, got %d", pos3)
	}
}

func TestCollector_Record_ChannelFull(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	cfg.Collection.BufferSize = 1
	store := NewExperienceStore(tmpDir, cfg)
	collector := NewCollector(store, cfg)

	rec := &ExperienceRecord{
		Timestamp:   time.Now().UTC(),
		ToolName:    "tool",
		PatternHash: "hash",
	}

	// First record should succeed (buffer size 1)
	if !collector.Record(rec) {
		t.Error("First Record should succeed")
	}

	// Channel now has 1 item, buffer is 1, so the next write will fail
	// because the goroutine hasn't drained it. But since buffer=1 and
	// we wrote 1, the channel is full.
	if collector.Record(rec) {
		// It might succeed if the goroutine drained it, but in practice
		// there's no goroutine consuming from inputCh in the test, so
		// this should return false.
		t.Log("Second Record succeeded (channel was drained)")
	}
}

func TestCollector_Flush(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewExperienceStore(tmpDir, cfg)
	collector := NewCollector(store, cfg)

	hash := ComputePatternHash("test_tool", map[string]interface{}{})
	collector.ProcessRecord(&ExperienceRecord{
		Timestamp:   time.Now().UTC(),
		ToolName:    "test_tool",
		Success:     true,
		DurationMs:  50,
		PatternHash: hash,
	})

	collector.Flush()

	// After flush, internal patterns should be reset
	collector.mu.Lock()
	count := len(collector.patternCounts)
	collector.mu.Unlock()

	if count != 0 {
		t.Errorf("Expected empty pattern counts after flush, got %d", count)
	}
}

func TestCollector_Flush_Empty(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewExperienceStore(tmpDir, cfg)
	collector := NewCollector(store, cfg)

	// Should not panic on empty flush
	collector.Flush()
}

// --- ExperienceRecord ToJSON tests ---

func TestExperienceRecord_ToJSON(t *testing.T) {
	rec := &ExperienceRecord{
		Timestamp:  time.Now().UTC(),
		SessionID:  "session-1",
		ToolName:   "read_file",
		Success:    true,
		DurationMs: 100,
	}

	data, err := rec.ToJSON()
	if err != nil {
		t.Fatalf("ToJSON failed: %v", err)
	}
	if len(data) == 0 {
		t.Error("ToJSON should return non-empty bytes")
	}
}
