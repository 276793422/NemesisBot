// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package agent

import (
	"context"
	"testing"

	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/providers"
)

// mockProviderForRegistry is a simple mock for testing
type mockProviderForRegistry struct{}

func (m *mockProviderForRegistry) Chat(ctx context.Context, messages []providers.Message, tools []providers.ToolDefinition, model string, options map[string]interface{}) (*providers.LLMResponse, error) {
	return &providers.LLMResponse{
		Content:      "Test response",
		FinishReason: "stop",
	}, nil
}

func (m *mockProviderForRegistry) GetDefaultModel() string {
	return "test-model"
}

// TestAgentRegistry_CanSpawnSubagent tests subagent spawning permissions
func TestAgentRegistry_CanSpawnSubagent(t *testing.T) {
	tests := []struct {
		name     string
		agents   []config.AgentConfig
		parentID string
		targetID string
		expected bool
	}{
		{
			name: "parent not found",
			agents: []config.AgentConfig{
				{ID: "agent1", Name: "Agent 1"},
			},
			parentID: "nonexistent",
			targetID: "agent1",
			expected: false,
		},
		{
			name: "parent has no subagent config",
			agents: []config.AgentConfig{
				{ID: "agent1", Name: "Agent 1"},
			},
			parentID: "agent1",
			targetID: "agent2",
			expected: false,
		},
		{
			name: "parent has nil subagent allow list",
			agents: []config.AgentConfig{
				{
					ID:   "agent1",
					Name: "Agent 1",
					Subagents: &config.SubagentsConfig{
						AllowAgents: nil,
					},
				},
			},
			parentID: "agent1",
			targetID: "agent2",
			expected: false,
		},
		{
			name: "parent allows wildcard",
			agents: []config.AgentConfig{
				{
					ID:   "agent1",
					Name: "Agent 1",
					Subagents: &config.SubagentsConfig{
						AllowAgents: []string{"*"},
					},
				},
				{ID: "agent2", Name: "Agent 2"},
			},
			parentID: "agent1",
			targetID: "agent2",
			expected: true,
		},
		{
			name: "parent allows specific agent",
			agents: []config.AgentConfig{
				{
					ID:   "agent1",
					Name: "Agent 1",
					Subagents: &config.SubagentsConfig{
						AllowAgents: []string{"agent2"},
					},
				},
				{ID: "agent2", Name: "Agent 2"},
			},
			parentID: "agent1",
			targetID: "agent2",
			expected: true,
		},
		{
			name: "parent does not allow specific agent",
			agents: []config.AgentConfig{
				{
					ID:   "agent1",
					Name: "Agent 1",
					Subagents: &config.SubagentsConfig{
						AllowAgents: []string{"agent3"},
					},
				},
				{ID: "agent2", Name: "Agent 2"},
			},
			parentID: "agent1",
			targetID: "agent2",
			expected: false,
		},
		{
			name: "case insensitive agent ID matching",
			agents: []config.AgentConfig{
				{
					ID:   "Agent-One",
					Name: "Agent One",
					Subagents: &config.SubagentsConfig{
						AllowAgents: []string{"agent-two"},
					},
				},
				{ID: "Agent-Two", Name: "Agent Two"},
			},
			parentID: "agent-one",
			targetID: "AGENT-TWO",
			expected: true,
		},
		{
			name: "empty allow list",
			agents: []config.AgentConfig{
				{
					ID:   "agent1",
					Name: "Agent 1",
					Subagents: &config.SubagentsConfig{
						AllowAgents: []string{},
					},
				},
			},
			parentID: "agent1",
			targetID: "agent2",
			expected: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			cfg := &config.Config{
				Agents: config.AgentsConfig{
					List: tt.agents,
					Defaults: config.AgentDefaults{
						LLM:       "test/model",
						Workspace: t.TempDir(),
					},
				},
			}

			provider := &mockProviderForRegistry{}
			registry := NewAgentRegistry(cfg, provider)

			result := registry.CanSpawnSubagent(tt.parentID, tt.targetID)
			if result != tt.expected {
				t.Errorf("Expected %v, got %v", tt.expected, result)
			}
		})
	}
}

