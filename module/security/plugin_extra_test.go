package security

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/plugin"
)

// --- Operation type constants tests ---

func TestOperationTypeConstants(t *testing.T) {
	ops := []OperationType{
		OpFileRead, OpFileWrite, OpFileDelete,
		OpDirRead, OpDirCreate, OpDirDelete,
		OpProcessExec, OpProcessSpawn, OpProcessKill, OpProcessSuspend,
		OpNetworkDownload, OpNetworkUpload, OpNetworkRequest,
		OpHardwareI2C, OpHardwareSPI, OpHardwareGPIO,
		OpSystemShutdown, OpSystemReboot, OpSystemConfig, OpSystemService, OpSystemInstall,
		OpRegistryRead, OpRegistryWrite, OpRegistryDelete,
	}

	seen := make(map[OperationType]bool)
	for _, op := range ops {
		if seen[op] {
			t.Errorf("Duplicate operation type: %s", op)
		}
		seen[op] = true
	}
	if len(seen) != 24 {
		t.Errorf("Expected 24 distinct operation types, got %d", len(seen))
	}
}

// --- DangerLevel tests ---

func TestDangerLevel_String(t *testing.T) {
	tests := []struct {
		level    DangerLevel
		expected string
	}{
		{DangerLow, "LOW"},
		{DangerMedium, "MEDIUM"},
		{DangerHigh, "HIGH"},
		{DangerCritical, "CRITICAL"},
		{DangerLevel(99), "UNKNOWN"},
	}
	for _, tt := range tests {
		if got := tt.level.String(); got != tt.expected {
			t.Errorf("DangerLevel(%d).String() = %s, want %s", tt.level, got, tt.expected)
		}
	}
}

// --- GetDangerLevel tests ---

func TestGetDangerLevel_AllTypes(t *testing.T) {
	tests := []struct {
		opType  OperationType
		danger  DangerLevel
	}{
		{OpFileRead, DangerLow},
		{OpDirRead, DangerLow},
		{OpNetworkDownload, DangerMedium},
		{OpNetworkRequest, DangerMedium},
		{OpFileWrite, DangerHigh},
		{OpFileDelete, DangerHigh},
		{OpDirCreate, DangerHigh},
		{OpDirDelete, DangerHigh},
		{OpProcessSpawn, DangerHigh},
		{OpProcessExec, DangerCritical},
		{OpProcessKill, DangerCritical},
		{OpSystemShutdown, DangerCritical},
		{OpRegistryWrite, DangerCritical},
		{OpRegistryDelete, DangerCritical},
		// Default case
		{OpHardwareI2C, DangerMedium},
		{OpHardwareSPI, DangerMedium},
		{OpHardwareGPIO, DangerMedium},
	}
	for _, tt := range tests {
		if got := GetDangerLevel(tt.opType); got != tt.danger {
			t.Errorf("GetDangerLevel(%s) = %s, want %s", tt.opType, got, tt.danger)
		}
	}
}

// --- ValidatePath tests ---

func TestValidatePath_WithinWorkspace(t *testing.T) {
	tmpDir := t.TempDir()
	testFile := filepath.Join(tmpDir, "test.txt")
	if err := os.WriteFile(testFile, []byte("test"), 0644); err != nil {
		t.Fatal(err)
	}

	result, err := ValidatePath(testFile, tmpDir, OpFileRead)
	if err != nil {
		t.Errorf("ValidatePath failed: %v", err)
	}
	if result == "" {
		t.Error("Expected non-empty result path")
	}
}

func TestValidatePath_OutsideWorkspace(t *testing.T) {
	tmpDir := t.TempDir()
	outsidePath := filepath.Join(os.TempDir(), "outside_test.txt")

	_, err := ValidatePath(outsidePath, tmpDir, OpFileRead)
	if err == nil {
		t.Error("Expected error for path outside workspace")
	}
}

func TestValidatePath_NoWorkspace(t *testing.T) {
	result, err := ValidatePath("somefile.txt", "", OpFileRead)
	if err != nil {
		t.Errorf("ValidatePath with empty workspace should not fail: %v", err)
	}
	if result == "" {
		t.Error("Expected non-empty result")
	}
}

func TestValidatePath_DangerousSystemPaths(t *testing.T) {
	// Skip on Windows as /etc paths are not meaningful system paths there
	if runtime.GOOS == "windows" {
		t.Skip("Skipping Unix-specific path tests on Windows")
	}
	dangerousPaths := []string{
		"/etc/passwd",
		"/etc/shadow",
		"/etc/sudoers",
	}

	for _, dp := range dangerousPaths {
		t.Run(dp, func(t *testing.T) {
			_, err := ValidatePath(dp, "", OpFileRead)
			if err == nil {
				t.Errorf("Expected error for dangerous path: %s", dp)
			}
		})
	}
}

// --- IsSafeCommand tests ---

func TestIsSafeCommand_SafeCommands(t *testing.T) {
	safeCommands := []string{
		"ls -la",
		"echo hello",
		"git status",
		"go test ./...",
		"dir",
		"type readme.txt",
	}
	for _, cmd := range safeCommands {
		safe, reason := IsSafeCommand(cmd)
		if !safe {
			t.Errorf("Expected '%s' to be safe, got: %s", cmd, reason)
		}
	}
}

