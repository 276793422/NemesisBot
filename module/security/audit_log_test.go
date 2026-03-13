// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package security

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestInitAuditLogFile(t *testing.T) {
	t.Run("success case", func(t *testing.T) {
		tempDir, err := os.MkdirTemp("", "audit-test-*")
		if err != nil {
			t.Fatalf("failed to create temp dir: %v", err)
		}
		defer os.RemoveAll(tempDir)

		auditor := NewSecurityAuditor(&AuditorConfig{
			Enabled:             true,
			AuditLogDir:         tempDir,
			AuditLogFileEnabled: true,
		})

		err = auditor.initAuditLogFile()
		if err != nil {
			t.Errorf("initAuditLogFile() returned error: %v", err)
		}

		if auditor.logFile == nil {
			t.Error("log file should be initialized")
		}
		if auditor.logFilePath == "" {
			t.Error("log file path should be set")
		}

		// Close the file to prevent resource leak
		auditor.Close()
	})

	t.Run("nil config", func(t *testing.T) {
		auditor := NewSecurityAuditor(&AuditorConfig{
			Enabled: true,
		})

		err := auditor.initAuditLogFile()
		if err == nil {
			t.Error("initAuditLogFile() should return error for nil config")
		}
	})

	t.Run("empty audit log dir", func(t *testing.T) {
		auditor := NewSecurityAuditor(&AuditorConfig{
			Enabled:             true,
			AuditLogDir:         "",
			AuditLogFileEnabled: true,
		})

		err := auditor.initAuditLogFile()
		if err == nil {
			t.Error("initAuditLogFile() should return error for empty audit log dir")
		}
	})

	t.Run("directory creation failure", func(t *testing.T) {
		// Create a file where we expect a directory
		tempFile, err := os.CreateTemp("", "audit-test")
		if err != nil {
			t.Fatalf("failed to create temp file: %v", err)
		}
		defer tempFile.Close()
		defer os.Remove(tempFile.Name())

		auditor := NewSecurityAuditor(&AuditorConfig{
			Enabled:             true,
			AuditLogDir:         tempFile.Name(),
			AuditLogFileEnabled: true,
		})

		err = auditor.initAuditLogFile()
		if err == nil {
			t.Error("initAuditLogFile() should return error when directory creation fails")
		}
	})
}

func TestWriteAuditLogToFile(t *testing.T) {
	t.Run("success case", func(t *testing.T) {
		tempDir, err := os.MkdirTemp("", "audit-write-test-*")
		if err != nil {
			t.Fatalf("failed to create temp dir: %v", err)
		}
		defer os.RemoveAll(tempDir)

		auditor := NewSecurityAuditor(&AuditorConfig{
			Enabled:             true,
			AuditLogDir:         tempDir,
			AuditLogFileEnabled: true,
		})

		// Initialize the audit log file
		err = auditor.initAuditLogFile()
		if err != nil {
			t.Fatalf("initAuditLogFile() failed: %v", err)
		}
		defer auditor.Close()

		// Create a test audit event
		event := AuditEvent{
			EventID: "test-event-123",
			Request: OperationRequest{
				Type:        OpFileRead,
				DangerLevel: DangerLow,
				User:        "test-user",
				Source:      "test-source",
				Target:      "/home/user/test.txt",
			},
			Decision:   "allowed",
			Reason:     "test reason",
			PolicyRule: "test-policy",
			Timestamp:  time.Now(),
		}

		// Write the event to the log file
		auditor.writeAuditLogToFile(event)

		// Verify the file exists and contains the log entry
		if _, err := os.Stat(auditor.logFilePath); os.IsNotExist(err) {
			t.Error("audit log file should exist")
		}

		// Read the file and verify content
		content, err := os.ReadFile(auditor.logFilePath)
		if err != nil {
			t.Fatalf("failed to read audit log file: %v", err)
		}

		logContent := string(content)
		if logContent == "" {
			t.Error("audit log file should not be empty")
		}

		// Check for key components in the log entry
		if !strings.Contains(logContent, "test-event-123") {
			t.Error("log should contain event ID")
		}
		if !strings.Contains(logContent, "allowed") {
			t.Error("log should contain decision")
		}
		if !strings.Contains(logContent, "file_read") {
			t.Error("log should contain operation type")
		}
	})

	t.Run("nil log file", func(t *testing.T) {
		auditor := NewSecurityAuditor(&AuditorConfig{
			Enabled: true,
		})

		event := AuditEvent{
			EventID: "test-event",
			Request: OperationRequest{
				Type:   OpFileRead,
				User:   "test",
				Source: "test",
			},
		}

		// Should not panic
		auditor.writeAuditLogToFile(event)
	})
}

