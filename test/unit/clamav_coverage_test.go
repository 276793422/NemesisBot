// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package clamav_test

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

	. "github.com/276793422/NemesisBot/module/security/scanner/clamav"
)

// ---------------------------------------------------------------------------
// Mock clamd servers (replicated here because they live in the internal
// client_test.go package and are inaccessible from this external test package).
// ---------------------------------------------------------------------------

// testMockClamd is a clean-response mock clamd server.
type testMockClamd struct {
	listener net.Listener
}

func newTestMockClamd() (*testMockClamd, error) {
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return nil, err
	}
	return &testMockClamd{listener: l}, nil
}

func (m *testMockClamd) address() string { return m.listener.Addr().String() }

func (m *testMockClamd) serve() {
	for {
		conn, err := m.listener.Accept()
		if err != nil {
			return
		}
		go m.handleConn(conn)
	}
}

func (m *testMockClamd) handleConn(conn net.Conn) {
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
		case line == "VERSION":
			fmt.Fprintf(conn, "ClamAV 1.5.2\n")
		case line == "RELOAD":
			fmt.Fprintf(conn, "RELOADING\n")
		case strings.HasPrefix(line, "SCAN "):
			path := strings.TrimPrefix(line, "SCAN ")
			fmt.Fprintf(conn, "%s: OK\n", path)
		case strings.HasPrefix(line, "CONTSCAN "):
			path := strings.TrimPrefix(line, "CONTSCAN ")
			fmt.Fprintf(conn, "%s: OK\n", path)
		case line == "STATS":
			fmt.Fprintf(conn, "POOLS: 1\n")
		case line == "INSTREAM":
			for {
				lenBuf := make([]byte, 4)
				if _, err := io.ReadFull(reader, lenBuf); err != nil {
					return
				}
				cl := int(lenBuf[0])<<24 | int(lenBuf[1])<<16 | int(lenBuf[2])<<8 | int(lenBuf[3])
				if cl == 0 {
					break
				}
				data := make([]byte, cl)
				if _, err := io.ReadFull(reader, data); err != nil {
					return
				}
			}
			conn.Write([]byte("stream: OK\n"))
		}
	}
}

func (m *testMockClamd) close() { m.listener.Close() }

// testMockClamdInfected returns infected scan results.
type testMockClamdInfected struct {
	listener net.Listener
}

func newTestMockClamdInfected() (*testMockClamdInfected, error) {
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return nil, err
	}
	return &testMockClamdInfected{listener: l}, nil
}

func (m *testMockClamdInfected) address() string { return m.listener.Addr().String() }

func (m *testMockClamdInfected) serve() {
	for {
		conn, err := m.listener.Accept()
		if err != nil {
			return
		}
		go m.handleConn(conn)
	}
}

func (m *testMockClamdInfected) handleConn(conn net.Conn) {
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
			fmt.Fprintf(conn, "%s: Win.Trojan.Mock FOUND\n", path)
		case strings.HasPrefix(line, "CONTSCAN "):
			path := strings.TrimPrefix(line, "CONTSCAN ")
			fmt.Fprintf(conn, "%s: Win.Trojan.Mock FOUND\n", path)
		case line == "INSTREAM":
			for {
				lenBuf := make([]byte, 4)
				if _, err := io.ReadFull(reader, lenBuf); err != nil {
					return
				}
				cl := int(lenBuf[0])<<24 | int(lenBuf[1])<<16 | int(lenBuf[2])<<8 | int(lenBuf[3])
				if cl == 0 {
					break
				}
				data := make([]byte, cl)
				if _, err := io.ReadFull(reader, data); err != nil {
					return
				}
			}
			conn.Write([]byte("stream: Win.Trojan.Mock FOUND\n"))
		}
	}
}

func (m *testMockClamdInfected) close() { m.listener.Close() }

// ---------------------------------------------------------------------------
// Priority 1: Exported types and methods (no mock needed)
// ---------------------------------------------------------------------------

func TestClamavScanResult_Clean(t *testing.T) {
	r := &ScanResult{Infected: false}
	if !r.Clean() {
		t.Error("non-infected result should be clean")
	}
	r2 := &ScanResult{Infected: true}
	if r2.Clean() {
		t.Error("infected result should not be clean")
	}
}

