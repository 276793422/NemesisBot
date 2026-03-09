// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package security

import (
	"context"
	"os"
	"path/filepath"
	"sync"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/config"
)

func TestSecureFileWrapper(t *testing.T) {
	// Create a temporary directory for testing
	tempDir, err := os.MkdirTemp("", "security-test-*")
	if err != nil {
		t.Fatalf("failed to create temp dir: %v", err)
	}
	defer os.RemoveAll(tempDir)

	auditor := NewSecurityAuditor(&AuditorConfig{
		Enabled:       true,
		DefaultAction: "allow",
	})
	auditor.SetRules(OpFileRead, []config.SecurityRule{
		{
			Pattern: tempDir + "/*",
			Action:  "allow",
		},
	})
	auditor.SetRules(OpFileWrite, []config.SecurityRule{
		{
			Pattern: tempDir + "/*",
			Action:  "allow",
		},
	})
	auditor.SetRules(OpFileDelete, []config.SecurityRule{
		{
			Pattern: tempDir + "/*",
			Action:  "allow",
		},
	})
	auditor.SetRules(OpDirRead, []config.SecurityRule{
		{
			Pattern: tempDir + "/*",
			Action:  "allow",
		},
	})
	auditor.SetRules(OpDirCreate, []config.SecurityRule{
		{
			Pattern: tempDir + "/*",
			Action:  "allow",
		},
	})
	auditor.SetRules(OpDirDelete, []config.SecurityRule{
		{
			Pattern: tempDir + "/*",
			Action:  "allow",
		},
	})

	wrapper := NewSecureFileWrapper(auditor, "test-user", "test-source", tempDir)

	t.Run("ReadFile", func(t *testing.T) {
		// Create a test file
		testFile := filepath.Join(tempDir, "test.txt")
		err := os.WriteFile(testFile, []byte("test content"), 0644)
		if err != nil {
			t.Fatalf("failed to create test file: %v", err)
		}

		content, err := wrapper.ReadFile(testFile)
		if err != nil {
			t.Errorf("ReadFile() returned error: %v", err)
		}
		if string(content) != "test content" {
			t.Errorf("ReadFile() = %q, want 'test content'", string(content))
		}
	})

	t.Run("WriteFile", func(t *testing.T) {
		testFile := filepath.Join(tempDir, "write-test.txt")
		content := []byte("write test")

		err := wrapper.WriteFile(testFile, content)
		if err != nil {
			t.Errorf("WriteFile() returned error: %v", err)
		}

		// Verify file was written
		readContent, err := os.ReadFile(testFile)
		if err != nil {
			t.Errorf("failed to read written file: %v", err)
		}
		if string(readContent) != string(content) {
			t.Errorf("file content = %q, want %q", string(readContent), string(content))
		}
	})

	t.Run("EditFile", func(t *testing.T) {
		testFile := filepath.Join(tempDir, "edit-test.txt")
		err := os.WriteFile(testFile, []byte("hello world"), 0644)
		if err != nil {
			t.Fatalf("failed to create test file: %v", err)
		}

		err = wrapper.EditFile(testFile, "hello", "goodbye")
		if err != nil {
			t.Errorf("EditFile() returned error: %v", err)
		}

		content, _ := os.ReadFile(testFile)
		if string(content) != "goodbye world" {
			t.Errorf("file content = %q, want 'goodbye world'", string(content))
		}
	})

	t.Run("EditFile old text not found", func(t *testing.T) {
		testFile := filepath.Join(tempDir, "edit-not-found.txt")
		err := os.WriteFile(testFile, []byte("hello world"), 0644)
		if err != nil {
			t.Fatalf("failed to create test file: %v", err)
		}

		err = wrapper.EditFile(testFile, "goodbye", "hello")
		if err == nil {
			t.Error("EditFile() should return error when old text not found")
		}
	})

	t.Run("AppendFile", func(t *testing.T) {
		testFile := filepath.Join(tempDir, "append-test.txt")
		err := os.WriteFile(testFile, []byte("hello"), 0644)
		if err != nil {
			t.Fatalf("failed to create test file: %v", err)
		}

		err = wrapper.AppendFile(testFile, []byte(" world"))
		if err != nil {
			t.Errorf("AppendFile() returned error: %v", err)
		}

		content, _ := os.ReadFile(testFile)
		if string(content) != "hello world" {
			t.Errorf("file content = %q, want 'hello world'", string(content))
		}
	})

	t.Run("DeleteFile", func(t *testing.T) {
		testFile := filepath.Join(tempDir, "delete-test.txt")
		err := os.WriteFile(testFile, []byte("test"), 0644)
		if err != nil {
			t.Fatalf("failed to create test file: %v", err)
		}

		err = wrapper.DeleteFile(testFile)
		if err != nil {
			t.Errorf("DeleteFile() returned error: %v", err)
		}

		if _, err := os.Stat(testFile); !os.IsNotExist(err) {
			t.Error("file should be deleted")
		}
	})

	t.Run("ReadDirectory", func(t *testing.T) {
		// Create test directory with files
		testDir := filepath.Join(tempDir, "readdir-test")
		err := os.MkdirAll(testDir, 0755)
		if err != nil {
			t.Fatalf("failed to create test directory: %v", err)
		}
		os.WriteFile(filepath.Join(testDir, "file1.txt"), []byte("test1"), 0644)
		os.WriteFile(filepath.Join(testDir, "file2.txt"), []byte("test2"), 0644)

		entries, err := wrapper.ReadDirectory(testDir)
		if err != nil {
			t.Errorf("ReadDirectory() returned error: %v", err)
		}
		if len(entries) != 2 {
			t.Errorf("ReadDirectory() = %d entries, want 2", len(entries))
		}
	})

	t.Run("CreateDirectory", func(t *testing.T) {
		testDir := filepath.Join(tempDir, "mkdir-test")
		err := wrapper.CreateDirectory(testDir)
		if err != nil {
			t.Errorf("CreateDirectory() returned error: %v", err)
		}

		if _, err := os.Stat(testDir); os.IsNotExist(err) {
			t.Error("directory should exist")
		}
	})

	t.Run("DeleteDirectory", func(t *testing.T) {
		testDir := filepath.Join(tempDir, "rmdir-test")
		err := os.MkdirAll(testDir, 0755)
		if err != nil {
			t.Fatalf("failed to create test directory: %v", err)
		}

		err = wrapper.DeleteDirectory(testDir)
		if err != nil {
			t.Errorf("DeleteDirectory() returned error: %v", err)
		}

		if _, err := os.Stat(testDir); !os.IsNotExist(err) {
			t.Error("directory should be deleted")
		}
	})
}

