// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package config

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestLoadScannerConfig_FileExists(t *testing.T) {
	dir := t.TempDir()
	cfgPath := filepath.Join(dir, "config.scanner.json")

	data := `{"enabled":["clamav"],"engines":{"clamav":{"address":"127.0.0.1:3310"}}}`
	if err := os.WriteFile(cfgPath, []byte(data), 0644); err != nil {
		t.Fatal(err)
	}

	cfg, err := LoadScannerConfig(cfgPath)
	if err != nil {
		t.Fatalf("LoadScannerConfig() error: %v", err)
	}
	if len(cfg.Enabled) != 1 || cfg.Enabled[0] != "clamav" {
		t.Errorf("Enabled = %v, want [clamav]", cfg.Enabled)
	}
	if _, ok := cfg.Engines["clamav"]; !ok {
		t.Error("Expected clamav engine config")
	}
}

func TestLoadScannerConfig_FileNotExists_Embedded(t *testing.T) {
	// Set embedded default
	embeddedDefaults.mu.Lock()
	embeddedDefaults.scanner = []byte(`{"enabled":[],"engines":{"clamav":{"address":"127.0.0.1:3310"}}}`)
	embeddedDefaults.mu.Unlock()
	defer func() {
		embeddedDefaults.mu.Lock()
		embeddedDefaults.scanner = nil
		embeddedDefaults.mu.Unlock()
	}()

	cfg, err := LoadScannerConfig("/nonexistent/path/config.scanner.json")
	if err != nil {
		t.Fatalf("LoadScannerConfig() error: %v", err)
	}
	if cfg == nil {
		t.Fatal("Config should not be nil")
	}
	if _, ok := cfg.Engines["clamav"]; !ok {
		t.Error("Expected clamav engine from embedded default")
	}
}

func TestLoadScannerConfig_FileNotExists_Fallback(t *testing.T) {
	// No embedded default set
	embeddedDefaults.mu.Lock()
	embeddedDefaults.scanner = nil
	embeddedDefaults.mu.Unlock()

	cfg, err := LoadScannerConfig("/nonexistent/path/config.scanner.json")
	if err != nil {
		t.Fatalf("LoadScannerConfig() error: %v", err)
	}
	if cfg == nil {
		t.Fatal("Config should not be nil")
	}
	if cfg.Enabled == nil {
		t.Error("Enabled should be empty slice, not nil")
	}
	if cfg.Engines == nil {
		t.Error("Engines should be empty map, not nil")
	}
}

func TestSaveAndLoadScannerConfig(t *testing.T) {
	dir := t.TempDir()
	cfgPath := filepath.Join(dir, "config.scanner.json")

	original := &ScannerFullConfig{
		Enabled: []string{"clamav"},
		Engines: map[string]json.RawMessage{
			"clamav": json.RawMessage(`{"address":"127.0.0.1:3310","scan_on_write":true}`),
		},
	}

	if err := SaveScannerConfig(cfgPath, original); err != nil {
		t.Fatalf("SaveScannerConfig() error: %v", err)
	}

	loaded, err := LoadScannerConfig(cfgPath)
	if err != nil {
		t.Fatalf("LoadScannerConfig() error: %v", err)
	}

	if len(loaded.Enabled) != 1 || loaded.Enabled[0] != "clamav" {
		t.Errorf("Enabled = %v, want [clamav]", loaded.Enabled)
	}
	if _, ok := loaded.Engines["clamav"]; !ok {
		t.Error("Expected clamav engine config")
	}
}

