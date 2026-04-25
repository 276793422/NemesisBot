// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package scanner

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"runtime"
	"testing"

	"github.com/276793422/NemesisBot/module/config"
)

// --- ScanChain additional tests ---

func TestScanChain_Stop(t *testing.T) {
	chain := NewScanChain()
	// Stop on empty chain should not panic
	chain.Stop()
}

func TestScanChain_Stop_WithEngines(t *testing.T) {
	chain := NewScanChain()
	chain.engines = []VirusScanner{
		&mockEngine{name: "engine-a", ready: true, clean: true},
	}
	chain.Stop()
}

func TestScanChain_LoadFromConfig_EmptyEnabled(t *testing.T) {
	cfg := &config.ScannerFullConfig{
		Enabled: []string{},
		Engines: map[string]json.RawMessage{},
	}

	chain := NewScanChain()
	if err := chain.LoadFromConfig(cfg); err != nil {
		t.Fatalf("LoadFromConfig() error: %v", err)
	}

	engines := chain.Engines()
	if len(engines) != 0 {
		t.Errorf("Expected 0 engines, got %d", len(engines))
	}
}

func TestScanChain_LoadFromConfig_MissingConfig(t *testing.T) {
	cfg := &config.ScannerFullConfig{
		Enabled: []string{"nonexistent_engine"},
		Engines: map[string]json.RawMessage{},
	}

	chain := NewScanChain()
	if err := chain.LoadFromConfig(cfg); err != nil {
		t.Fatalf("LoadFromConfig() should not error on missing config: %v", err)
	}

	engines := chain.Engines()
	if len(engines) != 0 {
		t.Errorf("Expected 0 engines for missing config, got %d", len(engines))
	}
}

func TestScanChain_ScanToolInvocation_NoPath(t *testing.T) {
	chain := NewScanChain()
	chain.engines = []VirusScanner{
		&mockEngine{name: "clamav", ready: true, clean: true},
	}

	// Tool invocation without path should still work
	allowed, err := chain.ScanToolInvocation(context.Background(), "exec", map[string]interface{}{
		"command": "echo hello",
	})
	if !allowed {
		t.Error("Should be allowed even without path")
	}
	if err != nil {
		t.Errorf("Unexpected error: %v", err)
	}
}

func TestScanChain_ScanToolInvocation_ScanByExtension(t *testing.T) {
	chain := NewScanChain()
	chain.configs = map[string]json.RawMessage{
		"clamav": []byte(`{"scan_extensions":[".exe",".dll"]}`),
	}
	chain.engines = []VirusScanner{
		&mockEngine{name: "clamav", ready: true, clean: true},
	}

	// .exe file should be scanned
	allowed, err := chain.ScanToolInvocation(context.Background(), "write_file", map[string]interface{}{
		"path":    "/test/file.exe",
		"content": "test",
	})
	if !allowed {
		t.Error("Clean .exe should be allowed")
	}
	if err != nil {
		t.Errorf("Unexpected error: %v", err)
	}
}

func TestScanChain_ScanToolInvocation_NotInScanExtensions(t *testing.T) {
	chain := NewScanChain()
	chain.configs = map[string]json.RawMessage{
		"clamav": []byte(`{"scan_extensions":[".exe",".dll"]}`),
	}
	chain.engines = []VirusScanner{
		&mockEngine{name: "clamav", ready: true, clean: false, virus: "Bad"},
	}

	// .txt file should be skipped when scan_extensions is set
	allowed, _ := chain.ScanToolInvocation(context.Background(), "write_file", map[string]interface{}{
		"path":    "/test/file.txt",
		"content": "test",
	})
	if !allowed {
		t.Error(".txt should be skipped when scan_extensions only has .exe/.dll")
	}
}

func TestScanChain_Start_Error(t *testing.T) {
	chain := NewScanChain()
	chain.engines = []VirusScanner{
		&mockEngine{name: "engine-a", ready: false, clean: true},
	}

	// Start should not fail even if engines are not ready
	err := chain.Start(context.Background())
	if err != nil {
		t.Errorf("Start should not fail: %v", err)
	}
}

// --- Engine state constants ---

func TestInstallStatusConstants(t *testing.T) {
	if InstallStatusPending != "pending" {
		t.Errorf("InstallStatusPending = %q, want 'pending'", InstallStatusPending)
	}
	if InstallStatusInstalled != "installed" {
		t.Errorf("InstallStatusInstalled = %q, want 'installed'", InstallStatusInstalled)
	}
	if InstallStatusFailed != "failed" {
		t.Errorf("InstallStatusFailed = %q, want 'failed'", InstallStatusFailed)
	}
}

