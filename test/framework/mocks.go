// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package framework

import (
	"context"
	"fmt"
	"sync"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/providers"
	"github.com/276793422/NemesisBot/module/providers/protocoltypes"
)

// MockLLMProvider is a configurable mock LLM provider for testing
type MockLLMProvider struct {
	mu             sync.Mutex
	responses      []string
	responseIndex  int
	shouldError    bool
	customResponse *providers.LLMResponse
	delay          time.Duration
	callCount      int
	calls          []LLMCall
	model          string
}

type LLMCall struct {
	Messages []providers.Message
	Tools    []providers.ToolDefinition
	Model    string
}

// NewMockLLMProvider creates a new mock LLM provider
func NewMockLLMProvider() *MockLLMProvider {
	return &MockLLMProvider{
		responses: []string{"Mock response"},
		model:     "mock-model",
	}
}

// Chat implements the LLMProvider interface
func (m *MockLLMProvider) Chat(ctx context.Context, messages []providers.Message, tools []providers.ToolDefinition, model string, options map[string]interface{}) (*providers.LLMResponse, error) {
	m.mu.Lock()
	defer m.mu.Unlock()

	m.callCount++
	m.calls = append(m.calls, LLMCall{
		Messages: messages,
		Tools:    tools,
		Model:    model,
	})

	if m.delay > 0 {
		select {
		case <-ctx.Done():
			return nil, ctx.Err()
		case <-time.After(m.delay):
		}
	}

	if m.shouldError {
		return nil, &providers.FailoverError{
			Reason:   providers.FailoverUnknown,
			Provider: "mock",
			Model:    m.model,
			Wrapped:  fmt.Errorf("mock LLM error"),
		}
	}

	if m.customResponse != nil {
		return m.customResponse, nil
	}

	response := "Mock response"
	if m.responses != nil && m.responseIndex < len(m.responses) {
		response = m.responses[m.responseIndex]
		m.responseIndex++
	}

	return &providers.LLMResponse{
		Content:      response,
		FinishReason: "stop",
		ToolCalls:    []protocoltypes.ToolCall{},
		Usage: &providers.UsageInfo{
			PromptTokens:     100,
			CompletionTokens: 50,
			TotalTokens:      150,
		},
	}, nil
}

// GetDefaultModel returns the default model
func (m *MockLLMProvider) GetDefaultModel() string {
	return m.model
}

// SetResponses sets the responses to return
func (m *MockLLMProvider) SetResponses(responses []string) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.responses = responses
	m.responseIndex = 0
}

// SetError sets whether to return an error
func (m *MockLLMProvider) SetError(shouldError bool) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.shouldError = shouldError
}

// SetCustomResponse sets a custom response to return
func (m *MockLLMProvider) SetCustomResponse(response *providers.LLMResponse) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.customResponse = response
}

// SetDelay sets the delay before responding
func (m *MockLLMProvider) SetDelay(delay time.Duration) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.delay = delay
}

// GetCallCount returns the number of times Chat was called
func (m *MockLLMProvider) GetCallCount() int {
	m.mu.Lock()
	defer m.mu.Unlock()
	return m.callCount
}

// GetCalls returns the list of calls made to Chat
func (m *MockLLMProvider) GetCalls() []LLMCall {
	m.mu.Lock()
	defer m.mu.Unlock()
	return append([]LLMCall{}, m.calls...)
}

// Reset resets the mock state
func (m *MockLLMProvider) Reset() {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.responseIndex = 0
	m.shouldError = false
	m.customResponse = nil
	m.callCount = 0
	m.calls = nil
}

// MockMessageBus is an in-memory message bus for testing
type MockMessageBus struct {
	mu           sync.Mutex
	inboundMsgs  []bus.InboundMessage
	outboundMsgs []bus.OutboundMessage
	subscribers  []chan bus.InboundMessage
}

// NewMockMessageBus creates a new mock message bus
func NewMockMessageBus() *MockMessageBus {
	return &MockMessageBus{
		subscribers: make([]chan bus.InboundMessage, 0),
	}
}

// PublishInbound publishes an inbound message
func (m *MockMessageBus) PublishInbound(ctx context.Context, msg bus.InboundMessage) error {
	m.mu.Lock()
	defer m.mu.Unlock()

	m.inboundMsgs = append(m.inboundMsgs, msg)

	// Notify subscribers
	for _, sub := range m.subscribers {
		select {
		case sub <- msg:
		case <-ctx.Done():
			return ctx.Err()
		default:
			// Channel full, skip
		}
	}

	return nil
}