func TestSanitizeLogTarget(t *testing.T) {
	tests := []struct {
		name     string
		input    string
		expected string
	}{
		{
			name:     "normal path",
			input:    "/home/user/file.txt",
			expected: "/home/user/file.txt",
		},
		{
			name:     "path with newlines",
			input:    "/home/user/\nfile.txt",
			expected: "/home/user/ file.txt",
		},
		{
			name:     "path with carriage returns",
			input:    "/home/user/\rfile.txt",
			expected: "/home/user/ file.txt",
		},
		{
			name:     "path with tabs",
			input:    "/home/user/\tfile.txt",
			expected: "/home/user/ file.txt",
		},
		{
			name:     "path with multiple special characters",
			input:    "/home/user/\n\r\tfile.txt",
			expected: "/home/user/   file.txt",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := sanitizeLogTarget(tt.input)
			if result != tt.expected {
				t.Errorf("sanitizeLogTarget(%q) = %q, want %q", tt.input, result, tt.expected)
			}
		})
	}
}

func TestSanitizeLogReason(t *testing.T) {
	tests := []struct {
		name     string
		input    string
		expected string
	}{
		{
			name:     "normal reason",
			input:    "normal operation",
			expected: "normal operation",
		},
		{
			name:     "reason with newlines",
			input:    "normal\noperation",
			expected: "normal operation",
		},
		{
			name:     "reason with carriage returns",
			input:    "normal\roperation",
			expected: "normal operation",
		},
		{
			name:     "reason with tabs",
			input:    "normal\toperation",
			expected: "normal operation",
		},
		{
			name:     "reason with multiple special characters",
			input:    "normal\n\r\toperation",
			expected: "normal   operation",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := sanitizeLogReason(tt.input)
			if result != tt.expected {
				t.Errorf("sanitizeLogReason(%q) = %q, want %q", tt.input, result, tt.expected)
			}
		})
	}
}

func TestSecurityAuditorClose(t *testing.T) {
	t.Run("close with open file", func(t *testing.T) {
		tempDir, err := os.MkdirTemp("", "audit-close-test-*")
		if err != nil {
			t.Fatalf("failed to create temp dir: %v", err)
		}
		defer os.RemoveAll(tempDir)

		auditor := NewSecurityAuditor(&AuditorConfig{
			Enabled:             true,
			AuditLogDir:         tempDir,
			AuditLogFileEnabled: true,
		})

		// Initialize the audit log file
		err = auditor.initAuditLogFile()
		if err != nil {
			t.Fatalf("initAuditLogFile() failed: %v", err)
		}

		// Close the auditor
		err = auditor.Close()
		if err != nil {
			t.Errorf("Close() returned error: %v", err)
		}

		// Verify the file is closed (by trying to write to it through the auditor)
		event := AuditEvent{
			EventID: "test-event",
			Request: OperationRequest{
				Type: OpFileRead,
			},
		}

		// Should not panic but should do nothing
		auditor.writeAuditLogToFile(event)
	})

	t.Run("close without open file", func(t *testing.T) {
		auditor := NewSecurityAuditor(&AuditorConfig{
			Enabled: true,
		})

		// Should not panic
		err := auditor.Close()
		if err != nil {
			t.Errorf("Close() returned error: %v", err)
		}
	})
}

