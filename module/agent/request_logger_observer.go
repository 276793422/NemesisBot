package agent

import (
	"context"
	"sync"
	"time"

	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/observer"
)

// RequestLoggerObserver adapts the existing RequestLogger to the Observer interface.
// It creates a new RequestLogger per conversation (via traceID mapping) to maintain
// session isolation — identical to the original behavior where each runAgentLoop call
// created its own RequestLogger instance.
type RequestLoggerObserver struct {
	cfg       *config.LoggingConfig
	workspace string

	active map[string]*rlState // traceID → per-conversation state
	mu     sync.Mutex
}

type rlState struct {
	logger     *RequestLogger
	operations map[int][]Operation
}

// NewRequestLoggerObserver creates a new RequestLoggerObserver adapter.
func NewRequestLoggerObserver(cfg *config.LoggingConfig, workspace string) *RequestLoggerObserver {
	return &RequestLoggerObserver{
		cfg:       cfg,
		workspace: workspace,
		active:    make(map[string]*rlState),
	}
}

func (r *RequestLoggerObserver) Name() string { return "request_logger" }

func (r *RequestLoggerObserver) OnEvent(ctx context.Context, event observer.ConversationEvent) {
	switch event.Type {
	case observer.EventConversationStart:
		data, ok := event.Data.(*observer.ConversationStartData)
		if !ok {
			return
		}
		rl := NewRequestLogger(r.cfg, r.workspace)
		if !rl.IsEnabled() {
			return
		}
		if err := rl.CreateSession(); err != nil {
			return
		}
		rl.LogUserRequest(UserRequestInfo{
			Timestamp: event.Timestamp,
			Channel:   data.Channel,
			SenderID:  data.SenderID,
			ChatID:    data.ChatID,
			Content:   data.Content,
		})
		r.mu.Lock()
		r.active[event.TraceID] = &rlState{
			logger:     rl,
			operations: make(map[int][]Operation),
		}
		r.mu.Unlock()

	case observer.EventLLMRequest:
		r.mu.Lock()
		state := r.active[event.TraceID]
		r.mu.Unlock()
		if state == nil {
			return
		}
		data, ok := event.Data.(*observer.LLMRequestData)
		if !ok {
			return
		}
		state.logger.LogLLMRequest(LLMRequestInfo{
			Round:        data.Round,
			Timestamp:    event.Timestamp,
			Model:        data.Model,
			ProviderName: data.ProviderName,
			APIKey:       data.APIKey,
			APIBase:      data.APIBase,
			HTTPHeaders:  data.HTTPHeaders,
			FullConfig:   data.FullConfig,
			Messages:     data.Messages,
			Tools:        data.Tools,
		})

	case observer.EventLLMResponse:
		r.mu.Lock()
		state := r.active[event.TraceID]
		r.mu.Unlock()
		if state == nil {
			return
		}
		data, ok := event.Data.(*observer.LLMResponseData)
		if !ok {
			return
		}
		state.logger.LogLLMResponse(LLMResponseInfo{
			Round:        data.Round,
			Timestamp:    event.Timestamp,
			Duration:     data.Duration,
			Content:      data.Content,
			ToolCalls:    data.ToolCalls,
			Usage:        data.Usage,
			FinishReason: data.FinishReason,
		})

	case observer.EventToolCall:
		r.mu.Lock()
		state := r.active[event.TraceID]
		r.mu.Unlock()
		if state == nil {
			return
		}
		data, ok := event.Data.(*observer.ToolCallData)
		if !ok {
			return
		}
		op := Operation{
			Type:      "tool_call",
			Name:      data.ToolName,
			Arguments: data.Arguments,
			Status:    "Success",
			Duration:  data.Duration,
		}
		if !data.Success {
			op.Status = "Failed"
			op.Error = data.Error
		}
		state.operations[data.LLMRound] = append(state.operations[data.LLMRound], op)

	case observer.EventConversationEnd:
		r.mu.Lock()
		state := r.active[event.TraceID]
		delete(r.active, event.TraceID)
		r.mu.Unlock()
		if state == nil {
			return
		}
		data, ok := event.Data.(*observer.ConversationEndData)
		if !ok {
			return
		}
		// Flush collected operations per round
		for round := 1; round <= data.TotalRounds; round++ {
			ops := state.operations[round]
			if len(ops) > 0 {
				state.logger.LogLocalOperations(LocalOperationInfo{
					Round:      round,
					Timestamp:  event.Timestamp,
					Operations: ops,
				})
			}
		}
		totalDuration := data.TotalDuration
		if totalDuration == 0 {
			totalDuration = time.Since(state.logger.startTime)
		}
		state.logger.LogFinalResponse(FinalResponseInfo{
			Timestamp:     event.Timestamp,
			TotalDuration: totalDuration,
			LLMRounds:     data.TotalRounds,
			Content:       data.Content,
			Channel:       data.Channel,
			ChatID:        data.ChatID,
		})
		state.logger.Close()
	}
}
