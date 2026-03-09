// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package agent_test

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/276793422/NemesisBot/module/agent"
	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/providers"
)

// mockProvider is a mock implementation of LLMProvider for testing
type mockProvider struct {
	defaultModel string
}

func (m *mockProvider) Chat(ctx context.Context, messages []providers.Message, tools []providers.ToolDefinition, model string, options map[string]interface{}) (*providers.LLMResponse, error) {
	return &providers.LLMResponse{
		Content:      "Mock response",
		FinishReason: "stop",
	}, nil
}

func (m *mockProvider) GetDefaultModel() string {
	if m.defaultModel != "" {
		return m.defaultModel
	}
	return "mock-model"
}

// TestNewAgentInstance tests creating a new agent instance
func TestNewAgentInstance(t *testing.T) {
	tempDir := t.TempDir()

	agentCfg := &config.AgentConfig{
		ID:   "test-agent",
		Name: "Test Agent",
	}

	defaults := &config.AgentDefaults{
		LLM:                 "mock/mock-model",
		Workspace:           tempDir,
		MaxToolIterations:   10,
		MaxTokens:           4000,
		RestrictToWorkspace: true,
	}

	cfg := &config.Config{
		Security: &config.SecurityFlagConfig{
			Enabled: false,
		},
	}

	provider := &mockProvider{}

	instance := agent.NewAgentInstance(agentCfg, defaults, cfg, provider)

	if instance == nil {
		t.Fatal("Expected non-nil AgentInstance")
	}

	// Verify basic fields
	if instance.ID != "test-agent" {
		t.Errorf("Expected ID 'test-agent', got '%s'", instance.ID)
	}

	if instance.Name != "Test Agent" {
		t.Errorf("Expected Name 'Test Agent', got '%s'", instance.Name)
	}

	// Note: non-default agents get workspace under .nemesisbot/workspace-{agentID}
	// So we just check it exists and contains the agent ID
	if instance.Workspace == "" {
		t.Error("Expected Workspace to be set")
	}

	if instance.Model != "mock/mock-model" {
		t.Errorf("Expected Model 'mock/mock-model', got '%s'", instance.Model)
	}

	if instance.MaxIterations != 10 {
		t.Errorf("Expected MaxIterations 10, got %d", instance.MaxIterations)
	}

	if instance.ContextWindow != 4000 {
		t.Errorf("Expected ContextWindow 4000, got %d", instance.ContextWindow)
	}

	// Verify sub-components are initialized
	if instance.Sessions == nil {
		t.Error("Expected Sessions to be initialized")
	}

	if instance.ContextBuilder == nil {
		t.Error("Expected ContextBuilder to be initialized")
	}

	if instance.Tools == nil {
		t.Error("Expected Tools to be initialized")
	}

	if instance.Provider == nil {
		t.Error("Expected Provider to be set")
	}
}

// TestNewAgentInstance_DefaultAgent tests creating default agent
func TestNewAgentInstance_DefaultAgent(t *testing.T) {
	tempDir := t.TempDir()

	defaults := &config.AgentDefaults{
		LLM:                 "mock/default-model",
		Workspace:           tempDir,
		MaxToolIterations:   20,
		MaxTokens:           8000,
		RestrictToWorkspace: false,
	}

	cfg := &config.Config{}

	provider := &mockProvider{}

	// Create instance with nil agent config (default agent)
	instance := agent.NewAgentInstance(nil, defaults, cfg, provider)

	if instance == nil {
		t.Fatal("Expected non-nil AgentInstance")
	}

	// Default agent should have ID "main"
	if instance.ID != "main" {
		t.Errorf("Expected default ID 'main', got '%s'", instance.ID)
	}

	// Should use defaults
	if instance.Workspace != tempDir {
		t.Errorf("Expected Workspace '%s', got '%s'", tempDir, instance.Workspace)
	}
}

