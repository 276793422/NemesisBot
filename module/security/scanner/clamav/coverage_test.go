// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package clamav

import (
	"bufio"
	"context"
	"fmt"
	"io"
	"net"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"
)

// ---------------------------------------------------------------------------
// Priority 1: Pure functions (no mock needed)
// ---------------------------------------------------------------------------

// --- extractExecutablePath ---

func TestExtractExecutablePath_WindowsAbsPath(t *testing.T) {
	if runtime.GOOS != "windows" {
		t.Skip("windows-only")
	}
	got := extractExecutablePath(`C:\Users\test\program.exe --flag`)
	want := `C:\Users\test\program.exe`
	if got != want {
		t.Errorf("extractExecutablePath(%q) = %q, want %q",
			`C:\Users\test\program.exe --flag`, got, want)
	}
}

func TestExtractExecutablePath_WindowsFwdSlash(t *testing.T) {
	if runtime.GOOS != "windows" {
		t.Skip("windows-only")
	}
	got := extractExecutablePath(`C:/Users/test/program.exe`)
	if got != `C:/Users/test/program.exe` {
		t.Errorf("got %q", got)
	}
}

func TestExtractExecutablePath_WindowsBareCommand(t *testing.T) {
	if runtime.GOOS != "windows" {
		t.Skip("windows-only")
	}
	// Bare name without path separators returns empty on Windows
	got := extractExecutablePath("python script.py")
	if got != "" {
		t.Errorf("expected empty for bare command, got %q", got)
	}
}

func TestExtractExecutablePath_UnixAbsPath(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("unix-only")
	}
	got := extractExecutablePath("/usr/bin/python3 script.py")
	if got != "/usr/bin/python3" {
		t.Errorf("got %q", got)
	}
}

func TestExtractExecutablePath_UnixBareCommand(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("unix-only")
	}
	got := extractExecutablePath("ls -la")
	if got != "" {
		t.Errorf("expected empty for bare command, got %q", got)
	}
}

func TestExtractExecutablePath_EmptyString(t *testing.T) {
	got := extractExecutablePath("")
	if got != "" {
		t.Errorf("expected empty, got %q", got)
	}
}

func TestExtractExecutablePath_QuotedPath(t *testing.T) {
	if runtime.GOOS != "windows" {
		t.Skip("windows-only")
	}
	// Note: extractExecutablePath uses Trim + Fields, so quoted paths with spaces
	// get split by Fields(). This tests the actual behavior.
	got := extractExecutablePath(`"C:\Program Files\app.exe" --flag`)
	// After Trim("), the string becomes: C:\Program Files\app.exe --flag
	// Fields splits on whitespace, so first token is: C:\Program
	// This contains a backslash so it gets returned.
	if got != `C:\Program` {
		t.Errorf("got %q, want %q", got, `C:\Program`)
	}
}

// --- parseDurationString ---

func TestParseDurationString_Valid(t *testing.T) {
	tests := []struct {
		input string
		want  time.Duration
	}{
		{"24h", 24 * time.Hour},
		{"1h30m", 90 * time.Minute},
		{"30m", 30 * time.Minute},
		{"2h", 2 * time.Hour},
	}
	for _, tt := range tests {
		got := parseDurationString(tt.input)
		if got != tt.want {
			t.Errorf("parseDurationString(%q) = %v, want %v", tt.input, got, tt.want)
		}
	}
}

func TestParseDurationString_Empty(t *testing.T) {
	if parseDurationString("") != 0 {
		t.Error("expected 0 for empty string")
	}
}

func TestParseDurationString_Invalid(t *testing.T) {
	if parseDurationString("not-a-duration") != 0 {
		t.Error("expected 0 for invalid string")
	}
}

// --- isSingleResponseCommand ---

func TestIsSingleResponseCommand(t *testing.T) {
	tests := []struct {
		cmd  string
		want bool
	}{
		{"PING", true},
		{"VERSION", true},
		{"SCAN /tmp/test.exe", true},
		{"CONTSCAN /tmp", true},
		{"STATS", false},
		{"RELOAD", false},
		{"INSTREAM", false},
	}
	for _, tt := range tests {
		got := isSingleResponseCommand(tt.cmd)
		if got != tt.want {
			t.Errorf("isSingleResponseCommand(%q) = %v, want %v", tt.cmd, got, tt.want)
		}
	}
}

// --- trimTrailingNewlines ---

func TestTrimTrailingNewlines(t *testing.T) {
	tests := []struct {
		input string
		want  string
	}{
		{"hello\n", "hello"},
		{"hello\r\n", "hello"},
		{"hello\r", "hello"},
		{"hello\n\n\n", "hello"},
		{"hello", "hello"},
		{"", ""},
		{"\n", ""},
		{"\r\n", ""},
		{"line1\nline2\n", "line1\nline2"},
	}
	for _, tt := range tests {
		got := trimTrailingNewlines(tt.input)
		if got != tt.want {
			t.Errorf("trimTrailingNewlines(%q) = %q, want %q", tt.input, got, tt.want)
		}
	}
}

// --- ScanResult.Clean ---

func TestScanResult_Clean(t *testing.T) {
	if (&ScanResult{Infected: false}).Clean() != true {
		t.Error("non-infected should be clean")
	}
	if (&ScanResult{Infected: true}).Clean() != false {
		t.Error("infected should not be clean")
	}
}

// --- logWriter.Write ---

func TestLogWriter_Write(t *testing.T) {
	w := &logWriter{prefix: "test"}
	n, err := w.Write([]byte("hello\n"))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if n != 6 {
		t.Errorf("expected n=6, got %d", n)
	}
}

func TestLogWriter_WriteEmpty(t *testing.T) {
	w := &logWriter{prefix: "test"}
	n, err := w.Write([]byte("\n"))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if n != 1 {
		t.Errorf("expected n=1, got %d", n)
	}
}

// --- findInPath ---

func TestFindInPath_NotFound(t *testing.T) {
	result := findInPath("nonexistent_binary_that_does_not_exist_12345")
	if result != "" {
		t.Errorf("expected empty for nonexistent binary, got %q", result)
	}
}

func TestFindInPath_KnownBinary(t *testing.T) {
	// On Windows, "cmd.exe" should be in PATH
	if runtime.GOOS == "windows" {
		result := findInPath("cmd.exe")
		if result == "" {
			t.Error("expected to find cmd.exe in PATH")
		}
	}
}

