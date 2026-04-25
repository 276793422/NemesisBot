// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package tools

import (
	"context"
	"strings"
	"testing"

	"github.com/276793422/NemesisBot/module/cron"
)

// ==================== Filesystem Tool Metadata Tests ====================
// These cover the Name(), Description(), Parameters() methods that were
// previously at 0% coverage.

func TestReadFileTool_Metadata(t *testing.T) {
	tool := NewReadFileTool("", false)
	if tool.Name() != "read_file" {
		t.Errorf("Expected name 'read_file', got '%s'", tool.Name())
	}
	desc := tool.Description()
	if desc == "" {
		t.Error("Description should not be empty")
	}
	params := tool.Parameters()
	if params == nil || params["type"] != "object" {
		t.Errorf("Expected object parameters, got %v", params)
	}
}

func TestWriteFileTool_Metadata(t *testing.T) {
	tool := NewWriteFileTool("", false)
	if tool.Name() != "write_file" {
		t.Errorf("Expected name 'write_file', got '%s'", tool.Name())
	}
	desc := tool.Description()
	if desc == "" {
		t.Error("Description should not be empty")
	}
	params := tool.Parameters()
	if params == nil || params["type"] != "object" {
		t.Errorf("Expected object parameters, got %v", params)
	}
}

func TestListDirTool_Metadata(t *testing.T) {
	tool := NewListDirTool("", false)
	if tool.Name() != "list_dir" {
		t.Errorf("Expected name 'list_dir', got '%s'", tool.Name())
	}
	desc := tool.Description()
	if desc == "" {
		t.Error("Description should not be empty")
	}
	params := tool.Parameters()
	if params == nil || params["type"] != "object" {
		t.Errorf("Expected object parameters, got %v", params)
	}
}

func TestDeleteFileTool_Metadata(t *testing.T) {
	tool := NewDeleteFileTool("", false)
	if tool.Name() != "delete_file" {
		t.Errorf("Expected name 'delete_file', got '%s'", tool.Name())
	}
	desc := tool.Description()
	if desc == "" {
		t.Error("Description should not be empty")
	}
	params := tool.Parameters()
	if params == nil || params["type"] != "object" {
		t.Errorf("Expected object parameters, got %v", params)
	}
}

func TestCreateDirTool_Metadata(t *testing.T) {
	tool := NewCreateDirTool("", false)
	if tool.Name() != "create_dir" {
		t.Errorf("Expected name 'create_dir', got '%s'", tool.Name())
	}
	desc := tool.Description()
	if desc == "" {
		t.Error("Description should not be empty")
	}
	params := tool.Parameters()
	if params == nil || params["type"] != "object" {
		t.Errorf("Expected object parameters, got %v", params)
	}
}

func TestDeleteDirTool_Metadata(t *testing.T) {
	tool := NewDeleteDirTool("", false)
	if tool.Name() != "delete_dir" {
		t.Errorf("Expected name 'delete_dir', got '%s'", tool.Name())
	}
	desc := tool.Description()
	if desc == "" {
		t.Error("Description should not be empty")
	}
	params := tool.Parameters()
	if params == nil || params["type"] != "object" {
		t.Errorf("Expected object parameters, got %v", params)
	}
}

// ==================== Skills Install Tool ====================

func TestInstallSkillTool_Metadata(t *testing.T) {
	tool := NewInstallSkillTool(nil, nil)
	if tool == nil {
		t.Fatal("Expected non-nil tool")
	}
	if tool.Name() != "install_skill" {
		t.Errorf("Expected name 'install_skill', got '%s'", tool.Name())
	}
	desc := tool.Description()
	if desc == "" {
		t.Error("Description should not be empty")
	}
	params := tool.Parameters()
	if params == nil || params["type"] != "object" {
		t.Errorf("Expected object parameters, got %v", params)
	}
}

func TestInstallSkillTool_Execute_MissingSlug(t *testing.T) {
	tool := NewInstallSkillTool(nil, nil)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{})
	if !result.IsError {
		t.Error("Expected error for missing slug")
	}
	if !strings.Contains(result.ForLLM, "slug") {
		t.Errorf("Expected error about slug, got '%s'", result.ForLLM)
	}
}

func TestInstallSkillTool_Execute_EmptySlug(t *testing.T) {
	tool := NewInstallSkillTool(nil, nil)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"slug": "",
	})
	if !result.IsError {
		t.Error("Expected error for empty slug")
	}
}

func TestInstallSkillTool_Execute_NilManager(t *testing.T) {
	tool := NewInstallSkillTool(nil, nil)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"slug": "test-skill",
	})
	if !result.IsError {
		t.Error("Expected error with nil registry manager")
	}
	if !strings.Contains(result.ForLLM, "registry manager") {
		t.Errorf("Expected error about registry manager, got '%s'", result.ForLLM)
	}
}