// TestNewAgentInstance_WithFallbacks tests creating agent with fallback models
func TestNewAgentInstance_WithFallbacks(t *testing.T) {
	tempDir := t.TempDir()

	agentCfg := &config.AgentConfig{
		ID: "fallback-agent",
		Model: &config.AgentModelConfig{
			Primary: "mock/primary-model",
			Fallbacks: []string{
				"mock/fallback1",
				"mock/fallback2",
			},
		},
	}

	defaults := &config.AgentDefaults{
		LLM:       "mock/default-model",
		Workspace: tempDir,
	}

	cfg := &config.Config{}

	provider := &mockProvider{}

	instance := agent.NewAgentInstance(agentCfg, defaults, cfg, provider)

	if instance == nil {
		t.Fatal("Expected non-nil AgentInstance")
	}

	// Should use agent's primary model
	if instance.Model != "mock/primary-model" {
		t.Errorf("Expected Model 'mock/primary-model', got '%s'", instance.Model)
	}

	// Should have fallbacks
	if len(instance.Fallbacks) != 2 {
		t.Errorf("Expected 2 fallbacks, got %d", len(instance.Fallbacks))
	}

	if instance.Fallbacks[0] != "mock/fallback1" {
		t.Errorf("Expected fallback 'mock/fallback1', got '%s'", instance.Fallbacks[0])
	}
}

// TestNewAgentInstance_WithSecurity tests creating agent with security enabled
func TestNewAgentInstance_WithSecurity(t *testing.T) {
	tempDir := t.TempDir()

	// Create security config directory
	securityDir := filepath.Join(tempDir, "config")
	err := os.MkdirAll(securityDir, 0755)
	if err != nil {
		t.Fatalf("Failed to create security config dir: %v", err)
	}

	// Create a minimal security config
	securityConfigPath := filepath.Join(securityDir, "config.security.json")
	securityConfig := `{
		"version": "1.0",
		"default_action": "deny",
		"rules": []
	}`
	err = os.WriteFile(securityConfigPath, []byte(securityConfig), 0644)
	if err != nil {
		t.Fatalf("Failed to write security config: %v", err)
	}

	agentCfg := &config.AgentConfig{
		ID: "secure-agent",
	}

	defaults := &config.AgentDefaults{
		LLM:                 "mock/model",
		Workspace:           tempDir,
		RestrictToWorkspace: true,
	}

	cfg := &config.Config{
		Security: &config.SecurityFlagConfig{
			Enabled: true,
		},
	}

	provider := &mockProvider{}

	instance := agent.NewAgentInstance(agentCfg, defaults, cfg, provider)

	if instance == nil {
		t.Fatal("Expected non-nil AgentInstance")
	}

	// Plugin manager should be initialized
	if instance.PluginMgr == nil {
		t.Error("Expected PluginMgr to be initialized with security plugin")
	}
}

// TestNewAgentInstance_CustomWorkspace tests creating agent with custom workspace
func TestNewAgentInstance_CustomWorkspace(t *testing.T) {
	tempDir := t.TempDir()
	customWorkspace := t.TempDir()

	agentCfg := &config.AgentConfig{
		ID:        "custom-workspace-agent",
		Workspace: customWorkspace,
	}

	defaults := &config.AgentDefaults{
		LLM:       "mock/model",
		Workspace: tempDir, // Default workspace
	}

	cfg := &config.Config{}

	provider := &mockProvider{}

	instance := agent.NewAgentInstance(agentCfg, defaults, cfg, provider)

	if instance == nil {
		t.Fatal("Expected non-nil AgentInstance")
	}

	// Should use custom workspace from agent config
	if instance.Workspace != customWorkspace {
		t.Errorf("Expected custom workspace '%s', got '%s'", customWorkspace, instance.Workspace)
	}

	// Custom workspace should exist
	if _, err := os.Stat(customWorkspace); os.IsNotExist(err) {
		t.Error("Custom workspace should be created")
	}
}

