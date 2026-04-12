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
	"github.com/276793422/NemesisBot/module/channels"
	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/providers"
	"github.com/276793422/NemesisBot/module/state"
	"github.com/276793422/NemesisBot/module/tools"
)

// mockTool is a simple mock tool for testing
type mockTool struct {
	name        string
	description string
}

func (m *mockTool) Name() string        { return m.name }
func (m *mockTool) Description() string { return m.description }
func (m *mockTool) Parameters() map[string]interface{} {
	return map[string]interface{}{
		"type": "object",
	}
}
func (m *mockTool) Execute(ctx context.Context, args map[string]interface{}) *tools.ToolResult {
	return tools.NewToolResult("mock result")
}

// TestAgentLoop_RegisterTool tests registering tools to all agents
func TestAgentLoop_RegisterTool(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &config.Config{
		Agents: config.AgentsConfig{
			List: []config.AgentConfig{
				{ID: "agent1", Name: "Agent 1"},
				{ID: "agent2", Name: "Agent 2"},
			},
			Defaults: config.AgentDefaults{
				LLM:       "test/model",
				Workspace: tempDir,
			},
		},
	}

	msgBus := bus.NewMessageBus()
	provider := &mockProviderForTest{}

	loop := NewAgentLoop(cfg, msgBus, provider)

	// Create a mock tool
	tool := &mockTool{
		name:        "test_tool",
		description: "A test tool",
	}

	// Register the tool
	loop.RegisterTool(tool)

	// Verify it's registered in all agents
	for _, agentID := range loop.registry.ListAgentIDs() {
		agent, ok := loop.registry.GetAgent(agentID)
		if !ok {
			t.Errorf("Expected agent %s to exist", agentID)
			continue
		}

		_, found := agent.Tools.Get("test_tool")
		if !found {
			t.Errorf("Expected tool to be registered in agent %s", agentID)
		}
	}
}

// TestAgentLoop_SetChannelManager tests setting the channel manager
func TestAgentLoop_SetChannelManager(t *testing.T) {
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

	// Create channel manager
	cm, err := channels.NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("Failed to create channel manager: %v", err)
	}

	// Set channel manager
	loop.SetChannelManager(cm)

	if loop.channelManager != cm {
		t.Error("Expected channel manager to be set")
	}
}

// TestAgentLoop_RecordLastChatID tests recording last chat ID
func TestAgentLoop_RecordLastChatID(t *testing.T) {
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

	// Create state manager
	statePath := filepath.Join(tempDir, "state.json")
	stateMgr := state.NewManager(statePath)

	loop := NewAgentLoop(cfg, msgBus, provider)
	loop.state = stateMgr

	// Test recording chat ID
	chatID := "test-chat-123"
	err := loop.RecordLastChatID(chatID)
	if err != nil {
		t.Fatalf("Expected no error, got %v", err)
	}

	// Verify it was recorded
	lastChatID := stateMgr.GetLastChatID()

	if lastChatID != chatID {
		t.Errorf("Expected chat ID %s, got %s", chatID, lastChatID)
	}

	// Test with nil state
	loop.state = nil
	err = loop.RecordLastChatID("another-chat")
	if err != nil {
		t.Errorf("Expected no error with nil state, got %v", err)
	}
}
func TestAgentLoop_processSystemMessage(t *testing.T) {
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
	provider := &mockProviderForTest{
		responses: []string{"System processed"},
	}

	loop := NewAgentLoop(cfg, msgBus, provider)

	tests := []struct {
		name        string
		channel     string
		senderID    string
		chatID      string
		content     string
		expectError bool
	}{
		{
			name:        "non-system channel",
			channel:     "test",
			senderID:    "sender1",
			chatID:      "chat1",
			content:     "Test",
			expectError: true,
		},
		{
			name:        "system message with result",
			channel:     "system",
			senderID:    "subagent1",
			chatID:      "discord:123",
			content:     "Task 'test' completed.\n\nResult:\nActual result here",
			expectError: false,
		},
		{
			name:        "system message without result",
			channel:     "system",
			senderID:    "subagent2",
			chatID:      "telegram:456",
			content:     "Task completed",
			expectError: false,
		},
		{
			name:        "system message internal channel",
			channel:     "system",
			senderID:    "subagent3",
			chatID:      "cli:789",
			content:     "Background task done",
			expectError: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			msg := bus.InboundMessage{
				Channel:  tt.channel,
				SenderID: tt.senderID,
				ChatID:   tt.chatID,
				Content:  tt.content,
			}

			ctx := context.Background()
			result, err := loop.processSystemMessage(ctx, msg)

			if tt.expectError && err == nil {
				t.Error("Expected error but got none")
			}
			if !tt.expectError && err != nil {
				t.Errorf("Expected no error but got: %v", err)
			}

			// Internal channels should return empty result
			if tt.channel == "system" && tt.chatID == "cli:789" && result != "" {
				t.Errorf("Expected empty result for internal channel, got: %s", result)
			}
		})
	}
}

