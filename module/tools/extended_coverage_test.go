// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package tools

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/cron"
)

// ==================== CompleteBootstrap Execute Tests ====================

func TestCompleteBootstrapTool_Execute_NotConfirmed(t *testing.T) {
	tempDir := t.TempDir()
	tool := NewCompleteBootstrapTool(tempDir)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"confirmed": false,
	})
	if !result.IsError {
		t.Error("Expected error when not confirmed")
	}
}

func TestCompleteBootstrapTool_Execute_NotBool(t *testing.T) {
	tempDir := t.TempDir()
	tool := NewCompleteBootstrapTool(tempDir)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"confirmed": "yes",
	})
	if !result.IsError {
		t.Error("Expected error when confirmed is not bool")
	}
}

func TestCompleteBootstrapTool_Execute_AlreadyDeleted(t *testing.T) {
	tempDir := t.TempDir()
	tool := NewCompleteBootstrapTool(tempDir)
	ctx := context.Background()

	// Don't create BOOTSTRAP.md - it doesn't exist
	result := tool.Execute(ctx, map[string]interface{}{
		"confirmed": true,
	})
	if result.IsError {
		t.Errorf("Expected success when BOOTSTRAP.md already deleted, got error: %s", result.ForLLM)
	}
	// The message is in Chinese: "已经被删除"
	msg := result.ForLLM
	if !strings.Contains(msg, "删除") {
		t.Errorf("Expected message about deletion, got '%s'", msg)
	}
}

func TestCompleteBootstrapTool_Execute_Success(t *testing.T) {
	tempDir := t.TempDir()
	bootstrapPath := filepath.Join(tempDir, "BOOTSTRAP.md")
	err := os.WriteFile(bootstrapPath, []byte("# Bootstrap Guide"), 0644)
	if err != nil {
		t.Fatalf("Failed to create BOOTSTRAP.md: %v", err)
	}

	tool := NewCompleteBootstrapTool(tempDir)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"confirmed": true,
	})
	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}

	// Verify file was deleted
	if _, err := os.Stat(bootstrapPath); !os.IsNotExist(err) {
		t.Error("BOOTSTRAP.md should have been deleted")
	}
}

// ==================== CronTool Execute Add/Remove/Enable/Disable Tests ====================

func TestCronTool_Execute_MissingAction(t *testing.T) {
	cronService := cron.NewCronService(filepath.Join(t.TempDir(), "cron.json"), nil)
	tool := NewCronTool(cronService, nil, nil, "", false, 0, nil)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{})
	if !result.IsError {
		t.Error("Expected error for missing action")
	}
}

func TestCronTool_Execute_AddNoContext(t *testing.T) {
	cronService := cron.NewCronService(filepath.Join(t.TempDir(), "cron.json"), nil)
	tool := NewCronTool(cronService, nil, nil, "", false, 0, nil)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"action":      "add",
		"message":     "Test reminder",
		"at_seconds":  float64(60),
	})
	if !result.IsError {
		t.Error("Expected error when no session context")
	}
	if !strings.Contains(result.ForLLM, "no session context") {
		t.Errorf("Expected 'no session context' error, got '%s'", result.ForLLM)
	}
}

func TestCronTool_Execute_AddNoMessage(t *testing.T) {
	cronService := cron.NewCronService(filepath.Join(t.TempDir(), "cron.json"), nil)
	tool := NewCronTool(cronService, nil, nil, "", false, 0, nil)
	tool.SetContext("test-channel", "test-chat")
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"action":     "add",
		"at_seconds": float64(60),
	})
	if !result.IsError {
		t.Error("Expected error when no message")
	}
}

func TestCronTool_Execute_AddNoSchedule(t *testing.T) {
	cronService := cron.NewCronService(filepath.Join(t.TempDir(), "cron.json"), nil)
	tool := NewCronTool(cronService, nil, nil, "", false, 0, nil)
	tool.SetContext("test-channel", "test-chat")
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"action":  "add",
		"message": "Test",
	})
	if !result.IsError {
		t.Error("Expected error when no schedule specified")
	}
	if !strings.Contains(result.ForLLM, "at_seconds") {
		t.Errorf("Expected error about schedule parameters, got '%s'", result.ForLLM)
	}
}