// TestNewAgentInstance_WithSubagents tests creating agent with subagent configuration
func TestNewAgentInstance_WithSubagents(t *testing.T) {
	tempDir := t.TempDir()

	agentCfg := &config.AgentConfig{
		ID: "parent-agent",
		Subagents: &config.SubagentsConfig{
			AllowAgents: []string{"subagent1", "subagent2"},
		},
	}

	defaults := &config.AgentDefaults{
		LLM:       "mock/model",
		Workspace: tempDir,
	}

	cfg := &config.Config{}

	provider := &mockProvider{}

	instance := agent.NewAgentInstance(agentCfg, defaults, cfg, provider)

	if instance == nil {
		t.Fatal("Expected non-nil AgentInstance")
	}

	if instance.Subagents == nil {
		t.Fatal("Expected Subagents to be set")
	}

	if len(instance.Subagents.AllowAgents) != 2 {
		t.Errorf("Expected 2 allowed subagents, got %d", len(instance.Subagents.AllowAgents))
	}
}

// TestNewAgentInstance_WithSkillsFilter tests creating agent with skills filter
func TestNewAgentInstance_WithSkillsFilter(t *testing.T) {
	tempDir := t.TempDir()

	agentCfg := &config.AgentConfig{
		ID:     "skills-agent",
		Skills: []string{"skill1", "skill2"},
	}

	defaults := &config.AgentDefaults{
		LLM:       "mock/model",
		Workspace: tempDir,
	}

	cfg := &config.Config{}

	provider := &mockProvider{}

	instance := agent.NewAgentInstance(agentCfg, defaults, cfg, provider)

	if instance == nil {
		t.Fatal("Expected non-nil AgentInstance")
	}

	if len(instance.SkillsFilter) != 2 {
		t.Errorf("Expected 2 skills in filter, got %d", len(instance.SkillsFilter))
	}

	if instance.SkillsFilter[0] != "skill1" {
		t.Errorf("Expected skill 'skill1', got '%s'", instance.SkillsFilter[0])
	}
}

// TestAgentInstance_Candidates tests fallback candidates are resolved
func TestAgentInstance_Candidates(t *testing.T) {
	tempDir := t.TempDir()

	agentCfg := &config.AgentConfig{
		ID: "candidate-agent",
		Model: &config.AgentModelConfig{
			Primary: "mock/primary",
			Fallbacks: []string{
				"mock/fallback1",
				"mock/fallback2",
			},
		},
	}

	defaults := &config.AgentDefaults{
		LLM:       "mock/default",
		Workspace: tempDir,
	}

	cfg := &config.Config{}

	provider := &mockProvider{}

	instance := agent.NewAgentInstance(agentCfg, defaults, cfg, provider)

	if instance == nil {
		t.Fatal("Expected non-nil AgentInstance")
	}

	// Candidates should be resolved
	if instance.Candidates == nil {
		t.Error("Expected Candidates to be initialized")
	} else if len(instance.Candidates) == 0 {
		t.Error("Expected at least one candidate")
	}

	// First candidate should have the model name (without provider prefix in the Model field)
	// The FallbackCandidate.Model field stores just the model name
	if len(instance.Candidates) > 0 && instance.Candidates[0].Model == "" {
		t.Error("Expected first candidate to have a model name")
	}
}

// TestNewAgentRegistry tests creating agent registry
func TestNewAgentRegistry(t *testing.T) {
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

	provider := &mockProvider{}

	registry := agent.NewAgentRegistry(cfg, provider)

	if registry == nil {
		t.Fatal("Expected non-nil AgentRegistry")
	}

	// Should have registered agents
	agentIDs := registry.ListAgentIDs()
	if len(agentIDs) != 2 {
		t.Errorf("Expected 2 agents, got %d", len(agentIDs))
	}
}

