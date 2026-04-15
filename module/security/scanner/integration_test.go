// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package scanner

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/config"
)

const (
	// clamAVPath is the real ClamAV installation directory.
	clamAVPath = `C:\AI\NemesisBot\clamav-1.5.2.win.x64`
	// clamAVDBDir is the directory containing the downloaded virus database.
	clamAVDBDir = `C:\Users\Zoo\AppData\Local\Temp\clamav_test_db`
	// persistentEICARFile is a pre-created EICAR test file that clamd can access.
	// Dynamic temp files fail with "File path check failure" on Windows.
	persistentEICARFile = `C:\AI\NemesisBot\NemesisBot\test\eicar_test.exe`
)

// skipIfNoClamAV skips the test if ClamAV is not available.
func skipIfNoClamAV(t *testing.T) {
	t.Helper()
	if runtime.GOOS != "windows" {
		t.Skip("Integration test only runs on Windows")
	}
	clamdExe := filepath.Join(clamAVPath, "clamd.exe")
	if _, err := os.Stat(clamdExe); os.IsNotExist(err) {
		t.Skipf("ClamAV not found at %s", clamAVPath)
	}
	// Check database exists
	if _, err := os.Stat(filepath.Join(clamAVDBDir, "main.cvd")); os.IsNotExist(err) {
		t.Skipf("ClamAV database not found at %s (run freshclam first)", clamAVDBDir)
	}
}

// skipIfNoEICAR skips the test if the persistent EICAR test file is not available.
func skipIfNoEICAR(t *testing.T) {
	t.Helper()
	if _, err := os.Stat(persistentEICARFile); os.IsNotExist(err) {
		t.Skipf("EICAR test file not found at %s", persistentEICARFile)
	}
}

// setupTestDataDir creates a temporary data directory with the virus database
// copied from the pre-downloaded location. Returns the dataDir path and a cleanup function.
func setupTestDataDir(t *testing.T) (string, string) {
	t.Helper()

	dataDir := filepath.Join(t.TempDir(), "clamav-data")
	dbDir := filepath.Join(dataDir, "database")
	if err := os.MkdirAll(dbDir, 0755); err != nil {
		t.Fatalf("Failed to create db dir: %v", err)
	}

	// Copy database files
	entries, err := os.ReadDir(clamAVDBDir)
	if err != nil {
		t.Fatalf("Failed to read DB dir: %v", err)
	}
	for _, entry := range entries {
		if entry.IsDir() {
			continue
		}
		src := filepath.Join(clamAVDBDir, entry.Name())
		dst := filepath.Join(dbDir, entry.Name())
		copyFile(t, src, dst)
	}

	// Pick a random port by binding temporarily
	port := findFreePort(t)
	address := fmt.Sprintf("127.0.0.1:%d", port)

	return dataDir, address
}

func copyFile(t *testing.T, src, dst string) {
	t.Helper()
	in, err := os.Open(src)
	if err != nil {
		t.Fatalf("Failed to open %s: %v", src, err)
	}
	defer in.Close()

	out, err := os.Create(dst)
	if err != nil {
		t.Fatalf("Failed to create %s: %v", dst, err)
	}
	defer out.Close()

	if _, err := io.Copy(out, in); err != nil {
		t.Fatalf("Failed to copy %s → %s: %v", src, dst, err)
	}
}

func findFreePort(t *testing.T) int {
	t.Helper()
	// Use a deterministic high port based on PID to avoid conflicts
	return 33900 + (os.Getpid() % 100)
}

// newTestEngine creates a ClamAVEngine configured for integration testing.
func newTestEngine(t *testing.T) (*ClamAVEngine, string) {
	t.Helper()

	dataDir, address := setupTestDataDir(t)

	rawConfig, _ := json.Marshal(map[string]interface{}{
		"clamav_path":     clamAVPath,
		"address":         address,
		"data_dir":        dataDir,
		"scan_on_write":   true,
		"scan_on_download": true,
		"scan_on_exec":     true,
		"max_file_size":    52428800,
		"update_interval":  "0",
		"skip_extensions":  []string{".txt", ".md", ".json"},
	})

	engine, err := NewClamAVEngine(rawConfig)
	if err != nil {
		t.Fatalf("NewClamAVEngine() error: %v", err)
	}
	return engine, address
}

