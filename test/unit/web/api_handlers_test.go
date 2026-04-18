// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Web Channel Unit Tests - API Handlers

package web_test

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"
	"time"

	. "github.com/276793422/NemesisBot/module/web"
)

// newTestServer creates a Server with a temporary workspace and session manager for testing.
func newTestServer(t *testing.T) (*Server, string) {
	t.Helper()
	dir := t.TempDir()
	s := NewServer(ServerConfig{
		Workspace:  dir,
		Version:    "test-1.0.0",
		SessionMgr: NewSessionManager(1 * time.Hour),
	})
	return s, dir
}

// --- handleAPIStatus ---

func TestHandleAPIStatus_BasicFields(t *testing.T) {
	s, _ := newTestServer(t)

	req := httptest.NewRequest("GET", "/api/status", nil)
	w := httptest.NewRecorder()
	s.HandleAPIStatusForTest(w, req)

	if w.Code != 200 {
		t.Fatalf("expected 200, got %d: %s", w.Code, w.Body.String())
	}

	var resp map[string]interface{}
	if err := json.Unmarshal(w.Body.Bytes(), &resp); err != nil {
		t.Fatalf("invalid JSON: %v", err)
	}

	if resp["version"] != "test-1.0.0" {
		t.Errorf("version = %v, want test-1.0.0", resp["version"])
	}
	if _, ok := resp["uptime_seconds"]; !ok {
		t.Error("missing uptime_seconds")
	}
	if _, ok := resp["ws_connected"]; !ok {
		t.Error("missing ws_connected")
	}
	if _, ok := resp["session_count"]; !ok {
		t.Error("missing session_count")
	}
}

func TestHandleAPIStatus_ExtendedFields(t *testing.T) {
	s, dir := newTestServer(t)

	// Create scanner config
	configDir := filepath.Join(dir, "config")
	os.MkdirAll(configDir, 0755)
	os.WriteFile(filepath.Join(configDir, "config.scanner.json"), []byte(`{
		"enabled": ["clamav"],
		"engines": {"clamav": {"url": "http://localhost:3310"}}
	}`), 0644)

	s.SetModelName("test-model")

	req := httptest.NewRequest("GET", "/api/status", nil)
	w := httptest.NewRecorder()
	s.HandleAPIStatusForTest(w, req)

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)

	if resp["model"] != "test-model" {
		t.Errorf("model = %v, want test-model", resp["model"])
	}
	scannerStatus, ok := resp["scanner_status"].(map[string]interface{})
	if !ok {
		t.Fatal("scanner_status missing or wrong type")
	}
	if scannerStatus["enabled"] != true {
		t.Errorf("scanner enabled = %v, want true", scannerStatus["enabled"])
	}
}

func TestHandleAPIStatus_MethodNotAllowed(t *testing.T) {
	s, _ := newTestServer(t)

	req := httptest.NewRequest("POST", "/api/status", nil)
	w := httptest.NewRecorder()
	s.HandleAPIStatusForTest(w, req)

	if w.Code != http.StatusMethodNotAllowed {
		t.Errorf("expected 405, got %d", w.Code)
	}
}

func TestHandleAPIStatus_NoWorkspace_NoExtendedFields(t *testing.T) {
	s := NewServer(ServerConfig{
		Version:    "test-no-workspace",
		SessionMgr: NewSessionManager(1 * time.Hour),
	})

	req := httptest.NewRequest("GET", "/api/status", nil)
	w := httptest.NewRecorder()
	s.HandleAPIStatusForTest(w, req)

	if w.Code != 200 {
		t.Fatalf("expected 200, got %d", w.Code)
	}

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)

	// Basic fields should still be present
	if resp["version"] != "test-no-workspace" {
		t.Errorf("version = %v, want test-no-workspace", resp["version"])
	}
	// Extended fields should NOT be present without workspace
	if _, ok := resp["scanner_status"]; ok {
		t.Error("scanner_status should not be present without workspace")
	}
	if _, ok := resp["model"]; ok {
		t.Error("model should not be present without workspace")
	}
}

