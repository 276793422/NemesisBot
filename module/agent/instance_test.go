// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package agent

import (
	"testing"

	"github.com/276793422/NemesisBot/module/config"
)

// TestNewAgentInstance tests creating a new agent instance
func TestNewAgentInstance(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &config.Config{
		Agents: config.AgentsConfig{
			Defaults: config.AgentDefaults{
				LLM:       "test/model",
				Workspace: tempDir,
			},
		},
	}

	provider := &mockProviderForTest{}

	instance := NewAgentInstance(
		&config.AgentConfig{
			ID:   "test-agent",
			Name: "Test Agent",
		},
		&cfg.Agents.Defaults,
		cfg,
		provider,
	)

	if instance == nil {
		t.Fatal("Expected non-nil agent instance")
	}
}

// TestAgentInstance_ProviderMetadata tests provider metadata
func TestAgentInstance_ProviderMetadata(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &config.Config{
		Agents: config.AgentsConfig{
			Defaults: config.AgentDefaults{
				LLM:       "test-model",
				Workspace: tempDir,
			},
		},
	}

	provider := &mockProviderForTest{}

	instance := NewAgentInstance(
		&config.AgentConfig{
			ID:   "test-agent",
			Name: "Test Agent",
		},
		&cfg.Agents.Defaults,
		cfg,
		provider,
	)

	if instance == nil {
		t.Fatal("Expected non-nil agent instance")
	}

	// Provider should be accessible from instance
	if instance.Provider == nil {
		t.Error("Expected provider to be set")
	}

	if instance.Provider.GetDefaultModel() != "test-model" {
		t.Errorf("Expected model test-model, got: %s", instance.Provider.GetDefaultModel())
	}
}

// TestAgentInstance_WorkspaceResolution tests workspace path resolution
func TestAgentInstance_WorkspaceResolution(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &config.Config{
		Agents: config.AgentsConfig{
			Defaults: config.AgentDefaults{
				LLM:       "test/model",
				Workspace: tempDir,
			},
		},
	}

	provider := &mockProviderForTest{}

	instance := NewAgentInstance(
		&config.AgentConfig{
			ID:   "test-agent",
			Name: "Test Agent",
		},
		&cfg.Agents.Defaults,
		cfg,
		provider,
	)

	if instance == nil {
		t.Fatal("Expected non-nil agent instance")
	}

	// Workspace should be set
	if instance.Workspace == "" {
		t.Error("Expected workspace to be set")
	}
}

// TestAgentInstance_ToolsRegistry tests tools registry initialization
func TestAgentInstance_ToolsRegistry(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &config.Config{
		Agents: config.AgentsConfig{
			Defaults: config.AgentDefaults{
				LLM:       "test/model",
				Workspace: tempDir,
			},
		},
	}

	provider := &mockProviderForTest{}

	instance := NewAgentInstance(
		&config.AgentConfig{
			ID:   "test-agent",
			Name: "Test Agent",
		},
		&cfg.Agents.Defaults,
		cfg,
		provider,
	)

	if instance == nil {
		t.Fatal("Expected non-nil agent instance")
	}

	// Tools registry should be initialized
	if instance.Tools == nil {
		t.Error("Expected tools registry to be initialized")
	}
}

// TestAgentInstance_SessionManager tests session manager initialization
func TestAgentInstance_SessionManager(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &config.Config{
		Agents: config.AgentsConfig{
			Defaults: config.AgentDefaults{
				LLM:       "test/model",
				Workspace: tempDir,
			},
		},
	}

	provider := &mockProviderForTest{}

	instance := NewAgentInstance(
		&config.AgentConfig{
			ID:   "test-agent",
			Name: "Test Agent",
		},
		&cfg.Agents.Defaults,
		cfg,
		provider,
	)

	if instance == nil {
		t.Fatal("Expected non-nil agent instance")
	}

	// Session manager should be initialized
	if instance.Sessions == nil {
		t.Error("Expected session manager to be initialized")
	}
}

