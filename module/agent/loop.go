// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package agent

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"sync/atomic"
	"time"
	"unicode/utf8"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/channels"
	"github.com/276793422/NemesisBot/module/cluster"
	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/constants"
	"github.com/276793422/NemesisBot/module/logger"
	"github.com/276793422/NemesisBot/module/mcp"
	"github.com/276793422/NemesisBot/module/path"
	"github.com/276793422/NemesisBot/module/providers"
	"github.com/276793422/NemesisBot/module/routing"
	"github.com/276793422/NemesisBot/module/state"
	"github.com/276793422/NemesisBot/module/tools"
	"github.com/276793422/NemesisBot/module/utils"
)

type AgentLoop struct {
	bus            *bus.MessageBus
	cfg            *config.Config
	registry       *AgentRegistry
	state          *state.Manager
	running        atomic.Bool
	summarizing    sync.Map
	fallback       *providers.FallbackChain
	channelManager *channels.Manager
	cluster        *cluster.Cluster
	// Session-level busy state management
	sessionBusy    sync.Map // sessionKey -> *sessionBusyState
	concurrentMode string   // "reject" or "queue"
	queueSize      int      // Only effective in queue mode
}

// sessionBusyState tracks the busy state and queue for a session
type sessionBusyState struct {
	mu          sync.Mutex
	busy        bool
	queueLength int
}

// Busy message returned when session is busy
const busyMessage = "⏳ AI 正在处理上一个请求，请稍后再试"

// processOptions configures how a message is processed
type processOptions struct {
	SessionKey      string         // Session identifier for history/context
	Channel         string         // Target channel for tool execution
	ChatID          string         // Target chat ID for tool execution
	UserMessage     string         // User message content (may include prefix)
	DefaultResponse string         // Response when LLM returns empty
	EnableSummary   bool           // Whether to trigger summarization
	SendResponse    bool           // Whether to send response via bus
	NoHistory       bool           // If true, don't load session history (for heartbeat)
	RequestLogger   *RequestLogger // Request logger instance
}

func NewAgentLoop(cfg *config.Config, msgBus *bus.MessageBus, provider providers.LLMProvider) *AgentLoop {
	registry := NewAgentRegistry(cfg, provider)

	// Initialize cluster if enabled (load from separate config file)
	var clusterInstance *cluster.Cluster
	workspace := cfg.Agents.Defaults.Workspace
	// Resolve workspace path
	if strings.HasPrefix(workspace, "~/") {
		homeDir, _ := os.UserHomeDir()
		workspace = filepath.Join(homeDir, workspace[2:])
	}

	// Load cluster config from workspace/config/config.cluster.json
	clusterCfg, err := cluster.LoadAppConfig(workspace)
	if err != nil {
		logger.WarnCF("agent", "Failed to load cluster config",
			map[string]interface{}{"error": err.Error()})
	} else if clusterCfg.Enabled {
		var err error
		clusterInstance, err = cluster.NewCluster(workspace)
		if err != nil {
			logger.ErrorCF("agent", "Failed to create cluster",
				map[string]interface{}{"error": err.Error()})
		} else {
			// Set ports from config
			clusterInstance.SetPorts(clusterCfg.Port, clusterCfg.RPCPort)

			// Start cluster discovery and RPC
			if err := clusterInstance.Start(); err != nil {
				logger.ErrorCF("agent", "Failed to start cluster",
					map[string]interface{}{"error": err.Error()})
				clusterInstance = nil
			} else {
				logger.InfoCF("agent", "Cluster started",
					map[string]interface{}{
						"node_id":  clusterInstance.GetNodeID(),
						"udp_port": clusterCfg.Port,
						"rpc_port": clusterCfg.RPCPort,
						"address":  clusterInstance.GetAddress(),
					})

				// Create and setup RPC channel for LLM forwarding
				if err := setupClusterRPCChannel(clusterInstance, msgBus); err != nil {
					logger.WarnCF("agent", "Failed to setup RPC channel for LLM forwarding",
						map[string]interface{}{"error": err.Error()})
				}
			}
		}
	}

	// Register shared tools to all agents (pass cluster instance for tool registration)
	registerSharedTools(cfg, msgBus, registry, provider, clusterInstance)

	// Set up shared fallback chain
	cooldown := providers.NewCooldownTracker()
	fallbackChain := providers.NewFallbackChain(cooldown)

	// Create state manager using default agent's workspace for channel recording
	defaultAgent := registry.GetDefaultAgent()
	var stateManager *state.Manager
	if defaultAgent != nil {
		stateManager = state.NewManager(defaultAgent.Workspace)
	}

	// Get concurrent request mode settings
	concurrentMode := cfg.Agents.Defaults.ConcurrentRequestMode
	if concurrentMode == "" {
		concurrentMode = "reject" // default
	}
	queueSize := cfg.Agents.Defaults.QueueSize
	if queueSize <= 0 {
		queueSize = 8 // default
	}

	return &AgentLoop{
		bus:            msgBus,
		cfg:            cfg,
		registry:       registry,
		state:          stateManager,
		summarizing:    sync.Map{},
		fallback:       fallbackChain,
		cluster:        clusterInstance,
		sessionBusy:    sync.Map{},
		concurrentMode: concurrentMode,
		queueSize:      queueSize,
	}
}

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

