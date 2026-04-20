package forge

import (
	"context"
	"crypto/sha256"
	"fmt"
	"sort"
	"sync"
	"time"

	"github.com/276793422/NemesisBot/module/observer"
)

// ConversationTrace holds metadata for a complete conversation (no raw content).
type ConversationTrace struct {
	TraceID     string          `json:"trace_id"`
	SessionKey  string          `json:"session_key"` // SHA256 hash
	Channel     string          `json:"channel"`
	StartTime   time.Time       `json:"start_time"`
	EndTime     time.Time       `json:"end_time"`
	DurationMs  int64           `json:"duration_ms"`
	TotalRounds int             `json:"total_rounds"`
	ToolSteps   []ToolStep      `json:"tool_steps"`
	Signals     []SessionSignal `json:"signals,omitempty"`
	TokensUsed  int             `json:"tokens_used,omitempty"`
}

// ToolStep records a single tool invocation within a conversation.
type ToolStep struct {
	ToolName   string   `json:"tool_name"`
	Success    bool     `json:"success"`
	DurationMs int64    `json:"duration_ms"`
	LLMRound   int      `json:"llm_round"`
	ChainPos   int      `json:"chain_pos"`
	ArgKeys    []string `json:"arg_keys"`
	ErrorCode  string   `json:"error_code,omitempty"`
}

// SessionSignal marks an interesting pattern detected during conversation.
type SessionSignal struct {
	Type      string    `json:"type"` // "retry", "backtrack"
	Timestamp time.Time `json:"timestamp"`
	Round     int       `json:"round"`
}

// TraceCollector observes conversation lifecycle events and builds traces.
// It implements the observer.Observer interface.
type TraceCollector struct {
	store  *TraceStore
	config *ForgeConfig

	active   map[string]*ConversationTrace // traceID → in-progress trace
	activeMu sync.Mutex
}

// NewTraceCollector creates a new TraceCollector.
func NewTraceCollector(store *TraceStore, config *ForgeConfig) *TraceCollector {
	return &TraceCollector{
		store:  store,
		config: config,
		active: make(map[string]*ConversationTrace),
	}
}

func (t *TraceCollector) Name() string { return "forge_trace" }

func (t *TraceCollector) OnEvent(ctx context.Context, event observer.ConversationEvent) {
	switch event.Type {
	case observer.EventConversationStart:
		data, ok := event.Data.(*observer.ConversationStartData)
		if !ok {
			return
		}
		t.onStart(event.TraceID, data, event.Timestamp)

	case observer.EventLLMResponse:
		data, ok := event.Data.(*observer.LLMResponseData)
		if !ok {
			return
		}
		t.onLLMResponse(event.TraceID, data)

	case observer.EventToolCall:
		data, ok := event.Data.(*observer.ToolCallData)
		if !ok {
			return
		}
		t.onToolCall(event.TraceID, data, event.Timestamp)

	case observer.EventConversationEnd:
		data, ok := event.Data.(*observer.ConversationEndData)
		if !ok {
			return
		}
		t.onEnd(event.TraceID, data, event.Timestamp)
	}
}

func (t *TraceCollector) onStart(traceID string, data *observer.ConversationStartData, ts time.Time) {
	t.activeMu.Lock()
	defer t.activeMu.Unlock()

	t.active[traceID] = &ConversationTrace{
		TraceID:    traceID,
		SessionKey: hashSessionKey(data.SessionKey),
		Channel:    data.Channel,
		StartTime:  ts,
	}
}

func (t *TraceCollector) onLLMResponse(traceID string, data *observer.LLMResponseData) {
	t.activeMu.Lock()
	trace := t.active[traceID]
	t.activeMu.Unlock()
	if trace == nil {
		return
	}

	// Accumulate token usage
	if data.Usage != nil {
		trace.TokensUsed += data.Usage.TotalTokens
	}
}

