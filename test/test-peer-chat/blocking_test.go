package main

import (
	"context"
	"fmt"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/channels"
)

// TestPeerChatBlocking reproduces the blocking issue
func TestPeerChatBlocking(t *testing.T) {
	fmt.Println("\n========================================")
	fmt.Println(" Peer Chat Blocking Test")
	fmt.Println("========================================")

	// Test 1: Short timeout with quick response (baseline)
	t.Run("ShortTimeout_QuickResponse", func(t *testing.T) {
		runTest(t, 5*time.Second, 1*time.Second, true)
	})

	// Test 2: Long timeout with delayed response (realistic)
	// This simulates: 28min timeout + 10min LLM processing
	t.Run("LongTimeout_RealisticDelay", func(t *testing.T) {
		runTest(t, 28*time.Minute, 10*time.Minute, true)
	})

	// Test 3: Long timeout with very long delay (extreme case)
	// This simulates: 28min timeout + 20min LLM processing (should timeout)
	t.Run("LongTimeout_ExtremeDelay", func(t *testing.T) {
		runTest(t, 28*time.Minute, 20*time.Minute, false)
	})
}

func runTest(t *testing.T, timeout time.Duration, delay time.Duration, shouldSucceed bool) {
	fmt.Printf("\nTest Config:\n")
	fmt.Printf("  Timeout: %v\n", timeout)
	fmt.Printf("  Response Delay: %v\n", delay)
	fmt.Printf("  Expected: ")
	if shouldSucceed {
		fmt.Printf("✅ Success\n")
	} else {
		fmt.Printf("❌ Timeout\n")
	}
	fmt.Println()

	// Create message bus
	msgBus := bus.NewMessageBus()

	// Create RPC channel
	cfg := &channels.RPCChannelConfig{
		MessageBus:      msgBus,
		RequestTimeout:  timeout,
		CleanupInterval: 10 * time.Second,
	}

	rpcCh, err := channels.NewRPCChannel(cfg)
	if err != nil {
		t.Fatalf("Failed to create RPC channel: %v", err)
	}

	ctx := context.Background()
	if err := rpcCh.Start(ctx); err != nil {
		t.Fatalf("Failed to start RPC channel: %v", err)
	}
	defer rpcCh.Stop(ctx)

	// Start dispatch loop (runs until test completes)
	dispatchCtx, dispatchCancel := context.WithCancel(context.Background())
	defer dispatchCancel()
	go func() {
		for {
			select {
			case msg := <-msgBus.OutboundChannel():
				if msg.Channel == "rpc" {
					rpcCh.Send(ctx, msg)
				}
			case <-dispatchCtx.Done():
				return
			}
		}
	}()

	// Create request
	inbound := &bus.InboundMessage{
		Channel:       "rpc",
		ChatID:        "test-chat",
		Content:       "test message",
		CorrelationID: fmt.Sprintf("test-%d", time.Now().UnixNano()),
	}

	fmt.Printf("📤 Sending request...\n")
	respCh, err := rpcCh.Input(ctx, inbound)
	if err != nil {
		t.Fatalf("Failed to send request: %v", err)
	}

	// Simulate delayed response
	go func() {
		time.Sleep(delay)
		fmt.Printf("\n⏰ Sending response after %v...\n", delay)
		msgBus.PublishOutbound(bus.OutboundMessage{
			Channel: "rpc",
			ChatID:  "test-chat",
			Content: fmt.Sprintf("[rpc:%s] Response after %v", inbound.CorrelationID, delay),
		})
	}()

	// Wait for response
	start := time.Now()
	fmt.Printf("⏳ Waiting for response...\n")

	select {
	case <-respCh:
		elapsed := time.Since(start)
		fmt.Printf("✅ Received response in %v\n", elapsed)
		if !shouldSucceed {
			t.Errorf("Expected timeout but got response")
		}
	case <-time.After(timeout + 5*time.Second):
		elapsed := time.Since(start)
		fmt.Printf("❌ Timeout after %v\n", elapsed)
		if shouldSucceed {
			t.Errorf("Expected success but got timeout")
		}
	}
}