func TestClamavNewClient(t *testing.T) {
	c := NewClient("127.0.0.1:3310")
	if c.Address != "127.0.0.1:3310" {
		t.Errorf("address mismatch: %s", c.Address)
	}
	if c.Timeout != 30*time.Second {
		t.Errorf("timeout mismatch: %v", c.Timeout)
	}
}

func TestClamavNewClientWithTimeout(t *testing.T) {
	c := NewClientWithTimeout("127.0.0.1:3310", 10*time.Second)
	if c.Timeout != 10*time.Second {
		t.Errorf("timeout mismatch: %v", c.Timeout)
	}
}

func TestClamavDefaultScannerConfig(t *testing.T) {
	cfg := DefaultScannerConfig()
	if !cfg.Enabled {
		t.Error("should be enabled by default")
	}
	if cfg.Address != "127.0.0.1:3310" {
		t.Errorf("address mismatch: %s", cfg.Address)
	}
	if cfg.MaxFileSize != 50*1024*1024 {
		t.Errorf("max file size mismatch: %d", cfg.MaxFileSize)
	}
}

func TestClamavNewScanner(t *testing.T) {
	cfg := DefaultScannerConfig()
	s := NewScanner(cfg)
	if s == nil {
		t.Fatal("scanner should not be nil")
	}
}

func TestClamavNewScannerWithClient(t *testing.T) {
	client := NewClient("127.0.0.1:3310")
	cfg := DefaultScannerConfig()
	s := NewScannerWithClient(client, cfg)
	if s == nil {
		t.Fatal("scanner should not be nil")
	}
}

func TestClamavScanner_ShouldScan(t *testing.T) {
	cfg := DefaultScannerConfig()
	s := NewScanner(cfg)

	cases := map[string]bool{
		"write_file":  true,
		"edit_file":   true,
		"append_file": true,
		"download":    true,
		"exec":        true,
		"read_file":   false,
		"unknown":     false,
	}
	for op, want := range cases {
		if got := s.ShouldScan(op, ""); got != want {
			t.Errorf("ShouldScan(%q) = %v, want %v", op, got, want)
		}
	}
}

func TestClamavScanner_ShouldScanFile(t *testing.T) {
	cfg := DefaultScannerConfig()
	s := NewScanner(cfg)

	// Safe extensions
	for _, ext := range []string{"test.txt", "data.json", "conf.yaml", "data.xml"} {
		if s.ShouldScanFile(ext) {
			t.Errorf("safe file %q should not need scan", ext)
		}
	}
	// Executable extensions
	for _, ext := range []string{"program.exe", "script.bat", "lib.dll"} {
		if !s.ShouldScanFile(ext) {
			t.Errorf("exec file %q should need scan", ext)
		}
	}
}

func TestClamavScanner_DisabledShouldNotScan(t *testing.T) {
	cfg := DefaultScannerConfig()
	cfg.Enabled = false
	s := NewScanner(cfg)

	if s.ShouldScan("write_file", "test.exe") {
		t.Error("disabled scanner should not scan")
	}
	if s.ShouldScanFile("test.exe") {
		t.Error("disabled scanner should not scan files")
	}
}

func TestClamavFormatScanResult(t *testing.T) {
	tests := []struct {
		result *ScanResult
		want   string
	}{
		{nil, "no scan performed"},
		{&ScanResult{Path: "/tmp/a.txt"}, "CLEAN: /tmp/a.txt"},
		{&ScanResult{Path: "/tmp/b.exe", Infected: true, Virus: "Trojan.X"}, "INFECTED: /tmp/b.exe (virus: Trojan.X)"},
	}
	for _, tt := range tests {
		got := FormatScanResult(tt.result)
		if got != tt.want {
			t.Errorf("FormatScanResult() = %q, want %q", got, tt.want)
		}
	}
}

func TestClamavNewDaemon(t *testing.T) {
	d := NewDaemon(&DaemonConfig{})
	if d == nil {
		t.Fatal("daemon should not be nil")
	}
}

func TestClamavDaemon_IsRunning(t *testing.T) {
	d := NewDaemon(&DaemonConfig{})
	if d.IsRunning() {
		t.Error("new daemon should not be running")
	}
}

func TestClamavDaemon_Client(t *testing.T) {
	d := NewDaemon(&DaemonConfig{ListenAddr: "127.0.0.1:3310"})
	if d.Client() == nil {
		t.Error("daemon should have a client")
	}
}