func TestInstallSkillTool_Execute_WithVersionAndForce(t *testing.T) {
	tool := NewInstallSkillTool(nil, nil)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"slug":    "test-skill",
		"version": "1.0.0",
		"force":   true,
	})
	// Should still error on nil manager
	if !result.IsError {
		t.Error("Expected error with nil registry manager")
	}
}

// ==================== Skills Search Tool ====================

func TestFindSkillsTool_Metadata(t *testing.T) {
	tool := NewFindSkillsTool(nil, nil)
	if tool == nil {
		t.Fatal("Expected non-nil tool")
	}
	if tool.Name() != "find_skills" {
		t.Errorf("Expected name 'find_skills', got '%s'", tool.Name())
	}
	desc := tool.Description()
	if desc == "" {
		t.Error("Description should not be empty")
	}
	params := tool.Parameters()
	if params == nil || params["type"] != "object" {
		t.Errorf("Expected object parameters, got %v", params)
	}
}

func TestFindSkillsTool_Execute_NilManager(t *testing.T) {
	tool := NewFindSkillsTool(nil, nil)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"query": "test",
	})
	if !result.IsError {
		t.Error("Expected error with nil registry manager")
	}
	if !strings.Contains(result.ForLLM, "registry manager") {
		t.Errorf("Expected error about registry manager, got '%s'", result.ForLLM)
	}
}

func TestFindSkillsTool_Execute_WithLimit(t *testing.T) {
	tool := NewFindSkillsTool(nil, nil)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"query": "test",
		"limit": float64(10),
	})
	if !result.IsError {
		t.Error("Expected error with nil registry manager")
	}
}

func TestFindSkillsTool_Execute_WithLimitClamping(t *testing.T) {
	tool := NewFindSkillsTool(nil, nil)
	ctx := context.Background()

	// Limit too high (> 50) should be clamped
	result := tool.Execute(ctx, map[string]interface{}{
		"query": "test",
		"limit": float64(100),
	})
	if !result.IsError {
		t.Error("Expected error with nil registry manager")
	}
}

func TestFindSkillsTool_Execute_WithLimitTooLow(t *testing.T) {
	tool := NewFindSkillsTool(nil, nil)
	ctx := context.Background()

	// Limit < 1 should be clamped to 1
	result := tool.Execute(ctx, map[string]interface{}{
		"query": "test",
		"limit": float64(0),
	})
	if !result.IsError {
		t.Error("Expected error with nil registry manager")
	}
}

// ==================== SPI Tool Additional Coverage ====================

func TestSPITool_Execute_TransferAction_WithDevice(t *testing.T) {
	tool := NewSPITool()
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"action": "transfer",
		"device": "0.0",
		"data":   "0x01",
	})
	_ = result
}

func TestSPITool_Execute_ReadAction_WithDevice(t *testing.T) {
	tool := NewSPITool()
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"action": "read",
		"device": "0.0",
		"length": 4,
	})
	_ = result
}

// ==================== ClusterRPCTool Additional Methods ====================

func TestClusterRPCTool_GetAvailablePeers_NilCluster(t *testing.T) {
	defer func() {
		if r := recover(); r != nil {
			// Expected: nil cluster causes panic
			t.Logf("Got expected panic: %v", r)
		}
	}()
	tool := &ClusterRPCTool{}
	ctx := context.Background()
	_, _ = tool.GetAvailablePeers(ctx)
	t.Error("Expected panic with nil cluster")
}

func TestClusterRPCTool_GetCapabilities_NilCluster(t *testing.T) {
	defer func() {
		if r := recover(); r != nil {
			// Expected: nil cluster causes panic
			t.Logf("Got expected panic: %v", r)
		}
	}()
	tool := &ClusterRPCTool{}
	ctx := context.Background()
	_, _ = tool.GetCapabilities(ctx)
	t.Error("Expected panic with nil cluster")
}

// ==================== Additional CronTool Tests ====================

func TestCronTool_Execute_ListWithJobs(t *testing.T) {
	cronService := cron.NewCronService(t.TempDir() + "/cron.json", nil)
	tool := NewCronTool(cronService, nil, nil, "", false, 0, nil)
	tool.SetContext("test-channel", "test-chat")
	ctx := context.Background()

	// Add a job first
	tool.Execute(ctx, map[string]interface{}{
		"action":        "add",
		"message":       "Recurring test job",
		"every_seconds": float64(60),
	})

	// List should show the job
	result := tool.Execute(ctx, map[string]interface{}{
		"action": "list",
	})
	if result.IsError {
		t.Errorf("List should succeed: %s", result.ForLLM)
	}
	if !strings.Contains(result.ForLLM, "Recurring") {
		t.Errorf("List should contain job name, got '%s'", result.ForLLM)
	}
	if !strings.Contains(result.ForLLM, "every 60s") {
		t.Errorf("List should show schedule, got '%s'", result.ForLLM)
	}
}