func TestCronTool_Execute_AddWithAtSeconds(t *testing.T) {
	cronService := cron.NewCronService(filepath.Join(t.TempDir(), "cron.json"), nil)
	tool := NewCronTool(cronService, nil, nil, "", false, 0, nil)
	tool.SetContext("test-channel", "test-chat")
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"action":     "add",
		"message":    "Test reminder",
		"at_seconds": float64(60),
	})
	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}
	if !result.Silent {
		t.Error("Cron add result should be silent")
	}
}

func TestCronTool_Execute_AddWithEverySeconds(t *testing.T) {
	cronService := cron.NewCronService(filepath.Join(t.TempDir(), "cron.json"), nil)
	tool := NewCronTool(cronService, nil, nil, "", false, 0, nil)
	tool.SetContext("test-channel", "test-chat")
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"action":        "add",
		"message":       "Recurring task",
		"every_seconds": float64(3600),
	})
	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}
}

func TestCronTool_Execute_AddWithCronExpr(t *testing.T) {
	cronService := cron.NewCronService(filepath.Join(t.TempDir(), "cron.json"), nil)
	tool := NewCronTool(cronService, nil, nil, "", false, 0, nil)
	tool.SetContext("test-channel", "test-chat")
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"action":    "add",
		"message":   "Daily task",
		"cron_expr": "0 9 * * *",
	})
	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}
}

func TestCronTool_Execute_AddWithCommand(t *testing.T) {
	cronService := cron.NewCronService(filepath.Join(t.TempDir(), "cron.json"), nil)
	tool := NewCronTool(cronService, nil, nil, "", false, 0, nil)
	tool.SetContext("test-channel", "test-chat")
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"action":     "add",
		"message":    "Run df",
		"command":    "df -h",
		"at_seconds": float64(60),
	})
	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}
}

func TestCronTool_Execute_AddWithDeliver(t *testing.T) {
	cronService := cron.NewCronService(filepath.Join(t.TempDir(), "cron.json"), nil)
	tool := NewCronTool(cronService, nil, nil, "", false, 0, nil)
	tool.SetContext("test-channel", "test-chat")
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"action":     "add",
		"message":    "Test",
		"at_seconds": float64(60),
		"deliver":    false,
	})
	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}
}

func TestCronTool_Execute_RemoveNoJobID(t *testing.T) {
	cronService := cron.NewCronService(filepath.Join(t.TempDir(), "cron.json"), nil)
	tool := NewCronTool(cronService, nil, nil, "", false, 0, nil)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"action": "remove",
	})
	if !result.IsError {
		t.Error("Expected error when no job_id provided")
	}
}

func TestCronTool_Execute_RemoveNotFound(t *testing.T) {
	cronService := cron.NewCronService(filepath.Join(t.TempDir(), "cron.json"), nil)
	tool := NewCronTool(cronService, nil, nil, "", false, 0, nil)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"action": "remove",
		"job_id": "nonexistent",
	})
	if !result.IsError {
		t.Error("Expected error for nonexistent job")
	}
}

func TestCronTool_Execute_EnableNoJobID(t *testing.T) {
	cronService := cron.NewCronService(filepath.Join(t.TempDir(), "cron.json"), nil)
	tool := NewCronTool(cronService, nil, nil, "", false, 0, nil)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"action": "enable",
	})
	if !result.IsError {
		t.Error("Expected error when no job_id for enable")
	}
}

func TestCronTool_Execute_DisableNoJobID(t *testing.T) {
	cronService := cron.NewCronService(filepath.Join(t.TempDir(), "cron.json"), nil)
	tool := NewCronTool(cronService, nil, nil, "", false, 0, nil)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"action": "disable",
	})
	if !result.IsError {
		t.Error("Expected error when no job_id for disable")
	}
}

func TestCronTool_Execute_EnableNotFound(t *testing.T) {
	cronService := cron.NewCronService(filepath.Join(t.TempDir(), "cron.json"), nil)
	tool := NewCronTool(cronService, nil, nil, "", false, 0, nil)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"action": "enable",
		"job_id": "nonexistent",
	})
	if !result.IsError {
		t.Error("Expected error for nonexistent job enable")
	}
}