func TestHandleAPIStatus_ExtendedFields_NoWorkspace(t *testing.T) {
	s, _ := newTestServer(t)
	// workspace is set, so extended fields should appear

	req := httptest.NewRequest("GET", "/api/status", nil)
	w := httptest.NewRecorder()
	s.HandleAPIStatusForTest(w, req)

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)

	// scanner_status should be present (workspace is set)
	if _, ok := resp["scanner_status"]; !ok {
		t.Error("scanner_status should be present with workspace")
	}
	// cluster_status should be present
	if _, ok := resp["cluster_status"]; !ok {
		t.Error("cluster_status should be present with workspace")
	}
}

// --- handleAPILogs ---

func TestHandleAPILogs_EmptyWorkspace(t *testing.T) {
	s := NewServer(ServerConfig{Version: "test"})

	req := httptest.NewRequest("GET", "/api/logs?source=general&n=50", nil)
	w := httptest.NewRecorder()
	s.HandleAPILogsForTest(w, req)

	if w.Code != http.StatusServiceUnavailable {
		t.Errorf("expected 503, got %d", w.Code)
	}
}

func TestHandleAPILogs_NoLogFile(t *testing.T) {
	s, _ := newTestServer(t)

	req := httptest.NewRequest("GET", "/api/logs?source=general&n=50", nil)
	w := httptest.NewRecorder()
	s.HandleAPILogsForTest(w, req)

	if w.Code != 200 {
		t.Fatalf("expected 200, got %d", w.Code)
	}

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)
	entries, ok := resp["entries"].([]interface{})
	if !ok {
		t.Fatal("entries missing or wrong type")
	}
	if len(entries) != 0 {
		t.Errorf("entries length = %d, want 0 for non-existent file", len(entries))
	}
}

func TestHandleAPILogs_WithEntries(t *testing.T) {
	s, dir := newTestServer(t)

	// Create log directory and file
	logsDir := filepath.Join(dir, "logs")
	os.MkdirAll(logsDir, 0755)
	logFile := filepath.Join(logsDir, "nemesisbot.log")

	entries := []string{
		`{"level":"INFO","timestamp":"2026-04-18T10:00:00Z","component":"gateway","message":"started"}`,
		`{"level":"WARN","timestamp":"2026-04-18T10:00:01Z","component":"security","message":"suspicious"}`,
		`{"level":"ERROR","timestamp":"2026-04-18T10:00:02Z","component":"scanner","message":"failed"}`,
	}
	os.WriteFile(logFile, []byte(entries[0]+"\n"+entries[1]+"\n"+entries[2]+"\n"), 0644)

	req := httptest.NewRequest("GET", "/api/logs?source=general&n=50", nil)
	w := httptest.NewRecorder()
	s.HandleAPILogsForTest(w, req)

	if w.Code != 200 {
		t.Fatalf("expected 200, got %d: %s", w.Code, w.Body.String())
	}

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)
	resultEntries, ok := resp["entries"].([]interface{})
	if !ok {
		t.Fatal("entries missing or wrong type")
	}
	if len(resultEntries) != 3 {
		t.Fatalf("entries length = %d, want 3", len(resultEntries))
	}

	// Verify first entry
	first, _ := resultEntries[0].(map[string]interface{})
	if first["level"] != "INFO" {
		t.Errorf("first entry level = %v, want INFO", first["level"])
	}
}

func TestHandleAPILogs_LimitParameter(t *testing.T) {
	s, dir := newTestServer(t)

	logsDir := filepath.Join(dir, "logs")
	os.MkdirAll(logsDir, 0755)
	logFile := filepath.Join(logsDir, "nemesisbot.log")

	var lines string
	for i := 0; i < 10; i++ {
		lines += fmt.Sprintf(`{"level":"INFO","message":"entry %d"}`+"\n", i)
	}
	os.WriteFile(logFile, []byte(lines), 0644)

	req := httptest.NewRequest("GET", "/api/logs?source=general&n=3", nil)
	w := httptest.NewRecorder()
	s.HandleAPILogsForTest(w, req)

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)
	resultEntries, _ := resp["entries"].([]interface{})

	if len(resultEntries) != 3 {
		t.Fatalf("entries length = %d, want 3 (limit)", len(resultEntries))
	}

	// Should get last 3 entries
	last, _ := resultEntries[2].(map[string]interface{})
	if last["message"] != "entry 9" {
		t.Errorf("last entry message = %v, want 'entry 9'", last["message"])
	}
}

