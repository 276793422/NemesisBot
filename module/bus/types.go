// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package bus

type InboundMessage struct {
	Channel       string            `json:"channel"`
	SenderID      string            `json:"sender_id"`
	ChatID        string            `json:"chat_id"`
	Content       string            `json:"content"`
	Media         []string          `json:"media,omitempty"`
	SessionKey    string            `json:"session_key"`
	Metadata      map[string]string `json:"metadata,omitempty"`
	CorrelationID string            `json:"correlation_id,omitempty"` // For RPC response matching
}

type OutboundMessage struct {
	Channel string `json:"channel"`
	ChatID  string `json:"chat_id"`
	Content string `json:"content"`
	Type    string `json:"type,omitempty"` // "" = normal, "history" = history response
}

type MessageHandler func(InboundMessage) error