// createEICARFile returns the path to an EICAR test file.
// Uses the persistent file if available; otherwise creates a new one.
func createEICARFile(t *testing.T) string {
	t.Helper()
	// Prefer the persistent EICAR file which is known to work with clamd on Windows
	if _, err := os.Stat(persistentEICARFile); err == nil {
		return persistentEICARFile
	}
	// Fallback: create dynamically (may fail with clamd on Windows)
	eicar := `X5O!P%@AP[4\PZX54(P^)7CC)7}$EICAR-STANDARD-ANTIVIRUS-TEST-FILE!$H+H*`
	dir := filepath.Join(clamAVPath, "test_files")
	os.MkdirAll(dir, 0755)
	tmpFile := filepath.Join(dir, fmt.Sprintf("eicar_%d.exe", time.Now().UnixNano()))
	if err := os.WriteFile(tmpFile, []byte(eicar), 0644); err != nil {
		t.Fatalf("Failed to create EICAR file: %v", err)
	}
	t.Cleanup(func() { os.Remove(tmpFile) })
	return tmpFile
}

// createCleanFile creates a benign test file.
func createCleanFile(t *testing.T) string {
	t.Helper()
	dir := filepath.Join(clamAVPath, "test_files")
	os.MkdirAll(dir, 0755)
	tmpFile := filepath.Join(dir, fmt.Sprintf("clean_%d.txt", time.Now().UnixNano()))
	if err := os.WriteFile(tmpFile, []byte("This is a clean test file for ClamAV integration testing.\n"), 0644); err != nil {
		t.Fatalf("Failed to create clean file: %v", err)
	}
	t.Cleanup(func() { os.Remove(tmpFile) })
	return tmpFile
}

// --- Integration Tests ---

func TestIntegration_ClamAVEngine_Validate(t *testing.T) {
	skipIfNoClamAV(t)

	engine, _ := newTestEngine(t)

	// Valid directory
	if err := engine.Validate(clamAVPath); err != nil {
		t.Errorf("Validate(%s) should succeed: %v", clamAVPath, err)
	}

	// Invalid directory
	if err := engine.Validate(t.TempDir()); err == nil {
		t.Error("Validate on empty dir should fail")
	}
}

func TestIntegration_ClamAVEngine_StartStop(t *testing.T) {
	skipIfNoClamAV(t)

	engine, _ := newTestEngine(t)
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()

	// Start
	if err := engine.Start(ctx); err != nil {
		t.Fatalf("Start() error: %v", err)
	}
	defer engine.Stop()

	// Should be ready
	if !engine.IsReady() {
		t.Error("IsReady() should be true after Start()")
	}

	// Double start should be no-op
	if err := engine.Start(ctx); err != nil {
		t.Errorf("Second Start() should be no-op: %v", err)
	}

	// Stop
	if err := engine.Stop(); err != nil {
		t.Errorf("Stop() error: %v", err)
	}

	// Should not be ready after stop
	if engine.IsReady() {
		t.Error("IsReady() should be false after Stop()")
	}

	// Double stop should be safe
	if err := engine.Stop(); err != nil {
		t.Errorf("Second Stop() should be safe: %v", err)
	}
}

func TestIntegration_ClamAVEngine_GetInfo_Running(t *testing.T) {
	skipIfNoClamAV(t)

	engine, _ := newTestEngine(t)
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()

	if err := engine.Start(ctx); err != nil {
		t.Fatalf("Start() error: %v", err)
	}
	defer engine.Stop()

	info, err := engine.GetInfo(ctx)
	if err != nil {
		t.Fatalf("GetInfo() error: %v", err)
	}

	if info.Name != "clamav" {
		t.Errorf("Name = %q, want 'clamav'", info.Name)
	}
	if !info.Ready {
		t.Error("Ready should be true")
	}
	if info.Version == "" {
		t.Error("Version should not be empty for running engine")
	}
	if !strings.Contains(info.Version, "ClamAV") {
		t.Errorf("Version should contain 'ClamAV', got %q", info.Version)
	}
	t.Logf("Engine info: name=%s version=%s address=%s ready=%v",
		info.Name, info.Version, info.Address, info.Ready)
}