func TestHandleAPILogs_UnknownSource(t *testing.T) {
	s, _ := newTestServer(t)

	req := httptest.NewRequest("GET", "/api/logs?source=unknown", nil)
	w := httptest.NewRecorder()
	s.HandleAPILogsForTest(w, req)

	if w.Code != 200 {
		t.Fatalf("expected 200, got %d", w.Code)
	}

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)
	entries, _ := resp["entries"].([]interface{})
	if len(entries) != 0 {
		t.Errorf("unknown source should return empty entries, got %d", len(entries))
	}
}

func TestHandleAPILogs_MethodNotAllowed(t *testing.T) {
	s, _ := newTestServer(t)

	req := httptest.NewRequest("POST", "/api/logs", nil)
	w := httptest.NewRecorder()
	s.HandleAPILogsForTest(w, req)

	if w.Code != http.StatusMethodNotAllowed {
		t.Errorf("expected 405, got %d", w.Code)
	}
}

func TestHandleAPILogs_InvalidJSONLines(t *testing.T) {
	s, dir := newTestServer(t)

	logsDir := filepath.Join(dir, "logs")
	os.MkdirAll(logsDir, 0755)
	logFile := filepath.Join(logsDir, "nemesisbot.log")

	// Mix valid JSON and plain text lines
	content := `{"level":"INFO","message":"valid entry"}
this is plain text not JSON
{"level":"WARN","message":"another valid"}` + "\n"
	os.WriteFile(logFile, []byte(content), 0644)

	req := httptest.NewRequest("GET", "/api/logs?source=general&n=50", nil)
	w := httptest.NewRecorder()
	s.HandleAPILogsForTest(w, req)

	if w.Code != 200 {
		t.Fatalf("expected 200, got %d: %s", w.Code, w.Body.String())
	}

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)
	entries, _ := resp["entries"].([]interface{})
	if len(entries) != 3 {
		t.Fatalf("entries length = %d, want 3", len(entries))
	}

	// Second entry should be plain text fallback
	plainEntry, _ := entries[1].(map[string]interface{})
	if _, ok := plainEntry["level"]; ok {
		t.Error("plain text entry should not have 'level' field")
	}
	if plainEntry["message"] != "this is plain text not JSON" {
		t.Errorf("plain text message = %v, want 'this is plain text not JSON'", plainEntry["message"])
	}
}

func TestHandleAPILogs_LimitOverMax(t *testing.T) {
	s, dir := newTestServer(t)

	logsDir := filepath.Join(dir, "logs")
	os.MkdirAll(logsDir, 0755)
	logFile := filepath.Join(logsDir, "nemesisbot.log")

	var content string
	for i := 0; i < 1200; i++ {
		content += fmt.Sprintf(`{"level":"INFO","message":"entry %d"}`+"\n", i)
	}
	os.WriteFile(logFile, []byte(content), 0644)

	// Request more than max (1000)
	req := httptest.NewRequest("GET", "/api/logs?source=general&n=5000", nil)
	w := httptest.NewRecorder()
	s.HandleAPILogsForTest(w, req)

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)
	entries, _ := resp["entries"].([]interface{})

	// Should be capped at 1000
	if len(entries) != 1000 {
		t.Errorf("entries length = %d, want 1000 (max cap)", len(entries))
	}
}

func TestHandleAPILogs_DefaultSource(t *testing.T) {
	s, dir := newTestServer(t)

	logsDir := filepath.Join(dir, "logs")
	os.MkdirAll(logsDir, 0755)
	logFile := filepath.Join(logsDir, "nemesisbot.log")
	os.WriteFile(logFile, []byte(`{"level":"INFO","message":"test"}`+"\n"), 0644)

	// No source parameter - should default to "general"
	req := httptest.NewRequest("GET", "/api/logs?n=50", nil)
	w := httptest.NewRecorder()
	s.HandleAPILogsForTest(w, req)

	if w.Code != 200 {
		t.Fatalf("expected 200, got %d", w.Code)
	}

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)
	entries, _ := resp["entries"].([]interface{})
	if len(entries) != 1 {
		t.Errorf("entries length = %d, want 1 (default source=general)", len(entries))
	}
}