func (al *AgentLoop) Run(ctx context.Context) error {
	al.running.Store(true)

	for al.running.Load() {
		select {
		case <-ctx.Done():
			return nil
		default:
			msg, ok := al.bus.ConsumeInbound(ctx)
			if !ok {
				continue
			}

			usedAgentID, response, err := al.processMessage(ctx, msg)
			if err != nil {
				response = fmt.Sprintf("Error processing message: %v", err)
				logger.DebugCF("agent", "Error captured, creating error response",
					map[string]interface{}{
						"error":            err.Error(),
						"response_preview": utils.Truncate(response, 100),
					})
			}

			if response != "" {
				logger.DebugCF("agent", "Response ready to send",
					map[string]interface{}{
						"channel":          msg.Channel,
						"chat_id":          msg.ChatID,
						"response_len":     len(response),
						"response_preview": utils.Truncate(response, 80),
					})

				// Check if the message tool already sent a response during this round.
				// If so, skip publishing to avoid duplicate messages to the user.
				// Use the agent that actually processed the request.
				alreadySent := false
				usedAgent, agentOK := al.registry.GetAgent(usedAgentID)
				if !agentOK {
					usedAgent = al.registry.GetDefaultAgent()
				}
				if usedAgent != nil {
					if tool, ok := usedAgent.Tools.Get("message"); ok {
						if mt, ok := tool.(*tools.MessageTool); ok {
							alreadySent = mt.HasSentInRound()
							logger.DebugCF("agent", "MessageTool alreadySent check",
								map[string]interface{}{
									"alreadySent": alreadySent,
									"agent_id":    usedAgent.ID,
								})
						} else {
							logger.DebugCF("agent", "MessageTool not a MessageTool instance",
								map[string]interface{}{"tool_type": fmt.Sprintf("%T", tool)})
						}
					} else {
						logger.DebugCF("agent", "MessageTool not found in agent", nil)
					}
				} else {
					logger.DebugCF("agent", "Agent not found for alreadySent check", nil)
				}

				if !alreadySent {
					logger.DebugCF("agent", "Publishing outbound response",
						map[string]interface{}{
							"channel":     msg.Channel,
							"chat_id":     msg.ChatID,
							"content_len": len(response),
						})

					// For RPC channel, we need to add correlation ID prefix
					// This is required because the response might not have gone through MessageTool
					finalContent := response
					if msg.Channel == "rpc" && msg.CorrelationID != "" {
						finalContent = fmt.Sprintf("[rpc:%s] %s", msg.CorrelationID, response)
						logger.InfoCF("agent", "Added correlation ID prefix to RPC response",
							map[string]interface{}{
								"correlation_id":  msg.CorrelationID,
								"content_preview": utils.Truncate(finalContent, 100),
							})
					}

					al.bus.PublishOutbound(bus.OutboundMessage{
						Channel: msg.Channel,
						ChatID:  msg.ChatID,
						Content: finalContent,
					})
					logger.InfoCF("agent", "Outbound response published",
						map[string]interface{}{
							"channel": msg.Channel,
							"chat_id": msg.ChatID,
						})
				} else {
					logger.InfoCF("agent", "Skipping outbound response (message tool already sent)",
						map[string]interface{}{
							"channel": msg.Channel,
							"chat_id": msg.ChatID,
						})
				}
			} else {
				logger.DebugCF("agent", "Empty response, not sending",
					map[string]interface{}{
						"channel": msg.Channel,
						"chat_id": msg.ChatID,
					})
			}
		}
	}

	return nil
}

func (al *AgentLoop) Stop() {
	al.running.Store(false)

	// Stop cluster if it was initialized
	if al.cluster != nil {
		if err := al.cluster.Stop(); err != nil {
			logger.ErrorCF("agent", "Failed to stop cluster",
				map[string]interface{}{"error": err.Error()})
		}
	}
}

func (al *AgentLoop) RegisterTool(tool tools.Tool) {
	for _, agentID := range al.registry.ListAgentIDs() {
		if agent, ok := al.registry.GetAgent(agentID); ok {
			agent.Tools.Register(tool)
		}
	}
}

func (al *AgentLoop) SetChannelManager(cm *channels.Manager) {
	al.channelManager = cm

	// Register RPC channel to channel manager if cluster has one
	// This is needed so that dispatchOutbound can deliver messages to RPC channel
	if al.cluster != nil {
		if rpcCh := al.cluster.GetRPCChannel(); rpcCh != nil {
			cm.RegisterChannel("rpc", rpcCh)
			logger.InfoC("agent", "RPC channel registered to channel manager")
		}
	}
}

// RecordLastChannel records the last active channel for this workspace.
// This uses the atomic state save mechanism to prevent data loss on crash.
func (al *AgentLoop) RecordLastChannel(channel string) error {
	if al.state == nil {
		return nil
	}
	return al.state.SetLastChannel(channel)
}

// RecordLastChatID records the last active chat ID for this workspace.
// This uses the atomic state save mechanism to prevent data loss on crash.
func (al *AgentLoop) RecordLastChatID(chatID string) error {
	if al.state == nil {
		return nil
	}
	return al.state.SetLastChatID(chatID)
}

// getSessionBusyState gets or creates the busy state for a session
func (al *AgentLoop) getSessionBusyState(sessionKey string) *sessionBusyState {
	if value, ok := al.sessionBusy.Load(sessionKey); ok {
		return value.(*sessionBusyState)
	}

	// Create new state
	state := &sessionBusyState{busy: false, queueLength: 0}
	actual, loaded := al.sessionBusy.LoadOrStore(sessionKey, state)
	if loaded {
		return actual.(*sessionBusyState)
	}
	return state
}

// tryAcquireSession tries to acquire the session for processing
// Returns true if acquired, false if session is busy (and queue is full in queue mode)
func (al *AgentLoop) tryAcquireSession(sessionKey string) bool {
	state := al.getSessionBusyState(sessionKey)
	state.mu.Lock()
	defer state.mu.Unlock()

	if !state.busy {
		state.busy = true
		return true
	}

	// Session is busy
	if al.concurrentMode == "reject" {
		return false
	}

	// Queue mode: check if queue is full
	if state.queueLength >= al.queueSize {
		return false
	}

	// Increment queue length
	state.queueLength++
	return false
}

// releaseSession releases the session and returns true if there are queued requests
func (al *AgentLoop) releaseSession(sessionKey string) bool {
	state := al.getSessionBusyState(sessionKey)
	state.mu.Lock()
	defer state.mu.Unlock()

	if state.queueLength > 0 {
		state.queueLength--
		// Keep busy true since there are queued requests
		return true
	}

	state.busy = false
	return false
}

func (al *AgentLoop) ProcessDirect(ctx context.Context, content, sessionKey string) (string, error) {
	return al.ProcessDirectWithChannel(ctx, content, sessionKey, "cli", "direct")
}

