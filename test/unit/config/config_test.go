// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package config_test

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"

	. "github.com/276793422/NemesisBot/module/config"
)

func TestDefaultConfig(t *testing.T) {
	cfg := DefaultConfig()

	if cfg == nil {
		t.Fatal("DefaultConfig() returned nil")
	}

	// Verify workspace is set
	if cfg.Agents.Defaults.Workspace == "" {
		t.Error("Workspace should not be empty")
	}

	// Verify LLM is set
	if cfg.Agents.Defaults.LLM == "" {
		t.Error("LLM should not be empty")
	}

	// Verify default LLM format
	if cfg.Agents.Defaults.LLM != "zhipu/glm-4.7-flash" {
		t.Errorf("Expected default LLM zhipu/glm-4.7-flash, got %s", cfg.Agents.Defaults.LLM)
	}
}

func TestConfig_SaveAndLoad(t *testing.T) {
	tmpDir := t.TempDir()
	configPath := filepath.Join(tmpDir, "config.json")

	// Create default config
	cfg1 := DefaultConfig()
	cfg1.Agents.Defaults.LLM = "openai/gpt-4"

	// Save config
	err := SaveConfig(configPath, cfg1)
	if err != nil {
		t.Fatalf("SaveConfig failed: %v", err)
	}

	// Verify file was created
	if _, err := os.Stat(configPath); os.IsNotExist(err) {
		t.Fatal("Config file was not created")
	}

	// Load config
	cfg2, err := LoadConfig(configPath)
	if err != nil {
		t.Fatalf("LoadConfig failed: %v", err)
	}

	// Verify loaded config matches
	if cfg2.Agents.Defaults.LLM != "openai/gpt-4" {
		t.Errorf("Expected LLM openai/gpt-4, got %s", cfg2.Agents.Defaults.LLM)
	}
}

func TestFlexibleStringSlice_Unmarshal(t *testing.T) {
	data := `["item1", "item2", 123]`
	var slice FlexibleStringSlice
	err := json.Unmarshal([]byte(data), &slice)
	if err != nil {
		t.Fatalf("Unmarshal failed: %v", err)
	}

	if len(slice) != 3 {
		t.Errorf("Expected 3 items, got %d", len(slice))
	}

	if slice[0] != "item1" || slice[1] != "item2" || slice[2] != "123" {
		t.Errorf("Unexpected values: %v", slice)
	}
}