// TestNewAgentRegistry_NoAgents tests creating registry with no agents configured
func TestNewAgentRegistry_NoAgents(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &config.Config{
		Agents: config.AgentsConfig{
			List: []config.AgentConfig{}, // Empty list
			Defaults: config.AgentDefaults{
				LLM:       "mock/model",
				Workspace: tempDir,
			},
		},
	}

	provider := &mockProvider{}

	registry := agent.NewAgentRegistry(cfg, provider)

	if registry == nil {
		t.Fatal("Expected non-nil AgentRegistry")
	}

	// Should create implicit main agent
	agentIDs := registry.ListAgentIDs()
	if len(agentIDs) != 1 {
		t.Errorf("Expected 1 implicit agent, got %d", len(agentIDs))
	}

	if agentIDs[0] != "main" {
		t.Errorf("Expected implicit agent ID 'main', got '%s'", agentIDs[0])
	}
}

// TestAgentRegistry_GetAgent tests getting agent by ID
func TestAgentRegistry_GetAgent(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &config.Config{
		Agents: config.AgentsConfig{
			List: []config.AgentConfig{
				{
					ID:   "test-agent",
					Name: "Test Agent",
				},
			},
			Defaults: config.AgentDefaults{
				LLM:       "mock/model",
				Workspace: tempDir,
			},
		},
	}

	provider := &mockProvider{}

	registry := agent.NewAgentRegistry(cfg, provider)

	// Get existing agent
	agent, ok := registry.GetAgent("test-agent")
	if !ok {
		t.Error("Expected to find agent 'test-agent'")
	}
	if agent == nil {
		t.Fatal("Expected non-nil agent")
	}
	if agent.ID != "test-agent" {
		t.Errorf("Expected agent ID 'test-agent', got '%s'", agent.ID)
	}

	// Get non-existent agent
	_, ok = registry.GetAgent("non-existent")
	if ok {
		t.Error("Expected not to find non-existent agent")
	}
}

// TestAgentRegistry_GetDefaultAgent tests getting default agent
func TestAgentRegistry_GetDefaultAgent(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &config.Config{
		Agents: config.AgentsConfig{
			List: []config.AgentConfig{
				{
					ID:   "main",
					Name: "Main Agent",
				},
			},
			Defaults: config.AgentDefaults{
				LLM:       "mock/model",
				Workspace: tempDir,
			},
		},
	}

	provider := &mockProvider{}

	registry := agent.NewAgentRegistry(cfg, provider)

	agent := registry.GetDefaultAgent()
	if agent == nil {
		t.Fatal("Expected non-nil default agent")
	}

	if agent.ID != "main" {
		t.Errorf("Expected default agent ID 'main', got '%s'", agent.ID)
	}
}

// TestAgentRegistry_CanSpawnSubagent tests subagent spawning permissions
func TestAgentRegistry_CanSpawnSubagent(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &config.Config{
		Agents: config.AgentsConfig{
			List: []config.AgentConfig{
				{
					ID:   "parent",
					Name: "Parent Agent",
					Subagents: &config.SubagentsConfig{
						AllowAgents: []string{"child1", "child2"},
					},
				},
				{
					ID:   "child1",
					Name: "Child Agent 1",
				},
				{
					ID:   "child2",
					Name: "Child Agent 2",
				},
			},
			Defaults: config.AgentDefaults{
				LLM:       "mock/model",
				Workspace: tempDir,
			},
		},
	}

	provider := &mockProvider{}

	registry := agent.NewAgentRegistry(cfg, provider)

	// Test allowed subagent
	if !registry.CanSpawnSubagent("parent", "child1") {
		t.Error("Expected parent to be able to spawn child1")
	}

	// Test wildcard permission
	cfg.Agents.List[0].Subagents.AllowAgents = []string{"*"}
	registry = agent.NewAgentRegistry(cfg, provider)
	if !registry.CanSpawnSubagent("parent", "any-agent") {
		t.Error("Expected wildcard to allow any agent")
	}

	// Test non-allowed subagent
	cfg.Agents.List[0].Subagents.AllowAgents = []string{"child1"}
	registry = agent.NewAgentRegistry(cfg, provider)
	if registry.CanSpawnSubagent("parent", "child2") {
		t.Error("Expected parent not to be able to spawn child2")
	}

	// Test non-existent parent
	if registry.CanSpawnSubagent("non-existent", "child1") {
		t.Error("Expected non-existent parent to not be able to spawn")
	}
}

