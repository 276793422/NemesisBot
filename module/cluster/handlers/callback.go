// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package handlers

import "fmt"

// TaskCompleter 定义 TaskManager 的回调接口（避免循环依赖）
type TaskCompleter interface {
	CompleteCallback(taskID string, status string, response string, errMsg string) error
}

// RegisterCallbackHandler 注册 peer_chat_callback 处理器
// 当远程节点完成异步 LLM 处理后，通过此 action 回传结果
func RegisterCallbackHandler(logger Logger, taskManager TaskCompleter, registrar Registrar) {
	registrar("peer_chat_callback", func(payload map[string]interface{}) (map[string]interface{}, error) {
		// 1. 提取 task_id
		taskID, _ := payload["task_id"].(string)
		if taskID == "" {
			return map[string]interface{}{
				"status": "error",
				"error":  "task_id is required",
			}, nil
		}

		// 2. 提取 status
		status, _ := payload["status"].(string)
		if status == "" {
			status = "error"
		}

		// 3. 提取 response
		response, _ := payload["response"].(string)

		// 4. 提取 error
		errMsg, _ := payload["error"].(string)

		logger.LogRPCInfo("[Callback] Received callback for task_id=%s, status=%s", taskID, status)

		// 5. 调用 TaskManager 完成任务
		if err := taskManager.CompleteCallback(taskID, status, response, errMsg); err != nil {
			logger.LogRPCError("[Callback] Failed to complete task %s: %v", taskID, err)
			return map[string]interface{}{
				"status":  "error",
				"task_id": taskID,
				"error":   fmt.Sprintf("task not found: %s", taskID),
			}, nil
		}

		logger.LogRPCInfo("[Callback] Task %s completed successfully", taskID)

		// 6. 返回 ACK
		return map[string]interface{}{
			"status":  "received",
			"task_id": taskID,
		}, nil
	})

	logger.LogRPCInfo("Registered callback handler: peer_chat_callback")
}