func TestCronTool_Execute_DisableNotFound(t *testing.T) {
	cronService := cron.NewCronService(filepath.Join(t.TempDir(), "cron.json"), nil)
	tool := NewCronTool(cronService, nil, nil, "", false, 0, nil)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"action": "disable",
		"job_id": "nonexistent",
	})
	if !result.IsError {
		t.Error("Expected error for nonexistent job disable")
	}
}

func TestCronTool_ExecuteJob_DeliverTrue(t *testing.T) {
	cronService := cron.NewCronService(filepath.Join(t.TempDir(), "cron.json"), nil)
	msgBus := bus.NewMessageBus()
	tool := NewCronTool(cronService, nil, msgBus, "", false, 0, nil)
	ctx := context.Background()

	job := &cron.CronJob{
		ID:   "test-job-1",
		Name: "Test Job",
		Payload: cron.CronPayload{
			Message: "Test reminder message",
			Deliver: true,
			Channel: "test-channel",
			To:      "test-chat",
		},
	}

	result := tool.ExecuteJob(ctx, job)
	if result != "ok" {
		t.Errorf("Expected 'ok', got '%s'", result)
	}
}

func TestCronTool_ExecuteJob_WithCommand(t *testing.T) {
	cronService := cron.NewCronService(filepath.Join(t.TempDir(), "cron.json"), nil)
	msgBus := bus.NewMessageBus()
	tool := NewCronTool(cronService, nil, msgBus, t.TempDir(), false, 10*1e7, nil)
	ctx := context.Background()

	job := &cron.CronJob{
		ID:   "test-job-2",
		Name: "Command Job",
		Payload: cron.CronPayload{
			Command: "echo hello",
			Channel: "test-channel",
			To:      "test-chat",
		},
	}

	result := tool.ExecuteJob(ctx, job)
	if result != "ok" {
		t.Errorf("Expected 'ok', got '%s'", result)
	}
}

func TestCronTool_ExecuteJob_DefaultChannel(t *testing.T) {
	cronService := cron.NewCronService(filepath.Join(t.TempDir(), "cron.json"), nil)
	msgBus := bus.NewMessageBus()
	tool := NewCronTool(cronService, nil, msgBus, "", false, 0, nil)
	ctx := context.Background()

	job := &cron.CronJob{
		ID:   "test-job-3",
		Name: "Default Channel",
		Payload: cron.CronPayload{
			Message: "Test",
			Deliver: true,
			// No Channel or To - should use defaults
		},
	}

	result := tool.ExecuteJob(ctx, job)
	if result != "ok" {
		t.Errorf("Expected 'ok', got '%s'", result)
	}
}

func TestCronTool_ExecuteJob_DeliverFalse_WithExecutor(t *testing.T) {
	cronService := cron.NewCronService(filepath.Join(t.TempDir(), "cron.json"), nil)
	msgBus := bus.NewMessageBus()
	executor := &MockJobExecutor{}
	tool := NewCronTool(cronService, executor, msgBus, "", false, 0, nil)
	ctx := context.Background()

	job := &cron.CronJob{
		ID:   "test-job-4",
		Name: "Process Job",
		Payload: cron.CronPayload{
			Message: "Process this",
			Deliver: false,
			Channel: "test-channel",
			To:      "test-chat",
		},
	}

	result := tool.ExecuteJob(ctx, job)
	if result != "ok" {
		t.Errorf("Expected 'ok', got '%s'", result)
	}
}

// ==================== ClusterRPCTool SetContext ====================

func TestClusterRPCTool_SetContext_Coverage(t *testing.T) {
	tool := &ClusterRPCTool{}
	tool.SetContext("rpc", "chat-123")

	if tool.originChannel != "rpc" {
		t.Errorf("Expected originChannel 'rpc', got '%s'", tool.originChannel)
	}
	if tool.originChatID != "chat-123" {
		t.Errorf("Expected originChatID 'chat-123', got '%s'", tool.originChatID)
	}
}

func TestGenerateTaskTimestamp(t *testing.T) {
	ts := generateTaskTimestamp()
	if ts <= 0 {
		t.Error("Expected positive timestamp")
	}
}

