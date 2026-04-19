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

	// 2. 先获取任务结果（数据齐全后再删除快照）
	task, err := al.cluster.GetTask(taskID)
	if err != nil {
		logger.ErrorCF("agent", "Task not found for continuation",
			map[string]interface{}{"task_id": taskID})
		// 快照仍在，可重试
		return
	}

	// 3. 数据已齐全，安全清理快照（内存 + 磁盘）
	al.contMu.Lock()
	delete(al.continuations, taskID)
	al.contMu.Unlock()
	if al.cluster != nil {
		if store := al.cluster.GetContinuationStore(); store != nil {
			store.Delete(taskID)
		}
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
		// NOTE: Continuation executes all available tools, same as the normal LLM loop.
		// This is intentional — the continuation IS the LLM loop resuming. Stateful tools
		// (e.g., MessageTool, cluster_rpc) produce real side effects. This is NOT a bug.
		// If tool restriction is needed in the future, add a whitelist filter here.
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
		// 续行完成，清理任务记录
		if al.cluster != nil {
			al.cluster.CleanupTask(taskID)
		}
	}
}

// loadContinuation 加载续行快照（内存优先，磁盘回退）
// 使用 save barrier 模式：如果内存中存在条目但 ready 未 close，
// 等待 saveContinuation 完成数据填充（最多 5 秒），避免竞态条件。
// 磁盘路径不受 barrier 影响（进程重启后走磁盘恢复）。
func (al *AgentLoop) loadContinuation(taskID string) *continuationData {
	// 尝试从内存加载（带 save barrier 等待）
	if data := al.waitForContinuation(taskID); data != nil {
		return data
	}

	// 内存中没有，尝试磁盘（AgentLoop 重启后的恢复路径）
	return al.tryLoadFromDisk(taskID)
}

// waitForContinuation 等待续行快照就绪（save barrier 实现）
// 如果内存中已有条目，等待其 ready channel close（最多 5 秒）。
// 如果没有条目，短暂重试等待 saveContinuation 注册条目。
func (al *AgentLoop) waitForContinuation(taskID string) *continuationData {
	// 最多等待 5 秒，分多次检查
	// 场景：SubmitTask 返回后 → AgentLoop 正在从 ExecuteWithContext 返回 → 还没到 saveContinuation
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		al.contMu.RLock()
		data, exists := al.continuations[taskID]
		al.contMu.RUnlock()

		if !exists {
			// 快照还没注册，短暂等待后重试
			time.Sleep(10 * time.Millisecond)
			continue
		}

		// 条目已存在，等待 ready channel close（saveContinuation 填充数据并 close）
		select {
		case <-data.ready:
			// 数据已就绪，返回
			return data
		case <-time.After(time.Until(deadline)):
			// 超时，ready 未 close（异常情况：saveContinuation 卡在磁盘写入）
			logger.WarnCF("agent", "Continuation ready timeout, falling back to disk",
				map[string]interface{}{"task_id": taskID})
			return nil
		}
	}

	// 5 秒内未找到条目
	return nil
}

// tryLoadFromDisk 从磁盘加载续行快照（进程重启后的恢复路径）
func (al *AgentLoop) tryLoadFromDisk(taskID string) *continuationData {
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
// 使用 save barrier 模式：先注册 ready channel，数据填充后 close，
// 确保 loadContinuation 能等待数据就绪而不需要轮询重试。
func (al *AgentLoop) saveContinuation(taskID string, messages []providers.Message,
	toolCallID, channel, chatID string) {
	snapshot := make([]providers.Message, len(messages))
	copy(snapshot, messages)

	contData := &continuationData{
		messages:   snapshot,
		toolCallID: toolCallID,
		channel:    channel,
		chatID:     chatID,
		ready:      make(chan struct{}),
	}

	// 写入内存（此时 ready 未 close，loadContinuation 会等待）
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

	// 数据已就绪，close ready channel（解除 loadContinuation 的等待）
	close(contData.ready)

	logger.InfoCF("agent", "Continuation snapshot saved (memory + disk)",
		map[string]interface{}{"task_id": taskID})
}
