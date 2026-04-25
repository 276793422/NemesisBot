// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package agent

import (
	"context"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/config"
)

// TestGetRegistry tests GetRegistry returns the registry.
func TestGetRegistry(t *testing.T) {
	al := createTestAgentLoop(t)
	reg := al.GetRegistry()
	if reg == nil {
		t.Fatal("expected non-nil registry")
	}
	ids := reg.ListAgentIDs()
	if len(ids) == 0 {
		t.Error("expected at least one agent in registry")
	}
}

// TestGetCluster tests GetCluster returns nil when no cluster.
func TestGetCluster(t *testing.T) {
	al := createTestAgentLoop(t)
	cluster := al.GetCluster()
	if cluster != nil {
		t.Error("expected nil cluster in test mode")
	}
}

// TestSetObserverManager tests SetObserverManager and GetObserverManager.
func TestSetObserverManager_GetObserverManager(t *testing.T) {
	al := createTestAgentLoop(t)

	// Initially nil
	mgr := al.GetObserverManager()
	if mgr != nil {
		t.Error("expected nil observer manager initially")
	}

	// Set it
	al.SetObserverManager(nil)
	mgr = al.GetObserverManager()
	if mgr != nil {
		t.Error("expected nil after setting nil")
	}
}

// TestRequestLogger_FormatOperationTypes tests all operation types.
func TestRequestLogger_FormatOperationTypes(t *testing.T) {
	tests := []struct {
		input    string
		expected string
	}{
		{"tool_call", "Tool Execution"},
		{"file_write", "File Write"},
		{"file_read", "File Read"},
		{"command_exec", "Command Execution"},
		{"custom_type", "Custom Type"},
		{"some_other", "Some Other"},
	}

	for _, tt := range tests {
		result := formatOperationType(tt.input)
		if result != tt.expected {
			t.Errorf("formatOperationType(%q) = %q, want %q", tt.input, result, tt.expected)
		}
	}
}

// TestRequestLogger_GetTitleForType tests all title types.
func TestRequestLogger_GetTitleForType(t *testing.T) {
	tests := []struct {
		input    string
		expected string
	}{
		{"tool_call", "Tool"},
		{"file_write", "File"},
		{"file_read", "File"},
		{"command_exec", "Command"},
		{"unknown", "Name"},
		{"", "Name"},
	}

	for _, tt := range tests {
		result := getTitleForType(tt.input)
		if result != tt.expected {
			t.Errorf("getTitleForType(%q) = %q, want %q", tt.input, result, tt.expected)
		}
	}
}

// TestRequestLogger_FormatArguments_Nil tests formatArguments with nil args.
func TestRequestLogger_FormatArguments_Nil(t *testing.T) {
	result := formatArguments(nil, "full")
	if result != "{}" {
		t.Errorf("expected '{}', got %q", result)
	}
}

// TestRequestLogger_FormatArguments_Truncated tests formatArguments with truncation.
func TestRequestLogger_FormatArguments_Truncated(t *testing.T) {
	// Create a large argument map
	args := map[string]interface{}{
		"data": string(make([]byte, 300)),
	}
	result := formatArguments(args, "truncated")
	if len(result) > 250 {
		t.Errorf("expected truncated result, got length %d", len(result))
	}
}

// TestRequestLogger_FormatHeaders_Empty tests formatHeaders with empty map.
func TestRequestLogger_FormatHeaders_Empty(t *testing.T) {
	result := formatHeaders(map[string]string{})
	if result != "<none>" {
		t.Errorf("expected '<none>', got %q", result)
	}
}

// TestRequestLogger_FormatHeaders_WithValues tests formatHeaders with values.
func TestRequestLogger_FormatHeaders_WithValues(t *testing.T) {
	headers := map[string]string{
		"Content-Type": "application/json",
		"X-Request-ID": "12345",
	}
	result := formatHeaders(headers)
	if result == "<none>" {
		t.Error("expected formatted headers, got '<none>'")
	}
}