// PublishOutbound publishes an outbound message
func (m *MockMessageBus) PublishOutbound(ctx context.Context, msg bus.OutboundMessage) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.outboundMsgs = append(m.outboundMsgs, msg)
	return nil
}

// SubscribeInbound subscribes to inbound messages
func (m *MockMessageBus) SubscribeInbound() <-chan bus.InboundMessage {
	m.mu.Lock()
	defer m.mu.Unlock()

	ch := make(chan bus.InboundMessage, 100)
	m.subscribers = append(m.subscribers, ch)
	return ch
}

// OutboundChannel returns the outbound message channel
func (m *MockMessageBus) OutboundChannel() <-chan bus.OutboundMessage {
	ch := make(chan bus.OutboundMessage, 100)
	go func() {
		m.mu.Lock()
		msgs := append([]bus.OutboundMessage{}, m.outboundMsgs...)
		m.mu.Unlock()

		for _, msg := range msgs {
			ch <- msg
		}
	}()
	return ch
}

// GetInboundMessages returns all published inbound messages
func (m *MockMessageBus) GetInboundMessages() []bus.InboundMessage {
	m.mu.Lock()
	defer m.mu.Unlock()
	return append([]bus.InboundMessage{}, m.inboundMsgs...)
}

// GetOutboundMessages returns all published outbound messages
func (m *MockMessageBus) GetOutboundMessages() []bus.OutboundMessage {
	m.mu.Lock()
	defer m.mu.Unlock()
	return append([]bus.OutboundMessage{}, m.outboundMsgs...)
}

// Clear clears all messages
func (m *MockMessageBus) Clear() {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.inboundMsgs = nil
	m.outboundMsgs = nil
}

// MockChannel is a mock channel implementation
type MockChannel struct {
	mu          sync.Mutex
	name        string
	running     bool
	messages    []bus.OutboundMessage
	startErr    error
	stopErr     error
	sendErr     error
	allowed     map[string]bool
	syncTargets []string
}

// NewMockChannel creates a new mock channel
func NewMockChannel(name string) *MockChannel {
	return &MockChannel{
		name:        name,
		allowed:     make(map[string]bool),
		syncTargets: make([]string, 0),
	}
}

// Name returns the channel name
func (m *MockChannel) Name() string {
	return m.name
}

// Start starts the channel
func (m *MockChannel) Start() error {
	m.mu.Lock()
	defer m.mu.Unlock()

	if m.startErr != nil {
		return m.startErr
	}

	m.running = true
	return nil
}

// Stop stops the channel
func (m *MockChannel) Stop() error {
	m.mu.Lock()
	defer m.mu.Unlock()

	if m.stopErr != nil {
		return m.stopErr
	}

	m.running = false
	return nil
}

// Send sends a message
func (m *MockChannel) Send(ctx context.Context, msg bus.OutboundMessage) error {
	m.mu.Lock()
	defer m.mu.Unlock()

	if m.sendErr != nil {
		return m.sendErr
	}

	m.messages = append(m.messages, msg)
	return nil
}

// IsRunning returns whether the channel is running
func (m *MockChannel) IsRunning() bool {
	m.mu.Lock()
	defer m.mu.Unlock()
	return m.running
}

// IsAllowed checks if a sender is allowed
func (m *MockChannel) IsAllowed(senderID string) bool {
	m.mu.Lock()
	defer m.mu.Unlock()

	if len(m.allowed) == 0 {
		return true
	}

	return m.allowed[senderID]
}

// SetAllowed sets the allowed senders
func (m *MockChannel) SetAllowed(allowed map[string]bool) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.allowed = allowed
}

// AddSyncTarget adds a sync target
func (m *MockChannel) AddSyncTarget(channelName string) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.syncTargets = append(m.syncTargets, channelName)
}

// GetSyncTargets returns the sync targets
func (m *MockChannel) GetSyncTargets() []string {
	m.mu.Lock()
	defer m.mu.Unlock()
	return append([]string{}, m.syncTargets...)
}

// GetMessages returns all sent messages
func (m *MockChannel) GetMessages() []bus.OutboundMessage {
	m.mu.Lock()
	defer m.mu.Unlock()
	return append([]bus.OutboundMessage{}, m.messages...)
}

// SetStartError sets an error to return on Start
func (m *MockChannel) SetStartError(err error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.startErr = err
}