// ==================== SpawnTool Execute Tests ====================

func TestSpawnTool_New_Coverage(t *testing.T) {
	tool := NewSpawnTool(nil)
	if tool == nil {
		t.Fatal("Expected non-nil tool")
	}
	if tool.originChannel != "cli" {
		t.Errorf("Expected default channel 'cli', got '%s'", tool.originChannel)
	}
	if tool.originChatID != "direct" {
		t.Errorf("Expected default chatID 'direct', got '%s'", tool.originChatID)
	}
}

func TestSpawnTool_Execute_AllowlistBlocked(t *testing.T) {
	tool := NewSpawnTool(nil)
	tool.SetAllowlistChecker(func(targetAgentID string) bool {
		return false
	})
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"task":     "Do something",
		"agent_id": "restricted-agent",
	})
	if !result.IsError {
		t.Error("Expected error when agent not in allowlist")
	}
	if !strings.Contains(result.ForLLM, "not allowed") {
		t.Errorf("Expected 'not allowed' error, got '%s'", result.ForLLM)
	}
}

func TestSpawnTool_Execute_AllowlistAllowed(t *testing.T) {
	// Use nil manager - the allowlist check passes, but nil manager should error
	tool := NewSpawnTool(nil)
	tool.SetAllowlistChecker(func(targetAgentID string) bool {
		return true
	})
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"task":     "Do something",
		"agent_id": "allowed-agent",
	})
	// Should error because manager is nil
	if !result.IsError {
		t.Error("Expected error when manager is nil")
	}
	if !strings.Contains(result.ForLLM, "not configured") {
		t.Errorf("Expected 'not configured' error, got '%s'", result.ForLLM)
	}
}

func TestSpawnTool_Execute_WithLabel(t *testing.T) {
	// Use nil manager - tests label parsing only, will error on nil manager
	tool := NewSpawnTool(nil)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"task":  "Do something",
		"label": "my-task",
	})
	// Should error because manager is nil
	if !result.IsError {
		t.Error("Expected error when manager is nil")
	}
}

// ==================== Shell getShortPath Tests ====================

func TestGetShortPath_NoSpaces(t *testing.T) {
	tool := NewExecTool(t.TempDir(), false)
	result, err := tool.getShortPath("C:\\test\\path")
	if err != nil {
		t.Errorf("Expected no error, got %v", err)
	}
	if result != "C:\\test\\path" {
		t.Errorf("Expected unchanged path, got '%s'", result)
	}
}

func TestGetShortPath_WithSpaces(t *testing.T) {
	tool := NewExecTool(t.TempDir(), false)
	result, err := tool.getShortPath("C:\\Program Files\\test")
	if err != nil {
		t.Errorf("Expected no error, got %v", err)
	}
	// Spaces should be escaped with ^
	if !contains(result, "^ ") {
		t.Errorf("Expected spaces to be escaped, got '%s'", result)
	}
}

// ==================== Web stripTags and extractResults Tests ====================

func TestStripTags(t *testing.T) {
	tests := []struct {
		name     string
		input    string
		expected string
	}{
		{
			name:     "no tags",
			input:    "plain text",
			expected: "plain text",
		},
		{
			name:     "simple tag",
			input:    "<b>bold</b>",
			expected: "bold",
		},
		{
			name:     "nested tags",
			input:    "<div><p>hello</p></div>",
			expected: "hello",
		},
		{
			name:     "tag with attributes",
			input:    `<a href="http://example.com">link</a>`,
			expected: "link",
		},
		{
			name:     "multiple tags",
			input:    "<h1>Title</h1><p>Content</p>",
			expected: "TitleContent",
		},
		{
			name:     "empty string",
			input:    "",
			expected: "",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := stripTags(tt.input)
			if result != tt.expected {
				t.Errorf("Expected '%s', got '%s'", tt.expected, result)
			}
		})
	}
}

