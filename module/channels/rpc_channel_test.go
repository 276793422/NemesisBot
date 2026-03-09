// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package channels

import (
	"context"
	"fmt"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
)

// TestNewRPCChannel tests the creation of a new RPC channel
func TestNewRPCChannel(t *testing.T) {
	msgBus := bus.NewMessageBus()

	tests := []struct {
		name    string
		config  *RPCChannelConfig
		wantErr bool
	}{
		{
			name:    "Nil config",
			config:  nil,
			wantErr: true,
		},
		{
			name: "Nil message bus",
			config: &RPCChannelConfig{
				MessageBus: nil,
			},
			wantErr: true,
		},
		{
			name: "Valid config with defaults",
			config: &RPCChannelConfig{
				MessageBus: msgBus,
			},
			wantErr: false,
		},
		{
			name: "Valid config with custom timeouts",
			config: &RPCChannelConfig{
				MessageBus:      msgBus,
				RequestTimeout:  30 * time.Second,
				CleanupInterval: 10 * time.Second,
			},
			wantErr: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			channel, err := NewRPCChannel(tt.config)

			if tt.wantErr {
				if err == nil {
					t.Errorf("NewRPCChannel() expected error, got nil")
				}
				return
			}

			if err != nil {
				t.Errorf("NewRPCChannel() unexpected error: %v", err)
				return
			}

			if channel == nil {
				t.Fatal("NewRPCChannel() returned nil channel")
			}

			if channel.Name() != "rpc" {
				t.Errorf("Expected name 'rpc', got '%s'", channel.Name())
			}

			if channel.IsRunning() {
				t.Error("New channel should not be running")
			}
		})
	}
}

// TestRPCChannelStartStop tests the Start and Stop lifecycle
func TestRPCChannelStartStop(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := &RPCChannelConfig{
		MessageBus:      msgBus,
		RequestTimeout:  5 * time.Second,
		CleanupInterval: 1 * time.Second,
	}

	channel, err := NewRPCChannel(cfg)
	if err != nil {
		t.Fatalf("Failed to create RPC channel: %v", err)
	}

	ctx := context.Background()

	// Start the channel
	if err := channel.Start(ctx); err != nil {
		t.Fatalf("Failed to start channel: %v", err)
	}

	if !channel.IsRunning() {
		t.Error("Channel should be running after Start()")
	}

	// Start again should fail
	if err := channel.Start(ctx); err == nil {
		t.Error("Expected error when starting already running channel")
	}

	// Stop the channel
	if err := channel.Stop(ctx); err != nil {
		t.Fatalf("Failed to stop channel: %v", err)
	}

	if channel.IsRunning() {
		t.Error("Channel should not be running after Stop()")
	}

	// Stop again should succeed (idempotent)
	if err := channel.Stop(ctx); err != nil {
		t.Errorf("Stop() should be idempotent, got error: %v", err)
	}
}

// TestRPCChannelInput tests the Input method for submitting requests
func TestRPCChannelInput(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := &RPCChannelConfig{
		MessageBus:      msgBus,
		RequestTimeout:  5 * time.Second,
		CleanupInterval: 1 * time.Second,
	}

	channel, err := NewRPCChannel(cfg)
	if err != nil {
		t.Fatalf("Failed to create RPC channel: %v", err)
	}

	ctx := context.Background()

	// Input when not running should fail
	inbound := &bus.InboundMessage{
		Content: "Test message",
		ChatID:  "test-chat",
	}
	_, err = channel.Input(ctx, inbound)
	if err == nil {
		t.Error("Expected error when Input is called on stopped channel")
	}

	// Start the channel
	if err := channel.Start(ctx); err != nil {
		t.Fatalf("Failed to start channel: %v", err)
	}
	defer channel.Stop(ctx)

	// Valid input should succeed
	respCh, err := channel.Input(ctx, inbound)
	if err != nil {
		t.Errorf("Input() failed: %v", err)
	}

	if respCh == nil {
		t.Error("Input() returned nil response channel")
	}

	// Verify correlation ID was set
	if inbound.CorrelationID == "" {
		t.Error("Input() should set correlation ID")
	}

	if inbound.Channel != "rpc" {
		t.Errorf("Expected channel 'rpc', got '%s'", inbound.Channel)
	}

	// Verify message was published to message bus
	received := false
	done := make(chan struct{})
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
		defer cancel()
		msg, ok := msgBus.ConsumeInbound(ctx)
		if ok {
			received = true
			if msg.Content != "Test message" {
				t.Errorf("Expected content 'Test message', got '%s'", msg.Content)
			}
			if msg.Channel != "rpc" {
				t.Errorf("Expected channel 'rpc', got '%s'", msg.Channel)
			}
		}
		close(done)
	}()

	select {
	case <-done:
		if !received {
			t.Error("Message was not published to message bus")
		}
	case <-time.After(200 * time.Millisecond):
		t.Error("Timeout waiting for message on bus")
	}
}