func TestIsSafeCommand_DangerousCommands(t *testing.T) {
	dangerousCommands := []string{
		"rm -rf /",
		"del /f /q C:\\*",
		"format C:",
		"mkfs.ext4 /dev/sda",
		"dd if=/dev/zero of=/dev/sda",
		"shutdown -h now",
		"sudo apt-get install malware",
		"chmod 777 /etc/passwd",
		"chown root:root /tmp/exploit",
	}
	for _, cmd := range dangerousCommands {
		safe, reason := IsSafeCommand(cmd)
		if safe {
			t.Errorf("Expected '%s' to be blocked", cmd)
		}
		if reason == "" {
			t.Errorf("Expected non-empty reason for blocked command: %s", cmd)
		}
	}
}

// --- SecurityAuditor tests ---

func TestNewSecurityAuditor_DefaultConfig(t *testing.T) {
	sa := NewSecurityAuditor(nil)
	if sa == nil {
		t.Fatal("NewSecurityAuditor returned nil")
	}
	if !sa.enabled {
		t.Error("Auditor should be enabled by default")
	}
	if sa.defaultAction != "deny" {
		t.Errorf("Expected default action 'deny', got '%s'", sa.defaultAction)
	}
}

func TestNewSecurityAuditor_CustomConfig(t *testing.T) {
	cfg := &AuditorConfig{
		Enabled:            true,
		LogAllOperations:   true,
		ApprovalTimeout:    10 * time.Second,
		MaxPendingRequests: 50,
		DefaultAction:      "allow",
	}
	sa := NewSecurityAuditor(cfg)
	if sa.defaultAction != "allow" {
		t.Errorf("Expected 'allow', got '%s'", sa.defaultAction)
	}
}

func TestNewSecurityAuditor_WithLogFile(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := &AuditorConfig{
		Enabled:             true,
		AuditLogFileEnabled: true,
		AuditLogDir:         tmpDir,
		DefaultAction:       "deny",
	}
	sa := NewSecurityAuditor(cfg)
	defer sa.Close()

	if sa.logFile == nil {
		t.Error("Expected log file to be initialized")
	}

	// Check that the file was created
	files, err := os.ReadDir(tmpDir)
	if err != nil {
		t.Fatal(err)
	}
	if len(files) == 0 {
		t.Error("Expected log file to be created in directory")
	}
}

func TestSecurityAuditor_EnableDisable(t *testing.T) {
	sa := NewSecurityAuditor(nil)

	sa.Disable()
	if sa.enabled {
		t.Error("Should be disabled")
	}

	sa.Enable()
	if !sa.enabled {
		t.Error("Should be enabled")
	}
}

func TestSecurityAuditor_SetRules(t *testing.T) {
	sa := NewSecurityAuditor(nil)
	sa.SetRules(OpFileRead, nil) // no panic on nil
}

func TestSecurityAuditor_SetDefaultAction(t *testing.T) {
	sa := NewSecurityAuditor(nil)
	sa.SetDefaultAction("allow")
	if sa.defaultAction != "allow" {
		t.Errorf("Expected 'allow', got '%s'", sa.defaultAction)
	}
}

func TestSecurityAuditor_RequestPermission_Disabled(t *testing.T) {
	cfg := &AuditorConfig{Enabled: false}
	sa := NewSecurityAuditor(cfg)

	req := &OperationRequest{
		Type:   OpProcessExec,
		Target: "rm -rf /",
	}

	allowed, err, reqID := sa.RequestPermission(context.Background(), req)
	if !allowed {
		t.Error("Disabled auditor should allow everything")
	}
	if err != nil {
		t.Errorf("Unexpected error: %v", err)
	}
	if reqID == "" {
		t.Error("Expected non-empty request ID")
	}
}

func TestSecurityAuditor_RequestPermission_AllowByDefault(t *testing.T) {
	cfg := &AuditorConfig{
		Enabled:       true,
		DefaultAction: "allow",
	}
	sa := NewSecurityAuditor(cfg)

	req := &OperationRequest{
		Type:   OpFileRead,
		Target: "/some/file",
	}

	allowed, err, _ := sa.RequestPermission(context.Background(), req)
	if !allowed {
		t.Error("Default allow should permit operation")
	}
	if err != nil {
		t.Errorf("Unexpected error: %v", err)
	}
}

func TestSecurityAuditor_RequestPermission_DenyByDefault(t *testing.T) {
	cfg := &AuditorConfig{
		Enabled:       true,
		DefaultAction: "deny",
	}
	sa := NewSecurityAuditor(cfg)

	req := &OperationRequest{
		Type:   OpFileRead,
		Target: "/some/file",
	}

	allowed, err, _ := sa.RequestPermission(context.Background(), req)
	if allowed {
		t.Error("Default deny should block operation")
	}
	if err == nil {
		t.Error("Expected error for denied operation")
	}
}

func TestSecurityAuditor_RequestPermission_RequireApproval(t *testing.T) {
	cfg := &AuditorConfig{
		Enabled:       true,
		DefaultAction: "ask", // maps to require_approval
	}
	sa := NewSecurityAuditor(cfg)

	req := &OperationRequest{
		Type:   OpProcessExec,
		Target: "some-command",
	}

	allowed, err, reqID := sa.RequestPermission(context.Background(), req)
	if allowed {
		t.Error("Should not be allowed without approval")
	}
	if err == nil {
		t.Error("Expected error for pending approval")
	}
	if reqID == "" {
		t.Error("Expected non-empty request ID")
	}

	// Should be in pending requests
	pending := sa.GetPendingRequests()
	if len(pending) == 0 {
		t.Error("Expected pending request")
	}
}

