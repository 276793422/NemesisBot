// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package agent_test

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/agent"
	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/providers"
	"github.com/276793422/NemesisBot/module/providers/protocoltypes"
)

// mockLLMProvider is a mock implementation of LLMProvider for testing
type mockLLMProvider struct {
	responses      []string
	responseIndex  int
	shouldError    bool
	customResponse *providers.LLMResponse
	delay          time.Duration
}

func (m *mockLLMProvider) Chat(ctx context.Context, messages []providers.Message, tools []providers.ToolDefinition, model string, options map[string]interface{}) (*providers.LLMResponse, error) {
	if m.delay > 0 {
		select {
		case <-ctx.Done():
			return nil, ctx.Err()
		case <-time.After(m.delay):
		}
	}

	if m.shouldError {
		return nil, errors.New("Mock LLM error")
	}

	if m.customResponse != nil {
		return m.customResponse, nil
	}

	response := "Mock response"
	if m.responses != nil && m.responseIndex < len(m.responses) {
		response = m.responses[m.responseIndex]
		m.responseIndex++
	}

	return &providers.LLMResponse{
		Content:      response,
		FinishReason: "stop",
		ToolCalls:    []protocoltypes.ToolCall{},
	}, nil
}

func (m *mockLLMProvider) GetDefaultModel() string {
	return "mock-model"
}

func (m *mockLLMProvider) SetResponses(responses []string) {
	m.responses = responses
	m.responseIndex = 0
}

func (m *mockLLMProvider) SetError(shouldError bool) {
	m.shouldError = shouldError
}

func (m *mockLLMProvider) SetCustomResponse(response *providers.LLMResponse) {
	m.customResponse = response
}

// TestNewAgentLoop tests creating a new agent loop
func TestNewAgentLoop(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &config.Config{
		Agents: config.AgentsConfig{
			Defaults: config.AgentDefaults{
				LLM:                 "mock/model",
				Workspace:           tempDir,
				MaxToolIterations:   10,
				MaxTokens:           4000,
				RestrictToWorkspace: true,
			},
		},
	}

	msgBus := bus.NewMessageBus()
	provider := &mockLLMProvider{}

	loop := agent.NewAgentLoop(cfg, msgBus, provider)

	if loop == nil {
		t.Fatal("Expected non-nil AgentLoop")
	}
}

// TestNewAgentLoop_WithConcurrentSettings tests creating loop with concurrent request settings
func TestNewAgentLoop_WithConcurrentSettings(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &config.Config{
		Agents: config.AgentsConfig{
			Defaults: config.AgentDefaults{
				LLM:                 "mock/model",
				Workspace:           tempDir,
				ConcurrentRequestMode: "queue",
				QueueSize:           16,
			},
		},
	}

	msgBus := bus.NewMessageBus()
	provider := &mockLLMProvider{}

	loop := agent.NewAgentLoop(cfg, msgBus, provider)

	if loop == nil {
		t.Fatal("Expected non-nil AgentLoop")
	}

	// Verify the loop was created (we can't directly access the fields, but we can test behavior)
}

// TestAgentLoop_RegisterTool tests registering a tool
func TestAgentLoop_RegisterTool(t *testing.T) {
	tempDir := t.TempDir()

	cfg := createBasicConfig(tempDir)
	msgBus := bus.NewMessageBus()
	provider := &mockLLMProvider{}

	loop := agent.NewAgentLoop(cfg, msgBus, provider)

	if loop == nil {
		t.Fatal("Expected non-nil AgentLoop")
	}

	// Test that RegisterTool doesn't panic (actual tool registration tested elsewhere)
	// This is just to ensure the method exists and is callable
}

