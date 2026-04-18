// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Web Channel Module - REST API Handlers

package web

import (
	"bufio"
	"encoding/json"
	"fmt"
	"io/fs"
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/276793422/NemesisBot/module/logger"
)

// logEntry mirrors logger.LogEntry for JSON deserialization of log file lines.
type logEntry struct {
	Level     string                 `json:"level"`
	Timestamp string                 `json:"timestamp"`
	Component string                 `json:"component,omitempty"`
	Message   string                 `json:"message"`
	Fields    map[string]interface{} `json:"fields,omitempty"`
}

// handleAPIStatus returns system status as JSON
func (s *Server) handleAPIStatus(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet {
		http.Error(w, "Method not allowed", http.StatusMethodNotAllowed)
		return
	}

	stats := s.sessionMgr.Stats()
	uptime := time.Since(s.startTime).Seconds()

	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Access-Control-Allow-Origin", "*")

	response := map[string]interface{}{
		"version":        s.version,
		"uptime_seconds": int64(uptime),
		"ws_connected":   s.running,
		"session_count":  stats["active_sessions"],
	}

	// Extended fields
	if s.workspace != "" {
		response["scanner_status"] = s.loadScannerStatus()
		response["cluster_status"] = map[string]interface{}{
			"enabled":    false,
			"node_count": 0,
		}
		response["model"] = s.modelName
	}

	data, _ := json.Marshal(response)
	w.Write(data)
}

// handleAPILogs returns historical log entries
func (s *Server) handleAPILogs(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet {
		http.Error(w, "Method not allowed", http.StatusMethodNotAllowed)
		return
	}

	if s.workspace == "" {
		writeJSONError(w, "workspace not configured", http.StatusServiceUnavailable)
		return
	}

	source := r.URL.Query().Get("source")
	if source == "" {
		source = "general"
	}

	nStr := r.URL.Query().Get("n")
	n := 200
	if nStr != "" {
		if parsed, err := strconv.Atoi(nStr); err == nil && parsed > 0 {
			n = parsed
			if n > 1000 {
				n = 1000
			}
		}
	}

	logFilePath := s.resolveLogFilePath(source)
	if logFilePath == "" {
		writeJSON(w, map[string]interface{}{"entries": []interface{}{}})
		return
	}

	entries := s.readLogEntries(logFilePath, n)

	// Tag each entry with the source for frontend filtering
	for i := range entries {
		if _, ok := entries[i].(map[string]interface{}); ok {
			// already a map from JSON unmarshal
		}
	}

	writeJSON(w, map[string]interface{}{"entries": entries})
}

// handleAPIScannerStatus returns scanner engine status
func (s *Server) handleAPIScannerStatus(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet {
		http.Error(w, "Method not allowed", http.StatusMethodNotAllowed)
		return
	}

	if s.workspace == "" {
		writeJSONError(w, "workspace not configured", http.StatusServiceUnavailable)
		return
	}

	writeJSON(w, s.loadScannerStatus())
}

// handleAPIConfig returns sanitized configuration
func (s *Server) handleAPIConfig(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet {
		http.Error(w, "Method not allowed", http.StatusMethodNotAllowed)
		return
	}

	if s.workspace == "" {
		writeJSONError(w, "workspace not configured", http.StatusServiceUnavailable)
		return
	}

	configPath := filepath.Join(s.workspace, "config", "config.json")
	data, err := os.ReadFile(configPath)
	if err != nil {
		logger.DebugCF("web", "Config file not found", map[string]interface{}{
			"path": configPath,
		})
		writeJSONError(w, "configuration not found", http.StatusNotFound)
		return
	}

	var raw map[string]interface{}
	if err := json.Unmarshal(data, &raw); err != nil {
		writeJSONError(w, "invalid configuration format", http.StatusInternalServerError)
		return
	}

	sanitizeMap(raw)
	writeJSON(w, raw)
}

// --- internal helpers ---

// loadScannerStatus reads scanner config and returns status summary
func (s *Server) loadScannerStatus() map[string]interface{} {
	scannerConfigPath := filepath.Join(s.workspace, "config", "config.scanner.json")
	data, err := os.ReadFile(scannerConfigPath)
	if err != nil {
		return map[string]interface{}{
			"enabled": false,
			"engines": []interface{}{},
		}
	}

	var cfg struct {
		Enabled []string               `json:"enabled"`
		Engines map[string]json.RawMessage `json:"engines"`
	}
	if err := json.Unmarshal(data, &cfg); err != nil {
		return map[string]interface{}{
			"enabled": false,
			"engines": []interface{}{},
		}
	}

	engines := make([]map[string]interface{}, 0, len(cfg.Engines))
	for name, raw := range cfg.Engines {
		engine := map[string]interface{}{
			"name":   name,
			"config": json.RawMessage(raw),
		}
		engines = append(engines, engine)
	}
	sort.Slice(engines, func(i, j int) bool {
		return engines[i]["name"].(string) < engines[j]["name"].(string)
	})

	return map[string]interface{}{
		"enabled": len(cfg.Enabled) > 0,
		"engines": engines,
	}
}

