// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package scanner

import (
	"context"
	"encoding/json"
	"testing"

	"github.com/276793422/NemesisBot/module/config"
)

// mockEngine is a test double for VirusScanner.
type mockEngine struct {
	name    string
	ready   bool
	clean   bool
	virus   string
	scanErr error
	stats   map[string]interface{}
}

func (m *mockEngine) Name() string                                            { return m.name }
func (m *mockEngine) GetInfo(ctx context.Context) (*EngineInfo, error)        { return &EngineInfo{Name: m.name, Ready: m.ready}, nil }
func (m *mockEngine) Download(ctx context.Context, dir string) error          { return nil }
func (m *mockEngine) Validate(dir string) error                               { return nil }
func (m *mockEngine) Setup(config json.RawMessage) error                      { return nil }
func (m *mockEngine) Start(ctx context.Context) error                         { return nil }
func (m *mockEngine) Stop() error                                             { return nil }
func (m *mockEngine) IsReady() bool                                           { return m.ready }
func (m *mockEngine) UpdateDatabase(ctx context.Context) error                { return nil }
func (m *mockEngine) GetDatabaseStatus(ctx context.Context) (*DatabaseStatus, error) { return nil, nil }
func (m *mockEngine) GetStats() map[string]interface{}                        { return m.stats }

func (m *mockEngine) ScanFile(ctx context.Context, path string) (*ScanResult, error) {
	if m.scanErr != nil {
		return nil, m.scanErr
	}
	return &ScanResult{Path: path, Infected: !m.clean, Virus: m.virus, Engine: m.name}, nil
}

func (m *mockEngine) ScanContent(ctx context.Context, data []byte) (*ScanResult, error) {
	if m.scanErr != nil {
		return nil, m.scanErr
	}
	return &ScanResult{Infected: !m.clean, Virus: m.virus, Engine: m.name}, nil
}

func (m *mockEngine) ScanDirectory(ctx context.Context, path string) ([]*ScanResult, error) {
	if m.scanErr != nil {
		return nil, m.scanErr
	}
	return []*ScanResult{{Path: path, Infected: !m.clean, Virus: m.virus, Engine: m.name}}, nil
}

func TestScanChain_AllClean(t *testing.T) {
	chain := NewScanChain()
	chain.engines = []VirusScanner{
		&mockEngine{name: "engine-a", ready: true, clean: true},
		&mockEngine{name: "engine-b", ready: true, clean: true},
	}

	result := chain.ScanFile(context.Background(), "/test/file.exe")
	if !result.Clean {
		t.Error("Expected clean result")
	}
	if result.Blocked {
		t.Error("Should not be blocked")
	}
	if len(result.Results) != 2 {
		t.Errorf("Expected 2 results, got %d", len(result.Results))
	}
}

func TestScanChain_FirstEngineBlocks(t *testing.T) {
	chain := NewScanChain()
	chain.engines = []VirusScanner{
		&mockEngine{name: "engine-a", ready: true, clean: false, virus: "EICAR-Test"},
		&mockEngine{name: "engine-b", ready: true, clean: true},
	}

	result := chain.ScanFile(context.Background(), "/test/file.exe")
	if result.Clean {
		t.Error("Expected infected result")
	}
	if !result.Blocked {
		t.Error("Should be blocked")
	}
	if result.Engine != "engine-a" {
		t.Errorf("Engine = %q, want %q", result.Engine, "engine-a")
	}
	if result.Virus != "EICAR-Test" {
		t.Errorf("Virus = %q, want %q", result.Virus, "EICAR-Test")
	}
}

func TestScanChain_SecondEngineBlocks(t *testing.T) {
	chain := NewScanChain()
	chain.engines = []VirusScanner{
		&mockEngine{name: "engine-a", ready: true, clean: true},
		&mockEngine{name: "engine-b", ready: true, clean: false, virus: "Malware.X"},
	}

	result := chain.ScanFile(context.Background(), "/test/file.exe")
	if result.Clean {
		t.Error("Expected infected result")
	}
	if result.Engine != "engine-b" {
		t.Errorf("Engine = %q, want %q", result.Engine, "engine-b")
	}
	if len(result.Results) != 2 {
		t.Errorf("Expected 2 results, got %d", len(result.Results))
	}
}

func TestScanChain_EngineDegraded(t *testing.T) {
	chain := NewScanChain()
	chain.engines = []VirusScanner{
		&mockEngine{name: "engine-a", ready: false, clean: true},
		&mockEngine{name: "engine-b", ready: true, clean: true},
	}

	result := chain.ScanFile(context.Background(), "/test/file.exe")
	if !result.Clean {
		t.Error("Expected clean result (degraded engine should be skipped)")
	}
	if len(result.Results) != 1 {
		t.Errorf("Expected 1 result (degraded engine skipped), got %d", len(result.Results))
	}
}

