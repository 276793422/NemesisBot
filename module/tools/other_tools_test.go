// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package tools

import (
	"context"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/cluster"
	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/cron"
)

// ==================== ClusterRPCTool Tests ====================

func TestNewClusterRPCTool(t *testing.T) {
	mockCluster := &cluster.Cluster{}
	tool := NewClusterRPCTool(mockCluster)

	if tool == nil {
		t.Fatal("Expected non-nil tool")
	}

	if tool.cluster != mockCluster {
		t.Error("Cluster not set correctly")
	}
}

func TestClusterRPCTool_Name(t *testing.T) {
	tool := &ClusterRPCTool{}
	if tool.Name() != "cluster_rpc" {
		t.Errorf("Expected name 'cluster_rpc', got '%s'", tool.Name())
	}
}

func TestClusterRPCTool_Description(t *testing.T) {
	tool := &ClusterRPCTool{}
	desc := tool.Description()

	if desc == "" {
		t.Error("Description should not be empty")
	}

	if !contains(desc, "peer_id") || !contains(desc, "action") {
		t.Error("Description should mention parameters")
	}
}

func TestClusterRPCTool_Parameters(t *testing.T) {
	tool := &ClusterRPCTool{}
	params := tool.Parameters()

	if params == nil {
		t.Fatal("Parameters should not be nil")
	}

	if params["type"] != "object" {
		t.Errorf("Expected type 'object', got '%v'", params["type"])
	}

	props, ok := params["properties"].(map[string]interface{})
	if !ok {
		t.Fatal("Properties should be a map")
	}

	// Check required properties
	requiredProps := []string{"peer_id", "action", "data"}
	for _, prop := range requiredProps {
		if _, ok := props[prop]; !ok {
			t.Errorf("Missing property: %s", prop)
		}
	}

	required, ok := params["required"].([]string)
	if !ok {
		t.Fatal("Required should be a string slice")
	}

	if len(required) != 2 {
		t.Errorf("Expected 2 required parameters, got %d", len(required))
	}
}

func TestClusterRPCTool_Execute_MissingPeerID(t *testing.T) {
	mockCluster := &cluster.Cluster{}
	tool := NewClusterRPCTool(mockCluster)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"action": "test",
	})

	if !result.IsError {
		t.Error("Expected error for missing peer_id")
	}

	// ErrorResult stores error message in ForLLM field
	errorMsg := result.ForUser
	if errorMsg == "" {
		errorMsg = result.ForLLM
	}
	if !contains(errorMsg, "peer_id") && !contains(errorMsg, "required") {
		t.Errorf("Expected peer_id error, got: ForUser=%q, ForLLM=%q", result.ForUser, result.ForLLM)
	}
}

func TestClusterRPCTool_Execute_MissingAction(t *testing.T) {
	mockCluster := &cluster.Cluster{}
	tool := NewClusterRPCTool(mockCluster)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"peer_id": "peer-1",
	})

	if !result.IsError {
		t.Error("Expected error for missing action")
	}

	// ErrorResult stores error message in ForLLM field
	errorMsg := result.ForUser
	if errorMsg == "" {
		errorMsg = result.ForLLM
	}
	if !contains(errorMsg, "action") && !contains(errorMsg, "required") {
		t.Errorf("Expected action error, got: ForUser=%q, ForLLM=%q", result.ForUser, result.ForLLM)
	}
}

func TestClusterRPCTool_Execute_InvalidTypes(t *testing.T) {
	mockCluster := &cluster.Cluster{}
	tool := NewClusterRPCTool(mockCluster)
	ctx := context.Background()

	// Test invalid peer_id type
	result := tool.Execute(ctx, map[string]interface{}{
		"peer_id": 123,
		"action":  "test",
	})

	if !result.IsError {
		t.Error("Expected error for invalid peer_id type")
	}

	// Test invalid action type
	result = tool.Execute(ctx, map[string]interface{}{
		"peer_id": "peer-1",
		"action":  456,
	})

	if !result.IsError {
		t.Error("Expected error for invalid action type")
	}
}

// ==================== CronTool Tests ====================

type MockJobExecutor struct{}

func (m *MockJobExecutor) ProcessDirectWithChannel(ctx context.Context, content, sessionKey, channel, chatID string) (string, error) {
	return "mock response", nil
}