// --- DetectClamAVPath ---

func TestDetectClamAVPath_ReturnsString(t *testing.T) {
	// Just verify it doesn't panic and returns a string (may be empty)
	path := DetectClamAVPath()
	_ = path // no panic is sufficient
}

// --- Daemon.findExecutable ---

func TestDaemon_FindExecutable(t *testing.T) {
	d := &Daemon{config: &DaemonConfig{ClamAVPath: "/opt/clamav"}}
	exe := d.findExecutable("clamd")
	if runtime.GOOS == "windows" {
		if exe != filepath.Join("/opt/clamav", "clamd.exe") {
			t.Errorf("unexpected path: %s", exe)
		}
	} else {
		if exe != filepath.Join("/opt/clamav", "clamd") {
			t.Errorf("unexpected path: %s", exe)
		}
	}
}

// --- Updater.findExecutable ---

func TestUpdater_FindExecutable(t *testing.T) {
	u := &Updater{config: &UpdaterConfig{ClamAVPath: "/opt/clamav"}}
	exe := u.findExecutable("freshclam")
	if runtime.GOOS == "windows" {
		if exe != filepath.Join("/opt/clamav", "freshclam.exe") {
			t.Errorf("unexpected path: %s", exe)
		}
	} else {
		if exe != filepath.Join("/opt/clamav", "freshclam") {
			t.Errorf("unexpected path: %s", exe)
		}
	}
}

// --- Updater.LastUpdate ---

func TestUpdater_LastUpdate_Zero(t *testing.T) {
	u := NewUpdater(&UpdaterConfig{})
	if !u.LastUpdate().IsZero() {
		t.Error("expected zero time for new updater")
	}
}

// --- Manager.Hook ---

func TestManager_Hook_Nil(t *testing.T) {
	m := NewManager(&ManagerConfig{})
	if h := m.Hook(); h != nil {
		t.Error("expected nil hook for unstarted manager")
	}
}

// --- Manager.Scanner ---

func TestManager_Scanner_Nil(t *testing.T) {
	m := NewManager(&ManagerConfig{})
	if s := m.Scanner(); s != nil {
		t.Error("expected nil scanner for unstarted manager")
	}
}

// --- Manager.IsRunning ---

func TestManager_IsRunning_NotStarted(t *testing.T) {
	m := NewManager(&ManagerConfig{})
	if m.IsRunning() {
		t.Error("should not be running when not started")
	}
}

// --- Daemon.IsRunning ---

func TestDaemon_IsRunning(t *testing.T) {
	d := NewDaemon(&DaemonConfig{})
	if d.IsRunning() {
		t.Error("new daemon should not be running")
	}
}

// --- Daemon.Client ---

func TestDaemon_Client(t *testing.T) {
	cfg := &DaemonConfig{ListenAddr: "127.0.0.1:3310"}
	d := NewDaemon(cfg)
	if d.Client() == nil {
		t.Error("expected non-nil client")
	}
}

// --- Daemon default config ---

func TestNewDaemon_Defaults(t *testing.T) {
	d := NewDaemon(&DaemonConfig{})
	if d.config.ListenAddr != "127.0.0.1:3310" {
		t.Errorf("expected default listen addr, got %s", d.config.ListenAddr)
	}
	if d.config.StartupTimeout != 120*time.Second {
		t.Errorf("expected default timeout, got %v", d.config.StartupTimeout)
	}
}

// --- NewDaemon with custom config ---

func TestNewDaemon_CustomConfig(t *testing.T) {
	d := NewDaemon(&DaemonConfig{
		ListenAddr:     "0.0.0.0:9310",
		StartupTimeout: 30 * time.Second,
	})
	if d.config.ListenAddr != "0.0.0.0:9310" {
		t.Errorf("expected custom listen addr, got %s", d.config.ListenAddr)
	}
	if d.config.StartupTimeout != 30*time.Second {
		t.Errorf("expected custom timeout, got %v", d.config.StartupTimeout)
	}
}

// --- Updater.IsDatabaseStale ---

func TestUpdater_IsDatabaseStale_NoLastUpdate(t *testing.T) {
	tmpDir := t.TempDir()
	u := NewUpdater(&UpdaterConfig{DatabaseDir: tmpDir})
	// No last update and no CVD files => stale
	if !u.IsDatabaseStale(1 * time.Hour) {
		t.Error("expected stale when no updates and no CVD files")
	}
}

func TestUpdater_IsDatabaseStale_RecentUpdate(t *testing.T) {
	u := NewUpdater(&UpdaterConfig{})
	u.lastUpdate = time.Now()
	if u.IsDatabaseStale(1 * time.Hour) {
		t.Error("should not be stale with recent update")
	}
}

func TestUpdater_IsDatabaseStale_OldUpdate(t *testing.T) {
	u := NewUpdater(&UpdaterConfig{})
	u.lastUpdate = time.Now().Add(-2 * time.Hour)
	if !u.IsDatabaseStale(1 * time.Hour) {
		t.Error("should be stale with old update")
	}
}

func TestUpdater_IsDatabaseStale_OldCVDFile(t *testing.T) {
	tmpDir := t.TempDir()
	// Create an old CVD file
	cvdPath := filepath.Join(tmpDir, "main.cvd")
	if err := os.WriteFile(cvdPath, []byte("fake"), 0644); err != nil {
		t.Fatal(err)
	}
	// Set modification time to 2 days ago
	oldTime := time.Now().Add(-48 * time.Hour)
	if err := os.Chtimes(cvdPath, oldTime, oldTime); err != nil {
		t.Fatal(err)
	}

	u := NewUpdater(&UpdaterConfig{DatabaseDir: tmpDir})
	if !u.IsDatabaseStale(24 * time.Hour) {
		t.Error("should be stale with old CVD file")
	}
}

func TestUpdater_IsDatabaseStale_RecentCVDFile_StillStale(t *testing.T) {
	// When lastUpdate is zero, IsDatabaseStale always returns true regardless of
	// CVD file freshness. The CVD check only triggers an early "true" return for
	// old files; it never causes a "false" return.
	tmpDir := t.TempDir()
	cvdPath := filepath.Join(tmpDir, "daily.cvd")
	if err := os.WriteFile(cvdPath, []byte("fake"), 0644); err != nil {
		t.Fatal(err)
	}

	u := NewUpdater(&UpdaterConfig{DatabaseDir: tmpDir})
	// Even with a fresh CVD file, lastUpdate is zero so it's always stale
	if !u.IsDatabaseStale(24 * time.Hour) {
		t.Error("should always be stale when lastUpdate is zero")
	}
}

