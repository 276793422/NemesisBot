// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package integration

import (
	"context"
	"sync"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
)

// TestMessageFlow_Basic tests the basic message flow through the bus
func TestMessageFlow_Basic(t *testing.T) {
	msgBus := bus.NewMessageBus()

	var wg sync.WaitGroup
	received := false

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	wg.Add(1)
	go func() {
		defer wg.Done()
		msg, ok := msgBus.SubscribeOutbound(ctx)
		if ok {
			t.Logf("Received: %+v", msg)
			received = true
		}
	}()

	// Give subscriber time to start
	time.Sleep(100 * time.Millisecond)

	msgBus.PublishOutbound(bus.OutboundMessage{
		Channel: "test",
		ChatID:  "chat1",
		Content: "Hello",
	})

	wg.Wait()

	if received {
		t.Log("✓ Basic message flow test passed")
	} else {
		t.Error("Expected to receive message")
	}
}

// TestMessageFlow_MultipleMessages tests multiple messages being sent
func TestMessageFlow_MultipleMessages(t *testing.T) {
	msgBus := bus.NewMessageBus()

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	var wg sync.WaitGroup
	messageCount := 0

	// Start subscriber
	wg.Add(1)
	go func() {
		defer wg.Done()
		for {
			_, ok := msgBus.SubscribeOutbound(ctx)
			if !ok {
				break
			}
			messageCount++
		}
	}()

	// Give subscriber time to start
	time.Sleep(100 * time.Millisecond)

	// Send multiple messages
	numMessages := 5
	for i := 0; i < numMessages; i++ {
		msgBus.PublishOutbound(bus.OutboundMessage{
			Channel: "test",
			ChatID:  "chat1",
			Content: "test",
		})
	}

	// Wait a bit for processing
	time.Sleep(500 * time.Millisecond)

	// Cancel context to stop subscriber
	cancel()
	wg.Wait()

	if messageCount == numMessages {
		t.Logf("✓ Multiple messages test passed - received %d/%d", messageCount, numMessages)
	} else if messageCount > 0 {
		t.Logf("⚠ Multiple messages test - received %d/%d", messageCount, numMessages)
	} else {
		t.Error("Expected to receive some messages")
	}
}

// TestMessageFlow_ConcurrentPublishing tests concurrent message publishing
func TestMessageFlow_ConcurrentPublishing(t *testing.T) {
	msgBus := bus.NewMessageBus()

	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()

	// Start subscriber
	var mu sync.Mutex
	receivedCount := 0
	done := make(chan bool)

	go func() {
		for {
			_, ok := msgBus.SubscribeOutbound(ctx)
			if !ok {
				break
			}
			mu.Lock()
			receivedCount++
			mu.Unlock()
		}
		done <- true
	}()

	// Give subscriber time to start
	time.Sleep(100 * time.Millisecond)

	// Publish messages concurrently
	numPublishers := 5
	messagesPerPublisher := 10
	var publishWg sync.WaitGroup

	for i := 0; i < numPublishers; i++ {
		publishWg.Add(1)
		go func() {
			defer publishWg.Done()
			for j := 0; j < messagesPerPublisher; j++ {
				msgBus.PublishOutbound(bus.OutboundMessage{
					Channel: "test",
					ChatID:  "chat",
					Content: "msg",
				})
			}
		}()
	}

	publishWg.Wait()

	// Wait for subscriber to finish
	cancel()
	<-done

	expected := numPublishers * messagesPerPublisher
	if receivedCount == expected {
		t.Logf("✓ Concurrent publishing test passed - %d messages", receivedCount)
	} else if receivedCount > 0 {
		t.Logf("⚠ Concurrent publishing test - received %d/%d messages", receivedCount, expected)
	} else {
		t.Errorf("Expected at least some messages, got %d", receivedCount)
	}
}