func (al *AgentLoop) ProcessDirectWithChannel(ctx context.Context, content, sessionKey, channel, chatID string) (string, error) {
	msg := bus.InboundMessage{
		Channel:    channel,
		SenderID:   "cron",
		ChatID:     chatID,
		Content:    content,
		SessionKey: sessionKey,
	}

	_, response, err := al.processMessage(ctx, msg)
	return response, err
}

// ProcessHeartbeat processes a heartbeat request without session history.
// Each heartbeat is independent and doesn't accumulate context.
func (al *AgentLoop) ProcessHeartbeat(ctx context.Context, content, channel, chatID string) (string, error) {
	agent := al.registry.GetDefaultAgent()
	return al.runAgentLoop(ctx, agent, processOptions{
		SessionKey:      "heartbeat",
		Channel:         channel,
		ChatID:          chatID,
		UserMessage:     content,
		DefaultResponse: "I've completed processing but have no response to give.",
		EnableSummary:   false,
		SendResponse:    false,
		NoHistory:       true, // Don't load session history for heartbeat
	})
}

func (al *AgentLoop) processMessage(ctx context.Context, msg bus.InboundMessage) (string, string, error) {
	// Add message preview to log (show full content for error messages)
	var logContent string
	if strings.Contains(msg.Content, "Error:") || strings.Contains(msg.Content, "error") {
		logContent = msg.Content // Full content for errors
	} else {
		logContent = utils.Truncate(msg.Content, 80)
	}
	logger.InfoCF("agent", fmt.Sprintf("Processing message from %s:%s: %s", msg.Channel, msg.SenderID, logContent),
		map[string]interface{}{
			"channel":     msg.Channel,
			"chat_id":     msg.ChatID,
			"sender_id":   msg.SenderID,
			"session_key": msg.SessionKey,
		})

	// Route system messages to processSystemMessage
	if msg.Channel == "system" {
		resp, err := al.processSystemMessage(ctx, msg)
		return "", resp, err
	}

	// Check for commands
	if response, handled := al.handleCommand(ctx, msg); handled {
		return "", response, nil
	}

	// Route to determine agent and session key
	route := al.registry.ResolveRoute(routing.RouteInput{
		Channel:    msg.Channel,
		AccountID:  msg.Metadata["account_id"],
		Peer:       extractPeer(msg),
		ParentPeer: extractParentPeer(msg),
		GuildID:    msg.Metadata["guild_id"],
		TeamID:     msg.Metadata["team_id"],
	})

	agent, ok := al.registry.GetAgent(route.AgentID)
	if !ok {
		agent = al.registry.GetDefaultAgent()
	}

	// Use routed session key, but honor pre-set agent-scoped keys (for ProcessDirect/cron)
	sessionKey := route.SessionKey
	if msg.SessionKey != "" && strings.HasPrefix(msg.SessionKey, "agent:") {
		sessionKey = msg.SessionKey
	}

	logger.InfoCF("agent", "Routed message",
		map[string]interface{}{
			"agent_id":    agent.ID,
			"session_key": sessionKey,
			"matched_by":  route.MatchedBy,
		})

	// Check if session is busy and try to acquire it
	if !al.tryAcquireSession(sessionKey) {
		logger.WarnCF("agent", "Session busy, returning busy message",
			map[string]interface{}{
				"session_key":     sessionKey,
				"concurrent_mode": al.concurrentMode,
			})
		return agent.ID, busyMessage, nil
	}

	// Ensure session is released when done
	defer func() {
		al.releaseSession(sessionKey)
	}()

	// If message contains CorrelationID (for RPC), add it to context
	// This allows tools like MessageTool to include it in responses
	if msg.CorrelationID != "" {
		ctx = context.WithValue(ctx, "correlation_id", msg.CorrelationID)
	}

	result, err := al.runAgentLoop(ctx, agent, processOptions{
		SessionKey:      sessionKey,
		Channel:         msg.Channel,
		ChatID:          msg.ChatID,
		UserMessage:     msg.Content,
		DefaultResponse: "I've completed processing but have no response to give.",
		EnableSummary:   true,
		SendResponse:    false,
	})

	return agent.ID, result, err
}

func (al *AgentLoop) processSystemMessage(ctx context.Context, msg bus.InboundMessage) (string, error) {
	if msg.Channel != "system" {
		return "", fmt.Errorf("processSystemMessage called with non-system message channel: %s", msg.Channel)
	}

	logger.InfoCF("agent", "Processing system message",
		map[string]interface{}{
			"sender_id": msg.SenderID,
			"chat_id":   msg.ChatID,
		})

	// Parse origin channel from chat_id (format: "channel:chat_id")
	var originChannel, originChatID string
	if idx := strings.Index(msg.ChatID, ":"); idx > 0 {
		originChannel = msg.ChatID[:idx]
		originChatID = msg.ChatID[idx+1:]
	} else {
		originChannel = "cli"
		originChatID = msg.ChatID
	}

	// Extract subagent result from message content
	// Format: "Task 'label' completed.\n\nResult:\n<actual content>"
	content := msg.Content
	if idx := strings.Index(content, "Result:\n"); idx >= 0 {
		content = content[idx+8:] // Extract just the result part
	}

	// Skip internal channels - only log, don't send to user
	if constants.IsInternalChannel(originChannel) {
		logger.InfoCF("agent", "Subagent completed (internal channel)",
			map[string]interface{}{
				"sender_id":   msg.SenderID,
				"content_len": len(content),
				"channel":     originChannel,
			})
		return "", nil
	}

	// Use default agent for system messages
	agent := al.registry.GetDefaultAgent()

	// Use the origin session for context
	sessionKey := routing.BuildAgentMainSessionKey(agent.ID)

	return al.runAgentLoop(ctx, agent, processOptions{
		SessionKey:      sessionKey,
		Channel:         originChannel,
		ChatID:          originChatID,
		UserMessage:     fmt.Sprintf("[System: %s] %s", msg.SenderID, msg.Content),
		DefaultResponse: "Background task completed.",
		EnableSummary:   false,
		SendResponse:    true,
	})
}

