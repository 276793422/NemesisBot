// Package observer provides a generic event observation framework for
// conversation lifecycle events. It decouples event producers (AgentLoop)
// from event consumers (RequestLogger, TraceCollector, etc.).
package observer

import (
	"context"
	"time"

	"github.com/276793422/NemesisBot/module/providers"
)

// EventType identifies a conversation lifecycle event.
type EventType string

const (
	EventConversationStart EventType = "conversation_start"
	EventConversationEnd   EventType = "conversation_end"
	EventLLMRequest        EventType = "llm_request"
	EventLLMResponse       EventType = "llm_response"
	EventToolCall          EventType = "tool_call"
)

// ConversationEvent represents a single event during a conversation.
type ConversationEvent struct {
	Type      EventType
	TraceID   string      // Correlates all events in one conversation
	Timestamp time.Time
	Data      interface{} // Pointer to one of the *Data types below
}

// ConversationStartData is the payload for EventConversationStart.
type ConversationStartData struct {
	SessionKey string
	Channel    string
	ChatID     string
	SenderID   string
	Content    string // User message (used by RequestLogger, ignored by TraceCollector)
}

// ConversationEndData is the payload for EventConversationEnd.
type ConversationEndData struct {
	SessionKey    string
	Channel       string
	ChatID        string
	TotalRounds   int
	TotalDuration time.Duration
	Content       string // Final response (used by RequestLogger, ignored by TraceCollector)
	Error         error  // Non-nil if the conversation ended with an error
}

// LLMRequestData is the payload for EventLLMRequest.
type LLMRequestData struct {
	Round        int
	Model        string
	ProviderName string
	APIKey       string
	APIBase      string
	HTTPHeaders  map[string]string
	FullConfig   map[string]interface{}
	Messages     []providers.Message
	Tools        []providers.ToolDefinition
}

// LLMResponseData is the payload for EventLLMResponse.
type LLMResponseData struct {
	Round        int
	Duration     time.Duration
	Content      string
	ToolCalls    []providers.ToolCall
	Usage        *providers.UsageInfo
	FinishReason string
}

// ToolCallData is the payload for EventToolCall.
type ToolCallData struct {
	ToolName  string
	Arguments map[string]interface{}
	Success   bool
	Duration  time.Duration
	Error     string
	LLMRound  int // Which LLM iteration
	ChainPos  int // Position within the round's tool calls
}

// Observer receives conversation lifecycle events.
type Observer interface {
	Name() string
	OnEvent(ctx context.Context, event ConversationEvent)
}
