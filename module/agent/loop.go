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

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/channels"
	"github.com/276793422/NemesisBot/module/cluster"
	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/constants"
	"github.com/276793422/NemesisBot/module/logger"
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

	// Phase 2: 续行快照（内存缓存，磁盘持久化由 Cluster.ContinuationStore 管理）
	continuations map[string]*continuationData // taskID → 续行数据
	contMu        sync.RWMutex
}

// sessionBusyState tracks the busy state and queue for a session
type sessionBusyState struct {
	mu          sync.Mutex
	busy        bool
	queueLength int
}

// continuationData 续行快照数据（内存中）
type continuationData struct {
	messages   []providers.Message // LLM 消息快照（到 assistant 的 tool_call 为止）
	toolCallID string              // 触发异步的 tool call ID
	channel    string              // 原始通道
	chatID     string              // 原始会话 ID
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
		continuations:  make(map[string]*continuationData),
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

// GetRegistry returns the agent registry.
func (al *AgentLoop) GetRegistry() *AgentRegistry {
	return al.registry
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
		// Phase 2: 拦截集群续行消息
		if strings.HasPrefix(msg.SenderID, "cluster_continuation:") {
			taskID := strings.TrimPrefix(msg.SenderID, "cluster_continuation:")
			go al.handleClusterContinuation(context.Background(), taskID)
			return "", "", nil
		}
		resp, err := al.processSystemMessage(ctx, msg)
		return "", resp, err
	}

	// Handle history requests (bypass LLM, read directly from session)
	if msg.Metadata != nil && msg.Metadata["request_type"] == "history" {
		al.handleHistoryRequest(msg)
		return "", "", nil
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

// handleHistoryRequest processes a history request by reading from the agent session
// and publishing the response directly via the bus (bypasses LLM).
func (al *AgentLoop) handleHistoryRequest(msg bus.InboundMessage) {
	// Parse request data from Content (JSON payload)
	var reqData struct {
		RequestID   string `json:"request_id"`
		Limit       int    `json:"limit,omitempty"`
		BeforeIndex *int   `json:"before_index,omitempty"`
	}
	if err := json.Unmarshal([]byte(msg.Content), &reqData); err != nil {
		logger.ErrorCF("agent", "Failed to parse history request", map[string]interface{}{
			"error": err.Error(),
		})
		al.publishHistoryResponse(msg.ChatID, "", nil, false, 0, 0)
		return
	}

	// Default limit
	limit := reqData.Limit
	if limit <= 0 {
		limit = 20
	}

	// Resolve agent and session key using routing
	agent := al.registry.GetDefaultAgent()
	sessionKey := routing.BuildAgentMainSessionKey(agent.ID)

	// Read history from session
	allMsgs := agent.Sessions.GetHistory(sessionKey)

	// Filter: only keep user and assistant messages
	type historyMessage struct {
		Role    string `json:"role"`
		Content string `json:"content"`
	}
	filtered := make([]historyMessage, 0, len(allMsgs))
	for _, m := range allMsgs {
		if m.Role == "user" || m.Role == "assistant" {
			filtered = append(filtered, historyMessage{
				Role:    m.Role,
				Content: m.Content,
			})
		}
	}

	totalCount := len(filtered)

	// Determine pagination boundaries
	end := totalCount
	if reqData.BeforeIndex != nil && *reqData.BeforeIndex >= 0 && *reqData.BeforeIndex < totalCount {
		end = *reqData.BeforeIndex
	}
	start := end - limit
	if start < 0 {
		start = 0
	}

	hasMore := start > 0
	oldestIndex := start

	var pageMessages []historyMessage
	if start < end {
		pageMessages = filtered[start:end]
	} else {
		pageMessages = []historyMessage{}
	}

	al.publishHistoryResponse(msg.ChatID, reqData.RequestID, pageMessages, hasMore, oldestIndex, totalCount)
}

// publishHistoryResponse builds and publishes a history response via the bus.
func (al *AgentLoop) publishHistoryResponse(chatID, requestID string, messages interface{}, hasMore bool, oldestIndex, totalCount int) {
	responseData := map[string]interface{}{
		"request_id":   requestID,
		"messages":     messages,
		"has_more":     hasMore,
		"oldest_index": oldestIndex,
		"total_count":  totalCount,
	}

	content, err := json.Marshal(responseData)
	if err != nil {
		logger.ErrorCF("agent", "Failed to marshal history response", map[string]interface{}{
			"error": err.Error(),
		})
		return
	}

	al.bus.PublishOutbound(bus.OutboundMessage{
		Channel: "web",
		ChatID:  chatID,
		Content: string(content),
		Type:    "history",
	})

	logger.DebugCF("agent", "History response published", map[string]interface{}{
		"chat_id":      chatID,
		"request_id":   requestID,
		"total_count":  totalCount,
		"has_more":     hasMore,
		"oldest_index": oldestIndex,
	})
}
