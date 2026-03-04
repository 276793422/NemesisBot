// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package rpc

import (
	"context"
	"encoding/json"
	"fmt"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/channels"
)

// LLMForwardPayload represents the payload for llm_forward action
type LLMForwardPayload struct {
	Channel    string            `json:"channel"`     // Target channel (for future use)
	ChatID     string            `json:"chat_id"`     // Target chat ID
	Content    string            `json:"content"`     // User message content
	SenderID   string            `json:"sender_id"`   // Sender ID (optional)
	SessionKey string            `json:"session_key"` // Session key (optional)
	Metadata   map[string]string `json:"metadata"`    // Additional metadata
}

// LLMForwardResponse represents the response from llm_forward action
type LLMForwardResponse struct {
	Success bool   `json:"success"` // Whether the call succeeded
	Content string `json:"content"` // LLM response content
	Error   string `json:"error,omitempty"` // Error message if failed
}

// LLMForwardHandler handles RPC requests to forward messages to the local LLM
type LLMForwardHandler struct {
	cluster    Cluster
	rpcChannel *channels.RPCChannel
}

// NewLLMForwardHandler creates a new LLM forward handler
func NewLLMForwardHandler(cluster Cluster, rpcChannel *channels.RPCChannel) *LLMForwardHandler {
	return &LLMForwardHandler{
		cluster:    cluster,
		rpcChannel: rpcChannel,
	}
}

// Handle handles an LLM forward request
// This is called by the RPC server when it receives an "llm_forward" action
func (h *LLMForwardHandler) Handle(payload map[string]interface{}) (map[string]interface{}, error) {
	h.cluster.LogRPCInfo("[LLMForward] Received request", nil)

	// 1. Parse payload
	var req LLMForwardPayload
	if err := h.parsePayload(payload, &req); err != nil {
		h.cluster.LogRPCError("[LLMForward] Invalid payload: %v", err)
		return h.errorResponse("invalid payload: " + err.Error()), nil
	}

	// Validate required fields
	if req.ChatID == "" {
		h.cluster.LogRPCError("[LLMForward] Missing required field: chat_id", nil)
		return h.errorResponse("chat_id is required"), nil
	}

	if req.Content == "" {
		h.cluster.LogRPCError("[LLMForward] Missing required field: content", nil)
		return h.errorResponse("content is required"), nil
	}

	// 2. Construct InboundMessage
	inbound := bus.InboundMessage{
		Channel:    "rpc", // Use RPC channel
		ChatID:     req.ChatID,
		Content:    req.Content,
		SenderID:   req.SenderID,
		SessionKey: req.SessionKey,
		Metadata:   req.Metadata,
	}

	// If a specific channel is requested, pass it via metadata
	// (This allows the bot to use different channel-specific logic if needed)
	if req.Channel != "" && req.Channel != "rpc" {
		if inbound.Metadata == nil {
			inbound.Metadata = make(map[string]string)
		}
		inbound.Metadata["target_channel"] = req.Channel
	}

	// 3. Send to RPCChannel and wait for response
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()

	respCh, err := h.rpcChannel.Input(ctx, &inbound)
	if err != nil {
		h.cluster.LogRPCError("[LLMForward] Failed to send to RPC channel: %v", err)
		return h.errorResponse("failed to process: " + err.Error()), nil
	}

	h.cluster.LogRPCInfo("[LLMForward] Waiting for LLM response", map[string]interface{}{
		"correlation_id": inbound.CorrelationID,
		"chat_id":        req.ChatID,
		"content_len":    len(req.Content),
	})

	// 4. Wait for response
	select {
	case response, ok := <-respCh:
		// Check if channel was closed (timeout)
		if !ok {
			h.cluster.LogRPCError("[LLMForward] Timeout waiting for LLM response", nil)
			return h.errorResponse("LLM processing timeout"), nil
		}

		h.cluster.LogRPCInfo("[LLMForward] Received LLM response", map[string]interface{}{
			"correlation_id": inbound.CorrelationID,
			"response_len":   len(response),
		})
		return h.successResponse(response), nil

	case <-ctx.Done():
		h.cluster.LogRPCError("[LLMForward] Timeout waiting for LLM response", nil)
		return h.errorResponse("LLM processing timeout (60s)"), nil

	case <-time.After(65 * time.Second):
		// Fallback timeout (slightly longer than context timeout)
		h.cluster.LogRPCError("[LLMForward] Timeout (fallback)", nil)
		return h.errorResponse("LLM processing timeout"), nil
	}
}

// parsePayload parses the RPC payload into LLMForwardPayload
func (h *LLMForwardHandler) parsePayload(payload map[string]interface{}, req *LLMForwardPayload) error {
	// Convert to JSON first for proper parsing
	payloadBytes, err := json.Marshal(payload)
	if err != nil {
		return fmt.Errorf("failed to marshal payload: %w", err)
	}

	if err := json.Unmarshal(payloadBytes, req); err != nil {
		return fmt.Errorf("failed to unmarshal payload: %w", err)
	}

	return nil
}

// successResponse creates a successful response
func (h *LLMForwardHandler) successResponse(content string) map[string]interface{} {
	return map[string]interface{}{
		"success": true,
		"content": content,
	}
}

// errorResponse creates an error response
func (h *LLMForwardHandler) errorResponse(errMsg string) map[string]interface{} {
	return map[string]interface{}{
		"success": false,
		"error":   errMsg,
	}
}