// --- Updater.Stop (idempotent) ---

func TestUpdater_Stop(t *testing.T) {
	u := NewUpdater(&UpdaterConfig{})
	// Should not panic when stopping without auto-update
	u.Stop()
}

// --- Manager.Stop when not started ---

func TestManager_Stop_NotStarted(t *testing.T) {
	m := NewManager(&ManagerConfig{})
	if err := m.Stop(); err != nil {
		t.Errorf("expected nil error for stop on unstarted manager, got %v", err)
	}
}

// --- Manager.GetStats ---

func TestManager_GetStats_NotStarted(t *testing.T) {
	m := NewManager(&ManagerConfig{Enabled: true})
	stats := m.GetStats()
	if stats["enabled"] != true {
		t.Error("expected enabled=true in stats")
	}
	if stats["started"] != false {
		t.Error("expected started=false in stats")
	}
}

// ---------------------------------------------------------------------------
// Priority 2: Tests using mockClamd
// ---------------------------------------------------------------------------

// mockClamdInfected is a mock that returns infected results
type mockClamdInfected struct {
	listener net.Listener
}

func newMockClamdInfected() (*mockClamdInfected, error) {
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return nil, err
	}
	return &mockClamdInfected{listener: listener}, nil
}

func (m *mockClamdInfected) address() string {
	return m.listener.Addr().String()
}

func (m *mockClamdInfected) serve() {
	for {
		conn, err := m.listener.Accept()
		if err != nil {
			return
		}
		go m.handleConn(conn)
	}
}

func (m *mockClamdInfected) handleConn(conn net.Conn) {
	defer conn.Close()
	reader := bufio.NewReader(conn)

	for {
		line, err := reader.ReadString('\n')
		if err != nil {
			return
		}
		line = strings.TrimSpace(line)
		if len(line) > 0 && line[0] == 'n' {
			line = line[1:]
		}

		switch {
		case line == "PING":
			fmt.Fprintf(conn, "PONG\n")
		case strings.HasPrefix(line, "SCAN "):
			path := strings.TrimPrefix(line, "SCAN ")
			fmt.Fprintf(conn, "%s: Win.Trojan.Test-123 FOUND\n", path)
		case strings.HasPrefix(line, "CONTSCAN "):
			path := strings.TrimPrefix(line, "CONTSCAN ")
			fmt.Fprintf(conn, "%s: Win.Trojan.Test-123 FOUND\n", path)
		case line == "INSTREAM":
			// Read INSTREAM data
			for {
				lenBuf := make([]byte, 4)
				if _, err := io.ReadFull(reader, lenBuf); err != nil {
					return
				}
				chunkLen := int(lenBuf[0])<<24 | int(lenBuf[1])<<16 | int(lenBuf[2])<<8 | int(lenBuf[3])
				if chunkLen == 0 {
					break
				}
				data := make([]byte, chunkLen)
				if _, err := io.ReadFull(reader, data); err != nil {
					return
				}
			}
			conn.Write([]byte("stream: Win.Trojan.Test-123 FOUND\n"))
		}
	}
}

func (m *mockClamdInfected) close() {
	m.listener.Close()
}

// --- Scanner.ScanFile with real mock server ---

func TestScanner_ScanFile_Clean(t *testing.T) {
	mock, err := newMockClamd()
	if err != nil {
		t.Fatal(err)
	}
	defer mock.close()
	go mock.serve()

	cfg := DefaultScannerConfig()
	cfg.Address = mock.address()
	s := NewScanner(cfg)

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	// Create a temp file to scan
	tmpFile := filepath.Join(t.TempDir(), "test.exe")
	if err := os.WriteFile(tmpFile, []byte("fake exe content"), 0644); err != nil {
		t.Fatal(err)
	}

	result, err := s.ScanFile(ctx, tmpFile)
	if err != nil {
		t.Fatalf("ScanFile failed: %v", err)
	}
	if result.Infected {
		t.Error("expected clean result")
	}
	if result.Path != tmpFile {
		t.Errorf("expected path %s, got %s", tmpFile, result.Path)
	}
}

func TestScanner_ScanFile_Infected(t *testing.T) {
	mock, err := newMockClamdInfected()
	if err != nil {
		t.Fatal(err)
	}
	defer mock.close()
	go mock.serve()

	cfg := DefaultScannerConfig()
	cfg.Address = mock.address()
	s := NewScanner(cfg)

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	tmpFile := filepath.Join(t.TempDir(), "malware.exe")
	if err := os.WriteFile(tmpFile, []byte("malicious content"), 0644); err != nil {
		t.Fatal(err)
	}

	result, err := s.ScanFile(ctx, tmpFile)
	if err != nil {
		t.Fatalf("ScanFile failed: %v", err)
	}
	if !result.Infected {
		t.Error("expected infected result")
	}
	if result.Virus != "Win.Trojan.Test-123" {
		t.Errorf("expected virus name, got %s", result.Virus)
	}
}

func TestScanner_ScanFile_FileTooLarge(t *testing.T) {
	cfg := DefaultScannerConfig()
	cfg.MaxFileSize = 10 // 10 bytes max
	cfg.Enabled = true
	s := NewScanner(cfg)

	ctx := context.Background()

	tmpFile := filepath.Join(t.TempDir(), "bigfile.exe")
	if err := os.WriteFile(tmpFile, []byte("this is more than ten bytes"), 0644); err != nil {
		t.Fatal(err)
	}

	result, err := s.ScanFile(ctx, tmpFile)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result.Infected {
		t.Error("should not be infected for size limit")
	}
	if !strings.Contains(result.Raw, "file too large") {
		t.Errorf("expected 'file too large' in raw, got %s", result.Raw)
	}
}

func TestScanner_ScanFile_NonexistentFile(t *testing.T) {
	cfg := DefaultScannerConfig()
	cfg.Enabled = true
	s := NewScanner(cfg)

	ctx := context.Background()
	_, err := s.ScanFile(ctx, "/nonexistent/path/test.exe")
	if err == nil {
		t.Error("expected error for nonexistent file")
	}
}

