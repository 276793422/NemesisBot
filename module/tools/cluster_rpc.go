// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"time"

	"github.com/276793422/NemesisBot/module/cluster"
)

// ClusterRPCTool enables agents to make RPC calls to other bots in the cluster
type ClusterRPCTool struct {
	cluster       *cluster.Cluster
	originChannel string // Phase 2: 当前消息的通道
	originChatID  string // Phase 2: 当前消息的会话 ID
}

// NewClusterRPCTool creates a new cluster RPC tool
func NewClusterRPCTool(cluster *cluster.Cluster) *ClusterRPCTool {
	return &ClusterRPCTool{
		cluster: cluster,
	}
}

// SetContext 实现 ContextualTool 接口（Phase 2）
func (t *ClusterRPCTool) SetContext(channel, chatID string) {
	t.originChannel = channel
	t.originChatID = chatID
}

// Name returns the tool name
func (t *ClusterRPCTool) Name() string {
	return "cluster_rpc"
}

// Description returns the tool description
func (t *ClusterRPCTool) Description() string {
	return "Call other bots in the cluster via RPC. Parameters: peer_id (string, required), action (string, required), data (object, optional)"
}

// Parameters returns the tool parameters schema
func (t *ClusterRPCTool) Parameters() map[string]interface{} {
	return map[string]interface{}{
		"type": "object",
		"properties": map[string]interface{}{
			"peer_id": map[string]interface{}{
				"type":        "string",
				"description": "ID of the peer bot to call",
			},
			"action": map[string]interface{}{
				"type":        "string",
				"description": "RPC action to perform",
			},
			"data": map[string]interface{}{
				"type":        "object",
				"description": "Optional data payload for the RPC call",
			},
		},
		"required": []string{"peer_id", "action"},
	}
}

// Execute executes the cluster RPC tool
func (t *ClusterRPCTool) Execute(ctx context.Context, params map[string]interface{}) *ToolResult {
	// Extract parameters
	peerID, _ := params["peer_id"].(string)
	if peerID == "" {
		return ErrorResult("peer_id is required")
	}

	action, _ := params["action"].(string)
	if action == "" {
		return ErrorResult("action is required")
	}

	// Extract data (optional)
	var payload map[string]interface{}
	if data, ok := params["data"].(map[string]interface{}); ok {
		payload = data
	} else {
		payload = make(map[string]interface{})
	}

	// peer_chat 走异步路径
	if action == "peer_chat" {
		return t.executeAsyncPeerChat(ctx, peerID, payload)
	}

	// 同步路径（ping, get_capabilities 等）
	response, err := t.cluster.CallWithContext(ctx, peerID, action, payload)
	if err != nil {
		return ErrorResult(fmt.Sprintf("RPC call failed: %v", err))
	}

	return SilentResult(string(response))
}

// executeAsyncPeerChat 异步 peer_chat 路径（Phase 2: 非阻塞）
// 提交任务到 B 端，返回 AsyncResult（含 TaskID），由 AgentLoop 保存续行快照
func (t *ClusterRPCTool) executeAsyncPeerChat(ctx context.Context, peerID string, payload map[string]interface{}) *ToolResult {
	// 1. 注入 source 信息（本节点 ID、地址、RPC 端口）
	payload["_source"] = map[string]interface{}{
		"node_id":   t.cluster.GetNodeID(),
		"addresses": t.cluster.GetAllLocalIPs(),
		"rpc_port":  t.cluster.GetRPCPort(),
	}

	// 2. 生成 task_id 并注入到 payload
	taskID := fmt.Sprintf("task-%d", generateTaskTimestamp())
	payload["task_id"] = taskID

	// 3. 提交异步任务（传入 channel/chatID 供续行通知使用）
	submittedID, err := t.cluster.SubmitTask(ctx, peerID, "peer_chat", payload, t.originChannel, t.originChatID)
	if err != nil {
		return ErrorResult(fmt.Sprintf("Failed to submit task: %v", err))
	}

	// 4. 返回 AsyncResult（包含 taskID 供 AgentLoop 关联续行快照）
	result := AsyncResult(fmt.Sprintf("peer_chat 任务已提交到 %s (task_id: %s)，等待回调...",
		peerID, submittedID))
	result.TaskID = submittedID
	return result
}

// GetAvailablePeers returns information about available peers
func (t *ClusterRPCTool) GetAvailablePeers(ctx context.Context) (string, error) {
	peersIface := t.cluster.GetOnlinePeers()
	if len(peersIface) == 0 {
		return "No other bots currently online", nil
	}

	result := make([]map[string]interface{}, 0, len(peersIface))
	for _, peerIface := range peersIface {
		// Type assert to access node properties
		if peer, ok := peerIface.(interface {
			GetID() string
			GetName() string
			GetCapabilities() []string
			GetStatus() string
		}); ok {
			result = append(result, map[string]interface{}{
				"id":           peer.GetID(),
				"name":         peer.GetName(),
				"capabilities": peer.GetCapabilities(),
				"status":       peer.GetStatus(),
			})
		}
	}

	jsonData, err := json.MarshalIndent(result, "", "  ")
	if err != nil {
		return "", fmt.Errorf("failed to marshal peers: %w", err)
	}

	return string(jsonData), nil
}

// GetCapabilities returns all available capabilities in the cluster
func (t *ClusterRPCTool) GetCapabilities(ctx context.Context) (string, error) {
	caps := t.cluster.GetCapabilities()
	if len(caps) == 0 {
		return "No capabilities available", nil
	}

	jsonData, err := json.MarshalIndent(caps, "", "  ")
	if err != nil {
		return "", fmt.Errorf("failed to marshal capabilities: %w", err)
	}

	return string(jsonData), nil
}

// generateTaskTimestamp returns a nanosecond timestamp for task ID generation
func generateTaskTimestamp() int64 {
	return time.Now().UnixNano()
}
