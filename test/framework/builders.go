// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package framework

import (
	"github.com/276793422/NemesisBot/module/bus"
)

// MessageBuilder helps build test messages
type MessageBuilder struct {
	channel       string
	senderID      string
	chatID        string
	content       string
	media         []string
	sessionKey    string
	correlationID string
}

// NewMessageBuilder creates a new message builder
func NewMessageBuilder() *MessageBuilder {
	return &MessageBuilder{
		media: make([]string, 0),
	}
}

// WithChannel sets the channel
func (b *MessageBuilder) WithChannel(channel string) *MessageBuilder {
	b.channel = channel
	return b
}

// WithSenderID sets the sender ID
func (b *MessageBuilder) WithSenderID(senderID string) *MessageBuilder {
	b.senderID = senderID
	return b
}

// WithChatID sets the chat ID
func (b *MessageBuilder) WithChatID(chatID string) *MessageBuilder {
	b.chatID = chatID
	return b
}

// WithContent sets the message content
func (b *MessageBuilder) WithContent(content string) *MessageBuilder {
	b.content = content
	return b
}

// WithMedia adds media to the message
func (b *MessageBuilder) WithMedia(media []string) *MessageBuilder {
	b.media = media
	return b
}

// WithSessionKey sets the session key
func (b *MessageBuilder) WithSessionKey(sessionKey string) *MessageBuilder {
	b.sessionKey = sessionKey
	return b
}

// WithCorrelationID sets the correlation ID
func (b *MessageBuilder) WithCorrelationID(correlationID string) *MessageBuilder {
	b.correlationID = correlationID
	return b
}

// BuildInbound builds an inbound message
func (b *MessageBuilder) BuildInbound() bus.InboundMessage {
	return bus.InboundMessage{
		Channel:       b.channel,
		SenderID:      b.senderID,
		ChatID:        b.chatID,
		Content:       b.content,
		Media:         b.media,
		SessionKey:    b.sessionKey,
		CorrelationID: b.correlationID,
		Metadata:      make(map[string]string),
	}
}

// BuildOutbound builds an outbound message
func (b *MessageBuilder) BuildOutbound() bus.OutboundMessage {
	return bus.OutboundMessage{
		Channel: b.channel,
		ChatID:  b.chatID,
		Content: b.content,
	}
}

// OutboundMessageBuilder helps build outbound messages
type OutboundMessageBuilder struct {
	channel string
	chatID  string
	content string
}

// NewOutboundMessageBuilder creates a new outbound message builder
func NewOutboundMessageBuilder() *OutboundMessageBuilder {
	return &OutboundMessageBuilder{}
}

// WithChannel sets the channel
func (b *OutboundMessageBuilder) WithChannel(channel string) *OutboundMessageBuilder {
	b.channel = channel
	return b
}

// WithChatID sets the chat ID
func (b *OutboundMessageBuilder) WithChatID(chatID string) *OutboundMessageBuilder {
	b.chatID = chatID
	return b
}

// WithContent sets the content
func (b *OutboundMessageBuilder) WithContent(content string) *OutboundMessageBuilder {
	b.content = content
	return b
}

// Build builds the outbound message
func (b *OutboundMessageBuilder) Build() bus.OutboundMessage {
	return bus.OutboundMessage{
		Channel: b.channel,
		ChatID:  b.chatID,
		Content: b.content,
	}
}

// MediaBuilder helps build test media URLs (media is represented as strings)
type MediaBuilder struct {
	media []string
}

// NewMediaBuilder creates a new media builder
func NewMediaBuilder() *MediaBuilder {
	return &MediaBuilder{
		media: make([]string, 0),
	}
}

// AddURL adds a media URL
func (b *MediaBuilder) AddURL(url string) *MediaBuilder {
	b.media = append(b.media, url)
	return b
}

// Build builds the media URL list
func (b *MediaBuilder) Build() []string {
	return append([]string{}, b.media...)
}

// PayloadBuilder helps build test payloads for RPC calls
type PayloadBuilder struct {
	data map[string]interface{}
}