func TestSecureProcessWrapper(t *testing.T) {
	auditor := NewSecurityAuditor(&AuditorConfig{
		Enabled:       true,
		DefaultAction: "allow",
	})
	auditor.SetRules(OpProcessExec, []config.SecurityRule{
		{
			Pattern: "ls *",
			Action:  "allow",
		},
	})

	wrapper := NewSecureProcessWrapper(auditor, "test-user", "test-source", "/tmp")

	t.Run("ExecuteCommand safe command", func(t *testing.T) {
		result, err := wrapper.ExecuteCommand("ls -la")
		if err != nil {
			t.Errorf("ExecuteCommand() returned error: %v", err)
		}
		if result == "" {
			t.Error("ExecuteCommand() should return non-empty result")
		}
	})

	t.Run("ExecuteCommand dangerous command", func(t *testing.T) {
		_, err := wrapper.ExecuteCommand("rm -rf /tmp/test")
		if err == nil {
			t.Error("ExecuteCommand() should return error for dangerous command")
		}
	})
}

func TestSecureNetworkWrapper(t *testing.T) {
	auditor := NewSecurityAuditor(&AuditorConfig{
		Enabled:       true,
		DefaultAction: "allow",
	})
	auditor.SetRules(OpNetworkDownload, []config.SecurityRule{
		{
			Pattern: "*.example.com",
			Action:  "allow",
		},
	})

	wrapper := NewSecureNetworkWrapper(auditor, "test-user", "test-source")

	t.Run("DownloadURL valid URL", func(t *testing.T) {
		err := wrapper.DownloadURL("https://example.com/file", "/tmp/file")
		if err != nil {
			t.Errorf("DownloadURL() returned error: %v", err)
		}
	})

	t.Run("DownloadURL invalid scheme", func(t *testing.T) {
		err := wrapper.DownloadURL("file:///etc/passwd", "/tmp/file")
		if err == nil {
			t.Error("DownloadURL() should return error for non-http scheme")
		}
	})
}

