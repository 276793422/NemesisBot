// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Web Channel Integration Tests - REST API Endpoints

package web_test

import (
	"context"
	"encoding/json"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	. "github.com/276793422/NemesisBot/module/web"
)

// --- API Status Integration ---

func TestIntegrationAPIStatus(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)
	testBus := bus.NewMessageBus()
	dir := t.TempDir()

	server := NewServer(ServerConfig{
		Host:       "localhost",
		Port:       0,
		WSPath:     "/ws",
		SessionMgr: sessionMgr,
		Bus:        testBus,
		Workspace:  dir,
		Version:    "integration-1.0.0",
	})

	// Start server
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	errChan := make(chan error, 1)
	go func() {
		errChan <- server.Start(ctx)
	}()

	time.Sleep(200 * time.Millisecond)
	cancel()

	// Verify status endpoint via httptest
	req := httptest.NewRequest("GET", "/api/status", nil)
	w := httptest.NewRecorder()
	server.HandleAPIStatusForTest(w, req)

	if w.Code != 200 {
		t.Fatalf("expected 200, got %d: %s", w.Code, w.Body.String())
	}

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)

	if resp["version"] != "integration-1.0.0" {
		t.Errorf("version = %v, want integration-1.0.0", resp["version"])
	}
	if _, ok := resp["scanner_status"]; !ok {
		t.Error("missing scanner_status (extended field)")
	}
}

// --- API Logs Integration ---

func TestIntegrationAPILogs(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)
	testBus := bus.NewMessageBus()
	dir := t.TempDir()

	server := NewServer(ServerConfig{
		Host:       "localhost",
		Port:       0,
		WSPath:     "/ws",
		SessionMgr: sessionMgr,
		Bus:        testBus,
		Workspace:  dir,
		Version:    "test",
	})

	// Create a real log file
	logsDir := filepath.Join(dir, "logs")
	os.MkdirAll(logsDir, 0755)
	logFile := filepath.Join(logsDir, "nemesisbot.log")
	for i := 0; i < 5; i++ {
		line, _ := json.Marshal(map[string]string{
			"level":     "INFO",
			"timestamp": time.Now().Format(time.RFC3339),
			"component": "gateway",
			"message":   "integration test message",
		})
		f, _ := os.OpenFile(logFile, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0644)
		f.Write(append(line, '\n'))
		f.Close()
	}

	req := httptest.NewRequest("GET", "/api/logs?source=general&n=3", nil)
	w := httptest.NewRecorder()
	server.HandleAPILogsForTest(w, req)

	if w.Code != 200 {
		t.Fatalf("expected 200, got %d: %s", w.Code, w.Body.String())
	}

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)
	entries, _ := resp["entries"].([]interface{})
	if len(entries) != 3 {
		t.Errorf("entries = %d, want 3 (limit applied)", len(entries))
	}
}

// --- API Config Integration ---

func TestIntegrationAPIConfigSanitization(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)
	testBus := bus.NewMessageBus()
	dir := t.TempDir()

	server := NewServer(ServerConfig{
		Host:       "localhost",
		Port:       0,
		WSPath:     "/ws",
		SessionMgr: sessionMgr,
		Bus:        testBus,
		Workspace:  dir,
		Version:    "test",
	})

	// Create real config
	configDir := filepath.Join(dir, "config")
	os.MkdirAll(configDir, 0755)
	os.WriteFile(filepath.Join(configDir, "config.json"), []byte(`{
		"agents": {"model": "zhipu/glm-4.7"},
		"channels": {"web": {"auth_token": "super-secret-token-12345", "port": 49000}}
	}`), 0644)

	req := httptest.NewRequest("GET", "/api/config", nil)
	w := httptest.NewRecorder()
	server.HandleAPIConfigForTest(w, req)

	if w.Code != 200 {
		t.Fatalf("expected 200, got %d: %s", w.Code, w.Body.String())
	}

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)

	channels, _ := resp["channels"].(map[string]interface{})
	web, _ := channels["web"].(map[string]interface{})
	if web["auth_token"] == "super-secret-token-12345" {
		t.Error("auth_token should be masked!")
	}
	if web["port"] != float64(49000) {
		t.Errorf("port = %v, want 49000", web["port"])
	}
}

