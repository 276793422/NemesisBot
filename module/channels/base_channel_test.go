// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package channels

import (
	"context"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
)

// MockChannel is a mock implementation of the Channel interface for testing
type MockChannel struct {
	name      string
	running   bool
	allowList []string
	sentMsgs  []bus.OutboundMessage
	mu        chan struct{}
}

func NewMockChannel(name string, allowList []string) *MockChannel {
	return &MockChannel{
		name:      name,
		allowList: allowList,
		sentMsgs:  make([]bus.OutboundMessage, 0),
		mu:        make(chan struct{}, 1),
	}
}

func (m *MockChannel) Name() string {
	return m.name
}

func (m *MockChannel) Start(ctx context.Context) error {
	m.running = true
	return nil
}

func (m *MockChannel) Stop(ctx context.Context) error {
	m.running = false
	return nil
}

func (m *MockChannel) Send(ctx context.Context, msg bus.OutboundMessage) error {
	m.mu <- struct{}{}
	m.sentMsgs = append(m.sentMsgs, msg)
	<-m.mu
	return nil
}

func (m *MockChannel) IsRunning() bool {
	return m.running
}

func (m *MockChannel) IsAllowed(senderID string) bool {
	if len(m.allowList) == 0 {
		return true
	}
	for _, allowed := range m.allowList {
		if senderID == allowed {
			return true
		}
	}
	return false
}

func (m *MockChannel) AddSyncTarget(name string, channel Channel) error {
	return nil
}

func (m *MockChannel) RemoveSyncTarget(name string) {
}

func (m *MockChannel) GetSentMessages() []bus.OutboundMessage {
	return m.sentMsgs
}

func (m *MockChannel) ClearSentMessages() {
	m.sentMsgs = make([]bus.OutboundMessage, 0)
}

// TestNewBaseChannel tests the creation of a new BaseChannel
func TestNewBaseChannel(t *testing.T) {
	msgBus := bus.NewMessageBus()
	config := "test-config"
	allowList := []string{"user1", "user2"}

	channel := NewBaseChannel("test-channel", config, msgBus, allowList)

	if channel == nil {
		t.Fatal("NewBaseChannel returned nil")
	}

	if channel.Name() != "test-channel" {
		t.Errorf("Expected name 'test-channel', got '%s'", channel.Name())
	}

	if channel.IsRunning() {
		t.Error("New channel should not be running")
	}
}

// TestBaseChannelName tests the Name method
func TestBaseChannelName(t *testing.T) {
	msgBus := bus.NewMessageBus()
	channel := NewBaseChannel("test-name", nil, msgBus, nil)

	if channel.Name() != "test-name" {
		t.Errorf("Expected name 'test-name', got '%s'", channel.Name())
	}
}

// TestBaseChannelIsRunning tests the IsRunning method
func TestBaseChannelIsRunning(t *testing.T) {
	msgBus := bus.NewMessageBus()
	channel := NewBaseChannel("test", nil, msgBus, nil)

	// Initially not running
	if channel.IsRunning() {
		t.Error("New channel should not be running")
	}

	// Set running state
	channel.setRunning(true)
	if !channel.IsRunning() {
		t.Error("Channel should be running after setRunning(true)")
	}

	// Set not running
	channel.setRunning(false)
	if channel.IsRunning() {
		t.Error("Channel should not be running after setRunning(false)")
	}
}

// TestBaseChannelIsAllowed tests the IsAllowed method with various scenarios
func TestBaseChannelIsAllowed(t *testing.T) {
	tests := []struct {
		name      string
		allowList []string
		senderID  string
		expected  bool
	}{
		{
			name:      "Empty allowlist allows all",
			allowList: []string{},
			senderID:  "any-user",
			expected:  true,
		},
		{
			name:      "Nil allowlist allows all",
			allowList: nil,
			senderID:  "any-user",
			expected:  true,
		},
		{
			name:      "Exact match in allowlist",
			allowList: []string{"user1", "user2"},
			senderID:  "user1",
			expected:  true,
		},
		{
			name:      "Not in allowlist",
			allowList: []string{"user1", "user2"},
			senderID:  "user3",
			expected:  false,
		},
		{
			name:      "Compound senderID with match",
			allowList: []string{"123456", "user1"},
			senderID:  "123456|username",
			expected:  true,
		},
		{
			name:      "Compound senderID with compound allowlist match",
			allowList: []string{"123456|username", "user2"},
			senderID:  "123456|username",
			expected:  true,
		},
		{
			name:      "Allowlist with @ prefix matches username",
			allowList: []string{"@user1", "@user2"},
			senderID:  "user1",
			expected:  true,
		},
		{
			name:      "Allowlist with @ prefix matches compound senderID username",
			allowList: []string{"@username"},
			senderID:  "123456|username",
			expected:  true,
		},
		{
			name:      "Compound allowlist matches ID part",
			allowList: []string{"123456|correct_user"},
			senderID:  "123456|username",
			expected:  true,
		},
		{
			name:      "Username part matches",
			allowList: []string{"username"},
			senderID:  "123456|username",
			expected:  true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			msgBus := bus.NewMessageBus()
			channel := NewBaseChannel("test", nil, msgBus, tt.allowList)

			result := channel.IsAllowed(tt.senderID)
			if result != tt.expected {
				t.Errorf("IsAllowed(%q) = %v, expected %v", tt.senderID, result, tt.expected)
			}
		})
	}
}