// runAgentLoop is the core message processing logic.
func (al *AgentLoop) runAgentLoop(ctx context.Context, agent *AgentInstance, opts processOptions) (string, error) {
	// Initialize request logger if enabled
	var reqLogger *RequestLogger
	if al.cfg.Logging != nil && al.cfg.Logging.LLM != nil && al.cfg.Logging.LLM.Enabled {
		workspace := al.cfg.WorkspacePath()
		reqLogger = NewRequestLogger(al.cfg.Logging, workspace)
		if reqLogger.IsEnabled() {
			if err := reqLogger.CreateSession(); err != nil {
				logger.WarnC("request_logger", fmt.Sprintf("Failed to create logging session: %v", err))
			} else {
				// Log user request
				reqLogger.LogUserRequest(UserRequestInfo{
					Timestamp: time.Now(),
					Channel:   opts.Channel,
					SenderID:  "user", // Could be extracted from msg if needed
					ChatID:    opts.ChatID,
					Content:   opts.UserMessage,
				})
			}
		}
	}
	opts.RequestLogger = reqLogger

	// 0. Record last channel for heartbeat notifications (skip internal channels)
	if opts.Channel != "" && opts.ChatID != "" {
		// Don't record internal channels (cli, system, subagent)
		if !constants.IsInternalChannel(opts.Channel) {
			channelKey := fmt.Sprintf("%s:%s", opts.Channel, opts.ChatID)
			if err := al.RecordLastChannel(channelKey); err != nil {
				logger.WarnCF("agent", "Failed to record last channel", map[string]interface{}{"error": err.Error()})
			}
		}
	}

	// 1. Update tool contexts
	al.updateToolContexts(agent, opts.Channel, opts.ChatID)

	// 2. Build messages (skip history for heartbeat)
	var history []providers.Message
	var summary string
	if !opts.NoHistory {
		history = agent.Sessions.GetHistory(opts.SessionKey)
		summary = agent.Sessions.GetSummary(opts.SessionKey)
	}

	// Determine if this is a heartbeat request (to skip bootstrap)
	skipBootstrap := (opts.SessionKey == "heartbeat")

	messages := agent.ContextBuilder.BuildMessages(
		history,
		summary,
		opts.UserMessage,
		nil,
		opts.Channel,
		opts.ChatID,
		skipBootstrap, // Pass skipBootstrap parameter
	)

	// 3. Save user message to session
	agent.Sessions.AddMessage(opts.SessionKey, "user", opts.UserMessage)

	// 4. Run LLM iteration loop
	finalContent, iteration, err := al.runLLMIteration(ctx, agent, messages, opts)
	if err != nil {
		// Log error and close logger before returning
		if reqLogger != nil && reqLogger.IsEnabled() {
			totalDuration := time.Since(reqLogger.startTime)
			reqLogger.LogFinalResponse(FinalResponseInfo{
				Timestamp:     time.Now(),
				TotalDuration: totalDuration,
				LLMRounds:     iteration,
				Content:       fmt.Sprintf("Error: %s", err.Error()),
				Channel:       opts.Channel,
				ChatID:        opts.ChatID,
			})
			reqLogger.Close()
		}
		return "", err
	}

	// If last tool had ForUser content and we already sent it, we might not need to send final response
	// This is controlled by the tool's Silent flag and ForUser content

	// 5. Handle empty response
	if finalContent == "" {
		finalContent = opts.DefaultResponse
	}

	// 6. Save final assistant message to session
	agent.Sessions.AddMessage(opts.SessionKey, "assistant", finalContent)
	agent.Sessions.Save(opts.SessionKey)

	// 7. Optional: summarization
	if opts.EnableSummary {
		al.maybeSummarize(agent, opts.SessionKey, opts.Channel, opts.ChatID)
	}

	// 8. Optional: send response via bus
	if opts.SendResponse {
		al.bus.PublishOutbound(bus.OutboundMessage{
			Channel: opts.Channel,
			ChatID:  opts.ChatID,
			Content: finalContent,
		})
	}

	// 9. Log response
	responsePreview := utils.Truncate(finalContent, 120)
	logger.InfoCF("agent", fmt.Sprintf("Response: %s", responsePreview),
		map[string]interface{}{
			"agent_id":     agent.ID,
			"session_key":  opts.SessionKey,
			"iterations":   iteration,
			"final_length": len(finalContent),
		})

	// 10. Log final response to request logger and close
	if reqLogger != nil && reqLogger.IsEnabled() {
		totalDuration := time.Since(reqLogger.startTime)
		reqLogger.LogFinalResponse(FinalResponseInfo{
			Timestamp:     time.Now(),
			TotalDuration: totalDuration,
			LLMRounds:     iteration,
			Content:       finalContent,
			Channel:       opts.Channel,
			ChatID:        opts.ChatID,
		})
		reqLogger.Close()
	}

	return finalContent, nil
}