// TestAgentRegistry_ConcurrentAccess tests concurrent access to registry
func TestAgentRegistry_ConcurrentAccess(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &config.Config{
		Agents: config.AgentsConfig{
			List: []config.AgentConfig{
				{ID: "agent1"},
				{ID: "agent2"},
				{ID: "agent3"},
			},
			Defaults: config.AgentDefaults{
				LLM:       "mock/model",
				Workspace: tempDir,
			},
		},
	}

	provider := &mockProvider{}

	registry := agent.NewAgentRegistry(cfg, provider)

	done := make(chan bool)

	// Concurrent reads
	for i := 0; i < 10; i++ {
		go func() {
			_, _ = registry.GetAgent("agent1")
			_ = registry.ListAgentIDs()
			done <- true
		}()
	}

	// Wait for all goroutines
	for i := 0; i < 10; i++ {
		<-done
	}

	// Verify registry still works
	agentIDs := registry.ListAgentIDs()
	if len(agentIDs) != 3 {
		t.Errorf("Expected 3 agents after concurrent access, got %d", len(agentIDs))
	}
}

// TestNewAgentInstance_WithMaxIterations tests creating agent with custom max iterations
func TestNewAgentInstance_WithMaxIterations(t *testing.T) {
	tempDir := t.TempDir()

	agentCfg := &config.AgentConfig{
		ID: "iterations-agent",
	}

	defaults := &config.AgentDefaults{
		LLM:             "mock/model",
		Workspace:       tempDir,
		MaxToolIterations: 30,
	}

	cfg := &config.Config{}

	provider := &mockProvider{}

	instance := agent.NewAgentInstance(agentCfg, defaults, cfg, provider)

	if instance == nil {
		t.Fatal("Expected non-nil AgentInstance")
	}

	if instance.MaxIterations != 30 {
		t.Errorf("Expected MaxIterations 30, got %d", instance.MaxIterations)
	}
}

// TestNewAgentInstance_ZeroMaxIterations tests that zero max iterations defaults to 20
func TestNewAgentInstance_ZeroMaxIterations(t *testing.T) {
	tempDir := t.TempDir()

	agentCfg := &config.AgentConfig{
		ID: "zero-iterations-agent",
	}

	defaults := &config.AgentDefaults{
		LLM:             "mock/model",
		Workspace:       tempDir,
		MaxToolIterations: 0, // Should default to 20
	}

	cfg := &config.Config{}

	provider := &mockProvider{}

	instance := agent.NewAgentInstance(agentCfg, defaults, cfg, provider)

	if instance == nil {
		t.Fatal("Expected non-nil AgentInstance")
	}

	if instance.MaxIterations != 20 {
		t.Errorf("Expected MaxIterations to default to 20, got %d", instance.MaxIterations)
	}
}

// TestNewAgentInstance_WithNoSecurity tests creating agent without security
func TestNewAgentInstance_WithNoSecurity(t *testing.T) {
	tempDir := t.TempDir()

	agentCfg := &config.AgentConfig{
		ID: "no-security-agent",
	}

	defaults := &config.AgentDefaults{
		LLM:       "mock/model",
		Workspace: tempDir,
	}

	cfg := &config.Config{
		Security: &config.SecurityFlagConfig{
			Enabled: false,
		},
	}

	provider := &mockProvider{}

	instance := agent.NewAgentInstance(agentCfg, defaults, cfg, provider)

	if instance == nil {
		t.Fatal("Expected non-nil AgentInstance")
	}

	// Plugin manager should still be initialized (for other plugins)
	if instance.PluginMgr == nil {
		t.Error("Expected PluginMgr to be initialized even without security")
	}
}

