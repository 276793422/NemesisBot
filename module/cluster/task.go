// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package cluster

import "time"

// TaskStatus 任务状态
type TaskStatus string

const (
	TaskPending   TaskStatus = "pending"   // 已提交，等待 B 处理
	TaskCompleted TaskStatus = "completed" // B 已返回结果
	TaskFailed    TaskStatus = "failed"    // B 处理失败或回调失败
	TaskCancelled TaskStatus = "cancelled" // 主动取消（Phase 2）
)

// Task 表示一个异步 RPC 任务
type Task struct {
	ID          string                 `json:"id"`          // 唯一任务 ID
	Action      string                 `json:"action"`      // RPC action (peer_chat)
	PeerID      string                 `json:"peer_id"`     // 目标节点 ID
	Payload     map[string]interface{} `json:"payload"`     // 原始请求 payload
	Status      TaskStatus             `json:"status"`      // 当前状态
	CreatedAt   time.Time              `json:"created_at"`  // 创建时间
	CompletedAt *time.Time             `json:"completed_at"` // 完成时间

	// 结果
	Response string                 `json:"response,omitempty"` // LLM 回复内容
	Result   map[string]interface{} `json:"result,omitempty"`   // 回调结果
	Error    string                 `json:"error,omitempty"`    // 错误信息

	// Phase 2: 原始通道信息（用于续行通知路由）
	OriginalChannel string `json:"original_channel,omitempty"` // 发起方的通道（如 "web"）
	OriginalChatID  string `json:"original_chat_id,omitempty"`  // 发起方的会话 ID
}

// TaskResult 任务回调结果
type TaskResult struct {
	TaskID   string                 `json:"task_id"`
	Status   string                 `json:"status"`            // success | error
	Response string                 `json:"response"`
	Result   map[string]interface{} `json:"result,omitempty"`
	Error    string                 `json:"error,omitempty"`
}
