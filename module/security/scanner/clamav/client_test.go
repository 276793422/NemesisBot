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
	"strings"
	"testing"
	"time"
)

// mockClamd is a minimal mock clamd server for testing
type mockClamd struct {
	listener net.Listener
	scans    []string
}

func newMockClamd() (*mockClamd, error) {
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return nil, err
	}
	return &mockClamd{listener: listener}, nil
}

func (m *mockClamd) address() string {
	return m.listener.Addr().String()
}

func (m *mockClamd) serve() {
	for {
		conn, err := m.listener.Accept()
		if err != nil {
			return
		}
		go m.handleConn(conn)
	}
}

func (m *mockClamd) handleConn(conn net.Conn) {
	defer conn.Close()
	reader := bufio.NewReader(conn)

	for {
		line, err := reader.ReadString('\n')
		if err != nil {
			return
		}
		line = strings.TrimSpace(line)
		// Strip the "n" prefix if present
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
			m.scans = append(m.scans, path)
			fmt.Fprintf(conn, "%s: OK\n", path)
		case strings.HasPrefix(line, "CONTSCAN "):
			path := strings.TrimPrefix(line, "CONTSCAN ")
			m.scans = append(m.scans, path)
			fmt.Fprintf(conn, "%s: OK\n", path)
		case line == "STATS":
			fmt.Fprintf(conn, "POOLS: 1\n")
		case line == "INSTREAM":
			m.handleInstream(reader, conn)
			return // INSTREAM uses the same connection; close after response
		case line == "STREAM":
			// Find a free port for data connection
			dataListener, _ := net.Listen("tcp", "127.0.0.1:0")
			_, port, _ := net.SplitHostPort(dataListener.Addr().String())
			fmt.Fprintf(conn, "PORT %s\n", port)
			go m.handleStream(dataListener, conn)
		}
	}
}

// handleInstream reads INSTREAM data on the same connection and responds.
func (m *mockClamd) handleInstream(reader io.Reader, conn net.Conn) {
	// Read INSTREAM data (4-byte length + data chunks, 0-length = end)
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
	// Send result
	conn.Write([]byte("stream: OK\n"))
}

func (m *mockClamd) handleStream(dataListener net.Listener, controlConn net.Conn) {
	defer dataListener.Close()

	dataConn, err := dataListener.Accept()
	if err != nil {
		return
	}
	defer dataConn.Close()

	// Read INSTREAM data (4-byte length + data chunks, 0-length = end)
	for {
		lenBuf := make([]byte, 4)
		if _, err := dataConn.Read(lenBuf); err != nil {
			return
		}
		chunkLen := int(lenBuf[0])<<24 | int(lenBuf[1])<<16 | int(lenBuf[2])<<8 | int(lenBuf[3])
		if chunkLen == 0 {
			break
		}
		data := make([]byte, chunkLen)
		if _, err := dataConn.Read(data); err != nil {
			return
		}
	}

	// Send result back on control connection
	controlConn.Write([]byte("stream: OK\n"))
}

func (m *mockClamd) close() {
	m.listener.Close()
}

// --- Tests ---

func TestClient_Ping(t *testing.T) {
	mock, err := newMockClamd()
	if err != nil {
		t.Fatal(err)
	}
	defer mock.close()
	go mock.serve()

	client := NewClient(mock.address())
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := client.Ping(ctx); err != nil {
		t.Fatalf("Ping failed: %v", err)
	}
}

func TestClient_Version(t *testing.T) {
	mock, err := newMockClamd()
	if err != nil {
		t.Fatal(err)
	}
	defer mock.close()
	go mock.serve()

	client := NewClient(mock.address())
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	version, err := client.Version(ctx)
	if err != nil {
		t.Fatalf("Version failed: %v", err)
	}
	if !strings.Contains(version, "ClamAV") {
		t.Errorf("Unexpected version: %s", version)
	}
}

func TestClient_ScanFile(t *testing.T) {
	mock, err := newMockClamd()
	if err != nil {
		t.Fatal(err)
	}
	defer mock.close()
	go mock.serve()

	client := NewClient(mock.address())
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	result, err := client.ScanFile(ctx, "/tmp/test.exe")
	if err != nil {
		t.Fatalf("ScanFile failed: %v", err)
	}
	if result.Infected {
		t.Error("Expected clean result")
	}
	if result.Path != "/tmp/test.exe" {
		t.Errorf("Unexpected path: %s", result.Path)
	}
}

func TestClient_ContScan(t *testing.T) {
	mock, err := newMockClamd()
	if err != nil {
		t.Fatal(err)
	}
	defer mock.close()
	go mock.serve()

	client := NewClient(mock.address())
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	results, err := client.ContScan(ctx, "/tmp")
	if err != nil {
		t.Fatalf("ContScan failed: %v", err)
	}
	if len(results) == 0 {
		t.Error("Expected at least one result")
	}
}

