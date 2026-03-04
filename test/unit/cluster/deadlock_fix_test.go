// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package cluster_test

import (
	"context"
	"sync"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/channels"
	"github.com/276793422/NemesisBot/module/cluster"
	clusterrpc "github.com/276793422/NemesisBot/module/cluster/rpc"
)

// TestSetRPCChannelNoDeadlock verifies that SetRPCChannel doesn't cause deadlock
// This is a regression test for the P0 deadlock issue where:
// - SetRPCChannel holds c.mu.Lock()
// - Calls registerLLMHandlers()
// - Which calls RegisterRPCHandler()
// - Which tries c.mu.RLock()
// - Deadlock! (write lock prevents read lock)
//
// The fix releases the lock before calling registerLLMHandlers()
func TestSetRPCChannelNoDeadlock(t *testing.T) {
	// Create a temporary cluster for testing
	workspace := t.TempDir()
	c, err := cluster.NewCluster(workspace)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}

	// Start the cluster (this starts RPC server)
	if err := c.Start(); err != nil {
		t.Fatalf("Failed to start cluster: %v", err)
	}
	defer c.Stop()

	// Create a mock message bus for RPCChannel
	mockBus := bus.NewMessageBus()

	// Create RPCChannel
	cfg := &channels.RPCChannelConfig{
		MessageBus:      mockBus,
		RequestTimeout:  60 * time.Second,
		CleanupInterval: 30 * time.Second,
	}
	rpcCh, err := channels.NewRPCChannel(cfg)
	if err != nil {
		t.Fatalf("Failed to create RPC channel: %v", err)
	}

	// Start RPCChannel
	ctx := context.Background()
	if err := rpcCh.Start(ctx); err != nil {
		t.Fatalf("Failed to start RPC channel: %v", err)
	}
	defer rpcCh.Stop(ctx)

	// Use a channel to detect completion
	done := make(chan bool, 1)
	deadlockTimeout := 5 * time.Second

	// Call SetRPCChannel in a goroutine
	go func() {
		// This should NOT deadlock
		c.SetRPCChannel(rpcCh)
		done <- true
	}()

	// Wait for completion or timeout
	select {
	case <-done:
		t.Log("✅ SetRPCChannel completed without deadlock")
	case <-time.After(deadlockTimeout):
		t.Error("❌ DEADLOCK DETECTED: SetRPCChannel did not complete within timeout")
	}
}

// TestSetRPCChannelConcurrent verifies concurrent SetRPCChannel calls don't cause issues
func TestSetRPCChannelConcurrent(t *testing.T) {
	workspace := t.TempDir()
	c, err := cluster.NewCluster(workspace)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}

	if err := c.Start(); err != nil {
		t.Fatalf("Failed to start cluster: %v", err)
	}
	defer c.Stop()

	mockBus := bus.NewMessageBus()

	cfg := &channels.RPCChannelConfig{
		MessageBus:      mockBus,
		RequestTimeout:  60 * time.Second,
		CleanupInterval: 30 * time.Second,
	}

	// Create multiple RPCChannels and call SetRPCChannel concurrently
	const numConcurrent = 5
	var wg sync.WaitGroup
	wg.Add(numConcurrent)

	for i := 0; i < numConcurrent; i++ {
		go func(idx int) {
			defer wg.Done()

			rpcCh, err := channels.NewRPCChannel(cfg)
			if err != nil {
				t.Errorf("Goroutine %d: failed to create RPC channel: %v", idx, err)
				return
			}

			ctx := context.Background()
			if err := rpcCh.Start(ctx); err != nil {
				t.Errorf("Goroutine %d: failed to start RPC channel: %v", idx, err)
				return
			}
			defer rpcCh.Stop(ctx)

			// This should be safe even when called concurrently
			c.SetRPCChannel(rpcCh)
		}(i)
	}

	// Wait for all goroutines to complete
	done := make(chan struct{})
	go func() {
		wg.Wait()
		close(done)
	}()

	select {
	case <-done:
		t.Log("✅ All concurrent SetRPCChannel calls completed successfully")
	case <-time.After(10 * time.Second):
		t.Error("❌ TIMEOUT: Concurrent SetRPCChannel calls did not complete")
	}
}

// TestSetRPCChannelBeforeServerStart verifies SetRPCChannel works when called before server starts
func TestSetRPCChannelBeforeServerStart(t *testing.T) {
	workspace := t.TempDir()
	c, err := cluster.NewCluster(workspace)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}
	defer c.Stop()

	mockBus := bus.NewMessageBus()

	cfg := &channels.RPCChannelConfig{
		MessageBus:      mockBus,
		RequestTimeout:  60 * time.Second,
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

	// Call SetRPCChannel BEFORE starting the server
	// This should not panic or deadlock
	c.SetRPCChannel(rpcCh)

	// Now start the server
	if err := c.Start(); err != nil {
		t.Fatalf("Failed to start cluster: %v", err)
	}

	t.Log("✅ SetRPCChannel before server start completed successfully")
}

// TestSetRPCChannelAfterStop verifies SetRPCChannel handles stopped cluster gracefully
func TestSetRPCChannelAfterStop(t *testing.T) {
	workspace := t.TempDir()
	c, err := cluster.NewCluster(workspace)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}

	// Start and then immediately stop
	if err := c.Start(); err != nil {
		t.Fatalf("Failed to start cluster: %v", err)
	}
	c.Stop()

	mockBus := bus.NewMessageBus()

	cfg := &channels.RPCChannelConfig{
		MessageBus:      mockBus,
		RequestTimeout:  60 * time.Second,
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

	// Call SetRPCChannel AFTER stopping the cluster
	// This should not panic or deadlock
	// (LLM handlers won't be registered because cluster is not running)
	c.SetRPCChannel(rpcCh)

	t.Log("✅ SetRPCChannel after cluster stop completed successfully")
}

// Verify Cluster implements the rpc.Cluster interface
var _ clusterrpc.Cluster = (*cluster.Cluster)(nil)
