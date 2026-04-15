// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package scanner

import (
	"os"
	"path/filepath"
	"runtime"
	"testing"
)

func TestClamAVEngine_Name(t *testing.T) {
	engine, err := NewClamAVEngine([]byte(`{"address":"127.0.0.1:3310"}`))
	if err != nil {
		t.Fatalf("NewClamAVEngine() error: %v", err)
	}
	if engine.Name() != "clamav" {
		t.Errorf("Name() = %q, want %q", engine.Name(), "clamav")
	}
}

func TestClamAVEngine_Setup_ValidConfig(t *testing.T) {
	raw := []byte(`{
		"url": "https://example.com/clamav.zip",
		"clamav_path": "/usr/local/bin",
		"address": "127.0.0.1:3310",
		"scan_on_write": true,
		"scan_on_download": false,
		"scan_on_exec": true,
		"max_file_size": 104857600,
		"update_interval": "12h"
	}`)
	engine, err := NewClamAVEngine(raw)
	if err != nil {
		t.Fatalf("NewClamAVEngine() error: %v", err)
	}
	if engine.config.Address != "127.0.0.1:3310" {
		t.Errorf("Address = %q, want %q", engine.config.Address, "127.0.0.1:3310")
	}
	if !engine.config.ScanOnWrite {
		t.Error("ScanOnWrite should be true")
	}
	if engine.config.ScanOnDownload {
		t.Error("ScanOnDownload should be false")
	}
	if engine.config.MaxFileSize != 104857600 {
		t.Errorf("MaxFileSize = %d, want %d", engine.config.MaxFileSize, 104857600)
	}
}

func TestClamAVEngine_Setup_InvalidConfig(t *testing.T) {
	_, err := NewClamAVEngine([]byte(`{invalid json`))
	if err == nil {
		t.Error("Expected error for invalid JSON")
	}
}

func TestClamAVEngine_Setup_EmptyConfig(t *testing.T) {
	engine, err := NewClamAVEngine(nil)
	if err != nil {
		t.Fatalf("NewClamAVEngine(nil) error: %v", err)
	}
	if engine.config == nil {
		t.Error("config should not be nil even with empty input")
	}
}

func TestClamAVEngine_Validate_Missing(t *testing.T) {
	engine, _ := NewClamAVEngine(nil)
	tmpDir := t.TempDir()
	err := engine.Validate(tmpDir)
	if err == nil {
		t.Error("Expected error for missing clamd executable")
	}
}

func TestClamAVEngine_IsReady_NotStarted(t *testing.T) {
	engine, _ := NewClamAVEngine(nil)
	if engine.IsReady() {
		t.Error("IsReady() should be false when not started")
	}
}

func TestClamAVEngine_GetStats_Disabled(t *testing.T) {
	engine, _ := NewClamAVEngine(nil)
	stats := engine.GetStats()
	if stats == nil {
		t.Error("GetStats() should not return nil")
	}
	if started, ok := stats["started"].(bool); !ok || started {
		t.Error("started should be false")
	}
}

func TestClamAVEngine_GetInfo_NotReady(t *testing.T) {
	engine, _ := NewClamAVEngine([]byte(`{"address":"127.0.0.1:3310"}`))
	info, err := engine.GetInfo(nil)
	if err != nil {
		t.Fatalf("GetInfo() error: %v", err)
	}
	if info.Name != "clamav" {
		t.Errorf("Name = %q, want %q", info.Name, "clamav")
	}
	if info.Ready {
		t.Error("Ready should be false")
	}
}

func TestClamAVEngine_ScanFile_NotReady(t *testing.T) {
	engine, _ := NewClamAVEngine(nil)
	result, err := engine.ScanFile(nil, "/some/file.exe")
	if err != nil {
		t.Fatalf("ScanFile() error: %v", err)
	}
	if result.Infected {
		t.Error("Should not report infection when not ready")
	}
	if result.Engine != "clamav" {
		t.Errorf("Engine = %q, want %q", result.Engine, "clamav")
	}
}

func TestClamAVEngine_ScanContent_NotReady(t *testing.T) {
	engine, _ := NewClamAVEngine(nil)
	result, err := engine.ScanContent(nil, []byte("test"))
	if err != nil {
		t.Fatalf("ScanContent() error: %v", err)
	}
	if result.Engine != "clamav" {
		t.Errorf("Engine = %q, want %q", result.Engine, "clamav")
	}
}

