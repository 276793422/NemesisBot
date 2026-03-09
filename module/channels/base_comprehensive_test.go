// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package channels

import (
	"context"
	"fmt"
	"sync"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
)

// ============================================================================
// Base Channel Thread Safety Tests
// ============================================================================

func TestBaseChannel_ConcurrentHandleMessage(t *testing.T) {
	msgBus := bus.NewMessageBus()
	channel := NewBaseChannel("test", nil, msgBus, nil)

	// Subscribe to inbound messages
	receivedCount := 0
	receivedMu := sync.Mutex{}

	done := make(chan bool)
	go func() {
		for {
			ctx, cancel := context.WithTimeout(context.Background(), 500*time.Millisecond)
			_, ok := msgBus.ConsumeInbound(ctx)
			cancel()

			if !ok {
				break
			}

			receivedMu.Lock()
			receivedCount++
			receivedMu.Unlock()
		}
		done <- true
	}()

	// Send multiple concurrent messages
	const numMessages = 100
	var wg sync.WaitGroup
	wg.Add(numMessages)

	for i := 0; i < numMessages; i++ {
		go func(index int) {
			defer wg.Done()
			channel.HandleMessage(
				"sender123",
				"chat456",
				"message content",
				nil,
				map[string]string{"index": string(rune('0' + index))},
			)
		}(i)
	}

	wg.Wait()

	// Give consumer time to finish
	time.Sleep(100 * time.Millisecond)

	receivedMu.Lock()
	count := receivedCount
	receivedMu.Unlock()

	if count != numMessages {
		t.Errorf("Expected %d messages, received %d", numMessages, count)
	}
}

func TestBaseChannel_ConcurrentSyncTargets(t *testing.T) {
	msgBus := bus.NewMessageBus()
	channel := NewBaseChannel("source", nil, msgBus, nil)

	// Add and remove sync targets concurrently
	var wg sync.WaitGroup

	// Add targets
	for i := 0; i < 10; i++ {
		wg.Add(1)
		go func(index int) {
			defer wg.Done()
			name := channelNameForIndex(index)
			target := NewMockChannel(name, nil)
			channel.AddSyncTarget(name, target)
		}(i)
	}

	// Remove targets
	for i := 0; i < 5; i++ {
		wg.Add(1)
		go func(index int) {
			defer wg.Done()
			name := channelNameForIndex(index)
			channel.RemoveSyncTarget(name)
		}(i)
	}

	wg.Wait()

	// Verify no data races occurred
	time.Sleep(50 * time.Millisecond)
}

// ============================================================================
// Base Channel IsAllowed Edge Cases
// ============================================================================

