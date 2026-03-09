package bus

import (
	"context"
	"testing"
	"time"
)

func TestNewMessageBus(t *testing.T) {
	bus := NewMessageBus()
	if bus == nil {
		t.Fatal("NewMessageBus returned nil")
	}
	if bus.inbound == nil {
		t.Error("Inbound channel should be initialized")
	}
	if bus.outbound == nil {
		t.Error("Outbound channel should be initialized")
	}
}

func TestPublishInbound(t *testing.T) {
	bus := NewMessageBus()

	msg := InboundMessage{
		Channel:    "test",
		SenderID:   "user123",
		ChatID:     "chat456",
		Content:    "Hello",
		SessionKey: "session:key",
	}

	bus.PublishInbound(msg)

	ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
	defer cancel()

	received, ok := bus.ConsumeInbound(ctx)
	if !ok {
		t.Fatal("Failed to consume message")
	}
	if received.Channel != "test" {
		t.Errorf("Expected channel 'test', got %v", received.Channel)
	}
	if received.Content != "Hello" {
		t.Errorf("Expected content 'Hello', got %v", received.Content)
	}
}

func TestPublishOutbound(t *testing.T) {
	bus := NewMessageBus()

	msg := OutboundMessage{
		Channel: "test",
		ChatID:  "chat456",
		Content: "Response",
	}

	bus.PublishOutbound(msg)

	ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
	defer cancel()

	received, ok := bus.SubscribeOutbound(ctx)
	if !ok {
		t.Fatal("Failed to consume message")
	}
	if received.Channel != "test" {
		t.Errorf("Expected channel 'test', got %v", received.Channel)
	}
	if received.Content != "Response" {
		t.Errorf("Expected content 'Response', got %v", received.Content)
	}
}

func TestConsumeInboundContext(t *testing.T) {
	bus := NewMessageBus()

	// Test context cancellation
	ctx, cancel := context.WithCancel(context.Background())
	cancel()

	_, ok := bus.ConsumeInbound(ctx)
	if ok {
		t.Error("Should return false when context is cancelled")
	}
}

func TestSubscribeOutboundContext(t *testing.T) {
	bus := NewMessageBus()

	// Test context cancellation
	ctx, cancel := context.WithCancel(context.Background())
	cancel()

	_, ok := bus.SubscribeOutbound(ctx)
	if ok {
		t.Error("Should return false when context is cancelled")
	}
}

func TestRegisterHandler(t *testing.T) {
	bus := NewMessageBus()

	handler := func(msg InboundMessage) error {
		return nil
	}

	bus.RegisterHandler("test-channel", handler)

	retrievedHandler, ok := bus.GetHandler("test-channel")
	if !ok {
		t.Error("Handler should be registered")
	}
	if retrievedHandler == nil {
		t.Error("Retrieved handler should not be nil")
	}
}

func TestGetHandlerNotFound(t *testing.T) {
	bus := NewMessageBus()

	_, ok := bus.GetHandler("non-existent")
	if ok {
		t.Error("Should return false for non-existent handler")
	}
}

func TestClose(t *testing.T) {
	bus := NewMessageBus()

	// Publish before close
	bus.PublishInbound(InboundMessage{Channel: "test"})
	bus.Close()

	// Publish after close (should be ignored)
	bus.PublishInbound(InboundMessage{Channel: "test"})

	ctx, cancel := context.WithTimeout(context.Background(), 50*time.Millisecond)
	defer cancel()

	_, ok := bus.ConsumeInbound(ctx)
	_ = ok // Result depends on timing
}

func TestCloseMultiple(t *testing.T) {
	bus := NewMessageBus()

	bus.Close()
	bus.Close() // Should not panic

	if !bus.closed {
		t.Error("Bus should be marked as closed")
	}
}

func TestConcurrentPublishConsume(t *testing.T) {
	bus := NewMessageBus()
	ctx := context.Background()

	done := make(chan bool)

	// Publishers
	for i := 0; i < 5; i++ {
		go func(idx int) {
			for j := 0; j < 20; j++ {
				msg := InboundMessage{
					Channel:    "test",
					SenderID:   "user",
					ChatID:     "chat",
					Content:    "Message",
					SessionKey: "key",
				}
				bus.PublishInbound(msg)
			}
			done <- true
		}(i)
	}

	// Consumers
	receivedCount := 0
	for i := 0; i < 5; i++ {
		go func() {
			for {
				msg, ok := bus.ConsumeInbound(ctx)
				if !ok {
					return
				}
				if msg.Channel == "test" {
					receivedCount++
				}
			}
		}()
	}

	// Wait for publishers
	for i := 0; i < 5; i++ {
		<-done
	}

	// Give consumers time to process
	time.Sleep(100 * time.Millisecond)

	if receivedCount != 100 {
		t.Errorf("Expected 100 messages, got %d", receivedCount)
	}

	bus.Close()
}

func TestOutboundChannel(t *testing.T) {
	bus := NewMessageBus()

	ch := bus.OutboundChannel()
	if ch == nil {
		t.Error("OutboundChannel should not return nil")
	}

	msg := OutboundMessage{
		Channel: "test",
		Content: "Test",
	}

	go func() {
		bus.PublishOutbound(msg)
	}()

	select {
	case received := <-ch:
		if received.Content != "Test" {
			t.Errorf("Expected 'Test', got %v", received.Content)
		}
	case <-time.After(100 * time.Millisecond):
		t.Error("Did not receive message from OutboundChannel")
	}
}