func TestScanChain_EmptyChain(t *testing.T) {
	chain := NewScanChain()
	result := chain.ScanFile(context.Background(), "/test/file.exe")
	if !result.Clean {
		t.Error("Empty chain should be clean")
	}
	if result.Blocked {
		t.Error("Empty chain should not block")
	}
}

func TestScanChain_Order(t *testing.T) {
	chain := NewScanChain()
	chain.engines = []VirusScanner{
		&mockEngine{name: "first", ready: true, clean: true},
		&mockEngine{name: "second", ready: true, clean: false, virus: "Test.Virus"},
	}

	result := chain.ScanFile(context.Background(), "/test/file.exe")
	if result.Engine != "second" {
		t.Errorf("Engine = %q, want %q", result.Engine, "second")
	}

	// Verify first engine result is present
	if len(result.Results) < 1 || result.Results[0].Engine != "first" {
		t.Error("First engine result should be present")
	}
}

func TestScanChain_ScanContent(t *testing.T) {
	chain := NewScanChain()
	chain.engines = []VirusScanner{
		&mockEngine{name: "engine-a", ready: true, clean: true},
	}

	result := chain.ScanContent(context.Background(), []byte("test data"))
	if !result.Clean {
		t.Error("Expected clean result")
	}
}

func TestScanChain_ScanContent_Blocked(t *testing.T) {
	chain := NewScanChain()
	chain.engines = []VirusScanner{
		&mockEngine{name: "engine-a", ready: true, clean: false, virus: "Bad"},
	}

	result := chain.ScanContent(context.Background(), []byte("test data"))
	if result.Clean {
		t.Error("Expected infected result")
	}
	if !result.Blocked {
		t.Error("Should be blocked")
	}
}

func TestScanChain_ScanDirectory(t *testing.T) {
	chain := NewScanChain()
	chain.engines = []VirusScanner{
		&mockEngine{name: "engine-a", ready: true, clean: true},
	}

	result := chain.ScanDirectory(context.Background(), "/test/dir")
	if !result.Clean {
		t.Error("Expected clean result")
	}
}

func TestScanChain_ScanDirectory_Blocked(t *testing.T) {
	chain := NewScanChain()
	chain.engines = []VirusScanner{
		&mockEngine{name: "engine-a", ready: true, clean: false, virus: "DirVirus"},
	}

	result := chain.ScanDirectory(context.Background(), "/test/dir")
	if result.Clean {
		t.Error("Expected infected result")
	}
	if result.Virus != "DirVirus" {
		t.Errorf("Virus = %q, want %q", result.Virus, "DirVirus")
	}
}

func TestScanChain_Engines(t *testing.T) {
	chain := NewScanChain()
	e1 := &mockEngine{name: "a", ready: true, clean: true}
	e2 := &mockEngine{name: "b", ready: true, clean: true}
	chain.engines = []VirusScanner{e1, e2}

	engines := chain.Engines()
	if len(engines) != 2 {
		t.Errorf("Expected 2 engines, got %d", len(engines))
	}
}

func TestScanChain_GetStats(t *testing.T) {
	chain := NewScanChain()
	chain.engines = []VirusScanner{
		&mockEngine{name: "a", ready: true, clean: true, stats: map[string]interface{}{"scans": 10}},
	}

	stats := chain.GetStats()
	if len(stats) != 1 {
		t.Errorf("Expected 1 engine in stats, got %d", len(stats))
	}
}

func TestScanChain_GetExtensionRules(t *testing.T) {
	chain := NewScanChain()
	chain.configs = map[string]json.RawMessage{
		"clamav": []byte(`{"scan_extensions":[".exe"],"skip_extensions":[".txt"]}`),
	}

	rules := chain.GetExtensionRules()
	if len(rules.ScanExtensions) != 1 || rules.ScanExtensions[0] != ".exe" {
		t.Errorf("ScanExtensions = %v, want [.exe]", rules.ScanExtensions)
	}
	if len(rules.SkipExtensions) != 1 || rules.SkipExtensions[0] != ".txt" {
		t.Errorf("SkipExtensions = %v, want [.txt]", rules.SkipExtensions)
	}
}

func TestScanChain_RawConfig(t *testing.T) {
	chain := NewScanChain()
	rawCfg := []byte(`{"address":"127.0.0.1:3310"}`)
	chain.configs = map[string]json.RawMessage{
		"clamav": rawCfg,
	}

	cfg, ok := chain.RawConfig("clamav")
	if !ok {
		t.Error("Expected to find clamav config")
	}
	if string(cfg) != string(rawCfg) {
		t.Errorf("Config = %s, want %s", cfg, rawCfg)
	}

	_, ok = chain.RawConfig("nonexistent")
	if ok {
		t.Error("Should not find nonexistent config")
	}
}