// TestNewAgentInstance_WithNilConfig tests creating agent with nil config
func TestNewAgentInstance_WithNilConfig(t *testing.T) {
	tempDir := t.TempDir()

	defaults := &config.AgentDefaults{
		LLM:       "mock/model",
		Workspace: tempDir,
	}

	cfg := &config.Config{} // Need non-nil config

	provider := &mockProvider{}

	// Create with nil agent config (but valid config)
	instance := agent.NewAgentInstance(nil, defaults, cfg, provider)

	if instance == nil {
		t.Fatal("Expected non-nil AgentInstance even with nil agent config")
	}

	// Should have default values
	if instance.ID == "" {
		t.Error("Expected ID to be set")
	}

	if instance.Model == "" {
		t.Error("Expected Model to be set")
	}
}

// TestNewAgentInstance_DefaultAgentWorkspace tests that default agent uses configured workspace
func TestNewAgentInstance_DefaultAgentWorkspace(t *testing.T) {
	tempDir := t.TempDir()

	defaults := &config.AgentDefaults{
		LLM:       "mock/model",
		Workspace: tempDir,
	}

	cfg := &config.Config{}

	provider := &mockProvider{}

	// Create default agent (nil agent config)
	instance := agent.NewAgentInstance(nil, defaults, cfg, provider)

	if instance.Workspace != tempDir {
		t.Errorf("Expected workspace '%s', got '%s'", tempDir, instance.Workspace)
	}

	// Workspace should exist
	if _, err := os.Stat(tempDir); os.IsNotExist(err) {
		t.Error("Expected workspace to be created")
	}
}

// TestNewAgentInstance_NamedAgentWorkspace tests that named agent gets separate workspace
func TestNewAgentInstance_NamedAgentWorkspace(t *testing.T) {
	tempDir := t.TempDir()

	agentCfg := &config.AgentConfig{
		ID: "named-agent",
	}

	defaults := &config.AgentDefaults{
		LLM:       "mock/model",
		Workspace: tempDir,
	}

	cfg := &config.Config{}

	provider := &mockProvider{}

	instance := agent.NewAgentInstance(agentCfg, defaults, cfg, provider)

	if instance.Workspace == "" {
		t.Error("Expected workspace to be set")
	}

	// Named agents should have a different workspace (under .nemesisbot)
	if instance.Workspace == tempDir {
		t.Error("Named agent should have different workspace than default")
	}

	// Workspace should contain agent ID
	if !strings.Contains(instance.Workspace, "named-agent") {
		t.Errorf("Expected workspace to contain agent ID, got '%s'", instance.Workspace)
	}

	// Workspace should exist
	if _, err := os.Stat(instance.Workspace); os.IsNotExist(err) {
		t.Error("Expected workspace to be created")
	}
}

// TestNewAgentInstance_WithEmptyModel tests handling empty model configuration
func TestNewAgentInstance_WithEmptyModel(t *testing.T) {
	tempDir := t.TempDir()

	agentCfg := &config.AgentConfig{
		ID: "empty-model-agent",
		Model: &config.AgentModelConfig{
			Primary: "", // Empty primary model
		},
	}

	defaults := &config.AgentDefaults{
		LLM:       "mock/default-model",
		Workspace: tempDir,
	}

	cfg := &config.Config{}

	provider := &mockProvider{}

	instance := agent.NewAgentInstance(agentCfg, defaults, cfg, provider)

	if instance == nil {
		t.Fatal("Expected non-nil AgentInstance")
	}

	// Should fall back to defaults
	if instance.Model != "mock/default-model" {
		t.Errorf("Expected model to fall back to default, got '%s'", instance.Model)
	}
}

// TestNewAgentInstance_MainAgentID tests that various forms of main agent ID are normalized
func TestNewAgentInstance_MainAgentID(t *testing.T) {
	tempDir := t.TempDir()

	testCases := []struct {
		name     string
		agentID  string
		expected string
	}{
		{"Empty ID", "", "main"},
		{"Default flag", "default", "default"}, // "default" normalizes to "default"
		{"Main ID", "main", "main"},
		{"Custom ID", "custom", "custom"},
	}

	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			agentCfg := &config.AgentConfig{
				ID: tc.agentID,
			}

			defaults := &config.AgentDefaults{
				LLM:       "mock/model",
				Workspace: tempDir,
			}

			cfg := &config.Config{}

			provider := &mockProvider{}

			instance := agent.NewAgentInstance(agentCfg, defaults, cfg, provider)

			if instance.ID != tc.expected {
				t.Errorf("Expected ID '%s', got '%s'", tc.expected, instance.ID)
			}
		})
	}
}

