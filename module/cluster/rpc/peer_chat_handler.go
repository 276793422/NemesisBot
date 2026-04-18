// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package rpc

import (
	"context"
	"fmt"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/channels"
)

// PeerChatPayload represents the payload for peer_chat action
type PeerChatPayload struct {
	Type    string                 `json:"type"`    // chat|request|task|query
	Content string                 `json:"content"` // 对话内容或任务描述
	Context map[string]interface{} `json:"context"` // 附加上下文信息
}

// PeerChatResponse represents the response from peer_chat action
type PeerChatResponse struct {
	Response string                 `json:"response"`         // 节点的响应内容
	Result   map[string]interface{} `json:"result,omitempty"` // 结构化结果
	Status   string                 `json:"status"`           // success|error|busy
}

// PeerChatHandler handles peer-to-peer chat and collaboration requests
type PeerChatHandler struct {
	cluster    Cluster
	rpcChannel *channels.RPCChannel
}

// NewPeerChatHandler creates a new peer chat handler
func NewPeerChatHandler(cluster Cluster, rpcChannel *channels.RPCChannel) *PeerChatHandler {
	return &PeerChatHandler{
		cluster:    cluster,
		rpcChannel: rpcChannel,
	}
}

// Handle handles a peer chat request — 立即返回 ACK，异步处理 LLM
func (h *PeerChatHandler) Handle(payload map[string]interface{}) (map[string]interface{}, error) {
	h.cluster.LogRPCInfo("[PeerChat] Received request, type=%s", payload["type"])

	// 1. Parse payload
	var req PeerChatPayload
	if err := h.parsePayload(payload, &req); err != nil {
		h.cluster.LogRPCError("[PeerChat] Invalid payload: %v", err)
		return h.errorResponse("error", "invalid payload: "+err.Error()), nil
	}

	// 2. Validate required fields
	if req.Content == "" {
		h.cluster.LogRPCError("[PeerChat] Missing required field: content", nil)
		return h.errorResponse("error", "content is required"), nil
	}

	// 3. Validate and set default type
	if req.Type == "" {
		req.Type = "request" // Default to request type
	}

	// 4. 提取 task_id（由 A 端注入）
	taskID, _ := payload["task_id"].(string)
	if taskID == "" {
		taskID = fmt.Sprintf("auto-%d", time.Now().UnixNano())
	}

	// 5. 提取 source 信息（用于回调）
	sourceInfo, _ := payload["_source"].(map[string]interface{})

	// 6. 启动异步处理
	go h.processAsync(payload, &req, taskID, sourceInfo)

	// 7. 立即返回 ACK
	h.cluster.LogRPCInfo("[PeerChat] Task %s accepted, processing asynchronously", taskID)
	return map[string]interface{}{
		"status":  "accepted",
		"task_id": taskID,
	}, nil
}

