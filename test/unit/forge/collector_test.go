package forge_test

import (
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/forge"
)

// === Collector Tests ===

func TestComputePatternHash(t *testing.T) {
	args1 := map[string]interface{}{"path": "/tmp/a", "content": "hello"}
	args2 := map[string]interface{}{"content": "hello", "path": "/tmp/a"} // different order, same keys

	hash1 := forge.ComputePatternHash("edit_file", args1)
	hash2 := forge.ComputePatternHash("edit_file", args2)

	// Same tool + same arg keys should produce same hash regardless of map order
	if hash1 != hash2 {
		t.Errorf("Expected same hash for same keys in different order: %s vs %s", hash1, hash2)
	}
}

func TestComputePatternHashDifferentTool(t *testing.T) {
	args := map[string]interface{}{"path": "/tmp/a"}
	hash1 := forge.ComputePatternHash("read_file", args)
	hash2 := forge.ComputePatternHash("edit_file", args)

	if hash1 == hash2 {
		t.Error("Different tools should produce different hashes")
	}
}

func TestComputePatternHashDifferentArgs(t *testing.T) {
	args1 := map[string]interface{}{"path": "/tmp/a"}
	args2 := map[string]interface{}{"path": "/tmp/a", "content": "hello"}

	hash1 := forge.ComputePatternHash("edit_file", args1)
	hash2 := forge.ComputePatternHash("edit_file", args2)

	if hash1 == hash2 {
		t.Error("Different arg sets should produce different hashes")
	}
}

func TestComputePatternHashFormat(t *testing.T) {
	hash := forge.ComputePatternHash("test", map[string]interface{}{})
	if len(hash) < 10 {
		t.Errorf("Hash too short: %s", hash)
	}
	if hash[:7] != "sha256:" {
		t.Errorf("Hash should start with 'sha256:', got: %s", hash)
	}
}

func TestSanitizeArgs(t *testing.T) {
	args := map[string]interface{}{
		"path":      "/tmp/file.txt",
		"api_key":   "secret123",
		"token":     "tok_abc",
		"content":   "normal content",
		"password":  "mypass",
		"my_secret": "hidden",
	}

	fields := []string{"api_key", "token", "password", "secret"}
	cleaned := forge.SanitizeArgs(args, fields)

	if cleaned["api_key"] != "[REDACTED]" {
		t.Errorf("api_key should be redacted, got: %v", cleaned["api_key"])
	}
	if cleaned["token"] != "[REDACTED]" {
		t.Errorf("token should be redacted, got: %v", cleaned["token"])
	}
	if cleaned["password"] != "[REDACTED]" {
		t.Errorf("password should be redacted, got: %v", cleaned["password"])
	}
	if cleaned["my_secret"] != "[REDACTED]" {
		t.Errorf("my_secret should be redacted, got: %v", cleaned["my_secret"])
	}
	if cleaned["path"] != "/tmp/file.txt" {
		t.Errorf("path should be preserved, got: %v", cleaned["path"])
	}
	if cleaned["content"] != "normal content" {
		t.Errorf("content should be preserved, got: %v", cleaned["content"])
	}
}

func TestSanitizeArgsEmptyFields(t *testing.T) {
	args := map[string]interface{}{
		"api_key": "secret123",
	}
	cleaned := forge.SanitizeArgs(args, nil)

	if cleaned["api_key"] != "secret123" {
		t.Error("With no sanitize fields, args should not be modified")
	}
}

func TestSanitizeArgsCaseInsensitive(t *testing.T) {
	args := map[string]interface{}{
		"API_KEY": "secret",
		"MyToken": "tok",
	}

	cleaned := forge.SanitizeArgs(args, []string{"api_key", "token"})

	if cleaned["API_KEY"] != "[REDACTED]" {
		t.Error("API_KEY should be redacted (case insensitive match)")
	}
	if cleaned["MyToken"] != "[REDACTED]" {
		t.Error("MyToken should be redacted (contains 'token')")
	}
}

func TestNewCollector(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	store := forge.NewExperienceStore(tmpDir, cfg)
	collector := forge.NewCollector(store, cfg)

	if collector == nil {
		t.Fatal("NewCollector returned nil")
	}
}

func TestCollectorRecord(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	cfg.Collection.BufferSize = 10
	store := forge.NewExperienceStore(tmpDir, cfg)
	collector := forge.NewCollector(store, cfg)

	rec := &forge.ExperienceRecord{
		ToolName:    "read_file",
		Args:        map[string]interface{}{"path": "/tmp/test"},
		Success:     true,
		DurationMs:  50,
		PatternHash: "sha256:test",
	}

	ok := collector.Record(rec)
	if !ok {
		t.Error("Record should succeed when channel is not full")
	}
}

func TestCollectorBackPressure(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	cfg.Collection.BufferSize = 2
	store := forge.NewExperienceStore(tmpDir, cfg)
	collector := forge.NewCollector(store, cfg)

	for i := 0; i < 2; i++ {
		ok := collector.Record(&forge.ExperienceRecord{
			ToolName:    "test",
			PatternHash: "sha256:test",
		})
		if !ok {
			t.Errorf("Record %d should succeed", i)
		}
	}

	// Third record - may be dropped due to back-pressure
	_ = collector.Record(&forge.ExperienceRecord{
		ToolName:    "overflow",
		PatternHash: "sha256:overflow",
	})
}

func TestCollectorFlushEmpty(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	store := forge.NewExperienceStore(tmpDir, cfg)
	collector := forge.NewCollector(store, cfg)

	// Flush with no aggregated data should not panic
	collector.Flush()
}

func TestCollectorProcessRecordAndFlush(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	store := forge.NewExperienceStore(tmpDir, cfg)
	collector := forge.NewCollector(store, cfg)

	rec1 := &forge.ExperienceRecord{
		ToolName:    "read_file",
		PatternHash: "sha256:abc",
		Success:     true,
		DurationMs:  50,
		Timestamp:   time.Now().UTC(),
	}
	rec2 := &forge.ExperienceRecord{
		ToolName:    "read_file",
		PatternHash: "sha256:abc",
		Success:     false,
		DurationMs:  100,
		Timestamp:   time.Now().UTC(),
	}

	// Process two records with same hash (dedup)
	collector.ProcessRecord(rec1)
	collector.ProcessRecord(rec2)

	// Verify deduplication by flushing and reading
	collector.Flush()

	records, err := store.ReadAggregated(time.Time{})
	if err != nil {
		t.Fatalf("ReadAggregated failed: %v", err)
	}
	if len(records) != 1 {
		t.Fatalf("Expected 1 aggregated record, got %d", len(records))
	}
	if records[0].Count != 2 {
		t.Errorf("Expected count 2, got %d", records[0].Count)
	}
	if records[0].SuccessRate != 0.5 {
		t.Errorf("Expected success rate 0.5, got %f", records[0].SuccessRate)
	}
}