func TestSecurityAuditor_ApproveRequest(t *testing.T) {
	cfg := &AuditorConfig{
		Enabled:       true,
		DefaultAction: "ask",
	}
	sa := NewSecurityAuditor(cfg)

	req := &OperationRequest{
		Type:   OpProcessExec,
		Target: "some-command",
	}

	_, _, reqID := sa.RequestPermission(context.Background(), req)

	err := sa.ApproveRequest(reqID, "admin")
	if err != nil {
		t.Errorf("ApproveRequest failed: %v", err)
	}

	// Should no longer be in pending
	pending := sa.GetPendingRequests()
	if len(pending) != 0 {
		t.Error("Expected no pending requests after approval")
	}
}

func TestSecurityAuditor_DenyRequest(t *testing.T) {
	cfg := &AuditorConfig{
		Enabled:       true,
		DefaultAction: "ask",
	}
	sa := NewSecurityAuditor(cfg)

	req := &OperationRequest{
		Type:   OpProcessExec,
		Target: "some-command",
	}

	_, _, reqID := sa.RequestPermission(context.Background(), req)

	err := sa.DenyRequest(reqID, "admin", "too dangerous")
	if err != nil {
		t.Errorf("DenyRequest failed: %v", err)
	}
}

func TestSecurityAuditor_ApproveRequest_NotFound(t *testing.T) {
	sa := NewSecurityAuditor(nil)
	err := sa.ApproveRequest("nonexistent", "admin")
	if err == nil {
		t.Error("Expected error for non-existent request")
	}
}

func TestSecurityAuditor_DenyRequest_NotFound(t *testing.T) {
	sa := NewSecurityAuditor(nil)
	err := sa.DenyRequest("nonexistent", "admin", "reason")
	if err == nil {
		t.Error("Expected error for non-existent request")
	}
}

func TestSecurityAuditor_GetStatistics(t *testing.T) {
	cfg := &AuditorConfig{
		Enabled:       true,
		DefaultAction: "allow",
	}
	sa := NewSecurityAuditor(cfg)

	req := &OperationRequest{Type: OpFileRead, Target: "/test"}
	sa.RequestPermission(context.Background(), req)

	stats := sa.GetStatistics()
	if stats == nil {
		t.Fatal("GetStatistics returned nil")
	}
	if stats["total_events"].(int64) != 1 {
		t.Errorf("Expected 1 total event, got %v", stats["total_events"])
	}
	if stats["allowed"].(int64) != 1 {
		t.Errorf("Expected 1 allowed, got %v", stats["allowed"])
	}
}

func TestSecurityAuditor_GetAuditLog(t *testing.T) {
	sa := NewSecurityAuditor(nil)
	events := sa.GetAuditLog(AuditFilter{})
	if events == nil {
		t.Error("GetAuditLog should not return nil")
	}
	if len(events) != 0 {
		t.Errorf("Expected empty events, got %d", len(events))
	}
}

func TestSecurityAuditor_CleanupOldAuditLogs(t *testing.T) {
	sa := NewSecurityAuditor(nil)
	err := sa.CleanupOldAuditLogs()
	if err != nil {
		t.Errorf("CleanupOldAuditLogs should return nil: %v", err)
	}
}

func TestSecurityAuditor_ExportAuditLog(t *testing.T) {
	tmpDir := t.TempDir()
	exportPath := filepath.Join(tmpDir, "export.json")

	sa := NewSecurityAuditor(nil)
	err := sa.ExportAuditLog(exportPath)
	if err != nil {
		t.Errorf("ExportAuditLog failed: %v", err)
	}

	// File should exist
	if _, err := os.Stat(exportPath); os.IsNotExist(err) {
		t.Error("Export file should be created")
	}
}

func TestSecurityAuditor_ExportAuditLog_WithLogFile(t *testing.T) {
	logDir := t.TempDir()
	exportDir := t.TempDir()

	cfg := &AuditorConfig{
		Enabled:             true,
		AuditLogFileEnabled: true,
		AuditLogDir:         logDir,
		DefaultAction:       "allow",
	}
	sa := NewSecurityAuditor(cfg)
	defer sa.Close()

	// Generate an event
	req := &OperationRequest{Type: OpFileRead, Target: "/test"}
	sa.RequestPermission(context.Background(), req)

	exportPath := filepath.Join(exportDir, "export.json")
	err := sa.ExportAuditLog(exportPath)
	if err != nil {
		t.Errorf("ExportAuditLog failed: %v", err)
	}
}

func TestSecurityAuditor_Close(t *testing.T) {
	sa := NewSecurityAuditor(nil)
	err := sa.Close()
	if err != nil {
		t.Errorf("Close failed: %v", err)
	}

	// Double close should not error
	err = sa.Close()
	if err != nil {
		t.Errorf("Double close failed: %v", err)
	}
}

func TestSecurityAuditor_SetApprovalManager(t *testing.T) {
	sa := NewSecurityAuditor(nil)
	sa.SetApprovalManager(nil)
	if sa.GetApprovalManager() != nil {
		t.Error("Expected nil approval manager")
	}
}

// --- normalizeDecision tests ---