// runLLMIteration executes the LLM call loop with tool handling.
func (al *AgentLoop) runLLMIteration(ctx context.Context, agent *AgentInstance, messages []providers.Message, opts processOptions) (string, int, error) {
	iteration := 0
	var finalContent string
	// Track local operations for each round
	localOperations := make(map[int][]Operation)

	for iteration < agent.MaxIterations {
		iteration++
		roundStartTime := time.Now()

		logger.DebugCF("agent", "LLM iteration",
			map[string]interface{}{
				"agent_id":  agent.ID,
				"iteration": iteration,
				"max":       agent.MaxIterations,
			})

		// Build tool definitions
		providerToolDefs := agent.Tools.ToProviderDefs()

		// Build full configuration map
		fullConfig := map[string]interface{}{
			"max_tokens":  8192,
			"temperature": 0.7,
		}

		// Prepare HTTP headers (excluding Authorization)
		httpHeaders := map[string]string{
			"Content-Type": "application/json",
		}

		// Log LLM request details
		logger.DebugCF("agent", "LLM request",
			map[string]interface{}{
				"agent_id":          agent.ID,
				"iteration":         iteration,
				"model":             agent.Model,
				"messages_count":    len(messages),
				"tools_count":       len(providerToolDefs),
				"max_tokens":        8192,
				"temperature":       0.7,
				"system_prompt_len": len(messages[0].Content),
			})

		// Log full messages (detailed)
		logger.DebugCF("agent", "Full LLM request",
			map[string]interface{}{
				"iteration":     iteration,
				"messages_json": formatMessagesForLog(messages),
				"tools_json":    formatToolsForLog(providerToolDefs),
			})

		// Log LLM request to request logger with enhanced information
		if opts.RequestLogger != nil && opts.RequestLogger.IsEnabled() {
			opts.RequestLogger.LogLLMRequest(LLMRequestInfo{
				Round:        iteration,
				Timestamp:    time.Now(),
				Model:        agent.Model,
				ProviderName: agent.ProviderMeta.Name,
				APIKey:       agent.ProviderMeta.APIKey,
				APIBase:      agent.ProviderMeta.APIBase,
				HTTPHeaders:  httpHeaders,
				FullConfig:   fullConfig,
				Messages:     messages,
				Tools:        providerToolDefs,
			})
		}

		// Call LLM with fallback chain if candidates are configured.
		var response *providers.LLMResponse
		var err error

		callLLM := func() (*providers.LLMResponse, error) {
			if len(agent.Candidates) > 1 && al.fallback != nil {
				var fbErr error
				fbResult, fbErr := al.fallback.Execute(ctx, agent.Candidates,
					func(ctx context.Context, provider, model string) (*providers.LLMResponse, error) {
						return agent.Provider.Chat(ctx, messages, providerToolDefs, model, map[string]interface{}{
							"max_tokens":  8192,
							"temperature": 0.7,
						})
					},
				)
				if fbErr != nil {
					return nil, fbErr
				}
				if fbResult.Provider != "" && len(fbResult.Attempts) > 0 {
					logger.InfoCF("agent", fmt.Sprintf("Fallback: succeeded with %s/%s after %d attempts",
						fbResult.Provider, fbResult.Model, len(fbResult.Attempts)+1),
						map[string]interface{}{"agent_id": agent.ID, "iteration": iteration})
				}
				return fbResult.Response, nil
			}
			return agent.Provider.Chat(ctx, messages, providerToolDefs, agent.Model, map[string]interface{}{
				"max_tokens":  8192,
				"temperature": 0.7,
			})
		}

		// Retry loop for context/token errors
		maxRetries := 2
		for retry := 0; retry <= maxRetries; retry++ {
			response, err = callLLM()
			if err == nil {
				break
			}

			errMsg := strings.ToLower(err.Error())
			isContextError := strings.Contains(errMsg, "token") ||
				strings.Contains(errMsg, "context") ||
				strings.Contains(errMsg, "invalidparameter") ||
				strings.Contains(errMsg, "length")

			if isContextError && retry < maxRetries {
				logger.WarnCF("agent", "Context window error detected, attempting compression", map[string]interface{}{
					"error": err.Error(),
					"retry": retry,
				})

				if retry == 0 && !constants.IsInternalChannel(opts.Channel) {
					al.bus.PublishOutbound(bus.OutboundMessage{
						Channel: opts.Channel,
						ChatID:  opts.ChatID,
						Content: "Context window exceeded. Compressing history and retrying...",
					})
				}

				al.forceCompression(agent, opts.SessionKey)
				newHistory := agent.Sessions.GetHistory(opts.SessionKey)
				newSummary := agent.Sessions.GetSummary(opts.SessionKey)

				// Use the same skipBootstrap logic
				skipBootstrap := (opts.SessionKey == "heartbeat")

				messages = agent.ContextBuilder.BuildMessages(
					newHistory, newSummary, "",
					nil, opts.Channel, opts.ChatID, skipBootstrap,
				)
				continue
			}
			break
		}

		if err != nil {
			logger.ErrorCF("agent", "LLM call failed",
				map[string]interface{}{
					"agent_id":  agent.ID,
					"iteration": iteration,
					"error":     err.Error(),
				})
			return "", iteration, fmt.Errorf("LLM call failed after retries: %w", err)
		}

		// Log LLM response to request logger
		if opts.RequestLogger != nil && opts.RequestLogger.IsEnabled() {
			duration := time.Since(roundStartTime)
			opts.RequestLogger.LogLLMResponse(LLMResponseInfo{
				Round:        iteration,
				Timestamp:    time.Now(),
				Duration:     duration,
				Content:      response.Content,
				ToolCalls:    response.ToolCalls,
				Usage:        response.Usage,
				FinishReason: response.FinishReason,
			})
		}

		// Check if no tool calls - we're done
		if len(response.ToolCalls) == 0 {
			finalContent = response.Content
			logger.InfoCF("agent", "LLM response without tool calls (direct answer)",
				map[string]interface{}{
					"agent_id":      agent.ID,
					"iteration":     iteration,
					"content_chars": len(finalContent),
				})
			break
		}

		// Log tool calls
		toolNames := make([]string, 0, len(response.ToolCalls))
		for _, tc := range response.ToolCalls {
			toolNames = append(toolNames, tc.Name)
		}
		logger.InfoCF("agent", "LLM requested tool calls",
			map[string]interface{}{
				"agent_id":  agent.ID,
				"tools":     toolNames,
				"count":     len(response.ToolCalls),
				"iteration": iteration,
			})

		// Build assistant message with tool calls
		assistantMsg := providers.Message{
			Role:    "assistant",
			Content: response.Content,
		}
		for _, tc := range response.ToolCalls {
			argumentsJSON, _ := json.Marshal(tc.Arguments)
			assistantMsg.ToolCalls = append(assistantMsg.ToolCalls, providers.ToolCall{
				ID:   tc.ID,
				Type: "function",
				Function: &providers.FunctionCall{
					Name:      tc.Name,
					Arguments: string(argumentsJSON),
				},
				Name: tc.Name,
			})
		}
		messages = append(messages, assistantMsg)

		// Save assistant message with tool calls to session
		agent.Sessions.AddFullMessage(opts.SessionKey, assistantMsg)

		// Execute tool calls
		for _, tc := range response.ToolCalls {
			argsJSON, _ := json.Marshal(tc.Arguments)
			argsPreview := utils.Truncate(string(argsJSON), 200)
			logger.InfoCF("agent", fmt.Sprintf("Tool call: %s(%s)", tc.Name, argsPreview),
				map[string]interface{}{
					"agent_id":  agent.ID,
					"tool":      tc.Name,
					"iteration": iteration,
				})

			toolStartTime := time.Now()

			// Create async callback for tools that implement AsyncTool
			// NOTE: Following openclaw's design, async tools do NOT send results directly to users.
			// Instead, they notify the agent via PublishInbound, and the agent decides
			// whether to forward the result to the user (in processSystemMessage).
			asyncCallback := func(callbackCtx context.Context, result *tools.ToolResult) {
				// Log the async completion but don't send directly to user
				// The agent will handle user notification via processSystemMessage
				if !result.Silent && result.ForUser != "" {
					logger.InfoCF("agent", "Async tool completed, agent will handle notification",
						map[string]interface{}{
							"tool":        tc.Name,
							"content_len": len(result.ForUser),
						})
				}
			}

			toolResult := agent.Tools.ExecuteWithContext(ctx, tc.Name, tc.Arguments, opts.Channel, opts.ChatID, asyncCallback)
			toolDuration := time.Since(toolStartTime)

			// Record tool execution for local operations log
			if opts.RequestLogger != nil && opts.RequestLogger.IsEnabled() {
				op := Operation{
					Type:      "tool_call",
					Name:      tc.Name,
					Arguments: tc.Arguments,
					Status:    "Success",
					Duration:  toolDuration,
				}
				if toolResult.Err != nil {
					op.Status = "Failed"
					op.Error = toolResult.Err.Error()
				} else {
					op.Result = map[string]interface{}{
						"for_llm": toolResult.ForLLM,
					}
				}
				localOperations[iteration] = append(localOperations[iteration], op)
			}

			// Send ForUser content to user immediately if not Silent
			if !toolResult.Silent && toolResult.ForUser != "" && opts.SendResponse {
				al.bus.PublishOutbound(bus.OutboundMessage{
					Channel: opts.Channel,
					ChatID:  opts.ChatID,
					Content: toolResult.ForUser,
				})
				logger.DebugCF("agent", "Sent tool result to user",
					map[string]interface{}{
						"tool":        tc.Name,
						"content_len": len(toolResult.ForUser),
					})
			}

			// Determine content for LLM based on tool result
			contentForLLM := toolResult.ForLLM
			if contentForLLM == "" && toolResult.Err != nil {
				contentForLLM = toolResult.Err.Error()
			}

			toolResultMsg := providers.Message{
				Role:       "tool",
				Content:    contentForLLM,
				ToolCallID: tc.ID,
			}
			messages = append(messages, toolResultMsg)

			// Save tool result message to session
			agent.Sessions.AddFullMessage(opts.SessionKey, toolResultMsg)
		}

		// Log local operations for this round
		if opts.RequestLogger != nil && opts.RequestLogger.IsEnabled() && len(localOperations[iteration]) > 0 {
			opts.RequestLogger.LogLocalOperations(LocalOperationInfo{
				Round:      iteration,
				Timestamp:  time.Now(),
				Operations: localOperations[iteration],
			})
		}
	}

	return finalContent, iteration, nil
}

