// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package agent

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/path"
	"github.com/276793422/NemesisBot/module/plugin"
	"github.com/276793422/NemesisBot/module/providers"
	"github.com/276793422/NemesisBot/module/routing"
	"github.com/276793422/NemesisBot/module/security"
	"github.com/276793422/NemesisBot/module/session"
	"github.com/276793422/NemesisBot/module/tools"
)

// resolveProviderMetadata extracts provider metadata for logging
func resolveProviderMetadata(cfg *config.Config, model string) *ProviderMetadata {
	// Get effective LLM reference
	llmRef := config.GetEffectiveLLM(cfg)

	// Resolve model configuration
	providerRes, err := config.ResolveModelConfig(cfg, llmRef)
	if err != nil {
		return &ProviderMetadata{
			Name:    "<unknown>",
			APIKey:  "<error>",
			APIBase: "",
		}
	}

	return &ProviderMetadata{
		Name:    providerRes.ProviderName,
		APIKey:  maskAPIKey(providerRes.APIKey),
		APIBase: providerRes.APIBase,
	}
}

// AgentInstance represents a fully configured agent with its own workspace,
// session manager, context builder, and tool registry.
type AgentInstance struct {
	ID             string
	Name           string
	Model          string
	Fallbacks      []string
	Workspace      string
	MaxIterations  int
	ContextWindow  int
	Provider       providers.LLMProvider
	ProviderMeta   *ProviderMetadata // Provider metadata for logging
	Sessions       *session.SessionManager
	ContextBuilder *ContextBuilder
	Tools          *tools.ToolRegistry
	Subagents      *config.SubagentsConfig
	SkillsFilter   []string
	Candidates     []providers.FallbackCandidate
	PluginMgr      *plugin.Manager
}

// NewAgentInstance creates an agent instance from config.
func NewAgentInstance(
	agentCfg *config.AgentConfig,
	defaults *config.AgentDefaults,
	cfg *config.Config,
	provider providers.LLMProvider,
) *AgentInstance {
	workspace := resolveAgentWorkspace(agentCfg, defaults)
	os.MkdirAll(workspace, 0755)

	// Create temp directory for downloads and temporary files
	tempDir := filepath.Join(workspace, "temp")
	os.MkdirAll(tempDir, 0755)

	model := resolveAgentModel(agentCfg, defaults)
	fallbacks := resolveAgentFallbacks(agentCfg, defaults)

	// Resolve provider metadata for logging
	providerMeta := resolveProviderMetadata(cfg, model)

	restrict := defaults.RestrictToWorkspace
	toolsRegistry := tools.NewToolRegistry()

	// Initialize plugin manager
	pluginMgr := plugin.NewManager()

	// Initialize and register security plugin if enabled
	if cfg.Security != nil && cfg.Security.Enabled {
		// Security config should be in workspace/config/config.security.json
		securityConfigPath := path.ResolveSecurityConfigPathInWorkspace(workspace)

		securityPlugin := security.NewSecurityPlugin()
		pluginConfig := map[string]interface{}{
			"config_path": securityConfigPath,
			"enabled":     true,
		}

		if err := securityPlugin.Init(pluginConfig); err != nil {
			// Log error but continue without security
			os.Stderr.WriteString(fmt.Sprintf("Warning: Failed to initialize security plugin: %v\n", err))
		} else {
			if err := pluginMgr.Register(securityPlugin); err != nil {
				os.Stderr.WriteString(fmt.Sprintf("Warning: Failed to register security plugin: %v\n", err))
			}
		}
	}

	// Register tools with plugin support
	// Create plugin-aware tools
	user := "agent"
	source := "cli"

	// Register core tools with plugin wrapper
	toolsRegistry.RegisterWithPlugin(tools.NewReadFileTool(workspace, restrict), pluginMgr, user, source, workspace)
	toolsRegistry.RegisterWithPlugin(tools.NewWriteFileTool(workspace, restrict), pluginMgr, user, source, workspace)
	toolsRegistry.RegisterWithPlugin(tools.NewEditFileTool(workspace, restrict), pluginMgr, user, source, workspace)
	toolsRegistry.RegisterWithPlugin(tools.NewAppendFileTool(workspace, restrict), pluginMgr, user, source, workspace)
	toolsRegistry.RegisterWithPlugin(tools.NewDeleteFileTool(workspace, restrict), pluginMgr, user, source, workspace)
	toolsRegistry.RegisterWithPlugin(tools.NewListDirTool(workspace, restrict), pluginMgr, user, source, workspace)
	toolsRegistry.RegisterWithPlugin(tools.NewCreateDirTool(workspace, restrict), pluginMgr, user, source, workspace)
	toolsRegistry.RegisterWithPlugin(tools.NewDeleteDirTool(workspace, restrict), pluginMgr, user, source, workspace)
	toolsRegistry.RegisterWithPlugin(tools.NewExecToolWithConfig(workspace, restrict, cfg), pluginMgr, user, source, workspace)
	toolsRegistry.RegisterWithPlugin(tools.NewAsyncExecToolWithConfig(workspace, restrict, cfg), pluginMgr, user, source, workspace) // Async exec tool
	toolsRegistry.Register(tools.NewCompleteBootstrapTool(workspace))                                                                // Bootstrap tool doesn't need security
	//	NOTE : ???????,????????
	//toolsRegistry.Register(tools.NewSleepTool())                                                                                     // Sleep tool doesn't need security

	sessionsDir := filepath.Join(workspace, "sessions")
	sessionsManager := session.NewSessionManager(sessionsDir)

	contextBuilder := NewContextBuilder(workspace)
	contextBuilder.SetToolsRegistry(toolsRegistry)

	agentID := routing.DefaultAgentID
	agentName := ""
	var subagents *config.SubagentsConfig
	var skillsFilter []string

	if agentCfg != nil {
		agentID = routing.NormalizeAgentID(agentCfg.ID)
		agentName = agentCfg.Name
		subagents = agentCfg.Subagents
		skillsFilter = agentCfg.Skills
	}

	maxIter := defaults.MaxToolIterations
	if maxIter == 0 {
		maxIter = 20
	}

	// Resolve fallback candidates
	modelCfg := providers.ModelConfig{
		Primary:   model,
		Fallbacks: fallbacks,
	}
	// Extract default provider from model reference (e.g., "openai/gpt-4" -> "openai")
	defaultProvider := ""
	if parts := strings.SplitN(model, "/", 2); len(parts) == 2 {
		defaultProvider = parts[0]
	}
	candidates := providers.ResolveCandidates(modelCfg, defaultProvider)

	return &AgentInstance{
		ID:             agentID,
		Name:           agentName,
		Model:          model,
		Fallbacks:      fallbacks,
		Workspace:      workspace,
		MaxIterations:  maxIter,
		ContextWindow:  defaults.MaxTokens,
		Provider:       provider,
		ProviderMeta:   providerMeta,
		Sessions:       sessionsManager,
		ContextBuilder: contextBuilder,
		Tools:          toolsRegistry,
		Subagents:      subagents,
		SkillsFilter:   skillsFilter,
		Candidates:     candidates,
		PluginMgr:      pluginMgr,
	}
}