// --- Scanner.ScanContentBytes ---

func TestScanner_ScanContentBytes_Clean(t *testing.T) {
	mock, err := newMockClamd()
	if err != nil {
		t.Fatal(err)
	}
	defer mock.close()
	go mock.serve()

	cfg := DefaultScannerConfig()
	cfg.Address = mock.address()
	s := NewScanner(cfg)

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	result, err := s.ScanContentBytes(ctx, []byte("test content"))
	if err != nil {
		t.Fatalf("ScanContentBytes failed: %v", err)
	}
	if result.Infected {
		t.Error("expected clean result")
	}
}

func TestScanner_ScanContentBytes_Infected(t *testing.T) {
	mock, err := newMockClamdInfected()
	if err != nil {
		t.Fatal(err)
	}
	defer mock.close()
	go mock.serve()

	cfg := DefaultScannerConfig()
	cfg.Address = mock.address()
	s := NewScanner(cfg)

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	result, err := s.ScanContentBytes(ctx, []byte("malicious content"))
	if err != nil {
		t.Fatalf("ScanContentBytes failed: %v", err)
	}
	if !result.Infected {
		t.Error("expected infected result")
	}
}

// --- Scanner.ScanContent ---

func TestScanner_ScanContent_Disabled(t *testing.T) {
	cfg := DefaultScannerConfig()
	cfg.Enabled = false
	s := NewScanner(cfg)

	ctx := context.Background()
	result, err := s.ScanContent(ctx, strings.NewReader("data"), 4)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result == nil {
		t.Fatal("expected non-nil result")
	}
	if result.Raw != "scanning disabled" {
		t.Errorf("expected 'scanning disabled', got %s", result.Raw)
	}
}

func TestScanner_ScanContent_TooLarge(t *testing.T) {
	cfg := DefaultScannerConfig()
	cfg.Enabled = true
	cfg.MaxFileSize = 10
	s := NewScanner(cfg)

	ctx := context.Background()
	result, err := s.ScanContent(ctx, nil, 100)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if !strings.Contains(result.Raw, "content too large") {
		t.Errorf("expected 'content too large', got %s", result.Raw)
	}
}

// --- Scanner.ScanDirectory ---

func TestScanner_ScanDirectory_Disabled(t *testing.T) {
	cfg := DefaultScannerConfig()
	cfg.Enabled = false
	s := NewScanner(cfg)

	ctx := context.Background()
	results, err := s.ScanDirectory(ctx, "/tmp")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if results != nil {
		t.Error("expected nil results when disabled")
	}
}

func TestScanner_ScanDirectory_WithMock(t *testing.T) {
	mock, err := newMockClamd()
	if err != nil {
		t.Fatal(err)
	}
	defer mock.close()
	go mock.serve()

	cfg := DefaultScannerConfig()
	cfg.Address = mock.address()
	s := NewScanner(cfg)

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	results, err := s.ScanDirectory(ctx, t.TempDir())
	if err != nil {
		t.Fatalf("ScanDirectory failed: %v", err)
	}
	if len(results) == 0 {
		t.Error("expected at least one result")
	}
}

// --- Scanner.ShouldScanFile ---

func TestScanner_ShouldScanFile_Disabled(t *testing.T) {
	cfg := DefaultScannerConfig()
	cfg.Enabled = false
	s := NewScanner(cfg)

	if s.ShouldScanFile("test.exe") {
		t.Error("should not scan when disabled")
	}
}

func TestScanner_ShouldScanFile_SafeExtensions(t *testing.T) {
	cfg := DefaultScannerConfig()
	s := NewScanner(cfg)

	safeFiles := []string{
		"readme.txt", "data.json", "config.yaml", "config.yml",
		"data.xml", "data.csv", "app.log", "settings.ini",
		"config.toml", "page.html", "style.css", "app.js",
		"app.ts",
	}
	for _, f := range safeFiles {
		if s.ShouldScanFile(f) {
			t.Errorf("should not scan safe file: %s", f)
		}
	}
}

func TestScanner_ShouldScanFile_ExecExtensions(t *testing.T) {
	cfg := DefaultScannerConfig()
	s := NewScanner(cfg)

	execFiles := []string{
		"program.exe", "lib.dll", "script.bat", "script.cmd",
		"script.ps1", "script.sh", "lib.so", "lib.dylib",
		"setup.msi", "script.vbs", "app.com", "screensaver.scr",
		"app.pif", "app.jar", "script.py",
	}
	for _, f := range execFiles {
		if !s.ShouldScanFile(f) {
			t.Errorf("should scan exec file: %s", f)
		}
	}
}

func TestScanner_ShouldScanFile_UnknownExtension(t *testing.T) {
	cfg := DefaultScannerConfig()
	s := NewScanner(cfg)
	// Unknown extensions should be scanned (conservative)
	if !s.ShouldScanFile("archive.zip") {
		t.Error("should scan unknown extension")
	}
}

func TestScanner_ShouldScanFile_NoExtension(t *testing.T) {
	cfg := DefaultScannerConfig()
	s := NewScanner(cfg)
	// Files without extension should be scanned
	if !s.ShouldScanFile("Makefile") {
		t.Error("should scan file without extension")
	}
}

// --- Scanner.GetStats ---

func TestScanner_GetStats(t *testing.T) {
	cfg := DefaultScannerConfig()
	cfg.Enabled = false
	s := NewScanner(cfg)

	// Do a disabled scan (which returns early, no stats update)
	ctx := context.Background()
	s.ScanFile(ctx, "test.txt")

	stats := s.GetStats()
	if stats.TotalScans != 0 {
		t.Errorf("expected 0 total scans, got %d", stats.TotalScans)
	}
}

func TestScanner_GetStats_WithRecords(t *testing.T) {
	cfg := DefaultScannerConfig()
	cfg.Enabled = false
	s := NewScanner(cfg)

	// Manually record stats via recordScan (internal method)
	s.recordScan(100, false, false) // clean scan
	s.recordScan(200, true, false)  // infected scan
	s.recordScan(50, false, true)   // error scan

	stats := s.GetStats()
	if stats.TotalScans != 3 {
		t.Errorf("expected 3 total scans, got %d", stats.TotalScans)
	}
	if stats.CleanScans != 1 {
		t.Errorf("expected 1 clean scan, got %d", stats.CleanScans)
	}
	if stats.InfectedScans != 1 {
		t.Errorf("expected 1 infected scan, got %d", stats.InfectedScans)
	}
	if stats.Errors != 1 {
		t.Errorf("expected 1 error, got %d", stats.Errors)
	}
	if stats.TotalBytes != 350 {
		t.Errorf("expected 350 total bytes, got %d", stats.TotalBytes)
	}
}