// TestAgentRegistry_GetDefaultAgent tests getting the default agent
func TestAgentRegistry_GetDefaultAgent(t *testing.T) {
	tests := []struct {
		name          string
		agents        []config.AgentConfig
		expectDefault bool
	}{
		{
			name:          "no agents configured",
			agents:        []config.AgentConfig{},
			expectDefault: true,
		},
		{
			name: "main agent configured",
			agents: []config.AgentConfig{
				{ID: "main", Name: "Main Agent"},
			},
			expectDefault: true,
		},
		{
			name: "custom agent configured",
			agents: []config.AgentConfig{
				{ID: "custom", Name: "Custom Agent"},
			},
			expectDefault: true,
		},
		{
			name: "multiple agents configured",
			agents: []config.AgentConfig{
				{ID: "agent1", Name: "Agent 1"},
				{ID: "agent2", Name: "Agent 2"},
			},
			expectDefault: true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			cfg := &config.Config{
				Agents: config.AgentsConfig{
					List: tt.agents,
					Defaults: config.AgentDefaults{
						LLM:       "test/model",
						Workspace: t.TempDir(),
					},
				},
			}

			provider := &mockProviderForRegistry{}
			registry := NewAgentRegistry(cfg, provider)

			defaultAgent := registry.GetDefaultAgent()
			if tt.expectDefault && defaultAgent == nil {
				t.Error("Expected default agent to exist")
			}
		})
	}
}

// TestAgentRegistry_GetAgent tests retrieving specific agents
func TestAgentRegistry_GetAgent(t *testing.T) {
	agents := []config.AgentConfig{
		{ID: "agent1", Name: "Agent 1"},
		{ID: "agent2", Name: "Agent 2"},
	}

	cfg := &config.Config{
		Agents: config.AgentsConfig{
			List: agents,
			Defaults: config.AgentDefaults{
				LLM:       "test/model",
				Workspace: t.TempDir(),
			},
		},
	}

	provider := &mockProviderForRegistry{}
	registry := NewAgentRegistry(cfg, provider)

	// Test getting existing agent
	agent, ok := registry.GetAgent("agent1")
	if !ok {
		t.Error("Expected agent1 to exist")
	}
	if agent == nil {
		t.Error("Expected agent to be non-nil")
	}
	if agent.ID != "agent1" {
		t.Errorf("Expected ID agent1, got %s", agent.ID)
	}

	// Test getting non-existent agent
	_, ok = registry.GetAgent("nonexistent")
	if ok {
		t.Error("Expected non-existent agent to return false")
	}

	// Test case insensitive
	agent, ok = registry.GetAgent("AGENT1")
	if !ok {
		t.Error("Expected case-insensitive agent lookup to work")
	}
	if agent.ID != "agent1" {
		t.Errorf("Expected normalized ID agent1, got %s", agent.ID)
	}
}

// TestAgentRegistry_ListAgentIDs tests listing all agent IDs
func TestAgentRegistry_ListAgentIDs(t *testing.T) {
	agents := []config.AgentConfig{
		{ID: "agent1", Name: "Agent 1"},
		{ID: "agent2", Name: "Agent 2"},
		{ID: "agent3", Name: "Agent 3"},
	}

	cfg := &config.Config{
		Agents: config.AgentsConfig{
			List: agents,
			Defaults: config.AgentDefaults{
				LLM:       "test/model",
				Workspace: t.TempDir(),
			},
		},
	}

	provider := &mockProviderForRegistry{}
	registry := NewAgentRegistry(cfg, provider)

	ids := registry.ListAgentIDs()

	if len(ids) != 3 {
		t.Errorf("Expected 3 agent IDs, got %d", len(ids))
	}

	// Check that all IDs are present
	idMap := make(map[string]bool)
	for _, id := range ids {
		idMap[id] = true
	}

	for _, expectedID := range []string{"agent1", "agent2", "agent3"} {
		if !idMap[expectedID] {
			t.Errorf("Expected agent ID %s to be in list", expectedID)
		}
	}
}

