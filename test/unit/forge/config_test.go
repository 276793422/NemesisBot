package forge_test

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/forge"
)

// === Config Tests ===

func TestDefaultForgeConfig(t *testing.T) {
	cfg := forge.DefaultForgeConfig()

	if !cfg.Collection.Enabled {
		t.Error("Collection should be enabled by default")
	}
	if cfg.Collection.BufferSize != 256 {
		t.Errorf("Expected BufferSize 256, got %d", cfg.Collection.BufferSize)
	}
	if cfg.Collection.MaxExperiencesPerDay != 500 {
		t.Errorf("Expected MaxExperiencesPerDay 500, got %d", cfg.Collection.MaxExperiencesPerDay)
	}
	if cfg.Reflection.Interval.Duration != 6*time.Hour {
		t.Errorf("Expected Reflection.Interval 6h, got %v", cfg.Reflection.Interval.Duration)
	}
	if cfg.Reflection.MinExperiences != 10 {
		t.Errorf("Expected MinExperiences 10, got %d", cfg.Reflection.MinExperiences)
	}
	if cfg.Artifacts.DefaultStatus != "draft" {
		t.Errorf("Expected DefaultStatus 'draft', got %s", cfg.Artifacts.DefaultStatus)
	}
}

func TestSaveAndLoadForgeConfig(t *testing.T) {
	tmpDir := t.TempDir()
	configPath := filepath.Join(tmpDir, "forge.json")

	cfg := forge.DefaultForgeConfig()
	cfg.Collection.BufferSize = 512
	cfg.Reflection.Interval = forge.Duration{Duration: 12 * time.Hour}

	// Save
	if err := forge.SaveForgeConfig(configPath, cfg); err != nil {
		t.Fatalf("SaveForgeConfig failed: %v", err)
	}

	// Verify file exists
	if _, err := os.Stat(configPath); os.IsNotExist(err) {
		t.Fatal("Config file was not created")
	}

	// Load
	loaded, err := forge.LoadForgeConfig(configPath)
	if err != nil {
		t.Fatalf("LoadForgeConfig failed: %v", err)
	}

	if loaded.Collection.BufferSize != 512 {
		t.Errorf("Expected BufferSize 512, got %d", loaded.Collection.BufferSize)
	}
	if loaded.Reflection.Interval.Duration != 12*time.Hour {
		t.Errorf("Expected Reflection.Interval 12h, got %v", loaded.Reflection.Interval.Duration)
	}
}

func TestLoadForgeConfigNonexistent(t *testing.T) {
	_, err := forge.LoadForgeConfig("/nonexistent/path/forge.json")
	if err == nil {
		t.Error("Expected error for nonexistent file")
	}
}

func TestDurationJSONMarshal(t *testing.T) {
	d := forge.Duration{Duration: 30 * time.Second}
	data, err := json.Marshal(d)
	if err != nil {
		t.Fatalf("Marshal failed: %v", err)
	}
	if string(data) != `"30s"` {
		t.Errorf("Expected \"30s\", got %s", string(data))
	}
}

func TestDurationJSONUnmarshal(t *testing.T) {
	var d forge.Duration
	if err := json.Unmarshal([]byte(`"5m30s"`), &d); err != nil {
		t.Fatalf("Unmarshal failed: %v", err)
	}
	if d.Duration != 5*time.Minute+30*time.Second {
		t.Errorf("Expected 5m30s, got %v", d.Duration)
	}
}

func TestDurationJSONUnmarshalInvalid(t *testing.T) {
	var d forge.Duration
	err := json.Unmarshal([]byte(`"invalid"`), &d)
	if err == nil {
		t.Error("Expected error for invalid duration")
	}
}

func TestSanitizeFields(t *testing.T) {
	cfg := forge.DefaultForgeConfig()
	fields := cfg.Collection.SanitizeFields

	expectedFields := []string{"api_key", "token", "password", "secret", "credential", "key"}
	for _, ef := range expectedFields {
		found := false
		for _, f := range fields {
			if f == ef {
				found = true
				break
			}
		}
		if !found {
			t.Errorf("Expected sanitize field '%s' not found", ef)
		}
	}
}

// === ValidationConfig Tests ===

func TestDefaultValidationConfig(t *testing.T) {
	cfg := forge.DefaultForgeConfig()

	if !cfg.Validation.AutoValidate {
		t.Error("AutoValidate should be true by default")
	}
	if cfg.Validation.MinQualityScore != 60 {
		t.Errorf("Expected MinQualityScore 60, got %d", cfg.Validation.MinQualityScore)
	}
	if cfg.Validation.LLMMaxTokens != 2000 {
		t.Errorf("Expected LLMMaxTokens 2000, got %d", cfg.Validation.LLMMaxTokens)
	}
	if cfg.Validation.Timeout.Duration != 60*time.Second {
		t.Errorf("Expected Timeout 60s, got %v", cfg.Validation.Timeout.Duration)
	}
}

func TestValidationConfigSerialization(t *testing.T) {
	tmpDir := t.TempDir()
	configPath := filepath.Join(tmpDir, "forge.json")

	cfg := forge.DefaultForgeConfig()
	cfg.Validation.MinQualityScore = 80
	cfg.Validation.AutoValidate = false

	if err := forge.SaveForgeConfig(configPath, cfg); err != nil {
		t.Fatalf("Save failed: %v", err)
	}

	loaded, err := forge.LoadForgeConfig(configPath)
	if err != nil {
		t.Fatalf("Load failed: %v", err)
	}

	if loaded.Validation.MinQualityScore != 80 {
		t.Errorf("Expected MinQualityScore 80, got %d", loaded.Validation.MinQualityScore)
	}
	if loaded.Validation.AutoValidate {
		t.Error("AutoValidate should be false")
	}
}