// TestAgentInstance_CustomWorkspace tests custom workspace per agent
func TestAgentInstance_CustomWorkspace(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &config.Config{
		Agents: config.AgentsConfig{
			Defaults: config.AgentDefaults{
				LLM:       "test/model",
				Workspace: tempDir,
			},
		},
	}

	provider := &mockProviderForTest{}

	customWorkspace := t.TempDir()

	instance := NewAgentInstance(
		&config.AgentConfig{
			ID:        "test-agent",
			Name:      "Test Agent",
			Workspace: customWorkspace,
		},
		&cfg.Agents.Defaults,
		cfg,
		provider,
	)

	if instance == nil {
		t.Fatal("Expected non-nil agent instance")
	}

	// Custom workspace should be used
	if instance.Workspace != customWorkspace {
		t.Errorf("Expected workspace %s, got: %s", customWorkspace, instance.Workspace)
	}
}

// TestAgentInstance_ModelConfiguration tests model configuration
func TestAgentInstance_ModelConfiguration(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &config.Config{
		Agents: config.AgentsConfig{
			Defaults: config.AgentDefaults{
				LLM:       "test/model",
				Workspace: tempDir,
			},
		},
	}

	provider := &mockProviderForTest{}

	instance := NewAgentInstance(
		&config.AgentConfig{
			ID:   "test-agent",
			Name: "Test Agent",
			Model: &config.AgentModelConfig{
				Primary:   "custom/model",
				Fallbacks: []string{"fallback/model"},
			},
		},
		&cfg.Agents.Defaults,
		cfg,
		provider,
	)

	if instance == nil {
		t.Fatal("Expected non-nil agent instance")
	}

	// Agent should be created with custom model
	_ = instance
}

// TestAgentInstance_SkillsConfiguration tests skills configuration
func TestAgentInstance_SkillsConfiguration(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &config.Config{
		Agents: config.AgentsConfig{
			Defaults: config.AgentDefaults{
				LLM:       "test/model",
				Workspace: tempDir,
			},
		},
	}

	provider := &mockProviderForTest{}

	instance := NewAgentInstance(
		&config.AgentConfig{
			ID:   "test-agent",
			Name: "Test Agent",
			Skills: []string{
				"skill1",
				"skill2",
			},
		},
		&cfg.Agents.Defaults,
		cfg,
		provider,
	)

	if instance == nil {
		t.Fatal("Expected non-nil agent instance")
	}

	// Agent should be created with skills
	_ = instance
}

// TestAgentInstance_DefaultAgent tests default agent flag
func TestAgentInstance_DefaultAgent(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &config.Config{
		Agents: config.AgentsConfig{
			Defaults: config.AgentDefaults{
				LLM:       "test/model",
				Workspace: tempDir,
			},
		},
	}

	provider := &mockProviderForTest{}

	instance := NewAgentInstance(
		&config.AgentConfig{
			ID:      "default-agent",
			Name:    "Default Agent",
			Default: true,
		},
		&cfg.Agents.Defaults,
		cfg,
		provider,
	)

	if instance == nil {
		t.Fatal("Expected non-nil agent instance")
	}

	// Agent should be created
	_ = instance
}

// TestAgentInstance_SubagentsConfiguration tests subagents configuration
func TestAgentInstance_SubagentsConfiguration(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &config.Config{
		Agents: config.AgentsConfig{
			Defaults: config.AgentDefaults{
				LLM:       "test/model",
				Workspace: tempDir,
			},
		},
	}

	provider := &mockProviderForTest{}

	instance := NewAgentInstance(
		&config.AgentConfig{
			ID:   "test-agent",
			Name: "Test Agent",
			Subagents: &config.SubagentsConfig{
				AllowAgents: []string{"agent1", "agent2"},
				Model: &config.AgentModelConfig{
					Primary: "subagent/model",
				},
			},
		},
		&cfg.Agents.Defaults,
		cfg,
		provider,
	)

	if instance == nil {
		t.Fatal("Expected non-nil agent instance")
	}

	// Agent should be created with subagents config
	_ = instance
}
