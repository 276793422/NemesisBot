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
	Type    string                 `json:"type"`     // chat|request|task|query
	Content string                 `json:"content"`   // 对话内容或任务描述
	Context map[string]interface{} `json:"context"`   // 附加上下文信息
}

// PeerChatResponse represents the response from peer_chat action
type PeerChatResponse struct {
	Response string                 `json:"response"` // 节点的响应内容
	Result   map[string]interface{} `json:"result,omitempty"` // 结构化结果
	Status   string                 `json:"status"`   // success|error|busy
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

// Handle handles a peer chat request
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

	// 4. Route based on type (all types currently go through LLM)
	return h.handleLLMRequest(payload, &req)
}

// handleLLMRequest processes LLM-based chat requests
func (h *PeerChatHandler) handleLLMRequest(rawPayload map[string]interface{}, req *PeerChatPayload) (map[string]interface{}, error) {
	h.cluster.LogRPCInfo("[PeerChat] Processing %s request", req.Type)
	h.cluster.LogRPCInfo("[PeerChat] Request content: %s", req.Content)

	// Check if rpcChannel is available
	if h.rpcChannel == nil {
		h.cluster.LogRPCError("[PeerChat] RPC channel is not available", nil)
		return h.errorResponse("error", "rpc channel not available"), nil
	}
	h.cluster.LogRPCInfo("[PeerChat] RPC channel is available", nil)

	// Extract chat_id and session_key from context
	chatID := "default"
	if req.Context != nil {
		if v, ok := req.Context["chat_id"].(string); ok {
			chatID = v
		}
	}

	// Determine sender ID with fallback priority:
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

	// Construct session key as "cluster_rpc:{sender_id}"
	sessionKey := fmt.Sprintf("cluster_rpc:%s", senderID)

	// Log session key for verification
	h.cluster.LogRPCInfo("[PeerChat] Using session_key=%s for sender=%s", sessionKey, senderID)

	// Construct InboundMessage
	inbound := &bus.InboundMessage{
		Channel:    "rpc",
		ChatID:     chatID,
		Content:    req.Content,
		SenderID:   senderID,
		SessionKey: sessionKey,
	}

	// Set correlation ID for tracking
	correlationID := fmt.Sprintf("peer-chat-%d", time.Now().UnixNano())
	inbound.CorrelationID = correlationID

	h.cluster.LogRPCInfo("[PeerChat] Created inbound message: chat_id=%s, correlation_id=%s", chatID, correlationID)
	h.cluster.LogRPCDebug("[PeerChat] Inbound message details: channel=%s, sender=%s, session=%s",
		inbound.Channel, inbound.SenderID, inbound.SessionKey)

	// Send to RPCChannel
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()

	respCh, err := h.rpcChannel.Input(ctx, inbound)
	if err != nil {
		h.cluster.LogRPCError("[PeerChat] Failed to send to RPC channel: %v", err)
		return h.errorResponse("error", "failed to process: "+err.Error()), nil
	}

	h.cluster.LogRPCInfo("[PeerChat] Request sent to MessageBus, waiting for LLM response (correlation_id=%s)", correlationID)

	// Wait for response
	select {
	case response := <-respCh:
		h.cluster.LogRPCInfo("[PeerChat] Response received! correlation_id=%s, response=%s", correlationID, response)
		return h.successResponse(response, nil), nil

	case <-ctx.Done():
		h.cluster.LogRPCError("[PeerChat] Timeout waiting for response (correlation_id=%s)", correlationID)
		return h.errorResponse("error", "timeout"), nil
	}
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

// successResponse creates a successful response
func (h *PeerChatHandler) successResponse(content string, result map[string]interface{}) map[string]interface{} {
	response := map[string]interface{}{
		"status":   "success",
		"response": content,
	}
	if result != nil {
		response["result"] = result
	}
	return response
}

// errorResponse creates an error response
func (h *PeerChatHandler) errorResponse(status, errMsg string) map[string]interface{} {
	return map[string]interface{}{
		"status":   status,
		"response": errMsg,
	}
}