func TestClamavNewUpdater(t *testing.T) {
	u := NewUpdater(&UpdaterConfig{})
	if u == nil {
		t.Fatal("updater should not be nil")
	}
}

func TestClamavUpdater_LastUpdate(t *testing.T) {
	u := NewUpdater(&UpdaterConfig{})
	if !u.LastUpdate().IsZero() {
		t.Error("new updater should have zero last update")
	}
}

func TestClamavNewManager(t *testing.T) {
	m := NewManager(&ManagerConfig{})
	if m == nil {
		t.Fatal("manager should not be nil")
	}
}

func TestClamavManager_Hook_Nil(t *testing.T) {
	m := NewManager(&ManagerConfig{})
	if m.Hook() != nil {
		t.Error("unstarted manager should have nil hook")
	}
}

func TestClamavManager_Scanner_Nil(t *testing.T) {
	m := NewManager(&ManagerConfig{})
	if m.Scanner() != nil {
		t.Error("unstarted manager should have nil scanner")
	}
}

func TestClamavManager_IsRunning_NotStarted(t *testing.T) {
	m := NewManager(&ManagerConfig{})
	if m.IsRunning() {
		t.Error("unstarted manager should not be running")
	}
}

func TestClamavManager_Disabled(t *testing.T) {
	m := NewManager(&ManagerConfig{Enabled: false})
	ctx := context.Background()
	if err := m.Start(ctx); err != nil {
		t.Fatalf("disabled start should succeed: %v", err)
	}
	if m.IsRunning() {
		t.Error("disabled manager should not be running")
	}
}

func TestClamavManager_GetStats(t *testing.T) {
	m := NewManager(&ManagerConfig{Enabled: true})
	stats := m.GetStats()
	if stats["enabled"] != true {
		t.Error("stats should show enabled=true")
	}
}

func TestClamavManager_Stop_NotStarted(t *testing.T) {
	m := NewManager(&ManagerConfig{})
	if err := m.Stop(); err != nil {
		t.Errorf("stop on unstarted manager should succeed: %v", err)
	}
}

func TestClamavNewScanHook(t *testing.T) {
	cfg := DefaultScannerConfig()
	s := NewScanner(cfg)
	h := NewScanHook(s)
	if h == nil {
		t.Fatal("hook should not be nil")
	}
}

func TestClamavScanHook_GetScanner(t *testing.T) {
	cfg := DefaultScannerConfig()
	s := NewScanner(cfg)
	h := NewScanHook(s)
	if h.GetScanner() != s {
		t.Error("GetScanner should return the same scanner")
	}
}

func TestClamavScanHook_GetScanner_Nil(t *testing.T) {
	h := NewScanHook(nil)
	if h.GetScanner() != nil {
		t.Error("expected nil scanner")
	}
}

func TestClamavScanHook_Disabled(t *testing.T) {
	cfg := DefaultScannerConfig()
	cfg.Enabled = false
	s := NewScanner(cfg)
	h := NewScanHook(s)

	ctx := context.Background()
	clean, err := h.ScanToolInvocation(ctx, "write_file", map[string]interface{}{
		"path": "/tmp/test.txt", "content": "hello",
	})
	if !clean || err != nil {
		t.Errorf("disabled hook should return clean; got clean=%v err=%v", clean, err)
	}
}

func TestClamavScanHook_NilScanner(t *testing.T) {
	h := NewScanHook(nil)
	ctx := context.Background()
	clean, err := h.ScanToolInvocation(ctx, "write_file", nil)
	if !clean || err != nil {
		t.Errorf("nil scanner hook should return clean; got clean=%v err=%v", clean, err)
	}
}

func TestClamavScanHook_HealthCheck_Nil(t *testing.T) {
	h := NewScanHook(nil)
	err := h.HealthCheck(context.Background())
	if err == nil {
		t.Error("expected error for nil scanner")
	}
}

func TestClamavScanHook_HealthCheck_ServerDown(t *testing.T) {
	cfg := DefaultScannerConfig()
	cfg.Address = "127.0.0.1:1"
	s := NewScanner(cfg)
	h := NewScanHook(s)
	err := h.HealthCheck(context.Background())
	if err == nil {
		t.Error("expected error for server down")
	}
}

