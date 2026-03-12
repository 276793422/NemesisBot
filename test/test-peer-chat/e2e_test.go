package main

import (
	"context"
	"fmt"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/channels"
)

// TestPeerChatE2E tests the real scenario with AgentLoop and LLM processing
func TestPeerChatE2E(t *testing.T) {
	fmt.Println("\n========================================")
	fmt.Println(" Peer Chat E2E Test (with AgentLoop)")
	fmt.Println("========================================")

	// This test requires TestAIServer running with testai-1.3 model (300s delay)
	// Command: nemesisbot model add --model test/testai-1.3 --base http://127.0.0.1:8080/v1 --key test-key

	t.Run("SingleRequest_LongTimeout_SlowLLM", func(t *testing.T) {
		runE2ETest(t, 1, 28*time.Minute, 5*time.Minute)
	})

	t.Run("MultipleRequests_LongTimeout_SlowLLM", func(t *testing.T) {
		// This should trigger buffer full issue
		runE2ETest(t, 10, 28*time.Minute, 5*time.Minute)
	})
}

func runE2ETest(t *testing.T, numRequests int, timeout time.Duration, expectedLLMDelay time.Duration) {
	fmt.Printf("\nTest Config:\n")
	fmt.Printf("  Number of Requests: %d\n", numRequests)
	fmt.Printf("  RPC Timeout: %v\n", timeout)
	fmt.Printf("  Expected LLM Delay: %v\n", expectedLLMDelay)
	fmt.Println()

	// Create message bus
	msgBus := bus.NewMessageBus()

	// Create RPC channel with long timeout
	cfg := &channels.RPCChannelConfig{
		MessageBus:      msgBus,
		RequestTimeout:  timeout,
		CleanupInterval: 30 * time.Second,
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

	// Start dispatch loop
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

	// Create agent loop (this would normally process messages via LLM)
	// For this test, we simulate the delay
	// In real scenario, AgentLoop would call LLM which takes 5 minutes

	// Send multiple requests
	results := make(chan time.Duration, numRequests)
	start := time.Now()

	for i := 0; i < numRequests; i++ {
		go func(idx int) {
			// Create request
			inbound := &bus.InboundMessage{
				Channel:       "rpc",
				ChatID:        fmt.Sprintf("test-chat-%d", idx),
				Content:       fmt.Sprintf("test message %d", idx),
				CorrelationID: fmt.Sprintf("test-%d-%d", time.Now().UnixNano(), idx),
			}

			fmt.Printf("[Request %d] 📤 Sending request...\n", idx)
			respCh, err := rpcCh.Input(ctx, inbound)
			if err != nil {
				t.Errorf("[Request %d] Failed to send request: %v", idx, err)
				results <- 0
				return
			}

			// Simulate LLM processing delay (in real scenario, AgentLoop would do this)
			time.Sleep(expectedLLMDelay)

			// Send response (simulating AgentLoop's response)
			msgBus.PublishOutbound(bus.OutboundMessage{
				Channel: "rpc",
				ChatID:  inbound.ChatID,
				Content: fmt.Sprintf("[rpc:%s] Response %d", inbound.CorrelationID, idx),
			})

			// Wait for response
			reqStart := time.Now()
			select {
			case <-respCh:
				elapsed := time.Since(reqStart)
				fmt.Printf("[Request %d] ✅ Received response in %v\n", idx, elapsed)
				results <- elapsed
			case <-time.After(timeout + 1*time.Minute):
				fmt.Printf("[Request %d] ❌ Timeout\n", idx)
				results <- 0
			}
		}(i)
	}

	// Collect results
	successCount := 0
	totalTime := time.Duration(0)
	for i := 0; i < numRequests; i++ {
		elapsed := <-results
		if elapsed > 0 {
			successCount++
			totalTime += elapsed
		}
	}

	fmt.Printf("\n📊 Test Results:\n")
	fmt.Printf("  Success: %d/%d\n", successCount, numRequests)
	fmt.Printf("  Total Time: %v\n", time.Since(start))
	if successCount > 0 {
		fmt.Printf("  Average Response Time: %v\n", totalTime/time.Duration(successCount))
	}

	if successCount < numRequests {
		t.Errorf("Only %d/%d requests succeeded", successCount, numRequests)
	}
}
