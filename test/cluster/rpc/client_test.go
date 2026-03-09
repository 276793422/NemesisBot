// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package rpc_test

import (
	"context"
	"encoding/json"
	"sync"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/cluster/rpc"
)

func TestNewClient(t *testing.T) {
	cluster := &mockCluster{nodeID: "test-node"}
	client := createRPCClient(cluster)

	if client == nil {
		t.Fatal("NewClient() returned nil")
	}
}

func TestClientCallWithContext(t *testing.T) {
	// Setup test server
	serverCluster := &mockCluster{nodeID: "server-node"}
	testServer := rpc.NewServer(serverCluster)

	// Register test handler
	testServer.RegisterHandler("echo", func(payload map[string]interface{}) (map[string]interface{}, error) {
		return payload, nil
	})

	// Start server
	port := 21952
	err := testServer.Start(port)
	if err != nil {
		t.Fatalf("Failed to start server: %v", err)
	}
	defer testServer.Stop()

	// Setup mock peer
	mockPeer := &mockNode{
		id:        "peer-1",
		name:      "Test Peer",
		address:   "127.0.0.1",
		addresses: []string{"127.0.0.1"},
		rpcPort:   port,
		status:    "online",
		online:    true,
	}

	// Setup client cluster
	cluster := &mockCluster{
		nodeID: "client-node",
		peers: map[string]*mockNode{
			"peer-1": mockPeer,
		},
	}
	client := createRPCClient(cluster)

	// Test successful call
	payload := map[string]interface{}{
		"message": "hello world",
		"number":  42,
	}

	response, err := client.CallWithContext(context.Background(), "peer-1", "echo", payload)
	if err != nil {
		t.Fatalf("Call failed: %v", err)
	}

	// Verify response
	var respData map[string]interface{}
	if err := json.Unmarshal(response, &respData); err != nil {
		t.Fatalf("Failed to unmarshal response: %v", err)
	}

	if respData["message"] != "hello world" {
		t.Errorf("Expected message 'hello world', got %v", respData["message"])
	}

	if respData["number"].(float64) != 42 {
		t.Errorf("Expected number 42, got %v", respData["number"])
	}
}

func TestClientCallWithTimeout(t *testing.T) {
	// Setup test server with slow handler
	serverCluster := &mockCluster{nodeID: "server-node"}
	testServer := rpc.NewServer(serverCluster)

	// Register slow handler
	testServer.RegisterHandler("slow", func(payload map[string]interface{}) (map[string]interface{}, error) {
		time.Sleep(2 * time.Second)
		return map[string]interface{}{"status": "done"}, nil
	})

	// Start server
	port := 21953
	err := testServer.Start(port)
	if err != nil {
		t.Fatalf("Failed to start server: %v", err)
	}
	defer testServer.Stop()

	// Setup mock peer
	mockPeer := &mockNode{
		id:        "peer-1",
		name:      "Test Peer",
		address:   "127.0.0.1",
		addresses: []string{"127.0.0.1"},
		rpcPort:   port,
		status:    "online",
		online:    true,
	}

	// Setup client cluster
	cluster := &mockCluster{
		nodeID: "client-node",
		peers: map[string]*mockNode{
			"peer-1": mockPeer,
		},
	}
	client := createRPCClient(cluster)

	// Test timeout
	ctx, cancel := context.WithTimeout(context.Background(), 1*time.Second)
	defer cancel()

	_, err = client.CallWithContext(ctx, "peer-1", "slow", nil)
	if err == nil {
		t.Error("Expected timeout error, got none")
	}
}

func TestClientCallOfflinePeer(t *testing.T) {
	// Setup offline peer
	mockPeer := &mockNode{
		id:        "peer-1",
		name:      "Test Peer",
		address:   "127.0.0.1",
		addresses: []string{"127.0.0.1"},
		rpcPort:   21954,
		status:    "offline",
		online:    false,
	}

	// Setup client cluster
	cluster := &mockCluster{
		nodeID: "client-node",
		peers: map[string]*mockNode{
			"peer-1": mockPeer,
		},
	}
	client := createRPCClient(cluster)

	// Test call to offline peer
	_, err := client.CallWithContext(context.Background(), "peer-1", "test", nil)
	if err == nil {
		t.Error("Expected error for offline peer, got none")
	}
}