// processAsync 异步处理 LLM 请求
func (h *PeerChatHandler) processAsync(rawPayload map[string]interface{}, req *PeerChatPayload, taskID string, sourceInfo map[string]interface{}) {
	h.cluster.LogRPCInfo("[PeerChat] Async processing started for task %s", taskID)

	// 1. Check if rpcChannel is available
	if h.rpcChannel == nil {
		h.cluster.LogRPCError("[PeerChat] RPC channel is not available", nil)
		h.sendCallback(sourceInfo, taskID, "error", "", "rpc channel not available")
		return
	}

	// 2. Extract chat_id and session_key from context
	chatID := "default"
	if req.Context != nil {
		if v, ok := req.Context["chat_id"].(string); ok {
			chatID = v
		}
	}

	// 3. Determine sender ID with fallback priority:
	// 1. _rpc.from (injected by server)
	// 2. context.sender_id (legacy)
	// 3. "remote-peer" (default)
	senderID := "remote-peer"
	if rpcMeta, ok := rawPayload["_rpc"].(map[string]interface{}); ok {
		if from, ok := rpcMeta["from"].(string); ok && from != "" {
			senderID = from
		}
	}
	if senderID == "remote-peer" && req.Context != nil {
		if v, ok := req.Context["sender_id"].(string); ok && v != "" {
			senderID = v
		}
	}

	// 4. Construct session key as "cluster_rpc:{sender_id}"
	sessionKey := fmt.Sprintf("cluster_rpc:%s", senderID)
	h.cluster.LogRPCInfo("[PeerChat] Using session_key=%s for sender=%s", sessionKey, senderID)

	// 5. Construct InboundMessage
	correlationID := fmt.Sprintf("peer-chat-%d", time.Now().UnixNano())
	inbound := &bus.InboundMessage{
		Channel:       "rpc",
		ChatID:        chatID,
		Content:       req.Content,
		SenderID:      senderID,
		SessionKey:    sessionKey,
		CorrelationID: correlationID,
	}

	h.cluster.LogRPCInfo("[PeerChat] Created inbound message: chat_id=%s, correlation_id=%s", chatID, correlationID)

	// 6. Send to RPCChannel (no timeout — wait for LLM to complete)
	ctx := context.Background()
	respCh, err := h.rpcChannel.Input(ctx, inbound)
	if err != nil {
		h.cluster.LogRPCError("[PeerChat] Failed to send to RPC channel: %v", err)
		h.sendCallback(sourceInfo, taskID, "error", "", "failed to process: "+err.Error())
		return
	}

	h.cluster.LogRPCInfo("[PeerChat] Request sent to MessageBus, waiting for LLM response (correlation_id=%s)", correlationID)

	// 7. Wait for response with timeout (59min = RPCChannel 58min cleanup + 1min margin)
	waitCtx, waitCancel := context.WithTimeout(context.Background(), 59*time.Minute)
	defer waitCancel()

	select {
	case response, ok := <-respCh:
		if ok {
			h.cluster.LogRPCInfo("[PeerChat] LLM response received for task %s, correlation_id=%s", taskID, correlationID)
			h.sendCallback(sourceInfo, taskID, "success", response, "")
		} else {
			h.cluster.LogRPCError("[PeerChat] Response channel closed for task %s (channel stopped)", taskID)
			h.sendCallback(sourceInfo, taskID, "error", "", "response channel closed")
		}
	case <-waitCtx.Done():
		h.cluster.LogRPCError("[PeerChat] LLM processing timeout for task %s (59min)", taskID)
		h.sendCallback(sourceInfo, taskID, "error", "", "LLM processing timeout")
	}
}

// sendCallback 回调源节点
func (h *PeerChatHandler) sendCallback(sourceInfo map[string]interface{}, taskID, status, response, errMsg string) {
	// 提取 source 节点信息
	sourceNodeID, _ := sourceInfo["node_id"].(string)
	if sourceNodeID == "" {
		h.cluster.LogRPCError("[PeerChat] No source node_id in task %s, cannot callback", taskID)
		return
	}

	h.cluster.LogRPCInfo("[PeerChat] Calling back to source node %s for task %s", sourceNodeID, taskID)

	// 构造回调 payload
	payload := map[string]interface{}{
		"task_id":  taskID,
		"status":   status,
		"response": response,
	}
	if errMsg != "" {
		payload["error"] = errMsg
	}

	// 通过 RPC client 发送回调（3 次重试，覆盖短暂网络故障）
	maxRetries := 3
	for attempt := 0; attempt < maxRetries; attempt++ {
		ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
		_, err := h.cluster.CallWithContext(ctx, sourceNodeID, "peer_chat_callback", payload)
		cancel()
		if err == nil {
			h.cluster.LogRPCInfo("[PeerChat] Callback sent successfully for task %s to node %s", taskID, sourceNodeID)
			return
		}
		h.cluster.LogRPCError("[PeerChat] Callback attempt %d/%d failed for task %s to node %s: %v",
			attempt+1, maxRetries, taskID, sourceNodeID, err)
		if attempt < maxRetries-1 {
			backoff := time.Duration(attempt+1) * 5 * time.Second
			time.Sleep(backoff)
		}
	}
	h.cluster.LogRPCError("[PeerChat] All callback retries exhausted for task %s to node %s", taskID, sourceNodeID)
}

// parsePayload parses the incoming payload into PeerChatPayload
func (h *PeerChatHandler) parsePayload(payload map[string]interface{}, req *PeerChatPayload) error {
	// Parse Type
	if v, ok := payload["type"].(string); ok {
		req.Type = v
	}

	// Parse Content
	if v, ok := payload["content"].(string); ok {
		req.Content = v
	}

	// Parse Context
	if v, ok := payload["context"].(map[string]interface{}); ok {
		req.Context = v
	}

	return nil
}

// errorResponse creates an error response
func (h *PeerChatHandler) errorResponse(status, errMsg string) map[string]interface{} {
	return map[string]interface{}{
		"status":   status,
		"response": errMsg,
	}
}
