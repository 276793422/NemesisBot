// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package channels

import (
	"context"
	"testing"

	"github.com/276793422/NemesisBot/module/bus"
)

func TestBaseChannel_Name(t *testing.T) {
	channel := NewBaseChannel("test-channel", nil, nil, nil)

	if channel.Name() != "test-channel" {
		t.Errorf("Expected name 'test-channel', got '%s'", channel.Name())
	}
}

func TestBaseChannel_IsRunning_Default(t *testing.T) {
	channel := NewBaseChannel("test-channel", nil, nil, nil)

	if channel.IsRunning() {
		t.Error("Expected IsRunning to be false by default")
	}
}

func TestBaseChannel_IsAllowed_EmptyAllowList(t *testing.T) {
	channel := NewBaseChannel("test-channel", nil, nil, []string{})

	// Empty allow list means no restrictions
	if !channel.IsAllowed("any-sender") {
		t.Error("Expected all senders to be allowed when allow list is empty")
	}
}

func TestBaseChannel_IsAllowed_NilAllowList(t *testing.T) {
	channel := NewBaseChannel("test-channel", nil, nil, nil)

	// Nil allow list means no restrictions
	if !channel.IsAllowed("any-sender") {
		t.Error("Expected all senders to be allowed when allow list is nil")
	}
}

func TestBaseChannel_IsAllowed_WithAllowList(t *testing.T) {
	allowList := []string{"user1", "user2"}
	channel := NewBaseChannel("test-channel", nil, nil, allowList)

	if !channel.IsAllowed("user1") {
		t.Error("Expected user1 to be allowed")
	}

	if !channel.IsAllowed("user2") {
		t.Error("Expected user2 to be allowed")
	}

	if channel.IsAllowed("user3") {
		t.Error("Expected user3 to be denied")
	}
}

func TestBaseChannel_AddSyncTarget(t *testing.T) {
	channel := NewBaseChannel("test-channel", nil, nil, nil)
	// Create a mock channel that implements the full Channel interface
	mockChannel := &mockChannel{name: "target-channel"}

	err := channel.AddSyncTarget("target", mockChannel)
	if err != nil {
		t.Fatalf("AddSyncTarget failed: %v", err)
	}

	// Verify target was added by testing removal
	channel.RemoveSyncTarget("target")
}

func TestBaseChannel_RemoveSyncTarget(t *testing.T) {
	channel := NewBaseChannel("test-channel", nil, nil, nil)
	mockChannel := &mockChannel{name: "target-channel"}

	// Add then remove
	_ = channel.AddSyncTarget("target", mockChannel)
	channel.RemoveSyncTarget("target")

	// If this doesn't panic, the removal was successful
}

func TestBaseChannel_AddSyncTarget_Duplicate(t *testing.T) {
	channel := NewBaseChannel("test-channel", nil, nil, nil)
	mockChannel := &mockChannel{name: "target-channel"}

	_ = channel.AddSyncTarget("target", mockChannel)
	err := channel.AddSyncTarget("target", mockChannel)

	// Adding duplicate should succeed (it just overwrites)
	if err != nil {
		t.Errorf("Expected no error when adding duplicate sync target, got %v", err)
	}
}

func TestBaseChannel_AddSyncTarget_Self(t *testing.T) {
	channel := NewBaseChannel("test-channel", nil, nil, nil)
	mockChannel := &mockChannel{name: "test-channel"}

	// Try to add a channel with the same name as the parent channel
	err := channel.AddSyncTarget("test-channel", mockChannel)
	if err == nil {
		t.Error("Expected error when adding channel with same name as sync target")
	}
}

// mockChannel is a minimal implementation of Channel interface for testing
type mockChannel struct {
	name string
}

func (m *mockChannel) Name() string {
	return m.name
}

func (m *mockChannel) Start(ctx context.Context) error {
	return nil
}

func (m *mockChannel) Stop(ctx context.Context) error {
	return nil
}

func (m *mockChannel) Send(ctx context.Context, msg bus.OutboundMessage) error {
	return nil
}

func (m *mockChannel) IsRunning() bool {
	return false
}

func (m *mockChannel) IsAllowed(senderID string) bool {
	return true
}

func (m *mockChannel) AddSyncTarget(name string, channel Channel) error {
	return nil
}

func (m *mockChannel) RemoveSyncTarget(name string) {
}

func TestBaseChannel_WithMessageBus(t *testing.T) {
	msgBus := bus.NewMessageBus()
	channel := NewBaseChannel("test-channel", nil, msgBus, nil)

	if channel.Name() != "test-channel" {
		t.Errorf("Expected name 'test-channel', got '%s'", channel.Name())
	}

	if channel.IsRunning() {
		t.Error("Expected IsRunning to be false")
	}
}

func TestBaseChannel_WithConfig(t *testing.T) {
	config := map[string]string{
		"key1": "value1",
		"key2": "value2",
	}

	channel := NewBaseChannel("test-channel", config, nil, nil)

	if channel.Name() != "test-channel" {
		t.Errorf("Expected name 'test-channel', got '%s'", channel.Name())
	}

	if channel.IsRunning() {
		t.Error("Expected IsRunning to be false")
	}
}

func TestBaseChannel_MultipleSyncTargets(t *testing.T) {
	channel := NewBaseChannel("test-channel", nil, nil, nil)
	mock1 := &mockChannel{name: "target1"}
	mock2 := &mockChannel{name: "target2"}
	mock3 := &mockChannel{name: "target3"}

	_ = channel.AddSyncTarget("target1", mock1)
	_ = channel.AddSyncTarget("target2", mock2)
	_ = channel.AddSyncTarget("target3", mock3)

	// Remove all
	channel.RemoveSyncTarget("target1")
	channel.RemoveSyncTarget("target2")
	channel.RemoveSyncTarget("target3")
}

func TestBaseChannel_AllowListCaseSensitivity(t *testing.T) {
	allowList := []string{"User1", "User2"}
	channel := NewBaseChannel("test-channel", nil, nil, allowList)

	// Test exact match
	if !channel.IsAllowed("User1") {
		t.Error("Expected 'User1' to be allowed (exact match)")
	}

	// Test case sensitivity
	if channel.IsAllowed("user1") {
		t.Error("Expected 'user1' to be denied (case mismatch)")
	}

	if channel.IsAllowed("USER1") {
		t.Error("Expected 'USER1' to be denied (case mismatch)")
	}
}

func TestBaseChannel_EmptyChannelName(t *testing.T) {
	channel := NewBaseChannel("", nil, nil, nil)

	if channel.Name() != "" {
		t.Errorf("Expected empty name, got '%s'", channel.Name())
	}

	// Empty name should still allow basic operations
	if !channel.IsAllowed("any-sender") {
		t.Error("Expected all senders to be allowed with empty channel name")
	}
}

func TestBaseChannel_SyncTargetNil(t *testing.T) {
	channel := NewBaseChannel("test-channel", nil, nil, nil)

	// Test with nil channel - should panic or handle gracefully
	defer func() {
		if r := recover(); r != nil {
			// Expected to panic with nil channel
			t.Log("Panicked as expected with nil channel:", r)
		}
	}()

	_ = channel.AddSyncTarget("nil-target", nil)
}

func TestChannelInterface(t *testing.T) {
	// Verify that mockChannel implements the Channel interface
	var _ Channel = &mockChannel{name: "test"}

	// BaseChannel alone doesn't implement the full Channel interface
	// It only provides common functionality used by concrete channel implementations
	baseChannel := NewBaseChannel("test", nil, nil, nil)
	if baseChannel == nil {
		t.Error("NewBaseChannel should not return nil")
	}
}