// TestBaseChannelHandleMessage tests the HandleMessage method
func TestBaseChannelHandleMessage(t *testing.T) {
	msgBus := bus.NewMessageBus()
	channel := NewBaseChannel("test", nil, msgBus, nil)

	// Subscribe to inbound messages
	received := make(chan bus.InboundMessage, 1)
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 200*time.Millisecond)
		defer cancel()
		msg, ok := msgBus.ConsumeInbound(ctx)
		if ok {
			received <- msg
		}
	}()

	// Test handling a message
	channel.HandleMessage("sender123", "chat456", "Hello, world!", []string{"media1.jpg"}, nil)

	select {
	case msg := <-received:
		if msg.Channel != "test" {
			t.Errorf("Expected channel 'test', got '%s'", msg.Channel)
		}
		if msg.SenderID != "sender123" {
			t.Errorf("Expected senderID 'sender123', got '%s'", msg.SenderID)
		}
		if msg.ChatID != "chat456" {
			t.Errorf("Expected chatID 'chat456', got '%s'", msg.ChatID)
		}
		if msg.Content != "Hello, world!" {
			t.Errorf("Expected content 'Hello, world!', got '%s'", msg.Content)
		}
		if len(msg.Media) != 1 || msg.Media[0] != "media1.jpg" {
			t.Errorf("Expected media ['media1.jpg'], got %v", msg.Media)
		}
	case <-time.After(300 * time.Millisecond):
		t.Error("No message received within timeout")
	}
}

// TestBaseChannelHandleMessageNotAllowed tests that messages from non-allowed senders are ignored
func TestBaseChannelHandleMessageNotAllowed(t *testing.T) {
	msgBus := bus.NewMessageBus()
	allowList := []string{"allowed-user"}
	channel := NewBaseChannel("test", nil, msgBus, allowList)

	received := make(chan bus.InboundMessage, 1)
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
		defer cancel()
		msg, ok := msgBus.ConsumeInbound(ctx)
		if ok {
			received <- msg
		}
	}()

	// Try to send from non-allowed user
	channel.HandleMessage("blocked-user", "chat456", "Hello!", nil, nil)

	// Give some time for async processing
	time.Sleep(50 * time.Millisecond)

	// Check if any message was received
	select {
	case <-received:
		t.Error("Message from non-allowed sender should not be published")
	default:
		// No message received, which is correct
	}
}

// TestBaseChannelAddSyncTarget tests adding sync targets
func TestBaseChannelAddSyncTarget(t *testing.T) {
	msgBus := bus.NewMessageBus()
	channel := NewBaseChannel("test", nil, msgBus, nil)
	target := NewMockChannel("target", nil)

	// Test adding a target
	err := channel.AddSyncTarget("target", target)
	if err != nil {
		t.Errorf("AddSyncTarget failed: %v", err)
	}

	// Verify target was added
	if len(channel.syncTargets) != 1 {
		t.Errorf("Expected 1 sync target, got %d", len(channel.syncTargets))
	}
	if channel.syncTargets["target"] != target {
		t.Error("Sync target not stored correctly")
	}
}

// TestBaseChannelAddSyncTargetSelfSync tests that self-sync is rejected
func TestBaseChannelAddSyncTargetSelfSync(t *testing.T) {
	msgBus := bus.NewMessageBus()
	sourceChannel := NewBaseChannel("source", nil, msgBus, nil)

	// Create a mock channel with the same name to test self-sync prevention
	sameNameChannel := NewMockChannel("source", nil)

	// Test adding a channel with the same name (should fail)
	err := sourceChannel.AddSyncTarget("source", sameNameChannel)
	if err == nil {
		t.Error("Expected error when adding channel with same name as sync target")
	}
	if err.Error() != "channel cannot sync to itself" {
		t.Errorf("Expected 'channel cannot sync to itself' error, got: %v", err)
	}

	// Verify no target was added
	if len(sourceChannel.syncTargets) != 0 {
		t.Errorf("Expected 0 sync targets after failed self-sync, got %d", len(sourceChannel.syncTargets))
	}
}

