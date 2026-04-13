// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package agent

import (
	"context"
	"encoding/json"
	"fmt"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/cluster"
	"github.com/276793422/NemesisBot/module/logger"
	"github.com/276793422/NemesisBot/module/providers"
)

// handleClusterContinuation 处理集群续行（Phase 2）
// 回调到达后，从快照恢复 LLM 上下文，追加真实工具结果，续行 LLM 调用
func (al *AgentLoop) handleClusterContinuation(ctx context.Context, taskID string) {
	// 1. 获取续行快照（先查内存，再查磁盘）
	contData := al.loadContinuation(taskID)
	if contData == nil {
		logger.WarnCF("agent", "Continuation data not found",
			map[string]interface{}{"task_id": taskID})
		return
	}

	// 2. 清理快照（内存 + 磁盘）
	al.contMu.Lock()
	delete(al.continuations, taskID)
	al.contMu.Unlock()
	if al.cluster != nil {
		if store := al.cluster.GetContinuationStore(); store != nil {
			store.Delete(taskID)
		}
	}

	// 3. 从 Cluster 获取回调结果
	task, err := al.cluster.GetTask(taskID)
	if err != nil {
		logger.ErrorCF("agent", "Task not found for continuation",
			map[string]interface{}{"task_id": taskID})
		return
	}

	// 4. 构建续行消息：快照 + 真实工具结果
	messages := make([]providers.Message, len(contData.messages))
	copy(messages, contData.messages)

	toolResultContent := task.Response
	if task.Status == cluster.TaskFailed {
		toolResultContent = fmt.Sprintf("Error: %s", task.Error)
	}
	messages = append(messages, providers.Message{
		Role:       "tool",
		Content:    toolResultContent,
		ToolCallID: contData.toolCallID,
	})

	// 5. 获取 Agent 并更新工具上下文
	agent := al.registry.GetDefaultAgent()
	al.updateToolContexts(agent, contData.channel, contData.chatID)

	// 6. 续行 LLM + 工具执行循环
	finalContent := ""
	maxIterations := 20
	providerToolDefs := agent.Tools.ToProviderDefs()

	for i := 0; i < maxIterations; i++ {
		response, err := agent.Provider.Chat(ctx, messages, providerToolDefs, agent.Model,
			map[string]interface{}{"max_tokens": 8192, "temperature": 0.7})
		if err != nil {
			logger.ErrorCF("agent", "Continuation LLM call failed",
				map[string]interface{}{"error": err.Error()})
			break
		}

		if len(response.ToolCalls) == 0 {
			finalContent = response.Content
			break
		}

		// 有工具调用 → 构建 assistant 消息
		assistantMsg := providers.Message{Role: "assistant", Content: response.Content}
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

		// 执行工具调用
		for _, tc := range response.ToolCalls {
			toolResult := agent.Tools.ExecuteWithContext(ctx, tc.Name, tc.Arguments,
				contData.channel, contData.chatID, nil)

			if !toolResult.Silent && toolResult.ForUser != "" {
				al.bus.PublishOutbound(bus.OutboundMessage{
					Channel: contData.channel,
					ChatID:  contData.chatID,
					Content: toolResult.ForUser,
				})
			}

			// 嵌套异步 → 保存新快照（内存 + 磁盘）
			if toolResult.Async && toolResult.TaskID != "" {
				al.saveContinuation(toolResult.TaskID, messages, tc.ID,
					contData.channel, contData.chatID)
			}

			contentForLLM := toolResult.ForLLM
			if contentForLLM == "" && toolResult.Err != nil {
				contentForLLM = toolResult.Err.Error()
			}
			messages = append(messages, providers.Message{
				Role: "tool", Content: contentForLLM, ToolCallID: tc.ID,
			})
		}
	}

	// 7. 发送最终响应给用户
	if finalContent != "" {
		al.bus.PublishOutbound(bus.OutboundMessage{
			Channel: contData.channel,
			ChatID:  contData.chatID,
			Content: finalContent,
		})
		logger.InfoCF("agent", "Continuation response sent",
			map[string]interface{}{
				"task_id":        taskID,
				"content_len":    len(finalContent),
				"target_channel": contData.channel,
			})
	}
}

// loadContinuation 加载续行快照（内存优先，磁盘回退）
func (al *AgentLoop) loadContinuation(taskID string) *continuationData {
	// 先查内存
	al.contMu.RLock()
	data, exists := al.continuations[taskID]
	al.contMu.RUnlock()
	if exists {
		return data
	}

	// 再查磁盘（AgentLoop 重启后的恢复路径）
	if al.cluster == nil {
		return nil
	}
	store := al.cluster.GetContinuationStore()
	if store == nil {
		return nil
	}
	snapshot, err := store.Load(taskID)
	if err != nil {
		return nil
	}
	var messages []providers.Message
	if err := json.Unmarshal(snapshot.Messages, &messages); err != nil {
		return nil
	}
	return &continuationData{
		messages:   messages,
		toolCallID: snapshot.ToolCallID,
		channel:    snapshot.Channel,
		chatID:     snapshot.ChatID,
	}
}

// saveContinuation 保存续行快照（内存 + 磁盘双写）
func (al *AgentLoop) saveContinuation(taskID string, messages []providers.Message,
	toolCallID, channel, chatID string) {
	snapshot := make([]providers.Message, len(messages))
	copy(snapshot, messages)

	contData := &continuationData{
		messages:   snapshot,
		toolCallID: toolCallID,
		channel:    channel,
		chatID:     chatID,
	}

	// 写入内存
	al.contMu.Lock()
	al.continuations[taskID] = contData
	al.contMu.Unlock()

	// 写入磁盘
	if al.cluster != nil {
		if store := al.cluster.GetContinuationStore(); store != nil {
			messagesJSON, _ := json.Marshal(snapshot)
			store.Save(&cluster.ContinuationSnapshot{
				TaskID:     taskID,
				Messages:   messagesJSON,
				ToolCallID: toolCallID,
				Channel:    channel,
				ChatID:     chatID,
				CreatedAt:  time.Now(),
			})
		}
	}

	logger.InfoCF("agent", "Continuation snapshot saved (memory + disk)",
		map[string]interface{}{"task_id": taskID})
}