func TestScanChain_ScanToolInvocation(t *testing.T) {
	chain := NewScanChain()
	chain.engines = []VirusScanner{
		&mockEngine{name: "clamav", ready: true, clean: true},
	}

	allowed, err := chain.ScanToolInvocation(context.Background(), "write_file", map[string]interface{}{
		"path":    "/test/file.exe",
		"content": "hello",
	})
	if !allowed {
		t.Error("Should be allowed")
	}
	if err != nil {
		t.Errorf("Unexpected error: %v", err)
	}
}

func TestScanChain_ScanToolInvocation_Blocked(t *testing.T) {
	chain := NewScanChain()
	chain.engines = []VirusScanner{
		&mockEngine{name: "clamav", ready: true, clean: false, virus: "Test.Virus"},
	}

	allowed, err := chain.ScanToolInvocation(context.Background(), "write_file", map[string]interface{}{
		"path":    "/test/file.exe",
		"content": "malicious",
	})
	if allowed {
		t.Error("Should be blocked")
	}
	if err == nil {
		t.Error("Expected error")
	}
}

func TestScanChain_ScanToolInvocation_EmptyChain(t *testing.T) {
	chain := NewScanChain()

	allowed, err := chain.ScanToolInvocation(context.Background(), "write_file", map[string]interface{}{
		"path": "/test/file.exe",
	})
	if !allowed {
		t.Error("Empty chain should allow")
	}
	if err != nil {
		t.Errorf("Unexpected error: %v", err)
	}
}

func TestScanChain_ScanToolInvocation_SkipByExtension(t *testing.T) {
	chain := NewScanChain()
	chain.configs = map[string]json.RawMessage{
		"clamav": []byte(`{"skip_extensions":[".txt"]}`),
	}
	chain.engines = []VirusScanner{
		&mockEngine{name: "clamav", ready: true, clean: false, virus: "Bad"},
	}

	// .txt file should be skipped
	allowed, err := chain.ScanToolInvocation(context.Background(), "write_file", map[string]interface{}{
		"path":    "/test/file.txt",
		"content": "text",
	})
	if !allowed {
		t.Error(".txt files should be skipped")
	}
	if err != nil {
		t.Errorf("Unexpected error: %v", err)
	}
}

func TestScanChain_LoadFromConfig_SkipNotInstalled(t *testing.T) {
	cfg := &config.ScannerFullConfig{
		Enabled: []string{"clamav"},
		Engines: map[string]json.RawMessage{
			"clamav": json.RawMessage(`{
				"address": "127.0.0.1:3310",
				"state": {"install_status": "failed"}
			}`),
		},
	}

	chain := NewScanChain()
	if err := chain.LoadFromConfig(cfg); err != nil {
		t.Fatalf("LoadFromConfig() error: %v", err)
	}

	engines := chain.Engines()
	if len(engines) != 0 {
		t.Errorf("Expected 0 engines (failed status should skip), got %d", len(engines))
	}
}

func TestScanChain_LoadFromConfig_SkipPending(t *testing.T) {
	cfg := &config.ScannerFullConfig{
		Enabled: []string{"clamav"},
		Engines: map[string]json.RawMessage{
			"clamav": json.RawMessage(`{
				"address": "127.0.0.1:3310",
				"state": {"install_status": "pending"}
			}`),
		},
	}

	chain := NewScanChain()
	if err := chain.LoadFromConfig(cfg); err != nil {
		t.Fatalf("LoadFromConfig() error: %v", err)
	}

	engines := chain.Engines()
	if len(engines) != 0 {
		t.Errorf("Expected 0 engines (pending status should skip), got %d", len(engines))
	}
}

func TestScanChain_LoadFromConfig_NoState_BackCompat(t *testing.T) {
	cfg := &config.ScannerFullConfig{
		Enabled: []string{"clamav"},
		Engines: map[string]json.RawMessage{
			"clamav": json.RawMessage(`{"address": "127.0.0.1:3310"}`),
		},
	}

	chain := NewScanChain()
	if err := chain.LoadFromConfig(cfg); err != nil {
		t.Fatalf("LoadFromConfig() error: %v", err)
	}

	engines := chain.Engines()
	if len(engines) != 1 {
		t.Errorf("Expected 1 engine (no state = load normally), got %d", len(engines))
	}
}

func TestScanChain_LoadFromConfig_Installed(t *testing.T) {
	cfg := &config.ScannerFullConfig{
		Enabled: []string{"clamav"},
		Engines: map[string]json.RawMessage{
			"clamav": json.RawMessage(`{
				"address": "127.0.0.1:3310",
				"state": {"install_status": "installed"}
			}`),
		},
	}

	chain := NewScanChain()
	if err := chain.LoadFromConfig(cfg); err != nil {
		t.Fatalf("LoadFromConfig() error: %v", err)
	}

	engines := chain.Engines()
	if len(engines) != 1 {
		t.Errorf("Expected 1 engine (installed status should load), got %d", len(engines))
	}
}