func TestNormalizeDecisionExtra(t *testing.T) {
	tests := []struct {
		input    string
		expected string
	}{
		{"allow", "allowed"},
		{"allowed", "allowed"},
		{"deny", "denied"},
		{"denied", "denied"},
		{"ask", "require_approval"},
		{"require_approval", "require_approval"},
		{"something_else", "something_else"},
	}
	for _, tt := range tests {
		if got := normalizeDecision(tt.input); got != tt.expected {
			t.Errorf("normalizeDecision(%q) = %q, want %q", tt.input, got, tt.expected)
		}
	}
}

// --- ApprovalRequiredError tests ---

func TestApprovalRequiredErrorExtra(t *testing.T) {
	err := &ApprovalRequiredError{RequestID: "req-123", Reason: "test"}
	if err.Error() == "" {
		t.Error("Error message should not be empty")
	}
	if !strings.Contains(err.Error(), "req-123") {
		t.Error("Error should contain request ID")
	}
	if !err.IsApprovalRequired() {
		t.Error("IsApprovalRequired should return true")
	}
}

// --- AuditFilter tests ---

func TestAuditFilter_IsEmpty(t *testing.T) {
	filter := AuditFilter{}
	if !filter.IsEmpty() {
		t.Error("Empty filter should return true")
	}

	filter.OperationType = OpFileRead
	if filter.IsEmpty() {
		t.Error("Non-empty filter should return false")
	}
}

func TestAuditFilter_Matches(t *testing.T) {
	now := time.Now()
	event := AuditEvent{
		Request:   OperationRequest{Type: OpFileRead, User: "admin", Source: "cli"},
		Decision:  "allowed",
		Timestamp: now,
	}

	tests := []struct {
		name   string
		filter AuditFilter
		match  bool
	}{
		{"empty filter matches", AuditFilter{}, true},
		{"matching operation", AuditFilter{OperationType: OpFileRead}, true},
		{"non-matching operation", AuditFilter{OperationType: OpFileWrite}, false},
		{"matching user", AuditFilter{User: "admin"}, true},
		{"non-matching user", AuditFilter{User: "guest"}, false},
		{"matching decision", AuditFilter{Decision: "allowed"}, true},
		{"non-matching decision", AuditFilter{Decision: "denied"}, false},
		{"matching source regex", AuditFilter{Source: "cl.*"}, true},
		{"non-matching source regex", AuditFilter{Source: "web.*"}, false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := tt.filter.Matches(event); got != tt.match {
				t.Errorf("Matches() = %v, want %v", got, tt.match)
			}
		})
	}
}

func TestAuditFilter_Matches_TimeRange(t *testing.T) {
	now := time.Now()
	event := AuditEvent{Timestamp: now}

	before := now.Add(-1 * time.Hour)
	after := now.Add(1 * time.Hour)

	// Start time filter
	f1 := AuditFilter{StartTime: &before}
	if !f1.Matches(event) {
		t.Error("Event after start time should match")
	}

	f2 := AuditFilter{StartTime: &after}
	if f2.Matches(event) {
		t.Error("Event before start time should not match")
	}

	// End time filter
	f3 := AuditFilter{EndTime: &after}
	if !f3.Matches(event) {
		t.Error("Event before end time should match")
	}

	f4 := AuditFilter{EndTime: &before}
	if f4.Matches(event) {
		t.Error("Event after end time should not match")
	}
}

// --- convertContextToStringMap tests ---

func TestConvertContextToStringMapExtra(t *testing.T) {
	ctx := map[string]interface{}{
		"key1": "value1",
		"key2": 42,
		"key3": true,
	}
	result := convertContextToStringMap(ctx)
	if result["key1"] != "value1" {
		t.Errorf("Expected 'value1', got '%s'", result["key1"])
	}
	if result["key2"] != "42" {
		t.Errorf("Expected '42', got '%s'", result["key2"])
	}
	if result["key3"] != "true" {
		t.Errorf("Expected 'true', got '%s'", result["key3"])
	}
}

// --- SecurityPlugin toolToOperation tests ---

func TestSecurityPlugin_ToolToOperation(t *testing.T) {
	p := NewSecurityPlugin()

	tests := []struct {
		tool   string
		opType OperationType
	}{
		{"read_file", OpFileRead},
		{"write_file", OpFileWrite},
		{"edit_file", OpFileWrite},
		{"append_file", OpFileWrite},
		{"delete_file", OpFileDelete},
		{"list_directory", OpDirRead},
		{"list_dir", OpDirRead},
		{"create_directory", OpDirCreate},
		{"create_dir", OpDirCreate},
		{"delete_directory", OpDirDelete},
		{"delete_dir", OpDirDelete},
		{"exec", OpProcessExec},
		{"execute_command", OpProcessExec},
		{"spawn", OpProcessSpawn},
		{"kill", OpProcessKill},
		{"kill_process", OpProcessKill},
		{"download", OpNetworkDownload},
		{"upload", OpNetworkUpload},
		{"http_request", OpNetworkRequest},
		{"web_request", OpNetworkRequest},
		{"unknown_tool", OperationType("")},
	}

	for _, tt := range tests {
		t.Run(tt.tool, func(t *testing.T) {
			if got := p.toolToOperation(tt.tool); got != tt.opType {
				t.Errorf("toolToOperation(%q) = %q, want %q", tt.tool, got, tt.opType)
			}
		})
	}
}

// --- SecurityPlugin extractTarget tests ---