func TestClient_ScanStream(t *testing.T) {
	mock, err := newMockClamd()
	if err != nil {
		t.Fatal(err)
	}
	defer mock.close()
	go mock.serve()

	client := NewClient(mock.address())
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	content := strings.NewReader("test file content")
	result, err := client.ScanStream(ctx, content)
	if err != nil {
		t.Fatalf("ScanStream failed: %v", err)
	}
	if result.Infected {
		t.Error("Expected clean result")
	}
}

func TestClient_ConnectionRefused(t *testing.T) {
	client := NewClient("127.0.0.1:1") // Port 1 should not be listening
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()

	err := client.Ping(ctx)
	if err == nil {
		t.Error("Expected connection error")
	}
}

func TestClient_ContextCancellation(t *testing.T) {
	mock, err := newMockClamd()
	if err != nil {
		t.Fatal(err)
	}
	defer mock.close()
	go mock.serve()

	client := NewClient(mock.address())
	ctx, cancel := context.WithCancel(context.Background())
	cancel() // Cancel immediately

	err = client.Ping(ctx)
	if err == nil {
		t.Error("Expected context cancellation error")
	}
}

func TestClient_Reload(t *testing.T) {
	mock, err := newMockClamd()
	if err != nil {
		t.Fatal(err)
	}
	defer mock.close()
	go mock.serve()

	client := NewClient(mock.address())
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := client.Reload(ctx); err != nil {
		t.Fatalf("Reload failed: %v", err)
	}
}

// --- Scan Result Parsing Tests ---

func TestParseScanResponse_Clean(t *testing.T) {
	result := parseScanResponse("/tmp/test.exe: OK")
	if result.Infected {
		t.Error("Should not be infected")
	}
	if result.Path != "/tmp/test.exe" {
		t.Errorf("Expected path /tmp/test.exe, got %s", result.Path)
	}
}

func TestParseScanResponse_Infected(t *testing.T) {
	result := parseScanResponse("/tmp/test.exe: Win.Trojan.Agent-123 FOUND")
	if !result.Infected {
		t.Error("Should be infected")
	}
	if result.Virus != "Win.Trojan.Agent-123" {
		t.Errorf("Expected virus Win.Trojan.Agent-123, got %s", result.Virus)
	}
}

func TestParseScanResponse_Empty(t *testing.T) {
	result := parseScanResponse("")
	if result.Infected {
		t.Error("Should not be infected for empty response")
	}
}

// --- Scanner Tests ---

func TestScanner_ShouldScan(t *testing.T) {
	cfg := DefaultScannerConfig()
	s := NewScanner(cfg)

	tests := []struct {
		op   string
		want bool
	}{
		{"write_file", true},
		{"edit_file", true},
		{"append_file", true},
		{"download", true},
		{"exec", true},
		{"read_file", false},
		{"unknown", false},
	}

	for _, tt := range tests {
		got := s.ShouldScan(tt.op, "")
		if got != tt.want {
			t.Errorf("ShouldScan(%q) = %v, want %v", tt.op, got, tt.want)
		}
	}
}

func TestScanner_ShouldScanFile(t *testing.T) {
	cfg := DefaultScannerConfig()
	s := NewScanner(cfg)

	// Safe extensions should not be scanned
	if s.ShouldScanFile("readme.txt") {
		t.Error("txt files should not require scanning")
	}
	if s.ShouldScanFile("config.json") {
		t.Error("json files should not require scanning")
	}

	// Executable extensions should be scanned
	if !s.ShouldScanFile("program.exe") {
		t.Error("exe files should be scanned")
	}
	if !s.ShouldScanFile("script.bat") {
		t.Error("bat files should be scanned")
	}
}

func TestScanner_Disabled(t *testing.T) {
	cfg := DefaultScannerConfig()
	cfg.Enabled = false
	s := NewScanner(cfg)

	// Should not scan when disabled
	if s.ShouldScan("write_file", "test.exe") {
		t.Error("Should not scan when disabled")
	}
}

func TestScanner_Stats(t *testing.T) {
	cfg := DefaultScannerConfig()
	cfg.Enabled = false // Disabled so we don't need a real daemon
	s := NewScanner(cfg)

	// When disabled, scan returns early with "scanning disabled" result
	ctx := context.Background()
	result, _ := s.ScanFile(ctx, "nonexistent.txt")

	// The disabled scanner returns a result but doesn't update stats
	// because it returns early. Verify the result indicates disabled.
	if result == nil {
		t.Error("Expected non-nil result even when disabled")
	}

	stats := s.GetStats()
	// Stats should be 0 when disabled (early return)
	if stats.TotalScans != 0 {
		t.Errorf("Expected 0 scans when disabled, got %d", stats.TotalScans)
	}
}