// SetStopError sets an error to return on Stop
func (m *MockChannel) SetStopError(err error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.stopErr = err
}

// SetSendError sets an error to return on Send
func (m *MockChannel) SetSendError(err error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.sendErr = err
}

// MockSecurityAuditor is a mock security auditor
type MockSecurityAuditor struct {
	mu             sync.Mutex
	allowed        bool
	permissionType string
	logs           []AuditLogEntry
	policies       map[string]bool
}

type AuditLogEntry struct {
	Operation string
	Target    string
	Allowed   bool
}

// NewMockSecurityAuditor creates a new mock security auditor
func NewMockSecurityAuditor() *MockSecurityAuditor {
	return &MockSecurityAuditor{
		allowed:  true,
		policies: make(map[string]bool),
	}
}

// RequestPermission requests permission for an operation
func (m *MockSecurityAuditor) RequestPermission(ctx context.Context, permissionType, target string, metadata map[string]interface{}) (bool, error) {
	m.mu.Lock()
	defer m.mu.Unlock()

	entry := AuditLogEntry{
		Operation: permissionType,
		Target:    target,
		Allowed:   m.allowed,
	}
	m.logs = append(m.logs, entry)

	// Check policies
	if policy, ok := m.policies[permissionType+":"+target]; ok {
		return policy, nil
	}

	return m.allowed, nil
}

// SetAllowed sets whether to allow operations
func (m *MockSecurityAuditor) SetAllowed(allowed bool) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.allowed = allowed
}

// SetPolicy sets a policy for a specific operation
func (m *MockSecurityAuditor) SetPolicy(permissionType, target string, allowed bool) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.policies[permissionType+":"+target] = allowed
}

// GetLogs returns all audit logs
func (m *MockSecurityAuditor) GetLogs() []AuditLogEntry {
	m.mu.Lock()
	defer m.mu.Unlock()
	return append([]AuditLogEntry{}, m.logs...)
}

// ClearLogs clears all audit logs
func (m *MockSecurityAuditor) ClearLogs() {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.logs = nil
}

// MockConfigBuilder helps build test configurations
type MockConfigBuilder struct {
	config *config.Config
}

// NewMockConfigBuilder creates a new mock config builder
func NewMockConfigBuilder() *MockConfigBuilder {
	return &MockConfigBuilder{
		config: &config.Config{},
	}
}

// Build returns the built configuration
func (b *MockConfigBuilder) Build() *config.Config {
	return b.config
}

// WithWorkspace sets the workspace path
func (b *MockConfigBuilder) WithWorkspace(path string) *MockConfigBuilder {
	b.config.Agents.Defaults.Workspace = path
	return b
}

// WithLLM sets the LLM model
func (b *MockConfigBuilder) WithLLM(model string) *MockConfigBuilder {
	b.config.Agents.Defaults.LLM = model
	return b
}

// WithMaxTokens sets the max tokens
func (b *MockConfigBuilder) WithMaxTokens(tokens int) *MockConfigBuilder {
	b.config.Agents.Defaults.MaxTokens = tokens
	return b
}

// WithMaxToolIterations sets the max tool iterations
func (b *MockConfigBuilder) WithMaxToolIterations(iterations int) *MockConfigBuilder {
	b.config.Agents.Defaults.MaxToolIterations = iterations
	return b
}

// WithConcurrentRequestMode sets the concurrent request mode
func (b *MockConfigBuilder) WithConcurrentRequestMode(mode string) *MockConfigBuilder {
	b.config.Agents.Defaults.ConcurrentRequestMode = mode
	return b
}

// WithQueueSize sets the queue size
func (b *MockConfigBuilder) WithQueueSize(size int) *MockConfigBuilder {
	b.config.Agents.Defaults.QueueSize = size
	return b
}

// WithRestrictToWorkspace sets whether to restrict to workspace
func (b *MockConfigBuilder) WithRestrictToWorkspace(restrict bool) *MockConfigBuilder {
	b.config.Agents.Defaults.RestrictToWorkspace = restrict
	return b
}

// WithAgent adds an agent configuration
func (b *MockConfigBuilder) WithAgent(id, name string) *MockConfigBuilder {
	if b.config.Agents.List == nil {
		b.config.Agents.List = make([]config.AgentConfig, 0)
	}

	b.config.Agents.List = append(b.config.Agents.List, config.AgentConfig{
		ID:   id,
		Name: name,
	})

	return b
}