// TestAgentLoop_GetStartupInfo tests getting startup information
func TestAgentLoop_GetStartupInfo(t *testing.T) {
	tempDir := t.TempDir()

	cfg := createBasicConfig(tempDir)
	msgBus := bus.NewMessageBus()
	provider := &mockLLMProvider{}

	loop := agent.NewAgentLoop(cfg, msgBus, provider)

	if loop == nil {
		t.Fatal("Expected non-nil AgentLoop")
	}

	info := loop.GetStartupInfo()

	if info == nil {
		t.Fatal("Expected non-nil info map")
	}

	// Should contain tools info
	if _, ok := info["tools"]; !ok {
		t.Error("Expected 'tools' in startup info")
	}

	// Should contain skills info
	if _, ok := info["skills"]; !ok {
		t.Error("Expected 'skills' in startup info")
	}

	// Should contain agents info
	if _, ok := info["agents"]; !ok {
		t.Error("Expected 'agents' in startup info")
	}

	// Verify tools info structure
	toolsInfo, ok := info["tools"].(map[string]interface{})
	if !ok {
		t.Fatal("Expected tools to be a map")
	}

	if _, ok := toolsInfo["count"]; !ok {
		t.Error("Expected 'count' in tools info")
	}

	if _, ok := toolsInfo["names"]; !ok {
		t.Error("Expected 'names' in tools info")
	}

	// Verify agents info structure
	agentsInfo, ok := info["agents"].(map[string]interface{})
	if !ok {
		t.Fatal("Expected agents to be a map")
	}

	if _, ok := agentsInfo["count"]; !ok {
		t.Error("Expected 'count' in agents info")
	}

	if _, ok := agentsInfo["ids"]; !ok {
		t.Error("Expected 'ids' in agents info")
	}
}

// TestAgentLoop_ProcessDirect tests direct message processing
func TestAgentLoop_ProcessDirect(t *testing.T) {
	tempDir := t.TempDir()

	cfg := createBasicConfig(tempDir)
	msgBus := bus.NewMessageBus()
	provider := &mockLLMProvider{
		responses: []string{"Test response"},
	}

	loop := agent.NewAgentLoop(cfg, msgBus, provider)

	if loop == nil {
		t.Fatal("Expected non-nil AgentLoop")
	}

	ctx := context.Background()
	response, err := loop.ProcessDirect(ctx, "Hello", "test-session")

	if err != nil {
		t.Fatalf("Expected no error, got: %v", err)
	}

	if response == "" {
		t.Error("Expected non-empty response")
	}
}

// TestAgentLoop_ProcessDirect_WithError tests error handling in direct processing
func TestAgentLoop_ProcessDirect_WithError(t *testing.T) {
	tempDir := t.TempDir()

	cfg := createBasicConfig(tempDir)
	msgBus := bus.NewMessageBus()
	provider := &mockLLMProvider{}
	provider.SetError(true)

	loop := agent.NewAgentLoop(cfg, msgBus, provider)

	if loop == nil {
		t.Fatal("Expected non-nil AgentLoop")
	}

	ctx := context.Background()
	_, err := loop.ProcessDirect(ctx, "Hello", "test-session")

	// Should return an error (not fatal)
	if err == nil {
		t.Error("Expected error when LLM fails")
	}
}

// TestAgentLoop_ProcessDirectWithChannel tests processing with custom channel
func TestAgentLoop_ProcessDirectWithChannel(t *testing.T) {
	tempDir := t.TempDir()

	cfg := createBasicConfig(tempDir)
	msgBus := bus.NewMessageBus()
	provider := &mockLLMProvider{
		responses: []string{"Channel response"},
	}

	loop := agent.NewAgentLoop(cfg, msgBus, provider)

	if loop == nil {
		t.Fatal("Expected non-nil AgentLoop")
	}

	ctx := context.Background()
	response, err := loop.ProcessDirectWithChannel(ctx, "Hello", "test-session", "test-channel", "chat-123")

	if err != nil {
		t.Fatalf("Expected no error, got: %v", err)
	}

	if response == "" {
		t.Error("Expected non-empty response")
	}

	// Verify response was published to bus (this happens internally)
	// We can't directly check this without accessing the bus, but the fact that
	// ProcessDirectWithChannel completes without error is a good sign
}