func TestClamavGenerateClamdConfig(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := &DaemonConfig{
		ClamAVPath:  "/usr/bin",
		ConfigFile:  filepath.Join(tmpDir, "clamd.conf"),
		DatabaseDir: filepath.Join(tmpDir, "db"),
		ListenAddr:  "127.0.0.1:3310",
		TempDir:     filepath.Join(tmpDir, "tmp"),
	}
	if err := GenerateClamdConfig(cfg); err != nil {
		t.Fatalf("GenerateClamdConfig failed: %v", err)
	}
	content, _ := os.ReadFile(cfg.ConfigFile)
	str := string(content)
	if !strings.Contains(str, "TCPSocket 3310") {
		t.Error("should contain TCPSocket")
	}
}

func TestClamavGenerateClamdConfig_NoPath(t *testing.T) {
	err := GenerateClamdConfig(&DaemonConfig{})
	if err == nil {
		t.Error("expected error for empty config file path")
	}
}

func TestClamavGenerateFreshclamConfig(t *testing.T) {
	tmpDir := t.TempDir()
	cfgFile := filepath.Join(tmpDir, "freshclam.conf")
	dbDir := filepath.Join(tmpDir, "db")
	if err := GenerateFreshclamConfig(dbDir, cfgFile); err != nil {
		t.Fatalf("GenerateFreshclamConfig failed: %v", err)
	}
	content, _ := os.ReadFile(cfgFile)
	if !strings.Contains(string(content), "DatabaseDirectory") {
		t.Error("should contain DatabaseDirectory")
	}
}

func TestClamavGenerateFreshclamConfig_NoPath(t *testing.T) {
	err := GenerateFreshclamConfig("", "")
	if err == nil {
		t.Error("expected error for empty config file path")
	}
}

func TestClamavDetectClamAVPath(t *testing.T) {
	// Just ensure it doesn't panic
	_ = DetectClamAVPath()
}

// ---------------------------------------------------------------------------
// Priority 2: Tests using mock clamd server
// ---------------------------------------------------------------------------

func TestClamavScanner_ScanFile_Clean(t *testing.T) {
	mock, err := newTestMockClamd()
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

	tmpFile := filepath.Join(t.TempDir(), "test.exe")
	os.WriteFile(tmpFile, []byte("fake"), 0644)

	result, err := s.ScanFile(ctx, tmpFile)
	if err != nil {
		t.Fatalf("ScanFile failed: %v", err)
	}
	if result.Infected {
		t.Error("expected clean")
	}
}

func TestClamavScanner_ScanFile_Infected(t *testing.T) {
	mock, err := newTestMockClamdInfected()
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
	os.WriteFile(tmpFile, []byte("malicious"), 0644)

	result, err := s.ScanFile(ctx, tmpFile)
	if err != nil {
		t.Fatalf("ScanFile failed: %v", err)
	}
	if !result.Infected {
		t.Error("expected infected")
	}
}

func TestClamavScanner_ScanContentBytes_Clean(t *testing.T) {
	mock, err := newTestMockClamd()
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
		t.Error("expected clean")
	}
}

func TestClamavScanner_ScanContentBytes_Infected(t *testing.T) {
	mock, err := newTestMockClamdInfected()
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

	result, err := s.ScanContentBytes(ctx, []byte("malicious"))
	if err != nil {
		t.Fatalf("ScanContentBytes failed: %v", err)
	}
	if !result.Infected {
		t.Error("expected infected")
	}
}

func TestClamavScanner_ScanContent_Disabled(t *testing.T) {
	cfg := DefaultScannerConfig()
	cfg.Enabled = false
	s := NewScanner(cfg)

	result, err := s.ScanContent(context.Background(), strings.NewReader("x"), 1)
	if err != nil {
		t.Fatal(err)
	}
	if result.Raw != "scanning disabled" {
		t.Errorf("unexpected raw: %s", result.Raw)
	}
}