func TestSecurityPlugin_ExtractTarget(t *testing.T) {
	p := NewSecurityPlugin()

	tests := []struct {
		tool   string
		args   map[string]interface{}
		target string
	}{
		{"read_file", map[string]interface{}{"path": "/etc/passwd"}, "/etc/passwd"},
		{"read_file", map[string]interface{}{}, ""},
		{"write_file", map[string]interface{}{"path": "C:\\test.txt"}, "C:\\test.txt"},
		{"exec", map[string]interface{}{"command": "ls -la"}, "ls -la"},
		{"spawn", map[string]interface{}{"command": "test"}, "test"},
		{"download", map[string]interface{}{"url": "http://example.com"}, "http://example.com"},
		{"upload", map[string]interface{}{"url": "http://example.com/upload"}, "http://example.com/upload"},
		{"http_request", map[string]interface{}{"url": "http://api.test.com"}, "http://api.test.com"},
		{"list_directory", map[string]interface{}{"path": "/home"}, "/home"},
		{"unknown", map[string]interface{}{"data": "something"}, ""},
	}

	for _, tt := range tests {
		t.Run(tt.tool, func(t *testing.T) {
			if got := p.extractTarget(tt.tool, tt.args); got != tt.target {
				t.Errorf("extractTarget(%q, ...) = %q, want %q", tt.tool, got, tt.target)
			}
		})
	}
}

// --- SecurityPlugin extractURL tests ---

func TestSecurityPlugin_ExtractURL(t *testing.T) {
	p := NewSecurityPlugin()

	tests := []struct {
		tool   string
		args   map[string]interface{}
		urlStr string
	}{
		{"download", map[string]interface{}{"url": "http://example.com"}, "http://example.com"},
		{"upload", map[string]interface{}{"url": "https://api.test.com"}, "https://api.test.com"},
		{"http_request", map[string]interface{}{"url": "http://api.example.com/v1"}, "http://api.example.com/v1"},
		{"web_request", map[string]interface{}{"url": "http://test.com"}, "http://test.com"},
		{"read_file", map[string]interface{}{"url": "http://test.com"}, ""}, // not a URL tool
		{"download", map[string]interface{}{"path": "/test"}, ""},          // no URL arg
	}

	for _, tt := range tests {
		t.Run(tt.tool, func(t *testing.T) {
			if got := p.extractURL(tt.tool, tt.args); got != tt.urlStr {
				t.Errorf("extractURL(%q, ...) = %q, want %q", tt.tool, got, tt.urlStr)
			}
		})
	}
}

// --- SecurityPlugin Execute tests ---

func TestSecurityPlugin_Execute_Disabled(t *testing.T) {
	p := NewSecurityPlugin()
	p.enabled = false

	allowed, err, critical := p.Execute(context.Background(), &plugin.ToolInvocation{
		ToolName: "exec",
		Args:     map[string]interface{}{"command": "rm -rf /"},
	})
	if !allowed {
		t.Error("Disabled plugin should allow everything")
	}
	if err != nil {
		t.Errorf("Unexpected error: %v", err)
	}
	if critical {
		t.Error("Should not be critical")
	}
}

func TestSecurityPlugin_Execute_UnknownTool(t *testing.T) {
	p := NewSecurityPlugin()
	p.enabled = true
	p.auditor = NewSecurityAuditor(&AuditorConfig{Enabled: true, DefaultAction: "allow"})

	allowed, err, _ := p.Execute(context.Background(), &plugin.ToolInvocation{
		ToolName: "unknown_tool",
		Args:     map[string]interface{}{},
	})
	if !allowed {
		t.Error("Unknown tool should be allowed by default")
	}
	if err != nil {
		t.Errorf("Unexpected error: %v", err)
	}
}

// --- SecurityPlugin Init/Cleanup lifecycle ---

func TestSecurityPlugin_Init_Disabled(t *testing.T) {
	p := NewSecurityPlugin()
	pluginConfig := map[string]interface{}{
		"enabled":     false,
		"config_path": filepath.Join(t.TempDir(), "nonexistent.json"),
	}
	// Should not fail even if config file doesn't exist when disabled
	_ = p.Init(pluginConfig)
}

func TestSecurityPlugin_Cleanup_Nil(t *testing.T) {
	p := NewSecurityPlugin()
	err := p.Cleanup()
	if err != nil {
		t.Errorf("Cleanup on fresh plugin should succeed: %v", err)
	}
}

func TestSecurityPlugin_IsEnabled(t *testing.T) {
	p := NewSecurityPlugin()
	if p.IsEnabled() {
		t.Error("Should be disabled by default")
	}

	p.SetEnabled(true)
	if !p.IsEnabled() {
		t.Error("Should be enabled after SetEnabled(true)")
	}
}

func TestSecurityPlugin_GetAuditor(t *testing.T) {
	p := NewSecurityPlugin()
	if p.GetAuditor() != nil {
		t.Error("Should be nil before init")
	}
}

// --- SecureFileWrapper tests ---

func TestSecureFileWrapper_ReadFile_PathOutsideWorkspace(t *testing.T) {
	sa := NewSecurityAuditor(&AuditorConfig{Enabled: true, DefaultAction: "allow"})
	w := NewSecureFileWrapper(sa, "user", "cli", t.TempDir())

	_, err := w.ReadFile("../../etc/passwd")
	if err == nil {
		t.Error("Expected error for path outside workspace")
	}
}

