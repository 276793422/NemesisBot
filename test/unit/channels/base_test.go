// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package channels_test

import (
	"context"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/channels"
)

// testMockChannel is a minimal implementation of the Channel interface for testing base channel
// Named testMockChannel to avoid conflict with external_test.go's mockChannel
type testMockChannel struct {
	name        string
	running     bool
	allowList   []string
	syncTargets map[string]channels.Channel
	sendFunc    func(context.Context, bus.OutboundMessage) error
}

func newTestMockChannel(name string, allowList []string) *testMockChannel {
	return &testMockChannel{
		name:        name,
		allowList:   allowList,
		syncTargets: make(map[string]channels.Channel),
		sendFunc:    func(ctx context.Context, msg bus.OutboundMessage) error { return nil },
	}
}

func (m *testMockChannel) Name() string {
	return m.name
}

func (m *testMockChannel) Start(ctx context.Context) error {
	m.running = true
	return nil
}

func (m *testMockChannel) Stop(ctx context.Context) error {
	m.running = false
	return nil
}

func (m *testMockChannel) Send(ctx context.Context, msg bus.OutboundMessage) error {
	return m.sendFunc(ctx, msg)
}

func (m *testMockChannel) IsRunning() bool {
	return m.running
}

func (m *testMockChannel) IsAllowed(senderID string) bool {
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

func (m *testMockChannel) AddSyncTarget(name string, channel channels.Channel) error {
	if name == m.name {
		return nil // Self-sync prevention
	}
	m.syncTargets[name] = channel
	return nil
}

func (m *testMockChannel) RemoveSyncTarget(name string) {
	delete(m.syncTargets, name)
}

// TestNewBaseChannel tests creating a new base channel
func TestNewBaseChannel(t *testing.T) {
	msgBus := bus.NewMessageBus()
	allowList := []string{"user1", "user2"}

	base := channels.NewBaseChannel("test", nil, msgBus, allowList)
	if base == nil {
		t.Fatal("Expected base channel to be created, got nil")
	}

	if base.Name() != "test" {
		t.Errorf("Expected name 'test', got '%s'", base.Name())
	}

	if base.IsRunning() {
		t.Error("Expected new base channel to not be running")
	}
}

// TestBaseChannelName tests the Name method
func TestBaseChannelName(t *testing.T) {
	msgBus := bus.NewMessageBus()
	base := channels.NewBaseChannel("test-channel", nil, msgBus, nil)

	if base.Name() != "test-channel" {
		t.Errorf("Expected name 'test-channel', got '%s'", base.Name())
	}
}

// TestBaseChannelIsRunning tests the IsRunning and setRunning methods
func TestBaseChannelIsRunning(t *testing.T) {
	msgBus := bus.NewMessageBus()
	base := channels.NewBaseChannel("test", nil, msgBus, nil)

	if base.IsRunning() {
		t.Error("Expected new base channel to not be running")
	}

	// Note: setRunning is not exported, so we can't test it directly
	// This is tested implicitly through the concrete channel implementations
}

// TestBaseChannelIsAllowed tests the IsAllowed method
func TestBaseChannelIsAllowed(t *testing.T) {
	msgBus := bus.NewMessageBus()
	allowList := []string{"user1", "user2", "user3"}
	base := channels.NewBaseChannel("test", nil, msgBus, allowList)

	tests := []struct {
		name     string
		senderID string
		allowed  bool
	}{
		{"Allowed user1", "user1", true},
		{"Allowed user2", "user2", true},
		{"Allowed user3", "user3", true},
		{"Denied user4", "user4", false},
		{"Denied empty", "", false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := base.IsAllowed(tt.senderID)
			if result != tt.allowed {
				t.Errorf("IsAllowed(%s) = %v, want %v", tt.senderID, result, tt.allowed)
			}
		})
	}
}

// TestBaseChannelIsAllowedEmptyAllowList tests that empty allowlist allows all
func TestBaseChannelIsAllowedEmptyAllowList(t *testing.T) {
	msgBus := bus.NewMessageBus()
	base := channels.NewBaseChannel("test", nil, msgBus, []string{})

	tests := []struct {
		name     string
		senderID string
		allowed  bool
	}{
		{"Any user", "random_user", true},
		{"Empty sender", "", true},
		{"Special chars", "user@123", true},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := base.IsAllowed(tt.senderID)
			if result != tt.allowed {
				t.Errorf("IsAllowed(%s) = %v, want %v", tt.senderID, result, tt.allowed)
			}
		})
	}
}

// TestBaseChannelIsAllowedComplexSenderID tests complex sender ID formats
func TestBaseChannelIsAllowedComplexSenderID(t *testing.T) {
	msgBus := bus.NewMessageBus()
	// Test with compound IDs like "123456|username"
	allowList := []string{"123456|user1", "@user2", "789012|user3"}
	base := channels.NewBaseChannel("test", nil, msgBus, allowList)

	tests := []struct {
		name     string
		senderID string
		allowed  bool
	}{
		{"Exact match compound", "123456|user1", true},
		{"ID part match", "123456", true},
		{"Username with @", "@user2", true},
		{"Username without @", "user2", true},
		{"Username match from compound", "user1", true},
		{"Different compound", "789012|user3", true},
		{"Different ID part", "789012", true},
		{"Different username", "user3", true},
		{"Not allowed", "999999|user4", false},
		{"Partial match fail", "12345", false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := base.IsAllowed(tt.senderID)
			if result != tt.allowed {
				t.Errorf("IsAllowed(%s) = %v, want %v", tt.senderID, result, tt.allowed)
			}
		})
	}
}

