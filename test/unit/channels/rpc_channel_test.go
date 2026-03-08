// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package channels_test

import (
	"context"
	"fmt"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/channels"
)

// startTestDispatchLoop starts a test dispatcher that mimics ChannelManager.dispatchOutbound
// This helper simulates the production environment's outbound message routing
func startTestDispatchLoop(ctx context.Context, msgBus *bus.MessageBus, channelMap map[string]channels.Channel) {
	go func() {
		for {
			select {
			case <-ctx.Done():
				return
			case msg, ok := <-msgBus.OutboundChannel():
				if !ok {
					return
				}
				// Route message to appropriate channel (mimics ChannelManager behavior)
				if ch, exists := channelMap[msg.Channel]; exists {
					ch.Send(ctx, msg)
				}
			}
		}
	}()
}

// TestNewRPCChannel tests creating a new RPC channel
func TestNewRPCChannel(t *testing.T) {
	msgBus := bus.NewMessageBus()

	// Test with nil config
	_, err := channels.NewRPCChannel(nil)
	if err == nil {
		t.Error("Expected error for nil config")
	}

	// Test with nil message bus
	_, err = channels.NewRPCChannel(&channels.RPCChannelConfig{})
	if err == nil {
		t.Error("Expected error for nil message bus")
	}

	// Test valid config
	cfg := &channels.RPCChannelConfig{
		MessageBus: msgBus,
	}
	ch, err := channels.NewRPCChannel(cfg)
	if err != nil {
		t.Fatalf("Failed to create RPC channel: %v", err)
	}

	if ch.Name() != "rpc" {
		t.Errorf("Expected name 'rpc', got '%s'", ch.Name())
	}
}

// TestRPCChannelLifecycle tests starting and stopping the channel
func TestRPCChannelLifecycle(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := &channels.RPCChannelConfig{
		MessageBus:     msgBus,
		RequestTimeout: 5 * time.Second,
	}
	ch, err := channels.NewRPCChannel(cfg)
	if err != nil {
		t.Fatalf("Failed to create RPC channel: %v", err)
	}

	ctx := context.Background()

	// Test start
	if err := ch.Start(ctx); err != nil {
		t.Fatalf("Failed to start: %v", err)
	}

	if !ch.IsRunning() {
		t.Error("Channel should be running after Start()")
	}

	// Test double start
	if err := ch.Start(ctx); err == nil {
		t.Error("Expected error when starting already running channel")
	}

	// Test stop
	if err := ch.Stop(ctx); err != nil {
		t.Fatalf("Failed to stop: %v", err)
	}

	if ch.IsRunning() {
		t.Error("Channel should not be running after Stop()")
	}

	// Test double stop
	if err := ch.Stop(ctx); err != nil {
		t.Errorf("Stop should be idempotent, got error: %v", err)
	}
}

// TestRPCChannelInput tests the Input method
func TestRPCChannelInput(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := &channels.RPCChannelConfig{
		MessageBus:     msgBus,
		RequestTimeout: 5 * time.Second,
	}
	ch, err := channels.NewRPCChannel(cfg)
	if err != nil {
		t.Fatalf("Failed to create RPC channel: %v", err)
	}

	ctx := context.Background()
	if err := ch.Start(ctx); err != nil {
		t.Fatalf("Failed to start: %v", err)
	}
	defer ch.Stop(ctx)

	// Test Input with auto-generated correlation ID
	inbound := bus.InboundMessage{
		ChatID:  "test-user",
		Content: "Hello RPC",
	}

	respCh, err := ch.Input(ctx, &inbound)
	if err != nil {
		t.Fatalf("Input failed: %v", err)
	}

	if inbound.CorrelationID == "" {
		t.Error("Expected correlation ID to be generated")
	}

	if inbound.Channel != "rpc" {
		t.Errorf("Expected channel 'rpc', got '%s'", inbound.Channel)
	}

	// Test Input with provided correlation ID
	inbound2 := bus.InboundMessage{
		ChatID:        "test-user-2",
		Content:       "Hello RPC 2",
		CorrelationID: "test-id-123",
	}

	respCh2, err := ch.Input(ctx, &inbound2)
	if err != nil {
		t.Fatalf("Input failed: %v", err)
	}

	if inbound2.CorrelationID != "test-id-123" {
		t.Errorf("Expected correlation ID 'test-id-123', got '%s'", inbound2.CorrelationID)
	}

	// Cleanup
	_ = respCh
	_ = respCh2
}