func TestSecureFileWrapper_WriteFile_PathOutsideWorkspace(t *testing.T) {
	sa := NewSecurityAuditor(&AuditorConfig{Enabled: true, DefaultAction: "allow"})
	w := NewSecureFileWrapper(sa, "user", "cli", t.TempDir())

	err := w.WriteFile("../../tmp/test.txt", []byte("test"))
	if err == nil {
		t.Error("Expected error for path outside workspace")
	}
}

func TestSecureFileWrapper_EditFile_PathOutsideWorkspace(t *testing.T) {
	sa := NewSecurityAuditor(&AuditorConfig{Enabled: true, DefaultAction: "allow"})
	w := NewSecureFileWrapper(sa, "user", "cli", t.TempDir())

	err := w.EditFile("../../tmp/test.txt", "old", "new")
	if err == nil {
		t.Error("Expected error for path outside workspace")
	}
}

func TestSecureFileWrapper_DeleteFile_Denied(t *testing.T) {
	sa := NewSecurityAuditor(&AuditorConfig{Enabled: true, DefaultAction: "deny"})
	tmpDir := t.TempDir()
	w := NewSecureFileWrapper(sa, "user", "cli", tmpDir)

	testFile := filepath.Join(tmpDir, "test.txt")
	if err := os.WriteFile(testFile, []byte("test"), 0644); err != nil {
		t.Fatal(err)
	}

	err := w.DeleteFile(testFile)
	if err == nil {
		t.Error("Expected permission denied")
	}
}

func TestSecureFileWrapper_ReadDirectory_Denied(t *testing.T) {
	sa := NewSecurityAuditor(&AuditorConfig{Enabled: true, DefaultAction: "deny"})
	tmpDir := t.TempDir()
	w := NewSecureFileWrapper(sa, "user", "cli", tmpDir)

	_, err := w.ReadDirectory(tmpDir)
	if err == nil {
		t.Error("Expected permission denied")
	}
}

func TestSecureFileWrapper_CreateDirectory_Denied(t *testing.T) {
	sa := NewSecurityAuditor(&AuditorConfig{Enabled: true, DefaultAction: "deny"})
	tmpDir := t.TempDir()
	w := NewSecureFileWrapper(sa, "user", "cli", tmpDir)

	err := w.CreateDirectory(filepath.Join(tmpDir, "newdir"))
	if err == nil {
		t.Error("Expected permission denied")
	}
}

func TestSecureFileWrapper_DeleteDirectory_Denied(t *testing.T) {
	sa := NewSecurityAuditor(&AuditorConfig{Enabled: true, DefaultAction: "deny"})
	tmpDir := t.TempDir()
	w := NewSecureFileWrapper(sa, "user", "cli", tmpDir)

	err := w.DeleteDirectory(tmpDir)
	if err == nil {
		t.Error("Expected permission denied")
	}
}

// --- SecureProcessWrapper tests ---

func TestSecureProcessWrapper_DangerousCommand(t *testing.T) {
	sa := NewSecurityAuditor(&AuditorConfig{Enabled: true, DefaultAction: "allow"})
	w := NewSecureProcessWrapper(sa, "user", "cli", "/tmp")

	_, err := w.ExecuteCommand("rm -rf /")
	if err == nil {
		t.Error("Expected error for dangerous command")
	}
}

func TestSecureProcessWrapper_DeniedByPolicy(t *testing.T) {
	sa := NewSecurityAuditor(&AuditorConfig{Enabled: true, DefaultAction: "deny"})
	w := NewSecureProcessWrapper(sa, "user", "cli", "/tmp")

	_, err := w.ExecuteCommand("echo hello")
	if err == nil {
		t.Error("Expected permission denied")
	}
}

func TestSecureProcessWrapper_SafeCommand_Allowed(t *testing.T) {
	sa := NewSecurityAuditor(&AuditorConfig{Enabled: true, DefaultAction: "allow"})
	w := NewSecureProcessWrapper(sa, "user", "cli", "/tmp")

	result, err := w.ExecuteCommand("echo hello")
	if err != nil {
		t.Errorf("Unexpected error: %v", err)
	}
	if result == "" {
		t.Error("Expected non-empty result")
	}
}

// --- SecureNetworkWrapper tests ---

func TestSecureNetworkWrapper_InvalidScheme(t *testing.T) {
	sa := NewSecurityAuditor(&AuditorConfig{Enabled: true, DefaultAction: "allow"})
	w := NewSecureNetworkWrapper(sa, "user", "cli")

	err := w.DownloadURL("ftp://example.com/file", "/tmp/file")
	if err == nil {
		t.Error("Expected error for non-http URL")
	}
}

func TestSecureNetworkWrapper_DeniedByPolicy(t *testing.T) {
	sa := NewSecurityAuditor(&AuditorConfig{Enabled: true, DefaultAction: "deny"})
	w := NewSecureNetworkWrapper(sa, "user", "cli")

	err := w.DownloadURL("http://example.com/file", "/tmp/file")
	if err == nil {
		t.Error("Expected permission denied")
	}
}

func TestSecureNetworkWrapper_Allowed(t *testing.T) {
	sa := NewSecurityAuditor(&AuditorConfig{Enabled: true, DefaultAction: "allow"})
	w := NewSecureNetworkWrapper(sa, "user", "cli")

	err := w.DownloadURL("http://example.com/file", "/tmp/file")
	if err != nil {
		t.Errorf("Unexpected error: %v", err)
	}
}

// --- SecureHardwareWrapper tests ---