// NewPayloadBuilder creates a new payload builder
func NewPayloadBuilder() *PayloadBuilder {
	return &PayloadBuilder{
		data: make(map[string]interface{}),
	}
}

// With sets a key-value pair
func (b *PayloadBuilder) With(key string, value interface{}) *PayloadBuilder {
	b.data[key] = value
	return b
}

// WithMessage sets the message
func (b *PayloadBuilder) WithMessage(msg string) *PayloadBuilder {
	b.data["message"] = msg
	return b
}

// WithAction sets the action
func (b *PayloadBuilder) WithAction(action string) *PayloadBuilder {
	b.data["action"] = action
	return b
}

// WithSenderID sets the sender ID
func (b *PayloadBuilder) WithSenderID(senderID string) *PayloadBuilder {
	b.data["sender_id"] = senderID
	return b
}

// WithChatID sets the chat ID
func (b *PayloadBuilder) WithChatID(chatID string) *PayloadBuilder {
	b.data["chat_id"] = chatID
	return b
}

// WithSessionKey sets the session key
func (b *PayloadBuilder) WithSessionKey(sessionKey string) *PayloadBuilder {
	b.data["session_key"] = sessionKey
	return b
}

// WithTimestamp sets the timestamp
func (b *PayloadBuilder) WithTimestamp(timestamp int64) *PayloadBuilder {
	b.data["timestamp"] = timestamp
	return b
}

// Build builds the payload
func (b *PayloadBuilder) Build() map[string]interface{} {
	return b.data
}

// ToolCallBuilder helps build tool calls for testing
type ToolCallBuilder struct {
	id        string
	name      string
	arguments map[string]interface{}
}

// NewToolCallBuilder creates a new tool call builder
func NewToolCallBuilder() *ToolCallBuilder {
	return &ToolCallBuilder{
		arguments: make(map[string]interface{}),
	}
}

// WithID sets the tool call ID
func (b *ToolCallBuilder) WithID(id string) *ToolCallBuilder {
	b.id = id
	return b
}

// WithName sets the tool name
func (b *ToolCallBuilder) WithName(name string) *ToolCallBuilder {
	b.name = name
	return b
}

// WithArgument sets an argument
func (b *ToolCallBuilder) WithArgument(key string, value interface{}) *ToolCallBuilder {
	b.arguments[key] = value
	return b
}

// WithArguments sets all arguments
func (b *ToolCallBuilder) WithArguments(args map[string]interface{}) *ToolCallBuilder {
	b.arguments = args
	return b
}

// Build builds the tool call
func (b *ToolCallBuilder) Build() map[string]interface{} {
	return map[string]interface{}{
		"id":        b.id,
		"name":      b.name,
		"arguments": b.arguments,
	}
}

// MessageListBuilder helps build lists of messages for conversation history
type MessageListBuilder struct {
	messages []map[string]interface{}
}

// NewMessageListBuilder creates a new message list builder
func NewMessageListBuilder() *MessageListBuilder {
	return &MessageListBuilder{
		messages: make([]map[string]interface{}, 0),
	}
}

// AddUser adds a user message
func (b *MessageListBuilder) AddUser(content string) *MessageListBuilder {
	b.messages = append(b.messages, map[string]interface{}{
		"role":    "user",
		"content": content,
	})
	return b
}

// AddAssistant adds an assistant message
func (b *MessageListBuilder) AddAssistant(content string) *MessageListBuilder {
	b.messages = append(b.messages, map[string]interface{}{
		"role":    "assistant",
		"content": content,
	})
	return b
}

// AddSystem adds a system message
func (b *MessageListBuilder) AddSystem(content string) *MessageListBuilder {
	b.messages = append(b.messages, map[string]interface{}{
		"role":    "system",
		"content": content,
	})
	return b
}

// AddTool adds a tool result message
func (b *MessageListBuilder) AddTool(toolID, content string) *MessageListBuilder {
	b.messages = append(b.messages, map[string]interface{}{
		"role":       "tool",
		"tool_id":    toolID,
		"content":    content,
	})
	return b
}

// Build builds the message list
func (b *MessageListBuilder) Build() []map[string]interface{} {
	return b.messages
}