func TestHandleAPILogs_EmptyLines(t *testing.T) {
	s, dir := newTestServer(t)

	logsDir := filepath.Join(dir, "logs")
	os.MkdirAll(logsDir, 0755)
	logFile := filepath.Join(logsDir, "nemesisbot.log")

	// Log with empty lines between entries
	content := `{"level":"INFO","message":"first"}


{"level":"WARN","message":"second"}

` + "\n"
	os.WriteFile(logFile, []byte(content), 0644)

	req := httptest.NewRequest("GET", "/api/logs?source=general&n=50", nil)
	w := httptest.NewRecorder()
	s.HandleAPILogsForTest(w, req)

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)
	entries, _ := resp["entries"].([]interface{})
	if len(entries) != 2 {
		t.Errorf("entries length = %d, want 2 (empty lines skipped)", len(entries))
	}
}

func TestHandleAPILogs_InvalidLimit(t *testing.T) {
	s, dir := newTestServer(t)

	logsDir := filepath.Join(dir, "logs")
	os.MkdirAll(logsDir, 0755)
	logFile := filepath.Join(logsDir, "nemesisbot.log")

	var content string
	for i := 0; i < 300; i++ {
		content += fmt.Sprintf(`{"level":"INFO","message":"entry %d"}`+"\n", i)
	}
	os.WriteFile(logFile, []byte(content), 0644)

	// Invalid n parameter - should use default 200
	req := httptest.NewRequest("GET", "/api/logs?source=general&n=abc", nil)
	w := httptest.NewRecorder()
	s.HandleAPILogsForTest(w, req)

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)
	entries, _ := resp["entries"].([]interface{})
	if len(entries) != 200 {
		t.Errorf("entries length = %d, want 200 (default when n is invalid)", len(entries))
	}
}

func TestHandleAPILogs_ClusterSource(t *testing.T) {
	s, _ := newTestServer(t)

	req := httptest.NewRequest("GET", "/api/logs?source=cluster", nil)
	w := httptest.NewRecorder()
	s.HandleAPILogsForTest(w, req)

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)
	entries, _ := resp["entries"].([]interface{})
	if len(entries) != 0 {
		t.Errorf("cluster source should return empty (not yet implemented), got %d", len(entries))
	}
}

// --- handleAPIScannerStatus ---

func TestHandleAPIScannerStatus_NoConfig(t *testing.T) {
	s, dir := newTestServer(t)
	_ = dir

	req := httptest.NewRequest("GET", "/api/scanner/status", nil)
	w := httptest.NewRecorder()
	s.HandleAPIScannerStatusForTest(w, req)

	if w.Code != 200 {
		t.Fatalf("expected 200, got %d", w.Code)
	}

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)
	if resp["enabled"] != false {
		t.Errorf("enabled = %v, want false (no config file)", resp["enabled"])
	}
}

func TestHandleAPIScannerStatus_WithConfig(t *testing.T) {
	s, dir := newTestServer(t)

	configDir := filepath.Join(dir, "config")
	os.MkdirAll(configDir, 0755)
	os.WriteFile(filepath.Join(configDir, "config.scanner.json"), []byte(`{
		"enabled": ["clamav"],
		"engines": {"clamav": {"url": "http://localhost:3310"}}
	}`), 0644)

	req := httptest.NewRequest("GET", "/api/scanner/status", nil)
	w := httptest.NewRecorder()
	s.HandleAPIScannerStatusForTest(w, req)

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)
	if resp["enabled"] != true {
		t.Errorf("enabled = %v, want true", resp["enabled"])
	}

	engines, ok := resp["engines"].([]interface{})
	if !ok {
		t.Fatal("engines missing or wrong type")
	}
	if len(engines) != 1 {
		t.Fatalf("engines length = %d, want 1", len(engines))
	}
	engine, _ := engines[0].(map[string]interface{})
	if engine["name"] != "clamav" {
		t.Errorf("engine name = %v, want clamav", engine["name"])
	}
}

func TestHandleAPIScannerStatus_MethodNotAllowed(t *testing.T) {
	s, _ := newTestServer(t)

	req := httptest.NewRequest("POST", "/api/scanner/status", nil)
	w := httptest.NewRecorder()
	s.HandleAPIScannerStatusForTest(w, req)

	if w.Code != http.StatusMethodNotAllowed {
		t.Errorf("expected 405, got %d", w.Code)
	}
}