func TestDuckDuckGoExtractResults(t *testing.T) {
	provider := &DuckDuckGoSearchProvider{}

	tests := []struct {
		name      string
		html      string
		count     int
		query     string
		wantEmpty bool
	}{
		{
			name:      "no results html",
			html:      "<html><body>No results</body></html>",
			count:     5,
			query:     "test",
			wantEmpty: true, // No result links found
		},
		{
			name: "with results",
			html: `<html><body>
				<a class="result__a" href="http://example.com">Example</a>
				<a class="result__snippet" href="#">Description here</a>
			</body></html>`,
			count:     5,
			query:     "example",
			wantEmpty: false,
		},
		{
			name: "results with uddg URL",
			html: `<html><body>
				<a class="result__a" href="/?uddg=http%3A%2F%2Fexample.com&r=1">Example</a>
			</body></html>`,
			count:     5,
			query:     "example",
			wantEmpty: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result, err := provider.extractResults(tt.html, tt.count, tt.query)
			if err != nil {
				t.Errorf("Unexpected error: %v", err)
			}
			if tt.wantEmpty && !strings.Contains(result, "No results") && len(result) > 100 {
				t.Errorf("Expected empty/no results, got '%s'", result[:min(100, len(result))])
			}
			if !tt.wantEmpty && result == "" {
				t.Error("Expected non-empty result")
			}
		})
	}
}

// ==================== WebSearchTool Search Provider Tests ====================

// extMockSearchProvider for testing (different name to avoid conflict with automation_test.go)
type extMockSearchProvider struct {
	result string
	err    error
}

func (m *extMockSearchProvider) Search(ctx context.Context, query string, count int) (string, error) {
	return m.result, m.err
}

func TestWebSearchTool_Execute_WithProvider(t *testing.T) {
	provider := &extMockSearchProvider{result: "1. Example\n   http://example.com\n   Description"}
	tool := &WebSearchTool{
		provider:   provider,
		maxResults: 5,
	}
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"query": "test",
	})
	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}
}

func TestWebSearchTool_Execute_ProviderError(t *testing.T) {
	provider := &extMockSearchProvider{err: context.DeadlineExceeded}
	tool := &WebSearchTool{
		provider:   provider,
		maxResults: 5,
	}
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"query": "test",
	})
	if !result.IsError {
		t.Error("Expected error when search provider fails")
	}
}

func TestWebSearchTool_Execute_ProviderWithCount(t *testing.T) {
	provider := &extMockSearchProvider{result: "results"}
	tool := &WebSearchTool{
		provider:   provider,
		maxResults: 5,
	}
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"query": "test",
		"count": float64(3),
	})
	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}
}

// ==================== WebFetchTool Options Tests ====================

func TestNewWebFetchTool_Options(t *testing.T) {
	tests := []struct {
		name    string
		max     int
		wantMax int
	}{
		{"zero uses default", 0, 50000},
		{"negative uses default", -100, 50000},
		{"custom value", 10000, 10000},
		{"small value", 100, 100},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			tool := NewWebFetchTool(tt.max)
			if tool.maxChars != tt.wantMax {
				t.Errorf("Expected maxChars %d, got %d", tt.wantMax, tool.maxChars)
			}
		})
	}
}

// ==================== WebSearchTool Options Tests ====================

func TestNewWebSearchTool_Options(t *testing.T) {
	tests := []struct {
		name       string
		opts       WebSearchToolOptions
		wantNil    bool
		wantMax    int
	}{
		{
			name:    "all disabled returns nil",
			opts:    WebSearchToolOptions{},
			wantNil: true,
		},
		{
			name: "DuckDuckGo enabled default max",
			opts: WebSearchToolOptions{DuckDuckGoEnabled: true},
			wantMax: 5,
		},
		{
			name: "DuckDuckGo custom max",
			opts: WebSearchToolOptions{DuckDuckGoEnabled: true, DuckDuckGoMaxResults: 10},
			wantMax: 10,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			tool := NewWebSearchTool(tt.opts)
			if tt.wantNil {
				if tool != nil {
					t.Error("Expected nil tool")
				}
				return
			}
			if tool == nil {
				t.Fatal("Expected non-nil tool")
			}
			if tool.maxResults != tt.wantMax {
				t.Errorf("Expected maxResults %d, got %d", tt.wantMax, tool.maxResults)
			}
		})
	}
}

// ==================== CronTool Add/Remove Lifecycle ====================