func TestIntegration_ClamAVEngine_ScanFile_Clean(t *testing.T) {
	skipIfNoClamAV(t)

	engine, _ := newTestEngine(t)
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()

	if err := engine.Start(ctx); err != nil {
		t.Fatalf("Start() error: %v", err)
	}
	defer engine.Stop()

	cleanFile := createCleanFile(t)
	result, err := engine.ScanFile(ctx, cleanFile)
	if err != nil {
		t.Fatalf("ScanFile() error: %v", err)
	}

	if result.Infected {
		t.Error("Clean file should not be infected")
	}
	if result.Engine != "clamav" {
		t.Errorf("Engine = %q, want 'clamav'", result.Engine)
	}
	if result.Path != cleanFile {
		t.Errorf("Path = %q, want %q", result.Path, cleanFile)
	}
	t.Logf("Scan clean file: infected=%v path=%s raw=%s", result.Infected, result.Path, result.Raw)
}

func TestIntegration_ClamAVEngine_ScanFile_Infected(t *testing.T) {
	skipIfNoClamAV(t)
	skipIfNoEICAR(t)

	engine, _ := newTestEngine(t)
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()

	if err := engine.Start(ctx); err != nil {
		t.Fatalf("Start() error: %v", err)
	}
	defer engine.Stop()

	eicarFile := createEICARFile(t)
	result, err := engine.ScanFile(ctx, eicarFile)
	if err != nil {
		t.Fatalf("ScanFile() error: %v", err)
	}

	if !result.Infected {
		t.Error("EICAR test file should be detected as infected")
	}
	if result.Virus == "" {
		t.Error("Virus name should not be empty for infected file")
	}
	if result.Engine != "clamav" {
		t.Errorf("Engine = %q, want 'clamav'", result.Engine)
	}
	t.Logf("Scan infected file: infected=%v virus=%q raw=%s", result.Infected, result.Virus, result.Raw)
}

func TestIntegration_ClamAVEngine_ScanFile_NonExistent(t *testing.T) {
	skipIfNoClamAV(t)

	engine, _ := newTestEngine(t)
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()

	if err := engine.Start(ctx); err != nil {
		t.Fatalf("Start() error: %v", err)
	}
	defer engine.Stop()

	_, err := engine.ScanFile(ctx, "/nonexistent/path/file.exe")
	if err == nil {
		t.Error("Scanning nonexistent file should return error")
	}
}

func TestIntegration_ClamAVEngine_ScanContent_Clean(t *testing.T) {
	skipIfNoClamAV(t)

	engine, _ := newTestEngine(t)
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()

	if err := engine.Start(ctx); err != nil {
		t.Fatalf("Start() error: %v", err)
	}
	defer engine.Stop()

	result, err := engine.ScanContent(ctx, []byte("This is safe content"))
	if err != nil {
		t.Fatalf("ScanContent() error: %v", err)
	}

	if result.Infected {
		t.Error("Clean content should not be infected")
	}
	if result.Engine != "clamav" {
		t.Errorf("Engine = %q, want 'clamav'", result.Engine)
	}
}

func TestIntegration_ClamAVEngine_ScanContent_Infected(t *testing.T) {
	skipIfNoClamAV(t)

	engine, _ := newTestEngine(t)
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()

	if err := engine.Start(ctx); err != nil {
		t.Fatalf("Start() error: %v", err)
	}
	defer engine.Stop()

	eicar := `X5O!P%@AP[4\PZX54(P^)7CC)7}$EICAR-STANDARD-ANTIVIRUS-TEST-FILE!$H+H*`
	result, err := engine.ScanContent(ctx, []byte(eicar))
	if err != nil {
		t.Fatalf("ScanContent() error: %v", err)
	}

	if !result.Infected {
		t.Error("EICAR content should be detected as infected")
	}
	if result.Virus == "" {
		t.Error("Virus name should not be empty")
	}
	t.Logf("Content scan: infected=%v virus=%q", result.Infected, result.Virus)
}

