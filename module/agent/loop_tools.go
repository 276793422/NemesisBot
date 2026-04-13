// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package agent

import (
	"context"
	"fmt"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/channels"
	"github.com/276793422/NemesisBot/module/cluster"
	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/logger"
	"github.com/276793422/NemesisBot/module/mcp"
	"github.com/276793422/NemesisBot/module/path"
	"github.com/276793422/NemesisBot/module/providers"
	"github.com/276793422/NemesisBot/module/tools"
)

// registerSharedTools registers tools that are shared across all agents (web, message, spawn).
func registerSharedTools(cfg *config.Config, msgBus *bus.MessageBus, registry *AgentRegistry, provider providers.LLMProvider, clusterInstance *cluster.Cluster) {
	for _, agentID := range registry.ListAgentIDs() {
		agent, ok := registry.GetAgent(agentID)
		if !ok {
			continue
		}

		// Extract commonly used fields for consistency
		workspace := agent.Workspace
		model := agent.Model

		// Web tools
		if searchTool := tools.NewWebSearchTool(tools.WebSearchToolOptions{
			BraveAPIKey:          cfg.Tools.Web.Brave.APIKey,
			BraveMaxResults:      cfg.Tools.Web.Brave.MaxResults,
			BraveEnabled:         cfg.Tools.Web.Brave.Enabled,
			DuckDuckGoMaxResults: cfg.Tools.Web.DuckDuckGo.MaxResults,
			DuckDuckGoEnabled:    cfg.Tools.Web.DuckDuckGo.Enabled,
			PerplexityAPIKey:     cfg.Tools.Web.Perplexity.APIKey,
			PerplexityMaxResults: cfg.Tools.Web.Perplexity.MaxResults,
			PerplexityEnabled:    cfg.Tools.Web.Perplexity.Enabled,
		}); searchTool != nil {
			agent.Tools.Register(searchTool)
		}
		agent.Tools.Register(tools.NewWebFetchTool(50000))

		// Hardware tools (I2C, SPI) - Linux only, returns error on other platforms
		agent.Tools.Register(tools.NewI2CTool())
		agent.Tools.Register(tools.NewSPITool())

		// Message tool
		messageTool := tools.NewMessageTool()
		messageTool.SetSendCallback(func(channel, chatID, content string) error {
			msgBus.PublishOutbound(bus.OutboundMessage{
				Channel: channel,
				ChatID:  chatID,
				Content: content,
			})
			return nil
		})
		agent.Tools.Register(messageTool)

		// Spawn tool with allowlist checker
		subagentManager := tools.NewSubagentManager(provider, model, workspace, msgBus)
		spawnTool := tools.NewSpawnTool(subagentManager)
		currentAgentID := agentID
		spawnTool.SetAllowlistChecker(func(targetAgentID string) bool {
			return registry.CanSpawnSubagent(currentAgentID, targetAgentID)
		})
		agent.Tools.Register(spawnTool)

		// MCP tools (Model Context Protocol)
		// Load MCP configuration from workspace/config/config.mcp.json
		mcpConfigPath := path.ResolveMCPConfigPathInWorkspace(workspace)
		mcpConfig, err := config.LoadMCPConfig(mcpConfigPath)
		if err != nil {
			logger.WarnCF("agent", "Failed to load MCP config",
				map[string]interface{}{
					"agent_id": agentID,
					"error":    err.Error(),
				})
		} else if mcpConfig.Enabled {
			mcpTools, err := registerMCPTools(mcpConfig, agent)
			if err != nil {
				logger.ErrorCF("agent", "Failed to register MCP tools",
					map[string]interface{}{
						"agent_id": agentID,
						"error":    err.Error(),
					})
			} else if len(mcpTools) > 0 {
				for _, tool := range mcpTools {
					agent.Tools.Register(tool)
				}
				logger.InfoCF("agent", "Registered MCP tools",
					map[string]interface{}{
						"agent_id":   agentID,
						"tool_count": len(mcpTools),
					})
			}
		}

		// Cluster RPC tool (bot-to-bot communication)
		if clusterInstance != nil {
			clusterTool := tools.NewClusterRPCTool(clusterInstance)
			agent.Tools.Register(clusterTool)
			logger.InfoCF("agent", "Registered cluster RPC tool",
				map[string]interface{}{
					"agent_id": agentID,
				})
		}

		// Update context builder with the complete tools registry
		agent.ContextBuilder.SetToolsRegistry(agent.Tools)
	}
}