// TestRPCChannelInputWithCustomCorrelationID tests Input with pre-set correlation ID
func TestRPCChannelInputWithCustomCorrelationID(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := &RPCChannelConfig{
		MessageBus: msgBus,
	}

	channel, err := NewRPCChannel(cfg)
	if err != nil {
		t.Fatalf("Failed to create RPC channel: %v", err)
	}

	ctx := context.Background()
	if err := channel.Start(ctx); err != nil {
		t.Fatalf("Failed to start channel: %v", err)
	}
	defer channel.Stop(ctx)

	customID := "custom-correlation-id-123"
	inbound := &bus.InboundMessage{
		Content:       "Test message",
		ChatID:        "test-chat",
		CorrelationID: customID,
	}

	respCh, err := channel.Input(ctx, inbound)
	if err != nil {
		t.Errorf("Input() failed: %v", err)
	}

	if inbound.CorrelationID != customID {
		t.Errorf("Correlation ID was not preserved, expected '%s', got '%s'", customID, inbound.CorrelationID)
	}

	// Send response directly via Send method
	go func() {
		time.Sleep(50 * time.Millisecond)
		outbound := bus.OutboundMessage{
			Channel: "rpc",
			Content: fmt.Sprintf("[rpc:%s] Response message", customID),
		}
		if err := channel.Send(ctx, outbound); err != nil {
			t.Errorf("Failed to send response: %v", err)
		}
	}()

	select {
	case resp := <-respCh:
		if resp != "Response message" {
			t.Errorf("Expected response 'Response message', got '%s'", resp)
		}
	case <-time.After(200 * time.Millisecond):
		t.Error("Timeout waiting for response")
	}
}

// TestRPCChannelSend tests the Send method for delivering responses
func TestRPCChannelSend(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := &RPCChannelConfig{
		MessageBus:      msgBus,
		RequestTimeout:  5 * time.Second,
		CleanupInterval: 1 * time.Second,
	}

	channel, err := NewRPCChannel(cfg)
	if err != nil {
		t.Fatalf("Failed to create RPC channel: %v", err)
	}

	ctx := context.Background()
	if err := channel.Start(ctx); err != nil {
		t.Fatalf("Failed to start channel: %v", err)
	}
	defer channel.Stop(ctx)

	// Submit a request first
	inbound := &bus.InboundMessage{
		Content:       "Test question",
		ChatID:        "test-chat",
		CorrelationID: "test-correlation-123",
	}

	respCh, err := channel.Input(ctx, inbound)
	if err != nil {
		t.Fatalf("Input() failed: %v", err)
	}

	// Send a response via Send method
	outbound := bus.OutboundMessage{
		Channel: "rpc",
		Content: "[rpc:test-correlation-123] Test answer",
	}

	if err := channel.Send(ctx, outbound); err != nil {
		t.Errorf("Send() failed: %v", err)
	}

	// Verify response was delivered
	select {
	case resp := <-respCh:
		if resp != "Test answer" {
			t.Errorf("Expected response 'Test answer', got '%s'", resp)
		}
	case <-time.After(100 * time.Millisecond):
		t.Error("Timeout waiting for response")
	}
}