// --- ScanHook.ScanToolInvocation ---

func TestScanHook_ScanToolInvocation_Enabled(t *testing.T) {
	mock, err := newMockClamd()
	if err != nil {
		t.Fatal(err)
	}
	defer mock.close()
	go mock.serve()

	cfg := DefaultScannerConfig()
	cfg.Address = mock.address()
	s := NewScanner(cfg)
	hook := NewScanHook(s)

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	// Test write_file with content
	clean, err := hook.ScanToolInvocation(ctx, "write_file", map[string]interface{}{
		"path":    "/tmp/test.txt",
		"content": "hello world",
	})
	if !clean {
		t.Errorf("expected clean for safe content; err=%v", err)
	}
}

func TestScanHook_ScanToolInvocation_UnknownTool(t *testing.T) {
	cfg := DefaultScannerConfig()
	s := NewScanner(cfg)
	hook := NewScanHook(s)

	ctx := context.Background()
	clean, err := hook.ScanToolInvocation(ctx, "unknown_tool", map[string]interface{}{})
	if !clean || err != nil {
		t.Errorf("expected clean=true, err=nil for unknown tool; got clean=%v, err=%v", clean, err)
	}
}

func TestScanHook_ScanToolInvocation_NilScanner(t *testing.T) {
	hook := NewScanHook(nil)
	ctx := context.Background()
	clean, err := hook.ScanToolInvocation(ctx, "write_file", map[string]interface{}{"path": "/tmp/test.txt"})
	if !clean || err != nil {
		t.Errorf("expected clean=true for nil scanner; got clean=%v, err=%v", clean, err)
	}
}

func TestScanHook_ScanToolInvocation_WriteNoContent(t *testing.T) {
	cfg := DefaultScannerConfig()
	s := NewScanner(cfg)
	hook := NewScanHook(s)

	ctx := context.Background()
	clean, err := hook.ScanToolInvocation(ctx, "write_file", map[string]interface{}{
		"path": "/tmp/test.txt",
	})
	if !clean || err != nil {
		t.Errorf("expected clean=true, err=nil; got clean=%v, err=%v", clean, err)
	}
}

func TestScanHook_ScanToolInvocation_EditFileWithNewText(t *testing.T) {
	mock, err := newMockClamd()
	if err != nil {
		t.Fatal(err)
	}
	defer mock.close()
	go mock.serve()

	cfg := DefaultScannerConfig()
	cfg.Address = mock.address()
	s := NewScanner(cfg)
	hook := NewScanHook(s)

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	clean, err := hook.ScanToolInvocation(ctx, "edit_file", map[string]interface{}{
		"path":     "/tmp/test.txt",
		"content":  "old content",
		"new_text": "new content",
	})
	if !clean {
		t.Errorf("expected clean; err=%v", err)
	}
}

func TestScanHook_ScanToolInvocation_DownloadNoPath(t *testing.T) {
	cfg := DefaultScannerConfig()
	s := NewScanner(cfg)
	hook := NewScanHook(s)

	ctx := context.Background()
	clean, err := hook.ScanToolInvocation(ctx, "download", map[string]interface{}{})
	if !clean || err != nil {
		t.Errorf("expected clean=true, err=nil; got clean=%v, err=%v", clean, err)
	}
}

func TestScanHook_ScanToolInvocation_ExecNoCommand(t *testing.T) {
	cfg := DefaultScannerConfig()
	s := NewScanner(cfg)
	hook := NewScanHook(s)

	ctx := context.Background()
	clean, err := hook.ScanToolInvocation(ctx, "exec", map[string]interface{}{})
	if !clean || err != nil {
		t.Errorf("expected clean=true, err=nil; got clean=%v, err=%v", clean, err)
	}
}

func TestScanHook_ScanToolInvocation_ScanOnWriteOff(t *testing.T) {
	cfg := DefaultScannerConfig()
	cfg.ScanOnWrite = false
	s := NewScanner(cfg)
	hook := NewScanHook(s)

	ctx := context.Background()
	clean, err := hook.ScanToolInvocation(ctx, "write_file", map[string]interface{}{
		"path": "/tmp/test.exe",
	})
	if !clean || err != nil {
		t.Errorf("expected clean when ScanOnWrite=false; got clean=%v, err=%v", clean, err)
	}
}

func TestScanHook_ScanToolInvocation_ScanOnDownloadOff(t *testing.T) {
	cfg := DefaultScannerConfig()
	cfg.ScanOnDownload = false
	s := NewScanner(cfg)
	hook := NewScanHook(s)

	ctx := context.Background()
	clean, err := hook.ScanToolInvocation(ctx, "download", map[string]interface{}{
		"save_path": "/tmp/test.exe",
	})
	if !clean || err != nil {
		t.Errorf("expected clean when ScanOnDownload=false; got clean=%v, err=%v", clean, err)
	}
}

func TestScanHook_ScanToolInvocation_ScanOnExecOff(t *testing.T) {
	cfg := DefaultScannerConfig()
	cfg.ScanOnExec = false
	s := NewScanner(cfg)
	hook := NewScanHook(s)

	ctx := context.Background()
	clean, err := hook.ScanToolInvocation(ctx, "exec", map[string]interface{}{
		"command": "/usr/bin/ls",
	})
	if !clean || err != nil {
		t.Errorf("expected clean when ScanOnExec=false; got clean=%v, err=%v", clean, err)
	}
}

// --- ScanHook.ScanFilePath ---

func TestScanHook_ScanFilePath_NonexistentFile(t *testing.T) {
	cfg := DefaultScannerConfig()
	s := NewScanner(cfg)
	hook := NewScanHook(s)

	ctx := context.Background()
	clean, result, err := hook.ScanFilePath(ctx, "/nonexistent/path/test.exe")
	if !clean || err != nil {
		t.Errorf("expected clean=true, err=nil for nonexistent; got clean=%v, err=%v", clean, err)
	}
	if result != nil {
		t.Error("expected nil result for nonexistent file")
	}
}

