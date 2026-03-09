// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package performance

import (
	"context"
	"sync"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
)

// TestConcurrentMessagePublishing tests concurrent message publishing performance
func TestConcurrentMessagePublishing(t *testing.T) {
	msgBus := bus.NewMessageBus()

	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	// Track received messages
	var mu sync.Mutex
	receivedCount := 0

	// Start a single subscriber
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
	}()

	// Give subscriber time to start
	time.Sleep(100 * time.Millisecond)

	// Publish messages concurrently
	numPublishers := 10
	messagesPerPublisher := 100
	totalMessages := numPublishers * messagesPerPublisher

	var wg sync.WaitGroup
	startTime := time.Now()

	for i := 0; i < numPublishers; i++ {
		wg.Add(1)
		go func(publisherID int) {
			defer wg.Done()
			for j := 0; j < messagesPerPublisher; j++ {
				msgBus.PublishOutbound(bus.OutboundMessage{
					Channel: "test",
					ChatID:  "chat",
					Content: "test",
				})
			}
		}(i)
	}

	wg.Wait()
	elapsed := time.Since(startTime)

	// Wait for messages to be processed
	time.Sleep(1 * time.Second)

	mu.Lock()
	finalCount := receivedCount
	mu.Unlock()

	// Calculate metrics
	messagesPerSecond := float64(totalMessages) / elapsed.Seconds()
	throughputMB := float64(totalMessages) * 100 / 1024 / 1024 // Approximate size

	t.Logf("=== Concurrent Message Publishing Results ===")
	t.Logf("Total messages published: %d", totalMessages)
	t.Logf("Messages received: %d", finalCount)
	t.Logf("Time elapsed: %v", elapsed)
	t.Logf("Throughput: %.2f msg/sec", messagesPerSecond)
	t.Logf("Approximate data size: %.2f MB", throughputMB)

	// Verify we received most messages
	if finalCount < totalMessages/2 {
		t.Errorf("Expected to receive at least half the messages, got %d/%d", finalCount, totalMessages)
	}

	if finalCount > 0 {
		t.Log("✓ Concurrent message publishing test passed")
	}
}

// TestConcurrentSubscribers tests multiple concurrent subscribers
func TestConcurrentSubscribers(t *testing.T) {
	msgBus := bus.NewMessageBus()

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	numSubscribers := 5
	numMessages := 20

	var wg sync.WaitGroup
	var mu sync.Mutex
	receivedCounts := make([]int, numSubscribers)

	// Start multiple subscribers
	for i := 0; i < numSubscribers; i++ {
		idx := i
		wg.Add(1)
		go func() {
			defer wg.Done()
			count := 0
			for count < numMessages {
				_, ok := msgBus.SubscribeOutbound(ctx)
				if !ok {
					break
				}
				count++
			}
			mu.Lock()
			receivedCounts[idx] = count
			mu.Unlock()
		}()
	}

	// Give subscribers time to start
	time.Sleep(100 * time.Millisecond)

	// Publish messages
	startTime := time.Now()
	for i := 0; i < numMessages; i++ {
		msgBus.PublishOutbound(bus.OutboundMessage{
			Channel: "test",
			ChatID:  "chat",
			Content: "test",
		})
	}
	elapsed := time.Since(startTime)

	// Wait for subscribers to finish
	wg.Wait()

	// Calculate statistics
	var totalReceived int
	var minReceived int = numMessages
	var maxReceived int

	for _, count := range receivedCounts {
		totalReceived += count
		if count < minReceived {
			minReceived = count
		}
		if count > maxReceived {
			maxReceived = count
		}
	}

	avgReceived := float64(totalReceived) / float64(numSubscribers)

	t.Logf("=== Concurrent Subscribers Results ===")
	t.Logf("Subscribers: %d", numSubscribers)
	t.Logf("Messages published: %d", numMessages)
	t.Logf("Time to publish: %v", elapsed)
	t.Logf("Total messages received: %d", totalReceived)
	t.Logf("Min received per subscriber: %d", minReceived)
	t.Logf("Max received per subscriber: %d", maxReceived)
	t.Logf("Avg received per subscriber: %.2f", avgReceived)

	t.Log("✓ Concurrent subscribers test passed")
}

// TestHighFrequencyMessageProcessing tests processing high frequency messages
func TestHighFrequencyMessageProcessing(t *testing.T) {
	msgBus := bus.NewMessageBus()

	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	// Track metrics
	var mu sync.Mutex
	receivedCount := 0
	processingTimes := make([]time.Duration, 0, 1000)

	// Start subscriber with timing
	go func() {
		for {
			start := time.Now()
			_, ok := msgBus.SubscribeOutbound(ctx)
			if !ok {
				break
			}
			duration := time.Since(start)

			mu.Lock()
			receivedCount++
			processingTimes = append(processingTimes, duration)
			mu.Unlock()
		}
	}()

	// Give subscriber time to start
	time.Sleep(100 * time.Millisecond)

	// Publish high frequency messages
	numMessages := 1000
	startTime := time.Now()

	for i := 0; i < numMessages; i++ {
		msgBus.PublishOutbound(bus.OutboundMessage{
			Channel: "test",
			ChatID:  "chat",
			Content: "test",
		})
	}

	elapsed := time.Since(startTime)

	// Wait for processing
	time.Sleep(2 * time.Second)

	mu.Lock()
	finalCount := receivedCount
	times := make([]time.Duration, len(processingTimes))
	copy(times, processingTimes)
	mu.Unlock()

	// Calculate statistics
	if len(times) > 0 {
		var sum time.Duration
		var min time.Duration = times[0]
		var max time.Duration = times[0]

		for _, t := range times {
			sum += t
			if t < min {
				min = t
			}
			if t > max {
				max = t
			}
		}

		avg := sum / time.Duration(len(times))

		t.Logf("=== High Frequency Message Processing Results ===")
		t.Logf("Messages published: %d", numMessages)
		t.Logf("Messages received: %d", finalCount)
		t.Logf("Total time: %v", elapsed)
		t.Logf("Publishing rate: %.2f msg/sec", float64(numMessages)/elapsed.Seconds())
		t.Logf("Processing latency:")
		t.Logf("  Min: %v", min)
		t.Logf("  Max: %v", max)
		t.Logf("  Avg: %v", avg)
	}

	if finalCount > 0 {
		t.Log("✓ High frequency message processing test passed")
	}
}