func TestIntegration_ClamAVEngine_ScanDirectory(t *testing.T) {
	skipIfNoClamAV(t)

	engine, _ := newTestEngine(t)
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()

	if err := engine.Start(ctx); err != nil {
		t.Fatalf("Start() error: %v", err)
	}
	defer engine.Stop()

	// Create a directory with mixed files in ClamAV dir
	dir := filepath.Join(clamAVPath, "test_files", "scandir")
	os.RemoveAll(dir)
	os.MkdirAll(dir, 0755)
	os.WriteFile(filepath.Join(dir, "clean.txt"), []byte("clean"), 0644)
	os.WriteFile(filepath.Join(dir, "eicar.exe"), []byte(
		`X5O!P%@AP[4\PZX54(P^)7CC)7}$EICAR-STANDARD-ANTIVIRUS-TEST-FILE!$H+H*`), 0644)

	results, err := engine.ScanDirectory(ctx, dir)
	if err != nil {
		t.Fatalf("ScanDirectory() error: %v", err)
	}

	if len(results) == 0 {
		t.Fatal("Expected at least one result from directory scan")
	}

	// At least one should be the EICAR file
	foundInfected := false
	for _, r := range results {
		t.Logf("Dir scan result: path=%s infected=%v virus=%q", r.Path, r.Infected, r.Virus)
		if r.Infected {
			foundInfected = true
		}
	}
	if !foundInfected {
		t.Error("Expected at least one infected file in directory scan")
	}
}

func TestIntegration_ClamAVEngine_GetStats_Running(t *testing.T) {
	skipIfNoClamAV(t)

	engine, _ := newTestEngine(t)
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()

	if err := engine.Start(ctx); err != nil {
		t.Fatalf("Start() error: %v", err)
	}
	defer engine.Stop()

	stats := engine.GetStats()
	if stats == nil {
		t.Fatal("GetStats() should not return nil")
	}

	started, ok := stats["started"].(bool)
	if !ok || !started {
		t.Errorf("Expected started=true, got %v", stats["started"])
	}

	enabled, ok := stats["enabled"].(bool)
	if !ok || !enabled {
		t.Errorf("Expected enabled=true, got %v", stats["enabled"])
	}

	t.Logf("Engine stats: %v", stats)
}

func TestIntegration_ClamAVEngine_GetDatabaseStatus(t *testing.T) {
	skipIfNoClamAV(t)

	engine, _ := newTestEngine(t)
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()

	if err := engine.Start(ctx); err != nil {
		t.Fatalf("Start() error: %v", err)
	}
	defer engine.Stop()

	status, err := engine.GetDatabaseStatus(ctx)
	if err != nil {
		t.Fatalf("GetDatabaseStatus() error: %v", err)
	}

	t.Logf("Database status: available=%v last_update=%v", status.Available, status.LastUpdate)
}

func TestIntegration_ClamAVEngine_UpdateDatabase_NotReady(t *testing.T) {
	skipIfNoClamAV(t)

	engine, _ := newTestEngine(t)
	// Not started
	err := engine.UpdateDatabase(context.Background())
	if err == nil {
		t.Error("UpdateDatabase should fail when engine not ready")
	}
}

// --- ScanChain Integration Tests ---

func TestIntegration_ScanChain_RealEngine_CleanFile(t *testing.T) {
	skipIfNoClamAV(t)

	engine, _ := newTestEngine(t)
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()

	if err := engine.Start(ctx); err != nil {
		t.Fatalf("Start() error: %v", err)
	}
	defer engine.Stop()

	chain := NewScanChain()
	chain.engines = []VirusScanner{engine}

	cleanFile := createCleanFile(t)
	result := chain.ScanFile(ctx, cleanFile)

	if !result.Clean {
		t.Error("Clean file should pass scan chain")
	}
	if result.Blocked {
		t.Error("Should not be blocked")
	}
	t.Logf("Chain result: clean=%v blocked=%v duration=%v", result.Clean, result.Blocked, result.Duration)
}