// TestBaseChannelHandleMessage tests the HandleMessage method
func TestBaseChannelHandleMessage(t *testing.T) {
	msgBus := bus.NewMessageBus()
	allowList := []string{"user1"}
	base := channels.NewBaseChannel("test", nil, msgBus, allowList)

	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()

	// Start consuming inbound messages
	received := make(chan bus.InboundMessage, 1)
	go func() {
		msg, ok := msgBus.ConsumeInbound(ctx)
		if ok {
			received <- msg
		}
	}()

	// Test allowed user
	base.HandleMessage("user1", "chat1", "Hello", []string{"media1"}, nil)

	select {
	case msg := <-received:
		if msg.Channel != "test" {
			t.Errorf("Expected channel 'test', got '%s'", msg.Channel)
		}
		if msg.SenderID != "user1" {
			t.Errorf("Expected sender 'user1', got '%s'", msg.SenderID)
		}
		if msg.Content != "Hello" {
			t.Errorf("Expected content 'Hello', got '%s'", msg.Content)
		}
	case <-time.After(1 * time.Second):
		t.Fatal("Timeout waiting for message")
	}

	// Test denied user - should not publish
	base.HandleMessage("user2", "chat1", "Hello", nil, nil)

	select {
	case <-received:
		t.Error("Should not receive message from denied user")
	case <-time.After(100 * time.Millisecond):
		// Expected - no message
	}
}

// TestBaseChannelAddSyncTarget tests adding sync targets
func TestBaseChannelAddSyncTarget(t *testing.T) {
	msgBus := bus.NewMessageBus()
	base := channels.NewBaseChannel("source", nil, msgBus, nil)
	target := newTestMockChannel("target", nil)

	// Test adding valid target
	err := base.AddSyncTarget("target", target)
	if err != nil {
		t.Errorf("Failed to add sync target: %v", err)
	}

	// Test adding self (should fail)
	err = base.AddSyncTarget("source", newTestMockChannel("source", nil))
	if err == nil {
		t.Error("Expected error when adding self as sync target")
	}
}

// TestBaseChannelRemoveSyncTarget tests removing sync targets
func TestBaseChannelRemoveSyncTarget(t *testing.T) {
	msgBus := bus.NewMessageBus()
	base := channels.NewBaseChannel("source", nil, msgBus, nil)
	target := newTestMockChannel("target", nil)

	// Add then remove
	base.AddSyncTarget("target", target)
	base.RemoveSyncTarget("target")

	// Removing non-existent target should not panic
	base.RemoveSyncTarget("nonexistent")
}

// TestBaseChannelSyncToTargets tests syncing to target channels
func TestBaseChannelSyncToTargets(t *testing.T) {
	msgBus := bus.NewMessageBus()
	base := channels.NewBaseChannel("source", nil, msgBus, nil)

	// Create mock target channel
	received := make(chan bus.OutboundMessage, 1)
	target := newTestMockChannel("target", nil)
	target.sendFunc = func(ctx context.Context, msg bus.OutboundMessage) error {
		received <- msg
		return nil
	}

	base.AddSyncTarget("target", target)

	// Sync to targets
	base.SyncToTargets("assistant", "Test message")

	select {
	case msg := <-received:
		if msg.Channel != "target" {
			t.Errorf("Expected channel 'target', got '%s'", msg.Channel)
		}
		if msg.Content != "Test message" {
			t.Errorf("Expected content 'Test message', got '%s'", msg.Content)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("Timeout waiting for sync message")
	}
}

// TestBaseChannelSyncToWebChannel tests syncing specifically to web channel
func TestBaseChannelSyncToWebChannel(t *testing.T) {
	msgBus := bus.NewMessageBus()
	base := channels.NewBaseChannel("source", nil, msgBus, nil)

	received := make(chan bus.OutboundMessage, 1)
	webTarget := newTestMockChannel("web", nil)
	webTarget.sendFunc = func(ctx context.Context, msg bus.OutboundMessage) error {
		received <- msg
		return nil
	}

	base.AddSyncTarget("web", webTarget)
	base.SyncToTargets("assistant", "Broadcast message")

	select {
	case msg := <-received:
		if msg.ChatID != "web:broadcast" {
			t.Errorf("Expected chat ID 'web:broadcast', got '%s'", msg.ChatID)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("Timeout waiting for web sync message")
	}
}

// TestBaseChannelSyncToTargetsNoTargets tests syncing with no targets configured
func TestBaseChannelSyncToTargetsNoTargets(t *testing.T) {
	msgBus := bus.NewMessageBus()
	base := channels.NewBaseChannel("source", nil, msgBus, nil)

	// Should not panic when no targets configured
	base.SyncToTargets("assistant", "Test message")
}

// TestBaseChannelSyncToTargetsMultiple tests syncing to multiple targets
func TestBaseChannelSyncToTargetsMultiple(t *testing.T) {
	msgBus := bus.NewMessageBus()
	base := channels.NewBaseChannel("source", nil, msgBus, nil)

	received := make(chan bus.OutboundMessage, 10)

	target1 := newTestMockChannel("target1", nil)
	target1.sendFunc = func(ctx context.Context, msg bus.OutboundMessage) error {
		received <- msg
		return nil
	}

	target2 := newTestMockChannel("target2", nil)
	target2.sendFunc = func(ctx context.Context, msg bus.OutboundMessage) error {
		received <- msg
		return nil
	}

	base.AddSyncTarget("target1", target1)
	base.AddSyncTarget("target2", target2)

	base.SyncToTargets("assistant", "Test message")

	// Should receive 2 messages
	receivedCount := 0
	timeout := time.After(5 * time.Second)

	for receivedCount < 2 {
		select {
		case <-received:
			receivedCount++
		case <-timeout:
			t.Fatalf("Timeout waiting for sync messages, received %d/2", receivedCount)
		}
	}
}