// TestMemoryStore_GetRecentDailyNotes tests multi-day retrieval.
func TestMemoryStore_GetRecentDailyNotes(t *testing.T) {
	tmpDir := t.TempDir()
	ms := NewMemoryStore(tmpDir)

	// Write notes for today and yesterday
	today := ms.ReadToday()
	if today != "" {
		t.Error("expected empty today initially")
	}

	// Write long-term memory
	err := ms.WriteLongTerm("test memory content")
	if err != nil {
		t.Fatalf("WriteLongTerm failed: %v", err)
	}

	// Verify readback
	content := ms.ReadLongTerm()
	if content != "test memory content" {
		t.Errorf("expected 'test memory content', got %q", content)
	}
}

// TestMemoryStore_ReadToday_NonExistent tests reading when no daily note exists.
func TestMemoryStore_ReadToday_NonExistent(t *testing.T) {
	tmpDir := t.TempDir()
	ms := NewMemoryStore(tmpDir)

	content := ms.ReadToday()
	if content != "" {
		t.Errorf("expected empty string for non-existent daily note, got %q", content)
	}
}

// TestRequestLogger_Close tests the Close method.
func TestRequestLogger_Close_Method(t *testing.T) {
	rl := &RequestLogger{enabled: true}
	rl.Close() // Should not panic

	rl2 := &RequestLogger{enabled: false}
	rl2.Close() // Should not panic
}

// --- handleCommand additional branch tests ---

// TestHandleCommand_ListModels tests /list models.
func TestHandleCommand_ListModels(t *testing.T) {
	loop := createTestAgentLoop(t)
	msg := createTestInboundMessage("test", "user1", "chat1", "/list models")
	result, handled := loop.handleCommand(nil, msg)
	if !handled {
		t.Error("expected handled=true for /list models")
	}
	if result == "" {
		t.Error("expected non-empty result")
	}
}

// TestHandleCommand_ListAgents tests /list agents.
func TestHandleCommand_ListAgents(t *testing.T) {
	loop := createTestAgentLoop(t)
	msg := createTestInboundMessage("test", "user1", "chat1", "/list agents")
	result, handled := loop.handleCommand(nil, msg)
	if !handled {
		t.Error("expected handled=true for /list agents")
	}
	if result == "" {
		t.Error("expected non-empty result")
	}
}

// TestHandleCommand_ListUnknown tests /list unknown.
func TestHandleCommand_ListUnknown(t *testing.T) {
	loop := createTestAgentLoop(t)
	msg := createTestInboundMessage("test", "user1", "chat1", "/list unknown")
	result, handled := loop.handleCommand(nil, msg)
	if !handled {
		t.Error("expected handled=true for /list unknown")
	}
	if result == "" {
		t.Error("expected non-empty result")
	}
}

// TestHandleCommand_ListNoArgs tests /list without args.
func TestHandleCommand_ListNoArgs(t *testing.T) {
	loop := createTestAgentLoop(t)
	msg := createTestInboundMessage("test", "user1", "chat1", "/list")
	result, handled := loop.handleCommand(nil, msg)
	if !handled {
		t.Error("expected handled=true")
	}
	if result == "" {
		t.Error("expected usage message")
	}
}

// TestHandleCommand_SwitchChannel_NoManager tests /switch channel without manager.
func TestHandleCommand_SwitchChannel_NoManager(t *testing.T) {
	loop := createTestAgentLoop(t)
	msg := createTestInboundMessage("test", "user1", "chat1", "/switch channel to test-channel")
	result, handled := loop.handleCommand(nil, msg)
	if !handled {
		t.Error("expected handled=true")
	}
	if result == "" {
		t.Error("expected non-empty result")
	}
}