// TestRPCChannelSendWrongChannel tests that Send ignores messages for other channels
func TestRPCChannelSendWrongChannel(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := &RPCChannelConfig{
		MessageBus: msgBus,
	}

	channel, err := NewRPCChannel(cfg)
	if err != nil {
		t.Fatalf("Failed to create RPC channel: %v", err)
	}

	ctx := context.Background()
	if err := channel.Start(ctx); err != nil {
		t.Fatalf("Failed to start channel: %v", err)
	}
	defer channel.Stop(ctx)

	// Submit a request
	inbound := &bus.InboundMessage{
		Content:       "Test",
		ChatID:        "test-chat",
		CorrelationID: "test-123",
	}

	respCh, err := channel.Input(ctx, inbound)
	if err != nil {
		t.Fatalf("Input() failed: %v", err)
	}

	// Send message for different channel (should be ignored)
	outbound := bus.OutboundMessage{
		Channel: "telegram", // Wrong channel
		Content: "[rpc:test-123] Response",
	}

	if err := channel.Send(ctx, outbound); err != nil {
		t.Errorf("Send() should not error for wrong channel, got: %v", err)
	}

	// Verify no response was delivered
	select {
	case <-respCh:
		t.Error("Should not receive response for wrong channel")
	case <-time.After(50 * time.Millisecond):
		// Expected - no response
	}
}

// TestRPCChannelSendNoCorrelationID tests Send with message missing correlation ID
func TestRPCChannelSendNoCorrelationID(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := &RPCChannelConfig{
		MessageBus: msgBus,
	}

	channel, err := NewRPCChannel(cfg)
	if err != nil {
		t.Fatalf("Failed to create RPC channel: %v", err)
	}

	ctx := context.Background()
	if err := channel.Start(ctx); err != nil {
		t.Fatalf("Failed to start channel: %v", err)
	}
	defer channel.Stop(ctx)

	// Send message without correlation ID (should be ignored)
	outbound := bus.OutboundMessage{
		Channel: "rpc",
		Content: "Response without correlation ID",
	}

	if err := channel.Send(ctx, outbound); err != nil {
		t.Errorf("Send() should not error for message without correlation ID, got: %v", err)
	}
}

// TestRPCChannelSendUnknownRequest tests Send with unknown correlation ID
func TestRPCChannelSendUnknownRequest(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := &RPCChannelConfig{
		MessageBus: msgBus,
	}

	channel, err := NewRPCChannel(cfg)
	if err != nil {
		t.Fatalf("Failed to create RPC channel: %v", err)
	}

	ctx := context.Background()
	if err := channel.Start(ctx); err != nil {
		t.Fatalf("Failed to start channel: %v", err)
	}
	defer channel.Stop(ctx)

	// Send response for unknown request (should not error)
	outbound := bus.OutboundMessage{
		Channel: "rpc",
		Content: "[rpc:unknown-id] Response",
	}

	if err := channel.Send(ctx, outbound); err != nil {
		t.Errorf("Send() should not error for unknown request, got: %v", err)
	}
}

// TestExtractCorrelationID tests the correlation ID extraction function
func TestExtractCorrelationID(t *testing.T) {
	tests := []struct {
		name     string
		content  string
		expected string
	}{
		{
			name:     "Valid correlation ID",
			content:  "[rpc:test-123] Response message",
			expected: "test-123",
		},
		{
			name:     "No correlation ID prefix",
			content:  "Plain message",
			expected: "",
		},
		{
			name:     "Empty content",
			content:  "",
			expected: "",
		},
		{
			name:     "Missing closing bracket",
			content:  "[rpc:test-123 Response",
			expected: "",
		},
		{
			name:     "Empty correlation ID",
			content:  "[rpc:] Message",
			expected: "",
		},
		{
			name:     "Correlation ID with special chars",
			content:  "[rpc:test-123_abc] Response",
			expected: "test-123_abc",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := extractCorrelationID(tt.content)
			if result != tt.expected {
				t.Errorf("extractCorrelationID() = %v, want %v", result, tt.expected)
			}
		})
	}
}