func TestSecureHardwareWrapper_I2CWrite_Denied(t *testing.T) {
	sa := NewSecurityAuditor(&AuditorConfig{Enabled: true, DefaultAction: "deny"})
	w := NewSecureHardwareWrapper(sa, "user", "cli")

	err := w.I2CWrite("1", 0x50, []byte{0x00})
	if err == nil {
		t.Error("Expected permission denied")
	}
}

func TestSecureHardwareWrapper_I2CWrite_Allowed(t *testing.T) {
	sa := NewSecurityAuditor(&AuditorConfig{Enabled: true, DefaultAction: "allow"})
	w := NewSecureHardwareWrapper(sa, "user", "cli")

	err := w.I2CWrite("1", 0x50, []byte{0x00})
	if err != nil {
		t.Errorf("Unexpected error: %v", err)
	}
}

func TestSecureHardwareWrapper_SPIWrite_Denied(t *testing.T) {
	sa := NewSecurityAuditor(&AuditorConfig{Enabled: true, DefaultAction: "deny"})
	w := NewSecureHardwareWrapper(sa, "user", "cli")

	err := w.SPIWrite("0", []byte{0xFF})
	if err == nil {
		t.Error("Expected permission denied")
	}
}

func TestSecureHardwareWrapper_SPIWrite_Allowed(t *testing.T) {
	sa := NewSecurityAuditor(&AuditorConfig{Enabled: true, DefaultAction: "allow"})
	w := NewSecureHardwareWrapper(sa, "user", "cli")

	err := w.SPIWrite("0", []byte{0xFF})
	if err != nil {
		t.Errorf("Unexpected error: %v", err)
	}
}

// --- SecurityMiddleware tests ---

func TestSecurityMiddleware_Constructors(t *testing.T) {
	sa := NewSecurityAuditor(nil)
	mw := NewSecurityMiddleware(sa, "user", "web", "/workspace")

	if mw.File() == nil {
		t.Error("File() should not return nil")
	}
	if mw.Process() == nil {
		t.Error("Process() should not return nil")
	}
	if mw.Network() == nil {
		t.Error("Network() should not return nil")
	}
	if mw.Hardware() == nil {
		t.Error("Hardware() should not return nil")
	}
}

func TestSecurityMiddleware_GetSecuritySummary(t *testing.T) {
	sa := NewSecurityAuditor(&AuditorConfig{Enabled: true, DefaultAction: "allow"})
	mw := NewSecurityMiddleware(sa, "user", "web", "/workspace")

	summary := mw.GetSecuritySummary()
	if summary == nil {
		t.Fatal("GetSecuritySummary returned nil")
	}
	if summary["user"] != "user" {
		t.Errorf("Expected user='user', got %v", summary["user"])
	}
	if summary["source"] != "web" {
		t.Errorf("Expected source='web', got %v", summary["source"])
	}
	if summary["workspace"] != "/workspace" {
		t.Errorf("Expected workspace='/workspace', got %v", summary["workspace"])
	}
}

func TestSecurityMiddleware_BatchPermission_Empty(t *testing.T) {
	sa := NewSecurityAuditor(nil)
	mw := NewSecurityMiddleware(sa, "user", "web", "/workspace")

	allowed, err, _ := mw.RequestBatchPermission(context.Background(), &BatchOperationRequest{})
	if allowed {
		t.Error("Empty batch should not be allowed")
	}
	if err == nil {
		t.Error("Expected error for empty batch")
	}
}

func TestSecurityMiddleware_GetAuditLog(t *testing.T) {
	sa := NewSecurityAuditor(nil)
	mw := NewSecurityMiddleware(sa, "user", "web", "/workspace")

	events := mw.GetAuditLog(AuditFilter{})
	if events == nil {
		t.Error("GetAuditLog should not return nil")
	}
}

func TestSecurityMiddleware_ExportAuditLog(t *testing.T) {
	sa := NewSecurityAuditor(nil)
	mw := NewSecurityMiddleware(sa, "user", "web", "/workspace")

	exportPath := filepath.Join(t.TempDir(), "export.json")
	err := mw.ExportAuditLog(exportPath)
	if err != nil {
		t.Errorf("ExportAuditLog failed: %v", err)
	}
}

func TestSecurityMiddleware_ApproveDenyPendingRequest(t *testing.T) {
	sa := NewSecurityAuditor(&AuditorConfig{Enabled: true, DefaultAction: "ask"})
	mw := NewSecurityMiddleware(sa, "user", "web", "/workspace")

	req := &OperationRequest{Type: OpProcessExec, Target: "test"}
	_, _, reqID := sa.RequestPermission(context.Background(), req)

	// Approve
	err := mw.ApprovePendingRequest(reqID)
	if err != nil {
		t.Errorf("ApprovePendingRequest failed: %v", err)
	}

	// Create another pending request for deny
	req2 := &OperationRequest{Type: OpProcessExec, Target: "test2"}
	_, _, reqID2 := sa.RequestPermission(context.Background(), req2)

	err = mw.DenyPendingRequest(reqID2, "too dangerous")
	if err != nil {
		t.Errorf("DenyPendingRequest failed: %v", err)
	}
}

// --- Permission constructors ---

func TestCreateCLIPermission(t *testing.T) {
	p := CreateCLIPermission()
	if len(p.AllowedTypes) == 0 {
		t.Error("CLI permission should have allowed types")
	}
	if p.MaxDangerLevel != DangerHigh {
		t.Errorf("Expected DangerHigh, got %d", p.MaxDangerLevel)
	}
}