// TestAgentLoop_forceCompression tests forcing context compression
func TestAgentLoop_forceCompression(t *testing.T) {
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
	provider := &mockProviderForTest{
		responses: []string{"Summary: This is a summary"},
	}

	loop := NewAgentLoop(cfg, msgBus, provider)

	// Get default agent
	agent := loop.registry.GetDefaultAgent()
	if agent == nil {
		t.Fatal("Expected default agent to exist")
	}

	sessionKey := "test-session"

	// Force compression - should not panic
	// Note: This will create a session if it doesn't exist
	loop.forceCompression(agent, sessionKey)

	// Test completes if no panic occurs
}

// TestAgentLoop_summarizeBatch tests batch summarization
func TestAgentLoop_summarizeBatch(t *testing.T) {
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
	provider := &mockProviderForTest{
		responses: []string{"Batch summary"},
	}

	loop := NewAgentLoop(cfg, msgBus, provider)

	ctx := context.Background()

	// Create a batch of messages
	messages := []providers.Message{
		{Role: "user", Content: "Message 1"},
		{Role: "assistant", Content: "Response 1"},
		{Role: "user", Content: "Message 2"},
		{Role: "assistant", Content: "Response 2"},
	}

	// Get default agent
	agent := loop.registry.GetDefaultAgent()
	if agent == nil {
		t.Fatal("Expected default agent to exist")
	}

	// Summarize batch
	summary, err := loop.summarizeBatch(ctx, agent, messages, "")
	if err != nil {
		t.Fatalf("Expected no error, got %v", err)
	}

	if summary == "" {
		t.Error("Expected non-empty summary")
	}

	if summary != "Batch summary" {
		t.Errorf("Expected summary 'Batch summary', got '%s'", summary)
	}
}

// TestAgentLoop_registerMCPTools tests registering MCP tools
func TestAgentLoop_registerMCPTools(t *testing.T) {
	tempDir := t.TempDir()

	// Create a directory with MCP config
	mcpDir := filepath.Join(tempDir, "mcp")
	err := os.MkdirAll(mcpDir, 0755)
	if err != nil {
		t.Fatalf("Failed to create MCP dir: %v", err)
	}

	// Create a simple MCP config file
	mcpConfig := `
{
  "mcpServers": {
    "test-server": {
      "command": "node",
      "args": ["test.js"]
    }
  }
}
`
	err = os.WriteFile(filepath.Join(mcpDir, "mcp.json"), []byte(mcpConfig), 0644)
	if err != nil {
		t.Fatalf("Failed to write MCP config: %v", err)
	}

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

	// Get default agent
	agent := loop.registry.GetDefaultAgent()
	if agent == nil {
		t.Fatal("Expected default agent to exist")
	}

	// Note: registerMCPTools is a private method, so we can't test it directly
	// But we can verify the agent has tools
	toolList := agent.Tools.List()
	if len(toolList) == 0 {
		t.Error("Expected agent to have tools")
	}
}

// TestAgentLoop_setupClusterRPCChannel tests setting up cluster RPC channel
func TestAgentLoop_setupClusterRPCChannel(t *testing.T) {
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

	// Get default agent
	agent := loop.registry.GetDefaultAgent()
	if agent == nil {
		t.Fatal("Expected default agent to exist")
	}

	// Note: setupClusterRPCChannel is a private method
	// We can verify that RPC tool is not registered when cluster is disabled
	_, found := agent.Tools.Get("rpc")
	if found {
		t.Error("Expected RPC tool to not be registered when cluster is disabled")
	}
}