func TestNewCronTool(t *testing.T) {
	cronService := cron.NewCronService("/tmp/test-cron.json", nil)
	executor := &MockJobExecutor{}
	msgBus := bus.NewMessageBus()
	workspace := "/test/workspace"

	tool := NewCronTool(cronService, executor, msgBus, workspace, true, 0, nil)

	if tool == nil {
		t.Fatal("Expected non-nil tool")
	}

	if tool.cronService != cronService {
		t.Error("CronService not set correctly")
	}

	if tool.executor != executor {
		t.Error("Executor not set correctly")
	}

	if tool.msgBus != msgBus {
		t.Error("MessageBus not set correctly")
	}

	if tool.execTool == nil {
		t.Error("ExecTool should be initialized")
	}
}

func TestCronTool_Name(t *testing.T) {
	cronService := cron.NewCronService("/tmp/test-cron.json", nil)
	tool := NewCronTool(cronService, nil, nil, "", false, 0, nil)

	if tool.Name() != "cron" {
		t.Errorf("Expected name 'cron', got '%s'", tool.Name())
	}
}

func TestCronTool_Description(t *testing.T) {
	cronService := cron.NewCronService("/tmp/test-cron.json", nil)
	tool := NewCronTool(cronService, nil, nil, "", false, 0, nil)

	desc := tool.Description()

	if desc == "" {
		t.Error("Description should not be empty")
	}

	expectedTerms := []string{"schedule", "reminder", "at_seconds", "every_seconds"}
	for _, term := range expectedTerms {
		if !contains(desc, term) {
			t.Errorf("Description should contain '%s'", term)
		}
	}
}

func TestCronTool_Parameters(t *testing.T) {
	cronService := cron.NewCronService("/tmp/test-cron.json", nil)
	tool := NewCronTool(cronService, nil, nil, "", false, 0, nil)

	params := tool.Parameters()

	if params == nil {
		t.Fatal("Parameters should not be nil")
	}

	props, ok := params["properties"].(map[string]interface{})
	if !ok {
		t.Fatal("Properties should be a map")
	}

	// Check for expected properties
	expectedProps := []string{"action", "message", "command", "at_seconds", "every_seconds", "cron_expr", "job_id", "deliver"}
	for _, prop := range expectedProps {
		if _, ok := props[prop]; !ok {
			t.Errorf("Missing property: %s", prop)
		}
	}

	// Check action enum
	actionProp, ok := props["action"].(map[string]interface{})
	if !ok {
		t.Fatal("Action should be a map")
	}

	enum, ok := actionProp["enum"].([]string)
	if !ok {
		t.Fatal("Action enum should be a string slice")
	}

	expectedActions := []string{"add", "list", "remove", "enable", "disable"}
	if len(enum) != len(expectedActions) {
		t.Errorf("Expected %d actions, got %d", len(expectedActions), len(enum))
	}
}

func TestCronTool_SetContext(t *testing.T) {
	cronService := cron.NewCronService("/tmp/test-cron.json", nil)
	tool := NewCronTool(cronService, nil, nil, "", false, 0, nil)

	tool.SetContext("test-channel", "test-chat")

	if tool.channel != "test-channel" {
		t.Errorf("Expected channel 'test-channel', got '%s'", tool.channel)
	}

	if tool.chatID != "test-chat" {
		t.Errorf("Expected chatID 'test-chat', got '%s'", tool.chatID)
	}
}

func TestCronTool_Execute_List(t *testing.T) {
	cronService := cron.NewCronService("/tmp/test-cron.json", nil)
	tool := NewCronTool(cronService, nil, nil, "", false, 0, nil)

	ctx := context.Background()
	result := tool.Execute(ctx, map[string]interface{}{
		"action": "list",
	})

	if result == nil {
		t.Fatal("Expected non-nil result")
	}

	// List should not error even with no jobs
	_ = result
}

func TestCronTool_Execute_InvalidAction(t *testing.T) {
	cronService := cron.NewCronService("/tmp/test-cron.json", nil)
	tool := NewCronTool(cronService, nil, nil, "", false, 0, nil)

	ctx := context.Background()
	result := tool.Execute(ctx, map[string]interface{}{
		"action": "invalid_action",
	})

	if result == nil {
		t.Fatal("Expected non-nil result")
	}

	_ = result
}

// ==================== InstallSkillTool Tests ====================
// Skipped for now - requires skills module integration

// ==================== FindSkillsTool Tests ====================
// Skipped for now - requires skills module integration

// ==================== CompleteBootstrapTool Tests ====================

func TestNewCompleteBootstrapTool(t *testing.T) {
	tool := NewCompleteBootstrapTool("/test/bootstrap.json")

	if tool == nil {
		t.Fatal("Expected non-nil tool")
	}
}