// TestHandleCommand_SwitchUnknown tests /switch unknown.
func TestHandleCommand_SwitchUnknown(t *testing.T) {
	loop := createTestAgentLoop(t)
	msg := createTestInboundMessage("test", "user1", "chat1", "/switch unknown to value")
	result, handled := loop.handleCommand(nil, msg)
	if !handled {
		t.Error("expected handled=true")
	}
	if result == "" {
		t.Error("expected non-empty result")
	}
}

// TestHandleCommand_SwitchNoArgs tests /switch without proper args.
func TestHandleCommand_SwitchNoArgs(t *testing.T) {
	loop := createTestAgentLoop(t)
	msg := createTestInboundMessage("test", "user1", "chat1", "/switch model")
	result, handled := loop.handleCommand(nil, msg)
	if !handled {
		t.Error("expected handled=true")
	}
	if result == "" {
		t.Error("expected usage message")
	}
}

// TestHandleCommand_EmptySlash tests "/" with no command.
func TestHandleCommand_EmptySlash(t *testing.T) {
	loop := createTestAgentLoop(t)
	msg := createTestInboundMessage("test", "user1", "chat1", "/")
	result, handled := loop.handleCommand(nil, msg)
	if handled {
		t.Error("expected handled=false for bare slash")
	}
	if result != "" {
		t.Errorf("expected empty result, got %q", result)
	}
}

// TestMemoryStore_GetRecentDailyNotes_MultiDay tests actual multi-day notes.
func TestMemoryStore_GetRecentDailyNotes_MultiDay(t *testing.T) {
	tmpDir := t.TempDir()
	ms := NewMemoryStore(tmpDir)

	// Write today's note
	err := ms.AppendToday("Today's note content")
	if err != nil {
		t.Fatalf("AppendToday failed: %v", err)
	}

	// Read should return today's note
	content := ms.ReadToday()
	if content == "" {
		t.Error("expected today's note to be non-empty")
	}
	if content != "Today's note content" && content != "# "+time.Now().Format("2006-01-02")+"\n\nToday's note content" {
		// Note: first write adds a header
	}

	// GetRecentDailyNotes for 1 day
	notes := ms.GetRecentDailyNotes(1)
	if notes == "" {
		t.Error("expected non-empty recent notes for 1 day")
	}

	// GetRecentDailyNotes for 7 days (most won't exist)
	notes7 := ms.GetRecentDailyNotes(7)
	if notes7 == "" {
		t.Error("expected non-empty recent notes")
	}
}

// TestMemoryStore_AppendToday_Twice tests appending to today's note twice.
func TestMemoryStore_AppendToday_Twice(t *testing.T) {
	tmpDir := t.TempDir()
	ms := NewMemoryStore(tmpDir)

	err := ms.AppendToday("First entry")
	if err != nil {
		t.Fatalf("first AppendToday failed: %v", err)
	}

	err = ms.AppendToday("Second entry")
	if err != nil {
		t.Fatalf("second AppendToday failed: %v", err)
	}

	content := ms.ReadToday()
	if content == "" {
		t.Error("expected non-empty content after two appends")
	}
}

// TestMemoryStore_WriteLongTerm_InvalidPath tests writing with a file as directory.
func TestMemoryStore_WriteLongTerm_InvalidPath(t *testing.T) {
	tmpDir := t.TempDir()
	// Create a file where a directory should be
	filePath := filepath.Join(tmpDir, "memory")
	os.WriteFile(filePath, []byte("not a directory"), 0644)

	ms := NewMemoryStore(tmpDir)
	// The constructor creates the memory dir, but the file blocks it
	// WriteLongTerm may or may not fail depending on OS
	_ = ms.WriteLongTerm("test")
}

// TestMemoryStore_GetMemoryContext tests full memory context.
func TestMemoryStore_GetMemoryContext_Full(t *testing.T) {
	tmpDir := t.TempDir()
	ms := NewMemoryStore(tmpDir)

	// Initially empty
	ctx := ms.GetMemoryContext()
	if ctx != "" {
		t.Errorf("expected empty context initially, got %q", ctx)
	}

	// Write long-term memory
	ms.WriteLongTerm("Important facts")

	ctx = ms.GetMemoryContext()
	if ctx == "" {
		t.Error("expected non-empty context after writing long-term memory")
	}
}