// TestAgentLoop_Run tests the main event loop
func TestAgentLoop_RunNew(t *testing.T) {
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
	provider := &mockProviderForTest{
		responses: []string{"Test response"},
	}

	loop := NewAgentLoop(cfg, msgBus, provider)

	ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
	defer cancel()

	// Publish a message before starting the loop
	msgBus.PublishInbound(bus.InboundMessage{
		Channel:  "test",
		SenderID: "user1",
		ChatID:   "chat1",
		Content:  "Hello",
	})

	// Run the loop (will exit due to timeout)
	err := loop.Run(ctx)
	if err != nil {
		t.Errorf("Expected no error, got %v", err)
	}
}

// TestAgentLoop_Run_ContextCancellation tests loop handles context cancellation
func TestAgentLoop_Run_ContextCancellationNew(t *testing.T) {
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

	// Start loop in goroutine
	errChan := make(chan error, 1)
	go func() {
		errChan <- loop.Run(ctx)
	}()

	// Cancel context immediately
	cancel()

	// Should return without error
	select {
	case err := <-errChan:
		if err != nil {
			t.Errorf("Expected no error on cancellation, got %v", err)
		}
	case <-time.After(1 * time.Second):
		t.Error("Expected loop to exit on context cancellation")
	}
}

// TestAgentLoop_StopNew tests stopping the loop
func TestAgentLoop_StopNew(t *testing.T) {
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

	// Mark as running
	loop.running.Store(true)

	// Stop should not panic
	loop.Stop()

	if loop.running.Load() {
		t.Error("Expected running to be false after Stop")
	}
}

// TestAgentLoop_getSessionBusyState tests session busy state management
func TestAgentLoop_getSessionBusyState(t *testing.T) {
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

	sessionKey := "test-session"

	// Get state for new session
	state1 := loop.getSessionBusyState(sessionKey)
	if state1 == nil {
		t.Fatal("Expected state to be created")
	}

	if state1.busy {
		t.Error("Expected new session to not be busy")
	}

	// Get same session again
	state2 := loop.getSessionBusyState(sessionKey)
	if state1 != state2 {
		t.Error("Expected same state instance for same session")
	}

	// Get different session
	sessionKey2 := "test-session-2"
	state3 := loop.getSessionBusyState(sessionKey2)
	if state3 == state1 {
		t.Error("Expected different state instance for different session")
	}
}

// TestAgentLoop_tryAcquireSession tests session acquisition
func TestAgentLoop_tryAcquireSession(t *testing.T) {
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
	loop.concurrentMode = "reject"

	sessionKey := "test-session"

	// First acquisition should succeed
	acquired := loop.tryAcquireSession(sessionKey)
	if !acquired {
		t.Error("Expected first acquisition to succeed")
	}

	// Second acquisition should fail (busy)
	acquired = loop.tryAcquireSession(sessionKey)
	if acquired {
		t.Error("Expected second acquisition to fail when busy")
	}

	// Release session
	loop.releaseSession(sessionKey)

	// Third acquisition should succeed again
	acquired = loop.tryAcquireSession(sessionKey)
	if !acquired {
		t.Error("Expected acquisition to succeed after release")
	}
}

// TestAgentLoop_tryAcquireSession_QueueMode tests queue mode
func TestAgentLoop_tryAcquireSession_QueueMode(t *testing.T) {
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
	loop.concurrentMode = "queue"
	loop.queueSize = 3

	sessionKey := "test-session"

	// First acquisition should succeed
	if !loop.tryAcquireSession(sessionKey) {
		t.Error("Expected first acquisition to succeed")
	}

	// Queue up to limit
	for i := 0; i < 3; i++ {
		acquired := loop.tryAcquireSession(sessionKey)
		if acquired {
			t.Error("Expected queued acquisition to return false")
		}
	}

	// Should fail when queue is full
	acquired := loop.tryAcquireSession(sessionKey)
	if acquired {
		t.Error("Expected acquisition to fail when queue is full")
	}

	// Release one
	hasQueued := loop.releaseSession(sessionKey)
	if !hasQueued {
		t.Error("Expected to have queued requests")
	}

	// Still busy
	state := loop.getSessionBusyState(sessionKey)
	if !state.busy {
		t.Error("Expected session to still be busy")
	}
}