func TestClamavScanner_ScanDirectory(t *testing.T) {
	mock, err := newTestMockClamd()
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

func TestClamavScanner_Ping(t *testing.T) {
	mock, err := newTestMockClamd()
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

func TestClamavScanHook_ScanToolInvocation_WriteFile(t *testing.T) {
	mock, err := newTestMockClamd()
	if err != nil {
		t.Fatal(err)
	}
	defer mock.close()
	go mock.serve()

	cfg := DefaultScannerConfig()
	cfg.Address = mock.address()
	s := NewScanner(cfg)
	h := NewScanHook(s)

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	clean, err := h.ScanToolInvocation(ctx, "write_file", map[string]interface{}{
		"path": "/tmp/test.exe", "content": "hello",
	})
	if !clean {
		t.Errorf("expected clean; err=%v", err)
	}
}

func TestClamavScanHook_ScanToolInvocation_EditFile(t *testing.T) {
	mock, err := newTestMockClamd()
	if err != nil {
		t.Fatal(err)
	}
	defer mock.close()
	go mock.serve()

	cfg := DefaultScannerConfig()
	cfg.Address = mock.address()
	s := NewScanner(cfg)
	h := NewScanHook(s)

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	clean, err := h.ScanToolInvocation(ctx, "edit_file", map[string]interface{}{
		"path": "/tmp/test.exe", "content": "old", "new_text": "new",
	})
	if !clean {
		t.Errorf("expected clean; err=%v", err)
	}
}

func TestClamavScanHook_ScanToolInvocation_UnknownTool(t *testing.T) {
	cfg := DefaultScannerConfig()
	s := NewScanner(cfg)
	h := NewScanHook(s)

	clean, err := h.ScanToolInvocation(context.Background(), "read_file", nil)
	if !clean || err != nil {
		t.Errorf("unknown tool should return clean; got clean=%v err=%v", clean, err)
	}
}

func TestClamavScanHook_ScanFilePath_Nonexistent(t *testing.T) {
	cfg := DefaultScannerConfig()
	s := NewScanner(cfg)
	h := NewScanHook(s)

	clean, result, err := h.ScanFilePath(context.Background(), "/nonexistent/file.exe")
	if !clean || err != nil || result != nil {
		t.Errorf("nonexistent file should be clean; clean=%v err=%v result=%v", clean, err, result)
	}
}

func TestClamavScanHook_ScanFilePath_SafeExtension(t *testing.T) {
	cfg := DefaultScannerConfig()
	s := NewScanner(cfg)
	h := NewScanHook(s)

	tmpFile := filepath.Join(t.TempDir(), "test.txt")
	os.WriteFile(tmpFile, []byte("hello"), 0644)

	clean, result, err := h.ScanFilePath(context.Background(), tmpFile)
	if !clean || err != nil || result != nil {
		t.Errorf("safe extension should be clean; clean=%v err=%v result=%v", clean, err, result)
	}
}

func TestClamavScanHook_ScanFilePath_WithMock(t *testing.T) {
	mock, err := newTestMockClamd()
	if err != nil {
		t.Fatal(err)
	}
	defer mock.close()
	go mock.serve()

	cfg := DefaultScannerConfig()
	cfg.Address = mock.address()
	s := NewScanner(cfg)
	h := NewScanHook(s)

	tmpFile := filepath.Join(t.TempDir(), "test.exe")
	os.WriteFile(tmpFile, []byte("fake"), 0644)

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	clean, result, err := h.ScanFilePath(ctx, tmpFile)
	if !clean {
		t.Errorf("expected clean; err=%v", err)
	}
	if result == nil {
		t.Fatal("expected non-nil result")
	}
}

func TestClamavScanHook_ScanDownloadedFile_Nonexistent(t *testing.T) {
	cfg := DefaultScannerConfig()
	s := NewScanner(cfg)
	h := NewScanHook(s)

	clean, result, err := h.ScanDownloadedFile(context.Background(), "/nonexistent/file.exe")
	if !clean || result != nil || err != nil {
		t.Errorf("nonexistent download should be clean; clean=%v result=%v err=%v", clean, result, err)
	}
}

func TestClamavScanHook_ScanDownloadedFile_Clean(t *testing.T) {
	mock, err := newTestMockClamd()
	if err != nil {
		t.Fatal(err)
	}
	defer mock.close()
	go mock.serve()

	cfg := DefaultScannerConfig()
	cfg.Address = mock.address()
	s := NewScanner(cfg)
	h := NewScanHook(s)

	tmpFile := filepath.Join(t.TempDir(), "download.exe")
	os.WriteFile(tmpFile, []byte("downloaded"), 0644)

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	clean, result, err := h.ScanDownloadedFile(ctx, tmpFile)
	if !clean {
		t.Errorf("expected clean; err=%v", err)
	}
	if result == nil {
		t.Fatal("expected result")
	}
	// File should still exist
	if _, statErr := os.Stat(tmpFile); statErr != nil {
		t.Errorf("clean file should still exist: %v", statErr)
	}
}

func TestClamavScanHook_ScanDownloadedFile_Infected(t *testing.T) {
	mock, err := newTestMockClamdInfected()
	if err != nil {
		t.Fatal(err)
	}
	defer mock.close()
	go mock.serve()

	cfg := DefaultScannerConfig()
	cfg.Address = mock.address()
	s := NewScanner(cfg)
	h := NewScanHook(s)

	tmpFile := filepath.Join(t.TempDir(), "infected.exe")
	os.WriteFile(tmpFile, []byte("malicious"), 0644)

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	clean, result, err := h.ScanDownloadedFile(ctx, tmpFile)
	if clean {
		t.Error("infected file should not be clean")
	}
	if result == nil || !result.Infected {
		t.Error("expected infected result")
	}
	// Infected file should be removed
	if _, statErr := os.Stat(tmpFile); !os.IsNotExist(statErr) {
		t.Error("infected file should have been removed")
	}
}

func TestClamavScanHook_HealthCheck_ServerUp(t *testing.T) {
	mock, err := newTestMockClamd()
	if err != nil {
		t.Fatal(err)
	}
	defer mock.close()
	go mock.serve()

	cfg := DefaultScannerConfig()
	cfg.Address = mock.address()
	s := NewScanner(cfg)
	h := NewScanHook(s)

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := h.HealthCheck(ctx); err != nil {
		t.Errorf("HealthCheck should succeed: %v", err)
	}
}

// --- Platform-specific Updater.findExecutable test ---

func TestClamavUpdater_FindExecutable(t *testing.T) {
	// We can't access findExecutable directly from this package,
	// but we can verify Update() fails with the expected path.
	u := NewUpdater(&UpdaterConfig{ClamAVPath: "/nonexistent"})
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	err := u.Update(ctx)
	if err == nil {
		t.Error("expected error for nonexistent path")
	}
	if runtime.GOOS == "windows" {
		if !strings.Contains(err.Error(), "freshclam.exe") {
			t.Errorf("error should mention freshclam.exe: %v", err)
		}
	} else {
		if !strings.Contains(err.Error(), "freshclam") {
			t.Errorf("error should mention freshclam: %v", err)
		}
	}
}

// --- Manager.Start with missing ClamAV ---

func TestClamavManager_Start_MissingClamAV(t *testing.T) {
	m := NewManager(&ManagerConfig{
		Enabled:    true,
		ClamAVPath: "/nonexistent/path",
	})
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	err := m.Start(ctx)
	if err == nil {
		t.Error("expected error for missing ClamAV")
	}
}

// --- Updater.IsDatabaseStale ---

func TestClamavUpdater_IsDatabaseStale_RecentUpdate(t *testing.T) {
	u := NewUpdater(&UpdaterConfig{})
	// Set lastUpdate via reflection-free approach: Update() sets it but needs freshclam.
	// Instead just test with zero lastUpdate.
	if !u.IsDatabaseStale(1 * time.Hour) {
		t.Error("zero lastUpdate should always be stale")
	}
}

// --- Client connection error tests ---

func TestClamavClient_ConnectionRefused_Ping(t *testing.T) {
	c := NewClient("127.0.0.1:1")
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	if err := c.Ping(ctx); err == nil {
		t.Error("expected connection error")
	}
}

func TestClamavClient_ConnectionRefused_Version(t *testing.T) {
	c := NewClient("127.0.0.1:1")
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	if _, err := c.Version(ctx); err == nil {
		t.Error("expected connection error")
	}
}

func TestClamavClient_ConnectionRefused_ScanFile(t *testing.T) {
	c := NewClient("127.0.0.1:1")
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	if _, err := c.ScanFile(ctx, "/tmp/test.exe"); err == nil {
		t.Error("expected connection error")
	}
}

func TestClamavClient_ConnectionRefused_ScanStream(t *testing.T) {
	c := NewClient("127.0.0.1:1")
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	if _, err := c.ScanStream(ctx, strings.NewReader("test")); err == nil {
		t.Error("expected connection error")
	}
}

func TestClamavClient_ContextCancelled(t *testing.T) {
	mock, err := newTestMockClamd()
	if err != nil {
		t.Fatal(err)
	}
	defer mock.close()
	go mock.serve()

	c := NewClient(mock.address())
	ctx, cancel := context.WithCancel(context.Background())
	cancel() // cancel immediately

	if err := c.Ping(ctx); err == nil {
		t.Error("expected cancellation error")
	}
}

func TestClamavScanner_ScanFile_Disabled(t *testing.T) {
	cfg := DefaultScannerConfig()
	cfg.Enabled = false
	s := NewScanner(cfg)

	result, err := s.ScanFile(context.Background(), "test.txt")
	if err != nil {
		t.Fatal(err)
	}
	if result == nil {
		t.Fatal("expected non-nil result")
	}
	if result.Raw != "scanning disabled" {
		t.Errorf("unexpected raw: %s", result.Raw)
	}
}

func TestClamavScanner_ScanFile_TooLarge(t *testing.T) {
	cfg := DefaultScannerConfig()
	cfg.MaxFileSize = 10
	s := NewScanner(cfg)

	tmpFile := filepath.Join(t.TempDir(), "big.exe")
	os.WriteFile(tmpFile, []byte("this is more than ten bytes"), 0644)

	result, err := s.ScanFile(context.Background(), tmpFile)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(result.Raw, "file too large") {
		t.Errorf("expected too-large message: %s", result.Raw)
	}
}

func TestClamavScanner_ScanFile_Nonexistent(t *testing.T) {
	cfg := DefaultScannerConfig()
	s := NewScanner(cfg)
	_, err := s.ScanFile(context.Background(), "/nonexistent/test.exe")
	if err == nil {
		t.Error("expected error for nonexistent file")
	}
}

func TestClamavScanner_ScanDirectory_Disabled(t *testing.T) {
	cfg := DefaultScannerConfig()
	cfg.Enabled = false
	s := NewScanner(cfg)

	results, err := s.ScanDirectory(context.Background(), "/tmp")
	if err != nil || results != nil {
		t.Errorf("disabled scan dir should return nil; err=%v results=%v", err, results)
	}
}

func TestClamavScanHook_ScanToolInvocation_WriteOff(t *testing.T) {
	cfg := DefaultScannerConfig()
	cfg.ScanOnWrite = false
	s := NewScanner(cfg)
	h := NewScanHook(s)

	clean, err := h.ScanToolInvocation(context.Background(), "write_file", map[string]interface{}{"path": "/tmp/test.exe"})
	if !clean || err != nil {
		t.Errorf("ScanOnWrite off should skip; clean=%v err=%v", clean, err)
	}
}

func TestClamavScanHook_ScanToolInvocation_DownloadOff(t *testing.T) {
	cfg := DefaultScannerConfig()
	cfg.ScanOnDownload = false
	s := NewScanner(cfg)
	h := NewScanHook(s)

	clean, err := h.ScanToolInvocation(context.Background(), "download", map[string]interface{}{"save_path": "/tmp/test.exe"})
	if !clean || err != nil {
		t.Errorf("ScanOnDownload off should skip; clean=%v err=%v", clean, err)
	}
}

func TestClamavScanHook_ScanToolInvocation_ExecOff(t *testing.T) {
	cfg := DefaultScannerConfig()
	cfg.ScanOnExec = false
	s := NewScanner(cfg)
	h := NewScanHook(s)

	clean, err := h.ScanToolInvocation(context.Background(), "exec", map[string]interface{}{"command": "ls"})
	if !clean || err != nil {
		t.Errorf("ScanOnExec off should skip; clean=%v err=%v", clean, err)
	}
}

func TestClamavDaemon_Start_MissingExecutable(t *testing.T) {
	d := NewDaemon(&DaemonConfig{
		ClamAVPath: "/nonexistent",
		ConfigFile: "/nonexistent/clamd.conf",
	})
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	if err := d.Start(ctx); err == nil {
		t.Error("expected error for missing executable")
	}
}

func TestClamavDaemon_Stop_NotRunning(t *testing.T) {
	d := NewDaemon(&DaemonConfig{})
	if err := d.Stop(); err != nil {
		t.Errorf("stopping non-running daemon should succeed: %v", err)
	}
}