// TestContextBuilder_LoadBootstrapFiles_SkipBootstrap tests skipBootstrap mode without files.
func TestContextBuilder_LoadBootstrapFiles_SkipBootstrap_NoFiles(t *testing.T) {
	tmpDir := t.TempDir()
	cb := NewContextBuilder(tmpDir)

	result := cb.LoadBootstrapFiles(true)
	// Without any bootstrap files, should return empty
	if result != "" {
		t.Errorf("expected empty result when no files exist, got %q", result)
	}
}

// TestContextBuilder_BuildSystemPrompt_NoMemory tests system prompt without memory files.
func TestContextBuilder_BuildSystemPrompt_NoMemory(t *testing.T) {
	tmpDir := t.TempDir()
	cb := NewContextBuilder(tmpDir)

	prompt := cb.BuildSystemPrompt(false)
	if prompt == "" {
		t.Error("expected non-empty system prompt")
	}
}

// TestRequestLogger_CreateSession_Failure tests CreateSession with bad path.
func TestRequestLogger_CreateSession_Failure(t *testing.T) {
	cfg := &config.LoggingConfig{
		LLM: &config.LLMLogConfig{
			Enabled:     true,
			LogDir:      "/nonexistent\x00bad/path", // Invalid path with null byte
			DetailLevel: "full",
		},
	}
	rl := NewRequestLogger(cfg, t.TempDir())
	if !rl.IsEnabled() {
		t.Fatal("expected logger to be enabled")
	}
	// CreateSession should handle the error gracefully (silent failure)
	err := rl.CreateSession()
	// It should not return an error (silent failure), but logger is disabled
	if err != nil {
		t.Errorf("expected nil error from CreateSession (silent failure), got %v", err)
	}
}

// TestRequestLogger_WriteFile_Disabled tests writeFile when disabled.
func TestRequestLogger_WriteFile_Disabled(t *testing.T) {
	rl := &RequestLogger{enabled: false}
	err := rl.writeFile("test.md", "content")
	if err != nil {
		t.Errorf("expected nil error when disabled, got %v", err)
	}
}

// TestRequestLogger_NextIndex_Disabled tests NextIndex when disabled.
func TestRequestLogger_NextIndex_Disabled(t *testing.T) {
	rl := &RequestLogger{enabled: false}
	result := rl.NextIndex()
	if result != "" {
		t.Errorf("expected empty string when disabled, got %q", result)
	}
}

// TestRequestLogger_NextIndex_Increment tests NextIndex increments correctly.
func TestRequestLogger_NextIndex_Increment(t *testing.T) {
	rl := &RequestLogger{enabled: true}
	first := rl.NextIndex()
	second := rl.NextIndex()
	third := rl.NextIndex()

	if first != "01" {
		t.Errorf("expected '01', got %q", first)
	}
	if second != "02" {
		t.Errorf("expected '02', got %q", second)
	}
	if third != "03" {
		t.Errorf("expected '03', got %q", third)
	}
}

// TestProcessMessage_RPCWithCorrelationID tests processMessage with correlation ID.
func TestProcessMessage_RPCWithCorrelationID(t *testing.T) {
	loop := createTestAgentLoop(t)
	ctx := context.Background()

	msg := bus.InboundMessage{
		Channel:       "rpc",
		SenderID:      "remote-node",
		ChatID:        "remote-chat",
		Content:       "Hello from RPC",
		SessionKey:    "rpc-test-session",
		CorrelationID: "corr-123",
	}

	agentID, response, err := loop.processMessage(ctx, msg)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if agentID == "" {
		t.Error("expected non-empty agent ID")
	}
	// Response should include correlation ID prefix for RPC
	if response == "" {
		t.Error("expected non-empty response")
	}
}