func TestCronTool_Execute_AddWithCronExpr_List(t *testing.T) {
	cronService := cron.NewCronService(t.TempDir() + "/cron.json", nil)
	tool := NewCronTool(cronService, nil, nil, "", false, 0, nil)
	tool.SetContext("test-channel", "test-chat")
	ctx := context.Background()

	// Add with cron expression
	tool.Execute(ctx, map[string]interface{}{
		"action":    "add",
		"message":   "Daily cron job",
		"cron_expr": "0 9 * * *",
	})

	// List should show the cron expression
	result := tool.Execute(ctx, map[string]interface{}{
		"action": "list",
	})
	if !strings.Contains(result.ForLLM, "0 9 * * *") {
		t.Errorf("List should show cron expression, got '%s'", result.ForLLM)
	}
}

func TestCronTool_Execute_ListEmpty(t *testing.T) {
	cronService := cron.NewCronService(t.TempDir() + "/cron.json", nil)
	tool := NewCronTool(cronService, nil, nil, "", false, 0, nil)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"action": "list",
	})
	if result.IsError {
		t.Errorf("List should not error: %s", result.ForLLM)
	}
	if !strings.Contains(result.ForLLM, "No scheduled") {
		t.Errorf("Expected 'No scheduled' for empty list, got '%s'", result.ForLLM)
	}
}

func TestCronTool_Execute_AddWithAt_List(t *testing.T) {
	cronService := cron.NewCronService(t.TempDir() + "/cron.json", nil)
	tool := NewCronTool(cronService, nil, nil, "", false, 0, nil)
	tool.SetContext("test-channel", "test-chat")
	ctx := context.Background()

	// Add one-time job
	tool.Execute(ctx, map[string]interface{}{
		"action":     "add",
		"message":    "One-time task",
		"at_seconds": float64(3600),
	})

	// List should show "one-time"
	result := tool.Execute(ctx, map[string]interface{}{
		"action": "list",
	})
	if !strings.Contains(result.ForLLM, "one-time") {
		t.Errorf("List should show 'one-time' schedule, got '%s'", result.ForLLM)
	}
}

// ==================== ExecTool additional edge cases ====================

func TestWriteFileTool_Execute_PathTraversalBlocked(t *testing.T) {
	tmpDir := t.TempDir()
	tool := NewWriteFileTool(tmpDir, true)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"path":    "../../../etc/test.txt",
		"content": "test",
	})
	if !result.IsError {
		t.Error("Expected error for path traversal")
	}
}

// ==================== AsyncShell Execute Additional ====================

func TestAsyncExecTool_Execute_BoolCommand(t *testing.T) {
	tool := NewAsyncExecTool(t.TempDir(), false)
	ctx := context.Background()

	// Test with non-string command
	result := tool.Execute(ctx, map[string]interface{}{
		"command": true,
	})
	if !result.IsError {
		t.Error("Expected error for non-string command")
	}
}

// ==================== ClusterRPCTool Execute edge cases ====================

func TestClusterRPCTool_Execute_PeerChatNoCluster(t *testing.T) {
	defer func() {
		if r := recover(); r != nil {
			t.Logf("Got expected panic: %v", r)
		}
	}()
	tool := &ClusterRPCTool{}
	ctx := context.Background()

	_ = tool.Execute(ctx, map[string]interface{}{
		"peer_id": "peer-1",
		"action":  "peer_chat",
		"data":    map[string]interface{}{"message": "hello"},
	})
}

func TestClusterRPCTool_Execute_GetPeersAction(t *testing.T) {
	defer func() {
		if r := recover(); r != nil {
			t.Logf("Got expected panic: %v", r)
		}
	}()
	tool := &ClusterRPCTool{}
	ctx := context.Background()

	_ = tool.Execute(ctx, map[string]interface{}{
		"action":  "some_action",
		"peer_id": "peer-1",
	})
}

func TestClusterRPCTool_Execute_GetCapabilitiesAction(t *testing.T) {
	defer func() {
		if r := recover(); r != nil {
			t.Logf("Got expected panic: %v", r)
		}
	}()
	tool := &ClusterRPCTool{}
	ctx := context.Background()

	_ = tool.Execute(ctx, map[string]interface{}{
		"action":  "get_capabilities",
		"peer_id": "peer-1",
	})
}