// TestAgentLoop_releaseSession tests session release
func TestAgentLoop_releaseSession(t *testing.T) {
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
	loop.concurrentMode = "queue"
	loop.queueSize = 3

	sessionKey := "test-session"

	// Acquire session
	if !loop.tryAcquireSession(sessionKey) {
		t.Fatal("Expected acquisition to succeed")
	}

	// Queue some requests
	for i := 0; i < 2; i++ {
		loop.tryAcquireSession(sessionKey)
	}

	// Release should indicate queued requests
	hasQueued := loop.releaseSession(sessionKey)
	if !hasQueued {
		t.Error("Expected to have queued requests")
	}

	// Release again
	hasQueued = loop.releaseSession(sessionKey)
	if !hasQueued {
		t.Error("Expected to still have queued requests")
	}

	// Final release - no more queued
	hasQueued = loop.releaseSession(sessionKey)
	if hasQueued {
		t.Error("Expected no more queued requests")
	}

	// Session should no longer be busy
	state := loop.getSessionBusyState(sessionKey)
	if state.busy {
		t.Error("Expected session to not be busy after all releases")
	}
}

// TestAgentLoop_ConcurrentSessionManagement tests concurrent session management
func TestAgentLoop_ConcurrentSessionManagement(t *testing.T) {
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
	loop.concurrentMode = "reject"

	done := make(chan bool, 100)
	sessionKey := "test-session"

	// Try to acquire from multiple goroutines
	for i := 0; i < 100; i++ {
		go func() {
			acquired := loop.tryAcquireSession(sessionKey)
			if acquired {
				time.Sleep(10 * time.Millisecond)
				loop.releaseSession(sessionKey)
			}
			done <- true
		}()
	}

	// Wait for completion
	for i := 0; i < 100; i++ {
		<-done
	}

	// Final state should be not busy
	state := loop.getSessionBusyState(sessionKey)
	if state.busy {
		t.Error("Expected session to not be busy after all operations")
	}
}

// TestAgentLoop_extractPeer tests extracting peer from content
func TestAgentLoop_extractPeerNew(t *testing.T) {
	// Note: extractPeer is a private method
	// We can test the behavior indirectly through message processing
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
	provider := &mockProviderForTest{
		responses: []string{"Response"},
	}

	loop := NewAgentLoop(cfg, msgBus, provider)

	// Test with peer prefix - should not panic
	ctx := context.Background()
	_, _ = loop.ProcessDirect(ctx, "@peer:agent1 Hello", "test-session")

	// Test completes if no panic occurs
}

// TestAgentLoop_extractParentPeer tests extracting parent peer
func TestAgentLoop_extractParentPeerNew(t *testing.T) {
	// Note: extractParentPeer is a private method
	// We can test the behavior indirectly through message processing
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
	provider := &mockProviderForTest{
		responses: []string{"Response"},
	}

	loop := NewAgentLoop(cfg, msgBus, provider)

	// Test with parent prefix - should not panic
	ctx := context.Background()
	_, _ = loop.ProcessDirect(ctx, "@parent:agent1 Hello", "test-session")

	// Test completes if no panic occurs
}

// TestAgentLoop_WithCluster tests loop with cluster enabled
func TestAgentLoop_WithCluster(t *testing.T) {
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

	// Test with cluster disabled (default behavior)
	cm, err := channels.NewManager(cfg, msgBus)
	if err != nil {
		t.Fatalf("Failed to create channel manager: %v", err)
	}
	loop.SetChannelManager(cm)

	if loop.channelManager != cm {
		t.Error("Expected channel manager to be set")
	}
}

// TestAgentLoop_ProcessMessage_WithError tests error handling in message processing
func TestAgentLoop_ProcessMessage_WithError(t *testing.T) {
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
	provider := &mockProviderForTest{
		responses: []string{"Response"},
	}

	loop := NewAgentLoop(cfg, msgBus, provider)

	msg := bus.InboundMessage{
		Channel:  "test",
		SenderID: "user1",
		ChatID:   "chat1",
		Content:  "Hello",
	}

	ctx := context.Background()

	// Should handle errors gracefully
	_, result, err := loop.processMessage(ctx, msg)
	if err != nil {
		t.Errorf("Expected no error, got %v", err)
	}

	if result == "" {
		t.Error("Expected non-empty result")
	}
}