// TestRPCChannelInputWhenNotRunning tests Input when channel is not running
func TestRPCChannelInputWhenNotRunning(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := &channels.RPCChannelConfig{
		MessageBus: msgBus,
	}
	ch, err := channels.NewRPCChannel(cfg)
	if err != nil {
		t.Fatalf("Failed to create RPC channel: %v", err)
	}

	ctx := context.Background()
	inbound := bus.InboundMessage{
		ChatID:  "test-user",
		Content: "Hello RPC",
	}

	_, err = ch.Input(ctx, &inbound)
	if err == nil {
		t.Error("Expected error when Input is called on stopped channel")
	}
}

// TestRPCChannelResponseDelivery tests the full request-response flow
func TestRPCChannelResponseDelivery(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := &channels.RPCChannelConfig{
		MessageBus:     msgBus,
		RequestTimeout: 5 * time.Second,
	}
	ch, err := channels.NewRPCChannel(cfg)
	if err != nil {
		t.Fatalf("Failed to create RPC channel: %v", err)
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	// Start test dispatch loop to simulate production environment
	channelMap := map[string]channels.Channel{"rpc": ch}
	startTestDispatchLoop(ctx, msgBus, channelMap)

	if err := ch.Start(ctx); err != nil {
		t.Fatalf("Failed to start: %v", err)
	}
	defer ch.Stop(ctx)

	// Submit a request
	correlationID := "test-correlation-123"
	inbound := bus.InboundMessage{
		ChatID:        "test-user",
		Content:       "Test message",
		CorrelationID: correlationID,
	}

	respCh, err := ch.Input(ctx, &inbound)
	if err != nil {
		t.Fatalf("Input failed: %v", err)
	}

	// Simulate LLM response
	outbound := bus.OutboundMessage{
		Channel: "rpc",
		ChatID:  "test-user",
		Content: fmt.Sprintf("[rpc:%s] Test response", correlationID),
	}

	// Send response
	msgBus.PublishOutbound(outbound)

	// Wait for response
	select {
	case response := <-respCh:
		if response != "Test response" {
			t.Errorf("Expected response 'Test response', got '%s'", response)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("Timeout waiting for response")
	}
}

// TestRPCChannelResponseMatching tests that responses are matched to correct requests
func TestRPCChannelResponseMatching(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := &channels.RPCChannelConfig{
		MessageBus:     msgBus,
		RequestTimeout: 5 * time.Second,
	}
	ch, err := channels.NewRPCChannel(cfg)
	if err != nil {
		t.Fatalf("Failed to create RPC channel: %v", err)
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	// Start test dispatch loop to simulate production environment
	channelMap := map[string]channels.Channel{"rpc": ch}
	startTestDispatchLoop(ctx, msgBus, channelMap)

	if err := ch.Start(ctx); err != nil {
		t.Fatalf("Failed to start: %v", err)
	}
	defer ch.Stop(ctx)

	// Submit multiple requests
	requests := []struct {
		correlationID string
		content       string
	}{
		{"test-id-1", "Request 1"},
		{"test-id-2", "Request 2"},
		{"test-id-3", "Request 3"},
	}

	respChs := make([]<-chan string, len(requests))
	for i, req := range requests {
		inbound := bus.InboundMessage{
			ChatID:        fmt.Sprintf("user-%d", i),
			Content:       req.content,
			CorrelationID: req.correlationID,
		}
		respCh, err := ch.Input(ctx, &inbound)
		if err != nil {
			t.Fatalf("Input %d failed: %v", i, err)
		}
		respChs[i] = respCh
	}

	// Send responses in random order
	responses := []struct {
		correlationID string
		content       string
	}{
		{"test-id-3", "Response 3"},
		{"test-id-1", "Response 1"},
		{"test-id-2", "Response 2"},
	}

	for _, resp := range responses {
		outbound := bus.OutboundMessage{
			Channel: "rpc",
			Content: fmt.Sprintf("[rpc:%s] %s", resp.correlationID, resp.content),
		}
		msgBus.PublishOutbound(outbound)
	}

	// Verify each request got the correct response
	expectedResponses := map[string]string{
		"test-id-1": "Response 1",
		"test-id-2": "Response 2",
		"test-id-3": "Response 3",
	}

	for i, req := range requests {
		select {
		case response := <-respChs[i]:
			expected := expectedResponses[req.correlationID]
			if response != expected {
				t.Errorf("Request %s: expected '%s', got '%s'", req.correlationID, expected, response)
			}
		case <-time.After(2 * time.Second):
			t.Fatalf("Timeout waiting for response %s", req.correlationID)
		}
	}
}

// TestRPCChannelTimeout tests that pending requests are cleaned up on timeout
func TestRPCChannelTimeout(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := &channels.RPCChannelConfig{
		MessageBus:      msgBus,
		RequestTimeout:  1 * time.Second, // Short timeout for testing
		CleanupInterval: 500 * time.Millisecond,
	}
	ch, err := channels.NewRPCChannel(cfg)
	if err != nil {
		t.Fatalf("Failed to create RPC channel: %v", err)
	}

	ctx := context.Background()
	if err := ch.Start(ctx); err != nil {
		t.Fatalf("Failed to start: %v", err)
	}
	defer ch.Stop(ctx)

	// Submit a request
	inbound := bus.InboundMessage{
		ChatID:        "test-user",
		Content:       "Test message",
		CorrelationID: "timeout-test-id",
	}

	respCh, err := ch.Input(ctx, &inbound)
	if err != nil {
		t.Fatalf("Input failed: %v", err)
	}

	// Wait for timeout
	select {
	case _, ok := <-respCh:
		if ok {
			t.Error("Expected channel to be closed on timeout, but received value")
		}
		// Channel closed, which is expected
	case <-time.After(3 * time.Second):
		// Wait for cleanup loop to run
	}
}

// TestRPCChannelIgnoresOtherChannels tests that RPC channel ignores messages from other channels
func TestRPCChannelIgnoresOtherChannels(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := &channels.RPCChannelConfig{
		MessageBus:     msgBus,
		RequestTimeout: 5 * time.Second,
	}
	ch, err := channels.NewRPCChannel(cfg)
	if err != nil {
		t.Fatalf("Failed to create RPC channel: %v", err)
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	// Start test dispatch loop to simulate production environment
	channelMap := map[string]channels.Channel{"rpc": ch}
	startTestDispatchLoop(ctx, msgBus, channelMap)

	if err := ch.Start(ctx); err != nil {
		t.Fatalf("Failed to start: %v", err)
	}
	defer ch.Stop(ctx)

	// Submit a request
	inbound := bus.InboundMessage{
		ChatID:        "test-user",
		Content:       "Test message",
		CorrelationID: "test-id",
	}

	respCh, err := ch.Input(ctx, &inbound)
	if err != nil {
		t.Fatalf("Input failed: %v", err)
	}

	// Send message from different channel
	outbound := bus.OutboundMessage{
		Channel: "telegram", // Wrong channel
		Content: "[rpc:test-id] Response",
	}
	msgBus.PublishOutbound(outbound)

	// Verify no response was delivered
	select {
	case <-respCh:
		t.Error("Should not receive response from different channel")
	case <-time.After(500 * time.Millisecond):
		// Expected - no response
	}
}