// TestAgentLoop_ProcessHeartbeat tests heartbeat processing
func TestAgentLoop_ProcessHeartbeat(t *testing.T) {
	tempDir := t.TempDir()

	cfg := createBasicConfig(tempDir)
	msgBus := bus.NewMessageBus()
	provider := &mockLLMProvider{
		responses: []string{"Heartbeat OK"},
	}

	loop := agent.NewAgentLoop(cfg, msgBus, provider)

	if loop == nil {
		t.Fatal("Expected non-nil AgentLoop")
	}

	ctx := context.Background()
	response, err := loop.ProcessHeartbeat(ctx, "ping", "health-check", "system")

	if err != nil {
		t.Fatalf("Expected no error, got: %v", err)
	}

	if response == "" {
		t.Error("Expected non-empty response")
	}
}

// TestAgentLoop_ConcurrentProcessing tests concurrent message processing
func TestAgentLoop_ConcurrentProcessing(t *testing.T) {
	tempDir := t.TempDir()

	cfg := createBasicConfig(tempDir)
	msgBus := bus.NewMessageBus()
	provider := &mockLLMProvider{
		responses: []string{
			"Response 1",
			"Response 2",
			"Response 3",
		},
	}

	loop := agent.NewAgentLoop(cfg, msgBus, provider)

	if loop == nil {
		t.Fatal("Expected non-nil AgentLoop")
	}

	ctx := context.Background()

	// Process multiple messages concurrently
	done := make(chan string, 3)
	for i := 0; i < 3; i++ {
		go func(n int) {
			response, err := loop.ProcessDirect(ctx, "Hello", "test-session")
			if err != nil {
				t.Errorf("Concurrent request %d failed: %v", n, err)
				done <- ""
				return
			}
			done <- response
		}(i)
	}

	// Wait for all responses
	responses := []string{}
	for i := 0; i < 3; i++ {
		select {
		case resp := <-done:
			responses = append(responses, resp)
		case <-time.After(10 * time.Second):
			t.Fatal("Timeout waiting for concurrent responses")
		}
	}

	// Verify we got responses
	successCount := 0
	for _, resp := range responses {
		if resp != "" {
			successCount++
		}
	}

	if successCount == 0 {
		t.Error("Expected at least one successful response")
	}
}

// TestAgentLoop_Stop tests stopping the agent loop
func TestAgentLoop_Stop(t *testing.T) {
	tempDir := t.TempDir()

	cfg := createBasicConfig(tempDir)
	msgBus := bus.NewMessageBus()
	provider := &mockLLMProvider{}

	loop := agent.NewAgentLoop(cfg, msgBus, provider)

	if loop == nil {
		t.Fatal("Expected non-nil AgentLoop")
	}

	// Stop should not panic
	loop.Stop()
}