func (t *TraceCollector) onToolCall(traceID string, data *observer.ToolCallData, ts time.Time) {
	t.activeMu.Lock()
	trace := t.active[traceID]
	t.activeMu.Unlock()
	if trace == nil {
		return
	}

	// Extract arg keys only (privacy: no values)
	argKeys := make([]string, 0, len(data.Arguments))
	for k := range data.Arguments {
		argKeys = append(argKeys, k)
	}
	sort.Strings(argKeys)

	errCode := ""
	if !data.Success && data.Error != "" {
		errCode = truncateError(data.Error)
	}

	trace.ToolSteps = append(trace.ToolSteps, ToolStep{
		ToolName:   data.ToolName,
		Success:    data.Success,
		DurationMs: data.Duration.Milliseconds(),
		LLMRound:   data.LLMRound,
		ChainPos:   data.ChainPos,
		ArgKeys:    argKeys,
		ErrorCode:  errCode,
	})
}

func (t *TraceCollector) onEnd(traceID string, data *observer.ConversationEndData, ts time.Time) {
	t.activeMu.Lock()
	trace := t.active[traceID]
	delete(t.active, traceID)
	t.activeMu.Unlock()
	if trace == nil {
		return
	}

	trace.EndTime = ts
	trace.TotalRounds = data.TotalRounds
	if data.TotalDuration > 0 {
		trace.DurationMs = data.TotalDuration.Milliseconds()
	} else {
		trace.DurationMs = ts.Sub(trace.StartTime).Milliseconds()
	}

	// Detect signals
	trace.Signals = detectSignals(trace.ToolSteps, ts)

	// Persist
	if t.store != nil {
		if err := t.store.Append(trace); err != nil {
			// Silent failure — trace data is ephemeral
			_ = err
		}
	}
}

// hashSessionKey returns a SHA256 hex digest for privacy.
func hashSessionKey(key string) string {
	h := sha256.Sum256([]byte(key))
	return fmt.Sprintf("%x", h[:])
}

// truncateError keeps only the first 100 chars of an error message.
func truncateError(s string) string {
	if len(s) <= 100 {
		return s
	}
	return s[:100]
}

// detectSignals analyzes tool steps for retry and backtrack patterns.
func detectSignals(steps []ToolStep, ts time.Time) []SessionSignal {
	var signals []SessionSignal

	// Group by round
	byRound := make(map[int][]ToolStep)
	for _, s := range steps {
		byRound[s.LLMRound] = append(byRound[s.LLMRound], s)
	}

	// Track tool failures for retry detection
	type toolAttempt struct {
		round   int
		success bool
	}
	toolHistory := make(map[string][]toolAttempt)

	for round := 1; len(byRound[round]) > 0 || round <= maxRound(byRound); round++ {
		roundSteps := byRound[round]
		for _, s := range roundSteps {
			toolHistory[s.ToolName] = append(toolHistory[s.ToolName], toolAttempt{
				round:   round,
				success: s.Success,
			})
		}
	}

	// Detect retry: same tool name appears 2+ times with at least one failure in between
	for _, attempts := range toolHistory {
		if len(attempts) < 2 {
			continue
		}
		hasFailure := false
		for _, a := range attempts[:len(attempts)-1] {
			if !a.success {
				hasFailure = true
				break
			}
		}
		if hasFailure {
			signals = append(signals, SessionSignal{
				Type:      "retry",
				Timestamp: ts,
				Round:     attempts[len(attempts)-1].round,
			})
		}
	}

	// Detect backtrack: tool A fails, then a different tool B is called in the same or next round
	for round := 1; round <= maxRound(byRound); round++ {
		roundSteps := byRound[round]
		for i, s := range roundSteps {
			if !s.Success && i+1 < len(roundSteps) {
				next := roundSteps[i+1]
				if next.ToolName != s.ToolName {
					signals = append(signals, SessionSignal{
						Type:      "backtrack",
						Timestamp: ts,
						Round:     round,
					})
					break // one backtrack signal per round
				}
			}
		}
	}

	return signals
}

func maxRound(byRound map[int][]ToolStep) int {
	max := 0
	for r := range byRound {
		if r > max {
			max = r
		}
	}
	return max
}