func TestSecureHardwareWrapper(t *testing.T) {
	auditor := NewSecurityAuditor(&AuditorConfig{
		Enabled:       true,
		DefaultAction: "allow",
	})

	wrapper := NewSecureHardwareWrapper(auditor, "test-user", "test-source")

	t.Run("I2CWrite", func(t *testing.T) {
		err := wrapper.I2CWrite("1", 0x50, []byte{0x01, 0x02, 0x03})
		if err != nil {
			t.Errorf("I2CWrite() returned error: %v", err)
		}
	})

	t.Run("SPIWrite", func(t *testing.T) {
		err := wrapper.SPIWrite("0.0", []byte{0x01, 0x02, 0x03})
		if err != nil {
			t.Errorf("SPIWrite() returned error: %v", err)
		}
	})
}

func TestSecurityMiddleware(t *testing.T) {
	auditor := NewSecurityAuditor(&AuditorConfig{
		Enabled:       true,
		DefaultAction: "allow",
	})

	middleware := NewSecurityMiddleware(auditor, "test-user", "test-source", "/tmp")

	t.Run("File wrapper", func(t *testing.T) {
		fileWrapper := middleware.File()
		if fileWrapper == nil {
			t.Error("File() should return non-nil wrapper")
		}
		if fileWrapper.user != "test-user" {
			t.Errorf("file wrapper user = %q, want 'test-user'", fileWrapper.user)
		}
	})

	t.Run("Process wrapper", func(t *testing.T) {
		processWrapper := middleware.Process()
		if processWrapper == nil {
			t.Error("Process() should return non-nil wrapper")
		}
	})

	t.Run("Network wrapper", func(t *testing.T) {
		networkWrapper := middleware.Network()
		if networkWrapper == nil {
			t.Error("Network() should return non-nil wrapper")
		}
	})

	t.Run("Hardware wrapper", func(t *testing.T) {
		hardwareWrapper := middleware.Hardware()
		if hardwareWrapper == nil {
			t.Error("Hardware() should return non-nil wrapper")
		}
	})
}

func TestSecurityMiddlewareBatchOperations(t *testing.T) {
	auditor := NewSecurityAuditor(&AuditorConfig{
		Enabled:       true,
		DefaultAction: "allow",
	})
	auditor.SetRules(OpFileRead, []config.SecurityRule{
		{
			Pattern: "/tmp/*",
			Action:  "allow",
		},
	})

	middleware := NewSecurityMiddleware(auditor, "test-user", "test-source", "/tmp")

	t.Run("RequestBatchPermission with operations", func(t *testing.T) {
		batch := &BatchOperationRequest{
			ID: "batch-1",
			Operations: []*OperationRequest{
				{
					Type:        OpFileRead,
					DangerLevel: DangerLow,
					Target:      "/tmp/file1.txt",
				},
				{
					Type:        OpFileRead,
					DangerLevel: DangerLow,
					Target:      "/tmp/file2.txt",
				},
			},
			User:        "test-user",
			Source:      "test-source",
			Description: "Read multiple files",
		}

		allowed, _, requestID := middleware.RequestBatchPermission(context.Background(), batch)
		if !allowed {
			t.Error("RequestBatchPermission() should allow batch operation")
		}
		if requestID == "" {
			t.Error("RequestBatchPermission() should return request ID")
		}
	})

	t.Run("RequestBatchPermission empty operations", func(t *testing.T) {
		batch := &BatchOperationRequest{
			ID:         "batch-2",
			Operations: []*OperationRequest{},
			User:       "test-user",
			Source:     "test-source",
		}

		_, _, requestID := middleware.RequestBatchPermission(context.Background(), batch)
		if requestID != "" {
			t.Error("RequestBatchPermission() should return empty request ID for empty batch")
		}
	})
}