// resolveAgentWorkspace determines the workspace directory for an agent.
func resolveAgentWorkspace(agentCfg *config.AgentConfig, defaults *config.AgentDefaults) string {
	pm := path.DefaultPathManager()

	// If agent config has a custom workspace, use it
	if agentCfg != nil && strings.TrimSpace(agentCfg.Workspace) != "" {
		return path.ExpandHome(strings.TrimSpace(agentCfg.Workspace))
	}

	// For default/main agents, use the default workspace from config
	if agentCfg == nil || agentCfg.Default || agentCfg.ID == "" || routing.NormalizeAgentID(agentCfg.ID) == "main" {
		return path.ExpandHome(defaults.Workspace)
	}

	// For named agents, use a separate workspace under .nemesisbot
	agentID := routing.NormalizeAgentID(agentCfg.ID)
	return pm.AgentWorkspace(agentID)
}

// resolveAgentModel resolves the primary model for an agent.
// It checks the agent config first, then falls back to defaults.LLM.
func resolveAgentModel(agentCfg *config.AgentConfig, defaults *config.AgentDefaults) string {
	if agentCfg != nil && agentCfg.Model != nil && strings.TrimSpace(agentCfg.Model.Primary) != "" {
		return strings.TrimSpace(agentCfg.Model.Primary)
	}
	// Use the new LLM field as fallback (format: "provider/model")
	return defaults.LLM
}

// resolveAgentFallbacks resolves the fallback models for an agent.
// It checks the agent config first. If no agent config, returns nil
// (fallbacks should be configured in agent's Model or via ModelList).
func resolveAgentFallbacks(agentCfg *config.AgentConfig, defaults *config.AgentDefaults) []string {
	if agentCfg != nil && agentCfg.Model != nil && agentCfg.Model.Fallbacks != nil {
		return agentCfg.Model.Fallbacks
	}
	// No global fallbacks anymore - configure per-agent or via ModelList
	return nil
}