// --- ScanHook Tests ---

func TestScanHook_DisabledScanner(t *testing.T) {
	cfg := DefaultScannerConfig()
	cfg.Enabled = false
	s := NewScanner(cfg)
	hook := NewScanHook(s)

	ctx := context.Background()
	clean, err := hook.ScanToolInvocation(ctx, "write_file", map[string]interface{}{
		"path":    "/tmp/test.txt",
		"content": "hello",
	})
	if !clean || err != nil {
		t.Errorf("Expected clean=true, err=nil when disabled; got clean=%v, err=%v", clean, err)
	}
}

// --- Config Generation Tests ---

func TestGenerateClamdConfig(t *testing.T) {
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

	content, err := os.ReadFile(cfg.ConfigFile)
	if err != nil {
		t.Fatalf("Failed to read config: %v", err)
	}

	str := string(content)
	if !strings.Contains(str, "TCPSocket 3310") {
		t.Error("Config should contain TCPSocket")
	}
	if !strings.Contains(str, "TCPAddr 127.0.0.1") {
		t.Error("Config should contain TCPAddr")
	}
	if !strings.Contains(str, "ScanPE yes") {
		t.Error("Config should contain ScanPE")
	}
}

func TestGenerateFreshclamConfig(t *testing.T) {
	tmpDir := t.TempDir()
	configFile := filepath.Join(tmpDir, "freshclam.conf")
	dbDir := filepath.Join(tmpDir, "db")

	if err := GenerateFreshclamConfig(dbDir, configFile); err != nil {
		t.Fatalf("GenerateFreshclamConfig failed: %v", err)
	}

	content, err := os.ReadFile(configFile)
	if err != nil {
		t.Fatalf("Failed to read config: %v", err)
	}

	str := string(content)
	if !strings.Contains(str, "DatabaseDirectory") {
		t.Error("Config should contain DatabaseDirectory")
	}
}

// --- Host Parsing Test ---

func TestHostPart(t *testing.T) {
	tests := []struct {
		addr string
		want string
	}{
		{"127.0.0.1:3310", "127.0.0.1"},
		{"192.168.1.1:8080", "192.168.1.1"},
		{"invalid", "invalid"},
	}

	for _, tt := range tests {
		got := hostPart(tt.addr)
		if got != tt.want {
			t.Errorf("hostPart(%q) = %q, want %q", tt.addr, got, tt.want)
		}
	}
}

// --- Multi-line response parsing ---

func TestParseMultiScanResponse(t *testing.T) {
	raw := "/tmp/a.exe: OK\n/tmp/b.exe: Win.Trojan.Test FOUND\n"
	results := parseMultiScanResponse(raw)
	if len(results) != 2 {
		t.Fatalf("Expected 2 results, got %d", len(results))
	}
	if results[0].Infected {
		t.Error("First result should be clean")
	}
	if !results[1].Infected {
		t.Error("Second result should be infected")
	}
	if results[1].Virus != "Win.Trojan.Test" {
		t.Errorf("Expected virus Win.Trojan.Test, got %s", results[1].Virus)
	}
}

func TestParseMultiScanResponse_EmptyLines(t *testing.T) {
	raw := "/tmp/a.exe: OK\n\n\n"
	results := parseMultiScanResponse(raw)
	if len(results) != 1 {
		t.Errorf("Expected 1 result (skip empty lines), got %d", len(results))
	}
}

// --- Manager Disabled Test ---

func TestManager_Disabled(t *testing.T) {
	mgr := NewManager(&ManagerConfig{
		Enabled: false,
	})
	ctx := context.Background()
	if err := mgr.Start(ctx); err != nil {
		t.Fatalf("Start with disabled should not error: %v", err)
	}
	if mgr.IsRunning() {
		t.Error("Disabled manager should not be running")
	}
}

// --- FormatScanResult Test ---

func TestFormatScanResult(t *testing.T) {
	tests := []struct {
		result *ScanResult
		want   string
	}{
		{nil, "no scan performed"},
		{&ScanResult{Path: "/tmp/a.txt", Infected: false}, "CLEAN: /tmp/a.txt"},
		{&ScanResult{Path: "/tmp/b.exe", Infected: true, Virus: "Trojan.Test"}, "INFECTED: /tmp/b.exe (virus: Trojan.Test)"},
	}

	for _, tt := range tests {
		got := FormatScanResult(tt.result)
		if got != tt.want {
			t.Errorf("FormatScanResult() = %q, want %q", got, tt.want)
		}
	}
}