func TestSecurityMiddlewareGetSecuritySummary(t *testing.T) {
	auditor := NewSecurityAuditor(&AuditorConfig{
		Enabled:       true,
		DefaultAction: "allow",
	})

	middleware := NewSecurityMiddleware(auditor, "test-user", "test-source", "/tmp")

	summary := middleware.GetSecuritySummary()
	if summary == nil {
		t.Error("GetSecuritySummary() should return non-nil summary")
	}
	if summary["user"] != "test-user" {
		t.Errorf("summary user = %q, want 'test-user'", summary["user"])
	}
	if summary["source"] != "test-source" {
		t.Errorf("summary source = %q, want 'test-source'", summary["source"])
	}
	if summary["workspace"] != "/tmp" {
		t.Errorf("summary workspace = %q, want '/tmp'", summary["workspace"])
	}
}

func TestSecurityMiddlewareApproveDenyPendingRequest(t *testing.T) {
	auditor := NewSecurityAuditor(&AuditorConfig{
		Enabled:       true,
		DefaultAction: "deny",
	})

	middleware := NewSecurityMiddleware(auditor, "test-user", "test-source", "/tmp")

	// Create a pending request
	req := &OperationRequest{
		Type:        OpFileWrite,
		DangerLevel: DangerHigh,
		User:        "test-user",
		Source:      "test-source",
		Target:      "/tmp/test.txt",
	}
	auditor.activeRequests[req.ID] = req

	t.Run("ApprovePendingRequest", func(t *testing.T) {
		err := middleware.ApprovePendingRequest(req.ID)
		if err != nil {
			t.Errorf("ApprovePendingRequest() returned error: %v", err)
		}
	})

	t.Run("DenyPendingRequest", func(t *testing.T) {
		req2 := &OperationRequest{
			Type:        OpFileWrite,
			DangerLevel: DangerHigh,
			User:        "test-user",
			Source:      "test-source",
			Target:      "/tmp/test2.txt",
		}
		auditor.activeRequests[req2.ID] = req2

		err := middleware.DenyPendingRequest(req2.ID, "test reason")
		if err != nil {
			t.Errorf("DenyPendingRequest() returned error: %v", err)
		}
	})
}

func TestSecurityMiddlewareGetAuditLog(t *testing.T) {
	auditor := NewSecurityAuditor(&AuditorConfig{
		Enabled:       true,
		DefaultAction: "allow",
	})

	middleware := NewSecurityMiddleware(auditor, "test-user", "test-source", "/tmp")

	// Add an audit event
	req := &OperationRequest{
		Type:        OpFileRead,
		DangerLevel: DangerLow,
		User:        "test-user",
		Source:      "test-source",
		Target:      "/tmp/test.txt",
	}
	auditor.RequestPermission(context.Background(), req)

	filter := AuditFilter{
		OperationType: OpFileRead,
	}
	logs := middleware.GetAuditLog(filter)
	if len(logs) == 0 {
		t.Error("GetAuditLog() should return at least one event")
	}
}

