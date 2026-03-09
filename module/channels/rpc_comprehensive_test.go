// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package channels

import (
	"context"
	"fmt"
	"strings"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
)

// ============================================================================
// RPC Channel Comprehensive Tests
// ============================================================================

func TestRPCChannel_RequestTimeout(t *testing.T) {
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
		Content:       "Test question",
		ChatID:        "test-chat",
		CorrelationID: "timeout-test",
	}

	respCh, err := channel.Input(ctx, inbound)
	if err != nil {
		t.Fatalf("Input() failed: %v", err)
	}

	// Wait for timeout (request expires)
	time.Sleep(200 * time.Millisecond)

	// Try to receive - channel should be closed
	select {
	case _, ok := <-respCh:
		if ok {
			t.Error("Response channel should be closed after timeout")
		}
	case <-time.After(100 * time.Millisecond):
		t.Error("Timeout waiting for channel to close")
	}
}

func TestRPCChannel_ContextCancellation(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := &RPCChannelConfig{
		MessageBus: msgBus,
	}

	channel, err := NewRPCChannel(cfg)
	if err != nil {
		t.Fatalf("Failed to create RPC channel: %v", err)
	}

	ctx, cancel := context.WithCancel(context.Background())

	if err := channel.Start(ctx); err != nil {
		t.Fatalf("Failed to start channel: %v", err)
	}

	// Submit a request
	inbound := &bus.InboundMessage{
		Content:       "Test",
		ChatID:        "test-chat",
		CorrelationID: "cancel-test",
	}

	respCh, err := channel.Input(ctx, inbound)
	if err != nil {
		t.Fatalf("Input() failed: %v", err)
	}

	// Cancel context immediately
	cancel()

	// Stop channel (should handle cancelled context)
	if err := channel.Stop(ctx); err != nil {
		t.Logf("Stop() with cancelled context returned error: %v", err)
	}

	// Response channel should be closed
	select {
	case _, ok := <-respCh:
		if ok {
			t.Error("Response channel should be closed after context cancellation")
		}
	case <-time.After(100 * time.Millisecond):
		t.Error("Timeout waiting for channel to close")
	}
}

func TestRPCChannel_MultipleConcurrentRequests(t *testing.T) {
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

	// Submit multiple concurrent requests
	const numRequests = 10
	respChs := make([]<-chan string, numRequests)
	correlationIDs := make([]string, numRequests)

	for i := 0; i < numRequests; i++ {
		inbound := &bus.InboundMessage{
			Content:       fmt.Sprintf("Question %d", i),
			ChatID:        "test-chat",
			CorrelationID: fmt.Sprintf("correlation-%d", i),
		}

		respCh, err := channel.Input(ctx, inbound)
		if err != nil {
			t.Fatalf("Input() %d failed: %v", i, err)
		}

		respChs[i] = respCh
		correlationIDs[i] = inbound.CorrelationID
	}

	// Send responses in random order
	responses := []int{3, 7, 1, 9, 0, 5, 2, 8, 4, 6}
	for _, idx := range responses {
		outbound := bus.OutboundMessage{
			Channel: "rpc",
			Content: fmt.Sprintf("[rpc:%s] Answer %d", correlationIDs[idx], idx),
		}

		if err := channel.Send(ctx, outbound); err != nil {
			t.Errorf("Send() failed for request %d: %v", idx, err)
		}
	}

	// Verify all responses were received
	for i, respCh := range respChs {
		select {
		case resp := <-respCh:
			expected := fmt.Sprintf("Answer %d", i)
			if resp != expected {
				t.Errorf("Request %d: expected response '%s', got '%s'", i, expected, resp)
			}
		case <-time.After(200 * time.Millisecond):
			t.Errorf("Request %d: timeout waiting for response", i)
		}
	}
}

func TestRPCChannel_GetRequestTimeout(t *testing.T) {
	msgBus := bus.NewMessageBus()
	customTimeout := 30 * time.Second

	cfg := &RPCChannelConfig{
		MessageBus:      msgBus,
		RequestTimeout:  customTimeout,
		CleanupInterval: 10 * time.Second,
	}

	channel, err := NewRPCChannel(cfg)
	if err != nil {
		t.Fatalf("Failed to create RPC channel: %v", err)
	}

	tests := []struct {
		name     string
		metadata map[string]string
		expected time.Duration
	}{
		{
			name:     "No metadata",
			metadata: nil,
			expected: customTimeout,
		},
		{
			name:     "Empty metadata",
			metadata: map[string]string{},
			expected: customTimeout,
		},
		{
			name:     "No rpc_timeout in metadata",
			metadata: map[string]string{"other_key": "other_value"},
			expected: customTimeout,
		},
		{
			name:     "Valid rpc_timeout in metadata",
			metadata: map[string]string{"rpc_timeout": "45s"},
			expected: 45 * time.Second,
		},
		{
			name:     "Invalid rpc_timeout in metadata",
			metadata: map[string]string{"rpc_timeout": "invalid"},
			expected: customTimeout, // Should fall back to default
		},
		{
			name:     "rpc_timeout with milliseconds",
			metadata: map[string]string{"rpc_timeout": "1500ms"},
			expected: 1500 * time.Millisecond,
		},
		{
			name:     "rpc_timeout with minutes",
			metadata: map[string]string{"rpc_timeout": "2m"},
			expected: 2 * time.Minute,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := channel.getRequestTimeout(tt.metadata)
			if result != tt.expected {
				t.Errorf("getRequestTimeout() = %v, want %v", result, tt.expected)
			}
		})
	}
}