func TestIntegration_ScanChain_RealEngine_InfectedFile(t *testing.T) {
	skipIfNoClamAV(t)
	skipIfNoEICAR(t)

	engine, _ := newTestEngine(t)
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()

	if err := engine.Start(ctx); err != nil {
		t.Fatalf("Start() error: %v", err)
	}
	defer engine.Stop()

	chain := NewScanChain()
	chain.engines = []VirusScanner{engine}

	eicarFile := createEICARFile(t)
	result := chain.ScanFile(ctx, eicarFile)

	if result.Clean {
		t.Error("EICAR file should be detected")
	}
	if !result.Blocked {
		t.Error("Should be blocked")
	}
	if result.Engine != "clamav" {
		t.Errorf("Engine = %q, want 'clamav'", result.Engine)
	}
	if result.Virus == "" {
		t.Error("Virus name should not be empty")
	}
	t.Logf("Chain result: clean=%v blocked=%v engine=%s virus=%q duration=%v",
		result.Clean, result.Blocked, result.Engine, result.Virus, result.Duration)
}

func TestIntegration_ScanChain_RealEngine_ScanContent(t *testing.T) {
	skipIfNoClamAV(t)

	engine, _ := newTestEngine(t)
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()

	if err := engine.Start(ctx); err != nil {
		t.Fatalf("Start() error: %v", err)
	}
	defer engine.Stop()

	chain := NewScanChain()
	chain.engines = []VirusScanner{engine}

	// Clean content
	result := chain.ScanContent(ctx, []byte("Hello World"))
	if !result.Clean {
		t.Errorf("Clean content should pass: %+v", result)
	}

	// EICAR content
	eicar := `X5O!P%@AP[4\PZX54(P^)7CC)7}$EICAR-STANDARD-ANTIVIRUS-TEST-FILE!$H+H*`
	result = chain.ScanContent(ctx, []byte(eicar))
	if result.Clean {
		t.Errorf("EICAR content should be blocked: %+v", result)
	}
	if !result.Blocked {
		t.Error("Should be blocked")
	}
}

func TestIntegration_ScanChain_RealEngine_ScanToolInvocation(t *testing.T) {
	skipIfNoClamAV(t)

	engine, _ := newTestEngine(t)
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()

	if err := engine.Start(ctx); err != nil {
		t.Fatalf("Start() error: %v", err)
	}
	defer engine.Stop()

	chain := NewScanChain()
	chain.engines = []VirusScanner{engine}
	chain.configs = map[string]json.RawMessage{
		"clamav": []byte(`{"skip_extensions":[".txt",".md",".json"]}`),
	}

	// Test write_file with clean content
	allowed, err := chain.ScanToolInvocation(ctx, "write_file", map[string]interface{}{
		"path":    "/test/clean.exe",
		"content": "This is safe content",
	})
	if !allowed {
		t.Errorf("Clean write_file should be allowed: %v", err)
	}

	// Test write_file with EICAR content
	eicar := `X5O!P%@AP[4\PZX54(P^)7CC)7}$EICAR-STANDARD-ANTIVIRUS-TEST-FILE!$H+H*`
	allowed, err = chain.ScanToolInvocation(ctx, "write_file", map[string]interface{}{
		"path":    "/test/eicar.exe",
		"content": eicar,
	})
	if allowed {
		t.Error("EICAR write should be blocked")
	}
	if err == nil {
		t.Error("Should return error for infected content")
	} else {
		if !strings.Contains(err.Error(), "virus detected") {
			t.Errorf("Error should mention virus: %v", err)
		}
		t.Logf("Tool invocation blocked: %v", err)
	}

	// Test skip by extension
	allowed, err = chain.ScanToolInvocation(ctx, "write_file", map[string]interface{}{
		"path":    "/test/readme.txt",
		"content": "some text",
	})
	if !allowed {
		t.Errorf(".txt should be skipped: %v", err)
	}
}

// --- LoadFromConfig Integration Test ---