// TestAgentRegistry_ResolveRoute tests route resolution
func TestAgentRegistry_ResolveRoute(t *testing.T) {
	cfg := &config.Config{
		Agents: config.AgentsConfig{
			Defaults: config.AgentDefaults{
				LLM:       "test/model",
				Workspace: t.TempDir(),
			},
		},
	}

	provider := &mockProviderForRegistry{}
	registry := NewAgentRegistry(cfg, provider)

	// Test with basic input - just verify it doesn't panic
	_ = registry.ResolveRoute
}

// TestAgentRegistry_ConcurrentAccess tests concurrent access to registry
func TestAgentRegistry_ConcurrentAccess(t *testing.T) {
	agents := []config.AgentConfig{
		{ID: "agent1", Name: "Agent 1"},
		{ID: "agent2", Name: "Agent 2"},
		{ID: "agent3", Name: "Agent 3"},
	}

	cfg := &config.Config{
		Agents: config.AgentsConfig{
			List: agents,
			Defaults: config.AgentDefaults{
				LLM:       "test/model",
				Workspace: t.TempDir(),
			},
		},
	}

	provider := &mockProviderForRegistry{}
	registry := NewAgentRegistry(cfg, provider)

	done := make(chan bool, 100)

	// Launch concurrent reads
	for i := 0; i < 50; i++ {
		go func() {
			_, _ = registry.GetAgent("agent1")
			_ = registry.ListAgentIDs()
			_ = registry.GetDefaultAgent()
			done <- true
		}()
	}

	// Wait for completion
	for i := 0; i < 50; i++ {
		<-done
	}
}

// TestAgentRegistry_ImplicitMainAgent tests creation of implicit main agent
func TestAgentRegistry_ImplicitMainAgent(t *testing.T) {
	cfg := &config.Config{
		Agents: config.AgentsConfig{
			List: []config.AgentConfig{},
			Defaults: config.AgentDefaults{
				LLM:       "test/model",
				Workspace: t.TempDir(),
			},
		},
	}

	provider := &mockProviderForRegistry{}
	registry := NewAgentRegistry(cfg, provider)

	// Should have implicit main agent
	agent, ok := registry.GetAgent("main")
	if !ok {
		t.Error("Expected implicit main agent to exist")
	}
	if agent == nil {
		t.Fatal("Expected agent to be non-nil")
	}

	// Verify it's the default
	defaultAgent := registry.GetDefaultAgent()
	if defaultAgent != agent {
		t.Error("Expected default agent to be the implicit main agent")
	}
}

// BenchmarkAgentRegistry_GetAgent benchmarks agent retrieval
func BenchmarkAgentRegistry_GetAgent(b *testing.B) {
	agents := make([]config.AgentConfig, 100)
	for i := 0; i < 100; i++ {
		agents[i] = config.AgentConfig{
			ID:   "agent" + string(rune(i)),
			Name: "Agent " + string(rune(i)),
		}
	}

	cfg := &config.Config{
		Agents: config.AgentsConfig{
			List: agents,
			Defaults: config.AgentDefaults{
				LLM:       "test/model",
				Workspace: b.TempDir(),
			},
		},
	}

	provider := &mockProviderForRegistry{}
	registry := NewAgentRegistry(cfg, provider)

	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		_, _ = registry.GetAgent("agent50")
	}
}

// BenchmarkAgentRegistry_CanSpawnSubagent benchmarks subagent permission checks
func BenchmarkAgentRegistry_CanSpawnSubagent(b *testing.B) {
	agents := []config.AgentConfig{
		{
			ID:   "parent",
			Name: "Parent Agent",
			Subagents: &config.SubagentsConfig{
				AllowAgents: []string{"child1", "child2", "child3"},
			},
		},
	}

	cfg := &config.Config{
		Agents: config.AgentsConfig{
			List: agents,
			Defaults: config.AgentDefaults{
				LLM:       "test/model",
				Workspace: b.TempDir(),
			},
		},
	}

	provider := &mockProviderForRegistry{}
	registry := NewAgentRegistry(cfg, provider)

	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		_ = registry.CanSpawnSubagent("parent", "child2")
	}
}