func TestClientCallRateLimited(t *testing.T) {
	// Setup test server
	serverCluster := &mockCluster{nodeID: "server-node"}
	testServer := rpc.NewServer(serverCluster)

	// Register a slow handler that takes time to complete
	// This prevents tokens from being released quickly
	testServer.RegisterHandler("slow", func(payload map[string]interface{}) (map[string]interface{}, error) {
		time.Sleep(200 * time.Millisecond) // Hold the connection
		return payload, nil
	})

	// Start server
	port := 21955
	err := testServer.Start(port)
	if err != nil {
		t.Fatalf("Failed to start server: %v", err)
	}
	defer testServer.Stop()

	// Setup mock peer
	mockPeer := &mockNode{
		id:        "peer-1",
		name:      "Test Peer",
		address:   "127.0.0.1",
		addresses: []string{"127.0.0.1"},
		rpcPort:   port,
		status:    "online",
		online:    true,
	}

	// Setup client cluster
	cluster := &mockCluster{
		nodeID: "client-node",
		peers: map[string]*mockNode{
			"peer-1": mockPeer,
		},
	}

	// Create client with default rate limiter (10 tokens per second)
	client := createRPCClient(cluster)

	// Make 11 concurrent calls to exhaust rate limiter
	// Since each call takes 200ms and rate limiter has 10 tokens,
	// the 11th concurrent call should be blocked by rate limit
	var wg sync.WaitGroup
	successCount := 0
	failCount := 0
	var mu sync.Mutex

	for i := 0; i < 11; i++ {
		wg.Add(1)
		go func(callNum int) {
			defer wg.Done()
			ctx, cancel := context.WithTimeout(context.Background(), 500*time.Millisecond)
			defer cancel()
			_, err := client.CallWithContext(ctx, "peer-1", "slow", map[string]interface{}{"msg": callNum})
			mu.Lock()
			if err != nil {
				failCount++
			} else {
				successCount++
			}
			mu.Unlock()
		}(i)
	}

	wg.Wait()

	// At least one call should fail due to rate limiting
	if failCount == 0 {
		t.Errorf("Expected at least one call to fail due to rate limit, but all %d succeeded", successCount)
	}

	// Verify success count is reasonable (should be at most 10)
	if successCount > 10 {
		t.Errorf("Expected at most 10 successful calls, got %d", successCount)
	}
}

func TestClientCallPeerNotFound(t *testing.T) {
	// Setup client cluster without peers
	cluster := &mockCluster{
		nodeID: "client-node",
		peers:  map[string]*mockNode{},
	}
	client := createRPCClient(cluster)

	// Test call to non-existent peer
	_, err := client.CallWithContext(context.Background(), "non-existent", "test", nil)
	if err == nil {
		t.Error("Expected error for non-existent peer, got none")
	}
}

func TestClientCallMultipleAddresses(t *testing.T) {
	// Setup test server on first address
	serverCluster := &mockCluster{nodeID: "server-node"}
	testServer := rpc.NewServer(serverCluster)

	testServer.RegisterHandler("echo", func(payload map[string]interface{}) (map[string]interface{}, error) {
		return payload, nil
	})

	// Start server on first address
	port1 := 21956
	err := testServer.Start(port1)
	if err != nil {
		t.Fatalf("Failed to start server: %v", err)
	}
	defer testServer.Stop()

	// Setup mock peer with multiple addresses
	mockPeer := &mockNode{
		id:        "peer-1",
		name:      "Test Peer",
		address:   "127.0.0.1:21957", // Wrong port
		addresses: []string{"127.0.0.1:21956", "127.0.0.1:21957"}, // First is correct
		rpcPort:   21956,
		status:    "online",
		online:    true,
	}

	// Setup client cluster
	cluster := &mockCluster{
		nodeID: "client-node",
		peers: map[string]*mockNode{
			"peer-1": mockPeer,
		},
	}
	client := createRPCClient(cluster)

	// Test call with multiple addresses (should pick the first working one)
	payload := map[string]interface{}{"message": "test"}
	response, err := client.CallWithContext(context.Background(), "peer-1", "echo", payload)
	if err != nil {
		t.Fatalf("Call failed: %v", err)
	}

	// Verify response
	var respData map[string]interface{}
	if err := json.Unmarshal(response, &respData); err != nil {
		t.Fatalf("Failed to unmarshal response: %v", err)
	}

	if respData["message"] != "test" {
		t.Errorf("Expected message 'test', got %v", respData["message"])
	}
}