func TestClamAVEngineConfig_Parse(t *testing.T) {
	raw := `{
		"url": "https://example.com/clamav.zip",
		"clamav_path": "/opt/clamav",
		"address": "127.0.0.1:3310",
		"scan_on_write": true,
		"scan_on_download": false,
		"scan_on_exec": true,
		"scan_extensions": [".exe", ".dll"],
		"skip_extensions": [".txt", ".md"],
		"max_file_size": 104857600,
		"update_interval": "12h"
	}`

	var cfg ClamAVEngineConfig
	if err := json.Unmarshal([]byte(raw), &cfg); err != nil {
		t.Fatalf("Unmarshal() error: %v", err)
	}

	if cfg.URL != "https://example.com/clamav.zip" {
		t.Errorf("URL = %q, want %q", cfg.URL, "https://example.com/clamav.zip")
	}
	if cfg.ClamAVPath != "/opt/clamav" {
		t.Errorf("ClamAVPath = %q", cfg.ClamAVPath)
	}
	if !cfg.ScanOnWrite {
		t.Error("ScanOnWrite should be true")
	}
	if cfg.ScanOnDownload {
		t.Error("ScanOnDownload should be false")
	}
	if len(cfg.ScanExtensions) != 2 {
		t.Errorf("ScanExtensions len = %d, want 2", len(cfg.ScanExtensions))
	}
	if cfg.MaxFileSize != 104857600 {
		t.Errorf("MaxFileSize = %d, want 104857600", cfg.MaxFileSize)
	}
	if cfg.UpdateInterval != "12h" {
		t.Errorf("UpdateInterval = %q, want 12h", cfg.UpdateInterval)
	}
}

func TestSaveScannerConfig_CreatesDirectory(t *testing.T) {
	dir := t.TempDir()
	cfgPath := filepath.Join(dir, "subdir", "config.scanner.json")

	cfg := &ScannerFullConfig{
		Enabled: []string{},
		Engines: map[string]json.RawMessage{},
	}

	if err := SaveScannerConfig(cfgPath, cfg); err != nil {
		t.Fatalf("SaveScannerConfig() error: %v", err)
	}

	if _, err := os.Stat(cfgPath); os.IsNotExist(err) {
		t.Error("Config file should exist")
	}
}

func TestEngineState_JSONRoundTrip(t *testing.T) {
	state := EngineState{
		InstallStatus:      "installed",
		InstallError:       "",
		LastInstallAttempt: "2026-04-15T10:00:00Z",
		DBStatus:           "ready",
		LastDBUpdate:       "2026-04-15T10:00:00Z",
	}

	data, err := json.Marshal(state)
	if err != nil {
		t.Fatalf("Marshal() error: %v", err)
	}

	var decoded EngineState
	if err := json.Unmarshal(data, &decoded); err != nil {
		t.Fatalf("Unmarshal() error: %v", err)
	}

	if decoded.InstallStatus != "installed" {
		t.Errorf("InstallStatus = %q, want %q", decoded.InstallStatus, "installed")
	}
	if decoded.DBStatus != "ready" {
		t.Errorf("DBStatus = %q, want %q", decoded.DBStatus, "ready")
	}
	if decoded.LastInstallAttempt != "2026-04-15T10:00:00Z" {
		t.Errorf("LastInstallAttempt = %q", decoded.LastInstallAttempt)
	}
}

func TestEngineState_Omitempty(t *testing.T) {
	state := EngineState{}

	data, err := json.Marshal(state)
	if err != nil {
		t.Fatalf("Marshal() error: %v", err)
	}

	s := strings.TrimSpace(string(data))
	if s != "{}" {
		t.Errorf("Empty state should serialize to {}, got %q", s)
	}
}

func TestEngineState_InClamAVConfig(t *testing.T) {
	raw := `{
		"address": "127.0.0.1:3310",
		"state": {
			"install_status": "installed",
			"db_status": "ready"
		}
	}`

	var cfg ClamAVEngineConfig
	if err := json.Unmarshal([]byte(raw), &cfg); err != nil {
		t.Fatalf("Unmarshal() error: %v", err)
	}

	if cfg.State.InstallStatus != "installed" {
		t.Errorf("State.InstallStatus = %q, want %q", cfg.State.InstallStatus, "installed")
	}
	if cfg.State.DBStatus != "ready" {
		t.Errorf("State.DBStatus = %q, want %q", cfg.State.DBStatus, "ready")
	}
	if cfg.Address != "127.0.0.1:3310" {
		t.Errorf("Address = %q", cfg.Address)
	}
}