// updateToolContexts updates the context for tools that need channel/chatID info.
func (al *AgentLoop) updateToolContexts(agent *AgentInstance, channel, chatID string) {
	// Use ContextualTool interface instead of type assertions
	if tool, ok := agent.Tools.Get("message"); ok {
		if mt, ok := tool.(tools.ContextualTool); ok {
			mt.SetContext(channel, chatID)
		}
	}
	if tool, ok := agent.Tools.Get("spawn"); ok {
		if st, ok := tool.(tools.ContextualTool); ok {
			st.SetContext(channel, chatID)
		}
	}
	if tool, ok := agent.Tools.Get("subagent"); ok {
		if st, ok := tool.(tools.ContextualTool); ok {
			st.SetContext(channel, chatID)
		}
	}
}

// maybeSummarize triggers summarization if the session history exceeds thresholds.
func (al *AgentLoop) maybeSummarize(agent *AgentInstance, sessionKey, channel, chatID string) {
	newHistory := agent.Sessions.GetHistory(sessionKey)
	tokenEstimate := al.estimateTokens(newHistory)
	threshold := agent.ContextWindow * 75 / 100

	if len(newHistory) > 20 || tokenEstimate > threshold {
		summarizeKey := agent.ID + ":" + sessionKey
		if _, loading := al.summarizing.LoadOrStore(summarizeKey, true); !loading {
			go func() {
				defer al.summarizing.Delete(summarizeKey)
				if !constants.IsInternalChannel(channel) {
					al.bus.PublishOutbound(bus.OutboundMessage{
						Channel: channel,
						ChatID:  chatID,
						Content: "Memory threshold reached. Optimizing conversation history...",
					})
				}
				al.summarizeSession(agent, sessionKey)
			}()
		}
	}
}