func TestRPCChannel_Send_ResponseDelivery(t *testing.T) {
	tests := []struct {
		name     string
		content  string
		expected string
	}{
		{
			name:     "Standard format with space",
			content:  "[rpc:test-123] Response message",
			expected: "Response message",
		},
		{
			name:     "Standard format without space",
			content:  "[rpc:test-456]Response message",
			expected: "Response message",
		},
		{
			name:     "Response with newlines",
			content:  "[rpc:test-789] Line1\nLine2\nLine3",
			expected: "Line1\nLine2\nLine3",
		},
		{
			name:     "Response with special characters",
			content:  "[rpc:test-abc] Test <>&\"'`\n\t日本語🎉",
			expected: "Test <>&\"'`\n\t日本語🎉",
		},
		{
			name:     "Empty response after correlation ID",
			content:  "[rpc:test-empty]",
			expected: "",
		},
		{
			name:     "Response with only whitespace",
			content:  "[rpc:test-ws]   \n\t  ",
			expected: "  \n\t  ", // After ']' and space, returns remaining
		},
	}

	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
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

			// Extract correlation ID from test content
			correlationID := extractCorrelationID(tc.content)

			// Submit request
			inbound := &bus.InboundMessage{
				Content:       "Test",
				ChatID:        "test-chat",
				CorrelationID: correlationID,
			}

			respCh, err := channel.Input(ctx, inbound)
			if err != nil {
				t.Fatalf("Input() failed: %v", err)
			}

			// Send response
			outbound := bus.OutboundMessage{
				Channel: "rpc",
				Content: tc.content,
			}

			if err := channel.Send(ctx, outbound); err != nil {
				t.Errorf("Send() failed: %v", err)
				return
			}

			select {
			case resp := <-respCh:
				if resp != tc.expected {
					t.Errorf("Expected response '%s', got '%s'", tc.expected, resp)
				}
			case <-time.After(100 * time.Millisecond):
				t.Error("Timeout waiting for response")
			}
		})
	}
}

