// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package agent

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/constants"
	"github.com/276793422/NemesisBot/module/logger"
	"github.com/276793422/NemesisBot/module/observer"
	"github.com/276793422/NemesisBot/module/providers"
	"github.com/276793422/NemesisBot/module/tools"
	"github.com/276793422/NemesisBot/module/utils"
)

// runAgentLoop is the core message processing logic.
func (al *AgentLoop) runAgentLoop(ctx context.Context, agent *AgentInstance, opts processOptions) (string, error) {
	// Phase 5: Generate TraceID and emit conversation_start event
	traceID := fmt.Sprintf("%s-%d", opts.SessionKey, time.Now().UnixNano())
	opts.TraceID = traceID
	conversationStartTime := time.Now()

	if al.observerMgr != nil && al.observerMgr.HasObservers() {
		al.observerMgr.EmitSync(ctx, observer.ConversationEvent{
			Type:      observer.EventConversationStart,
			TraceID:   traceID,
			Timestamp: time.Now(),
			Data: &observer.ConversationStartData{
				SessionKey: opts.SessionKey,
				Channel:    opts.Channel,
				ChatID:     opts.ChatID,
				SenderID:   "user",
				Content:    opts.UserMessage,
			},
		})
	}

	// Legacy: Initialize request logger if enabled (backward compat when no observer)
	var reqLogger *RequestLogger
	if al.observerMgr == nil || !al.observerMgr.HasObservers() {
		if al.cfg.Logging != nil && al.cfg.Logging.LLM != nil && al.cfg.Logging.LLM.Enabled {
			workspace := al.cfg.WorkspacePath()
			reqLogger = NewRequestLogger(al.cfg.Logging, workspace)
			if reqLogger.IsEnabled() {
				if err := reqLogger.CreateSession(); err != nil {
					logger.WarnC("request_logger", fmt.Sprintf("Failed to create logging session: %v", err))
				} else {
					reqLogger.LogUserRequest(UserRequestInfo{
						Timestamp: time.Now(),
						Channel:   opts.Channel,
						SenderID:  "user",
						ChatID:    opts.ChatID,
						Content:   opts.UserMessage,
					})
				}
			}
		}
		opts.RequestLogger = reqLogger
	}

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
		// Emit conversation_end event (error path)
		if al.observerMgr != nil && al.observerMgr.HasObservers() {
			al.observerMgr.EmitSync(ctx, observer.ConversationEvent{
				Type:      observer.EventConversationEnd,
				TraceID:   opts.TraceID,
				Timestamp: time.Now(),
				Data: &observer.ConversationEndData{
					SessionKey:    opts.SessionKey,
					Channel:       opts.Channel,
					ChatID:        opts.ChatID,
					TotalRounds:   iteration,
					TotalDuration: time.Since(conversationStartTime),
					Content:       fmt.Sprintf("Error: %s", err.Error()),
					Error:         err,
				},
			})
		}
		// Legacy: close logger on error
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
	if al.observerMgr != nil && al.observerMgr.HasObservers() {
		al.observerMgr.EmitSync(ctx, observer.ConversationEvent{
			Type:      observer.EventConversationEnd,
			TraceID:   opts.TraceID,
			Timestamp: time.Now(),
			Data: &observer.ConversationEndData{
				SessionKey:    opts.SessionKey,
				Channel:       opts.Channel,
				ChatID:        opts.ChatID,
				TotalRounds:   iteration,
				TotalDuration: time.Since(conversationStartTime),
				Content:       finalContent,
			},
		})
	}
	// Legacy: close logger when no observer manager
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
		if al.observerMgr != nil && al.observerMgr.HasObservers() {
			al.observerMgr.Emit(ctx, observer.ConversationEvent{
				Type:      observer.EventLLMRequest,
				TraceID:   opts.TraceID,
				Timestamp: time.Now(),
				Data: &observer.LLMRequestData{
					Round:        iteration,
					Model:        agent.Model,
					ProviderName: agent.ProviderMeta.Name,
					APIKey:       agent.ProviderMeta.APIKey,
					APIBase:      agent.ProviderMeta.APIBase,
					HTTPHeaders:  httpHeaders,
					FullConfig:   fullConfig,
					Messages:     messages,
					Tools:        providerToolDefs,
				},
			})
		} else if opts.RequestLogger != nil && opts.RequestLogger.IsEnabled() {
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
		if al.observerMgr != nil && al.observerMgr.HasObservers() {
			duration := time.Since(roundStartTime)
			al.observerMgr.Emit(ctx, observer.ConversationEvent{
				Type:      observer.EventLLMResponse,
				TraceID:   opts.TraceID,
				Timestamp: time.Now(),
				Data: &observer.LLMResponseData{
					Round:        iteration,
					Duration:     duration,
					Content:      response.Content,
					ToolCalls:    response.ToolCalls,
					Usage:        response.Usage,
					FinishReason: response.FinishReason,
				},
			})
		} else if opts.RequestLogger != nil && opts.RequestLogger.IsEnabled() {
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
		for chainPos, tc := range response.ToolCalls {
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

			// Record tool execution via observer or legacy logger
			if al.observerMgr != nil && al.observerMgr.HasObservers() {
				errMsg := ""
				if toolResult.Err != nil {
					errMsg = toolResult.Err.Error()
				}
				al.observerMgr.Emit(ctx, observer.ConversationEvent{
					Type:      observer.EventToolCall,
					TraceID:   opts.TraceID,
					Timestamp: time.Now(),
					Data: &observer.ToolCallData{
						ToolName:  tc.Name,
						Arguments: tc.Arguments,
						Success:   toolResult.Err == nil,
						Duration:  toolDuration,
						Error:     errMsg,
						LLMRound:  iteration,
						ChainPos:  chainPos,
					},
				})
			} else if opts.RequestLogger != nil && opts.RequestLogger.IsEnabled() {
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

			// Phase 2: 保存续行快照（异步工具返回时，保存 LLM 上下文供后续续行）
			if toolResult.Async && toolResult.TaskID != "" {
				al.saveContinuation(toolResult.TaskID, messages, tc.ID,
					opts.Channel, opts.ChatID)
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

		// Log local operations for this round (legacy path only)
		if al.observerMgr == nil || !al.observerMgr.HasObservers() {
			if opts.RequestLogger != nil && opts.RequestLogger.IsEnabled() && len(localOperations[iteration]) > 0 {
				opts.RequestLogger.LogLocalOperations(LocalOperationInfo{
					Round:      iteration,
					Timestamp:  time.Now(),
					Operations: localOperations[iteration],
				})
			}
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
	if tool, ok := agent.Tools.Get("cluster_rpc"); ok {
		if ct, ok := tool.(tools.ContextualTool); ok {
			ct.SetContext(channel, chatID)
		}
	}
}