func TestClientCallNoAddresses(t *testing.T) {
	// Setup mock peer with no addresses
	mockPeer := &mockNode{
		id:        "peer-1",
		name:      "Test Peer",
		address:   "",
		addresses: []string{},
		rpcPort:   0,
		status:    "online",
		online:    true,
	}

	// Setup client cluster
	cluster := &mockCluster{
		nodeID: "client-node",
		peers: map[string]*mockNode{
			"peer-1": mockPeer,
		},
	}
	client := createRPCClient(cluster)

	// Test call to peer with no addresses
	_, err := client.CallWithContext(context.Background(), "peer-1", "test", nil)
	if err == nil {
		t.Error("Expected error for peer with no addresses, got none")
	}
}

func TestClientClose(t *testing.T) {
	cluster := &mockCluster{nodeID: "test-node"}
	client := createRPCClient(cluster)

	// Close client
	err := client.Close()
	if err != nil {
		t.Errorf("Close() failed: %v", err)
	}
}

func TestClientConcurrentCalls(t *testing.T) {
	// Setup test server
	serverCluster := &mockCluster{nodeID: "server-node"}
	testServer := rpc.NewServer(serverCluster)

	// Register test handler
	testServer.RegisterHandler("echo", func(payload map[string]interface{}) (map[string]interface{}, error) {
		// Small delay to simulate processing
		time.Sleep(50 * time.Millisecond)
		return payload, nil
	})

	// Start server
	port := 21958
	err := testServer.Start(port)
	if err != nil {
		t.Fatalf("Failed to start server: %v", err)
	}
	defer testServer.Stop()

	// Setup mock peer
	mockPeer := &mockNode{
		id:        "peer-1",
		name:      "Test Peer",
		address:   "127.0.0.1",
		addresses: []string{"127.0.0.1"},
		rpcPort:   port,
		status:    "online",
		online:    true,
	}

	// Setup client cluster
	cluster := &mockCluster{
		nodeID: "client-node",
		peers: map[string]*mockNode{
			"peer-1": mockPeer,
		},
	}
	client := createRPCClient(cluster)

	// Test concurrent calls
	const numCalls = 5
	var wg sync.WaitGroup
	results := make(chan []byte, numCalls)
	errors := make(chan error, numCalls)

	for i := 0; i < numCalls; i++ {
		wg.Add(1)
		go func(id int) {
			defer wg.Done()
			payload := map[string]interface{}{"id": id, "message": "test"}
			response, err := client.CallWithContext(context.Background(), "peer-1", "echo", payload)
			if err != nil {
				errors <- err
			} else {
				results <- response
			}
		}(i)
	}

	wg.Wait()
	close(results)
	close(errors)

	// Check for errors
	for err := range errors {
		t.Errorf("Concurrent call error: %v", err)
	}

	// Check results
	count := 0
	for response := range results {
		var respData map[string]interface{}
		if err := json.Unmarshal(response, &respData); err != nil {
			t.Errorf("Failed to unmarshal response: %v", err)
			continue
		}
		count++
		if respData["message"] != "test" {
			t.Errorf("Expected message 'test', got %v", respData["message"])
		}
	}

	if count != numCalls {
		t.Errorf("Expected %d successful calls, got %d", numCalls, count)
	}
}