// forceCompression aggressively reduces context when the limit is hit.
// It drops the oldest 50% of messages (keeping system prompt and last user message).
func (al *AgentLoop) forceCompression(agent *AgentInstance, sessionKey string) {
	history := agent.Sessions.GetHistory(sessionKey)
	if len(history) <= 4 {
		return
	}

	// Keep system prompt (usually [0]) and the very last message (user's trigger)
	// We want to drop the oldest half of the *conversation*
	// Assuming [0] is system, [1:] is conversation
	conversation := history[1 : len(history)-1]
	if len(conversation) == 0 {
		return
	}

	// Helper to find the mid-point of the conversation
	mid := len(conversation) / 2

	// New history structure:
	// 1. System Prompt
	// 2. [Summary of dropped part] - synthesized
	// 3. Second half of conversation
	// 4. Last message

	// Simplified approach for emergency: Drop first half of conversation
	// and rely on existing summary if present, or create a placeholder.

	droppedCount := mid
	keptConversation := conversation[mid:]

	newHistory := make([]providers.Message, 0)
	newHistory = append(newHistory, history[0]) // System prompt

	// Add a note about compression
	compressionNote := fmt.Sprintf("[System: Emergency compression dropped %d oldest messages due to context limit]", droppedCount)
	// If there was an existing summary, we might lose it if it was in the dropped part (which is just messages).
	// The summary is stored separately in session.Summary, so it persists!
	// We just need to ensure the user knows there's a gap.

	// We only modify the messages list here
	newHistory = append(newHistory, providers.Message{
		Role:    "system",
		Content: compressionNote,
	})

	newHistory = append(newHistory, keptConversation...)
	newHistory = append(newHistory, history[len(history)-1]) // Last message

	// Update session
	agent.Sessions.SetHistory(sessionKey, newHistory)
	agent.Sessions.Save(sessionKey)

	logger.WarnCF("agent", "Forced compression executed", map[string]interface{}{
		"session_key":  sessionKey,
		"dropped_msgs": droppedCount,
		"new_count":    len(newHistory),
	})
}

// GetStartupInfo returns information about loaded tools and skills for logging.
func (al *AgentLoop) GetStartupInfo() map[string]interface{} {
	info := make(map[string]interface{})

	agent := al.registry.GetDefaultAgent()
	if agent == nil {
		return info
	}

	// Tools info
	toolsList := agent.Tools.List()
	info["tools"] = map[string]interface{}{
		"count": len(toolsList),
		"names": toolsList,
	}

	// Skills info
	info["skills"] = agent.ContextBuilder.GetSkillsInfo()

	// Agents info
	info["agents"] = map[string]interface{}{
		"count": len(al.registry.ListAgentIDs()),
		"ids":   al.registry.ListAgentIDs(),
	}

	return info
}

// formatMessagesForLog formats messages for logging
func formatMessagesForLog(messages []providers.Message) string {
	if len(messages) == 0 {
		return "[]"
	}

	var result string
	result += "[\n"
	for i, msg := range messages {
		result += fmt.Sprintf("  [%d] Role: %s\n", i, msg.Role)
		if len(msg.ToolCalls) > 0 {
			result += "  ToolCalls:\n"
			for _, tc := range msg.ToolCalls {
				result += fmt.Sprintf("    - ID: %s, Type: %s, Name: %s\n", tc.ID, tc.Type, tc.Name)
				if tc.Function != nil {
					result += fmt.Sprintf("      Arguments: %s\n", utils.Truncate(tc.Function.Arguments, 200))
				}
			}
		}
		if msg.Content != "" {
			content := utils.Truncate(msg.Content, 200)
			result += fmt.Sprintf("  Content: %s\n", content)
		}
		if msg.ToolCallID != "" {
			result += fmt.Sprintf("  ToolCallID: %s\n", msg.ToolCallID)
		}
		result += "\n"
	}
	result += "]"
	return result
}

// formatToolsForLog formats tool definitions for logging
func formatToolsForLog(tools []providers.ToolDefinition) string {
	if len(tools) == 0 {
		return "[]"
	}

	var result string
	result += "[\n"
	for i, tool := range tools {
		result += fmt.Sprintf("  [%d] Type: %s, Name: %s\n", i, tool.Type, tool.Function.Name)
		result += fmt.Sprintf("      Description: %s\n", tool.Function.Description)
		if len(tool.Function.Parameters) > 0 {
			result += fmt.Sprintf("      Parameters: %s\n", utils.Truncate(fmt.Sprintf("%v", tool.Function.Parameters), 200))
		}
	}
	result += "]"
	return result
}

// summarizeSession summarizes the conversation history for a session.
func (al *AgentLoop) summarizeSession(agent *AgentInstance, sessionKey string) {
	ctx, cancel := context.WithTimeout(context.Background(), 120*time.Second)
	defer cancel()

	history := agent.Sessions.GetHistory(sessionKey)
	summary := agent.Sessions.GetSummary(sessionKey)

	// Keep last 4 messages for continuity
	if len(history) <= 4 {
		return
	}

	toSummarize := history[:len(history)-4]

	// Oversized Message Guard
	maxMessageTokens := agent.ContextWindow / 2
	validMessages := make([]providers.Message, 0)
	omitted := false

	for _, m := range toSummarize {
		if m.Role != "user" && m.Role != "assistant" {
			continue
		}
		msgTokens := len(m.Content) / 2
		if msgTokens > maxMessageTokens {
			omitted = true
			continue
		}
		validMessages = append(validMessages, m)
	}

	if len(validMessages) == 0 {
		return
	}

	// Multi-Part Summarization
	var finalSummary string
	if len(validMessages) > 10 {
		mid := len(validMessages) / 2
		part1 := validMessages[:mid]
		part2 := validMessages[mid:]

		s1, _ := al.summarizeBatch(ctx, agent, part1, "")
		s2, _ := al.summarizeBatch(ctx, agent, part2, "")

		mergePrompt := fmt.Sprintf("Merge these two conversation summaries into one cohesive summary:\n\n1: %s\n\n2: %s", s1, s2)
		resp, err := agent.Provider.Chat(ctx, []providers.Message{{Role: "user", Content: mergePrompt}}, nil, agent.Model, map[string]interface{}{
			"max_tokens":  1024,
			"temperature": 0.3,
		})
		if err == nil {
			finalSummary = resp.Content
		} else {
			finalSummary = s1 + " " + s2
		}
	} else {
		finalSummary, _ = al.summarizeBatch(ctx, agent, validMessages, summary)
	}

	if omitted && finalSummary != "" {
		finalSummary += "\n[Note: Some oversized messages were omitted from this summary for efficiency.]"
	}

	if finalSummary != "" {
		agent.Sessions.SetSummary(sessionKey, finalSummary)
		agent.Sessions.TruncateHistory(sessionKey, 4)
		agent.Sessions.Save(sessionKey)
	}
}