func TestBaseChannel_IsAllowed_SpecialCharacters(t *testing.T) {
	tests := []struct {
		name      string
		allowList []string
		senderID  string
		expected  bool
	}{
		{
			name:      "Special characters in senderID",
			allowList: []string{"user@domain.com", "user|name"},
			senderID:  "user|name",
			expected:  true,
		},
		{
			name:      "Unicode characters",
			allowList: []string{"用户123", "user中国"},
			senderID:  "用户123",
			expected:  true,
		},
		{
			name:      "Empty senderID with empty allowlist",
			allowList: []string{},
			senderID:  "",
			expected:  true,
		},
		{
			name:      "Empty senderID with populated allowlist",
			allowList: []string{"user1", "user2"},
			senderID:  "",
			expected:  false,
		},
		{
			name:      "Multiple pipe characters",
			allowList: []string{"123|user|extra"},
			senderID:  "123|user|extra",
			expected:  true,
		},
		{
			name:      "Leading/trailing spaces in allowlist",
			allowList: []string{" user1 ", " user2"},
			senderID:  "user1",
			expected:  false, // Exact match required
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

// ============================================================================
// Base Channel HandleMessage Edge Cases
// ============================================================================

func TestBaseChannel_HandleMessage_WithMetadata(t *testing.T) {
	msgBus := bus.NewMessageBus()
	channel := NewBaseChannel("test", nil, msgBus, nil)

	received := make(chan bus.InboundMessage, 1)
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 200*time.Millisecond)
		defer cancel()
		msg, ok := msgBus.ConsumeInbound(ctx)
		if ok {
			received <- msg
		}
	}()

	metadata := map[string]string{
		"key1":        "value1",
		"key2":        "value2",
		"user_id":     "123456",
		"guild_id":    "guild789",
		"message_id":  "msg456",
		"timestamp":   "2024-01-01T00:00:00Z",
		"custom_data": "custom_value",
	}

	channel.HandleMessage("sender123", "chat456", "Test message", nil, metadata)

	select {
	case msg := <-received:
		if len(msg.Metadata) != len(metadata) {
			t.Errorf("Expected %d metadata fields, got %d", len(metadata), len(msg.Metadata))
		}

		for key, expectedValue := range metadata {
			actualValue, exists := msg.Metadata[key]
			if !exists {
				t.Errorf("Metadata key '%s' not found", key)
			} else if actualValue != expectedValue {
				t.Errorf("Metadata key '%s': expected '%s', got '%s'", key, expectedValue, actualValue)
			}
		}
	case <-time.After(300 * time.Millisecond):
		t.Error("No message received within timeout")
	}
}

func TestBaseChannel_HandleMessage_EmptyContent(t *testing.T) {
	msgBus := bus.NewMessageBus()
	channel := NewBaseChannel("test", nil, msgBus, nil)

	received := make(chan bus.InboundMessage, 1)
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 200*time.Millisecond)
		defer cancel()
		msg, ok := msgBus.ConsumeInbound(ctx)
		if ok {
			received <- msg
		}
	}()

	// Send message with empty content
	channel.HandleMessage("sender123", "chat456", "", nil, nil)

	select {
	case msg := <-received:
		if msg.Content != "" {
			t.Errorf("Expected empty content, got '%s'", msg.Content)
		}
	case <-time.After(300 * time.Millisecond):
		t.Error("No message received within timeout")
	}
}

func TestBaseChannel_HandleMessage_WithMedia(t *testing.T) {
	msgBus := bus.NewMessageBus()
	channel := NewBaseChannel("test", nil, msgBus, nil)

	received := make(chan bus.InboundMessage, 1)
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 200*time.Millisecond)
		defer cancel()
		msg, ok := msgBus.ConsumeInbound(ctx)
		if ok {
			received <- msg
		}
	}()

	media := []string{
		"https://example.com/image1.jpg",
		"https://example.com/image2.png",
		"https://example.com/video.mp4",
		"https://example.com/audio.mp3",
	}

	channel.HandleMessage("sender123", "chat456", "Check out these files!", media, nil)

	select {
	case msg := <-received:
		if len(msg.Media) != len(media) {
			t.Errorf("Expected %d media items, got %d", len(media), len(msg.Media))
		}

		for i, expectedMedia := range media {
			if msg.Media[i] != expectedMedia {
				t.Errorf("Media item %d: expected '%s', got '%s'", i, expectedMedia, msg.Media[i])
			}
		}
	case <-time.After(300 * time.Millisecond):
		t.Error("No message received within timeout")
	}
}

func TestBaseChannel_HandleMessage_NilMetadata(t *testing.T) {
	msgBus := bus.NewMessageBus()
	channel := NewBaseChannel("test", nil, msgBus, nil)

	received := make(chan bus.InboundMessage, 1)
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 200*time.Millisecond)
		defer cancel()
		msg, ok := msgBus.ConsumeInbound(ctx)
		if ok {
			received <- msg
		}
	}()

	// Send with nil metadata
	channel.HandleMessage("sender123", "chat456", "Test message", nil, nil)

	select {
	case msg := <-received:
		if msg.Metadata != nil {
			t.Error("Expected nil metadata, got non-nil")
		}
	case <-time.After(300 * time.Millisecond):
		t.Error("No message received within timeout")
	}
}