func TestDBStatusConstants(t *testing.T) {
	if DBStatusMissing != "missing" {
		t.Errorf("DBStatusMissing = %q, want 'missing'", DBStatusMissing)
	}
	if DBStatusReady != "ready" {
		t.Errorf("DBStatusReady = %q, want 'ready'", DBStatusReady)
	}
	if DBStatusStale != "stale" {
		t.Errorf("DBStatusStale = %q, want 'stale'", DBStatusStale)
	}
}

// --- ScanResult.Clean() test ---

func TestScanResult_Clean(t *testing.T) {
	tests := []struct {
		infected bool
		clean    bool
	}{
		{false, true},
		{true, false},
	}
	for _, tt := range tests {
		r := &ScanResult{Infected: tt.infected}
		if r.Clean() != tt.clean {
			t.Errorf("ScanResult{Infected=%v}.Clean() = %v, want %v", tt.infected, r.Clean(), tt.clean)
		}
	}
}

// --- ScanChainResult fields test ---

func TestScanChainResult_Fields(t *testing.T) {
	r := ScanChainResult{
		Clean:   true,
		Blocked: false,
		Engine:  "test",
		Virus:   "",
		Path:    "/test",
	}
	if r.Clean != true {
		t.Error("Clean should be true")
	}
	if r.Blocked != false {
		t.Error("Blocked should be false")
	}
	if r.Engine != "test" {
		t.Error("Engine should be 'test'")
	}
}

// --- EngineInfo fields test ---

func TestEngineInfo_Fields(t *testing.T) {
	info := EngineInfo{
		Name:      "clamav",
		Version:   "1.0.0",
		Address:   "127.0.0.1:3310",
		Ready:     true,
		StartTime: "2026-01-01T00:00:00Z",
	}
	if info.Name != "clamav" {
		t.Error("Name mismatch")
	}
	if !info.Ready {
		t.Error("Ready should be true")
	}
}

// --- DatabaseStatus fields test ---

func TestDatabaseStatus_Fields(t *testing.T) {
	status := DatabaseStatus{
		Available: true,
		Version:   "daily.26000",
		Path:      "/var/lib/clamav",
		SizeBytes: 1024,
	}
	if !status.Available {
		t.Error("Available should be true")
	}
	if status.SizeBytes != 1024 {
		t.Error("SizeBytes mismatch")
	}
}

// --- ClamAV engine config edge cases ---

func TestClamAVEngine_Setup_DefaultConfig(t *testing.T) {
	engine, err := NewClamAVEngine([]byte(`{}`))
	if err != nil {
		t.Fatalf("NewClamAVEngine({}) error: %v", err)
	}
	if engine.config == nil {
		t.Error("Config should not be nil")
	}
}

func TestClamAVEngine_Setup_WithScanOnWrite(t *testing.T) {
	raw := []byte(`{
		"scan_on_write": true,
		"scan_on_download": true,
		"scan_on_exec": false
	}`)
	engine, err := NewClamAVEngine(raw)
	if err != nil {
		t.Fatalf("NewClamAVEngine() error: %v", err)
	}
	if !engine.config.ScanOnWrite {
		t.Error("ScanOnWrite should be true")
	}
	if !engine.config.ScanOnDownload {
		t.Error("ScanOnDownload should be true")
	}
	if engine.config.ScanOnExec {
		t.Error("ScanOnExec should be false")
	}
}

func TestClamAVEngine_Setup_WithMaxFileSize(t *testing.T) {
	raw := []byte(`{"max_file_size": 52428800}`)
	engine, err := NewClamAVEngine(raw)
	if err != nil {
		t.Fatalf("NewClamAVEngine() error: %v", err)
	}
	if engine.config.MaxFileSize != 52428800 {
		t.Errorf("MaxFileSize = %d, want 52428800", engine.config.MaxFileSize)
	}
}

func TestClamAVEngine_GetExtensionRules_NoConfig(t *testing.T) {
	engine, _ := NewClamAVEngine(nil)
	rules := engine.GetExtensionRules()
	if len(rules.ScanExtensions) != 0 {
		t.Errorf("Expected empty ScanExtensions, got %d", len(rules.ScanExtensions))
	}
	if len(rules.SkipExtensions) != 0 {
		t.Errorf("Expected empty SkipExtensions, got %d", len(rules.SkipExtensions))
	}
}

func TestClamAVEngine_Validate_WithExe(t *testing.T) {
	engine, _ := NewClamAVEngine(nil)
	tmpDir := t.TempDir()

	// Create the expected executable
	exeName := "clamd"
	if runtime.GOOS == "windows" {
		exeName = "clamd.exe"
	}
	if err := os.WriteFile(filepath.Join(tmpDir, exeName), []byte("binary"), 0644); err != nil {
		t.Fatal(err)
	}

	err := engine.Validate(tmpDir)
	if err != nil {
		t.Errorf("Validate() should succeed with executable present: %v", err)
	}
}