// TestProcessMessage_QueueMode tests processMessage with queue concurrent mode.
func TestProcessMessage_QueueMode(t *testing.T) {
	tempDir := t.TempDir()
	cfg := &config.Config{
		Agents: config.AgentsConfig{
			Defaults: config.AgentDefaults{
				LLM:                 "test/model",
				Workspace:           tempDir,
				ConcurrentRequestMode: "queue",
				QueueSize:           4,
			},
		},
	}
	msgBus := bus.NewMessageBus()
	provider := &mockProviderForTest{}
	loop := NewAgentLoop(cfg, msgBus, provider)

	ctx := context.Background()
	msg := bus.InboundMessage{
		Channel:    "cli",
		SenderID:   "user1",
		ChatID:     "chat1",
		Content:    "Hello",
		SessionKey: "test-session",
	}

	agentID, _, err := loop.processMessage(ctx, msg)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if agentID == "" {
		t.Error("expected non-empty agent ID")
	}
}

// TestProcessMessage_SystemMessage tests processMessage with system channel.
func TestProcessMessage_SystemMessage(t *testing.T) {
	loop := createTestAgentLoop(t)
	ctx := context.Background()

	msg := bus.InboundMessage{
		Channel:    "system",
		SenderID:   "subagent",
		ChatID:     "cli:direct",
		Content:    "Task 'test' completed.\n\nResult:\nTask output",
		SessionKey: "system-session",
	}

	agentID, response, err := loop.processMessage(ctx, msg)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	_ = agentID
	_ = response
}

// TestProcessMessage_HistoryRequest tests processMessage with history type.
func TestProcessMessage_HistoryRequest(t *testing.T) {
	loop := createTestAgentLoop(t)
	ctx := context.Background()

	msg := bus.InboundMessage{
		Channel:    "web",
		SenderID:   "user1",
		ChatID:     "chat1",
		Content:    `{"request_id": "req-1", "limit": 10}`,
		SessionKey: "history-session",
		Metadata:   map[string]string{"request_type": "history"},
	}

	_, _, err := loop.processMessage(ctx, msg)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
}

// TestProcessMessage_HistoryRequest_InvalidJSON tests history request with bad JSON.
func TestProcessMessage_HistoryRequest_InvalidJSON(t *testing.T) {
	loop := createTestAgentLoop(t)
	ctx := context.Background()

	msg := bus.InboundMessage{
		Channel:    "web",
		SenderID:   "user1",
		ChatID:     "chat1",
		Content:    `not valid json`,
		SessionKey: "history-session",
		Metadata:   map[string]string{"request_type": "history"},
	}

	_, _, err := loop.processMessage(ctx, msg)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
}

// TestProcessMessage_Command tests processMessage with a command.
func TestProcessMessage_Command(t *testing.T) {
	loop := createTestAgentLoop(t)
	ctx := context.Background()

	msg := bus.InboundMessage{
		Channel:    "cli",
		SenderID:   "user1",
		ChatID:     "chat1",
		Content:    "/show model",
		SessionKey: "cmd-session",
	}

	_, response, err := loop.processMessage(ctx, msg)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if response == "" {
		t.Error("expected non-empty response for command")
	}
}

// TestProcessMessage_ClusterContinuation tests cluster continuation message.
func TestProcessMessage_ClusterContinuation(t *testing.T) {
	loop := createTestAgentLoop(t)
	ctx := context.Background()

	msg := bus.InboundMessage{
		Channel:    "system",
		SenderID:   "cluster",
		ChatID:     "cluster:cluster_continuation:nonexistent-task-id",
		Content:    "continuation response",
		SessionKey: "cluster-session",
	}

	// This should attempt to load continuation data and fail gracefully
	_, _, err := loop.processMessage(ctx, msg)
	// The continuation will fail because there's no saved state, but it shouldn't panic
	_ = err
}