func TestIntegration_ScanChain_LoadFromConfig(t *testing.T) {
	skipIfNoClamAV(t)
	skipIfNoEICAR(t)

	dataDir, address := setupTestDataDir(t)

	scannerCfg := map[string]interface{}{
		"clamav_path":      clamAVPath,
		"address":          address,
		"data_dir":         dataDir,
		"scan_on_write":    true,
		"scan_on_download": true,
		"scan_on_exec":     true,
		"max_file_size":    52428800,
		"update_interval":  "0",
		"skip_extensions":  []string{".txt", ".md", ".json"},
	}
	rawCfg, _ := json.Marshal(scannerCfg)

	fullCfg := &config.ScannerFullConfig{
		Enabled: []string{"clamav"},
		Engines: map[string]json.RawMessage{
			"clamav": rawCfg,
		},
	}

	chain := NewScanChain()
	if err := chain.LoadFromConfig(fullCfg); err != nil {
		t.Fatalf("LoadFromConfig() error: %v", err)
	}

	engines := chain.Engines()
	if len(engines) != 1 {
		t.Fatalf("Expected 1 engine, got %d", len(engines))
	}
	if engines[0].Name() != "clamav" {
		t.Errorf("Engine name = %q, want 'clamav'", engines[0].Name())
	}

	// Start the chain
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()

	if err := chain.Start(ctx); err != nil {
		t.Fatalf("Start() error: %v", err)
	}
	defer chain.Stop()

	// Wait for engine to be ready
	time.Sleep(3 * time.Second)

	if !engines[0].IsReady() {
		t.Fatal("Engine should be ready after start")
	}

	// Test scanning through the loaded chain
	cleanFile := createCleanFile(t)
	result := chain.ScanFile(ctx, cleanFile)
	if !result.Clean {
		t.Error("Clean file should pass")
	}

	// Test EICAR through the loaded chain
	eicarFile := createEICARFile(t)
	result = chain.ScanFile(ctx, eicarFile)
	if result.Clean {
		t.Error("EICAR should be blocked")
	}

	t.Logf("LoadFromConfig chain test: clean=%v blocked=%v virus=%q",
		result.Clean, result.Blocked, result.Virus)
}

// --- Plugin-like Integration Test ---

func TestIntegration_FullWorkflow(t *testing.T) {
	skipIfNoClamAV(t)
	skipIfNoEICAR(t)

	engine, _ := newTestEngine(t)
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()

	// 1. Create engine
	t.Log("Step 1: Create engine")
	if engine.Name() != "clamav" {
		t.Fatalf("Name = %q, want 'clamav'", engine.Name())
	}

	// 2. Validate
	t.Log("Step 2: Validate installation")
	if err := engine.Validate(clamAVPath); err != nil {
		t.Fatalf("Validate() error: %v", err)
	}

	// 3. Start
	t.Log("Step 3: Start engine")
	if err := engine.Start(ctx); err != nil {
		t.Fatalf("Start() error: %v", err)
	}
	defer engine.Stop()

	// 4. GetInfo
	t.Log("Step 4: GetInfo")
	info, err := engine.GetInfo(ctx)
	if err != nil {
		t.Fatalf("GetInfo() error: %v", err)
	}
	t.Logf("  Version: %s, Ready: %v", info.Version, info.Ready)

	// 5. Scan clean file
	t.Log("Step 5: Scan clean file")
	cleanFile := createCleanFile(t)
	result, err := engine.ScanFile(ctx, cleanFile)
	if err != nil {
		t.Fatalf("ScanFile(clean) error: %v", err)
	}
	if result.Infected {
		t.Error("Clean file reported as infected")
	}

	// 6. Scan infected file
	t.Log("Step 6: Scan infected file")
	eicarFile := createEICARFile(t)
	result, err = engine.ScanFile(ctx, eicarFile)
	if err != nil {
		t.Fatalf("ScanFile(eicar) error: %v", err)
	}
	if !result.Infected {
		t.Error("EICAR file not detected")
	}
	t.Logf("  Virus detected: %s", result.Virus)

	// 7. Scan content
	t.Log("Step 7: Scan content")
	result, err = engine.ScanContent(ctx, []byte("safe content"))
	if err != nil {
		t.Fatalf("ScanContent(clean) error: %v", err)
	}
	if result.Infected {
		t.Error("Clean content reported as infected")
	}

	// 8. GetStats
	t.Log("Step 8: GetStats")
	stats := engine.GetStats()
	t.Logf("  Stats: %v", stats)

	// 9. Stop
	t.Log("Step 9: Stop engine")
	if err := engine.Stop(); err != nil {
		t.Fatalf("Stop() error: %v", err)
	}

	t.Log("Full workflow completed successfully")
}