// summarizeBatch summarizes a batch of messages.
func (al *AgentLoop) summarizeBatch(ctx context.Context, agent *AgentInstance, batch []providers.Message, existingSummary string) (string, error) {
	prompt := "Provide a concise summary of this conversation segment, preserving core context and key points.\n"
	if existingSummary != "" {
		prompt += "Existing context: " + existingSummary + "\n"
	}
	prompt += "\nCONVERSATION:\n"
	for _, m := range batch {
		prompt += fmt.Sprintf("%s: %s\n", m.Role, m.Content)
	}

	response, err := agent.Provider.Chat(ctx, []providers.Message{{Role: "user", Content: prompt}}, nil, agent.Model, map[string]interface{}{
		"max_tokens":  1024,
		"temperature": 0.3,
	})
	if err != nil {
		return "", err
	}
	return response.Content, nil
}

// estimateTokens estimates the number of tokens in a message list.
// Uses a safe heuristic of 2.5 characters per token to account for CJK and other
// overheads better than the previous 3 chars/token.
func (al *AgentLoop) estimateTokens(messages []providers.Message) int {
	totalChars := 0
	for _, m := range messages {
		totalChars += utf8.RuneCountInString(m.Content)
	}
	// 2.5 chars per token = totalChars * 2 / 5
	return totalChars * 2 / 5
}

func (al *AgentLoop) handleCommand(ctx context.Context, msg bus.InboundMessage) (string, bool) {
	content := strings.TrimSpace(msg.Content)
	if !strings.HasPrefix(content, "/") {
		return "", false
	}

	parts := strings.Fields(content)
	if len(parts) == 0 {
		return "", false
	}

	cmd := parts[0]
	args := parts[1:]

	switch cmd {
	case "/show":
		if len(args) < 1 {
			return "Usage: /show [model|channel|agents]", true
		}
		switch args[0] {
		case "model":
			defaultAgent := al.registry.GetDefaultAgent()
			if defaultAgent == nil {
				return "No default agent configured", true
			}
			return fmt.Sprintf("Current model: %s", defaultAgent.Model), true
		case "channel":
			return fmt.Sprintf("Current channel: %s", msg.Channel), true
		case "agents":
			agentIDs := al.registry.ListAgentIDs()
			return fmt.Sprintf("Registered agents: %s", strings.Join(agentIDs, ", ")), true
		default:
			return fmt.Sprintf("Unknown show target: %s", args[0]), true
		}

	case "/list":
		if len(args) < 1 {
			return "Usage: /list [models|channels|agents]", true
		}
		switch args[0] {
		case "models":
			return "Available models: configured in config.json per agent", true
		case "channels":
			if al.channelManager == nil {
				return "Channel manager not initialized", true
			}
			channels := al.channelManager.GetEnabledChannels()
			if len(channels) == 0 {
				return "No channels enabled", true
			}
			return fmt.Sprintf("Enabled channels: %s", strings.Join(channels, ", ")), true
		case "agents":
			agentIDs := al.registry.ListAgentIDs()
			return fmt.Sprintf("Registered agents: %s", strings.Join(agentIDs, ", ")), true
		default:
			return fmt.Sprintf("Unknown list target: %s", args[0]), true
		}

	case "/switch":
		if len(args) < 3 || args[1] != "to" {
			return "Usage: /switch [model|channel] to <name>", true
		}
		target := args[0]
		value := args[2]

		switch target {
		case "model":
			defaultAgent := al.registry.GetDefaultAgent()
			if defaultAgent == nil {
				return "No default agent configured", true
			}
			oldModel := defaultAgent.Model
			defaultAgent.Model = value
			return fmt.Sprintf("Switched model from %s to %s", oldModel, value), true
		case "channel":
			if al.channelManager == nil {
				return "Channel manager not initialized", true
			}
			if _, exists := al.channelManager.GetChannel(value); !exists && value != "cli" {
				return fmt.Sprintf("Channel '%s' not found or not enabled", value), true
			}
			return fmt.Sprintf("Switched target channel to %s", value), true
		default:
			return fmt.Sprintf("Unknown switch target: %s", target), true
		}
	}

	return "", false
}

// extractPeer extracts the routing peer from inbound message metadata.
func extractPeer(msg bus.InboundMessage) *routing.RoutePeer {
	peerKind := msg.Metadata["peer_kind"]
	if peerKind == "" {
		return nil
	}
	peerID := msg.Metadata["peer_id"]
	if peerID == "" {
		if peerKind == "direct" {
			peerID = msg.SenderID
		} else {
			peerID = msg.ChatID
		}
	}
	return &routing.RoutePeer{Kind: peerKind, ID: peerID}
}

// extractParentPeer extracts the parent peer (reply-to) from inbound message metadata.
func extractParentPeer(msg bus.InboundMessage) *routing.RoutePeer {
	parentKind := msg.Metadata["parent_peer_kind"]
	parentID := msg.Metadata["parent_peer_id"]
	if parentKind == "" || parentID == "" {
		return nil
	}
	return &routing.RoutePeer{Kind: parentKind, ID: parentID}
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
	// Long timeout configuration: RPC Client (60min) > PeerChatHandler (59min) > RPCChannel (58min)
	cfg := &channels.RPCChannelConfig{
		MessageBus:      msgBus,
		RequestTimeout:  58 * time.Minute, // RPCChannel timeout
		CleanupInterval: 30 * time.Second,
	}

	// Create RPC channel
	rpcCh, err := channels.NewRPCChannel(cfg)
	if err != nil {
		return fmt.Errorf("failed to create RPC channel: %w", err)
	}

	// NOTE: Don't start RPC channel here!
	// It will be started by ChannelManager.StartAll() after registration
	// This prevents "RPC channel already running" error

	// Set RPC channel on cluster (triggers LLM handler registration)
	clusterInstance.SetRPCChannel(rpcCh)

	logger.InfoC("agent", "RPC channel for peer chat created and configured (will be started by ChannelManager)")

	return nil
}