func TestHandleAPIScannerStatus_InvalidJSON(t *testing.T) {
	s, dir := newTestServer(t)

	configDir := filepath.Join(dir, "config")
	os.MkdirAll(configDir, 0755)
	os.WriteFile(filepath.Join(configDir, "config.scanner.json"), []byte(`not valid json`), 0644)

	req := httptest.NewRequest("GET", "/api/scanner/status", nil)
	w := httptest.NewRecorder()
	s.HandleAPIScannerStatusForTest(w, req)

	if w.Code != 200 {
		t.Fatalf("expected 200, got %d", w.Code)
	}

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)
	// Should return disabled state on parse error
	if resp["enabled"] != false {
		t.Errorf("enabled = %v, want false (invalid JSON)", resp["enabled"])
	}
}

func TestHandleAPIScannerStatus_MultipleEngines(t *testing.T) {
	s, dir := newTestServer(t)

	configDir := filepath.Join(dir, "config")
	os.MkdirAll(configDir, 0755)
	os.WriteFile(filepath.Join(configDir, "config.scanner.json"), []byte(`{
		"enabled": ["clamav", "yara"],
		"engines": {
			"clamav": {"url": "http://localhost:3310"},
			"yara": {"rules_path": "/etc/yara/rules"}
		}
	}`), 0644)

	req := httptest.NewRequest("GET", "/api/scanner/status", nil)
	w := httptest.NewRecorder()
	s.HandleAPIScannerStatusForTest(w, req)

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)

	engines, _ := resp["engines"].([]interface{})
	if len(engines) != 2 {
		t.Fatalf("engines length = %d, want 2", len(engines))
	}

	// Engines should be sorted by name
	first, _ := engines[0].(map[string]interface{})
	second, _ := engines[1].(map[string]interface{})
	if first["name"] != "clamav" {
		t.Errorf("first engine = %v, want clamav (sorted)", first["name"])
	}
	if second["name"] != "yara" {
		t.Errorf("second engine = %v, want yara (sorted)", second["name"])
	}
}

func TestHandleAPIScannerStatus_EmptyEngines(t *testing.T) {
	s, dir := newTestServer(t)

	configDir := filepath.Join(dir, "config")
	os.MkdirAll(configDir, 0755)
	os.WriteFile(filepath.Join(configDir, "config.scanner.json"), []byte(`{
		"enabled": [],
		"engines": {}
	}`), 0644)

	req := httptest.NewRequest("GET", "/api/scanner/status", nil)
	w := httptest.NewRecorder()
	s.HandleAPIScannerStatusForTest(w, req)

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)
	if resp["enabled"] != false {
		t.Errorf("enabled = %v, want false (empty enabled array)", resp["enabled"])
	}
}

func TestHandleAPIScannerStatus_EmptyWorkspace(t *testing.T) {
	s := NewServer(ServerConfig{Version: "test"})

	req := httptest.NewRequest("GET", "/api/scanner/status", nil)
	w := httptest.NewRecorder()
	s.HandleAPIScannerStatusForTest(w, req)

	if w.Code != http.StatusServiceUnavailable {
		t.Errorf("expected 503, got %d", w.Code)
	}
}

// --- handleAPIConfig ---

func TestHandleAPIConfig_Sanitization(t *testing.T) {
	s, dir := newTestServer(t)

	configDir := filepath.Join(dir, "config")
	os.MkdirAll(configDir, 0755)
	os.WriteFile(filepath.Join(configDir, "config.json"), []byte(`{
		"agents": {"model": "gpt-4"},
		"channels": {"web": {"auth_token": "secret-token-123"}},
		"tools": {"brave": {"api_key": "sk-abc123longkey"}}
	}`), 0644)

	req := httptest.NewRequest("GET", "/api/config", nil)
	w := httptest.NewRecorder()
	s.HandleAPIConfigForTest(w, req)

	if w.Code != 200 {
		t.Fatalf("expected 200, got %d: %s", w.Code, w.Body.String())
	}

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)

	// Check that sensitive fields are masked
	channels, _ := resp["channels"].(map[string]interface{})
	web, _ := channels["web"].(map[string]interface{})
	if web["auth_token"] != "secr****" {
		t.Errorf("auth_token = %v, want 'secr****'", web["auth_token"])
	}

	tools, _ := resp["tools"].(map[string]interface{})
	brave, _ := tools["brave"].(map[string]interface{})
	if brave["api_key"] != "sk-a****" {
		t.Errorf("api_key = %v, want 'sk-a****'", brave["api_key"])
	}

	// Non-sensitive fields should be unchanged
	agents, _ := resp["agents"].(map[string]interface{})
	if agents["model"] != "gpt-4" {
		t.Errorf("model = %v, want 'gpt-4'", agents["model"])
	}
}