// TestAgentRegistry_ListAgentIDs tests listing agent IDs
func TestAgentRegistry_ListAgentIDs(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &config.Config{
		Agents: config.AgentsConfig{
			List: []config.AgentConfig{
				{ID: "agent1"},
				{ID: "agent2"},
				{ID: "agent3"},
			},
			Defaults: config.AgentDefaults{
				LLM:       "mock/model",
				Workspace: tempDir,
			},
		},
	}

	provider := &mockProvider{}

	registry := agent.NewAgentRegistry(cfg, provider)

	agentIDs := registry.ListAgentIDs()

	if len(agentIDs) != 3 {
		t.Errorf("Expected 3 agent IDs, got %d", len(agentIDs))
	}

	// Check that all expected IDs are present
	idMap := make(map[string]bool)
	for _, id := range agentIDs {
		idMap[id] = true
	}

	if !idMap["agent1"] || !idMap["agent2"] || !idMap["agent3"] {
		t.Error("Expected all agent IDs to be present")
	}
}

// TestAgentRegistry_CanSpawnSubagent_Wildcard tests wildcard subagent permission
func TestAgentRegistry_CanSpawnSubagent_Wildcard(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &config.Config{
		Agents: config.AgentsConfig{
			List: []config.AgentConfig{
				{
					ID: "parent",
					Subagents: &config.SubagentsConfig{
						AllowAgents: []string{"*"},
					},
				},
			},
			Defaults: config.AgentDefaults{
				LLM:       "mock/model",
				Workspace: tempDir,
			},
		},
	}

	provider := &mockProvider{}

	registry := agent.NewAgentRegistry(cfg, provider)

	// Wildcard should allow any agent
	testAgents := []string{"child1", "child2", "any-agent", "random-name"}
	for _, targetAgent := range testAgents {
		if !registry.CanSpawnSubagent("parent", targetAgent) {
			t.Errorf("Expected wildcard to allow spawning '%s'", targetAgent)
		}
	}
}

// TestAgentRegistry_CanSpawnSubagent_NoSubagentsConfig tests agent without subagent config
func TestAgentRegistry_CanSpawnSubagent_NoSubagentsConfig(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &config.Config{
		Agents: config.AgentsConfig{
			List: []config.AgentConfig{
				{
					ID: "parent",
					// No Subagents config
				},
			},
			Defaults: config.AgentDefaults{
				LLM:       "mock/model",
				Workspace: tempDir,
			},
		},
	}

	provider := &mockProvider{}

	registry := agent.NewAgentRegistry(cfg, provider)

	// Agent without config should not be able to spawn
	if registry.CanSpawnSubagent("parent", "child1") {
		t.Error("Expected agent without subagent config to not be able to spawn")
	}
}

// TestNewAgentInstance_WithEmptyFallbacks tests creating agent with empty fallbacks array
func TestNewAgentInstance_WithEmptyFallbacks(t *testing.T) {
	tempDir := t.TempDir()

	agentCfg := &config.AgentConfig{
		ID: "empty-fallbacks-agent",
		Model: &config.AgentModelConfig{
			Primary:   "mock/primary",
			Fallbacks: []string{}, // Empty array
		},
	}

	defaults := &config.AgentDefaults{
		LLM:       "mock/model",
		Workspace: tempDir,
	}

	cfg := &config.Config{}

	provider := &mockProvider{}

	instance := agent.NewAgentInstance(agentCfg, defaults, cfg, provider)

	if instance == nil {
		t.Fatal("Expected non-nil AgentInstance")
	}

	// Should have empty fallbacks
	if instance.Fallbacks == nil {
		t.Error("Expected Fallbacks to be initialized (even if empty)")
	}
}