func TestClamAVEngine_ScanDirectory_NotReady(t *testing.T) {
	engine, _ := NewClamAVEngine(nil)
	results, err := engine.ScanDirectory(nil, "/some/dir")
	if err != nil {
		t.Fatalf("ScanDirectory() error: %v", err)
	}
	if results != nil {
		t.Error("Should return nil results when not ready")
	}
}

func TestClamAVEngine_GetExtensionRules(t *testing.T) {
	raw := []byte(`{
		"scan_extensions": [".exe", ".dll"],
		"skip_extensions": [".txt", ".md"]
	}`)
	engine, _ := NewClamAVEngine(raw)
	rules := engine.GetExtensionRules()
	if len(rules.ScanExtensions) != 2 {
		t.Errorf("ScanExtensions len = %d, want 2", len(rules.ScanExtensions))
	}
	if len(rules.SkipExtensions) != 2 {
		t.Errorf("SkipExtensions len = %d, want 2", len(rules.SkipExtensions))
	}
}

func TestClamAVEngine_TargetExecutables(t *testing.T) {
	engine, _ := NewClamAVEngine(nil)
	targets := engine.TargetExecutables()
	if len(targets) != 1 {
		t.Fatalf("TargetExecutables() returned %d items, want 1", len(targets))
	}
	if runtime.GOOS == "windows" {
		if targets[0] != "clamd.exe" {
			t.Errorf("TargetExecutables()[0] = %q, want %q", targets[0], "clamd.exe")
		}
	} else {
		if targets[0] != "clamd" {
			t.Errorf("TargetExecutables()[0] = %q, want %q", targets[0], "clamd")
		}
	}
}

func TestClamAVEngine_DetectInstallPath_Found(t *testing.T) {
	engine, _ := NewClamAVEngine(nil)
	dir := t.TempDir()

	// Create nested directory with target executable
	targetExe := "clamd.exe"
	if runtime.GOOS != "windows" {
		targetExe = "clamd"
	}
	nestedDir := filepath.Join(dir, "subdir", "bin")
	if err := os.MkdirAll(nestedDir, 0755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(nestedDir, targetExe), []byte("binary"), 0644); err != nil {
		t.Fatal(err)
	}

	foundPath, err := engine.DetectInstallPath(dir)
	if err != nil {
		t.Fatalf("DetectInstallPath() error: %v", err)
	}
	if foundPath != nestedDir {
		t.Errorf("DetectInstallPath() = %q, want %q", foundPath, nestedDir)
	}
}

func TestClamAVEngine_DetectInstallPath_NotFound(t *testing.T) {
	engine, _ := NewClamAVEngine(nil)
	dir := t.TempDir()

	_, err := engine.DetectInstallPath(dir)
	if err == nil {
		t.Error("DetectInstallPath() should return error when not found")
	}
}

func TestClamAVEngine_DatabaseFileName(t *testing.T) {
	engine, _ := NewClamAVEngine(nil)
	if engine.DatabaseFileName() != "main.cvd" {
		t.Errorf("DatabaseFileName() = %q, want %q", engine.DatabaseFileName(), "main.cvd")
	}
}

func TestClamAVEngine_GetEngineState(t *testing.T) {
	engine, _ := NewClamAVEngine(nil)
	state := engine.GetEngineState()
	if state == nil {
		t.Fatal("GetEngineState() returned nil")
	}

	// Verify read/write through pointer
	state.InstallStatus = InstallStatusInstalled
	if engine.GetEngineState().InstallStatus != InstallStatusInstalled {
		t.Error("State change through pointer not reflected")
	}
}

func TestClamAVEngine_GetClamAVPath(t *testing.T) {
	engine, _ := NewClamAVEngine([]byte(`{"clamav_path":"/opt/clamav"}`))
	if engine.GetClamAVPath() != "/opt/clamav" {
		t.Errorf("GetClamAVPath() = %q, want %q", engine.GetClamAVPath(), "/opt/clamav")
	}
}

func TestClamAVEngine_ImplementsInstallableEngine(t *testing.T) {
	engine, _ := NewClamAVEngine(nil)
	var _ InstallableEngine = engine
}