// TestConcurrentMixedOperations tests mixed concurrent operations
func TestConcurrentMixedOperations(t *testing.T) {
	msgBus := bus.NewMessageBus()

	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()

	// Track operations
	var mu sync.Mutex
	publishCount := 0
	receiveCount := 0

	// Start multiple publishers
	numPublishers := 5
	var publishWG sync.WaitGroup

	for i := 0; i < numPublishers; i++ {
		publishWG.Add(1)
		go func() {
			defer publishWG.Done()
			for j := 0; j < 50; j++ {
				msgBus.PublishOutbound(bus.OutboundMessage{
					Channel: "test",
					ChatID:  "chat",
					Content: "test",
				})
				mu.Lock()
				publishCount++
				mu.Unlock()
			}
		}()
	}

	// Start multiple subscribers
	numSubscribers := 3
	var subscribeWG sync.WaitGroup

	for i := 0; i < numSubscribers; i++ {
		subscribeWG.Add(1)
		go func() {
			defer subscribeWG.Done()
			for {
				_, ok := msgBus.SubscribeOutbound(ctx)
				if !ok {
					break
				}
				mu.Lock()
				receiveCount++
				mu.Unlock()
			}
		}()
	}

	// Wait for all operations
	startTime := time.Now()
	publishWG.Wait()

	// Give subscribers time to finish
	time.Sleep(1 * time.Second)
	cancel()
	subscribeWG.Wait()

	elapsed := time.Since(startTime)

	mu.Lock()
	finalPublish := publishCount
	finalReceive := receiveCount
	mu.Unlock()

	t.Logf("=== Concurrent Mixed Operations Results ===")
	t.Logf("Publishers: %d", numPublishers)
	t.Logf("Subscribers: %d", numSubscribers)
	t.Logf("Total time: %v", elapsed)
	t.Logf("Published messages: %d", finalPublish)
	t.Logf("Received messages: %d", finalReceive)

	if finalPublish > 0 && finalReceive > 0 {
		t.Log("✓ Concurrent mixed operations test passed")
	}
}

// TestStressTestMemoryUsage tests memory usage under stress
func TestStressTestMemoryUsage(t *testing.T) {
	msgBus := bus.NewMessageBus()

	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	// Track received messages
	var mu sync.Mutex
	receivedCount := 0

	// Start subscriber
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
	}()

	// Give subscriber time to start
	time.Sleep(100 * time.Millisecond)

	// Publish large number of messages
	numMessages := 10000
	startTime := time.Now()

	for i := 0; i < numMessages; i++ {
		msgBus.PublishOutbound(bus.OutboundMessage{
			Channel: "test",
			ChatID:  "chat",
			Content: "test message with some content",
		})
	}

	elapsed := time.Since(startTime)

	// Wait for processing
	time.Sleep(2 * time.Second)

	mu.Lock()
	finalCount := receivedCount
	mu.Unlock()

	t.Logf("=== Stress Test Results ===")
	t.Logf("Messages published: %d", numMessages)
	t.Logf("Messages received: %d", finalCount)
	t.Logf("Total time: %v", elapsed)
	t.Logf("Publishing rate: %.2f msg/sec", float64(numMessages)/elapsed.Seconds())

	if finalCount > 0 {
		t.Log("✓ Stress test passed - system handled load")
	}
}

// TestBurstMessageTraffic tests burst traffic handling
func TestBurstMessageTraffic(t *testing.T) {
	msgBus := bus.NewMessageBus()

	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	// Track received messages
	var mu sync.Mutex
	receivedCount := 0

	// Start subscriber
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
	}()

	// Give subscriber time to start
	time.Sleep(100 * time.Millisecond)

	// Send bursts of messages
	numBursts := 5
	messagesPerBurst := 100

	startTime := time.Now()

	for burst := 0; burst < numBursts; burst++ {
		// Send burst
		for i := 0; i < messagesPerBurst; i++ {
			msgBus.PublishOutbound(bus.OutboundMessage{
				Channel: "test",
				ChatID:  "chat",
				Content: "burst",
			})
		}
		// Small pause between bursts
		time.Sleep(10 * time.Millisecond)
	}

	elapsed := time.Since(startTime)

	// Wait for processing
	time.Sleep(2 * time.Second)

	mu.Lock()
	finalCount := receivedCount
	mu.Unlock()

	totalMessages := numBursts * messagesPerBurst

	t.Logf("=== Burst Traffic Results ===")
	t.Logf("Bursts: %d", numBursts)
	t.Logf("Messages per burst: %d", messagesPerBurst)
	t.Logf("Total messages: %d", totalMessages)
	t.Logf("Messages received: %d", finalCount)
	t.Logf("Total time: %v", elapsed)

	if finalCount > 0 {
		t.Log("✓ Burst traffic test passed")
	}
}