// --- API Scanner Status Integration ---

func TestIntegrationAPIScannerStatus(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)
	testBus := bus.NewMessageBus()
	dir := t.TempDir()

	server := NewServer(ServerConfig{
		Host:       "localhost",
		Port:       0,
		WSPath:     "/ws",
		SessionMgr: sessionMgr,
		Bus:        testBus,
		Workspace:  dir,
		Version:    "test",
	})

	// Create scanner config
	configDir := filepath.Join(dir, "config")
	os.MkdirAll(configDir, 0755)
	os.WriteFile(filepath.Join(configDir, "config.scanner.json"), []byte(`{
		"enabled": ["clamav"],
		"engines": {"clamav": {"url": "http://localhost:3310", "path": "/usr/bin/clamscan"}}
	}`), 0644)

	req := httptest.NewRequest("GET", "/api/scanner/status", nil)
	w := httptest.NewRecorder()
	server.HandleAPIScannerStatusForTest(w, req)

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)

	if resp["enabled"] != true {
		t.Errorf("scanner enabled = %v, want true", resp["enabled"])
	}
}

// --- SSE EventHub Integration ---

func TestIntegrationSSELogEvent(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)
	testBus := bus.NewMessageBus()
	dir := t.TempDir()

	server := NewServer(ServerConfig{
		Host:       "localhost",
		Port:       0,
		WSPath:     "/ws",
		SessionMgr: sessionMgr,
		Bus:        testBus,
		Workspace:  dir,
		Version:    "test",
	})

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	req := httptest.NewRequest("GET", "/api/events/stream", nil).WithContext(ctx)
	w := httptest.NewRecorder()

	done := make(chan struct{})
	go func() {
		server.HandleEventsStreamForTest(w, req)
		close(done)
	}()

	// Publish a log event
	time.Sleep(50 * time.Millisecond)
	server.GetEventHub().Publish("log", map[string]interface{}{
		"source":    "general",
		"level":     "ERROR",
		"component": "scanner",
		"message":   "SSE integration test",
	})
	time.Sleep(50 * time.Millisecond)
	cancel()

	<-done

	body := w.Body.String()
	if !contains(body, "event: log") {
		t.Errorf("expected log event in SSE stream, got: %s", body)
	}
	if !contains(body, "SSE integration test") {
		t.Errorf("expected log message content, got: %s", body)
	}
}

func TestIntegrationSSEStatusLoop(t *testing.T) {
	sessionMgr := NewSessionManager(1 * time.Hour)
	testBus := bus.NewMessageBus()
	dir := t.TempDir()

	server := NewServer(ServerConfig{
		Host:       "localhost",
		Port:       0,
		WSPath:     "/ws",
		SessionMgr: sessionMgr,
		Bus:        testBus,
		Workspace:  dir,
		Version:    "test-loop",
	})

	// Start status loop
	ctx, cancel := context.WithCancel(context.Background())
	go server.PublishStatusLoopForTest(ctx)

	// Subscribe and wait for events
	ch := server.GetEventHub().Subscribe()
	defer server.GetEventHub().Unsubscribe(ch)

	// Wait for a status event (5 second interval)
	select {
	case event := <-ch:
		if event.Type != "status" {
			t.Errorf("event type = %q, want 'status'", event.Type)
		}
	case <-time.After(8 * time.Second):
		t.Fatal("timed out waiting for status event")
	}
	cancel()
}

func contains(s, substr string) bool {
	for i := 0; i <= len(s)-len(substr); i++ {
		if s[i:i+len(substr)] == substr {
			return true
		}
	}
	return false
}