func TestRPCChannel_CleanupLoop(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := &RPCChannelConfig{
		MessageBus:      msgBus,
		RequestTimeout:  50 * time.Millisecond,
		CleanupInterval: 25 * time.Millisecond,
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

	// Submit multiple requests
	const numRequests = 5
	for i := 0; i < numRequests; i++ {
		inbound := &bus.InboundMessage{
			Content:       fmt.Sprintf("Request %d", i),
			ChatID:        "test-chat",
			CorrelationID: fmt.Sprintf("cleanup-%d", i),
		}

		_, err := channel.Input(ctx, inbound)
		if err != nil {
			t.Fatalf("Input() %d failed: %v", i, err)
		}
	}

	// Wait for requests to expire and be cleaned up
	time.Sleep(150 * time.Millisecond)

	// Verify pending requests map is empty
	channel.mu.Lock()
	pendingCount := len(channel.pendingReqs)
	channel.mu.Unlock()

	if pendingCount != 0 {
		t.Errorf("Expected 0 pending requests after cleanup, got %d", pendingCount)
	}
}

func TestRPCChannel_StopWithPendingRequests(t *testing.T) {
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

	// Submit some requests
	const numRequests = 3
	respChs := make([]<-chan string, numRequests)

	for i := 0; i < numRequests; i++ {
		inbound := &bus.InboundMessage{
			Content:       fmt.Sprintf("Request %d", i),
			ChatID:        "test-chat",
			CorrelationID: fmt.Sprintf("stop-%d", i),
		}

		respCh, err := channel.Input(ctx, inbound)
		if err != nil {
			t.Fatalf("Input() %d failed: %v", i, err)
		}
		respChs[i] = respCh
	}

	// Stop channel (should close all response channels)
	if err := channel.Stop(ctx); err != nil {
		t.Fatalf("Stop() failed: %v", err)
	}

	// Verify all response channels are closed
	for i, respCh := range respChs {
		select {
		case _, ok := <-respCh:
			if ok {
				t.Errorf("Response channel %d should be closed after Stop()", i)
			}
		case <-time.After(100 * time.Millisecond):
			t.Errorf("Timeout waiting for response channel %d to close", i)
		}
	}
}

// ============================================================================
// Correlation ID Extraction/Removal Tests
// ============================================================================

func TestExtractCorrelationID_EdgeCases(t *testing.T) {
	tests := []struct {
		name     string
		content  string
		expected string
	}{
		{
			name:     "Multiple brackets",
			content:  "[rpc:test1] [rpc:test2] Content",
			expected: "test1",
		},
		{
			name:     "Nested brackets",
			content:  "[rpc:[nested]] Content",
			expected: "[nested", // Extracts from index 5 to first ]
		},
		{
			name:     "Only opening bracket",
			content:  "[rpc:test Content",
			expected: "",
		},
		{
			name:     "Empty correlation ID with space",
			content:  "[rpc: ] Content",
			expected: " ", // Single space is extracted (end=6, which is >5)
		},
		{
			name:     "Very long correlation ID",
			content:  "[rpc:" + strings.Repeat("a", 1000) + "] Content",
			expected: strings.Repeat("a", 1000),
		},
		{
			name:     "Special characters in correlation ID",
			content:  "[rpc:test-123_abc.xyz] Content",
			expected: "test-123_abc.xyz",
		},
		{
			name:     "Correlation ID with path separators",
			content:  "[rpc:/path/to/id] Content",
			expected: "/path/to/id",
		},
		{
			name:     "Correlation ID with URL",
			content:  "[rpc:https://example.com/id] Content",
			expected: "https://example.com/id",
		},
		{
			name:     "Unicode characters in correlation ID",
			content:  "[rpc:テスト-id-123] Content",
			expected: "テスト-id-123",
		},
		{
			name:     "Emoji in correlation ID",
			content:  "[rpc:test-🎉-123] Content",
			expected: "test-🎉-123",
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

func TestRemoveCorrelationID_EdgeCases(t *testing.T) {
	tests := []struct {
		name     string
		content  string
		expected string
	}{
		{
			name:     "Multiple correlation ID prefixes",
			content:  "[rpc:id1] [rpc:id2] Content",
			expected: "[rpc:id2] Content", // Only removes first prefix
		},
		{
			name:     "Correlation ID with immediate content",
			content:  "[rpc:test-id]Content",
			expected: "Content",
		},
		{
			name:     "Only correlation ID",
			content:  "[rpc:test-id]",
			expected: "",
		},
		{
			name:     "Empty correlation ID",
			content:  "[rpc:] Content here",
			expected: "Content here",
		},
		{
			name:     "Very long content after correlation ID",
			content:  "[rpc:id] " + strings.Repeat("word ", 1000),
			expected: strings.Repeat("word ", 1000),
		},
		{
			name:     "Content with newlines",
			content:  "[rpc:id]\nLine1\nLine2\n",
			expected: "\nLine1\nLine2\n",
		},
		{
			name:     "Content with tabs",
			content:  "[rpc:id]\tTabbed\tcontent\t",
			expected: "\tTabbed\tcontent\t",
		},
		{
			name:     "Content with mixed whitespace",
			content:  "[rpc:id] \t\n Mixed \r\n ",
			expected: "\t\n Mixed \r\n ",
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

func TestGenerateCorrelationID_Uniqueness(t *testing.T) {
	// Generate correlation IDs with small delays to ensure uniqueness
	const numIDs = 10
	ids := make(map[string]bool)

	for i := 0; i < numIDs; i++ {
		time.Sleep(1 * time.Microsecond) // Small delay to get different timestamps
		id := generateCorrelationID()

		// Check format
		if !strings.HasPrefix(id, "rpc-") {
			t.Errorf("Generated ID doesn't start with 'rpc-': %s", id)
		}

		// Check uniqueness (with very small delay, duplicates are unlikely but possible)
		if ids[id] {
			t.Logf("Warning: Duplicate correlation ID generated: %s", id)
		}

		ids[id] = true
	}

	// We expect at least 80% unique IDs (due to timing, some duplicates are possible)
	uniqueCount := len(ids)
	minUnique := numIDs * 8 / 10
	if uniqueCount < minUnique {
		t.Errorf("Expected at least %d unique IDs, got %d", minUnique, uniqueCount)
	}
}

// ============================================================================
// RPC Channel SyncTarget Tests
// ============================================================================

func TestRPCChannel_SyncTargets(t *testing.T) {
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

func TestRPCChannel_IsAllowed_All(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := &RPCChannelConfig{
		MessageBus: msgBus,
	}

	channel, err := NewRPCChannel(cfg)
	if err != nil {
		t.Fatalf("Failed to create RPC channel: %v", err)
	}

	// RPC channel should allow all senders
	testSenders := []string{
		"",
		"any-sender",
		"123456",
		"rpc-handler",
		"system",
		"unknown",
	}

	for _, senderID := range testSenders {
		if !channel.IsAllowed(senderID) {
			t.Errorf("RPC channel should allow sender '%s'", senderID)
		}
	}
}