func TestBaseChannel_HandleMessage_EmptyMediaArray(t *testing.T) {
	msgBus := bus.NewMessageBus()
	channel := NewBaseChannel("test", nil, msgBus, nil)

	received := make(chan bus.InboundMessage, 1)
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 200*time.Millisecond)
		defer cancel()
		msg, ok := msgBus.ConsumeInbound(ctx)
		if ok {
			received <- msg
		}
	}()

	// Send with empty media array
	channel.HandleMessage("sender123", "chat456", "Test message", []string{}, nil)

	select {
	case msg := <-received:
		if len(msg.Media) != 0 {
			t.Errorf("Expected empty media array, got %d items", len(msg.Media))
		}
	case <-time.After(300 * time.Millisecond):
		t.Error("No message received within timeout")
	}
}

// ============================================================================
// Base Channel SyncToTargets Edge Cases
// ============================================================================

func TestBaseChannel_SyncToTargets_WithWebChannelBroadcast(t *testing.T) {
	msgBus := bus.NewMessageBus()
	channel := NewBaseChannel("source", nil, msgBus, nil)

	webChannel := NewMockChannel("web", nil)
	channel.AddSyncTarget("web", webChannel)

	// Sync to targets
	channel.SyncToTargets("assistant", "Broadcast this message")

	time.Sleep(100 * time.Millisecond)

	msgs := webChannel.GetSentMessages()
	if len(msgs) != 1 {
		t.Fatalf("Expected 1 message, got %d", len(msgs))
	}

	if msgs[0].ChatID != "web:broadcast" {
		t.Errorf("Expected chat ID 'web:broadcast', got '%s'", msgs[0].ChatID)
	}
}

func TestBaseChannel_SyncToTargets_LongContent(t *testing.T) {
	msgBus := bus.NewMessageBus()
	channel := NewBaseChannel("source", nil, msgBus, nil)

	target := NewMockChannel("target", nil)
	channel.AddSyncTarget("target", target)

	// Create very long content
	longContent := ""
	for i := 0; i < 10000; i++ {
		longContent += "test "
	}

	channel.SyncToTargets("assistant", longContent)

	time.Sleep(100 * time.Millisecond)

	msgs := target.GetSentMessages()
	if len(msgs) != 1 {
		t.Fatalf("Expected 1 message, got %d", len(msgs))
	}

	if len(msgs[0].Content) != len(longContent) {
		t.Errorf("Content length mismatch: expected %d, got %d", len(longContent), len(msgs[0].Content))
	}
}

func TestBaseChannel_SyncToTargets_SpecialCharacters(t *testing.T) {
	msgBus := bus.NewMessageBus()
	channel := NewBaseChannel("source", nil, msgBus, nil)

	target := NewMockChannel("target", nil)
	channel.AddSyncTarget("target", target)

	// Content with special characters
	specialContent := "Test with special chars: <>&\"'`\n\t\r日本語🎉"

	channel.SyncToTargets("assistant", specialContent)

	time.Sleep(100 * time.Millisecond)

	msgs := target.GetSentMessages()
	if len(msgs) != 1 {
		t.Fatalf("Expected 1 message, got %d", len(msgs))
	}

	if msgs[0].Content != specialContent {
		t.Errorf("Content mismatch: expected '%s', got '%s'", specialContent, msgs[0].Content)
	}
}

func TestBaseChannel_SyncToTargets_EmptyRole(t *testing.T) {
	msgBus := bus.NewMessageBus()
	channel := NewBaseChannel("source", nil, msgBus, nil)

	target := NewMockChannel("target", nil)
	channel.AddSyncTarget("target", target)

	channel.SyncToTargets("", "Test message")

	time.Sleep(100 * time.Millisecond)

	msgs := target.GetSentMessages()
	if len(msgs) != 1 {
		t.Fatalf("Expected 1 message, got %d", len(msgs))
	}
}