func TestCronTool_AddRemoveLifecycle(t *testing.T) {
	cronService := cron.NewCronService(filepath.Join(t.TempDir(), "cron.json"), nil)
	tool := NewCronTool(cronService, nil, nil, "", false, 0, nil)
	tool.SetContext("test-channel", "test-chat")
	ctx := context.Background()

	// Add a job
	addResult := tool.Execute(ctx, map[string]interface{}{
		"action":     "add",
		"message":    "Lifecycle test job",
		"at_seconds": float64(3600),
	})
	if addResult.IsError {
		t.Fatalf("Failed to add job: %s", addResult.ForLLM)
	}

	// Extract job ID from result
	resultText := addResult.ForLLM
	if !strings.Contains(resultText, "id:") {
		t.Fatalf("Expected job ID in result, got '%s'", resultText)
	}

	// List jobs should contain the new job
	listResult := tool.Execute(ctx, map[string]interface{}{
		"action": "list",
	})
	if listResult.IsError {
		t.Errorf("List should not error: %s", listResult.ForLLM)
	}
	if !strings.Contains(listResult.ForLLM, "Lifecycle") {
		t.Errorf("List should contain job name, got '%s'", listResult.ForLLM)
	}
}

func TestCronTool_AddEnableDisableLifecycle(t *testing.T) {
	cronService := cron.NewCronService(filepath.Join(t.TempDir(), "cron.json"), nil)
	tool := NewCronTool(cronService, nil, nil, "", false, 0, nil)
	tool.SetContext("test-channel", "test-chat")
	ctx := context.Background()

	// Add a job first
	addResult := tool.Execute(ctx, map[string]interface{}{
		"action":        "add",
		"message":       "Enable/disable test",
		"every_seconds": float64(60),
	})
	if addResult.IsError {
		t.Fatalf("Failed to add job: %s", addResult.ForLLM)
	}

	// Extract job ID from "Cron job added: name (id: xxx)"
	resultText := addResult.ForLLM
	start := strings.Index(resultText, "(id: ")
	if start < 0 {
		t.Fatalf("Cannot find job ID in result: %s", resultText)
	}
	jobID := resultText[start+5:]
	end := strings.Index(jobID, ")")
	if end >= 0 {
		jobID = jobID[:end]
	}

	// Disable the job
	disableResult := tool.Execute(ctx, map[string]interface{}{
		"action": "disable",
		"job_id": jobID,
	})
	if disableResult.IsError {
		t.Errorf("Disable should succeed: %s", disableResult.ForLLM)
	}

	// Enable the job
	enableResult := tool.Execute(ctx, map[string]interface{}{
		"action": "enable",
		"job_id": jobID,
	})
	if enableResult.IsError {
		t.Errorf("Enable should succeed: %s", enableResult.ForLLM)
	}

	// Remove the job
	removeResult := tool.Execute(ctx, map[string]interface{}{
		"action": "remove",
		"job_id": jobID,
	})
	if removeResult.IsError {
		t.Errorf("Remove should succeed: %s", removeResult.ForLLM)
	}
}

// ==================== CronTool ExecuteJob Error Cases ====================

func TestCronTool_ExecuteJob_ExecutorError(t *testing.T) {
	cronService := cron.NewCronService(filepath.Join(t.TempDir(), "cron.json"), nil)
	msgBus := bus.NewMessageBus()
	executor := &errorJobExecutor{}
	tool := NewCronTool(cronService, executor, msgBus, "", false, 0, nil)
	ctx := context.Background()

	job := &cron.CronJob{
		ID:   "test-err-job",
		Name: "Error Job",
		Payload: cron.CronPayload{
			Message: "This will fail",
			Deliver: false,
			Channel: "test-channel",
			To:      "test-chat",
		},
	}

	result := tool.ExecuteJob(ctx, job)
	if result == "ok" {
		t.Error("Expected error result when executor fails")
	}
	if !strings.Contains(result, "Error") {
		t.Errorf("Expected error message, got '%s'", result)
	}
}

// errorJobExecutor returns an error for testing
type errorJobExecutor struct{}

func (e *errorJobExecutor) ProcessDirectWithChannel(ctx context.Context, content, sessionKey, channel, chatID string) (string, error) {
	return "", context.DeadlineExceeded
}