// TestNewAgentInstance_SessionsDirectory tests that sessions directory is created
func TestNewAgentInstance_SessionsDirectory(t *testing.T) {
	tempDir := t.TempDir()

	agentCfg := &config.AgentConfig{
		ID: "sessions-agent",
	}

	defaults := &config.AgentDefaults{
		LLM:       "mock/model",
		Workspace: tempDir,
	}

	cfg := &config.Config{}

	provider := &mockProvider{}

	instance := agent.NewAgentInstance(agentCfg, defaults, cfg, provider)

	if instance == nil {
		t.Fatal("Expected non-nil AgentInstance")
	}

	// Check that sessions directory exists in workspace
	sessionsDir := filepath.Join(instance.Workspace, "sessions")
	info, err := os.Stat(sessionsDir)
	if err != nil {
		t.Fatalf("Failed to stat sessions directory: %v", err)
	}

	if !info.IsDir() {
		t.Error("Sessions path should be a directory")
	}
}

// TestNewAgentInstance_ToolsRegistered tests that core tools are registered
func TestNewAgentInstance_ToolsRegistered(t *testing.T) {
	tempDir := t.TempDir()

	agentCfg := &config.AgentConfig{
		ID: "tools-agent",
	}

	defaults := &config.AgentDefaults{
		LLM:       "mock/model",
		Workspace: tempDir,
	}

	cfg := &config.Config{}

	provider := &mockProvider{}

	instance := agent.NewAgentInstance(agentCfg, defaults, cfg, provider)

	if instance == nil {
		t.Fatal("Expected non-nil AgentInstance")
	}

	// Check that some core tools are registered
	expectedTools := []string{
		"read_file",
		"write_file",
		"edit_file",
		"list_dir",
		"create_dir",
	}

	for _, toolName := range expectedTools {
		if _, ok := instance.Tools.Get(toolName); !ok {
			t.Errorf("Expected tool '%s' to be registered", toolName)
		}
	}
}

// TestNewAgentInstance_ProviderMeta tests that provider metadata is set
func TestNewAgentInstance_ProviderMeta(t *testing.T) {
	tempDir := t.TempDir()

	agentCfg := &config.AgentConfig{
		ID: "provider-meta-agent",
	}

	defaults := &config.AgentDefaults{
		LLM:       "mock/model",
		Workspace: tempDir,
	}

	cfg := &config.Config{}

	provider := &mockProvider{}

	instance := agent.NewAgentInstance(agentCfg, defaults, cfg, provider)

	if instance == nil {
		t.Fatal("Expected non-nil AgentInstance")
	}

	// Provider metadata should be set
	if instance.ProviderMeta == nil {
		t.Error("Expected ProviderMeta to be set")
	} else {
		// Check that metadata has expected fields
		if instance.ProviderMeta.Name == "" {
			t.Error("Expected ProviderMeta.Name to be set")
		}
	}
}

// TestNewAgentInstance_ContextBuilderInitialized tests that context builder is properly initialized
func TestNewAgentInstance_ContextBuilderInitialized(t *testing.T) {
	tempDir := t.TempDir()

	agentCfg := &config.AgentConfig{
		ID: "context-builder-agent",
	}

	defaults := &config.AgentDefaults{
		LLM:       "mock/model",
		Workspace: tempDir,
	}

	cfg := &config.Config{}

	provider := &mockProvider{}

	instance := agent.NewAgentInstance(agentCfg, defaults, cfg, provider)

	if instance == nil {
		t.Fatal("Expected non-nil AgentInstance")
	}

	// Context builder should be initialized
	if instance.ContextBuilder == nil {
		t.Error("Expected ContextBuilder to be initialized")
	}

	// Should be able to build a system prompt
	prompt := instance.ContextBuilder.BuildSystemPrompt(false)
	if prompt == "" {
		t.Error("Expected non-empty system prompt from context builder")
	}
}