func TestScanHook_ScanFilePath_SafeExtension(t *testing.T) {
	cfg := DefaultScannerConfig()
	s := NewScanner(cfg)
	hook := NewScanHook(s)

	tmpFile := filepath.Join(t.TempDir(), "test.txt")
	if err := os.WriteFile(tmpFile, []byte("hello"), 0644); err != nil {
		t.Fatal(err)
	}

	ctx := context.Background()
	clean, result, err := hook.ScanFilePath(ctx, tmpFile)
	if !clean || err != nil {
		t.Errorf("expected clean for safe extension; got clean=%v, err=%v", clean, err)
	}
	if result != nil {
		t.Error("expected nil result for safe extension")
	}
}

func TestScanHook_ScanFilePath_ExistingFile(t *testing.T) {
	mock, err := newMockClamd()
	if err != nil {
		t.Fatal(err)
	}
	defer mock.close()
	go mock.serve()

	cfg := DefaultScannerConfig()
	cfg.Address = mock.address()
	s := NewScanner(cfg)
	hook := NewScanHook(s)

	tmpFile := filepath.Join(t.TempDir(), "test.exe")
	if err := os.WriteFile(tmpFile, []byte("fake"), 0644); err != nil {
		t.Fatal(err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	clean, result, err := hook.ScanFilePath(ctx, tmpFile)
	if !clean {
		t.Errorf("expected clean for mock clean response; err=%v", err)
	}
	if result == nil {
		t.Fatal("expected non-nil result")
	}
}

func TestScanHook_ScanFilePath_InfectedFile(t *testing.T) {
	mock, err := newMockClamdInfected()
	if err != nil {
		t.Fatal(err)
	}
	defer mock.close()
	go mock.serve()

	cfg := DefaultScannerConfig()
	cfg.Address = mock.address()
	s := NewScanner(cfg)
	hook := NewScanHook(s)

	tmpFile := filepath.Join(t.TempDir(), "malware.exe")
	if err := os.WriteFile(tmpFile, []byte("malicious"), 0644); err != nil {
		t.Fatal(err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	clean, result, err := hook.ScanFilePath(ctx, tmpFile)
	if clean {
		t.Error("expected infected (not clean)")
	}
	if err != nil {
		t.Errorf("expected nil error, got %v", err)
	}
	if result == nil || !result.Infected {
		t.Error("expected infected result")
	}
}

// --- ScanHook.ScanDownloadedFile ---

func TestScanHook_ScanDownloadedFile_Nonexistent(t *testing.T) {
	cfg := DefaultScannerConfig()
	s := NewScanner(cfg)
	hook := NewScanHook(s)

	ctx := context.Background()
	clean, result, err := hook.ScanDownloadedFile(ctx, "/nonexistent/file.exe")
	if !clean {
		t.Error("expected clean for nonexistent file")
	}
	if result != nil {
		t.Error("expected nil result for nonexistent file")
	}
	if err != nil {
		t.Errorf("expected nil error, got %v", err)
	}
}

func TestScanHook_ScanDownloadedFile_Clean(t *testing.T) {
	mock, err := newMockClamd()
	if err != nil {
		t.Fatal(err)
	}
	defer mock.close()
	go mock.serve()

	cfg := DefaultScannerConfig()
	cfg.Address = mock.address()
	s := NewScanner(cfg)
	hook := NewScanHook(s)

	tmpFile := filepath.Join(t.TempDir(), "download.exe")
	if err := os.WriteFile(tmpFile, []byte("downloaded content"), 0644); err != nil {
		t.Fatal(err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	clean, result, err := hook.ScanDownloadedFile(ctx, tmpFile)
	if !clean {
		t.Errorf("expected clean; err=%v", err)
	}
	if result == nil {
		t.Fatal("expected non-nil result")
	}
	// File should still exist
	if _, statErr := os.Stat(tmpFile); statErr != nil {
		t.Errorf("file should still exist after clean scan: %v", statErr)
	}
}

func TestScanHook_ScanDownloadedFile_Infected(t *testing.T) {
	mock, err := newMockClamdInfected()
	if err != nil {
		t.Fatal(err)
	}
	defer mock.close()
	go mock.serve()

	cfg := DefaultScannerConfig()
	cfg.Address = mock.address()
	s := NewScanner(cfg)
	hook := NewScanHook(s)

	tmpFile := filepath.Join(t.TempDir(), "infected_download.exe")
	if err := os.WriteFile(tmpFile, []byte("malicious download"), 0644); err != nil {
		t.Fatal(err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	clean, result, err := hook.ScanDownloadedFile(ctx, tmpFile)
	if clean {
		t.Error("expected infected (not clean)")
	}
	if err != nil {
		t.Errorf("expected nil error, got %v", err)
	}
	if result == nil || !result.Infected {
		t.Error("expected infected result")
	}
	// Infected file should have been removed
	if _, statErr := os.Stat(tmpFile); !os.IsNotExist(statErr) {
		t.Error("infected file should have been removed")
	}
}

// --- ScanHook.GetScanner ---

func TestScanHook_GetScanner(t *testing.T) {
	cfg := DefaultScannerConfig()
	s := NewScanner(cfg)
	hook := NewScanHook(s)

	if hook.GetScanner() != s {
		t.Error("GetScanner should return the same scanner")
	}
}

func TestScanHook_GetScanner_Nil(t *testing.T) {
	hook := NewScanHook(nil)
	if hook.GetScanner() != nil {
		t.Error("expected nil scanner")
	}
}

// --- ScanHook.HealthCheck ---

func TestScanHook_HealthCheck_NilScanner(t *testing.T) {
	hook := NewScanHook(nil)
	err := hook.HealthCheck(context.Background())
	if err == nil {
		t.Error("expected error for nil scanner")
	}
	if !strings.Contains(err.Error(), "scanner not initialized") {
		t.Errorf("unexpected error: %v", err)
	}
}

func TestScanHook_HealthCheck_ServerDown(t *testing.T) {
	cfg := DefaultScannerConfig()
	cfg.Address = "127.0.0.1:1" // port 1 - nobody listening
	s := NewScanner(cfg)
	hook := NewScanHook(s)

	err := hook.HealthCheck(context.Background())
	if err == nil {
		t.Error("expected error when server is down")
	}
}

func TestScanHook_HealthCheck_ServerUp(t *testing.T) {
	mock, err := newMockClamd()
	if err != nil {
		t.Fatal(err)
	}
	defer mock.close()
	go mock.serve()

	cfg := DefaultScannerConfig()
	cfg.Address = mock.address()
	s := NewScanner(cfg)
	hook := NewScanHook(s)

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := hook.HealthCheck(ctx); err != nil {
		t.Errorf("expected nil error for healthy server, got %v", err)
	}
}

// --- scanFileWriteArgs ---

func TestScanHook_scanFileWriteArgs_InfectedContent(t *testing.T) {
	mock, err := newMockClamdInfected()
	if err != nil {
		t.Fatal(err)
	}
	defer mock.close()
	go mock.serve()

	cfg := DefaultScannerConfig()
	cfg.Address = mock.address()
	s := NewScanner(cfg)
	hook := NewScanHook(s)

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	clean, err := hook.ScanToolInvocation(ctx, "write_file", map[string]interface{}{
		"path":    "/tmp/malware.exe",
		"content": "malicious content",
	})
	if clean {
		t.Error("expected infected content to be blocked")
	}
	if err == nil {
		t.Error("expected error for infected content")
	}
}

// --- scanExecArgs ---

func TestScanHook_scanExecArgs_WithPath(t *testing.T) {
	// This tests scanExecArgs indirectly through ScanToolInvocation
	cfg := DefaultScannerConfig()
	cfg.ScanOnExec = true
	s := NewScanner(cfg)
	hook := NewScanHook(s)

	ctx := context.Background()

	// Exec with command that has no path separators => extractExecutablePath returns ""
	// So it should return clean
	clean, err := hook.ScanToolInvocation(ctx, "exec", map[string]interface{}{
		"command": "ls -la",
	})
	if !clean || err != nil {
		t.Errorf("expected clean for bare command; got clean=%v, err=%v", clean, err)
	}
}

func TestScanHook_scanExecArgs_NonexistentPath(t *testing.T) {
	cfg := DefaultScannerConfig()
	cfg.ScanOnExec = true
	s := NewScanner(cfg)
	hook := NewScanHook(s)

	ctx := context.Background()

	if runtime.GOOS == "windows" {
		clean, err := hook.ScanToolInvocation(ctx, "exec", map[string]interface{}{
			"command": `C:\nonexistent\program.exe --flag`,
		})
		if !clean || err != nil {
			t.Errorf("expected clean for nonexistent path; got clean=%v, err=%v", clean, err)
		}
	} else {
		clean, err := hook.ScanToolInvocation(ctx, "exec", map[string]interface{}{
			"command": "/nonexistent/path/program --flag",
		})
		if !clean || err != nil {
			t.Errorf("expected clean for nonexistent path; got clean=%v, err=%v", clean, err)
		}
	}
}

// --- parseScanResponse additional cases ---

func TestParseScanResponse_ERROR(t *testing.T) {
	result := parseScanResponse("/tmp/test.exe: lstat error ERROR")
	if result.Infected {
		t.Error("ERROR response should not be infected")
	}
}

func TestParseScanResponse_FoundNoPath(t *testing.T) {
	result := parseScanResponse("Win.Trojan.Test FOUND")
	if !result.Infected {
		t.Error("should be infected")
	}
	if result.Virus != "Win.Trojan.Test" {
		t.Errorf("expected virus Win.Trojan.Test, got %s", result.Virus)
	}
}

func TestParseScanResponse_OK(t *testing.T) {
	result := parseScanResponse("/tmp/test.exe: OK")
	if result.Infected {
		t.Error("OK should not be infected")
	}
	if result.Path != "/tmp/test.exe" {
		t.Errorf("expected path /tmp/test.exe, got %s", result.Path)
	}
}

// --- Client.Stats ---

func TestClient_Stats(t *testing.T) {
	mock, err := newMockClamd()
	if err != nil {
		t.Fatal(err)
	}
	defer mock.close()
	go mock.serve()

	client := NewClient(mock.address())
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	stats, err := client.Stats(ctx)
	if err != nil {
		t.Fatalf("Stats failed: %v", err)
	}
	if !strings.Contains(stats, "POOLS") {
		t.Errorf("unexpected stats: %s", stats)
	}
}

// --- Client.ConnectionRefused for various commands ---

func TestClient_ConnectionRefused_Version(t *testing.T) {
	client := NewClient("127.0.0.1:1")
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()

	_, err := client.Version(ctx)
	if err == nil {
		t.Error("expected connection error")
	}
}

func TestClient_ConnectionRefused_ScanFile(t *testing.T) {
	client := NewClient("127.0.0.1:1")
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()

	_, err := client.ScanFile(ctx, "/tmp/test.exe")
	if err == nil {
		t.Error("expected connection error")
	}
}

func TestClient_ConnectionRefused_ContScan(t *testing.T) {
	client := NewClient("127.0.0.1:1")
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()

	_, err := client.ContScan(ctx, "/tmp")
	if err == nil {
		t.Error("expected connection error")
	}
}

func TestClient_ConnectionRefused_Reload(t *testing.T) {
	client := NewClient("127.0.0.1:1")
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()

	err := client.Reload(ctx)
	if err == nil {
		t.Error("expected connection error")
	}
}

// --- Config validation ---

func TestGenerateClamdConfig_NoConfigPath(t *testing.T) {
	err := GenerateClamdConfig(&DaemonConfig{})
	if err == nil {
		t.Error("expected error for empty config file path")
	}
}

func TestGenerateFreshclamConfig_NoConfigPath(t *testing.T) {
	err := GenerateFreshclamConfig("", "")
	if err == nil {
		t.Error("expected error for empty config file path")
	}
}

func TestGenerateClamdConfig_WithAllOptions(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := &DaemonConfig{
		ClamAVPath:  "/usr/bin",
		ConfigFile:  filepath.Join(tmpDir, "clamd.conf"),
		DatabaseDir: filepath.Join(tmpDir, "db"),
		ListenAddr:  "0.0.0.0:9310",
		TempDir:     filepath.Join(tmpDir, "tmp"),
	}

	if err := GenerateClamdConfig(cfg); err != nil {
		t.Fatalf("GenerateClamdConfig failed: %v", err)
	}

	content, err := os.ReadFile(cfg.ConfigFile)
	if err != nil {
		t.Fatalf("Failed to read config: %v", err)
	}

	str := string(content)
	if !strings.Contains(str, "TCPSocket 9310") {
		t.Error("Config should contain TCPSocket 9310")
	}
	if !strings.Contains(str, "TCPAddr 0.0.0.0") {
		t.Error("Config should contain TCPAddr 0.0.0.0")
	}
	if !strings.Contains(str, "DatabaseDirectory") {
		t.Error("Config should contain DatabaseDirectory")
	}
	if !strings.Contains(str, "TemporaryDirectory") {
		t.Error("Config should contain TemporaryDirectory")
	}
	if !strings.Contains(str, "ScanPE yes") {
		t.Error("Config should contain ScanPE")
	}
}

// --- NewScannerWithClient ---

func TestNewScannerWithClient(t *testing.T) {
	client := NewClient("127.0.0.1:3310")
	cfg := DefaultScannerConfig()
	s := NewScannerWithClient(client, cfg)

	if s.client != client {
		t.Error("client mismatch")
	}
	if s.config != cfg {
		t.Error("config mismatch")
	}
}

// --- Scanner.Ping ---

func TestScanner_Ping_Success(t *testing.T) {
	mock, err := newMockClamd()
	if err != nil {
		t.Fatal(err)
	}
	defer mock.close()
	go mock.serve()

	cfg := DefaultScannerConfig()
	cfg.Address = mock.address()
	s := NewScanner(cfg)

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := s.Ping(ctx); err != nil {
		t.Errorf("Ping failed: %v", err)
	}
}

// --- Manager.Start already started (disabled case) ---

func TestManager_Start_DisabledIdempotent(t *testing.T) {
	m := NewManager(&ManagerConfig{Enabled: false})
	ctx := context.Background()

	// Multiple starts with disabled config should succeed
	if err := m.Start(ctx); err != nil {
		t.Fatalf("first start failed: %v", err)
	}
	if err := m.Start(ctx); err != nil {
		t.Fatalf("second start (disabled) should also succeed: %v", err)
	}
}

// --- Daemon.Stop when not running ---

func TestDaemon_Stop_NotRunning(t *testing.T) {
	d := NewDaemon(&DaemonConfig{})
	if err := d.Stop(); err != nil {
		t.Errorf("expected nil error for stop on non-running daemon, got %v", err)
	}
}

// --- Daemon.Start with missing executable ---

func TestDaemon_Start_MissingExecutable(t *testing.T) {
	d := NewDaemon(&DaemonConfig{
		ClamAVPath:  "/nonexistent/path",
		ConfigFile:  "/nonexistent/clamd.conf",
		ListenAddr:  "127.0.0.1:3310",
	})
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	err := d.Start(ctx)
	if err == nil {
		t.Error("expected error for missing executable")
	}
}

// --- Daemon.Start already running ---

func TestDaemon_Start_AlreadyRunning(t *testing.T) {
	d := NewDaemon(&DaemonConfig{})
	d.running = true // Force running state

	ctx := context.Background()
	err := d.Start(ctx)
	if err == nil {
		t.Error("expected error when starting already-running daemon")
	}
	if !strings.Contains(err.Error(), "already running") {
		t.Errorf("unexpected error: %v", err)
	}
}

// --- NewClient ---

func TestNewClient(t *testing.T) {
	client := NewClient("127.0.0.1:3310")
	if client.Address != "127.0.0.1:3310" {
		t.Errorf("expected address 127.0.0.1:3310, got %s", client.Address)
	}
	if client.Timeout != 30*time.Second {
		t.Errorf("expected default timeout 30s, got %v", client.Timeout)
	}
}

func TestNewClientWithTimeout(t *testing.T) {
	client := NewClientWithTimeout("127.0.0.1:3310", 10*time.Second)
	if client.Timeout != 10*time.Second {
		t.Errorf("expected timeout 10s, got %v", client.Timeout)
	}
}

// --- parseMultiScanResponse edge cases ---

func TestParseMultiScanResponse_SingleResult(t *testing.T) {
	raw := "/tmp/test.exe: OK"
	results := parseMultiScanResponse(raw)
	if len(results) != 1 {
		t.Fatalf("expected 1 result, got %d", len(results))
	}
	if results[0].Path != "/tmp/test.exe" {
		t.Errorf("unexpected path: %s", results[0].Path)
	}
}

func TestParseMultiScanResponse_MultipleInfected(t *testing.T) {
	raw := "/tmp/a.exe: Trojan.A FOUND\n/tmp/b.exe: Trojan.B FOUND\n/tmp/c.exe: OK"
	results := parseMultiScanResponse(raw)
	if len(results) != 3 {
		t.Fatalf("expected 3 results, got %d", len(results))
	}
	if !results[0].Infected || results[0].Virus != "Trojan.A" {
		t.Errorf("unexpected first result: %+v", results[0])
	}
	if !results[1].Infected || results[1].Virus != "Trojan.B" {
		t.Errorf("unexpected second result: %+v", results[1])
	}
	if results[2].Infected {
		t.Error("third result should be clean")
	}
}

// --- Updater.StartAutoUpdate with zero interval ---

func TestUpdater_StartAutoUpdate_ZeroInterval(t *testing.T) {
	u := NewUpdater(&UpdaterConfig{UpdateInterval: 0})
	ctx := context.Background()
	// Should return immediately
	done := make(chan struct{})
	go func() {
		u.StartAutoUpdate(ctx)
		close(done)
	}()
	select {
	case <-done:
		// Good - returned immediately
	case <-time.After(2 * time.Second):
		t.Error("StartAutoUpdate should return immediately with zero interval")
	}
}

// --- Updater.Update with missing freshclam ---

func TestUpdater_Update_MissingFreshclam(t *testing.T) {
	u := NewUpdater(&UpdaterConfig{
		ClamAVPath: "/nonexistent/path",
	})
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	err := u.Update(ctx)
	if err == nil {
		t.Error("expected error for missing freshclam")
	}
}

// --- Manager.Start no ClamAV path and not installed ---

func TestManager_Start_NoClamAVPath(t *testing.T) {
	m := NewManager(&ManagerConfig{
		Enabled:   true,
		ClamAVPath: "/nonexistent/path/that/does/not/exist",
	})
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	err := m.Start(ctx)
	if err == nil {
		t.Error("expected error when ClamAV path is invalid")
	}
}