// TestRemoveCorrelationID tests the correlation ID removal function
func TestRemoveCorrelationID(t *testing.T) {
	tests := []struct {
		name     string
		content  string
		expected string
	}{
		{
			name:     "Remove with space after bracket",
			content:  "[rpc:test-123] Response message",
			expected: "Response message",
		},
		{
			name:     "Remove without space after bracket",
			content:  "[rpc:test-123]Response message",
			expected: "Response message",
		},
		{
			name:     "No correlation ID prefix",
			content:  "Plain message",
			expected: "Plain message",
		},
		{
			name:     "Empty correlation ID",
			content:  "[rpc:] Message",
			expected: "Message",
		},
		{
			name:     "Only correlation ID",
			content:  "[rpc:test-123]",
			expected: "",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := removeCorrelationID(tt.content)
			if result != tt.expected {
				t.Errorf("removeCorrelationID() = %v, want %v", result, tt.expected)
			}
		})
	}
}

// TestRPCChannelCleanupExpiredRequests tests the cleanup of expired requests
func TestRPCChannelCleanupExpiredRequests(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := &RPCChannelConfig{
		MessageBus:      msgBus,
		RequestTimeout:  100 * time.Millisecond,
		CleanupInterval: 50 * time.Millisecond,
	}

	channel, err := NewRPCChannel(cfg)
	if err != nil {
		t.Fatalf("Failed to create RPC channel: %v", err)
	}

	ctx := context.Background()
	if err := channel.Start(ctx); err != nil {
		t.Fatalf("Failed to start channel: %v", err)
	}
	defer channel.Stop(ctx)

	// Submit a request
	inbound := &bus.InboundMessage{
		Content:       "Test",
		ChatID:        "test-chat",
		CorrelationID: "expire-test",
	}

	respCh, err := channel.Input(ctx, inbound)
	if err != nil {
		t.Fatalf("Input() failed: %v", err)
	}

	// Wait for request to expire
	time.Sleep(200 * time.Millisecond)

	// Try to receive - channel should be closed due to expiration
	select {
	case _, ok := <-respCh:
		if ok {
			t.Error("Response channel should be closed after expiration")
		}
		// Channel closed - this is expected
	case <-time.After(100 * time.Millisecond):
		t.Error("Timeout waiting for channel to close")
	}
}

// TestRPCChannelIsAllowed tests that RPC channel allows all senders
func TestRPCChannelIsAllowed(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := &RPCChannelConfig{
		MessageBus: msgBus,
	}

	channel, err := NewRPCChannel(cfg)
	if err != nil {
		t.Fatalf("Failed to create RPC channel: %v", err)
	}

	tests := []struct {
		senderID string
		expected bool
	}{
		{"any-sender", true},
		{"", true},
		{"rpc-123", true},
		{"system", true},
	}

	for _, tt := range tests {
		if result := channel.IsAllowed(tt.senderID); result != tt.expected {
			t.Errorf("IsAllowed(%q) = %v, want %v", tt.senderID, result, tt.expected)
		}
	}
}

// TestRPCChannelAddSyncTarget tests sync target management
func TestRPCChannelAddSyncTarget(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := &RPCChannelConfig{
		MessageBus: msgBus,
	}

	channel, err := NewRPCChannel(cfg)
	if err != nil {
		t.Fatalf("Failed to create RPC channel: %v", err)
	}

	target := NewMockChannel("target", nil)

	// Add sync target
	err = channel.AddSyncTarget("target", target)
	if err != nil {
		t.Errorf("AddSyncTarget() failed: %v", err)
	}

	// Remove sync target
	channel.RemoveSyncTarget("target")

	// Should not panic
}

// TestGenerateCorrelationID tests correlation ID generation
func TestGenerateCorrelationID(t *testing.T) {
	// Add a small delay to ensure different timestamps
	time.Sleep(1 * time.Millisecond)
	id1 := generateCorrelationID()
	time.Sleep(1 * time.Millisecond)
	id2 := generateCorrelationID()

	if id1 == id2 {
		t.Error("Generated correlation IDs should be unique")
	}

	if id1 == "" {
		t.Error("Generated correlation ID should not be empty")
	}

	// Check format
	if len(id1) < 5 {
		t.Error("Generated correlation ID should be at least 5 characters")
	}
}