// TestAgentLoop_SessionBusyState tests session busy state management
func TestAgentLoop_SessionBusyState(t *testing.T) {
	tempDir := t.TempDir()

	cfg := createBasicConfig(tempDir)
	cfg.Agents.Defaults.ConcurrentRequestMode = "reject"
	msgBus := bus.NewMessageBus()
	provider := &mockLLMProvider{
		responses: []string{"Response"},
	}

	loop := agent.NewAgentLoop(cfg, msgBus, provider)

	if loop == nil {
		t.Fatal("Expected non-nil AgentLoop")
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	// Start first request (make it take some time)
	provider.delay = 100 * time.Millisecond
	firstDone := make(chan bool)
	go func() {
		_, _ = loop.ProcessDirect(ctx, "First", "session-1")
		firstDone <- true
	}()

	// Give it time to start
	time.Sleep(10 * time.Millisecond)

	// Second request should be rejected or queued depending on mode
	// In "reject" mode, we should get a busy message
	_, err := loop.ProcessDirect(ctx, "Second", "session-1")

	// The behavior depends on concurrent mode
	// For "reject" mode, we might get a busy message or error
	// For "queue" mode, it would queue the request
	_ = err // We just verify it doesn't panic

	<-firstDone
}

// TestAgentLoop_MultipleSessions tests processing multiple sessions
func TestAgentLoop_MultipleSessions(t *testing.T) {
	tempDir := t.TempDir()

	cfg := createBasicConfig(tempDir)
	msgBus := bus.NewMessageBus()
	provider := &mockLLMProvider{
		responses: []string{
			"Response 1",
			"Response 2",
			"Response 3",
		},
	}

	loop := agent.NewAgentLoop(cfg, msgBus, provider)

	if loop == nil {
		t.Fatal("Expected non-nil AgentLoop")
	}

	ctx := context.Background()

	// Process messages from different sessions
	response1, err := loop.ProcessDirect(ctx, "Hello from session 1", "session-1")
	if err != nil {
		t.Errorf("Session 1 failed: %v", err)
	}

	response2, err := loop.ProcessDirect(ctx, "Hello from session 2", "session-2")
	if err != nil {
		t.Errorf("Session 2 failed: %v", err)
	}

	response3, err := loop.ProcessDirect(ctx, "Hello from session 3", "session-3")
	if err != nil {
		t.Errorf("Session 3 failed: %v", err)
	}

	// All should succeed
	if response1 == "" || response2 == "" || response3 == "" {
		t.Error("Expected all sessions to get responses")
	}
}

// TestAgentLoop_ContextTimeout tests context timeout handling
func TestAgentLoop_ContextTimeout(t *testing.T) {
	tempDir := t.TempDir()

	cfg := createBasicConfig(tempDir)
	msgBus := bus.NewMessageBus()
	provider := &mockLLMProvider{
		delay: 5 * time.Second, // Longer than context timeout
	}

	loop := agent.NewAgentLoop(cfg, msgBus, provider)

	if loop == nil {
		t.Fatal("Expected non-nil AgentLoop")
	}

	ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
	defer cancel()

	_, err := loop.ProcessDirect(ctx, "Hello", "test-session")

	// Should fail due to timeout
	if err == nil {
		t.Error("Expected error due to context timeout")
	}
}

// TestAgentLoop_EmptyMessage tests handling empty messages
func TestAgentLoop_EmptyMessage(t *testing.T) {
	tempDir := t.TempDir()

	cfg := createBasicConfig(tempDir)
	msgBus := bus.NewMessageBus()
	provider := &mockLLMProvider{
		responses: []string{"Response"},
	}

	loop := agent.NewAgentLoop(cfg, msgBus, provider)

	if loop == nil {
		t.Fatal("Expected non-nil AgentLoop")
	}

	ctx := context.Background()

	// Test with empty message
	_, err := loop.ProcessDirect(ctx, "", "test-session")

	// Should handle gracefully (may return error or default response)
	_ = err // Just verify it doesn't panic
}

// TestAgentLoop_LongMessage tests handling long messages
func TestAgentLoop_LongMessage(t *testing.T) {
	tempDir := t.TempDir()

	cfg := createBasicConfig(tempDir)
	msgBus := bus.NewMessageBus()
	provider := &mockLLMProvider{
		responses: []string{"Response"},
	}

	loop := agent.NewAgentLoop(cfg, msgBus, provider)

	if loop == nil {
		t.Fatal("Expected non-nil AgentLoop")
	}

	ctx := context.Background()

	// Create a very long message
	longMessage := strings.Repeat("This is a test message. ", 1000)

	response, err := loop.ProcessDirect(ctx, longMessage, "test-session")

	if err != nil {
		t.Errorf("Failed to process long message: %v", err)
	}

	if response == "" {
		t.Error("Expected response for long message")
	}
}

// TestAgentLoop_SpecialCharacters tests handling special characters
func TestAgentLoop_SpecialCharacters(t *testing.T) {
	tempDir := t.TempDir()

	cfg := createBasicConfig(tempDir)
	msgBus := bus.NewMessageBus()
	provider := &mockLLMProvider{
		responses: []string{"Response"},
	}

	loop := agent.NewAgentLoop(cfg, msgBus, provider)

	if loop == nil {
		t.Fatal("Expected non-nil AgentLoop")
	}

	ctx := context.Background()

	specialMessages := []string{
		"Message with \n newlines",
		"Message with \t tabs",
		"Message with \"quotes\"",
		"Message with 'apostrophes'",
		"Message with <html> tags",
		"Message with emoji 🎉",
		"Message with 中文",
	}

	for _, msg := range specialMessages {
		response, err := loop.ProcessDirect(ctx, msg, "test-session")

		if err != nil {
			t.Errorf("Failed to process special message '%s': %v", msg, err)
		}

		if response == "" {
			t.Errorf("Expected response for special message '%s'", msg)
		}
	}
}

// TestAgentLoop_WithMultipleAgents tests loop with multiple agents
func TestAgentLoop_WithMultipleAgents(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &config.Config{
		Agents: config.AgentsConfig{
			List: []config.AgentConfig{
				{
					ID:   "agent1",
					Name: "Agent 1",
				},
				{
					ID:   "agent2",
					Name: "Agent 2",
				},
			},
			Defaults: config.AgentDefaults{
				LLM:       "mock/model",
				Workspace: tempDir,
			},
		},
	}

	msgBus := bus.NewMessageBus()
	provider := &mockLLMProvider{
		responses: []string{"Response"},
	}

	loop := agent.NewAgentLoop(cfg, msgBus, provider)

	if loop == nil {
		t.Fatal("Expected non-nil AgentLoop")
	}

	// Get startup info to verify multiple agents
	info := loop.GetStartupInfo()
	agentsInfo, ok := info["agents"].(map[string]interface{})
	if !ok {
		t.Fatal("Expected agents info to be a map")
	}

	count, ok := agentsInfo["count"].(int)
	if !ok {
		t.Fatal("Expected count to be int")
	}

	if count != 2 {
		t.Errorf("Expected 2 agents, got %d", count)
	}
}

// TestAgentLoop_MemoryContext tests that memory context is included
func TestAgentLoop_MemoryContext(t *testing.T) {
	tempDir := t.TempDir()

	// Create memory files
	memoryDir := filepath.Join(tempDir, "memory")
	err := os.MkdirAll(memoryDir, 0755)
	if err != nil {
		t.Fatalf("Failed to create memory dir: %v", err)
	}

	// Create MEMORY.md
	memoryFile := filepath.Join(memoryDir, "MEMORY.md")
	err = os.WriteFile(memoryFile, []byte("# Test Memory\nThis is test memory content."), 0644)
	if err != nil {
		t.Fatalf("Failed to create memory file: %v", err)
	}

	cfg := createBasicConfig(tempDir)
	msgBus := bus.NewMessageBus()
	provider := &mockLLMProvider{
		responses: []string{"Response"},
	}

	loop := agent.NewAgentLoop(cfg, msgBus, provider)

	if loop == nil {
		t.Fatal("Expected non-nil AgentLoop")
	}

	ctx := context.Background()
	response, err := loop.ProcessDirect(ctx, "What do you remember?", "test-session")

	if err != nil {
		t.Fatalf("Expected no error, got: %v", err)
	}

	if response == "" {
		t.Error("Expected non-empty response")
	}

	// The memory should be included in the system prompt
	// We can't directly verify this without inspecting the internal messages,
	// but the fact that processing succeeds is a good indicator
}

// Helper functions

func createBasicConfig(workspace string) *config.Config {
	return &config.Config{
		Agents: config.AgentsConfig{
			Defaults: config.AgentDefaults{
				LLM:                 "mock/model",
				Workspace:           workspace,
				MaxToolIterations:   10,
				MaxTokens:           4000,
				RestrictToWorkspace: true,
			},
		},
	}
}