// TestBaseChannelRemoveSyncTarget tests removing sync targets
func TestBaseChannelRemoveSyncTarget(t *testing.T) {
	msgBus := bus.NewMessageBus()
	channel := NewBaseChannel("test", nil, msgBus, nil)
	target := NewMockChannel("target", nil)

	// Add target first
	channel.AddSyncTarget("target", target)

	// Remove target
	channel.RemoveSyncTarget("target")

	// Verify target was removed
	if len(channel.syncTargets) != 0 {
		t.Errorf("Expected 0 sync targets after removal, got %d", len(channel.syncTargets))
	}

	// Remove non-existent target (should not panic)
	channel.RemoveSyncTarget("non-existent")
	if len(channel.syncTargets) != 0 {
		t.Error("Removing non-existent target should not affect other targets")
	}
}

// TestBaseChannelSyncToTargets tests syncing messages to targets
func TestBaseChannelSyncToTargets(t *testing.T) {
	msgBus := bus.NewMessageBus()
	channel := NewBaseChannel("source", nil, msgBus, nil)

	target1 := NewMockChannel("target1", nil)
	target2 := NewMockChannel("target2", nil)

	// Add sync targets
	channel.AddSyncTarget("target1", target1)
	channel.AddSyncTarget("target2", target2)

	// Sync a message
	channel.SyncToTargets("user", "Test message")

	// Give some time for async send
	time.Sleep(100 * time.Millisecond)

	// Verify both targets received the message
	msgs1 := target1.GetSentMessages()
	msgs2 := target2.GetSentMessages()

	if len(msgs1) != 1 {
		t.Errorf("Expected target1 to receive 1 message, got %d", len(msgs1))
	} else {
		if msgs1[0].Content != "Test message" {
			t.Errorf("Expected message 'Test message', got '%s'", msgs1[0].Content)
		}
	}

	if len(msgs2) != 1 {
		t.Errorf("Expected target2 to receive 1 message, got %d", len(msgs2))
	}
}

// TestBaseChannelSyncToTargetsWithWebChannel tests syncing to web channel with broadcast
func TestBaseChannelSyncToTargetsWithWebChannel(t *testing.T) {
	msgBus := bus.NewMessageBus()
	channel := NewBaseChannel("source", nil, msgBus, nil)
	webChannel := NewMockChannel("web", nil)

	channel.AddSyncTarget("web", webChannel)

	// Sync a message
	channel.SyncToTargets("assistant", "Broadcast message")

	time.Sleep(100 * time.Millisecond)

	// Verify web channel received with broadcast chat ID
	msgs := webChannel.GetSentMessages()
	if len(msgs) != 1 {
		t.Fatalf("Expected 1 message, got %d", len(msgs))
	}

	if msgs[0].ChatID != "web:broadcast" {
		t.Errorf("Expected chat ID 'web:broadcast', got '%s'", msgs[0].ChatID)
	}

	if msgs[0].Content != "Broadcast message" {
		t.Errorf("Expected content 'Broadcast message', got '%s'", msgs[0].Content)
	}
}

// TestBaseChannelSyncToTargetsWithNoTargets tests syncing with no targets configured
func TestBaseChannelSyncToTargetsWithNoTargets(t *testing.T) {
	msgBus := bus.NewMessageBus()
	channel := NewBaseChannel("source", nil, msgBus, nil)

	// Sync with no targets (should not panic)
	channel.SyncToTargets("user", "Test message")

	// Verify no targets stored
	if len(channel.syncTargets) != 0 {
		t.Errorf("Expected 0 sync targets, got %d", len(channel.syncTargets))
	}
}

// TestBaseChannelSyncToTargetsSendError tests handling of send errors when syncing
func TestBaseChannelSyncToTargetsSendError(t *testing.T) {
	msgBus := bus.NewMessageBus()
	channel := NewBaseChannel("source", nil, msgBus, nil)

	// Create a mock that always fails to send
	failingTarget := &FailingMockChannel{name: "failing"}
	channel.AddSyncTarget("failing", failingTarget)

	// Sync should not panic even if target fails
	channel.SyncToTargets("user", "Test message")

	// Give time for async operations
	time.Sleep(100 * time.Millisecond)
}

// FailingMockChannel is a mock that always fails to send
type FailingMockChannel struct {
	name    string
	running bool
}

func (f *FailingMockChannel) Name() string {
	return f.name
}

func (f *FailingMockChannel) Start(ctx context.Context) error {
	f.running = true
	return nil
}

func (f *FailingMockChannel) Stop(ctx context.Context) error {
	f.running = false
	return nil
}

func (f *FailingMockChannel) Send(ctx context.Context, msg bus.OutboundMessage) error {
	return context.DeadlineExceeded
}

func (f *FailingMockChannel) IsRunning() bool {
	return f.running
}

func (f *FailingMockChannel) IsAllowed(senderID string) bool {
	return true
}

func (f *FailingMockChannel) AddSyncTarget(name string, channel Channel) error {
	return nil
}

func (f *FailingMockChannel) RemoveSyncTarget(name string) {
}
