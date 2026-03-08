// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package tools

import (
	"context"
	"fmt"

	"github.com/276793422/NemesisBot/module/logger"
	"github.com/276793422/NemesisBot/module/utils"
)

type SendCallback func(channel, chatID, content string) error

type MessageTool struct {
	sendCallback   SendCallback
	defaultChannel string
	defaultChatID  string
	sentInRound    bool // Tracks whether a message was sent in the current processing round
}

func NewMessageTool() *MessageTool {
	return &MessageTool{}
}

func (t *MessageTool) Name() string {
	return "message"
}

func (t *MessageTool) Description() string {
	return "Send a message to user on a chat channel. Use this when you want to communicate something."
}

func (t *MessageTool) Parameters() map[string]interface{} {
	return map[string]interface{}{
		"type": "object",
		"properties": map[string]interface{}{
			"content": map[string]interface{}{
				"type":        "string",
				"description": "The message content to send",
			},
			"channel": map[string]interface{}{
				"type":        "string",
				"description": "Optional: target channel (telegram, whatsapp, etc.)",
			},
			"chat_id": map[string]interface{}{
				"type":        "string",
				"description": "Optional: target chat/user ID",
			},
		},
		"required": []string{"content"},
	}
}

func (t *MessageTool) SetContext(channel, chatID string) {
	t.defaultChannel = channel
	t.defaultChatID = chatID
	t.sentInRound = false // Reset send tracking for new processing round
}

// HasSentInRound returns true if the message tool sent a message during the current round.
func (t *MessageTool) HasSentInRound() bool {
	return t.sentInRound
}

func (t *MessageTool) SetSendCallback(callback SendCallback) {
	t.sendCallback = callback
}

func (t *MessageTool) Execute(ctx context.Context, args map[string]interface{}) *ToolResult {
	content, ok := args["content"].(string)
	if !ok {
		return &ToolResult{ForLLM: "content is required", IsError: true}
	}

	channel, _ := args["channel"].(string)
	chatID, _ := args["chat_id"].(string)

	if channel == "" {
		channel = t.defaultChannel
	}
	if chatID == "" {
		chatID = t.defaultChatID
	}

	if channel == "" || chatID == "" {
		return &ToolResult{ForLLM: "No target channel/chat specified", IsError: true}
	}

	if t.sendCallback == nil {
		return &ToolResult{ForLLM: "Message sending not configured", IsError: true}
	}

	// For RPC channel, check if we need to add correlation ID
	finalContent := content
	if channel == "rpc" {
		// Try to get correlation ID from context
		correlationID := getCorrelationIDFromContext(ctx)
		logger.InfoCF("agent", "MessageTool: RPC channel detected",
			map[string]interface{}{
				"correlation_id":  correlationID,
				"content_preview": utils.Truncate(content, 100),
			})

		if correlationID != "" {
			finalContent = fmt.Sprintf("[rpc:%s] %s", correlationID, content)
			logger.InfoCF("agent", "MessageTool: Added correlation ID prefix to RPC message",
				map[string]interface{}{
					"correlation_id":        correlationID,
					"final_content_preview": utils.Truncate(finalContent, 120),
				})
		} else {
			logger.WarnCF("agent", "MessageTool: ⚠️ No correlation ID in context for RPC channel - response will not be delivered!",
				map[string]interface{}{
					"content_preview": utils.Truncate(content, 100),
				})
		}
	}

	if err := t.sendCallback(channel, chatID, finalContent); err != nil {
		return &ToolResult{
			ForLLM:  fmt.Sprintf("sending message: %v", err),
			IsError: true,
			Err:     err,
		}
	}

	t.sentInRound = true
	// Silent: user already received the message directly
	return &ToolResult{
		ForLLM: fmt.Sprintf("Message sent to %s:%s", channel, chatID),
		Silent: true,
	}
}

// getCorrelationIDFromContext extracts correlation ID from context
func getCorrelationIDFromContext(ctx context.Context) string {
	if ctx == nil {
		return ""
	}

	// Try to get correlation_id from context values
	if v := ctx.Value("correlation_id"); v != nil {
		if id, ok := v.(string); ok {
			return id
		}
	}
	return ""
}