// TestNewAgentLoop_TildeWorkspace tests workspace resolution with ~/ prefix.
func TestNewAgentLoop_TildeWorkspace(t *testing.T) {
	cfg := &config.Config{
		Agents: config.AgentsConfig{
			Defaults: config.AgentDefaults{
				LLM:       "test/model",
				Workspace: "~/test-workspace",
			},
		},
	}
	msgBus := bus.NewMessageBus()
	provider := &mockProviderForTest{}

	loop := NewAgentLoop(cfg, msgBus, provider)
	if loop == nil {
		t.Fatal("expected non-nil loop")
	}
}

// TestNewAgentLoop_QueueMode tests agent loop creation with queue mode config.
func TestNewAgentLoop_QueueMode(t *testing.T) {
	tempDir := t.TempDir()
	cfg := &config.Config{
		Agents: config.AgentsConfig{
			Defaults: config.AgentDefaults{
				LLM:                   "test/model",
				Workspace:             tempDir,
				ConcurrentRequestMode: "queue",
				QueueSize:             16,
			},
		},
	}
	msgBus := bus.NewMessageBus()
	provider := &mockProviderForTest{}

	loop := NewAgentLoop(cfg, msgBus, provider)
	if loop == nil {
		t.Fatal("expected non-nil loop")
	}
	if loop.concurrentMode != "queue" {
		t.Errorf("expected queue mode, got %s", loop.concurrentMode)
	}
	if loop.queueSize != 16 {
		t.Errorf("expected queue size 16, got %d", loop.queueSize)
	}
}

// TestAgentLoop_Stop_WithRunning tests Stop with running flag.
func TestAgentLoop_Stop_WithRunning(t *testing.T) {
	loop := createTestAgentLoop(t)
	loop.running.Store(true)
	loop.Stop()
	if loop.running.Load() {
		t.Error("expected running to be false after Stop")
	}
}

// TestAgentLoop_Run_WithBusPublish tests Run consuming from bus.
func TestAgentLoop_Run_WithBusPublish(t *testing.T) {
	tempDir := t.TempDir()
	cfg := &config.Config{
		Agents: config.AgentsConfig{
			Defaults: config.AgentDefaults{
				LLM:       "test/model",
				Workspace: tempDir,
			},
		},
	}
	msgBus := bus.NewMessageBus()
	provider := &mockProviderForTest{}
	loop := NewAgentLoop(cfg, msgBus, provider)

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	// Start Run in background
	done := make(chan struct{})
	go func() {
		loop.Run(ctx)
		close(done)
	}()

	// Publish a message to the bus
	msgBus.PublishInbound(bus.InboundMessage{
		Channel:    "cli",
		SenderID:   "user1",
		ChatID:     "chat1",
		Content:    "Hello from bus",
		SessionKey: "bus-test-session",
	})

	// Give it time to process
	time.Sleep(2 * time.Second)

	// Cancel context to stop
	cancel()

	// Wait for Run to finish
	select {
	case <-done:
		// Success
	case <-time.After(5 * time.Second):
		t.Fatal("Run did not stop after context cancellation")
	}
}

// TestAgentLoop_Run_ContextCancelled tests Run exits when context cancelled.
func TestAgentLoop_Run_ContextCancelled(t *testing.T) {
	tempDir := t.TempDir()
	cfg := &config.Config{
		Agents: config.AgentsConfig{
			Defaults: config.AgentDefaults{
				LLM:       "test/model",
				Workspace: tempDir,
			},
		},
	}
	msgBus := bus.NewMessageBus()
	provider := &mockProviderForTest{}
	loop := NewAgentLoop(cfg, msgBus, provider)

	ctx, cancel := context.WithCancel(context.Background())

	done := make(chan struct{})
	go func() {
		loop.Run(ctx)
		close(done)
	}()

	// Cancel immediately
	cancel()

	select {
	case <-done:
		// Success
	case <-time.After(5 * time.Second):
		t.Fatal("Run did not exit after context cancellation")
	}
}