// registerMCPTools initializes MCP clients and creates tool adapters.
// This function is called during agent initialization if MCP is enabled in config.mcp.json.
func registerMCPTools(mcpConfig *config.MCPConfig, agent *AgentInstance) ([]tools.Tool, error) {
	if !mcpConfig.Enabled || len(mcpConfig.Servers) == 0 {
		return nil, nil
	}

	ctx := context.Background()
	var allTools []tools.Tool

	logger.InfoCF("agent", "Initializing MCP tools",
		map[string]interface{}{
			"agent_id":     agent.ID,
			"server_count": len(mcpConfig.Servers),
		})

	for _, serverCfg := range mcpConfig.Servers {
		// Use defer/recover to catch panics from individual servers
		func() {
			defer func() {
				if r := recover(); r != nil {
					logger.ErrorCF("agent", "Panic during MCP tool registration",
						map[string]interface{}{
							"server": serverCfg.Name,
							"panic":  r,
						})
				}
			}()

			// Create MCP client
			client, err := mcp.NewClient(&mcp.ServerConfig{
				Name:    serverCfg.Name,
				Command: serverCfg.Command,
				Args:    serverCfg.Args,
				Env:     serverCfg.Env,
				Timeout: serverCfg.Timeout,
			})
			if err != nil {
				logger.WarnCF("agent", "Failed to create MCP client",
					map[string]interface{}{
						"server": serverCfg.Name,
						"error":  err.Error(),
					})
				return
			}

			// Initialize client (starts subprocess and performs handshake)
			// Use a timeout context for the Initialize call to prevent hanging.
			// The subprocess itself is not tied to this context (it uses background context).
			timeout := time.Duration(mcpConfig.Timeout) * time.Second
			if serverCfg.Timeout > 0 {
				timeout = time.Duration(serverCfg.Timeout) * time.Second
			}
			initCtx, cancel := context.WithTimeout(ctx, timeout)
			_, err = client.Initialize(initCtx)
			cancel() // Safe to cancel now - subprocess uses background context
			if err != nil {
				logger.WarnCF("agent", "Failed to initialize MCP client",
					map[string]interface{}{
						"server": serverCfg.Name,
						"error":  err.Error(),
					})
				client.Close()
				return
			}

			// Create tool adapters from all available tools
			serverTools, err := mcp.CreateToolsFromClient(client)
			if err != nil {
				logger.WarnCF("agent", "Failed to list MCP tools",
					map[string]interface{}{
						"server": serverCfg.Name,
						"error":  err.Error(),
					})
				client.Close()
				return
			}

			allTools = append(allTools, serverTools...)
			logger.InfoCF("agent", "Connected to MCP server and registered tools",
				map[string]interface{}{
					"server":     serverCfg.Name,
					"tool_count": len(serverTools),
				})
		}()
	}

	return allTools, nil
}

// setupClusterRPCChannel sets up the RPC channel and LLM forward handler for the cluster
func setupClusterRPCChannel(clusterInstance *cluster.Cluster, msgBus *bus.MessageBus) error {
	// Create RPC channel configuration
	// Async callback model: B 端 RPCChannel 等待 LLM 完成，不设超时
	// A 端不再通过 TCP 连接等待，而是通过本地 channel 等待回调
	cfg := &channels.RPCChannelConfig{
		MessageBus:      msgBus,
		RequestTimeout:  24 * time.Hour,     // B端: 极长超时（安全网），LLM 需要多久就等多久
		CleanupInterval: 30 * time.Second,
	}

	// Create RPC channel
	rpcCh, err := channels.NewRPCChannel(cfg)
	if err != nil {
		return fmt.Errorf("failed to create RPC channel: %w", err)
	}

	// Set message bus on cluster for continuation notifications (Phase 2)
	clusterInstance.SetMessageBus(msgBus)

	// NOTE: Don't start RPC channel here!
	// It will be started by ChannelManager.StartAll() after registration
	// This prevents "RPC channel already running" error

	// Set RPC channel on cluster (triggers LLM handler registration)
	clusterInstance.SetRPCChannel(rpcCh)

	logger.InfoC("agent", "RPC channel for peer chat created and configured (will be started by ChannelManager)")

	return nil
}