func TestSecurityMiddlewareExportAuditLog(t *testing.T) {
	auditor := NewSecurityAuditor(&AuditorConfig{
		Enabled:       true,
		DefaultAction: "allow",
	})

	middleware := NewSecurityMiddleware(auditor, "test-user", "test-source", "/tmp")

	// Add an audit event
	req := &OperationRequest{
		Type:        OpFileRead,
		DangerLevel: DangerLow,
		User:        "test-user",
		Source:      "test-source",
		Target:      "/tmp/test.txt",
	}
	auditor.RequestPermission(context.Background(), req)

	tempFile := filepath.Join(os.TempDir(), "audit-log-test.csv")
	defer os.Remove(tempFile)

	err := middleware.ExportAuditLog(tempFile)
	if err != nil {
		t.Errorf("ExportAuditLog() returned error: %v", err)
	}

	// Check if file exists
	if _, err := os.Stat(tempFile); os.IsNotExist(err) {
		t.Error("export file should exist")
	}
}

func TestCreatePermissions(t *testing.T) {
	t.Run("CreateCLIPermission", func(t *testing.T) {
		perm := CreateCLIPermission()
		if perm == nil {
			t.Error("CreateCLIPermission() should return non-nil permission")
		}
		if !perm.AllowedTypes[OpFileRead] {
			t.Error("CLI permission should allow file_read")
		}
		if perm.MaxDangerLevel != DangerHigh {
			t.Errorf("CLI permission max danger = %q, want HIGH", perm.MaxDangerLevel)
		}
	})

	t.Run("CreateWebPermission", func(t *testing.T) {
		perm := CreateWebPermission()
		if perm == nil {
			t.Error("CreateWebPermission() should return non-nil permission")
		}
		if !perm.AllowedTypes[OpFileRead] {
			t.Error("Web permission should allow file_read")
		}
		if perm.AllowedTypes[OpProcessExec] {
			t.Error("Web permission should not allow process_exec")
		}
		if perm.MaxDangerLevel != DangerMedium {
			t.Errorf("Web permission max danger = %q, want MEDIUM", perm.MaxDangerLevel)
		}
	})

	t.Run("CreateAgentPermission", func(t *testing.T) {
		agentID := "test-agent"
		perm := CreateAgentPermission(agentID)
		if perm == nil {
			t.Error("CreateAgentPermission() should return non-nil permission")
		}
		if !perm.AllowedTypes[OpFileRead] {
			t.Error("Agent permission should allow file_read")
		}
		if !perm.RequireApproval[OpFileDelete] {
			t.Error("Agent permission should require approval for file_delete")
		}
	})
}

func TestMonitorSecurityStatus(t *testing.T) {
	auditor := NewSecurityAuditor(&AuditorConfig{
		Enabled:               true,
		AuditLogRetentionDays: 1,
	})

	ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
	defer cancel()

	// This should not panic
	MonitorSecurityStatus(ctx, auditor, 50*time.Millisecond)
}

func TestGlobalAuditor(t *testing.T) {
	t.Run("InitGlobalAuditor", func(t *testing.T) {
		auditor := InitGlobalAuditor(&AuditorConfig{
			Enabled: true,
		})
		if auditor == nil {
			t.Error("InitGlobalAuditor() should return non-nil auditor")
		}
	})

	t.Run("GetGlobalAuditor", func(t *testing.T) {
		// Reset global auditor
		globalAuditor = nil
		auditorOnce = sync.Once{}

		auditor := GetGlobalAuditor()
		if auditor == nil {
			t.Error("GetGlobalAuditor() should return non-nil auditor")
		}
	})

	t.Run("InitGlobalAuditor idempotent", func(t *testing.T) {
		// Reset
		globalAuditor = nil
		auditorOnce = sync.Once{}

		auditor1 := InitGlobalAuditor(&AuditorConfig{Enabled: true})
		auditor2 := InitGlobalAuditor(&AuditorConfig{Enabled: false})
		if auditor1 != auditor2 {
			t.Error("InitGlobalAuditor() should be idempotent")
		}
	})
}