func TestHandleAPIConfig_DeepNestedSanitization(t *testing.T) {
	s, dir := newTestServer(t)

	configDir := filepath.Join(dir, "config")
	os.MkdirAll(configDir, 0755)
	os.WriteFile(filepath.Join(configDir, "config.json"), []byte(`{
		"level1": {
			"level2": {
				"level3": {
					"api_key": "deep-secret-key-12345",
					"normal_field": "visible-value"
				}
			}
		}
	}`), 0644)

	req := httptest.NewRequest("GET", "/api/config", nil)
	w := httptest.NewRecorder()
	s.HandleAPIConfigForTest(w, req)

	if w.Code != 200 {
		t.Fatalf("expected 200, got %d: %s", w.Code, w.Body.String())
	}

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)

	// Navigate deep nesting
	level1, _ := resp["level1"].(map[string]interface{})
	level2, _ := level1["level2"].(map[string]interface{})
	level3, _ := level2["level3"].(map[string]interface{})

	// api_key should be masked
	if level3["api_key"] != "deep****" {
		t.Errorf("deep nested api_key = %v, want 'deep****'", level3["api_key"])
	}
	// normal field should be unchanged
	if level3["normal_field"] != "visible-value" {
		t.Errorf("normal_field = %v, want 'visible-value'", level3["normal_field"])
	}
}

func TestHandleAPIConfig_InvalidJSON(t *testing.T) {
	s, dir := newTestServer(t)

	configDir := filepath.Join(dir, "config")
	os.MkdirAll(configDir, 0755)
	os.WriteFile(filepath.Join(configDir, "config.json"), []byte(`{invalid json`), 0644)

	req := httptest.NewRequest("GET", "/api/config", nil)
	w := httptest.NewRecorder()
	s.HandleAPIConfigForTest(w, req)

	if w.Code != http.StatusInternalServerError {
		t.Errorf("expected 500, got %d", w.Code)
	}
}

func TestHandleAPIConfig_ShortSensitiveValue(t *testing.T) {
	s, dir := newTestServer(t)

	configDir := filepath.Join(dir, "config")
	os.MkdirAll(configDir, 0755)
	os.WriteFile(filepath.Join(configDir, "config.json"), []byte(`{
		"database": {"password": "ab"},
		"api": {"token": "xyz"}
	}`), 0644)

	req := httptest.NewRequest("GET", "/api/config", nil)
	w := httptest.NewRecorder()
	s.HandleAPIConfigForTest(w, req)

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)

	db, _ := resp["database"].(map[string]interface{})
	// Short values (<=4 chars) should become "****"
	if db["password"] != "****" {
		t.Errorf("short password = %v, want '****'", db["password"])
	}

	api, _ := resp["api"].(map[string]interface{})
	if api["token"] != "****" {
		t.Errorf("short token = %v, want '****'", api["token"])
	}
}

func TestHandleAPIConfig_AllSensitivePatterns(t *testing.T) {
	s, dir := newTestServer(t)

	configDir := filepath.Join(dir, "config")
	os.MkdirAll(configDir, 0755)
	os.WriteFile(filepath.Join(configDir, "config.json"), []byte(`{
		"secrets": {
			"my_key": "key-value-12345",
			"my_token": "token-value-12345",
			"my_secret": "secret-value-12345",
			"my_password": "password-value-12345",
			"my_auth": "auth-value-12345",
			"my_credential": "credential-value-12345"
		}
	}`), 0644)

	req := httptest.NewRequest("GET", "/api/config", nil)
	w := httptest.NewRecorder()
	s.HandleAPIConfigForTest(w, req)

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)

	secrets, _ := resp["secrets"].(map[string]interface{})

	patterns := map[string]string{
		"my_key":        "key-****",
		"my_token":      "toke****",
		"my_secret":     "secr****",
		"my_password":   "pass****",
		"my_auth":       "auth****",
		"my_credential": "cred****",
	}
	for field, expected := range patterns {
		if secrets[field] != expected {
			t.Errorf("%s = %v, want %q", field, secrets[field], expected)
		}
	}
}

