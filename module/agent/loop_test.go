// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package agent

import (
	"context"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/providers"
	"github.com/276793422/NemesisBot/module/providers/protocoltypes"
)

// Mock provider for testing
type mockProviderForTest struct {
	responses []string
	index     int
}

func (m *mockProviderForTest) Chat(ctx context.Context, messages []providers.Message, tools []providers.ToolDefinition, model string, options map[string]interface{}) (*providers.LLMResponse, error) {
	response := "Test response"
	if m.responses != nil && m.index < len(m.responses) {
		response = m.responses[m.index]
		m.index++
	}

	return &providers.LLMResponse{
		Content:      response,
		FinishReason: "stop",
		ToolCalls:    []protocoltypes.ToolCall{},
		Usage: &providers.UsageInfo{
			PromptTokens:     100,
			CompletionTokens: 50,
			TotalTokens:      150,
		},
	}, nil
}

func (m *mockProviderForTest) GetDefaultModel() string {
	return "test-model"
}

// TestNewAgentLoop tests creating a new agent loop
func TestNewAgentLoop(t *testing.T) {
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

	if loop == nil {
		t.Fatal("Expected non-nil AgentLoop")
	}
}

// TestAgentLoop_ProcessDirect tests direct message processing
func TestAgentLoop_ProcessDirect(t *testing.T) {
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

	ctx := context.Background()
	response, err := loop.ProcessDirect(ctx, "Hello", "test-session")

	if err != nil {
		t.Fatalf("Expected no error, got: %v", err)
	}

	if response == "" {
		t.Error("Expected non-empty response")
	}
}

// TestAgentLoop_ProcessDirectWithChannel tests processing with custom channel
func TestAgentLoop_ProcessDirectWithChannel(t *testing.T) {
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
		responses: []string{"Channel response"},
	}

	loop := NewAgentLoop(cfg, msgBus, provider)

	ctx := context.Background()
	response, err := loop.ProcessDirectWithChannel(ctx, "Hello", "test-session", "test-channel", "chat-123")

	if err != nil {
		t.Fatalf("Expected no error, got: %v", err)
	}

	if response == "" {
		t.Error("Expected non-empty response")
	}
}

// TestAgentLoop_ProcessHeartbeat tests heartbeat processing
func TestAgentLoop_ProcessHeartbeat(t *testing.T) {
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
		responses: []string{"Heartbeat OK"},
	}

	loop := NewAgentLoop(cfg, msgBus, provider)

	ctx := context.Background()
	response, err := loop.ProcessHeartbeat(ctx, "ping", "health-check", "system")

	if err != nil {
		t.Fatalf("Expected no error, got: %v", err)
	}

	if response == "" {
		t.Error("Expected non-empty response")
	}
}

// TestAgentLoop_GetStartupInfo tests getting startup information
func TestAgentLoop_GetStartupInfo(t *testing.T) {
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
}

// TestAgentLoop_ConcurrentProcessing tests concurrent message processing
func TestAgentLoop_ConcurrentProcessing(t *testing.T) {
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
		responses: []string{
			"Response 1",
			"Response 2",
			"Response 3",
		},
	}

	loop := NewAgentLoop(cfg, msgBus, provider)

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

	// Stop should not panic
	loop.Stop()
}

// TestAgentLoop_MultipleSessions tests processing multiple sessions
func TestAgentLoop_MultipleSessions(t *testing.T) {
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
		responses: []string{
			"Response 1",
			"Response 2",
			"Response 3",
		},
	}

	loop := NewAgentLoop(cfg, msgBus, provider)

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

	cfg := &config.Config{
		Agents: config.AgentsConfig{
			Defaults: config.AgentDefaults{
				LLM:       "test/model",
				Workspace: tempDir,
			},
		},
	}

	msgBus := bus.NewMessageBus()

	// Create a provider that delays
	provider := &mockProviderForTest{}

	loop := NewAgentLoop(cfg, msgBus, provider)

	ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
	defer cancel()

	// This should complete within timeout
	_, err := loop.ProcessDirect(ctx, "Hello", "test-session")

	// With fast mock provider, should not timeout
	if err != nil && ctx.Err() == context.DeadlineExceeded {
		t.Error("Expected request to complete within timeout")
	}
}