func TestClamAVEngine_GetClamAVPath_Default(t *testing.T) {
	engine, _ := NewClamAVEngine(nil)
	path := engine.GetClamAVPath()
	if path != "" {
		t.Errorf("Expected empty path for nil config, got %q", path)
	}
}

func TestClamAVEngine_GetInfo_WithContext(t *testing.T) {
	engine, _ := NewClamAVEngine([]byte(`{"address":"127.0.0.1:3310"}`))
	ctx := context.Background()
	info, err := engine.GetInfo(ctx)
	if err != nil {
		t.Fatalf("GetInfo() error: %v", err)
	}
	if info.Name != "clamav" {
		t.Errorf("Name = %q, want 'clamav'", info.Name)
	}
	if info.Address != "127.0.0.1:3310" {
		t.Errorf("Address = %q, want '127.0.0.1:3310'", info.Address)
	}
}

// --- Registry edge cases ---

func TestRegistry_CreateEngine_NilConfig(t *testing.T) {
	engine, err := CreateEngine("clamav", nil)
	if err != nil {
		t.Fatalf("CreateEngine(clamav, nil) error: %v", err)
	}
	if engine == nil {
		t.Error("Engine should not be nil")
	}
}

// --- ExtensionRules edge cases ---

func TestShouldScanFile_BothListsSet(t *testing.T) {
	rules := ExtensionRules{
		ScanExtensions: []string{".exe"},
		SkipExtensions: []string{".exe"},
	}
	// ScanExtensions takes priority
	if !ShouldScanFile("C:\\test\\file.exe", rules) {
		t.Error("ScanExtensions should take priority for .exe")
	}
	if ShouldScanFile("C:\\test\\file.txt", rules) {
		t.Error(".txt should not be in ScanExtensions whitelist")
	}
}

func TestShouldScanFile_MultipleExtensions(t *testing.T) {
	rules := ExtensionRules{
		ScanExtensions: []string{".exe", ".dll", ".sys", ".bat"},
	}

	tests := []struct {
		path    string
		want    bool
	}{
		{"C:\\test\\program.exe", true},
		{"C:\\test\\library.dll", true},
		{"C:\\test\\driver.sys", true},
		{"C:\\test\\script.bat", true},
		{"C:\\test\\readme.txt", false},
		{"C:\\test\\image.png", false},
	}

	for _, tt := range tests {
		if got := ShouldScanFile(tt.path, rules); got != tt.want {
			t.Errorf("ShouldScanFile(%q) = %v, want %v", tt.path, got, tt.want)
		}
	}
}

func TestShouldScanFile_SkipMultipleExtensions(t *testing.T) {
	rules := ExtensionRules{
		SkipExtensions: []string{".txt", ".md", ".json", ".log"},
	}

	tests := []struct {
		path    string
		want    bool
	}{
		{"C:\\test\\readme.txt", false},
		{"C:\\test\\doc.md", false},
		{"C:\\test\\config.json", false},
		{"C:\\test\\app.log", false},
		{"C:\\test\\program.exe", true},
		{"C:\\test\\image.png", true},
	}

	for _, tt := range tests {
		if got := ShouldScanFile(tt.path, rules); got != tt.want {
			t.Errorf("ShouldScanFile(%q) = %v, want %v", tt.path, got, tt.want)
		}
	}
}

// --- ScanChain RawConfig edge case ---

func TestScanChain_RawConfig_NotFound(t *testing.T) {
	chain := NewScanChain()
	_, ok := chain.RawConfig("nonexistent")
	if ok {
		t.Error("Should not find nonexistent config")
	}
}

func TestScanChain_GetExtensionRules_NoConfig(t *testing.T) {
	chain := NewScanChain()
	rules := chain.GetExtensionRules()
	if len(rules.ScanExtensions) != 0 || len(rules.SkipExtensions) != 0 {
		t.Error("Empty chain should have empty extension rules")
	}
}

func TestScanChain_GetStats_Empty(t *testing.T) {
	chain := NewScanChain()
	stats := chain.GetStats()
	if len(stats) != 0 {
		t.Errorf("Empty chain should have empty stats, got %d", len(stats))
	}
}

// --- DetectInstallPath edge cases ---

func TestClamAVEngine_DetectInstallPath_NestedDeeply(t *testing.T) {
	engine, _ := NewClamAVEngine(nil)
	dir := t.TempDir()

	// Create deeply nested directory with target executable
	targetExe := "clamd.exe"
	if runtime.GOOS != "windows" {
		targetExe = "clamd"
	}
	nestedDir := filepath.Join(dir, "a", "b", "c", "bin")
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

// --- Import helper ---
var _ = runtime.GOOS
var _ = context.Background