func TestHandleAPIConfig_NoConfigFile(t *testing.T) {
	s, dir := newTestServer(t)

	// No config file created
	configDir := filepath.Join(dir, "config")
	os.MkdirAll(configDir, 0755)

	req := httptest.NewRequest("GET", "/api/config", nil)
	w := httptest.NewRecorder()
	s.HandleAPIConfigForTest(w, req)

	if w.Code != http.StatusNotFound {
		t.Errorf("expected 404, got %d", w.Code)
	}
}

func TestHandleAPIConfig_EmptyStringToken(t *testing.T) {
	s, dir := newTestServer(t)

	configDir := filepath.Join(dir, "config")
	os.MkdirAll(configDir, 0755)
	os.WriteFile(filepath.Join(configDir, "config.json"), []byte(`{
		"channels": {"web": {"auth_token": ""}}
	}`), 0644)

	req := httptest.NewRequest("GET", "/api/config", nil)
	w := httptest.NewRecorder()
	s.HandleAPIConfigForTest(w, req)

	var resp map[string]interface{}
	json.Unmarshal(w.Body.Bytes(), &resp)

	channels, _ := resp["channels"].(map[string]interface{})
	web, _ := channels["web"].(map[string]interface{})
	// Empty string should remain empty (not become "****")
	if web["auth_token"] != "" {
		t.Errorf("empty auth_token = %v, want ''", web["auth_token"])
	}
}

func TestHandleAPIConfig_EmptyWorkspace(t *testing.T) {
	s := NewServer(ServerConfig{Version: "test"})

	req := httptest.NewRequest("GET", "/api/config", nil)
	w := httptest.NewRecorder()
	s.HandleAPIConfigForTest(w, req)

	if w.Code != http.StatusServiceUnavailable {
		t.Errorf("expected 503, got %d", w.Code)
	}
}

func TestHandleAPIConfig_MethodNotAllowed(t *testing.T) {
	s, _ := newTestServer(t)

	req := httptest.NewRequest("DELETE", "/api/config", nil)
	w := httptest.NewRecorder()
	s.HandleAPIConfigForTest(w, req)

	if w.Code != http.StatusMethodNotAllowed {
		t.Errorf("expected 405, got %d", w.Code)
	}
}

// --- handleEventsStream ---

func TestHandleEventsStream_StatusEvent(t *testing.T) {
	s, _ := newTestServer(t)

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	req := httptest.NewRequest("GET", "/api/events/stream", nil).WithContext(ctx)
	w := httptest.NewRecorder()

	// Run handler in goroutine
	done := make(chan struct{})
	go func() {
		s.HandleEventsStreamForTest(w, req)
		close(done)
	}()

	// Wait briefly, then publish event and cancel
	time.Sleep(100 * time.Millisecond)
	s.GetEventHub().Publish("status", map[string]interface{}{
		"version": "test",
	})
	time.Sleep(100 * time.Millisecond)
	cancel() // Cancel context to end SSE handler

	select {
	case <-done:
	case <-time.After(2 * time.Second):
		t.Fatal("SSE handler didn't complete in time")
	}

	body := w.Body.String()
	if !contains(body, "event: heartbeat") {
		t.Errorf("expected initial heartbeat event, got: %s", body)
	}
	if !contains(body, "event: status") {
		t.Errorf("expected status event, got: %s", body)
	}
}

func TestHandleEventsStream_ContextCancel(t *testing.T) {
	s, _ := newTestServer(t)

	ctx, cancel := context.WithCancel(context.Background())
	cancel() // Cancel immediately

	req := httptest.NewRequest("GET", "/api/events/stream", nil).WithContext(ctx)
	w := httptest.NewRecorder()

	s.HandleEventsStreamForTest(w, req)

	body := w.Body.String()
	// Should have at least the initial heartbeat before context was cancelled
	if !contains(body, "event: heartbeat") {
		t.Errorf("expected heartbeat event before cancel, got: %s", body)
	}
}

func contains(s, substr string) bool {
	return len(s) >= len(substr) && (s == substr || len(s) > 0 && containsHelper(s, substr))
}

func containsHelper(s, substr string) bool {
	for i := 0; i <= len(s)-len(substr); i++ {
		if s[i:i+len(substr)] == substr {
			return true
		}
	}
	return false
}