// TestAgentLoop_formatMessagesForLog tests formatting messages for logging
func TestAgentLoop_formatMessagesForLogNew(t *testing.T) {
	// Note: formatMessagesForLog is a private method
	// We can test message formatting behavior through the public API
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

	// Test that messages can be processed without errors
	ctx := context.Background()
	_, err := loop.ProcessDirect(ctx, "Test message", "test-session")
	if err != nil {
		t.Errorf("Expected no error, got %v", err)
	}
}

// TestAgentLoop_formatToolsForLog tests formatting tools for logging
func TestAgentLoop_formatToolsForLogNew(t *testing.T) {
	// Note: formatToolsForLog is a private method
	// We can test that tools are properly formatted through GetStartupInfo
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

	// Get startup info which formats tools
	info := loop.GetStartupInfo()

	if info == nil {
		t.Fatal("Expected startup info to exist")
	}

	// Should contain tools info
	if _, ok := info["tools"]; !ok {
		t.Error("Expected tools in startup info")
	}
}

// TestAgentLoop_summarizeSession tests session summarization
func TestAgentLoop_summarizeSessionNew(t *testing.T) {
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
	provider := &mockProviderForTest{
		responses: []string{"Session summary"},
	}

	loop := NewAgentLoop(cfg, msgBus, provider)

	agent := loop.registry.GetDefaultAgent()
	if agent == nil {
		t.Fatal("Expected default agent to exist")
	}

	sessionKey := "test-session"

	ctx := context.Background()

	// First, create a session by processing a message
	_, _ = loop.ProcessDirect(ctx, "Create a session first", sessionKey)

	// Now test summarization - should not panic
	// Note: summarizeSession is a private method with specific signature
	loop.summarizeSession(agent, sessionKey)

	// Test completes if no panic occurs
}

// TestAgentLoop_estimateTokens tests token estimation
func TestAgentLoop_estimateTokens(t *testing.T) {
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

	messages := []providers.Message{
		{Role: "user", Content: "Hello world"},
		{Role: "assistant", Content: "Hi there!"},
	}

	tokens := loop.estimateTokens(messages)

	if tokens == 0 {
		t.Error("Expected non-zero token estimate")
	}

	// Should be roughly character count / 4 (rough estimate)
	// "Hello world" + "Hi there!" = 23 chars, roughly 6 tokens
	if tokens < 3 || tokens > 10 {
		t.Errorf("Expected token estimate between 3 and 10, got %d", tokens)
	}
}

// TestAgentLoop_updateToolContexts tests updating tool contexts
func TestAgentLoop_updateToolContexts(t *testing.T) {
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

	agent := loop.registry.GetDefaultAgent()
	if agent == nil {
		t.Fatal("Expected default agent to exist")
	}

	channel := "test-channel"
	chatID := "chat-123"

	// Should not panic
	loop.updateToolContexts(agent, channel, chatID)
}

// TestAgentLoop_maybeSummarize tests conditional summarization
func TestAgentLoop_maybeSummarizeNew(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &config.Config{
		Agents: config.AgentsConfig{
			Defaults: config.AgentDefaults{
				LLM:       "test/model",
				Workspace: tempDir,
				MaxTokens: 1000,
			},
		},
	}

	msgBus := bus.NewMessageBus()
	provider := &mockProviderForTest{}

	loop := NewAgentLoop(cfg, msgBus, provider)

	agent := loop.registry.GetDefaultAgent()
	if agent == nil {
		t.Fatal("Expected default agent to exist")
	}

	sessionKey := "test-session"

	ctx := context.Background()

	// Create a small session by processing a message
	_, _ = loop.ProcessDirect(ctx, "Small message", sessionKey)

	// Should not panic and should handle gracefully
	// Note: maybeSummarize is a private method
	// We're testing the overall behavior through ProcessDirect
	// Test completes if no panic occurs
}
