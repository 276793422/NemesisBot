package forge

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
	"time"
)

// --- DefaultForgeConfig tests ---

func TestDefaultForgeConfig_Values(t *testing.T) {
	cfg := DefaultForgeConfig()

	if !cfg.Collection.Enabled {
		t.Error("Collection should be enabled by default")
	}
	if cfg.Collection.BufferSize != 256 {
		t.Errorf("Expected BufferSize 256, got %d", cfg.Collection.BufferSize)
	}
	if cfg.Storage.MaxExperienceAgeDays != 90 {
		t.Errorf("Expected MaxExperienceAgeDays 90, got %d", cfg.Storage.MaxExperienceAgeDays)
	}
	if cfg.Reflection.MinExperiences != 10 {
		t.Errorf("Expected MinExperiences 10, got %d", cfg.Reflection.MinExperiences)
	}
	if !cfg.Reflection.UseLLM {
		t.Error("UseLLM should be true by default")
	}
	if !cfg.Trace.Enabled {
		t.Error("Trace should be enabled by default")
	}
	if cfg.Learning.Enabled {
		t.Error("Learning should be disabled by default")
	}
	if cfg.Learning.MinPatternFrequency != 5 {
		t.Errorf("Expected MinPatternFrequency 5, got %d", cfg.Learning.MinPatternFrequency)
	}
	if cfg.Learning.HighConfThreshold != 0.8 {
		t.Errorf("Expected HighConfThreshold 0.8, got %f", cfg.Learning.HighConfThreshold)
	}
}

// --- Duration JSON serialization tests ---

func TestDuration_MarshalJSON(t *testing.T) {
	d := Duration{30 * time.Second}
	data, err := json.Marshal(d)
	if err != nil {
		t.Fatalf("Marshal failed: %v", err)
	}
	if string(data) != `"30s"` {
		t.Errorf("Expected '\"30s\"', got '%s'", string(data))
	}
}

func TestDuration_UnmarshalJSON(t *testing.T) {
	var d Duration
	if err := json.Unmarshal([]byte(`"1h0m0s"`), &d); err != nil {
		t.Fatalf("Unmarshal failed: %v", err)
	}
	if d.Duration != time.Hour {
		t.Errorf("Expected 1h, got %v", d.Duration)
	}
}

func TestDuration_UnmarshalJSON_Invalid(t *testing.T) {
	var d Duration
	if err := json.Unmarshal([]byte(`"invalid"`), &d); err == nil {
		t.Error("Expected error for invalid duration")
	}
}

func TestDuration_UnmarshalJSON_NotString(t *testing.T) {
	var d Duration
	if err := json.Unmarshal([]byte(`123`), &d); err == nil {
		t.Error("Expected error for non-string duration")
	}
}

// --- LoadForgeConfig tests ---

func TestLoadForgeConfig_Nonexistent(t *testing.T) {
	_, err := LoadForgeConfig("/nonexistent/path/forge.json")
	if err == nil {
		t.Error("Should error on nonexistent file")
	}
}

func TestLoadForgeConfig_Valid(t *testing.T) {
	tmpDir := t.TempDir()
	path := filepath.Join(tmpDir, "forge.json")

	// Write a valid config
	config := map[string]interface{}{
		"collection": map[string]interface{}{
			"enabled":  false,
			"buffer_size": 128,
		},
	}
	data, _ := json.Marshal(config)
	os.WriteFile(path, data, 0644)

	cfg, err := LoadForgeConfig(path)
	if err != nil {
		t.Fatalf("LoadForgeConfig failed: %v", err)
	}
	if cfg.Collection.Enabled {
		t.Error("Collection should be disabled from file")
	}
	if cfg.Collection.BufferSize != 128 {
		t.Errorf("Expected BufferSize 128, got %d", cfg.Collection.BufferSize)
	}
	// Other fields should have defaults
	if cfg.Storage.MaxExperienceAgeDays != 90 {
		t.Errorf("Default should be preserved: 90, got %d", cfg.Storage.MaxExperienceAgeDays)
	}
}

func TestLoadForgeConfig_InvalidJSON(t *testing.T) {
	tmpDir := t.TempDir()
	path := filepath.Join(tmpDir, "forge.json")
	os.WriteFile(path, []byte("{invalid json}"), 0644)

	_, err := LoadForgeConfig(path)
	if err == nil {
		t.Error("Should error on invalid JSON")
	}
}

// --- SaveForgeConfig tests ---

func TestSaveForgeConfig(t *testing.T) {
	tmpDir := t.TempDir()
	path := filepath.Join(tmpDir, "forge.json")

	cfg := DefaultForgeConfig()
	cfg.Collection.Enabled = false

	if err := SaveForgeConfig(path, cfg); err != nil {
		t.Fatalf("SaveForgeConfig failed: %v", err)
	}

	// Verify the file exists and can be loaded
	loaded, err := LoadForgeConfig(path)
	if err != nil {
		t.Fatalf("LoadForgeConfig failed: %v", err)
	}
	if loaded.Collection.Enabled {
		t.Error("Saved config should have collection disabled")
	}
}

// --- ForgeConfig round-trip ---

func TestForgeConfig_RoundTrip(t *testing.T) {
	tmpDir := t.TempDir()
	path := filepath.Join(tmpDir, "forge.json")

	original := DefaultForgeConfig()
	original.Collection.FlushInterval = Duration{15 * time.Second}
	original.Reflection.Interval = Duration{12 * time.Hour}
	original.Learning.Enabled = true
	original.Learning.DegradeThreshold = -0.3

	SaveForgeConfig(path, original)
	loaded, err := LoadForgeConfig(path)
	if err != nil {
		t.Fatalf("LoadForgeConfig failed: %v", err)
	}

	if loaded.Collection.FlushInterval.Duration != 15*time.Second {
		t.Errorf("Expected 15s flush interval, got %v", loaded.Collection.FlushInterval.Duration)
	}
	if loaded.Reflection.Interval.Duration != 12*time.Hour {
		t.Errorf("Expected 12h reflection interval, got %v", loaded.Reflection.Interval.Duration)
	}
	if !loaded.Learning.Enabled {
		t.Error("Learning should be enabled")
	}
	if loaded.Learning.DegradeThreshold != -0.3 {
		t.Errorf("Expected -0.3 threshold, got %f", loaded.Learning.DegradeThreshold)
	}
}