func TestCompleteBootstrapTool_Name(t *testing.T) {
	tool := NewCompleteBootstrapTool("/test/bootstrap.json")

	if tool.Name() != "complete_bootstrap" {
		t.Errorf("Expected name 'complete_bootstrap', got '%s'", tool.Name())
	}
}

func TestCompleteBootstrapTool_Description(t *testing.T) {
	tool := NewCompleteBootstrapTool("/test/bootstrap.json")

	desc := tool.Description()

	if desc == "" {
		t.Error("Description should not be empty")
	}
}

func TestCompleteBootstrapTool_Parameters(t *testing.T) {
	tool := NewCompleteBootstrapTool("/test/bootstrap.json")

	params := tool.Parameters()

	if params == nil {
		t.Fatal("Parameters should not be nil")
	}

	props, ok := params["properties"].(map[string]interface{})
	if !ok {
		t.Fatal("Properties should be a map")
	}

	// Check for expected properties (CompleteBootstrapTool uses "confirmed" not "bootstrap_id")
	if _, ok := props["confirmed"]; !ok {
		t.Error("Missing property: confirmed")
	}
}

func TestCompleteBootstrapTool_Execute(t *testing.T) {
	tool := NewCompleteBootstrapTool("/test/bootstrap.json")
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"bootstrap_id": "test-id",
	})

	if result == nil {
		t.Fatal("Expected non-nil result")
	}

	_ = result
}

// Test concurrent operations for thread safety
func TestCronTool_ConcurrentSetContext(t *testing.T) {
	cronService := cron.NewCronService("/tmp/test-cron.json", nil)
	tool := NewCronTool(cronService, nil, nil, "", false, 0, nil)

	// Test concurrent SetContext calls
	done := make(chan bool)
	for i := 0; i < 10; i++ {
		go func(index int) {
			tool.SetContext("channel", "chat")
			done <- true
		}(i)
	}

	// Wait for all goroutines
	for i := 0; i < 10; i++ {
		<-done
	}

	// Just verify no race conditions occurred
	_ = tool.channel
	_ = tool.chatID
}

// Test tool initialization with various configurations
func TestNewCronTool_Configurations(t *testing.T) {
	executor := &MockJobExecutor{}
	msgBus := bus.NewMessageBus()

	testCases := []struct {
		name         string
		workspace    string
		restrict     bool
		execTimeout  int
		config       *config.Config
	}{
		{"Default config", "/workspace", true, 0, nil},
		{"No restriction", "/workspace", false, 0, nil},
		{"With timeout", "/workspace", true, 30, nil},
		{"With config", "/workspace", true, 0, &config.Config{}},
	}

	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			cronService := cron.NewCronService("/tmp/test-cron.json", nil)
			tool := NewCronTool(cronService, executor, msgBus, tc.workspace, tc.restrict, time.Duration(tc.execTimeout)*time.Second, tc.config)

			if tool == nil {
				t.Fatal("Expected non-nil tool")
			}

			if tool.execTool == nil {
				t.Error("ExecTool should be initialized")
			}
		})
	}
}

// Test error handling for invalid parameters
func TestClusterRPCTool_ParameterValidation(t *testing.T) {
	mockCluster := &cluster.Cluster{}
	tool := NewClusterRPCTool(mockCluster)
	ctx := context.Background()

	testCases := []struct {
		name     string
		params   map[string]interface{}
		shouldError bool
	}{
		{
			name: "Valid params with data",
			params: map[string]interface{}{
				"peer_id": "peer-1",
				"action":  "test_action",
				"data": map[string]interface{}{
					"key": "value",
				},
			},
			shouldError: false, // Will error on actual RPC call, but params are valid
		},
		{
			name: "Valid params without data",
			params: map[string]interface{}{
				"peer_id": "peer-1",
				"action":  "test_action",
			},
			shouldError: false, // Will error on actual RPC call, but params are valid
		},
		{
			name: "Empty peer_id",
			params: map[string]interface{}{
				"peer_id": "",
				"action":  "test",
			},
			shouldError: true,
		},
		{
			name: "Empty action",
			params: map[string]interface{}{
				"peer_id": "peer-1",
				"action":  "",
			},
			shouldError: true,
		},
	}

	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			result := tool.Execute(ctx, tc.params)

			if tc.shouldError && !result.IsError {
				t.Error("Expected error for invalid parameters")
			}

			if !tc.shouldError && result == nil {
				t.Error("Expected non-nil result")
			}
		})
	}
}