func TestCreateWebPermission(t *testing.T) {
	p := CreateWebPermission()
	if len(p.AllowedTypes) == 0 {
		t.Error("Web permission should have allowed types")
	}
	if p.MaxDangerLevel != DangerMedium {
		t.Errorf("Expected DangerMedium, got %d", p.MaxDangerLevel)
	}
}

func TestCreateAgentPermission(t *testing.T) {
	p := CreateAgentPermission("agent-1")
	if len(p.AllowedTypes) == 0 {
		t.Error("Agent permission should have allowed types")
	}
	if p.MaxDangerLevel != DangerHigh {
		t.Errorf("Expected DangerHigh, got %d", p.MaxDangerLevel)
	}
}

// --- MatchPattern tests ---

func TestMatchPattern_ExactMatch(t *testing.T) {
	if !MatchPattern("/etc/passwd", "/etc/passwd") {
		t.Error("Exact match should work")
	}
	if MatchPattern("/etc/passwd", "/etc/shadow") {
		t.Error("Non-matching paths should not match")
	}
}

func TestMatchPattern_Wildcard(t *testing.T) {
	if !MatchPattern("*.key", "/home/user/test.key") {
		t.Error("*.key should match .key files in any directory")
	}
	if MatchPattern("*.key", "/home/user/test.txt") {
		t.Error("*.key should not match .txt files")
	}
}

func TestMatchPattern_DoubleWildcard(t *testing.T) {
	if !MatchPattern("D:/123/**.key", "D:/123/sub/test.key") {
		t.Error("** should match across directories")
	}
}

// --- MatchCommandPattern tests ---

func TestMatchCommandPatternExtra(t *testing.T) {
	if !MatchCommandPattern("git *", "git status") {
		t.Error("'git *' should match 'git status'")
	}
	if !MatchCommandPattern("*sudo*", "sudo apt-get install") {
		t.Error("'*sudo*' should match 'sudo apt-get install'")
	}
	if MatchCommandPattern("git *", "svn update") {
		t.Error("'git *' should not match 'svn update'")
	}
}

// --- MatchDomainPattern extra tests ---

func TestMatchDomainPatternExtra(t *testing.T) {
	if !MatchDomainPattern("*.github.com", "api.github.com") {
		t.Error("*.github.com should match api.github.com")
	}
	if !MatchDomainPattern("github.com", "github.com") {
		t.Error("Exact domain match should work")
	}
	if MatchDomainPattern("*.github.com", "github.com") {
		t.Error("*.github.com should not match github.com (no subdomain)")
	}
}

// --- MonitorSecurityStatus test ---

func TestMonitorSecurityStatus_ContextCancel(t *testing.T) {
	sa := NewSecurityAuditor(nil)
	ctx, cancel := context.WithCancel(context.Background())

	done := make(chan struct{})
	go func() {
		MonitorSecurityStatus(ctx, sa, 10*time.Second)
		close(done)
	}()

	// Cancel immediately
	cancel()

	select {
	case <-done:
		// Expected
	case <-time.After(2 * time.Second):
		t.Error("MonitorSecurityStatus should exit when context is cancelled")
	}
}

// --- DefaultDenyPatterns test ---

func TestDefaultDenyPatterns(t *testing.T) {
	if len(DefaultDenyPatterns) == 0 {
		t.Error("DefaultDenyPatterns should not be empty")
	}
	if _, ok := DefaultDenyPatterns[OpProcessExec]; !ok {
		t.Error("Should have patterns for process_exec")
	}
	if _, ok := DefaultDenyPatterns[OpFileWrite]; !ok {
		t.Error("Should have patterns for file_write")
	}
}

// --- Generate IDs test ---

func TestGenerateRequestID(t *testing.T) {
	id1 := generateRequestID()
	id2 := generateRequestID()
	if id1 == id2 {
		t.Error("Generated IDs should be unique")
	}
	if !strings.HasPrefix(id1, "req-") {
		t.Errorf("Expected 'req-' prefix, got %s", id1)
	}
}

func TestGenerateEventID(t *testing.T) {
	id1 := generateEventID()
	id2 := generateEventID()
	if id1 == id2 {
		t.Error("Generated IDs should be unique")
	}
	if !strings.HasPrefix(id1, "evt-") {
		t.Errorf("Expected 'evt-' prefix, got %s", id1)
	}
}

// --- InitGlobalAuditor test ---

func TestInitGlobalAuditor(t *testing.T) {
	// Reset the global auditor for testing
	auditorOnce = sync.Once{}
	globalAuditor = nil

	auditor := InitGlobalAuditor(nil)
	if auditor == nil {
		t.Fatal("InitGlobalAuditor returned nil")
	}

	// Second call should return the same instance
	auditor2 := InitGlobalAuditor(&AuditorConfig{DefaultAction: "allow"})
	if auditor2 != auditor {
		t.Error("Should return the same singleton instance")
	}
}

// --- SecurityPlugin initAuditLogFile test ---

func TestSecurityPlugin_InitAuditLogFile(t *testing.T) {
	p := NewSecurityPlugin()
	p.auditor = nil
	err := p.initAuditLogFile()
	if err == nil {
		t.Error("Expected error when auditor is nil")
	}
}

// --- Import helpers ---
var _ = json.Marshal
var _ = fmt.Sprintf