func TestExportAuditLog(t *testing.T) {
	t.Run("export to CSV", func(t *testing.T) {
		tempDir, err := os.MkdirTemp("", "audit-export-test-*")
		if err != nil {
			t.Fatalf("failed to create temp dir: %v", err)
		}
		defer os.RemoveAll(tempDir)

		auditor := NewSecurityAuditor(&AuditorConfig{
			Enabled:             true,
			AuditLogDir:         tempDir,
			AuditLogFileEnabled: true,
		})

		// Initialize the audit log file
		err = auditor.initAuditLogFile()
		if err != nil {
			t.Fatalf("initAuditLogFile() failed: %v", err)
		}
		defer auditor.Close()

		// Add some audit events
		event1 := AuditEvent{
			EventID: "event-1",
			Request: OperationRequest{
				Type:        OpFileRead,
				DangerLevel: DangerLow,
				User:        "user1",
				Source:      "source1",
				Target:      "/tmp/file1.txt",
			},
			Decision:   "allowed",
			Reason:     "read allowed",
			PolicyRule: "policy1",
			Timestamp:  time.Now(),
		}

		event2 := AuditEvent{
			EventID: "event-2",
			Request: OperationRequest{
				Type:        OpFileDelete,
				DangerLevel: DangerHigh,
				User:        "user2",
				Source:      "source2",
				Target:      "/tmp/file2.txt",
			},
			Decision:   "denied",
			Reason:     "dangerous operation",
			PolicyRule: "policy2",
			Timestamp:  time.Now(),
		}

		// Add some audit events directly to the audit log
		auditor.auditLog = append(auditor.auditLog, event1)
		auditor.auditLog = append(auditor.auditLog, event2)

		// Create a temporary CSV file for export
		csvFile := filepath.Join(tempDir, "export.csv")

		// Export the audit log
		err = auditor.ExportAuditLog(csvFile)
		if err != nil {
			t.Errorf("ExportAuditLog() returned error: %v", err)
		}

		// Verify the CSV file exists
		if _, err := os.Stat(csvFile); os.IsNotExist(err) {
			t.Error("export CSV file should exist")
		}

		// Read and verify CSV content
		content, err := os.ReadFile(csvFile)
		if err != nil {
			t.Fatalf("failed to read exported CSV: %v", err)
		}

		csvContent := string(content)
		if !strings.Contains(csvContent, "event-1") {
			t.Error("CSV should contain event-1")
		}
		if !strings.Contains(csvContent, "event-2") {
			t.Error("CSV should contain event-2")
		}
		if !strings.Contains(csvContent, "allowed") {
			t.Error("CSV should contain 'allowed' decision")
		}
		if !strings.Contains(csvContent, "denied") {
			t.Error("CSV should contain 'denied' decision")
		}
	})

	t.Run("export empty audit log", func(t *testing.T) {
		auditor := NewSecurityAuditor(&AuditorConfig{
			Enabled: true,
		})

		csvFile := filepath.Join(os.TempDir(), "empty-export.csv")
		defer os.Remove(csvFile)

		err := auditor.ExportAuditLog(csvFile)
		if err != nil {
			t.Errorf("ExportAuditLog() returned error for empty log: %v", err)
		}
	})
}

func BenchmarkSanitizeLogTarget(b *testing.B) {
	target := "/very/long/path/with/\n\r\tnewlines/and/other/characters/that/need/sanitization.txt"
	for i := 0; i < b.N; i++ {
		sanitizeLogTarget(target)
	}
}

func BenchmarkSanitizeLogReason(b *testing.B) {
	reason := "normal\n\r\treason with special characters"
	for i := 0; i < b.N; i++ {
		sanitizeLogReason(reason)
	}
}