func TestBaseChannel_SyncToTargets_EmptyContent(t *testing.T) {
	msgBus := bus.NewMessageBus()
	channel := NewBaseChannel("source", nil, msgBus, nil)

	target := NewMockChannel("target", nil)
	channel.AddSyncTarget("target", target)

	channel.SyncToTargets("assistant", "")

	time.Sleep(100 * time.Millisecond)

	msgs := target.GetSentMessages()
	if len(msgs) != 1 {
		t.Fatalf("Expected 1 message, got %d", len(msgs))
	}

	if msgs[0].Content != "" {
		t.Errorf("Expected empty content, got '%s'", msgs[0].Content)
	}
}

// ============================================================================
// Base Channel AddSyncTarget Edge Cases
// ============================================================================

func TestBaseChannel_AddSyncTarget_DuplicateTarget(t *testing.T) {
	msgBus := bus.NewMessageBus()
	channel := NewBaseChannel("source", nil, msgBus, nil)

	target := NewMockChannel("target", nil)

	// Add same target twice
	err := channel.AddSyncTarget("target", target)
	if err != nil {
		t.Errorf("First AddSyncTarget failed: %v", err)
	}

	err = channel.AddSyncTarget("target", target)
	if err != nil {
		t.Errorf("Second AddSyncTarget failed: %v", err)
	}

	// Should still have only 1 target
	if len(channel.syncTargets) != 1 {
		t.Errorf("Expected 1 sync target, got %d", len(channel.syncTargets))
	}
}

func TestBaseChannel_AddSyncTarget_MultipleTargets(t *testing.T) {
	msgBus := bus.NewMessageBus()
	channel := NewBaseChannel("source", nil, msgBus, nil)

	// Add multiple targets
	for i := 0; i < 10; i++ {
		name := channelNameForIndex(i)
		target := NewMockChannel(name, nil)
		err := channel.AddSyncTarget(name, target)
		if err != nil {
			t.Errorf("AddSyncTarget for '%s' failed: %v", name, err)
		}
	}

	if len(channel.syncTargets) != 10 {
		t.Errorf("Expected 10 sync targets, got %d", len(channel.syncTargets))
	}
}

// ============================================================================
// Base Channel RemoveSyncTarget Edge Cases
// ============================================================================

func TestBaseChannel_RemoveSyncTarget_MultipleRemovals(t *testing.T) {
	msgBus := bus.NewMessageBus()
	channel := NewBaseChannel("source", nil, msgBus, nil)

	// Add targets
	for i := 0; i < 5; i++ {
		name := channelNameForIndex(i)
		target := NewMockChannel(name, nil)
		channel.AddSyncTarget(name, target)
	}

	// Remove all targets
	for i := 0; i < 5; i++ {
		name := channelNameForIndex(i)
		channel.RemoveSyncTarget(name)
	}

	if len(channel.syncTargets) != 0 {
		t.Errorf("Expected 0 sync targets after removal, got %d", len(channel.syncTargets))
	}

	// Remove again (should not panic)
	for i := 0; i < 5; i++ {
		name := channelNameForIndex(i)
		channel.RemoveSyncTarget(name)
	}
}

func TestBaseChannel_RemoveSyncTarget_WhileSyncing(t *testing.T) {
	msgBus := bus.NewMessageBus()
	channel := NewBaseChannel("source", nil, msgBus, nil)

	target := NewMockChannel("target", nil)
	channel.AddSyncTarget("target", target)

	// Start syncing in background
	done := make(chan bool)
	go func() {
		for i := 0; i < 10; i++ {
			channel.SyncToTargets("assistant", "Test message")
			time.Sleep(10 * time.Millisecond)
		}
		done <- true
	}()

	// Remove target while syncing
	time.Sleep(25 * time.Millisecond)
	channel.RemoveSyncTarget("target")

	<-done

	// Verify no panic occurred
}

// ============================================================================
// Helper Functions
// ============================================================================

func channelNameForIndex(index int) string {
	names := []string{"alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta", "iota", "kappa"}
	if index < len(names) {
		return names[index]
	}
	return fmt.Sprintf("channel%d", index)
}