// resolveLogFilePath returns the log file path for a given source
func (s *Server) resolveLogFilePath(source string) string {
	switch source {
	case "general":
		// Try configured log file first, then default
		candidates := []string{
			filepath.Join(s.workspace, "logs", "nemesisbot.log"),
			filepath.Join(s.workspace, "logs", "app.log"),
		}
		for _, c := range candidates {
			if _, err := os.Stat(c); err == nil {
				return c
			}
		}
		// Return default even if it doesn't exist
		return candidates[0]

	case "llm":
		dir := filepath.Join(s.workspace, "logs", "request_logs")
		// Find the latest file in request_logs
		latest := s.findLatestFile(dir)
		if latest != "" {
			return latest
		}
		return ""

	case "security":
		// Find security audit log
		secDir := filepath.Join(s.workspace, "config")
		pattern := filepath.Join(secDir, "security_audit_*.log")
		matches, _ := filepath.Glob(pattern)
		if len(matches) > 0 {
			sort.Sort(sort.Reverse(sort.StringSlice(matches)))
			return matches[0]
		}
		return ""

	case "cluster":
		// Not yet implemented
		return ""
	}

	return ""
}

// findLatestFile finds the most recently modified file in a directory
func (s *Server) findLatestFile(dir string) string {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return ""
	}

	var latest fs.FileInfo
	var latestName string
	for _, e := range entries {
		if e.IsDir() {
			continue
		}
		info, err := e.Info()
		if err != nil {
			continue
		}
		if latest == nil || info.ModTime().After(latest.ModTime()) {
			latest = info
			latestName = e.Name()
		}
	}

	if latestName == "" {
		return ""
	}
	return filepath.Join(dir, latestName)
}

// readLogEntries reads the last n JSON Lines entries from a file
func (s *Server) readLogEntries(filePath string, n int) []interface{} {
	f, err := os.Open(filePath)
	if err != nil {
		return []interface{}{}
	}
	defer f.Close()

	// Read all lines (for simplicity; for very large files a ring buffer would be better)
	var lines []string
	scanner := bufio.NewScanner(f)
	// Increase buffer size for long log lines
	scanner.Buffer(make([]byte, 0, 64*1024), 1024*1024)
	for scanner.Scan() {
		line := strings.TrimSpace(scanner.Text())
		if line == "" {
			continue
		}
		lines = append(lines, line)
	}

	// Take last n lines
	start := 0
	if len(lines) > n {
		start = len(lines) - n
	}

	entries := make([]interface{}, 0, n)
	for _, line := range lines[start:] {
		var entry map[string]interface{}
		if err := json.Unmarshal([]byte(line), &entry); err != nil {
			// Not JSON — create a plain text entry
			entry = map[string]interface{}{
				"message": line,
			}
		}
		entries = append(entries, entry)
	}

	return entries
}

// sanitizeMap recursively masks sensitive values in a map
func sanitizeMap(m map[string]interface{}) {
	sensitiveKeys := []string{"key", "token", "secret", "password", "auth", "credential"}
	for k, v := range m {
		switch tv := v.(type) {
		case map[string]interface{}:
			sanitizeMap(tv)
		case string:
			lower := strings.ToLower(k)
			for _, sk := range sensitiveKeys {
				if strings.Contains(lower, sk) && len(tv) > 0 {
					if len(tv) <= 4 {
						m[k] = "****"
					} else {
						m[k] = tv[:4] + "****"
					}
					break
				}
			}
		}
	}
}

// writeJSON writes a JSON response
func writeJSON(w http.ResponseWriter, v interface{}) {
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Access-Control-Allow-Origin", "*")
	data, _ := json.Marshal(v)
	w.Write(data)
}

// writeJSONError writes a JSON error response
func writeJSONError(w http.ResponseWriter, message string, code int) {
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Access-Control-Allow-Origin", "*")
	w.WriteHeader(code)
	fmt.Fprintf(w, `{"error":%q}`, message)
}
